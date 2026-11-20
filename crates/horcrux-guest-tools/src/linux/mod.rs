//! Linux guest tools: FFmpeg capture (kmsgrab on the console/any KMS
//! desktop, x11grab in an X session) and encode (VA-API, NVENC, libx264),
//! uinput keyboard/mouse/gamepad, clipboard and display layout through the
//! active graphical session (wl-clipboard / xclip, xrandr), PulseAudio /
//! PipeWire audio to Opus.
mod session;
pub mod shm;
mod uinput;
mod video;

use crate::agent::{Out, Platform};
use crate::config::Config;
use horcrux_guest_proto::{
    self as proto, Caps, Clipboard, Hello, Input, Layout, Msg, StartAudio, StartVideo, VideoStatus,
};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub struct Linux {
    cfg: Config,
    video: Option<video::Running>,
    input: Option<uinput::Devices>,
    input_failed: bool,
    clip_last: Arc<Mutex<String>>,
    clip_stop: Arc<AtomicBool>,
    audio: Option<Child>,
}

impl Linux {
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            video: None,
            input: None,
            input_failed: false,
            clip_last: Arc::new(Mutex::new(String::new())),
            clip_stop: Arc::new(AtomicBool::new(false)),
            audio: None,
        }
    }
}

impl Drop for Linux {
    fn drop(&mut self) {
        self.clip_stop.store(true, Ordering::Relaxed);
    }
}

fn os_version() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines().find_map(|l| {
                l.strip_prefix("PRETTY_NAME=")
                    .map(|v| v.trim_matches('"').to_string())
            })
        })
        .unwrap_or_default()
}

impl Platform for Linux {
    fn hello(&self) -> Hello {
        let enc = video::probe_encoders(&self.cfg);
        let capture = video::capture_methods(&self.cfg);
        let sess = session::find();
        let has = |p: &str| session::which(p);
        let clipboard = sess.as_ref().map(|s| {
            if s.wayland.is_some() {
                has("wl-copy") && has("wl-paste")
            } else {
                has("xclip")
            }
        });
        let resize = sess
            .as_ref()
            .map(|s| s.display.is_some() && has("xrandr"))
            .unwrap_or(false);
        let uinput = std::path::Path::new("/dev/uinput").exists();
        Hello {
            v: proto::VERSION,
            os: "linux".into(),
            os_version: os_version(),
            agent_version: env!("CARGO_PKG_VERSION").into(),
            caps: Caps {
                codecs: if enc.is_empty() || capture.is_empty() {
                    vec![]
                } else {
                    vec!["h264".into()]
                },
                encoders: enc,
                capture,
                clipboard: clipboard.unwrap_or(false),
                resize,
                multi_monitor: resize,
                relative_mouse: uinput,
                gamepad: uinput,
                audio: has(&self.cfg.ffmpeg) || std::path::Path::new(&self.cfg.ffmpeg).exists(),
                virtual_display: false,
                chroma444: true,
                hdr: false,
                shm_frames: shm::available(&self.cfg),
            },
        }
    }

    fn start_video(&mut self, cfg: &StartVideo, out: Out) -> VideoStatus {
        self.stop_video();
        match video::start(&self.cfg, cfg, out) {
            Ok((r, st)) => {
                self.video = Some(r);
                st
            }
            Err(e) => VideoStatus {
                error: Some(e),
                ..Default::default()
            },
        }
    }

    fn stop_video(&mut self) {
        if let Some(v) = self.video.take() {
            v.stop();
        }
    }

    fn keyframe(&mut self) {
        if let Some(v) = self.video.as_mut() {
            v.keyframe();
        }
    }

    fn set_clipboard(&mut self, c: &Clipboard) {
        if let Ok(mut l) = self.clip_last.lock() {
            *l = c.text.clone();
        }
        if let Some(s) = session::find() {
            if let Err(e) = s.set_clipboard(&c.text) {
                eprintln!("clipboard: {e}");
            }
        }
    }

    fn watch_clipboard(&mut self, out: Out, max_bytes: usize) {
        let last = self.clip_last.clone();
        let stop = self.clip_stop.clone();
        let every = std::time::Duration::from_millis(self.cfg.clipboard_poll_ms);
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(every);
                let Some(s) = session::find() else { continue };
                let Ok(text) = s.get_clipboard() else {
                    continue;
                };
                if text.is_empty() || text.len() > max_bytes {
                    continue;
                }
                let changed = match last.lock() {
                    Ok(mut l) if *l != text => {
                        *l = text.clone();
                        true
                    }
                    _ => false,
                };
                if changed {
                    let m = Msg::Clipboard(Clipboard {
                        mime: "text/plain".into(),
                        text,
                    });
                    if out.send(m).is_err() {
                        break;
                    }
                }
            }
        });
    }

    fn set_layout(&mut self, l: &Layout) -> Option<Layout> {
        let s = session::find()?;
        if let Err(e) = s.apply_layout(l) {
            eprintln!("layout: {e}");
        }
        s.layout().ok()
    }

    fn input(&mut self, i: &Input) {
        if self.input.is_none() && !self.input_failed {
            match uinput::Devices::create(&self.cfg.input_device_name) {
                Ok(d) => self.input = Some(d),
                Err(e) => {
                    self.input_failed = true;
                    eprintln!("uinput: {e}");
                }
            }
        }
        if let Some(d) = self.input.as_mut() {
            let size = video::current_size(&self.video);
            if let Err(e) = d.inject(i, size) {
                eprintln!("uinput: {e}");
            }
        }
    }

    fn start_audio(&mut self, a: &StartAudio, out: Out) -> Result<(), String> {
        self.stop_audio();
        let mut child = Command::new(&self.cfg.ffmpeg)
            .args(audio_args(&self.cfg, a))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("{}: {e}", self.cfg.ffmpeg))?;
        let mut stdout = child.stdout.take().ok_or("no stdout")?;
        std::thread::spawn(move || {
            use std::io::Read;
            let mut ogg = proto::OggReader::default();
            let mut buf = vec![0u8; 16384];
            while let Ok(n) = stdout.read(&mut buf) {
                if n == 0 {
                    break;
                }
                for pkt in ogg.push(&buf[..n]) {
                    let m = Msg::Audio(proto::AudioPacket {
                        capture_us: crate::agent::now_us(),
                        data: pkt,
                    });
                    if out.send(m).is_err() {
                        return;
                    }
                }
            }
        });
        self.audio = Some(child);
        Ok(())
    }

    fn stop_audio(&mut self) {
        if let Some(mut c) = self.audio.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// FFmpeg arguments for audio capture: PulseAudio/PipeWire source -> Opus
/// in Ogg on stdout (low-delay application, short frames, one packet per
/// page so nothing waits for a page to fill).
pub fn audio_args(cfg: &Config, a: &StartAudio) -> Vec<String> {
    let frame = if a.frame_ms == 0 { 10 } else { a.frame_ms };
    let rate = if a.sample_rate == 0 {
        48000
    } else {
        a.sample_rate
    };
    let ch = a.channels.max(1);
    let br = if a.bitrate_kbps == 0 {
        cfg.audio_bitrate_kbps
    } else {
        a.bitrate_kbps
    };
    [
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
        "-fflags",
        "nobuffer",
        "-f",
        "pulse",
        "-fragment_size",
        "1920",
        "-i",
        &cfg.audio_source,
        "-ac",
        &ch.to_string(),
        "-ar",
        &rate.to_string(),
        "-c:a",
        "libopus",
        "-application",
        "lowdelay",
        "-frame_duration",
        &frame.to_string(),
        "-b:a",
        &format!("{br}k"),
        "-page_duration",
        &(frame * 1000).to_string(),
        "-flush_packets",
        "1",
        "-f",
        "ogg",
        "pipe:1",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_is_low_delay_opus() {
        let a = audio_args(
            &Config::default(),
            &StartAudio {
                codec: "opus".into(),
                sample_rate: 48000,
                channels: 2,
                bitrate_kbps: 96,
                frame_ms: 10,
            },
        )
        .join(" ");
        assert!(a.contains("-f pulse") && a.contains("-i default"), "{a}");
        assert!(
            a.contains("-c:a libopus -application lowdelay -frame_duration 10 -b:a 96k"),
            "{a}"
        );
        assert!(a.ends_with("-f ogg pipe:1"), "{a}");
    }
}
