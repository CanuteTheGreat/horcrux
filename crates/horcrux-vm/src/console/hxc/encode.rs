//! Host-side video encoding for the Horcrux Console.
//!
//! Frames from the guest's display are encoded by an FFmpeg child process
//! (a separate program, not linked: the node's packaged FFmpeg with libx264,
//! libvpx, VA-API or NVENC, whatever is installed). Raw frames go in on
//! stdin, one encoded frame per input frame comes out on stdout:
//!
//! * VP8 in an IVF container (12-byte frame headers, trivially split);
//! * H.264 as an Annex-B byte stream with an access unit delimiter
//!   inserted in front of every frame (`h264_metadata=aud=insert`), so
//!   frames are split at AUD NAL units whatever the encoder.
//!
//! Rate control is low-delay for every encoder: no B-frames, no lookahead,
//! one output frame per input frame (`-fps_mode passthrough`), keyframes at
//! the profile's interval.
use horcrux_common::console::{ConsoleSettings, EncoderPreference, ProfileSettings};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;

/// Video codec on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    H264,
    Vp8,
}

impl VideoCodec {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "h264" => Some(Self::H264),
            "vp8" => Some(Self::Vp8),
            _ => None,
        }
    }
    pub fn name(&self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Vp8 => "vp8",
        }
    }
}

/// Which FFmpeg encoder actually runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EncoderBackend {
    Software,
    Vaapi,
    Nvenc,
}

/// Pixel layout of the frames handed to the encoder (pixman formats QEMU
/// uses for its scanouts, little-endian memory order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// pixman x8r8g8b8 / a8r8g8b8: bytes B, G, R, X.
    Bgrx,
    /// pixman x8b8g8r8 / a8b8g8r8: bytes R, G, B, X.
    Rgbx,
    /// pixman r5g6b5.
    Rgb565,
}

impl PixelFormat {
    /// From a pixman format code (`PIXMAN_FORMAT(bpp, type, a, r, g, b)`).
    pub fn from_pixman(code: u32) -> Option<Self> {
        match code {
            0x2002_0888 | 0x2002_8888 => Some(Self::Bgrx),
            0x2003_0888 | 0x2003_8888 => Some(Self::Rgbx),
            0x1002_0565 => Some(Self::Rgb565),
            _ => None,
        }
    }
    pub fn bytes_per_pixel(&self) -> usize {
        match self {
            Self::Bgrx | Self::Rgbx => 4,
            Self::Rgb565 => 2,
        }
    }
    fn ffmpeg(&self) -> &'static str {
        match self {
            Self::Bgrx => "bgr0",
            Self::Rgbx => "rgb0",
            Self::Rgb565 => "rgb565le",
        }
    }
}

/// What one encoder run is set up for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodeParams {
    pub codec: VideoCodec,
    pub backend: EncoderBackend,
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    /// Output size after scaling to the node's maximum (even numbers).
    pub out_width: u32,
    pub out_height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub max_bitrate_kbps: u32,
    pub keyframe_interval_secs: u32,
    pub x264_preset: String,
    pub vaapi_device: String,
}

/// Fit `w`x`h` into the maximum, keeping the aspect ratio; even sizes
/// (4:2:0 chroma needs them).
pub fn fit(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let (mut ow, mut oh) = (w.max(2), h.max(2));
    if ow > max_w || oh > max_h {
        let s = f64::min(max_w as f64 / ow as f64, max_h as f64 / oh as f64);
        ow = ((ow as f64 * s) as u32).max(2);
        oh = ((oh as f64 * s) as u32).max(2);
    }
    (ow & !1, oh & !1)
}

impl EncodeParams {
    pub fn new(
        codec: VideoCodec,
        backend: EncoderBackend,
        width: u32,
        height: u32,
        format: PixelFormat,
        profile: &ProfileSettings,
        settings: &ConsoleSettings,
    ) -> Self {
        let (out_width, out_height) = fit(width, height, settings.max_width, settings.max_height);
        Self {
            codec,
            backend: if codec == VideoCodec::Vp8 {
                // VP8 is software-encoded (libvpx) in every case.
                EncoderBackend::Software
            } else {
                backend
            },
            width,
            height,
            format,
            out_width,
            out_height,
            fps: profile.max_fps,
            bitrate_kbps: profile.bitrate_kbps,
            max_bitrate_kbps: profile.max_bitrate_kbps,
            keyframe_interval_secs: profile.keyframe_interval_secs,
            x264_preset: settings.x264_preset.clone(),
            vaapi_device: settings.vaapi_device.clone(),
        }
    }

    /// FFmpeg arguments (without the program name).
    pub fn ffmpeg_args(&self) -> Vec<String> {
        let mut a: Vec<String> = Vec::new();
        let mut push = |items: &[&str]| a.extend(items.iter().map(|s| s.to_string()));
        push(&["-hide_banner", "-loglevel", "error", "-nostdin"]);
        if self.backend == EncoderBackend::Vaapi {
            push(&["-vaapi_device", &self.vaapi_device]);
        }
        let size = format!("{}x{}", self.width, self.height);
        let fps = self.fps.to_string();
        push(&[
            "-f",
            "rawvideo",
            "-pix_fmt",
            self.format.ffmpeg(),
            "-s",
            &size,
            "-r",
            &fps,
            "-i",
            "pipe:0",
        ]);
        let scale = (self.out_width, self.out_height) != (self.width, self.height)
            || self.width % 2 == 1
            || self.height % 2 == 1;
        let mut filters: Vec<String> = Vec::new();
        if scale {
            filters.push(format!("scale={}:{}", self.out_width, self.out_height));
        }
        match self.backend {
            EncoderBackend::Vaapi => {
                filters.push("format=nv12".into());
                filters.push("hwupload".into());
            }
            _ => filters.push("format=yuv420p".into()),
        }
        push(&["-vf", &filters.join(",")]);
        push(&["-fps_mode", "passthrough", "-flush_packets", "1"]);
        let gop = (self.fps * self.keyframe_interval_secs).max(1).to_string();
        let b = format!("{}k", self.bitrate_kbps);
        let maxrate = format!("{}k", self.max_bitrate_kbps);
        // One frame's worth of buffer at the ceiling: low delay.
        let bufsize = format!(
            "{}k",
            (self.max_bitrate_kbps / self.fps.max(1)).max(100) * 2
        );
        match (self.codec, self.backend) {
            (VideoCodec::Vp8, _) => {
                push(&[
                    "-c:v",
                    "libvpx",
                    "-deadline",
                    "realtime",
                    "-cpu-used",
                    "8",
                    "-lag-in-frames",
                    "0",
                    "-error-resilient",
                    "1",
                    "-auto-alt-ref",
                    "0",
                    "-g",
                    &gop,
                    "-b:v",
                    &b,
                    "-maxrate",
                    &maxrate,
                    "-bufsize",
                    &bufsize,
                    "-f",
                    "ivf",
                    "pipe:1",
                ]);
            }
            (VideoCodec::H264, backend) => {
                match backend {
                    EncoderBackend::Software => push(&[
                        "-c:v",
                        "libx264",
                        "-preset",
                        &self.x264_preset,
                        "-tune",
                        "zerolatency",
                        "-profile:v",
                        "baseline",
                    ]),
                    EncoderBackend::Vaapi => {
                        push(&["-c:v", "h264_vaapi", "-profile:v", "constrained_baseline"])
                    }
                    EncoderBackend::Nvenc => push(&[
                        "-c:v",
                        "h264_nvenc",
                        "-preset",
                        "p1",
                        "-tune",
                        "ull",
                        "-zerolatency",
                        "1",
                        "-profile:v",
                        "baseline",
                    ]),
                }
                push(&[
                    "-bf",
                    "0",
                    "-g",
                    &gop,
                    "-b:v",
                    &b,
                    "-maxrate",
                    &maxrate,
                    "-bufsize",
                    &bufsize,
                    "-bsf:v",
                    "h264_metadata=aud=insert",
                    "-f",
                    "h264",
                    "pipe:1",
                ]);
            }
        }
        a
    }
}

/// One encoded frame.
#[derive(Debug)]
pub struct EncodedFrame {
    pub data: Vec<u8>,
    /// Index of the raw frame (in submission order) this came from.
    pub seq: u64,
}

/// Splits an IVF stream into frames.
#[derive(Default)]
pub struct IvfSplitter {
    buf: Vec<u8>,
    header_done: bool,
}

impl IvfSplitter {
    pub fn push(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        if !self.header_done {
            if self.buf.len() < 32 {
                return out;
            }
            let hlen = u16::from_le_bytes([self.buf[6], self.buf[7]]) as usize;
            if self.buf.len() < hlen {
                return out;
            }
            self.buf.drain(..hlen);
            self.header_done = true;
        }
        loop {
            if self.buf.len() < 12 {
                break;
            }
            let len =
                u32::from_le_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
            if self.buf.len() < 12 + len {
                break;
            }
            out.push(self.buf[12..12 + len].to_vec());
            self.buf.drain(..12 + len);
        }
        out
    }
}

/// Splits an Annex-B H.264 stream into access units at AUD NAL units
/// (type 9). A frame is complete when the next AUD arrives.
#[derive(Default)]
pub struct AnnexBSplitter {
    buf: Vec<u8>,
}

/// Offsets of start codes (index of the first 0 of 00 00 01 / 00 00 00 01)
/// followed by an AUD NAL.
fn aud_positions(b: &[u8]) -> Vec<usize> {
    let mut v = Vec::new();
    let mut i = 0;
    while i + 3 < b.len() {
        if b[i] == 0 && b[i + 1] == 0 && b[i + 2] == 1 {
            if b[i + 3] & 0x1f == 9 {
                let start = if i > 0 && b[i - 1] == 0 { i - 1 } else { i };
                v.push(start);
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    v
}

impl AnnexBSplitter {
    pub fn push(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        self.buf.extend_from_slice(data);
        let pos = aud_positions(&self.buf);
        let mut out = Vec::new();
        if pos.len() < 2 {
            return out;
        }
        let last = *pos.last().unwrap();
        for w in pos.windows(2) {
            out.push(self.buf[w[0]..w[1]].to_vec());
        }
        self.buf.drain(..last);
        out
    }

    /// The frame still buffered (the encoder only closes it with the next
    /// frame's AUD); used to flush the last frame.
    pub fn pending(&self) -> Option<&[u8]> {
        (!self.buf.is_empty()).then_some(&self.buf[..])
    }

    pub fn take_pending(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.buf)
    }
}

/// A running encoder. Raw frames go in through `submit` (the newest one
/// wins when the encoder is busy), encoded frames come out of the channel
/// given to `spawn`.
///
/// Software H.264 runs in-process through the system libx264 when the
/// settings allow it and the library loads ([`LibraryEncoder`]): rate
/// changes and keyframe requests then apply between two frames. Everything
/// else (VP8, VA-API, NVENC, no libx264) runs the FFmpeg program, where a
/// new rate or a keyframe means a restart.
pub enum Encoder {
    Program(ProgramEncoder),
    Library(LibraryEncoder),
}

impl Encoder {
    pub fn spawn(
        settings: &ConsoleSettings,
        params: EncodeParams,
        out: mpsc::Sender<EncodedFrame>,
    ) -> std::io::Result<Self> {
        if settings.x264_in_process
            && params.codec == VideoCodec::H264
            && params.backend == EncoderBackend::Software
        {
            match LibraryEncoder::spawn(settings, params.clone(), out.clone()) {
                Ok(e) => return Ok(Self::Library(e)),
                Err(e) => tracing::info!("console: in-process x264 not used ({e}); FFmpeg program"),
            }
        }
        ProgramEncoder::spawn(&settings.ffmpeg_path, params, out).map(Self::Program)
    }

    pub fn params(&self) -> &EncodeParams {
        match self {
            Self::Program(e) => &e.params,
            Self::Library(e) => &e.params,
        }
    }

    /// `libx264` (in-process) or `ffmpeg` (program).
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Program(_) => "ffmpeg",
            Self::Library(_) => "libx264",
        }
    }

    pub fn submit(&mut self, frame: Vec<u8>) -> Option<u64> {
        match self {
            Self::Program(e) => e.submit(frame),
            Self::Library(e) => e.submit(frame),
        }
    }

    pub fn alive(&mut self) -> bool {
        match self {
            Self::Program(e) => e.alive(),
            Self::Library(e) => e.alive(),
        }
    }

    /// Change bitrate (kbit/s) and frame rate in-stream. False when this
    /// encoder can't (the caller restarts it).
    pub fn set_rate(&mut self, kbps: u32, fps: u32) -> bool {
        match self {
            Self::Program(_) => false,
            Self::Library(e) => {
                e.set_rate(kbps, fps);
                true
            }
        }
    }

    /// Ask for an IDR on the next frame. False when this encoder can't.
    pub fn request_keyframe(&mut self) -> bool {
        match self {
            Self::Program(_) => false,
            Self::Library(e) => {
                e.ctl.keyframe.store(true, Ordering::Relaxed);
                true
            }
        }
    }
}

/// x264 rate control for `kbps` (target and ceiling) when frames are paced
/// at `pace_fps` while the encoder was opened for `enc_fps`: x264's rate
/// control budgets per frame at its own frame rate, so the numbers are
/// scaled to give each frame `kbps / pace_fps`, and the VBV holds two such
/// frames (at least 200 kbit), as the FFmpeg program's `-bufsize` did.
pub fn x264_rate(
    kbps: u32,
    max_kbps: u32,
    pace_fps: u32,
    enc_fps: u32,
) -> horcrux_media::x264::Rate {
    let pace = pace_fps.clamp(1, enc_fps.max(1)) as u64;
    let enc = enc_fps.max(1) as u64;
    let scale = |k: u32| ((k.max(1) as u64 * enc / pace).min(u32::MAX as u64)) as u32;
    horcrux_media::x264::Rate {
        bitrate_kbps: scale(kbps),
        maxrate_kbps: scale(max_kbps.max(kbps)),
        bufsize_kbit: ((max_kbps.max(kbps) as u64 / pace).max(100) * 2) as u32,
    }
}

#[derive(Default)]
pub struct LibraryCtl {
    rate: std::sync::Mutex<Option<(u32, u32)>>,
    keyframe: AtomicBool,
    dead: AtomicBool,
}

/// Software H.264 in-process (libx264 on its own thread).
pub struct LibraryEncoder {
    pub params: EncodeParams,
    raw_tx: mpsc::Sender<(u64, Vec<u8>)>,
    ctl: Arc<LibraryCtl>,
    next_seq: u64,
}

fn x264_library(names: &[String]) -> Result<Arc<horcrux_media::x264::Library>, String> {
    type Cache = std::sync::Mutex<
        Option<(
            Vec<String>,
            Result<Arc<horcrux_media::x264::Library>, String>,
        )>,
    >;
    static LIB: OnceLock<Cache> = OnceLock::new();
    let mut g = LIB
        .get_or_init(Default::default)
        .lock()
        .map_err(|e| e.to_string())?;
    match g.as_ref() {
        Some((n, r)) if n == names => r.clone(),
        _ => {
            let r = horcrux_media::x264::Library::load(names);
            *g = Some((names.to_vec(), r.clone()));
            r
        }
    }
}

impl LibraryEncoder {
    pub fn spawn(
        settings: &ConsoleSettings,
        params: EncodeParams,
        out: mpsc::Sender<EncodedFrame>,
    ) -> Result<Self, String> {
        let lib = x264_library(&settings.x264_library)?;
        let config = horcrux_media::x264::Config {
            width: params.out_width,
            height: params.out_height,
            fps: params.fps.max(1),
            keyint: (params.fps * params.keyframe_interval_secs).max(1),
            preset: params.x264_preset.clone(),
            tune: "zerolatency".into(),
            profile: "baseline".into(),
            rate: x264_rate(
                params.bitrate_kbps,
                params.max_bitrate_kbps,
                params.fps,
                params.fps,
            ),
        };
        let mut enc = horcrux_media::x264::Encoder::open(lib, config)?;
        let (raw_tx, mut raw_rx) = mpsc::channel::<(u64, Vec<u8>)>(1);
        let ctl = Arc::new(LibraryCtl::default());
        let c = ctl.clone();
        let p = params.clone();
        let enc_fps = params.fps.max(1);
        std::thread::Builder::new()
            .name("hxc-x264".into())
            .spawn(move || {
                let format = match p.format {
                    PixelFormat::Bgrx => horcrux_media::yuv::Packed::Bgrx,
                    PixelFormat::Rgbx => horcrux_media::yuv::Packed::Rgbx,
                    PixelFormat::Rgb565 => horcrux_media::yuv::Packed::Rgb565,
                };
                let (w, h) = (p.width as usize, p.height as usize);
                let stride = w * p.format.bytes_per_pixel();
                let (ow, oh) = (p.out_width as usize, p.out_height as usize);
                while let Some((seq, frame)) = raw_rx.blocking_recv() {
                    let want = c.rate.lock().ok().and_then(|mut r| r.take());
                    if let Some((kbps, fps)) = want {
                        if let Err(e) = enc.set_rate(x264_rate(kbps, kbps, fps, enc_fps)) {
                            tracing::warn!("console: x264 rate change: {e}");
                        }
                    }
                    let key = c.keyframe.swap(false, Ordering::Relaxed);
                    let src = horcrux_media::yuv::Source {
                        data: &frame,
                        width: w,
                        height: h,
                        stride,
                        format,
                    };
                    let res = enc.encode(
                        |y, ys, u, us, v, vs| {
                            horcrux_media::yuv::convert(
                                &src,
                                &mut horcrux_media::yuv::I420 {
                                    y,
                                    y_stride: ys,
                                    u,
                                    u_stride: us,
                                    v,
                                    v_stride: vs,
                                    width: ow,
                                    height: oh,
                                },
                            )
                        },
                        key,
                    );
                    match res {
                        Ok(data) if data.is_empty() => {}
                        Ok(data) => {
                            if out.blocking_send(EncodedFrame { data, seq }).is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::warn!("console: x264: {e}");
                            break;
                        }
                    }
                }
                c.dead.store(true, Ordering::Relaxed);
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            params,
            raw_tx,
            ctl,
            next_seq: 0,
        })
    }

    pub fn submit(&mut self, frame: Vec<u8>) -> Option<u64> {
        let seq = self.next_seq;
        match self.raw_tx.try_send((seq, frame)) {
            Ok(()) => {
                self.next_seq += 1;
                Some(seq)
            }
            Err(_) => None,
        }
    }

    pub fn alive(&mut self) -> bool {
        !self.ctl.dead.load(Ordering::Relaxed)
    }

    pub fn set_rate(&mut self, kbps: u32, fps: u32) {
        if let Ok(mut r) = self.ctl.rate.lock() {
            *r = Some((kbps.max(1), fps.max(1)));
        }
        self.params.bitrate_kbps = kbps;
        self.params.max_bitrate_kbps = kbps;
    }
}

/// The FFmpeg program as the encoder.
pub struct ProgramEncoder {
    pub params: EncodeParams,
    child: Child,
    raw_tx: mpsc::Sender<(u64, Vec<u8>)>,
    next_seq: u64,
}

impl ProgramEncoder {
    pub fn spawn(
        program: &str,
        params: EncodeParams,
        out: mpsc::Sender<EncodedFrame>,
    ) -> std::io::Result<Self> {
        let mut child = Command::new(program)
            .args(params.ffmpeg_args())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let mut stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let (raw_tx, raw_rx) = mpsc::channel::<(u64, Vec<u8>)>(1);
        let (seq_tx, mut seq_rx) = mpsc::unbounded_channel::<u64>();
        tokio::spawn(feed(stdin, raw_rx, seq_tx));
        let codec = params.codec;
        tokio::spawn(async move {
            let mut buf = vec![0u8; 256 * 1024];
            let mut ivf = IvfSplitter::default();
            let mut annexb = AnnexBSplitter::default();
            loop {
                // An H.264 access unit is only closed by the next frame's
                // AUD. Updates are damage-driven, so the next frame may be
                // a second away: when the encoder goes quiet for a moment
                // with a frame buffered, that frame is complete (FFmpeg
                // writes each packet in one go).
                let read = if codec == VideoCodec::H264 && annexb.pending().is_some() {
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(3),
                        stdout.read(&mut buf),
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(_) => {
                            let data = annexb.take_pending();
                            let seq = seq_rx.recv().await.unwrap_or(0);
                            if out.send(EncodedFrame { data, seq }).await.is_err() {
                                return;
                            }
                            continue;
                        }
                    }
                } else {
                    stdout.read(&mut buf).await
                };
                let n = match read {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                let frames = match codec {
                    VideoCodec::Vp8 => ivf.push(&buf[..n]),
                    VideoCodec::H264 => annexb.push(&buf[..n]),
                };
                for data in frames {
                    let seq = seq_rx.recv().await.unwrap_or(0);
                    if out.send(EncodedFrame { data, seq }).await.is_err() {
                        return;
                    }
                }
            }
        });
        tokio::spawn(async move {
            let mut s = String::new();
            let _ = stderr.read_to_string(&mut s).await;
            if !s.trim().is_empty() {
                tracing::warn!("console encoder: {}", s.trim());
            }
        });
        Ok(Self {
            params,
            child,
            raw_tx,
            next_seq: 0,
        })
    }

    /// Hand a raw frame to the encoder. Returns its sequence number, or
    /// None when the encoder is still busy with the previous one (the
    /// caller keeps the frame dirty and retries on the next tick).
    pub fn submit(&mut self, frame: Vec<u8>) -> Option<u64> {
        let seq = self.next_seq;
        match self.raw_tx.try_send((seq, frame)) {
            Ok(()) => {
                self.next_seq += 1;
                Some(seq)
            }
            Err(_) => None,
        }
    }

    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

async fn feed(
    mut stdin: ChildStdin,
    mut rx: mpsc::Receiver<(u64, Vec<u8>)>,
    seq_tx: mpsc::UnboundedSender<u64>,
) {
    while let Some((seq, frame)) = rx.recv().await {
        let _ = seq_tx.send(seq);
        if stdin.write_all(&frame).await.is_err() {
            break;
        }
    }
}

/// What this node's FFmpeg can do, probed once.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct EncoderCaps {
    pub ffmpeg: bool,
    pub libx264: bool,
    pub libvpx: bool,
    pub vaapi_h264: bool,
    pub nvenc_h264: bool,
    pub error: Option<String>,
}

impl EncoderCaps {
    pub fn codecs(&self) -> Vec<VideoCodec> {
        let mut v = Vec::new();
        if self.libx264 || self.vaapi_h264 || self.nvenc_h264 {
            v.push(VideoCodec::H264);
        }
        if self.libvpx {
            v.push(VideoCodec::Vp8);
        }
        v
    }

    /// Backend used for H.264 under the node's preference.
    pub fn h264_backend(&self, pref: EncoderPreference) -> Option<EncoderBackend> {
        match pref {
            EncoderPreference::Software => self.libx264.then_some(EncoderBackend::Software),
            EncoderPreference::Vaapi => self.vaapi_h264.then_some(EncoderBackend::Vaapi),
            EncoderPreference::Nvenc => self.nvenc_h264.then_some(EncoderBackend::Nvenc),
            EncoderPreference::Auto => {
                if self.nvenc_h264 {
                    Some(EncoderBackend::Nvenc)
                } else if self.vaapi_h264 {
                    Some(EncoderBackend::Vaapi)
                } else if self.libx264 {
                    Some(EncoderBackend::Software)
                } else {
                    None
                }
            }
        }
    }
}

/// Encode one tiny test frame with `encoder` (hardware encoders are listed
/// by `-encoders` even without a usable device; only a real encode proves
/// one works).
async fn try_encode(program: &str, pre: &[&str], vf: &str, encoder: &str) -> bool {
    let mut args: Vec<&str> = vec!["-hide_banner", "-loglevel", "error", "-nostdin"];
    args.extend_from_slice(pre);
    args.extend_from_slice(&[
        "-f",
        "lavfi",
        "-i",
        "color=c=black:s=64x64:r=1",
        "-frames:v",
        "1",
    ]);
    if !vf.is_empty() {
        args.extend_from_slice(&["-vf", vf]);
    }
    args.extend_from_slice(&["-c:v", encoder, "-f", "null", "-"]);
    let run = Command::new(program)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status();
    matches!(
        tokio::time::timeout(std::time::Duration::from_secs(15), run).await,
        Ok(Ok(s)) if s.success()
    )
}

pub async fn probe(settings: &ConsoleSettings) -> EncoderCaps {
    let program = settings.ffmpeg_path.as_str();
    let listed = match Command::new(program)
        .args(["-hide_banner", "-encoders"])
        .stdin(Stdio::null())
        .output()
        .await
    {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        Ok(o) => {
            return EncoderCaps {
                error: Some(format!(
                    "{program} -encoders failed: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                )),
                ..Default::default()
            }
        }
        Err(e) => {
            return EncoderCaps {
                error: Some(format!("{program} not runnable: {e} (install FFmpeg)")),
                ..Default::default()
            }
        }
    };
    let has = |name: &str| listed.split_whitespace().any(|w| w == name);
    let mut caps = EncoderCaps {
        ffmpeg: true,
        libx264: has("libx264"),
        libvpx: has("libvpx"),
        ..Default::default()
    };
    if has("h264_nvenc") {
        caps.nvenc_h264 = try_encode(program, &[], "format=yuv420p", "h264_nvenc").await;
    }
    if has("h264_vaapi") && std::path::Path::new(&settings.vaapi_device).exists() {
        caps.vaapi_h264 = try_encode(
            program,
            &["-vaapi_device", &settings.vaapi_device],
            "format=nv12,hwupload",
            "h264_vaapi",
        )
        .await;
    }
    if caps.codecs().is_empty() {
        caps.error = Some("FFmpeg has none of libx264, libvpx, h264_vaapi, h264_nvenc".into());
    }
    caps
}

/// Cached probe (per process; the settings' ffmpeg path rarely changes and a
/// restart re-probes).
pub async fn caps(settings: &ConsoleSettings) -> EncoderCaps {
    static CAPS: OnceLock<tokio::sync::OnceCell<EncoderCaps>> = OnceLock::new();
    CAPS.get_or_init(tokio::sync::OnceCell::new)
        .get_or_init(|| probe(settings))
        .await
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_aspect_and_even() {
        assert_eq!(fit(1024, 768, 3840, 2160), (1024, 768));
        assert_eq!(fit(721, 401, 3840, 2160), (720, 400));
        assert_eq!(fit(7680, 4320, 3840, 2160), (3840, 2160));
        assert_eq!(fit(5120, 1440, 3840, 2160), (3840, 1080));
    }

    #[test]
    fn pixman_formats() {
        assert_eq!(
            PixelFormat::from_pixman(0x20020888),
            Some(PixelFormat::Bgrx)
        );
        assert_eq!(
            PixelFormat::from_pixman(0x10020565),
            Some(PixelFormat::Rgb565)
        );
        assert_eq!(PixelFormat::from_pixman(0x1234), None);
    }

    fn params(codec: VideoCodec, backend: EncoderBackend) -> EncodeParams {
        let s = ConsoleSettings::default();
        EncodeParams::new(codec, backend, 1280, 800, PixelFormat::Bgrx, &s.desktop, &s)
    }

    #[test]
    fn ffmpeg_args_are_low_delay() {
        let a = params(VideoCodec::H264, EncoderBackend::Software)
            .ffmpeg_args()
            .join(" ");
        assert!(
            a.contains("-f rawvideo -pix_fmt bgr0 -s 1280x800 -r 30 -i pipe:0"),
            "{a}"
        );
        assert!(
            a.contains("-c:v libx264 -preset ultrafast -tune zerolatency"),
            "{a}"
        );
        assert!(a.contains("-bf 0 -g 120"), "{a}");
        assert!(a.contains("h264_metadata=aud=insert -f h264 pipe:1"), "{a}");
        assert!(a.contains("-fps_mode passthrough"), "{a}");
        let v = params(VideoCodec::Vp8, EncoderBackend::Vaapi);
        assert_eq!(v.backend, EncoderBackend::Software, "VP8 is always libvpx");
        let a = v.ffmpeg_args().join(" ");
        assert!(a.contains("-c:v libvpx -deadline realtime") && a.contains("-lag-in-frames 0"));
        assert!(a.ends_with("-f ivf pipe:1"));
        let va = params(VideoCodec::H264, EncoderBackend::Vaapi)
            .ffmpeg_args()
            .join(" ");
        assert!(va.starts_with(
            "-hide_banner -loglevel error -nostdin -vaapi_device /dev/dri/renderD128"
        ));
        assert!(va.contains("format=nv12,hwupload") && va.contains("h264_vaapi"));
    }

    #[test]
    fn x264_rate_scales_to_the_paced_frame_rate() {
        // paced at the encoder's rate: as asked, two frames of VBV
        let r = x264_rate(3000, 3000, 30, 30);
        assert_eq!(
            (r.bitrate_kbps, r.maxrate_kbps, r.bufsize_kbit),
            (3000, 3000, 200)
        );
        let r = x264_rate(20000, 20000, 60, 60);
        assert_eq!(r.bufsize_kbit, 666);
        // paced at half the encoder's rate: each frame gets kbps/15
        let r = x264_rate(3000, 3000, 15, 30);
        assert_eq!(
            (r.bitrate_kbps, r.maxrate_kbps, r.bufsize_kbit),
            (6000, 6000, 400)
        );
        // pacing faster than the encoder's rate is clamped
        assert_eq!(x264_rate(1000, 1000, 120, 30).bitrate_kbps, 1000);
    }

    #[test]
    fn in_process_encoder_falls_back_to_the_program() {
        let s = ConsoleSettings {
            x264_library: vec!["libdoes-not-exist-hx.so".into()],
            ffmpeg_path: "/bin/cat".into(),
            ..Default::default()
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let (tx, _rx) = mpsc::channel(4);
            let p = params(VideoCodec::H264, EncoderBackend::Software);
            let e = Encoder::spawn(&s, p, tx).unwrap();
            assert_eq!(e.kind(), "ffmpeg");
            let s2 = ConsoleSettings {
                x264_in_process: false,
                ffmpeg_path: "/bin/cat".into(),
                ..Default::default()
            };
            let (tx, _rx) = mpsc::channel(4);
            let p = params(VideoCodec::H264, EncoderBackend::Software);
            assert_eq!(
                Encoder::spawn(&s2, p, tx).unwrap().kind(),
                "ffmpeg",
                "setting off"
            );
        });
    }

    /// Where libx264 is installed (CI's Test image): the session's encoder
    /// runs in-process, changes rate without a new keyframe and produces
    /// one access unit per frame.
    #[test]
    fn in_process_encoder_changes_rate_without_restart() {
        let s = ConsoleSettings::default();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let (tx, mut rx) = mpsc::channel(4);
            let mut p = params(VideoCodec::H264, EncoderBackend::Software);
            p.width = 320;
            p.height = 240;
            p.out_width = 320;
            p.out_height = 240;
            let mut e = match Encoder::spawn(&s, p, tx) {
                Ok(e @ Encoder::Library(_)) => e,
                _ => {
                    eprintln!("skip: no libx264 on this host");
                    return;
                }
            };
            let mut idr = Vec::new();
            for i in 0..12u32 {
                if i == 4 {
                    assert!(e.set_rate(300, 15));
                }
                if i == 9 {
                    assert!(e.request_keyframe());
                }
                let frame = vec![(i * 20) as u8; 320 * 240 * 4];
                let seq = loop {
                    if let Some(s) = e.submit(frame.clone()) {
                        break s;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                };
                let f = rx.recv().await.unwrap();
                assert_eq!(f.seq, seq);
                let nal = |t: u8| {
                    f.data
                        .windows(4)
                        .any(|w| w[..3] == [0, 0, 1] && w[3] & 0x1f == t)
                };
                idr.push(nal(5));
            }
            assert!(e.alive());
            assert_eq!(
                idr,
                vec![
                    true, false, false, false, false, false, false, false, false, true, false,
                    false
                ]
            );
        });
    }

    #[test]
    fn ivf_split() {
        let mut s = IvfSplitter::default();
        let mut stream = vec![0u8; 32];
        stream[6] = 32;
        for (i, len) in [3usize, 5].iter().enumerate() {
            stream.extend_from_slice(&(*len as u32).to_le_bytes());
            stream.extend_from_slice(&[0u8; 8]);
            stream.extend(std::iter::repeat_n(i as u8 + 1, *len));
        }
        // fed in awkward chunks
        let mut frames = Vec::new();
        for c in stream.chunks(7) {
            frames.extend(s.push(c));
        }
        assert_eq!(frames, vec![vec![1, 1, 1], vec![2, 2, 2, 2, 2]]);
    }

    #[test]
    fn annexb_split_at_aud() {
        let aud = [0u8, 0, 0, 1, 0x09, 0xf0];
        let idr = [0u8, 0, 0, 1, 0x65, 1, 2, 3];
        let p = [0u8, 0, 1, 0x41, 9, 9];
        let mut stream = Vec::new();
        stream.extend_from_slice(&aud);
        stream.extend_from_slice(&idr);
        stream.extend_from_slice(&aud);
        stream.extend_from_slice(&p);
        stream.extend_from_slice(&aud);
        let mut s = AnnexBSplitter::default();
        let mut frames = Vec::new();
        for c in stream.chunks(5) {
            frames.extend(s.push(c));
        }
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0], [&aud[..], &idr[..]].concat());
        assert_eq!(frames[1], [&aud[..], &p[..]].concat());
        assert_eq!(s.pending(), Some(&aud[..]));
    }
}
