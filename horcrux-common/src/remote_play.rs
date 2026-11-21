//! "Remote Play ready" for VMs (docs/design/gaming-vm.md section 16):
//! the VM is prepared so Steam Remote Play / Steam Link clients (TVs,
//! phones, other PCs, Remote Play Together) and, as the user's choice,
//! Moonlight clients talking to a Sunshine host the user installed in the
//! guest can find, wake and stream from it on the LAN.
//!
//! Horcrux ships no Steam or Sunshine code: Steam (and Sunshine) run inside
//! the guest, installed by the user. Horcrux makes the network reach them
//! (a LAN network card, or a discovery relay plus port forwards behind
//! NAT, with firewall rules scoped to the LAN), wakes the VM on demand
//! (Wake-on-LAN to the VM's MAC), keeps the guest awake and logged in, and
//! shows every prerequisite on a checklist. Horcrux Console stays the
//! native path; this is an extra, off by default.
//!
//! Types here are shared by the API and the web UI.
use serde::{Deserialize, Serialize};

/// Streaming clients a VM is prepared for.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum RemotePlayClient {
    /// Steam Remote Play / Steam Link / Remote Play Together (Steam runs in
    /// the guest).
    Steam,
    /// Moonlight clients; the user installs a GameStream-compatible host
    /// (Sunshine) in the guest. Horcrux neither ships nor installs it.
    Moonlight,
}

impl RemotePlayClient {
    pub fn label(self) -> &'static str {
        match self {
            RemotePlayClient::Steam => "Steam Remote Play / Steam Link",
            RemotePlayClient::Moonlight => "Moonlight (Sunshine in the guest)",
        }
    }
}

/// How the VM reaches the LAN for remote play.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RemotePlayNetwork {
    /// LAN if the VM's network card is on the LAN (macvtap, or a bridge
    /// with a physical port), otherwise NAT.
    #[default]
    Auto,
    /// The VM's card is on the LAN: clients find it directly (recommended).
    Lan,
    /// The VM sits behind NAT: Horcrux relays discovery and forwards the
    /// ports from the LAN to it.
    Nat,
}

/// Per-VM remote play settings (`hardware.adv.remote_play`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct VmRemotePlay {
    pub enabled: bool,
    /// Clients to prepare for (default: Steam).
    pub clients: Vec<RemotePlayClient>,
    pub network: RemotePlayNetwork,
    /// IPv4 networks (CIDR) allowed to discover, stream from and wake this
    /// VM. Empty = the networks of the host's LAN interfaces.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lan_sources: Vec<String>,
    /// Start (or resume) the VM when a Wake-on-LAN packet for its MAC
    /// arrives from an allowed network.
    pub wake_on_lan: bool,
    /// Also start it when a Steam client on the LAN looks for Steam hosts
    /// (for Steam Link clients that send no Wake-on-LAN). Every open Steam
    /// client on the LAN does that, so it is off by default.
    pub wake_on_discovery: bool,
    /// Guest side: open Steam in Big Picture at logon.
    pub big_picture: bool,
    /// Guest side: keep Windows from sleeping or hibernating on its own
    /// (a sleeping guest can't answer a client).
    pub keep_awake: bool,
}

impl Default for VmRemotePlay {
    fn default() -> Self {
        Self {
            enabled: false,
            clients: vec![RemotePlayClient::Steam],
            network: RemotePlayNetwork::Auto,
            lan_sources: Vec::new(),
            wake_on_lan: true,
            wake_on_discovery: false,
            big_picture: false,
            keep_awake: true,
        }
    }
}

impl VmRemotePlay {
    pub fn validate(&self) -> Result<(), String> {
        if self.enabled && self.clients.is_empty() {
            return Err("remote_play.clients: pick at least one client (steam, moonlight)".into());
        }
        if self.lan_sources.len() > 32 {
            return Err("remote_play.lan_sources: at most 32 networks".into());
        }
        for s in &self.lan_sources {
            parse_cidr4(s).map_err(|e| format!("remote_play.lan_sources: {e}"))?;
        }
        if self.wake_on_discovery && !self.clients.contains(&RemotePlayClient::Steam) {
            return Err("remote_play.wake_on_discovery needs the steam client".into());
        }
        Ok(())
    }

    pub fn has(&self, c: RemotePlayClient) -> bool {
        self.enabled && self.clients.contains(&c)
    }
}

/// Ports and listeners of one client kind (node settings).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ClientPorts {
    /// TCP ports the guest listens on (`27036-27037`, comma lists allowed).
    pub tcp: String,
    /// UDP ports the guest listens on.
    pub udp: String,
}

/// Node settings (`GET/PUT /api/vms/remote-play/settings`, declarative
/// section `remote_play`). Every value has a default; the port numbers are
/// the ones the clients use and should only change if they do.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RemotePlaySettings {
    /// Steam Remote Play ports (Steam support: UDP 27031 and 27036, TCP
    /// 27036 and 27037; Steam uses the range in between).
    pub steam: ClientPorts,
    /// Moonlight / GameStream host ports (Sunshine defaults).
    pub moonlight: ClientPorts,
    /// UDP port of Steam's LAN discovery (broadcast). Behind NAT the
    /// node's relay listens here.
    pub steam_discovery_port: u16,
    /// Relay Steam discovery for VMs behind NAT.
    pub discovery_relay: bool,
    /// UDP ports the node listens on for Wake-on-LAN packets (9 and 7 are
    /// the usual ones; Moonlight also sends to its own ports).
    pub wol_ports: Vec<u16>,
    /// A VM is woken at most once per this many seconds.
    pub wake_cooldown_secs: u64,
    /// How long a client that sent a discovery packet gets the VM's
    /// answers relayed back (seconds).
    pub relay_client_secs: u64,
    /// How often the node re-reads VM addresses and re-applies the port
    /// forwards (seconds).
    pub reconcile_secs: u64,
    /// nftables table holding the port forwards (`inet <name>`).
    pub nft_table: String,
    /// Where the guest downloads the Steam installer from (Valve).
    pub steam_installer_url: String,
    /// Seconds the guest setup (Steam install, logon settings) may take.
    pub guest_setup_timeout_secs: u64,
}

impl Default for RemotePlaySettings {
    fn default() -> Self {
        Self {
            steam: ClientPorts {
                tcp: "27036-27037".into(),
                udp: "27031-27036".into(),
            },
            moonlight: ClientPorts {
                tcp: "47984,47989,48010".into(),
                udp: "47998-48000,48002,48010".into(),
            },
            steam_discovery_port: 27036,
            discovery_relay: true,
            wol_ports: vec![9, 7],
            wake_cooldown_secs: 60,
            relay_client_secs: 30,
            reconcile_secs: 15,
            nft_table: "horcrux_remote_play".into(),
            steam_installer_url:
                "https://cdn.cloudflare.steamstatic.com/client/installer/SteamSetup.exe".into(),
            guest_setup_timeout_secs: 900,
        }
    }
}

impl RemotePlaySettings {
    pub fn validate(&self) -> Result<(), String> {
        for (what, p) in [
            ("steam.tcp", &self.steam.tcp),
            ("steam.udp", &self.steam.udp),
            ("moonlight.tcp", &self.moonlight.tcp),
            ("moonlight.udp", &self.moonlight.udp),
        ] {
            parse_ports(p).map_err(|e| format!("{what}: {e}"))?;
        }
        if self.steam_discovery_port == 0 {
            return Err("steam_discovery_port: 1-65535".into());
        }
        if self.wol_ports.is_empty() || self.wol_ports.len() > 16 || self.wol_ports.contains(&0) {
            return Err("wol_ports: 1-16 UDP ports (9 and 7 are the usual ones)".into());
        }
        if !(1..=3600).contains(&self.wake_cooldown_secs) {
            return Err("wake_cooldown_secs: 1-3600".into());
        }
        if !(1..=600).contains(&self.relay_client_secs) {
            return Err("relay_client_secs: 1-600".into());
        }
        if !(2..=3600).contains(&self.reconcile_secs) {
            return Err("reconcile_secs: 2-3600".into());
        }
        let t = &self.nft_table;
        if t.is_empty()
            || t.len() > 32
            || !t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err("nft_table: 1-32 letters, digits or _".into());
        }
        if !self.steam_installer_url.starts_with("https://") || self.steam_installer_url.len() > 512
        {
            return Err("steam_installer_url must be an https:// URL".into());
        }
        if !(60..=7200).contains(&self.guest_setup_timeout_secs) {
            return Err("guest_setup_timeout_secs: 60-7200".into());
        }
        Ok(())
    }

    pub fn ports(&self, c: RemotePlayClient) -> &ClientPorts {
        match c {
            RemotePlayClient::Steam => &self.steam,
            RemotePlayClient::Moonlight => &self.moonlight,
        }
    }
}

/// `27031-27036,27040` -> [(27031, 27036), (27040, 27040)] (sorted, merged).
pub fn parse_ports(s: &str) -> Result<Vec<(u16, u16)>, String> {
    let mut out: Vec<(u16, u16)> = Vec::new();
    for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (a, b) = match part.split_once('-') {
            Some((a, b)) => (a.trim(), b.trim()),
            None => (part, part),
        };
        let a: u16 = a.parse().map_err(|_| format!("bad port {part:?}"))?;
        let b: u16 = b.parse().map_err(|_| format!("bad port {part:?}"))?;
        if a == 0 || b < a {
            return Err(format!("bad port range {part:?}"));
        }
        out.push((a, b));
    }
    if out.is_empty() {
        return Err("no ports".into());
    }
    if out.len() > 32 {
        return Err("at most 32 port ranges".into());
    }
    out.sort();
    let mut merged: Vec<(u16, u16)> = Vec::new();
    for (a, b) in out {
        match merged.last_mut() {
            Some(last) if a <= last.1.saturating_add(1) => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    Ok(merged)
}

/// Remove `port` from a range list (the node's discovery relay owns it).
pub fn ports_without(ranges: &[(u16, u16)], port: u16) -> Vec<(u16, u16)> {
    let mut out = Vec::new();
    for &(a, b) in ranges {
        if port < a || port > b {
            out.push((a, b));
            continue;
        }
        if a < port {
            out.push((a, port - 1));
        }
        if port < b {
            out.push((port + 1, b));
        }
    }
    out
}

/// nft set syntax: `27031-27035, 27037`.
pub fn ports_nft(ranges: &[(u16, u16)]) -> String {
    ranges
        .iter()
        .map(|(a, b)| {
            if a == b {
                a.to_string()
            } else {
                format!("{a}-{b}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `192.168.1.0/24` -> (network, prefix). The host bits must be zero
/// (`192.168.1.7/24` is refused: say `192.168.1.0/24` or `192.168.1.7/32`).
pub fn parse_cidr4(s: &str) -> Result<(std::net::Ipv4Addr, u8), String> {
    let (ip, len) = s
        .trim()
        .split_once('/')
        .ok_or_else(|| format!("{s:?}: an IPv4 network like 192.168.1.0/24"))?;
    let ip: std::net::Ipv4Addr = ip
        .parse()
        .map_err(|_| format!("{s:?}: an IPv4 network like 192.168.1.0/24"))?;
    let len: u8 = len
        .parse()
        .ok()
        .filter(|l| *l <= 32)
        .ok_or_else(|| format!("{s:?}: prefix length 0-32"))?;
    let mask = if len == 0 { 0 } else { u32::MAX << (32 - len) };
    if u32::from(ip) & !mask != 0 {
        return Err(format!(
            "{s:?}: host bits set (use {}/{len})",
            std::net::Ipv4Addr::from(u32::from(ip) & mask)
        ));
    }
    Ok((ip, len))
}

pub fn cidr4_contains(net: (std::net::Ipv4Addr, u8), ip: std::net::Ipv4Addr) -> bool {
    let mask = if net.1 == 0 {
        0
    } else {
        u32::MAX << (32 - net.1)
    };
    u32::from(ip) & mask == u32::from(net.0) & mask
}

/// One line of the prerequisite checklist.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckItem {
    /// Stable id (`network`, `wake_on_lan`, ...).
    pub id: String,
    pub title: String,
    /// `ok`, `warn`, `fail`, `info` or `unknown`.
    pub status: String,
    pub detail: String,
    /// What to do about it (empty when nothing).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fix: String,
}

/// A firewall / forwarding rule Horcrux keeps for remote play.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemotePlayRule {
    /// `forward` (NAT port forward), `allow` (VM firewall rule) or `relay`
    /// (node listener).
    pub kind: String,
    pub protocol: String,
    pub ports: String,
    /// Allowed source networks.
    pub sources: Vec<String>,
    /// Where it goes (the VM's address).
    pub to: String,
    pub client: RemotePlayClient,
    pub applied: bool,
}

/// `GET /api/vms/:id/remote-play/status`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct RemotePlayStatus {
    pub vm: String,
    pub config: VmRemotePlay,
    /// `lan` or `nat` (what `auto` resolved to), empty when off.
    pub network: String,
    /// The card the clients reach (`macvtap on enp5s0`, `bridge vmbr0`).
    pub nic: String,
    pub mac: Option<String>,
    pub address: Option<String>,
    /// Networks allowed in (resolved `lan_sources`).
    pub sources: Vec<String>,
    pub checks: Vec<CheckItem>,
    pub rules: Vec<RemotePlayRule>,
    /// The nftables text Horcrux applies for this VM (empty when none).
    pub nft: String,
    /// Last guest-side report (from the guest setup).
    pub guest: Option<GuestReport>,
}

/// What the guest setup found and changed inside the guest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct GuestReport {
    pub os: String,
    pub steam_installed: bool,
    pub steam_running: bool,
    pub sunshine_installed: bool,
    pub auto_login: bool,
    pub auto_login_user: String,
    pub big_picture: bool,
    pub keep_awake: bool,
    /// Windows network profile (`Private`, `Public`, `DomainAuthenticated`).
    pub network_profile: String,
    /// Windows Firewall rules for Steam exist.
    pub steam_firewall_rules: bool,
    /// Unix seconds.
    pub checked_at: i64,
    pub messages: Vec<String>,
}

/// `POST /api/vms/:id/remote-play/guest-setup`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GuestSetupRequest {
    /// Download and install Steam (from Valve) when it isn't installed.
    pub install_steam: bool,
    /// Turn automatic logon on for this Windows account (needs
    /// `password`; stored by Windows as an LSA secret, never by Horcrux).
    pub auto_login_user: Option<String>,
    pub password: Option<String>,
    /// Turn automatic logon off.
    pub auto_login_off: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_parse_merge_and_split_around_the_relay_port() {
        let p = parse_ports("27036-27037, 27031-27036").unwrap();
        assert_eq!(p, vec![(27031, 27037)]);
        assert_eq!(parse_ports("48010,47998-48000,48002").unwrap().len(), 3);
        assert!(parse_ports("").is_err());
        assert!(parse_ports("5-1").is_err());
        assert!(parse_ports("0").is_err());
        assert!(parse_ports("70000").is_err());
        let u = parse_ports("27031-27036").unwrap();
        assert_eq!(ports_without(&u, 27036), vec![(27031, 27035)]);
        assert_eq!(ports_without(&[(1, 9)], 5), vec![(1, 4), (6, 9)]);
        assert_eq!(ports_without(&[(7, 7)], 7), vec![]);
        assert_eq!(
            ports_nft(&[(27031, 27035), (27037, 27037)]),
            "27031-27035, 27037"
        );
    }

    #[test]
    fn cidrs_need_clean_networks() {
        assert!(parse_cidr4("192.168.1.0/24").is_ok());
        assert!(parse_cidr4("192.168.1.7/24")
            .unwrap_err()
            .contains("192.168.1.0/24"));
        assert!(parse_cidr4("10.0.0.1/32").is_ok());
        assert!(parse_cidr4("fe80::/64").is_err());
        assert!(parse_cidr4("10.0.0.0").is_err());
        let n = parse_cidr4("192.168.1.0/24").unwrap();
        assert!(cidr4_contains(n, "192.168.1.200".parse().unwrap()));
        assert!(!cidr4_contains(n, "192.168.2.1".parse().unwrap()));
        assert!(cidr4_contains(
            parse_cidr4("0.0.0.0/0").unwrap(),
            "8.8.8.8".parse().unwrap()
        ));
    }

    #[test]
    fn defaults_are_valid_and_off() {
        let v = VmRemotePlay::default();
        assert!(!v.enabled && v.wake_on_lan && !v.wake_on_discovery && v.keep_awake);
        assert_eq!(v.clients, vec![RemotePlayClient::Steam]);
        assert!(v.validate().is_ok());
        assert!(RemotePlaySettings::default().validate().is_ok());
        let s = RemotePlaySettings {
            nft_table: "bad table".into(),
            ..Default::default()
        };
        assert!(s.validate().is_err());
        let mut v = VmRemotePlay {
            enabled: true,
            clients: vec![],
            ..Default::default()
        };
        assert!(v.validate().is_err());
        v.clients = vec![RemotePlayClient::Moonlight];
        v.wake_on_discovery = true;
        assert!(v.validate().is_err());
        v.wake_on_discovery = false;
        v.lan_sources = vec!["192.168.1.0/24".into()];
        assert!(v.validate().is_ok());
    }

    #[test]
    fn old_configs_without_the_section_still_parse() {
        let v: VmRemotePlay = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert!(v.enabled && v.wake_on_lan);
        assert_eq!(v.clients, vec![RemotePlayClient::Steam]);
    }
}
