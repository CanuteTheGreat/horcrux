//! The viewer window: newest frame from the ring, scaled to fit with the
//! aspect kept, keyboard/mouse to the guest, pointer lock with raw
//! relative motion (toggle: the grab key with Ctrl+Alt held), fullscreen
//! toggle, gamepads (see `pad`).
use crate::conn::{Input, Mapped};
use crate::measure::now_ns;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalPosition;
use winit::event::{
    DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Fullscreen, Window, WindowId};

/// Viewer settings (command line / config file).
#[derive(Debug, Clone)]
pub struct ViewOpts {
    pub title: String,
    pub fullscreen: bool,
    /// Grab the pointer (relative mode) from the start (gaming).
    pub grab: bool,
    /// Key that toggles the grab while Ctrl+Alt are held (W3C code name).
    pub grab_key: String,
    /// Key that toggles fullscreen while Ctrl+Alt are held.
    pub fullscreen_key: String,
    /// Frame rate asked of the guest tools (0 = the node's default).
    pub fps: u32,
    /// Ring poll interval (microseconds).
    pub poll_us: u64,
    pub input_dir: String,
}

/// Newest frame, shared between the ring thread and the window.
#[derive(Default)]
struct Latest {
    w: u32,
    h: u32,
    px: Vec<u8>,
    seq: u64,
    shown: u64,
}

enum UserEvent {
    Frame,
}

struct App {
    opts: ViewOpts,
    input: Input,
    latest: Arc<Mutex<Latest>>,
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    grabbed: bool,
    ctrl: bool,
    alt: bool,
    seq: u32,
    /// frame rect inside the window: x, y, w, h
    view: (u32, u32, u32, u32),
}

/// W3C `KeyboardEvent.code` name of a winit key (what the node's keymap
/// takes).
pub fn code_name(k: KeyCode) -> String {
    let n = format!("{k:?}");
    match n.as_str() {
        "SuperLeft" => "MetaLeft".into(),
        "SuperRight" => "MetaRight".into(),
        _ => n,
    }
}

impl App {
    fn set_grab(&mut self, on: bool) {
        let Some(w) = &self.window else { return };
        if on {
            let ok = w
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| w.set_cursor_grab(CursorGrabMode::Confined))
                .is_ok();
            if ok {
                w.set_cursor_visible(false);
                self.grabbed = true;
            }
        } else {
            let _ = w.set_cursor_grab(CursorGrabMode::None);
            w.set_cursor_visible(true);
            self.grabbed = false;
        }
        w.set_title(&if self.grabbed {
            format!(
                "{} (pointer grabbed: Ctrl+Alt+{} releases)",
                self.opts.title, self.opts.grab_key
            )
        } else {
            self.opts.title.clone()
        });
    }

    fn redraw(&mut self) {
        let (Some(w), Some(surface)) = (&self.window, self.surface.as_mut()) else {
            return;
        };
        let size = w.inner_size();
        let (Some(ww), Some(wh)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return;
        };
        if surface.resize(ww, wh).is_err() {
            return;
        }
        let Ok(mut buf) = surface.buffer_mut() else {
            return;
        };
        let mut l = match self.latest.lock() {
            Ok(l) => l,
            Err(_) => return,
        };
        buf.fill(0);
        if l.w > 0 && l.h > 0 && l.px.len() >= (l.w * l.h * 4) as usize {
            let (fw, fh) = (l.w, l.h);
            // fit, keep aspect
            let scale = (size.width as f64 / fw as f64).min(size.height as f64 / fh as f64);
            let vw = ((fw as f64 * scale) as u32).max(1).min(size.width);
            let vh = ((fh as f64 * scale) as u32).max(1).min(size.height);
            let ox = (size.width - vw) / 2;
            let oy = (size.height - vh) / 2;
            self.view = (ox, oy, vw, vh);
            let px = &l.px;
            for y in 0..vh {
                let sy = (y as u64 * fh as u64 / vh as u64) as usize;
                let row = &px[sy * fw as usize * 4..(sy + 1) * fw as usize * 4];
                let dst = &mut buf[((oy + y) * size.width + ox) as usize
                    ..((oy + y) * size.width + ox + vw) as usize];
                if vw == fw {
                    for (x, d) in dst.iter_mut().enumerate() {
                        let o = x * 4;
                        *d = u32::from_le_bytes([row[o], row[o + 1], row[o + 2], 0]);
                    }
                } else {
                    for (x, d) in dst.iter_mut().enumerate() {
                        let o = (x as u64 * fw as u64 / vw as u64) as usize * 4;
                        *d = u32::from_le_bytes([row[o], row[o + 1], row[o + 2], 0]);
                    }
                }
            }
            l.shown = l.seq;
        }
        drop(l);
        let _ = buf.present();
    }

    /// Window position -> guest pixel.
    fn to_guest(&self, p: PhysicalPosition<f64>) -> Option<(i32, i32)> {
        let l = self.latest.lock().ok()?;
        let (ox, oy, vw, vh) = self.view;
        if vw == 0 || vh == 0 || l.w == 0 {
            return None;
        }
        let x = ((p.x - ox as f64) * l.w as f64 / vw as f64).clamp(0.0, l.w as f64 - 1.0);
        let y = ((p.y - oy as f64) * l.h as f64 / vh as f64).clamp(0.0, l.h as f64 - 1.0);
        Some((x as i32, y as i32))
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let mut attrs = Window::default_attributes().with_title(self.opts.title.clone());
        if self.opts.fullscreen {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        let w = match el.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("horcrux-viewer: no window: {e}");
                el.exit();
                return;
            }
        };
        let ctx = match softbuffer::Context::new(w.clone()) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("horcrux-viewer: {e}");
                el.exit();
                return;
            }
        };
        self.surface = softbuffer::Surface::new(&ctx, w.clone()).ok();
        self.window = Some(w);
        if self.opts.grab {
            self.set_grab(true);
        }
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, _ev: UserEvent) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, ev: WindowEvent) {
        match ev {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Resized(_) => {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::Focused(false) => {
                // never leave keys stuck down in the guest
                for k in ["ControlLeft", "AltLeft", "ShiftLeft", "MetaLeft"] {
                    self.input.key(k, false);
                }
                self.ctrl = false;
                self.alt = false;
                if self.grabbed {
                    self.set_grab(false);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                let down = event.state == ElementState::Pressed;
                match code {
                    KeyCode::ControlLeft | KeyCode::ControlRight => self.ctrl = down,
                    KeyCode::AltLeft | KeyCode::AltRight => self.alt = down,
                    _ => {}
                }
                let name = code_name(code);
                if down && self.ctrl && self.alt && name == self.opts.grab_key {
                    let g = !self.grabbed;
                    self.set_grab(g);
                    return;
                }
                if down && self.ctrl && self.alt && name == self.opts.fullscreen_key {
                    if let Some(w) = &self.window {
                        let f = w.fullscreen().is_none();
                        w.set_fullscreen(f.then_some(Fullscreen::Borderless(None)));
                    }
                    return;
                }
                if event.repeat || !self.input.allowed {
                    return;
                }
                self.input.key(&name, down);
            }
            WindowEvent::CursorMoved { position, .. } => {
                if self.grabbed || !self.input.allowed {
                    return;
                }
                if let Some((x, y)) = self.to_guest(position) {
                    self.seq = self.seq.wrapping_add(1);
                    self.input.motion(1, self.seq, x, y);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if !self.input.allowed {
                    return;
                }
                let b = match button {
                    MouseButton::Left => 0,
                    MouseButton::Middle => 1,
                    MouseButton::Right => 2,
                    MouseButton::Back => 3,
                    MouseButton::Forward => 4,
                    MouseButton::Other(_) => return,
                };
                self.input.button(b, state == ElementState::Pressed);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if !self.input.allowed {
                    return;
                }
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x, -y),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32, -p.y as f32),
                };
                self.input.wheel(dx.signum() as i32, dy.signum() as i32);
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, ev: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = ev {
            if self.grabbed && self.input.allowed {
                self.seq = self.seq.wrapping_add(1);
                self.input
                    .motion(2, self.seq, delta.0.round() as i32, delta.1.round() as i32);
            }
        }
    }
}

/// Copy new frames out of the ring and wake the window (own thread).
fn ring_thread(
    m: Arc<Mapped>,
    latest: Arc<Mutex<Latest>>,
    proxy: EventLoopProxy<UserEvent>,
    fps: u32,
    poll_us: u64,
) {
    let mut buf = Vec::new();
    let mut last = 0;
    loop {
        m.ring.reader_tick(now_ns(), fps);
        if let Some(fi) = m.ring.read_latest(last, &mut buf) {
            last = fi.seq;
            if let Ok(mut l) = latest.lock() {
                l.w = fi.width;
                l.h = fi.height;
                // tight rows for the window
                if fi.stride == fi.width * 4 {
                    std::mem::swap(&mut l.px, &mut buf);
                } else {
                    l.px.clear();
                    for y in 0..fi.height as usize {
                        let o = y * fi.stride as usize;
                        l.px.extend_from_slice(&buf[o..o + fi.width as usize * 4]);
                    }
                }
                l.seq = fi.seq;
            }
            if proxy.send_event(UserEvent::Frame).is_err() {
                return;
            }
        } else {
            std::thread::sleep(Duration::from_micros(poll_us));
        }
    }
}

/// Run the window until it is closed.
pub fn run(opts: ViewOpts, ring: Mapped, input: Input, gamepad: bool) -> Result<(), String> {
    let el = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|e| format!("no display server (Wayland/X11): {e}"))?;
    el.set_control_flow(ControlFlow::Wait);
    let latest = Arc::new(Mutex::new(Latest::default()));
    let ring = Arc::new(ring);
    {
        let (r, l, p) = (ring.clone(), latest.clone(), el.create_proxy());
        let (fps, poll) = (opts.fps, opts.poll_us);
        std::thread::spawn(move || ring_thread(r, l, p, fps, poll));
    }
    if gamepad && input.allowed {
        let i = input.clone();
        let n = crate::pad::spawn(&opts.input_dir, move |pkt| i.pad(pkt));
        if n > 0 {
            eprintln!("horcrux-viewer: {n} gamepad(s) forwarded");
        }
    }
    let mut app = App {
        opts,
        input,
        latest,
        window: None,
        surface: None,
        grabbed: false,
        ctrl: false,
        alt: false,
        seq: 0,
        view: (0, 0, 0, 0),
    };
    el.run_app(&mut app).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names_follow_the_web_codes() {
        assert_eq!(code_name(KeyCode::KeyA), "KeyA");
        assert_eq!(code_name(KeyCode::Digit1), "Digit1");
        assert_eq!(code_name(KeyCode::SuperLeft), "MetaLeft");
        assert_eq!(code_name(KeyCode::ArrowUp), "ArrowUp");
        assert_eq!(code_name(KeyCode::Enter), "Enter");
        assert_eq!(code_name(KeyCode::ShiftRight), "ShiftRight");
    }
}
