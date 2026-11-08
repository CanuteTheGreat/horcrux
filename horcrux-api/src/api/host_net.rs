//! Host Linux bridges (Infrastructure > Network > Host bridges): the bridges
//! VM network cards attach to. Without a way to create one from the UI, a
//! fresh host offered VM NICs nothing to plug into.
//!
//! - `GET    /api/network/bridges`        every bridge on the host (managed or
//!   not) with its ports, addresses and VLAN filtering
//! - `POST   /api/network/bridges`        create a Horcrux-managed bridge
//! - `PUT    /api/network/bridges/:name`  change ports / trunk VLANs of a managed
//!   bridge on the live kernel bridge (no VM restart)
//! - `DELETE /api/network/bridges/:name`  remove a managed bridge
//!
//! Trunk VLANs: on a VLAN-aware bridge a port only forwards the VLAN ids it
//! was added to (`bridge vlan add`). `trunk_vlans` lists the ids/ranges
//! (`"20"`, `"100-110"`) allowed on every uplink port in `ports`;
//! `port_vlans` overrides that for single ports. Tap ports of VMs are
//! access ports and are not touched.
//!
//! Managed bridges are stored in `config_objects` (kind `host_bridge`) and
//! re-created when the API starts, so they survive a host reboot. A port
//! that carries an IP address (the host's own uplink) is refused: enslaving
//! it would move the host's address onto the bridge and cut the host off.
use crate::db::config_store::ConfigStore;
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use tokio::process::Command;

const KIND: &str = "host_bridge";

/// A bridge Horcrux creates and re-creates at startup.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManagedBridge {
    pub name: String,
    /// VLAN filtering on (VM NICs can then carry a VLAN tag).
    #[serde(default)]
    pub vlan_aware: bool,
    /// Host address on the bridge, CIDR (e.g. 10.10.0.1/24); none = pure L2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// Host interfaces to enslave (must not carry an IP address).
    #[serde(default)]
    pub ports: Vec<String>,
    /// VLAN ids / ranges ("20", "100-110") trunked on every port in `ports`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trunk_vlans: Vec<String>,
    /// Per-port override of `trunk_vlans` (port name -> ids/ranges).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub port_vlans: BTreeMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// A bridge as the host has it right now.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BridgeInfo {
    pub name: String,
    pub vlan_aware: bool,
    pub up: bool,
    pub ports: Vec<String>,
    pub addresses: Vec<String>,
    /// VLAN ids the kernel really has on each non-tap port (compressed
    /// ranges, e.g. "1", "20", "100-110").
    #[serde(default)]
    pub port_vlans: BTreeMap<String, Vec<String>>,
    /// Created (and re-created at startup) by Horcrux; only these can be
    /// deleted here.
    pub managed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

pub fn routes(state: &Arc<AppState>) -> Router<Arc<AppState>> {
    // Re-create managed bridges after a host reboot.
    let store = ConfigStore::new(&state.database);
    tokio::spawn(async move {
        match store.load_values::<ManagedBridge>(KIND).await {
            Ok(list) => {
                for b in list {
                    if crate::vm::qemu::HostNet::system()
                        .bridge_vlan_aware(&b.name)
                        .is_none()
                    {
                        match apply(&b).await {
                            Ok(()) => tracing::info!("Re-created host bridge {}", b.name),
                            Err(e) => tracing::warn!("Could not re-create bridge {}: {e}", b.name),
                        }
                    }
                }
            }
            Err(e) => tracing::warn!("Could not load managed bridges: {e}"),
        }
    });
    Router::new()
        .route("/api/network/bridges", get(list).post(create))
        .route("/api/network/bridges/:name", delete(remove).put(update))
}

async fn ip_json(args: &[&str]) -> serde_json::Value {
    match Command::new("ip").args(args).output().await {
        Ok(o) if o.status.success() => serde_json::from_slice(&o.stdout).unwrap_or_default(),
        _ => serde_json::Value::Null,
    }
}

async fn run(program: &str, args: &[&str]) -> Result<(), String> {
    let o = Command::new(program)
        .args(args)
        .output()
        .await
        .map_err(|e| format!("{program}: {e}"))?;
    if o.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{program} {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&o.stderr).trim()
        ))
    }
}

/// Global addresses of interface `dev` (`ip -j addr`).
async fn addresses(dev: &str) -> Vec<String> {
    ip_json(&["-j", "addr", "show", "dev", dev])
        .await
        .as_array()
        .and_then(|a| a.first())
        .and_then(|i| i["addr_info"].as_array())
        .map(|a| {
            a.iter()
                .filter(|x| x["scope"].as_str() == Some("global"))
                .filter_map(|x| {
                    Some(format!(
                        "{}/{}",
                        x["local"].as_str()?,
                        x["prefixlen"].as_u64()?
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn valid_cidr(s: &str) -> bool {
    let Some((ip, len)) = s.split_once('/') else {
        return false;
    };
    match (ip.parse::<std::net::IpAddr>(), len.parse::<u8>()) {
        (Ok(std::net::IpAddr::V4(_)), Ok(l)) => l <= 32,
        (Ok(std::net::IpAddr::V6(_)), Ok(l)) => l <= 128,
        _ => false,
    }
}

/// Validate a bridge definition (the host-independent part).
pub fn validate(b: &ManagedBridge) -> Result<(), String> {
    if !horcrux_common::valid_ifname(&b.name) {
        return Err(format!(
            "{:?} is not a valid interface name (1-15 of a-z 0-9 - _ .)",
            b.name
        ));
    }
    if let Some(a) = &b.address {
        if !valid_cidr(a) {
            return Err(format!(
                "address {a:?} must be an IP with a prefix length, e.g. 10.10.0.1/24"
            ));
        }
    }
    for p in &b.ports {
        if !horcrux_common::valid_ifname(p) || p == &b.name {
            return Err(format!("invalid port {p:?}"));
        }
    }
    let vlans = parse_vlans(&b.trunk_vlans)?;
    let mut any = !vlans.is_empty();
    for (port, v) in &b.port_vlans {
        if !b.ports.contains(port) {
            return Err(format!("port_vlans names {port:?}, which is not in ports"));
        }
        any |= !parse_vlans(v)?.is_empty();
    }
    if any && !b.vlan_aware {
        return Err("trunk VLANs need a VLAN-aware bridge (vlan_aware: true)".into());
    }
    Ok(())
}

/// Parse `["20", "100-110"]` into the set of VLAN ids (1-4094).
pub fn parse_vlans(list: &[String]) -> Result<BTreeSet<u16>, String> {
    let mut out = BTreeSet::new();
    for item in list {
        let item = item.trim();
        let (a, b) = match item.split_once('-') {
            Some((a, b)) => (a.trim(), b.trim()),
            None => (item, item),
        };
        let (Ok(a), Ok(b)) = (a.parse::<u16>(), b.parse::<u16>()) else {
            return Err(format!(
                "invalid VLAN id or range {item:?} (e.g. 20 or 100-110)"
            ));
        };
        if !(1..=4094).contains(&a) || !(1..=4094).contains(&b) || a > b {
            return Err(format!("VLAN {item:?} is outside 1-4094 or reversed"));
        }
        out.extend(a..=b);
    }
    Ok(out)
}

/// Compress a VLAN set into ranges: {1,20,21,22} -> ["1", "20-22"].
pub fn compress_vlans(set: &BTreeSet<u16>) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = set.iter().copied().peekable();
    while let Some(start) = it.next() {
        let mut end = start;
        while it.peek() == Some(&(end + 1)) {
            end = it.next().unwrap_or(end);
        }
        out.push(if start == end {
            start.to_string()
        } else {
            format!("{start}-{end}")
        });
    }
    out
}

/// VLAN ids wanted on `port`: its override, else the bridge-wide list.
pub fn wanted_vlans(b: &ManagedBridge, port: &str) -> BTreeSet<u16> {
    let list = b.port_vlans.get(port).unwrap_or(&b.trunk_vlans);
    parse_vlans(list).unwrap_or_default()
}

/// What the kernel has on `port` (`bridge -j vlan show dev`).
async fn kernel_vlans(port: &str) -> BTreeSet<u16> {
    let mut set = BTreeSet::new();
    if let Ok(o) = Command::new("bridge")
        .args(["-j", "vlan", "show", "dev", port])
        .output()
        .await
    {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&o.stdout) {
            for e in v.as_array().into_iter().flatten() {
                for x in e["vlans"].as_array().into_iter().flatten() {
                    if let Some(a) = x["vlan"].as_u64() {
                        let end = x["vlanEnd"].as_u64().unwrap_or(a);
                        set.extend((a as u16)..=(end as u16));
                    }
                }
            }
        }
    }
    set
}

/// Make `port` carry exactly `want` on top of what it has, removing
/// `drop` ids (those Horcrux put there before).
async fn sync_port_vlans(
    port: &str,
    want: &BTreeSet<u16>,
    drop: &BTreeSet<u16>,
) -> Result<(), String> {
    for v in drop.difference(want) {
        let _ = run(
            "bridge",
            &["vlan", "del", "dev", port, "vid", &v.to_string()],
        )
        .await;
    }
    let have = kernel_vlans(port).await;
    let missing: BTreeSet<u16> = want.difference(&have).copied().collect();
    for r in compress_vlans(&missing) {
        run("bridge", &["vlan", "add", "dev", port, "vid", &r]).await?;
    }
    Ok(())
}

async fn apply(b: &ManagedBridge) -> Result<(), String> {
    let vf = if b.vlan_aware { "1" } else { "0" };
    run(
        "ip",
        &[
            "link",
            "add",
            "name",
            &b.name,
            "type",
            "bridge",
            "vlan_filtering",
            vf,
        ],
    )
    .await?;
    let result: Result<(), String> = async {
        if let Some(a) = &b.address {
            run("ip", &["addr", "add", a, "dev", &b.name]).await?;
        }
        for p in &b.ports {
            run("ip", &["link", "set", p, "master", &b.name]).await?;
            run("ip", &["link", "set", p, "up"]).await?;
            sync_port_vlans(p, &wanted_vlans(b, p), &BTreeSet::new()).await?;
        }
        run("ip", &["link", "set", &b.name, "up"]).await
    }
    .await;
    if result.is_err() {
        let _ = run("ip", &["link", "del", &b.name]).await;
    }
    result
}

async fn list(State(state): State<Arc<AppState>>) -> Result<Json<Vec<BridgeInfo>>, ApiError> {
    let managed = ConfigStore::new(&state.database)
        .load_values::<ManagedBridge>(KIND)
        .await?;
    let mut out = Vec::new();
    for b in super::vm_hw::host_bridges() {
        let ports: Vec<String> = ip_json(&["-j", "link", "show", "master", &b.name])
            .await
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|l| l["ifname"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let up = std::fs::read_to_string(format!("/sys/class/net/{}/operstate", b.name))
            .map(|s| s.trim() != "down")
            .unwrap_or(false);
        let m = managed.iter().find(|m| m.name == b.name);
        let mut port_vlans = BTreeMap::new();
        for p in ports.iter().filter(|p| !p.starts_with("tap")) {
            port_vlans.insert(p.clone(), compress_vlans(&kernel_vlans(p).await));
        }
        out.push(BridgeInfo {
            port_vlans,
            addresses: addresses(&b.name).await,
            name: b.name,
            vlan_aware: b.vlan_aware,
            up,
            ports,
            managed: m.is_some(),
            comment: m.and_then(|m| m.comment.clone()),
        });
    }
    Ok(Json(out))
}

async fn create(
    State(state): State<Arc<AppState>>,
    Json(mut b): Json<ManagedBridge>,
) -> Result<(StatusCode, Json<ManagedBridge>), ApiError> {
    b.name = b.name.trim().to_string();
    b.address = b
        .address
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty());
    b.ports.retain(|p| !p.trim().is_empty());
    validate(&b).map_err(ApiError::BadRequest)?;
    if std::path::Path::new("/sys/class/net")
        .join(&b.name)
        .exists()
    {
        return Err(ApiError::Conflict(format!(
            "an interface called {} already exists",
            b.name
        )));
    }
    for p in &b.ports {
        if !std::path::Path::new("/sys/class/net").join(p).exists() {
            return Err(ApiError::BadRequest(format!("port {p} does not exist")));
        }
        let addrs = addresses(p).await;
        if !addrs.is_empty() {
            return Err(ApiError::BadRequest(format!(
                "port {p} carries the host address {}: moving it into a bridge would cut the \
                 host's network. Use a port without an address.",
                addrs.join(", ")
            )));
        }
    }
    apply(&b).await.map_err(ApiError::Internal)?;
    if let Err(e) = ConfigStore::new(&state.database)
        .put(KIND, &b.name, &b)
        .await
    {
        let _ = run("ip", &["link", "del", &b.name]).await;
        return Err(e.into());
    }
    tracing::info!("Created host bridge {}", b.name);
    Ok((StatusCode::CREATED, Json(b)))
}

/// Body of `PUT /api/network/bridges/:name`: new ports / trunk VLANs / comment
/// (fields left out keep their value).
#[derive(Debug, Deserialize)]
struct BridgeUpdate {
    ports: Option<Vec<String>>,
    trunk_vlans: Option<Vec<String>>,
    port_vlans: Option<BTreeMap<String, Vec<String>>>,
    comment: Option<String>,
}

async fn update(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(u): Json<BridgeUpdate>,
) -> Result<Json<ManagedBridge>, ApiError> {
    let store = ConfigStore::new(&state.database);
    let old = store
        .load_values::<ManagedBridge>(KIND)
        .await?
        .into_iter()
        .find(|m| m.name == name)
        .ok_or_else(|| ApiError::NotFound(format!("managed bridge {name} not found")))?;
    let mut b = old.clone();
    if let Some(p) = u.ports {
        b.ports = p.into_iter().filter(|p| !p.trim().is_empty()).collect();
    }
    if let Some(t) = u.trunk_vlans {
        b.trunk_vlans = t;
    }
    if let Some(t) = u.port_vlans {
        b.port_vlans = t;
    }
    if u.comment.is_some() {
        b.comment = u.comment;
    }
    validate(&b).map_err(ApiError::BadRequest)?;
    let live = std::path::Path::new("/sys/class/net").join(&name).exists();
    if live {
        for p in b.ports.iter().filter(|p| !old.ports.contains(p)) {
            if !std::path::Path::new("/sys/class/net").join(p).exists() {
                return Err(ApiError::BadRequest(format!("port {p} does not exist")));
            }
            let addrs = addresses(p).await;
            if !addrs.is_empty() {
                return Err(ApiError::BadRequest(format!(
                    "port {p} carries the host address {}: it cannot join a bridge",
                    addrs.join(", ")
                )));
            }
        }
        for p in old.ports.iter().filter(|p| !b.ports.contains(p)) {
            let _ = run("ip", &["link", "set", p, "nomaster"]).await;
        }
        for p in &b.ports {
            if !old.ports.contains(p) {
                run("ip", &["link", "set", p, "master", &name])
                    .await
                    .map_err(ApiError::Internal)?;
                run("ip", &["link", "set", p, "up"])
                    .await
                    .map_err(ApiError::Internal)?;
            }
            let before = if old.ports.contains(p) {
                wanted_vlans(&old, p)
            } else {
                BTreeSet::new()
            };
            sync_port_vlans(p, &wanted_vlans(&b, p), &before)
                .await
                .map_err(ApiError::Internal)?;
        }
    }
    store.put(KIND, &b.name, &b).await?;
    Ok(Json(b))
}

async fn remove(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let store = ConfigStore::new(&state.database);
    let managed = store.load_values::<ManagedBridge>(KIND).await?;
    if !managed.iter().any(|m| m.name == name) {
        return Err(
            if crate::vm::qemu::HostNet::system()
                .bridge_vlan_aware(&name)
                .is_some()
            {
                ApiError::Conflict(format!(
                    "{name} was not created by Horcrux (it belongs to the host's network \
                 configuration), so it is not removed here"
                ))
            } else {
                ApiError::NotFound(format!("bridge {name} not found"))
            },
        );
    }
    let users: Vec<String> = state
        .vm_manager
        .list_vms()
        .await
        .into_iter()
        .filter(|v| v.hardware.nets.iter().any(|n| n.bridge == name))
        .map(|v| format!("{} ({})", v.name, v.id))
        .collect();
    if !users.is_empty() {
        return Err(ApiError::Conflict(format!(
            "{name} is used by the network cards of {}",
            users.join(", ")
        )));
    }
    if crate::vm::qemu::HostNet::system()
        .bridge_vlan_aware(&name)
        .is_some()
    {
        run("ip", &["link", "del", &name])
            .await
            .map_err(ApiError::Internal)?;
    }
    store.delete(KIND, &name).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(name: &str) -> ManagedBridge {
        ManagedBridge {
            name: name.into(),
            vlan_aware: false,
            address: None,
            ports: vec![],
            trunk_vlans: vec![],
            port_vlans: BTreeMap::new(),
            comment: None,
        }
    }

    #[test]
    fn trunk_vlans_parse_validate_and_compress() {
        let l = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let set = parse_vlans(&l(&["20", "100-102", " 30 "])).unwrap();
        assert_eq!(compress_vlans(&set), l(&["20", "30", "100-102"]));
        assert!(parse_vlans(&l(&["0"])).is_err());
        assert!(parse_vlans(&l(&["4095"])).is_err());
        assert!(parse_vlans(&l(&["9-3"])).is_err());
        assert!(parse_vlans(&l(&["x"])).is_err());
        let mut x = b("vmbr1");
        x.ports = vec!["eth1".into()];
        x.trunk_vlans = l(&["20"]);
        assert!(validate(&x).is_err(), "needs vlan_aware");
        x.vlan_aware = true;
        assert!(validate(&x).is_ok());
        x.port_vlans.insert("eth9".into(), l(&["5"]));
        assert!(validate(&x).is_err(), "override for a non-port");
        x.port_vlans.clear();
        x.port_vlans.insert("eth1".into(), l(&["5-6"]));
        assert_eq!(wanted_vlans(&x, "eth1").len(), 2);
        assert_eq!(wanted_vlans(&x, "eth2").len(), 1);
    }

    #[test]
    fn bridge_definitions_are_validated() {
        assert!(validate(&b("vmbr0")).is_ok());
        assert!(validate(&b("")).is_err());
        assert!(validate(&b("bad name")).is_err());
        assert!(validate(&b("waytoolongbridgename")).is_err());
        let mut x = b("vmbr1");
        x.address = Some("10.10.0.1/24".into());
        assert!(validate(&x).is_ok());
        x.address = Some("fd00::1/64".into());
        assert!(validate(&x).is_ok());
        x.address = Some("10.10.0.1".into());
        assert!(validate(&x).is_err());
        x.address = Some("10.10.0.1/33".into());
        assert!(validate(&x).is_err());
        x.address = None;
        x.ports = vec!["vmbr1".into()];
        assert!(validate(&x).is_err());
    }
}
