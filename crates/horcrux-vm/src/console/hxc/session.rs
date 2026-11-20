//! One Horcrux Console viewer session: QEMU D-Bus display -> host encoder
//! -> WebRTC (str0m) to the client, and input back from the client's data
//! channels into the guest.
//!
//! Tasks per session:
//! * the **compositor** keeps the guest framebuffer (Scanout/Update from
//!   the D-Bus listener), paces frames at the profile's rate (damage-driven,
//!   plus an idle refresh) into the encoder and forwards cursor changes;
//! * the **transport loop** owns the `str0m::Rtc` (sans-IO), the UDP
//!   sockets (one per host candidate address) and writes encoded frames as
//!   RTP; data-channel messages are decoded here;
//! * the **input** task turns client input into D-Bus keyboard/mouse calls,
//!   only when the session holds `VM.Console.Input`.
//!
//! Wire format of the data channels: docs/design/horcrux-console.md
//! section 4 (`ctl` JSON, `keys` JSON, `input` binary, `cursor` JSON).
use super::abr::{self, Abr, AbrConfig};
use super::encode::{self, EncodeParams, EncodedFrame, Encoder, PixelFormat, VideoCodec};
use super::guest::{self, GuestLink};
use super::keymap;
use super::qemu_dbus::{DbusDisplay, DisplayEvent};
use base64::Engine as _;
use horcrux_common::console::{ConsoleProfile, ConsoleSettings};
use horcrux_guest_proto as gproto;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use str0m::bwe::{Bitrate, BweKind};
use str0m::change::SdpOffer;
use str0m::channel::ChannelId;
use str0m::format::Codec;
use str0m::media::{MediaKind, MediaTime, Mid, Pt};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, watch};

/// Everything a session needs to start.
pub struct SessionRequest {
    pub session_id: String,
    pub vm_id: String,
    pub user: String,
    pub profile: ConsoleProfile,
    pub allow_input: bool,
    pub settings: ConsoleSettings,
    pub qmp_socket: PathBuf,
    pub candidate_ips: Vec<IpAddr>,
    pub offer_sdp: String,
    pub encoder_caps: encode::EncoderCaps,
    /// Clipboard sync allowed (VM.Console.Clipboard).
    pub allow_clipboard: bool,
    /// Host end of the guest-tools port, when this VM run has one.
    pub guest_tools_socket: Option<PathBuf>,
    /// Host end of the VM's serial console (the `console` channel).
    pub serial_socket: Option<PathBuf>,
    /// QEMU guest agent socket (automatic guest tools install).
    pub qga_socket: Option<PathBuf>,
    /// The guest tools CD is attached to this VM run.
    pub guest_tools_cd: bool,
    /// The ticket's relay credentials tag (revoked when the session ends).
    pub relay_tag: Option<String>,
}

/// Live numbers of a session (GET .../sessions, ctl `stats`).
#[derive(Debug, Clone, Default, Serialize)]
pub struct SessionStats {
    pub connected: bool,
    pub codec: Option<VideoCodec>,
    pub encoder: Option<encode::EncoderBackend>,
    pub guest_width: u32,
    pub guest_height: u32,
    pub frames_sent: u64,
    pub bytes_sent: u64,
    /// Mean time from handing a frame to the encoder until it is on the
    /// wire (ms, last 60 frames).
    pub encode_ms_avg: f64,
    pub keys_in: u64,
    pub pointer_in: u64,
    pub input_denied: u64,
    /// Data-channel round trip measured by the client's pings (ms).
    pub client_rtt_ms: Option<f64>,
    /// Last client-reported input-to-picture latency (ms).
    pub client_input_to_frame_ms: Option<f64>,
    pub end_reason: Option<String>,
    /// `guest` (guest tools capture + encode, forwarded without
    /// transcoding) or `host` (QEMU display encoded on the node).
    pub video_source: String,
    /// Guest tools that answered: OS and agent version.
    pub guest_tools: Option<String>,
    /// Encoder the guest tools use (nvenc, amf, qsv, vaapi,
    /// mediafoundation-software, software).
    pub guest_encoder: Option<String>,
    /// Median guest capture-to-port time (ms) over the last 60 frames.
    pub guest_encode_ms: Option<f64>,
    pub clipboard_in: u64,
    pub clipboard_out: u64,
    pub audio_packets: u64,
    pub gamepad_in: u64,
    /// `direct` or `relay` (the node's TURN relay, WAN).
    pub path: Option<String>,
    /// Congestion-control estimate of the link (kbit/s).
    pub link_estimate_kbps: Option<u32>,
    /// What adaptation currently sends.
    pub target: Option<abr::Target>,
    /// Round trip and loss reported by the client's receiver reports.
    pub rtt_ms: Option<f64>,
    pub loss_pct: Option<f64>,
    pub nacks: u64,
    /// Refresh rate of the client's display (frames are paced to it).
    pub client_refresh_hz: Option<u32>,
    /// Node encoder starts (each one begins with a keyframe) and what the
    /// running one produces: [width, height, fps, kbit/s].
    pub encoder_starts: u64,
    pub encoded: Option<[u32; 4]>,
}

pub struct SessionHandle {
    pub id: String,
    pub vm_id: String,
    pub user: String,
    pub profile: ConsoleProfile,
    pub started: chrono::DateTime<chrono::Utc>,
    pub stats: Arc<Mutex<SessionStats>>,
    stop: watch::Sender<bool>,
    pub done: watch::Receiver<bool>,
}

impl SessionHandle {
    pub fn stop(&self) {
        let _ = self.stop.send(true);
    }
    pub fn finished(&self) -> bool {
        *self.done.borrow()
    }
}

/// What the transport loop is told by the other tasks.
enum ToRtc {
    Frame(EncodedFrame, Instant),
    /// An access unit encoded in the guest, forwarded as it is.
    GuestFrame(Vec<u8>, Instant),
    Ctl(String),
    Cursor(String),
    Clip(String),
    Console(Vec<u8>),
    Audio(Vec<u8>),
    /// The guest tools answered after the session started (just
    /// installed, or started at logon): start the guest streams.
    GuestLinked,
}

/// What the compositor is told.
enum ToCompositor {
    KeyframeRequest,
    /// Adaptation changed the bitrate, output size or frame rate.
    Rate(abr::Target),
}

/// Client input, decoded from the data channels.
#[derive(Debug, PartialEq)]
pub enum InputEvent {
    Key { code: String, down: bool },
    Button { button: u8, down: bool },
    Abs { x: u32, y: u32 },
    Rel { dx: i32, dy: i32 },
    Wheel { dx: i32, dy: i32 },
}

/// `keys` channel JSON: `{"t":"down"|"up","code":"KeyA"}`,
/// `{"t":"btn","b":0,"down":true}`, `{"t":"wheel","dx":0,"dy":-1}`.
pub fn parse_keys(text: &str) -> Option<InputEvent> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    match v.get("t")?.as_str()? {
        t @ ("down" | "up") => Some(InputEvent::Key {
            code: v.get("code")?.as_str()?.chars().take(32).collect(),
            down: t == "down",
        }),
        "btn" => Some(InputEvent::Button {
            button: v.get("b")?.as_u64()?.min(255) as u8,
            down: v.get("down")?.as_bool()?,
        }),
        "wheel" => Some(InputEvent::Wheel {
            dx: v.get("dx").and_then(|x| x.as_i64()).unwrap_or(0).signum() as i32,
            dy: v.get("dy").and_then(|x| x.as_i64()).unwrap_or(0).signum() as i32,
        }),
        _ => None,
    }
}

/// `input` channel binary: kind u8 (1 = absolute, 2 = relative), seq u32
/// LE, then x and y as i32 LE (guest framebuffer pixels for absolute).
pub fn parse_motion(b: &[u8]) -> Option<InputEvent> {
    if b.len() < 13 {
        return None;
    }
    let x = i32::from_le_bytes(b[5..9].try_into().ok()?);
    let y = i32::from_le_bytes(b[9..13].try_into().ok()?);
    match b[0] {
        1 => Some(InputEvent::Abs {
            x: x.max(0) as u32,
            y: y.max(0) as u32,
        }),
        2 => Some(InputEvent::Rel {
            dx: x.clamp(-4096, 4096),
            dy: y.clamp(-4096, 4096),
        }),
        _ => None,
    }
}

/// `pad` channel binary: index u8, buttons u32 LE (W3C standard order),
/// lx, ly, rx, ry as i16 LE, lt u8, rt u8 (16 bytes).
pub fn parse_pad(b: &[u8]) -> Option<gproto::Gamepad> {
    if b.len() < 16 {
        return None;
    }
    let i16_at = |o: usize| i16::from_le_bytes([b[o], b[o + 1]]);
    Some(gproto::Gamepad {
        index: b[0].min(3),
        buttons: u32::from_le_bytes(b[1..5].try_into().ok()?),
        lx: i16_at(5),
        ly: i16_at(7),
        rx: i16_at(9),
        ry: i16_at(11),
        lt: b[13],
        rt: b[14],
    })
}

/// `ctl` resize request -> the layout asked of the guest tools:
/// `{"t":"resize","width":W,"height":H,"refresh_hz":R}` or
/// `{"t":"resize","monitors":[{...}, ...]}` (sizes clamped to the node's
/// max_width/max_height).
pub fn parse_resize(v: &serde_json::Value, max_w: u32, max_h: u32) -> Option<gproto::Layout> {
    let one = |m: &serde_json::Value| -> Option<gproto::Monitor> {
        let w = m.get("width")?.as_u64()? as u32;
        let h = m.get("height")?.as_u64()? as u32;
        if w < 64 || h < 64 {
            return None;
        }
        Some(gproto::Monitor {
            x: m.get("x").and_then(|x| x.as_i64()).unwrap_or(0) as i32,
            y: m.get("y").and_then(|x| x.as_i64()).unwrap_or(0) as i32,
            width: w.min(max_w) & !1,
            height: h.min(max_h) & !1,
            refresh_hz: m
                .get("refresh_hz")
                .and_then(|x| x.as_u64())
                .unwrap_or(0)
                .min(500) as u32,
            primary: m.get("primary").and_then(|x| x.as_bool()).unwrap_or(false),
        })
    };
    let monitors: Vec<gproto::Monitor> = match v.get("monitors").and_then(|m| m.as_array()) {
        Some(list) => list.iter().take(16).filter_map(one).collect(),
        None => one(v).into_iter().collect(),
    };
    (!monitors.is_empty()).then_some(gproto::Layout { monitors })
}

/// What the guest tools are asked to encode for this profile.
pub fn start_audio_for(s: &ConsoleSettings) -> gproto::StartAudio {
    gproto::StartAudio {
        codec: "opus".into(),
        sample_rate: 48000,
        channels: 2,
        bitrate_kbps: s.audio_bitrate_kbps,
        frame_ms: 10,
    }
}

pub fn start_video_for(p: &horcrux_common::console::ProfileSettings) -> gproto::StartVideo {
    let fps = p.max_fps.max(1);
    gproto::StartVideo {
        codec: "h264".into(),
        width: 0,
        height: 0,
        fps,
        bitrate_kbps: p.bitrate_kbps,
        max_bitrate_kbps: p.max_bitrate_kbps,
        keyframe_interval_frames: fps * p.keyframe_interval_secs.max(1),
        low_delay: true,
        fixed_rate: p.fixed_rate,
        chroma: p.chroma.clone(),
        hdr: p.hdr,
        intra_refresh: p.intra_refresh,
        monitor: 0,
    }
}

/// Our video payload type: the first codec in the node's preference list
/// that the encoder can produce and the client offered. For H.264 the
/// browser's non-interleaved (packetization-mode 1) constrained-baseline
/// entry is preferred, since that is what the encoders produce.
fn choose_pt(
    rtc: &mut Rtc,
    mid: Mid,
    prefs: &[String],
    caps: &encode::EncoderCaps,
) -> Option<(Pt, VideoCodec)> {
    let writer = rtc.writer(mid)?;
    let params: Vec<_> = writer.payload_params().cloned().collect();
    let available = caps.codecs();
    for name in prefs {
        let Some(codec) = VideoCodec::parse(name) else {
            continue;
        };
        if !available.contains(&codec) {
            continue;
        }
        let want = match codec {
            VideoCodec::H264 => Codec::H264,
            VideoCodec::Vp8 => Codec::Vp8,
        };
        let mut best: Option<(i32, Pt)> = None;
        for p in &params {
            let spec = p.spec();
            if spec.codec != want {
                continue;
            }
            let mut score = 0;
            if codec == VideoCodec::H264 {
                if spec.format.packetization_mode == Some(1) {
                    score += 2;
                }
                if spec
                    .format
                    .profile_level_id
                    .map(|id| id >> 16 == 0x42)
                    .unwrap_or(false)
                {
                    score += 1;
                }
            }
            if best.map(|(s, _)| score > s).unwrap_or(true) {
                best = Some((score, p.pt()));
            }
        }
        if let Some((_, pt)) = best {
            return Some((pt, codec));
        }
    }
    None
}

/// Bind one UDP socket per candidate address, each on the first free port
/// of the configured range.
async fn bind_sockets(ips: &[IpAddr], min: u16, max: u16) -> Result<Vec<Arc<UdpSocket>>, String> {
    let mut out = Vec::new();
    for ip in ips {
        let mut bound = None;
        for port in min..=max {
            if let Ok(s) = UdpSocket::bind(SocketAddr::new(*ip, port)).await {
                bound = Some(s);
                break;
            }
        }
        match bound {
            Some(s) => out.push(Arc::new(s)),
            None => tracing::warn!("console: no free UDP port {min}-{max} on {ip}"),
        }
    }
    if out.is_empty() {
        return Err(format!(
            "no free UDP port in {min}-{max} on any console address (settings udp_port_min/max)"
        ));
    }
    Ok(out)
}

fn install_crypto() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        str0m::crypto::from_feature_flags().install_process_default();
    });
}

/// Start a session: returns its handle and the SDP answer for the client.
pub async fn start(req: SessionRequest) -> Result<(SessionHandle, String), String> {
    install_crypto();
    let offer = SdpOffer::from_sdp_string(&req.offer_sdp).map_err(|e| format!("bad offer: {e}"))?;
    let sockets = bind_sockets(
        &req.candidate_ips,
        req.settings.udp_port_min,
        req.settings.udp_port_max,
    )
    .await?;

    let caps = &req.encoder_caps;
    let mut cfg = Rtc::builder().set_ice_lite(true).clear_codecs();
    for name in &req.settings.codecs {
        match VideoCodec::parse(name) {
            Some(VideoCodec::H264) if caps.codecs().contains(&VideoCodec::H264) => {
                cfg = cfg.enable_h264(true)
            }
            Some(VideoCodec::Vp8) if caps.codecs().contains(&VideoCodec::Vp8) => {
                cfg = cfg.enable_vp8(true)
            }
            _ => {}
        }
    }
    if req.settings.audio {
        cfg = cfg.enable_opus(true, false);
    }
    let profile = req.settings.profile(req.profile);
    if req.settings.wan.adaptive {
        cfg = cfg.enable_bwe(Some(Bitrate::kbps(profile.bitrate_kbps as u64)));
    }
    cfg = cfg
        .set_send_buffer_video(req.settings.wan.nack_history_packets as usize)
        .set_stats_interval(Some(Duration::from_secs(1)));
    let mut rtc = cfg.build(Instant::now());
    for s in &sockets {
        let addr = s.local_addr().map_err(|e| e.to_string())?;
        let c = Candidate::host(addr, "udp").map_err(|e| format!("candidate {addr}: {e}"))?;
        rtc.add_local_candidate(c);
    }
    let answer = rtc
        .sdp_api()
        .accept_offer(offer)
        .map_err(|e| format!("offer not acceptable: {e}"))?;
    let answer_sdp = answer.to_sdp_string();

    // The connection to the guest's display is made before answering, so
    // a VM without the D-Bus display fails the request, not the session.
    let (disp_tx, disp_rx) = mpsc::channel::<DisplayEvent>(64);
    let display = DbusDisplay::connect(&req.qmp_socket, &req.session_id, disp_tx, false).await?;
    // Guest tools, when the VM has the port and the tools answer in time.
    let link = match &req.guest_tools_socket {
        Some(sock) if req.settings.guest_tools && sock.exists() => {
            let wait = Duration::from_millis(req.settings.guest_tools_wait_ms as u64);
            match guest::connect(sock, wait, req.settings.clipboard_max_bytes).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::debug!("console guest tools: {e}");
                    None
                }
            }
        }
        _ => None,
    };
    let install = if link.is_none() && req.guest_tools_cd && req.settings.guest_tools_auto_install {
        req.qga_socket.clone().and_then(|qga| {
            super::deliver::spawn_install(
                req.vm_id.clone(),
                qga,
                req.settings.guest_tools_install_timeout_secs.into(),
                req.settings.guest_tools_auto_install_retry_secs.into(),
            )
        })
    } else {
        None
    };

    let stats = Arc::new(Mutex::new(SessionStats {
        video_source: "host".into(),
        guest_tools: link.as_ref().map(|l| {
            format!(
                "{} {} (agent {})",
                l.hello.os, l.hello.os_version, l.hello.agent_version
            )
        }),
        ..Default::default()
    }));
    let (stop_tx, stop_rx) = watch::channel(false);
    let (done_tx, done_rx) = watch::channel(false);
    let handle = SessionHandle {
        id: req.session_id.clone(),
        vm_id: req.vm_id.clone(),
        user: req.user.clone(),
        profile: req.profile,
        started: chrono::Utc::now(),
        stats: stats.clone(),
        stop: stop_tx,
        done: done_rx,
    };

    let (to_rtc_tx, to_rtc_rx) = mpsc::channel::<ToRtc>(8);
    let (to_comp_tx, to_comp_rx) = mpsc::channel::<ToCompositor>(4);
    let (input_tx, input_rx) = mpsc::channel::<InputEvent>(256);

    let absolute = display.absolute;
    let linked = link.is_some();
    let (guest_tx, guest_rx, guest_caps, reader) = match link {
        Some(GuestLink {
            hello,
            tx,
            rx,
            reader,
        }) => (Some(tx), Some(rx), Some(hello.caps), Some(reader)),
        None => (None, None, None, None),
    };
    let relay_addrs = super::relay::relay_candidates(&req.offer_sdp);
    let ctx = Ctx {
        relay_addrs,
        req,
        stats: stats.clone(),
        guest_tx: Mutex::new(guest_tx),
        guest_caps: Mutex::new(guest_caps),
        guest_reader: Mutex::new(reader),
        guest_video: AtomicBool::new(false),
        guest_started: Mutex::new(Vec::new()),
    };
    let ctx = Arc::new(ctx);
    if let Some(rx) = guest_rx {
        tokio::spawn(guest_task(ctx.clone(), rx, to_rtc_tx.clone()));
    }
    if let Some(job) = install {
        let to_rtc = to_rtc_tx.clone();
        tokio::spawn(async move {
            let _ = to_rtc
                .send(ToRtc::Ctl(
                    serde_json::json!({"t": "guest_tools", "state": "installing"}).to_string(),
                ))
                .await;
            let (state, detail) = match job.await {
                Ok(Ok(m)) => ("installed", m),
                Ok(Err(e)) => ("install_failed", e),
                Err(e) => ("install_failed", e.to_string()),
            };
            let _ = to_rtc
                .send(ToRtc::Ctl(
                    serde_json::json!({"t": "guest_tools", "state": state, "detail": detail})
                        .to_string(),
                ))
                .await;
        });
    }
    // Without tools now, keep checking: they may have just been installed
    // (above) or start when a user logs on; the session switches to them.
    let retry = ctx.req.settings.guest_tools_retry_secs;
    if !linked && ctx.req.settings.guest_tools && retry > 0 {
        if let Some(sock) = ctx.req.guest_tools_socket.clone() {
            tokio::spawn(link_watch(
                ctx.clone(),
                sock,
                Duration::from_secs(retry as u64),
                to_rtc_tx.clone(),
                stop_rx.clone(),
            ));
        }
    }
    tokio::spawn(input_task(display, input_rx, stats.clone()));
    tokio::spawn(compositor(
        ctx.clone(),
        disp_rx,
        to_comp_rx,
        to_rtc_tx.clone(),
        stop_rx.clone(),
    ));
    let to_rtc_self = to_rtc_tx.clone();
    tokio::spawn(async move {
        let reason = transport(
            ctx.clone(),
            rtc,
            sockets,
            to_rtc_rx,
            to_comp_tx,
            input_tx,
            stop_rx,
            absolute,
            to_rtc_self,
        )
        .await;
        if let Ok(mut s) = ctx.stats.lock() {
            s.end_reason = Some(reason);
            s.connected = false;
        }
        if let Some(tag) = ctx.req.relay_tag.as_deref() {
            super::relay::revoke(tag);
        }
        let _ = done_tx.send(true);
    });
    Ok((handle, answer_sdp))
}

struct Ctx {
    req: SessionRequest,
    /// The client's relayed ICE candidates (media from them = WAN path).
    relay_addrs: Vec<SocketAddr>,
    stats: Arc<Mutex<SessionStats>>,
    /// To the guest tools, when they answered.
    guest_tx: Mutex<Option<mpsc::Sender<gproto::Msg>>>,
    guest_caps: Mutex<Option<gproto::Caps>>,
    /// Keeps the port connection while the session holds the tools.
    guest_reader: Mutex<Option<guest::ReaderGuard>>,
    /// The guest tools are capturing + encoding: the node's encoder rests.
    guest_video: AtomicBool,
    /// What the guest tools were asked to run, re-sent when they say
    /// Hello again (their end of the port was reopened).
    guest_started: Mutex<Vec<gproto::Msg>>,
}

impl Ctx {
    /// Start a guest stream and remember it for a re-Hello.
    fn start_in_guest(&self, m: gproto::Msg) -> bool {
        if let Ok(mut v) = self.guest_started.lock() {
            v.push(m.clone());
        }
        self.to_guest(m)
    }
    fn to_guest(&self, m: gproto::Msg) -> bool {
        let tx = self.guest_tx.lock().ok().and_then(|g| g.clone());
        match tx {
            Some(tx) => tx.try_send(m).is_ok(),
            None => false,
        }
    }
    fn guest_can(&self, f: impl Fn(&gproto::Caps) -> bool) -> bool {
        self.guest_caps
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(f))
            .unwrap_or(false)
    }
    fn has_guest(&self) -> bool {
        self.guest_caps.lock().map(|g| g.is_some()).unwrap_or(false)
    }
}

/// A session that started without the guest tools checks every `every`
/// whether they answer now and then hands the link to the session.
async fn link_watch(
    ctx: Arc<Ctx>,
    sock: PathBuf,
    every: Duration,
    to_rtc: mpsc::Sender<ToRtc>,
    mut stop: watch::Receiver<bool>,
) {
    let wait = Duration::from_millis(ctx.req.settings.guest_tools_wait_ms.max(500) as u64);
    loop {
        tokio::select! {
            _ = stop.changed() => return,
            _ = tokio::time::sleep(every) => {}
        }
        if to_rtc.is_closed() || !sock.exists() {
            return;
        }
        let link = match guest::connect(&sock, wait, ctx.req.settings.clipboard_max_bytes).await {
            Ok(Some(l)) => l,
            Ok(None) => continue,
            Err(e) => {
                tracing::debug!("console guest tools: {e}");
                continue;
            }
        };
        tracing::info!(
            "console session {}: guest tools answered ({} {}, agent {})",
            ctx.req.session_id,
            link.hello.os,
            link.hello.os_version,
            link.hello.agent_version
        );
        if let Ok(mut s) = ctx.stats.lock() {
            s.guest_tools = Some(format!(
                "{} {} (agent {})",
                link.hello.os, link.hello.os_version, link.hello.agent_version
            ));
        }
        let GuestLink {
            hello,
            tx,
            rx,
            reader,
        } = link;
        if let Ok(mut g) = ctx.guest_reader.lock() {
            *g = Some(reader);
        }
        if let Ok(mut g) = ctx.guest_caps.lock() {
            *g = Some(hello.caps);
        }
        if let Ok(mut g) = ctx.guest_tx.lock() {
            *g = Some(tx);
        }
        tokio::spawn(guest_task(ctx.clone(), rx, to_rtc.clone()));
        let _ = to_rtc.send(ToRtc::GuestLinked).await;
        return;
    }
}

/// Messages from the guest tools: encoded frames go straight to the
/// transport (no transcoding), the rest becomes data-channel messages.
async fn guest_task(
    ctx: Arc<Ctx>,
    mut rx: mpsc::Receiver<gproto::Msg>,
    to_rtc: mpsc::Sender<ToRtc>,
) {
    let mut enc: VecDeque<f64> = VecDeque::new();
    let linked_at = Instant::now();
    while let Some(m) = rx.recv().await {
        match m {
            gproto::Msg::Video(p) => {
                if !ctx.guest_video.load(Ordering::Relaxed) {
                    // first packet must be decodable on its own
                    if !p.keyframe {
                        let _ = ctx.to_guest(gproto::Msg::Keyframe);
                        continue;
                    }
                    ctx.guest_video.store(true, Ordering::Relaxed);
                    if let Ok(mut s) = ctx.stats.lock() {
                        s.video_source = "guest".into();
                        if p.width > 0 {
                            s.guest_width = p.width;
                            s.guest_height = p.height;
                        }
                    }
                    let msg = serde_json::json!({"t": "source", "video": "guest", "width": p.width, "height": p.height}).to_string();
                    let _ = to_rtc.send(ToRtc::Ctl(msg)).await;
                }
                enc.push_back(p.encode_us as f64 / 1000.0);
                if enc.len() > 60 {
                    enc.pop_front();
                }
                if let Ok(mut s) = ctx.stats.lock() {
                    let mut v: Vec<f64> = enc.iter().copied().collect();
                    v.sort_by(|a, b| a.total_cmp(b));
                    s.guest_encode_ms = v.get(v.len() / 2).copied();
                }
                // a full queue drops the frame rather than adding delay
                let _ = to_rtc.try_send(ToRtc::GuestFrame(p.data, Instant::now()));
            }
            gproto::Msg::VideoStatus(st) => {
                if let Ok(mut s) = ctx.stats.lock() {
                    if st.running {
                        s.guest_encoder = Some(st.encoder.clone());
                    }
                }
                if let Some(e) = &st.error {
                    tracing::info!("console guest tools video: {e}; using the node's capture");
                    ctx.guest_video.store(false, Ordering::Relaxed);
                    if let Ok(mut s) = ctx.stats.lock() {
                        s.video_source = "host".into();
                    }
                }
                let msg = serde_json::json!({"t": "guest_video", "status": st}).to_string();
                let _ = to_rtc.send(ToRtc::Ctl(msg)).await;
            }
            gproto::Msg::Clipboard(c) => {
                if ctx.req.allow_clipboard
                    && c.text.len() <= ctx.req.settings.clipboard_max_bytes as usize
                {
                    if let Ok(mut s) = ctx.stats.lock() {
                        s.clipboard_out += 1;
                    }
                    let msg = serde_json::json!({"t": "text", "text": c.text}).to_string();
                    let _ = to_rtc.send(ToRtc::Clip(msg)).await;
                }
            }
            gproto::Msg::Layout(l) => {
                let msg =
                    serde_json::json!({"t": "guest_layout", "monitors": l.monitors}).to_string();
                let _ = to_rtc.send(ToRtc::Ctl(msg)).await;
            }
            gproto::Msg::Audio(a) => {
                let _ = to_rtc.try_send(ToRtc::Audio(a.data));
            }
            // the duplicate Hello of the handshake (one sent when the tools
            // opened the port, one answering HostHello) is not a reopen
            gproto::Msg::Hello(_) if linked_at.elapsed() < Duration::from_secs(2) => {}
            gproto::Msg::Hello(_) => {
                let again: Vec<gproto::Msg> = ctx
                    .guest_started
                    .lock()
                    .map(|v| v.clone())
                    .unwrap_or_default();
                for m in again {
                    let _ = ctx.to_guest(m);
                }
            }
            gproto::Msg::Log(l) => tracing::debug!("console guest tools: {l}"),
            _ => {}
        }
    }
    // tools went away: back to the node's own capture
    ctx.guest_video.store(false, Ordering::Relaxed);
    if let Ok(mut s) = ctx.stats.lock() {
        s.video_source = "host".into();
    }
}

/// The VM's serial console for the `console` channel.
async fn serial_task(
    sock: PathBuf,
    mut input: mpsc::Receiver<Vec<u8>>,
    to_rtc: mpsc::Sender<ToRtc>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let s = match tokio::net::UnixStream::connect(&sock).await {
        Ok(s) => s,
        Err(e) => {
            let msg = serde_json::json!({"t": "error", "message": format!("serial console: {e} (another serial viewer may be open)")}).to_string();
            let _ = to_rtc.send(ToRtc::Ctl(msg)).await;
            return;
        }
    };
    let (mut rd, mut wr) = s.into_split();
    let w = tokio::spawn(async move {
        while let Some(b) = input.recv().await {
            if wr.write_all(&b).await.is_err() {
                break;
            }
        }
    });
    let mut buf = vec![0u8; 4096];
    loop {
        match rd.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if to_rtc
                    .send(ToRtc::Console(buf[..n].to_vec()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }
    }
    w.abort();
}

impl Ctx {
    fn profile(&self) -> &horcrux_common::console::ProfileSettings {
        self.req.settings.profile(self.req.profile)
    }
}

async fn input_task(
    display: DbusDisplay,
    mut rx: mpsc::Receiver<InputEvent>,
    stats: Arc<Mutex<SessionStats>>,
) {
    // Last pointer position the client asked for: when the guest's current
    // pointer is relative (PS/2 before a tablet driver runs, firmware), an
    // absolute move becomes the difference to it. QEMU switches between
    // the two at run time, so every move tries absolute first.
    let mut last: Option<(u32, u32)> = None;
    let mut mode: Option<bool> = None;
    while let Some(ev) = rx.recv().await {
        let r = match ev {
            InputEvent::Key { code, down } => match keymap::qnum(&code) {
                Some(q) if down => display.keyboard.press(q).await,
                Some(q) => display.keyboard.release(q).await,
                None => Ok(()),
            },
            InputEvent::Button { button, down } => match keymap::mouse_button(button) {
                Some(b) if down => display.mouse.press(b).await,
                Some(b) => display.mouse.release(b).await,
                None => Ok(()),
            },
            InputEvent::Abs { x, y } => {
                let prev = last.replace((x, y));
                match display.mouse.set_abs_position(x, y).await {
                    Ok(()) => {
                        if mode != Some(true) {
                            mode = Some(true);
                            tracing::debug!("console pointer: absolute");
                        }
                        Ok(())
                    }
                    Err(e) => {
                        if mode != Some(false) {
                            mode = Some(false);
                            tracing::debug!("console pointer: relative ({e})");
                        }
                        match prev {
                            Some((px, py)) => {
                                display
                                    .mouse
                                    .rel_motion(x as i32 - px as i32, y as i32 - py as i32)
                                    .await
                            }
                            None => Ok(()),
                        }
                    }
                }
            }
            InputEvent::Rel { dx, dy } => display.mouse.rel_motion(dx, dy).await,
            InputEvent::Wheel { dx, dy } => {
                let mut r = Ok(());
                for b in [
                    (dy < 0).then_some(keymap::WHEEL_UP),
                    (dy > 0).then_some(keymap::WHEEL_DOWN),
                    (dx < 0).then_some(keymap::WHEEL_LEFT),
                    (dx > 0).then_some(keymap::WHEEL_RIGHT),
                ]
                .into_iter()
                .flatten()
                {
                    r = display.mouse.press(b).await;
                    if r.is_ok() {
                        r = display.mouse.release(b).await;
                    }
                }
                r
            }
        };
        if let Err(e) = r {
            tracing::debug!("console input: {e}");
        }
    }
    drop(stats);
    // Dropping the display closes the D-Bus connections; QEMU unregisters
    // the listener.
    drop(display);
}

/// Guest framebuffer in the scanout's own pixel format, tightly packed.
struct Framebuffer {
    width: u32,
    height: u32,
    format: PixelFormat,
    pixels: Vec<u8>,
}

impl Framebuffer {
    fn scanout(width: u32, height: u32, stride: u32, format: PixelFormat, data: &[u8]) -> Self {
        let bpp = format.bytes_per_pixel();
        let row = width as usize * bpp;
        let mut pixels = vec![0u8; row * height as usize];
        for y in 0..height as usize {
            let src = y * stride as usize;
            if src + row <= data.len() {
                pixels[y * row..(y + 1) * row].copy_from_slice(&data[src..src + row]);
            }
        }
        Self {
            width,
            height,
            format,
            pixels,
        }
    }

    fn update(&mut self, x: i32, y: i32, w: i32, h: i32, stride: u32, data: &[u8]) {
        let bpp = self.format.bytes_per_pixel();
        let (x, y) = (x.max(0) as usize, y.max(0) as usize);
        let w = (w.max(0) as usize).min((self.width as usize).saturating_sub(x));
        let h = (h.max(0) as usize).min((self.height as usize).saturating_sub(y));
        let row = self.width as usize * bpp;
        for r in 0..h {
            let src = r * stride as usize;
            let dst = (y + r) * row + x * bpp;
            let n = w * bpp;
            if src + n <= data.len() && dst + n <= self.pixels.len() {
                self.pixels[dst..dst + n].copy_from_slice(&data[src..src + n]);
            }
        }
    }
}

#[derive(Serialize)]
struct CursorShape<'a> {
    t: &'a str,
    w: i32,
    h: i32,
    hx: i32,
    hy: i32,
    /// Little-endian ARGB32 (bytes B, G, R, A), base64.
    bgra: String,
}

async fn compositor(
    ctx: Arc<Ctx>,
    mut disp: mpsc::Receiver<DisplayEvent>,
    mut ctl: mpsc::Receiver<ToCompositor>,
    to_rtc: mpsc::Sender<ToRtc>,
    mut stop: watch::Receiver<bool>,
) {
    let settings = &ctx.req.settings;
    let profile = ctx.profile().clone();
    let mut fb: Option<Framebuffer> = None;
    let mut encoder: Option<Encoder> = None;
    let (enc_tx, mut enc_rx) = mpsc::channel::<EncodedFrame>(8);
    let mut submitted: VecDeque<(u64, Instant)> = VecDeque::new();
    let mut dirty = false;
    let mut last_sent = Instant::now() - Duration::from_secs(3600);
    let mut frame_gap = Duration::from_secs_f64(1.0 / profile.max_fps.max(1) as f64);
    // adaptation target (bitrate, output size, frame rate), when set
    let mut rate: Option<abr::Target> = None;
    // (kbit/s, output width, output height, fps) of the running encoder
    let mut running: Option<(u32, u32, u32, u32)> = None;
    let idle_gap = (profile.idle_refresh_fps > 0)
        .then(|| Duration::from_secs_f64(1.0 / profile.idle_refresh_fps as f64));
    // Wake often and send as soon as the guest drew something and the
    // frame-rate cap allows it, instead of waiting for the next frame slot
    // (up to one frame interval of extra latency).
    let mut tick = tokio::time::interval(frame_gap.min(Duration::from_millis(4)));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut codec: Option<(VideoCodec, encode::EncoderBackend)> = None;
    let mut last_keyframe_restart = Instant::now() - Duration::from_secs(60);

    loop {
        tokio::select! {
            _ = stop.changed() => break,
            ev = disp.recv() => {
                let Some(ev) = ev else { break };
                match ev {
                    DisplayEvent::Scanout { width, height, stride, format, data } => {
                        let Some(pf) = PixelFormat::from_pixman(format) else {
                            tracing::warn!("console: unsupported pixel format {format:#x}");
                            continue;
                        };
                        let resized = fb.as_ref().map(|f| (f.width, f.height, f.format) != (width, height, pf)).unwrap_or(true);
                        fb = Some(Framebuffer::scanout(width, height, stride, pf, &data));
                        if resized {
                            encoder = None;
                            submitted.clear();
                            if let Ok(mut s) = ctx.stats.lock() {
                                s.guest_width = width;
                                s.guest_height = height;
                            }
                            let msg = serde_json::json!({"t": "layout", "width": width, "height": height}).to_string();
                            let _ = to_rtc.send(ToRtc::Ctl(msg)).await;
                        }
                        dirty = true;
                    }
                    DisplayEvent::Update { x, y, width, height, stride, format, data } => {
                        if let Some(f) = fb.as_mut() {
                            if PixelFormat::from_pixman(format) == Some(f.format) {
                                f.update(x, y, width, height, stride, &data);
                                dirty = true;
                            }
                        }
                    }
                    DisplayEvent::Disable => {
                        let _ = to_rtc.send(ToRtc::Ctl(r#"{"t":"disabled"}"#.into())).await;
                    }
                    DisplayEvent::CursorDefine { width, height, hot_x, hot_y, data } => {
                        if width > 0 && height > 0 && width <= 256 && height <= 256 {
                            let msg = CursorShape {
                                t: "shape", w: width, h: height, hx: hot_x, hy: hot_y,
                                bgra: base64::engine::general_purpose::STANDARD.encode(&data),
                            };
                            if let Ok(s) = serde_json::to_string(&msg) {
                                let _ = to_rtc.send(ToRtc::Cursor(s)).await;
                            }
                        }
                    }
                    DisplayEvent::MouseSet { x, y, on } => {
                        let msg = serde_json::json!({"t": "pos", "x": x, "y": y, "on": on != 0}).to_string();
                        let _ = to_rtc.try_send(ToRtc::Cursor(msg));
                    }
                    _ => {}
                }
            }
            c = ctl.recv() => {
                match c {
                    Some(ToCompositor::KeyframeRequest) if ctx.guest_video.load(Ordering::Relaxed) => {
                        let _ = ctx.to_guest(gproto::Msg::Keyframe);
                    }
                    Some(ToCompositor::KeyframeRequest) => {
                        // FFmpeg's CLI can't be asked for an IDR mid-stream;
                        // a fresh encoder starts with one. Rate-limited.
                        if last_keyframe_restart.elapsed() > Duration::from_millis(500) {
                            last_keyframe_restart = Instant::now();
                            encoder = None;
                            submitted.clear();
                            dirty = true;
                        }
                    }
                    Some(ToCompositor::Rate(_)) => {
                        // wake-up only: the target is read from the session stats below,
                        // so a full channel never loses one
                    }
                    None => break,
                }
            }
            out = enc_rx.recv() => {
                let Some(frame) = out else { continue };
                let mut at = Instant::now();
                while let Some((seq, t)) = submitted.front().copied() {
                    if seq <= frame.seq { submitted.pop_front(); }
                    if seq == frame.seq { at = t; break; }
                    if seq > frame.seq { break; }
                }
                if to_rtc.send(ToRtc::Frame(frame, at)).await.is_err() { break; }
            }
            _ = tick.tick() => {
                if to_rtc.is_closed() { break; }
                if ctx.guest_video.load(Ordering::Relaxed) {
                    // the guest tools encode; the node's encoder rests
                    if encoder.is_some() {
                        encoder = None;
                        submitted.clear();
                    }
                    continue;
                }
                let Some(f) = fb.as_ref() else { continue };
                if profile.fixed_rate {
                    dirty = true;
                }
                let idle_due = idle_gap.map(|g| last_sent.elapsed() >= g).unwrap_or(false);
                if !dirty && !idle_due { continue; }
                if last_sent.elapsed() < frame_gap { continue; }
                if encoder.as_mut().map(|e| !e.alive()).unwrap_or(false) {
                    encoder = None;
                }
                if encoder.is_none() {
                    // pick codec/backend once the transport knows the PT
                    let chosen = ctx.stats.lock().ok().and_then(|s| s.codec.zip(s.encoder));
                    let Some((c, b)) = chosen.or(codec) else { continue };
                    codec = Some((c, b));
                    let mut params = EncodeParams::new(c, b, f.width, f.height, f.format, &profile, settings);
                    if let Some(t) = rate {
                        params.bitrate_kbps = t.kbps.max(100);
                        params.max_bitrate_kbps = t.kbps.max(100);
                        params.fps = t.fps.max(1);
                        let (w, h) = encode::fit(f.width, f.height, t.width.max(2), t.height.max(2));
                        params.out_width = params.out_width.min(w);
                        params.out_height = params.out_height.min(h);
                    }
                    running = Some((params.bitrate_kbps, params.out_width, params.out_height, params.fps));
                    if let Ok(mut s) = ctx.stats.lock() {
                        s.encoder_starts += 1;
                        s.encoded = Some([params.out_width, params.out_height, params.fps, params.bitrate_kbps]);
                    }
                    match Encoder::spawn(&settings.ffmpeg_path, params, enc_tx.clone()) {
                        Ok(e) => encoder = Some(e),
                        Err(e) => {
                            tracing::warn!("console: encoder did not start: {e}");
                            let msg = serde_json::json!({"t": "error", "message": format!("encoder did not start: {e}")}).to_string();
                            let _ = to_rtc.send(ToRtc::Ctl(msg)).await;
                            break;
                        }
                    }
                }
                if let Some(e) = encoder.as_mut() {
                    if let Some(seq) = e.submit(f.pixels.clone()) {
                        submitted.push_back((seq, Instant::now()));
                        if submitted.len() > 120 { submitted.pop_front(); }
                        dirty = false;
                        last_sent = Instant::now();
                    }
                }
            }
        }
        // Apply the newest link-adaptation target (set by the transport).
        let want = ctx.stats.lock().ok().and_then(|s| s.target);
        if let Some(t) = want.filter(|t| Some(*t) != rate) {
            frame_gap = Duration::from_secs_f64(1.0 / t.fps.max(1) as f64);
            rate = Some(t);
            if ctx.guest_video.load(Ordering::Relaxed) {
                // the guest encoder restarts with the new rate
                let mut sv = start_video_for(&profile);
                sv.bitrate_kbps = t.kbps;
                sv.max_bitrate_kbps = t.kbps;
                sv.fps = t.fps.max(1);
                sv.keyframe_interval_frames = sv.fps * profile.keyframe_interval_secs.max(1);
                let _ = ctx.to_guest(gproto::Msg::StartVideo(sv));
            } else if let (Some((kbps, ow, oh, fps)), Some(f)) = (running, fb.as_ref()) {
                // FFmpeg's CLI can't change rate or size in-stream and
                // a restart costs a keyframe: restart for a new size or
                // frame rate, or a bitrate change of -30% / +50%.
                let (w, h) = encode::fit(f.width, f.height, t.width.max(2), t.height.max(2));
                let (mw, mh) =
                    encode::fit(f.width, f.height, settings.max_width, settings.max_height);
                let shape = (w.min(mw), h.min(mh), t.fps.max(1)) != (ow, oh, fps);
                if shape || t.kbps * 10 < kbps * 7 || t.kbps * 10 > kbps * 15 {
                    encoder = None;
                    submitted.clear();
                    dirty = true;
                }
            }
        }
    }
}

/// The `ctl` hello: what this session offers the client. Sent again when
/// the guest tools link up mid-session.
fn hello_msg(ctx: &Ctx, audio: bool, absolute: bool) -> String {
    let (codec, backend, w, h) = ctx
        .stats
        .lock()
        .map(|s| (s.codec, s.encoder, s.guest_width, s.guest_height))
        .unwrap_or_default();
    serde_json::json!({
        "t": "hello", "v": 1,
        "session": ctx.req.session_id,
        "vm": ctx.req.vm_id,
        "profile": ctx.req.profile,
        "codec": codec, "encoder": backend,
        "width": w, "height": h,
        "input": ctx.req.allow_input,
        "absolute": absolute,
        // C2 (guest tools) adds clipboard/resize/audio.
        "clipboard": ctx.req.allow_clipboard && ctx.guest_can(|c| c.clipboard),
        "audio": audio && ctx.guest_can(|c| c.audio),
        "gamepad": ctx.req.allow_input && ctx.guest_can(|c| c.gamepad),
        "resize": ctx.req.allow_input && ctx.guest_can(|c| c.resize),
        "relative": ctx.guest_can(|c| c.relative_mouse),
        "guest_tools": ctx.has_guest(),
        "console": ctx.req.serial_socket.is_some(),
    })
    .to_string()
}

/// Re-plan with the last link estimate after a cap changed (path, display
/// rate): estimates only arrive when the link changes.
fn replan(
    abr: &mut Abr,
    ctx: &Ctx,
    to_comp: &mpsc::Sender<ToCompositor>,
    pending_ctl: &mut Vec<String>,
    link_msg: &dyn Fn(&SessionStats) -> String,
) {
    if abr.last_estimate_kbps == 0 {
        return;
    }
    if let Ok(mut s) = ctx.stats.lock() {
        abr.set_source(s.guest_width, s.guest_height);
        if let Some(t) = abr.on_estimate(abr.last_estimate_kbps, Instant::now()) {
            s.target = Some(t);
            let _ = to_comp.try_send(ToCompositor::Rate(t));
            pending_ctl.push(link_msg(&s));
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn transport(
    ctx: Arc<Ctx>,
    mut rtc: Rtc,
    sockets: Vec<Arc<UdpSocket>>,
    mut from_tasks: mpsc::Receiver<ToRtc>,
    to_comp: mpsc::Sender<ToCompositor>,
    input: mpsc::Sender<InputEvent>,
    mut stop: watch::Receiver<bool>,
    absolute: bool,
    to_rtc_self: mpsc::Sender<ToRtc>,
) -> String {
    let (net_tx, mut net_rx) = mpsc::channel::<(Vec<u8>, SocketAddr, SocketAddr)>(256);
    let mut readers = Vec::new();
    for s in &sockets {
        let s = s.clone();
        let tx = net_tx.clone();
        readers.push(tokio::spawn(async move {
            let local = match s.local_addr() {
                Ok(a) => a,
                Err(_) => return,
            };
            let mut buf = vec![0u8; 2048];
            while let Ok((n, src)) = s.recv_from(&mut buf).await {
                if tx.send((buf[..n].to_vec(), src, local)).await.is_err() {
                    break;
                }
            }
        }));
    }
    let by_addr: HashMap<SocketAddr, Arc<UdpSocket>> = sockets
        .iter()
        .filter_map(|s| s.local_addr().ok().map(|a| (a, s.clone())))
        .collect();
    let settings = &ctx.req.settings;
    let idle_timeout = Duration::from_secs(settings.session_idle_timeout_secs as u64);
    let mut last_rx = Instant::now();
    let mut video: Option<(Mid, Pt, VideoCodec)> = None;
    let mut audio: Option<(Mid, Pt)> = None;
    let mut pending_out: VecDeque<(&'static str, Vec<u8>, bool)> = VecDeque::new();
    let mut serial_tx: Option<mpsc::Sender<Vec<u8>>> = None;
    let allow_clipboard = ctx.req.allow_clipboard;
    let mut channels: HashMap<String, ChannelId> = HashMap::new();
    let mut labels: HashMap<ChannelId, String> = HashMap::new();
    let mut pending_ctl: Vec<String> = Vec::new();
    let mut pending_cursor: Option<String> = None;
    let start = Instant::now();
    let mut enc_lat: VecDeque<f64> = VecDeque::new();
    let mut stats_tick = tokio::time::interval(Duration::from_secs(2));
    let allow_input = ctx.req.allow_input;
    let mut denied_reported = false;
    let wan = &settings.wan;
    let tprofile = ctx.profile().clone();
    let mut abr = Abr::new(AbrConfig::new(
        wan,
        tprofile.max_bitrate_kbps,
        tprofile.max_fps,
        ctx.req.profile == ConsoleProfile::Gaming,
    ));
    let mut path: Option<&'static str> = None;
    let link_msg = |stats: &SessionStats| {
        serde_json::json!({"t": "link", "path": stats.path, "estimate_kbps": stats.link_estimate_kbps,
            "target": stats.target, "uplink_kbps": wan.uplink_kbps, "rtt_ms": stats.rtt_ms,
            "loss_pct": stats.loss_pct, "adaptive": wan.adaptive})
        .to_string()
    };

    let reason = loop {
        // Drain str0m's output until it asks for a timeout.
        let timeout = loop {
            match rtc.poll_output() {
                Ok(Output::Timeout(t)) => break t,
                Ok(Output::Transmit(t)) => {
                    let sock = by_addr.get(&t.source).or_else(|| sockets.first());
                    if let Some(s) = sock {
                        let _ = s.try_send_to(&t.contents, t.destination);
                    }
                }
                Ok(Output::Event(e)) => match e {
                    Event::Connected => {
                        if let Ok(mut s) = ctx.stats.lock() {
                            s.connected = true;
                        }
                        if wan.adaptive {
                            rtc.bwe().set_bitrate(
                                Bitrate::kbps(wan.min_kbps as u64),
                                Bitrate::kbps(tprofile.max_bitrate_kbps as u64),
                            );
                        }
                    }
                    Event::EgressBitrateEstimate(BweKind::Twcc { estimate, .. }) => {
                        let kbps = (estimate.as_u64() / 1000).min(u32::MAX as u64) as u32;
                        if let Ok(mut s) = ctx.stats.lock() {
                            s.link_estimate_kbps = Some(kbps);
                            abr.set_source(s.guest_width, s.guest_height);
                            if let Some(t) = abr.on_estimate(kbps, Instant::now()) {
                                s.target = Some(t);
                                let _ = to_comp.try_send(ToCompositor::Rate(t));
                                pending_ctl.push(link_msg(&s));
                            }
                        }
                    }
                    Event::MediaEgressStats(m) if video.map(|v| v.0 == m.mid).unwrap_or(false) => {
                        if let Ok(mut s) = ctx.stats.lock() {
                            s.rtt_ms = m.rtt.map(|d| d.as_secs_f64() * 1000.0).or(s.rtt_ms);
                            s.loss_pct = m.loss.map(|l| l as f64 * 100.0).or(s.loss_pct);
                            s.nacks = m.nacks;
                        }
                    }
                    Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                        rtc.disconnect();
                    }
                    Event::MediaAdded(m) if m.kind == MediaKind::Video && video.is_none() => {
                        let caps = &ctx.req.encoder_caps;
                        if let Some((pt, codec)) =
                            choose_pt(&mut rtc, m.mid, &settings.codecs, caps)
                        {
                            let backend = match codec {
                                VideoCodec::H264 => caps
                                    .h264_backend(settings.encoder)
                                    .unwrap_or(encode::EncoderBackend::Software),
                                VideoCodec::Vp8 => encode::EncoderBackend::Software,
                            };
                            video = Some((m.mid, pt, codec));
                            if let Ok(mut s) = ctx.stats.lock() {
                                s.codec = Some(codec);
                                s.encoder = Some(backend);
                            }
                            // guest tools encode H.264 in the guest
                            if codec == VideoCodec::H264
                                && ctx.profile().guest_capture
                                && ctx.guest_can(|c| {
                                    c.codecs.iter().any(|x| x == "h264") && !c.capture.is_empty()
                                })
                            {
                                let sv = start_video_for(ctx.profile());
                                let _ = ctx.start_in_guest(gproto::Msg::StartVideo(sv));
                            }
                        }
                    }
                    Event::MediaAdded(m) if m.kind == MediaKind::Audio && audio.is_none() => {
                        let opus = rtc.writer(m.mid).and_then(|w| {
                            w.payload_params()
                                .find(|p| p.spec().codec == Codec::Opus)
                                .map(|p| p.pt())
                        });
                        if let Some(pt) = opus {
                            audio = Some((m.mid, pt));
                            if settings.audio && ctx.guest_can(|c| c.audio) {
                                let _ = ctx.start_in_guest(gproto::Msg::StartAudio(
                                    start_audio_for(settings),
                                ));
                            }
                        }
                    }
                    Event::KeyframeRequest(_) => {
                        let _ = to_comp.try_send(ToCompositor::KeyframeRequest);
                    }
                    Event::ChannelOpen(id, label) => {
                        channels.insert(label.clone(), id);
                        labels.insert(id, label.clone());
                        if label == "ctl" {
                            let hello = hello_msg(&ctx, audio.is_some(), absolute);
                            pending_ctl.insert(0, hello);
                        }
                        if label == "console" && serial_tx.is_none() {
                            if let Some(sock) = ctx.req.serial_socket.clone() {
                                let (tx, rx) = mpsc::channel::<Vec<u8>>(64);
                                serial_tx = Some(tx);
                                tokio::spawn(serial_task(sock, rx, to_rtc_self.clone()));
                            }
                        }
                        // A new viewer needs a keyframe.
                        let _ = to_comp.try_send(ToCompositor::KeyframeRequest);
                    }
                    Event::ChannelData(d) => {
                        let label = labels.get(&d.id).map(String::as_str).unwrap_or("");
                        let ev = match label {
                            "keys" => std::str::from_utf8(&d.data).ok().and_then(parse_keys),
                            "input" => parse_motion(&d.data),
                            "clip" => {
                                let text = serde_json::from_slice::<serde_json::Value>(&d.data)
                                    .ok()
                                    .and_then(|v| {
                                        v.get("text").and_then(|t| t.as_str()).map(str::to_string)
                                    });
                                match text {
                                    Some(t)
                                        if allow_clipboard
                                            && t.len() <= settings.clipboard_max_bytes as usize =>
                                    {
                                        if ctx.to_guest(gproto::Msg::Clipboard(gproto::Clipboard {
                                            mime: "text/plain".into(),
                                            text: t,
                                        })) {
                                            if let Ok(mut s) = ctx.stats.lock() {
                                                s.clipboard_in += 1;
                                            }
                                        }
                                    }
                                    Some(_) if !allow_clipboard => {
                                        pending_ctl.push(r#"{"t":"denied","what":"clipboard","privilege":"VM.Console.Clipboard"}"#.into());
                                    }
                                    _ => {}
                                }
                                None
                            }
                            "pad" => {
                                if let Some(g) = parse_pad(&d.data) {
                                    if allow_input {
                                        if ctx
                                            .to_guest(gproto::Msg::Input(gproto::Input::Gamepad(g)))
                                        {
                                            if let Ok(mut s) = ctx.stats.lock() {
                                                s.gamepad_in += 1;
                                            }
                                        }
                                    } else if let Ok(mut s) = ctx.stats.lock() {
                                        s.input_denied += 1;
                                    }
                                }
                                None
                            }
                            "console" => {
                                if allow_input {
                                    if let Some(tx) = &serial_tx {
                                        let _ = tx.try_send(d.data.to_vec());
                                    }
                                } else if let Ok(mut s) = ctx.stats.lock() {
                                    s.input_denied += 1;
                                }
                                None
                            }
                            "ctl" => {
                                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&d.data)
                                {
                                    match v.get("t").and_then(|t| t.as_str()) {
                                        // the page is going away: end now instead of
                                        // waiting for ICE consent to time out (which also
                                        // holds the session's relay allocations)
                                        Some("bye") => rtc.disconnect(),
                                        Some("ping") => {
                                            let pong =
                                                serde_json::json!({"t": "pong", "id": v.get("id")});
                                            pending_ctl.push(pong.to_string());
                                        }
                                        Some("report") => {
                                            if let Ok(mut s) = ctx.stats.lock() {
                                                s.client_rtt_ms = v
                                                    .get("rtt_ms")
                                                    .and_then(|x| x.as_f64())
                                                    .or(s.client_rtt_ms);
                                                s.client_input_to_frame_ms = v
                                                    .get("input_to_frame_ms")
                                                    .and_then(|x| x.as_f64())
                                                    .or(s.client_input_to_frame_ms);
                                            }
                                        }
                                        Some("keyframe") => {
                                            let _ = to_comp.try_send(ToCompositor::KeyframeRequest);
                                        }
                                        Some("display") => {
                                            // pace frames to the client's display
                                            if let Some(hz) =
                                                v.get("refresh_hz").and_then(|x| x.as_u64())
                                            {
                                                let hz = (hz as u32).clamp(1, 500);
                                                if let Ok(mut s) = ctx.stats.lock() {
                                                    s.client_refresh_hz = Some(hz);
                                                }
                                                abr.set_max_fps(hz.min(tprofile.max_fps));
                                                replan(
                                                    &mut abr,
                                                    &ctx,
                                                    &to_comp,
                                                    &mut pending_ctl,
                                                    &link_msg,
                                                );
                                            }
                                        }
                                        Some("resize") if allow_input => {
                                            if let Some(l) = parse_resize(
                                                &v,
                                                settings.max_width,
                                                settings.max_height,
                                            ) {
                                                if !ctx.to_guest(gproto::Msg::SetLayout(l)) {
                                                    pending_ctl.push(r#"{"t":"resize_unavailable","reason":"no Horcrux guest tools in this VM"}"#.into());
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                None
                            }
                            _ => None,
                        };
                        if let Some(ev) = ev {
                            if allow_input {
                                if let Ok(mut s) = ctx.stats.lock() {
                                    match ev {
                                        InputEvent::Key { .. } | InputEvent::Button { .. } => {
                                            s.keys_in += 1
                                        }
                                        _ => s.pointer_in += 1,
                                    }
                                }
                                // raw relative motion through the guest tools
                                // (no pointer acceleration in Linux uinput)
                                let via_guest = matches!(ev, InputEvent::Rel { .. })
                                    && ctx.guest_can(|c| c.relative_mouse);
                                if let (true, InputEvent::Rel { dx, dy }) = (via_guest, &ev) {
                                    let _ = ctx.to_guest(gproto::Msg::Input(gproto::Input::Rel {
                                        dx: *dx,
                                        dy: *dy,
                                    }));
                                } else {
                                    let _ = input.try_send(ev);
                                }
                            } else {
                                if let Ok(mut s) = ctx.stats.lock() {
                                    s.input_denied += 1;
                                }
                                if !denied_reported {
                                    denied_reported = true;
                                    pending_ctl.push(r#"{"t":"denied","what":"input","privilege":"VM.Console.Input"}"#.into());
                                }
                            }
                        }
                    }
                    _ => {}
                },
                Err(e) => {
                    tracing::debug!("console rtc: {e}");
                    rtc.disconnect();
                    break Instant::now();
                }
            }
        };
        if !rtc.is_alive() {
            break "connection closed".to_string();
        }
        // Queued data-channel messages (one mutation per drain).
        if !pending_ctl.is_empty() {
            if let Some(id) = channels.get("ctl").copied() {
                let msg = pending_ctl.remove(0);
                if let Some(mut ch) = rtc.channel(id) {
                    let _ = ch.write(false, msg.as_bytes());
                }
                continue;
            }
        }
        if let Some((label, data, binary)) = pending_out.pop_front() {
            if let Some(id) = channels.get(label).copied() {
                if let Some(mut ch) = rtc.channel(id) {
                    let _ = ch.write(binary, &data);
                }
            }
            continue;
        }
        if let Some(msg) = pending_cursor.take() {
            if let Some(id) = channels.get("cursor").copied() {
                if let Some(mut ch) = rtc.channel(id) {
                    let _ = ch.write(false, msg.as_bytes());
                }
                continue;
            }
            pending_cursor = Some(msg);
        }
        if last_rx.elapsed() > idle_timeout {
            break "client went silent (session_idle_timeout_secs)".to_string();
        }
        let wait = timeout
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(250));
        tokio::select! {
            _ = stop.changed() => break "stopped".to_string(),
            _ = tokio::time::sleep(wait) => {
                let _ = rtc.handle_input(Input::Timeout(Instant::now()));
            }
            pkt = net_rx.recv() => {
                let Some((data, source, destination)) = pkt else { break "sockets closed".into() };
                last_rx = Instant::now();
                let p = if ctx.relay_addrs.contains(&source) { "relay" } else { "direct" };
                if path != Some(p) {
                    path = Some(p);
                    abr.set_uplink((p == "relay" && wan.uplink_kbps > 0).then_some(wan.uplink_kbps));
                    if let Ok(mut s) = ctx.stats.lock() {
                        s.path = Some(p.to_string());
                        pending_ctl.push(link_msg(&s));
                    }
                    replan(&mut abr, &ctx, &to_comp, &mut pending_ctl, &link_msg);
                }
                if let Ok(contents) = data.as_slice().try_into() {
                    let input = Input::Receive(Instant::now(), Receive { proto: Protocol::Udp, source, destination, contents });
                    if rtc.accepts(&input) {
                        if let Err(e) = rtc.handle_input(input) {
                            tracing::debug!("console rtc input: {e}");
                        }
                    }
                }
            }
            _ = stats_tick.tick() => {
                if !ctx.req.qmp_socket.exists() {
                    break "the VM stopped".to_string();
                }
                if let Ok(s) = ctx.stats.lock() {
                    let msg = serde_json::json!({"t": "stats", "frames": s.frames_sent, "bytes": s.bytes_sent,
                        "encode_ms": s.encode_ms_avg, "codec": s.codec, "encoder": s.encoder,
                        "width": s.guest_width, "height": s.guest_height, "path": s.path,
                        "estimate_kbps": s.link_estimate_kbps, "target": s.target,
                        "rtt_ms": s.rtt_ms, "loss_pct": s.loss_pct, "nacks": s.nacks,
                        "encoded": s.encoded, "encoder_starts": s.encoder_starts});
                    pending_ctl.push(msg.to_string());
                }
                let _ = rtc.handle_input(Input::Timeout(Instant::now()));
            }
            m = from_tasks.recv() => {
                match m {
                    Some(ToRtc::GuestFrame(data, at)) => {
                        if let Some((mid, pt, VideoCodec::H264)) = video {
                            let now = Instant::now();
                            let rtp = MediaTime::new(
                                (now - start).as_micros() as u64 * 90 / 1000,
                                str0m::media::Frequency::NINETY_KHZ,
                            );
                            let len = data.len();
                            if let Some(w) = rtc.writer(mid) {
                                if let Err(e) = w.write(pt, now, rtp, data) {
                                    tracing::debug!("console video write: {e}");
                                }
                            }
                            let _ = at;
                            if let Ok(mut s) = ctx.stats.lock() {
                                s.frames_sent += 1;
                                s.bytes_sent += len as u64;
                            }
                        }
                    }
                    Some(ToRtc::Audio(data)) => {
                        if let Some((mid, pt)) = audio {
                            let now = Instant::now();
                            let t = MediaTime::new(
                                (now - start).as_micros() as u64 * 48 / 1000,
                                str0m::media::Frequency::FORTY_EIGHT_KHZ,
                            );
                            if let Some(w) = rtc.writer(mid) {
                                let _ = w.write(pt, now, t, data);
                            }
                            if let Ok(mut s) = ctx.stats.lock() {
                                s.audio_packets += 1;
                            }
                        }
                    }
                    Some(ToRtc::Clip(m)) => pending_out.push_back(("clip", m.into_bytes(), false)),
                    Some(ToRtc::Console(b)) => {
                        if pending_out.len() < 256 {
                            pending_out.push_back(("console", b, true));
                        }
                    }
                    Some(ToRtc::Frame(_, _)) if ctx.guest_video.load(Ordering::Relaxed) => {}
                    Some(ToRtc::Frame(frame, at)) => {
                        if let Some((mid, pt, _)) = video {
                            let now = Instant::now();
                            let rtp = MediaTime::new(
                                (now - start).as_micros() as u64 * 90 / 1000,
                                str0m::media::Frequency::NINETY_KHZ,
                            );
                            let len = frame.data.len();
                            if let Some(w) = rtc.writer(mid) {
                                if let Err(e) = w.write(pt, now, rtp, frame.data) {
                                    tracing::debug!("console video write: {e}");
                                }
                            }
                            enc_lat.push_back((now - at).as_secs_f64() * 1000.0);
                            if enc_lat.len() > 60 { enc_lat.pop_front(); }
                            if let Ok(mut s) = ctx.stats.lock() {
                                s.frames_sent += 1;
                                s.bytes_sent += len as u64;
                                // median: encoder (re)starts take ~1 s and
                                // would dominate a mean
                                let mut v: Vec<f64> = enc_lat.iter().copied().collect();
                                v.sort_by(|a, b| a.total_cmp(b));
                                s.encode_ms_avg = v[v.len() / 2];
                            }
                        }
                    }
                    Some(ToRtc::Ctl(m)) => pending_ctl.push(m),
                    Some(ToRtc::GuestLinked) => {
                        if let Some((_, _, VideoCodec::H264)) = video {
                            if ctx.profile().guest_capture
                                && ctx.guest_can(|c| {
                                    c.codecs.iter().any(|x| x == "h264") && !c.capture.is_empty()
                                })
                            {
                                let sv = start_video_for(ctx.profile());
                                let _ = ctx.start_in_guest(gproto::Msg::StartVideo(sv));
                            }
                        }
                        if audio.is_some() && settings.audio && ctx.guest_can(|c| c.audio) {
                            let _ = ctx.start_in_guest(gproto::Msg::StartAudio(start_audio_for(settings)));
                        }
                        pending_ctl.push(hello_msg(&ctx, audio.is_some(), absolute));
                    }
                    Some(ToRtc::Cursor(m)) => {
                        // Only the newest cursor state matters, except that
                        // a pending shape is never replaced by a position.
                        let keep_shape = pending_cursor
                            .as_deref()
                            .map(|p| p.contains("\"shape\""))
                            .unwrap_or(false)
                            && !m.contains("\"shape\"");
                        if !keep_shape {
                            pending_cursor = Some(m);
                        }
                    }
                    None => break "session ended".into(),
                }
            }
        }
    };
    for r in readers {
        r.abort();
    }
    reason
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_channel() {
        assert_eq!(
            parse_keys(r#"{"t":"down","code":"KeyA"}"#),
            Some(InputEvent::Key {
                code: "KeyA".into(),
                down: true
            })
        );
        assert_eq!(
            parse_keys(r#"{"t":"btn","b":2,"down":false}"#),
            Some(InputEvent::Button {
                button: 2,
                down: false
            })
        );
        assert_eq!(
            parse_keys(r#"{"t":"wheel","dy":-120}"#),
            Some(InputEvent::Wheel { dx: 0, dy: -1 })
        );
        assert_eq!(parse_keys("not json"), None);
        assert_eq!(parse_keys(r#"{"t":"paste","text":"x"}"#), None);
    }

    #[test]
    fn motion_channel() {
        let mut b = vec![1u8];
        b.extend_from_slice(&7u32.to_le_bytes());
        b.extend_from_slice(&100i32.to_le_bytes());
        b.extend_from_slice(&(-5i32).to_le_bytes());
        assert_eq!(parse_motion(&b), Some(InputEvent::Abs { x: 100, y: 0 }));
        b[0] = 2;
        assert_eq!(parse_motion(&b), Some(InputEvent::Rel { dx: 100, dy: -5 }));
        assert_eq!(parse_motion(&b[..12]), None);
    }

    #[test]
    fn framebuffer_updates_land_in_place() {
        // 4x2 BGRX, stride 20 (padded rows)
        let mut data = vec![0u8; 40];
        data[20] = 9; // row 1, pixel 0, blue
        let mut fb = Framebuffer::scanout(4, 2, 20, PixelFormat::Bgrx, &data);
        assert_eq!(fb.pixels.len(), 32);
        assert_eq!(fb.pixels[16], 9);
        // 1x1 update at (3,1)
        fb.update(3, 1, 1, 1, 4, &[1, 2, 3, 4]);
        assert_eq!(&fb.pixels[28..32], &[1, 2, 3, 4]);
        // out-of-range updates are clipped, not panics
        fb.update(3, 1, 10, 10, 40, &[7u8; 400]);
        fb.update(-5, -5, 2, 2, 8, &[7u8; 16]);
    }
}
