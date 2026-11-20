//! Horcrux guest-tools wire protocol (docs/design/horcrux-console.md
//! section 4.1).
//!
//! The node and the guest tools talk over one byte stream: a virtio-serial
//! port named [`PORT_NAME`] (QEMU `virtserialport`, a unix socket on the
//! node) or, where the node offers it, virtio-vsock. The stream is private
//! to one VM; nothing else can open the guest end.
//!
//! Framing: `len: u32 LE` (bytes that follow), `kind: u8`, payload
//! (`len - 1` bytes). Control messages carry JSON, media and input carry a
//! small binary header. Unknown kinds are skipped (the length says how
//! far), unknown JSON fields are ignored, so either side may be newer.
//!
//! This crate has no I/O of its own and no dependencies besides serde, so
//! the Windows and Linux guest tools build it for any target.
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

/// Protocol version spoken by this crate.
pub const VERSION: u32 = 1;
/// Default virtio-serial port name (`/dev/virtio-ports/<name>` in Linux
/// guests, `\\.\Global\<name>` in Windows guests). Configurable on both
/// ends (`[console] guest_tools_port`, guest tools `--port`).
pub const PORT_NAME: &str = "org.horcrux.console.0";
/// Largest frame either side accepts (a 4K keyframe at high quality fits).
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

/// Message kinds.
pub mod kind {
    pub const HELLO: u8 = 0x01;
    pub const HOST_HELLO: u8 = 0x02;
    pub const START_VIDEO: u8 = 0x10;
    pub const STOP_VIDEO: u8 = 0x11;
    pub const KEYFRAME: u8 = 0x12;
    pub const VIDEO: u8 = 0x13;
    pub const VIDEO_STATUS: u8 = 0x14;
    pub const CLIPBOARD: u8 = 0x20;
    pub const SET_LAYOUT: u8 = 0x30;
    pub const LAYOUT: u8 = 0x31;
    pub const INPUT: u8 = 0x40;
    pub const START_AUDIO: u8 = 0x50;
    pub const AUDIO: u8 = 0x51;
    pub const STOP_AUDIO: u8 = 0x52;
    pub const PING: u8 = 0x60;
    pub const PONG: u8 = 0x61;
    pub const LOG: u8 = 0x7f;
}

/// What the guest tools can do (sent once in [`Msg::Hello`]).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Caps {
    /// Codecs the in-guest encoder produces, best first (`h264`, `hevc`, `av1`).
    pub codecs: Vec<String>,
    /// Encoder backends found in the guest (`nvenc`, `amf`, `qsv`,
    /// `vaapi`, `mediafoundation`, `software`).
    pub encoders: Vec<String>,
    /// Capture method (`dxgi-dup`, `idd`, `kmsgrab`, `x11grab`, `pipewire`).
    pub capture: Vec<String>,
    pub clipboard: bool,
    pub resize: bool,
    pub multi_monitor: bool,
    /// Relative (raw) mouse injection.
    pub relative_mouse: bool,
    pub gamepad: bool,
    pub audio: bool,
    /// A Horcrux virtual display (Indirect Display Driver on Windows) is
    /// installed and can add monitors.
    pub virtual_display: bool,
    pub chroma444: bool,
    pub hdr: bool,
}

/// Guest -> node, first message after the port opens.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Hello {
    pub v: u32,
    /// `windows` or `linux`.
    pub os: String,
    pub os_version: String,
    pub agent_version: String,
    pub caps: Caps,
}

/// Node -> guest, answer to [`Hello`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HostHello {
    pub v: u32,
    /// Largest clipboard payload either side sends (bytes).
    pub clipboard_max_bytes: u32,
}

/// Node -> guest: start (or reconfigure) capture + encode.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct StartVideo {
    pub codec: String,
    /// 0 = the monitor's own size.
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub max_bitrate_kbps: u32,
    pub keyframe_interval_frames: u32,
    /// Constant-bitrate low-delay rate control, no B-frames, no lookahead.
    pub low_delay: bool,
    /// Send a frame every `1/fps` even when nothing changed (gaming)
    /// instead of only on damage (desktop).
    pub fixed_rate: bool,
    /// `420` or `444`.
    pub chroma: String,
    pub hdr: bool,
    /// Periodic intra refresh instead of big keyframes (smooth bitrate).
    pub intra_refresh: bool,
    pub monitor: u32,
}

impl Default for StartVideo {
    fn default() -> Self {
        Self {
            codec: "h264".into(),
            width: 0,
            height: 0,
            fps: 30,
            bitrate_kbps: 4000,
            max_bitrate_kbps: 12000,
            keyframe_interval_frames: 120,
            low_delay: true,
            fixed_rate: false,
            chroma: "420".into(),
            hdr: false,
            intra_refresh: false,
            monitor: 0,
        }
    }
}

/// Guest -> node: what the encoder actually runs with, or why it failed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct VideoStatus {
    pub running: bool,
    pub codec: String,
    pub encoder: String,
    pub capture: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub error: Option<String>,
}

/// Guest -> node: one encoded access unit (H.264/HEVC Annex B, AV1 OBUs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoPacket {
    pub keyframe: bool,
    pub monitor: u16,
    /// Guest monotonic clock when the frame was captured (microseconds).
    pub capture_us: u64,
    /// Guest microseconds spent between capture and handing the packet to
    /// the port (capture copy + encode).
    pub encode_us: u32,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Clipboard {
    /// `text/plain` (images and files are later phases).
    pub mime: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Monitor {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,
    pub primary: bool,
}

/// Node -> guest (`SET_LAYOUT`, sized to the client) and guest -> node
/// (`LAYOUT`, what the guest now shows).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Layout {
    pub monitors: Vec<Monitor>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct StartAudio {
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u8,
    pub bitrate_kbps: u32,
    /// Opus frame length in ms (10/20).
    pub frame_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioPacket {
    pub capture_us: u64,
    pub data: Vec<u8>,
}

/// A gamepad's full state (XInput-like layout; Linux maps it to an evdev
/// gamepad, Windows to the Horcrux virtual HID gamepad).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Gamepad {
    pub index: u8,
    /// Bit per button, W3C "standard" gamepad order (0 = A/south ... 16 = home).
    pub buttons: u32,
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
    pub lt: u8,
    pub rt: u8,
}

/// Input injected by the guest tools (node -> guest).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// QEMU "qnum" key number (PC XT set-1 scancode, 0x80 bit = E0 prefix).
    Key { qnum: u16, down: bool },
    /// 0 left, 1 middle, 2 right, 3 back, 4 forward.
    Button { button: u8, down: bool },
    /// Raw relative motion (gaming, pointer lock).
    Rel { dx: i32, dy: i32 },
    /// Absolute position in the given monitor's pixels.
    Abs { x: u32, y: u32, monitor: u16 },
    Wheel { dx: i32, dy: i32 },
    Gamepad(Gamepad),
}

/// One decoded message.
#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    Hello(Hello),
    HostHello(HostHello),
    StartVideo(StartVideo),
    StopVideo,
    Keyframe,
    Video(VideoPacket),
    VideoStatus(VideoStatus),
    Clipboard(Clipboard),
    SetLayout(Layout),
    Layout(Layout),
    Input(Input),
    StartAudio(StartAudio),
    Audio(AudioPacket),
    StopAudio,
    Ping(u64),
    Pong(u64),
    Log(String),
    /// A kind this version does not know (skipped by the receiver).
    Unknown(u8),
}

fn json<T: Serialize>(k: u8, v: &T) -> (u8, Vec<u8>) {
    (k, serde_json::to_vec(v).unwrap_or_default())
}

impl Msg {
    /// Kind byte and payload.
    pub fn encode(&self) -> (u8, Vec<u8>) {
        match self {
            Msg::Hello(h) => json(kind::HELLO, h),
            Msg::HostHello(h) => json(kind::HOST_HELLO, h),
            Msg::StartVideo(s) => json(kind::START_VIDEO, s),
            Msg::StopVideo => (kind::STOP_VIDEO, vec![]),
            Msg::Keyframe => (kind::KEYFRAME, vec![]),
            Msg::Video(p) => {
                let mut b = Vec::with_capacity(24 + p.data.len());
                b.push(p.keyframe as u8);
                b.push(0);
                b.extend_from_slice(&p.monitor.to_le_bytes());
                b.extend_from_slice(&p.capture_us.to_le_bytes());
                b.extend_from_slice(&p.encode_us.to_le_bytes());
                b.extend_from_slice(&(p.width as u16).to_le_bytes());
                b.extend_from_slice(&(p.height as u16).to_le_bytes());
                b.extend_from_slice(&p.data);
                (kind::VIDEO, b)
            }
            Msg::VideoStatus(s) => json(kind::VIDEO_STATUS, s),
            Msg::Clipboard(c) => json(kind::CLIPBOARD, c),
            Msg::SetLayout(l) => json(kind::SET_LAYOUT, l),
            Msg::Layout(l) => json(kind::LAYOUT, l),
            Msg::Input(i) => (kind::INPUT, encode_input(i)),
            Msg::StartAudio(a) => json(kind::START_AUDIO, a),
            Msg::Audio(a) => {
                let mut b = Vec::with_capacity(8 + a.data.len());
                b.extend_from_slice(&a.capture_us.to_le_bytes());
                b.extend_from_slice(&a.data);
                (kind::AUDIO, b)
            }
            Msg::StopAudio => (kind::STOP_AUDIO, vec![]),
            Msg::Ping(t) => (kind::PING, t.to_le_bytes().to_vec()),
            Msg::Pong(t) => (kind::PONG, t.to_le_bytes().to_vec()),
            Msg::Log(s) => (kind::LOG, s.as_bytes().to_vec()),
            Msg::Unknown(k) => (*k, vec![]),
        }
    }

    pub fn decode(k: u8, p: &[u8]) -> Result<Msg, String> {
        fn j<T: for<'de> Deserialize<'de>>(p: &[u8]) -> Result<T, String> {
            serde_json::from_slice(p).map_err(|e| e.to_string())
        }
        Ok(match k {
            kind::HELLO => Msg::Hello(j(p)?),
            kind::HOST_HELLO => Msg::HostHello(j(p)?),
            kind::START_VIDEO => Msg::StartVideo(j(p)?),
            kind::STOP_VIDEO => Msg::StopVideo,
            kind::KEYFRAME => Msg::Keyframe,
            kind::VIDEO => {
                if p.len() < 20 {
                    return Err("short video packet".into());
                }
                Msg::Video(VideoPacket {
                    keyframe: p[0] & 1 != 0,
                    monitor: u16::from_le_bytes([p[2], p[3]]),
                    capture_us: u64::from_le_bytes(p[4..12].try_into().unwrap()),
                    encode_us: u32::from_le_bytes(p[12..16].try_into().unwrap()),
                    width: u16::from_le_bytes([p[16], p[17]]) as u32,
                    height: u16::from_le_bytes([p[18], p[19]]) as u32,
                    data: p[20..].to_vec(),
                })
            }
            kind::VIDEO_STATUS => Msg::VideoStatus(j(p)?),
            kind::CLIPBOARD => Msg::Clipboard(j(p)?),
            kind::SET_LAYOUT => Msg::SetLayout(j(p)?),
            kind::LAYOUT => Msg::Layout(j(p)?),
            kind::INPUT => Msg::Input(decode_input(p).ok_or("bad input event")?),
            kind::START_AUDIO => Msg::StartAudio(j(p)?),
            kind::AUDIO => {
                if p.len() < 8 {
                    return Err("short audio packet".into());
                }
                Msg::Audio(AudioPacket {
                    capture_us: u64::from_le_bytes(p[..8].try_into().unwrap()),
                    data: p[8..].to_vec(),
                })
            }
            kind::STOP_AUDIO => Msg::StopAudio,
            kind::PING | kind::PONG => {
                let t = p
                    .get(..8)
                    .and_then(|b| b.try_into().ok())
                    .map(u64::from_le_bytes)
                    .ok_or("short ping")?;
                if k == kind::PING {
                    Msg::Ping(t)
                } else {
                    Msg::Pong(t)
                }
            }
            kind::LOG => Msg::Log(String::from_utf8_lossy(p).into_owned()),
            other => Msg::Unknown(other),
        })
    }
}

fn encode_input(i: &Input) -> Vec<u8> {
    let mut b = Vec::with_capacity(20);
    match *i {
        Input::Key { qnum, down } => {
            b.push(1);
            b.extend_from_slice(&qnum.to_le_bytes());
            b.push(down as u8);
        }
        Input::Button { button, down } => {
            b.extend_from_slice(&[2, button, down as u8]);
        }
        Input::Rel { dx, dy } => {
            b.push(3);
            b.extend_from_slice(&dx.to_le_bytes());
            b.extend_from_slice(&dy.to_le_bytes());
        }
        Input::Abs { x, y, monitor } => {
            b.push(4);
            b.extend_from_slice(&x.to_le_bytes());
            b.extend_from_slice(&y.to_le_bytes());
            b.extend_from_slice(&monitor.to_le_bytes());
        }
        Input::Wheel { dx, dy } => {
            b.push(5);
            b.extend_from_slice(&dx.to_le_bytes());
            b.extend_from_slice(&dy.to_le_bytes());
        }
        Input::Gamepad(g) => {
            b.push(6);
            b.push(g.index);
            b.extend_from_slice(&g.buttons.to_le_bytes());
            for a in [g.lx, g.ly, g.rx, g.ry] {
                b.extend_from_slice(&a.to_le_bytes());
            }
            b.push(g.lt);
            b.push(g.rt);
        }
    }
    b
}

fn decode_input(p: &[u8]) -> Option<Input> {
    let i32_at = |o: usize| p.get(o..o + 4).map(|b| i32::from_le_bytes(b.try_into().unwrap()));
    let u32_at = |o: usize| p.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let i16_at = |o: usize| p.get(o..o + 2).map(|b| i16::from_le_bytes(b.try_into().unwrap()));
    Some(match *p.first()? {
        1 => Input::Key {
            qnum: u16::from_le_bytes(p.get(1..3)?.try_into().ok()?),
            down: *p.get(3)? != 0,
        },
        2 => Input::Button {
            button: *p.get(1)?,
            down: *p.get(2)? != 0,
        },
        3 => Input::Rel {
            dx: i32_at(1)?,
            dy: i32_at(5)?,
        },
        4 => Input::Abs {
            x: u32_at(1)?,
            y: u32_at(5)?,
            monitor: u16::from_le_bytes(p.get(9..11)?.try_into().ok()?),
        },
        5 => Input::Wheel {
            dx: i32_at(1)?,
            dy: i32_at(5)?,
        },
        6 => Input::Gamepad(Gamepad {
            index: *p.get(1)?,
            buttons: u32_at(2)?,
            lx: i16_at(6)?,
            ly: i16_at(8)?,
            rx: i16_at(10)?,
            ry: i16_at(12)?,
            lt: *p.get(14)?,
            rt: *p.get(15)?,
        }),
        _ => return None,
    })
}

/// Write one framed message.
pub fn write_msg<W: Write>(w: &mut W, m: &Msg) -> io::Result<()> {
    let (k, p) = m.encode();
    w.write_all(&frame(k, &p))?;
    w.flush()
}

/// One frame as bytes (for async writers).
pub fn frame(k: u8, payload: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(5 + payload.len());
    b.extend_from_slice(&((payload.len() + 1) as u32).to_le_bytes());
    b.push(k);
    b.extend_from_slice(payload);
    b
}

/// Read one framed message (blocking).
pub fn read_msg<R: Read>(r: &mut R) -> io::Result<Msg> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("bad frame length {len}")));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Msg::decode(buf[0], &buf[1..]).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Incremental decoder for non-blocking/async readers.
#[derive(Default)]
pub struct Decoder {
    buf: Vec<u8>,
}

impl Decoder {
    /// Feed bytes; returns every complete message. A malformed frame
    /// length is an error (the stream cannot be resynchronised).
    pub fn push(&mut self, data: &[u8]) -> Result<Vec<Msg>, String> {
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        let mut off = 0;
        while self.buf.len() - off >= 4 {
            let len = u32::from_le_bytes(self.buf[off..off + 4].try_into().unwrap()) as usize;
            if len == 0 || len > MAX_FRAME {
                return Err(format!("bad frame length {len}"));
            }
            if self.buf.len() - off - 4 < len {
                break;
            }
            let body = &self.buf[off + 4..off + 4 + len];
            match Msg::decode(body[0], &body[1..]) {
                Ok(m) => out.push(m),
                // A bad payload of a known kind is dropped, not fatal.
                Err(_) => out.push(Msg::Unknown(body[0])),
            }
            off += 4 + len;
        }
        self.buf.drain(..off);
        Ok(out)
    }
}

/// Linux evdev key code for a QEMU qnum (non-extended set-1 scancodes
/// 1..=88 are the same numbers; extended keys are listed).
pub fn qnum_to_evdev(q: u16) -> Option<u16> {
    if (1..=88).contains(&q) {
        return Some(q);
    }
    Some(match q {
        0x57 => 87,  // F11
        0x58 => 88,  // F12
        0x56 => 86,  // 102nd key
        0x9c => 96,  // KP Enter
        0x9d => 97,  // RightCtrl
        0xb5 => 98,  // KP /
        0xb7 => 99,  // SysRq/PrintScreen
        0xb8 => 100, // RightAlt
        0xc7 => 102, // Home
        0xc8 => 103, // Up
        0xc9 => 104, // PageUp
        0xcb => 105, // Left
        0xcd => 106, // Right
        0xcf => 107, // End
        0xd0 => 108, // Down
        0xd1 => 109, // PageDown
        0xd2 => 110, // Insert
        0xd3 => 111, // Delete
        0xdb => 125, // LeftMeta
        0xdc => 126, // RightMeta
        0xdd => 127, // Compose/Menu
        0xc6 => 119, // Pause
        _ => return None,
    })
}

/// Split an H.264/HEVC Annex B byte stream into NAL units' types; true if
/// the access unit holds an IDR/IRAP picture.
pub fn h264_is_keyframe(au: &[u8]) -> bool {
    let mut i = 0;
    while i + 3 < au.len() {
        if au[i] == 0 && au[i + 1] == 0 && au[i + 2] == 1 {
            let t = au[i + 3] & 0x1f;
            if t == 5 || t == 7 {
                return true;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    false
}

/// Splits an encoder's Annex B output into access units at access unit
/// delimiters (the encoders are asked to insert one in front of every
/// frame). The last unit stays pending until the next delimiter arrives or
/// the caller takes it (when the reader has been idle, the frame is whole).
#[derive(Default)]
pub struct AudSplitter {
    buf: Vec<u8>,
}

fn aud_positions(b: &[u8]) -> Vec<usize> {
    let mut v = Vec::new();
    let mut i = 0;
    while i + 4 < b.len() {
        if b[i] == 0 && b[i + 1] == 0 && b[i + 2] == 0 && b[i + 3] == 1 && b[i + 4] & 0x1f == 9 {
            v.push(i);
            i += 5;
        } else {
            i += 1;
        }
    }
    v
}

impl AudSplitter {
    pub fn push(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        self.buf.extend_from_slice(data);
        let pos = aud_positions(&self.buf);
        let mut out = Vec::new();
        if pos.len() < 2 {
            return out;
        }
        for w in pos.windows(2) {
            out.push(self.buf[w[0]..w[1]].to_vec());
        }
        let last = *pos.last().unwrap();
        self.buf.drain(..last);
        out
    }
    pub fn take_pending(&mut self) -> Option<Vec<u8>> {
        if aud_positions(&self.buf).first() == Some(&0) && self.buf.len() > 6 {
            Some(std::mem::take(&mut self.buf))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(m: Msg) {
        let (k, p) = m.encode();
        assert_eq!(Msg::decode(k, &p).unwrap(), m);
        let mut wire = Vec::new();
        write_msg(&mut wire, &m).unwrap();
        assert_eq!(read_msg(&mut &wire[..]).unwrap(), m);
    }

    #[test]
    fn every_message_round_trips() {
        round(Msg::Hello(Hello {
            v: VERSION,
            os: "linux".into(),
            caps: Caps {
                codecs: vec!["h264".into()],
                clipboard: true,
                ..Default::default()
            },
            ..Default::default()
        }));
        round(Msg::HostHello(HostHello { v: 1, clipboard_max_bytes: 1 << 20 }));
        round(Msg::StartVideo(StartVideo { fps: 240, fixed_rate: true, ..Default::default() }));
        round(Msg::StopVideo);
        round(Msg::Keyframe);
        round(Msg::Video(VideoPacket {
            keyframe: true,
            monitor: 1,
            capture_us: 123456789,
            encode_us: 2500,
            width: 1920,
            height: 1080,
            data: vec![0, 0, 0, 1, 0x65, 1, 2, 3],
        }));
        round(Msg::VideoStatus(VideoStatus { running: true, encoder: "nvenc".into(), ..Default::default() }));
        round(Msg::Clipboard(Clipboard { mime: "text/plain".into(), text: "héllo".into() }));
        let l = Layout {
            monitors: vec![
                Monitor { width: 2560, height: 1440, refresh_hz: 240, primary: true, ..Default::default() },
                Monitor { x: 2560, width: 1920, height: 1080, refresh_hz: 60, ..Default::default() },
            ],
        };
        round(Msg::SetLayout(l.clone()));
        round(Msg::Layout(l));
        for i in [
            Input::Key { qnum: 0xc8, down: true },
            Input::Button { button: 2, down: false },
            Input::Rel { dx: -5, dy: 7 },
            Input::Abs { x: 100, y: 200, monitor: 1 },
            Input::Wheel { dx: 0, dy: -1 },
            Input::Gamepad(Gamepad { index: 1, buttons: 0b101, lx: -32768, ly: 32767, rx: 1, ry: -1, lt: 255, rt: 3 }),
        ] {
            round(Msg::Input(i));
        }
        round(Msg::StartAudio(StartAudio { codec: "opus".into(), sample_rate: 48000, channels: 2, bitrate_kbps: 128, frame_ms: 10 }));
        round(Msg::Audio(AudioPacket { capture_us: 9, data: vec![1, 2, 3] }));
        round(Msg::StopAudio);
        round(Msg::Ping(42));
        round(Msg::Pong(42));
        round(Msg::Log("x".into()));
    }

    #[test]
    fn decoder_handles_split_and_unknown_frames() {
        let mut wire = Vec::new();
        write_msg(&mut wire, &Msg::Ping(1)).unwrap();
        wire.extend_from_slice(&frame(0x99, b"future"));
        write_msg(&mut wire, &Msg::Clipboard(Clipboard { mime: "text/plain".into(), text: "a".into() })).unwrap();
        let mut d = Decoder::default();
        let mut got = Vec::new();
        for chunk in wire.chunks(3) {
            got.extend(d.push(chunk).unwrap());
        }
        assert_eq!(got.len(), 3);
        assert_eq!(got[0], Msg::Ping(1));
        assert_eq!(got[1], Msg::Unknown(0x99));
        assert!(matches!(got[2], Msg::Clipboard(_)));
        // unknown JSON fields from a newer peer are ignored
        let m = Msg::decode(kind::HOST_HELLO, br#"{"v":2,"clipboard_max_bytes":5,"new":1}"#).unwrap();
        assert_eq!(m, Msg::HostHello(HostHello { v: 2, clipboard_max_bytes: 5 }));
    }

    #[test]
    fn oversize_frame_is_refused() {
        let mut d = Decoder::default();
        assert!(d.push(&(MAX_FRAME as u32 + 1).to_le_bytes()).is_err());
        assert!(read_msg(&mut &0u32.to_le_bytes()[..]).is_err());
    }

    #[test]
    fn keys_map_to_evdev() {
        assert_eq!(qnum_to_evdev(0x1e), Some(30)); // A
        assert_eq!(qnum_to_evdev(0xc8), Some(103)); // Up
        assert_eq!(qnum_to_evdev(0x9d), Some(97)); // RightCtrl
        assert_eq!(qnum_to_evdev(0x200), None);
    }

    #[test]
    fn keyframes_and_aud_split() {
        let aud = [0u8, 0, 0, 1, 0x09, 0xf0];
        let idr = [0u8, 0, 0, 1, 0x67, 1, 0, 0, 1, 0x65, 2];
        let p = [0u8, 0, 0, 1, 0x41, 3];
        assert!(h264_is_keyframe(&idr));
        assert!(!h264_is_keyframe(&p));
        let mut s = AudSplitter::default();
        let mut stream = Vec::new();
        for f in [&idr[..], &p[..]] {
            stream.extend_from_slice(&aud);
            stream.extend_from_slice(f);
        }
        let out = s.push(&stream);
        assert_eq!(out, vec![[&aud[..], &idr[..]].concat()]);
        assert_eq!(s.take_pending().unwrap(), [&aud[..], &p[..]].concat());
        assert!(s.take_pending().is_none());
    }
}
