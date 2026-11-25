//! System log viewer (Proxmox "Node > System Log" level): journald when the
//! host runs it, otherwise the syslog file syslog-ng/metalog write
//! (`/var/log/messages`, `syslog`, `everything/current`).
//!
//! * `GET    /api/system/logs`  entries, newest first. Filters: `since`,
//!   `until` (absolute or relative), `priority` (0-7, minimum), `unit`,
//!   `identifier` (`*` wildcard), `kernel`, `boot`, `node`, plus the older
//!   `level`, `service`, `search`, `limit`
//! * `DELETE /api/system/logs` clear
//! * `GET    /api/system/logs/export` the same filters as txt/csv/json
//! * `GET    /api/system/logs/boots` the node's boots (reboot separators)
//! * `GET    /api/system/logs/units` units/services to pick from
//! * `GET    /api/system/logs/info` which source, what it cannot give
//! * `GET|PUT /api/system/logs/settings` typed limits (declarative `logs`)
//! * `POST   /api/system/logs/stream/ticket` + `GET .../stream/ws/:ticket`
//!   live mode over a WebSocket, same filters, bounded, closes when idle
//! * `/api/ha/peer/logs/*` the same reads for another cluster node
//!
//! `node=<name>` answers from another cluster node over the peer API.
mod core;
mod live;
#[cfg(test)]
mod tests;

pub use self::core::*;
use super::{rooted, run, user_of, which};
use crate::api::tasks;
use crate::db::config_store::ConfigStore;
use crate::error::ApiError;
use crate::middleware::auth::{AuthUser, CLUSTER_HA_ROLE};
use crate::AppState;
use axum::{
    extract::{Query, State},
    http::header,
    response::IntoResponse,
    routing::{get, post},
    Extension, Json, Router,
};
use horcrux_common::system_logs::LogSettings;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::AsyncBufReadExt;

const SETTINGS_KIND: &str = "system_log_settings";

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/system/logs", get(list).delete(clear))
        .route("/api/system/logs/export", get(export))
        .route("/api/system/logs/boots", get(boots))
        .route("/api/system/logs/units", get(units))
        .route("/api/system/logs/info", get(info))
        .route(
            "/api/system/logs/settings",
            get(get_settings).put(put_settings),
        )
        .merge(live::routes())
        .route("/api/ha/peer/logs/list", post(peer_list))
        .route("/api/ha/peer/logs/boots", post(peer_boots))
        .route("/api/ha/peer/logs/units", post(peer_units))
        .route("/api/ha/peer/logs/info", post(peer_info))
}

type AuthExt = Option<Extension<AuthUser>>;

// ------------------------------------------------------------- settings

pub async fn load_settings(state: &AppState) -> LogSettings {
    ConfigStore::new(&state.database)
        .load_values::<LogSettings>(SETTINGS_KIND)
        .await
        .ok()
        .and_then(|v| v.into_iter().next())
        .unwrap_or_default()
}

pub async fn set_settings(state: &AppState, s: LogSettings) -> Result<(), String> {
    s.validate()?;
    ConfigStore::new(&state.database)
        .put(SETTINGS_KIND, "default", &s)
        .await
        .map_err(|e| e.to_string())
}

async fn get_settings(State(state): State<Arc<AppState>>) -> Json<LogSettings> {
    Json(load_settings(&state).await)
}

async fn put_settings(
    State(state): State<Arc<AppState>>,
    u: AuthExt,
    Json(s): Json<LogSettings>,
) -> Result<Json<LogSettings>, ApiError> {
    set_settings(&state, s.clone())
        .await
        .map_err(ApiError::BadRequest)?;
    tasks::record_done(
        "log-settings",
        "system-logs",
        &user_of(&u),
        &Ok("system log settings saved".into()),
    );
    Ok(Json(s))
}

// -------------------------------------------------------------- sources

#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Journald,
    File(PathBuf),
}

fn journald_present() -> bool {
    which("journalctl").is_some()
        && (rooted("run/log/journal").is_dir() || rooted("var/log/journal").is_dir())
}

pub fn detect(s: &LogSettings) -> Option<Source> {
    if journald_present() {
        return Some(Source::Journald);
    }
    s.syslog_files
        .iter()
        .map(|p| rooted(p))
        .find(|p| p.is_file())
        .map(Source::File)
}

fn no_source(s: &LogSettings) -> ApiError {
    crate::api::unavailable::no_log_source(&format!(
        "journald is not running and none of {} exists.",
        s.syslog_files.join(", ")
    ))
}

pub fn now_us() -> i64 {
    chrono::Utc::now().timestamp_micros()
}

fn resolved(f: &Filters, s: &LogSettings) -> Result<Resolved, ApiError> {
    resolve(f, s.default_lines, s.max_lines, now_us(), local_offset()).map_err(ApiError::BadRequest)
}

fn tail_lines(path: &std::path::Path, max_bytes: u64) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return vec![];
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(max_bytes);
    let _ = f.seek(SeekFrom::Start(start));
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    let mut lines = text.lines();
    if start > 0 {
        lines.next(); // a cut-off first line
    }
    lines.map(|l| l.to_string()).collect()
}

/// Every parsed line of the file's tail, oldest first.
fn file_entries(file: &std::path::Path, s: &LogSettings) -> Vec<LogEntry> {
    let now = now_us();
    let tz = local_offset();
    tail_lines(file, s.syslog_tail_bytes)
        .iter()
        .filter_map(|l| parse_syslog_line_now(l, now, tz))
        .collect()
}

fn boot_start_of_machine() -> Option<i64> {
    let t = std::fs::read_to_string(rooted("proc/stat")).ok()?;
    t.lines()
        .find_map(|l| l.strip_prefix("btime "))
        .and_then(|v| v.trim().parse::<i64>().ok())
        .map(|s| s * 1_000_000)
}

/// Reboots of a host without journald: login records, kernel boot lines,
/// and the kernel's own boot time.
fn file_boots(entries: &[LogEntry], s: &LogSettings) -> Vec<BootInfo> {
    let mut starts: Vec<i64> = std::fs::read(rooted(&s.wtmp_path))
        .map(|b| {
            wtmp_boot_times(&b)
                .into_iter()
                .map(|x| x * 1_000_000)
                .collect()
        })
        .unwrap_or_default();
    starts.extend(
        entries
            .iter()
            .filter(|e| is_boot_line(e))
            .filter_map(|e| e.ts_us),
    );
    starts.extend(boot_start_of_machine());
    boots_from_starts(starts, entries.first().and_then(|e| e.ts_us), now_us())
}

/// Run journalctl and hand every stdout line to `on_line` until it says stop.
async fn journal_lines(
    args: Vec<String>,
    secs: u64,
    mut on_line: impl FnMut(&str) -> bool,
) -> Result<(), ApiError> {
    let bin = which("journalctl").ok_or_else(|| no_source(&LogSettings::default()))?;
    let mut child = tokio::process::Command::new(bin)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| ApiError::Internal(format!("journalctl: {e}")))?;
    let stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    let mut stopped_early = false;
    let work = async {
        while let Ok(Some(l)) = lines.next_line().await {
            if !on_line(&l) {
                stopped_early = true;
                break;
            }
        }
    };
    if tokio::time::timeout(std::time::Duration::from_secs(secs), work)
        .await
        .is_err()
    {
        let _ = child.kill().await;
        return Err(ApiError::Internal(format!(
            "journalctl did not answer within {secs}s"
        )));
    }
    if stopped_early {
        let _ = child.kill().await;
        return Ok(());
    }
    let status = child.wait().await.ok();
    if status.is_some_and(|s| !s.success()) {
        let mut err = String::new();
        let _ = tokio::io::AsyncReadExt::read_to_string(&mut stderr, &mut err).await;
        let err = err.trim();
        if !err.is_empty() {
            return Err(ApiError::BadRequest(format!("journalctl: {err}")));
        }
    }
    Ok(())
}

/// Newest-first entries of THIS node after every filter.
pub async fn read_local(f: &Filters, s: &LogSettings) -> Result<Vec<LogEntry>, ApiError> {
    let r = resolved(f, s)?;
    let source = detect(s).ok_or_else(|| no_source(s))?;
    let mut out: Vec<LogEntry> = match source {
        Source::Journald => {
            let args = journal_args(&r, s.max_scan_lines);
            let mut out = Vec::new();
            let mut scanned = 0usize;
            let limit = r.limit;
            let max_scan = s.max_scan_lines;
            journal_lines(args, s.query_timeout_secs, |l| {
                scanned += 1;
                if let Some(e) = parse_journal_json(l) {
                    if post_match(&e, &r) {
                        out.push(e);
                    }
                }
                out.len() < limit && scanned < max_scan
            })
            .await?;
            out
        }
        Source::File(file) => {
            let mut all = file_entries(&file, s);
            let boots = file_boots(&all, s);
            for e in &mut all {
                e.boot_id = e
                    .ts_us
                    .and_then(|t| boot_of(t, &boots))
                    .map(|b| b.boot_id.clone());
            }
            all.reverse();
            all.retain(|e| file_match(e, &r, &boots));
            all.truncate(r.limit);
            all
        }
    };
    let node = local_name_str();
    for e in &mut out {
        e.node = Some(node.clone());
    }
    Ok(out)
}

fn local_name_str() -> String {
    std::fs::read_to_string(rooted("etc/hostname"))
        .ok()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .or_else(|| {
            std::fs::read_to_string(rooted("proc/sys/kernel/hostname"))
                .ok()
                .map(|h| h.trim().to_string())
        })
        .unwrap_or_else(|| "localhost".into())
}

async fn boots_local(s: &LogSettings) -> Result<Vec<BootInfo>, ApiError> {
    match detect(s).ok_or_else(|| no_source(s))? {
        Source::Journald => {
            let json = run("journalctl", &["--list-boots", "--no-pager", "-o", "json"]).await;
            if let Some(b) = json.ok().and_then(|t| parse_list_boots_json(&t)) {
                return Ok(b);
            }
            let text = run("journalctl", &["--list-boots", "--no-pager"])
                .await
                .map_err(ApiError::Internal)?;
            Ok(parse_list_boots_text(&text))
        }
        Source::File(file) => Ok(file_boots(&file_entries(&file, s), s)),
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct UnitList {
    /// `systemd`, `openrc` or `identifiers`: what the names are.
    pub kind: String,
    pub units: Vec<String>,
}

async fn units_local(s: &LogSettings) -> Result<UnitList, ApiError> {
    match detect(s).ok_or_else(|| no_source(s))? {
        Source::Journald => {
            let out = run("journalctl", &["-F", "_SYSTEMD_UNIT", "--no-pager"])
                .await
                .map_err(ApiError::Internal)?;
            let mut units: Vec<String> = out
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect();
            units.sort();
            units.dedup();
            Ok(UnitList {
                kind: "systemd".into(),
                units,
            })
        }
        Source::File(file) => {
            let mut units: Vec<String> = std::fs::read_dir(rooted("etc/init.d"))
                .map(|d| {
                    d.flatten()
                        .filter(|e| e.path().is_file())
                        .filter_map(|e| e.file_name().into_string().ok())
                        .filter(|n| !n.starts_with('.') && !n.ends_with(".sh"))
                        .collect()
                })
                .unwrap_or_default();
            let kind = if units.is_empty() {
                "identifiers"
            } else {
                "openrc"
            };
            units.extend(
                file_entries(&file, s)
                    .into_iter()
                    .filter_map(|e| e.identifier),
            );
            units.sort();
            units.dedup();
            Ok(UnitList {
                kind: kind.into(),
                units,
            })
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct LogInfo {
    pub node: String,
    /// `journald` or `syslog-file`.
    pub source: String,
    pub file: Option<String>,
    /// What this source cannot give, shown in the UI.
    pub limitations: Vec<String>,
    pub max_lines: usize,
    pub default_lines: usize,
    pub follow_idle_secs: u64,
    pub follow_max_secs: u64,
}

async fn info_local(s: &LogSettings) -> Result<LogInfo, ApiError> {
    let source = detect(s).ok_or_else(|| no_source(s))?;
    let (name, file, limits) = match &source {
        Source::Journald => ("journald", None, vec![]),
        Source::File(p) => (
            "syslog-file",
            Some(p.display().to_string()),
            vec![
                "This node logs to a plain syslog file, not journald. The file has no priority \
                 field, so the priority is guessed from the message text (error, warning, debug \
                 words) unless the logger writes <PRI>; the minimum-priority filter works on \
                 that guess."
                    .to_string(),
                "The unit filter matches the program name written in each line (an OpenRC \
                 service name is usually the same as its program name)."
                    .to_string(),
                "Reboots are found from the login records (wtmp), kernel boot lines and the \
                 kernel's boot time; older boots have synthetic ids (boot-<time>)."
                    .to_string(),
                format!(
                    "Only the last {} MiB of the file are searched.",
                    s.syslog_tail_bytes / (1024 * 1024)
                ),
            ],
        ),
    };
    Ok(LogInfo {
        node: local_name_str(),
        source: name.into(),
        file,
        limitations: limits,
        max_lines: s.max_lines,
        default_lines: s.default_lines,
        follow_idle_secs: s.follow_idle_secs,
        follow_max_secs: s.follow_max_secs,
    })
}

// --------------------------------------------------------------- cluster

#[cfg(feature = "cluster")]
async fn call<B: Serialize, T: serde::de::DeserializeOwned>(
    state: &Arc<AppState>,
    node: &crate::cluster::node::Node,
    path: &str,
    body: &B,
) -> Result<T, String> {
    crate::api::drain::call_peer(state, node, path, body).await
}

#[cfg(not(feature = "cluster"))]
async fn call<B: Serialize, T: serde::de::DeserializeOwned>(
    _state: &Arc<AppState>,
    _node: &crate::cluster::node::Node,
    _path: &str,
    _body: &B,
) -> Result<T, String> {
    Err("clustering is not compiled into this build".into())
}

/// `Some(node)` when `name` is another node of the cluster.
async fn remote_node(
    state: &Arc<AppState>,
    name: Option<&str>,
) -> Result<Option<crate::cluster::node::Node>, ApiError> {
    let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) else {
        return Ok(None);
    };
    let local = state
        .cluster_manager
        .get_local_node_name()
        .await
        .unwrap_or_default();
    if name == local || name == local_name_str() {
        return Ok(None);
    }
    state
        .cluster_manager
        .list_nodes()
        .await
        .into_iter()
        .find(|n| n.name == name)
        .map(Some)
        .ok_or_else(|| ApiError::NotFound(format!("node {name} is not in this cluster")))
}

fn peer_error(node: &str, e: String) -> ApiError {
    if e.contains("no-log-source") {
        return crate::api::unavailable::no_log_source(&format!("(on node {node})"));
    }
    ApiError::Internal(format!("asking node {node}: {e}"))
}

/// Entries of the named node (this one when `node` is empty or local).
pub async fn read_node(state: &Arc<AppState>, f: &Filters) -> Result<Vec<LogEntry>, ApiError> {
    match remote_node(state, f.node.as_deref()).await? {
        None => {
            let mut v = read_local(f, &load_settings(state).await).await?;
            let me = state
                .cluster_manager
                .get_local_node_name()
                .await
                .unwrap_or_else(|_| local_name_str());
            for e in &mut v {
                e.node = Some(me.clone());
            }
            Ok(v)
        }
        Some(n) => {
            let body = Filters {
                node: None,
                ..f.clone()
            };
            let mut v: Vec<LogEntry> = call(state, &n, "/api/ha/peer/logs/list", &body)
                .await
                .map_err(|e| peer_error(&n.name, e))?;
            for e in &mut v {
                e.node = Some(n.name.clone());
            }
            Ok(v)
        }
    }
}

#[derive(Deserialize, Default)]
struct NodeQuery {
    node: Option<String>,
}

fn require_peer(user: &AuthExt) -> Result<(), ApiError> {
    match user {
        Some(Extension(u)) if u.role == CLUSTER_HA_ROLE => Ok(()),
        _ => Err(ApiError::Forbidden(
            "this route is only for other cluster nodes".into(),
        )),
    }
}

async fn peer_list(
    State(state): State<Arc<AppState>>,
    user: AuthExt,
    Json(f): Json<Filters>,
) -> Result<Json<Vec<LogEntry>>, ApiError> {
    require_peer(&user)?;
    Ok(Json(read_local(&f, &load_settings(&state).await).await?))
}

async fn peer_boots(
    State(state): State<Arc<AppState>>,
    user: AuthExt,
) -> Result<Json<Vec<BootInfo>>, ApiError> {
    require_peer(&user)?;
    Ok(Json(boots_local(&load_settings(&state).await).await?))
}

async fn peer_units(
    State(state): State<Arc<AppState>>,
    user: AuthExt,
) -> Result<Json<UnitList>, ApiError> {
    require_peer(&user)?;
    Ok(Json(units_local(&load_settings(&state).await).await?))
}

async fn peer_info(
    State(state): State<Arc<AppState>>,
    user: AuthExt,
) -> Result<Json<LogInfo>, ApiError> {
    require_peer(&user)?;
    Ok(Json(info_local(&load_settings(&state).await).await?))
}

// -------------------------------------------------------------- handlers

async fn list(
    State(state): State<Arc<AppState>>,
    Query(f): Query<Filters>,
) -> Result<Json<Vec<LogEntry>>, ApiError> {
    Ok(Json(read_node(&state, &f).await?))
}

async fn boots(
    State(state): State<Arc<AppState>>,
    Query(q): Query<NodeQuery>,
) -> Result<Json<Vec<BootInfo>>, ApiError> {
    match remote_node(&state, q.node.as_deref()).await? {
        None => Ok(Json(boots_local(&load_settings(&state).await).await?)),
        Some(n) => call(
            &state,
            &n,
            "/api/ha/peer/logs/boots",
            &serde_json::json!({}),
        )
        .await
        .map(Json)
        .map_err(|e| peer_error(&n.name, e)),
    }
}

async fn units(
    State(state): State<Arc<AppState>>,
    Query(q): Query<NodeQuery>,
) -> Result<Json<UnitList>, ApiError> {
    match remote_node(&state, q.node.as_deref()).await? {
        None => Ok(Json(units_local(&load_settings(&state).await).await?)),
        Some(n) => call(
            &state,
            &n,
            "/api/ha/peer/logs/units",
            &serde_json::json!({}),
        )
        .await
        .map(Json)
        .map_err(|e| peer_error(&n.name, e)),
    }
}

async fn info(
    State(state): State<Arc<AppState>>,
    Query(q): Query<NodeQuery>,
) -> Result<Json<LogInfo>, ApiError> {
    match remote_node(&state, q.node.as_deref()).await? {
        None => Ok(Json(info_local(&load_settings(&state).await).await?)),
        Some(n) => call(&state, &n, "/api/ha/peer/logs/info", &serde_json::json!({}))
            .await
            .map(Json)
            .map_err(|e| peer_error(&n.name, e)),
    }
}

async fn export(
    State(state): State<Arc<AppState>>,
    Query(f): Query<Filters>,
) -> Result<impl IntoResponse, ApiError> {
    let max = load_settings(&state).await.max_lines;
    let f = Filters {
        limit: Some(f.limit.unwrap_or(max)),
        ..f
    };
    let entries = read_node(&state, &f).await?;
    let (ctype, body) = match f.format.as_deref().unwrap_or("txt") {
        "json" => (
            "application/json",
            serde_json::to_string_pretty(&entries).unwrap_or_default(),
        ),
        "csv" => ("text/csv", render_csv(&entries)),
        _ => ("text/plain", render_text(&entries)),
    };
    Ok(([(header::CONTENT_TYPE, ctype)], body))
}

pub fn render_text(v: &[LogEntry]) -> String {
    v.iter()
        .map(|e| {
            format!(
                "{} [{}] {}: {}\n",
                e.timestamp,
                e.level,
                e.service.as_deref().unwrap_or("-"),
                e.message
            )
        })
        .collect()
}

fn csv_field(s: &str) -> String {
    // Quote, and defuse spreadsheet formulas in untrusted log text.
    let s = if s.starts_with(['=', '+', '-', '@']) {
        format!("'{s}")
    } else {
        s.to_string()
    };
    format!("\"{}\"", s.replace('"', "\"\""))
}

pub fn render_csv(v: &[LogEntry]) -> String {
    let mut out = String::from(
        "timestamp,level,service,message,priority,unit,identifier,pid,boot_id,hostname,node\n",
    );
    for e in v {
        out += &format!(
            "{},{},{},{},{},{},{},{},{},{},{}\n",
            csv_field(&e.timestamp),
            csv_field(&e.level),
            csv_field(e.service.as_deref().unwrap_or("")),
            csv_field(&e.message),
            csv_field(&e.priority.map(|p| p.to_string()).unwrap_or_default()),
            csv_field(e.unit.as_deref().unwrap_or("")),
            csv_field(e.identifier.as_deref().unwrap_or("")),
            csv_field(&e.pid.map(|p| p.to_string()).unwrap_or_default()),
            csv_field(e.boot_id.as_deref().unwrap_or("")),
            csv_field(e.hostname.as_deref().unwrap_or("")),
            csv_field(e.node.as_deref().unwrap_or("")),
        );
    }
    out
}

async fn clear(
    State(state): State<Arc<AppState>>,
    u: AuthExt,
) -> Result<Json<serde_json::Value>, ApiError> {
    let who = user_of(&u);
    let s = load_settings(&state).await;
    let r: Result<String, String> = match detect(&s) {
        Some(Source::Journald) => match run("journalctl", &["--rotate"]).await {
            Ok(_) => run("journalctl", &["--vacuum-time=1s"])
                .await
                .map(|o| format!("journald vacuumed: {}", o.trim())),
            Err(e) => Err(e),
        },
        Some(Source::File(f)) => std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&f)
            .map(|_| format!("{} truncated", f.display()))
            .map_err(|e| format!("{}: {e}", f.display())),
        None => Err("no system log source".into()),
    };
    tasks::record_done("log-clear", "system-logs", &who, &r);
    r.map_err(ApiError::Internal)?;
    Ok(Json(serde_json::json!({"cleared": true})))
}
