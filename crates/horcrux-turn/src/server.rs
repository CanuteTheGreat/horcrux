//! The Horcrux relay: a TURN server (RFC 8656 subset, UDP relaying over
//! UDP or TCP client connections) built into the node.
//!
//! Security properties (docs/design/horcrux-console.md section 5 and 12):
//! * Off unless the node's WAN access is turned on (the caller starts it).
//! * Credentials are minted per console session (TURN REST style:
//!   `username = "<expiry unix>:<tag>"`, `password = base64(HMAC-SHA1(secret,
//!   username))`), valid for a new allocation only until the expiry, and
//!   revoked (allocations dropped) when the session ends.
//! * Not an open relay: permissions and channels are only granted towards
//!   peers matching the configured rules (the node's own console media
//!   sockets), and data from or to any other address is dropped.
//! * Never decrypts: it forwards DTLS/SRTP datagrams as they are.
use crate::stun::{self, attr, method, Builder, Class, Message};
use hmac::{Hmac, Mac};
use sha1::Sha1;
use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Peers the relay may forward to: one address and a port range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRule {
    pub ip: IpAddr,
    pub port_min: u16,
    pub port_max: u16,
}

impl PeerRule {
    fn matches(&self, a: &SocketAddr) -> bool {
        a.ip() == self.ip && (self.port_min..=self.port_max).contains(&a.port())
    }
}

#[derive(Debug, Clone)]
pub struct RelayConfig {
    /// Where clients reach the relay (UDP, and TCP when `tcp`).
    pub listen: SocketAddr,
    pub tcp: bool,
    pub realm: String,
    /// Shared secret the credentials are derived from.
    pub secret: Vec<u8>,
    /// Address the relayed sockets bind to and advertise
    /// (XOR-RELAYED-ADDRESS); the console sockets must reach it.
    pub relay_ip: IpAddr,
    pub port_min: u16,
    pub port_max: u16,
    pub allowed_peers: Vec<PeerRule>,
    pub max_allocations: usize,
    pub default_lifetime_secs: u32,
    pub max_lifetime_secs: u32,
    /// Seconds a nonce stays valid before the client gets 438 Stale Nonce.
    pub nonce_lifetime_secs: u32,
}

impl Default for RelayConfig {
    fn default() -> Self {
        RelayConfig {
            listen: "0.0.0.0:3478".parse().expect("addr"),
            tcp: true,
            realm: "horcrux".into(),
            secret: Vec::new(),
            relay_ip: "127.0.0.1".parse().expect("ip"),
            port_min: 61300,
            port_max: 61399,
            allowed_peers: Vec::new(),
            max_allocations: 64,
            default_lifetime_secs: 600,
            max_lifetime_secs: 3600,
            nonce_lifetime_secs: 3600,
        }
    }
}

/// Live counters (API status page).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelayStats {
    pub allocations: usize,
    pub allocations_total: u64,
    pub auth_failures: u64,
    pub forbidden_peers: u64,
    /// Requests with a valid credential of an ended (revoked) session:
    /// normal while a closing client deallocates, not an attack signal.
    pub revoked_rejects: u64,
    pub bytes_to_peers: u64,
    pub bytes_to_clients: u64,
}

const SOFTWARE: &[u8] = b"Horcrux relay";
const PERMISSION_LIFETIME: Duration = Duration::from_secs(300);
const CHANNEL_LIFETIME: Duration = Duration::from_secs(600);

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn hmac_sha1(key: &[u8], data: &[u8]) -> [u8; 20] {
    let mut m = Hmac::<Sha1>::new_from_slice(key).expect("hmac key");
    m.update(data);
    m.finalize().into_bytes().into()
}

/// Credentials for one client: valid for a new allocation until
/// `expiry_unix`; `tag` names the console session for revocation.
pub fn credentials(secret: &[u8], tag: &str, expiry_unix: u64) -> (String, String) {
    use base64::Engine as _;
    let username = format!("{expiry_unix}:{tag}");
    let password =
        base64::engine::general_purpose::STANDARD.encode(hmac_sha1(secret, username.as_bytes()));
    (username, password)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ClientKey {
    Udp(SocketAddr),
    Tcp(u64),
}

#[derive(Clone)]
enum Sink {
    Udp(Arc<UdpSocket>, SocketAddr),
    Tcp(mpsc::Sender<Vec<u8>>),
}

impl Sink {
    fn send(&self, b: Vec<u8>) {
        match self {
            Sink::Udp(s, a) => {
                let _ = s.try_send_to(&b, *a);
            }
            Sink::Tcp(tx) => {
                let _ = tx.try_send(b);
            }
        }
    }
    fn stream(&self) -> bool {
        matches!(self, Sink::Tcp(_))
    }
}

struct Allocation {
    username: String,
    tag: String,
    key: [u8; 16],
    socket: Arc<UdpSocket>,
    relayed: SocketAddr,
    expires: Instant,
    perms: HashMap<IpAddr, Instant>,
    channels: HashMap<u16, (SocketAddr, Instant)>,
    by_peer: HashMap<SocketAddr, u16>,
    sink: Sink,
    reader: Option<JoinHandle<()>>,
    /// The Allocate transaction and its answer (a retransmitted request
    /// gets the same answer).
    last_allocate: ([u8; 12], Vec<u8>),
}

impl Drop for Allocation {
    fn drop(&mut self) {
        if let Some(r) = self.reader.take() {
            r.abort();
        }
    }
}

#[derive(Default)]
struct State {
    allocs: HashMap<ClientKey, Allocation>,
    revoked: HashMap<String, Instant>,
}

struct Inner {
    cfg: RelayConfig,
    state: Mutex<State>,
    allocations_total: AtomicU64,
    auth_failures: AtomicU64,
    forbidden: AtomicU64,
    revoked_rejects: AtomicU64,
    to_peers: AtomicU64,
    to_clients: AtomicU64,
    next_conn: AtomicU64,
}

/// A running relay. Dropping it (or `stop`) ends every task.
pub struct Relay {
    inner: Arc<Inner>,
    tasks: Vec<JoinHandle<()>>,
    pub udp_addr: SocketAddr,
    pub tcp_addr: Option<SocketAddr>,
}

impl Drop for Relay {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
        if let Ok(mut s) = self.inner.state.lock() {
            s.allocs.clear();
        }
    }
}

impl Relay {
    pub async fn start(cfg: RelayConfig) -> io::Result<Relay> {
        if cfg.secret.len() < 16 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "relay secret must be at least 16 bytes",
            ));
        }
        let udp = Arc::new(UdpSocket::bind(cfg.listen).await?);
        let udp_addr = udp.local_addr()?;
        let tcp = if cfg.tcp {
            Some(TcpListener::bind(SocketAddr::new(cfg.listen.ip(), udp_addr.port())).await?)
        } else {
            None
        };
        let tcp_addr = tcp.as_ref().and_then(|l| l.local_addr().ok());
        let inner = Arc::new(Inner {
            cfg,
            state: Mutex::new(State::default()),
            allocations_total: AtomicU64::new(0),
            auth_failures: AtomicU64::new(0),
            forbidden: AtomicU64::new(0),
            revoked_rejects: AtomicU64::new(0),
            to_peers: AtomicU64::new(0),
            to_clients: AtomicU64::new(0),
            next_conn: AtomicU64::new(1),
        });
        let mut tasks = Vec::new();
        {
            let inner = inner.clone();
            let udp = udp.clone();
            tasks.push(tokio::spawn(async move {
                let mut buf = vec![0u8; 65536];
                loop {
                    let Ok((n, src)) = udp.recv_from(&mut buf).await else {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                        continue;
                    };
                    let sink = Sink::Udp(udp.clone(), src);
                    if let Some(resp) = inner.handle(ClientKey::Udp(src), src, &buf[..n], &sink) {
                        let _ = udp.try_send_to(&resp, src);
                    }
                }
            }));
        }
        if let Some(l) = tcp {
            let inner = inner.clone();
            tasks.push(tokio::spawn(async move {
                while let Ok((s, src)) = l.accept().await {
                    let _ = s.set_nodelay(true);
                    tokio::spawn(tcp_conn(inner.clone(), s, src));
                }
            }));
        }
        {
            let inner = inner.clone();
            tasks.push(tokio::spawn(async move {
                let mut t = tokio::time::interval(Duration::from_secs(2));
                loop {
                    t.tick().await;
                    inner.sweep();
                }
            }));
        }
        Ok(Relay {
            inner,
            tasks,
            udp_addr,
            tcp_addr,
        })
    }

    /// Credentials for a console session (`tag`), valid for `lifetime`.
    pub fn credentials(&self, tag: &str, lifetime: Duration) -> (String, String) {
        credentials(
            &self.inner.cfg.secret,
            tag,
            unix_now() + lifetime.as_secs().max(1),
        )
    }

    /// The session ended: refuse its credentials and drop its allocations.
    pub fn revoke(&self, tag: &str) {
        if let Ok(mut s) = self.inner.state.lock() {
            let keep = Instant::now() + Duration::from_secs(self.inner.cfg.max_lifetime_secs as u64);
            s.revoked.insert(tag.to_string(), keep);
            s.allocs.retain(|_, a| a.tag != tag);
        }
    }

    pub fn stats(&self) -> RelayStats {
        let allocations = self
            .inner
            .state
            .lock()
            .map(|s| s.allocs.len())
            .unwrap_or(0);
        RelayStats {
            allocations,
            allocations_total: self.inner.allocations_total.load(Ordering::Relaxed),
            auth_failures: self.inner.auth_failures.load(Ordering::Relaxed),
            forbidden_peers: self.inner.forbidden.load(Ordering::Relaxed),
            revoked_rejects: self.inner.revoked_rejects.load(Ordering::Relaxed),
            bytes_to_peers: self.inner.to_peers.load(Ordering::Relaxed),
            bytes_to_clients: self.inner.to_clients.load(Ordering::Relaxed),
        }
    }

    /// Relayed addresses currently allocated for `tag` (tests and the
    /// session's path detection).
    pub fn relayed_addresses(&self, tag: &str) -> Vec<SocketAddr> {
        self.inner
            .state
            .lock()
            .map(|s| {
                s.allocs
                    .values()
                    .filter(|a| a.tag == tag)
                    .map(|a| a.relayed)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn config(&self) -> &RelayConfig {
        &self.inner.cfg
    }
}

async fn tcp_conn(inner: Arc<Inner>, s: tokio::net::TcpStream, src: SocketAddr) {
    let id = inner.next_conn.fetch_add(1, Ordering::Relaxed);
    let key = ClientKey::Tcp(id);
    let (mut rd, mut wr) = s.into_split();
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(1024);
    let writer = tokio::spawn(async move {
        while let Some(b) = rx.recv().await {
            if wr.write_all(&b).await.is_err() {
                break;
            }
        }
    });
    let sink = Sink::Tcp(tx.clone());
    let mut buf: Vec<u8> = Vec::with_capacity(8192);
    let mut chunk = vec![0u8; 16384];
    'outer: loop {
        let n = match rd.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        buf.extend_from_slice(&chunk[..n]);
        while let Some(len) = stun::stream_frame_len(&buf) {
            if len > 65536 + 24 {
                break 'outer;
            }
            if buf.len() < len {
                break;
            }
            let frame: Vec<u8> = buf.drain(..len).collect();
            if let Some(resp) = inner.handle(key, src, &frame, &sink) {
                if tx.send(resp).await.is_err() {
                    break 'outer;
                }
            }
        }
    }
    if let Ok(mut st) = inner.state.lock() {
        st.allocs.remove(&key);
    }
    writer.abort();
}

enum Auth {
    Ok {
        username: String,
        tag: String,
        key: [u8; 16],
    },
    Reply(Vec<u8>),
}

impl Inner {
    fn nonce(&self) -> String {
        let ts = unix_now();
        let mac = hmac_sha1(&self.cfg.secret, format!("nonce:{ts}").as_bytes());
        let hex: String = mac[..8].iter().map(|b| format!("{b:02x}")).collect();
        format!("{ts:x}-{hex}")
    }

    fn nonce_ok(&self, n: &str) -> Result<(), bool> {
        let Some((ts, _)) = n.split_once('-') else {
            return Err(false);
        };
        let Ok(t) = u64::from_str_radix(ts, 16) else {
            return Err(false);
        };
        let mac = hmac_sha1(&self.cfg.secret, format!("nonce:{t}").as_bytes());
        let hex: String = mac[..8].iter().map(|b| format!("{b:02x}")).collect();
        if format!("{ts}-{hex}") != n {
            return Err(false);
        }
        if unix_now().saturating_sub(t) > self.cfg.nonce_lifetime_secs as u64 {
            return Err(true);
        }
        Ok(())
    }

    fn err(&self, m: &Message, code: u16, reason: &str, key: Option<&[u8]>) -> Vec<u8> {
        let mut b = Builder::new(m.method, Class::Error, m.txid)
            .error(code, reason)
            .attr(attr::SOFTWARE, SOFTWARE);
        if code == 401 || code == 438 {
            b = b
                .attr(attr::REALM, self.cfg.realm.as_bytes())
                .attr(attr::NONCE, self.nonce().as_bytes());
        }
        if let Some(k) = key {
            b = b.integrity(k);
        }
        b.fingerprint()
    }

    fn authenticate(&self, m: &Message, new_allocation: bool) -> Auth {
        if m.get(attr::MESSAGE_INTEGRITY).is_none() {
            return Auth::Reply(self.err(m, 401, "Unauthorized", None));
        }
        let (Some(username), Some(_realm), Some(nonce)) = (
            m.get_str(attr::USERNAME),
            m.get_str(attr::REALM),
            m.get_str(attr::NONCE),
        ) else {
            return Auth::Reply(self.err(m, 400, "Bad Request", None));
        };
        match self.nonce_ok(&nonce) {
            Ok(()) => {}
            Err(true) => return Auth::Reply(self.err(m, 438, "Stale Nonce", None)),
            Err(false) => {
                self.auth_failures.fetch_add(1, Ordering::Relaxed);
                return Auth::Reply(self.err(m, 401, "Unauthorized", None));
            }
        }
        let fail = |s: &Inner| {
            s.auth_failures.fetch_add(1, Ordering::Relaxed);
            Auth::Reply(s.err(m, 401, "Unauthorized", None))
        };
        let Some((exp, tag)) = username.split_once(':') else {
            return fail(self);
        };
        let Ok(exp) = exp.parse::<u64>() else {
            return fail(self);
        };
        let (_, password) = credentials(&self.cfg.secret, tag, exp);
        let key = stun::long_term_key(&username, &self.cfg.realm, &password);
        if !m.check_integrity(&key) {
            return fail(self);
        }
        let revoked = self
            .state
            .lock()
            .map(|s| s.revoked.contains_key(tag))
            .unwrap_or(true);
        if revoked {
            self.revoked_rejects.fetch_add(1, Ordering::Relaxed);
            return Auth::Reply(self.err(m, 401, "Unauthorized", None));
        }
        if new_allocation && exp < unix_now() {
            return fail(self);
        }
        Auth::Ok {
            username: username.clone(),
            tag: tag.to_string(),
            key,
        }
    }

    fn peer_allowed(&self, a: &SocketAddr) -> bool {
        self.cfg.allowed_peers.iter().any(|r| r.matches(a))
    }

    fn peer_ip_allowed(&self, ip: IpAddr) -> bool {
        self.cfg.allowed_peers.iter().any(|r| r.ip == ip)
    }

    /// One message from a client; returns the answer to send back.
    fn handle(
        self: &Arc<Self>,
        key: ClientKey,
        src: SocketAddr,
        b: &[u8],
        sink: &Sink,
    ) -> Option<Vec<u8>> {
        if let Some((ch, data)) = stun::parse_channel_data(b) {
            let st = self.state.lock().ok()?;
            let a = st.allocs.get(&key)?;
            let (peer, until) = a.channels.get(&ch)?;
            let now = Instant::now();
            if *until < now || a.perms.get(&peer.ip()).map(|t| *t < now).unwrap_or(true) {
                return None;
            }
            if !self.peer_allowed(peer) {
                self.forbidden.fetch_add(1, Ordering::Relaxed);
                return None;
            }
            if a.socket.try_send_to(data, *peer).is_ok() {
                self.to_peers.fetch_add(data.len() as u64, Ordering::Relaxed);
            }
            return None;
        }
        let m = Message::parse(b).ok()?;
        match (m.method, m.class) {
            (method::BINDING, Class::Request) => Some(
                Builder::new(method::BINDING, Class::Success, m.txid)
                    .xor_addr(attr::XOR_MAPPED_ADDRESS, src)
                    .attr(attr::SOFTWARE, SOFTWARE)
                    .fingerprint(),
            ),
            (method::SEND, Class::Indication) => {
                let peer = m.get_xor_addr(attr::XOR_PEER_ADDRESS)?;
                let data = m.get(attr::DATA)?;
                let st = self.state.lock().ok()?;
                let a = st.allocs.get(&key)?;
                if a.perms
                    .get(&peer.ip())
                    .map(|t| *t < Instant::now())
                    .unwrap_or(true)
                {
                    return None;
                }
                if !self.peer_allowed(&peer) {
                    self.forbidden.fetch_add(1, Ordering::Relaxed);
                    return None;
                }
                if a.socket.try_send_to(data, peer).is_ok() {
                    self.to_peers.fetch_add(data.len() as u64, Ordering::Relaxed);
                }
                None
            }
            (_, Class::Request) => {
                let unknown = m.unknown_required();
                if !unknown.is_empty() {
                    let v: Vec<u8> = unknown.iter().flat_map(|t| t.to_be_bytes()).collect();
                    return Some(
                        Builder::new(m.method, Class::Error, m.txid)
                            .error(420, "Unknown Attribute")
                            .attr(attr::UNKNOWN_ATTRIBUTES, &v)
                            .fingerprint(),
                    );
                }
                match m.method {
                    method::ALLOCATE => Some(self.allocate(key, src, &m, sink)),
                    method::REFRESH | method::CREATE_PERMISSION | method::CHANNEL_BIND => {
                        Some(self.on_allocation(key, &m))
                    }
                    _ => Some(self.err(&m, 400, "Bad Request", None)),
                }
            }
            _ => None,
        }
    }

    fn allocate(self: &Arc<Self>, key: ClientKey, src: SocketAddr, m: &Message, sink: &Sink) -> Vec<u8> {
        if let Ok(st) = self.state.lock() {
            if let Some(a) = st.allocs.get(&key) {
                if a.last_allocate.0 == m.txid {
                    return a.last_allocate.1.clone();
                }
            }
        }
        let (username, tag, k) = match self.authenticate(m, true) {
            Auth::Ok { username, tag, key } => (username, tag, key),
            Auth::Reply(r) => return r,
        };
        if self
            .state
            .lock()
            .map(|s| s.allocs.contains_key(&key))
            .unwrap_or(true)
        {
            return self.err(m, 437, "Allocation Mismatch", Some(&k));
        }
        match m.get_u32(attr::REQUESTED_TRANSPORT) {
            Some(v) if (v >> 24) == 17 => {}
            Some(_) => return self.err(m, 442, "Unsupported Transport Protocol", Some(&k)),
            None => return self.err(m, 400, "Bad Request", Some(&k)),
        }
        if let Some(f) = m.get(attr::REQUESTED_ADDRESS_FAMILY) {
            let want_v6 = f.first() == Some(&2);
            if want_v6 != self.cfg.relay_ip.is_ipv6() {
                return self.err(m, 440, "Address Family not Supported", Some(&k));
            }
        }
        if m.get(attr::RESERVATION_TOKEN).is_some() {
            return self.err(m, 508, "Insufficient Capacity", Some(&k));
        }
        let count = self.state.lock().map(|s| s.allocs.len()).unwrap_or(usize::MAX);
        if count >= self.cfg.max_allocations {
            return self.err(m, 486, "Allocation Quota Reached", Some(&k));
        }
        let Some(sock) = self.bind_relayed() else {
            return self.err(m, 508, "Insufficient Capacity", Some(&k));
        };
        let sock = Arc::new(sock);
        let relayed = match sock.local_addr() {
            Ok(a) => SocketAddr::new(self.cfg.relay_ip, a.port()),
            Err(_) => return self.err(m, 508, "Insufficient Capacity", Some(&k)),
        };
        let lifetime = m
            .get_u32(attr::LIFETIME)
            .unwrap_or(self.cfg.default_lifetime_secs)
            .clamp(1, self.cfg.max_lifetime_secs);
        let resp = Builder::new(method::ALLOCATE, Class::Success, m.txid)
            .xor_addr(attr::XOR_RELAYED_ADDRESS, relayed)
            .u32(attr::LIFETIME, lifetime)
            .xor_addr(attr::XOR_MAPPED_ADDRESS, src)
            .attr(attr::SOFTWARE, SOFTWARE)
            .integrity(&k)
            .fingerprint();
        let reader = tokio::spawn(peer_reader(self.clone(), key, sock.clone()));
        let a = Allocation {
            username,
            tag,
            key: k,
            socket: sock,
            relayed,
            expires: Instant::now() + Duration::from_secs(lifetime as u64),
            perms: HashMap::new(),
            channels: HashMap::new(),
            by_peer: HashMap::new(),
            sink: sink.clone(),
            reader: Some(reader),
            last_allocate: (m.txid, resp.clone()),
        };
        if let Ok(mut st) = self.state.lock() {
            st.allocs.insert(key, a);
        }
        self.allocations_total.fetch_add(1, Ordering::Relaxed);
        tracing::debug!("relay: allocation {relayed} for {src}");
        resp
    }

    fn bind_relayed(&self) -> Option<UdpSocket> {
        let (lo, hi) = (self.cfg.port_min, self.cfg.port_max);
        let span = (hi - lo) as u32 + 1;
        let start: u32 = rand::random::<u32>() % span;
        for i in 0..span {
            let port = lo + ((start + i) % span) as u16;
            if let Ok(s) = std::net::UdpSocket::bind(SocketAddr::new(self.cfg.relay_ip, port)) {
                if s.set_nonblocking(true).is_ok() {
                    if let Ok(s) = UdpSocket::from_std(s) {
                        return Some(s);
                    }
                }
            }
        }
        None
    }

    fn on_allocation(&self, key: ClientKey, m: &Message) -> Vec<u8> {
        let (username, k) = match self.authenticate(m, false) {
            Auth::Ok { username, key, .. } => (username, key),
            Auth::Reply(r) => return r,
        };
        let Ok(mut st) = self.state.lock() else {
            return self.err(m, 500, "Server Error", Some(&k));
        };
        let Some(a) = st.allocs.get_mut(&key) else {
            return self.err(m, 437, "Allocation Mismatch", Some(&k));
        };
        if a.username != username || a.key != k {
            drop(st);
            self.auth_failures.fetch_add(1, Ordering::Relaxed);
            return self.err(m, 441, "Wrong Credentials", None);
        }
        let now = Instant::now();
        let ok = |b: Builder| b.attr(attr::SOFTWARE, SOFTWARE).integrity(&k).fingerprint();
        match m.method {
            method::REFRESH => {
                let lt = m
                    .get_u32(attr::LIFETIME)
                    .unwrap_or(self.cfg.default_lifetime_secs)
                    .min(self.cfg.max_lifetime_secs);
                if lt == 0 {
                    st.allocs.remove(&key);
                } else {
                    a.expires = now + Duration::from_secs(lt as u64);
                }
                ok(Builder::new(method::REFRESH, Class::Success, m.txid).u32(attr::LIFETIME, lt))
            }
            method::CREATE_PERMISSION => {
                let peers: Vec<SocketAddr> = m
                    .attrs
                    .iter()
                    .filter(|(t, _)| *t == attr::XOR_PEER_ADDRESS)
                    .filter_map(|(_, v)| stun::decode_xor_addr(v, &m.txid))
                    .collect();
                if peers.is_empty() {
                    return self.err(m, 400, "Bad Request", Some(&k));
                }
                if peers.iter().any(|p| !self.peer_ip_allowed(p.ip())) {
                    self.forbidden.fetch_add(1, Ordering::Relaxed);
                    return self.err(m, 403, "Forbidden", Some(&k));
                }
                for p in peers {
                    a.perms.insert(p.ip(), now + PERMISSION_LIFETIME);
                }
                ok(Builder::new(method::CREATE_PERMISSION, Class::Success, m.txid))
            }
            _ => {
                let ch = m.get(attr::CHANNEL_NUMBER).filter(|v| v.len() >= 2).map(|v| u16::from_be_bytes([v[0], v[1]]));
                let peer = m.get_xor_addr(attr::XOR_PEER_ADDRESS);
                let (Some(ch), Some(peer)) = (ch, peer) else {
                    return self.err(m, 400, "Bad Request", Some(&k));
                };
                if !(0x4000..=0x4FFF).contains(&ch) {
                    return self.err(m, 400, "Bad Request", Some(&k));
                }
                if !self.peer_allowed(&peer) {
                    self.forbidden.fetch_add(1, Ordering::Relaxed);
                    return self.err(m, 403, "Forbidden", Some(&k));
                }
                let clash = a.channels.get(&ch).map(|(p, _)| *p != peer).unwrap_or(false)
                    || a.by_peer.get(&peer).map(|c| *c != ch).unwrap_or(false);
                if clash {
                    return self.err(m, 400, "Bad Request", Some(&k));
                }
                a.channels.insert(ch, (peer, now + CHANNEL_LIFETIME));
                a.by_peer.insert(peer, ch);
                a.perms.insert(peer.ip(), now + PERMISSION_LIFETIME);
                ok(Builder::new(method::CHANNEL_BIND, Class::Success, m.txid))
            }
        }
    }

    fn sweep(&self) {
        let now = Instant::now();
        if let Ok(mut st) = self.state.lock() {
            st.allocs.retain(|_, a| a.expires > now);
            for a in st.allocs.values_mut() {
                a.perms.retain(|_, t| *t > now);
                let gone: Vec<u16> = a
                    .channels
                    .iter()
                    .filter(|(_, (_, t))| *t <= now)
                    .map(|(c, _)| *c)
                    .collect();
                for c in gone {
                    if let Some((p, _)) = a.channels.remove(&c) {
                        a.by_peer.remove(&p);
                    }
                }
            }
            st.revoked.retain(|_, t| *t > now);
            let revoked: HashSet<String> = st.revoked.keys().cloned().collect();
            st.allocs.retain(|_, a| !revoked.contains(&a.tag));
        }
    }
}

/// Datagrams from peers to the relayed address go back to the client.
async fn peer_reader(inner: Arc<Inner>, key: ClientKey, sock: Arc<UdpSocket>) {
    let mut buf = vec![0u8; 65536];
    loop {
        let Ok((n, peer)) = sock.recv_from(&mut buf).await else {
            return;
        };
        let out = {
            let Ok(st) = inner.state.lock() else { return };
            let Some(a) = st.allocs.get(&key) else { return };
            let now = Instant::now();
            if a.perms.get(&peer.ip()).map(|t| *t < now).unwrap_or(true)
                || !inner.peer_allowed(&peer)
            {
                None
            } else {
                let msg = match a.by_peer.get(&peer) {
                    Some(ch) => stun::channel_data(*ch, &buf[..n], a.sink.stream()),
                    None => Builder::new(method::DATA, Class::Indication, rand::random())
                        .xor_addr(attr::XOR_PEER_ADDRESS, peer)
                        .attr(attr::DATA, &buf[..n])
                        .build(),
                };
                Some((a.sink.clone(), msg))
            }
        };
        if let Some((sink, msg)) = out {
            inner.to_clients.fetch_add(n as u64, Ordering::Relaxed);
            sink.send(msg);
        }
    }
}
