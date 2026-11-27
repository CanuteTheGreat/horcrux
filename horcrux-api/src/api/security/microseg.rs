//! Guest microsegmentation API (docs/design/security-suite.md, "Guest
//! microsegmentation"; user guide docs/SECURITY-SUITE.md).
//!
//! * `GET  /api/security/microseg/templates`          the profile templates
//! * `GET  /api/security/microseg/guests`             every VM/container with its profile
//! * `GET|PUT|DELETE /api/security/microseg/guests/:key`  one profile (`vm:100`, `container:web1`)
//! * `POST /api/security/microseg/guests/:key/apply`  install it into the kernel again
//! * `GET  /api/security/microseg/guests/:key/rules`  per-rule hit counters
//! * `GET  /api/security/microseg/guests/:key/flows?ip=` tracked flows and recent drops
//!
//! The profiles live in the security settings (`security.microseg`), so
//! the declarative `security:` section manages them too. Reads need
//! Sys.Audit, changes Sys.Modify.
use super::{load_settings, set_settings_warn};
use crate::error::ApiError;
use crate::firewall::{microseg, FirewallScope};
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use horcrux_common::security::{
    valid_guest_key, GuestProfile, MsegDenied, MsegFlowView, MsegGuestRow, MsegPutAnswer,
    MsegRuleHit, MsegTemplateInfo, MSEG_LOG_PREFIX,
};
use serde::Deserialize;
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/security/microseg/templates", get(templates))
        .route("/api/security/microseg/guests", get(list_guests))
        .route(
            "/api/security/microseg/guests/:key",
            get(get_profile).put(put_profile).delete(delete_profile),
        )
        .route("/api/security/microseg/guests/:key/apply", post(apply))
        .route("/api/security/microseg/guests/:key/rules", get(rules))
        .route("/api/security/microseg/guests/:key/flows", get(flows))
}

async fn templates() -> Json<Vec<MsegTemplateInfo>> {
    Json(microseg::template_infos())
}

/// Host-side devices of the macvtap NICs of a VM.
fn vm_macvtap_devices(vm: &horcrux_common::VmConfig) -> Vec<String> {
    vm.hardware
        .nets
        .iter()
        .enumerate()
        .filter(|(_, n)| n.attach == horcrux_common::NicAttach::Macvtap)
        .map(|(i, _)| horcrux_common::vm_tap_name(&vm.id, i))
        .collect()
}

fn scope_of(key: &str) -> Result<FirewallScope, ApiError> {
    if !valid_guest_key(key) {
        return Err(ApiError::BadRequest(
            "the guest key is vm:<id> or container:<id>".into(),
        ));
    }
    FirewallScope::from_key(key).ok_or_else(|| ApiError::BadRequest("bad guest key".into()))
}

async fn list_guests(State(state): State<Arc<AppState>>) -> Json<Vec<MsegGuestRow>> {
    let m = load_settings(&state).await.microseg;
    let mut out = Vec::new();
    let vms = state.vm_manager.list_vms().await;
    let cts = state.container_manager.list_containers().await;
    let mut rows: Vec<(String, String, String, String, bool, Vec<String>)> = Vec::new();
    for v in &vms {
        rows.push((
            format!("vm:{}", v.id),
            "vm".into(),
            v.id.clone(),
            v.name.clone(),
            v.status == horcrux_common::VmStatus::Running,
            vm_macvtap_devices(v),
        ));
    }
    for c in &cts {
        rows.push((
            format!("container:{}", c.id),
            "container".into(),
            c.id.clone(),
            c.name.clone(),
            c.status == horcrux_common::ContainerStatus::Running,
            Vec::new(),
        ));
    }
    for (key, kind, id, name, running, mv) in rows {
        let p = m.profiles.get(&key);
        let mut dropped = 0;
        if p.is_some() && m.enabled {
            if let Some(scope) = FirewallScope::from_key(&key) {
                dropped = state
                    .firewall_manager
                    .microseg_hits(&scope)
                    .await
                    .iter()
                    .filter(|h| h.rule_id.starts_with("default"))
                    .map(|h| h.packets)
                    .sum();
            }
        }
        out.push(MsegGuestRow {
            key,
            kind,
            id,
            name,
            running,
            template: p.map(|p| p.template.id().to_string()),
            profile_enabled: p.map(|p| p.enabled).unwrap_or(false),
            enforced: m.enabled && p.map(|p| p.enabled).unwrap_or(false),
            dropped_packets: dropped,
            macvtap_devices: mv,
        });
    }
    Json(out)
}

async fn get_profile(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
) -> Result<Json<GuestProfile>, ApiError> {
    scope_of(&key)?;
    load_settings(&state)
        .await
        .microseg
        .profiles
        .get(&key)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("{key} has no microsegmentation profile")))
}

/// The group references of a profile must exist (a `+name` IP set, an
/// alias name); addresses and CIDRs pass.
fn check_groups(state: &AppState, p: &GuestProfile) -> Result<(), ApiError> {
    let ipsets: Vec<String> = state
        .firewall_manager
        .list_ipsets()
        .into_iter()
        .map(|i| i.name)
        .collect();
    let aliases: Vec<String> = state
        .firewall_manager
        .list_aliases()
        .into_iter()
        .map(|a| a.name)
        .collect();
    let groups = p
        .allow_from
        .iter()
        .chain(p.allow_to.iter())
        .map(|x| x.group.as_str())
        .chain((!p.lab_network.is_empty()).then_some(p.lab_network.as_str()));
    for g in groups {
        let g = g.trim();
        if let Some(set) = g.strip_prefix('+') {
            if !ipsets.iter().any(|i| i == set) {
                return Err(ApiError::BadRequest(format!(
                    "IP set {set} does not exist (Firewall > IP sets)"
                )));
            }
        } else if !g.contains(['.', ':', '/']) && !aliases.iter().any(|a| a == g) {
            return Err(ApiError::BadRequest(format!(
                "alias {g} does not exist (use an address, a CIDR, +ipset or an alias)"
            )));
        }
    }
    Ok(())
}

async fn put_profile(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
    Json(mut p): Json<GuestProfile>,
) -> Result<Json<MsegPutAnswer>, ApiError> {
    scope_of(&key)?;
    // The guest must exist on this node.
    if let Some(id) = key.strip_prefix("vm:") {
        let vm = state
            .vm_manager
            .list_vms()
            .await
            .into_iter()
            .find(|v| v.id == id)
            .ok_or_else(|| ApiError::NotFound(format!("VM {id} not found")))?;
        if p.macvtap_devices.is_empty() {
            p.macvtap_devices = vm_macvtap_devices(&vm);
        }
    } else if let Some(id) = key.strip_prefix("container:") {
        if !state
            .container_manager
            .list_containers()
            .await
            .iter()
            .any(|c| c.id == id)
        {
            return Err(ApiError::NotFound(format!("container {id} not found")));
        }
        p.macvtap_devices.clear();
    }
    check_groups(&state, &p)?;
    let mut s = load_settings(&state).await;
    s.microseg.profiles.insert(key.clone(), p.clone());
    let warnings = set_settings_warn(&state, s)
        .await
        .map_err(ApiError::BadRequest)?;
    Ok(Json(MsegPutAnswer {
        profile: p,
        warnings,
    }))
}

async fn delete_profile(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
) -> Result<StatusCode, ApiError> {
    scope_of(&key)?;
    let mut s = load_settings(&state).await;
    if s.microseg.profiles.remove(&key).is_none() {
        return Err(ApiError::NotFound(format!(
            "{key} has no microsegmentation profile"
        )));
    }
    set_settings_warn(&state, s)
        .await
        .map_err(ApiError::BadRequest)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn apply(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
) -> Result<Json<Vec<String>>, ApiError> {
    let scope = scope_of(&key)?;
    state
        .firewall_manager
        .set_microseg(load_settings(&state).await.microseg)
        .await;
    let mut warnings = Vec::new();
    if let Err(e) = state.firewall_manager.apply_rules(scope).await {
        warnings.push(e.to_string());
    }
    Ok(Json(warnings))
}

/// Re-install a guest's profile after it started (its tap and macvtap
/// devices exist again). Best effort.
pub async fn on_guest_start(state: &AppState, key: &str) {
    let m = load_settings(state).await.microseg;
    if !m.enabled || !m.profiles.contains_key(key) {
        return;
    }
    if let Some(scope) = FirewallScope::from_key(key) {
        if let Err(e) = state.firewall_manager.apply_rules(scope).await {
            tracing::warn!("microsegmentation of {key} not re-applied after start: {e}");
        }
    }
}

/// Forget the profile of a deleted guest (a new guest with the same id
/// must not inherit it).
pub async fn forget_guest(state: &AppState, key: &str) {
    let mut s = load_settings(state).await;
    if s.microseg.profiles.remove(key).is_some() {
        if let Err(e) = super::set_settings(state, s).await {
            tracing::warn!("profile of {key} not removed: {e}");
        }
    }
}

async fn rules(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
) -> Result<Json<Vec<MsegRuleHit>>, ApiError> {
    let scope = scope_of(&key)?;
    let mut hits = state.firewall_manager.microseg_hits(&scope).await;
    // Name each rule by what it does, from the guest's stored rules and
    // the plan (the nft comment only carries the id).
    let m = load_settings(&state).await.microseg;
    let mut names: std::collections::HashMap<String, String> = Default::default();
    for r in state.firewall_manager.list_rules(scope.clone()).await {
        names.insert(r.id.clone(), r.comment.clone().unwrap_or_default());
    }
    if let Some(p) = m.profiles.get(&key) {
        for r in microseg::plan(p, &m).rules {
            names.insert(r.id.clone(), r.comment.clone().unwrap_or_default());
        }
    }
    for (id, label) in [
        ("established", "reply traffic of allowed flows"),
        ("invalid", "invalid packets"),
        ("arp", "ARP"),
        ("nd", "IPv6 neighbour discovery"),
        ("dhcp", "DHCP"),
        ("default", "default deny"),
        ("tcp-replies", "TCP replies (macvtap, stateless)"),
        ("icmp-replies", "ICMP replies (macvtap, stateless)"),
    ] {
        names.insert(id.to_string(), label.to_string());
    }
    for h in &mut hits {
        let base = h.rule_id.split(' ').next().unwrap_or("").to_string();
        let base = base.strip_suffix("-reply").unwrap_or(&base).to_string();
        h.rule = names.get(&base).cloned().unwrap_or_default();
    }
    Ok(Json(hits))
}

#[derive(Deserialize, Default)]
struct FlowQ {
    ip: Option<String>,
}

async fn kernel_log() -> Result<String, String> {
    let mut last = String::new();
    for (cmd, args) in [
        (
            "journalctl",
            vec!["-k", "--no-pager", "-o", "short-iso", "-n", "20000"],
        ),
        ("dmesg", vec!["--time-format", "iso"]),
    ] {
        match tokio::process::Command::new(cmd).args(&args).output().await {
            Ok(o) if o.status.success() => {
                return Ok(String::from_utf8_lossy(&o.stdout).to_string())
            }
            Ok(o) => last = String::from_utf8_lossy(&o.stderr).trim().to_string(),
            Err(e) => last = format!("{cmd}: {e}"),
        }
    }
    Err(last)
}

/// Drops written by the default-deny rule of `key`, newest first.
pub fn parse_denied(log: &str, key: &str, limit: usize) -> Vec<MsegDenied> {
    let tag = format!("{MSEG_LOG_PREFIX}{key}/");
    let mut v: Vec<MsegDenied> = log
        .lines()
        .filter_map(|line| {
            let at = line.find(&tag)?;
            let fields = &line[at + tag.len()..];
            let get = |k: &str| {
                fields
                    .split_whitespace()
                    .find_map(|f| f.strip_prefix(k))
                    .unwrap_or("")
                    .to_string()
            };
            let pre = line[..at].trim();
            let time = match pre.strip_prefix('[') {
                Some(rest) => rest.split(']').next().unwrap_or("").trim().to_string(),
                None => pre.split_whitespace().next().unwrap_or("").to_string(),
            };
            Some(MsegDenied {
                time,
                iface_in: get("IN="),
                iface_out: get("OUT="),
                src: get("SRC="),
                dst: get("DST="),
                proto: get("PROTO="),
                sport: get("SPT="),
                dport: get("DPT="),
            })
        })
        .collect();
    let skip = v.len().saturating_sub(limit);
    v.drain(..skip);
    v.reverse();
    v
}

async fn flows(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
    Query(q): Query<FlowQ>,
) -> Result<Json<MsegFlowView>, ApiError> {
    scope_of(&key)?;
    let m = load_settings(&state).await.microseg;
    let limit = m.flow_limit as usize;
    let ip = q.ip.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if let Some(ip) = ip {
        if !ip
            .chars()
            .all(|c| c.is_ascii_hexdigit() || matches!(c, '.' | ':'))
        {
            return Err(ApiError::BadRequest("ip: an address".into()));
        }
    }
    let mut view = MsegFlowView::default();
    // /proc/net/nf_conntrack where the kernel has it, else conntrack(8).
    let table = match std::fs::read_to_string("/proc/net/nf_conntrack") {
        Ok(t) => Some(t),
        Err(_) => match tokio::process::Command::new("conntrack")
            .args(["-L", "-o", "extended"])
            .output()
            .await
        {
            Ok(o) if o.status.success() => Some(String::from_utf8_lossy(&o.stdout).to_string()),
            _ => None,
        },
    };
    match table {
        Some(t) => view.flows = microseg::parse_conntrack(&t, ip, limit),
        None => view.notes.push(
            "Allowed flows are not listed: the kernel has no /proc/net/nf_conntrack and conntrack(8) (net-firewall/conntrack-tools) is not installed or connection tracking is not loaded yet."
                .into(),
        ),
    }
    match kernel_log().await {
        Ok(t) => view.denied = parse_denied(&t, &key, limit),
        Err(e) => view.notes.push(format!(
            "The kernel log is not readable here ({e}), so dropped flows are not listed."
        )),
    }
    if !m.log_denied {
        view.notes
            .push("Logging of dropped flows is off (microseg.log_denied).".into());
    }
    Ok(Json(view))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denied_lines_are_filtered_by_guest_and_parsed() {
        let log = "2026-11-27T10:00:01-0800 h kernel: hx-mseg: vm:100/out IN=tap1 OUT= SRC=10.0.0.5 DST=1.1.1.1 LEN=60 PROTO=TCP SPT=40000 DPT=443\n2026-11-27T10:00:02-0800 h kernel: hx-mseg: vm:101/in IN= OUT=tap2 SRC=10.0.0.9 DST=10.0.0.6 PROTO=UDP SPT=1 DPT=53\n2026-11-27T10:00:03-0800 h kernel: hx-mseg: vm:100/in IN=br0 OUT=tap1 SRC=9.9.9.9 DST=10.0.0.5 PROTO=TCP SPT=1 DPT=22\n";
        let d = parse_denied(log, "vm:100", 10);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].dport, "22"); // newest first
        assert_eq!(d[1].dst, "1.1.1.1");
        assert!(parse_denied(log, "vm:10", 10).is_empty());
        assert_eq!(parse_denied(log, "vm:100", 1).len(), 1);
    }

    #[test]
    fn guest_keys_are_checked() {
        assert!(scope_of("vm:100").is_ok());
        assert!(scope_of("container:web-1").is_ok());
        assert!(scope_of("node:x").is_err());
        assert!(scope_of("vm:a b").is_err());
        assert!(scope_of("vm:").is_err());
    }
}
