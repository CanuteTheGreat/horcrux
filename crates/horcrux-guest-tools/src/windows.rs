//! Windows guest tools: DXGI Desktop Duplication capture (GDI fallback),
//! Media Foundation H.264 encode (hardware MFTs first, the Microsoft
//! software MFT as fallback), SendInput, clipboard, display layout, and the
//! service wrapper that runs the agent in the console session.
mod audio;
mod capture;
mod clip;
mod convert;
pub mod drivers;
pub mod idd;
mod input;
mod layout;
mod mf;
mod pad;
pub mod port;
pub mod service;
mod video;

use crate::agent::{Out, Platform};
use crate::config::Config;
use crate::hxdrv;
use horcrux_guest_proto::{
    self as proto, Caps, Clipboard, Gamepad, Hello, HostHello, Input, Layout, Monitor, StartAudio,
    StartVideo, VideoStatus,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

pub fn open_port(path: &str) -> Result<crate::Port, String> {
    port::open(path)
}

pub struct Windows {
    cfg: Config,
    clip: Sender<clip::Cmd>,
    video: Option<video::Video>,
    audio: Option<audio::Audio>,
    /// The node asked for virtual monitors (HostHello.virtual_display).
    host_virtual: bool,
    pads: Option<pad::Pads>,
}

impl Windows {
    pub fn new(cfg: Config) -> Self {
        // Without this, coordinates and sizes are virtualised on scaled displays.
        // SAFETY: process-wide DPI setting; failure (already set) is harmless.
        unsafe {
            let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
                windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            );
        }
        let clip = clip::spawn(cfg.clipboard_poll_ms);
        let w = Self {
            cfg,
            clip,
            video: None,
            audio: None,
            host_virtual: false,
            pads: None,
        };
        w.restore_virtual_modes();
        w
    }

    /// Windows picks its own mode for a virtual monitor that comes back at
    /// boot (persisted monitors); put each back in the mode it was asked for.
    fn restore_virtual_modes(&self) {
        if self.cfg.virtual_display == "off" || !idd::present() {
            return;
        }
        let Ok(info) = idd::Idd::open().and_then(|d| d.info()) else {
            return;
        };
        let act = info.active();
        let off = act.iter().any(|m| {
            m.width > 0
                && m.pref_width > 0
                && (m.width, m.height, m.refresh_mhz.div_ceil(1000))
                    != (
                        m.pref_width,
                        m.pref_height,
                        m.pref_refresh_mhz.div_ceil(1000),
                    )
        });
        if !off {
            return;
        }
        let l = Layout {
            monitors: act
                .iter()
                .filter(|m| m.pref_width > 0)
                .map(|m| Monitor {
                    width: m.pref_width,
                    height: m.pref_height,
                    refresh_hz: m.pref_refresh_mhz.div_ceil(1000),
                    ..Default::default()
                })
                .collect(),
        };
        for n in layout::apply_to(&l, self.cfg.layout_settle_ms, true) {
            eprintln!("horcrux-guest-agent: layout (restore): {n}");
        }
    }

    /// Do layouts go to Horcrux virtual monitors?
    fn use_virtual(&self) -> bool {
        match self.cfg.virtual_display.as_str() {
            "on" => true,
            "off" => false,
            _ => self.host_virtual,
        }
    }

    /// Size the Horcrux virtual monitors to the layout (creating/removing
    /// monitors as needed) and give them the asked modes.
    fn set_virtual_layout(&mut self, l: &Layout) -> Result<(), String> {
        let dev = idd::Idd::open()?;
        let share = self.cfg.capture == "idd";
        let flags = if self.cfg.virtual_display_persist {
            hxdrv::SET_PERSIST
        } else {
            0
        };
        let req = hxdrv::idd_request(
            &l.monitors,
            &hxdrv::parse_modes(&self.cfg.virtual_display_modes),
            self.cfg.virtual_display_default_hz,
            self.cfg.virtual_display_max_monitors,
            flags,
            share,
        )?;
        dev.set(&req)?;
        let want = req.count as usize;
        let info = dev.wait_active(want, self.cfg.virtual_display_wait_ms)?;
        let active = info.active().iter().filter(|m| m.width > 0).count();
        if active < want {
            eprintln!(
                "horcrux-guest-agent: layout: {active} of {want} virtual monitors active after {} ms",
                self.cfg.virtual_display_wait_ms
            );
        }
        if self.cfg.virtual_display_primary && want > 0 {
            if let Err(e) = layout::make_virtual_primary(self.cfg.layout_settle_ms) {
                eprintln!("horcrux-guest-agent: layout: {e}");
            }
        }
        for n in layout::apply_to(l, self.cfg.layout_settle_ms, true) {
            eprintln!("horcrux-guest-agent: layout: {n}");
        }
        Ok(())
    }

    fn gamepad(&mut self, g: &Gamepad) {
        if self.pads.as_ref().map(|p| p.count()).unwrap_or(0) == 0 {
            self.pads = Some(pad::Pads::open());
        }
        let Some(p) = &self.pads else { return };
        if p.count() == 0 {
            static ONCE: AtomicBool = AtomicBool::new(false);
            if !ONCE.swap(true, Ordering::SeqCst) {
                eprintln!(
                    "horcrux-guest-agent: gamepad input ignored: no Horcrux virtual gamepad \
                     (install the guest drivers: --drivers install)"
                );
            }
            return;
        }
        if let Err(e) = p.write(g) {
            eprintln!("horcrux-guest-agent: {e}");
            self.pads = None; // reopen next time (device re-enumerated)
        }
    }
}

/// `--drivers`, `--layout`, `--gamepad-test`: run once and return the exit code.
pub fn one_shot(cfg: &Config) -> Option<i32> {
    let report = |r: Result<(), String>| match r {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("horcrux-guest-agent: {e}");
            1
        }
    };
    if let Some(a) = &cfg.drivers {
        return Some(match a.as_str() {
            "install" => report(drivers::install(cfg)),
            "remove" => report(drivers::remove(cfg)),
            _ => {
                println!("{}", drivers::status());
                0
            }
        });
    }
    if let Some(spec) = &cfg.layout {
        let monitors: Vec<Monitor> = hxdrv::parse_modes(spec)
            .iter()
            .map(|m| Monitor {
                width: m.width,
                height: m.height,
                refresh_hz: m.refresh_mhz.div_ceil(1000),
                ..Default::default()
            })
            .collect();
        if monitors.is_empty() {
            eprintln!("horcrux-guest-agent: --layout: no valid WxH@HZ in {spec}");
            return Some(2);
        }
        let mut w = Windows::new(cfg.clone());
        let now = w.set_layout(&Layout { monitors });
        println!("{}", serde_json::to_string_pretty(&now).unwrap_or_default());
        println!("{}", drivers::status());
        return Some(0);
    }
    if cfg.gamepad_test {
        let pads = pad::Pads::open();
        if pads.count() == 0 {
            eprintln!("horcrux-guest-agent: no Horcrux virtual gamepad");
            return Some(1);
        }
        // A held, then d-pad right + left stick right/down + right trigger, then released.
        let steps: [(&str, Gamepad, u64); 3] = [
            (
                "A",
                Gamepad {
                    buttons: 1,
                    ..Default::default()
                },
                3000,
            ),
            (
                "dpad-right+lx+ly+rt",
                Gamepad {
                    buttons: 1 << 15,
                    lx: 32767,
                    ly: 16384,
                    rt: 255,
                    ..Default::default()
                },
                3000,
            ),
            ("released", Gamepad::default(), 500),
        ];
        for (name, g, ms) in steps {
            if let Err(e) = pads.write(&g) {
                eprintln!("horcrux-guest-agent: {e}");
                return Some(1);
            }
            println!("gamepad-test: {name}");
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
        return Some(0);
    }
    None
}

#[repr(C)]
struct OsVersionInfo {
    size: u32,
    major: u32,
    minor: u32,
    build: u32,
    platform: u32,
    csd: [u16; 128],
}

#[link(name = "ntdll")]
extern "system" {
    fn RtlGetVersion(info: *mut OsVersionInfo) -> i32;
}

fn os_version() -> String {
    let mut v = OsVersionInfo {
        size: std::mem::size_of::<OsVersionInfo>() as u32,
        major: 0,
        minor: 0,
        build: 0,
        platform: 0,
        csd: [0; 128],
    };
    // SAFETY: v is a correctly sized OSVERSIONINFOW.
    if unsafe { RtlGetVersion(&mut v) } == 0 {
        format!("{}.{}.{}", v.major, v.minor, v.build)
    } else {
        String::new()
    }
}

impl Platform for Windows {
    fn hello(&self) -> Hello {
        let (encoders, mut capture) = video::probe(&self.cfg);
        let mons = layout::monitors();
        let virtual_display = idd::present();
        if virtual_display {
            capture.push("idd".into());
        }
        let gamepad = pad::Pads::open().count() > 0;
        Hello {
            v: proto::VERSION,
            os: "windows".into(),
            os_version: os_version(),
            agent_version: env!("CARGO_PKG_VERSION").into(),
            caps: Caps {
                codecs: if encoders.is_empty() {
                    vec![]
                } else {
                    vec!["h264".into()]
                },
                encoders,
                capture,
                clipboard: true,
                resize: true,
                multi_monitor: mons.len() > 1,
                relative_mouse: true,
                // the Horcrux virtual gamepad driver (hxpad)
                gamepad,
                // WASAPI loopback of a render endpoint (probed).
                audio: audio::probe(&self.cfg),
                virtual_display,
                chroma444: false,
                hdr: false,
            },
        }
    }

    fn start_video(&mut self, cfg: &StartVideo, out: Out) -> VideoStatus {
        if let Some(mut v) = self.video.take() {
            v.stop();
        }
        let (v, st) = video::Video::start(&self.cfg, cfg, out);
        self.video = v;
        st
    }

    fn stop_video(&mut self) {
        if let Some(mut v) = self.video.take() {
            v.stop();
        }
    }

    fn keyframe(&mut self) {
        if let Some(v) = &self.video {
            v.keyframe();
        }
    }

    fn set_clipboard(&mut self, c: &Clipboard) {
        let _ = self.clip.send(clip::Cmd::Set(c.text.clone()));
    }

    fn watch_clipboard(&mut self, out: Out, max_bytes: usize) {
        let _ = self.clip.send(clip::Cmd::Watch(out, max_bytes));
    }

    fn host_hello(&mut self, h: &HostHello) {
        self.host_virtual = h.virtual_display;
    }

    fn set_layout(&mut self, l: &Layout) -> Option<Layout> {
        if self.use_virtual() {
            match self.set_virtual_layout(l) {
                Ok(()) => return Some(layout::layout()),
                Err(e) => eprintln!("horcrux-guest-agent: layout: virtual display: {e}"),
            }
        }
        for n in layout::apply(l, self.cfg.layout_settle_ms) {
            eprintln!("horcrux-guest-agent: layout: {n}");
        }
        Some(layout::layout())
    }

    fn input(&mut self, i: &Input) {
        if let Input::Gamepad(g) = i {
            self.gamepad(g);
            return;
        }
        input::inject(i, self.cfg.wheel_scale_pct);
    }

    fn start_audio(&mut self, a: &StartAudio, out: Out) -> Result<(), String> {
        self.stop_audio();
        self.audio = Some(audio::Audio::start(&self.cfg, a, out)?);
        Ok(())
    }

    fn stop_audio(&mut self) {
        if let Some(mut a) = self.audio.take() {
            a.stop();
        }
    }
}
