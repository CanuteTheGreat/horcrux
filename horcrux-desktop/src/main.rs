//! Horcrux Console desktop client.
//!
//! The same protocol as the web client: log in, get a console ticket bound
//! to the API WebSocket connection, offer WebRTC on that socket, then
//! H.264 video plus the `ctl`/`keys`/`input`/`pad`/... data channels over
//! the direct path or the node's relay. The window adds what a browser
//! cannot do well: raw relative mouse motion while captured, gamepads read
//! from the system, and frames shown the moment they are decoded at the
//! monitor's refresh rate.
mod config;
mod gamepad;
mod net;
mod session;

use clap::Parser;
use config::{Args, Settings};
use horcrux_media::avcodec::Decoder;
use horcrux_media::yuv;
use serde_json::{json, Value};
use session::{FromSession, ToSession};
use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;
use winit::application::ApplicationHandler;
use winit::event::{
    DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode, PhysicalKey};
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

fn password(s: &Settings) -> Result<String, String> {
    if let Ok(p) = std::env::var(&s.password_env) {
        return Ok(p);
    }
    rpassword::prompt_password(format!("Password for {}@{}: ", s.user, s.node))
        .map_err(|e| e.to_string())
}

fn pct(v: &mut [f64], p: f64) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some(v[((v.len() - 1) as f64 * p).round() as usize])
}

#[derive(Default)]
struct Counters {
    decoded: u64,
    errors: u64,
    size: (usize, usize),
    connected_ms: Option<u64>,
    last_err: Option<String>,
}

/// No window: connect, decode, measure, report JSON (CI e2e).
fn headless(
    s: &Settings,
    args: &Args,
    rt: &tokio::runtime::Runtime,
    api: &net::Api,
    st: session::Started,
) -> Result<Value, String> {
    let mut dec = Decoder::open(&s.avcodec, &s.avutil)?;
    let t0 = Instant::now();
    let mut c = Counters::default();
    let step =
        |c: &mut Counters, dec: &mut Decoder, wait: Duration| -> Result<Option<Instant>, String> {
            match st.from.recv_timeout(wait) {
                Ok(FromSession::Frame(au, at)) => {
                    match dec.decode(&au, |p| c.size = (p.width, p.height)) {
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
    let _ = st.to.send(ToSession::Bye);
    // the session ends on its own; give the bye a moment
    rt.block_on(async { tokio::time::sleep(Duration::from_millis(300)).await });
    let mut l2 = lat.clone();
    Ok(json!({
        "session": st.session,
        "path": st.path,
        "decoder": format!("{} + {}", dec.libraries.0, dec.libraries.1),
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

enum UserEvent {
    Session(FromSession),
}

struct App {
    s: Settings,
    to: UnboundedSender<ToSession>,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    dec: Decoder,
    /// last decoded picture (I420) and its size
    pic: (Vec<u8>, Vec<u8>, Vec<u8>, usize, usize),
    shown: yuv::Rect,
    captured: bool,
    relative_ok: bool,
    last_click: Option<Instant>,
    status: String,
    title_at: Instant,
    frames: u64,
}

impl App {
    fn set_capture(&mut self, on: bool) {
        let Some(w) = self.window.as_ref() else {
            return;
        };
        if on {
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
        if let Some(w) = &self.window {
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
    }

    fn present(&mut self) {
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
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let mut attrs = Window::default_attributes()
            .with_title(format!("{} - Horcrux Console", self.s.vm))
            .with_inner_size(winit::dpi::LogicalSize::new(self.s.width, self.s.height));
        if self.s.fullscreen {
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
        let hz = if self.s.refresh_hz > 0 {
            self.s.refresh_hz
        } else {
            w.current_monitor()
                .and_then(|m| m.refresh_rate_millihertz())
                .map(|m| m.div_ceil(1000))
                .unwrap_or(60)
        };
        let _ = self
            .to
            .send(ToSession::Ctl(json!({"t": "display", "refresh_hz": hz})));
        let ctx = softbuffer::Context::new(w.clone()).expect("display connection");
        self.surface = Some(softbuffer::Surface::new(&ctx, w.clone()).expect("window surface"));
        self.window = Some(w);
        self.status = "connecting".into();
        self.title();
    }

    fn user_event(&mut self, el: &ActiveEventLoop, ev: UserEvent) {
        let UserEvent::Session(m) = ev;
        match m {
            FromSession::Frame(au, _) => {
                let mut got = None;
                match self.dec.decode(&au, |p| {
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
                    Err(_) => {
                        let _ = self.to.send(ToSession::Keyframe);
                    }
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
                el.exit();
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, ev: WindowEvent) {
        match ev {
            WindowEvent::CloseRequested => {
                let _ = self.to.send(ToSession::Bye);
                el.exit();
            }
            WindowEvent::RedrawRequested | WindowEvent::Resized(_) => self.present(),
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
                    let _ = self
                        .to
                        .send(ToSession::Key(code, event.state == ElementState::Pressed));
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if !self.captured {
                    if let Some((x, y)) = self.guest_xy(position.x, position.y) {
                        let _ = self.to.send(ToSession::Motion(1, x, y));
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
                    self.last_click = Some(Instant::now());
                    self.set_capture(true);
                }
                let _ = self
                    .to
                    .send(ToSession::Button(b, state == ElementState::Pressed));
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x.signum() as i32, -y.signum() as i32),
                    MouseScrollDelta::PixelDelta(p) => (p.x.signum() as i32, -p.y.signum() as i32),
                };
                let _ = self.to.send(ToSession::Wheel(dx, dy));
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, ev: DeviceEvent) {
        // raw, unaccelerated motion while the mouse is captured
        if let DeviceEvent::MouseMotion { delta } = ev {
            if self.captured {
                let _ = self.to.send(ToSession::Motion(
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

fn main() {
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
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let pw = password(&s)?;
    let t = net::tls(&s)?;
    let (api, st) = rt.block_on(async {
        let api = net::Api::login(&s, &t, &pw).await?;
        let st = session::start(&s, &api).await?;
        Ok::<_, String>((api, st))
    })?;
    drop(pw);
    if args.headless {
        let report = headless(&s, &args, &rt, &api, st)?;
        println!("{report}");
        return Ok(());
    }
    let dec = Decoder::open(&s.avcodec, &s.avutil)?;
    let el = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|e| e.to_string())?;
    forward(st.from, el.create_proxy());
    if s.gamepads {
        let to = st.to.clone();
        let n = gamepad::spawn(move |p| {
            let _ = to.send(ToSession::Pad(p.bytes()));
        });
        tracing::info!("{n} gamepad(s)");
    }
    let mut app = App {
        s,
        to: st.to,
        window: None,
        surface: None,
        dec,
        pic: (vec![], vec![], vec![], 0, 0),
        shown: yuv::Rect::default(),
        captured: false,
        relative_ok: false,
        last_click: None,
        status: String::new(),
        title_at: Instant::now(),
        frames: 0,
    };
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
}
