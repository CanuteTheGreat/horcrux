//! Windows guest tools (DXGI Desktop Duplication / Horcrux virtual display
//! capture, Media Foundation hardware encode with a software fallback,
//! SendInput, clipboard, display layout, WASAPI loopback audio).
//!
//! This file is the placeholder the Windows implementation replaces; it
//! keeps the crate building for `x86_64-pc-windows-gnu`.
use crate::agent::{Out, Platform};
use crate::config::Config;
use horcrux_guest_proto::{self as proto, Clipboard, Hello, Input, Layout, StartAudio, StartVideo, VideoStatus};

pub fn open_port(path: &str) -> Result<crate::Port, String> {
    let f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| format!("{path}: {e}"))?;
    let r = f.try_clone().map_err(|e| e.to_string())?;
    Ok((Box::new(r), Box::new(f)))
}

pub struct Windows {
    #[allow(dead_code)]
    cfg: Config,
}

impl Windows {
    pub fn new(cfg: Config) -> Self {
        Self { cfg }
    }
}

impl Platform for Windows {
    fn hello(&self) -> Hello {
        Hello {
            v: proto::VERSION,
            os: "windows".into(),
            agent_version: env!("CARGO_PKG_VERSION").into(),
            ..Default::default()
        }
    }
    fn start_video(&mut self, _: &StartVideo, _: Out) -> VideoStatus {
        VideoStatus {
            error: Some("not implemented yet".into()),
            ..Default::default()
        }
    }
    fn stop_video(&mut self) {}
    fn keyframe(&mut self) {}
    fn set_clipboard(&mut self, _: &Clipboard) {}
    fn watch_clipboard(&mut self, _: Out, _: usize) {}
    fn set_layout(&mut self, _: &Layout) -> Option<Layout> {
        None
    }
    fn input(&mut self, _: &Input) {}
    fn start_audio(&mut self, _: &StartAudio, _: Out) -> Result<(), String> {
        Err("not implemented yet".into())
    }
    fn stop_audio(&mut self) {}
}
