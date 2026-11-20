//! Guest audio: WASAPI shared-mode loopback capture of a render endpoint ->
//! 48 kHz (or the requested rate) -> Opus (libopus, CBR) -> `Msg::Audio`.
//!
//! One thread owns every COM object. Loopback capture delivers nothing
//! while nothing plays, so the stream is clocked by the wall clock: when the
//! endpoint is idle (`audio_idle_ms`) silence is encoded for the elapsed
//! time and the node's WebRTC track keeps its frame cadence. A vanished or
//! changed endpoint is reported with a `Log` message and reopened.
use crate::agent::{now_us, Out};
use crate::audio::{Converter, InFormat, Stream, StreamParams};
use crate::config::Config;
use horcrux_guest_proto::{AudioPacket, Msg, StartAudio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::TrySendError;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::core::PCWSTR;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_LOOPBACK, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED, STGM_READ,
};

pub struct Audio {
    stop: Arc<AtomicBool>,
    th: Option<JoinHandle<()>>,
}

impl Audio {
    pub fn start(cfg: &Config, a: &StartAudio, out: Out) -> Result<Audio, String> {
        let p = StreamParams::from_request(a, cfg.audio_bitrate_kbps)?;
        let enc = encoder(cfg, &p)?;
        let stop = Arc::new(AtomicBool::new(false));
        let (c, st) = (cfg.clone(), stop.clone());
        let th = std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || run(c, p, enc, out, st))
            .map_err(|e| e.to_string())?;
        Ok(Audio { stop, th: Some(th) })
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.th.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        self.stop();
    }
}

fn encoder(cfg: &Config, p: &StreamParams) -> Result<opus::Encoder, String> {
    let app = match cfg.audio_opus_application.as_str() {
        "audio" => opus::Application::Audio,
        "voip" => opus::Application::Voip,
        _ => opus::Application::LowDelay,
    };
    let ch = if p.channels == 1 {
        opus::Channels::Mono
    } else {
        opus::Channels::Stereo
    };
    let e = |e: opus::Error| format!("opus encoder: {e}");
    let mut enc = opus::Encoder::new(p.rate, ch, app).map_err(e)?;
    enc.set_vbr(false).map_err(e)?;
    enc.set_bitrate(opus::Bitrate::Bits(p.bitrate_bps as i32))
        .map_err(e)?;
    Ok(enc)
}

/// COM initialised for the current thread, released on drop.
struct Com;

impl Com {
    fn init() -> Com {
        // SAFETY: balanced by CoUninitialize in Drop on the same thread.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        Com
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        // SAFETY: matches the CoInitializeEx above.
        unsafe { CoUninitialize() }
    }
}

fn enumerator() -> Result<IMMDeviceEnumerator, String> {
    // SAFETY: COM is initialised on this thread.
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
        .map_err(|e| format!("MMDeviceEnumerator: {e}"))
}

fn device_id(d: &IMMDevice) -> String {
    // SAFETY: GetId returns a CoTaskMem string we free after copying.
    unsafe {
        match d.GetId() {
            Ok(p) => {
                let s = p.to_string().unwrap_or_default();
                CoTaskMemFree(Some(p.0 as *const _));
                s
            }
            Err(_) => String::new(),
        }
    }
}

fn device_name(d: &IMMDevice) -> String {
    // SAFETY: plain COM calls on a live device.
    unsafe {
        d.OpenPropertyStore(STGM_READ)
            .and_then(|s| s.GetValue(&PKEY_Device_FriendlyName))
            .map(|v| v.to_string())
            .unwrap_or_default()
    }
}

/// The render endpoint named by `audio_source`: `default`, an endpoint ID,
/// or a case-insensitive part of an active endpoint's friendly name.
fn find_device(en: &IMMDeviceEnumerator, source: &str) -> Result<IMMDevice, String> {
    // SAFETY: plain COM calls; the ID string outlives the GetDevice call.
    unsafe {
        if source.is_empty() || source == "default" {
            return en
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .map_err(|e| format!("no default audio output ({e})"));
        }
        let wide: Vec<u16> = source.encode_utf16().chain(Some(0)).collect();
        if let Ok(d) = en.GetDevice(PCWSTR(wide.as_ptr())) {
            return Ok(d);
        }
        let want = source.to_lowercase();
        let col = en
            .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
            .map_err(|e| format!("EnumAudioEndpoints: {e}"))?;
        for i in 0..col.GetCount().unwrap_or(0) {
            if let Ok(d) = col.Item(i) {
                if device_name(&d).to_lowercase().contains(&want) {
                    return Ok(d);
                }
            }
        }
        Err(format!("no active audio output matches {source:?}"))
    }
}

/// Whether there is an endpoint to capture (Hello's `caps.audio`).
pub fn probe(cfg: &Config) -> bool {
    let source = cfg.audio_source.clone();
    std::thread::spawn(move || {
        let _com = Com::init();
        enumerator()
            .and_then(|en| find_device(&en, &source))
            .is_ok()
    })
    .join()
    .unwrap_or(false)
}

struct Endpoint {
    client: IAudioClient,
    cap: IAudioCaptureClient,
    conv: Converter,
    id: String,
    name: String,
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        // SAFETY: stopping a started client; errors (device gone) are moot.
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

fn open(en: &IMMDeviceEnumerator, cfg: &Config, p: &StreamParams) -> Result<Endpoint, String> {
    let dev = find_device(en, &cfg.audio_source)?;
    let (id, name) = (device_id(&dev), device_name(&dev));
    // SAFETY: COM calls on this thread; the mix format pointer is read
    // within its reported size and freed after Initialize copied it.
    unsafe {
        let client: IAudioClient = dev
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| format!("{name}: Activate: {e}"))?;
        let wf = client
            .GetMixFormat()
            .map_err(|e| format!("{name}: GetMixFormat: {e}"))?;
        let len = 18 + (*wf).cbSize as usize;
        let bytes = std::slice::from_raw_parts(wf as *const u8, len).to_vec();
        let r = client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            cfg.audio_buffer_ms as i64 * 10_000,
            0,
            wf,
            None,
        );
        CoTaskMemFree(Some(wf as *const _));
        r.map_err(|e| format!("{name}: Initialize (loopback): {e}"))?;
        let fmt = InFormat::from_waveformat(&bytes).map_err(|e| format!("{name}: {e}"))?;
        let cap: IAudioCaptureClient = client
            .GetService()
            .map_err(|e| format!("{name}: IAudioCaptureClient: {e}"))?;
        client.Start().map_err(|e| format!("{name}: Start: {e}"))?;
        Ok(Endpoint {
            client,
            cap,
            conv: Converter::new(fmt, p),
            id,
            name,
        })
    }
}

/// Read every pending packet; converted samples go to `out`. Returns
/// whether the endpoint delivered any packet.
fn drain(ep: &mut Endpoint, out: &mut Vec<f32>) -> windows::core::Result<bool> {
    let mut got = false;
    let align = ep.conv.fmt.block_align();
    // SAFETY: the buffer GetBuffer returns is valid for `frames` frames
    // until ReleaseBuffer.
    unsafe {
        while ep.cap.GetNextPacketSize()? > 0 {
            let mut data = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            ep.cap
                .GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
            if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                ep.conv.push_silent(frames as usize, out);
            } else {
                let b = std::slice::from_raw_parts(data, frames as usize * align);
                ep.conv.push(b, out);
            }
            ep.cap.ReleaseBuffer(frames)?;
            got = true;
        }
    }
    Ok(got)
}

fn log(out: &Out, s: String) {
    eprintln!("horcrux-guest-agent: {s}");
    let _ = out.try_send(Msg::Log(s));
}

/// Open the configured endpoint; report the result (a failure only when it
/// differs from the previous one, so a missing device is said once).
fn try_open(
    en: &IMMDeviceEnumerator,
    cfg: &Config,
    p: &StreamParams,
    out: &Out,
    last_err: &mut String,
) -> Option<Endpoint> {
    match open(en, cfg, p) {
        Ok(e) => {
            let f = e.conv.fmt;
            log(
                out,
                format!(
                    "audio: capturing {} ({} Hz, {} ch, {} bit {:?}) -> Opus {} Hz {} ch {} kbit/s {} ms",
                    e.name,
                    f.rate,
                    f.channels,
                    f.valid_bits,
                    f.kind,
                    p.rate,
                    p.channels,
                    p.bitrate_bps / 1000,
                    p.frame_ms
                ),
            );
            last_err.clear();
            Some(e)
        }
        Err(e) => {
            if e != *last_err {
                log(
                    out,
                    format!(
                        "audio: {e}; sending silence, retrying every {} ms",
                        cfg.audio_retry_ms
                    ),
                );
                *last_err = e;
            }
            None
        }
    }
}

fn run(cfg: Config, p: StreamParams, mut enc: opus::Encoder, out: Out, stop: Arc<AtomicBool>) {
    let _com = Com::init();
    let en = match enumerator() {
        Ok(e) => Some(e),
        Err(e) => {
            log(&out, format!("audio: {e}; sending silence"));
            None
        }
    };
    let ms = |v: u64| v * p.rate as u64 / 1000;
    let mut stream = Stream::new(&p, ms(cfg.audio_idle_ms), ms(cfg.audio_max_fill_ms));
    let mut last_err = String::new();
    // Open before the stream clock starts: opening can take a few hundred ms,
    // and the stream should begin with its first frame, not with a skip.
    let mut ep: Option<Endpoint> = en
        .as_ref()
        .and_then(|en| try_open(en, &cfg, &p, &out, &mut last_err));
    let t0 = Instant::now();
    let t0_us = now_us();
    let poll = Duration::from_millis(cfg.audio_poll_ms);
    let retry = Duration::from_millis(cfg.audio_retry_ms);
    let check = Duration::from_millis(cfg.audio_device_check_ms);
    let follow_default = cfg.audio_source.is_empty() || cfg.audio_source == "default";
    let mut next_open = if ep.is_some() { t0 } else { t0 + retry };
    let mut next_check = t0 + check;
    let mut samples = Vec::new();
    let mut pkt = vec![0u8; 4000];
    let mut dropped = 0u64;
    while !stop.load(Ordering::SeqCst) {
        let now = Instant::now();
        let now_frames = now.duration_since(t0).as_micros() as u64 * p.rate as u64 / 1_000_000;
        if let (None, Some(en)) = (&ep, &en) {
            if now >= next_open {
                ep = try_open(en, &cfg, &p, &out, &mut last_err);
                next_open = now + retry;
            }
        }
        if let Some(e) = ep.as_mut() {
            samples.clear();
            match drain(e, &mut samples) {
                Ok(true) => stream.push(&samples, now_frames),
                Ok(false) => {}
                Err(err) => {
                    log(
                        &out,
                        format!("audio: {} stopped ({err}); reopening", e.name),
                    );
                    ep = None;
                    next_open = now + retry.min(Duration::from_millis(200));
                }
            }
        }
        if follow_default && now >= next_check {
            next_check = now + check;
            if let (Some(e), Some(en)) = (&ep, &en) {
                // SAFETY: plain COM call on this thread.
                let cur = unsafe { en.GetDefaultAudioEndpoint(eRender, eConsole) }
                    .map(|d| device_id(&d))
                    .unwrap_or_default();
                if cur != e.id {
                    log(
                        &out,
                        format!("audio: default output changed (was {}); switching", e.name),
                    );
                    ep = None;
                    next_open = now;
                }
            }
        }
        stream.fill(now_frames);
        while let Some((pos, frame)) = stream.pop() {
            let n = match enc.encode_float(&frame, &mut pkt) {
                Ok(n) => n,
                Err(e) => {
                    log(&out, format!("audio: opus encode: {e}"));
                    continue;
                }
            };
            let m = Msg::Audio(AudioPacket {
                capture_us: t0_us + pos * 1_000_000 / p.rate as u64,
                data: pkt[..n].to_vec(),
            });
            match out.try_send(m) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    dropped += 1;
                    if dropped.is_power_of_two() {
                        eprintln!(
                            "horcrux-guest-agent: audio: port busy, {dropped} packets dropped"
                        );
                    }
                }
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
        // Idle: wake exactly when silence completes the next frame, so idle
        // packets keep an even cadence; otherwise poll the endpoint.
        let mut wait = poll;
        let now_frames = t0.elapsed().as_micros() as u64 * p.rate as u64 / 1_000_000;
        if let Some(due) = stream.idle_frame_due(now_frames) {
            let us = due.saturating_sub(now_frames) * 1_000_000 / p.rate as u64;
            wait = wait.min(Duration::from_micros(us.max(200)));
        }
        std::thread::sleep(wait);
    }
}
