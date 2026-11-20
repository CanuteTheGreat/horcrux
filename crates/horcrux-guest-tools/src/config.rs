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
    /// Windows: run as a service that launches the agent in the console session.
    pub service: bool,
    /// Windows: BGRA->NV12 conversion threads (0 = up to 4, by CPU count).
    pub convert_threads: usize,
    /// Windows: GDI capture poll interval (ms).
    pub gdi_poll_ms: u64,
    /// Windows: idle wait for a changed frame before re-checking requests (ms).
    pub idle_poll_ms: u64,
    /// Windows: H.264 profile (`baseline`, `main`, `high`).
    pub h264_profile: String,
    /// Windows: longest wait for an asynchronous (hardware) MFT output (ms).
    pub mf_output_wait_ms: u64,
    /// Windows: how long StartVideo waits for capture + encoder init (ms).
    pub video_start_timeout_ms: u64,
    /// Windows: wheel scale in percent (negative inverts).
    pub wheel_scale_pct: i32,
    /// Windows: wait after a display mode change before reporting the layout (ms).
    pub layout_settle_ms: u64,
    /// Windows service: how often the console session is checked (ms).
    pub service_poll_ms: u64,
    /// Windows service: also run the agent (as SYSTEM) at the login screen.
    pub service_login_screen: bool,
    /// Windows: Horcrux virtual display (hxidd) use: `auto` (when the node
    /// asks for it: the guest's GPU has no monitor or no outputs), `on`
    /// (layouts always go to virtual monitors) or `off` (never; the
    /// installer then creates no virtual display adapter).
    pub virtual_display: String,
    /// Windows: modes every virtual monitor offers besides the one asked
    /// for (`WxH@HZ`, comma-separated; HZ may be fractional).
    pub virtual_display_modes: String,
    /// Windows: refresh rate for a layout monitor that names none (Hz).
    pub virtual_display_default_hz: u32,
    /// Windows: most virtual monitors created for a layout.
    pub virtual_display_max_monitors: usize,
    /// Windows: the virtual monitors come back at boot / on the sign-in screen.
    pub virtual_display_persist: bool,
    /// Windows: make the first virtual monitor the primary display.
    pub virtual_display_primary: bool,
    /// Windows: how long a layout waits for new virtual monitors (ms).
    pub virtual_display_wait_ms: u64,
    /// Windows: virtual gamepads created by `--drivers install`.
    pub gamepads: u32,
    /// Windows: how viewer gamepads reach games: `hid` (the Horcrux virtual
    /// HID gamepad: DirectInput, SDL, Windows.Gaming.Input; no Microsoft
    /// signature needed) or `xinput` (the optional ViGEm bus driver, an
    /// Xbox 360 class controller for XInput-only games; set up with
    /// `--drivers install-xinput`).
    pub gamepad_backend: String,
    /// Windows: the ViGEm bus setup program `--drivers install-xinput` runs
    /// (default: `optional\ViGEmBus_*.exe` next to the agent).
    pub vigem_installer: String,
    /// Windows: SHA-256 the setup program must have (lowercase hex). The
    /// default is the final upstream release; the setup's Authenticode
    /// signature is checked as well.
    pub vigem_sha256: String,
    /// Windows: signer (certificate subject text) the setup program must carry.
    pub vigem_signer: String,
    /// Windows: directory holding the driver packages (default: `drivers`
    /// next to the agent).
    pub drivers_dir: String,
    /// Windows: subject (CN) of the per-guest driver signing certificate.
    pub driver_cert_subject: String,
    /// Windows: wait for a new software device to be created (ms).
    pub driver_device_timeout_ms: u64,
    /// Windows: `install`, `remove` or `status` of the guest drivers, then exit.
    pub drivers: Option<String>,
    /// Apply this layout (`WxH@HZ[,WxH@HZ...]`), print the result and exit (tests).
    pub layout: Option<String>,
    /// Drive virtual gamepad 0 through a fixed sequence and exit (tests).
    pub gamepad_test: bool,
}

/// ViGEmBus 1.22.0 (final upstream release, 2023-11-02), the all-in-one
/// x64/x86/arm64 setup `ViGEmBus_1.22.0_x64_x86_arm64.exe`.
pub const VIGEM_SHA256: &str = "89220a7865076b342892f98865f3499fb7c4cfd673159e89d352c360fd014c6a";

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
            reconnect_ms: 1000,
            once: false,
            print_caps: false,
            input_device_name: "Horcrux virtual input".into(),
            service: false,
            convert_threads: 0,
            gdi_poll_ms: 8,
            idle_poll_ms: 33,
            h264_profile: "main".into(),
            mf_output_wait_ms: 100,
            video_start_timeout_ms: 15000,
            wheel_scale_pct: 100,
            layout_settle_ms: 500,
            service_poll_ms: 1000,
            service_login_screen: true,
            virtual_display: "auto".into(),
            virtual_display_modes:
                "3840x2160@60,3840x2160@120,2560x1440@60,2560x1440@144,2560x1440@240,\
1920x1080@60,1920x1080@144,1920x1080@240,1680x1050@60,1600x900@60,1366x768@60,1280x720@60"
                    .into(),
            virtual_display_default_hz: 60,
            virtual_display_max_monitors: 4,
            virtual_display_persist: true,
            virtual_display_primary: true,
            virtual_display_wait_ms: 5000,
            gamepads: 1,
            gamepad_backend: "hid".into(),
            vigem_installer: String::new(),
            vigem_sha256: VIGEM_SHA256.into(),
            vigem_signer: "Nefarius Software Solutions".into(),
            drivers_dir: String::new(),
            driver_cert_subject: "Horcrux Guest Drivers".into(),
            driver_device_timeout_ms: 30000,
            drivers: None,
            layout: None,
            gamepad_test: false,
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
                None if matches!(
                    k,
                    "once" | "print-caps" | "help" | "service" | "gamepad-test"
                ) =>
                {
                    (k.to_string(), "true".into())
                }
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
        let num = |v: &str| {
            v.parse::<u64>()
                .map_err(|_| format!("{k}: not a number: {v}"))
        };
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
            "service" => self.service = b(v),
            "convert_threads" => self.convert_threads = num(v)? as usize,
            "gdi_poll_ms" => self.gdi_poll_ms = num(v)?.max(1),
            "idle_poll_ms" => self.idle_poll_ms = num(v)?.max(1),
            "h264_profile" => self.h264_profile = v.into(),
            "mf_output_wait_ms" => self.mf_output_wait_ms = num(v)?,
            "video_start_timeout_ms" => self.video_start_timeout_ms = num(v)?.max(1000),
            "wheel_scale_pct" => {
                self.wheel_scale_pct = v.parse().map_err(|_| format!("{k}: not a number: {v}"))?
            }
            "layout_settle_ms" => self.layout_settle_ms = num(v)?,
            "service_poll_ms" => self.service_poll_ms = num(v)?.max(100),
            "service_login_screen" => self.service_login_screen = b(v),
            "virtual_display" => {
                if !matches!(v, "auto" | "on" | "off") {
                    return Err(format!("{k}: auto, on or off, not {v}"));
                }
                self.virtual_display = v.into()
            }
            "virtual_display_modes" => self.virtual_display_modes = v.into(),
            "virtual_display_default_hz" => {
                self.virtual_display_default_hz = num(v)?.clamp(1, 1000) as u32
            }
            "virtual_display_max_monitors" => {
                self.virtual_display_max_monitors = num(v)?.clamp(1, 16) as usize
            }
            "virtual_display_persist" => self.virtual_display_persist = b(v),
            "virtual_display_primary" => self.virtual_display_primary = b(v),
            "virtual_display_wait_ms" => self.virtual_display_wait_ms = num(v)?,
            "gamepads" => self.gamepads = num(v)?.min(16) as u32,
            "gamepad_backend" => {
                if !matches!(v, "hid" | "xinput") {
                    return Err(format!("{k}: hid or xinput, not {v}"));
                }
                self.gamepad_backend = v.into()
            }
            "vigem_installer" => self.vigem_installer = v.into(),
            "vigem_sha256" => {
                let v = v.to_ascii_lowercase();
                if v.len() != 64 || !v.bytes().all(|c| c.is_ascii_hexdigit()) {
                    return Err(format!("{k}: 64 hex digits"));
                }
                self.vigem_sha256 = v
            }
            "vigem_signer" => self.vigem_signer = v.into(),
            "drivers_dir" => self.drivers_dir = v.into(),
            "driver_cert_subject" => {
                if v.is_empty() || v.contains(['\'', ',', '=', '"']) {
                    return Err(format!("{k}: a plain name, not {v}"));
                }
                self.driver_cert_subject = v.into()
            }
            "driver_device_timeout_ms" => self.driver_device_timeout_ms = num(v)?.max(1000),
            "drivers" => {
                if !matches!(
                    v,
                    "install" | "remove" | "status" | "install-xinput" | "remove-xinput"
                ) {
                    return Err(format!(
                        "{k}: install, remove, status, install-xinput or remove-xinput, not {v}"
                    ));
                }
                self.drivers = Some(v.into())
            }
            "layout" => self.layout = Some(v.into()),
            "gamepad_test" => self.gamepad_test = b(v),
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
        .map(|(k, v)| {
            (
                k.trim().replace('-', "_"),
                v.trim().trim_matches('"').to_string(),
            )
        })
        .collect()
}

const HELP: &str = "usage: horcrux-guest-agent [--config FILE] [--port PATH | --connect HOST:PORT] \
[--capture auto|kmsgrab|x11grab|dxgi|idd] [--encoder auto|software|vaapi|nvenc|qsv|amf|mediafoundation] \
[--ffmpeg PATH] [--once] [--print-caps] [--drivers install|remove|status|install-xinput|remove-xinput] [--layout WxH@HZ[,...]] [--gamepad-test]";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_override_file_and_defaults() {
        let dir = std::env::temp_dir().join(format!("hxgt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("c.conf");
        std::fs::write(
            &f,
            "# c\nencoder = vaapi\nclipboard-poll-ms=250\ncapture=\"x11grab\"\n",
        )
        .unwrap();
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
        assert!(Config::load(vec!["--virtual-display=maybe".into()]).is_err());
        assert!(Config::load(vec!["--drivers".into(), "nuke".into()]).is_err());
        assert!(Config::load(vec!["--driver-cert-subject=a,b".into()]).is_err());
        let d = Config::load(vec![
            "--drivers".into(),
            "install".into(),
            "--gamepads=40".into(),
        ])
        .unwrap();
        assert_eq!(d.drivers.as_deref(), Some("install"));
        assert_eq!(d.gamepads, 16);
        assert_eq!(d.gamepad_backend, "hid", "XInput stays opt-in");
        assert!(Config::load(vec!["--gamepad-backend=ps5".into()]).is_err());
        assert!(Config::load(vec!["--vigem-sha256=abc".into()]).is_err());
        assert_eq!(
            Config::load(vec!["--drivers=install-xinput".into()])
                .unwrap()
                .drivers
                .as_deref(),
            Some("install-xinput")
        );
        assert_eq!(d.virtual_display, "auto");
        assert!(!crate::hxdrv::parse_modes(&d.virtual_display_modes).is_empty());
        assert_eq!(
            crate::hxdrv::parse_modes(&d.virtual_display_modes).len(),
            d.virtual_display_modes.split(',').count(),
            "every default mode is valid"
        );
        std::fs::remove_dir_all(dir).ok();
    }
}
