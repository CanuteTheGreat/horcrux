//! Runtime side of host intrusion detection and file integrity (pure logic
//! in `ids.rs`):
//!
//! * `GET  /api/security/ids`                       audit + integrity state
//! * `GET  /api/security/events?source&kind&severity&user&q&since&until&limit&offset`
//! * `POST /api/security/audit/collect`             take in new audit events now
//! * `POST /api/security/integrity/init|check|accept`  run AIDE (202; one at a time)
//!
//! Turning `audit.enabled` on or off (API or declarative `security:`) writes
//! or removes the rule file and loads it before the settings are stored, so
//! a node that cannot do it refuses the change with the reason.
use super::super::system_admin::{data_dir, rooted, run_secs, which};
use super::ids::{self, EventQuery, EventStore, IdsState};
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use horcrux_common::security::{
    AuditSettings, IdsStatus, IntegritySettings, SecurityEvent, SecuritySettings, Severity,
};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/security/ids", get(status))
        .route("/api/security/events", get(events))
        .route("/api/security/audit/collect", post(collect_now))
        .route("/api/security/integrity/init", post(integrity_init))
        .route("/api/security/integrity/check", post(integrity_check))
        .route("/api/security/integrity/accept", post(integrity_accept))
}

fn dd() -> std::path::PathBuf {
    data_dir()
}

/// Serialises state-file updates between the collector and AIDE runs.
fn state_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

fn update_state(f: impl FnOnce(&mut IdsState)) {
    let _g = state_lock().lock().unwrap_or_else(|e| e.into_inner());
    let mut s = ids::load_state(&dd());
    f(&mut s);
    if let Err(e) = ids::save_state(&dd(), &s) {
        tracing::warn!("security ids state: {e}");
    }
}

/// Lower a child's CPU (nice) and IO (idle class) priority before exec.
fn low_priority(cmd: &mut tokio::process::Command, nice: u8) {
    let nice = nice as i32;
    // SAFETY: only async-signal-safe libc calls between fork and exec.
    unsafe {
        cmd.pre_exec(move || {
            nix::libc::setpriority(nix::libc::PRIO_PROCESS, 0, nice);
            nix::libc::syscall(
                nix::libc::SYS_ioprio_set,
                1 as nix::libc::c_long,
                0 as nix::libc::c_long,
                (3 << 13) as nix::libc::c_long,
            );
            Ok(())
        });
    }
}

/// Run with a timeout; Ok((exit code, stdout, stderr)).
async fn run_low(
    bin: &std::path::Path,
    args: &[String],
    nice: u8,
    secs: u64,
) -> Result<(i32, String, String), String> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    low_priority(&mut cmd, nice);
    let out = tokio::time::timeout(Duration::from_secs(secs), cmd.output())
        .await
        .map_err(|_| {
            format!(
                "{} took longer than {secs} s and was stopped",
                bin.display()
            )
        })?
        .map_err(|e| format!("{}: {e}", bin.display()))?;
    Ok((
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    ))
}

// ------------------------------------------------------------ audit

fn audit_missing() -> ApiError {
    ApiError::NotAvailable {
        reason: "auditd-missing",
        message: "The Linux audit tools (auditd, auditctl, ausearch) are not installed on this node.".into(),
        hint: "Install sys-process/audit (Gentoo; USE=ids on app-emulation/horcrux pulls it in) or the distribution's auditd package, then turn audit on in Security > Settings.".into(),
    }
}

pub fn audit_installed() -> bool {
    which("auditctl").is_some() && which("ausearch").is_some()
}

/// Write/remove the rule file and (re)load the kernel rules for a settings
/// change. Called before the new settings are stored.
pub async fn apply_audit(old: &AuditSettings, new: &AuditSettings) -> Result<(), ApiError> {
    if !new.enabled && !old.enabled {
        return Ok(());
    }
    if !audit_installed() {
        if new.enabled {
            return Err(audit_missing());
        }
        // Turning off on a node that lost the tools: just drop the file.
        let _ = std::fs::remove_file(rooted(&old.rules_file));
        return Ok(());
    }
    let status = run_secs("auditctl", &["-s"], 20).await.unwrap_or_default();
    if ids::is_locked(&status) {
        return Err(ApiError::Conflict(
            "The kernel audit rules are locked until the next reboot (audit.immutable / -e 2). The change is applied at boot from the rule file; reboot the node to change them now.".into(),
        ));
    }
    // Drop our previous rules (by key only: other tools' rules stay).
    for k in horcrux_common::security::AUDIT_KINDS {
        let key = format!("hx-{k}");
        let _ = run_secs("auditctl", &["-D", "-k", &key], 20).await;
    }
    if old.enabled && old.rules_file != new.rules_file {
        let _ = std::fs::remove_file(rooted(&old.rules_file));
    }
    let file = rooted(&new.rules_file);
    if !new.enabled {
        let _ = std::fs::remove_file(&file);
        return Ok(());
    }
    let text = ids::render_rules(new, &|p| rooted(p).exists());
    if let Some(d) = file.parent() {
        std::fs::create_dir_all(d)
            .map_err(|e| ApiError::Internal(format!("{}: {e}", d.display())))?;
    }
    std::fs::write(&file, &text)
        .map_err(|e| ApiError::Internal(format!("{}: {e}", file.display())))?;
    let path = file.display().to_string();
    if let Err(e) = run_secs("auditctl", &["-R", &path], 60).await {
        // Leave nothing half-loaded behind.
        for k in horcrux_common::security::AUDIT_KINDS {
            let _ = run_secs("auditctl", &["-D", "-k", &format!("hx-{k}")], 20).await;
        }
        let _ = std::fs::remove_file(&file);
        return Err(ApiError::BadRequest(format!(
            "loading the audit rules failed: {e}"
        )));
    }
    if new.manage_service {
        for action in ["enable", "start"] {
            if let Err(e) =
                super::super::system_admin::services::service_action("auditd", action).await
            {
                tracing::warn!("auditd {action}: {e}");
            }
        }
    }
    if !old.enabled {
        // Take in only what happens from now on.
        let now = chrono::Utc::now().timestamp();
        update_state(|s| {
            s.audit_watermark = (now, 0, 0);
            s.audit_error = None;
        });
        let _ = std::fs::remove_file(checkpoint());
    }
    Ok(())
}

fn checkpoint() -> std::path::PathBuf {
    dd().join("security").join("ausearch.checkpoint")
}

/// Take in new audit events. Returns the events added.
pub async fn collect(state: &AppState, s: &SecuritySettings) -> Result<Vec<SecurityEvent>, String> {
    let bin = which("ausearch").ok_or("ausearch is not installed")?;
    let cp = checkpoint();
    if let Some(d) = cp.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let args: Vec<String> = [
        "--checkpoint",
        &cp.display().to_string(),
        "--start",
        "checkpoint",
        "--format",
        "raw",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let (mut code, mut out, mut err) = run_low(&bin, &args, 10, 300).await?;
    // 10-12: the checkpoint no longer matches the logs (rotated): start over;
    // the watermark keeps already-seen events out.
    if (10..=12).contains(&code) {
        let _ = std::fs::remove_file(&cp);
        (code, out, err) = run_low(&bin, &args, 10, 300).await?;
    }
    // 1 = no matches.
    if code != 0 && code != 1 {
        return Err(format!("ausearch exited with {code}: {}", err.trim()));
    }
    let wm = ids::load_state(&dd()).audit_watermark;
    let mut evs = ids::after_watermark(ids::parse_events(&out, &ids::passwd_names()), wm);
    evs.sort_by_key(ids::event_key);
    let now = chrono::Utc::now().timestamp();
    let new_wm = evs.last().and_then(ids::event_key);
    EventStore::new(&dd()).append(&evs, s.events.keep as usize)?;
    update_state(|st| {
        if let Some(w) = new_wm {
            st.audit_watermark = w;
        }
        st.audit_last_collect = Some(now);
        st.audit_error = None;
    });
    let notable: Vec<&SecurityEvent> = evs
        .iter()
        .filter(|e| s.audit.notify_kinds.contains(&e.kind))
        .collect();
    if !notable.is_empty() {
        let high = notable
            .iter()
            .any(|e| e.severity.is_some_and(|x| x >= Severity::High));
        let mut body: Vec<String> = notable
            .iter()
            .take(5)
            .map(|e| format!("- {}", e.summary))
            .collect();
        if notable.len() > 5 {
            body.push(format!("... and {} more", notable.len() - 5));
        }
        body.push(format!("Open Security > Events on {}.", super::node_name()));
        super::notify(
            state,
            if high { "critical" } else { "warning" },
            format!(
                "{} audit event(s) on {}: {}",
                notable.len(),
                super::node_name(),
                notable[0].summary
            ),
            body.join("\n"),
        )
        .await;
    }
    Ok(evs)
}

async fn collect_now(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let s = super::load_settings(&state).await;
    if !s.audit.enabled {
        return Err(ApiError::NotAvailable {
            reason: "audit-disabled",
            message: "Audit event collection is turned off on this node.".into(),
            hint: "Turn it on in Security > Settings (audit.enabled).".into(),
        });
    }
    if !audit_installed() {
        return Err(audit_missing());
    }
    let _g = collect_lock().lock().await;
    let evs = collect(&state, &s).await.map_err(|e| {
        update_state(|st| st.audit_error = Some(e.clone()));
        ApiError::Internal(e)
    })?;
    Ok(Json(serde_json::json!({ "added": evs.len() })))
}

fn collect_lock() -> &'static tokio::sync::Mutex<()> {
    static L: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    L.get_or_init(|| tokio::sync::Mutex::new(()))
}

// ------------------------------------------------------------ integrity

fn aide_bin(s: &IntegritySettings) -> Option<std::path::PathBuf> {
    let p = s.program();
    if p.starts_with('/') {
        let pb = std::path::PathBuf::from(&p);
        pb.is_file().then_some(pb)
    } else {
        which(&p)
    }
}

fn aide_missing(s: &IntegritySettings) -> ApiError {
    ApiError::NotAvailable {
        reason: "aide-missing",
        message: format!("The file integrity checker '{}' is not installed on this node.", s.program()),
        hint: "Install app-forensics/aide (Gentoo; USE=ids on app-emulation/horcrux pulls it in) or the distribution's aide package, or set its path in Security > Settings.".into(),
    }
}

fn running() -> &'static Mutex<Option<String>> {
    static R: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(None))
}

fn db_path(s: &IntegritySettings) -> std::path::PathBuf {
    ids::aide_db_dir(s, &dd()).join("aide.db")
}

fn aide_version_cache() -> &'static Mutex<Option<String>> {
    static V: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    V.get_or_init(|| Mutex::new(None))
}

async fn aide_version(bin: &std::path::Path) -> String {
    if let Some(v) = aide_version_cache().lock().ok().and_then(|g| g.clone()) {
        return v;
    }
    let v = run_low(bin, &["--version".into()], 0, 20)
        .await
        .map(|(_, o, e)| {
            format!("{o}\n{e}")
                .lines()
                .find(|l| l.starts_with("AIDE"))
                .map(|l| l.trim_start_matches("AIDE").trim().to_string())
                .unwrap_or_default()
        })
        .unwrap_or_default();
    if let Ok(mut g) = aide_version_cache().lock() {
        *g = Some(v.clone());
    }
    v
}

/// Start an AIDE run in the background (`init`, `check` or `accept`).
pub async fn start_integrity(state: &Arc<AppState>, what: &'static str) -> Result<(), ApiError> {
    let s = super::load_settings(state).await;
    let is = s.integrity.clone();
    if !is.enabled {
        return Err(ApiError::NotAvailable {
            reason: "integrity-disabled",
            message: "File integrity checking is turned off on this node.".into(),
            hint: "Turn it on in Security > Settings (integrity.enabled).".into(),
        });
    }
    let bin = aide_bin(&is).ok_or_else(|| aide_missing(&is))?;
    if what != "init" && !db_path(&is).is_file() {
        return Err(ApiError::Conflict(
            "There is no integrity baseline yet: run Initialize first.".into(),
        ));
    }
    {
        let mut r = running().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(cur) = r.as_ref() {
            return Err(ApiError::Conflict(format!(
                "an integrity {cur} is already running"
            )));
        }
        *r = Some(what.to_string());
    }
    let st = state.clone();
    tokio::spawn(async move {
        let res = run_integrity(&st, &s, &bin, what).await;
        if let Err(e) = &res {
            tracing::warn!("integrity {what}: {e}");
        }
        update_state(|x| x.integrity_error = res.err());
        *running().lock().unwrap_or_else(|e| e.into_inner()) = None;
    });
    Ok(())
}

async fn run_integrity(
    state: &AppState,
    s: &SecuritySettings,
    bin: &std::path::Path,
    what: &str,
) -> Result<(), String> {
    let is = &s.integrity;
    let db = ids::aide_db_dir(is, &dd());
    std::fs::create_dir_all(&db).map_err(|e| format!("{}: {e}", db.display()))?;
    let conf = db.join("aide.conf");
    std::fs::write(&conf, ids::render_aide_conf(is, &db))
        .map_err(|e| format!("{}: {e}", conf.display()))?;
    let flag = match what {
        "init" => "--init",
        "accept" => "--update",
        _ => "--check",
    };
    let args = vec![format!("--config={}", conf.display()), flag.to_string()];
    let (code, out, err) = run_low(bin, &args, is.nice, is.timeout_secs as u64).await?;
    // 0 = no differences, 1-7 = added/removed/changed bits, 14+ = errors.
    if !(0..14).contains(&code) {
        let tail: Vec<&str> = err.lines().rev().take(4).collect();
        return Err(format!(
            "aide {flag} exited with {code}: {}",
            tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
        ));
    }
    let now = chrono::Utc::now().timestamp();
    let new_db = db.join("aide.db.new");
    match what {
        "init" | "accept" => {
            std::fs::rename(&new_db, db.join("aide.db"))
                .map_err(|e| format!("{}: {e}", new_db.display()))?;
            update_state(|x| {
                x.integrity_initialized_at = Some(now);
                x.integrity_last_differences = Some(0);
            });
            let ev = SecurityEvent {
                id: format!("i-{now}-b"),
                at: now,
                source: Some(horcrux_common::security::EventSource::Integrity),
                kind: "baseline".into(),
                severity: Some(Severity::Negligible),
                summary: if what == "init" {
                    "Integrity baseline created".into()
                } else {
                    "Integrity baseline updated: current files accepted".into()
                },
                detail: format!("{} ({})", db.join("aide.db").display(), paths_line(is)),
                ..Default::default()
            };
            EventStore::new(&dd()).append(&[ev], s.events.keep as usize)?;
        }
        _ => {
            let rep = ids::parse_aide_json(&out)?;
            let evs = ids::integrity_events(&rep, now, is.max_file_events as usize);
            EventStore::new(&dd()).append(&evs, s.events.keep as usize)?;
            let n = rep.differences();
            update_state(|x| {
                x.integrity_last_check = Some(now);
                x.integrity_last_differences = Some(n);
            });
            if n > 0 && is.notify {
                let high = evs[0].severity.is_some_and(|x| x >= Severity::High);
                let mut body: Vec<String> = evs
                    .iter()
                    .skip(1)
                    .take(8)
                    .map(|e| format!("- {} {}", e.path, e.kind))
                    .collect();
                if n > 8 {
                    body.push(format!("... and {} more", n - 8));
                }
                body.push(format!(
                    "Open Security > Events on {}. If the changes are expected (updates), accept them with 'Accept current files'.",
                    super::node_name()
                ));
                super::notify(
                    state,
                    if high { "critical" } else { "warning" },
                    format!(
                        "File integrity: {n} difference(s) on {}",
                        super::node_name()
                    ),
                    body.join("\n"),
                )
                .await;
            }
        }
    }
    Ok(())
}

fn paths_line(is: &IntegritySettings) -> String {
    is.paths.join(" ")
}

async fn integrity_init(State(state): State<Arc<AppState>>) -> Result<StatusCode, ApiError> {
    start_integrity(&state, "init")
        .await
        .map(|_| StatusCode::ACCEPTED)
}

async fn integrity_check(State(state): State<Arc<AppState>>) -> Result<StatusCode, ApiError> {
    start_integrity(&state, "check")
        .await
        .map(|_| StatusCode::ACCEPTED)
}

async fn integrity_accept(State(state): State<Arc<AppState>>) -> Result<StatusCode, ApiError> {
    start_integrity(&state, "accept")
        .await
        .map(|_| StatusCode::ACCEPTED)
}

// ------------------------------------------------------------ status / events

/// Audit state for the hardening rule and the status endpoint:
/// (installed, auditd running, Horcrux rules loaded).
pub async fn audit_facts() -> (bool, bool, u32) {
    if !audit_installed() {
        return (false, false, 0);
    }
    let running = super::super::system_admin::services::is_active("auditd")
        .await
        .unwrap_or(false);
    let loaded = run_secs("auditctl", &["-l"], 20)
        .await
        .map(|o| ids::count_loaded(&o))
        .unwrap_or(0);
    (true, running, loaded)
}

pub async fn ids_status(state: &AppState) -> IdsStatus {
    let s = super::load_settings(state).await;
    // Read "running" BEFORE the state file: a run that ends while this
    // status is being built (the subprocess probes below take a while) must
    // not be reported as idle together with its pre-run numbers. When it is
    // idle here, the state read after it is final.
    let integrity_running = running().lock().ok().and_then(|g| g.clone());
    let st = ids::load_state(&dd());
    let (installed, running_d, loaded) = audit_facts().await;
    let bin = aide_bin(&s.integrity);
    let version = match &bin {
        Some(b) => aide_version(b).await,
        None => String::new(),
    };
    let all = EventStore::new(&dd()).all();
    let day = chrono::Utc::now().timestamp() - 86_400;
    IdsStatus {
        audit_enabled: s.audit.enabled,
        audit_installed: installed,
        auditd_running: running_d,
        audit_rules_loaded: loaded,
        audit_last_collect: st.audit_last_collect,
        audit_error: st.audit_error,
        integrity_enabled: s.integrity.enabled,
        aide_installed: bin.is_some(),
        aide_version: version,
        integrity_initialized: db_path(&s.integrity).is_file(),
        integrity_initialized_at: st.integrity_initialized_at,
        integrity_running,
        integrity_last_check: st.integrity_last_check,
        integrity_last_differences: st.integrity_last_differences,
        integrity_error: st.integrity_error,
        events_total: all.len() as u32,
        events_24h: all.iter().filter(|e| e.at >= day).count() as u32,
    }
}

async fn status(State(state): State<Arc<AppState>>) -> Json<IdsStatus> {
    Json(ids_status(&state).await)
}

async fn events(Query(q): Query<EventQuery>) -> Json<ids::EventPage> {
    Json(ids::query(EventStore::new(&dd()).all(), &q))
}

// ------------------------------------------------------------ schedule

/// Audit collection every `audit.collect_secs`, integrity checks every
/// `integrity.schedule_hours` (due time from the last check on record, so a
/// restart does not re-check at once).
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        let mut last_collect: i64 = 0;
        loop {
            let s = super::load_settings(&state).await;
            let now = chrono::Utc::now().timestamp();
            if s.audit.enabled
                && audit_installed()
                && now - last_collect >= s.audit.collect_secs as i64
            {
                last_collect = now;
                let _g = collect_lock().lock().await;
                if let Err(e) = collect(&state, &s).await {
                    tracing::warn!("security audit collect: {e}");
                    update_state(|st| st.audit_error = Some(e));
                }
            }
            let is = &s.integrity;
            if is.enabled && is.schedule_hours > 0 && db_path(is).is_file() {
                let st = ids::load_state(&dd());
                let last = st
                    .integrity_last_check
                    .max(st.integrity_initialized_at)
                    .unwrap_or(0);
                if now - last >= is.schedule_hours as i64 * 3600 {
                    let _ = start_integrity(&state, "check").await;
                }
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
}

/// Integrity state for the hardening rule: (aide installed, enabled +
/// baseline + a check within twice the schedule).
pub async fn integrity_facts(state: &AppState) -> (bool, bool) {
    let s = super::load_settings(state).await;
    let is = &s.integrity;
    let installed = aide_bin(is).is_some();
    let st = ids::load_state(&dd());
    let now = chrono::Utc::now().timestamp();
    let fresh = match is.schedule_hours {
        0 => true,
        h => st
            .integrity_last_check
            .max(st.integrity_initialized_at)
            .is_some_and(|t| now - t <= 2 * h as i64 * 3600),
    };
    (
        installed,
        installed && is.enabled && db_path(is).is_file() && fresh,
    )
}
