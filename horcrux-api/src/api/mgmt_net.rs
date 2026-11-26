//! Management link of the node (Infrastructure > Network > Management link):
//! the NICs, bond (link aggregation, incl. 802.3ad LACP) and VLAN tag that
//! carry the node's own address, web UI and SSH. The installer writes the
//! layout for the init system's network stack (netifrc on OpenRC,
//! systemd-networkd on systemd, NetworkManager on desktop installs; dhcpcd
//! keeps the address) and records it in `[management_network] record_path`.
//!
//! - `GET /api/network/management`  the recorded layout plus the live state
//!   (bond mode, each member's link, LACP partner, addresses)
//! - `PUT /api/network/management`  `{ "record": {...}, "apply_now": false }`:
//!   validate, then the node tool (`tool_path --mgmt-apply`) rewrites the
//!   config for the running init. Without `apply_now` the change takes effect
//!   at the next boot, so a wrong layout never cuts the node off mid-request.
//!
//! A node installed before the record existed answers 424
//! (`management-link-not-recorded`) with what to do.
use crate::error::ApiError;
use crate::AppState;
use axum::{extract::State, routing::get, Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

pub const BOND_MODES: &[&str] = &[
    "active-backup",
    "802.3ad",
    "balance-alb",
    "balance-tlb",
    "balance-xor",
    "balance-rr",
    "broadcast",
];
pub const HASH_POLICIES: &[&str] = &[
    "layer2",
    "layer2+3",
    "layer3+4",
    "encap2+3",
    "encap3+4",
    "vlan+srcmac",
];
const HASH_MODES: &[&str] = &["802.3ad", "balance-xor", "balance-tlb"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BondSpec {
    pub name: String,
    pub mode: String,
    pub miimon: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xmit_hash_policy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lacp_rate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<String>,
}

/// The layout as the installer / console menu records it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MgmtRecord {
    #[serde(default = "one")]
    pub version: u32,
    pub interfaces: Vec<String>,
    #[serde(default)]
    pub bond: Option<BondSpec>,
    /// 802.1Q VLAN id on top of the NIC/bond; none = untagged.
    #[serde(default)]
    pub vlan: Option<u16>,
    /// The interface that carries the address (bond0.42, bond0, eth0 ...).
    #[serde(default)]
    pub address_interface: String,
    /// dhcp | static
    #[serde(default = "dhcp")]
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dns: Vec<String>,
    /// Who configures the address: dhcpcd (with netifrc / networkd for
    /// layer 2) or networkmanager.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
}

fn one() -> u32 {
    1
}
fn dhcp() -> String {
    "dhcp".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct MemberState {
    pub name: String,
    /// MII status from the bond (up/down), or the NIC's operstate.
    pub link: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregator_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct LiveState {
    /// Bonding driver's mode text (e.g. "IEEE 802.3ad Dynamic link aggregation").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bond_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_member: Option<String>,
    /// LACP partner system MAC (802.3ad; all zero = no partner answered).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lacp_partner: Option<String>,
    pub members: Vec<MemberState>,
    pub addresses: Vec<String>,
    pub address_interface_up: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MgmtView {
    pub record: MgmtRecord,
    pub live: LiveState,
    pub bond_modes: Vec<String>,
    pub hash_policies: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MgmtUpdate {
    pub record: MgmtRecord,
    /// Apply on the running system now (the session may drop if the new
    /// link does not come up); default: written for the next boot.
    #[serde(default)]
    pub apply_now: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MgmtUpdateResult {
    pub record: MgmtRecord,
    pub applied: bool,
    /// What the node tool reported (files written per init system).
    pub log: Vec<String>,
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/api/network/management", get(show).put(update))
}

/// Host-independent checks (the node tool re-checks against the real NICs).
pub fn validate(r: &MgmtRecord) -> Result<(), String> {
    if r.interfaces.is_empty() {
        return Err("interfaces: at least one NIC".into());
    }
    for i in &r.interfaces {
        if !horcrux_common::valid_ifname(i) {
            return Err(format!("{i:?} is not a valid interface name"));
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    if !r.interfaces.iter().all(|i| seen.insert(i)) {
        return Err("interfaces: the same NIC is listed twice".into());
    }
    if let Some(v) = r.vlan {
        if !(1..=4094).contains(&v) {
            return Err(format!("vlan {v}: a VLAN id is 1-4094 (none = untagged)"));
        }
    }
    match (&r.bond, r.interfaces.len() > 1) {
        (None, true) => return Err("more than one NIC needs a bond (mode, miimon)".into()),
        (Some(b), true) => {
            if !horcrux_common::valid_ifname(&b.name) || r.interfaces.contains(&b.name) {
                return Err(format!("bond name {:?} is not usable", b.name));
            }
            if !BOND_MODES.contains(&b.mode.as_str()) {
                return Err(format!(
                    "bond mode {:?}: use one of {}",
                    b.mode,
                    BOND_MODES.join(", ")
                ));
            }
            if !(1..=60000).contains(&b.miimon) {
                return Err(format!("bond miimon {}: 1-60000 ms", b.miimon));
            }
            if let Some(h) = &b.xmit_hash_policy {
                if HASH_MODES.contains(&b.mode.as_str()) && !HASH_POLICIES.contains(&h.as_str()) {
                    return Err(format!("bond xmit_hash_policy {h:?}"));
                }
            }
            if let Some(l) = &b.lacp_rate {
                if b.mode == "802.3ad" && l != "slow" && l != "fast" {
                    return Err(format!("bond lacp_rate {l:?}: slow or fast"));
                }
            }
            if let Some(p) = &b.primary {
                if !r.interfaces.contains(p) {
                    return Err(format!("bond primary {p:?} is not a member"));
                }
            }
        }
        _ => {}
    }
    match r.mode.as_str() {
        "dhcp" => {}
        "static" => {
            let a = r.address.as_deref().unwrap_or("");
            let ok = a
                .split_once('/')
                .map(|(ip, len)| {
                    ip.parse::<std::net::IpAddr>().is_ok() && len.parse::<u8>().is_ok()
                })
                .unwrap_or(false);
            if !ok {
                return Err(format!(
                    "address {a:?}: an IP with a prefix length, e.g. 192.0.2.10/24"
                ));
            }
        }
        m => return Err(format!("mode {m:?}: dhcp or static")),
    }
    Ok(())
}

/// Parse `/proc/net/bonding/<bond>`.
pub fn parse_bonding(text: &str) -> LiveState {
    let mut st = LiveState::default();
    let mut cur: Option<MemberState> = None;
    let mut in_partner_block = false;
    for line in text.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("Bonding Mode: ") {
            st.bond_mode = Some(v.to_string());
        } else if let Some(v) = line.strip_prefix("Currently Active Slave: ") {
            st.active_member = Some(v.to_string());
        } else if let Some(v) = line.strip_prefix("Slave Interface: ") {
            if let Some(m) = cur.take() {
                st.members.push(m);
            }
            cur = Some(MemberState {
                name: v.to_string(),
                ..Default::default()
            });
            in_partner_block = false;
        } else if let Some(v) = line.strip_prefix("MII Status: ") {
            if let Some(m) = cur.as_mut() {
                m.link = v.to_string();
            }
        } else if let Some(v) = line.strip_prefix("Aggregator ID: ") {
            if let Some(m) = cur.as_mut() {
                m.aggregator_id = Some(v.to_string());
            }
        } else if line.starts_with("802.3ad info") {
            in_partner_block = true;
        } else if let Some(v) = line.strip_prefix("Partner Mac Address: ") {
            if in_partner_block && st.lacp_partner.is_none() {
                st.lacp_partner = Some(v.to_string());
            }
        }
    }
    if let Some(m) = cur.take() {
        st.members.push(m);
    }
    st
}

fn not_recorded() -> ApiError {
    ApiError::NotAvailable {
        reason: "management-link-not-recorded",
        message: "This node has no recorded management link (installed before bonds and VLANs could be set up by the installer)".into(),
        hint: "Set it once with the console menu (option 12, Management link: bond / VLAN); the page then shows and edits it".into(),
    }
}

async fn read_record(state: &AppState) -> Result<MgmtRecord, ApiError> {
    let p = &state.config.management_network.record_path;
    match tokio::fs::read_to_string(p).await {
        Ok(t) => serde_json::from_str(&t)
            .map_err(|e| ApiError::Internal(format!("{}: {e}", p.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(not_recorded()),
        Err(e) => Err(ApiError::Internal(format!("{}: {e}", p.display()))),
    }
}

async fn live(r: &MgmtRecord) -> LiveState {
    let mut st = match &r.bond {
        Some(b) if r.interfaces.len() > 1 => parse_bonding(
            &tokio::fs::read_to_string(format!("/proc/net/bonding/{}", b.name))
                .await
                .unwrap_or_default(),
        ),
        _ => LiveState::default(),
    };
    if st.members.is_empty() {
        for i in &r.interfaces {
            let link = std::fs::read_to_string(format!("/sys/class/net/{i}/operstate"))
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "missing".into());
            st.members.push(MemberState {
                name: i.clone(),
                link,
                aggregator_id: None,
            });
        }
    }
    st.address_interface_up =
        std::fs::read_to_string(format!("/sys/class/net/{}/operstate", r.address_interface))
            .map(|s| s.trim() == "up")
            .unwrap_or(false);
    st.addresses = super::host_net::addresses(&r.address_interface).await;
    st
}

async fn show(State(state): State<Arc<AppState>>) -> Result<Json<MgmtView>, ApiError> {
    let record = read_record(&state).await?;
    let live = live(&record).await;
    Ok(Json(MgmtView {
        record,
        live,
        bond_modes: BOND_MODES.iter().map(|s| s.to_string()).collect(),
        hash_policies: HASH_POLICIES.iter().map(|s| s.to_string()).collect(),
    }))
}

/// Run the node tool with `record` on stdin; Ok(result json) or the refusal.
pub async fn run_tool(
    cfg: &crate::config::ManagementNetworkConfig,
    record: &MgmtRecord,
    apply_now: bool,
) -> Result<serde_json::Value, ApiError> {
    if !cfg.tool_path.exists() {
        return Err(ApiError::NotAvailable {
            reason: "management-link-tool-missing",
            message: format!("The management link tool {} is not on this node", cfg.tool_path.display()),
            hint: "It ships with the Horcrux installer image (console-menu); set [management_network] tool_path if it lives elsewhere".into(),
        });
    }
    let mut cmd = tokio::process::Command::new(&cfg.tool_path);
    cmd.arg("--mgmt-apply").arg("-");
    if apply_now {
        cmd.arg("--live");
    }
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| ApiError::Internal(format!("{}: {e}", cfg.tool_path.display())))?;
    let body = serde_json::to_vec(record).map_err(|e| ApiError::Internal(e.to_string()))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&body)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
    }
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(cfg.tool_timeout_secs),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| {
        ApiError::Internal(format!(
            "management link tool took longer than {} s",
            cfg.tool_timeout_secs
        ))
    })?
    .map_err(|e| ApiError::Internal(e.to_string()))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = stdout
        .lines()
        .rev()
        .find_map(|l| serde_json::from_str(l).ok())
        .unwrap_or(serde_json::Value::Null);
    if out.status.code() == Some(2) {
        let errs: Vec<String> = v["errors"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        return Err(ApiError::ValidationError(errs.join("; ")));
    }
    if !out.status.success() {
        return Err(ApiError::Internal(format!(
            "management link tool failed: {}",
            String::from_utf8_lossy(&out.stderr)
                .trim()
                .chars()
                .rev()
                .take(300)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>()
        )));
    }
    let mut v = v;
    v["log"] = serde_json::Value::Array(
        String::from_utf8_lossy(&out.stderr)
            .lines()
            .map(|l| serde_json::Value::String(l.trim().to_string()))
            .filter(|l| !l.as_str().unwrap_or("").is_empty())
            .collect(),
    );
    Ok(v)
}

async fn update(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MgmtUpdate>,
) -> Result<Json<MgmtUpdateResult>, ApiError> {
    validate(&req.record).map_err(ApiError::ValidationError)?;
    let v = run_tool(&state.config.management_network, &req.record, req.apply_now).await?;
    let record = serde_json::from_value(v["record"].clone()).unwrap_or(req.record);
    let log = v["log"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    Ok(Json(MgmtUpdateResult {
        record,
        applied: v["applied"].as_bool().unwrap_or(false),
        log,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lacp42() -> MgmtRecord {
        serde_json::from_str(
            r#"{"version":1,"interfaces":["eth0","eth1"],"vlan":42,"address_interface":"bond0.42","mode":"dhcp",
                "bond":{"name":"bond0","mode":"802.3ad","miimon":100,"xmit_hash_policy":"layer3+4","lacp_rate":"fast"},"stack":"dhcpcd"}"#,
        )
        .unwrap()
    }

    #[test]
    fn installer_record_parses_and_validates() {
        let r = lacp42();
        assert_eq!(r.bond.as_ref().unwrap().mode, "802.3ad");
        assert_eq!(validate(&r), Ok(()));
    }

    #[test]
    fn validation_errors_are_specific() {
        let mut r = lacp42();
        r.vlan = Some(4095);
        assert!(validate(&r).unwrap_err().contains("1-4094"));
        let mut r = lacp42();
        r.bond.as_mut().unwrap().mode = "lacp".into();
        assert!(validate(&r).unwrap_err().contains("bond mode"));
        let mut r = lacp42();
        r.bond = None;
        assert!(validate(&r).unwrap_err().contains("needs a bond"));
        let mut r = lacp42();
        r.interfaces = vec!["eth0".into(), "eth0".into()];
        assert!(validate(&r).unwrap_err().contains("twice"));
        let mut r = lacp42();
        r.mode = "static".into();
        r.address = Some("nope".into());
        assert!(validate(&r).unwrap_err().contains("prefix length"));
        let mut r = lacp42();
        r.bond.as_mut().unwrap().lacp_rate = Some("medium".into());
        assert!(validate(&r).unwrap_err().contains("lacp_rate"));
    }

    #[test]
    fn parses_proc_bonding_lacp() {
        let t = "Ethernet Channel Bonding Driver: v6.12\n\nBonding Mode: IEEE 802.3ad Dynamic link aggregation\n\
                 Transmit Hash Policy: layer3+4 (1)\nMII Status: up\n\n802.3ad info\nLACP active: on\nLACP rate: fast\n\
                 System MAC address: 52:54:00:02:00:00\nActive Aggregator Info:\n\tAggregator ID: 1\n\tPartner Mac Address: 52:54:00:01:00:01\n\n\
                 Slave Interface: enp0s2\nMII Status: up\nAggregator ID: 1\n\nSlave Interface: enp0s3\nMII Status: down\nAggregator ID: 2\n";
        let st = parse_bonding(t);
        assert_eq!(
            st.bond_mode.as_deref(),
            Some("IEEE 802.3ad Dynamic link aggregation")
        );
        assert_eq!(st.lacp_partner.as_deref(), Some("52:54:00:01:00:01"));
        assert_eq!(st.members.len(), 2);
        assert_eq!(st.members[0].link, "up");
        assert_eq!(st.members[1].link, "down");
        assert_eq!(st.members[1].aggregator_id.as_deref(), Some("2"));
    }

    #[tokio::test]
    async fn missing_tool_is_not_available() {
        let cfg = crate::config::ManagementNetworkConfig {
            tool_path: "/nonexistent/console-menu".into(),
            ..Default::default()
        };
        match run_tool(&cfg, &lacp42(), false).await {
            Err(ApiError::NotAvailable { reason, .. }) => {
                assert_eq!(reason, "management-link-tool-missing")
            }
            other => panic!("expected NotAvailable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn tool_refusal_and_success() {
        let dir = std::env::temp_dir().join(format!("hx-mgmt-tool-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("bad.sh");
        std::fs::write(&bad, "#!/bin/sh\ncat >/dev/null\necho '{\"ok\": false, \"errors\": [\"eth9 does not exist\"]}'\nexit 2\n").unwrap();
        let good = dir.join("good.sh");
        std::fs::write(&good, "#!/bin/sh\nr=$(cat)\necho 'systemd-networkd: 10-mgmt-bond0.netdev' >&2\necho \"{\\\"ok\\\": true, \\\"applied\\\": false, \\\"record\\\": $r}\"\n").unwrap();
        for f in [&bad, &good] {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut cfg = crate::config::ManagementNetworkConfig {
            tool_path: bad,
            ..Default::default()
        };
        match run_tool(&cfg, &lacp42(), false).await {
            Err(ApiError::ValidationError(m)) => assert!(m.contains("eth9")),
            other => panic!("expected a refusal, got {other:?}"),
        }
        cfg.tool_path = good;
        let v = run_tool(&cfg, &lacp42(), false).await.unwrap();
        assert_eq!(v["record"]["address_interface"], "bond0.42");
        assert_eq!(v["log"][0], "systemd-networkd: 10-mgmt-bond0.netdev");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
