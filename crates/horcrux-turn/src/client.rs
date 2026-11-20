//! Minimal TURN client over UDP (RFC 8656): allocate with long-term
//! credentials, permissions, channel binding, refresh, and ChannelData /
//! Send-indication framing. Used by the Horcrux desktop app to reach a
//! node through its relay and by the relay's own tests.
use crate::stun::{self, attr, method, Builder, Class, Message};
use std::io;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::UdpSocket;

pub struct TurnClient {
    pub socket: UdpSocket,
    pub server: SocketAddr,
    username: String,
    realm: String,
    nonce: String,
    key: [u8; 16],
    pub relayed: Option<SocketAddr>,
    pub mapped: Option<SocketAddr>,
    pub lifetime: u32,
}

fn io_err(s: impl Into<String>) -> io::Error {
    io::Error::other(s.into())
}

impl TurnClient {
    pub async fn new(server: SocketAddr, username: &str) -> io::Result<Self> {
        let bind: SocketAddr = if server.is_ipv4() {
            "0.0.0.0:0".parse().expect("addr")
        } else {
            "[::]:0".parse().expect("addr")
        };
        Ok(TurnClient {
            socket: UdpSocket::bind(bind).await?,
            server,
            username: username.to_string(),
            realm: String::new(),
            nonce: String::new(),
            key: [0; 16],
            relayed: None,
            mapped: None,
            lifetime: 0,
        })
    }

    /// Send a request and wait for its answer (retransmits at 250 ms,
    /// 500 ms, 1 s ...). Datagrams that are not the answer are dropped.
    pub async fn transact(&self, req: &[u8]) -> io::Result<Message> {
        let txid: [u8; 12] = req[8..20].try_into().expect("txid");
        let mut wait = Duration::from_millis(250);
        let mut buf = vec![0u8; 65536];
        for _ in 0..6 {
            self.socket.send_to(req, self.server).await?;
            let deadline = tokio::time::Instant::now() + wait;
            loop {
                let r = tokio::time::timeout_at(deadline, self.socket.recv_from(&mut buf)).await;
                let Ok(Ok((n, _))) = r else { break };
                if let Ok(m) = Message::parse(&buf[..n]) {
                    if m.txid == txid {
                        return Ok(m);
                    }
                }
            }
            wait *= 2;
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "no answer from the relay",
        ))
    }

    fn authed(&self, b: Builder) -> Vec<u8> {
        b.attr(attr::USERNAME, self.username.as_bytes())
            .attr(attr::REALM, self.realm.as_bytes())
            .attr(attr::NONCE, self.nonce.as_bytes())
            .integrity(&self.key)
            .fingerprint()
    }

    fn error_code(m: &Message) -> Option<u16> {
        m.get(attr::ERROR_CODE)
            .filter(|v| v.len() >= 4)
            .map(|v| v[2] as u16 * 100 + v[3] as u16)
    }

    /// Allocate a relayed address; `password` from the node's ticket.
    pub async fn allocate(&mut self, password: &str) -> io::Result<SocketAddr> {
        let first = Builder::request(method::ALLOCATE)
            .u32(attr::REQUESTED_TRANSPORT, 17 << 24)
            .fingerprint();
        let r = self.transact(&first).await?;
        if r.class != Class::Error || Self::error_code(&r) != Some(401) {
            return Err(io_err("relay did not ask for credentials"));
        }
        self.realm = r.get_str(attr::REALM).unwrap_or_default();
        self.nonce = r.get_str(attr::NONCE).unwrap_or_default();
        self.key = stun::long_term_key(&self.username, &self.realm, password);
        let req = self
            .authed(Builder::request(method::ALLOCATE).u32(attr::REQUESTED_TRANSPORT, 17 << 24));
        let r = self.transact(&req).await?;
        if r.class != Class::Success {
            return Err(io_err(format!(
                "allocate refused: {:?}",
                Self::error_code(&r)
            )));
        }
        if !r.check_integrity(&self.key) {
            return Err(io_err("relay answer failed the integrity check"));
        }
        let relayed = r
            .get_xor_addr(attr::XOR_RELAYED_ADDRESS)
            .ok_or_else(|| io_err("no relayed address"))?;
        self.relayed = Some(relayed);
        self.mapped = r.get_xor_addr(attr::XOR_MAPPED_ADDRESS);
        self.lifetime = r.get_u32(attr::LIFETIME).unwrap_or(0);
        Ok(relayed)
    }

    /// Run an authenticated request; returns the error code on refusal.
    pub async fn request(&mut self, b: Builder) -> io::Result<Result<Message, u16>> {
        let req = self.authed(b);
        let r = self.transact(&req).await?;
        if r.class == Class::Success {
            Ok(Ok(r))
        } else {
            Ok(Err(Self::error_code(&r).unwrap_or(0)))
        }
    }

    pub async fn create_permission(&mut self, peer: SocketAddr) -> io::Result<Result<(), u16>> {
        let b = Builder::request(method::CREATE_PERMISSION).xor_addr(attr::XOR_PEER_ADDRESS, peer);
        Ok(self.request(b).await?.map(|_| ()))
    }

    pub async fn channel_bind(&mut self, ch: u16, peer: SocketAddr) -> io::Result<Result<(), u16>> {
        let b = Builder::request(method::CHANNEL_BIND)
            .attr(attr::CHANNEL_NUMBER, &[(ch >> 8) as u8, ch as u8, 0, 0])
            .xor_addr(attr::XOR_PEER_ADDRESS, peer);
        Ok(self.request(b).await?.map(|_| ()))
    }

    pub async fn refresh(&mut self, lifetime: u32) -> io::Result<Result<(), u16>> {
        let b = Builder::request(method::REFRESH).u32(attr::LIFETIME, lifetime);
        Ok(self.request(b).await?.map(|_| ()))
    }

    pub async fn send_channel(&self, ch: u16, data: &[u8]) -> io::Result<()> {
        self.socket
            .send_to(&stun::channel_data(ch, data, false), self.server)
            .await
            .map(|_| ())
    }

    pub async fn send_indication(&self, peer: SocketAddr, data: &[u8]) -> io::Result<()> {
        let b = Builder::new(method::SEND, Class::Indication, rand::random())
            .xor_addr(attr::XOR_PEER_ADDRESS, peer)
            .attr(attr::DATA, data)
            .build();
        self.socket.send_to(&b, self.server).await.map(|_| ())
    }

    /// Next relayed datagram: (peer, payload). Channel data maps back to
    /// the peer through `peer_of`.
    pub async fn recv(
        &self,
        timeout: Duration,
        peer_of: impl Fn(u16) -> Option<SocketAddr>,
    ) -> io::Result<Option<(SocketAddr, Vec<u8>)>> {
        let mut buf = vec![0u8; 65536];
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let r = tokio::time::timeout_at(deadline, self.socket.recv_from(&mut buf)).await;
            let Ok(r) = r else { return Ok(None) };
            let (n, _) = r?;
            let b = &buf[..n];
            if let Some((ch, d)) = stun::parse_channel_data(b) {
                if let Some(p) = peer_of(ch) {
                    return Ok(Some((p, d.to_vec())));
                }
                continue;
            }
            if let Ok(m) = Message::parse(b) {
                if (m.method, m.class) == (method::DATA, Class::Indication) {
                    if let (Some(p), Some(d)) =
                        (m.get_xor_addr(attr::XOR_PEER_ADDRESS), m.get(attr::DATA))
                    {
                        return Ok(Some((p, d.to_vec())));
                    }
                }
            }
        }
    }
}
