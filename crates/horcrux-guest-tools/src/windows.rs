//! Windows guest tools: DXGI Desktop Duplication capture (GDI fallback),
//! Media Foundation H.264 encode (hardware MFTs first, the Microsoft
//! software MFT as fallback), SendInput, clipboard, display layout, and the
//! service wrapper that runs the agent in the console session.
mod audio;
mod capture;
mod clip;
mod convert;
mod input;
mod layout;
mod mf;
pub mod port;
pub mod service;
pub mod shm;
mod video;

use crate::agent::{Out, Platform};
use crate::config::Config;
use horcrux_guest_proto::{
    self as proto, Caps, Clipboard, Hello, Input, Layout, StartAudio, StartVideo, VideoStatus,
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
        Self {
            cfg,
            clip,
            video: None,
            audio: None,
        }
    }
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
        let (encoders, capture) = video::probe(&self.cfg);
        let mons = layout::monitors();
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
                // No driver-free virtual gamepad exists on Windows.
                gamepad: false,
                // WASAPI loopback of a render endpoint (probed).
                audio: audio::probe(&self.cfg),
                virtual_display: false,
                chroma444: false,
                hdr: false,
                shm_frames: shm::available(&self.cfg),
                // SendInput cannot reach the secure desktop (sign-in,
                // UAC, Ctrl+Alt+Del): keys and the pointer stay on QEMU's
                // devices, which reach every desktop.
                keyboard: false,
                absolute_pointer: false,
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

    fn set_layout(&mut self, l: &Layout) -> Option<Layout> {
        for n in layout::apply(l, self.cfg.layout_settle_ms) {
            eprintln!("horcrux-guest-agent: layout: {n}");
        }
        Some(layout::layout())
    }

    fn input(&mut self, i: &Input) {
        if let Input::Gamepad(_) = i {
            static ONCE: AtomicBool = AtomicBool::new(false);
            if !ONCE.swap(true, Ordering::SeqCst) {
                eprintln!(
                    "horcrux-guest-agent: gamepad input ignored: Windows has no driver-free virtual gamepad \
                     (the Horcrux virtual HID gamepad driver is a later phase)"
                );
            }
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
