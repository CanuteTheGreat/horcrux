//! Guest tools settings. Defaults here; `key = value` lines in the config
//! file override them; `--key value` / `--key=value` flags override both.
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Config {
    /// Guest-tools port path. Linux: `/dev/virtio-ports/<name>`; Windows:
    /// `\\.\Global\<name>`.
    pub port: String,
    /// Connect over TCP instead (development and tests): `host:port`.
    pub connect: Option<String>,
    /// FFmpeg used for capture/encode on Linux (and the software fallback).
    pub ffmpeg: String,
    /// Capture method: `auto`, `kmsgrab`, `x11grab` (Linux), `dxgi`
    /// (Windows Desktop Duplication), `idd` (Horcrux virtual display).
    pub capture: String,
    /// Encoder: `auto` (hardware first), `software`, or a backend name
    /// (`vaapi`, `nvenc`, `qsv`, `amf`, `mediafoundation`).
    pub encoder: String,
    /// DRM card for kmsgrab (Linux).
    pub drm_card: String,
    /// VA-API render node (Linux).
    pub vaapi_device: String,
    /// x264 preset for the software encoder.
    pub x264_preset: String,
    /// Clipboard poll interval (ms) where the platform has no change event.
    pub clipboard_poll_ms: u64,
    /// Largest clipboard text sent (bytes); the node's limit also applies.
    pub clipboard_max_bytes: usize,
    /// Audio capture source (`default`, a PulseAudio/PipeWire source name).
    pub audio_source: String,
    /// Wait between reconnect attempts (ms).
    pub reconnect_ms: u64,
    /// Stop after one connection (tests).
    pub once: bool,
    /// Print the capability report and exit.
    pub print_caps: bool,
    /// Name given to the virtual input devices.
    pub input_device_name: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: default_port(),
            connect: None,
            ffmpeg: "ffmpeg".into(),
            capture: "auto".into(),
            encoder: "auto".into(),
            drm_card: "/dev/dri/card0".into(),
            vaapi_device: "/dev/dri/renderD128".into(),
            x264_preset: "ultrafast".into(),
            clipboard_poll_ms: 500,
            clipboard_max_bytes: 1 << 20,
            audio_source: "default".into(),
            reconnect_ms: 2000,
            once: false,
            print_caps: false,
            input_device_name: "Horcrux virtual input".into(),
        }
    }
}

#[cfg(windows)]
fn default_port() -> String {
    format!(r"\\.\Global\{}", horcrux_guest_proto::PORT_NAME)
}
#[cfg(not(windows))]
fn default_port() -> String {
    format!("/dev/virtio-ports/{}", horcrux_guest_proto::PORT_NAME)
}

#[cfg(windows)]
pub fn config_file() -> String {
    let base = std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into());
    format!(r"{base}\Horcrux\guest-tools.conf")
}
#[cfg(not(windows))]
pub fn config_file() -> String {
    "/etc/horcrux/guest-tools.conf".into()
}

impl Config {
    pub fn load(args: Vec<String>) -> Result<Self, String> {
        let mut kv: HashMap<String, String> = HashMap::new();
        let mut file = config_file();
        // --config first, so the file can be chosen before it is read.
        let mut i = 0;
        let mut flags: Vec<(String, String)> = Vec::new();
        while i < args.len() {
            let a = &args[i];
            let Some(k) = a.strip_prefix("--") else {
                return Err(format!("unexpected argument {a}"));
            };
            let (k, v) = match k.split_once('=') {
                Some((k, v)) => (k.to_string(), v.to_string()),
                None if matches!(k, "once" | "print-caps" | "help") => (k.to_string(), "true".into()),
                None => {
                    i += 1;
                    let v = args.get(i).ok_or(format!("--{k} needs a value"))?;
                    (k.to_string(), v.clone())
                }
            };
            if k == "help" {
                return Err(HELP.into());
            }
            if k == "config" {
                file = v;
            } else {
                flags.push((k, v));
            }
            i += 1;
        }
        if let Ok(text) = std::fs::read_to_string(&file) {
            kv.extend(parse_file(&text));
        }
        for (k, v) in flags {
            kv.insert(k.replace('-', "_"), v);
        }
        let mut c = Config::default();
        for (k, v) in kv {
            c.set(&k, &v)?;
        }
        Ok(c)
    }

    fn set(&mut self, k: &str, v: &str) -> Result<(), String> {
        let num = |v: &str| v.parse::<u64>().map_err(|_| format!("{k}: not a number: {v}"));
        let b = |v: &str| matches!(v, "true" | "1" | "yes" | "on");
        match k {
            "port" => self.port = v.into(),
            "connect" => self.connect = (!v.is_empty()).then(|| v.to_string()),
            "ffmpeg" => self.ffmpeg = v.into(),
            "capture" => self.capture = v.into(),
            "encoder" => self.encoder = v.into(),
            "drm_card" => self.drm_card = v.into(),
            "vaapi_device" => self.vaapi_device = v.into(),
            "x264_preset" => self.x264_preset = v.into(),
            "clipboard_poll_ms" => self.clipboard_poll_ms = num(v)?.max(50),
            "clipboard_max_bytes" => self.clipboard_max_bytes = num(v)? as usize,
            "audio_source" => self.audio_source = v.into(),
            "reconnect_ms" => self.reconnect_ms = num(v)?.max(100),
            "once" => self.once = b(v),
            "print_caps" => self.print_caps = b(v),
            "input_device_name" => self.input_device_name = v.into(),
            _ => return Err(format!("unknown setting {k}")),
        }
        Ok(())
    }
}

fn parse_file(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().replace('-', "_"), v.trim().trim_matches('"').to_string()))
        .collect()
}

const HELP: &str = "usage: horcrux-guest-agent [--config FILE] [--port PATH | --connect HOST:PORT] \
[--capture auto|kmsgrab|x11grab|dxgi|idd] [--encoder auto|software|vaapi|nvenc|qsv|amf|mediafoundation] \
[--ffmpeg PATH] [--once] [--print-caps]";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_override_file_and_defaults() {
        let dir = std::env::temp_dir().join(format!("hxgt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("c.conf");
        std::fs::write(&f, "# c\nencoder = vaapi\nclipboard-poll-ms=250\ncapture=\"x11grab\"\n").unwrap();
        let c = Config::load(vec![
            "--config".into(),
            f.display().to_string(),
            "--capture=kmsgrab".into(),
            "--once".into(),
        ])
        .unwrap();
        assert_eq!(c.encoder, "vaapi");
        assert_eq!(c.clipboard_poll_ms, 250);
        assert_eq!(c.capture, "kmsgrab");
        assert!(c.once);
        assert_eq!(c.ffmpeg, "ffmpeg");
        assert!(Config::load(vec!["--bogus=1".into()]).is_err());
        std::fs::remove_dir_all(dir).ok();
    }
}
