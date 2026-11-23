//! Guest-cooperative hibernate: the second method (after a host save) for a
//! VM that owns hardware QEMU cannot save, such as a passthrough GPU.
//!
//! When the host shuts down, the Horcrux guest agent inside the guest is
//! asked to hibernate: the guest writes its memory to its own disk and
//! powers the VM off, and the next host boot starts the VM, where the guest
//! resumes by itself. Guests without the tools (or with hibernation not
//! enabled) fall back to a clean shutdown, with the reason recorded.
//!
//! The node talks to the guest over the hypervisor's guest-agent channel
//! (`<disk>.qga.sock`), which works without a console viewer: it runs
//! `horcrux-guest-agent --hibernate-status` for the readiness check and
//! `--hibernate` for the request (crates/horcrux-guest-tools/src/power.rs).
//! When the channel allows no command execution but offers
//! `guest-suspend-disk`, that is used instead (its readiness cannot be
//! checked from outside, which the plan says).
use super::engine::GuestInfo;
use super::policy::Effective;
use crate::vm::devices::qga_exec;
use crate::AppState;
use horcrux_common::{Error, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Can this guest hibernate itself right now, and if not, why not?
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Probe {
    pub ready: bool,
    pub reason: Option<String>,
    /// How it will hibernate (`systemctl hibernate`, `shutdown /h`, ...).
    pub method: Option<String>,
    /// false: only the hypervisor's own `guest-suspend-disk` is available,
    /// the guest's swap/resume setup could not be checked.
    pub verified: bool,
}

impl Probe {
    fn no(reason: impl Into<String>) -> Self {
        Probe {
            ready: false,
            reason: Some(reason.into()),
            method: None,
            verified: true,
        }
    }
}

/// How long a readiness answer is reused for the views (the host stop pass
/// always asks again).
const CACHE_SECS: u64 = 30;

fn cache() -> &'static Mutex<HashMap<String, (Instant, Probe)>> {
    static C: OnceLock<Mutex<HashMap<String, (Instant, Probe)>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

// --------------------------------------------------------------- pure parts

/// Command line that runs the guest agent with one of its hibernate actions.
pub fn agent_command(windows: bool, eff: &Effective, flag: &str) -> (String, Vec<String>) {
    let path = if windows {
        &eff.guest_agent_path_windows
    } else {
        &eff.guest_agent_path_linux
    };
    (path.clone(), vec![flag.to_string()])
}

/// The JSON object the agent prints (its last line starting with `{`).
pub fn parse_agent_json(out: &str) -> Option<Value> {
    out.lines()
        .rev()
        .map(str::trim)
        .find(|l| l.starts_with('{'))
        .and_then(|l| serde_json::from_str(l).ok())
}

/// Turn `--hibernate-status` output into a [`Probe`].
pub fn interpret_status(v: &Value) -> Probe {
    let ready = v["ready"].as_bool() == Some(true);
    Probe {
        ready,
        reason: if ready {
            None
        } else {
            Some(
                v["reason"]
                    .as_str()
                    .unwrap_or("the guest reports it cannot hibernate")
                    .to_string(),
            )
        },
        method: v["method"].as_str().map(str::to_string),
        verified: true,
    }
}

/// Turn `--hibernate` output (exit code + JSON) into the method used.
pub fn interpret_request(exit: i64, v: Option<&Value>) -> std::result::Result<String, String> {
    match v {
        Some(v) if exit == 0 && v["ok"].as_bool() == Some(true) => Ok(v["method"]
            .as_str()
            .unwrap_or("guest hibernate")
            .to_string()),
        Some(v) => Err(v["reason"]
            .as_str()
            .unwrap_or("the guest refused to hibernate")
            .to_string()),
        None => Err(format!(
            "the guest agent answered nothing usable (exit {exit})"
        )),
    }
}

/// Did the channel refuse to run a command (as opposed to the guest failing)?
pub fn exec_disabled(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("disabled") || m.contains("not allowed") || m.contains("blocked")
}

// ------------------------------------------------------------ guest calls

fn decode(v: Option<&Value>) -> String {
    use base64::Engine as _;
    v.and_then(Value::as_str)
        .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .unwrap_or_default()
}

enum ExecError {
    /// The channel does not allow running commands.
    Disabled(String),
    Other(String),
}

/// Run `path args` in the guest and wait for its exit; returns exit code and
/// stdout.
async fn guest_run(
    sock: &Path,
    path: &str,
    args: &[String],
    secs: u64,
) -> std::result::Result<(i64, String), ExecError> {
    let r = qga_exec(
        sock,
        "guest-exec",
        json!({"path": path, "arg": args, "capture-output": true}),
        secs,
    )
    .await
    .map_err(|e| {
        let m = e.to_string();
        if exec_disabled(&m) {
            ExecError::Disabled(m)
        } else {
            ExecError::Other(m)
        }
    })?;
    let pid = r["pid"]
        .as_i64()
        .ok_or_else(|| ExecError::Other("guest-exec gave no pid".into()))?;
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let Ok(st) = qga_exec(sock, "guest-exec-status", json!({"pid": pid}), secs.min(10)).await
        else {
            continue;
        };
        if st["exited"].as_bool() == Some(true) {
            return Ok((
                st["exitcode"].as_i64().unwrap_or(-1),
                decode(st.get("out-data")),
            ));
        }
    }
    Err(ExecError::Other(format!(
        "the guest agent did not finish within {secs} s"
    )))
}

async fn guest_is_windows(sock: &Path, secs: u64) -> bool {
    qga_exec(sock, "guest-get-osinfo", json!({}), secs)
        .await
        .ok()
        .and_then(|v| v["id"].as_str().map(|s| s == "mswindows"))
        .unwrap_or(false)
}

/// Does the channel offer the hypervisor's own hibernate command?
async fn qga_suspend_disk(sock: &Path, secs: u64) -> bool {
    qga_exec(sock, "guest-info", json!({}), secs)
        .await
        .ok()
        .and_then(|v| {
            v["supported_commands"].as_array().map(|a| {
                a.iter().any(|c| {
                    c["name"] == "guest-suspend-disk" && c["enabled"].as_bool() == Some(true)
                })
            })
        })
        .unwrap_or(false)
}

/// Ask the guest whether it can hibernate itself.
pub async fn probe(state: &Arc<AppState>, g: &GuestInfo, eff: &Effective, fresh: bool) -> Probe {
    if !fresh {
        if let Some((t, p)) = cache().lock().unwrap().get(&g.id) {
            if t.elapsed() < Duration::from_secs(CACHE_SECS) {
                return p.clone();
            }
        }
    }
    // A hung guest must not hold up the page that lists the guests: the views
    // give the whole check one question's time, the stop pass twice that.
    let cap = Duration::from_secs(eff.hibernate_probe_secs * if fresh { 2 } else { 1 });
    let p = tokio::time::timeout(cap, probe_uncached(state, g, eff))
        .await
        .unwrap_or_else(|_| {
            Probe::no(format!(
                "the guest did not answer within {} s: its guest agent is not running, or the guest is hung",
                cap.as_secs()
            ))
        });
    cache()
        .lock()
        .unwrap()
        .insert(g.id.clone(), (Instant::now(), p.clone()));
    p
}

async fn probe_uncached(state: &Arc<AppState>, g: &GuestInfo, eff: &Effective) -> Probe {
    if !g.qemu {
        return Probe::no("guest hibernate is for QEMU VMs");
    }
    if g.status != "running" {
        return Probe::no("the VM is not running, so its guest cannot be asked");
    }
    let sock = match state.vm_manager.guest_agent_socket(&g.id).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return Probe::no(
                "the VM has no guest agent channel (enable \"QEMU guest agent\" in its hardware and restart it)",
            )
        }
        Err(e) => return Probe::no(e.to_string()),
    };
    let secs = eff.hibernate_probe_secs;
    let windows = guest_is_windows(&sock, secs).await;
    let (path, args) = agent_command(windows, eff, "--hibernate-status");
    match guest_run(&sock, &path, &args, secs).await {
        Ok((_, out)) => match parse_agent_json(&out) {
            Some(v) => interpret_status(&v),
            None => Probe::no(format!(
                "the Horcrux guest tools are not installed in the guest ({path} gave no answer)"
            )),
        },
        Err(ExecError::Disabled(_)) if qga_suspend_disk(&sock, secs).await => Probe {
            ready: true,
            reason: None,
            method: Some("guest agent guest-suspend-disk".into()),
            verified: false,
        },
        Err(ExecError::Disabled(m)) => Probe::no(format!(
            "the guest agent channel does not allow running commands ({m}) and offers no guest-suspend-disk"
        )),
        Err(ExecError::Other(m)) => Probe::no(format!(
            "the guest did not answer ({m}): the guest agent is not running, or the Horcrux guest tools are not installed"
        )),
    }
}

/// Hibernate the guest and wait for it to power the VM off. Returns the
/// method used. An error means the guest is (probably) still running and the
/// caller applies the fallback.
pub async fn hibernate(state: &Arc<AppState>, g: &GuestInfo, eff: &Effective) -> Result<String> {
    let p = probe(state, g, eff, true).await;
    if !p.ready {
        return Err(Error::Conflict(
            p.reason
                .unwrap_or_else(|| "the guest cannot hibernate".into()),
        ));
    }
    let sock = state
        .vm_manager
        .guest_agent_socket(&g.id)
        .await?
        .ok_or_else(|| Error::Conflict("the VM has no guest agent channel".into()))?;
    let secs = eff.hibernate_probe_secs;
    let method = if p.verified {
        let windows = guest_is_windows(&sock, secs).await;
        let (path, args) = agent_command(windows, eff, "--hibernate");
        let (exit, out) = guest_run(&sock, &path, &args, secs)
            .await
            .map_err(|e| match e {
                ExecError::Disabled(m) | ExecError::Other(m) => Error::System(m),
            })?;
        interpret_request(exit, parse_agent_json(&out).as_ref()).map_err(Error::Conflict)?
    } else {
        // `guest-suspend-disk` has no reply: the guest goes down first.
        let _ = qga_exec(&sock, "guest-suspend-disk", json!({}), 2).await;
        p.method.clone().unwrap_or_default()
    };
    let off = state
        .vm_manager
        .wait_guest_poweroff(&g.id, Duration::from_secs(eff.hibernate_timeout_secs))
        .await?;
    if off {
        cache().lock().unwrap().remove(&g.id);
        Ok(method)
    } else {
        Err(Error::System(format!(
            "the guest did not power off within {} s of starting to hibernate ({method})",
            eff.hibernate_timeout_secs
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::guest_state::policy::{combine, GuestOverride, NodePolicy};

    fn eff() -> Effective {
        combine(
            &NodePolicy::default(),
            &GuestOverride::default(),
            "/d".into(),
        )
    }

    #[test]
    fn agent_path_follows_the_guest_os_and_the_settings() {
        let mut e = eff();
        let (p, a) = agent_command(false, &e, "--hibernate");
        assert_eq!(
            (p.as_str(), a),
            (
                "/usr/bin/horcrux-guest-agent",
                vec!["--hibernate".to_string()]
            )
        );
        assert!(agent_command(true, &e, "--hibernate-status")
            .0
            .ends_with("horcrux-guest-agent.exe"));
        e.guest_agent_path_linux = "/opt/hx/agent".into();
        assert_eq!(agent_command(false, &e, "--hibernate").0, "/opt/hx/agent");
    }

    #[test]
    fn agent_output_is_read_from_the_last_json_line() {
        let out =
            "noise\n{\"ready\":false}\n  {\"ready\":true,\"method\":\"systemctl hibernate\"}\n";
        let v = parse_agent_json(out).unwrap();
        let p = interpret_status(&v);
        assert!(p.ready && p.verified);
        assert_eq!(p.method.as_deref(), Some("systemctl hibernate"));
        assert!(parse_agent_json("no json here").is_none());
    }

    #[test]
    fn not_ready_carries_the_guests_reason() {
        let v = json!({"ready": false, "reason": "swap is 0 MB but 2048 MB is needed"});
        let p = interpret_status(&v);
        assert!(!p.ready);
        assert_eq!(
            p.reason.as_deref(),
            Some("swap is 0 MB but 2048 MB is needed")
        );
        assert!(interpret_status(&json!({})).reason.is_some());
    }

    #[test]
    fn request_answers_are_judged_by_exit_code_and_ok() {
        assert_eq!(
            interpret_request(0, Some(&json!({"ok": true, "method": "shutdown /h"}))),
            Ok("shutdown /h".into())
        );
        assert_eq!(
            interpret_request(3, Some(&json!({"ok": false, "reason": "no swap"}))),
            Err("no swap".into())
        );
        // exit 0 but not ok, and no answer at all
        assert!(interpret_request(0, Some(&json!({"ok": false}))).is_err());
        assert!(interpret_request(127, None).unwrap_err().contains("127"));
    }

    #[test]
    fn a_blocked_command_is_told_from_a_failed_guest() {
        assert!(exec_disabled(
            "The command guest-exec has been disabled for this instance"
        ));
        assert!(!exec_disabled("the guest agent did not answer within 15 s"));
    }
}
