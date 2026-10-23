//! Software-defined networking (Proxmox "Datacenter > SDN"): zones, VNets,
//! subnets and IPAM, applied to this node's kernel.
//!
//! * a **zone** says how a VNet reaches the wire: `simple` (isolated host
//!   bridge), `vlan` (a VLAN sub-interface of an uplink) or `vxlan` (a VXLAN
//!   tunnel to the zone's peer nodes, unicast head-end replication)
//! * a **VNet** becomes a Linux bridge named after its id (VMs and
//!   containers attach to it like to any host bridge); `vlan` and `vxlan`
//!   VNets also get their VLAN sub-interface / VXLAN device enslaved
//! * a **subnet** puts the gateway address on the bridge and, with `snat`, a
//!   masquerade rule (nft table `inet horcrux_sdn`) lets the guests out
//! * **IPAM** hands out addresses of a subnet to hostnames/MACs and keeps
//!   the registry (static allocation; Horcrux runs no DHCP server)
//!
//! Everything is stored in `config_objects` and re-applied when the API
//! starts, so it survives a reboot. The configuration is per node: apply the
//! same zones on each node of a VXLAN overlay.
//!
//! Routes: `/api/sdn/zones[/:id]`, `/api/sdn/vnets[/:id]`,
//! `/api/sdn/vnets/:id/subnets`, `/api/sdn/subnets[/:id]`,
//! `/api/sdn/subnets/:id/ipam[/:ip]`, `GET /api/sdn/status`, `POST /api/sdn/apply`.
use super::system_admin::{run, which};
use crate::db::config_store::{kinds, ConfigStore};
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::{Path, State},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;
use std::sync::Arc;

pub fn routes(state: &Arc<AppState>) -> Router<Arc<AppState>> {
    spawn_reapply(state.clone());
    Router::new()
        .route("/api/sdn/zones", get(list_zones).post(create_zone))
        .route(
            "/api/sdn/zones/:id",
            get(get_zone).put(update_zone).delete(delete_zone),
        )
        .route("/api/sdn/vnets", get(list_vnets).post(create_vnet))
        .route("/api/sdn/vnets/:id", delete(delete_vnet))
        .route("/api/sdn/vnets/:id/subnets", post(create_subnet))
        .route("/api/sdn/subnets", get(list_subnets))
        .route("/api/sdn/subnets/:id", delete(delete_subnet))
        .route(
            "/api/sdn/subnets/:id/ipam",
            get(ipam_list).post(ipam_allocate),
        )
        .route("/api/sdn/subnets/:id/ipam/:ip", delete(ipam_release))
        .route("/api/sdn/status", get(status))
        .route("/api/sdn/apply", post(apply_all))
}

type Res<T> = Result<Json<T>, ApiError>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Zone {
    pub id: String,
    /// simple | vlan | vxlan
    pub zone_type: String,
    #[serde(default)]
    pub description: String,
    /// vlan: the physical interface or bridge the tagged traffic leaves on
    #[serde(default)]
    pub uplink: Option<String>,
    /// vxlan: this node's tunnel endpoint address
    #[serde(default)]
    pub local_ip: Option<String>,
    /// vxlan: the other nodes' tunnel endpoint addresses
    #[serde(default)]
    pub peers: Vec<String>,
    #[serde(default)]
    pub mtu: Option<u16>,
    /// vxlan UDP port (4789)
    #[serde(default)]
    pub dst_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VNet {
    pub id: String,
    pub zone: String,
    #[serde(default)]
    pub alias: Option<String>,
    /// VLAN id (1-4094) for a vlan zone, VNI (1-16777215) for a vxlan zone
    #[serde(default)]
    pub tag: Option<u32>,
    /// the bridge filters VLANs so guests can carry their own tags
    #[serde(default)]
    pub vlan_aware: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Subnet {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub vnet: String,
    pub cidr: String,
    #[serde(default)]
    pub gateway: Option<String>,
    #[serde(default)]
    pub snat: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Allocation {
    pub subnet: String,
    pub ip: String,
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub mac: Option<String>,
    /// static | gateway
    pub kind: String,
    pub created: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AllocateRequest {
    #[serde(default)]
    pub ip: Option<String>,
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub mac: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IpamView {
    pub subnet: String,
    pub total: u32,
    pub used: u32,
    pub free: u32,
    pub allocations: Vec<Allocation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VNetStatus {
    pub vnet: String,
    pub bridge_present: bool,
    pub up: bool,
    pub addresses: Vec<String>,
    pub ports: Vec<String>,
    /// the device that carries the zone (VLAN sub-interface / VXLAN device)
    pub carrier: Option<String>,
    pub carrier_present: bool,
    pub error: Option<String>,
}

// ----------------------------------------------------------------- validation

pub fn valid_zone_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 16
        && s.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A VNet id is its bridge name: short, lower case, no dots.
pub fn valid_vnet_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 8
        && s.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

fn valid_ifname(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 15
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

pub fn validate_zone(z: &Zone) -> Result<(), String> {
    if !valid_zone_id(&z.id) {
        return Err(
            "zone id must be 1-16 chars: lower-case letters, digits, '-', starting with a letter"
                .into(),
        );
    }
    if let Some(m) = z.mtu {
        if !(576..=9216).contains(&m) {
            return Err(format!("mtu {m} is outside 576-9216"));
        }
    }
    match z.zone_type.as_str() {
        "simple" => {}
        "vlan" => {
            let u = z.uplink.as_deref().unwrap_or("");
            if !valid_ifname(u) {
                return Err("a vlan zone needs an uplink interface or bridge".into());
            }
        }
        "vxlan" => {
            if z.peers.is_empty() {
                return Err("a vxlan zone needs at least one peer node address".into());
            }
            for p in z.peers.iter().chain(z.local_ip.iter()) {
                p.parse::<Ipv4Addr>()
                    .map_err(|_| format!("'{p}' is not an IPv4 tunnel address"))?;
            }
            if z.dst_port == Some(0) {
                return Err("dst_port must not be 0".into());
            }
        }
        t => {
            return Err(format!(
                "zone type must be simple, vlan or vxlan, not '{t}'"
            ))
        }
    }
    Ok(())
}

pub fn validate_vnet(v: &VNet, z: &Zone) -> Result<(), String> {
    if !valid_vnet_id(&v.id) {
        return Err("vnet id must be 1-8 chars: lower-case letters and digits, starting with a letter (it is the bridge name)".into());
    }
    match (z.zone_type.as_str(), v.tag) {
        ("simple", _) => {}
        ("vlan", Some(t)) if (1..=4094).contains(&t) => {}
        ("vlan", _) => return Err("a vlan zone needs a tag between 1 and 4094".into()),
        ("vxlan", Some(t)) if (1..=16_777_215).contains(&t) => {}
        ("vxlan", _) => return Err("a vxlan zone needs a VNI between 1 and 16777215".into()),
        _ => {}
    }
    Ok(())
}

/// (network address, prefix) of `a.b.c.d/p`, which must have no host bits set.
pub fn parse_cidr(c: &str) -> Result<(Ipv4Addr, u8), String> {
    let (a, p) = c
        .split_once('/')
        .ok_or("use CIDR notation like 10.20.0.0/24")?;
    let ip: Ipv4Addr = a
        .parse()
        .map_err(|_| format!("'{a}' is not an IPv4 address"))?;
    let p: u8 = p
        .parse()
        .map_err(|_| format!("'{p}' is not a prefix length"))?;
    if !(8..=30).contains(&p) {
        return Err("prefix length must be between 8 and 30".into());
    }
    let mask = u32::MAX << (32 - p);
    if u32::from(ip) & !mask != 0 {
        return Err(format!(
            "{c} has host bits set; the network is {}/{p}",
            Ipv4Addr::from(u32::from(ip) & mask)
        ));
    }
    Ok((ip, p))
}

fn net_range(net: Ipv4Addr, p: u8) -> (u32, u32) {
    let n = u32::from(net);
    let size = 1u32 << (32 - p);
    (n, n + size - 1)
}

pub fn subnets_overlap(a: &str, b: &str) -> bool {
    match (parse_cidr(a), parse_cidr(b)) {
        (Ok((an, ap)), Ok((bn, bp))) => {
            let (a0, a1) = net_range(an, ap);
            let (b0, b1) = net_range(bn, bp);
            a0 <= b1 && b0 <= a1
        }
        _ => false,
    }
}

pub fn validate_subnet(s: &Subnet, existing: &[Subnet]) -> Result<(Ipv4Addr, u8), String> {
    let (net, p) = parse_cidr(&s.cidr)?;
    if let Some(g) = s.gateway.as_deref().filter(|g| !g.is_empty()) {
        let g: Ipv4Addr = g
            .parse()
            .map_err(|_| format!("'{g}' is not an IPv4 gateway"))?;
        let (lo, hi) = net_range(net, p);
        let gv = u32::from(g);
        if gv <= lo || gv >= hi {
            return Err(format!(
                "gateway {g} must be a host address inside {}",
                s.cidr
            ));
        }
    } else if s.snat {
        return Err("SNAT needs a gateway address on the VNet".into());
    }
    if let Some(o) = existing
        .iter()
        .find(|e| e.id != s.id && subnets_overlap(&e.cidr, &s.cidr))
    {
        return Err(format!(
            "{} overlaps subnet {} of VNet {}",
            s.cidr, o.cidr, o.vnet
        ));
    }
    Ok((net, p))
}

pub fn subnet_id(vnet: &str, net: Ipv4Addr, p: u8) -> String {
    format!("{vnet}-{net}-{p}")
}

pub fn valid_mac(m: &str) -> bool {
    let parts: Vec<&str> = m.split(':').collect();
    parts.len() == 6
        && parts
            .iter()
            .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()))
}

fn valid_hostname(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 63
        && !h.starts_with('-')
        && !h.ends_with('-')
        && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

// ----------------------------------------------------------------------- IPAM

/// Pick the address for a request: the asked one (must be a free host
/// address of the subnet) or the lowest free host address.
pub fn pick_ip(
    cidr: &str,
    gateway: Option<&str>,
    taken: &[String],
    asked: Option<&str>,
) -> Result<Ipv4Addr, String> {
    let (net, p) = parse_cidr(cidr)?;
    let (lo, hi) = net_range(net, p);
    let gw = gateway
        .and_then(|g| g.parse::<Ipv4Addr>().ok())
        .map(u32::from);
    let used = |v: u32| {
        taken
            .iter()
            .any(|t| t.parse::<Ipv4Addr>().map(u32::from) == Ok(v))
            || Some(v) == gw
    };
    if let Some(a) = asked.filter(|a| !a.is_empty()) {
        let ip: Ipv4Addr = a
            .parse()
            .map_err(|_| format!("'{a}' is not an IPv4 address"))?;
        let v = u32::from(ip);
        if v <= lo || v >= hi {
            return Err(format!("{ip} is not a host address of {cidr}"));
        }
        if used(v) {
            return Err(format!("{ip} is already allocated or is the gateway"));
        }
        return Ok(ip);
    }
    (lo + 1..hi)
        .find(|v| !used(*v))
        .map(Ipv4Addr::from)
        .ok_or_else(|| format!("{cidr} has no free address left"))
}

pub fn host_count(cidr: &str) -> u32 {
    parse_cidr(cidr)
        .map(|(_, p)| (1u32 << (32 - p)) - 2)
        .unwrap_or(0)
}

// ------------------------------------------------------------- kernel effects

/// SNAT rules for every subnet that asks for it (replaced as one nft
/// transaction, so a half-applied ruleset never exists).
pub fn render_nft(subnets: &[Subnet]) -> String {
    let mut s = String::from("table inet horcrux_sdn {\n  chain postrouting {\n    type nat hook postrouting priority srcnat; policy accept;\n");
    for sub in subnets.iter().filter(|s| s.snat) {
        s += &format!(
            "    ip saddr {} oifname != \"{}\" masquerade\n",
            sub.cidr, sub.vnet
        );
    }
    s += "  }\n}\n";
    s
}

async fn link_exists(name: &str) -> bool {
    run("ip", &["link", "show", "dev", name]).await.is_ok()
}

fn tolerable(e: &str) -> bool {
    e.contains("File exists") || e.contains("already")
}

async fn ip(args: &[&str]) -> Result<(), String> {
    match run("ip", args).await {
        Ok(_) => Ok(()),
        Err(e) if tolerable(&e) => Ok(()),
        Err(e) => Err(e),
    }
}

pub fn carrier_name(z: &Zone, v: &VNet) -> Option<String> {
    match (z.zone_type.as_str(), v.tag) {
        ("vlan", Some(t)) => Some(format!("{}v{}", v.id, t)),
        ("vxlan", Some(t)) => Some(format!("vx{t}")),
        _ => None,
    }
}

async fn apply_vnet(z: &Zone, v: &VNet, subnets: &[Subnet]) -> Result<(), String> {
    let br = v.id.as_str();
    if !link_exists(br).await {
        let mut a = vec!["link", "add", "name", br, "type", "bridge"];
        if v.vlan_aware {
            a.extend(["vlan_filtering", "1"]);
        }
        run("ip", &a).await?;
    }
    if let Some(m) = z.mtu {
        ip(&["link", "set", "dev", br, "mtu", &m.to_string()]).await?;
    }
    if let Some(car) = carrier_name(z, v) {
        let tag = v.tag.unwrap_or(0).to_string();
        if !link_exists(&car).await {
            match z.zone_type.as_str() {
                "vlan" => {
                    let up = z.uplink.as_deref().unwrap_or("");
                    if !link_exists(up).await {
                        return Err(format!("uplink {up} does not exist on this node"));
                    }
                    run(
                        "ip",
                        &[
                            "link", "add", "link", up, "name", &car, "type", "vlan", "id", &tag,
                        ],
                    )
                    .await?;
                }
                _ => {
                    let port = z.dst_port.unwrap_or(4789).to_string();
                    let mut a = vec![
                        "link", "add", &car, "type", "vxlan", "id", &tag, "dstport", &port,
                    ];
                    if let Some(l) = z.local_ip.as_deref().filter(|l| !l.is_empty()) {
                        a.extend(["local", l]);
                    }
                    run("ip", &a).await?;
                }
            }
        }
        if let Some(m) = z.mtu {
            ip(&["link", "set", "dev", &car, "mtu", &m.to_string()]).await?;
        }
        ip(&["link", "set", "dev", &car, "master", br]).await?;
        ip(&["link", "set", "dev", &car, "up"]).await?;
        if z.zone_type == "vxlan" {
            sync_flood_list(&car, &z.peers).await?;
        }
    }
    for s in subnets.iter().filter(|s| s.vnet == v.id) {
        if let (Some(g), Ok((_, p))) = (
            s.gateway.as_deref().filter(|g| !g.is_empty()),
            parse_cidr(&s.cidr),
        ) {
            ip(&["addr", "replace", &format!("{g}/{p}"), "dev", br]).await?;
        }
    }
    ip(&["link", "set", "dev", br, "up"]).await
}

/// Head-end replication list of a VXLAN device: exactly `peers`.
async fn sync_flood_list(dev: &str, peers: &[String]) -> Result<(), String> {
    let cur = run("bridge", &["fdb", "show", "dev", dev])
        .await
        .unwrap_or_default();
    let have: Vec<String> = cur
        .lines()
        .filter(|l| l.starts_with("00:00:00:00:00:00"))
        .filter_map(|l| {
            l.split_whitespace()
                .skip_while(|w| *w != "dst")
                .nth(1)
                .map(String::from)
        })
        .collect();
    for h in have.iter().filter(|h| !peers.contains(h)) {
        let _ = run(
            "bridge",
            &["fdb", "del", "00:00:00:00:00:00", "dev", dev, "dst", h],
        )
        .await;
    }
    for p in peers.iter().filter(|p| !have.contains(p)) {
        match run(
            "bridge",
            &["fdb", "append", "00:00:00:00:00:00", "dev", dev, "dst", p],
        )
        .await
        {
            Ok(_) => {}
            Err(e) if tolerable(&e) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

async fn teardown_vnet(z: Option<&Zone>, v: &VNet) {
    if let Some(car) = z.and_then(|z| carrier_name(z, v)) {
        let _ = run("ip", &["link", "del", &car]).await;
    }
    let _ = run("ip", &["link", "del", &v.id]).await;
}

async fn apply_nft(subnets: &[Subnet]) -> Result<(), String> {
    if !subnets.iter().any(|s| s.snat) {
        // nothing to masquerade: drop our table if it is there
        let _ = run("nft", &["delete", "table", "inet", "horcrux_sdn"]).await;
        return Ok(());
    }
    if which("nft").is_none() {
        return Err("SNAT needs nftables (nft) on this node".into());
    }
    let _ = run("nft", &["delete", "table", "inet", "horcrux_sdn"]).await;
    let file = std::env::temp_dir().join(format!("horcrux-sdn-{}.nft", std::process::id()));
    std::fs::write(&file, render_nft(subnets)).map_err(|e| e.to_string())?;
    let r = run("nft", &["-f", &file.to_string_lossy()]).await;
    let _ = std::fs::remove_file(&file);
    r?;
    let _ = std::fs::write("/proc/sys/net/ipv4/ip_forward", "1");
    Ok(())
}

// --------------------------------------------------------------------- storage

struct Cfg {
    store: ConfigStore,
}

impl Cfg {
    fn new(state: &Arc<AppState>) -> Self {
        Cfg {
            store: ConfigStore::new(&state.database),
        }
    }
    async fn zones(&self) -> Vec<Zone> {
        let mut v = self
            .store
            .load_values::<Zone>(kinds::SDN_ZONE)
            .await
            .unwrap_or_default();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }
    async fn vnets(&self) -> Vec<VNet> {
        let mut v = self
            .store
            .load_values::<VNet>(kinds::SDN_VNET)
            .await
            .unwrap_or_default();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }
    async fn subnets(&self) -> Vec<Subnet> {
        let mut v = self
            .store
            .load_values::<Subnet>(kinds::SDN_SUBNET)
            .await
            .unwrap_or_default();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }
    async fn allocations(&self, subnet: &str) -> Vec<Allocation> {
        let mut v: Vec<Allocation> = self
            .store
            .load_values::<Allocation>(kinds::SDN_IPAM)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|a| a.subnet == subnet)
            .collect();
        v.sort_by_key(|a| a.ip.parse::<Ipv4Addr>().map(u32::from).unwrap_or(0));
        v
    }
}

fn internal<E: std::fmt::Display>(e: E) -> ApiError {
    ApiError::Internal(e.to_string())
}

async fn apply_everything(cfg: &Cfg) -> Vec<(String, Result<(), String>)> {
    let zones = cfg.zones().await;
    let subnets = cfg.subnets().await;
    let mut out = Vec::new();
    for v in cfg.vnets().await {
        let r = match zones.iter().find(|z| z.id == v.zone) {
            Some(z) => apply_vnet(z, &v, &subnets).await,
            None => Err(format!("zone {} is gone", v.zone)),
        };
        out.push((v.id.clone(), r));
    }
    if let Err(e) = apply_nft(&subnets).await {
        out.push(("snat".into(), Err(e)));
    }
    out
}

fn spawn_reapply(state: Arc<AppState>) {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        for (what, r) in apply_everything(&Cfg::new(&state)).await {
            if let Err(e) = r {
                tracing::warn!("SDN: re-applying {what}: {e}");
            }
        }
    });
}

// -------------------------------------------------------------------- handlers

async fn list_zones(State(state): State<Arc<AppState>>) -> Json<Vec<Zone>> {
    Json(Cfg::new(&state).zones().await)
}

async fn get_zone(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Res<Zone> {
    Cfg::new(&state)
        .zones()
        .await
        .into_iter()
        .find(|z| z.id == id)
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("zone {id} not found")))
}

async fn create_zone(State(state): State<Arc<AppState>>, Json(z): Json<Zone>) -> Res<Zone> {
    validate_zone(&z).map_err(ApiError::ValidationError)?;
    let cfg = Cfg::new(&state);
    if cfg.zones().await.iter().any(|x| x.id == z.id) {
        return Err(ApiError::Conflict(format!("zone {} already exists", z.id)));
    }
    cfg.store
        .put(kinds::SDN_ZONE, &z.id, &z)
        .await
        .map_err(internal)?;
    Ok(Json(z))
}

async fn update_zone(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(mut z): Json<Zone>,
) -> Res<Zone> {
    z.id = id.clone();
    validate_zone(&z).map_err(ApiError::ValidationError)?;
    let cfg = Cfg::new(&state);
    let old = cfg
        .zones()
        .await
        .into_iter()
        .find(|x| x.id == id)
        .ok_or_else(|| ApiError::NotFound(format!("zone {id} not found")))?;
    if old.zone_type != z.zone_type && cfg.vnets().await.iter().any(|v| v.zone == id) {
        return Err(ApiError::Conflict(
            "the zone type can't change while VNets use the zone".into(),
        ));
    }
    cfg.store
        .put(kinds::SDN_ZONE, &id, &z)
        .await
        .map_err(internal)?;
    // peers/MTU/uplink changes reach the kernel now
    let subnets = cfg.subnets().await;
    for v in cfg.vnets().await.iter().filter(|v| v.zone == id) {
        apply_vnet(&z, v, &subnets)
            .await
            .map_err(ApiError::Internal)?;
    }
    Ok(Json(z))
}

async fn delete_zone(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Res<serde_json::Value> {
    let cfg = Cfg::new(&state);
    if let Some(v) = cfg.vnets().await.iter().find(|v| v.zone == id) {
        return Err(ApiError::Conflict(format!(
            "VNet {} still uses zone {id}; delete it first",
            v.id
        )));
    }
    if !cfg
        .store
        .delete(kinds::SDN_ZONE, &id)
        .await
        .map_err(internal)?
    {
        return Err(ApiError::NotFound(format!("zone {id} not found")));
    }
    Ok(Json(serde_json::json!({"deleted": id})))
}

async fn list_vnets(State(state): State<Arc<AppState>>) -> Json<Vec<VNet>> {
    Json(Cfg::new(&state).vnets().await)
}

async fn create_vnet(State(state): State<Arc<AppState>>, Json(v): Json<VNet>) -> Res<VNet> {
    let cfg = Cfg::new(&state);
    let zone = cfg
        .zones()
        .await
        .into_iter()
        .find(|z| z.id == v.zone)
        .ok_or_else(|| ApiError::ValidationError(format!("zone {} does not exist", v.zone)))?;
    validate_vnet(&v, &zone).map_err(ApiError::ValidationError)?;
    let all = cfg.vnets().await;
    if all.iter().any(|x| x.id == v.id) {
        return Err(ApiError::Conflict(format!("VNet {} already exists", v.id)));
    }
    if let (Some(t), true) = (v.tag, zone.zone_type != "simple") {
        if all.iter().any(|x| x.tag == Some(t) && x.zone == v.zone) {
            return Err(ApiError::Conflict(format!(
                "tag {t} is already used in zone {}",
                zone.id
            )));
        }
    }
    // The bridge is named after the VNet: never adopt an interface that exists already.
    if link_exists(&v.id).await {
        return Err(ApiError::Conflict(format!(
            "an interface named {} already exists on this node; pick another VNet id",
            v.id
        )));
    }
    if let Err(e) = apply_vnet(&zone, &v, &[]).await {
        teardown_vnet(Some(&zone), &v).await;
        return Err(ApiError::Internal(e));
    }
    cfg.store
        .put(kinds::SDN_VNET, &v.id, &v)
        .await
        .map_err(internal)?;
    Ok(Json(v))
}

async fn delete_vnet(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Res<serde_json::Value> {
    let cfg = Cfg::new(&state);
    let v = cfg
        .vnets()
        .await
        .into_iter()
        .find(|v| v.id == id)
        .ok_or_else(|| ApiError::NotFound(format!("VNet {id} not found")))?;
    if let Some(s) = cfg.subnets().await.iter().find(|s| s.vnet == id) {
        return Err(ApiError::Conflict(format!(
            "subnet {} still belongs to VNet {id}; delete it first",
            s.cidr
        )));
    }
    let zone = cfg.zones().await.into_iter().find(|z| z.id == v.zone);
    teardown_vnet(zone.as_ref(), &v).await;
    cfg.store
        .delete(kinds::SDN_VNET, &id)
        .await
        .map_err(internal)?;
    Ok(Json(serde_json::json!({"deleted": id})))
}

async fn list_subnets(State(state): State<Arc<AppState>>) -> Json<Vec<Subnet>> {
    Json(Cfg::new(&state).subnets().await)
}

async fn create_subnet(
    State(state): State<Arc<AppState>>,
    Path(vnet): Path<String>,
    Json(mut s): Json<Subnet>,
) -> Res<Subnet> {
    let cfg = Cfg::new(&state);
    let v = cfg
        .vnets()
        .await
        .into_iter()
        .find(|v| v.id == vnet)
        .ok_or_else(|| ApiError::NotFound(format!("VNet {vnet} not found")))?;
    s.vnet = vnet.clone();
    let existing = cfg.subnets().await;
    let (net, p) = validate_subnet(&s, &existing).map_err(ApiError::ValidationError)?;
    s.id = subnet_id(&vnet, net, p);
    s.gateway = s.gateway.filter(|g| !g.is_empty());
    if existing.iter().any(|e| e.id == s.id) {
        return Err(ApiError::Conflict(format!(
            "subnet {} already exists",
            s.cidr
        )));
    }
    cfg.store
        .put(kinds::SDN_SUBNET, &s.id, &s)
        .await
        .map_err(internal)?;
    let zones = cfg.zones().await;
    let zone = zones
        .iter()
        .find(|z| z.id == v.zone)
        .ok_or_else(|| internal("zone missing"))?;
    let all = cfg.subnets().await;
    let applied = async {
        apply_vnet(zone, &v, &all).await?;
        apply_nft(&all).await
    }
    .await;
    if let Err(e) = applied {
        let _ = cfg.store.delete(kinds::SDN_SUBNET, &s.id).await;
        return Err(ApiError::Internal(e));
    }
    // the gateway address is taken: record it so IPAM never hands it out
    if let Some(g) = &s.gateway {
        let a = Allocation {
            subnet: s.id.clone(),
            ip: g.clone(),
            hostname: Some("gateway".into()),
            mac: None,
            kind: "gateway".into(),
            created: chrono::Utc::now().to_rfc3339(),
        };
        cfg.store
            .put(kinds::SDN_IPAM, &format!("{}/{}", s.id, g), &a)
            .await
            .map_err(internal)?;
    }
    Ok(Json(s))
}

async fn delete_subnet(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Res<serde_json::Value> {
    let cfg = Cfg::new(&state);
    let s = cfg
        .subnets()
        .await
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| ApiError::NotFound(format!("subnet {id} not found")))?;
    let allocs = cfg.allocations(&id).await;
    if let Some(a) = allocs.iter().find(|a| a.kind != "gateway") {
        return Err(ApiError::Conflict(format!(
            "{} is still allocated to {}; release the addresses first",
            a.ip,
            a.hostname.as_deref().unwrap_or("a guest")
        )));
    }
    if let (Some(g), Ok((_, p))) = (&s.gateway, parse_cidr(&s.cidr)) {
        let _ = run("ip", &["addr", "del", &format!("{g}/{p}"), "dev", &s.vnet]).await;
    }
    for a in allocs {
        let _ = cfg
            .store
            .delete(kinds::SDN_IPAM, &format!("{}/{}", id, a.ip))
            .await;
    }
    cfg.store
        .delete(kinds::SDN_SUBNET, &id)
        .await
        .map_err(internal)?;
    let rest = cfg.subnets().await;
    apply_nft(&rest).await.map_err(ApiError::Internal)?;
    Ok(Json(serde_json::json!({"deleted": id})))
}

async fn find_subnet(cfg: &Cfg, id: &str) -> Result<Subnet, ApiError> {
    cfg.subnets()
        .await
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| ApiError::NotFound(format!("subnet {id} not found")))
}

async fn ipam_list(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Res<IpamView> {
    let cfg = Cfg::new(&state);
    let s = find_subnet(&cfg, &id).await?;
    let allocations = cfg.allocations(&id).await;
    let total = host_count(&s.cidr);
    let used = allocations.len() as u32;
    Ok(Json(IpamView {
        subnet: id,
        total,
        used,
        free: total.saturating_sub(used),
        allocations,
    }))
}

async fn ipam_allocate(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<AllocateRequest>,
) -> Res<Allocation> {
    let cfg = Cfg::new(&state);
    let s = find_subnet(&cfg, &id).await?;
    if let Some(m) = req.mac.as_deref().filter(|m| !m.is_empty()) {
        if !valid_mac(m) {
            return Err(ApiError::ValidationError(format!(
                "'{m}' is not a MAC address (aa:bb:cc:dd:ee:ff)"
            )));
        }
    }
    if let Some(h) = req.hostname.as_deref().filter(|h| !h.is_empty()) {
        if !valid_hostname(h) {
            return Err(ApiError::ValidationError(format!(
                "'{h}' is not a valid host name"
            )));
        }
    }
    let existing = cfg.allocations(&id).await;
    if let Some(h) = req.hostname.as_deref().filter(|h| !h.is_empty()) {
        if existing.iter().any(|a| a.hostname.as_deref() == Some(h)) {
            return Err(ApiError::Conflict(format!(
                "host name {h} already has an address in this subnet"
            )));
        }
    }
    let taken: Vec<String> = existing.iter().map(|a| a.ip.clone()).collect();
    let ip = pick_ip(&s.cidr, s.gateway.as_deref(), &taken, req.ip.as_deref()).map_err(|e| {
        if e.contains("already allocated") || e.contains("no free") {
            ApiError::Conflict(e)
        } else {
            ApiError::ValidationError(e)
        }
    })?;
    let a = Allocation {
        subnet: id.clone(),
        ip: ip.to_string(),
        hostname: req.hostname.filter(|h| !h.is_empty()),
        mac: req
            .mac
            .filter(|m| !m.is_empty())
            .map(|m| m.to_ascii_lowercase()),
        kind: "static".into(),
        created: chrono::Utc::now().to_rfc3339(),
    };
    cfg.store
        .put(kinds::SDN_IPAM, &format!("{id}/{ip}"), &a)
        .await
        .map_err(internal)?;
    Ok(Json(a))
}

async fn ipam_release(
    State(state): State<Arc<AppState>>,
    Path((id, ip)): Path<(String, String)>,
) -> Res<serde_json::Value> {
    let cfg = Cfg::new(&state);
    find_subnet(&cfg, &id).await?;
    let a = cfg
        .allocations(&id)
        .await
        .into_iter()
        .find(|a| a.ip == ip)
        .ok_or_else(|| ApiError::NotFound(format!("{ip} is not allocated in {id}")))?;
    if a.kind == "gateway" {
        return Err(ApiError::Conflict(
            "the gateway address belongs to the subnet; delete the subnet instead".into(),
        ));
    }
    cfg.store
        .delete(kinds::SDN_IPAM, &format!("{id}/{ip}"))
        .await
        .map_err(internal)?;
    Ok(Json(serde_json::json!({"released": ip})))
}

async fn status(State(state): State<Arc<AppState>>) -> Json<Vec<VNetStatus>> {
    let cfg = Cfg::new(&state);
    let zones = cfg.zones().await;
    let mut out = Vec::new();
    for v in cfg.vnets().await {
        let z = zones.iter().find(|z| z.id == v.zone);
        let carrier = z.and_then(|z| carrier_name(z, &v));
        let j = run("ip", &["-j", "-d", "addr", "show", "dev", &v.id])
            .await
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
        let l = j.as_ref().and_then(|j| j.get(0));
        let ports = run("ip", &["-j", "link", "show", "master", &v.id])
            .await
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<serde_json::Value>>(&s).ok())
            .unwrap_or_default()
            .iter()
            .filter_map(|p| p["ifname"].as_str().map(String::from))
            .collect();
        out.push(VNetStatus {
            bridge_present: l.is_some(),
            up: l.is_some_and(|l| {
                l["operstate"] != "DOWN"
                    || l["flags"]
                        .as_array()
                        .is_some_and(|f| f.iter().any(|x| x == "UP"))
            }),
            addresses: l
                .and_then(|l| l["addr_info"].as_array())
                .map(|a| {
                    a.iter()
                        .filter(|x| x["family"] == "inet")
                        .map(|x| {
                            format!("{}/{}", x["local"].as_str().unwrap_or(""), x["prefixlen"])
                        })
                        .collect()
                })
                .unwrap_or_default(),
            ports,
            carrier_present: match &carrier {
                Some(c) => link_exists(c).await,
                None => true,
            },
            carrier,
            error: if z.is_none() {
                Some(format!("zone {} is missing", v.zone))
            } else {
                None
            },
            vnet: v.id,
        });
    }
    Json(out)
}

async fn apply_all(State(state): State<Arc<AppState>>) -> Res<Vec<serde_json::Value>> {
    let r = apply_everything(&Cfg::new(&state)).await;
    Ok(Json(
        r.into_iter()
            .map(|(what, r)| serde_json::json!({"target": what, "ok": r.is_ok(), "error": r.err()}))
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone(t: &str) -> Zone {
        Zone {
            id: "z1".into(),
            zone_type: t.into(),
            description: String::new(),
            uplink: None,
            local_ip: None,
            peers: vec![],
            mtu: None,
            dst_port: None,
        }
    }

    #[test]
    fn zones_are_validated_per_type() {
        assert!(validate_zone(&zone("simple")).is_ok());
        assert!(validate_zone(&zone("vlan")).unwrap_err().contains("uplink"));
        assert!(validate_zone(&Zone {
            uplink: Some("eth0".into()),
            ..zone("vlan")
        })
        .is_ok());
        assert!(validate_zone(&zone("vxlan")).unwrap_err().contains("peer"));
        assert!(validate_zone(&Zone {
            peers: vec!["10.0.0.2".into()],
            local_ip: Some("10.0.0.1".into()),
            ..zone("vxlan")
        })
        .is_ok());
        assert!(validate_zone(&Zone {
            peers: vec!["nope".into()],
            ..zone("vxlan")
        })
        .is_err());
        assert!(validate_zone(&zone("evpn")).is_err());
        assert!(validate_zone(&Zone {
            id: "Bad Id".into(),
            ..zone("simple")
        })
        .is_err());
        assert!(validate_zone(&Zone {
            mtu: Some(100),
            ..zone("simple")
        })
        .is_err());
    }

    #[test]
    fn vnets_need_a_tag_matching_the_zone() {
        let v = |id: &str, tag| VNet {
            id: id.into(),
            zone: "z1".into(),
            alias: None,
            tag,
            vlan_aware: false,
        };
        assert!(validate_vnet(&v("web", None), &zone("simple")).is_ok());
        assert!(validate_vnet(&v("web", None), &zone("vlan")).is_err());
        assert!(validate_vnet(&v("web", Some(4095)), &zone("vlan")).is_err());
        assert!(validate_vnet(&v("web", Some(100)), &zone("vlan")).is_ok());
        assert!(validate_vnet(&v("web", Some(0)), &zone("vxlan")).is_err());
        assert!(validate_vnet(&v("web", Some(16_777_215)), &zone("vxlan")).is_ok());
        for bad in ["", "Web", "1web", "toolongname", "we.b", "we-b"] {
            assert!(
                validate_vnet(&v(bad, None), &zone("simple")).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn cidr_gateway_and_overlap_rules() {
        assert_eq!(
            parse_cidr("10.20.0.0/24").unwrap(),
            (Ipv4Addr::new(10, 20, 0, 0), 24)
        );
        assert!(parse_cidr("10.20.0.5/24")
            .unwrap_err()
            .contains("host bits"));
        assert!(
            parse_cidr("10.20.0.0/31").is_err()
                && parse_cidr("10.0.0.0/7").is_err()
                && parse_cidr("x/24").is_err()
        );
        let s = |cidr: &str, gw: Option<&str>, snat| Subnet {
            id: String::new(),
            vnet: "web".into(),
            cidr: cidr.into(),
            gateway: gw.map(String::from),
            snat,
        };
        assert!(validate_subnet(&s("10.20.0.0/24", Some("10.20.0.1"), true), &[]).is_ok());
        assert!(
            validate_subnet(&s("10.20.0.0/24", Some("10.20.0.0"), false), &[]).is_err(),
            "network address"
        );
        assert!(
            validate_subnet(&s("10.20.0.0/24", Some("10.20.0.255"), false), &[]).is_err(),
            "broadcast"
        );
        assert!(
            validate_subnet(&s("10.20.0.0/24", Some("10.30.0.1"), false), &[]).is_err(),
            "outside"
        );
        assert!(validate_subnet(&s("10.20.0.0/24", None, true), &[])
            .unwrap_err()
            .contains("gateway"));
        let have = vec![Subnet {
            id: "web-10.20.0.0-24".into(),
            ..s("10.20.0.0/24", None, false)
        }];
        assert!(validate_subnet(&s("10.20.0.128/25", None, false), &have)
            .unwrap_err()
            .contains("overlaps"));
        assert!(validate_subnet(&s("10.21.0.0/24", None, false), &have).is_ok());
        assert_eq!(
            subnet_id("web", Ipv4Addr::new(10, 20, 0, 0), 24),
            "web-10.20.0.0-24"
        );
    }

    #[test]
    fn ipam_picks_the_lowest_free_host_and_honours_requests() {
        let taken = vec!["10.20.0.1".to_string(), "10.20.0.2".to_string()];
        assert_eq!(
            pick_ip("10.20.0.0/24", Some("10.20.0.1"), &taken, None)
                .unwrap()
                .to_string(),
            "10.20.0.3"
        );
        assert_eq!(
            pick_ip("10.20.0.0/24", Some("10.20.0.1"), &[], None)
                .unwrap()
                .to_string(),
            "10.20.0.2",
            "gateway skipped"
        );
        assert_eq!(
            pick_ip("10.20.0.0/24", None, &taken, Some("10.20.0.50"))
                .unwrap()
                .to_string(),
            "10.20.0.50"
        );
        assert!(pick_ip("10.20.0.0/24", None, &taken, Some("10.20.0.2"))
            .unwrap_err()
            .contains("already"));
        assert!(pick_ip("10.20.0.0/24", None, &taken, Some("10.20.0.0")).is_err());
        assert!(pick_ip("10.20.0.0/24", None, &taken, Some("10.20.0.255")).is_err());
        assert!(pick_ip("10.20.0.0/24", None, &taken, Some("10.99.0.5")).is_err());
        // a /30 has two hosts
        let full = vec!["10.0.0.1".to_string(), "10.0.0.2".to_string()];
        assert!(pick_ip("10.0.0.0/30", None, &full, None)
            .unwrap_err()
            .contains("no free"));
        assert_eq!(host_count("10.0.0.0/30"), 2);
        assert_eq!(host_count("10.20.0.0/24"), 254);
    }

    #[test]
    fn mac_and_host_names() {
        assert!(
            valid_mac("aa:bb:cc:dd:ee:0f")
                && !valid_mac("aa:bb:cc:dd:ee")
                && !valid_mac("zz:bb:cc:dd:ee:ff")
        );
        assert!(valid_hostname("vm-1") && !valid_hostname("-vm") && !valid_hostname("a b"));
    }

    #[test]
    fn nft_table_masquerades_only_snat_subnets() {
        let s = |cidr: &str, snat| Subnet {
            id: String::new(),
            vnet: "web".into(),
            cidr: cidr.into(),
            gateway: Some("x".into()),
            snat,
        };
        let r = render_nft(&[s("10.20.0.0/24", true), s("10.21.0.0/24", false)]);
        assert!(r.contains("ip saddr 10.20.0.0/24 oifname != \"web\" masquerade"));
        assert!(!r.contains("10.21.0.0"));
        assert!(r.starts_with("table inet horcrux_sdn"));
    }

    #[test]
    fn carriers_are_named_after_the_vnet_or_the_vni() {
        let v = VNet {
            id: "web".into(),
            zone: "z1".into(),
            alias: None,
            tag: Some(100),
            vlan_aware: false,
        };
        assert_eq!(carrier_name(&zone("vlan"), &v).as_deref(), Some("webv100"));
        assert_eq!(carrier_name(&zone("vxlan"), &v).as_deref(), Some("vx100"));
        assert_eq!(carrier_name(&zone("simple"), &v), None);
    }
}
