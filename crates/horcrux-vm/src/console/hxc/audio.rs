//! Guest audio without guest tools: QEMU's D-Bus audio output
//! (`-audiodev dbus`, `org.qemu.Display1.AudioOutListener`) delivers raw PCM
//! per voice; the node encodes it to Opus with an FFmpeg child (libopus,
//! low-delay application, short frames, one packet per Ogg page) and the
//! packets go on the session's WebRTC audio track. When the guest tools
//! send their own Opus audio, that wins and this encoder rests.
use horcrux_guest_proto::OggReader;
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc as smpsc;

/// PCM format of one QEMU voice (`AudioOutListener.Init`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcmFormat {
    pub bits: u8,
    pub is_signed: bool,
    pub is_float: bool,
    pub freq: u32,
    pub channels: u8,
}

impl PcmFormat {
    /// FFmpeg raw sample format name (QEMU's voices are little-endian on
    /// every host Horcrux runs on).
    pub fn ffmpeg_format(&self) -> Option<&'static str> {
        match (self.bits, self.is_float, self.is_signed) {
            (32, true, _) => Some("f32le"),
            (16, false, true) => Some("s16le"),
            (16, false, false) => Some("u16le"),
            (32, false, true) => Some("s32le"),
            (32, false, false) => Some("u32le"),
            (8, false, true) => Some("s8"),
            (8, false, false) => Some("u8"),
            _ => None,
        }
    }
}

/// FFmpeg arguments: raw PCM on stdin -> Opus in Ogg on stdout, 48 kHz
/// stereo (WebRTC's Opus clock), `frame_ms` packets.
pub fn ffmpeg_args(f: &PcmFormat, bitrate_kbps: u32, frame_ms: u32) -> Option<Vec<String>> {
    let fmt = f.ffmpeg_format()?;
    if f.freq == 0 || f.channels == 0 {
        return None;
    }
    let v: Vec<String> = [
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
        // No `-fflags nobuffer` (with a raw pipe input FFmpeg 5.1 then drops
        // the stream after the first read), and no probing: by default
        // FFmpeg reads megabytes before its first output packet, which on a
        // live pipe means seconds of silence (measured: 0 bytes in 2 s).
        "-probesize",
        "32",
        "-analyzeduration",
        "0",
        "-f",
        fmt,
        "-ar",
        &f.freq.to_string(),
        "-ac",
        &f.channels.to_string(),
        "-i",
        "pipe:0",
        "-ar",
        "48000",
        "-ac",
        "2",
        "-c:a",
        "libopus",
        "-application",
        "lowdelay",
        "-frame_duration",
        &frame_ms.to_string(),
        "-b:a",
        &format!("{bitrate_kbps}k"),
        "-page_duration",
        &(frame_ms * 1000).to_string(),
        "-flush_packets",
        "1",
        "-f",
        "ogg",
        "pipe:1",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    Some(v)
}

/// One running encoder for one voice.
pub struct Encoder {
    child: Child,
    pcm: Option<smpsc::SyncSender<Vec<u8>>>,
}

impl Encoder {
    /// Start FFmpeg; every Opus packet is handed to `on_packet` (from a
    /// reader thread). PCM that arrives faster than FFmpeg takes it is
    /// dropped, never queued without bound.
    pub fn spawn(
        ffmpeg: &str,
        f: &PcmFormat,
        bitrate_kbps: u32,
        frame_ms: u32,
        on_packet: impl Fn(Vec<u8>) -> bool + Send + 'static,
    ) -> Result<Encoder, String> {
        let args = ffmpeg_args(f, bitrate_kbps, frame_ms)
            .ok_or_else(|| format!("unsupported guest audio format {f:?}"))?;
        let mut child = Command::new(ffmpeg)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("{ffmpeg}: {e}"))?;
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        let mut stdout = child.stdout.take().ok_or("no stdout")?;
        // ~200 ms of 48 kHz stereo s16 in 10 ms chunks
        let (tx, rx) = smpsc::sync_channel::<Vec<u8>>(20);
        std::thread::spawn(move || {
            while let Ok(b) = rx.recv() {
                if stdin.write_all(&b).is_err() {
                    break;
                }
            }
        });
        std::thread::spawn(move || {
            let mut ogg = OggReader::default();
            let mut buf = vec![0u8; 16384];
            while let Ok(n) = stdout.read(&mut buf) {
                if n == 0 {
                    break;
                }
                for p in ogg.push(&buf[..n]) {
                    if !on_packet(p) {
                        return;
                    }
                }
            }
        });
        Ok(Encoder {
            child,
            pcm: Some(tx),
        })
    }

    /// Queue PCM; false when the queue is full (the chunk was dropped).
    pub fn push(&self, pcm: Vec<u8>) -> bool {
        self.pcm
            .as_ref()
            .map(|t| t.try_send(pcm).is_ok())
            .unwrap_or(false)
    }

    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        self.pcm.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_and_args() {
        let f = PcmFormat {
            bits: 16,
            is_signed: true,
            is_float: false,
            freq: 44100,
            channels: 2,
        };
        let a = ffmpeg_args(&f, 128, 10).unwrap().join(" ");
        assert!(
            a.contains("-f s16le -ar 44100 -ac 2 -i pipe:0 -ar 48000 -ac 2"),
            "{a}"
        );
        assert!(
            a.contains("-c:a libopus -application lowdelay -frame_duration 10 -b:a 128k"),
            "{a}"
        );
        assert!(
            a.contains("-page_duration 10000 -flush_packets 1 -f ogg pipe:1"),
            "{a}"
        );
        let fl = PcmFormat {
            bits: 32,
            is_float: true,
            ..f
        };
        assert_eq!(fl.ffmpeg_format(), Some("f32le"));
        assert!(ffmpeg_args(&PcmFormat { bits: 24, ..f }, 128, 10).is_none());
        assert!(ffmpeg_args(&PcmFormat { freq: 0, ..f }, 128, 10).is_none());
    }

    /// Real FFmpeg (skipped where it or libopus is missing): one second of
    /// a tone in, Opus packets of the asked length out.
    #[test]
    fn real_ffmpeg_encodes_pcm_to_opus_packets() {
        let ok = Command::new("ffmpeg")
            .args(["-hide_banner", "-encoders"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("libopus"))
            .unwrap_or(false);
        if !ok {
            eprintln!("skipped: no ffmpeg with libopus");
            return;
        }
        let f = PcmFormat {
            bits: 16,
            is_signed: true,
            is_float: false,
            freq: 48000,
            channels: 2,
        };
        let (tx, rx) = smpsc::channel::<Vec<u8>>();
        let enc = Encoder::spawn("ffmpeg", &f, 96, 10, move |p| tx.send(p).is_ok()).unwrap();
        let mut pcm = Vec::new();
        for i in 0..48000u32 {
            let s = ((i as f32 * 440.0 * std::f32::consts::TAU / 48000.0).sin() * 8000.0) as i16;
            pcm.extend_from_slice(&s.to_le_bytes());
            pcm.extend_from_slice(&s.to_le_bytes());
        }
        for c in pcm.chunks(1920 * 2) {
            while !enc.push(c.to_vec()) {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        let mut n = 0;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while n < 90 && std::time::Instant::now() < deadline {
            if let Ok(p) = rx.recv_timeout(std::time::Duration::from_millis(500)) {
                // TOC byte: config 0..31; 10 ms CELT/hybrid frames have
                // frame-size code 2 (CELT 10 ms) in the config bits.
                assert!(!p.is_empty());
                n += 1;
            }
        }
        assert!(n >= 90, "only {n} Opus packets for one second of audio");
        drop(enc);
    }
}
