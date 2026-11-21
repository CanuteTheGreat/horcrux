//! Remote Play readiness, node side (docs/design/gaming-vm.md section 16).
//!
//! Pure pieces the API's remote-play service is built from, all testable
//! without a network:
//!
//! * [`parse_wol`]: a Wake-on-LAN magic packet (6 x 0xFF, then the target
//!   MAC 16 times; an optional SecureOn password may follow) -> the MAC.
//! * [`steam_discovery_type`]: a Steam LAN discovery datagram (UDP 27036:
//!   8-byte magic, length-prefixed `CMsgRemoteClientBroadcastHeader`
//!   protobuf, length-prefixed body) -> its message type (0 = discovery
//!   request, 1 = status answer). Only the framing is read; Horcrux never
//!   changes the payload it relays.
//! * [`nic_reach`]: is a VM's network card on the LAN (macvtap on a host
//!   interface, or a bridge with a physical port) or behind NAT (a bridge
//!   with no physical port, e.g. an SDN subnet with masquerade)?
//! * [`Relay`]: the discovery relay's decisions for one datagram (who gets
//!   a copy), so the socket loop only does I/O.
//! * [`render_nft`]: the nftables table with the port forwards from the
//!   LAN to VMs behind NAT, scoped to the allowed source networks.
//! * [`checklist`]: the prerequisite list shown on the VM's Remote Play
//!   page.
//! * [`guest_setup_script`]: the PowerShell the QEMU guest agent runs in a
//!   Windows guest (Steam install from Valve, automatic logon stored as an
//!   LSA secret, Big Picture at logon, no sleep) and its JSON report.
use horcrux_common::remote_play::{
    cidr4_contains, parse_cidr4, CheckItem, GuestReport, RemotePlayClient, RemotePlayNetwork,
    VmRemotePlay,
};
use horcrux_common::{NicAttach, VmNic};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::Path;
use std::time::{Duration, Instant};

// ------------------------------------------------------------ Wake-on-LAN

/// The MAC a Wake-on-LAN magic packet addresses, if `p` is one. The sync
/// stream (6 x 0xFF) may sit anywhere in the datagram (some senders put a
/// header in front); 16 copies of the MAC must follow it.
pub fn parse_wol(p: &[u8]) -> Option<[u8; 6]> {
    if p.len() < 102 {
        return None;
    }
    for start in 0..=p.len() - 102 {
        if p[start..start + 6].iter().all(|b| *b == 0xff) {
            let body = &p[start + 6..start + 102];
            let mac: [u8; 6] = body[..6].try_into().ok()?;
            if mac == [0xff; 6] {
                // a longer run of 0xFF: the sync stream starts later
                continue;
            }
            if body.chunks(6).all(|c| c == mac) {
                return Some(mac);
            }
        }
    }
    None
}

pub fn parse_mac(s: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = s.trim().split([':', '-']).collect();
    if parts.len() != 6 {
        return None;
    }
    let mut m = [0u8; 6];
    for (i, p) in parts.iter().enumerate() {
        if p.len() != 2 {
            return None;
        }
        m[i] = u8::from_str_radix(p, 16).ok()?;
    }
    Some(m)
}

pub fn format_mac(m: &[u8; 6]) -> String {
    m.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

// ------------------------------------------------- Steam LAN discovery

/// First 8 bytes of every Steam LAN discovery datagram
/// (0xFFFFFFFF, 0xA05F4C21 little-endian).
pub const STEAM_DISCOVERY_MAGIC: [u8; 8] = [0xff, 0xff, 0xff, 0xff, 0x21, 0x4c, 0x5f, 0xa0];

/// `k_ERemoteClientBroadcastMsgDiscovery`
pub const STEAM_MSG_DISCOVERY: u64 = 0;
/// `k_ERemoteClientBroadcastMsgStatus`
pub const STEAM_MSG_STATUS: u64 = 1;

fn varint(b: &[u8], i: &mut usize) -> Option<u64> {
    let mut v: u64 = 0;
    for shift in (0..64).step_by(7) {
        let byte = *b.get(*i)?;
        *i += 1;
        v |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

/// The message type of a Steam discovery datagram (field 2 of the header
/// protobuf; absent = 0, the protobuf default), or None if `p` is not one.
pub fn steam_discovery_type(p: &[u8]) -> Option<u64> {
    if p.len() < 16 || p[..8] != STEAM_DISCOVERY_MAGIC {
        return None;
    }
    let hlen = u32::from_le_bytes(p[8..12].try_into().ok()?) as usize;
    let header = p.get(12..12usize.checked_add(hlen)?)?;
    let blen_at = 12 + hlen;
    let blen = u32::from_le_bytes(p.get(blen_at..blen_at + 4)?.try_into().ok()?) as usize;
    if p.len() < blen_at + 4 + blen {
        return None;
    }
    let mut i = 0;
    let mut msg_type = 0;
    while i < header.len() {
        let key = varint(header, &mut i)?;
        let (field, wire) = (key >> 3, key & 7);
        match wire {
            0 => {
                let v = varint(header, &mut i)?;
                if field == 2 {
                    msg_type = v;
                }
            }
            1 => i = i.checked_add(8)?,
            2 => {
                let l = varint(header, &mut i)? as usize;
                i = i.checked_add(l)?;
            }
            5 => i = i.checked_add(4)?,
            _ => return None,
        }
        if i > header.len() {
            return None;
        }
    }
    Some(msg_type)
}

/// A Steam discovery datagram (header with client id and message type,
/// body with a sequence number) as clients send it; used by tests and the
/// node's own reachability probe.
pub fn steam_discovery_packet(client_id: u64, msg_type: u64, seq: u64) -> Vec<u8> {
    fn put_varint(out: &mut Vec<u8>, mut v: u64) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }
    let mut header = vec![0x08];
    put_varint(&mut header, client_id);
    header.push(0x10);
    put_varint(&mut header, msg_type);
    let mut body = vec![0x08];
    put_varint(&mut body, seq);
    let mut p = STEAM_DISCOVERY_MAGIC.to_vec();
    p.extend_from_slice(&(header.len() as u32).to_le_bytes());
    p.extend_from_slice(&header);
    p.extend_from_slice(&(body.len() as u32).to_le_bytes());
    p.extend_from_slice(&body);
    p
}

// ------------------------------------------------------------ reachability

/// How clients on the LAN reach a VM's network card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reach {
    /// On the LAN: clients see the VM directly.
    Lan { nic: String },
    /// Behind the host (a bridge with no physical port): discovery needs
    /// the relay and the ports need forwarding.
    Nat { bridge: String },
    /// No network card.
    None,
}

fn is_guest_port(name: &str) -> bool {
    ["tap", "veth", "vnet", "fwpr", "fwln", "fwbr", "vc", "hxt"]
        .iter()
        .any(|p| name.starts_with(p))
}

/// A bridge port that leads to the LAN: a physical NIC (has `device`), or
/// a bond/VLAN/other stacked device on top of something (`lower_*`).
pub fn uplink_port(sys_net: &Path, port: &str) -> bool {
    if is_guest_port(port) {
        return false;
    }
    let p = sys_net.join(port);
    if p.join("device").exists() || p.join("bonding").exists() {
        return true;
    }
    std::fs::read_dir(&p)
        .map(|rd| {
            rd.flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with("lower_"))
        })
        .unwrap_or(false)
}

/// Physical/stacked ports of a bridge (empty = an isolated bridge).
pub fn bridge_uplinks(sys_net: &Path, bridge: &str) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(sys_net.join(bridge).join("brif"))
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|p| uplink_port(sys_net, p))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// Where the VM's first network card is (`sys_net` = `/sys/class/net`).
pub fn nic_reach(nets: &[VmNic], sys_net: &Path) -> Reach {
    let Some(nic) = nets.first() else {
        return Reach::None;
    };
    match nic.attach {
        NicAttach::Macvtap => Reach::Lan {
            nic: format!("macvtap on {}", nic.bridge),
        },
        NicAttach::Bridge => {
            let up = bridge_uplinks(sys_net, &nic.bridge);
            if up.is_empty() {
                Reach::Nat {
                    bridge: nic.bridge.clone(),
                }
            } else {
                Reach::Lan {
                    nic: format!("bridge {} (LAN port {})", nic.bridge, up.join(", ")),
                }
            }
        }
    }
}

/// What `network` resolves to for this card: `lan`, `nat` or `none`.
pub fn resolve_network(rp: &VmRemotePlay, reach: &Reach) -> &'static str {
    match (rp.network, reach) {
        (_, Reach::None) => "none",
        (RemotePlayNetwork::Lan, _) => "lan",
        (RemotePlayNetwork::Nat, _) => "nat",
        (RemotePlayNetwork::Auto, Reach::Lan { .. }) => "lan",
        (RemotePlayNetwork::Auto, Reach::Nat { .. }) => "nat",
    }
}

/// IPv4 address of the interface owning `mac` in an `ip -j neigh` /
/// guest-agent style list of (ip, mac) pairs.
pub fn address_for_mac(pairs: &[(Ipv4Addr, String)], mac: &str) -> Option<Ipv4Addr> {
    let want = parse_mac(mac)?;
    pairs
        .iter()
        .filter(|(ip, m)| parse_mac(m) == Some(want) && !ip.is_link_local() && !ip.is_unspecified())
        .map(|(ip, _)| *ip)
        .next()
}

/// `ip -j neigh show` -> (ip, mac) pairs (IPv4, entries with a MAC).
pub fn parse_ip_neigh_json(text: &str) -> Vec<(Ipv4Addr, String)> {
    let v: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    let ip: Ipv4Addr = e.get("dst")?.as_str()?.parse().ok()?;
                    let mac = e.get("lladdr")?.as_str()?.to_string();
                    let failed = e
                        .get("state")
                        .and_then(|s| s.as_array())
                        .map(|s| s.iter().any(|x| x == "FAILED" || x == "INCOMPLETE"))
                        .unwrap_or(false);
                    (!failed).then_some((ip, mac))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// QEMU guest agent `guest-network-get-interfaces` -> (ip, mac) pairs.
pub fn parse_qga_interfaces(v: &serde_json::Value) -> Vec<(Ipv4Addr, String)> {
    let mut out = Vec::new();
    for i in v.as_array().into_iter().flatten() {
        let Some(mac) = i.get("hardware-address").and_then(|m| m.as_str()) else {
            continue;
        };
        for a in i
            .get("ip-addresses")
            .and_then(|a| a.as_array())
            .into_iter()
            .flatten()
        {
            if a.get("ip-address-type").and_then(|t| t.as_str()) != Some("ipv4") {
                continue;
            }
            if let Some(ip) = a
                .get("ip-address")
                .and_then(|x| x.as_str())
                .and_then(|x| x.parse().ok())
            {
                out.push((ip, mac.to_string()));
            }
        }
    }
    out
}

/// `ip -j -4 addr show` -> (interface, network CIDR) for global addresses.
pub fn parse_ip_addr_json(text: &str) -> Vec<(String, String)> {
    let v: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
    let mut out = Vec::new();
    for i in v.as_array().into_iter().flatten() {
        let Some(name) = i.get("ifname").and_then(|n| n.as_str()) else {
            continue;
        };
        for a in i
            .get("addr_info")
            .and_then(|a| a.as_array())
            .into_iter()
            .flatten()
        {
            if a.get("family").and_then(|f| f.as_str()) != Some("inet")
                || a.get("scope").and_then(|s| s.as_str()) != Some("global")
            {
                continue;
            }
            let (Some(ip), Some(len)) = (
                a.get("local")
                    .and_then(|x| x.as_str())
                    .and_then(|x| x.parse::<Ipv4Addr>().ok()),
                a.get("prefixlen").and_then(|x| x.as_u64()),
            ) else {
                continue;
            };
            let len = len.min(32) as u8;
            let mask = if len == 0 { 0 } else { u32::MAX << (32 - len) };
            let net = Ipv4Addr::from(u32::from(ip) & mask);
            out.push((name.to_string(), format!("{net}/{len}")));
        }
    }
    out
}

/// The LAN networks of the host: global IPv4 networks of interfaces that
/// are not isolated VM bridges, guest ports, container bridges or loopback.
pub fn host_lan_networks(addrs: &[(String, String)], sys_net: &Path) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (ifname, net) in addrs {
        if ifname == "lo"
            || is_guest_port(ifname)
            || ifname.starts_with("docker")
            || ifname.starts_with("br-")
            || ifname.starts_with("virbr")
            || ifname.starts_with("cni")
            || ifname.starts_with("podman")
            || ifname.starts_with("incusbr")
            || ifname.starts_with("lxdbr")
        {
            continue;
        }
        let is_bridge = sys_net.join(ifname).join("bridge").exists();
        if is_bridge && bridge_uplinks(sys_net, ifname).is_empty() {
            continue;
        }
        if !out.contains(net) {
            out.push(net.clone());
        }
    }
    out
}

pub fn source_allowed(sources: &[String], ip: Ipv4Addr) -> bool {
    sources
        .iter()
        .filter_map(|s| parse_cidr4(s).ok())
        .any(|n| cidr4_contains(n, ip))
}

// ------------------------------------------------------- discovery relay

/// One VM behind NAT the relay serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayTarget {
    pub vm: String,
    pub addr: Ipv4Addr,
    pub sources: Vec<String>,
    pub running: bool,
    /// Behind NAT: requests are copied to it and its answers back. False =
    /// a LAN VM listed only to be woken by discovery.
    pub relay: bool,
    /// Start the VM when a client looks for Steam hosts.
    pub wake_on_discovery: bool,
}

/// What to do with one datagram.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RelayActions {
    pub send: Vec<SocketAddrV4>,
    /// VMs to start (wake on discovery).
    pub wake: Vec<String>,
}

/// The discovery relay: copies a LAN client's discovery request to each
/// running VM behind NAT the client may reach, and the VMs' answers (and
/// their own status broadcasts) back to the clients that asked recently.
/// Datagrams are relayed unchanged; the relay port is the discovery port,
/// so both sides see ordinary discovery traffic.
#[derive(Debug, Default)]
pub struct Relay {
    pub targets: Vec<RelayTarget>,
    clients: HashMap<SocketAddrV4, Instant>,
    pub relayed_to_vms: u64,
    pub relayed_to_clients: u64,
}

impl Relay {
    pub fn on_datagram(
        &mut self,
        from: SocketAddrV4,
        payload: &[u8],
        port: u16,
        client_ttl: Duration,
        now: Instant,
    ) -> RelayActions {
        let mut a = RelayActions::default();
        let Some(_kind) = steam_discovery_type(payload) else {
            return a;
        };
        self.clients
            .retain(|_, seen| now.duration_since(*seen) < client_ttl);
        if let Some(t) = self
            .targets
            .iter()
            .find(|t| t.relay && t.addr == *from.ip())
        {
            // a VM answering (or announcing itself): to the recent clients
            // allowed to reach that VM
            for c in self.clients.keys() {
                if source_allowed(&t.sources, *c.ip()) {
                    a.send.push(*c);
                }
            }
            a.send.sort();
            self.relayed_to_clients += a.send.len() as u64;
            return a;
        }
        let mut wanted = false;
        for t in &self.targets {
            if !source_allowed(&t.sources, *from.ip()) {
                continue;
            }
            wanted |= t.relay;
            if t.running {
                if t.relay {
                    a.send.push(SocketAddrV4::new(t.addr, port));
                }
            } else if t.wake_on_discovery
                && steam_discovery_type(payload) == Some(STEAM_MSG_DISCOVERY)
            {
                a.wake.push(t.vm.clone());
            }
        }
        if wanted {
            self.clients.insert(from, now);
        }
        self.relayed_to_vms += a.send.len() as u64;
        a
    }

    pub fn clients(&self) -> usize {
        self.clients.len()
    }
}

// --------------------------------------------------------------- nftables

/// Port forwards from the LAN to one VM behind NAT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forward {
    pub vm: String,
    pub client: RemotePlayClient,
    /// The VM's bridge: packets coming from it are not forwarded back.
    pub bridge: String,
    pub to: Ipv4Addr,
    pub sources: Vec<String>,
    pub tcp: Vec<(u16, u16)>,
    pub udp: Vec<(u16, u16)>,
}

fn nft_quote(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ' | ':'))
        .collect()
}

/// The whole `inet <table>` with every forward (empty string = no table).
/// Applied with `nft -f -` as one transaction (the table is re-created).
pub fn render_nft(table: &str, fwds: &[Forward]) -> String {
    if fwds.is_empty() {
        return String::new();
    }
    use horcrux_common::remote_play::ports_nft;
    let mut nat = String::new();
    let mut filter = String::new();
    for f in fwds {
        if f.sources.is_empty() {
            continue;
        }
        let src = f.sources.join(", ");
        let tag = nft_quote(&format!("remote-play {} {:?}", f.vm, f.client).to_lowercase());
        let br = nft_quote(&f.bridge);
        for (proto, ports) in [("tcp", &f.tcp), ("udp", &f.udp)] {
            if ports.is_empty() {
                continue;
            }
            let p = ports_nft(ports);
            nat.push_str(&format!(
                "    iifname != \"{br}\" ip saddr {{ {src} }} {proto} dport {{ {p} }} dnat ip to {} comment \"{tag}\"\n",
                f.to
            ));
            filter.push_str(&format!(
                "    ip saddr {{ {src} }} ip daddr {} {proto} dport {{ {p} }} ct status dnat accept comment \"{tag}\"\n",
                f.to
            ));
        }
    }
    if nat.is_empty() {
        return String::new();
    }
    format!(
        "table inet {table} {{\n  chain prerouting {{\n    type nat hook prerouting priority dstnat; policy accept;\n{nat}  }}\n  chain forward {{\n    type filter hook forward priority filter; policy accept;\n{filter}  }}\n}}\n"
    )
}

/// The `nft -f -` script replacing the table atomically (or removing it).
pub fn nft_apply_script(table: &str, body: &str) -> String {
    // `add` makes the delete safe when the table does not exist yet
    format!("add table inet {table}\ndelete table inet {table}\n{body}")
}

// ------------------------------------------------------------ checklist

/// Everything the checklist is computed from.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub rp: VmRemotePlay,
    pub network: String,
    pub nic: String,
    pub mac: Option<String>,
    pub address: Option<Ipv4Addr>,
    pub sources: Vec<String>,
    pub running: bool,
    pub guest_agent: bool,
    /// Passed-through PCI GPUs (vendor id, address).
    pub gpus: Vec<(String, String)>,
    /// Mediated / virtual GPU attached.
    pub vgpu: bool,
    pub relay_listening: Option<Result<(), String>>,
    pub wol_listening: Vec<(u16, Result<(), String>)>,
    pub nft_applied: Option<Result<(), String>>,
    pub forwards: usize,
    /// The VM has Horcrux firewall rules (then remote play adds allow rules).
    pub vm_firewall: bool,
    pub firewall_allows: bool,
    pub guest: Option<GuestReport>,
    /// What happens to the VM when the host shuts down (`save`,
    /// `hibernate`, `shutdown`).
    pub host_shutdown: String,
    pub resume_on_boot: bool,
}

fn item(id: &str, title: &str, status: &str, detail: String, fix: &str) -> CheckItem {
    CheckItem {
        id: id.into(),
        title: title.into(),
        status: status.into(),
        detail,
        fix: fix.into(),
    }
}

fn gpu_encoder(vendor: &str) -> Option<&'static str> {
    match vendor
        .trim_start_matches("0x")
        .to_ascii_lowercase()
        .as_str()
    {
        "10de" => Some("NVENC (NVIDIA)"),
        "1002" => Some("AMF (AMD)"),
        "8086" => Some("Quick Sync (Intel)"),
        _ => None,
    }
}

/// The prerequisite checklist (order = the order on the page).
pub fn checklist(f: &Facts) -> Vec<CheckItem> {
    let mut out = Vec::new();
    let rp = &f.rp;
    if !rp.enabled {
        out.push(item(
            "enabled",
            "Remote Play",
            "info",
            "Off. Turn it on to prepare this VM for Steam Remote Play / Steam Link (and Moonlight, if chosen).".into(),
            "Turn on Remote Play on this page.",
        ));
        return out;
    }
    let steam = rp.clients.contains(&RemotePlayClient::Steam);
    let moon = rp.clients.contains(&RemotePlayClient::Moonlight);
    out.push(item(
        "enabled",
        "Remote Play",
        "ok",
        format!(
            "On for {}.",
            rp.clients
                .iter()
                .map(|c| c.label())
                .collect::<Vec<_>>()
                .join(" and ")
        ),
        "",
    ));

    // ---- network
    match f.network.as_str() {
        "none" => out.push(item(
            "network",
            "Network card on the LAN",
            "fail",
            "The VM has no network card: clients can't reach it.".into(),
            "Add a network card on the LAN (\"Use a LAN network card\" below, or the Hardware page).",
        )),
        "lan" => out.push(item(
            "network",
            "Network card on the LAN",
            "ok",
            format!(
                "{}: clients on the LAN find the VM directly (discovery broadcasts reach it).",
                f.nic
            ),
            "",
        )),
        _ => out.push(item(
            "network",
            "Network card on the LAN",
            "warn",
            format!(
                "{}: the VM is behind NAT. Horcrux relays discovery and forwards the ports from the LAN; a card on the LAN (bridged or macvtap) is simpler and what Valve supports.",
                f.nic
            ),
            "Use a LAN network card (button below; the VM must be stopped), or keep NAT with the relay.",
        )),
    }
    out.push(item(
        "sources",
        "Allowed networks",
        if f.sources.is_empty() { "fail" } else { "ok" },
        if f.sources.is_empty() {
            "No allowed LAN network: nothing is forwarded or relayed and no Wake-on-LAN packet is accepted.".into()
        } else {
            format!(
                "Discovery, streaming and Wake-on-LAN are accepted from {}{}.",
                f.sources.join(", "),
                if rp.lan_sources.is_empty() {
                    " (the host's LAN networks)"
                } else {
                    ""
                }
            )
        },
        if f.sources.is_empty() {
            "Set the allowed networks (CIDR) on this page."
        } else {
            ""
        },
    ));
    match (f.address, f.network.as_str()) {
        (Some(a), _) => out.push(item(
            "address",
            "VM address",
            "ok",
            format!("{a} (MAC {}).", f.mac.clone().unwrap_or_default()),
            "",
        )),
        (None, "nat") => out.push(item(
            "address",
            "VM address",
            if f.running { "warn" } else { "unknown" },
            "Not known yet: the forwards need it (read from the guest agent or the host's neighbour table once the guest uses the network).".into(),
            if f.running {
                "Check the guest has an IPv4 address on its network."
            } else {
                "Start the VM."
            },
        )),
        (None, _) => out.push(item(
            "address",
            "VM address",
            "info",
            "Not known (not needed: clients find the VM on the LAN themselves).".into(),
            "",
        )),
    }
    if f.network == "nat" {
        if steam {
            match &f.relay_listening {
                Some(Ok(())) => out.push(item(
                    "discovery",
                    "Steam discovery relay",
                    "ok",
                    "Listening; LAN discovery requests are copied to the VM and its answers back."
                        .into(),
                    "",
                )),
                Some(Err(e)) => out.push(item(
                    "discovery",
                    "Steam discovery relay",
                    "fail",
                    format!("Not listening: {e}"),
                    "Free the discovery port on the host (another Steam on the host uses it), or use a LAN network card.",
                )),
                None => out.push(item(
                    "discovery",
                    "Steam discovery relay",
                    "warn",
                    "Off in the node settings: Steam clients won't list this VM (Moonlight can still add it by the host's address).".into(),
                    "Turn on discovery_relay in the Remote Play node settings.",
                )),
            }
        }
        match &f.nft_applied {
            Some(Ok(())) if f.forwards > 0 => out.push(item(
                "forwards",
                "Port forwards from the LAN",
                "ok",
                format!(
                    "{} rule(s) applied (nftables), only from the allowed networks.",
                    f.forwards
                ),
                "",
            )),
            Some(Err(e)) => out.push(item(
                "forwards",
                "Port forwards from the LAN",
                "fail",
                format!("Could not apply the rules: {e}"),
                "Check nftables (nft) is installed on the host.",
            )),
            _ => out.push(item(
                "forwards",
                "Port forwards from the LAN",
                "unknown",
                "Not applied yet (they need the VM's address).".into(),
                "",
            )),
        }
    } else if f.network == "lan" {
        if f.vm_firewall {
            out.push(item(
                "firewall",
                "VM firewall",
                if f.firewall_allows { "ok" } else { "warn" },
                if f.firewall_allows {
                    "The VM's Horcrux firewall allows the Remote Play ports from the allowed networks.".into()
                } else {
                    "The VM has Horcrux firewall rules; the Remote Play allow rules are not in place yet.".into()
                },
                "",
            ));
        } else {
            out.push(item(
                "firewall",
                "VM firewall",
                "ok",
                "No Horcrux firewall rules on this VM: nothing on the host blocks the ports."
                    .into(),
                "",
            ));
        }
    }
    out.push(item(
        "client_isolation",
        "Wi-Fi client isolation off",
        "info",
        "Steam Remote Play needs clients and host to talk directly: guest networks and Wi-Fi \"AP/client isolation\" block it (Steam's own requirement).".into(),
        "",
    ));

    // ---- wake
    if rp.wake_on_lan {
        let bound: Vec<u16> = f
            .wol_listening
            .iter()
            .filter(|(_, r)| r.is_ok())
            .map(|(p, _)| *p)
            .collect();
        let failed: Vec<String> = f
            .wol_listening
            .iter()
            .filter_map(|(p, r)| r.as_ref().err().map(|e| format!("{p}: {e}")))
            .collect();
        out.push(item(
            "wake_on_lan",
            "Wake-on-LAN",
            if bound.is_empty() { "fail" } else if failed.is_empty() { "ok" } else { "warn" },
            format!(
                "A magic packet for {} on UDP {} starts the VM (or resumes it from saved state).{}{}",
                f.mac.clone().unwrap_or_else(|| "the VM's MAC".into()),
                if bound.is_empty() {
                    "-".into()
                } else {
                    bound.iter().map(|p| p.to_string()).collect::<Vec<_>>().join("/")
                },
                if moon { " Moonlight sends it by itself when you open the PC." } else { "" },
                if failed.is_empty() {
                    String::new()
                } else {
                    format!(" Not listening on {}.", failed.join(", "))
                }
            ),
            if bound.is_empty() {
                "Free a Wake-on-LAN port on the host or change wol_ports in the node settings."
            } else {
                ""
            },
        ));
    } else {
        out.push(item(
            "wake_on_lan",
            "Wake-on-LAN",
            "info",
            "Off: start the VM from Horcrux before connecting.".into(),
            "",
        ));
    }
    if steam {
        out.push(item(
            "wake_on_discovery",
            "Start when a Steam client looks for it",
            "info",
            if rp.wake_on_discovery {
                "On: any Steam client on an allowed network that opens its Remote Play list starts this VM.".into()
            } else {
                "Off (default): every open Steam client searches all the time, so this would start the VM whenever someone opens Steam.".into()
            },
            "",
        ));
    }

    // ---- display + encode
    if let Some((vendor, addr)) = f.gpus.first() {
        out.push(item(
            "encode",
            "Hardware video encoder",
            "ok",
            format!(
                "GPU {addr}: Steam encodes with {} in the guest (install the vendor's Windows driver).",
                gpu_encoder(vendor).unwrap_or("the GPU's encoder")
            ),
            "",
        ));
        out.push(item(
            "display",
            "Display for streaming",
            "info",
            "A passed-through GPU without a monitor attached has no desktop to capture: the Horcrux guest tools' virtual display gives Windows one at any resolution and refresh, so Steam can match the client's (or connect a monitor/dummy plug).".into(),
            "Install the Horcrux guest tools (Console page) and keep the virtual display on (node setting console virtual_display).",
        ));
    } else if f.vgpu {
        out.push(item(
            "encode",
            "Hardware video encoder",
            "warn",
            "Virtual GPU: hardware encoding depends on the vendor's vGPU driver; otherwise Steam falls back to software encoding.".into(),
            "",
        ));
    } else {
        out.push(item(
            "encode",
            "Hardware video encoder",
            "warn",
            "No GPU passed through: Steam encodes in software on the VM's CPUs. Expect about 1080p at 30-60 fps for desktop apps and light games, with higher latency; 3D games need a GPU.".into(),
            "Add a GPU on the VM's Hardware page for real gaming.",
        ));
    }
    out.push(item(
        "devices",
        "Audio and controllers",
        "ok",
        "Steam brings its own streaming speakers/microphone and controllers (Steam Input) in the guest. The Horcrux virtual gamepad from the guest tools stays idle outside a Horcrux Console session; if a game picks it as player 1, turn it off in Windows' game controllers or Steam's controller settings.".into(),
        "",
    ));

    // ---- guest
    out.push(item(
        "guest_agent",
        "Guest agent",
        if f.guest_agent { "ok" } else { "warn" },
        if f.guest_agent {
            "On: Horcrux can install Steam and set logon/power options in the guest.".into()
        } else {
            "Off: the guest options below must be set by hand in the guest.".into()
        },
        if f.guest_agent { "" } else { "Turn on the guest agent on the Hardware page and install the QEMU guest agent in the guest." },
    ));
    match &f.guest {
        None => out.push(item(
            "guest",
            "Steam in the guest",
            "unknown",
            "Not checked yet.".into(),
            "Run \"Check / set up the guest\" (the VM must be running).",
        )),
        Some(g) => {
            if steam {
                out.push(item(
                    "steam",
                    "Steam in the guest",
                    if g.steam_installed { "ok" } else { "fail" },
                    match (g.steam_installed, g.steam_running) {
                        (true, true) => "Installed and running.".into(),
                        (true, false) => {
                            "Installed, not running (it starts at logon; sign in to Steam once)."
                                .into()
                        }
                        _ => "Not installed.".into(),
                    },
                    if g.steam_installed {
                        ""
                    } else {
                        "Run the guest setup with \"Install Steam\"."
                    },
                ));
            }
            if moon {
                out.push(item(
                    "sunshine",
                    "Sunshine (Moonlight host) in the guest",
                    if g.sunshine_installed { "ok" } else { "fail" },
                    if g.sunshine_installed {
                        "Installed.".into()
                    } else {
                        "Not installed. Moonlight needs a GameStream-compatible host in the guest; Horcrux does not ship one.".into()
                    },
                    if g.sunshine_installed { "" } else { "Install Sunshine in the guest yourself (its own installer)." },
                ));
            }
            out.push(item(
                "auto_login",
                "Logs on by itself",
                if g.auto_login { "ok" } else { "warn" },
                if g.auto_login {
                    format!("Windows logs on as {} after a start, so Steam is ready after a wake.", g.auto_login_user)
                } else {
                    "Windows stops at the logon screen after a start: Steam isn't running until someone signs in.".into()
                },
                if g.auto_login { "" } else { "Turn on automatic logon in the guest setup (enter the account password; Windows keeps it as an LSA secret)." },
            ));
            out.push(item(
                "keep_awake",
                "Guest doesn't sleep",
                if g.keep_awake { "ok" } else { "warn" },
                if g.keep_awake {
                    "Sleep and hibernate timers are off in the guest.".into()
                } else {
                    "Windows may sleep and drop off the network.".into()
                },
                "",
            ));
            if !g.network_profile.is_empty() {
                out.push(item(
                    "network_profile",
                    "Windows network profile",
                    if g.network_profile == "Public" && !g.steam_firewall_rules { "warn" } else { "ok" },
                    format!(
                        "{}{}",
                        g.network_profile,
                        if g.steam_firewall_rules { "; Windows Firewall rules for Steam present" } else { "; no Windows Firewall rule for Steam yet (Steam asks for it on its first start)" }
                    ),
                    "",
                ));
            }
        }
    }
    out.push(item(
        "host_shutdown",
        "Host shutdown and restart",
        "ok",
        format!(
            "When the host shuts down this VM is {}{}; a wake after that resumes it.",
            match f.host_shutdown.as_str() {
                "save" => "saved with its memory",
                "hibernate" => "hibernated by the guest",
                "shutdown" => "shut down cleanly",
                "shutdown-gpu" => "shut down cleanly (a VM with a passed-through GPU can't be saved with its memory)",
                "poweroff" => "powered off",
                "" => "handled by the node's guest power policy",
                other => other,
            },
            if f.resume_on_boot { " and comes back at boot" } else { "" }
        ),
        "",
    ));
    out
}

// --------------------------------------------------------- guest setup

/// The PowerShell the guest agent runs (as SYSTEM) in a Windows guest.
/// Secrets come on stdin as JSON, never on the command line. It prints one
/// JSON [`GuestReport`] line prefixed with `HXRP:`.
pub fn guest_setup_script() -> &'static str {
    include_str!("remote_play_guest.ps1")
}

/// JSON the script reads on stdin.
pub fn guest_setup_input(
    rp: &VmRemotePlay,
    install_steam: bool,
    steam_url: &str,
    auto_login: Option<(&str, &str)>,
    auto_login_off: bool,
) -> serde_json::Value {
    serde_json::json!({
        "install_steam": install_steam,
        "steam_url": steam_url,
        "big_picture": rp.big_picture,
        "keep_awake": rp.keep_awake,
        "auto_login_user": auto_login.map(|(u, _)| u),
        "auto_login_password": auto_login.map(|(_, p)| p),
        "auto_login_off": auto_login_off,
    })
}

/// The report line out of the script's output.
pub fn parse_guest_report(stdout: &str) -> Option<GuestReport> {
    let line = stdout.lines().rev().find(|l| l.starts_with("HXRP:"))?;
    serde_json::from_str(&line["HXRP:".len()..]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn magic(mac: [u8; 6]) -> Vec<u8> {
        let mut p = vec![0xff; 6];
        for _ in 0..16 {
            p.extend_from_slice(&mac);
        }
        p
    }

    #[test]
    fn wol_magic_packets_parse_with_headers_and_passwords() {
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        assert_eq!(parse_wol(&magic(mac)), Some(mac));
        // SecureOn password after it
        let mut p = magic(mac);
        p.extend_from_slice(&[1, 2, 3, 4, 5, 6]);
        assert_eq!(parse_wol(&p), Some(mac));
        // a header in front, and a longer 0xFF run
        let mut p = vec![0x42, 0xff, 0xff];
        p.extend(magic(mac));
        assert_eq!(parse_wol(&p), Some(mac));
        // damaged copies or short packets are not wakes
        let mut bad = magic(mac);
        bad[50] ^= 1;
        assert_eq!(parse_wol(&bad), None);
        assert_eq!(parse_wol(&magic(mac)[..90]), None);
        assert_eq!(parse_wol(&[0xff; 120]), None);
        assert_eq!(parse_mac("52:54:00:12:34:56"), Some(mac));
        assert_eq!(parse_mac("52-54-00-12-34-56"), Some(mac));
        assert_eq!(parse_mac("52:54:00:12:34"), None);
        assert_eq!(format_mac(&mac), "52:54:00:12:34:56");
    }

    #[test]
    fn steam_discovery_framing() {
        let p = steam_discovery_packet(0x1122334455667788, STEAM_MSG_DISCOVERY, 7);
        assert_eq!(steam_discovery_type(&p), Some(STEAM_MSG_DISCOVERY));
        let s = steam_discovery_packet(9, STEAM_MSG_STATUS, 1);
        assert_eq!(steam_discovery_type(&s), Some(STEAM_MSG_STATUS));
        // truncated body, wrong magic, nonsense
        assert_eq!(steam_discovery_type(&p[..p.len() - 1]), None);
        let mut w = p.clone();
        w[4] = 0;
        assert_eq!(steam_discovery_type(&w), None);
        assert_eq!(steam_discovery_type(&[0xff; 64]), None);
        // a real capture's prefix (header: client_id + msg_type 0)
        let mut cap = STEAM_DISCOVERY_MAGIC.to_vec();
        cap.extend_from_slice(&[0x0c, 0, 0, 0]);
        cap.extend_from_slice(&[
            0x08, 0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6, 0x07, 0x10, 0x00, 0x18, 0x05,
        ]);
        cap.extend_from_slice(&[0x02, 0, 0, 0, 0x08, 0x01]);
        assert_eq!(steam_discovery_type(&cap), Some(0));
    }

    fn sys(dir: &Path, iface: &str, physical: bool, ports: &[&str]) {
        let p = dir.join(iface);
        std::fs::create_dir_all(&p).unwrap();
        if physical {
            std::fs::create_dir_all(p.join("device")).unwrap();
        }
        if !ports.is_empty() {
            std::fs::create_dir_all(p.join("bridge")).unwrap();
            std::fs::create_dir_all(p.join("brif")).unwrap();
            for port in ports {
                std::fs::create_dir_all(p.join("brif").join(port)).unwrap();
            }
        }
    }

    #[test]
    fn reach_tells_lan_bridges_from_nat_bridges() {
        let d = tempfile::tempdir().unwrap();
        sys(d.path(), "enp5s0", true, &[]);
        sys(d.path(), "vmbr0", false, &["enp5s0", "tap100i0"]);
        sys(d.path(), "natbr", false, &["tap101i0", "veth3"]);
        // a VLAN uplink is stacked on a physical NIC
        sys(d.path(), "enp5s0.20", false, &[]);
        std::fs::create_dir_all(d.path().join("enp5s0.20/lower_enp5s0")).unwrap();
        sys(d.path(), "vmbr20", false, &["enp5s0.20"]);
        let nic = |br: &str, attach| VmNic {
            model: Default::default(),
            bridge: br.into(),
            attach,
            mac: Some("52:54:00:00:00:01".into()),
            vlan: None,
            rate_mbps: None,
        };
        assert!(matches!(
            nic_reach(&[nic("vmbr0", NicAttach::Bridge)], d.path()),
            Reach::Lan { .. }
        ));
        assert!(matches!(
            nic_reach(&[nic("vmbr20", NicAttach::Bridge)], d.path()),
            Reach::Lan { .. }
        ));
        assert_eq!(
            nic_reach(&[nic("natbr", NicAttach::Bridge)], d.path()),
            Reach::Nat {
                bridge: "natbr".into()
            }
        );
        assert!(matches!(
            nic_reach(&[nic("enp5s0", NicAttach::Macvtap)], d.path()),
            Reach::Lan { .. }
        ));
        assert_eq!(nic_reach(&[], d.path()), Reach::None);
        let mut rp = VmRemotePlay::default();
        assert_eq!(
            resolve_network(&rp, &Reach::Nat { bridge: "x".into() }),
            "nat"
        );
        rp.network = RemotePlayNetwork::Lan;
        assert_eq!(
            resolve_network(&rp, &Reach::Nat { bridge: "x".into() }),
            "lan"
        );
        // LAN networks: the uplink bridge's, not the NAT bridge's or docker's
        let addrs = vec![
            ("lo".to_string(), "127.0.0.0/8".to_string()),
            ("vmbr0".into(), "192.168.1.0/24".into()),
            ("natbr".into(), "10.77.0.0/24".into()),
            ("docker0".into(), "172.17.0.0/16".into()),
            ("enp5s0.20".into(), "192.168.20.0/24".into()),
        ];
        assert_eq!(
            host_lan_networks(&addrs, d.path()),
            vec!["192.168.1.0/24".to_string(), "192.168.20.0/24".to_string()]
        );
    }

    #[test]
    fn addresses_come_from_neighbours_or_the_guest_agent() {
        let neigh = r#"[{"dst":"10.77.0.10","dev":"natbr","lladdr":"52:54:00:aa:bb:cc","state":["REACHABLE"]},
            {"dst":"10.77.0.11","dev":"natbr","lladdr":"52:54:00:aa:bb:dd","state":["FAILED"]},
            {"dst":"10.77.0.12","dev":"natbr","state":["INCOMPLETE"]}]"#;
        let pairs = parse_ip_neigh_json(neigh);
        assert_eq!(pairs.len(), 1);
        assert_eq!(
            address_for_mac(&pairs, "52:54:00:AA:BB:CC"),
            Some("10.77.0.10".parse().unwrap())
        );
        assert_eq!(address_for_mac(&pairs, "52:54:00:aa:bb:dd"), None);
        let qga = serde_json::json!([
            {"name":"Ethernet","hardware-address":"52:54:00:aa:bb:cc","ip-addresses":[
                {"ip-address-type":"ipv6","ip-address":"fe80::1","prefix":64},
                {"ip-address-type":"ipv4","ip-address":"169.254.3.4","prefix":16},
                {"ip-address-type":"ipv4","ip-address":"10.77.0.20","prefix":24}]},
            {"name":"Loopback","hardware-address":"00:00:00:00:00:00","ip-addresses":[
                {"ip-address-type":"ipv4","ip-address":"127.0.0.1","prefix":8}]}]);
        let pairs = parse_qga_interfaces(&qga);
        assert_eq!(
            address_for_mac(&pairs, "52:54:00:aa:bb:cc"),
            Some("10.77.0.20".parse().unwrap())
        );
        let addr = r#"[{"ifname":"vmbr0","addr_info":[{"family":"inet","local":"192.168.1.7","prefixlen":24,"scope":"global"},
            {"family":"inet","local":"127.0.0.1","prefixlen":8,"scope":"host"}]}]"#;
        assert_eq!(
            parse_ip_addr_json(addr),
            vec![("vmbr0".to_string(), "192.168.1.0/24".to_string())]
        );
    }

    #[test]
    fn relay_copies_requests_to_vms_and_answers_back_only_within_scope() {
        let mut r = Relay {
            targets: vec![
                RelayTarget {
                    vm: "g1".into(),
                    addr: "10.77.0.10".parse().unwrap(),
                    sources: vec!["192.168.1.0/24".into()],
                    running: true,
                    relay: true,
                    wake_on_discovery: false,
                },
                RelayTarget {
                    vm: "g2".into(),
                    addr: "10.77.0.11".parse().unwrap(),
                    sources: vec!["192.168.1.0/24".into()],
                    running: false,
                    relay: true,
                    wake_on_discovery: true,
                },
            ],
            ..Default::default()
        };
        let ttl = Duration::from_secs(30);
        let t0 = Instant::now();
        let disc = steam_discovery_packet(1, STEAM_MSG_DISCOVERY, 1);
        let cli: SocketAddrV4 = "192.168.1.50:27036".parse().unwrap();
        let a = r.on_datagram(cli, &disc, 27036, ttl, t0);
        assert_eq!(a.send, vec!["10.77.0.10:27036".parse().unwrap()]);
        assert_eq!(a.wake, vec!["g2".to_string()]);
        // not a discovery datagram: ignored
        assert_eq!(
            r.on_datagram(cli, b"hello", 27036, ttl, t0),
            RelayActions::default()
        );
        // outside the allowed networks: nothing, and not remembered
        let stranger: SocketAddrV4 = "10.9.9.9:27036".parse().unwrap();
        assert_eq!(
            r.on_datagram(stranger, &disc, 27036, ttl, t0),
            RelayActions::default()
        );
        // the VM's answer goes to the client that asked
        let status = steam_discovery_packet(2, STEAM_MSG_STATUS, 1);
        let vm: SocketAddrV4 = "10.77.0.10:27036".parse().unwrap();
        let a = r.on_datagram(vm, &status, 27036, ttl, t0 + Duration::from_secs(1));
        assert_eq!(a.send, vec![cli]);
        assert!(a.wake.is_empty());
        // ... until the client goes quiet
        let a = r.on_datagram(vm, &status, 27036, ttl, t0 + Duration::from_secs(31));
        assert!(a.send.is_empty());
        assert_eq!(r.clients(), 0);
        // a status (not a discovery request) never wakes a VM
        let a = r.on_datagram(cli, &status, 27036, ttl, t0);
        assert!(a.wake.is_empty());
        // a LAN VM listed only to be woken: no copies to it, and its own
        // broadcasts are not relayed (clients hear it directly)
        let mut lan = Relay {
            targets: vec![RelayTarget {
                vm: "g3".into(),
                addr: Ipv4Addr::UNSPECIFIED,
                sources: vec!["192.168.1.0/24".into()],
                running: true,
                relay: false,
                wake_on_discovery: true,
            }],
            ..Default::default()
        };
        assert_eq!(
            lan.on_datagram(cli, &disc, 27036, ttl, t0),
            RelayActions::default()
        );
        lan.targets[0].running = false;
        assert_eq!(
            lan.on_datagram(cli, &disc, 27036, ttl, t0).wake,
            vec!["g3".to_string()]
        );
        let lan_vm: SocketAddrV4 = "192.168.1.60:27036".parse().unwrap();
        assert!(lan
            .on_datagram(lan_vm, &status, 27036, ttl, t0)
            .send
            .is_empty());
    }

    #[test]
    fn nft_forwards_are_scoped_to_the_lan_and_skip_the_relay_port() {
        use horcrux_common::remote_play::{parse_ports, ports_without};
        let udp = ports_without(&parse_ports("27031-27036").unwrap(), 27036);
        let f = Forward {
            vm: "g1".into(),
            client: RemotePlayClient::Steam,
            bridge: "natbr".into(),
            to: "10.77.0.10".parse().unwrap(),
            sources: vec!["192.168.1.0/24".into(), "192.168.20.0/24".into()],
            tcp: parse_ports("27036-27037").unwrap(),
            udp,
        };
        let t = render_nft("horcrux_remote_play", std::slice::from_ref(&f));
        assert!(t.starts_with("table inet horcrux_remote_play {"));
        assert!(t.contains("iifname != \"natbr\" ip saddr { 192.168.1.0/24, 192.168.20.0/24 } tcp dport { 27036-27037 } dnat ip to 10.77.0.10 comment \"remote-play g1 steam\""), "{t}");
        assert!(
            t.contains("udp dport { 27031-27035 } dnat ip to 10.77.0.10"),
            "{t}"
        );
        assert!(t.contains("ip daddr 10.77.0.10 tcp dport { 27036-27037 } ct status dnat accept"));
        assert!(t.contains("type nat hook prerouting priority dstnat; policy accept;"));
        // no sources = nothing forwarded at all
        let mut none = f.clone();
        none.sources.clear();
        assert_eq!(render_nft("t", &[none]), "");
        assert_eq!(render_nft("t", &[]), "");
        // names can't break out of the comment
        let mut evil = f;
        evil.vm = "x\" accept; drop".into();
        assert!(!render_nft("t", &[evil]).contains("\" accept;"));
        assert!(nft_apply_script("t", "").starts_with("add table inet t\ndelete table inet t\n"));
    }

    fn base_facts() -> Facts {
        Facts {
            rp: VmRemotePlay {
                enabled: true,
                ..Default::default()
            },
            network: "nat".into(),
            nic: "bridge natbr".into(),
            mac: Some("52:54:00:aa:bb:cc".into()),
            address: Some("10.77.0.10".parse().unwrap()),
            sources: vec!["192.168.1.0/24".into()],
            running: true,
            guest_agent: true,
            relay_listening: Some(Ok(())),
            wol_listening: vec![(9, Ok(())), (7, Err("in use".into()))],
            nft_applied: Some(Ok(())),
            forwards: 4,
            host_shutdown: "save".into(),
            resume_on_boot: true,
            ..Default::default()
        }
    }

    fn st<'a>(c: &'a [CheckItem], id: &str) -> &'a str {
        c.iter()
            .find(|i| i.id == id)
            .map(|i| i.status.as_str())
            .unwrap_or("missing")
    }

    #[test]
    fn checklist_reports_each_prerequisite() {
        let off = checklist(&Facts::default());
        assert_eq!(off.len(), 1);
        assert_eq!(off[0].status, "info");

        let c = checklist(&base_facts());
        assert_eq!(
            st(&c, "network"),
            "warn",
            "NAT works but LAN is recommended"
        );
        assert_eq!(st(&c, "discovery"), "ok");
        assert_eq!(st(&c, "forwards"), "ok");
        assert_eq!(
            st(&c, "wake_on_lan"),
            "warn",
            "one WoL port could not be bound"
        );
        assert_eq!(st(&c, "encode"), "warn", "no GPU: software encoding");
        assert_eq!(st(&c, "guest"), "unknown");
        assert!(c.iter().all(|i| i.id != "firewall"));

        let mut f = base_facts();
        f.network = "lan".into();
        f.nic = "macvtap on enp5s0".into();
        f.gpus = vec![("10de".into(), "0000:01:00.0".into())];
        f.vm_firewall = true;
        f.guest = Some(GuestReport {
            steam_installed: true,
            steam_running: true,
            auto_login: false,
            keep_awake: true,
            network_profile: "Private".into(),
            ..Default::default()
        });
        let c = checklist(&f);
        assert_eq!(st(&c, "network"), "ok");
        assert!(c.iter().all(|i| i.id != "discovery" && i.id != "forwards"));
        assert_eq!(st(&c, "firewall"), "warn");
        assert_eq!(st(&c, "encode"), "ok");
        assert!(c.iter().any(|i| i.detail.contains("NVENC")));
        assert_eq!(st(&c, "steam"), "ok");
        assert_eq!(st(&c, "auto_login"), "warn");
        assert_eq!(st(&c, "sunshine"), "missing", "moonlight not chosen");

        let mut f = base_facts();
        f.rp.clients = vec![RemotePlayClient::Moonlight];
        f.relay_listening = Some(Err("address in use".into()));
        f.guest = Some(GuestReport::default());
        f.address = None;
        let c = checklist(&f);
        assert_eq!(st(&c, "discovery"), "missing", "Steam relay only for Steam");
        assert_eq!(st(&c, "sunshine"), "fail");
        assert_eq!(st(&c, "address"), "warn");
        assert!(c.iter().any(|i| i.detail.contains("Moonlight sends it")));
    }

    #[test]
    fn guest_setup_keeps_secrets_off_the_command_line() {
        let s = guest_setup_script();
        assert!(s.contains("[Console]::In.ReadToEnd()"));
        assert!(s.contains("LsaStorePrivateData"));
        assert!(s.contains("steam://open/bigpicture"));
        assert!(s.contains("HXRP:"));
        // never the plain-text registry password
        assert!(s.contains("Remove-ItemProperty") && s.contains("DefaultPassword"));
        let rp = VmRemotePlay {
            enabled: true,
            big_picture: true,
            ..Default::default()
        };
        let i = guest_setup_input(
            &rp,
            true,
            "https://x/SteamSetup.exe",
            Some(("gamer", "pw")),
            false,
        );
        assert_eq!(i["auto_login_user"], "gamer");
        assert_eq!(i["big_picture"], true);
        let r = parse_guest_report("noise\nHXRP:{\"steam_installed\":true,\"os\":\"windows\"}\n")
            .unwrap();
        assert!(r.steam_installed && r.os == "windows");
        assert!(parse_guest_report("nothing").is_none());
    }
}
