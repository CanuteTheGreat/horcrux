//! Horcrux Console settings (docs/design/horcrux-console.md section 9).
//!
//! One remote display for every guest: a console broker in horcrux-api
//! hands out short-lived tickets, a frame source (QEMU's D-Bus display in
//! phase C1) is encoded on the host and carried to the client over WebRTC.
//! Every value here is a typed setting with a default: `[console]` in
//! /etc/horcrux/config.toml gives the node's defaults, `PUT
//! /api/console/settings` and the declarative spec (`console:`) change them,
//! and the UI edits them under Settings. Nothing is baked into code.
use serde::{Deserialize, Serialize};

/// What a session is tuned for.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ConsoleProfile {
    /// Office/desktop VMs: crisp text, damage-driven updates, low idle bandwidth.
    #[default]
    Desktop,
    /// Gaming VMs (phase C2): fixed high frame rate, lowest latency.
    Gaming,
    /// Servers, installers, firmware screens: framebuffer plus text.
    Console,
}

impl ConsoleProfile {
    pub const ALL: [ConsoleProfile; 3] = [
        ConsoleProfile::Desktop,
        ConsoleProfile::Gaming,
        ConsoleProfile::Console,
    ];

    pub fn name(&self) -> &'static str {
        match self {
            ConsoleProfile::Desktop => "desktop",
            ConsoleProfile::Gaming => "gaming",
            ConsoleProfile::Console => "console",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.name() == s)
    }
}

/// Which encoder family the host uses.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum EncoderPreference {
    /// Hardware when a working one is found (NVENC, then VA-API), else software.
    #[default]
    Auto,
    Software,
    Vaapi,
    Nvenc,
}

/// Rate control and pacing of one profile.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ProfileSettings {
    /// Highest frame rate sent; updates are damage-driven below it.
    pub max_fps: u32,
    /// Target bitrate the encoder starts at (kbit/s).
    pub bitrate_kbps: u32,
    /// Upper bound for the bitrate (kbit/s).
    pub max_bitrate_kbps: u32,
    /// Seconds between forced keyframes (a new viewer also gets one).
    pub keyframe_interval_secs: u32,
    /// Frames re-sent per second while the screen is idle, so a lossy
    /// first frame is refined and late joiners see a picture (0 = off).
    pub idle_refresh_fps: u32,
    /// Send frames at `max_fps` even when nothing changed (gaming) instead
    /// of only when the screen changed (desktop, console).
    pub fixed_rate: bool,
    /// Chroma subsampling: `420` (every client) or `444` (crisp text where
    /// the encoder and the client's decoder support it).
    pub chroma: String,
    /// Ask the guest tools for 10-bit HDR where the GPU and client allow.
    pub hdr: bool,
    /// Periodic intra refresh instead of large keyframes (smoother
    /// bitrate on lossy links).
    pub intra_refresh: bool,
    /// Capture and encode inside the guest when Horcrux guest tools are
    /// running (the node forwards the packets without transcoding);
    /// otherwise the node encodes QEMU's display.
    pub guest_capture: bool,
}

impl ProfileSettings {
    pub fn for_profile(p: ConsoleProfile) -> Self {
        match p {
            ConsoleProfile::Desktop => Self {
                max_fps: 30,
                bitrate_kbps: 4000,
                max_bitrate_kbps: 12000,
                keyframe_interval_secs: 4,
                idle_refresh_fps: 1,
                fixed_rate: false,
                chroma: "420".into(),
                hdr: false,
                intra_refresh: false,
                guest_capture: true,
            },
            ConsoleProfile::Gaming => Self {
                max_fps: 120,
                bitrate_kbps: 20000,
                max_bitrate_kbps: 50000,
                keyframe_interval_secs: 2,
                idle_refresh_fps: 0,
                fixed_rate: true,
                chroma: "420".into(),
                hdr: false,
                intra_refresh: false,
                guest_capture: true,
            },
            ConsoleProfile::Console => Self {
                max_fps: 15,
                bitrate_kbps: 1500,
                max_bitrate_kbps: 4000,
                keyframe_interval_secs: 4,
                idle_refresh_fps: 1,
                fixed_rate: false,
                chroma: "420".into(),
                hdr: false,
                intra_refresh: false,
                guest_capture: false,
            },
        }
    }

    pub fn validate(&self, name: &str) -> Result<(), String> {
        if self.max_fps == 0 || self.max_fps > 360 {
            return Err(format!("{name}.max_fps must be 1..=360"));
        }
        if self.bitrate_kbps < 100 || self.bitrate_kbps > self.max_bitrate_kbps {
            return Err(format!(
                "{name}.bitrate_kbps must be at least 100 and not above max_bitrate_kbps"
            ));
        }
        if self.max_bitrate_kbps > 500_000 {
            return Err(format!("{name}.max_bitrate_kbps must be at most 500000"));
        }
        if self.keyframe_interval_secs == 0 || self.keyframe_interval_secs > 600 {
            return Err(format!("{name}.keyframe_interval_secs must be 1..=600"));
        }
        if self.chroma != "420" && self.chroma != "444" {
            return Err(format!("{name}.chroma must be 420 or 444"));
        }
        if self.idle_refresh_fps > self.max_fps {
            return Err(format!("{name}.idle_refresh_fps must not exceed max_fps"));
        }
        Ok(())
    }
}

impl Default for ProfileSettings {
    fn default() -> Self {
        Self::for_profile(ConsoleProfile::Desktop)
    }
}

fn desktop() -> ProfileSettings {
    ProfileSettings::for_profile(ConsoleProfile::Desktop)
}
fn gaming() -> ProfileSettings {
    ProfileSettings::for_profile(ConsoleProfile::Gaming)
}
fn console() -> ProfileSettings {
    ProfileSettings::for_profile(ConsoleProfile::Console)
}

/// Node-wide Horcrux Console settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ConsoleSettings {
    /// Start VMs with the D-Bus display so the Horcrux Console can show
    /// them. Off: only the classic consoles (VNC/SPICE/serial).
    pub enabled: bool,
    /// Profile for VMs that don't ask for one.
    pub default_profile: ConsoleProfile,
    /// Which console the UI opens first: the Horcrux Console or the
    /// VM's classic console.
    pub prefer_classic: bool,
    /// Seconds a console ticket may wait to be redeemed.
    pub ticket_lifetime_secs: u32,
    /// Concurrent viewers per guest.
    pub max_sessions_per_guest: u32,
    /// Video codecs in order of preference; the first one the client also
    /// offers wins. Known: h264, vp8.
    pub codecs: Vec<String>,
    pub encoder: EncoderPreference,
    /// DRM render node used for VA-API encoding.
    pub vaapi_device: String,
    /// Encoder program (FFmpeg). Bare names are looked up in PATH.
    pub ffmpeg_path: String,
    /// Software H.264 speed/quality trade-off (x264 preset name).
    pub x264_preset: String,
    /// UDP ports for the WebRTC media of console sessions (one per session).
    pub udp_port_min: u16,
    pub udp_port_max: u16,
    /// Addresses offered to clients as ICE host candidates. Empty: the
    /// address the client used to reach this node, plus its interface
    /// addresses.
    pub candidate_addresses: Vec<String>,
    /// Carry guest audio (Opus) when the VM has a sound card.
    pub audio: bool,
    /// Largest frame encoded; bigger guest screens are scaled down.
    pub max_width: u32,
    pub max_height: u32,
    /// Seconds without any packet from the client before a session ends.
    pub session_idle_timeout_secs: u32,
    /// Write session start/stop and denied-input events to the audit log.
    pub audit_sessions: bool,
    /// Give every VM the Horcrux guest-tools port (virtio-serial) and use
    /// the guest tools when they answer.
    pub guest_tools: bool,
    /// Name of the guest-tools virtio-serial port (the guest tools open
    /// `/dev/virtio-ports/<name>` or `\\.\Global\<name>`).
    pub guest_tools_port: String,
    /// Milliseconds a new session waits for the guest tools to answer
    /// before it uses the node's own capture.
    pub guest_tools_wait_ms: u32,
    /// Seconds between checks, during a session that started without the
    /// guest tools, whether they answer now (just installed, or started at
    /// logon); the session then switches to them without reconnecting.
    /// 0 = only check when a session starts.
    pub guest_tools_retry_secs: u32,
    /// Guest tools CD image attached to console VMs (read-only, virtio-scsi)
    /// so the tools can be installed from inside the guest. Empty or a
    /// missing file: no CD.
    pub guest_tools_iso: String,
    /// Install the guest tools automatically through the QEMU guest agent
    /// when a viewer opens and the tools do not answer (Windows guests: the
    /// MSI from the tools CD; Linux guests: the distribution's package from
    /// the CD, .deb/.rpm/.apk).
    pub guest_tools_auto_install: bool,
    /// Longest wait for a guest tools installer started through the guest
    /// agent (s); a guest agent that stops answering for a moment while
    /// the installer runs is retried until then.
    pub guest_tools_install_timeout_secs: u32,
    /// After an automatic install attempt, the next viewer of the same VM
    /// does not try again for this long (s), so a guest whose install
    /// fails is not hammered on every viewer. The API action ignores it.
    pub guest_tools_auto_install_retry_secs: u32,
    /// Horcrux virtual display (Windows guest tools' virtual monitors):
    /// `auto` = when the VM's passed-through GPU had no monitor attached or
    /// has no display outputs at all (recorded at the GPU hand-off), so the
    /// guest still gets a desktop at the viewer's size and refresh rate;
    /// `always`; `never`.
    pub virtual_display: String,
    /// Largest clipboard text carried in either direction (bytes).
    pub clipboard_max_bytes: u32,
    /// Opus bitrate for guest audio (kbit/s).
    pub audio_bitrate_kbps: u32,
    pub desktop: ProfileSettings,
    pub gaming: ProfileSettings,
    pub console: ProfileSettings,
}

impl Default for ConsoleSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            default_profile: ConsoleProfile::Desktop,
            prefer_classic: false,
            ticket_lifetime_secs: 60,
            max_sessions_per_guest: 4,
            codecs: vec!["h264".into(), "vp8".into()],
            encoder: EncoderPreference::Auto,
            vaapi_device: "/dev/dri/renderD128".into(),
            ffmpeg_path: "ffmpeg".into(),
            x264_preset: "ultrafast".into(),
            udp_port_min: 61200,
            udp_port_max: 61299,
            candidate_addresses: Vec::new(),
            audio: true,
            max_width: 3840,
            max_height: 2160,
            session_idle_timeout_secs: 30,
            audit_sessions: true,
            guest_tools: true,
            guest_tools_port: "org.horcrux.console.0".into(),
            guest_tools_wait_ms: 1500,
            guest_tools_retry_secs: 5,
            guest_tools_iso: "/usr/share/horcrux/guest-tools/horcrux-guest-tools.iso".into(),
            guest_tools_auto_install: true,
            guest_tools_install_timeout_secs: 600,
            guest_tools_auto_install_retry_secs: 600,
            virtual_display: "auto".into(),
            clipboard_max_bytes: 1 << 20,
            audio_bitrate_kbps: 128,
            desktop: desktop(),
            gaming: gaming(),
            console: console(),
        }
    }
}

/// Values of [`ConsoleSettings::virtual_display`].
pub const VIRTUAL_DISPLAY_POLICIES: [&str; 3] = ["auto", "always", "never"];

/// Codecs the C1 encoder pipeline can produce.
pub const KNOWN_CODECS: [&str; 2] = ["h264", "vp8"];

impl ConsoleSettings {
    pub fn profile(&self, p: ConsoleProfile) -> &ProfileSettings {
        match p {
            ConsoleProfile::Desktop => &self.desktop,
            ConsoleProfile::Gaming => &self.gaming,
            ConsoleProfile::Console => &self.console,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(5..=3600).contains(&self.ticket_lifetime_secs) {
            return Err("ticket_lifetime_secs must be 5..=3600".into());
        }
        if !(1..=64).contains(&self.max_sessions_per_guest) {
            return Err("max_sessions_per_guest must be 1..=64".into());
        }
        if self.codecs.is_empty() {
            return Err("codecs must name at least one codec".into());
        }
        for c in &self.codecs {
            if !KNOWN_CODECS.contains(&c.as_str()) {
                return Err(format!(
                    "unknown codec {c:?} (known: {})",
                    KNOWN_CODECS.join(", ")
                ));
            }
        }
        if self.udp_port_min == 0 || self.udp_port_min > self.udp_port_max {
            return Err("udp_port_min must be non-zero and not above udp_port_max".into());
        }
        for a in &self.candidate_addresses {
            if a.parse::<std::net::IpAddr>().is_err() {
                return Err(format!("candidate address {a:?} is not an IP address"));
            }
        }
        if self.ffmpeg_path.trim().is_empty() {
            return Err("ffmpeg_path must not be empty".into());
        }
        if self.x264_preset.trim().is_empty()
            || !self
                .x264_preset
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err("x264_preset must be a preset name such as ultrafast".into());
        }
        if !(64..=16384).contains(&self.max_width) || !(64..=16384).contains(&self.max_height) {
            return Err("max_width/max_height must be 64..=16384".into());
        }
        if !(5..=3600).contains(&self.session_idle_timeout_secs) {
            return Err("session_idle_timeout_secs must be 5..=3600".into());
        }
        if self.guest_tools_port.is_empty()
            || !self
                .guest_tools_port
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        {
            return Err(
                "guest_tools_port must be a port name such as org.horcrux.console.0".into(),
            );
        }
        if self.guest_tools_wait_ms > 30_000 {
            return Err("guest_tools_wait_ms must be at most 30000".into());
        }
        if !(30..=7200).contains(&self.guest_tools_install_timeout_secs) {
            return Err("guest_tools_install_timeout_secs must be between 30 and 7200".into());
        }
        if self.guest_tools_auto_install_retry_secs > 86_400 {
            return Err("guest_tools_auto_install_retry_secs must be at most 86400".into());
        }
        if self.guest_tools_retry_secs > 3600 {
            return Err("guest_tools_retry_secs must be at most 3600".into());
        }
        if !VIRTUAL_DISPLAY_POLICIES.contains(&self.virtual_display.as_str()) {
            return Err(format!(
                "virtual_display must be one of {}",
                VIRTUAL_DISPLAY_POLICIES.join(", ")
            ));
        }
        if !(1..=64 * 1024 * 1024).contains(&self.clipboard_max_bytes) {
            return Err("clipboard_max_bytes must be 1..=67108864".into());
        }
        if !(6..=510).contains(&self.audio_bitrate_kbps) {
            return Err("audio_bitrate_kbps must be 6..=510".into());
        }
        self.desktop.validate("desktop")?;
        self.gaming.validate("gaming")?;
        self.console.validate("console")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_round_trip() {
        let s = ConsoleSettings::default();
        s.validate().unwrap();
        let json = serde_json::to_string(&s).unwrap();
        let back: ConsoleSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
        // A partial document keeps every other default.
        let partial: ConsoleSettings =
            serde_json::from_str(r#"{"ticket_lifetime_secs":30,"desktop":{"max_fps":20}}"#)
                .unwrap();
        assert_eq!(partial.ticket_lifetime_secs, 30);
        assert_eq!(partial.desktop.max_fps, 20);
        assert_eq!(partial.desktop.bitrate_kbps, 4000);
        assert_eq!(partial.codecs, vec!["h264", "vp8"]);
    }

    #[test]
    fn validation_rejects_bad_values() {
        let bad = |f: &dyn Fn(&mut ConsoleSettings)| {
            let mut s = ConsoleSettings::default();
            f(&mut s);
            s.validate().is_err()
        };
        assert!(bad(&|s| s.codecs = vec!["mpeg2".into()]));
        assert!(bad(&|s| s.codecs.clear()));
        assert!(bad(&|s| s.udp_port_min = s.udp_port_max + 1));
        assert!(bad(&|s| s.udp_port_min = 0));
        assert!(bad(&|s| s.candidate_addresses = vec!["not-an-ip".into()]));
        assert!(bad(&|s| s.desktop.max_fps = 0));
        assert!(bad(&|s| s.gaming.bitrate_kbps = 60000));
        assert!(bad(&|s| s.x264_preset = "fast; rm -rf /".into()));
        assert!(bad(&|s| s.ticket_lifetime_secs = 1));
    }

    #[test]
    fn profiles_parse() {
        for p in ConsoleProfile::ALL {
            assert_eq!(ConsoleProfile::parse(p.name()), Some(p));
        }
        assert_eq!(ConsoleProfile::parse("tv"), None);
    }
}
