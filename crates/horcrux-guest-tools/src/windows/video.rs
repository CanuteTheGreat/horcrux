//! Capture + encode thread: desktop capture -> NV12 -> H.264 -> `Msg::Video`.
//!
//! One thread owns every COM object (capture device, encoder MFT), so no
//! interface crosses threads. Capture is DXGI Desktop Duplication (GDI
//! BitBlt when duplication is unavailable); the BGRA->NV12 conversion runs
//! on the CPU (multi-threaded) in this version.
use super::capture::{Capturer, Dup, Gdi, Grab};
use super::{convert, layout, mf};
use crate::agent::{now_us, Out};
use crate::config::Config;
use horcrux_guest_proto::{Msg, StartVideo, VideoPacket, VideoStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub struct Video {
    stop: Arc<AtomicBool>,
    key: Arc<AtomicBool>,
    th: Option<JoinHandle<()>>,
}

impl Video {
    pub fn start(cfg: &Config, sv: &StartVideo, out: Out) -> (Option<Video>, VideoStatus) {
        let fail = |e: String| VideoStatus {
            error: Some(e),
            ..Default::default()
        };
        if !sv.codec.is_empty() && sv.codec != "h264" {
            return (
                None,
                fail(format!("codec {} is not supported (h264 only)", sv.codec)),
            );
        }
        let stop = Arc::new(AtomicBool::new(false));
        let key = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<Result<VideoStatus, String>>();
        let (c, s, st, k) = (cfg.clone(), sv.clone(), stop.clone(), key.clone());
        let th = std::thread::spawn(move || run(c, s, out, st, k, tx));
        match rx.recv_timeout(Duration::from_millis(cfg.video_start_timeout_ms)) {
            Ok(Ok(status)) => (
                Some(Video {
                    stop,
                    key,
                    th: Some(th),
                }),
                status,
            ),
            Ok(Err(e)) => {
                let _ = th.join();
                (None, fail(e))
            }
            Err(_) => {
                stop.store(true, Ordering::SeqCst);
                (None, fail("capture/encoder start timed out".into()))
            }
        }
    }

    pub fn keyframe(&self) {
        self.key.store(true, Ordering::SeqCst);
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.th.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Video {
    fn drop(&mut self) {
        self.stop();
    }
}

fn open_capture(cfg: &Config, sv: &StartVideo) -> Result<Box<dyn Capturer>, String> {
    let mons = layout::monitors();
    let idx = sv.monitor as usize;
    let m = mons
        .get(idx)
        .or(mons.first())
        .ok_or("no display attached to the desktop")?;
    let origin = (m.x, m.y);
    let gdi = || {
        Gdi::open(origin, (m.w, m.h), cfg.gdi_poll_ms as u32)
            .map(|g| Box::new(g) as Box<dyn Capturer>)
    };
    match cfg.capture.as_str() {
        "gdi" => gdi(),
        _ => {
            match Dup::open(origin, idx) {
                Ok(d) => Ok(Box::new(d)),
                Err(e) if cfg.capture == "auto" => {
                    eprintln!("horcrux-guest-agent: desktop duplication unavailable ({e}); using GDI capture");
                    gdi()
                }
                Err(e) => Err(e),
            }
        }
    }
}

/// Encoder candidates allowed by the `encoder` setting, best first.
fn candidates(cfg: &Config) -> Vec<mf::Candidate> {
    mf::enumerate()
        .into_iter()
        .filter(|c| match cfg.encoder.as_str() {
            "auto" => true,
            "software" => !c.hardware,
            "mediafoundation" => true,
            k => c.kind == k,
        })
        .collect()
}

/// Probed capabilities: (encoder backends, capture methods).
pub fn probe(cfg: &Config) -> (Vec<String>, Vec<String>) {
    let mut enc = Vec::new();
    if mf::startup().is_ok() {
        for c in mf::enumerate() {
            let k = if c.hardware {
                c.kind.clone()
            } else {
                "software".into()
            };
            if !enc.contains(&k) {
                enc.push(k);
            }
            c.close();
        }
        mf::release();
    }
    let mut cap = Vec::new();
    let mons = layout::monitors();
    if let Some(m) = mons.first() {
        if Dup::open((m.x, m.y), 0).is_ok() {
            cap.push("dxgi-dup".to_string());
        }
        if Gdi::open((m.x, m.y), (m.w, m.h), cfg.gdi_poll_ms as u32).is_ok() {
            cap.push("gdi".to_string());
        }
    }
    (enc, cap)
}

fn even(v: u32) -> u32 {
    (v & !1).max(2)
}

fn profile(name: &str) -> u32 {
    match name {
        "baseline" => 66,
        "high" => 100,
        _ => 77,
    }
}

type Reply = mpsc::Sender<Result<VideoStatus, String>>;

fn run(
    cfg: Config,
    sv: StartVideo,
    out: Out,
    stop: Arc<AtomicBool>,
    key: Arc<AtomicBool>,
    reply: Reply,
) {
    if let Err(e) = mf::startup() {
        let _ = reply.send(Err(e));
        return;
    }
    let mut reply = Some(reply);
    let log = |m: String| {
        eprintln!("horcrux-guest-agent: {m}");
        let _ = out.try_send(Msg::Log(m));
    };
    if sv.chroma == "444" || sv.hdr || sv.intra_refresh {
        log("4:4:4, HDR and intra refresh are not available in the Windows agent yet; encoding 4:2:0 SDR".into());
    }
    let fps = sv.fps.clamp(1, 240);
    let interval = Duration::from_micros(1_000_000 / fps as u64);
    let threads = if cfg.convert_threads == 0 {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(4)
    } else {
        cfg.convert_threads
    };
    let mut cands = candidates(&cfg);
    let t0 = Instant::now();
    let mut cap: Option<Box<dyn Capturer>> = None;
    'rebuild: loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        // Capture.
        let c = match open_capture(&cfg, &sv) {
            Ok(c) => c,
            Err(e) => {
                if let Some(r) = reply.take() {
                    let _ = r.send(Err(format!("capture: {e}")));
                    break;
                }
                std::thread::sleep(Duration::from_millis(cfg.reconnect_ms));
                continue;
            }
        };
        let (sw, sh) = c.size();
        cap = Some(c);
        let (w, h) = if sv.width > 0 && sv.height > 0 {
            (even(sv.width), even(sv.height))
        } else {
            (even(sw), even(sh))
        };
        // Encoder: first candidate that accepts the configuration.
        let mut enc = None;
        let mut errors = Vec::new();
        while !cands.is_empty() {
            let p = mf::Params {
                w,
                h,
                fps,
                bitrate_kbps: sv.bitrate_kbps,
                max_bitrate_kbps: sv.max_bitrate_kbps,
                gop: sv.keyframe_interval_frames,
                low_delay: sv.low_delay,
                profile: profile(&cfg.h264_profile),
                output_wait_ms: cfg.mf_output_wait_ms,
            };
            match mf::Encoder::new(&cands[0], p) {
                Ok(e) => {
                    enc = Some(e);
                    break;
                }
                Err(e) => {
                    errors.push(e);
                    cands.remove(0).close();
                }
            }
        }
        let Some(mut enc) = enc else {
            let msg = if errors.is_empty() {
                "no H.264 encoder MFT available (is Media Foundation installed?)".to_string()
            } else {
                format!("no usable H.264 encoder: {}", errors.join("; "))
            };
            if let Some(r) = reply.take() {
                let _ = r.send(Err(msg));
            } else {
                log(msg);
            }
            break;
        };
        for n in enc.notes.drain(..) {
            log(format!("encoder {}: {n}", enc.name));
        }
        let cap_name = cap.as_ref().map(|c| c.name()).unwrap_or("");
        if let Some(r) = reply.take() {
            let _ = r.send(Ok(VideoStatus {
                running: true,
                codec: "h264".into(),
                encoder: enc.kind.clone(),
                capture: cap_name.into(),
                width: w,
                height: h,
                fps,
                error: None,
            }));
        }
        log(format!(
            "video: {} -> {} ({}x{} @ {} fps, {} kbit/s)",
            cap_name, enc.name, w, h, fps, sv.bitrate_kbps
        ));

        // Frame loop.
        let mut bgra: Vec<u8> = Vec::new();
        let mut nv12: Vec<u8> = Vec::new();
        let mut have_frame = false;
        let mut next = Instant::now();
        let mut last_encode = Instant::now() - interval;
        let mut need_key = true;
        let mut frame_size = (sw, sh);
        let mut sent_any = false;
        loop {
            if stop.load(Ordering::SeqCst) {
                break 'rebuild;
            }
            if cap.is_none() {
                match open_capture(&cfg, &sv) {
                    Ok(c) => cap = Some(c),
                    Err(_) => {
                        std::thread::sleep(Duration::from_millis(250));
                        continue;
                    }
                }
                let s = cap.as_ref().map(|c| c.size()).unwrap_or(frame_size);
                if s != frame_size {
                    frame_size = s;
                    if sv.width == 0 || sv.height == 0 {
                        // The monitor changed size: restart the encoder at the new one.
                        drop(enc);
                        continue 'rebuild;
                    }
                }
            }
            let c = cap.as_mut().expect("capture is open");
            let mut fresh = false;
            let now = Instant::now();
            let gate = if sv.fixed_rate {
                next
            } else {
                last_encode + interval
            };
            if gate > now {
                std::thread::sleep(gate - now);
            }
            let wait = if sv.fixed_rate && have_frame {
                0
            } else {
                cfg.idle_poll_ms as u32
            };
            match c.grab(wait, &mut bgra) {
                Grab::New => {
                    fresh = true;
                    have_frame = true;
                }
                Grab::Idle => {}
                Grab::Lost => {
                    cap = None;
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                Grab::Err(e) => {
                    log(format!("capture: {e}"));
                    cap = None;
                    std::thread::sleep(Duration::from_millis(250));
                    continue;
                }
            }
            let want_key = key.swap(false, Ordering::SeqCst);
            let due = if sv.fixed_rate {
                have_frame
            } else {
                fresh || (want_key && have_frame)
            };
            if !due {
                continue;
            }
            if need_key || want_key {
                enc.request_keyframe();
                need_key = false;
            }
            let cap_us = now_us();
            if fresh || nv12.is_empty() {
                let (fw, fh) = frame_size;
                convert::bgra_to_nv12(
                    &bgra,
                    fw as usize,
                    fh as usize,
                    &mut nv12,
                    w as usize,
                    h as usize,
                    threads,
                );
            }
            let pts = (t0.elapsed().as_nanos() / 100) as i64;
            let units = match enc.encode(&nv12, pts) {
                Ok(u) => u,
                Err(e) => {
                    // A hardware MFT that fails before producing anything: try the next one.
                    if !sent_any && cands.len() > 1 {
                        log(format!(
                            "encoder {} failed ({e}); trying the next one",
                            enc.name
                        ));
                        cands.remove(0).close();
                        drop(enc);
                        continue 'rebuild;
                    }
                    log(format!("encoder {} failed: {e}", enc.name));
                    break 'rebuild;
                }
            };
            for n in enc.notes.drain(..) {
                log(format!("encoder {}: {n}", enc.name));
            }
            last_encode = Instant::now();
            next += interval;
            if next < last_encode {
                next = last_encode;
            }
            for (au, k) in units {
                let msg = Msg::Video(VideoPacket {
                    keyframe: k,
                    monitor: sv.monitor as u16,
                    capture_us: cap_us,
                    encode_us: (now_us().saturating_sub(cap_us)).min(u32::MAX as u64) as u32,
                    width: w,
                    height: h,
                    data: au,
                });
                match out.try_send(msg) {
                    Ok(()) => sent_any = true,
                    // A dropped P-frame breaks the stream: resynchronise with an IDR.
                    Err(TrySendError::Full(_)) => need_key = true,
                    Err(TrySendError::Disconnected(_)) => break 'rebuild,
                }
            }
        }
    }
    drop(cap);
    for c in cands {
        c.close();
    }
    mf::release();
}
