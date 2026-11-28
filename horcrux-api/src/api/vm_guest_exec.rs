//! Run a command inside a VM through the QEMU guest agent (Proxmox
//! `qm guest exec`), for host jobs and automation that must not log in to
//! the guest.
//!
//! - `POST /api/vms/:id/guest-exec`
//!   `{path, args?, env?, stdin?, stdin_base64?, timeout_secs?, encoding?}`
//!   waits for the command and answers `{exit_code, signal, stdout, stderr,
//!   timed_out, ...}`; the HTTP status is 200 whatever the command's own
//!   exit code is (a non-zero exit is the command's answer, not an API
//!   failure).
//!
//! Access: the privilege of a VM console (`VM.Console` on `/vms/<id>`):
//! running a program as the guest agent's user is console-equivalent. Every
//! call is audited (`GuestExec`: who, which VM, program, argument count,
//! stdin size, exit code, duration); argument values are logged only when
//! `[guest_exec] log_arguments = true`, because they may hold secrets.
//!
//! Limits (`[guest_exec]` in config.toml): `enabled`, `default_timeout_secs`,
//! `max_timeout_secs`, `max_input_bytes`, `max_output_bytes`,
//! `poll_interval_ms`, `log_arguments`. The guest's own agent policy still
//! applies: a guest that disables `guest-exec` answers 424.
use crate::audit::{AuditEvent, AuditEventType, AuditResult, AuditSeverity};
use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::AppState;
use axum::{
    extract::{Path, State},
    routing::post,
    Extension, Json, Router,
};
use base64::Engine as _;
use horcrux_common::VmStatus;
use horcrux_core::config::GuestExecConfig;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/api/vms/:id/guest-exec", post(guest_exec))
}

#[derive(Debug, Deserialize)]
pub struct ExecRequest {
    /// Program to run (an absolute path or a name the guest agent resolves).
    pub path: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// `NAME=value` entries added to the command's environment.
    #[serde(default)]
    pub env: Vec<String>,
    /// Standard input as text.
    #[serde(default)]
    pub stdin: Option<String>,
    /// Standard input as base64 (binary safe); wins over `stdin`.
    #[serde(default)]
    pub stdin_base64: Option<String>,
    /// How long to wait for the command, in seconds (default and maximum
    /// from `[guest_exec]`).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// `utf8` (default, invalid bytes replaced) or `base64` for stdout and
    /// stderr.
    #[serde(default)]
    pub encoding: Option<String>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct ExecResponse {
    pub vm_id: String,
    pub pid: i64,
    pub exited: bool,
    /// The command's exit status (absent when it was killed by a signal or
    /// did not finish).
    pub exit_code: Option<i64>,
    pub signal: Option<i64>,
    /// The command did not finish within `timeout_secs`; it may still be
    /// running in the guest (`pid`).
    pub timed_out: bool,
    pub encoding: String,
    pub stdout: String,
    pub stderr: String,
    /// The guest agent or the node cut the output (`max_output_bytes`).
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub duration_ms: u64,
}

/// A request that passed validation, ready for the agent.
#[derive(Debug, PartialEq)]
pub struct Prepared {
    pub qga_args: Value,
    pub timeout_secs: u64,
    pub base64_output: bool,
    pub stdin_len: usize,
}

fn bad(m: impl Into<String>) -> ApiError {
    ApiError::ValidationError(m.into())
}

/// Validate a request against the node's limits and build the
/// `guest-exec` arguments.
pub fn prepare(req: &ExecRequest, cfg: &GuestExecConfig) -> Result<Prepared, ApiError> {
    if req.path.trim().is_empty() || req.path.contains('\0') {
        return Err(bad("path: a program to run is required"));
    }
    if req.args.iter().any(|a| a.contains('\0')) {
        return Err(bad("args: must not contain NUL bytes"));
    }
    for e in &req.env {
        match e.split_once('=') {
            Some((k, _)) if !k.is_empty() && !e.contains('\0') => {}
            _ => return Err(bad(format!("env: {e:?} is not NAME=value"))),
        }
    }
    let timeout_secs = req.timeout_secs.unwrap_or(cfg.default_timeout_secs);
    if timeout_secs == 0 || timeout_secs > cfg.max_timeout_secs {
        return Err(bad(format!(
            "timeout_secs: must be 1..={} on this node",
            cfg.max_timeout_secs
        )));
    }
    let base64_output = match req.encoding.as_deref().unwrap_or("utf8") {
        "utf8" => false,
        "base64" => true,
        other => return Err(bad(format!("encoding: {other:?} is not utf8 or base64"))),
    };
    let b64 = base64::engine::general_purpose::STANDARD;
    let stdin: Option<Vec<u8>> = match (&req.stdin_base64, &req.stdin) {
        (Some(s), _) => Some(
            b64.decode(s.trim())
                .map_err(|e| bad(format!("stdin_base64: {e}")))?,
        ),
        (None, Some(s)) => Some(s.clone().into_bytes()),
        (None, None) => None,
    };
    let stdin_len = stdin.as_ref().map_or(0, Vec::len);
    if stdin_len > cfg.max_input_bytes {
        return Err(bad(format!(
            "stdin: {stdin_len} bytes is over this node's limit of {}",
            cfg.max_input_bytes
        )));
    }
    let mut qga = json!({"path": req.path, "arg": req.args, "capture-output": true});
    if !req.env.is_empty() {
        qga["env"] = json!(req.env);
    }
    if let Some(s) = stdin {
        qga["input-data"] = json!(b64.encode(s));
    }
    Ok(Prepared {
        qga_args: qga,
        timeout_secs,
        base64_output,
        stdin_len,
    })
}

/// Decode an agent output field, cut it to `max` bytes and render it.
pub fn render_output(
    field: Option<&Value>,
    max: usize,
    agent_truncated: bool,
    base64_output: bool,
) -> (String, bool) {
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut bytes = field
        .and_then(Value::as_str)
        .and_then(|s| b64.decode(s).ok())
        .unwrap_or_default();
    let cut = bytes.len() > max;
    bytes.truncate(max);
    let text = if base64_output {
        b64.encode(&bytes)
    } else {
        String::from_utf8_lossy(&bytes).into_owned()
    };
    (text, cut || agent_truncated)
}

fn flag(v: &Value, k: &str) -> bool {
    v.get(k).and_then(Value::as_bool).unwrap_or(false)
}

/// Turn the final `guest-exec-status` reply into a response.
pub fn response_from_status(
    vm_id: &str,
    pid: i64,
    st: &Value,
    timed_out: bool,
    p: &Prepared,
    max_output: usize,
    started: Instant,
) -> ExecResponse {
    let (stdout, o_cut) = render_output(
        st.get("out-data"),
        max_output,
        flag(st, "out-truncated"),
        p.base64_output,
    );
    let (stderr, e_cut) = render_output(
        st.get("err-data"),
        max_output,
        flag(st, "err-truncated"),
        p.base64_output,
    );
    let exited = flag(st, "exited");
    ExecResponse {
        vm_id: vm_id.to_string(),
        pid,
        exited,
        exit_code: if exited {
            st.get("exitcode").and_then(Value::as_i64)
        } else {
            None
        },
        signal: if exited {
            st.get("signal").and_then(Value::as_i64)
        } else {
            None
        },
        timed_out,
        encoding: if p.base64_output { "base64" } else { "utf8" }.into(),
        stdout,
        stderr,
        stdout_truncated: o_cut,
        stderr_truncated: e_cut,
        duration_ms: started.elapsed().as_millis() as u64,
    }
}

/// Map a guest agent failure to the API's errors.
pub fn map_agent_error(e: horcrux_common::Error) -> ApiError {
    let m = e.to_string();
    if crate::api::guest_state::hibernate::exec_disabled(&m) {
        return ApiError::NotAvailable {
            reason: "guest-exec-disabled",
            message: "this guest's agent does not allow running commands".into(),
            hint: "allow guest-exec in the guest agent's configuration (qemu-ga \
                   --allow-rpcs / BLACKLIST_RPC) or run the command another way"
                .into(),
        };
    }
    ApiError::from(e)
}

async fn guest_exec(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    user: Option<Extension<AuthUser>>,
    Json(req): Json<ExecRequest>,
) -> Result<Json<ExecResponse>, ApiError> {
    let cfg = state.config.guest_exec.clone();
    let who = user.map(|Extension(u)| u.username);
    if !cfg.enabled {
        return Err(ApiError::NotAvailable {
            reason: "guest-exec-off",
            message: "running commands in guests through the API is switched off on this node"
                .into(),
            hint: "set `enabled = true` under [guest_exec] in config.toml".into(),
        });
    }
    let started = Instant::now();
    let out = run(&state, &id, &req, &cfg, started).await;
    audit(&state, who, &id, &req, &cfg, &out).await;
    out.map(Json)
}

async fn run(
    state: &Arc<AppState>,
    id: &str,
    req: &ExecRequest,
    cfg: &GuestExecConfig,
    started: Instant,
) -> Result<ExecResponse, ApiError> {
    let prepared = prepare(req, cfg)?;
    let vm = state.vm_manager.get_vm(id).await?;
    let disk = crate::console::runtime_disk(&vm)?;
    let sock = crate::vm::display::DisplayRuntime::qga_socket(&disk);
    if !sock.exists() && vm.status != VmStatus::Running {
        return Err(ApiError::Conflict(format!("VM {id} is not running")));
    }
    let timeout = prepared.timeout_secs;
    let call = |cmd: &'static str, args: Value, secs: u64| {
        let sock = sock.clone();
        async move {
            crate::vm::devices::qga_exec(&sock, cmd, args, secs)
                .await
                .map_err(map_agent_error)
        }
    };
    let r = call("guest-exec", prepared.qga_args.clone(), timeout.min(30)).await?;
    let pid = r["pid"]
        .as_i64()
        .ok_or_else(|| ApiError::Internal("guest agent gave no pid".into()))?;
    let deadline = started + Duration::from_secs(timeout);
    let poll = Duration::from_millis(cfg.poll_interval_ms.max(10));
    let mut last = json!({});
    loop {
        match call("guest-exec-status", json!({"pid": pid}), 10).await {
            Ok(st) if flag(&st, "exited") => {
                return Ok(response_from_status(
                    id,
                    pid,
                    &st,
                    false,
                    &prepared,
                    cfg.max_output_bytes,
                    started,
                ));
            }
            Ok(st) => last = st,
            // an agent that stalls for a moment is retried until the deadline
            Err(ApiError::NotAvailable { .. }) => {}
            Err(_) => {}
        }
        if Instant::now() >= deadline {
            return Ok(response_from_status(
                id,
                pid,
                &last,
                true,
                &prepared,
                cfg.max_output_bytes,
                started,
            ));
        }
        tokio::time::sleep(poll).await;
    }
}

async fn audit(
    state: &Arc<AppState>,
    who: Option<String>,
    id: &str,
    req: &ExecRequest,
    cfg: &GuestExecConfig,
    out: &Result<ExecResponse, ApiError>,
) {
    let stdin_len = req
        .stdin_base64
        .as_ref()
        .map(String::len)
        .or_else(|| req.stdin.as_ref().map(String::len))
        .unwrap_or(0);
    let mut detail = format!(
        "program={} args={} stdin_bytes~{}",
        req.path,
        req.args.len(),
        stdin_len
    );
    if cfg.log_arguments {
        detail.push_str(&format!(" argv={:?}", req.args));
    }
    let (ok, outcome) = match out {
        Ok(r) if r.timed_out => (false, "timed out".to_string()),
        Ok(r) => (
            r.exit_code == Some(0),
            format!(
                "exit={} signal={} ms={}",
                r.exit_code.map_or("-".into(), |c| c.to_string()),
                r.signal.map_or("-".into(), |c| c.to_string()),
                r.duration_ms
            ),
        ),
        Err(e) => (false, format!("refused: {e:?}")),
    };
    state
        .audit_logger
        .log(AuditEvent {
            timestamp: chrono::Utc::now(),
            event_type: AuditEventType::GuestExec,
            severity: if ok {
                AuditSeverity::Info
            } else {
                AuditSeverity::Warning
            },
            user: who,
            source_ip: None,
            resource: Some(id.to_string()),
            action: "guest command".into(),
            result: if out.is_ok() {
                AuditResult::Success
            } else {
                AuditResult::Failure
            },
            details: Some(format!("{detail} {outcome}")),
            session_id: None,
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(path: &str) -> ExecRequest {
        ExecRequest {
            path: path.into(),
            args: vec![],
            env: vec![],
            stdin: None,
            stdin_base64: None,
            timeout_secs: None,
            encoding: None,
        }
    }

    fn b64(s: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(s)
    }

    #[test]
    fn prepare_builds_agent_arguments_and_applies_defaults() {
        let cfg = GuestExecConfig::default();
        let mut r = req("/bin/sh");
        r.args = vec!["-c".into(), "cat".into()];
        r.env = vec!["A=1".into()];
        r.stdin = Some("hi".into());
        let p = prepare(&r, &cfg).unwrap();
        assert_eq!(p.timeout_secs, cfg.default_timeout_secs);
        assert_eq!(p.stdin_len, 2);
        assert_eq!(p.qga_args["path"], "/bin/sh");
        assert_eq!(p.qga_args["arg"], json!(["-c", "cat"]));
        assert_eq!(p.qga_args["env"], json!(["A=1"]));
        assert_eq!(p.qga_args["input-data"], b64("hi"));
        assert_eq!(p.qga_args["capture-output"], true);
        assert!(!p.base64_output);
    }

    #[test]
    fn binary_stdin_wins_and_limits_are_enforced() {
        let cfg = GuestExecConfig {
            max_timeout_secs: 100,
            max_input_bytes: 4,
            ..GuestExecConfig::default()
        };
        let mut r = req("x");
        r.stdin = Some("text".into());
        r.stdin_base64 = Some(b64("ab"));
        assert_eq!(prepare(&r, &cfg).unwrap().qga_args["input-data"], b64("ab"));
        r.stdin_base64 = Some(b64("abcde"));
        assert!(format!("{:?}", prepare(&r, &cfg).unwrap_err()).contains("limit of 4"));
        r.stdin_base64 = Some("!!not base64".into());
        assert!(prepare(&r, &cfg).is_err());
        let mut r = req("x");
        r.timeout_secs = Some(101);
        assert!(prepare(&r, &cfg).is_err());
        r.timeout_secs = Some(0);
        assert!(prepare(&r, &cfg).is_err());
    }

    #[test]
    fn bad_requests_are_refused() {
        let cfg = GuestExecConfig::default();
        assert!(prepare(&req("  "), &cfg).is_err());
        assert!(prepare(&req("a\0b"), &cfg).is_err());
        let mut r = req("x");
        r.env = vec!["NOEQUALS".into()];
        assert!(prepare(&r, &cfg).is_err());
        r.env = vec!["=v".into()];
        assert!(prepare(&r, &cfg).is_err());
        let mut r = req("x");
        r.encoding = Some("latin1".into());
        assert!(prepare(&r, &cfg).is_err());
        let mut r = req("x");
        r.args = vec!["a\0".into()];
        assert!(prepare(&r, &cfg).is_err());
    }

    #[test]
    fn output_is_cut_and_encoded() {
        let v = json!(b64("hello world"));
        let (t, cut) = render_output(Some(&v), 5, false, false);
        assert_eq!((t.as_str(), cut), ("hello", true));
        let (t, cut) = render_output(Some(&v), 100, true, false);
        assert_eq!((t.as_str(), cut), ("hello world", true));
        let (t, cut) = render_output(Some(&json!(b64("\u{1}z"))), 100, false, true);
        assert_eq!((t.as_str(), cut), (b64("\u{1}z").as_str(), false));
        let (t, _) = render_output(None, 10, false, false);
        assert_eq!(t, "");
        // invalid UTF-8 is replaced, not an error
        let raw = base64::engine::general_purpose::STANDARD.encode([0xffu8, b'a']);
        let (t, _) = render_output(Some(&json!(raw)), 10, false, false);
        assert_eq!(t, "\u{fffd}a");
    }

    #[test]
    fn status_maps_exit_signal_and_timeout() {
        let p = prepare(&req("x"), &GuestExecConfig::default()).unwrap();
        let st = json!({"exited": true, "exitcode": 3, "out-data": b64("o"), "err-data": b64("e")});
        let r = response_from_status("7", 42, &st, false, &p, 100, Instant::now());
        assert_eq!(
            (
                r.exit_code,
                r.signal,
                r.timed_out,
                r.stdout.as_str(),
                r.stderr.as_str()
            ),
            (Some(3), None, false, "o", "e")
        );
        let st = json!({"exited": true, "signal": 9});
        let r = response_from_status("7", 42, &st, false, &p, 100, Instant::now());
        assert_eq!((r.exit_code, r.signal), (None, Some(9)));
        let r = response_from_status(
            "7",
            42,
            &json!({"exited": false}),
            true,
            &p,
            100,
            Instant::now(),
        );
        assert!(r.timed_out && !r.exited && r.exit_code.is_none());
    }

    #[test]
    fn disabled_agent_is_a_424_not_a_500() {
        let e = horcrux_common::Error::System(
            "The command guest-exec has been disabled for this instance".into(),
        );
        assert!(matches!(
            map_agent_error(e),
            ApiError::NotAvailable {
                reason: "guest-exec-disabled",
                ..
            }
        ));
    }
}
