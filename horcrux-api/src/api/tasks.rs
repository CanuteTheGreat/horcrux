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
    let mut rec: TaskRecord =
        serde_json::from_slice(&std::fs::read(record_path(upid)).ok()?).ok()?;
    // A task still marked running that this process isn't running died with
    // the previous API process.
    if rec.status == "running" && !registry().running.lock().unwrap().contains_key(upid) {
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
        registry().running.lock().unwrap().remove(&u);
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
}
