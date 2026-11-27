//! Task log (Proxmox "Node > Tasks"): long-running host operations (package
//! installs, `emerge --sync`, service changes, ...) run as tasks that outlive
//! the HTTP request. Each task has a UPID, a status, and a log file that is
//! readable while it runs and after the API restarts; a running task can be
//! stopped.
//!
//! * `GET    /api/tasks?limit=&status=&type=`  newest first
//! * `GET    /api/tasks/:upid`
//! * `GET    /api/tasks/:upid/log?start=N`     log lines from line N
//! * `DELETE /api/tasks/:upid`                 stop a running task (SIGTERM to
//!   its process group, SIGKILL after 5 s)
//!
//! Records are JSON files under `<data dir>/tasks/`, so no table or migration
//! is involved; the 500 newest are kept.
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::{Path, Query},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

const KEEP: usize = 500;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/tasks", get(list))
        .route("/api/tasks/:upid", get(get_task).delete(stop_task))
        .route("/api/tasks/:upid/log", get(task_log))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskRecord {
    pub upid: String,
    pub node: String,
    pub task_type: String,
    #[serde(default)]
    pub target: Option<String>,
    pub user: String,
    /// running | ok | error | stopped | interrupted
    pub status: String,
    #[serde(default)]
    pub exit_code: Option<i32>,
    pub start_time: i64,
    #[serde(default)]
    pub end_time: Option<i64>,
}

struct Registry {
    dir: PathBuf,
    /// UPID -> process group id of a running task.
    running: Mutex<HashMap<String, u32>>,
    stopped: Mutex<std::collections::HashSet<String>>,
    /// UPIDs of guest operations running inside an API request (see
    /// `record_guest_actions`): live, but there is no process to stop.
    inline: Mutex<std::collections::HashSet<String>>,
}

static REGISTRY: OnceLock<Registry> = OnceLock::new();

fn registry() -> &'static Registry {
    REGISTRY.get_or_init(|| {
        let base = std::env::var("HORCRUX_DATA_DIR").unwrap_or_else(|_| "/var/lib/horcrux".into());
        let dir = PathBuf::from(base).join("tasks");
        let _ = std::fs::create_dir_all(&dir);
        Registry {
            dir,
            running: Mutex::new(HashMap::new()),
            stopped: Mutex::new(Default::default()),
            inline: Mutex::new(Default::default()),
        }
    })
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn clean(s: &str) -> String {
    let c: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@') {
                c
            } else {
                '_'
            }
        })
        .take(48)
        .collect();
    if c.is_empty() {
        "-".into()
    } else {
        c
    }
}

fn valid_upid(u: &str) -> bool {
    u.starts_with("UPID:")
        && u.len() < 200
        && u.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '.' | '_' | '-' | '@'))
}

fn record_path(upid: &str) -> PathBuf {
    registry().dir.join(format!("{upid}.json"))
}

fn log_path(upid: &str) -> PathBuf {
    registry().dir.join(format!("{upid}.log"))
}

fn save(rec: &TaskRecord) {
    if let Ok(j) = serde_json::to_vec(rec) {
        let tmp = record_path(&format!("{}.tmp", rec.upid));
        if std::fs::write(&tmp, j).is_ok() {
            let _ = std::fs::rename(&tmp, record_path(&rec.upid));
        }
    }
}

fn load(upid: &str) -> Option<TaskRecord> {
    // Snapshot the registry BEFORE reading the record: a task inserts itself
    // before its first save and leaves only after its final save, so "absent
    // first, record still running after" really is a dead task. Checking the
    // registry after the read called a task that finished in between
    // "interrupted" (CI flake: system-admin e2e "update runs emerge --sync").
    let in_registry = registry().running.lock().unwrap().contains_key(upid)
        || registry().inline.lock().unwrap().contains(upid);
    let mut rec: TaskRecord =
        serde_json::from_slice(&std::fs::read(record_path(upid)).ok()?).ok()?;
    // A task still marked running that this process isn't running died with
    // the previous API process.
    if rec.status == "running" && !in_registry {
        rec.status = "interrupted".into();
    }
    Some(rec)
}

fn new_upid(task_type: &str, target: &str, user: &str) -> String {
    let node = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "node".into());
    let n = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
    format!(
        "UPID:{}:{:012X}:{}:{}:{}:",
        clean(&node),
        n,
        clean(task_type),
        clean(target),
        clean(user)
    )
}

fn prune() {
    let mut all: Vec<TaskRecord> = list_all();
    if all.len() <= KEEP {
        return;
    }
    all.sort_by_key(|t| t.start_time);
    for t in all.iter().take(all.len() - KEEP) {
        if t.status != "running" {
            let _ = std::fs::remove_file(record_path(&t.upid));
            let _ = std::fs::remove_file(log_path(&t.upid));
        }
    }
}

fn list_all() -> Vec<TaskRecord> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(&registry().dir)
        .into_iter()
        .flatten()
        .flatten()
    {
        let name = e.file_name().to_string_lossy().to_string();
        if let Some(upid) = name.strip_suffix(".json") {
            if let Some(r) = load(upid) {
                out.push(r);
            }
        }
    }
    out
}

/// Run `program args` as a task. Returns the UPID at once; the command's
/// stdout and stderr stream into the task log.
pub fn spawn_command(
    task_type: &str,
    target: &str,
    user: &str,
    program: &str,
    args: &[String],
    env: &[(&str, &str)],
) -> Result<String, ApiError> {
    let upid = new_upid(task_type, target, user);
    let log = std::fs::File::create(log_path(&upid))
        .map_err(|e| ApiError::Internal(format!("task log: {e}")))?;
    let log2 = log
        .try_clone()
        .map_err(|e| ApiError::Internal(format!("task log: {e}")))?;
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(log)
        .stderr(log2)
        .process_group(0)
        .kill_on_drop(false);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| ApiError::ServiceUnavailable(format!("cannot run {program}: {e}")))?;
    let node = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut rec = TaskRecord {
        upid: upid.clone(),
        node,
        task_type: task_type.into(),
        target: Some(target.into()),
        user: user.into(),
        status: "running".into(),
        exit_code: None,
        start_time: now(),
        end_time: None,
    };
    if let Some(pid) = child.id() {
        registry().running.lock().unwrap().insert(upid.clone(), pid);
    }
    save(&rec);
    prune();
    let u = upid.clone();
    tokio::spawn(async move {
        let status = child.wait().await;
        let was_stopped = registry().stopped.lock().unwrap().remove(&u);
        rec.end_time = Some(now());
        match status {
            Ok(s) => {
                rec.exit_code = s.code();
                rec.status = if was_stopped {
                    "stopped"
                } else if s.success() {
                    "ok"
                } else {
                    "error"
                }
                .into();
            }
            Err(e) => {
                rec.status = "error".into();
                append_log(&u, &format!("wait failed: {e}"));
            }
        }
        append_log(&u, &format!("TASK {}", rec.status.to_uppercase()));
        save(&rec);
        // Leave the registry only AFTER the final record is on disk: load()
        // calls a "running" record that is not in the registry "interrupted",
        // so removing first made a poller read an ended task as interrupted
        // (CI flake: system-admin e2e "update runs emerge --sync", run 7664).
        registry().running.lock().unwrap().remove(&u);
    });
    Ok(upid)
}

/// Record an operation that already finished (config changes, service
/// actions done inline) so it shows in the task log.
pub fn record_done(task_type: &str, target: &str, user: &str, result: &Result<String, String>) {
    let upid = new_upid(task_type, target, user);
    let (status, text) = match result {
        Ok(t) => ("ok", t.clone()),
        Err(e) => ("error", e.clone()),
    };
    let _ = std::fs::write(
        log_path(&upid),
        format!("{text}\nTASK {}\n", status.to_uppercase()),
    );
    save(&TaskRecord {
        upid,
        node: hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_default(),
        task_type: task_type.into(),
        target: Some(target.into()),
        user: user.into(),
        status: status.into(),
        exit_code: Some(if status == "ok" { 0 } else { 1 }),
        start_time: now(),
        end_time: Some(now()),
    });
    prune();
}

fn append_log(upid: &str, line: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .append(true)
        .open(log_path(upid))
    {
        let _ = writeln!(f, "{line}");
    }
}

/// True while a task of this type is running (used to run one emerge at a time).
pub fn running_of_type(task_type: &str) -> Option<String> {
    list_all()
        .into_iter()
        .find(|t| t.task_type == task_type && t.status == "running")
        .map(|t| t.upid)
}

pub fn any_running(types: &[&str]) -> Option<String> {
    list_all()
        .into_iter()
        .find(|t| t.status == "running" && types.contains(&t.task_type.as_str()))
        .map(|t| t.upid)
}

#[derive(Deserialize)]
struct ListQuery {
    limit: Option<usize>,
    status: Option<String>,
    #[serde(rename = "type")]
    task_type: Option<String>,
}

async fn list(Query(q): Query<ListQuery>) -> Json<Vec<TaskRecord>> {
    let mut all = list_all();
    if let Some(s) = q.status.filter(|s| !s.is_empty()) {
        all.retain(|t| t.status == s);
    }
    if let Some(t) = q.task_type.filter(|s| !s.is_empty()) {
        all.retain(|x| x.task_type == t);
    }
    all.sort_by(|a, b| b.start_time.cmp(&a.start_time).then(b.upid.cmp(&a.upid)));
    all.truncate(q.limit.unwrap_or(100).min(KEEP));
    Json(all)
}

fn find(upid: &str) -> Result<TaskRecord, ApiError> {
    if !valid_upid(upid) {
        return Err(ApiError::BadRequest("malformed UPID".into()));
    }
    load(upid).ok_or_else(|| ApiError::NotFound(format!("task {upid} not found")))
}

async fn get_task(Path(upid): Path<String>) -> Result<Json<TaskRecord>, ApiError> {
    Ok(Json(find(&upid)?))
}

#[derive(Deserialize)]
struct LogQuery {
    start: Option<usize>,
}

#[derive(Serialize)]
struct LogView {
    lines: Vec<String>,
    /// Total lines in the log now; pass as `start` to get only newer lines.
    total: usize,
    running: bool,
}

async fn task_log(
    Path(upid): Path<String>,
    Query(q): Query<LogQuery>,
) -> Result<Json<LogView>, ApiError> {
    let rec = find(&upid)?;
    let text = std::fs::read(log_path(&upid)).unwrap_or_default();
    let text = String::from_utf8_lossy(&text);
    let all: Vec<&str> = text.lines().collect();
    let start = q.start.unwrap_or(0).min(all.len());
    // Never ship more than 5000 lines in one response.
    let lines = all[start..]
        .iter()
        .take(5000)
        .map(|s| s.to_string())
        .collect();
    Ok(Json(LogView {
        lines,
        total: all.len(),
        running: rec.status == "running",
    }))
}

async fn stop_task(Path(upid): Path<String>) -> Result<Json<TaskRecord>, ApiError> {
    let rec = find(&upid)?;
    if rec.status != "running" {
        return Err(ApiError::Conflict(format!(
            "task is {}, not running",
            rec.status
        )));
    }
    let pid = registry().running.lock().unwrap().get(&upid).copied();
    let Some(pid) = pid else {
        if registry().inline.lock().unwrap().contains(&upid) {
            return Err(ApiError::Conflict(
                "this guest operation cannot be stopped from the task log; it ends on its own \
                 (use Stop on the guest to force a stuck shutdown)"
                    .into(),
            ));
        }
        return Err(ApiError::Conflict("task is not running".into()));
    };
    registry().stopped.lock().unwrap().insert(upid.clone());
    let pgid = format!("-{pid}");
    let _ = tokio::process::Command::new("kill")
        .args(["-TERM", "--", &pgid])
        .status()
        .await;
    let u = upid.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        if registry().running.lock().unwrap().contains_key(&u) {
            let _ = tokio::process::Command::new("kill")
                .args(["-KILL", "--", &pgid])
                .status()
                .await;
        }
    });
    // Give the waiter a moment so the response usually shows the end state.
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if !registry().running.lock().unwrap().contains_key(&upid) {
            break;
        }
    }
    Ok(Json(find(&upid)?))
}

// ---------------------------------------------------------------------------
// Guest operations in the task log
// ---------------------------------------------------------------------------
//
// Proxmox records every guest operation (qmstart, qmshutdown, qmclone, vzdump,
// ...) in the task log, so "who did what to which guest, when, and did it
// work" has one answer. Horcrux's guest handlers run inline in the request,
// so instead of touching each handler, one middleware (innermost, after
// authentication and the permission check) records the operations listed in
// `guest_action` as tasks: "running" while the request runs, then ok/error
// with the HTTP status and the error text in the task's log.

/// The task type and target of a guest operation, or None for any other
/// request (reads, config edits, consoles, ...).
pub fn guest_action(method: &axum::http::Method, path: &str) -> Option<(&'static str, String)> {
    use axum::http::Method;
    let segs: Vec<&str> = path
        .trim_start_matches("/api/")
        .trim_end_matches('/')
        .split('/')
        .collect();
    let post = method == Method::POST;
    let del = method == Method::DELETE;
    let t = |kind: &'static str, target: &str| Some((kind, target.to_string()));
    match segs.as_slice() {
        // Virtual machines
        ["vms"] if post => t("vm-create", "new"),
        ["vms", "gaming"] if post => t("vm-create", "new"),
        ["vms", "import", kind] if post && matches!(*kind, "ovf" | "esxi" | "libvirt") => {
            t("vm-import", kind)
        }
        ["vms", "import", ..] => None,
        ["vms", id] if del => t("vm-destroy", id),
        ["vms", id, action] if post => match *action {
            "start" => t("vm-start", id),
            "stop" => t("vm-stop", id),
            "shutdown" => t("vm-shutdown", id),
            "reboot" => t("vm-reboot", id),
            "reset" => t("vm-reset", id),
            "pause" => t("vm-pause", id),
            "resume" => t("vm-resume", id),
            "hibernate" => t("vm-hibernate", id),
            "save-state" => t("vm-save-state", id),
            "resume-state" => t("vm-resume-state", id),
            "clone" | "clone-cross-node" => t("vm-clone", id),
            "snapshots" => t("vm-snapshot", id),
            "import-disk" => t("vm-import-disk", id),
            _ => None,
        },
        ["vms", id, "snapshots", _snap] if del => t("vm-snapshot-delete", id),
        ["vms", id, "snapshots", _snap, "restore"] if post => t("vm-rollback", id),
        ["migrate", id] if post => t("vm-migrate", id),
        // Containers
        ["containers"] if post => t("ct-create", "new"),
        ["containers", id] if del => t("ct-destroy", id),
        ["containers", id, action] if post => match *action {
            "start" => t("ct-start", id),
            "stop" => t("ct-stop", id),
            "shutdown" => t("ct-shutdown", id),
            "reboot" => t("ct-reboot", id),
            "pause" => t("ct-pause", id),
            "resume" => t("ct-resume", id),
            "save-state" => t("ct-save-state", id),
            "resume-state" => t("ct-resume-state", id),
            "clone" => t("ct-clone", id),
            "resize" => t("ct-resize", id),
            "snapshots" => t("ct-snapshot", id),
            _ => None,
        },
        ["containers", id, "snapshots", _snap] if del => t("ct-snapshot-delete", id),
        ["containers", id, "snapshots", _snap, "rollback"] if post => t("ct-rollback", id),
        // Backups
        ["backups"] if post => t("backup", "new"),
        ["backups", "prune"] if post => t("backup-prune", "all"),
        ["backups", id] if del => t("backup-delete", id),
        ["backups", id, "restore"] if post => t("backup-restore", id),
        ["backups", id, "verify"] if post => t("backup-verify", id),
        ["backup-jobs", id, "run"] if post => t("backup-job", id),
        _ => None,
    }
}

/// A guest operation running inside a request. Dropped without `finish`
/// (the client went away and the request was cancelled) it ends as an error.
struct InlineTask {
    rec: Option<TaskRecord>,
}

impl InlineTask {
    fn begin(task_type: &str, target: &str, user: &str, what: &str) -> Self {
        let upid = new_upid(task_type, target, user);
        let _ = std::fs::write(log_path(&upid), format!("{what}\n"));
        let rec = TaskRecord {
            upid: upid.clone(),
            node: hostname::get()
                .map(|h| h.to_string_lossy().to_string())
                .unwrap_or_default(),
            task_type: task_type.into(),
            target: Some(target.into()),
            user: user.into(),
            status: "running".into(),
            exit_code: None,
            start_time: now(),
            end_time: None,
        };
        registry().inline.lock().unwrap().insert(upid);
        save(&rec);
        prune();
        InlineTask { rec: Some(rec) }
    }

    fn finish(mut self, ok: bool, target: Option<String>, line: &str) {
        if let Some(rec) = self.rec.take() {
            end_inline(rec, ok, target, line);
        }
    }
}

impl Drop for InlineTask {
    fn drop(&mut self) {
        if let Some(rec) = self.rec.take() {
            end_inline(
                rec,
                false,
                None,
                "request cancelled before it finished (client disconnected)",
            );
        }
    }
}

fn end_inline(mut rec: TaskRecord, ok: bool, target: Option<String>, line: &str) {
    if let Some(t) = target {
        rec.target = Some(t);
    }
    rec.status = if ok { "ok" } else { "error" }.into();
    rec.exit_code = Some(if ok { 0 } else { 1 });
    rec.end_time = Some(now());
    if !line.is_empty() {
        append_log(&rec.upid, line);
    }
    append_log(&rec.upid, &format!("TASK {}", rec.status.to_uppercase()));
    save(&rec);
    registry().inline.lock().unwrap().remove(&rec.upid);
}

/// Bodies read back for the task log: API errors and create responses are
/// small JSON; anything without an exact size this small is left untouched.
const DETAIL_MAX: u64 = 256 * 1024;

/// The readable part of an API answer: `message`/`error` of a JSON error,
/// else the text itself, shortened.
fn detail_text(body: &[u8]) -> String {
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) {
        for k in ["message", "error", "detail", "reason"] {
            if let Some(m) = v.get(k).and_then(|m| m.as_str()) {
                return m.chars().take(2000).collect();
            }
        }
    }
    String::from_utf8_lossy(body).chars().take(2000).collect()
}

/// The id (and name) of what a create call made, from its JSON answer.
fn created_target(body: &[u8]) -> Option<(String, Option<String>)> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let v = v.get("vm").or_else(|| v.get("container")).unwrap_or(&v);
    let id = v.get("id").and_then(|i| {
        i.as_str()
            .map(str::to_string)
            .or_else(|| i.as_u64().map(|n| n.to_string()))
    })?;
    let name = v.get("name").and_then(|n| n.as_str()).map(str::to_string);
    Some((id, name))
}

/// Middleware: record guest operations (`guest_action`) as tasks.
pub async fn record_guest_actions(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::body::{Body, HttpBody};
    let Some((kind, target)) = guest_action(request.method(), request.uri().path()) else {
        return next.run(request).await;
    };
    let user = request
        .extensions()
        .get::<crate::middleware::auth::AuthUser>()
        .map(|u| u.username.clone())
        .unwrap_or_else(|| "unknown".into());
    let what = format!("{} {}", request.method(), request.uri().path());
    let task = InlineTask::begin(kind, &target, &user, &what);
    let resp = next.run(request).await;
    let status = resp.status();
    let ok = status.is_success();
    let wants_body = !ok || target == "new";
    let small = resp
        .body()
        .size_hint()
        .exact()
        .is_some_and(|n| n <= DETAIL_MAX);
    if !(wants_body && small) {
        let line = if ok {
            format!("HTTP {}", status.as_u16())
        } else {
            format!("failed: HTTP {}", status.as_u16())
        };
        task.finish(ok, None, &line);
        return resp;
    }
    let (parts, body) = resp.into_parts();
    let bytes = match axum::body::to_bytes(body, DETAIL_MAX as usize).await {
        Ok(b) => b,
        Err(e) => {
            task.finish(
                ok,
                None,
                &format!("HTTP {} (answer unreadable: {e})", status.as_u16()),
            );
            return axum::response::Response::from_parts(parts, Body::empty());
        }
    };
    if ok {
        match created_target(&bytes) {
            Some((id, name)) => {
                let line = match &name {
                    Some(n) => format!("created {n} (id {id})"),
                    None => format!("created id {id}"),
                };
                task.finish(true, Some(id), &line);
            }
            None => task.finish(true, None, &format!("HTTP {}", status.as_u16())),
        }
    } else {
        task.finish(
            false,
            None,
            &format!("failed: HTTP {}: {}", status.as_u16(), detail_text(&bytes)),
        );
    }
    axum::response::Response::from_parts(parts, Body::from(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upid_validation_blocks_path_tricks() {
        assert!(valid_upid(
            "UPID:node:0000000000AB:pkg-install:app-editors_vim:root_pam:"
        ));
        assert!(!valid_upid("UPID:../../etc/passwd"));
        assert!(!valid_upid("../x"));
        assert!(!valid_upid("UPID:a/b"));
        assert!(!valid_upid("UPID:a b"));
    }

    #[test]
    fn clean_makes_a_target_filename_safe() {
        assert_eq!(clean("app-editors/vim"), "app-editors_vim");
        assert_eq!(clean(""), "-");
        assert_eq!(clean("a b;c"), "a_b_c");
    }

    #[tokio::test]
    async fn a_task_streams_its_log_finishes_and_can_be_stopped() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("HORCRUX_DATA_DIR", dir.path());
        let upid = spawn_command(
            "unit-test",
            "echo",
            "tester",
            "sh",
            &["-c".into(), "echo hello; echo oops >&2; exit 3".into()],
            &[],
        )
        .unwrap();
        for _ in 0..50 {
            if find(&upid).unwrap().status != "running" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let r = find(&upid).unwrap();
        assert_eq!(r.status, "error");
        assert_eq!(r.exit_code, Some(3));
        let log = std::fs::read_to_string(log_path(&upid)).unwrap();
        assert!(log.contains("hello") && log.contains("oops") && log.contains("TASK ERROR"));

        // Stopping needs kill(2) on our own child; skip just that half where
        // the sandbox denies signals (the log/exit-code half above ran).
        if !crate::test_env::can_signal_own_children() {
            return;
        }
        let long = spawn_command(
            "unit-test",
            "sleep",
            "tester",
            "sh",
            &["-c".into(), "sleep 60".into()],
            &[],
        )
        .unwrap();
        assert_eq!(find(&long).unwrap().status, "running");
        assert!(running_of_type("unit-test").is_some());
        let stopped = stop_task(Path(long.clone())).await.unwrap().0;
        assert_eq!(stopped.status, "stopped");
        assert!(
            stop_task(Path(long)).await.is_err(),
            "stopping a finished task is a conflict"
        );
    }

    #[test]
    fn guest_operations_are_tasks_reads_and_edits_are_not() {
        use axum::http::Method;
        let g = |m: Method, p: &str| guest_action(&m, p).map(|(k, t)| (k.to_string(), t));
        let some = |k: &str, t: &str| Some((k.to_string(), t.to_string()));
        assert_eq!(
            g(Method::POST, "/api/vms/101/start"),
            some("vm-start", "101")
        );
        assert_eq!(
            g(Method::POST, "/api/vms/101/shutdown"),
            some("vm-shutdown", "101")
        );
        assert_eq!(g(Method::DELETE, "/api/vms/101"), some("vm-destroy", "101"));
        assert_eq!(g(Method::POST, "/api/vms"), some("vm-create", "new"));
        assert_eq!(
            g(Method::POST, "/api/vms/import/ovf"),
            some("vm-import", "ovf")
        );
        assert_eq!(g(Method::POST, "/api/vms/import/ovf/preview"), None);
        assert_eq!(
            g(Method::POST, "/api/vms/101/snapshots/s1/restore"),
            some("vm-rollback", "101")
        );
        assert_eq!(g(Method::POST, "/api/containers/c1/rollback"), None);
        assert_eq!(
            g(Method::POST, "/api/containers/c1/snapshots/s/rollback"),
            some("ct-rollback", "c1")
        );
        assert_eq!(
            g(Method::POST, "/api/backup-jobs/j1/run"),
            some("backup-job", "j1")
        );
        assert_eq!(
            g(Method::POST, "/api/backups/prune"),
            some("backup-prune", "all")
        );
        assert_eq!(
            g(Method::POST, "/api/migrate/101"),
            some("vm-migrate", "101")
        );
        assert_eq!(g(Method::GET, "/api/vms/101"), None);
        assert_eq!(g(Method::GET, "/api/vms/101/start"), None);
        assert_eq!(g(Method::PUT, "/api/vms/101/config"), None);
        assert_eq!(g(Method::POST, "/api/vms/101/agent/ping"), None);
        assert_eq!(g(Method::POST, "/api/tasks"), None);
    }

    #[test]
    fn error_and_create_answers_are_readable_in_the_log() {
        assert_eq!(
            detail_text(br#"{"error":"VM is already running"}"#),
            "VM is already running"
        );
        assert_eq!(detail_text(b"plain failure"), "plain failure");
        assert_eq!(
            created_target(br#"{"id":"104","name":"web"}"#),
            Some(("104".into(), Some("web".into())))
        );
        assert_eq!(
            created_target(br#"{"vm":{"id":7}}"#),
            Some(("7".into(), None))
        );
        assert_eq!(created_target(b"[]"), None);
    }

    #[tokio::test]
    async fn the_middleware_records_a_guest_operation_and_its_failure() {
        use axum::{routing::post, Router};
        use tower::ServiceExt;
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("HORCRUX_DATA_DIR", dir.path());
        let app = Router::new()
            .route("/api/vms/:id/start", post(|| async { "started" }))
            .route(
                "/api/vms/:id/stop",
                post(|| async {
                    (
                        axum::http::StatusCode::CONFLICT,
                        axum::Json(serde_json::json!({"error": "VM is not running"})),
                    )
                }),
            )
            .route("/api/vms/:id/config", axum::routing::put(|| async { "ok" }))
            .layer(axum::middleware::from_fn(record_guest_actions));
        let call = |m: &str, p: &str| {
            axum::http::Request::builder()
                .method(m)
                .uri(p)
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let r = app
            .clone()
            .oneshot(call("POST", "/api/vms/unit-vm-9/start"))
            .await
            .unwrap();
        assert!(r.status().is_success());
        let r = app
            .clone()
            .oneshot(call("POST", "/api/vms/unit-vm-9/stop"))
            .await
            .unwrap();
        assert_eq!(r.status(), axum::http::StatusCode::CONFLICT);
        let body = axum::body::to_bytes(r.into_body(), 4096).await.unwrap();
        assert!(
            String::from_utf8_lossy(&body).contains("VM is not running"),
            "the error body still reaches the client"
        );
        app.oneshot(call("PUT", "/api/vms/unit-vm-9/config"))
            .await
            .unwrap();
        let mine: Vec<TaskRecord> = list_all()
            .into_iter()
            .filter(|t| t.target.as_deref() == Some("unit-vm-9"))
            .collect();
        assert_eq!(
            mine.len(),
            2,
            "start + stop recorded, config edit not: {mine:?}"
        );
        let start = mine.iter().find(|t| t.task_type == "vm-start").unwrap();
        assert_eq!(start.status, "ok");
        let stop = mine.iter().find(|t| t.task_type == "vm-stop").unwrap();
        assert_eq!(stop.status, "error");
        let log = std::fs::read_to_string(log_path(&stop.upid)).unwrap();
        assert!(
            log.contains("HTTP 409") && log.contains("VM is not running"),
            "{log}"
        );
        assert!(stop_task(Path(stop.upid.clone())).await.is_err());
    }
}
