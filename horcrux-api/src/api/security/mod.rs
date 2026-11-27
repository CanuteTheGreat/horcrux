//! Security: vulnerability scanning and the host hardening baseline
//! (docs/design/security-suite.md, user guide docs/SECURITY-SUITE.md).
//!
//! * `GET  /api/security/overview`                  counts for the dashboard
//! * `GET|PUT /api/security/settings`               node settings (declarative `security:`)
//! * `GET  /api/security/targets`                   images and containers that can be scanned
//! * `GET  /api/security/scans`, `POST` {kind,target} list / start a scan (202)
//! * `POST /api/security/scans/all`                 scan every target (202)
//! * `GET|DELETE /api/security/scans/:id`           one scan with findings
//! * `GET  /api/security/hardening`                 evaluate the baseline now
//! * `GET  /api/security/hardening/report?format=text|html|json` download
//! * `POST /api/security/hardening/apply|revert`    {rules?, dry_run?}
//! * `GET  /api/security/host`                      host exposure to Gentoo advisories (GLSA)
//! * `POST /api/security/host/check`                re-check now
//!
//! * `GET  /api/security/runtime`                    runtime detection status
//! * `GET|DELETE /api/security/runtime/events`      kept detections (`?limit&min_priority&rule&container`)
//! * `GET  /api/security/runtime/rules`             the curated rule file
//!
//! Reads need Sys.Audit, changes Sys.Modify (privileges.rs area `security`).
pub mod hardening;
pub mod host_cve;
pub mod runtime;
pub mod vmscan;
pub mod vulnscan;

use super::system_admin::{data_dir, run, which};
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use horcrux_common::security::{
    HardeningApplyRequest, HardeningApplyResult, HardeningReport, HostAdvisory, HostCveReport,
    RuleStatus, ScanRequest, ScanResult, ScanStatus, ScanSummary, ScanTarget, ScanTargetKind,
    SecurityOverview, SecuritySettings, SeverityCounts,
};
use serde::Deserialize;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

const SETTINGS_KIND: &str = "security_settings";

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/security/overview", get(overview))
        .route(
            "/api/security/settings",
            get(get_settings).put(put_settings),
        )
        .route("/api/security/targets", get(targets))
        .route("/api/security/scans", get(list_scans).post(start_scan))
        .route("/api/security/scans/all", post(scan_all))
        .route("/api/security/scans/:id", get(get_scan).delete(delete_scan))
        .route("/api/security/hardening", get(hardening_check))
        .route("/api/security/hardening/report", get(hardening_report))
        .route("/api/security/hardening/apply", post(hardening_apply))
        .route("/api/security/hardening/revert", post(hardening_revert))
        .route("/api/security/host", get(host_get))
        .route("/api/security/host/check", post(host_check))
        .route("/api/security/runtime", get(runtime_status))
        .route(
            "/api/security/runtime/events",
            get(runtime_events).delete(runtime_clear),
        )
        .route("/api/security/runtime/rules", get(runtime_rules))
}

// ------------------------------------------------------------ runtime detection

async fn runtime_status(
    State(state): State<Arc<AppState>>,
) -> Json<horcrux_common::security::RuntimeStatus> {
    Json(runtime::status(&state).await)
}

#[derive(Deserialize, Default)]
struct RuntimeEventsQuery {
    limit: Option<usize>,
    min_priority: Option<String>,
    rule: Option<String>,
    container: Option<String>,
}

async fn runtime_events(
    State(state): State<Arc<AppState>>,
    Query(q): Query<RuntimeEventsQuery>,
) -> Json<Vec<horcrux_common::security::RuntimeEvent>> {
    let s = load_settings(&state).await.runtime;
    let min = q
        .min_priority
        .as_deref()
        .map(horcrux_common::security::runtime_priority_rank)
        .unwrap_or(0);
    let limit = q.limit.unwrap_or(500).clamp(1, 100_000);
    Json(
        runtime::events(s.keep_events as usize)
            .into_iter()
            .filter(|e| horcrux_common::security::runtime_priority_rank(&e.priority) >= min)
            .filter(|e| q.rule.as_deref().is_none_or(|r| e.rule == r))
            .filter(|e| q.container.as_deref().is_none_or(|c| e.container == c))
            .take(limit)
            .collect(),
    )
}

async fn runtime_clear(State(state): State<Arc<AppState>>) -> StatusCode {
    let s = load_settings(&state).await.runtime;
    runtime::clear_events(s.keep_events as usize);
    StatusCode::NO_CONTENT
}

async fn runtime_rules() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/yaml; charset=utf-8")],
        runtime::CURATED_RULES,
    )
}

// ------------------------------------------------------------ host exposure

fn host_cache() -> &'static Mutex<Option<HostCveReport>> {
    static C: OnceLock<Mutex<Option<HostCveReport>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

/// Advisory ids already reported on this node (so a restart does not
/// notify again); first run only records them.
fn host_seen_path() -> std::path::PathBuf {
    data_dir().join("security").join("host-cve-seen.json")
}

fn load_seen() -> Option<Vec<String>> {
    serde_json::from_slice(&std::fs::read(host_seen_path()).ok()?).ok()
}

fn save_seen(ids: &[String]) {
    let p = host_seen_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(b) = serde_json::to_vec(ids) {
        let _ = std::fs::write(p, b);
    }
}

/// Evaluate the host's exposure now and cache it.
pub async fn host_check_now(state: &AppState) -> HostCveReport {
    let s = load_settings(state).await.host_cve;
    let node = node_name();
    let r = if s.enabled {
        let (s2, n2) = (s.clone(), node.clone());
        tokio::task::spawn_blocking(move || host_cve::evaluate(&s2, &n2))
            .await
            .unwrap_or_default()
    } else {
        HostCveReport {
            node,
            checked_at: chrono::Utc::now().timestamp(),
            reason: "The host vulnerability check is turned off (host_cve.enabled).".into(),
            update_command: s.update_command.clone(),
            ..Default::default()
        }
    };
    if let Ok(mut c) = host_cache().lock() {
        *c = Some(r.clone());
    }
    r
}

async fn host_get(State(state): State<Arc<AppState>>) -> Json<HostCveReport> {
    if let Some(r) = host_cache().lock().ok().and_then(|g| g.clone()) {
        return Json(r);
    }
    Json(host_check_now(&state).await)
}

async fn host_check(State(state): State<Arc<AppState>>) -> Json<HostCveReport> {
    Json(host_check_now(&state).await)
}

// ------------------------------------------------------------ settings

fn cell() -> &'static Mutex<Option<SecuritySettings>> {
    static C: OnceLock<Mutex<Option<SecuritySettings>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

/// The node's settings (stored in the database; defaults otherwise).
pub async fn load_settings(state: &AppState) -> SecuritySettings {
    if let Some(s) = cell().lock().ok().and_then(|c| c.clone()) {
        return s;
    }
    let s = crate::db::config_store::ConfigStore::new(&state.database)
        .load_values::<SecuritySettings>(SETTINGS_KIND)
        .await
        .ok()
        .and_then(|v| v.into_iter().next())
        .unwrap_or_default();
    if let Ok(mut c) = cell().lock() {
        *c = Some(s.clone());
    }
    s
}

/// Validate and store (API and declarative apply).
pub async fn set_settings(state: &AppState, s: SecuritySettings) -> Result<(), String> {
    s.validate()?;
    crate::db::config_store::ConfigStore::new(&state.database)
        .put(SETTINGS_KIND, "default", &s)
        .await
        .map_err(|e| e.to_string())?;
    if let Ok(mut c) = cell().lock() {
        *c = Some(s);
    }
    Ok(())
}

async fn get_settings(State(state): State<Arc<AppState>>) -> Json<SecuritySettings> {
    Json(load_settings(&state).await)
}

async fn put_settings(
    State(state): State<Arc<AppState>>,
    Json(s): Json<SecuritySettings>,
) -> Result<Json<SecuritySettings>, ApiError> {
    set_settings(&state, s.clone())
        .await
        .map_err(ApiError::BadRequest)?;
    Ok(Json(s))
}

// ------------------------------------------------------------ scanning

fn store() -> vulnscan::Store {
    vulnscan::Store::new(&data_dir())
}

fn scanner_bin(s: &SecuritySettings) -> Option<std::path::PathBuf> {
    let p = s.vuln_scan.program();
    if p.starts_with('/') {
        let pb = std::path::PathBuf::from(&p);
        pb.is_file().then_some(pb)
    } else {
        which(&p)
    }
}

fn scanner_missing(s: &SecuritySettings) -> ApiError {
    let (pkg, url) = if s.vuln_scan.scanner == "trivy" {
        (
            "Trivy",
            "https://trivy.dev/latest/getting-started/installation/",
        )
    } else {
        (
            "Grype (app-containers/grype)",
            "https://github.com/anchore/grype#installation",
        )
    };
    ApiError::NotAvailable {
        reason: "scanner-missing",
        message: format!(
            "The vulnerability scanner '{}' is not installed on this node.",
            s.vuln_scan.program()
        ),
        hint: format!(
            "Install {pkg} ({url}), or pick another scanner or its path in Security > Settings."
        ),
    }
}

fn scanning_disabled() -> ApiError {
    ApiError::NotAvailable {
        reason: "scanning-disabled",
        message: "Vulnerability scanning is turned off on this node.".into(),
        hint: "Turn it on in Security > Settings (vuln_scan.enabled).".into(),
    }
}

/// Running scan count and the wake-up for queued ones.
fn slots() -> &'static (Mutex<u32>, tokio::sync::Notify) {
    static S: OnceLock<(Mutex<u32>, tokio::sync::Notify)> = OnceLock::new();
    S.get_or_init(|| (Mutex::new(0), tokio::sync::Notify::new()))
}

async fn acquire(max: u32) {
    loop {
        let notified = slots().1.notified();
        {
            let mut n = slots().0.lock().unwrap_or_else(|e| e.into_inner());
            if *n < max.max(1) {
                *n += 1;
                return;
            }
        }
        notified.await;
    }
}

fn release() {
    let mut n = slots().0.lock().unwrap_or_else(|e| e.into_inner());
    *n = n.saturating_sub(1);
    slots().1.notify_waiters();
}

fn scanner_version_cache() -> &'static Mutex<Option<(String, String)>> {
    static V: OnceLock<Mutex<Option<(String, String)>>> = OnceLock::new();
    V.get_or_init(|| Mutex::new(None))
}

async fn scanner_version(bin: &std::path::Path) -> String {
    let key = bin.display().to_string();
    if let Some((k, v)) = scanner_version_cache().lock().ok().and_then(|g| g.clone()) {
        if k == key {
            return v;
        }
    }
    let out = tokio::time::timeout(
        Duration::from_secs(20),
        tokio::process::Command::new(bin)
            .arg("version")
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let v = match out {
        Ok(Ok(o)) => vulnscan::parse_version(&String::from_utf8_lossy(&o.stdout)),
        _ => String::new(),
    };
    if let Ok(mut g) = scanner_version_cache().lock() {
        *g = Some((key, v.clone()));
    }
    v
}

/// Run one scan to completion (the record is already saved as queued).
async fn execute(state: Arc<AppState>, mut rec: ScanResult) {
    let s = load_settings(&state).await;
    let vs = s.vuln_scan.clone();
    acquire(vs.max_concurrent).await;
    rec.summary.status = ScanStatus::Running;
    let st = store();
    let _ = st.save(&rec);
    let kind = rec.summary.kind.unwrap_or(ScanTargetKind::Docker);
    let target = rec.summary.target.clone();
    let outcome: Result<(String, Vec<horcrux_common::security::VulnFinding>), String> = async {
        let bin = scanner_bin(&s).ok_or("the scanner is not installed")?;
        let version = scanner_version(&bin).await;
        let db = vulnscan::db_dir(&vs, &data_dir());
        let _ = std::fs::create_dir_all(&db);
        if kind.is_vm() {
            // A VM is scanned from its guest agent's inventory (running) or a
            // read-only mount of its disk (stopped); `prepared` cleans up on
            // drop after the scanner has read its input.
            let vms = state.vm_manager.list_vms().await;
            let vm = vmscan::find_vm(&vms, &target)
                .ok_or_else(|| format!("VM '{target}' not found on this node"))?;
            let prepared = vmscan::prepare(&vs, vm).await?;
            let findings = run_scanner(&bin, &vs, kind, &target, prepared.input(), &db).await?;
            Ok((version, findings))
        } else {
            let rootfs = if kind.is_image() {
                None
            } else {
                Some(
                    vulnscan::rootfs(&vs, kind, &target)
                        .ok_or_else(|| format!("{target} has no root file system"))?,
                )
            };
            let input = match &rootfs {
                Some(p) => vulnscan::ScanInput::Dir(p),
                None => vulnscan::ScanInput::Image,
            };
            let findings = run_scanner(&bin, &vs, kind, &target, input, &db).await?;
            Ok((version, findings))
        }
    }
    .await;
    match outcome {
        Ok((version, f)) => {
            let (f, counts) = vulnscan::finish(f);
            rec.summary.scanner_version = version;
            rec.summary.status = ScanStatus::Done;
            rec.summary.counts = counts;
            rec.findings = f;
        }
        Err(e) => {
            rec.summary.status = ScanStatus::Failed;
            rec.summary.error = Some(e);
        }
    }
    rec.summary.finished = Some(chrono::Utc::now().timestamp());
    if let Err(e) = st.save(&rec) {
        tracing::warn!("security scan {}: {e}", rec.summary.id);
    }
    release();
    st.prune(vs.keep_scans as usize);
    if rec.summary.status == ScanStatus::Done {
        if let Some(min) = vs.notify_threshold() {
            let n = rec.summary.counts.at_least(min);
            if n > 0 {
                notify(
                    &state,
                    if rec.summary.counts.critical > 0 { "critical" } else { "warning" },
                    format!(
                        "{} {}: {n} vulnerabilities at {} or above",
                        kind.label(),
                        rec.summary.target,
                        min.as_str()
                    ),
                    format!(
                        "Scan {} found {} critical, {} high, {} medium ({} with a fix). Open Security > Vulnerabilities on {}.",
                        rec.summary.id,
                        rec.summary.counts.critical,
                        rec.summary.counts.high,
                        rec.summary.counts.medium,
                        rec.summary.counts.fixable,
                        node_name()
                    ),
                )
                .await;
            }
        }
    }
}

/// Run the scanner process on one input (low priority, time-limited) and
/// parse its findings. Shared by image, container and VM scans.
async fn run_scanner(
    bin: &std::path::Path,
    vs: &horcrux_common::security::VulnScanSettings,
    kind: ScanTargetKind,
    target: &str,
    input: vulnscan::ScanInput<'_>,
    db: &std::path::Path,
) -> Result<Vec<horcrux_common::security::VulnFinding>, String> {
    let (args, env) = vulnscan::command(vs, kind, target, input, db);
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(&args)
        .envs(env)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    // Low priority: CPU niceness and the idle IO class, set in the child
    // before exec (best effort: a refusal leaves the default priority).
    let nice = vs.nice as i32;
    // SAFETY: only async-signal-safe libc calls between fork and exec.
    unsafe {
        cmd.pre_exec(move || {
            nix::libc::setpriority(nix::libc::PRIO_PROCESS, 0, nice);
            // ioprio_set(IOPRIO_WHO_PROCESS, self, IOPRIO_CLASS_IDLE << 13)
            nix::libc::syscall(
                nix::libc::SYS_ioprio_set,
                1 as nix::libc::c_long,
                0 as nix::libc::c_long,
                (3 << 13) as nix::libc::c_long,
            );
            Ok(())
        });
    }
    let out = tokio::time::timeout(Duration::from_secs(vs.timeout_secs as u64), cmd.output())
        .await
        .map_err(|_| {
            format!(
                "the scan took longer than {} s and was stopped",
                vs.timeout_secs
            )
        })?
        .map_err(|e| format!("{}: {e}", bin.display()))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() && stdout.trim().is_empty() {
        let err = String::from_utf8_lossy(&out.stderr);
        let last: Vec<&str> = err.lines().rev().take(6).collect();
        return Err(format!(
            "scanner exited with {}: {}",
            out.status,
            last.into_iter().rev().collect::<Vec<_>>().join(" | ")
        ));
    }
    vulnscan::parse_output(&vs.scanner, &stdout)
}

fn node_name() -> String {
    hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "horcrux".into())
}

/// Send a `security` event through the notification channels.
async fn notify(state: &AppState, severity: &str, title: String, body: String) {
    let msg = crate::notification_channels::Message {
        title,
        body,
        severity: severity.into(),
        event: "security".into(),
        alert: None,
    };
    if let Some(store) = state.alert_manager.channel_store() {
        store.dispatch(&msg).await;
    }
}

/// Queue a scan; returns its record (status queued).
async fn queue(
    state: &Arc<AppState>,
    kind: ScanTargetKind,
    target: &str,
    by: &str,
) -> Result<ScanSummary, ApiError> {
    let s = load_settings(state).await;
    if !s.vuln_scan.enabled {
        return Err(scanning_disabled());
    }
    if scanner_bin(&s).is_none() {
        return Err(scanner_missing(&s));
    }
    vulnscan::valid_target(kind, target).map_err(ApiError::BadRequest)?;
    if kind.is_vm() {
        // The VM must exist on this node; its disk/agent is resolved at scan
        // time (the live disk is never read).
        let vms = state.vm_manager.list_vms().await;
        if vmscan::find_vm(&vms, target).is_none() {
            return Err(ApiError::NotFound(format!("VM '{target}'")));
        }
    } else if !kind.is_image() && vulnscan::rootfs(&s.vuln_scan, kind, target).is_none() {
        return Err(ApiError::NotFound(format!(
            "{} '{target}' (no root file system found)",
            kind.label()
        )));
    }
    let rec = ScanResult {
        summary: ScanSummary {
            id: uuid::Uuid::new_v4().simple().to_string()[..16].to_string(),
            kind: Some(kind),
            target: target.to_string(),
            scanner: s.vuln_scan.scanner.clone(),
            status: ScanStatus::Queued,
            created: chrono::Utc::now().timestamp(),
            requested_by: by.to_string(),
            ..Default::default()
        },
        findings: vec![],
    };
    store().save(&rec).map_err(ApiError::Internal)?;
    let sum = rec.summary.clone();
    tokio::spawn(execute(state.clone(), rec));
    Ok(sum)
}

/// Every scannable image and container on this node.
pub async fn list_targets(state: &AppState) -> Vec<ScanTarget> {
    let s = load_settings(state).await.vuln_scan;
    let mut out = Vec::new();
    for (kind, prog) in [
        (ScanTargetKind::Podman, "podman"),
        (ScanTargetKind::Docker, "docker"),
    ] {
        if which(prog).is_none() {
            continue;
        }
        if let Ok(t) = super::system_admin::run_secs(
            prog,
            &["images", "--format", "{{.Repository}}:{{.Tag}} {{.ID}}"],
            30,
        )
        .await
        {
            out.extend(vulnscan::parse_image_list(kind, &t));
        }
    }
    out.extend(vulnscan::list_rootfs(
        ScanTargetKind::Lxc,
        std::path::Path::new(&s.lxc_path),
    ));
    out.extend(vulnscan::list_rootfs(
        ScanTargetKind::Incus,
        &std::path::Path::new(&s.incus_path).join("containers"),
    ));
    if s.vm_disk_scan || s.vm_agent_scan {
        out.extend(vmscan::vm_targets(&state.vm_manager.list_vms().await));
    }
    let latest = vulnscan::latest_by_target(&store().list());
    for t in &mut out {
        t.last_scan = latest
            .get(&(t.kind.as_str().to_string(), t.name.clone()))
            .cloned();
    }
    out
}

async fn targets(State(state): State<Arc<AppState>>) -> Json<Vec<ScanTarget>> {
    Json(list_targets(&state).await)
}

#[derive(Deserialize, Default)]
struct ScanListQ {
    kind: Option<ScanTargetKind>,
    target: Option<String>,
    limit: Option<usize>,
}

async fn list_scans(Query(q): Query<ScanListQ>) -> Json<Vec<ScanSummary>> {
    let v: Vec<ScanSummary> = store()
        .list()
        .into_iter()
        .filter(|s| q.kind.is_none() || s.kind == q.kind)
        .filter(|s| q.target.as_deref().is_none_or(|t| s.target == t))
        .take(q.limit.unwrap_or(500))
        .collect();
    Json(v)
}

fn caller(u: &Option<axum::Extension<crate::middleware::auth::AuthUser>>) -> String {
    super::system_admin::user_of(u)
}

async fn start_scan(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::middleware::auth::AuthUser>>,
    Json(req): Json<ScanRequest>,
) -> Result<(StatusCode, Json<ScanSummary>), ApiError> {
    let s = queue(&state, req.kind, req.target.trim(), &caller(&user)).await?;
    Ok((StatusCode::ACCEPTED, Json(s)))
}

async fn scan_all(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::middleware::auth::AuthUser>>,
) -> Result<(StatusCode, Json<Vec<ScanSummary>>), ApiError> {
    let by = caller(&user);
    let mut out = Vec::new();
    for t in list_targets(&state).await {
        out.push(queue(&state, t.kind, &t.name, &by).await?);
    }
    Ok((StatusCode::ACCEPTED, Json(out)))
}

async fn get_scan(Path(id): Path<String>) -> Result<Json<ScanResult>, ApiError> {
    store()
        .get(&id)
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("scan {id}")))
}

async fn delete_scan(Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    match store().get(&id) {
        None => Err(ApiError::NotFound(format!("scan {id}"))),
        Some(r) if matches!(r.summary.status, ScanStatus::Queued | ScanStatus::Running) => {
            Err(ApiError::Conflict("the scan is still running".into()))
        }
        Some(_) => {
            store().delete(&id);
            Ok(StatusCode::NO_CONTENT)
        }
    }
}

// ------------------------------------------------------------ hardening

async fn facts() -> hardening::Facts {
    let sshd = if which("sshd").is_some() {
        run("sshd", &["-T"])
            .await
            .ok()
            .map(|o| hardening::parse_sshd_t(&o))
    } else {
        None
    };
    let firewall_tables = if which("nft").is_some() {
        Some(
            run("nft", &["list", "tables"])
                .await
                .map(|o| o.lines().filter(|l| l.starts_with("table ")).count() as u32)
                .unwrap_or(0),
        )
    } else {
        None
    };
    let mut time_sync = false;
    if let Some(b) = super::system_admin::host::detect_ntp() {
        for u in b.units() {
            if super::system_admin::services::is_active(u)
                .await
                .unwrap_or(false)
            {
                time_sync = true;
                break;
            }
        }
    }
    let mut programs = Vec::new();
    for r in hardening::RULES {
        if let hardening::Check::Present(n) | hardening::Check::Absent(n) = r.check {
            for p in n {
                if which(p).is_some() {
                    programs.push(p.to_string());
                }
            }
        }
    }
    hardening::Facts {
        sshd,
        firewall_tables,
        access_log: horcrux_core::config::HorcruxConfig::load()
            .access_log
            .enabled,
        time_sync,
        programs,
    }
}

fn last_report() -> &'static Mutex<Option<HardeningReport>> {
    static R: OnceLock<Mutex<Option<HardeningReport>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(None))
}

/// Evaluate the baseline now (and remember it for the overview).
pub async fn check_now(state: &AppState) -> HardeningReport {
    let s = load_settings(state).await;
    let f = facts().await;
    let st = hardening::load_state(&data_dir());
    let r = hardening::evaluate(
        &s.hardening,
        &f,
        &st,
        &node_name(),
        chrono::Utc::now().timestamp(),
    );
    if let Ok(mut g) = last_report().lock() {
        *g = Some(r.clone());
    }
    r
}

async fn hardening_check(State(state): State<Arc<AppState>>) -> Json<HardeningReport> {
    Json(check_now(&state).await)
}

#[derive(Deserialize, Default)]
struct ReportQ {
    format: Option<String>,
}

async fn hardening_report(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ReportQ>,
) -> impl IntoResponse {
    let r = check_now(&state).await;
    let day = chrono::Utc::now().format("%Y%m%d");
    let (ct, ext, body) = match q.format.as_deref().unwrap_or("text") {
        "html" => (
            "text/html; charset=utf-8",
            "html",
            hardening::render_html(&r),
        ),
        "json" => (
            "application/json",
            "json",
            serde_json::to_string_pretty(&r).unwrap_or_default(),
        ),
        _ => (
            "text/plain; charset=utf-8",
            "txt",
            hardening::render_text(&r),
        ),
    };
    (
        [
            (header::CONTENT_TYPE, ct.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"hardening-{}-{day}.{ext}\"", r.node),
            ),
        ],
        body,
    )
}

/// One change at a time: apply/revert record previous values.
fn hardening_lock() -> &'static tokio::sync::Mutex<()> {
    static L: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    L.get_or_init(|| tokio::sync::Mutex::new(()))
}

async fn hardening_apply(
    State(state): State<Arc<AppState>>,
    Json(req): Json<HardeningApplyRequest>,
) -> Result<Json<HardeningApplyResult>, ApiError> {
    let _g = hardening_lock().lock().await;
    let s = load_settings(&state).await;
    let r = check_now(&state).await;
    let dd = data_dir();
    let mut st = hardening::load_state(&dd);
    let res = hardening::apply(&s.hardening, &r, &req.rules, req.dry_run, &mut st);
    if !req.dry_run {
        hardening::save_state(&dd, &st).map_err(ApiError::Internal)?;
        check_now(&state).await;
    }
    Ok(Json(res))
}

async fn hardening_revert(
    State(state): State<Arc<AppState>>,
    Json(req): Json<HardeningApplyRequest>,
) -> Result<Json<HardeningApplyResult>, ApiError> {
    let _g = hardening_lock().lock().await;
    let s = load_settings(&state).await;
    let dd = data_dir();
    let mut st = hardening::load_state(&dd);
    let res = hardening::revert(&s.hardening, &req.rules, req.dry_run, &mut st);
    if !req.dry_run {
        hardening::save_state(&dd, &st).map_err(ApiError::Internal)?;
        check_now(&state).await;
    }
    Ok(Json(res))
}

// ------------------------------------------------------------ overview

async fn overview(State(state): State<Arc<AppState>>) -> Json<SecurityOverview> {
    let s = load_settings(&state).await;
    let bin = scanner_bin(&s);
    let version = match &bin {
        Some(b) => scanner_version(b).await,
        None => String::new(),
    };
    let targets = list_targets(&state).await;
    let scans = store().list();
    let mut counts = SeverityCounts::default();
    let mut scanned = 0;
    for t in &targets {
        if let Some(l) = t
            .last_scan
            .as_ref()
            .filter(|l| l.status == ScanStatus::Done)
        {
            scanned += 1;
            for sev in horcrux_common::security::Severity::ALL {
                for _ in 0..l.counts.get(sev) {
                    counts.add(sev, false);
                }
            }
            counts.fixable += l.counts.fixable;
        }
    }
    let rep = match last_report().lock().ok().and_then(|g| g.clone()) {
        Some(r) => r,
        None => check_now(&state).await,
    };
    Json(SecurityOverview {
        scanner_available: bin.is_some(),
        scanner: s.vuln_scan.scanner.clone(),
        scanner_version: version,
        scanning_enabled: s.vuln_scan.enabled,
        targets: targets.len() as u32,
        targets_scanned: scanned,
        counts,
        running_scans: scans
            .iter()
            .filter(|x| matches!(x.status, ScanStatus::Queued | ScanStatus::Running))
            .count() as u32,
        last_scan_at: scans.iter().filter_map(|x| x.finished).max(),
        hardening_score: rep.score,
        hardening_failed: rep.failed,
        hardening_manual: rep.manual,
        hardening_checked_at: Some(rep.checked_at),
        runtime_enabled: s.runtime.enabled,
        runtime_running: runtime::is_running(),
        runtime_events_24h: runtime::events_since(
            s.runtime.keep_events as usize,
            chrono::Utc::now().timestamp() - 86400,
        ),
    })
}

// ------------------------------------------------------------ schedule

/// Periodic scans and baseline checks. Due times follow the newest scan /
/// check on record, so a restart does not rescan everything at once.
pub fn spawn_scheduler(state: Arc<AppState>) {
    runtime::spawn_supervisor(state.clone());
    tokio::spawn(async move {
        // Let the node finish starting before the first look.
        tokio::time::sleep(Duration::from_secs(120)).await;
        let mut last_check: Option<i64> = None;
        let mut previous_fail: Option<Vec<String>> = None;
        let mut last_host: Option<i64> = None;
        loop {
            let s = load_settings(&state).await;
            let now = chrono::Utc::now().timestamp();
            let vs = &s.vuln_scan;
            if vs.enabled && vs.schedule_hours > 0 && scanner_bin(&s).is_some() {
                let period = vs.schedule_hours as i64 * 3600;
                for t in list_targets(&state).await {
                    if !vs.schedule_kinds.contains(&t.kind) {
                        continue;
                    }
                    let due = t
                        .last_scan
                        .as_ref()
                        .is_none_or(|l| now - l.created >= period);
                    let busy = store().list().iter().any(|x| {
                        x.target == t.name
                            && x.kind == Some(t.kind)
                            && matches!(x.status, ScanStatus::Queued | ScanStatus::Running)
                    });
                    if due && !busy {
                        let _ = queue(&state, t.kind, &t.name, "scheduler").await;
                    }
                }
            }
            let hc = &s.host_cve;
            if hc.enabled
                && hc.schedule_hours > 0
                && last_host.is_none_or(|l| now - l >= hc.schedule_hours as i64 * 3600)
            {
                let r = host_check_now(&state).await;
                last_host = Some(now);
                if r.supported {
                    let ids: Vec<String> = r.advisories.iter().map(|a| a.id.clone()).collect();
                    if let Some(seen) = load_seen() {
                        let new: Vec<&HostAdvisory> = r
                            .advisories
                            .iter()
                            .filter(|a| !seen.contains(&a.id))
                            .collect();
                        if hc.notify_on_new && !new.is_empty() {
                            let worst = if new.iter().any(|a| a.severity == "high") {
                                "critical"
                            } else {
                                "warning"
                            };
                            notify(
                                &state,
                                worst,
                                format!(
                                    "{} new security advisor{} affect{} {}",
                                    new.len(),
                                    if new.len() == 1 { "y" } else { "ies" },
                                    if new.len() == 1 { "s" } else { "" },
                                    r.node
                                ),
                                format!(
                                    "{}. Update with: {}. Details: Security > Overview.",
                                    new.iter()
                                        .map(|a| format!(
                                            "{} ({})",
                                            a.id,
                                            a.packages
                                                .iter()
                                                .map(|p| p.package.as_str())
                                                .collect::<Vec<_>>()
                                                .join(", ")
                                        ))
                                        .collect::<Vec<_>>()
                                        .join("; "),
                                    r.update_command
                                ),
                            )
                            .await;
                        }
                    }
                    save_seen(&ids);
                }
            }
            let h = &s.hardening;
            if h.schedule_hours > 0
                && last_check.is_none_or(|l| now - l >= h.schedule_hours as i64 * 3600)
            {
                let r = check_now(&state).await;
                last_check = Some(now);
                let failing: Vec<String> = r
                    .rules
                    .iter()
                    .filter(|x| matches!(x.status, RuleStatus::Fail))
                    .map(|x| x.id.clone())
                    .collect();
                if let Some(prev) = &previous_fail {
                    let new: Vec<&String> = failing.iter().filter(|x| !prev.contains(x)).collect();
                    if h.notify_on_regression && !new.is_empty() {
                        notify(
                            &state,
                            "warning",
                            format!(
                                "Hardening regression on {}: {} rule(s) now failing",
                                r.node,
                                new.len()
                            ),
                            format!(
                                "Newly failing: {}. Score {:.1}%. Open Security > Hardening.",
                                new.iter()
                                    .map(|s| s.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", "),
                                r.score
                            ),
                        )
                        .await;
                    }
                }
                previous_fail = Some(failing);
            }
            tokio::time::sleep(Duration::from_secs(300)).await;
        }
    });
}
