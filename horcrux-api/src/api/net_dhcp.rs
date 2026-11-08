//! DHCP + DNS for the guests of a network (a host bridge, a VLAN of a
//! bridge, or an SDN VNet - they are all Linux bridges), served by one
//! `dnsmasq` per network that Horcrux writes the config for, starts, keeps
//! running and stops.
//!
//! * `GET/POST /api/network/dhcp`, `GET/PUT/DELETE /api/network/dhcp/:id`
//! * `POST /api/network/dhcp/:id/static-leases`, `DELETE .../static-leases/:mac`
//! * `GET  /api/network/dhcp/:id/leases`  the leases dnsmasq handed out
//!
//! A network = bridge (+ optional `vlan`: Horcrux then creates the
//! `<bridge>.<vlan>` interface dnsmasq listens on), the server address
//! (`address`, CIDR), the dynamic range, gateway / DNS servers handed to the
//! clients, a DNS domain, static leases (MAC -> IP + hostname) and extra DNS
//! records. DNS answers the names of leases and static hosts and forwards
//! the rest upstream.
//!
//! Cluster: the definitions are replicated to every node (pushed on change,
//! pulled every 15 s, newest `rev` wins) but **only `serve_node` runs the
//! dnsmasq of a network**, so a bridge/VLAN stretched over several nodes has
//! exactly one DHCP server. Moving a network = `PUT` with another
//! `serve_node` (dynamic leases are not carried over; static ones are
//! config).
use crate::db::config_store::ConfigStore;
use crate::error::ApiError;
use crate::middleware::auth::{AuthUser, CLUSTER_HA_ROLE};
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
    Extension, Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use tokio::process::Command;

const KIND: &str = "dhcp_network";

pub fn routes(state: &Arc<AppState>) -> Router<Arc<AppState>> {
    spawn_reconciler(state.clone());
    Router::new()
        .route("/api/network/dhcp", get(list).post(create))
        .route(
            "/api/network/dhcp/:id",
            get(get_one).put(update).delete(remove),
        )
        .route("/api/network/dhcp/:id/leases", get(active_leases))
        .route("/api/network/dhcp/:id/static-leases", post(add_static))
        .route(
            "/api/network/dhcp/:id/static-leases/:mac",
            delete(remove_static),
        )
        .route("/api/ha/peer/dhcp/sync", post(peer_sync))
        .route("/api/ha/peer/dhcp/list", post(peer_list))
        .route("/api/ha/peer/dhcp/leases", post(peer_leases))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StaticLease {
    pub mac: String,
    pub ip: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DnsRecord {
    pub name: String,
    pub ip: String,
}

fn yes() -> bool {
    true
}
fn default_lease() -> String {
    "12h".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DhcpNetwork {
    /// Defaults to the interface name (`vmbr1`, `vmbr1.20`).
    #[serde(default)]
    pub id: String,
    pub bridge: String,
    /// Serve this VLAN of a VLAN-aware bridge (interface `<bridge>.<vlan>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vlan: Option<u16>,
    /// The DHCP/DNS server's own address on the network, CIDR (192.168.20.1/24).
    pub address: String,
    pub range_start: String,
    pub range_end: String,
    /// Default gateway offered to clients (none = no router option).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway: Option<String>,
    /// DNS servers offered to clients (default: the server address when DNS is on).
    #[serde(default)]
    pub dns_servers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// `12h`, `30m`, `3600`, `infinite`.
    #[serde(default = "default_lease")]
    pub lease_time: String,
    /// Run the DNS side too (port 53 on the server address).
    #[serde(default = "yes")]
    pub dns_enabled: bool,
    /// Upstream resolvers for names that are not ours (default: the host's).
    #[serde(default)]
    pub upstream_dns: Vec<String>,
    #[serde(default)]
    pub static_leases: Vec<StaticLease>,
    #[serde(default)]
    pub dns_records: Vec<DnsRecord>,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// The one node that runs dnsmasq for this network (default: the node
    /// that created it).
    #[serde(default)]
    pub serve_node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Change counter (unix ms) for the cluster merge.
    #[serde(default)]
    pub rev: u64,
    /// Tombstone, so a delete reaches nodes that were offline.
    #[serde(default)]
    pub deleted: bool,
}

impl DhcpNetwork {
    /// The interface dnsmasq binds to.
    pub fn iface(&self) -> String {
        match self.vlan {
            Some(v) => format!("{}.{}", self.bridge, v),
            None => self.bridge.clone(),
        }
    }
}

/// What the API returns: the network plus this node's view of it.
#[derive(Debug, Serialize)]
struct DhcpView {
    #[serde(flatten)]
    net: DhcpNetwork,
    iface: String,
    /// this node is the serving node
    serving_here: bool,
    /// dnsmasq of this network is running on this node
    running: bool,
}

// ------------------------------------------------------------- validation

fn v4(s: &str, what: &str) -> Result<Ipv4Addr, String> {
    s.trim()
        .parse::<Ipv4Addr>()
        .map_err(|_| format!("{what} {s:?} is not an IPv4 address"))
}

fn parse_cidr(s: &str) -> Result<(Ipv4Addr, u8), String> {
    let (ip, len) = s
        .split_once('/')
        .ok_or_else(|| format!("address {s:?} must be CIDR, e.g. 192.168.20.1/24"))?;
    let ip = v4(ip, "address")?;
    let len: u8 = len
        .parse()
        .map_err(|_| format!("bad prefix length in {s:?}"))?;
    if !(8..=30).contains(&len) {
        return Err(format!("prefix length /{len} must be between /8 and /30"));
    }
    Ok((ip, len))
}

fn mask_of(len: u8) -> u32 {
    u32::MAX << (32 - len as u32)
}

fn in_subnet(ip: Ipv4Addr, net: Ipv4Addr, len: u8) -> bool {
    u32::from(ip) & mask_of(len) == u32::from(net) & mask_of(len)
}

fn valid_label(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && !s.starts_with('-')
        && !s.ends_with('-')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn valid_dns_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 253 && s.split('.').all(valid_label)
}

fn valid_lease_time(s: &str) -> bool {
    s == "infinite"
        || (s.is_ascii()
            && !s.is_empty()
            && s[..s.len() - 1].chars().all(|c| c.is_ascii_digit())
            && s.chars().next().is_some_and(|c| c.is_ascii_digit())
            && s.chars()
                .last()
                .is_some_and(|c| c.is_ascii_digit() || "smhd".contains(c)))
}

/// Normalise (trim, lower-case MACs) and validate; the host-independent part.
pub fn normalize_and_validate(n: &mut DhcpNetwork) -> Result<(), String> {
    n.bridge = n.bridge.trim().to_string();
    n.address = n.address.trim().to_string();
    if !horcrux_common::valid_ifname(&n.bridge) {
        return Err(format!("{:?} is not a valid bridge name", n.bridge));
    }
    if let Some(v) = n.vlan {
        if !(1..=4094).contains(&v) {
            return Err(format!("VLAN {v} is outside 1-4094"));
        }
    }
    let iface = n.iface();
    if !horcrux_common::valid_ifname(&iface) {
        return Err(format!(
            "interface name {iface:?} is too long (max 15 characters): use a shorter bridge name"
        ));
    }
    if n.id.trim().is_empty() {
        n.id = iface.clone();
    }
    n.id = n.id.trim().to_string();
    if !horcrux_common::valid_ifname(&n.id) {
        return Err(format!("id {:?} must be 1-15 of a-z 0-9 - _ .", n.id));
    }
    let (server, len) = parse_cidr(&n.address)?;
    let subnet = Ipv4Addr::from(u32::from(server) & mask_of(len));
    let start = v4(&n.range_start, "range_start")?;
    let end = v4(&n.range_end, "range_end")?;
    if !in_subnet(start, subnet, len) || !in_subnet(end, subnet, len) {
        return Err(format!("the range must lie inside {subnet}/{len}"));
    }
    if u32::from(start) > u32::from(end) {
        return Err("range_start is after range_end".into());
    }
    if (u32::from(start)..=u32::from(end)).contains(&u32::from(server)) {
        return Err("the server address must not be inside the dynamic range".into());
    }
    if let Some(g) = n
        .gateway
        .as_deref()
        .map(str::trim)
        .filter(|g| !g.is_empty())
    {
        let g = v4(g, "gateway")?;
        if !in_subnet(g, subnet, len) {
            return Err(format!("gateway {g} is outside {subnet}/{len}"));
        }
        n.gateway = Some(g.to_string());
    } else {
        n.gateway = None;
    }
    for d in n.dns_servers.iter().chain(n.upstream_dns.iter()) {
        d.trim()
            .parse::<IpAddr>()
            .map_err(|_| format!("DNS server {d:?} is not an IP address"))?;
    }
    n.dns_servers = n.dns_servers.iter().map(|d| d.trim().to_string()).collect();
    n.upstream_dns = n
        .upstream_dns
        .iter()
        .map(|d| d.trim().to_string())
        .collect();
    n.domain = n
        .domain
        .take()
        .map(|d| d.trim().trim_matches('.').to_lowercase())
        .filter(|d| !d.is_empty());
    if let Some(d) = &n.domain {
        if !valid_dns_name(d) {
            return Err(format!("domain {d:?} is not a valid DNS name"));
        }
    }
    n.lease_time = n.lease_time.trim().to_string();
    if !valid_lease_time(&n.lease_time) {
        return Err(format!(
            "lease_time {:?} must look like 12h, 30m, 3600 or infinite",
            n.lease_time
        ));
    }
    let (mut macs, mut ips, mut hosts) = (HashSet::new(), HashSet::new(), HashSet::new());
    for l in &mut n.static_leases {
        l.mac = horcrux_common::normalize_mac(&l.mac)?;
        let ip = v4(&l.ip, "static lease address")?;
        l.ip = ip.to_string();
        if !in_subnet(ip, subnet, len) || ip == server {
            return Err(format!(
                "static lease {ip} must be inside {subnet}/{len} and not the server's address"
            ));
        }
        if let Some(h) = l.hostname.take().map(|h| h.trim().to_lowercase()) {
            if !h.is_empty() {
                if !valid_label(&h) {
                    return Err(format!("hostname {h:?} is not a valid host name"));
                }
                if !hosts.insert(h.clone()) {
                    return Err(format!("hostname {h} is used by two static leases"));
                }
                l.hostname = Some(h);
            }
        }
        if !macs.insert(l.mac.clone()) {
            return Err(format!("MAC {} has two static leases", l.mac));
        }
        if !ips.insert(ip) {
            return Err(format!("address {ip} has two static leases"));
        }
    }
    for r in &mut n.dns_records {
        r.name = r.name.trim().trim_matches('.').to_lowercase();
        if !valid_dns_name(&r.name) {
            return Err(format!("DNS record name {:?} is not valid", r.name));
        }
        r.ip = r.ip.trim().to_string();
        r.ip.parse::<IpAddr>()
            .map_err(|_| format!("DNS record address {:?} is not an IP address", r.ip))?;
    }
    n.serve_node = n.serve_node.trim().to_string();
    Ok(())
}

// ----------------------------------------------------------- dnsmasq config

/// The dnsmasq configuration of a network (pure, unit tested).
pub fn render_conf(n: &DhcpNetwork, dir: &FsPath) -> String {
    let (server, len) = parse_cidr(&n.address).unwrap_or((Ipv4Addr::UNSPECIFIED, 24));
    let netmask = Ipv4Addr::from(mask_of(len));
    let mut c = vec![
        "# Managed by Horcrux - changes are overwritten".to_string(),
        format!("interface={}", n.iface()),
        "bind-interfaces".into(),
        "except-interface=lo".into(),
        format!("listen-address={server}"),
        "no-hosts".into(),
        format!("pid-file={}", dir.join(format!("{}.pid", n.id)).display()),
        format!(
            "dhcp-leasefile={}",
            dir.join(format!("{}.leases", n.id)).display()
        ),
        "dhcp-authoritative".into(),
        format!(
            "dhcp-range={},{},{},{}",
            n.range_start, n.range_end, netmask, n.lease_time
        ),
    ];
    match &n.gateway {
        Some(g) => c.push(format!("dhcp-option=3,{g}")),
        None => c.push("dhcp-option=3".into()),
    }
    if !n.dns_enabled {
        c.push("port=0".into());
    }
    let dns: Vec<String> = if !n.dns_servers.is_empty() {
        n.dns_servers.clone()
    } else if n.dns_enabled {
        vec![server.to_string()]
    } else {
        vec![]
    };
    if dns.is_empty() {
        c.push("dhcp-option=6".into());
    } else {
        c.push(format!("dhcp-option=6,{}", dns.join(",")));
    }
    if n.dns_enabled {
        if !n.upstream_dns.is_empty() {
            c.push("no-resolv".into());
            for u in &n.upstream_dns {
                c.push(format!("server={u}"));
            }
        }
        if let Some(d) = &n.domain {
            c.push(format!("domain={d}"));
            c.push(format!("local=/{d}/"));
            c.push("expand-hosts".into());
        }
        for r in &n.dns_records {
            c.push(format!("host-record={},{}", r.name, r.ip));
        }
    } else if let Some(d) = &n.domain {
        c.push(format!("dhcp-option=15,{d}"));
    }
    for l in &n.static_leases {
        match &l.hostname {
            Some(h) => c.push(format!("dhcp-host={},{},{}", l.mac, l.ip, h)),
            None => c.push(format!("dhcp-host={},{}", l.mac, l.ip)),
        }
    }
    c.join("\n") + "\n"
}

/// `expiry mac ip hostname clientid` lines of a dnsmasq lease file.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Lease {
    pub expires: i64,
    pub mac: String,
    pub ip: String,
    pub hostname: Option<String>,
}

pub fn parse_leases(text: &str) -> Vec<Lease> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 4 {
                return None;
            }
            Some(Lease {
                expires: f[0].parse().ok()?,
                mac: f[1].to_lowercase(),
                ip: f[2].to_string(),
                hostname: Some(f[3].to_string()).filter(|h| h != "*"),
            })
        })
        .collect()
}

// -------------------------------------------------------------- processes

fn run_dir(state: &AppState) -> PathBuf {
    state.config.paths.data_dir.join("dhcp")
}

async fn sh(program: &str, args: &[&str]) -> Result<String, String> {
    let o = Command::new(program)
        .args(args)
        .output()
        .await
        .map_err(|e| format!("{program}: {e}"))?;
    if o.status.success() {
        Ok(String::from_utf8_lossy(&o.stdout).to_string())
    } else {
        Err(format!(
            "{program} {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&o.stderr).trim()
        ))
    }
}

fn pid_of(dir: &FsPath, id: &str) -> Option<u32> {
    let pid: u32 = std::fs::read_to_string(dir.join(format!("{id}.pid")))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let cmd = String::from_utf8_lossy(&cmd).to_string();
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    let zombie = stat.rsplit(") ").next().is_some_and(|r| r.starts_with('Z'));
    (cmd.contains("dnsmasq") && !zombie).then_some(pid)
}

fn is_running(dir: &FsPath, id: &str) -> bool {
    pid_of(dir, id).is_some()
}

async fn stop_dnsmasq(dir: &FsPath, id: &str) {
    if let Some(pid) = pid_of(dir, id) {
        let _ = sh("kill", &["-TERM", &pid.to_string()]).await;
        for _ in 0..50 {
            if pid_of(dir, id).is_none() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        if pid_of(dir, id).is_some() {
            let _ = sh("kill", &["-KILL", &pid.to_string()]).await;
        }
    }
    let _ = std::fs::remove_file(dir.join(format!("{id}.pid")));
}

/// Interface + address for the server; returns the CIDR it had to add (so a
/// teardown removes only what Horcrux put there).
async fn prepare_iface(n: &DhcpNetwork) -> Result<Option<String>, String> {
    let sys = |i: &str| FsPath::new("/sys/class/net").join(i).exists();
    if !sys(&n.bridge) {
        return Err(format!("bridge {} does not exist on this node", n.bridge));
    }
    let iface = n.iface();
    if let Some(v) = n.vlan {
        if crate::vm::qemu::HostNet::system().bridge_vlan_aware(&n.bridge) != Some(true) {
            return Err(format!(
                "bridge {} is not VLAN-aware: a DHCP network on VLAN {v} needs vlan_aware",
                n.bridge
            ));
        }
        sh(
            "bridge",
            &[
                "vlan",
                "add",
                "dev",
                &n.bridge,
                "vid",
                &v.to_string(),
                "self",
            ],
        )
        .await
        .or_else(|e| {
            if e.contains("File exists") {
                Ok(String::new())
            } else {
                Err(e)
            }
        })?;
        if !sys(&iface) {
            sh(
                "ip",
                &[
                    "link",
                    "add",
                    "link",
                    &n.bridge,
                    "name",
                    &iface,
                    "type",
                    "vlan",
                    "id",
                    &v.to_string(),
                ],
            )
            .await?;
        }
    }
    let (server, _) = parse_cidr(&n.address)?;
    let have = sh("ip", &["-4", "-o", "addr", "show", "dev", &iface])
        .await
        .unwrap_or_default();
    let mut added = None;
    if !have.contains(&format!(" {server}/")) {
        sh("ip", &["addr", "add", &n.address, "dev", &iface]).await?;
        added = Some(n.address.clone());
    }
    sh("ip", &["link", "set", &iface, "up"]).await?;
    Ok(added)
}

async fn teardown_iface(dir: &FsPath, n: &DhcpNetwork) {
    let marker = dir.join(format!("{}.addr", n.id));
    let iface = n.iface();
    if n.vlan.is_some() {
        let _ = sh("ip", &["link", "del", &iface]).await;
    } else if let Ok(cidr) = std::fs::read_to_string(&marker) {
        let _ = sh("ip", &["addr", "del", cidr.trim(), "dev", &iface]).await;
    }
    let _ = std::fs::remove_file(marker);
}

/// Make this node's dnsmasq for `n` match the definition: running when this
/// node serves it, stopped (and the interface cleaned up) otherwise.
pub async fn apply_here(dir: &FsPath, local: &str, n: &DhcpNetwork) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let serving = n.enabled && !n.deleted && n.serve_node == local;
    let conf_path = dir.join(format!("{}.conf", n.id));
    if !serving {
        if is_running(dir, &n.id) || conf_path.exists() {
            stop_dnsmasq(dir, &n.id).await;
            teardown_iface(dir, n).await;
            let _ = std::fs::remove_file(&conf_path);
            let _ = std::fs::remove_file(dir.join(format!("{}.leases", n.id)));
        }
        return Ok(());
    }
    let conf = render_conf(n, dir);
    let unchanged = std::fs::read_to_string(&conf_path).is_ok_and(|c| c == conf);
    if unchanged && is_running(dir, &n.id) {
        return Ok(());
    }
    stop_dnsmasq(dir, &n.id).await;
    if let Some(added) = prepare_iface(n).await? {
        let _ = std::fs::write(dir.join(format!("{}.addr", n.id)), added);
    }
    std::fs::write(&conf_path, &conf).map_err(|e| format!("{}: {e}", conf_path.display()))?;
    let bin = crate::api::system_admin::which("dnsmasq")
        .ok_or_else(|| "dnsmasq is not installed on this node".to_string())?;
    let o = Command::new(&bin)
        .arg(format!("--conf-file={}", conf_path.display()))
        .output()
        .await
        .map_err(|e| format!("dnsmasq: {e}"))?;
    if !o.status.success() {
        return Err(format!(
            "dnsmasq failed to start for {}: {}",
            n.id,
            String::from_utf8_lossy(&o.stderr).trim()
        ));
    }
    for _ in 0..30 {
        if is_running(dir, &n.id) {
            tracing::info!("DHCP/DNS for {} running on {}", n.id, n.iface());
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Err(format!("dnsmasq for {} did not stay up", n.id))
}

// ---------------------------------------------------------------- cluster

async fn local_name(state: &AppState) -> String {
    state
        .cluster_manager
        .get_local_node_name()
        .await
        .unwrap_or_else(|_| "localhost".into())
}

async fn all_raw(state: &AppState) -> Result<Vec<DhcpNetwork>, ApiError> {
    Ok(ConfigStore::new(&state.database)
        .load_values::<DhcpNetwork>(KIND)
        .await?)
}

async fn all_live(state: &AppState) -> Result<Vec<DhcpNetwork>, ApiError> {
    Ok(all_raw(state)
        .await?
        .into_iter()
        .filter(|n| !n.deleted)
        .collect())
}

fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis() as u64
}

/// Store the objects that are newer than ours; true if anything changed.
async fn merge_in(state: &AppState, incoming: Vec<DhcpNetwork>) -> bool {
    let store = ConfigStore::new(&state.database);
    let have = all_raw(state).await.unwrap_or_default();
    let mut changed = false;
    for n in incoming {
        let newer = have
            .iter()
            .find(|h| h.id == n.id)
            .is_none_or(|h| n.rev > h.rev);
        if newer && store.put(KIND, &n.id, &n).await.is_ok() {
            changed = true;
        }
    }
    changed
}

#[cfg(feature = "cluster")]
async fn call<B: Serialize, T: serde::de::DeserializeOwned>(
    state: &Arc<AppState>,
    node: &crate::cluster::node::Node,
    path: &str,
    body: &B,
) -> Result<T, String> {
    super::drain::call_peer(state, node, path, body).await
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

fn peers(nodes: Vec<crate::cluster::node::Node>, local: &str) -> Vec<crate::cluster::node::Node> {
    nodes
        .into_iter()
        .filter(|n| {
            n.name != local && !n.is_local && n.status == crate::cluster::node::NodeStatus::Online
        })
        .collect()
}

async fn push_to_peers(state: &Arc<AppState>, nets: Vec<DhcpNetwork>) {
    let local = local_name(state).await;
    let body = serde_json::json!({ "networks": nets });
    for node in peers(state.cluster_manager.list_nodes().await, &local) {
        if let Err(e) =
            call::<_, serde_json::Value>(state, &node, "/api/ha/peer/dhcp/sync", &body).await
        {
            tracing::warn!(
                "DHCP config not pushed to {}: {e} (it pulls it later)",
                node.name
            );
        }
    }
}

async fn reconcile(state: &Arc<AppState>) {
    let local = local_name(state).await;
    let dir = run_dir(state);
    for n in all_raw(state).await.unwrap_or_default() {
        if let Err(e) = apply_here(&dir, &local, &n).await {
            tracing::warn!("DHCP network {}: {e}", n.id);
        }
    }
}

fn spawn_reconciler(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(15));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let local = local_name(&state).await;
            // Pull what other nodes know and we do not (a node that was
            // offline when a change was pushed).
            for node in peers(state.cluster_manager.list_nodes().await, &local) {
                if let Ok(list) = call::<_, PeerList>(
                    &state,
                    &node,
                    "/api/ha/peer/dhcp/list",
                    &serde_json::json!({}),
                )
                .await
                {
                    merge_in(&state, list.networks).await;
                }
            }
            reconcile(&state).await;
        }
    });
}

#[derive(Serialize, Deserialize)]
struct PeerList {
    networks: Vec<DhcpNetwork>,
}

fn require_peer(user: &Option<Extension<AuthUser>>) -> Result<(), ApiError> {
    match user {
        Some(Extension(u)) if u.role == CLUSTER_HA_ROLE => Ok(()),
        _ => Err(ApiError::Forbidden(
            "this route is only for other cluster nodes".into(),
        )),
    }
}

async fn peer_sync(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Json(req): Json<PeerList>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_peer(&user)?;
    if merge_in(&state, req.networks).await {
        reconcile(&state).await;
    }
    Ok(Json(serde_json::json!({"ok": true})))
}

async fn peer_list(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
) -> Result<Json<PeerList>, ApiError> {
    require_peer(&user)?;
    Ok(Json(PeerList {
        networks: all_raw(&state).await?,
    }))
}

#[derive(Deserialize)]
struct LeaseQuery {
    id: String,
}

async fn peer_leases(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Json(q): Json<LeaseQuery>,
) -> Result<Json<Vec<Lease>>, ApiError> {
    require_peer(&user)?;
    Ok(Json(read_leases(&run_dir(&state), &q.id)))
}

fn read_leases(dir: &FsPath, id: &str) -> Vec<Lease> {
    std::fs::read_to_string(dir.join(format!("{id}.leases")))
        .map(|t| parse_leases(&t))
        .unwrap_or_default()
}

// -------------------------------------------------------------- handlers

async fn view(state: &AppState, local: &str, n: DhcpNetwork) -> DhcpView {
    let running = n.serve_node == local && is_running(&run_dir(state), &n.id);
    DhcpView {
        iface: n.iface(),
        serving_here: n.serve_node == local,
        running,
        net: n,
    }
}

async fn list(State(state): State<Arc<AppState>>) -> Result<Json<Vec<DhcpView>>, ApiError> {
    let local = local_name(&state).await;
    let mut out = Vec::new();
    for n in all_live(&state).await? {
        out.push(view(&state, &local, n).await);
    }
    Ok(Json(out))
}

async fn find(state: &AppState, id: &str) -> Result<DhcpNetwork, ApiError> {
    all_live(state)
        .await?
        .into_iter()
        .find(|n| n.id == id)
        .ok_or_else(|| ApiError::NotFound(format!("DHCP network {id} not found")))
}

async fn get_one(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<DhcpView>, ApiError> {
    let local = local_name(&state).await;
    Ok(Json(view(&state, &local, find(&state, &id).await?).await))
}

/// A known node name (this one or a cluster member).
async fn check_serve_node(state: &AppState, local: &str, n: &DhcpNetwork) -> Result<(), ApiError> {
    if n.serve_node == local {
        return Ok(());
    }
    let known = state
        .cluster_manager
        .list_nodes()
        .await
        .iter()
        .any(|c| c.name == n.serve_node);
    if known {
        Ok(())
    } else {
        Err(ApiError::BadRequest(format!(
            "serve_node {:?} is not a node of this cluster",
            n.serve_node
        )))
    }
}

/// Save, replicate, apply here; the apply error (dnsmasq missing, bridge
/// absent) is returned to the caller after the definition is stored.
async fn commit(
    state: &Arc<AppState>,
    local: &str,
    mut n: DhcpNetwork,
) -> Result<DhcpView, ApiError> {
    n.rev = now_ms().max(n.rev + 1);
    ConfigStore::new(&state.database)
        .put(KIND, &n.id, &n)
        .await?;
    push_to_peers(state, vec![n.clone()]).await;
    if let Err(e) = apply_here(&run_dir(state), local, &n).await {
        return Err(ApiError::Internal(format!(
            "DHCP network {} saved, but not running on this node: {e}",
            n.id
        )));
    }
    Ok(view(state, local, n).await)
}

fn uniqueness(existing: &[DhcpNetwork], n: &DhcpNetwork, replacing: bool) -> Result<(), ApiError> {
    for o in existing {
        if o.id == n.id {
            if replacing {
                continue;
            }
            return Err(ApiError::Conflict(format!(
                "DHCP network {} already exists",
                n.id
            )));
        }
        if o.iface() == n.iface() {
            return Err(ApiError::Conflict(format!(
                "{} already has DHCP network {}: one DHCP/DNS server per interface",
                n.iface(),
                o.id
            )));
        }
    }
    Ok(())
}

async fn create(
    State(state): State<Arc<AppState>>,
    Json(mut n): Json<DhcpNetwork>,
) -> Result<(StatusCode, Json<DhcpView>), ApiError> {
    let local = local_name(&state).await;
    normalize_and_validate(&mut n).map_err(ApiError::BadRequest)?;
    if n.serve_node.is_empty() {
        n.serve_node = local.clone();
    }
    check_serve_node(&state, &local, &n).await?;
    n.deleted = false;
    n.rev = 0;
    uniqueness(&all_live(&state).await?, &n, false)?;
    // A tombstone of the same id is simply overwritten by the newer rev.
    if let Some(old) = all_raw(&state).await?.into_iter().find(|o| o.id == n.id) {
        n.rev = old.rev;
    }
    Ok((StatusCode::CREATED, Json(commit(&state, &local, n).await?)))
}

async fn update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(mut n): Json<DhcpNetwork>,
) -> Result<Json<DhcpView>, ApiError> {
    let local = local_name(&state).await;
    let old = find(&state, &id).await?;
    n.id = id;
    normalize_and_validate(&mut n).map_err(ApiError::BadRequest)?;
    if n.serve_node.is_empty() {
        n.serve_node = old.serve_node.clone();
    }
    check_serve_node(&state, &local, &n).await?;
    n.deleted = false;
    n.rev = old.rev;
    uniqueness(&all_live(&state).await?, &n, true)?;
    // The interface a network sits on cannot change under a running server:
    // tear the old one down first (here, when we serve it).
    if old.iface() != n.iface() || old.serve_node != n.serve_node {
        let mut gone = old.clone();
        gone.deleted = true;
        let _ = apply_here(&run_dir(&state), &local, &gone).await;
    }
    Ok(Json(commit(&state, &local, n).await?))
}

async fn remove(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let local = local_name(&state).await;
    let mut n = find(&state, &id).await?;
    n.deleted = true;
    n.rev = now_ms().max(n.rev + 1);
    ConfigStore::new(&state.database)
        .put(KIND, &n.id, &n)
        .await?;
    push_to_peers(&state, vec![n.clone()]).await;
    let _ = apply_here(&run_dir(&state), &local, &n).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn add_static(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(l): Json<StaticLease>,
) -> Result<(StatusCode, Json<DhcpView>), ApiError> {
    let local = local_name(&state).await;
    let mut n = find(&state, &id).await?;
    n.static_leases.push(l);
    normalize_and_validate(&mut n).map_err(ApiError::BadRequest)?;
    Ok((StatusCode::CREATED, Json(commit(&state, &local, n).await?)))
}

async fn remove_static(
    State(state): State<Arc<AppState>>,
    Path((id, mac)): Path<(String, String)>,
) -> Result<Json<DhcpView>, ApiError> {
    let local = local_name(&state).await;
    let mac = horcrux_common::normalize_mac(&mac).map_err(ApiError::BadRequest)?;
    let mut n = find(&state, &id).await?;
    let before = n.static_leases.len();
    n.static_leases.retain(|l| l.mac != mac);
    if n.static_leases.len() == before {
        return Err(ApiError::NotFound(format!("no static lease for {mac}")));
    }
    Ok(Json(commit(&state, &local, n).await?))
}

/// The leases the serving node's dnsmasq handed out (asked of that node).
async fn active_leases(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Vec<Lease>>, ApiError> {
    let local = local_name(&state).await;
    let n = find(&state, &id).await?;
    if n.serve_node == local {
        return Ok(Json(read_leases(&run_dir(&state), &n.id)));
    }
    let node = state
        .cluster_manager
        .list_nodes()
        .await
        .into_iter()
        .find(|c| c.name == n.serve_node)
        .ok_or_else(|| ApiError::NotFound(format!("node {} is not known", n.serve_node)))?;
    let leases = call::<_, Vec<Lease2>>(
        &state,
        &node,
        "/api/ha/peer/dhcp/leases",
        &serde_json::json!({ "id": n.id }),
    )
    .await
    .map_err(|e| ApiError::Internal(format!("asking {}: {e}", n.serve_node)))?;
    Ok(Json(
        leases
            .into_iter()
            .map(|l| Lease {
                expires: l.expires,
                mac: l.mac,
                ip: l.ip,
                hostname: l.hostname,
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
struct Lease2 {
    expires: i64,
    mac: String,
    ip: String,
    hostname: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net() -> DhcpNetwork {
        DhcpNetwork {
            id: String::new(),
            bridge: "vmbr1".into(),
            vlan: Some(20),
            address: "192.168.20.1/24".into(),
            range_start: "192.168.20.100".into(),
            range_end: "192.168.20.150".into(),
            gateway: Some("192.168.20.1".into()),
            dns_servers: vec![],
            domain: Some("Lab.Example.".into()),
            lease_time: "12h".into(),
            dns_enabled: true,
            upstream_dns: vec!["1.1.1.1".into()],
            static_leases: vec![StaticLease {
                mac: "52:54:00:AA:BB:CC".into(),
                ip: "192.168.20.10".into(),
                hostname: Some("Web1".into()),
            }],
            dns_records: vec![DnsRecord {
                name: "gw.lab.example".into(),
                ip: "192.168.20.1".into(),
            }],
            enabled: true,
            serve_node: "n1".into(),
            comment: None,
            rev: 0,
            deleted: false,
        }
    }

    #[test]
    fn a_valid_network_is_normalised() {
        let mut n = net();
        normalize_and_validate(&mut n).unwrap();
        assert_eq!(n.id, "vmbr1.20");
        assert_eq!(n.iface(), "vmbr1.20");
        assert_eq!(n.domain.as_deref(), Some("lab.example"));
        assert_eq!(n.static_leases[0].mac, "52:54:00:aa:bb:cc");
        assert_eq!(n.static_leases[0].hostname.as_deref(), Some("web1"));
    }

    #[test]
    fn bad_networks_are_refused() {
        let bad = |f: &dyn Fn(&mut DhcpNetwork)| {
            let mut n = net();
            f(&mut n);
            normalize_and_validate(&mut n).is_err()
        };
        assert!(bad(&|n| n.address = "192.168.20.1".into()), "needs prefix");
        assert!(bad(&|n| n.address = "192.168.20.1/31".into()));
        assert!(
            bad(&|n| n.range_end = "192.168.21.5".into()),
            "outside subnet"
        );
        assert!(
            bad(&|n| n.range_start = "192.168.20.200".into()),
            "reversed"
        );
        assert!(
            bad(&|n| n.range_start = "192.168.20.1".into()),
            "server in range"
        );
        assert!(bad(&|n| n.gateway = Some("10.0.0.1".into())));
        assert!(bad(&|n| n.dns_servers = vec!["not-an-ip".into()]));
        assert!(bad(&|n| n.lease_time = "12 hours\nbind".into()));
        assert!(bad(&|n| n.lease_time = "h".into()));
        assert!(bad(&|n| n.domain = Some("a b".into())));
        assert!(bad(&|n| n.vlan = Some(4095)));
        assert!(bad(&|n| n.bridge = "waytoolongbridgename".into()));
        assert!(
            bad(&|n| n.bridge = "vmbr123456789".into()),
            "iface too long with vlan"
        );
        assert!(
            bad(&|n| n.static_leases[0].ip = "192.168.20.1".into()),
            "lease = server"
        );
        assert!(bad(&|n| n.static_leases[0].ip = "192.168.99.1".into()));
        assert!(bad(&|n| n.static_leases[0].mac = "zz".into()));
        assert!(bad(&|n| n.static_leases[0].hostname = Some("x\ny".into())));
        assert!(bad(&|n| {
            let l = n.static_leases[0].clone();
            n.static_leases.push(l);
        }));
        assert!(bad(&|n| n.dns_records[0].name = "a;b".into()));
    }

    #[test]
    fn dnsmasq_conf_has_everything_and_no_injection_points() {
        let mut n = net();
        normalize_and_validate(&mut n).unwrap();
        let c = render_conf(&n, FsPath::new("/var/lib/horcrux/dhcp"));
        for want in [
            "interface=vmbr1.20",
            "bind-interfaces",
            "listen-address=192.168.20.1",
            "dhcp-range=192.168.20.100,192.168.20.150,255.255.255.0,12h",
            "dhcp-option=3,192.168.20.1",
            "dhcp-option=6,192.168.20.1",
            "no-resolv",
            "server=1.1.1.1",
            "domain=lab.example",
            "local=/lab.example/",
            "expand-hosts",
            "host-record=gw.lab.example,192.168.20.1",
            "dhcp-host=52:54:00:aa:bb:cc,192.168.20.10,web1",
            "pid-file=/var/lib/horcrux/dhcp/vmbr1.20.pid",
            "dhcp-leasefile=/var/lib/horcrux/dhcp/vmbr1.20.leases",
        ] {
            assert!(c.lines().any(|l| l == want), "missing {want:?} in:\n{c}");
        }
        assert!(!c.lines().any(|l| l == "port=0"));
        // DNS off: port=0, no resolver lines, router/dns options still there.
        n.dns_enabled = false;
        n.gateway = None;
        n.dns_servers = vec!["9.9.9.9".into(), "8.8.8.8".into()];
        let c = render_conf(&n, FsPath::new("/d"));
        assert!(c.lines().any(|l| l == "port=0"));
        assert!(c.lines().any(|l| l == "dhcp-option=3"));
        assert!(c.lines().any(|l| l == "dhcp-option=6,9.9.9.9,8.8.8.8"));
        assert!(!c.contains("host-record") && !c.contains("server="));
        assert!(c.lines().any(|l| l == "dhcp-option=15,lab.example"));
    }

    #[test]
    fn lease_files_parse() {
        let t = "1700000000 52:54:00:AA:BB:CC 192.168.20.101 cirros 01:52:54:00:aa:bb:cc\n\
                 1700000100 52:54:00:11:22:33 192.168.20.102 * *\nbroken\n";
        let l = parse_leases(t);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].mac, "52:54:00:aa:bb:cc");
        assert_eq!(l[0].hostname.as_deref(), Some("cirros"));
        assert_eq!(l[1].hostname, None);
    }

    #[test]
    fn one_server_per_interface() {
        let mut a = net();
        normalize_and_validate(&mut a).unwrap();
        let mut b = net();
        b.id = "other".into();
        normalize_and_validate(&mut b).unwrap();
        assert!(uniqueness(std::slice::from_ref(&a), &b, false).is_err());
        assert!(uniqueness(std::slice::from_ref(&a), &a, true).is_ok());
        assert!(uniqueness(std::slice::from_ref(&a), &a, false).is_err());
    }
}
