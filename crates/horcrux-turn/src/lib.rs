//! Horcrux Console relay (docs/design/horcrux-console.md section 12): a
//! TURN server built into every Horcrux node, the STUN/TURN codec, and a
//! small TURN client for the Horcrux desktop app. Clean-room from RFC 8489
//! (STUN), RFC 8656 (TURN) and RFC 5769 (test vectors); no third-party
//! relay service anywhere.
pub mod client;
pub mod pktinfo;
pub mod server;
pub mod stun;

pub use server::{credentials, PeerRule, Relay, RelayConfig, RelayStats};

#[cfg(test)]
mod tests {
    use super::client::TurnClient;
    use super::*;
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::net::UdpSocket;

    async fn relay_with_peer(tcp: bool) -> (Relay, UdpSocket, SocketAddr) {
        let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let pa = peer.local_addr().unwrap();
        let cfg = RelayConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            tcp,
            secret: b"0123456789abcdef0123".to_vec(),
            relay_ip: "127.0.0.1".parse().unwrap(),
            port_min: 40000,
            port_max: 40999,
            allowed_peers: vec![PeerRule {
                ip: pa.ip(),
                port_min: pa.port(),
                port_max: pa.port(),
            }],
            max_allocations: 2,
            ..Default::default()
        };
        (Relay::start(cfg).await.unwrap(), peer, pa)
    }

    #[tokio::test]
    async fn relays_both_ways_through_a_channel_and_indications() {
        let (relay, peer, pa) = relay_with_peer(false).await;
        let (u, p) = relay.credentials("sess1", Duration::from_secs(60));
        let mut c = TurnClient::new(relay.udp_addr, &u).await.unwrap();
        let relayed = c.allocate(&p).await.unwrap();
        assert_eq!(relayed.ip().to_string(), "127.0.0.1");
        assert!((40000..=40999).contains(&relayed.port()));
        assert_eq!(
            c.mapped.unwrap().port(),
            c.socket.local_addr().unwrap().port()
        );
        assert_eq!(relay.relayed_addresses("sess1"), vec![relayed]);

        // Before a permission nothing passes.
        c.send_indication(pa, b"early").await.unwrap();
        let mut buf = [0u8; 64];
        assert!(
            tokio::time::timeout(Duration::from_millis(200), peer.recv_from(&mut buf))
                .await
                .is_err()
        );

        c.create_permission(pa).await.unwrap().unwrap();
        c.send_indication(pa, b"hello peer").await.unwrap();
        let (n, from) = peer.recv_from(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"hello peer");
        assert_eq!(from, relayed);
        peer.send_to(b"hello client", relayed).await.unwrap();
        let got = c.recv(Duration::from_secs(2), |_| None).await.unwrap();
        assert_eq!(got, Some((pa, b"hello client".to_vec())));

        c.channel_bind(0x4001, pa).await.unwrap().unwrap();
        c.send_channel(0x4001, b"via channel").await.unwrap();
        let (n, _) = peer.recv_from(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"via channel");
        peer.send_to(b"back on channel", relayed).await.unwrap();
        let got = c
            .recv(Duration::from_secs(2), |ch| (ch == 0x4001).then_some(pa))
            .await
            .unwrap();
        assert_eq!(got, Some((pa, b"back on channel".to_vec())));
        c.refresh(300).await.unwrap().unwrap();
        let s = relay.stats();
        assert_eq!(s.allocations, 1);
        assert!(s.bytes_to_peers >= 21 && s.bytes_to_clients >= 27);

        // Revocation drops the allocation and refuses the credentials.
        relay.revoke("sess1");
        assert_eq!(relay.stats().allocations, 0);
        assert_eq!(c.refresh(300).await.unwrap(), Err(401));
        // a closing client's late refresh is not counted as an attack
        let s = relay.stats();
        assert_eq!((s.revoked_rejects, s.auth_failures), (1, 0));
    }

    #[tokio::test]
    async fn not_an_open_relay() {
        let (relay, _peer, pa) = relay_with_peer(false).await;
        let (u, p) = relay.credentials("s", Duration::from_secs(60));
        let mut c = TurnClient::new(relay.udp_addr, &u).await.unwrap();
        c.allocate(&p).await.unwrap();
        let elsewhere: SocketAddr = "192.0.2.10:443".parse().unwrap();
        assert_eq!(c.create_permission(elsewhere).await.unwrap(), Err(403));
        // same IP, port outside the console range
        let other_port = SocketAddr::new(pa.ip(), pa.port().wrapping_add(1));
        assert_eq!(c.channel_bind(0x4002, other_port).await.unwrap(), Err(403));
        assert!(relay.stats().forbidden_peers >= 2);
    }

    #[tokio::test]
    async fn bad_expired_and_quota() {
        let (relay, _peer, _pa) = relay_with_peer(false).await;
        // wrong password
        let (u, _) = relay.credentials("a", Duration::from_secs(60));
        let mut c = TurnClient::new(relay.udp_addr, &u).await.unwrap();
        assert!(c.allocate("not-the-password").await.is_err());
        // a username for another secret
        let (u2, p2) = credentials(b"another-secret-0000000", "a", 4_000_000_000);
        let mut c = TurnClient::new(relay.udp_addr, &u2).await.unwrap();
        assert!(c.allocate(&p2).await.is_err());
        // expired: valid signature, expiry in the past
        let (u3, p3) = credentials(b"0123456789abcdef0123", "a", 1);
        let mut c = TurnClient::new(relay.udp_addr, &u3).await.unwrap();
        assert!(c.allocate(&p3).await.is_err());
        assert!(relay.stats().auth_failures >= 3);
        // quota (max_allocations = 2)
        let mut ok = Vec::new();
        for i in 0..2 {
            let (u, p) = relay.credentials(&format!("q{i}"), Duration::from_secs(60));
            let mut c = TurnClient::new(relay.udp_addr, &u).await.unwrap();
            c.allocate(&p).await.unwrap();
            ok.push(c);
        }
        let (u, p) = relay.credentials("q3", Duration::from_secs(60));
        let mut c = TurnClient::new(relay.udp_addr, &u).await.unwrap();
        let e = c.allocate(&p).await.unwrap_err().to_string();
        assert!(e.contains("486"), "{e}");
    }

    #[tokio::test]
    async fn binding_answers_reflexive_address_and_tcp_listens() {
        let (relay, _peer, _pa) = relay_with_peer(true).await;
        let c = TurnClient::new(relay.udp_addr, "x").await.unwrap();
        let req = stun::Builder::request(stun::method::BINDING).fingerprint();
        let r = c.transact(&req).await.unwrap();
        assert_eq!(
            r.get_xor_addr(stun::attr::XOR_MAPPED_ADDRESS)
                .map(|a| a.port()),
            Some(c.socket.local_addr().unwrap().port())
        );
        // TCP: allocate over a stream with framing
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut s = tokio::net::TcpStream::connect(relay.tcp_addr.unwrap())
            .await
            .unwrap();
        let req = stun::Builder::request(stun::method::ALLOCATE)
            .u32(stun::attr::REQUESTED_TRANSPORT, 17 << 24)
            .fingerprint();
        s.write_all(&req).await.unwrap();
        let mut buf = vec![0u8; 1500];
        let n = s.read(&mut buf).await.unwrap();
        let m = stun::Message::parse(&buf[..n]).unwrap();
        assert_eq!(m.class, stun::Class::Error);
        assert!(m.get(stun::attr::NONCE).is_some());
    }
}
