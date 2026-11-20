//! The console session: str0m as the offering, ICE-controlling peer (the
//! node is ICE-lite), direct UDP and/or the node's relay (TURN over UDP,
//! credentials from the ticket), the web client's data channels.
use crate::config::Settings;
use crate::net::{self, Api};
use horcrux_turn::client::TurnClient;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::mpsc as smpsc;
use std::time::{Duration, Instant};
use str0m::change::SdpAnswer;
use str0m::channel::{ChannelConfig, ChannelId, Reliability};
use str0m::media::{Direction, KeyframeRequestKind, MediaKind, Mid};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

/// Input and control towards the node.
#[derive(Debug, Clone)]
pub enum ToSession {
    Key(String, bool),
    /// 1: absolute (guest pixels), 2: relative (raw motion).
    Motion(u8, i32, i32),
    Button(u32, bool),
    Wheel(i32, i32),
    Pad([u8; 16]),
    Ctl(Value),
    Keyframe,
    Bye,
}

/// What the session hands to the window / headless runner.
#[derive(Debug)]
pub enum FromSession {
    /// An H.264 access unit and when its last packet arrived.
    Frame(Vec<u8>, Instant),
    Ctl(Value),
    Cursor,
    Connected,
    Closed(String),
}

pub struct Started {
    pub to: mpsc::UnboundedSender<ToSession>,
    pub from: smpsc::Receiver<FromSession>,
    pub session: String,
    pub path: &'static str,
}

/// The labels and delivery of the web client's channels.
const CHANNELS: [(&str, bool, bool); 7] = [
    ("ctl", true, true),
    ("keys", true, true),
    ("input", false, false),
    ("cursor", true, true),
    ("clip", true, true),
    ("pad", false, false),
    ("console", true, true),
];

fn local_ip_towards(peer: IpAddr) -> Option<IpAddr> {
    let s = std::net::UdpSocket::bind(if peer.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    })
    .ok()?;
    s.connect(SocketAddr::new(peer, 9)).ok()?;
    s.local_addr().ok().map(|a| a.ip())
}

/// 13-byte motion message of the `input` channel.
pub fn motion_bytes(kind: u8, seq: u32, x: i32, y: i32) -> Vec<u8> {
    let mut b = Vec::with_capacity(13);
    b.push(kind);
    b.extend_from_slice(&seq.to_le_bytes());
    b.extend_from_slice(&x.to_le_bytes());
    b.extend_from_slice(&y.to_le_bytes());
    b
}

pub async fn start(s: &Settings, api: &Api) -> Result<Started, String> {
    let vm = &s.vm;
    let st = api.get(&format!("/console/{vm}/hxc/status")).await?;
    if st["available"] == false {
        return Err(st["reason"]
            .as_str()
            .unwrap_or("console not available for this VM")
            .to_string());
    }
    let mut sig = api.signalling().await?;
    let mut body = json!({"conn_id": sig.conn_id});
    if !s.profile.is_empty() {
        body["profile"] = json!(s.profile);
    }
    let t = api.post(&format!("/console/{vm}/hxc/ticket"), body).await?;
    let ticket = t["ticket"]
        .as_str()
        .ok_or("ticket answer has no ticket")?
        .to_string();
    let relay_only = t["ice_transport_policy"] == "relay";

    let node_host = reqwest::Url::parse(&s.node)
        .ok()
        .and_then(|u| u.host_str().map(String::from))
        .ok_or("node URL has no host")?;
    let node_ip: IpAddr = tokio::net::lookup_host((node_host.as_str(), 0))
        .await
        .map_err(|e| format!("{node_host}: {e}"))?
        .next()
        .ok_or("node host does not resolve")?
        .ip();

    let mut rtc = Rtc::builder()
        .clear_codecs()
        .enable_h264(true)
        .set_stats_interval(Some(Duration::from_secs(1)))
        .build(Instant::now());
    // direct path: one UDP socket on the interface towards the node
    let lip = local_ip_towards(node_ip).ok_or("no route to the node")?;
    let sock = UdpSocket::bind(SocketAddr::new(lip, 0))
        .await
        .map_err(|e| e.to_string())?;
    let local = sock.local_addr().map_err(|e| e.to_string())?;
    if !relay_only {
        rtc.add_local_candidate(Candidate::host(local, "udp").map_err(|e| e.to_string())?);
    }
    // relay path: an allocation on the node's relay
    let mut turn: Option<(TurnClient, SocketAddr)> = None;
    if let Some((hp, user, cred)) = net::turn_server(&t["ice_servers"]) {
        let server = tokio::net::lookup_host(hp.as_str())
            .await
            .ok()
            .and_then(|mut a| a.next());
        if let Some(server) = server {
            match TurnClient::new(server, &user).await {
                Ok(mut c) => match c.allocate(&cred).await {
                    Ok(relayed) => {
                        rtc.add_local_candidate(
                            Candidate::relayed(relayed, local, "udp").map_err(|e| e.to_string())?,
                        );
                        turn = Some((c, relayed));
                    }
                    Err(e) => tracing::warn!("relay allocation failed: {e}"),
                },
                Err(e) => tracing::warn!("relay: {e}"),
            }
        }
    }
    if relay_only && turn.is_none() {
        return Err("the node asks for its relay, and the relay allocation failed".into());
    }

    let mut api_sdp = rtc.sdp_api();
    let video = api_sdp.add_media(MediaKind::Video, Direction::RecvOnly, None, None, None);
    let mut ids: HashMap<String, ChannelId> = HashMap::new();
    for (label, ordered, reliable) in CHANNELS {
        let id = api_sdp.add_channel_with_config(ChannelConfig {
            label: label.into(),
            ordered,
            reliability: if reliable {
                Reliability::Reliable
            } else {
                Reliability::MaxRetransmits { retransmits: 0 }
            },
            ..Default::default()
        });
        ids.insert(label.into(), id);
    }
    let (offer, pending) = api_sdp.apply().ok_or("nothing to offer")?;
    let (session, answer_sdp) = sig.offer(&ticket, &offer.to_sdp_string()).await?;
    let answer = SdpAnswer::from_sdp_string(&answer_sdp).map_err(|e| format!("bad answer: {e}"))?;
    rtc.sdp_api()
        .accept_answer(pending, answer)
        .map_err(|e| format!("answer: {e}"))?;

    // relay permissions and channels for the node's candidates
    let mut peers: HashMap<u16, SocketAddr> = HashMap::new();
    if let Some((c, _)) = turn.as_mut() {
        for (i, peer) in net::candidates(&answer_sdp).into_iter().enumerate() {
            let ch = 0x4000 + i as u16;
            if matches!(c.create_permission(peer).await, Ok(Ok(())))
                && matches!(c.channel_bind(ch, peer).await, Ok(Ok(())))
            {
                peers.insert(ch, peer);
            }
        }
    }
    let path = if relay_only {
        "relay"
    } else if turn.is_some() {
        "direct+relay"
    } else {
        "direct"
    };
    let (to_tx, to_rx) = mpsc::unbounded_channel();
    let (from_tx, from_rx) = smpsc::channel();
    let timeout = Duration::from_millis(s.connect_timeout_ms);
    tokio::spawn(async move {
        let reason = run(
            rtc,
            sock,
            turn,
            peers,
            video,
            ids,
            to_rx,
            from_tx.clone(),
            timeout,
        )
        .await;
        // keep the signalling socket until the session ends
        drop(sig);
        let _ = from_tx.send(FromSession::Closed(reason));
    });
    Ok(Started {
        to: to_tx,
        from: from_rx,
        session,
        path,
    })
}

#[allow(clippy::too_many_arguments)]
async fn run(
    mut rtc: Rtc,
    sock: UdpSocket,
    mut turn: Option<(TurnClient, SocketAddr)>,
    peers: HashMap<u16, SocketAddr>,
    video: Mid,
    ids: HashMap<String, ChannelId>,
    mut to: mpsc::UnboundedReceiver<ToSession>,
    from: smpsc::Sender<FromSession>,
    connect_timeout: Duration,
) -> String {
    let local = sock.local_addr().ok();
    let ch_of_peer: HashMap<SocketAddr, u16> = peers.iter().map(|(c, p)| (*p, *c)).collect();
    let mut open: HashMap<ChannelId, String> = HashMap::new();
    let mut seq: u32 = 0;
    let mut buf = vec![0u8; 65536];
    let mut tbuf = vec![0u8; 65536];
    let started = Instant::now();
    let mut connected = false;
    let mut refresh_at = Instant::now() + Duration::from_secs(240);
    // messages for a channel that is not open yet wait here (the hello with
    // the refresh rate is sent before the ctl channel is up)
    let mut pending: Vec<(String, bool, Vec<u8>)> = Vec::new();
    let send = |rtc: &mut Rtc,
                open: &HashMap<ChannelId, String>,
                pending: &mut Vec<(String, bool, Vec<u8>)>,
                label: &str,
                binary: bool,
                data: &[u8]| {
        if let Some((id, _)) = open.iter().find(|(_, l)| l.as_str() == label) {
            if let Some(mut c) = rtc.channel(*id) {
                let _ = c.write(binary, data);
                return;
            }
        }
        // input state is only worth its latest value; control messages all
        if label == "ctl" || pending.len() < 64 {
            pending.push((label.to_string(), binary, data.to_vec()));
        }
    };
    loop {
        let timeout = loop {
            match rtc.poll_output() {
                Ok(Output::Timeout(t)) => break t,
                Ok(Output::Transmit(t)) => {
                    let via_relay = turn.as_ref().map(|(_, r)| *r == t.source).unwrap_or(false);
                    if via_relay {
                        if let (Some((c, _)), Some(ch)) =
                            (turn.as_ref(), ch_of_peer.get(&t.destination))
                        {
                            let _ = c.send_channel(*ch, &t.contents).await;
                        } else if let Some((c, _)) = turn.as_ref() {
                            let _ = c.send_indication(t.destination, &t.contents).await;
                        }
                    } else {
                        let _ = sock.send_to(&t.contents, t.destination).await;
                    }
                }
                Ok(Output::Event(e)) => match e {
                    Event::Connected => {
                        connected = true;
                        let _ = from.send(FromSession::Connected);
                    }
                    Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                        return "connection lost".into();
                    }
                    Event::MediaData(d) if d.mid == video => {
                        if !d.contiguous {
                            if let Some(s) = rtc.direct_api().stream_rx_by_mid(video, None) {
                                s.request_keyframe(KeyframeRequestKind::Pli);
                            }
                        }
                        if from
                            .send(FromSession::Frame(d.data.to_vec(), Instant::now()))
                            .is_err()
                        {
                            return "closed".into();
                        }
                    }
                    Event::ChannelOpen(id, label) => {
                        let mut keep = Vec::new();
                        for (l, b, d) in pending.drain(..) {
                            if l == label {
                                if let Some(mut c) = rtc.channel(id) {
                                    let _ = c.write(b, &d);
                                }
                            } else {
                                keep.push((l, b, d));
                            }
                        }
                        pending = keep;
                        open.insert(id, label);
                    }
                    Event::ChannelData(d) => {
                        let label = open.get(&d.id).map(String::as_str).unwrap_or("");
                        if let Ok(v) = serde_json::from_slice::<Value>(&d.data) {
                            let m = match label {
                                "ctl" => FromSession::Ctl(v),
                                "cursor" => FromSession::Cursor,
                                _ => continue,
                            };
                            let _ = from.send(m);
                        }
                    }
                    _ => {}
                },
                Err(e) => return format!("webrtc: {e}"),
            }
        };
        if !rtc.is_alive() {
            return "connection closed".into();
        }
        if !connected && started.elapsed() > connect_timeout {
            return "no media connection (timeout)".into();
        }
        let wait = timeout
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(100));
        let turn_sock = turn.as_ref().map(|(c, _)| &c.socket);
        tokio::select! {
            r = sock.recv_from(&mut buf) => {
                if let (Ok((n, src)), Some(dst)) = (r, local) {
                    if let Ok(contents) = buf[..n].try_into() {
                        let _ = rtc.handle_input(Input::Receive(Instant::now(), Receive { proto: Protocol::Udp, source: src, destination: dst, contents }));
                    }
                }
            }
            r = async { match turn_sock { Some(s) => s.recv_from(&mut tbuf).await, None => std::future::pending().await } } => {
                if let (Ok((n, _)), Some((_, relayed))) = (r, turn.as_ref()) {
                    let b = &tbuf[..n];
                    let got = horcrux_turn::stun::parse_channel_data(b)
                        .and_then(|(ch, d)| peers.get(&ch).map(|p| (*p, d.to_vec())))
                        .or_else(|| {
                            let m = horcrux_turn::stun::Message::parse(b).ok()?;
                            (m.method == horcrux_turn::stun::method::DATA && m.class == horcrux_turn::stun::Class::Indication)
                                .then(|| Some((m.get_xor_addr(horcrux_turn::stun::attr::XOR_PEER_ADDRESS)?, m.get(horcrux_turn::stun::attr::DATA)?.to_vec())))?
                        });
                    if let Some((peer, data)) = got {
                        if let Ok(contents) = data.as_slice().try_into() {
                            let _ = rtc.handle_input(Input::Receive(Instant::now(), Receive { proto: Protocol::Udp, source: peer, destination: *relayed, contents }));
                        }
                    }
                }
            }
            m = to.recv() => {
                let Some(m) = m else { return "closed".into() };
                match m {
                    ToSession::Key(code, down) => send(&mut rtc, &open, &mut pending, "keys", false, json!({"t": if down {"down"} else {"up"}, "code": code}).to_string().as_bytes()),
                    ToSession::Button(b, down) => send(&mut rtc, &open, &mut pending, "keys", false, json!({"t": "btn", "b": b, "down": down}).to_string().as_bytes()),
                    ToSession::Wheel(dx, dy) => send(&mut rtc, &open, &mut pending, "keys", false, json!({"t": "wheel", "dx": dx, "dy": dy}).to_string().as_bytes()),
                    ToSession::Motion(k, x, y) => { seq = seq.wrapping_add(1); send(&mut rtc, &open, &mut pending, "input", true, &motion_bytes(k, seq, x, y)) }
                    ToSession::Pad(b) => send(&mut rtc, &open, &mut pending, "pad", true, &b),
                    ToSession::Ctl(v) => send(&mut rtc, &open, &mut pending, "ctl", false, v.to_string().as_bytes()),
                    ToSession::Keyframe => {
                        if let Some(s) = rtc.direct_api().stream_rx_by_mid(video, None) {
                            s.request_keyframe(KeyframeRequestKind::Pli);
                        }
                    }
                    ToSession::Bye => {
                        send(&mut rtc, &open, &mut pending, "ctl", false, br#"{"t":"bye"}"#);
                        // let the bye leave before closing
                        let end = Instant::now() + Duration::from_millis(200);
                        while Instant::now() < end {
                            match rtc.poll_output() {
                                Ok(Output::Transmit(t)) => { let _ = sock.send_to(&t.contents, t.destination).await; }
                                Ok(Output::Timeout(_)) | Err(_) => break,
                                _ => {}
                            }
                        }
                        rtc.disconnect();
                        if let Some((c, _)) = turn.as_mut() {
                            // release the relay allocation now
                            let _ = tokio::time::timeout(Duration::from_secs(2), c.refresh(0)).await;
                        }
                        return "closed by the user".into();
                    }
                }
            }
            _ = tokio::time::sleep(wait) => {}
        }
        if Instant::now() >= refresh_at {
            refresh_at = Instant::now() + Duration::from_secs(240);
            if let Some((c, _)) = turn.as_mut() {
                let _ = c.refresh(600).await;
            }
        }
        if let Err(e) = rtc.handle_input(Input::Timeout(Instant::now())) {
            return format!("webrtc: {e}");
        }
        let _ = ids.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_layout_matches_the_web_client() {
        let b = motion_bytes(2, 7, -3, 1000);
        assert_eq!(b.len(), 13);
        assert_eq!(b[0], 2);
        assert_eq!(u32::from_le_bytes(b[1..5].try_into().unwrap()), 7);
        assert_eq!(i32::from_le_bytes(b[5..9].try_into().unwrap()), -3);
        assert_eq!(i32::from_le_bytes(b[9..13].try_into().unwrap()), 1000);
    }
}
