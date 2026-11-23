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
    /// Audio capture source. Linux: `default` or a PulseAudio/PipeWire
    /// source name. Windows: `default` (the default render endpoint, followed
    /// when it changes), an endpoint ID, or part of an endpoint's name.
    pub audio_source: String,
    /// Opus bitrate (kbit/s) when the node's StartAudio leaves it at 0.
    pub audio_bitrate_kbps: u32,
    /// Windows: Opus application (`lowdelay` = restricted low delay,
    /// `audio`, `voip`).
    pub audio_opus_application: String,
    /// Windows: WASAPI shared-mode buffer length (ms).
    pub audio_buffer_ms: u64,
    /// Windows: capture poll interval (ms).
    pub audio_poll_ms: u64,
    /// Windows: no samples from the endpoint for this long (ms) = idle, and
    /// silence is sent so the stream keeps its frame cadence.
    pub audio_idle_ms: u64,
    /// Windows: longest gap (ms) filled with silence at once; older time is
    /// skipped (after a stall).
    pub audio_max_fill_ms: u64,
    /// Windows: wait before reopening an endpoint that failed or vanished (ms).
    pub audio_retry_ms: u64,
    /// Windows: how often the default render endpoint is re-checked (ms).
    pub audio_device_check_ms: u64,
    /// Wait between reconnect attempts (ms).
    pub reconnect_ms: u64,
    /// Windows: largest single write to the virtio-serial port (bytes);
    /// larger messages go out in pieces (vioser fails writes spanning more
    /// pages than its ring has descriptors).
    pub port_write_max: u64,
    /// Windows: wait before retrying a write the port refused because its
    /// ring was full (ms).
    pub port_busy_wait_ms: u64,
    /// Windows: attempts per write piece before the connection is dropped.
    pub port_busy_tries: u64,
    /// Windows: an already open port handle inherited from the service
    /// (set by the service on the agent it launches; 0 = open `port`).
    pub port_handle: u64,
    /// Windows: a write the port has not taken after this long (ms) is
    /// cancelled and the message dropped (the node is not connected).
    pub port_write_timeout_ms: u64,
    /// Stop after one connection (tests).
    pub once: bool,
    /// Print the capability report and exit.
    pub print_caps: bool,
    /// Name given to the virtual input devices.
    pub input_device_name: String,
    /// Offer the node keyboard, buttons and absolute pointer through the
    /// tools' own input devices (Linux uinput; the node then prefers them
    /// over QEMU's emulated keyboard while the tools run).
    pub offer_input: bool,
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
    /// Write raw frames into the Horcrux frame ring (ivshmem) while a
    /// same-host viewer watches.
    pub shm_frames: bool,
    /// How often the frame ring is checked for a reader (ms).
    pub shm_poll_ms: u64,
    /// A reader whose clock stamp has not moved for this long is gone (ms).
    pub shm_reader_timeout_ms: u64,
    /// Linux: PCI sysfs directory searched for the ivshmem device.
    pub shm_pci_dir: String,
    /// Map the frame ring write-combined (Linux `resource2_wc` when the
    /// kernel offers it, the IVSHMEM driver's write-combined mode on
    /// Windows): the default uncached BAR mapping makes every frame write
    /// slow on hosts that honour the guest's memory types (AMD).
    pub shm_write_combine: bool,
    /// Windows: IVSHMEM driver device interface path override (empty =
    /// enumerate the driver's interface class).
    pub shm_device: String,
    /// Windows: print the frame ring writer's rate and per-frame capture and
    /// write times to stderr every this many seconds (0 = off).
    pub shm_stats_secs: u64,
    /// One-shot action instead of the service loop (`--hibernate-status`,
    /// `--hibernate-setup`, `--hibernate-enable`, `--hibernate`).
    pub action: Option<HibernateAction>,
    /// Does the install step (`--hibernate-setup`) prepare the guest for
    /// hibernation (swap/hiberfil, resume device)?
    pub hibernate_at_install: bool,
    /// Linux: command that hibernates the guest (split on spaces); empty =
    /// the first of systemctl / loginctl / pm-hibernate / s2disk found.
    pub hibernate_command: String,
    /// Wait between answering the host and starting the hibernate, so the
    /// answer is delivered before the guest freezes.
    pub hibernate_delay_ms: u64,
    /// Swap (Linux) the guest must have, as a percentage of its RAM; also
    /// the size of the swap file created by `--hibernate-enable`.
    pub hibernate_swap_pct: u64,
    /// Linux: swap file created when no active swap is big enough.
    pub hibernate_swapfile: String,
    /// Windows: hiberfil.sys size in percent of RAM (`powercfg /h /size`);
    /// 0 = leave the Windows default.
    pub hibernate_hiberfile_pct: u64,
    /// Linux: marker left by `--hibernate-enable` until the guest has been
    /// restarted with the new boot configuration.
    pub hibernate_pending_file: String,
}

/// What a one-shot invocation does (see [`crate::power`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HibernateAction {
    Status,
    Setup,
    Enable,
    Hibernate,
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
            audio_bitrate_kbps: 128,
            audio_opus_application: "lowdelay".into(),
            audio_buffer_ms: 50,
            audio_poll_ms: 5,
            audio_idle_ms: 40,
            audio_max_fill_ms: 200,
            audio_retry_ms: 1000,
            audio_device_check_ms: 1000,
            reconnect_ms: 1000,
            port_write_max: 65536,
            port_busy_wait_ms: 5,
            port_busy_tries: 400,
            port_handle: 0,
            port_write_timeout_ms: 2000,
            once: false,
            print_caps: false,
            input_device_name: "Horcrux virtual input".into(),
            offer_input: true,
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
            shm_frames: true,
            shm_poll_ms: 100,
            shm_reader_timeout_ms: 2000,
            shm_pci_dir: "/sys/bus/pci/devices".into(),
            shm_device: String::new(),
            shm_write_combine: true,
            shm_stats_secs: 0,
            action: None,
            hibernate_at_install: true,
            hibernate_command: String::new(),
            hibernate_delay_ms: 3000,
            hibernate_swap_pct: 100,
            hibernate_swapfile: "/swap-horcrux.img".into(),
            hibernate_hiberfile_pct: 0,
            hibernate_pending_file: "/var/lib/horcrux/hibernate-pending".into(),
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
                    "once"
                        | "print-caps"
                        | "help"
                        | "service"
                        | "hibernate"
                        | "hibernate-status"
                        | "hibernate-setup"
                        | "hibernate-enable"
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
            "audio_bitrate_kbps" => self.audio_bitrate_kbps = num(v)?.clamp(6, 510) as u32,
            "audio_opus_application" => match v {
                "lowdelay" | "audio" | "voip" => self.audio_opus_application = v.into(),
                _ => return Err(format!("{k}: one of lowdelay, audio, voip")),
            },
            "audio_buffer_ms" => self.audio_buffer_ms = num(v)?.clamp(10, 2000),
            "audio_poll_ms" => self.audio_poll_ms = num(v)?.clamp(1, 100),
            "audio_idle_ms" => self.audio_idle_ms = num(v)?.max(5),
            "audio_max_fill_ms" => self.audio_max_fill_ms = num(v)?.max(20),
            "audio_retry_ms" => self.audio_retry_ms = num(v)?.max(100),
            "audio_device_check_ms" => self.audio_device_check_ms = num(v)?.max(100),
            "reconnect_ms" => self.reconnect_ms = num(v)?.max(100),
            "port_write_max" => self.port_write_max = num(v)?.clamp(512, 16 << 20),
            "port_busy_wait_ms" => self.port_busy_wait_ms = num(v)?.max(1),
            "port_busy_tries" => self.port_busy_tries = num(v)?.max(1),
            "port_handle" => self.port_handle = num(v)?,
            "port_write_timeout_ms" => self.port_write_timeout_ms = num(v)?.max(10),
            "once" => self.once = b(v),
            "print_caps" => self.print_caps = b(v),
            "input_device_name" => self.input_device_name = v.into(),
            "offer_input" => self.offer_input = b(v),
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
            "shm_frames" => self.shm_frames = b(v),
            "shm_poll_ms" => self.shm_poll_ms = num(v)?.clamp(1, 5000),
            "shm_reader_timeout_ms" => self.shm_reader_timeout_ms = num(v)?.max(100),
            "shm_pci_dir" => self.shm_pci_dir = v.into(),
            "shm_device" => self.shm_device = v.into(),
            "shm_write_combine" => self.shm_write_combine = b(v),
            "hibernate" if b(v) => self.action = Some(HibernateAction::Hibernate),
            "hibernate_status" if b(v) => self.action = Some(HibernateAction::Status),
            "hibernate_setup" if b(v) => self.action = Some(HibernateAction::Setup),
            "hibernate_enable" if b(v) => self.action = Some(HibernateAction::Enable),
            "hibernate" | "hibernate_status" | "hibernate_enable" | "hibernate_setup" => {}
            "hibernate_at_install" => self.hibernate_at_install = b(v),
            "hibernate_command" => self.hibernate_command = v.into(),
            "hibernate_delay_ms" => self.hibernate_delay_ms = num(v)?.min(600_000),
            "hibernate_swap_pct" => self.hibernate_swap_pct = num(v)?.clamp(10, 400),
            "hibernate_swapfile" => self.hibernate_swapfile = v.into(),
            "hibernate_pending_file" => self.hibernate_pending_file = v.into(),
            "hibernate_hiberfile_pct" => self.hibernate_hiberfile_pct = num(v)?.min(100),
            "shm_stats_secs" => self.shm_stats_secs = num(v)?.min(3600),
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
[--ffmpeg PATH] [--once] [--print-caps] \
[--hibernate-status | --hibernate-setup | --hibernate-enable | --hibernate]";

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
        assert_eq!(c.audio_opus_application, "lowdelay");
        assert_eq!((c.audio_bitrate_kbps, c.audio_idle_ms), (128, 40));
        let c = Config::load(vec![
            "--config=/nonexistent".into(),
            "--audio-opus-application=audio".into(),
            "--audio-bitrate-kbps=9999".into(),
        ])
        .unwrap();
        assert_eq!(
            (c.audio_opus_application.as_str(), c.audio_bitrate_kbps),
            ("audio", 510)
        );
        assert!(Config::load(vec![
            "--config=/nonexistent".into(),
            "--audio-opus-application=x".into()
        ])
        .is_err());
        std::fs::remove_dir_all(dir).ok();
    }
}
