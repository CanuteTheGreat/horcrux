//! Audio plumbing for the agent's own capture paths (the Windows WASAPI
//! loopback capture): any PCM or float mix format -> interleaved `f32` at the
//! stream's rate and channel count, silence fill while the endpoint plays
//! nothing, and fixed-length frames for the Opus encoder.
//!
//! Platform-independent on purpose, so every conversion is unit-tested on
//! any build host; the Windows module only moves bytes between WASAPI and
//! these types.
#![cfg_attr(not(windows), allow(dead_code))]

use horcrux_guest_proto::StartAudio;

/// Stream parameters derived from a `StartAudio` request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamParams {
    /// Output sample rate (an Opus rate: 8, 12, 16, 24 or 48 kHz).
    pub rate: u32,
    /// Output channels (1 or 2).
    pub channels: u8,
    /// Frame length in ms (an Opus frame length: 5, 10, 20, 40 or 60).
    pub frame_ms: u32,
    /// Constant bitrate in bit/s.
    pub bitrate_bps: u32,
}

impl StreamParams {
    /// Validate a request; zero fields take the protocol defaults
    /// (48 kHz, stereo, 10 ms) and `default_kbps`.
    pub fn from_request(a: &StartAudio, default_kbps: u32) -> Result<Self, String> {
        if !a.codec.is_empty() && !a.codec.eq_ignore_ascii_case("opus") {
            return Err(format!("codec {} is not supported (opus only)", a.codec));
        }
        let rate = if a.sample_rate == 0 {
            48000
        } else {
            a.sample_rate
        };
        if ![8000, 12000, 16000, 24000, 48000].contains(&rate) {
            return Err(format!("sample rate {rate} is not an Opus rate"));
        }
        let channels = if a.channels == 0 { 2 } else { a.channels };
        if channels > 2 {
            return Err(format!("{channels} channels requested (1 or 2 supported)"));
        }
        let frame_ms = if a.frame_ms == 0 { 10 } else { a.frame_ms };
        if ![5, 10, 20, 40, 60].contains(&frame_ms) {
            return Err(format!(
                "frame length {frame_ms} ms is not an Opus frame length"
            ));
        }
        let kbps = if a.bitrate_kbps == 0 {
            default_kbps
        } else {
            a.bitrate_kbps
        };
        // libopus accepts 6..510 kbit/s.
        let bitrate_bps = kbps.clamp(6, 510) * 1000;
        Ok(Self {
            rate,
            channels,
            frame_ms,
            bitrate_bps,
        })
    }

    /// Samples per channel in one frame.
    pub fn frame_len(&self) -> usize {
        (self.rate * self.frame_ms / 1000) as usize
    }
}

/// Sample encoding of a capture format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleKind {
    Float,
    Int,
}

/// A capture (mix) format: what a `WAVEFORMATEX` / `WAVEFORMATEXTENSIBLE`
/// describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InFormat {
    pub rate: u32,
    pub channels: u16,
    /// Container bits per sample (8, 16, 24, 32, 64).
    pub bits: u16,
    /// Valid bits (left-justified in the container).
    pub valid_bits: u16,
    pub kind: SampleKind,
    /// Speaker positions (`SPEAKER_*` bits); 0 = the default for the channel count.
    pub channel_mask: u32,
}

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xfffe;
/// Bytes 4..16 of every `KSDATAFORMAT_SUBTYPE_*` GUID built from a format tag
/// (`xxxxxxxx-0000-0010-8000-00aa00389b71`, little-endian layout).
const KS_SUBTYPE_TAIL: [u8; 12] = [
    0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71,
];

impl InFormat {
    /// Parse the raw bytes of a `WAVEFORMATEX` (18 bytes + `cbSize`).
    pub fn from_waveformat(b: &[u8]) -> Result<Self, String> {
        let u16at = |o: usize| b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]));
        let u32at = |o: usize| {
            b.get(o..o + 4)
                .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        };
        let short = || "WAVEFORMATEX too short".to_string();
        let tag = u16at(0).ok_or_else(short)?;
        let channels = u16at(2).ok_or_else(short)?;
        let rate = u32at(4).ok_or_else(short)?;
        let bits = u16at(14).ok_or_else(short)?;
        let cb = u16at(16).unwrap_or(0);
        let (kind, valid_bits, mask) = match tag {
            WAVE_FORMAT_PCM => (SampleKind::Int, bits, 0),
            WAVE_FORMAT_IEEE_FLOAT => (SampleKind::Float, bits, 0),
            WAVE_FORMAT_EXTENSIBLE => {
                if cb < 22 || b.len() < 40 {
                    return Err("WAVEFORMATEXTENSIBLE too short".into());
                }
                let valid = u16at(18).unwrap_or(bits);
                let mask = u32at(20).unwrap_or(0);
                let sub = u32at(24).unwrap_or(0);
                if b[28..40] != KS_SUBTYPE_TAIL {
                    return Err("unsupported mix format subtype".into());
                }
                let kind = match sub as u16 {
                    WAVE_FORMAT_PCM => SampleKind::Int,
                    WAVE_FORMAT_IEEE_FLOAT => SampleKind::Float,
                    t => return Err(format!("unsupported mix format subtype {t:#x}")),
                };
                (kind, if valid == 0 { bits } else { valid }, mask)
            }
            t => return Err(format!("unsupported mix format tag {t:#x}")),
        };
        let f = Self {
            rate,
            channels,
            bits,
            valid_bits,
            kind,
            channel_mask: mask,
        };
        f.check()?;
        Ok(f)
    }

    pub fn check(&self) -> Result<(), String> {
        let ok = match self.kind {
            SampleKind::Float => matches!(self.bits, 32 | 64),
            SampleKind::Int => matches!(self.bits, 8 | 16 | 24 | 32),
        };
        if !ok {
            return Err(format!("unsupported sample format {self:?}"));
        }
        if self.channels == 0 || self.rate == 0 {
            return Err(format!("invalid format {self:?}"));
        }
        Ok(())
    }

    /// Bytes per frame (all channels).
    pub fn block_align(&self) -> usize {
        self.channels as usize * (self.bits as usize / 8)
    }

    /// Append the samples in `b` (whole frames, interleaved) as `f32`.
    pub fn decode(&self, b: &[u8], out: &mut Vec<f32>) {
        let w = self.bits as usize / 8;
        let n = b.len() / self.block_align() * self.channels as usize;
        out.reserve(n);
        let b = &b[..n * w];
        match (self.kind, self.bits) {
            (SampleKind::Float, 32) => {
                out.extend(b.as_chunks::<4>().0.iter().map(|s| f32::from_le_bytes(*s)))
            }
            (SampleKind::Float, _) => out.extend(
                b.as_chunks::<8>()
                    .0
                    .iter()
                    .map(|s| f64::from_le_bytes(*s) as f32),
            ),
            (SampleKind::Int, 8) => out.extend(b.iter().map(|&s| (s as f32 - 128.0) / 128.0)),
            (SampleKind::Int, 16) => out.extend(
                b.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|s| i16::from_le_bytes(*s) as f32 / 32768.0),
            ),
            (SampleKind::Int, 24) => out.extend(
                b.as_chunks::<3>()
                    .0
                    .iter()
                    .map(|s| i32::from_le_bytes([0, s[0], s[1], s[2]]) as f32 / 2147483648.0),
            ),
            (SampleKind::Int, _) => out.extend(
                b.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|s| i32::from_le_bytes(*s) as f32 / 2147483648.0),
            ),
        }
    }
}

/// `SPEAKER_*` mask a format without one implies (Windows' defaults).
fn default_mask(channels: u16) -> u32 {
    match channels {
        1 => 0x4,
        2 => 0x3,
        3 => 0x7,
        4 => 0x33,
        5 => 0x37,
        6 => 0x3f,
        7 => 0x13f,
        8 => 0x63f,
        _ => 0,
    }
}

/// (left, right) weight of a speaker position (bit index of `SPEAKER_*`).
fn speaker_weight(bit: u32) -> (f32, f32) {
    const H: f32 = std::f32::consts::FRAC_1_SQRT_2;
    match bit {
        0 | 6 => (1.0, 0.0),            // front left, front left of center
        1 | 7 => (0.0, 1.0),            // front right, front right of center
        2 => (H, H),                    // front center
        3 => (0.0, 0.0),                // LFE: dropped
        4 | 9 | 12 | 15 => (H, 0.0),    // back/side/top-front/top-back left
        5 | 10 | 14 | 17 => (0.0, H),   // right counterparts
        8 | 11 | 13 | 16 => (0.5, 0.5), // back/top centers
        _ => (0.0, 0.0),
    }
}

/// Channel mixer: any input layout -> mono or stereo.
#[derive(Debug, Clone)]
pub struct Downmix {
    weights: Vec<(f32, f32)>,
    out_channels: u8,
    identity: bool,
}

impl Downmix {
    pub fn new(in_channels: u16, mask: u32, out_channels: u8) -> Self {
        let mask = if mask == 0 {
            default_mask(in_channels)
        } else {
            mask
        };
        let mut weights = Vec::with_capacity(in_channels as usize);
        let mut bits = (0..32).filter(|b| mask & (1 << b) != 0);
        for _ in 0..in_channels {
            weights.push(bits.next().map(speaker_weight).unwrap_or((0.0, 0.0)));
        }
        if in_channels == 1 {
            // a mono source plays at full level on both sides
            weights[0] = (1.0, 1.0);
        }
        let identity =
            (in_channels == 2 && out_channels == 2 && weights == [(1.0, 0.0), (0.0, 1.0)])
                || (in_channels == 1 && out_channels == 1);
        Self {
            weights,
            out_channels,
            identity,
        }
    }

    /// Mix interleaved input frames into `out` (interleaved, `out_channels`).
    pub fn apply(&self, input: &[f32], out: &mut Vec<f32>) {
        if self.identity {
            out.extend_from_slice(input);
            return;
        }
        let n = self.weights.len();
        for f in input.chunks_exact(n) {
            let (mut l, mut r) = (0.0f32, 0.0f32);
            for (s, (wl, wr)) in f.iter().zip(&self.weights) {
                l += s * wl;
                r += s * wr;
            }
            if self.out_channels == 1 {
                out.push(((l + r) * 0.5).clamp(-1.0, 1.0));
            } else {
                out.push(l.clamp(-1.0, 1.0));
                out.push(r.clamp(-1.0, 1.0));
            }
        }
    }
}

/// Streaming band-limited resampler (windowed sinc, Blackman window,
/// table-driven), for interleaved `f32` with a fixed channel count.
#[derive(Debug, Clone)]
pub struct Resampler {
    channels: usize,
    step: f64,
    half: usize,
    phases: usize,
    table: Vec<f32>,
    buf: Vec<f32>,
    pos: f64,
    bypass: bool,
}

impl Resampler {
    /// Kernel half-width in samples at the lower of the two rates.
    const HALF_TAPS: usize = 32;
    const PHASES: usize = 256;

    pub fn new(in_rate: u32, out_rate: u32, channels: usize) -> Self {
        let ratio = (out_rate as f64 / in_rate as f64).min(1.0);
        // when downsampling the kernel widens with the ratio (in input samples)
        let half = (Self::HALF_TAPS as f64 / ratio).ceil() as usize;
        let phases = Self::PHASES;
        let step = in_rate as f64 / out_rate as f64;
        // cutoff relative to the input Nyquist frequency: 90 % of the lower
        // Nyquist, so the Blackman transition band ends below it
        let fc = ratio * 0.9;
        let len = 2 * half * phases + 1;
        let mut table = Vec::with_capacity(len);
        for i in 0..len {
            let x = i as f64 / phases as f64 - half as f64; // distance in input samples
            let t = x / half as f64;
            let w = 0.42
                + 0.5 * (std::f64::consts::PI * t).cos()
                + 0.08 * (2.0 * std::f64::consts::PI * t).cos();
            let a = std::f64::consts::PI * fc * x;
            let sinc = if a.abs() < 1e-12 { 1.0 } else { a.sin() / a };
            table.push((fc * sinc * w) as f32);
        }
        Self {
            channels,
            step,
            half,
            phases,
            table,
            // start with `half` frames of silence so the first output sample
            // is centered on the first input sample
            buf: vec![0.0; half * channels],
            pos: half as f64,
            bypass: in_rate == out_rate,
        }
    }

    fn kernel(&self, x: f64) -> f32 {
        let p = (x + self.half as f64) * self.phases as f64;
        if p <= 0.0 || p >= (self.table.len() - 1) as f64 {
            return 0.0;
        }
        let i = p as usize;
        let f = (p - i as f64) as f32;
        self.table[i] + (self.table[i + 1] - self.table[i]) * f
    }

    /// Feed interleaved frames; append every output frame now computable.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.bypass {
            out.extend_from_slice(input);
            return;
        }
        let ch = self.channels;
        self.buf.extend_from_slice(input);
        let frames = self.buf.len() / ch;
        let mut acc = vec![0.0f32; ch];
        while (self.pos.floor() as usize) + self.half < frames {
            let center = self.pos.floor() as isize;
            let frac = self.pos - center as f64;
            acc.iter_mut().for_each(|a| *a = 0.0);
            let mut wsum = 0.0f32;
            for k in (1 - self.half as isize)..=(self.half as isize) {
                let idx = center + k;
                if idx < 0 {
                    continue;
                }
                let w = self.kernel(k as f64 - frac);
                wsum += w;
                let base = idx as usize * ch;
                for (c, a) in acc.iter_mut().enumerate() {
                    *a += self.buf[base + c] * w;
                }
            }
            let norm = if wsum.abs() > 1e-6 { 1.0 / wsum } else { 1.0 };
            out.extend(acc.iter().map(|a| (a * norm).clamp(-1.0, 1.0)));
            self.pos += self.step;
        }
        // drop input no later output needs
        let keep_from = (self.pos.floor() as usize).saturating_sub(self.half);
        if keep_from > 0 {
            self.buf.drain(..keep_from * ch);
            self.pos -= keep_from as f64;
        }
    }
}

/// One capture format -> stream samples (decode, mix, resample).
#[derive(Debug, Clone)]
pub struct Converter {
    pub fmt: InFormat,
    mix: Downmix,
    rs: Resampler,
    tmp: Vec<f32>,
    mixed: Vec<f32>,
}

impl Converter {
    pub fn new(fmt: InFormat, p: &StreamParams) -> Self {
        Self {
            fmt,
            mix: Downmix::new(fmt.channels, fmt.channel_mask, p.channels),
            rs: Resampler::new(fmt.rate, p.rate, p.channels as usize),
            tmp: Vec::new(),
            mixed: Vec::new(),
        }
    }

    /// Convert captured bytes (whole frames); output appended to `out`.
    pub fn push(&mut self, bytes: &[u8], out: &mut Vec<f32>) {
        self.tmp.clear();
        self.fmt.decode(bytes, &mut self.tmp);
        self.finish(out);
    }

    /// `frames` of silence in the capture format (a packet flagged silent).
    pub fn push_silent(&mut self, frames: usize, out: &mut Vec<f32>) {
        self.tmp.clear();
        self.tmp.resize(frames * self.fmt.channels as usize, 0.0);
        self.finish(out);
    }

    fn finish(&mut self, out: &mut Vec<f32>) {
        self.mixed.clear();
        self.mix.apply(&self.tmp, &mut self.mixed);
        self.rs.process(&self.mixed, out);
    }
}

/// Output side: buffers converted samples, fills silence while the endpoint
/// is idle (WASAPI loopback delivers nothing while nothing plays, but the
/// stream must keep its frame cadence) and cuts frames for the encoder.
///
/// Positions are in stream frames (samples per channel) since the stream
/// started; `now` comes from the caller's clock.
#[derive(Debug, Clone)]
pub struct Stream {
    channels: usize,
    frame_len: usize,
    idle: u64,
    max_fill: u64,
    buf: Vec<f32>,
    /// Stream position just past the last buffered sample.
    end: u64,
    last_data: Option<u64>,
}

impl Stream {
    /// `idle`: no data for this many frames -> fill silence up to `now`;
    /// `max_fill`: longest gap filled at once (after a stall the stream
    /// skips ahead instead of sending a burst of old silence).
    pub fn new(p: &StreamParams, idle: u64, max_fill: u64) -> Self {
        Self {
            channels: p.channels as usize,
            frame_len: p.frame_len(),
            idle,
            max_fill: max_fill.max(p.frame_len() as u64),
            buf: Vec::new(),
            end: 0,
            last_data: None,
        }
    }

    /// Converted capture samples (interleaved) that arrived at `now`.
    pub fn push(&mut self, samples: &[f32], now: u64) {
        self.buf.extend_from_slice(samples);
        self.end += (samples.len() / self.channels) as u64;
        self.last_data = Some(now);
    }

    /// Fill silence up to `now` when the endpoint has been idle; returns the
    /// frames of silence added.
    pub fn fill(&mut self, now: u64) -> u64 {
        let idle = match self.last_data {
            None => true,
            Some(t) => now.saturating_sub(t) >= self.idle,
        };
        if !idle || now <= self.end {
            return 0;
        }
        let mut gap = now - self.end;
        if gap > self.max_fill {
            // a stall (VM paused, thread starved): skip the old part
            self.end = now - self.max_fill;
            gap = self.max_fill;
        }
        self.buf
            .resize(self.buf.len() + gap as usize * self.channels, 0.0);
        self.end = now;
        gap
    }

    /// Next whole frame: (stream position of its first sample, samples).
    pub fn pop(&mut self) -> Option<(u64, Vec<f32>)> {
        let n = self.frame_len * self.channels;
        if self.buf.len() < n {
            return None;
        }
        let pos = self.end - (self.buf.len() / self.channels) as u64;
        let f: Vec<f32> = self.buf.drain(..n).collect();
        Some((pos, f))
    }

    /// While the endpoint is idle at `now`: the stream position at which
    /// silence fill completes the next frame (the caller sleeps until then,
    /// so idle frames leave on time instead of on the next poll tick).
    pub fn idle_frame_due(&self, now: u64) -> Option<u64> {
        let idle = match self.last_data {
            None => true,
            Some(t) => now.saturating_sub(t) >= self.idle,
        };
        if !idle {
            return None;
        }
        let have = (self.buf.len() / self.channels) as u64;
        Some(self.end.max(now) + (self.frame_len as u64).saturating_sub(have))
    }

    /// Frames buffered but not yet popped.
    #[cfg(test)]
    pub fn buffered(&self) -> usize {
        self.buf.len() / self.channels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> StreamParams {
        StreamParams::from_request(&StartAudio::default(), 128).unwrap()
    }

    fn waveformatex(
        tag: u16,
        ch: u16,
        rate: u32,
        bits: u16,
        ext: Option<(u16, u32, u16)>,
    ) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&tag.to_le_bytes());
        b.extend_from_slice(&ch.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * ch as u32 * bits as u32 / 8).to_le_bytes());
        b.extend_from_slice(&(ch * bits / 8).to_le_bytes());
        b.extend_from_slice(&bits.to_le_bytes());
        match ext {
            None => b.extend_from_slice(&0u16.to_le_bytes()),
            Some((valid, mask, sub)) => {
                b.extend_from_slice(&22u16.to_le_bytes());
                b.extend_from_slice(&valid.to_le_bytes());
                b.extend_from_slice(&mask.to_le_bytes());
                b.extend_from_slice(&(sub as u32).to_le_bytes());
                b.extend_from_slice(&KS_SUBTYPE_TAIL);
            }
        }
        b
    }

    /// Amplitude of `freq` in a mono signal (Goertzel, normalized).
    fn tone_level(x: &[f32], rate: f64, freq: f64) -> f64 {
        let w = 2.0 * std::f64::consts::PI * freq / rate;
        let (mut s1, mut s2) = (0.0f64, 0.0f64);
        for &v in x {
            let s = v as f64 + 2.0 * w.cos() * s1 - s2;
            s2 = s1;
            s1 = s;
        }
        let p = s1 * s1 + s2 * s2 - 2.0 * w.cos() * s1 * s2;
        2.0 * p.sqrt() / x.len() as f64
    }

    #[test]
    fn request_defaults_and_limits() {
        let p = params();
        assert_eq!(
            p,
            StreamParams {
                rate: 48000,
                channels: 2,
                frame_ms: 10,
                bitrate_bps: 128_000
            }
        );
        assert_eq!(p.frame_len(), 480);
        let mut a = StartAudio {
            codec: "opus".into(),
            sample_rate: 48000,
            channels: 2,
            bitrate_kbps: 96,
            frame_ms: 20,
        };
        let p = StreamParams::from_request(&a, 128).unwrap();
        assert_eq!((p.frame_len(), p.bitrate_bps), (960, 96_000));
        a.codec = "aac".into();
        assert!(StreamParams::from_request(&a, 128).is_err());
        a.codec = "opus".into();
        a.frame_ms = 15;
        assert!(StreamParams::from_request(&a, 128).is_err());
        a.frame_ms = 10;
        a.sample_rate = 44100;
        assert!(StreamParams::from_request(&a, 128).is_err());
        a.sample_rate = 48000;
        a.channels = 6;
        assert!(StreamParams::from_request(&a, 128).is_err());
        a.channels = 2;
        a.bitrate_kbps = 9999;
        assert_eq!(
            StreamParams::from_request(&a, 128).unwrap().bitrate_bps,
            510_000
        );
    }

    #[test]
    fn parses_mix_formats() {
        // the usual shared-mode mix format: extensible float32 stereo 48 kHz
        let f = InFormat::from_waveformat(&waveformatex(0xfffe, 2, 48000, 32, Some((32, 3, 3))))
            .unwrap();
        assert_eq!(
            f,
            InFormat {
                rate: 48000,
                channels: 2,
                bits: 32,
                valid_bits: 32,
                kind: SampleKind::Float,
                channel_mask: 3
            }
        );
        let f = InFormat::from_waveformat(&waveformatex(0xfffe, 6, 44100, 32, Some((24, 0x3f, 1))))
            .unwrap();
        assert_eq!(
            (f.kind, f.valid_bits, f.channel_mask, f.block_align()),
            (SampleKind::Int, 24, 0x3f, 24)
        );
        let f = InFormat::from_waveformat(&waveformatex(1, 2, 44100, 16, None)).unwrap();
        assert_eq!((f.kind, f.bits, f.channel_mask), (SampleKind::Int, 16, 0));
        assert!(InFormat::from_waveformat(&waveformatex(0x55, 2, 44100, 16, None)).is_err());
        assert!(InFormat::from_waveformat(&waveformatex(3, 2, 44100, 16, None)).is_err());
        let mut bad = waveformatex(0xfffe, 2, 48000, 32, Some((32, 3, 3)));
        bad[30] = 0x99;
        assert!(InFormat::from_waveformat(&bad).is_err());
        assert!(InFormat::from_waveformat(&[1, 0, 2]).is_err());
    }

    #[test]
    fn decodes_every_sample_format() {
        let mk = |kind, bits, valid| InFormat {
            rate: 48000,
            channels: 1,
            bits,
            valid_bits: valid,
            kind,
            channel_mask: 0,
        };
        let mut o = Vec::new();
        mk(SampleKind::Float, 32, 32).decode(
            &[0.5f32.to_le_bytes(), (-1.0f32).to_le_bytes()].concat(),
            &mut o,
        );
        assert_eq!(o, [0.5, -1.0]);
        o.clear();
        mk(SampleKind::Float, 64, 64).decode(&0.25f64.to_le_bytes(), &mut o);
        assert_eq!(o, [0.25]);
        o.clear();
        mk(SampleKind::Int, 16, 16).decode(
            &[16384i16.to_le_bytes(), i16::MIN.to_le_bytes()].concat(),
            &mut o,
        );
        assert_eq!(o, [0.5, -1.0]);
        o.clear();
        // 24-bit packed: 0x400000 = 0.5
        mk(SampleKind::Int, 24, 24).decode(&[0x00, 0x00, 0x40, 0x00, 0x00, 0xc0], &mut o);
        assert_eq!(o, [0.5, -0.5]);
        o.clear();
        // 24 valid bits left-justified in 32
        mk(SampleKind::Int, 32, 24).decode(&0x4000_0000i32.to_le_bytes(), &mut o);
        assert_eq!(o, [0.5]);
        o.clear();
        mk(SampleKind::Int, 8, 8).decode(&[128, 192, 0], &mut o);
        assert_eq!(o, [0.0, 0.5, -1.0]);
        o.clear();
        // a trailing partial frame is ignored
        let st = InFormat {
            channels: 2,
            ..mk(SampleKind::Int, 16, 16)
        };
        st.decode(&[0, 0, 0, 0, 1], &mut o);
        assert_eq!(o.len(), 2);
    }

    #[test]
    fn downmix_layouts() {
        let mut o = Vec::new();
        // mono -> stereo at full level
        Downmix::new(1, 0, 2).apply(&[0.5, -0.25], &mut o);
        assert_eq!(o, [0.5, 0.5, -0.25, -0.25]);
        o.clear();
        // stereo passthrough
        Downmix::new(2, 3, 2).apply(&[0.1, 0.2], &mut o);
        assert_eq!(o, [0.1, 0.2]);
        o.clear();
        // stereo -> mono averages
        Downmix::new(2, 0, 1).apply(&[0.2, 0.4], &mut o);
        assert!((o[0] - 0.3).abs() < 1e-6);
        o.clear();
        // 5.1: FL FR FC LFE BL BR; LFE dropped, center at -3 dB on both sides
        Downmix::new(6, 0x3f, 2).apply(&[0.1, 0.2, 0.4, 1.0, 0.0, 0.3], &mut o);
        let h = std::f32::consts::FRAC_1_SQRT_2;
        assert!((o[0] - (0.1 + 0.4 * h)).abs() < 1e-6, "{o:?}");
        assert!((o[1] - (0.2 + 0.4 * h + 0.3 * h)).abs() < 1e-6, "{o:?}");
        o.clear();
        // 7.1 with no mask uses the default layout; stereo content in the
        // front pair keeps its level
        Downmix::new(8, 0, 2).apply(&[0.5, -0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], &mut o);
        assert_eq!(o, [0.5, -0.5]);
        o.clear();
        // clipping is clamped
        Downmix::new(6, 0x3f, 2).apply(&[1.0, 1.0, 1.0, 0.0, 1.0, 1.0], &mut o);
        assert_eq!(o, [1.0, 1.0]);
    }

    fn sine(rate: u32, freq: f64, amp: f64, n: usize, ch: usize) -> Vec<f32> {
        let mut v = Vec::with_capacity(n * ch);
        for i in 0..n {
            let s =
                (amp * (2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64).sin()) as f32;
            for _ in 0..ch {
                v.push(s);
            }
        }
        v
    }

    #[test]
    fn resampler_keeps_frequency_and_level() {
        for &(rin, freq) in &[
            (44100u32, 1000.0),
            (96000, 997.0),
            (32000, 440.0),
            (22050, 3000.0),
        ] {
            let input = sine(rin, freq, 0.5, rin as usize, 2);
            let mut rs = Resampler::new(rin, 48000, 2);
            let mut out = Vec::new();
            // uneven chunks, like WASAPI packets
            for c in input.chunks(2 * 441 + 6) {
                rs.process(c, &mut out);
            }
            let frames = out.len() / 2;
            assert!(
                (frames as i64 - 48000).abs() < 150,
                "{rin}: {frames} frames for 1 s"
            );
            let left: Vec<f32> = out.chunks(2).map(|f| f[0]).skip(1000).take(24000).collect();
            let lvl = tone_level(&left, 48000.0, freq);
            assert!((lvl - 0.5).abs() < 0.01, "{rin} Hz: level {lvl}");
            // nothing at a neighbouring frequency
            assert!(tone_level(&left, 48000.0, freq * 1.5) < 0.005);
            assert!(out.chunks(2).all(|f| f[0] == f[1]));
        }
    }

    #[test]
    fn resampler_chunking_does_not_change_output() {
        let input = sine(44100, 1234.0, 0.7, 10_000, 1);
        let mut a = Resampler::new(44100, 48000, 1);
        let mut one = Vec::new();
        a.process(&input, &mut one);
        let mut b = Resampler::new(44100, 48000, 1);
        let mut many = Vec::new();
        for c in input.chunks(37) {
            b.process(c, &mut many);
        }
        assert_eq!(one.len(), many.len());
        assert!(one.iter().zip(&many).all(|(x, y)| (x - y).abs() < 1e-6));
    }

    #[test]
    fn resampler_rejects_content_above_the_new_nyquist() {
        // 30 kHz at 96 kHz cannot exist at 48 kHz: must be filtered, not aliased to 18 kHz
        let input = sine(96000, 30000.0, 0.5, 96000, 1);
        let mut rs = Resampler::new(96000, 48000, 1);
        let mut out = Vec::new();
        rs.process(&input, &mut out);
        let lvl = tone_level(&out[1000..25000], 48000.0, 18000.0);
        assert!(lvl < 0.01, "alias at 18 kHz: {lvl}");
    }

    #[test]
    fn converter_float_48k_stereo_is_exact() {
        let p = params();
        let fmt = InFormat {
            rate: 48000,
            channels: 2,
            bits: 32,
            valid_bits: 32,
            kind: SampleKind::Float,
            channel_mask: 3,
        };
        let mut c = Converter::new(fmt, &p);
        let src = [0.25f32, -0.5, 0.125, 1.0];
        let bytes: Vec<u8> = src.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut out = Vec::new();
        c.push(&bytes, &mut out);
        assert_eq!(out, src);
        c.push_silent(3, &mut out);
        assert_eq!(out.len(), 10);
        assert!(out[4..].iter().all(|&s| s == 0.0));
    }

    #[test]
    fn converter_pcm16_44k_mono_to_48k_stereo() {
        let p = params();
        let fmt = InFormat {
            rate: 44100,
            channels: 1,
            bits: 16,
            valid_bits: 16,
            kind: SampleKind::Int,
            channel_mask: 0,
        };
        let mut c = Converter::new(fmt, &p);
        let bytes: Vec<u8> = sine(44100, 440.0, 0.5, 44100, 1)
            .iter()
            .flat_map(|s| ((s * 32767.0) as i16).to_le_bytes())
            .collect();
        let mut out = Vec::new();
        for ch in bytes.chunks(882) {
            c.push(ch, &mut out);
        }
        let l: Vec<f32> = out.chunks(2).map(|f| f[0]).skip(500).take(40000).collect();
        let r: Vec<f32> = out.chunks(2).map(|f| f[1]).skip(500).take(40000).collect();
        assert!((tone_level(&l, 48000.0, 440.0) - 0.5).abs() < 0.01);
        assert_eq!(l, r);
    }

    #[test]
    fn stream_fills_silence_only_while_idle() {
        let p = params(); // 480-sample frames
        let mut s = Stream::new(&p, 960, 9600);
        // nothing captured yet: silence right away up to now
        assert_eq!(s.fill(1000), 1000);
        let (pos, f) = s.pop().unwrap();
        assert_eq!((pos, f.len()), (0, 960));
        assert!(f.iter().all(|&x| x == 0.0));
        assert_eq!(s.pop().map(|x| x.0), Some(480));
        assert!(s.pop().is_none());
        assert_eq!(s.buffered(), 40);
        // data flows: no fill even when the endpoint clock lags a little
        s.push(&vec![0.5; 2 * 480], 1500);
        assert_eq!(s.fill(1600), 0);
        assert_eq!(s.fill(2000), 0);
        let (pos, f) = s.pop().unwrap();
        assert_eq!(pos, 960);
        assert_eq!(f[..80], [0.0; 80]);
        assert_eq!(f[80], 0.5);
        // data stops: after `idle` frames the gap is filled to now
        assert_eq!(s.fill(2459), 0);
        assert_eq!(s.fill(2460), 2460 - 1480);
        let mut last = 0;
        while let Some((pos, _)) = s.pop() {
            last = pos;
        }
        assert_eq!(last, 1920);
        // a long stall skips ahead instead of sending a burst
        assert_eq!(s.buffered(), 60);
        assert_eq!(s.fill(100_000), 9600);
        let (pos, _) = s.pop().unwrap();
        assert_eq!(pos, 100_000 - 9600 - 60);
    }

    #[test]
    fn idle_frame_due_points_at_the_next_frame() {
        let p = params(); // 480-sample frames
        let mut s = Stream::new(&p, 960, 9600);
        assert_eq!(s.idle_frame_due(0), Some(480));
        s.fill(1000);
        while s.pop().is_some() {}
        // 40 frames left over: 440 more complete the next frame
        assert_eq!(s.idle_frame_due(1000), Some(1440));
        s.fill(1440);
        assert_eq!(s.pop().map(|f| f.0), Some(960));
        assert!(s.pop().is_none());
        // fresh data: not idle, the caller polls the endpoint instead
        s.push(&[0.1; 20], 1500);
        assert_eq!(s.idle_frame_due(1600), None);
        assert!(s.idle_frame_due(2460).is_some());
    }

    #[test]
    fn stream_cadence_over_time_matches_the_clock() {
        // a silent endpoint polled every 5 ms for 10 s yields exactly one
        // 10 ms frame per 10 ms
        let p = params();
        let mut s = Stream::new(&p, 1920, 9600);
        let mut frames = 0;
        let mut expect_pos = 0;
        for tick in 1..=2000u64 {
            s.fill(tick * 240);
            while let Some((pos, f)) = s.pop() {
                assert_eq!(pos, expect_pos);
                assert_eq!(f.len(), 960);
                expect_pos += 480;
                frames += 1;
            }
        }
        assert_eq!(frames, 1000);
    }
}
