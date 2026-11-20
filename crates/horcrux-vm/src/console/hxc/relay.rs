//! The node's relay for WAN console sessions (phase C3): starts and stops
//! the built-in TURN server ([`horcrux_turn`]) with the `[console] wan`
//! settings, mints per-session credentials for console tickets and revokes
//! them when the session ends. Off unless `wan.enabled`.
use super::portmap;
use horcrux_common::console::{ConsoleSettings, WanSettings};
use horcrux_turn::{PeerRule, Relay, RelayConfig, RelayStats};
use serde::Serialize;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

struct Running {
    relay: Relay,
    /// Settings the relay was started with (restart when they change).
    wan: WanSettings,
    udp_min: u16,
    udp_max: u16,
    candidates: Vec<String>,
}

static RELAY: Mutex<Option<Running>> = Mutex::new(None);
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);
/// Serialises `apply` (settings writes can race the startup call).
static APPLY: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// One ICE server entry for the client (RTCIceServer shape).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct IceServer {
    pub urls: Vec<String>,
    pub username: String,
    pub credential: String,
}

/// GET /api/console/relay.
#[derive(Debug, Clone, Serialize, Default)]
pub struct RelayStatus {
    pub enabled: bool,
    pub running: bool,
    pub listen: Option<String>,
    pub tcp: bool,
    /// What clients are told (empty host = the address they used for the UI).
    pub public_address: String,
    pub public_port: u16,
    pub relay_address: Option<String>,
    pub error: Option<String>,
    pub allocations: usize,
    pub allocations_total: u64,
    pub auth_failures: u64,
    pub forbidden_peers: u64,
    pub revoked_rejects: u64,
    pub bytes_to_peers: u64,
    pub bytes_to_clients: u64,
    pub port_mapping: portmap::MappingStatus,
}

/// Secret the relay credentials are derived from: `console-relay.key` next
/// to the console settings, 32 random bytes, mode 0600, created once (a
/// cluster can share it so any node's relay accepts the others' tickets).
fn secret() -> std::io::Result<Vec<u8>> {
    let path: PathBuf = super::settings::store_path().with_file_name("console-relay.key");
    if let Ok(b) = std::fs::read(&path) {
        if b.len() >= 16 {
            return Ok(b);
        }
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let key: [u8; 32] = rand::random();
    let tmp = path.with_extension("key.tmp");
    {
        use std::io::Write;
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        o.open(&tmp)?.write_all(&key)?;
    }
    std::fs::rename(&tmp, &path)?;
    Ok(key.to_vec())
}

/// IPv4 addresses of this node (interface addresses incl. loopback).
fn node_ipv4() -> Vec<IpAddr> {
    let mut v = Vec::new();
    if let Ok(ifs) = nix::ifaddrs::getifaddrs() {
        for i in ifs {
            if let Some(sin) = i.address.as_ref().and_then(|a| a.as_sockaddr_in()) {
                let ip = IpAddr::V4(sin.ip());
                if !v.contains(&ip) {
                    v.push(ip);
                }
            }
        }
    }
    v
}

/// Peers the relay may forward to: the node's console media sockets.
fn peer_rules(s: &ConsoleSettings) -> Vec<PeerRule> {
    let mut ips: Vec<IpAddr> = s
        .candidate_addresses
        .iter()
        .filter_map(|a| a.parse().ok())
        .collect();
    if ips.is_empty() {
        ips = node_ipv4();
    }
    ips.into_iter()
        .map(|ip| PeerRule {
            ip,
            port_min: s.udp_port_min,
            port_max: s.udp_port_max,
        })
        .collect()
}

fn relay_ip(s: &ConsoleSettings) -> IpAddr {
    if let Ok(ip) = s.wan.relay_peer_address.parse() {
        return ip;
    }
    let ips = node_ipv4();
    ips.iter()
        .copied()
        .find(|ip| !ip.is_loopback())
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST))
}

fn set_error(e: Option<String>) {
    if let Ok(mut g) = LAST_ERROR.lock() {
        *g = e;
    }
}

/// Start, restart or stop the relay to match `s` (call after any settings
/// change and at startup).
pub async fn apply(s: &ConsoleSettings) {
    let _serial = APPLY.lock().await;
    let wan = s.wan.clone();
    let keep = {
        let mut g = match RELAY.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        // Only what the listening relay itself uses needs a restart; the
        // rest (force_relay, public address, adaptation) is read per
        // ticket/session.
        let same = g
            .as_ref()
            .map(|r| {
                let (a, b) = (&r.wan, &wan);
                a.relay_listen_address == b.relay_listen_address
                    && a.relay_port == b.relay_port
                    && a.relay_tcp == b.relay_tcp
                    && a.relay_reply_from_request_address == b.relay_reply_from_request_address
                    && a.relay_peer_address == b.relay_peer_address
                    && a.relay_port_min == b.relay_port_min
                    && a.relay_port_max == b.relay_port_max
                    && a.relay_realm == b.relay_realm
                    && a.relay_max_allocations == b.relay_max_allocations
                    && r.udp_min == s.udp_port_min
                    && r.udp_max == s.udp_port_max
                    && r.candidates == s.candidate_addresses
            })
            .unwrap_or(false);
        if wan.enabled && same {
            if let Some(r) = g.as_mut() {
                r.wan = wan.clone();
            }
            true
        } else {
            if g.take().is_some() {
                tracing::info!("console relay stopped");
            }
            false
        }
    };
    if keep {
        portmap::apply(&wan).await;
        return;
    }
    portmap::apply(&wan).await;
    if !wan.enabled {
        set_error(None);
        return;
    }
    let key = match secret() {
        Ok(k) => k,
        Err(e) => {
            set_error(Some(format!("relay secret: {e}")));
            return;
        }
    };
    let listen = SocketAddr::new(
        wan.relay_listen_address
            .parse()
            .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
        wan.relay_port,
    );
    let cfg = RelayConfig {
        listen,
        tcp: wan.relay_tcp,
        reply_from_request_address: wan.relay_reply_from_request_address,
        realm: wan.relay_realm.clone(),
        secret: key,
        relay_ip: relay_ip(s),
        port_min: wan.relay_port_min,
        port_max: wan.relay_port_max,
        allowed_peers: peer_rules(s),
        max_allocations: wan.relay_max_allocations as usize,
        ..Default::default()
    };
    // A relay that was just stopped releases its sockets when its tasks
    // are dropped by the runtime: retry the bind for a moment.
    let mut started = Relay::start(cfg.clone()).await;
    for _ in 0..25 {
        match &started {
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                tokio::time::sleep(Duration::from_millis(200)).await;
                started = Relay::start(cfg.clone()).await;
            }
            _ => break,
        }
    }
    match started {
        Ok(relay) => {
            tracing::info!(
                "console relay listening on {} (relayed address {})",
                relay.udp_addr,
                relay.config().relay_ip
            );
            set_error(None);
            if let Ok(mut g) = RELAY.lock() {
                *g = Some(Running {
                    relay,
                    wan,
                    udp_min: s.udp_port_min,
                    udp_max: s.udp_port_max,
                    candidates: s.candidate_addresses.clone(),
                });
            }
        }
        Err(e) => {
            tracing::warn!("console relay did not start on {listen}: {e}");
            set_error(Some(format!("relay did not start on {listen}: {e}")));
        }
    }
}

/// `apply` from synchronous code (settings writes), when a runtime exists.
pub fn apply_spawn(s: &ConsoleSettings) {
    if let Ok(h) = tokio::runtime::Handle::try_current() {
        let s = s.clone();
        h.spawn(async move { apply(&s).await });
    }
}

/// Host part of an HTTP Host header (strips the port, keeps IPv6 brackets).
fn host_only(host: &str) -> String {
    let h = host.trim();
    if h.starts_with('[') {
        return h
            .split(']')
            .next()
            .map(|x| format!("{x}]"))
            .unwrap_or_default();
    }
    match h.rsplit_once(':') {
        Some((a, p)) if p.chars().all(|c| c.is_ascii_digit()) && !a.contains(':') => a.into(),
        _ => h.into(),
    }
}

/// ICE servers for a new session (`tag` = the ticket's relay tag), when
/// the relay runs. `ui_host` is the Host header the client used.
pub fn ice_servers(ui_host: Option<&str>, tag: &str) -> Vec<IceServer> {
    let Ok(g) = RELAY.lock() else {
        return Vec::new();
    };
    let Some(r) = g.as_ref() else {
        return Vec::new();
    };
    let mapped = portmap::status();
    let host = if !r.wan.relay_public_address.is_empty() {
        let a = &r.wan.relay_public_address;
        if a.contains(':') && !a.starts_with('[') {
            format!("[{a}]")
        } else {
            a.clone()
        }
    } else if let Some(ext) = mapped.external_address.as_ref().filter(|_| r.wan.upnp) {
        ext.clone()
    } else {
        ui_host.map(host_only).unwrap_or_default()
    };
    if host.is_empty() {
        return Vec::new();
    }
    let port = if r.wan.relay_public_port != 0 {
        r.wan.relay_public_port
    } else if let (true, Some(p)) = (r.wan.upnp, mapped.external_port) {
        p
    } else {
        r.relay.udp_addr.port()
    };
    let (username, credential) = r.relay.credentials(
        tag,
        Duration::from_secs(r.wan.relay_credential_lifetime_secs as u64),
    );
    let mut urls = vec![format!("turn:{host}:{port}?transport=udp")];
    if r.wan.relay_tcp {
        urls.push(format!("turn:{host}:{port}?transport=tcp"));
    }
    vec![IceServer {
        urls,
        username,
        credential,
    }]
}

/// The session ended: drop its relay allocations and refuse its credentials.
pub fn revoke(tag: &str) {
    if let Ok(g) = RELAY.lock() {
        if let Some(r) = g.as_ref() {
            r.relay.revoke(tag);
        }
    }
}

pub fn running() -> bool {
    RELAY.lock().map(|g| g.is_some()).unwrap_or(false)
}

pub fn force_relay() -> bool {
    RELAY
        .lock()
        .map(|g| g.as_ref().map(|r| r.wan.force_relay).unwrap_or(false))
        .unwrap_or(false)
}

pub fn status(s: &ConsoleSettings) -> RelayStatus {
    let mut st = RelayStatus {
        enabled: s.wan.enabled,
        tcp: s.wan.relay_tcp,
        public_address: s.wan.relay_public_address.clone(),
        public_port: if s.wan.relay_public_port != 0 {
            s.wan.relay_public_port
        } else {
            s.wan.relay_port
        },
        error: LAST_ERROR.lock().ok().and_then(|g| g.clone()),
        port_mapping: portmap::status(),
        ..Default::default()
    };
    if let Ok(g) = RELAY.lock() {
        if let Some(r) = g.as_ref() {
            let RelayStats {
                allocations,
                allocations_total,
                auth_failures,
                forbidden_peers,
                revoked_rejects,
                bytes_to_peers,
                bytes_to_clients,
            } = r.relay.stats();
            st.running = true;
            st.listen = Some(r.relay.udp_addr.to_string());
            st.relay_address = Some(r.relay.config().relay_ip.to_string());
            st.allocations = allocations;
            st.allocations_total = allocations_total;
            st.auth_failures = auth_failures;
            st.forbidden_peers = forbidden_peers;
            st.revoked_rejects = revoked_rejects;
            st.bytes_to_peers = bytes_to_peers;
            st.bytes_to_clients = bytes_to_clients;
        }
    }
    st
}

/// Remote addresses of the client's relayed ICE candidates in an SDP offer
/// (a session whose media arrives from one of them runs over the relay).
pub fn relay_candidates(sdp: &str) -> Vec<SocketAddr> {
    let mut v = Vec::new();
    for line in sdp.lines() {
        let Some(c) = line.trim().strip_prefix("a=candidate:") else {
            continue;
        };
        let f: Vec<&str> = c.split_whitespace().collect();
        // foundation component transport priority address port typ <type>
        if f.len() >= 8 && f[6] == "typ" && f[7] == "relay" {
            if let (Ok(ip), Ok(port)) = (f[4].parse::<IpAddr>(), f[5].parse::<u16>()) {
                v.push(SocketAddr::new(ip, port));
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_header_and_relay_candidates() {
        assert_eq!(host_only("node.example:8006"), "node.example");
        assert_eq!(host_only("10.0.0.5"), "10.0.0.5");
        assert_eq!(host_only("[2001:db8::1]:8006"), "[2001:db8::1]");
        let sdp = "v=0\r\na=candidate:1 1 udp 2113937151 192.168.1.20 50000 typ host\r\n\
                   a=candidate:2 1 udp 41885439 10.77.0.1 61333 typ relay raddr 0.0.0.0 rport 0\r\n";
        assert_eq!(
            relay_candidates(sdp),
            vec!["10.77.0.1:61333".parse::<SocketAddr>().unwrap()]
        );
    }

    #[test]
    fn peers_are_the_console_sockets_only() {
        let s = ConsoleSettings {
            candidate_addresses: vec!["192.0.2.4".into()],
            ..Default::default()
        };
        let r = peer_rules(&s);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].ip.to_string(), "192.0.2.4");
        assert_eq!(
            (r[0].port_min, r[0].port_max),
            (s.udp_port_min, s.udp_port_max)
        );
    }
}
