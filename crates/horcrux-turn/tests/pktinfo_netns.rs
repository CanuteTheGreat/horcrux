//! The relay answers from the address a request reached, on a node whose
//! route back to the client leaves through a different address (the case a
//! multi-homed node or a Docker host hits). Needs root and `ip netns`; it
//! prints SKIP and passes where network namespaces cannot be created
//! (set HX_TURN_REQUIRE_NETNS=1 to make that a failure).
//!
//! Layout (all inside private network namespaces):
//!
//!   client ns: hxpk1 10.99.1.2/24, route 10.99.2.0/24 via 10.99.1.1
//!   node ns:   hxpk0 10.99.1.1/24 (the route back to the client), dummy
//!              hxpkd 10.99.2.1/24 (the address the client sends to), relay
//!              on 0.0.0.0:3478
//!
//! The kernel's source for an answer to 10.99.1.2 is 10.99.1.1; the client
//! sent to 10.99.2.1 and a NAT in front of it would drop the former.
#![cfg(target_os = "linux")]

use horcrux_turn::client::TurnClient;
use horcrux_turn::stun::{method, Builder, Class, Message};
use horcrux_turn::{credentials, PeerRule, Relay, RelayConfig};
use nix::sched::{setns, CloneFlags};
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CLI_NS: &str = "hxpk-cli";
const NODE_NS: &str = "hxpk-node";
const SECRET: &[u8] = b"0123456789abcdef0123456789";
const DST: &str = "10.99.2.1";
const ROUTE_BACK: &str = "10.99.1.1";

fn ip(args: &[&str]) -> bool {
    Command::new("ip")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

struct Net;
impl Net {
    fn setup() -> Option<Net> {
        teardown();
        let steps: &[&[&str]] = &[
            &["netns", "add", CLI_NS],
            &["netns", "add", NODE_NS],
            &[
                "link", "add", "hxpk0", "type", "veth", "peer", "name", "hxpk1",
            ],
            &["link", "set", "hxpk0", "netns", NODE_NS],
            &["link", "set", "hxpk1", "netns", CLI_NS],
            &["-n", NODE_NS, "addr", "add", "10.99.1.1/24", "dev", "hxpk0"],
            &["-n", NODE_NS, "link", "set", "hxpk0", "up"],
            &["-n", NODE_NS, "link", "add", "hxpkd", "type", "dummy"],
            &["-n", NODE_NS, "addr", "add", "10.99.2.1/24", "dev", "hxpkd"],
            &["-n", NODE_NS, "link", "set", "hxpkd", "up"],
            &["-n", NODE_NS, "link", "set", "lo", "up"],
            &["-n", CLI_NS, "addr", "add", "10.99.1.2/24", "dev", "hxpk1"],
            &["-n", CLI_NS, "link", "set", "hxpk1", "up"],
            &["-n", CLI_NS, "link", "set", "lo", "up"],
            &[
                "-n",
                CLI_NS,
                "route",
                "add",
                "10.99.2.0/24",
                "via",
                ROUTE_BACK,
            ],
        ];
        for s in steps {
            if !ip(s) {
                eprintln!("netns setup step failed: ip {}", s.join(" "));
                teardown();
                return None;
            }
        }
        Some(Net)
    }
}
impl Drop for Net {
    fn drop(&mut self) {
        teardown();
    }
}
fn teardown() {
    ip(&["netns", "del", CLI_NS]);
    ip(&["netns", "del", NODE_NS]);
}

fn enter(ns: &str) {
    let f = std::fs::File::open(format!("/var/run/netns/{ns}")).expect("open netns");
    setns(f, CloneFlags::CLONE_NEWNET).expect("setns");
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// Run the scenario with `reply_from_request_address = on`; returns the
/// source addresses of: the Binding answer, the Allocate challenge and the
/// Data indication relayed from a peer.
fn scenario(on: bool) -> Vec<IpAddr> {
    let (port_tx, port_rx) = mpsc::channel::<u16>();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let node = std::thread::spawn(move || {
        enter(NODE_NS);
        let peer = UdpSocket::bind((DST, 0)).expect("peer socket");
        let pp = peer.local_addr().unwrap().port();
        port_tx.send(pp).unwrap();
        // The peer: answers "ping" with "pong" to the relayed address.
        std::thread::spawn(move || {
            let mut b = [0u8; 64];
            if let Ok((n, from)) = peer.recv_from(&mut b) {
                if &b[..n] == b"ping" {
                    let _ = peer.send_to(b"pong", from);
                }
            }
        });
        rt().block_on(async move {
            let relay = Relay::start(RelayConfig {
                listen: "0.0.0.0:3478".parse().unwrap(),
                tcp: false,
                reply_from_request_address: on,
                secret: SECRET.to_vec(),
                relay_ip: DST.parse().unwrap(),
                port_min: 40000,
                port_max: 40099,
                allowed_peers: vec![PeerRule {
                    ip: DST.parse().unwrap(),
                    port_min: pp,
                    port_max: pp,
                }],
                ..Default::default()
            })
            .await
            .expect("relay start");
            ready_tx.send(()).unwrap();
            while stop_rx.try_recv().is_err() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            drop(relay);
        });
    });
    let pp = port_rx.recv_timeout(Duration::from_secs(10)).expect("peer");
    ready_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("relay");
    let server: SocketAddr = format!("{DST}:3478").parse().unwrap();
    let out = std::thread::spawn(move || {
        enter(CLI_NS);
        let mut seen = Vec::new();
        // 1) Binding (STUN) answer.
        let s = UdpSocket::bind("10.99.1.2:0").unwrap();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let req = Builder::new(method::BINDING, Class::Request, [7; 12]).build();
        s.send_to(&req, server).unwrap();
        let mut b = [0u8; 2048];
        let (n, from) = s.recv_from(&mut b).expect("binding answer");
        assert_eq!(Message::parse(&b[..n]).unwrap().class, Class::Success);
        assert_eq!(from.port(), 3478);
        seen.push(from.ip());
        rt().block_on(async {
            let exp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
                + 120;
            let (user, pass) = credentials(SECRET, "pktinfo", exp);
            let mut c = TurnClient::new(server, &user).await.unwrap();
            // 2) Allocate: the 401 challenge, read raw (the client library
            // does not look at an answer's source).
            let first = Builder::request(method::ALLOCATE)
                .u32(horcrux_turn::stun::attr::REQUESTED_TRANSPORT, 17 << 24)
                .fingerprint();
            c.socket.send_to(&first, server).await.unwrap();
            let mut b = vec![0u8; 2048];
            let (n, from) =
                tokio::time::timeout(Duration::from_secs(2), c.socket.recv_from(&mut b))
                    .await
                    .expect("allocate challenge")
                    .unwrap();
            assert_eq!(Message::parse(&b[..n]).unwrap().class, Class::Error);
            seen.push(from.ip());
            c.allocate(&pass).await.expect("allocate");
            let peer: SocketAddr = format!("{DST}:{pp}").parse().unwrap();
            c.create_permission(peer).await.unwrap().unwrap();
            // 4) Data relayed from the peer back to the client.
            c.send_indication(peer, b"ping").await.unwrap();
            let (n, from) =
                tokio::time::timeout(Duration::from_secs(3), c.socket.recv_from(&mut b))
                    .await
                    .expect("data indication")
                    .unwrap();
            let m = Message::parse(&b[..n]).unwrap();
            assert_eq!(m.get(horcrux_turn::stun::attr::DATA), Some(&b"pong"[..]));
            seen.push(from.ip());
        });
        seen
    })
    .join()
    .expect("client thread");
    stop_tx.send(()).unwrap();
    node.join().expect("node thread");
    out
}

#[test]
fn replies_leave_from_the_address_the_request_reached() {
    if nix::unistd::getuid().as_raw() != 0 {
        eprintln!("SKIP: needs root for network namespaces");
        assert!(std::env::var("HX_TURN_REQUIRE_NETNS").is_err());
        return;
    }
    let Some(_net) = Net::setup() else {
        eprintln!("SKIP: network namespaces unavailable");
        assert!(std::env::var("HX_TURN_REQUIRE_NETNS").is_err());
        return;
    };
    let want: IpAddr = DST.parse().unwrap();
    let back: IpAddr = ROUTE_BACK.parse().unwrap();

    // Without the fix the same relay answers from the route's address:
    // the test setup really reproduces the failure.
    let off = scenario(false);
    assert_eq!(off[0], back, "baseline: answers use the route's source");

    let on = scenario(true);
    assert_eq!(on, vec![want; 3], "every answer comes from {DST}");
    println!("ok - pktinfo: replies from {DST} (without it: {ROUTE_BACK})");
}
