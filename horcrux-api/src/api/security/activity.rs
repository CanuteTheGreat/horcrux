//! Security events: one dashboard over the API access log, the audit log,
//! failed logins and lockouts, baseline drift and scan results, alert rules
//! on them, and brute-force detection with an optional temporary block in
//! the host firewall (docs/design/security-suite.md, docs/SECURITY-SUITE.md).
//!
//! * `GET /api/security/activity?hours=` totals, timeline, top failing clients/users
//! * `GET /api/security/activity/events?since=&until=&source=&kind=&severity=&client=&user=&q=&limit=&offset=`
//! * `GET|POST|DELETE /api/security/blocks` (DELETE `?ip=`) temporary blocks
//!
//! Alerts go out as `security` notifications whose rule name and target
//! (client or user) are visible to notification matchers (`rule`, `target`).
//! Blocks live in their own nftables table (`detection.block_table`, input hook,
//! per-element timeouts), never touch the Horcrux firewall's rules, and are
//! re-applied after a restart from `<data_dir>/security-blocks.json`.
use super::{load_settings, node_name};
use crate::api::system_admin::{data_dir, which};
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::{ConnectInfo, Query, State},
    Json,
};
use horcrux_common::security_activity::{
    ip_in, normalize_ip, parse_ip_or_cidr, ActivityDashboard, ActivityTotals, BlockRequest,
    BlockedClient, DetectionSettings, EventBucket, EventRule, TimelineEvent, TimelinePage,
    TopFailing,
};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// Audit entries and access entries seen at most this far back are read.
const MAX_LOOKBACK_HOURS: i64 = 24 * 30;

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// One observation with the API path group it came from (rules filter on it).
#[derive(Debug, Clone)]
pub struct Obs {
    pub ev: TimelineEvent,
    pub group: String,
}

// ------------------------------------------------------------ classify

/// Audit entry -> security event, or `None` for entries that are not security-relevant.
pub fn classify_audit(
    kind: &crate::audit::AuditEventType,
    user: Option<&str>,
    details: Option<&str>,
) -> Option<(&'static str, &'static str, String)> {
    use crate::audit::AuditEventType as T;
    let u = user.unwrap_or("(unknown)");
    let det = details.unwrap_or("");
    Some(match kind {
        T::LoginFailed => {
            let d = det.to_ascii_lowercase();
            if d.contains("backing off") || d.contains("locked") || d.contains("too many") {
                (
                    "lockout",
                    "warning",
                    format!("Login for {u} refused: too many failures"),
                )
            } else {
                ("login_failed", "warning", format!("Failed login for {u}"))
            }
        }
        T::PermissionDenied | T::ConsoleAccessDenied => (
            "permission_denied",
            "warning",
            format!(
                "{u} was refused: {}",
                if det.is_empty() {
                    "permission denied"
                } else {
                    det
                }
            ),
        ),
        T::BruteForceDetected => ("brute_force", "critical", format!("Brute force on {u}")),
        T::SuspiciousActivity => (
            "suspicious",
            "warning",
            format!("Suspicious activity by {u}"),
        ),
        T::PasswordChanged => ("auth_change", "info", format!("Password changed for {u}")),
        T::TwoFactorEnabled => (
            "auth_change",
            "info",
            format!("Two-factor sign-in enabled for {u}"),
        ),
        T::TwoFactorDisabled => (
            "auth_change",
            "warning",
            format!("Two-factor sign-in disabled for {u}"),
        ),
        T::PermissionGranted => ("auth_change", "info", format!("Permission granted ({u})")),
        T::RoleAssigned => ("auth_change", "info", format!("Role assigned ({u})")),
        T::RoleRevoked => ("auth_change", "info", format!("Role revoked ({u})")),
        T::SecretAccessed => ("auth_change", "info", format!("Secret read by {u}")),
        T::CertificateRevoked => ("auth_change", "warning", "Certificate revoked".to_string()),
        T::SecurityPolicyChanged => (
            "policy_change",
            "warning",
            format!("Security policy changed by {u}"),
        ),
        T::FirewallRuleAdded => (
            "policy_change",
            "info",
            format!("Firewall rule added by {u}"),
        ),
        T::FirewallRuleDeleted => (
            "policy_change",
            "info",
            format!("Firewall rule removed by {u}"),
        ),
        _ => return None,
    })
}

/// A failed or refused API call -> event; successful calls and the login
/// endpoint itself (the audit log has every login attempt) are not events.
pub fn classify_access(e: &crate::api::access_log::AccessEntry) -> Option<Obs> {
    if e.path == "/api/auth/login" {
        return None;
    }
    let (kind, sev) = match e.status {
        401 | 403 => ("access_denied", "warning"),
        429 => ("rate_limited", "warning"),
        404 => ("not_found", "info"),
        500..=599 => ("server_error", "warning"),
        _ => return None,
    };
    Some(Obs {
        ev: TimelineEvent {
            ts: e.ts_ms.div_euclid(1000),
            source: "access".into(),
            kind: kind.into(),
            severity: sev.into(),
            client: Some(e.client.clone()),
            user: e.user.clone(),
            summary: format!("{} {} -> {}", e.method, e.path, e.status),
            detail: e.user_agent.clone(),
        },
        group: e.path_group.clone(),
    })
}

/// Everything the detector and the dashboard read, since `since` (unix s).
pub async fn collect(state: &AppState, since: i64) -> Vec<Obs> {
    let mut out = Vec::new();
    if let Some(log) = crate::api::access_log::global() {
        for e in log.all() {
            if e.ts_ms.div_euclid(1000) >= since {
                if let Some(o) = classify_access(&e) {
                    out.push(o);
                }
            }
        }
    }
    for a in state.audit_logger.all().await {
        let ts = a.timestamp.timestamp();
        if ts < since {
            continue;
        }
        if let Some((kind, sev, summary)) =
            classify_audit(&a.event_type, a.user.as_deref(), a.details.as_deref())
        {
            out.push(Obs {
                ev: TimelineEvent {
                    ts,
                    source: "audit".into(),
                    kind: kind.into(),
                    severity: sev.into(),
                    client: a.source_ip.clone().filter(|s| !s.is_empty()),
                    user: a.user.clone(),
                    summary,
                    detail: a.details.clone(),
                },
                group: String::new(),
            });
        }
    }
    for e in journal_read() {
        if e.ts >= since {
            out.push(Obs {
                ev: e,
                group: String::new(),
            });
        }
    }
    out.sort_by_key(|o| o.ev.ts);
    out
}

// ------------------------------------------------------------ journal

/// Detector, scan and baseline events (what the other sources cannot tell).
fn journal_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

fn journal_path() -> std::path::PathBuf {
    data_dir().join("security-activity.jsonl")
}

fn journal_read() -> Vec<TimelineEvent> {
    let _g = journal_lock().lock().ok();
    std::fs::read_to_string(journal_path())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Append one event; the file is rewritten to the newest `max` lines once it
/// holds twice that many.
pub fn journal_append(ev: &TimelineEvent, max: usize) {
    use std::io::Write;
    let _g = journal_lock().lock().ok();
    let path = journal_path();
    let Ok(line) = serde_json::to_string(ev) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{line}");
    }
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() > max.saturating_mul(2) {
        let keep = lines[lines.len() - max..].join("\n") + "\n";
        let tmp = path.with_extension("jsonl.tmp");
        if std::fs::write(&tmp, keep).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/// A timeline event stamped now.
pub fn ev(
    source: &str,
    kind: &str,
    severity: &str,
    client: Option<String>,
    user: Option<String>,
    summary: String,
    detail: Option<String>,
) -> TimelineEvent {
    TimelineEvent {
        ts: now(),
        source: source.into(),
        kind: kind.into(),
        severity: severity.into(),
        client,
        user,
        summary,
        detail,
    }
}

/// Journal an event (always) with the node's retention setting.
pub async fn journal(state: &AppState, e: TimelineEvent) {
    let max = load_settings(state).await.detection.journal_max_entries as usize;
    journal_append(&e, max);
}

/// A `security` notification that carries a rule name and a target so
/// notification matchers can route on `rule` and `target`.
/// What a notification is about.
struct AlertInfo<'a> {
    severity: &'a str,
    rule_id: &'a str,
    rule_name: &'a str,
    target: &'a str,
    value: f64,
    threshold: f64,
}

async fn alert(state: &AppState, i: AlertInfo<'_>, title: String, body: String) {
    use crate::alerts::{Alert, AlertSeverity, AlertStatus};
    let AlertInfo {
        severity,
        rule_id,
        rule_name,
        target,
        value,
        threshold,
    } = i;
    let sev = match severity {
        "critical" => AlertSeverity::Critical,
        "info" => AlertSeverity::Info,
        _ => AlertSeverity::Warning,
    };
    let msg = crate::notification_channels::Message {
        title,
        body: body.clone(),
        severity: severity.into(),
        event: "security".into(),
        alert: Some(Alert {
            id: format!("sec-{rule_id}-{}-{}", target, now()),
            rule_id: rule_id.into(),
            rule_name: rule_name.into(),
            severity: sev,
            status: AlertStatus::Firing,
            message: body,
            target: target.into(),
            metric_value: value,
            threshold,
            fired_at: now(),
            resolved_at: None,
            acknowledged_at: None,
            acknowledged_by: None,
        }),
    };
    if let Some(store) = state.alert_manager.channel_store() {
        store.dispatch(&msg).await;
    }
}

// ------------------------------------------------------------ detection (pure)

/// Is this client address on the allowlist?
pub fn allowlisted(allow: &[String], client: &str) -> bool {
    let Ok(ip) = client.parse::<IpAddr>() else {
        return false;
    };
    allow
        .iter()
        .filter_map(|a| parse_ip_or_cidr(a))
        .any(|net| ip_in(net, ip))
}

/// Clients with at least `failures` failed logins or lockouts in the window
/// that have not fired within it already. Returns (client, count).
pub fn detect_brute_force(
    s: &DetectionSettings,
    obs: &[Obs],
    now: i64,
    fired: &mut HashMap<String, i64>,
) -> Vec<(String, u32)> {
    let b = &s.brute_force;
    if !b.enabled {
        return Vec::new();
    }
    let window = b.window_minutes as i64 * 60;
    let mut count: BTreeMap<String, u32> = BTreeMap::new();
    for o in obs {
        if !matches!(o.ev.kind.as_str(), "login_failed" | "lockout") || now - o.ev.ts > window {
            continue;
        }
        let Some(c) = o.ev.client.as_deref() else {
            continue;
        };
        if allowlisted(&b.allowlist, c) {
            continue;
        }
        *count.entry(c.to_string()).or_insert(0) += 1;
    }
    let mut out = Vec::new();
    for (c, n) in count {
        if n < b.failures {
            continue;
        }
        let key = format!("bf:{c}");
        if fired.get(&key).is_some_and(|t| now - *t < window) {
            continue;
        }
        fired.insert(key, now);
        out.push((c, n));
    }
    out
}

/// What one alert rule found.
#[derive(Debug, PartialEq, Eq)]
pub struct Firing {
    pub rule_id: String,
    pub key: String,
    pub count: u32,
}

/// Rules that cross their threshold and are out of cooldown.
pub fn evaluate_rules(
    rules: &[EventRule],
    obs: &[Obs],
    now: i64,
    allow: &[String],
    fired: &mut HashMap<String, i64>,
) -> Vec<Firing> {
    let mut out = Vec::new();
    for r in rules.iter().filter(|r| r.enabled) {
        let window = r.window_minutes as i64 * 60;
        let mut count: BTreeMap<String, u32> = BTreeMap::new();
        for o in obs {
            if now - o.ev.ts > window || !r.kinds.contains(&o.ev.kind) {
                continue;
            }
            if !r.path_group.is_empty() && o.group != r.path_group {
                continue;
            }
            let key = match r.group_by.as_str() {
                "client" => match o.ev.client.as_deref() {
                    Some(c) if !allowlisted(allow, c) => c.to_string(),
                    _ => continue,
                },
                "user" => match o.ev.user.as_deref() {
                    Some(u) if !u.is_empty() => u.to_string(),
                    _ => continue,
                },
                _ => "(all)".to_string(),
            };
            *count.entry(key).or_insert(0) += 1;
        }
        for (key, n) in count {
            if n < r.threshold {
                continue;
            }
            let k = format!("rule:{}:{key}", r.id);
            if fired
                .get(&k)
                .is_some_and(|t| now - *t < (r.cooldown_minutes as i64 * 60).max(window))
            {
                continue;
            }
            fired.insert(k, now);
            out.push(Firing {
                rule_id: r.id.clone(),
                key,
                count: n,
            });
        }
    }
    out
}

// ------------------------------------------------------------ dashboard (pure)

fn is_login(k: &str) -> bool {
    matches!(k, "login_failed" | "lockout")
}

fn is_denial(k: &str) -> bool {
    matches!(k, "access_denied" | "rate_limited" | "permission_denied")
}

/// Kinds hidden from the event list unless asked for (high volume, low signal).
fn is_noise(k: &str) -> bool {
    matches!(k, "not_found" | "server_error")
}

fn top(m: HashMap<String, TopFailing>, blocked: &[String], clients: bool) -> Vec<TopFailing> {
    let mut v: Vec<TopFailing> = m.into_values().collect();
    for t in &mut v {
        t.total = t.failed_logins + t.access_denied;
        t.blocked = clients && blocked.contains(&t.key);
    }
    v.retain(|t| t.total > 0);
    v.sort_by(|a, b| {
        b.total
            .cmp(&a.total)
            .then_with(|| b.last_seen.cmp(&a.last_seen))
            .then_with(|| a.key.cmp(&b.key))
    });
    v.truncate(10);
    v
}

pub fn build_dashboard(
    obs: &[Obs],
    since: i64,
    until: i64,
    blocks: &[BlockedClient],
) -> ActivityDashboard {
    let bucket = (((until - since) / 48).max(60) / 60 * 60).max(60);
    let mut t = ActivityTotals::default();
    let mut tl: BTreeMap<i64, EventBucket> = BTreeMap::new();
    let mut clients: HashMap<String, TopFailing> = HashMap::new();
    let mut users: HashMap<String, TopFailing> = HashMap::new();
    let mut start = since.div_euclid(bucket) * bucket;
    while start <= until {
        tl.insert(
            start,
            EventBucket {
                start,
                failed_logins: 0,
                access_denied: 0,
                other: 0,
            },
        );
        start += bucket;
    }
    for o in obs.iter().filter(|o| o.ev.ts >= since && o.ev.ts <= until) {
        let k = o.ev.kind.as_str();
        match k {
            "login_failed" => t.failed_logins += 1,
            "lockout" => t.lockouts += 1,
            "access_denied" | "rate_limited" => t.access_denied += 1,
            "permission_denied" => t.permission_denied += 1,
            "brute_force" => t.brute_force += 1,
            "scan_finding" => t.scan_findings += 1,
            "hardening_regression" => t.baseline_drift += 1,
            _ => {}
        }
        if !is_noise(k) {
            if let Some(b) = tl.get_mut(&(o.ev.ts.div_euclid(bucket) * bucket)) {
                if is_login(k) {
                    b.failed_logins += 1;
                } else if is_denial(k) {
                    b.access_denied += 1;
                } else {
                    b.other += 1;
                }
            }
        }
        let bump = |m: &mut HashMap<String, TopFailing>, key: &str| {
            let e = m.entry(key.to_string()).or_insert(TopFailing {
                key: key.to_string(),
                failed_logins: 0,
                access_denied: 0,
                total: 0,
                last_seen: 0,
                blocked: false,
            });
            if is_login(k) {
                e.failed_logins += 1;
            } else if is_denial(k) {
                e.access_denied += 1;
            }
            e.last_seen = e.last_seen.max(o.ev.ts);
        };
        if is_login(k) || is_denial(k) {
            if let Some(c) = o.ev.client.as_deref() {
                bump(&mut clients, c);
            }
            if let Some(u) = o.ev.user.as_deref().filter(|u| !u.is_empty()) {
                bump(&mut users, u);
            }
        }
    }
    t.blocks_active = blocks.len() as u32;
    let blocked: Vec<String> = blocks.iter().map(|b| b.ip.clone()).collect();
    let mut recent: Vec<TimelineEvent> = obs
        .iter()
        .filter(|o| o.ev.ts >= since && o.ev.ts <= until && !is_noise(&o.ev.kind))
        .map(|o| o.ev.clone())
        .collect();
    recent.reverse();
    recent.truncate(15);
    ActivityDashboard {
        since,
        until,
        bucket_secs: bucket,
        totals: t,
        timeline: tl.into_values().collect(),
        top_clients: top(clients, &blocked, true),
        top_users: top(users, &blocked, false),
        recent,
        detector_enabled: false,
        block_enabled: false,
        firewall_available: false,
        access_log_enabled: false,
    }
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct EventQuery {
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub source: Option<String>,
    /// One kind or a comma separated list.
    pub kind: Option<String>,
    pub severity: Option<String>,
    pub client: Option<String>,
    pub user: Option<String>,
    /// Text in the summary or detail.
    pub q: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

fn pick(o: &Option<String>) -> Option<&str> {
    o.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// Matching events, newest first (not yet paged).
pub fn filter_events(obs: &[Obs], q: &EventQuery) -> Vec<TimelineEvent> {
    let kinds: Vec<&str> = pick(&q.kind)
        .map(|k| {
            k.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let text = pick(&q.q).map(str::to_ascii_lowercase);
    let mut v: Vec<TimelineEvent> = obs
        .iter()
        .map(|o| &o.ev)
        .filter(|e| q.since.is_none_or(|t| e.ts >= t))
        .filter(|e| q.until.is_none_or(|t| e.ts <= t))
        .filter(|e| pick(&q.source).is_none_or(|s| e.source == s))
        .filter(|e| {
            if kinds.is_empty() {
                !is_noise(&e.kind)
            } else {
                kinds.contains(&e.kind.as_str())
            }
        })
        .filter(|e| pick(&q.severity).is_none_or(|s| e.severity == s))
        .filter(|e| pick(&q.client).is_none_or(|c| e.client.as_deref() == Some(c)))
        .filter(|e| pick(&q.user).is_none_or(|u| e.user.as_deref() == Some(u)))
        .filter(|e| {
            text.as_ref().is_none_or(|t| {
                e.summary.to_ascii_lowercase().contains(t.as_str())
                    || e.detail
                        .as_deref()
                        .is_some_and(|d| d.to_ascii_lowercase().contains(t.as_str()))
            })
        })
        .cloned()
        .collect();
    v.reverse();
    v
}

// ------------------------------------------------------------ blocks

/// nft script that makes sure the table exists and blocks `ips` for `secs`.
pub fn block_script(table: &str, adds: &[(IpAddr, u64)]) -> String {
    let mut s = format!(
        "add table inet {table}\n\
         add set inet {table} blocked4 {{ type ipv4_addr; flags timeout; }}\n\
         add set inet {table} blocked6 {{ type ipv6_addr; flags timeout; }}\n\
         add chain inet {table} input {{ type filter hook input priority -150; policy accept; }}\n\
         flush chain inet {table} input\n\
         add rule inet {table} input ip saddr @blocked4 drop\n\
         add rule inet {table} input ip6 saddr @blocked6 drop\n"
    );
    for (ip, secs) in adds {
        let set = if ip.is_ipv4() { "blocked4" } else { "blocked6" };
        s.push_str(&format!(
            "add element inet {table} {set} {{ {ip} timeout {secs}s }}\n"
        ));
    }
    s
}

async fn nft(script: &str) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let bin = which("nft").ok_or("nftables (nft) is not installed on this node")?;
    let mut child = tokio::process::Command::new(bin)
        .args(["-f", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("nft: {e}"))?;
    if let Some(mut si) = child.stdin.take() {
        si.write_all(script.as_bytes())
            .await
            .map_err(|e| format!("nft: {e}"))?;
    }
    let out = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
        .await
        .map_err(|_| "nft timed out".to_string())?
        .map_err(|e| format!("nft: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "nft: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn blocks_path() -> std::path::PathBuf {
    data_dir().join("security-blocks.json")
}

fn blocks_lock() -> &'static tokio::sync::Mutex<()> {
    static L: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    L.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn blocks_load() -> Vec<BlockedClient> {
    std::fs::read_to_string(blocks_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn blocks_save(v: &[BlockedClient]) {
    let tmp = blocks_path().with_extension("json.tmp");
    if let Ok(t) = serde_json::to_string_pretty(v) {
        if std::fs::write(&tmp, t).is_ok() {
            let _ = std::fs::rename(&tmp, blocks_path());
        }
    }
}

/// The address the node itself owns (binding to it works only then).
fn is_local_address(ip: IpAddr) -> bool {
    std::net::UdpSocket::bind((ip, 0)).is_ok()
}

/// Why `ip` must never be blocked, if so.
pub fn refuse_reason(
    s: &DetectionSettings,
    ip: IpAddr,
    own_peer: Option<IpAddr>,
) -> Option<&'static str> {
    let ip = normalize_ip(ip);
    if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
        return Some("loopback, unspecified and multicast addresses are never blocked");
    }
    if allowlisted(&s.brute_force.allowlist, &ip.to_string()) {
        return Some("the address is on the allowlist");
    }
    if own_peer.is_some_and(|p| normalize_ip(p) == ip) {
        return Some("that is the address you are connected from");
    }
    if is_local_address(ip) {
        return Some("that is one of this node's own addresses");
    }
    None
}

/// Live blocks (expired ones are dropped and journaled).
pub async fn list_blocks(state: &AppState) -> Vec<BlockedClient> {
    let _g = blocks_lock().lock().await;
    let all = blocks_load();
    let t = now();
    let (live, gone): (Vec<_>, Vec<_>) = all.into_iter().partition(|b| b.until > t);
    if !gone.is_empty() {
        blocks_save(&live);
        for b in gone {
            journal(
                state,
                ev(
                    "detector",
                    "unblocked",
                    "info",
                    Some(b.ip.clone()),
                    None,
                    format!("Block on {} expired", b.ip),
                    None,
                ),
            )
            .await;
        }
    }
    live
}

pub async fn add_block(
    state: &AppState,
    ip: IpAddr,
    minutes: u32,
    reason: &str,
    by: &str,
    own_peer: Option<IpAddr>,
) -> Result<BlockedClient, ApiError> {
    let s = load_settings(state).await.detection;
    let ip = normalize_ip(ip);
    if let Some(why) = refuse_reason(&s, ip, own_peer) {
        return Err(ApiError::BadRequest(format!("not blocked: {why}")));
    }
    let minutes = if minutes == 0 {
        s.brute_force.block_minutes
    } else {
        minutes
    };
    if !(1..=10_080).contains(&minutes) {
        return Err(ApiError::BadRequest("minutes: 1 to 10080".into()));
    }
    let secs = minutes as u64 * 60;
    nft(&block_script(&s.block_table, &[(ip, secs)]))
        .await
        .map_err(|e| ApiError::NotAvailable {
            reason: "firewall-missing",
            message: format!("The client could not be blocked: {e}"),
            hint:
                "Install nftables (Gentoo: emerge net-firewall/nftables) and run Horcrux as root."
                    .into(),
        })?;
    let _g = blocks_lock().lock().await;
    let t = now();
    let rec = BlockedClient {
        ip: ip.to_string(),
        since: t,
        until: t + secs as i64,
        reason: reason.chars().take(200).collect(),
        by: by.into(),
    };
    let mut v: Vec<BlockedClient> = blocks_load()
        .into_iter()
        .filter(|b| b.ip != rec.ip)
        .collect();
    v.push(rec.clone());
    blocks_save(&v);
    drop(_g);
    journal(
        state,
        ev(
            "detector",
            "blocked",
            "warning",
            Some(rec.ip.clone()),
            None,
            format!("Blocked {} for {minutes} min ({by})", rec.ip),
            Some(rec.reason.clone()),
        ),
    )
    .await;
    Ok(rec)
}

pub async fn remove_block(state: &AppState, ip: IpAddr, by: &str) -> Result<bool, ApiError> {
    let s = load_settings(state).await.detection;
    let ip = normalize_ip(ip);
    let set = if ip.is_ipv4() { "blocked4" } else { "blocked6" };
    // The element may already be gone (timeout) or the table never created.
    let _ = nft(&format!(
        "delete element inet {} {set} {{ {ip} }}\n",
        s.block_table
    ))
    .await;
    let _g = blocks_lock().lock().await;
    let all = blocks_load();
    let had = all.iter().any(|b| b.ip == ip.to_string());
    let rest: Vec<_> = all.into_iter().filter(|b| b.ip != ip.to_string()).collect();
    blocks_save(&rest);
    drop(_g);
    if had {
        journal(
            state,
            ev(
                "detector",
                "unblocked",
                "info",
                Some(ip.to_string()),
                None,
                format!("Unblocked {ip} ({by})"),
                None,
            ),
        )
        .await;
    }
    Ok(had)
}

/// After a restart the kernel set is empty: put the live blocks back.
async fn reapply_blocks(state: &AppState) {
    let s = load_settings(state).await.detection;
    let live = list_blocks(state).await;
    if live.is_empty() {
        return;
    }
    let t = now();
    let adds: Vec<(IpAddr, u64)> = live
        .iter()
        .filter_map(|b| Some((b.ip.parse().ok()?, (b.until - t).max(1) as u64)))
        .collect();
    if let Err(e) = nft(&block_script(&s.block_table, &adds)).await {
        tracing::warn!("security blocks: could not re-apply: {e}");
    }
}

fn why(e: &ApiError) -> String {
    match e {
        ApiError::BadRequest(m) | ApiError::NotFound(m) => m.clone(),
        ApiError::NotAvailable { message, .. } => message.clone(),
        other => format!("{other:?}"),
    }
}

// ------------------------------------------------------------ detector loop

fn fired() -> &'static Mutex<HashMap<String, i64>> {
    static F: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
    F.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn spawn_detector(state: Arc<AppState>) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        reapply_blocks(&state).await;
        loop {
            let s = load_settings(&state).await.detection;
            if s.enabled {
                detect_pass(&state, &s).await;
            }
            tokio::time::sleep(Duration::from_secs(s.check_secs.max(10) as u64)).await;
        }
    });
}

/// What one pass found (`POST /api/security/activity/check` returns it).
#[derive(Debug, Default, serde::Serialize)]
pub struct PassResult {
    /// Clients the brute-force detector fired for.
    pub brute_force: Vec<String>,
    /// `rule id:key` of every alert rule that fired.
    pub rules: Vec<String>,
    pub blocked: Vec<String>,
}

pub async fn detect_pass(state: &AppState, s: &DetectionSettings) -> PassResult {
    let t = now();
    let mut res = PassResult::default();
    let longest = s
        .rules
        .iter()
        .filter(|r| r.enabled)
        .map(|r| r.window_minutes)
        .chain(std::iter::once(s.brute_force.window_minutes))
        .max()
        .unwrap_or(10) as i64
        * 60;
    let obs = collect(state, t - longest).await;
    let (hits, firings) = {
        let mut f = fired().lock().unwrap_or_else(|e| e.into_inner());
        f.retain(|_, ts| t - *ts < 7 * 86_400);
        let hits = detect_brute_force(s, &obs, t, &mut f);
        let firings = evaluate_rules(&s.rules, &obs, t, &s.brute_force.allowlist, &mut f);
        (hits, firings)
    };
    for (client, n) in hits {
        let b = &s.brute_force;
        let title = format!(
            "Brute force on {}: {n} failed logins from {client}",
            node_name()
        );
        let mut body = format!(
            "{n} failed logins from {client} within {} minutes (limit {}).",
            b.window_minutes, b.failures
        );
        if b.block {
            match client.parse::<IpAddr>() {
                Ok(ip) => match add_block(
                    state,
                    ip,
                    b.block_minutes,
                    &format!("{n} failed logins in {} min", b.window_minutes),
                    "brute-force detector",
                    None,
                )
                .await
                {
                    Ok(_) => {
                        res.blocked.push(client.clone());
                        body.push_str(&format!(
                            " The client is blocked in the host firewall for {} minutes.",
                            b.block_minutes
                        ));
                    }
                    Err(e) => body.push_str(&format!(" It was not blocked: {}.", why(&e))),
                },
                Err(_) => body.push_str(" It was not blocked: the client is not an IP address."),
            }
        }
        journal(
            state,
            ev(
                "detector",
                "brute_force",
                "critical",
                Some(client.clone()),
                None,
                title.clone(),
                Some(body.clone()),
            ),
        )
        .await;
        if b.notify {
            body.push_str(" Open Security > Activity.");
            alert(
                state,
                AlertInfo {
                    severity: "critical",
                    rule_id: "brute-force",
                    rule_name: "Brute force login",
                    target: &client,
                    value: n as f64,
                    threshold: b.failures as f64,
                },
                title,
                body,
            )
            .await;
        }
        res.brute_force.push(client);
    }
    for f in firings {
        let Some(r) = s.rules.iter().find(|r| r.id == f.rule_id) else {
            continue;
        };
        let title = format!("{}: {} ({})", r.name, f.key, f.count);
        let body = format!(
            "{} event(s) of kind {} for {} within {} minutes (limit {}). Open Security > Activity.",
            f.count,
            r.kinds.join(", "),
            f.key,
            r.window_minutes,
            r.threshold
        );
        journal(
            state,
            ev(
                "detector",
                "rule_fired",
                &r.severity,
                (r.group_by == "client").then(|| f.key.clone()),
                (r.group_by == "user").then(|| f.key.clone()),
                title.clone(),
                Some(body.clone()),
            ),
        )
        .await;
        alert(
            state,
            AlertInfo {
                severity: &r.severity,
                rule_id: &r.id,
                rule_name: &r.name,
                target: &f.key,
                value: f.count as f64,
                threshold: r.threshold as f64,
            },
            title,
            body,
        )
        .await;
        res.rules.push(format!("{}:{}", r.id, f.key));
    }
    res
}

// ------------------------------------------------------------ handlers

#[derive(Deserialize)]
pub struct DashQ {
    pub hours: Option<i64>,
}

pub async fn dashboard(
    State(state): State<Arc<AppState>>,
    Query(q): Query<DashQ>,
) -> Json<ActivityDashboard> {
    let s = load_settings(&state).await.detection;
    let hours = q.hours.unwrap_or(24).clamp(1, MAX_LOOKBACK_HOURS);
    let until = now();
    let since = until - hours * 3600;
    let obs = collect(&state, since).await;
    let blocks = list_blocks(&state).await;
    let mut d = build_dashboard(&obs, since, until, &blocks);
    d.detector_enabled = s.enabled;
    d.block_enabled = s.brute_force.block;
    d.firewall_available = which("nft").is_some();
    d.access_log_enabled = crate::api::access_log::global().is_some_and(|l| l.is_enabled());
    Json(d)
}

pub async fn list_events(
    State(state): State<Arc<AppState>>,
    Query(q): Query<EventQuery>,
) -> Json<TimelinePage> {
    let since = q
        .since
        .unwrap_or(now() - 24 * 3600)
        .max(now() - MAX_LOOKBACK_HOURS * 3600);
    let obs = collect(&state, since).await;
    let v = filter_events(&obs, &q);
    let total = v.len();
    let events = v
        .into_iter()
        .skip(q.offset.unwrap_or(0))
        .take(q.limit.unwrap_or(100).clamp(1, 1000))
        .collect();
    Json(TimelinePage { total, events })
}

/// Run the detector now (it also runs every `detection.check_secs`).
pub async fn check_now(State(state): State<Arc<AppState>>) -> Json<PassResult> {
    let s = load_settings(&state).await.detection;
    Json(detect_pass(&state, &s).await)
}

pub async fn blocks(State(state): State<Arc<AppState>>) -> Json<Vec<BlockedClient>> {
    Json(list_blocks(&state).await)
}

pub async fn create_block(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::middleware::auth::AuthUser>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(req): Json<BlockRequest>,
) -> Result<Json<BlockedClient>, ApiError> {
    let ip: IpAddr = req
        .ip
        .trim()
        .parse()
        .map_err(|_| ApiError::BadRequest("ip: an IPv4 or IPv6 address".into()))?;
    let by = super::caller(&user);
    add_block(&state, ip, req.minutes, &req.reason, &by, Some(peer.ip()))
        .await
        .map(Json)
}

#[derive(Deserialize)]
pub struct UnblockQ {
    pub ip: String,
}

pub async fn delete_block(
    State(state): State<Arc<AppState>>,
    user: Option<axum::Extension<crate::middleware::auth::AuthUser>>,
    Query(q): Query<UnblockQ>,
) -> Result<axum::http::StatusCode, ApiError> {
    let ip: IpAddr =
        q.ip.trim()
            .parse()
            .map_err(|_| ApiError::BadRequest("ip: an IPv4 or IPv6 address".into()))?;
    let by = super::caller(&user);
    if remove_block(&state, ip, &by).await? {
        Ok(axum::http::StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(format!("{ip} is not blocked")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ob(ts: i64, kind: &str, client: &str, user: &str, group: &str) -> Obs {
        Obs {
            ev: TimelineEvent {
                ts,
                source: "audit".into(),
                kind: kind.into(),
                severity: "warning".into(),
                client: (!client.is_empty()).then(|| client.to_string()),
                user: (!user.is_empty()).then(|| user.to_string()),
                summary: format!("{kind} {user}"),
                detail: None,
            },
            group: group.into(),
        }
    }

    fn settings() -> DetectionSettings {
        DetectionSettings::default()
    }

    #[test]
    fn brute_force_needs_n_failures_in_m_minutes_from_one_client() {
        let s = settings(); // 5 in 10 min
        let t = 10_000;
        let mut obs: Vec<Obs> = (0..4)
            .map(|i| ob(t - i * 30, "login_failed", "203.0.113.9", "root", ""))
            .collect();
        let mut fired = HashMap::new();
        assert!(
            detect_brute_force(&s, &obs, t, &mut fired).is_empty(),
            "4 < 5"
        );
        obs.push(ob(t - 100, "lockout", "203.0.113.9", "root", ""));
        let hits = detect_brute_force(&s, &obs, t, &mut fired);
        assert_eq!(hits, vec![("203.0.113.9".to_string(), 5)]);
        // same window: no second firing
        assert!(detect_brute_force(&s, &obs, t + 60, &mut fired).is_empty());
        // spread over two clients: neither reaches 5
        let mixed: Vec<Obs> = (0..6)
            .map(|i| {
                ob(
                    t - i,
                    "login_failed",
                    if i % 2 == 0 {
                        "198.51.100.1"
                    } else {
                        "198.51.100.2"
                    },
                    "a",
                    "",
                )
            })
            .collect();
        assert!(detect_brute_force(&s, &mixed, t, &mut HashMap::new()).is_empty());
        // too old
        let old: Vec<Obs> = (0..6)
            .map(|i| ob(t - 3600 - i, "login_failed", "198.51.100.3", "a", ""))
            .collect();
        assert!(detect_brute_force(&s, &old, t, &mut HashMap::new()).is_empty());
    }

    #[test]
    fn allowlisted_clients_are_never_counted_and_disabled_means_off() {
        let mut s = settings();
        s.brute_force.allowlist = vec!["10.0.0.0/8".into()];
        let obs: Vec<Obs> = (0..9)
            .map(|i| ob(1000 - i, "login_failed", "10.1.2.3", "a", ""))
            .collect();
        assert!(detect_brute_force(&s, &obs, 1000, &mut HashMap::new()).is_empty());
        s.brute_force.allowlist.clear();
        assert_eq!(
            detect_brute_force(&s, &obs, 1000, &mut HashMap::new()).len(),
            1
        );
        s.brute_force.enabled = false;
        assert!(detect_brute_force(&s, &obs, 1000, &mut HashMap::new()).is_empty());
    }

    #[test]
    fn rules_count_per_client_or_user_filter_group_and_honour_cooldown() {
        let rule = EventRule {
            id: "r1".into(),
            name: "denials".into(),
            kinds: vec!["access_denied".into()],
            path_group: "vms".into(),
            threshold: 3,
            window_minutes: 5,
            cooldown_minutes: 30,
            ..EventRule::default()
        };
        let t = 5000;
        let obs = vec![
            ob(t - 10, "access_denied", "1.1.1.1", "", "vms"),
            ob(t - 20, "access_denied", "1.1.1.1", "", "vms"),
            ob(t - 30, "access_denied", "1.1.1.1", "", "vms"),
            ob(t - 40, "access_denied", "2.2.2.2", "", "vms"),
            ob(t - 50, "access_denied", "2.2.2.2", "", "storage"), // other group
            ob(t - 60, "access_denied", "2.2.2.2", "", "vms"),
        ];
        let mut fired = HashMap::new();
        let f = evaluate_rules(std::slice::from_ref(&rule), &obs, t, &[], &mut fired);
        assert_eq!(
            f,
            vec![Firing {
                rule_id: "r1".into(),
                key: "1.1.1.1".into(),
                count: 3
            }]
        );
        assert!(
            evaluate_rules(std::slice::from_ref(&rule), &obs, t + 120, &[], &mut fired).is_empty(),
            "cooldown"
        );
        assert_eq!(
            evaluate_rules(
                std::slice::from_ref(&rule),
                &obs,
                t + 40 * 60,
                &[],
                &mut fired
            )
            .len(),
            0,
            "events left the window"
        );
        // by user
        let ur = EventRule {
            group_by: "user".into(),
            kinds: vec!["permission_denied".into()],
            path_group: String::new(),
            threshold: 2,
            ..rule.clone()
        };
        let o2 = vec![
            ob(t, "permission_denied", "", "alice", ""),
            ob(t, "permission_denied", "", "alice", ""),
            ob(t, "permission_denied", "", "bob", ""),
        ];
        let f = evaluate_rules(&[ur], &o2, t, &[], &mut HashMap::new());
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].key, "alice");
        // disabled rule
        let off = EventRule {
            enabled: false,
            ..rule
        };
        assert!(evaluate_rules(&[off], &obs, t, &[], &mut HashMap::new()).is_empty());
    }

    #[test]
    fn dashboard_totals_timeline_and_top_lists() {
        let t = 100_000;
        let obs = vec![
            ob(t - 7000, "login_failed", "9.9.9.9", "root", ""),
            ob(t - 3000, "login_failed", "9.9.9.9", "admin", ""),
            ob(t - 2000, "lockout", "9.9.9.9", "admin", ""),
            ob(t - 1500, "access_denied", "8.8.8.8", "bob", "vms"),
            ob(t - 1400, "not_found", "8.8.8.8", "", "x"),
            ob(t - 900, "brute_force", "9.9.9.9", "", ""),
            ob(t - 500, "hardening_regression", "", "", ""),
            ob(t - 100, "scan_finding", "", "", ""),
        ];
        let blocks = vec![BlockedClient {
            ip: "9.9.9.9".into(),
            since: t,
            until: t + 60,
            reason: String::new(),
            by: "x".into(),
        }];
        let d = build_dashboard(&obs, t - 8000, t, &blocks);
        assert_eq!(d.totals.failed_logins, 2);
        assert_eq!(d.totals.lockouts, 1);
        assert_eq!(d.totals.access_denied, 1);
        assert_eq!(d.totals.brute_force, 1);
        assert_eq!(d.totals.baseline_drift, 1);
        assert_eq!(d.totals.scan_findings, 1);
        assert_eq!(d.totals.blocks_active, 1);
        assert_eq!(d.top_clients[0].key, "9.9.9.9");
        assert_eq!(d.top_clients[0].failed_logins, 3);
        assert!(d.top_clients[0].blocked);
        assert_eq!(d.top_clients[1].key, "8.8.8.8");
        assert_eq!(d.top_users[0].key, "admin");
        assert_eq!(d.top_users[0].failed_logins, 2);
        let sum: u32 = d.timeline.iter().map(|b| b.failed_logins).sum();
        assert_eq!(sum, 3);
        assert!(
            d.recent.iter().all(|e| e.kind != "not_found"),
            "noise stays out of the recent list"
        );
        assert_eq!(d.recent[0].kind, "scan_finding", "newest first");
    }

    #[test]
    fn event_filters() {
        let t = 1000;
        let obs = vec![
            ob(t, "login_failed", "1.1.1.1", "root", ""),
            ob(t + 1, "not_found", "1.1.1.1", "", ""),
            ob(t + 2, "access_denied", "2.2.2.2", "bob", ""),
        ];
        let all = filter_events(&obs, &EventQuery::default());
        assert_eq!(all.len(), 2, "not_found hidden by default");
        assert_eq!(all[0].kind, "access_denied");
        let nf = filter_events(
            &obs,
            &EventQuery {
                kind: Some("not_found".into()),
                ..Default::default()
            },
        );
        assert_eq!(nf.len(), 1);
        let c = filter_events(
            &obs,
            &EventQuery {
                client: Some("1.1.1.1".into()),
                ..Default::default()
            },
        );
        assert_eq!(c.len(), 1);
        let q = filter_events(
            &obs,
            &EventQuery {
                q: Some("ROOT".into()),
                ..Default::default()
            },
        );
        assert_eq!(q.len(), 1);
        let two = filter_events(
            &obs,
            &EventQuery {
                kind: Some("login_failed, access_denied".into()),
                ..Default::default()
            },
        );
        assert_eq!(two.len(), 2);
    }

    #[test]
    fn classify_audit_events() {
        use crate::audit::AuditEventType as T;
        assert_eq!(
            classify_audit(&T::LoginFailed, Some("a"), Some("bad password"))
                .unwrap()
                .0,
            "login_failed"
        );
        assert_eq!(
            classify_audit(
                &T::LoginFailed,
                Some("a"),
                Some("too many failures: backing off")
            )
            .unwrap()
            .0,
            "lockout"
        );
        assert_eq!(
            classify_audit(&T::PermissionDenied, Some("a"), None)
                .unwrap()
                .0,
            "permission_denied"
        );
        assert_eq!(
            classify_audit(&T::TwoFactorDisabled, Some("a"), None)
                .unwrap()
                .1,
            "warning"
        );
        assert!(classify_audit(&T::VmStarted, Some("a"), None).is_none());
    }

    #[test]
    fn nft_script_has_table_sets_drop_rules_and_timeouts() {
        let s = block_script(
            "horcrux_secblock",
            &[
                ("203.0.113.9".parse().unwrap(), 1800),
                ("2001:db8::7".parse().unwrap(), 60),
            ],
        );
        assert!(s.contains("add table inet horcrux_secblock\n"));
        assert!(s.contains("hook input priority -150"));
        assert!(s.contains("ip saddr @blocked4 drop"));
        assert!(s.contains("ip6 saddr @blocked6 drop"));
        assert!(
            s.contains("add element inet horcrux_secblock blocked4 { 203.0.113.9 timeout 1800s }")
        );
        assert!(
            s.contains("add element inet horcrux_secblock blocked6 { 2001:db8::7 timeout 60s }")
        );
    }

    #[test]
    fn some_addresses_are_never_blocked() {
        let mut s = settings();
        s.brute_force.allowlist = vec!["192.0.2.0/24".into()];
        assert!(refuse_reason(&s, "127.0.0.1".parse().unwrap(), None).is_some());
        assert!(refuse_reason(&s, "::1".parse().unwrap(), None).is_some());
        assert!(refuse_reason(&s, "192.0.2.50".parse().unwrap(), None).is_some());
        assert!(refuse_reason(
            &s,
            "198.51.100.5".parse().unwrap(),
            Some("198.51.100.5".parse().unwrap())
        )
        .is_some());
        assert!(refuse_reason(
            &s,
            "::ffff:198.51.100.5".parse().unwrap(),
            Some("198.51.100.5".parse().unwrap())
        )
        .is_some());
        assert!(refuse_reason(
            &s,
            "198.51.100.5".parse().unwrap(),
            Some("198.51.100.6".parse().unwrap())
        )
        .is_none());
    }
}
