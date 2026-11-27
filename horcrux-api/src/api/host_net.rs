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
        // VLAN interfaces first: a bridge may use one as its port.
        match store.load_values::<ManagedVlan>(VLAN_KIND).await {
            Ok(list) => {
                for v in list {
                    if !std::path::Path::new("/sys/class/net")
                        .join(&v.name)
                        .exists()
                    {
                        match apply_vlan(&v).await {
                            Ok(()) => tracing::info!("Re-created VLAN interface {}", v.name),
                            Err(e) => tracing::warn!("Could not re-create VLAN {}: {e}", v.name),
                        }
                    }
                }
            }
            Err(e) => tracing::warn!("Could not load managed VLAN interfaces: {e}"),
        }
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
        .route("/api/network/interfaces", get(list_interfaces))
        .route("/api/network/vlans", get(list_vlans).post(create_vlan))
        .route("/api/network/vlans/:name", delete(remove_vlan))
}

const VLAN_KIND: &str = "host_vlan";

/// A VLAN interface Horcrux creates (and re-creates at startup): an
/// 802.1Q sub-interface of a NIC, bond or bridge (Proxmox "Linux VLAN").
/// It can carry a host address or be the port of a bridge.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManagedVlan {
    /// Interface name; default `<parent>.<id>`.
    #[serde(default)]
    pub name: String,
    pub parent: String,
    pub id: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// Fill the default name and check a VLAN definition.
pub fn validate_vlan(v: &mut ManagedVlan) -> Result<(), String> {
    v.parent = v.parent.trim().to_string();
    v.name = v.name.trim().to_string();
    if v.name.is_empty() {
        v.name = format!("{}.{}", v.parent, v.id);
    }
    v.address = v
        .address
        .take()
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty());
    if !(1..=4094).contains(&v.id) {
        return Err(format!("VLAN id {} must be 1-4094", v.id));
    }
    if !horcrux_common::valid_ifname(&v.parent) {
        return Err(format!("{:?} is not a valid parent interface", v.parent));
    }
    if !horcrux_common::valid_ifname(&v.name) {
        return Err(format!(
            "{:?} is not a valid interface name (1-15 of a-z 0-9 - _ .); choose a shorter name",
            v.name
        ));
    }
    if v.name == v.parent {
        return Err("the VLAN interface needs its own name".into());
    }
    if let Some(a) = &v.address {
        if !valid_cidr(a) {
            return Err(format!(
                "address {a:?} must be an IP with a prefix length, e.g. 10.20.0.1/24"
            ));
        }
    }
    Ok(())
}

async fn apply_vlan(v: &ManagedVlan) -> Result<(), String> {
    let id = v.id.to_string();
    run(
        "ip",
        &[
            "link", "add", "link", &v.parent, "name", &v.name, "type", "vlan", "id", &id,
        ],
    )
    .await?;
    let r: Result<(), String> = async {
        if let Some(a) = &v.address {
            run("ip", &["addr", "add", a, "dev", &v.name]).await?;
        }
        run("ip", &["link", "set", &v.name, "up"]).await
    }
    .await;
    if r.is_err() {
        let _ = run("ip", &["link", "del", &v.name]).await;
    }
    r
}

async fn list_vlans(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<ManagedVlan>>, ApiError> {
    Ok(Json(
        ConfigStore::new(&state.database)
            .load_values::<ManagedVlan>(VLAN_KIND)
            .await?,
    ))
}

async fn create_vlan(
    State(state): State<Arc<AppState>>,
    Json(mut v): Json<ManagedVlan>,
) -> Result<(StatusCode, Json<ManagedVlan>), ApiError> {
    validate_vlan(&mut v).map_err(ApiError::BadRequest)?;
    let net = std::path::Path::new("/sys/class/net");
    if !net.join(&v.parent).exists() {
        return Err(ApiError::BadRequest(format!(
            "parent interface {} does not exist",
            v.parent
        )));
    }
    if net.join(&v.name).exists() {
        return Err(ApiError::Conflict(format!(
            "an interface called {} already exists",
            v.name
        )));
    }
    apply_vlan(&v).await.map_err(ApiError::Internal)?;
    if let Err(e) = ConfigStore::new(&state.database)
        .put(VLAN_KIND, &v.name, &v)
        .await
    {
        let _ = run("ip", &["link", "del", &v.name]).await;
        return Err(e.into());
    }
    tracing::info!(
        "Created VLAN interface {} ({} tag {})",
        v.name,
        v.parent,
        v.id
    );
    Ok((StatusCode::CREATED, Json(v)))
}

async fn remove_vlan(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let store = ConfigStore::new(&state.database);
    let managed = store.load_values::<ManagedVlan>(VLAN_KIND).await?;
    if !managed.iter().any(|m| m.name == name) {
        return Err(
            if std::path::Path::new("/sys/class/net").join(&name).exists() {
                ApiError::Conflict(format!(
                    "{name} was not created by Horcrux (it belongs to the host's network \
                     configuration), so it is not removed here"
                ))
            } else {
                ApiError::NotFound(format!("VLAN interface {name} not found"))
            },
        );
    }
    let bridges = store.load_values::<ManagedBridge>(KIND).await?;
    if let Some(b) = bridges.iter().find(|b| b.ports.contains(&name)) {
        return Err(ApiError::Conflict(format!(
            "{name} is a port of bridge {}: remove it from the bridge first",
            b.name
        )));
    }
    if let Some(m) = std::fs::read_link(format!("/sys/class/net/{name}/master"))
        .ok()
        .and_then(|p| p.file_name().map(|f| f.to_string_lossy().to_string()))
    {
        return Err(ApiError::Conflict(format!(
            "{name} is a port of {m}: remove it from there first"
        )));
    }
    if std::path::Path::new("/sys/class/net").join(&name).exists() {
        run("ip", &["link", "del", &name])
            .await
            .map_err(ApiError::Internal)?;
    }
    store.delete(VLAN_KIND, &name).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// One host network interface (Infrastructure > Host Bridges > Network
/// interfaces), the way Proxmox lists a node's NICs, bonds and VLANs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HostInterface {
    pub name: String,
    /// physical, wireless, bond, vlan, bridge, wireguard, veth, tap, virtual
    pub kind: String,
    /// up, down, lowerlayerdown, unknown, ... (kernel operstate, lower case)
    pub state: String,
    /// Link detected (cable / carrier).
    pub carrier: bool,
    pub mac: Option<String>,
    pub mtu: u32,
    /// Negotiated speed (physical NICs with a link).
    pub speed_mbps: Option<u32>,
    /// Bridge or bond it is enslaved to.
    pub master: Option<String>,
    /// VLAN interfaces: parent device and tag.
    pub vlan_parent: Option<String>,
    pub vlan_id: Option<u16>,
    pub addresses: Vec<String>,
    /// Can be added as a port of a new bridge: a wired NIC, bond or VLAN
    /// without an address and not already in a bridge/bond.
    pub bridge_port_ok: bool,
    /// Why it can't be a bridge port (shown as a hint).
    pub bridge_port_note: Option<String>,
}

/// Classify one entry of `ip -j -d link show`; `addresses` and `speed`
/// come from elsewhere (`ip -j addr`, sysfs).
pub fn interface_from_link(
    l: &serde_json::Value,
    addresses: Vec<String>,
    speed_mbps: Option<u32>,
    wireless: bool,
) -> Option<HostInterface> {
    let name = l["ifname"].as_str()?.to_string();
    if l["link_type"].as_str() == Some("loopback") {
        return None;
    }
    let info_kind = l["linkinfo"]["info_kind"].as_str().unwrap_or("");
    let kind = match info_kind {
        "bridge" => "bridge",
        "bond" => "bond",
        "vlan" => "vlan",
        "wireguard" => "wireguard",
        "veth" => "veth",
        "tun" | "tap" => "tap",
        "" if l["parentbus"].is_string() || l["parentdev"].is_string() => {
            if wireless {
                "wireless"
            } else {
                "physical"
            }
        }
        _ => "virtual",
    }
    .to_string();
    let flags: Vec<&str> = l["flags"]
        .as_array()
        .map(|a| a.iter().filter_map(|f| f.as_str()).collect())
        .unwrap_or_default();
    let master = l["master"].as_str().map(String::from);
    let (vlan_parent, vlan_id) = if kind == "vlan" {
        (
            l["link"].as_str().map(String::from),
            l["linkinfo"]["info_data"]["id"].as_u64().map(|v| v as u16),
        )
    } else {
        (None, None)
    };
    let note = if !matches!(kind.as_str(), "physical" | "bond" | "vlan") {
        Some(match kind.as_str() {
            "wireless" => "Wi-Fi interfaces cannot join a Linux bridge".to_string(),
            "bridge" => "already a bridge".to_string(),
            _ => "not a wired uplink".to_string(),
        })
    } else if let Some(m) = &master {
        Some(format!("already a port of {m}"))
    } else if !addresses.is_empty() {
        Some("carries a host address: moving it into a bridge would cut the host off".into())
    } else {
        None
    };
    Some(HostInterface {
        state: l["operstate"].as_str().unwrap_or("unknown").to_lowercase(),
        carrier: flags.contains(&"LOWER_UP"),
        mac: l["address"]
            .as_str()
            .filter(|_| l["link_type"].as_str() == Some("ether"))
            .map(String::from),
        mtu: l["mtu"].as_u64().unwrap_or(0) as u32,
        speed_mbps: speed_mbps.filter(|_| kind == "physical" || kind == "bond"),
        bridge_port_ok: note.is_none(),
        bridge_port_note: note,
        name,
        kind,
        master,
        vlan_parent,
        vlan_id,
        addresses,
    })
}

#[derive(Debug, Deserialize)]
struct InterfacesQuery {
    /// Also list guest plumbing (veth/tap ports of VMs and containers).
    #[serde(default)]
    all: bool,
}

/// `GET /api/network/interfaces[?all=1]`: the host's NICs, bonds, VLANs,
/// bridges and tunnels. Guest ports (tap/veth) only with `all`.
async fn list_interfaces(
    axum::extract::Query(q): axum::extract::Query<InterfacesQuery>,
) -> Json<Vec<HostInterface>> {
    let links = ip_json(&["-j", "-d", "link", "show"]).await;
    let mut out = Vec::new();
    for l in links.as_array().cloned().unwrap_or_default() {
        let Some(name) = l["ifname"].as_str().map(String::from) else {
            continue;
        };
        let sys = std::path::Path::new("/sys/class/net").join(&name);
        let speed = std::fs::read_to_string(sys.join("speed"))
            .ok()
            .and_then(|s| s.trim().parse::<i64>().ok())
            .filter(|s| *s > 0)
            .map(|s| s as u32);
        let wireless = sys.join("wireless").exists() || sys.join("phy80211").exists();
        let addrs = addresses(&name).await;
        if let Some(i) = interface_from_link(&l, addrs, speed, wireless) {
            if q.all || !matches!(i.kind.as_str(), "veth" | "tap") {
                out.push(i);
            }
        }
    }
    Json(out)
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
    fn vlan_definitions_get_a_name_and_are_checked() {
        let mut v = ManagedVlan {
            name: String::new(),
            parent: " eno1 ".into(),
            id: 20,
            address: Some(" ".into()),
            comment: None,
        };
        validate_vlan(&mut v).unwrap();
        assert_eq!(
            (v.name.as_str(), v.parent.as_str(), v.address.as_deref()),
            ("eno1.20", "eno1", None)
        );
        let mut bad = v.clone();
        bad.id = 0;
        assert!(validate_vlan(&mut bad).is_err());
        let mut bad = v.clone();
        bad.id = 4095;
        assert!(validate_vlan(&mut bad).is_err());
        let mut bad = v.clone();
        bad.address = Some("10.0.0.1".into());
        assert!(validate_vlan(&mut bad).unwrap_err().contains("prefix"));
        let mut long = ManagedVlan {
            name: String::new(),
            parent: "enp132s0f1np1".into(),
            id: 4000,
            address: None,
            comment: None,
        };
        assert!(validate_vlan(&mut long)
            .unwrap_err()
            .contains("shorter name"));
    }

    #[test]
    fn host_interfaces_are_classified() {
        let nic = serde_json::json!({"ifname":"enp1s0","flags":["BROADCAST","UP","LOWER_UP"],"mtu":1500,
            "operstate":"UP","link_type":"ether","address":"d4:a2:cd:00:00:01","parentbus":"pci"});
        let i = interface_from_link(&nic, vec![], Some(1000), false).unwrap();
        assert_eq!(
            (i.kind.as_str(), i.state.as_str(), i.carrier, i.speed_mbps),
            ("physical", "up", true, Some(1000))
        );
        assert!(i.bridge_port_ok);
        // the host's own uplink must not be offered as a bridge port
        let i = interface_from_link(&nic, vec!["10.0.0.5/24".into()], None, false).unwrap();
        assert!(!i.bridge_port_ok && i.bridge_port_note.unwrap().contains("host address"));
        let wifi = serde_json::json!({"ifname":"wlan0","flags":["BROADCAST"],"mtu":1500,"operstate":"DOWN",
            "link_type":"ether","address":"aa:bb:cc:00:00:02","parentbus":"pci"});
        let i = interface_from_link(&wifi, vec![], None, true).unwrap();
        assert_eq!(
            (i.kind.as_str(), i.carrier, i.bridge_port_ok),
            ("wireless", false, false)
        );
        let vlan = serde_json::json!({"ifname":"enp1s0.20","link":"enp1s0","flags":["UP"],"mtu":1500,"operstate":"UP",
            "link_type":"ether","address":"d4:a2:cd:00:00:01","linkinfo":{"info_kind":"vlan","info_data":{"protocol":"802.1Q","id":20}}});
        let i = interface_from_link(&vlan, vec![], None, false).unwrap();
        assert_eq!(
            (i.kind.as_str(), i.vlan_parent.as_deref(), i.vlan_id),
            ("vlan", Some("enp1s0"), Some(20))
        );
        let port = serde_json::json!({"ifname":"eth2","master":"vmbr0","flags":["UP"],"mtu":1500,"operstate":"UP",
            "link_type":"ether","parentbus":"pci"});
        let i = interface_from_link(&port, vec![], None, false).unwrap();
        assert_eq!(
            i.bridge_port_note.as_deref(),
            Some("already a port of vmbr0")
        );
        let wg = serde_json::json!({"ifname":"wg0","flags":["UP"],"mtu":1380,"operstate":"UNKNOWN","link_type":"none",
            "linkinfo":{"info_kind":"wireguard"}});
        let i = interface_from_link(&wg, vec![], None, false).unwrap();
        assert_eq!((i.kind.as_str(), i.mac.is_none()), ("wireguard", true));
        let lo = serde_json::json!({"ifname":"lo","link_type":"loopback"});
        assert!(interface_from_link(&lo, vec![], None, false).is_none());
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
