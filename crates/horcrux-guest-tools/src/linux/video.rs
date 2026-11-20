//! Capture + encode through FFmpeg: the frames never leave the guest
//! unencoded. One child per StartVideo; a keyframe request restarts it
//! (FFmpeg's CLI cannot be told to emit an IDR mid-stream), rate-limited.
use crate::agent::{now_us, Out};
use crate::config::Config;
use horcrux_guest_proto::{AudSplitter, Msg, StartVideo, VideoPacket, VideoStatus};
use std::io::Read;
use std::os::unix::io::AsRawFd;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capture {
    Kms,
    X11,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Vaapi,
    Nvenc,
    Software,
}

impl Backend {
    fn name(&self) -> &'static str {
        match self {
            Backend::Vaapi => "vaapi",
            Backend::Nvenc => "nvenc",
            Backend::Software => "software",
        }
    }
}

/// Encoders this guest's FFmpeg can use (a device must exist too).
pub fn probe_encoders(cfg: &Config) -> Vec<String> {
    let Ok(o) = Command::new(&cfg.ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .output()
    else {
        return vec![];
    };
    let list = String::from_utf8_lossy(&o.stdout).to_string();
    let mut v = Vec::new();
    if list.contains("h264_nvenc") && std::path::Path::new("/dev/nvidiactl").exists() {
        v.push("nvenc".to_string());
    }
    if list.contains("h264_vaapi") && std::path::Path::new(&cfg.vaapi_device).exists() {
        v.push("vaapi".to_string());
    }
    if list.contains("libx264") {
        v.push("software".to_string());
    }
    v
}

pub fn capture_methods(cfg: &Config) -> Vec<String> {
    let mut v = Vec::new();
    // kmsgrab needs CAP_SYS_ADMIN (the agent runs as root) and a KMS card.
    if std::path::Path::new(&cfg.drm_card).exists() && unsafe { libc::geteuid() } == 0 {
        v.push("kmsgrab".to_string());
    }
    if super::session::find()
        .map(|s| s.display.is_some())
        .unwrap_or(false)
    {
        v.push("x11grab".to_string());
    }
    v
}

fn pick(cfg: &Config) -> Result<(Capture, Backend), String> {
    let caps = capture_methods(cfg);
    let capture = match cfg.capture.as_str() {
        "kmsgrab" => Capture::Kms,
        "x11grab" => Capture::X11,
        _ if caps.iter().any(|c| c == "kmsgrab") => Capture::Kms,
        _ if caps.iter().any(|c| c == "x11grab") => Capture::X11,
        _ => return Err("no capture method: needs root + a KMS device, or an X session".into()),
    };
    let enc = probe_encoders(cfg);
    let backend = match cfg.encoder.as_str() {
        "vaapi" => Backend::Vaapi,
        "nvenc" => Backend::Nvenc,
        "software" => Backend::Software,
        _ if enc.iter().any(|e| e == "nvenc") => Backend::Nvenc,
        _ if enc.iter().any(|e| e == "vaapi") => Backend::Vaapi,
        _ if enc.iter().any(|e| e == "software") => Backend::Software,
        _ => {
            return Err(
                "FFmpeg has no usable H.264 encoder (libx264, h264_vaapi, h264_nvenc)".into(),
            )
        }
    };
    Ok((capture, backend))
}

/// FFmpeg arguments for one capture/encode pipeline. Low-delay settings
/// from the node: no B-frames, CBR with a one-frame buffer when
/// `low_delay`, GOP from `keyframe_interval_frames`, an access unit
/// delimiter in front of every frame so the reader can split frames.
pub fn ffmpeg_args(
    cfg: &Config,
    sv: &StartVideo,
    capture: Capture,
    backend: Backend,
    display: Option<&str>,
) -> Vec<String> {
    let fps = sv.fps.clamp(1, 360);
    let mut a: Vec<String> = [
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
        "-fflags",
        "nobuffer",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let mut push = |v: &[&str]| a.extend(v.iter().map(|s| s.to_string()));
    match capture {
        Capture::Kms => push(&[
            "-device",
            &cfg.drm_card,
            "-framerate",
            &fps.to_string(),
            "-f",
            "kmsgrab",
            "-i",
            "-",
        ]),
        Capture::X11 => push(&[
            "-draw_mouse",
            "1",
            "-framerate",
            &fps.to_string(),
            "-f",
            "x11grab",
            "-i",
            display.unwrap_or(":0"),
        ]),
    }
    let scale = (sv.width > 0 && sv.height > 0).then(|| format!("{}:{}", sv.width, sv.height));
    let chroma444 = sv.chroma == "444" && backend == Backend::Software;
    let mut vf: Vec<String> = Vec::new();
    match (capture, backend) {
        (Capture::Kms, Backend::Vaapi) => {
            vf.push("hwmap=derive_device=vaapi".into());
            vf.push(match &scale {
                Some(s) => format!("scale_vaapi={s}:format=nv12"),
                None => "scale_vaapi=format=nv12".into(),
            });
        }
        (Capture::Kms, _) => {
            vf.push("hwdownload".into());
            vf.push("format=bgr0".into());
        }
        (Capture::X11, Backend::Vaapi) => {
            vf.push("format=nv12".into());
            vf.push("hwupload".into());
        }
        _ => {}
    }
    if backend != Backend::Vaapi {
        if let Some(s) = &scale {
            vf.push(format!("scale={s}"));
        }
        vf.push(format!(
            "format={}",
            if chroma444 { "yuv444p" } else { "yuv420p" }
        ));
    }
    if capture == Capture::X11 && backend == Backend::Vaapi {
        push(&["-vaapi_device", &cfg.vaapi_device]);
    }
    if !vf.is_empty() {
        push(&["-vf", &vf.join(",")]);
    }
    let br = format!("{}k", sv.bitrate_kbps.max(100));
    let max = format!("{}k", sv.max_bitrate_kbps.max(sv.bitrate_kbps).max(100));
    // one frame of buffer = lowest delay CBR
    let buf = format!("{}k", (sv.bitrate_kbps.max(100) / fps).max(1));
    let gop = sv.keyframe_interval_frames.max(1).to_string();
    match backend {
        Backend::Software => {
            push(&[
                "-c:v",
                "libx264",
                "-preset",
                &cfg.x264_preset,
                "-tune",
                "zerolatency",
            ]);
            if chroma444 {
                push(&["-profile:v", "high444"]);
            }
            if sv.intra_refresh {
                push(&["-x264-params", "intra-refresh=1"]);
            }
        }
        Backend::Vaapi => push(&[
            "-c:v",
            "h264_vaapi",
            "-rc_mode",
            if sv.low_delay { "CBR" } else { "VBR" },
        ]),
        Backend::Nvenc => {
            push(&[
                "-c:v",
                "h264_nvenc",
                "-preset",
                "p1",
                "-tune",
                "ull",
                "-zerolatency",
                "1",
                "-delay",
                "0",
            ]);
            if sv.low_delay {
                push(&["-rc", "cbr"]);
            }
        }
    }
    push(&["-bf", "0", "-g", &gop, "-b:v", &br, "-maxrate", &max]);
    if sv.low_delay {
        push(&["-bufsize", &buf]);
    }
    if !sv.fixed_rate {
        // damage-driven: duplicate frames are dropped instead of encoded
        push(&["-fps_mode", "vfr"]);
    }
    push(&[
        "-bsf:v",
        "h264_metadata=aud=insert",
        "-flush_packets",
        "1",
        "-f",
        "h264",
        "pipe:1",
    ]);
    a
}

pub struct Running {
    stop: Arc<AtomicBool>,
    restart: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    size: Arc<Mutex<(u32, u32)>>,
}

impl Running {
    pub fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Ok(mut c) = self.child.lock() {
            if let Some(mut c) = c.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }
    pub fn keyframe(&mut self) {
        self.restart.store(true, Ordering::Relaxed);
        if let Ok(mut c) = self.child.lock() {
            if let Some(c) = c.as_mut() {
                let _ = c.kill();
            }
        }
    }
}

pub fn current_size(v: &Option<Running>) -> (u32, u32) {
    v.as_ref()
        .and_then(|r| r.size.lock().ok().map(|s| *s))
        .unwrap_or((0, 0))
}

/// Width/height from the first SPS is not parsed here; the guest's mode
/// is read from sysfs (KMS) or xrandr instead.
fn mode_size(capture: Capture, sv: &StartVideo) -> (u32, u32) {
    if sv.width > 0 && sv.height > 0 {
        return (sv.width, sv.height);
    }
    if capture == Capture::X11 {
        if let Some(l) = super::session::find().and_then(|s| s.layout().ok()) {
            if let Some(m) = l.monitors.first() {
                return (m.width, m.height);
            }
        }
    }
    // /sys/class/drm/card0-*/modes: first line of a connected output
    if let Ok(rd) = std::fs::read_dir("/sys/class/drm") {
        for e in rd.flatten() {
            let p = e.path();
            let st = std::fs::read_to_string(p.join("status")).unwrap_or_default();
            if st.trim() != "connected" {
                continue;
            }
            if let Some((w, h)) = std::fs::read_to_string(p.join("modes"))
                .ok()
                .and_then(|m| m.lines().next().map(str::to_string))
                .and_then(|l| {
                    l.split_once('x')
                        .map(|(w, h)| (w.parse().ok(), h.trim_end_matches('i').parse().ok()))
                })
                .and_then(|(w, h)| w.zip(h))
            {
                return (w, h);
            }
        }
    }
    (0, 0)
}

pub fn start(cfg: &Config, sv: &StartVideo, out: Out) -> Result<(Running, VideoStatus), String> {
    let (capture, backend) = pick(cfg)?;
    let display = super::session::find().and_then(|s| s.display.clone());
    let args = ffmpeg_args(cfg, sv, capture, backend, display.as_deref());
    let size = Arc::new(Mutex::new(mode_size(capture, sv)));
    let stop = Arc::new(AtomicBool::new(false));
    let restart = Arc::new(AtomicBool::new(false));
    let child: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
    let r = Running {
        stop: stop.clone(),
        restart: restart.clone(),
        child: child.clone(),
        size: size.clone(),
    };
    let ffmpeg = cfg.ffmpeg.clone();
    let env = super::session::find().filter(|_| capture == Capture::X11);
    let fps = sv.fps.max(1);
    // Start once synchronously so a broken pipeline is reported.
    let spawn = move || -> Result<Child, String> {
        let mut c = Command::new(&ffmpeg);
        c.args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(s) = &env {
            s.apply(&mut c);
        }
        c.spawn().map_err(|e| format!("{ffmpeg}: {e}"))
    };
    let first = spawn()?;
    *child.lock().unwrap() = Some(first);
    let st = VideoStatus {
        running: true,
        codec: "h264".into(),
        encoder: backend.name().into(),
        capture: match capture {
            Capture::Kms => "kmsgrab",
            Capture::X11 => "x11grab",
        }
        .into(),
        width: size.lock().map(|s| s.0).unwrap_or(0),
        height: size.lock().map(|s| s.1).unwrap_or(0),
        fps,
        error: None,
    };
    std::thread::spawn(move || {
        let mut last_start = Instant::now();
        let mut first_failure_reported = false;
        loop {
            let (stdout, stderr) = {
                let mut g = child.lock().unwrap();
                let Some(c) = g.as_mut() else { break };
                (c.stdout.take(), c.stderr.take())
            };
            let Some(mut stdout) = stdout else { break };
            let mut errtext = String::new();
            let errt = std::thread::spawn(move || {
                let mut s = String::new();
                if let Some(mut e) = stderr {
                    let _ = e.read_to_string(&mut s);
                }
                s
            });
            let mut split = AudSplitter::default();
            let mut buf = vec![0u8; 1 << 16];
            let mut t_first: Option<u64> = None;
            let fd = stdout.as_raw_fd();
            let mut sent = 0u64;
            'read: loop {
                // flush a complete pending frame when the encoder went quiet
                let mut pfd = libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                };
                let ready = unsafe { libc::poll(&mut pfd, 1, 2) };
                if ready == 0 {
                    if let Some(au) = split.take_pending() {
                        if !emit(&out, &size, au, &mut t_first) {
                            break 'read;
                        }
                        sent += 1;
                    }
                    continue;
                }
                let n = match stdout.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                if t_first.is_none() {
                    t_first = Some(now_us());
                }
                for au in split.push(&buf[..n]) {
                    if !emit(&out, &size, au, &mut t_first) {
                        break 'read;
                    }
                    sent += 1;
                }
            }
            if let Ok(mut g) = child.lock() {
                if let Some(mut c) = g.take() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
            }
            if let Ok(s) = errt.join() {
                errtext = s;
            }
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let asked = restart.swap(false, Ordering::Relaxed);
            if !asked {
                if sent == 0 && !first_failure_reported {
                    first_failure_reported = true;
                    let _ = out.send(Msg::VideoStatus(VideoStatus {
                        error: Some(format!("encoder exited: {}", errtext.trim())),
                        ..Default::default()
                    }));
                }
                // back off when the pipeline keeps dying
                std::thread::sleep(Duration::from_millis(500));
            }
            let since = last_start.elapsed();
            if since < Duration::from_millis(200) {
                std::thread::sleep(Duration::from_millis(200) - since);
            }
            last_start = Instant::now();
            match spawn() {
                Ok(c) => *child.lock().unwrap() = Some(c),
                Err(e) => {
                    let _ = out.send(Msg::VideoStatus(VideoStatus {
                        error: Some(e),
                        ..Default::default()
                    }));
                    break;
                }
            }
        }
    });
    Ok((r, st))
}

fn emit(out: &Out, size: &Arc<Mutex<(u32, u32)>>, au: Vec<u8>, t_first: &mut Option<u64>) -> bool {
    let now = now_us();
    let cap = t_first.take().unwrap_or(now);
    let (w, h) = size.lock().map(|s| *s).unwrap_or((0, 0));
    let p = VideoPacket {
        keyframe: horcrux_guest_proto::h264_is_keyframe(&au),
        monitor: 0,
        capture_us: cap,
        encode_us: now.saturating_sub(cap) as u32,
        width: w,
        height: h,
        data: au,
    };
    // a full queue means the port is slow: drop this frame (the next
    // keyframe request repairs the picture) rather than add latency
    match out.try_send(Msg::Video(p)) {
        Ok(()) => true,
        Err(std::sync::mpsc::TrySendError::Full(_)) => true,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gaming_pipeline_is_low_delay() {
        let sv = StartVideo {
            fps: 240,
            fixed_rate: true,
            bitrate_kbps: 24000,
            max_bitrate_kbps: 24000,
            keyframe_interval_frames: 480,
            ..Default::default()
        };
        let a = ffmpeg_args(
            &Config::default(),
            &sv,
            Capture::Kms,
            Backend::Software,
            None,
        )
        .join(" ");
        assert!(
            a.contains("-device /dev/dri/card0 -framerate 240 -f kmsgrab -i -"),
            "{a}"
        );
        assert!(
            a.contains("-vf hwdownload,format=bgr0,format=yuv420p"),
            "{a}"
        );
        assert!(
            a.contains("libx264 -preset ultrafast -tune zerolatency"),
            "{a}"
        );
        assert!(
            a.contains("-bf 0 -g 480 -b:v 24000k -maxrate 24000k -bufsize 100k"),
            "{a}"
        );
        assert!(!a.contains("vfr"), "{a}");
        assert!(
            a.ends_with("h264_metadata=aud=insert -flush_packets 1 -f h264 pipe:1"),
            "{a}"
        );
    }

    #[test]
    fn desktop_444_and_vaapi() {
        let sv = StartVideo {
            chroma: "444".into(),
            width: 1280,
            height: 720,
            ..Default::default()
        };
        let a = ffmpeg_args(
            &Config::default(),
            &sv,
            Capture::X11,
            Backend::Software,
            Some(":1"),
        )
        .join(" ");
        assert!(a.contains("-f x11grab -i :1"), "{a}");
        assert!(
            a.contains("scale=1280:720,format=yuv444p") && a.contains("-profile:v high444"),
            "{a}"
        );
        assert!(a.contains("-fps_mode vfr"), "{a}");
        let v = ffmpeg_args(&Config::default(), &sv, Capture::Kms, Backend::Vaapi, None).join(" ");
        assert!(
            v.contains("hwmap=derive_device=vaapi,scale_vaapi=1280:720:format=nv12"),
            "{v}"
        );
        assert!(
            v.contains("h264_vaapi -rc_mode CBR") && !v.contains("444"),
            "{v}"
        );
    }
}
