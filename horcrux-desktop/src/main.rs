//! Horcrux Console desktop client.
//!
//! The same protocol as the web client: log in, get a console ticket bound
//! to the API WebSocket connection, offer WebRTC on that socket, then
//! H.264 video plus the `ctl`/`keys`/`input`/`pad`/... data channels over
//! the direct path or the node's relay. The window adds what a browser
//! cannot do well: raw relative mouse motion while captured, gamepads read
//! from the system, and frames shown the moment they are decoded at the
//! monitor's refresh rate.
//!
//! Without a node and VM to connect to (or with a `horcrux-console://`
//! link) the window starts as the connection window (`ui`): node, user,
//! password, then the node's VMs that can be shown, and a profile.
// A window application on Windows (no console window of its own); command
// line use attaches to the parent's console (see `attach_console`).
#![cfg_attr(windows, windows_subsystem = "windows")]
mod config;
mod gamepad;
#[cfg(target_os = "macos")]
mod macos_link;
mod net;
mod session;
mod ui;

use clap::Parser;
use config::{Args, Recent, Settings};
use horcrux_media::decoder::{self, Decoder};
use horcrux_media::yuv;
use serde_json::{json, Value};
use session::{FromSession, ToSession};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;
use winit::application::ApplicationHandler;
use winit::event::{
    DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::{CursorGrabMode, Fullscreen, Window, WindowId};

/// W3C `KeyboardEvent.code` for a winit key (winit uses the same names
/// except for the Meta keys).
fn w3c_code(k: KeyCode) -> String {
    match k {
        KeyCode::SuperLeft => "MetaLeft".into(),
        KeyCode::SuperRight => "MetaRight".into(),
        other => format!("{other:?}"),
    }
}

fn decoder_config(s: &Settings) -> decoder::Config {
    decoder::Config {
        backend: s.decoder.clone(),
        hardware: s.hardware_decode,
        avcodec: s.avcodec.clone(),
        avutil: s.avutil.clone(),
    }
}

fn env_password(s: &Settings) -> Option<String> {
    std::env::var(&s.password_env).ok()
}

fn terminal_password(s: &Settings) -> Result<String, String> {
    rpassword::prompt_password(format!("Password for {}@{}: ", s.user, s.node))
        .map_err(|e| e.to_string())
}

/// W3C key codes (and Shift) that type `text` on a US layout.
fn text_keys(text: &str) -> Result<Vec<(String, bool)>, String> {
    const PLAIN: &str = "`1234567890-=[]\\;',./";
    const SHIFTED: &str = "~!@#$%^&*()_+{}|:\"<>?";
    const CODES: [&str; 21] = [
        "Backquote",
        "Digit1",
        "Digit2",
        "Digit3",
        "Digit4",
        "Digit5",
        "Digit6",
        "Digit7",
        "Digit8",
        "Digit9",
        "Digit0",
        "Minus",
        "Equal",
        "BracketLeft",
        "BracketRight",
        "Backslash",
        "Semicolon",
        "Quote",
        "Comma",
        "Period",
        "Slash",
    ];
    text.chars()
        .map(|c| match c {
            'a'..='z' => Ok((format!("Key{}", c.to_ascii_uppercase()), false)),
            'A'..='Z' => Ok((format!("Key{c}"), true)),
            ' ' => Ok(("Space".into(), false)),
            '\n' => Ok(("Enter".into(), false)),
            '\t' => Ok(("Tab".into(), false)),
            _ => {
                if let Some(i) = PLAIN.chars().position(|p| p == c) {
                    Ok((CODES[i].into(), false))
                } else if let Some(i) = SHIFTED.chars().position(|p| p == c) {
                    Ok((CODES[i].into(), true))
                } else {
                    Err(format!("cannot type {c:?}"))
                }
            }
        })
        .collect()
}

/// A decoded I420 picture as a binary PPM.
fn ppm(y: &[u8], u: &[u8], v: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut px = vec![0u32; w * h];
    yuv::i420_to_xrgb(
        y,
        w,
        u,
        w.div_ceil(2),
        v,
        w.div_ceil(2),
        w,
        h,
        &mut px,
        w,
        h,
    );
    let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
    for p in px {
        out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
    }
    out
}

fn pct(v: &mut [f64], p: f64) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some(v[((v.len() - 1) as f64 * p).round() as usize])
}

/// A picture as owned I420 planes and its size.
type I420 = (Vec<u8>, Vec<u8>, Vec<u8>, usize, usize);

#[derive(Default)]
struct Counters {
    decoded: u64,
    errors: u64,
    size: (usize, usize),
    connected_ms: Option<u64>,
    last_err: Option<String>,
    /// the last picture, kept for --snapshot
    last: Option<I420>,
}

/// No window: connect, decode, measure, report JSON (CI e2e).
fn headless(
    s: &Settings,
    args: &Args,
    rt: &tokio::runtime::Runtime,
    api: &net::Api,
    st: session::Started,
) -> Result<Value, String> {
    let mut dec = Decoder::open(&decoder_config(s))?;
    let keep = args.snapshot.is_some();
    let t0 = Instant::now();
    let mut c = Counters::default();
    let step =
        |c: &mut Counters, dec: &mut Decoder, wait: Duration| -> Result<Option<Instant>, String> {
            match st.from.recv_timeout(wait) {
                Ok(FromSession::Frame(au, at)) => {
                    match dec.decode(&au, |p| {
                        c.size = (p.width, p.height);
                        if keep {
                            let (w, h, cw) = (p.width, p.height, p.width.div_ceil(2));
                            let ch = h.div_ceil(2);
                            let copy = |src: &[u8], stride: usize, w: usize, h: usize| {
                                let mut d = vec![0u8; w * h];
                                for r in 0..h {
                                    d[r * w..(r + 1) * w]
                                        .copy_from_slice(&src[r * stride..r * stride + w]);
                                }
                                d
                            };
                            c.last = Some((
                                copy(p.y, p.y_stride, w, h),
                                copy(p.u, p.u_stride, cw, ch),
                                copy(p.v, p.v_stride, cw, ch),
                                w,
                                h,
                            ));
                        }
                    }) {
                        Ok(true) => {
                            c.decoded += 1;
                            Ok(Some(at))
                        }
                        Ok(false) => Ok(None),
                        Err(e) => {
                            c.errors += 1;
                            c.last_err = Some(e);
                            let _ = st.to.send(ToSession::Keyframe);
                            Ok(None)
                        }
                    }
                }
                Ok(FromSession::Connected) => {
                    c.connected_ms
                        .get_or_insert(t0.elapsed().as_millis() as u64);
                    Ok(None)
                }
                Ok(FromSession::Closed(r)) => Err(r),
                Ok(_) => Ok(None),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(None),
                Err(_) => Err("session ended".into()),
            }
        };
    let _ = st.to.send(ToSession::Ctl(
        json!({"t": "display", "refresh_hz": if s.refresh_hz > 0 { s.refresh_hz } else { 60 }}),
    ));
    let warm_end = Instant::now() + Duration::from_secs(args.warmup_secs);
    while Instant::now() < warm_end {
        step(&mut c, &mut dec, Duration::from_millis(50))?;
    }
    let warm_frames = c.decoded;
    if let Some(t) = &args.type_text {
        // a literal backslash-n is Enter too (handy from scripts)
        for (code, shift) in text_keys(&t.replace("\\n", "\n"))? {
            if shift {
                let _ = st.to.send(ToSession::Key("ShiftLeft".into(), true));
            }
            let _ = st.to.send(ToSession::Key(code.clone(), true));
            let _ = st.to.send(ToSession::Key(code, false));
            if shift {
                let _ = st.to.send(ToSession::Key("ShiftLeft".into(), false));
            }
            step(&mut c, &mut dec, Duration::from_millis(30))?;
        }
        let end = Instant::now() + Duration::from_secs(3);
        while Instant::now() < end {
            step(&mut c, &mut dec, Duration::from_millis(20))?;
        }
    }
    // key-to-picture: press a key, time to the next decoded frame (the
    // node sends nothing new for an unchanged screen)
    let mut lat = Vec::new();
    for i in 0..args.trials {
        // drain, then wait for a quiet screen
        let quiet = Instant::now() + Duration::from_millis(400);
        while Instant::now() < quiet {
            step(&mut c, &mut dec, Duration::from_millis(20))?;
        }
        let code = if i % 2 == 0 { "KeyA" } else { "Backspace" };
        let sent = Instant::now();
        let _ = st.to.send(ToSession::Key(code.into(), true));
        let _ = st.to.send(ToSession::Key(code.into(), false));
        let end = sent + Duration::from_secs(2);
        while Instant::now() < end {
            if let Some(at) = step(&mut c, &mut dec, Duration::from_millis(5))? {
                lat.push(at.saturating_duration_since(sent).as_secs_f64() * 1000.0);
                break;
            }
        }
    }
    if args.exercise_input {
        let pad = gamepad::Pad {
            buttons: gamepad::bit::A,
            lx: 12000,
            ..Default::default()
        };
        let _ = st.to.send(ToSession::Pad(pad.bytes()));
        let _ = st.to.send(ToSession::Pad(gamepad::Pad::default().bytes()));
        let _ = st.to.send(ToSession::Motion(2, 5, -5));
        let _ = st.to.send(ToSession::Ctl(json!({"t": "ping", "id": 1})));
        let end = Instant::now() + Duration::from_secs(1);
        while Instant::now() < end {
            step(&mut c, &mut dec, Duration::from_millis(20))?;
        }
    }
    let total = t0.elapsed().as_secs_f64();
    // the node's view of this session (input counters, path, encoder)
    let node = rt
        .block_on(api.get(&format!("/console/{}/hxc/sessions", s.vm)))
        .ok()
        .and_then(|v| {
            v.as_array()?
                .iter()
                .find(|x| x["id"] == st.session.as_str())
                .cloned()
        })
        .unwrap_or(Value::Null);
    if let (Some(path), Some((y, u, v, w, h))) = (&args.snapshot, &c.last) {
        std::fs::write(path, ppm(y, u, v, *w, *h)).map_err(|e| format!("{path}: {e}"))?;
    }
    let _ = st.to.send(ToSession::Bye);
    // the session ends on its own; give the bye a moment
    rt.block_on(async { tokio::time::sleep(Duration::from_millis(300)).await });
    let mut l2 = lat.clone();
    Ok(json!({
        "session": st.session,
        "path": st.path,
        "decoder": dec.describe(),
        "connected_ms": c.connected_ms,
        "frames_decoded": c.decoded,
        "warmup_frames": warm_frames,
        "decode_errors": c.errors,
        "last_decode_error": c.last_err,
        "width": c.size.0,
        "height": c.size.1,
        "seconds": total,
        "key_to_frame_ms": lat,
        "key_to_frame_median_ms": pct(&mut l2.clone(), 0.5),
        "key_to_frame_p90_ms": pct(&mut l2, 0.9),
        "node": node,
    }))
}

/// How to change a new window so it fits on its monitor.
#[derive(Debug, PartialEq, Default)]
struct Fit {
    inner: Option<(u32, u32)>,
    position: Option<(i32, i32)>,
}

/// `monitor` = (x, y, width, height), `pos` = the window's outer position
/// (None where the system does not tell, e.g. Wayland: it places windows
/// itself), `inner`/`outer` = the window's sizes, all in physical pixels.
/// The inner size shrinks to 90 % of the monitor; a window that would then
/// stick out of the monitor is centred on it.
fn fit_on_monitor(
    monitor: (i32, i32, u32, u32),
    pos: Option<(i32, i32)>,
    inner: (u32, u32),
    outer: (u32, u32),
) -> Fit {
    let (mx, my, mw, mh) = monitor;
    if mw == 0 || mh == 0 {
        return Fit::default();
    }
    let (lw, lh) = (mw * 9 / 10, mh * 9 / 10);
    let new_inner = (inner.0.min(lw), inner.1.min(lh));
    let mut fit = Fit::default();
    if new_inner != inner {
        fit.inner = Some(new_inner);
    }
    // frame and title bar keep their size
    let frame = (
        outer.0.saturating_sub(inner.0),
        outer.1.saturating_sub(inner.1),
    );
    let (ow, oh) = (
        (new_inner.0 + frame.0).min(mw) as i32,
        (new_inner.1 + frame.1).min(mh) as i32,
    );
    if let Some((x, y)) = pos {
        let inside = x >= mx && y >= my && x + ow <= mx + mw as i32 && y + oh <= my + mh as i32;
        if !inside {
            fit.position = Some((mx + (mw as i32 - ow) / 2, my + (mh as i32 - oh) / 2));
        }
    }
    fit
}

/// A failed login, with the fingerprint of an untrusted certificate.
struct LoginFail {
    msg: String,
    fingerprint: Option<String>,
}

type LoggedIn = Result<(Arc<net::Api>, Vec<ui::VmEntry>, usize), LoginFail>;

enum UserEvent {
    Session(FromSession),
    LoggedIn(LoggedIn),
    Started(Box<Result<(session::Started, Settings), String>>),
    /// A `horcrux-console://` link the system handed to the running app
    /// (macOS sends clicked links as an Apple Event, see `macos_link`).
    #[cfg(target_os = "macos")]
    Link(String),
}

/// The node's VMs the Horcrux Console can show now (running QEMU VMs whose
/// console status says available), and how many others there are.
async fn console_vms(api: &net::Api) -> Result<(Vec<ui::VmEntry>, usize), String> {
    let all = api.get("/vms").await?;
    let all = all.as_array().cloned().unwrap_or_default();
    let candidates: Vec<&Value> = all
        .iter()
        .filter(|v| v["hypervisor"] == "qemu" && v["status"] == "running")
        .take(256)
        .collect();
    let checks = futures::future::join_all(candidates.iter().map(|v| async move {
        let id = v["id"].as_str().unwrap_or_default().to_string();
        let ok = !id.is_empty()
            && api
                .get(&format!("/console/{id}/hxc/status"))
                .await
                .map(|st| st["available"] == true)
                .unwrap_or(false);
        ok.then(|| ui::VmEntry {
            name: v["name"].as_str().unwrap_or(&id).to_string(),
            id,
        })
    }))
    .await;
    let mut vms: Vec<ui::VmEntry> = checks.into_iter().flatten().collect();
    vms.sort_by_key(|v| v.name.to_lowercase());
    let hidden = all.len() - vms.len();
    Ok((vms, hidden))
}

#[derive(PartialEq)]
enum Mode {
    Connect,
    Console,
}

struct App {
    s: Settings,
    rt: Arc<tokio::runtime::Runtime>,
    proxy: EventLoopProxy<UserEvent>,
    mode: Mode,
    form: ui::Form,
    layout: ui::Layout,
    recent: Recent,
    recent_path: Option<PathBuf>,
    api: Option<Arc<net::Api>>,
    /// The console was opened from the connection window: go back to it
    /// when the console closes.
    from_window: bool,
    /// Open `form.want_vm` right after the login (a link or a configured
    /// VM).
    auto_open: bool,
    /// The certificate pin used for the last login came from `recent`.
    pinned_from_recent: bool,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    to: Option<UnboundedSender<ToSession>>,
    /// Where gamepad threads send (the current session, if any).
    pads: Arc<Mutex<Option<UnboundedSender<ToSession>>>>,
    gamepads_started: bool,
    dec: Option<Decoder>,
    /// last decoded picture
    pic: I420,
    shown: yuv::Rect,
    captured: bool,
    relative_ok: bool,
    status: String,
    title_at: Instant,
    frames: u64,
    cursor: (f64, f64),
    mods: ModifiersState,
}

impl App {
    fn send(&self, m: ToSession) {
        if let Some(to) = &self.to {
            let _ = to.send(m);
        }
    }

    fn set_capture(&mut self, on: bool) {
        let Some(w) = self.window.as_ref() else {
            return;
        };
        if on {
            // Locked: macOS, X11/Wayland pointer lock; Windows has only
            // Confined (ClipCursor): raw WM_INPUT motion keeps coming at
            // the clip edge, so relative motion is unaffected.
            let ok = w
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| w.set_cursor_grab(CursorGrabMode::Confined))
                .is_ok();
            w.set_cursor_visible(!ok);
            self.captured = ok;
        } else {
            let _ = w.set_cursor_grab(CursorGrabMode::None);
            w.set_cursor_visible(true);
            self.captured = false;
        }
        self.title();
    }

    fn title(&self) {
        let Some(w) = &self.window else { return };
        if self.mode == Mode::Connect {
            w.set_title("Horcrux Console - connect");
            return;
        }
        let cap = if self.captured {
            format!(" - mouse captured ({} releases)", self.s.release_key)
        } else {
            String::new()
        };
        w.set_title(&format!(
            "{} - Horcrux Console{cap} {}",
            self.s.vm, self.status
        ));
    }

    fn ui_scale(&self) -> i32 {
        if self.s.ui_scale > 0 {
            return self.s.ui_scale as i32;
        }
        let f = self
            .window
            .as_ref()
            .map(|w| w.scale_factor())
            .unwrap_or(1.0);
        ((2.0 * f).round() as i32).clamp(1, 4)
    }

    /// Fill the connection window from a `horcrux-console://` link: node,
    /// user, profile and the VM to open once logged in. The password is
    /// always asked for (any web page can open a link). A running console
    /// keeps going; the window shows the link's node when it closes.
    fn apply_link(&mut self, l: config::Link) {
        let user = l.user.clone().unwrap_or_else(|| self.s.user.clone());
        let mut form = ui::Form::new(&l.node, &user, l.profile.as_deref().unwrap_or_default());
        form.want_vm = Some(l.vm);
        self.form = form;
        self.auto_open = true;
        if self.mode == Mode::Connect {
            self.api = None;
        }
    }

    fn present(&mut self) {
        let scale = self.ui_scale();
        let (Some(w), Some(surf)) = (self.window.as_ref(), self.surface.as_mut()) else {
            return;
        };
        let size = w.inner_size();
        let (Some(dw), Some(dh)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return;
        };
        if surf.resize(dw, dh).is_err() {
            return;
        }
        let Ok(mut buf) = surf.buffer_mut() else {
            return;
        };
        if self.mode == Mode::Connect {
            let (cw, ch) = (dw.get() as i32, dh.get() as i32);
            self.layout = ui::Layout::new(&self.form, cw, ch, scale);
            ui::draw(
                &self.form,
                &self.layout,
                &mut ui::Canvas {
                    px: &mut buf,
                    w: cw,
                    h: ch,
                },
            );
            let _ = buf.present();
            return;
        }
        let (y, u, v, pw, ph) = (
            &self.pic.0,
            &self.pic.1,
            &self.pic.2,
            self.pic.3,
            self.pic.4,
        );
        if pw == 0 {
            buf.fill(0);
        } else {
            self.shown = yuv::i420_to_xrgb(
                y,
                pw,
                u,
                pw.div_ceil(2),
                v,
                pw.div_ceil(2),
                pw,
                ph,
                &mut buf,
                dw.get() as usize,
                dh.get() as usize,
            );
        }
        let _ = buf.present();
    }

    fn guest_xy(&self, x: f64, y: f64) -> Option<(i32, i32)> {
        let r = self.shown;
        if r.w == 0 || self.pic.3 == 0 {
            return None;
        }
        let gx =
            ((x - r.x as f64) * self.pic.3 as f64 / r.w as f64).clamp(0.0, self.pic.3 as f64 - 1.0);
        let gy =
            ((y - r.y as f64) * self.pic.4 as f64 / r.h as f64).clamp(0.0, self.pic.4 as f64 - 1.0);
        Some((gx as i32, gy as i32))
    }

    fn send_display(&self) {
        let hz = if self.s.refresh_hz > 0 {
            self.s.refresh_hz
        } else {
            self.window
                .as_ref()
                .and_then(|w| w.current_monitor())
                .and_then(|m| m.refresh_rate_millihertz())
                .map(|m| m.div_ceil(1000))
                .unwrap_or(60)
        };
        self.send(ToSession::Ctl(json!({"t": "display", "refresh_hz": hz})));
    }

    fn start_gamepads(&mut self) {
        if self.gamepads_started || !self.s.gamepads {
            return;
        }
        self.gamepads_started = true;
        let pads = self.pads.clone();
        let n = gamepad::spawn(
            move |p| {
                if let Some(to) = pads.lock().ok().and_then(|g| g.clone()) {
                    let _ = to.send(ToSession::Pad(p.bytes()));
                }
            },
            self.s.gamepad_poll_hz,
        );
        tracing::info!("{n} gamepad(s)");
    }

    /// A session is up: switch the window to the console.
    fn begin_console(&mut self, st: session::Started, s: Settings) {
        let dec = match Decoder::open(&decoder_config(&s)) {
            Ok(d) => d,
            Err(e) => {
                let _ = st.to.send(ToSession::Bye);
                self.fail(format!("H.264 decoder: {e}"));
                return;
            }
        };
        tracing::info!("decoder: {}", dec.describe());
        self.s = s;
        forward(st.from, self.proxy.clone());
        if let Ok(mut g) = self.pads.lock() {
            *g = Some(st.to.clone());
        }
        self.to = Some(st.to);
        self.dec = Some(dec);
        self.pic = (vec![], vec![], vec![], 0, 0);
        self.mode = Mode::Console;
        self.status = "connecting".into();
        self.send_display();
        self.start_gamepads();
        self.title();
        self.present();
    }

    fn end_console(&mut self, why: String) {
        if self.captured {
            self.set_capture(false);
        }
        if let Ok(mut g) = self.pads.lock() {
            *g = None;
        }
        self.to = None;
        self.dec = None;
        self.mode = Mode::Connect;
        self.form.busy = false;
        self.form.error = true;
        self.form.status = format!("Console closed: {why}");
        self.title();
        self.present();
    }

    fn fail(&mut self, msg: String) {
        self.form.busy = false;
        self.form.error = true;
        self.form.status = msg;
        self.present();
    }

    fn save_recent(&mut self) {
        if !self.s.remember_connection {
            return;
        }
        if let Some(p) = &self.recent_path {
            if let Err(e) = self.recent.save(p) {
                tracing::warn!("{e}");
            }
        }
    }

    /// The settings for a connection from the window's fields.
    fn window_settings(&mut self) -> Result<Settings, String> {
        let mut s = self.s.clone();
        s.node = self.form.node.trim().trim_end_matches('/').to_string();
        s.user = self.form.user.trim().to_string();
        s.vm = self
            .form
            .selected_vm()
            .map(|v| v.id.clone())
            .unwrap_or_else(|| "x".into());
        s.profile = self.form.profile_name().into();
        s.validate_target()?;
        self.pinned_from_recent = false;
        if s.ca_file.is_empty() && s.cert_sha256.is_empty() {
            if let Some(fp) = self.recent.trusted.get(&s.node) {
                s.cert_sha256 = fp.clone();
                self.pinned_from_recent = true;
            }
        }
        Ok(s)
    }

    fn connect(&mut self) {
        let s = match self.window_settings() {
            Ok(s) => s,
            Err(e) => return self.fail(e),
        };
        if s.user.is_empty() {
            return self.fail("Enter a user name".into());
        }
        let t = match net::tls(&s) {
            Ok(t) => t,
            Err(e) => return self.fail(e),
        };
        self.form.busy = true;
        self.form.error = false;
        self.form.detail.clear();
        self.form.status = format!("Logging in to {} ...", s.node);
        self.present();
        let pw = self.form.password.clone();
        let proxy = self.proxy.clone();
        let pinned_by_user =
            !s.ca_file.is_empty() || (!s.cert_sha256.is_empty() && !self.pinned_from_recent);
        let from_recent = self.pinned_from_recent;
        self.rt.spawn(async move {
            let r: LoggedIn = async {
                let api = net::Api::login(&s, &t, &pw).await.map_err(|msg| {
                    // offer to trust the certificate only when nothing was
                    // pinned; a pinned one that changed is just refused
                    let fp = t.seen().filter(|_| !pinned_by_user && !from_recent);
                    LoginFail {
                        msg: if fp.is_some() {
                            "The node's certificate is not trusted yet. Compare its SHA-256 with the node (System > Certificates) before trusting it:".into()
                        } else if from_recent && t.seen().is_some() {
                            format!("The node's certificate changed since you trusted it ({msg})")
                        } else {
                            msg
                        },
                        fingerprint: fp,
                    }
                })?;
                let (vms, hidden) = console_vms(&api).await.map_err(|msg| LoginFail {
                    msg: format!("VM list: {msg}"),
                    fingerprint: None,
                })?;
                Ok((Arc::new(api), vms, hidden))
            }
            .await;
            let _ = proxy.send_event(UserEvent::LoggedIn(r));
        });
    }

    fn open(&mut self) {
        let (Some(api), Some(vm)) = (self.api.clone(), self.form.selected_vm().cloned()) else {
            return;
        };
        let s = match self.window_settings() {
            Ok(s) => s,
            Err(e) => return self.fail(e),
        };
        self.form.busy = true;
        self.form.error = false;
        self.form.status = format!("Opening {} ...", vm.name);
        self.present();
        let proxy = self.proxy.clone();
        self.rt.spawn(async move {
            let r = session::start(&s, &api).await.map(|st| (st, s));
            let _ = proxy.send_event(UserEvent::Started(Box::new(r)));
        });
    }

    fn act(&mut self, el: &ActiveEventLoop, a: ui::Action) {
        match a {
            ui::Action::None => {}
            ui::Action::Quit => el.exit(),
            ui::Action::Connect => self.connect(),
            ui::Action::Open => self.open(),
            ui::Action::Trust => {
                if let Some(fp) = self.form.untrusted.take() {
                    let node = self.form.node.trim().trim_end_matches('/').to_string();
                    self.recent.trusted.insert(node, fp);
                    self.save_recent();
                    self.form.detail.clear();
                    self.connect();
                }
            }
        }
        self.present();
    }

    fn ui_key(&self, ev: &winit::event::KeyEvent) -> Vec<ui::Key> {
        let shift = self.mods.shift_key();
        match &ev.logical_key {
            Key::Named(NamedKey::Tab) if shift => vec![ui::Key::BackTab],
            Key::Named(NamedKey::Tab) => vec![ui::Key::Tab],
            Key::Named(NamedKey::Enter) => vec![ui::Key::Enter],
            Key::Named(NamedKey::Backspace) => vec![ui::Key::Backspace],
            Key::Named(NamedKey::Escape) => vec![ui::Key::Escape],
            Key::Named(NamedKey::ArrowUp) => vec![ui::Key::Up],
            Key::Named(NamedKey::ArrowDown) => vec![ui::Key::Down],
            Key::Named(NamedKey::ArrowLeft) => vec![ui::Key::Left],
            Key::Named(NamedKey::ArrowRight) => vec![ui::Key::Right],
            _ => ev
                .text
                .as_ref()
                .map(|t| t.chars().map(ui::Key::Char).collect())
                .unwrap_or_default(),
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let title = if self.mode == Mode::Connect {
            "Horcrux Console - connect".to_string()
        } else {
            format!("{} - Horcrux Console", self.s.vm)
        };
        let mut attrs = Window::default_attributes()
            .with_title(title)
            .with_inner_size(winit::dpi::LogicalSize::new(self.s.width, self.s.height));
        if self.s.fullscreen && self.mode == Mode::Console {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        let w = match el.create_window(attrs) {
            Ok(w) => Rc::new(w),
            Err(e) => {
                eprintln!("window: {e}");
                el.exit();
                return;
            }
        };
        // a window bigger than the screen hides its own controls: shrink it
        // to 90 % of the monitor (the settings' size stays the upper bound)
        // and move it so it is wholly on that monitor (systems place new
        // windows cascaded, which pushed a large window off the screen)
        if w.fullscreen().is_none() {
            if let Some(mon) = w.current_monitor() {
                let (mp, ms) = (mon.position(), mon.size());
                let (is, os) = (w.inner_size(), w.outer_size());
                let fit = fit_on_monitor(
                    (mp.x, mp.y, ms.width, ms.height),
                    w.outer_position().ok().map(|p| (p.x, p.y)),
                    (is.width, is.height),
                    (os.width, os.height),
                );
                if let Some((iw, ih)) = fit.inner {
                    let _ = w.request_inner_size(winit::dpi::PhysicalSize::new(iw, ih));
                }
                if let Some((x, y)) = fit.position {
                    w.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
                }
            }
        }
        let ctx = match softbuffer::Context::new(w.clone()) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("display connection: {e}");
                el.exit();
                return;
            }
        };
        match softbuffer::Surface::new(&ctx, w.clone()) {
            Ok(s) => self.surface = Some(s),
            Err(e) => {
                eprintln!("window surface: {e}");
                el.exit();
                return;
            }
        }
        self.window = Some(w);
        if self.mode == Mode::Console {
            self.send_display();
            self.status = "connecting".into();
        }
        self.title();
        self.present();
    }

    fn user_event(&mut self, el: &ActiveEventLoop, ev: UserEvent) {
        let m = match ev {
            UserEvent::LoggedIn(r) => {
                self.form.busy = false;
                match r {
                    Ok((api, vms, hidden)) => {
                        let n = vms.len();
                        self.api = Some(api);
                        self.form.set_vms(vms, hidden);
                        self.form.error = false;
                        self.form.detail.clear();
                        self.form.status = format!(
                            "Logged in as {} - {n} VM{} can be shown",
                            self.form.user.trim(),
                            if n == 1 { "" } else { "s" }
                        );
                        self.recent.node = self.form.node.trim().trim_end_matches('/').into();
                        self.recent.user = self.form.user.trim().into();
                        self.save_recent();
                        let wanted = self.form.want_vm.clone();
                        if self.auto_open
                            && wanted.is_some()
                            && self
                                .form
                                .selected_vm()
                                .map(|v| Some(&v.id) == wanted.as_ref())
                                == Some(true)
                        {
                            self.auto_open = false;
                            self.open();
                        }
                    }
                    Err(f) => {
                        self.form.error = true;
                        self.form.status = f.msg;
                        self.form.detail = f.fingerprint.clone().unwrap_or_default();
                        self.form.untrusted = f.fingerprint;
                        if self.form.untrusted.is_some() {
                            self.form.focus = Some(ui::Field::Trust);
                        }
                    }
                }
                self.present();
                return;
            }
            UserEvent::Started(r) => {
                self.form.busy = false;
                match *r {
                    Ok((st, s)) => {
                        self.recent.vm = s.vm.clone();
                        self.recent.profile = s.profile.clone();
                        self.save_recent();
                        self.from_window = true;
                        self.begin_console(st, s);
                    }
                    Err(e) => self.fail(e),
                }
                return;
            }
            #[cfg(target_os = "macos")]
            UserEvent::Link(url) => {
                match config::parse_link(&url) {
                    Ok(l) => self.apply_link(l),
                    Err(e) => {
                        tracing::warn!("refused link: {e}");
                        if self.mode == Mode::Connect {
                            self.form.error = true;
                            self.form.status = format!("Link refused: {e}");
                        }
                    }
                }
                self.present();
                return;
            }
            UserEvent::Session(m) => m,
        };
        match m {
            FromSession::Frame(au, _) => {
                let Some(dec) = self.dec.as_mut() else { return };
                let mut got = None;
                match dec.decode(&au, |p| {
                    let (w, h) = (p.width, p.height);
                    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
                    let mut y = vec![0u8; w * h];
                    let mut u = vec![0u8; cw * ch];
                    let mut v = vec![0u8; cw * ch];
                    for r in 0..h {
                        y[r * w..(r + 1) * w]
                            .copy_from_slice(&p.y[r * p.y_stride..r * p.y_stride + w]);
                    }
                    for r in 0..ch {
                        u[r * cw..(r + 1) * cw]
                            .copy_from_slice(&p.u[r * p.u_stride..r * p.u_stride + cw]);
                        v[r * cw..(r + 1) * cw]
                            .copy_from_slice(&p.v[r * p.v_stride..r * p.v_stride + cw]);
                    }
                    got = Some((y, u, v, w, h));
                }) {
                    Ok(_) => {
                        if let Some(p) = got {
                            self.pic = p;
                            self.frames += 1;
                            self.present();
                        }
                    }
                    Err(_) => self.send(ToSession::Keyframe),
                }
                if self.title_at.elapsed() > Duration::from_secs(1) {
                    self.title_at = Instant::now();
                    self.status = format!("{}x{} {} fps", self.pic.3, self.pic.4, self.frames);
                    self.frames = 0;
                    self.title();
                }
            }
            FromSession::Connected => {
                self.status = "connected".into();
                self.title();
            }
            FromSession::Ctl(v) => {
                if v["t"] == "hello" {
                    // guest tools can take raw relative motion
                    self.relative_ok = v["relative"].as_bool().unwrap_or(false);
                }
            }
            FromSession::Cursor => {}
            FromSession::Closed(r) => {
                eprintln!("console closed: {r}");
                if self.from_window {
                    self.end_console(r);
                } else {
                    el.exit();
                }
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, ev: WindowEvent) {
        match ev {
            WindowEvent::CloseRequested => {
                self.send(ToSession::Bye);
                el.exit();
                return;
            }
            WindowEvent::RedrawRequested | WindowEvent::Resized(_) => {
                self.present();
                return;
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                self.present();
                return;
            }
            WindowEvent::ModifiersChanged(m) => {
                self.mods = m.state();
                return;
            }
            _ => {}
        }
        if self.mode == Mode::Connect {
            match ev {
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == ElementState::Pressed =>
                {
                    for k in self.ui_key(&event) {
                        let a = self.form.key(k);
                        self.act(el, a);
                    }
                    self.present();
                }
                WindowEvent::CursorMoved { position, .. } => {
                    self.cursor = (position.x, position.y);
                }
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    button: MouseButton::Left,
                    ..
                } => {
                    let a = self.form.click(
                        self.cursor.0 as i32,
                        self.cursor.1 as i32,
                        &self.layout.clone(),
                    );
                    self.act(el, a);
                    self.present();
                }
                _ => {}
            }
            return;
        }
        match ev {
            WindowEvent::Focused(false) => {
                if self.captured {
                    self.set_capture(false);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(k) = event.physical_key {
                    let code = w3c_code(k);
                    if self.captured
                        && code == self.s.release_key
                        && event.state == ElementState::Pressed
                    {
                        self.set_capture(false);
                        return;
                    }
                    if event.repeat {
                        return;
                    }
                    self.send(ToSession::Key(code, event.state == ElementState::Pressed));
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if !self.captured {
                    if let Some((x, y)) = self.guest_xy(position.x, position.y) {
                        self.send(ToSession::Motion(1, x, y));
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let b = match button {
                    MouseButton::Left => 0,
                    MouseButton::Middle => 1,
                    MouseButton::Right => 2,
                    MouseButton::Back => 3,
                    MouseButton::Forward => 4,
                    MouseButton::Other(n) => n as u32,
                };
                let want_capture = match self.s.relative_mouse.as_str() {
                    "on" => true,
                    "auto" => self.relative_ok,
                    _ => false,
                };
                if state == ElementState::Pressed && !self.captured && want_capture {
                    self.set_capture(true);
                }
                self.send(ToSession::Button(b, state == ElementState::Pressed));
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x.signum() as i32, -y.signum() as i32),
                    MouseScrollDelta::PixelDelta(p) => (p.x.signum() as i32, -p.y.signum() as i32),
                };
                self.send(ToSession::Wheel(dx, dy));
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, ev: DeviceEvent) {
        // raw, unaccelerated motion while the mouse is captured (WM_INPUT
        // on Windows, XInput2 raw events / relative-pointer on Linux,
        // CGEvent deltas on macOS)
        if let DeviceEvent::MouseMotion { delta } = ev {
            if self.captured {
                self.send(ToSession::Motion(
                    2,
                    delta.0.round() as i32,
                    delta.1.round() as i32,
                ));
            }
        }
    }
}

fn forward(rx: std::sync::mpsc::Receiver<FromSession>, proxy: EventLoopProxy<UserEvent>) {
    std::thread::spawn(move || {
        while let Ok(m) = rx.recv() {
            let end = matches!(m, FromSession::Closed(_));
            if proxy.send_event(UserEvent::Session(m)).is_err() || end {
                return;
            }
        }
    });
}

/// Windows: the program is a window application, so a command-line start
/// (headless runs, --print-defaults, --help, errors) attaches to the
/// console it was started from; redirected output keeps its handles.
#[cfg(windows)]
fn attach_console() {
    use windows::Win32::System::Console::{
        AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_OUTPUT_HANDLE,
    };
    // SAFETY: plain console API calls.
    unsafe {
        let redirected = GetStdHandle(STD_OUTPUT_HANDLE)
            .map(|h| !h.is_invalid() && !h.0.is_null())
            .unwrap_or(false);
        if !redirected {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

#[cfg(not(windows))]
fn attach_console() {}

fn main() {
    attach_console();
    let args = Args::parse();
    if args.print_defaults {
        print!(
            "{}",
            toml::to_string(&Settings::default()).expect("defaults serialize")
        );
        return;
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let code = match real_main(args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("horcrux-console: {e}");
            1
        }
    };
    std::process::exit(code);
}

fn real_main(args: Args) -> Result<(), String> {
    let s = Settings::load(&args)?;
    let link = args.url.as_deref().map(config::parse_link).transpose()?;
    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?,
    );
    if args.headless {
        s.validate()?;
        let pw = match env_password(&s) {
            Some(p) => p,
            None => terminal_password(&s)?,
        };
        let t = net::tls(&s)?;
        let (api, st) = rt.block_on(async {
            let api = net::Api::login(&s, &t, &pw).await?;
            let st = session::start(&s, &api).await?;
            Ok::<_, String>((api, st))
        })?;
        let report = headless(&s, &args, &rt, &api, st)?;
        println!("{report}");
        return Ok(());
    }
    let recent_path = Recent::path(Settings::file(&args).as_deref());
    let recent = match (&recent_path, s.remember_connection) {
        (Some(p), true) => Recent::load(p),
        _ => Recent::default(),
    };
    let el = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|e| e.to_string())?;
    // Connect at once only with a node, a VM and the password in the
    // environment, never for a link (any web page can open one; the
    // window always asks for the password there).
    let direct = link.is_none() && !args.connect_window && s.has_target();
    let direct_pw = if direct { env_password(&s) } else { None };
    let mut app = App {
        s: s.clone(),
        rt: rt.clone(),
        proxy: el.create_proxy(),
        mode: Mode::Connect,
        form: ui::Form::default(),
        layout: ui::Layout::default(),
        recent,
        recent_path,
        api: None,
        from_window: false,
        auto_open: false,
        pinned_from_recent: false,
        window: None,
        surface: None,
        to: None,
        pads: Arc::new(Mutex::new(None)),
        gamepads_started: false,
        dec: None,
        pic: (vec![], vec![], vec![], 0, 0),
        shown: yuv::Rect::default(),
        captured: false,
        relative_ok: false,
        status: String::new(),
        title_at: Instant::now(),
        frames: 0,
        cursor: (0.0, 0.0),
        mods: ModifiersState::empty(),
    };
    if let Some(pw) = direct_pw {
        s.validate()?;
        let t = net::tls(&s)?;
        let (_api, st) = rt.block_on(async {
            let api = net::Api::login(&s, &t, &pw).await?;
            let st = session::start(&s, &api).await?;
            Ok::<_, String>((api, st))
        })?;
        drop(pw);
        let dec = Decoder::open(&decoder_config(&s))?;
        tracing::info!("decoder: {}", dec.describe());
        forward(st.from, el.create_proxy());
        if let Ok(mut g) = app.pads.lock() {
            *g = Some(st.to.clone());
        }
        app.to = Some(st.to);
        app.dec = Some(dec);
        app.mode = Mode::Console;
        app.start_gamepads();
    } else {
        let r = &app.recent;
        let (node, user, profile, vm) = match &link {
            Some(_) => Default::default(),
            None if !s.node.is_empty() => (
                s.node.clone(),
                s.user.clone(),
                s.profile.clone(),
                (!s.vm.is_empty()).then(|| s.vm.clone()),
            ),
            None => (
                r.node.clone(),
                if r.user.is_empty() {
                    s.user.clone()
                } else {
                    r.user.clone()
                },
                r.profile.clone(),
                (!r.vm.is_empty()).then(|| r.vm.clone()),
            ),
        };
        app.auto_open = s.has_target();
        app.form = ui::Form::new(&node, &user, &profile);
        app.form.want_vm = vm;
        if let Some(l) = link {
            app.apply_link(l);
        }
    }
    // macOS hands clicked links to the running app as an Apple Event, not
    // on the command line: install the handler before the event loop runs
    // (the launch's own link arrives while it finishes launching).
    #[cfg(target_os = "macos")]
    macos_link::install(el.create_proxy());
    el.run_app(&mut app).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names_are_w3c_codes() {
        assert_eq!(w3c_code(KeyCode::KeyA), "KeyA");
        assert_eq!(w3c_code(KeyCode::Digit1), "Digit1");
        assert_eq!(w3c_code(KeyCode::ControlRight), "ControlRight");
        assert_eq!(w3c_code(KeyCode::SuperLeft), "MetaLeft");
        assert_eq!(w3c_code(KeyCode::ArrowUp), "ArrowUp");
        assert_eq!(w3c_code(KeyCode::F11), "F11");
    }

    #[test]
    fn new_windows_fit_on_their_monitor() {
        // the Windows eval VM: 1024x768, default 1280x800 window cascaded to
        // (164, 157) with an 8+31 px frame
        let f = fit_on_monitor(
            (0, 0, 1024, 768),
            Some((164, 157)),
            (1280, 800),
            (1296, 839),
        );
        assert_eq!(f.inner, Some((921, 691)));
        let (x, y) = f.position.unwrap();
        assert_eq!((x, y), ((1024 - 937) / 2, (768 - 730) / 2));
        // fits already: nothing changes
        let f = fit_on_monitor(
            (0, 0, 2560, 1440),
            Some((100, 100)),
            (1280, 800),
            (1296, 839),
        );
        assert_eq!(f, Fit::default());
        // a second monitor to the left (negative coordinates)
        let f = fit_on_monitor(
            (-1920, 0, 1920, 1080),
            Some((-200, 50)),
            (1280, 800),
            (1296, 839),
        );
        assert_eq!(f.inner, None);
        assert_eq!(
            f.position,
            Some((-1920 + (1920 - 1296) / 2, (1080 - 839) / 2))
        );
        // the system does not report positions (Wayland): size only
        let f = fit_on_monitor((0, 0, 1024, 768), None, (1280, 800), (1280, 800));
        assert_eq!((f.inner, f.position), (Some((921, 691)), None));
        assert_eq!(
            fit_on_monitor((0, 0, 0, 0), None, (1, 1), (1, 1)),
            Fit::default()
        );
    }

    #[test]
    fn percentiles() {
        assert_eq!(pct(&mut [], 0.5), None);
        assert_eq!(pct(&mut [3.0, 1.0, 2.0], 0.5), Some(2.0));
        assert_eq!(
            pct(
                &mut [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0],
                0.9
            ),
            Some(9.0)
        );
    }

    #[test]
    fn typing_text() {
        let k = text_keys("Ab1 _\n").unwrap();
        let want: Vec<(String, bool)> = [
            ("KeyA", true),
            ("KeyB", false),
            ("Digit1", false),
            ("Space", false),
            ("Minus", true),
            ("Enter", false),
        ]
        .iter()
        .map(|(c, s)| (c.to_string(), *s))
        .collect();
        assert_eq!(k, want);
        assert_eq!(text_keys(">").unwrap(), vec![("Period".to_string(), true)]);
        assert_eq!(text_keys("\"").unwrap(), vec![("Quote".to_string(), true)]);
        assert_eq!(
            text_keys("\\").unwrap(),
            vec![("Backslash".to_string(), false)]
        );
        assert!(text_keys("\u{e9}").is_err());
        let p = ppm(&[235; 4], &[128], &[128], 2, 2);
        assert!(p.starts_with(b"P6\n2 2\n255\n"));
        assert_eq!(p.len(), 11 + 12);
    }

    #[test]
    fn decoder_settings_pass_through() {
        let s = Settings {
            decoder: "avcodec".into(),
            hardware_decode: false,
            ..Default::default()
        };
        let c = decoder_config(&s);
        assert_eq!((c.backend.as_str(), c.hardware), ("avcodec", false));
        assert_eq!(c.avcodec, s.avcodec);
    }
}
