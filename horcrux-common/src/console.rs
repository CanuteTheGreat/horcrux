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
    /// Ask the browser to show each frame as soon as it is decoded (WebRTC
    /// playout delay 0): no smoothing buffer, lowest input-to-picture time.
    /// Off: the browser paces playout itself (smoother on jittery links).
    pub low_latency_playout: bool,
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
                low_latency_playout: true,
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
                low_latency_playout: true,
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
                low_latency_playout: true,
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

/// WAN access and link adaptation (phase C3, docs/design/horcrux-console.md
/// section 12): the node's own relay (TURN) so clients outside the LAN reach
/// console sessions through one port, optional UPnP/NAT-PMP port mapping,
/// and adaptive bitrate/resolution/frame rate fitted to the measured link.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct WanSettings {
    /// Run the node's relay (TURN) so clients anywhere can connect. Off by
    /// default; a standalone machine needs nothing else (plus one forwarded
    /// port, or `upnp`).
    pub enabled: bool,
    /// Address the relay listens on (UDP, and TCP when `relay_tcp`).
    pub relay_listen_address: String,
    /// Relay port (UDP and TCP).
    pub relay_port: u16,
    /// Also accept TURN over TCP (networks that block UDP).
    pub relay_tcp: bool,
    /// With the relay on the unspecified address (0.0.0.0 / ::), answer
    /// each UDP request from the address it reached instead of the source
    /// address of the outgoing route (multi-homed nodes: several NICs, VM
    /// and container bridges, VPN interfaces). Turn off only to compare.
    pub relay_reply_from_request_address: bool,
    /// Name or address clients use for the relay (the router's public
    /// address or a DNS name). Empty: the address the client used to reach
    /// this node's web UI, or the external address UPnP/NAT-PMP reported.
    pub relay_public_address: String,
    /// Port clients use for the relay when the router forwards a different
    /// external port. 0: `relay_port` (or the mapped port).
    pub relay_public_port: u16,
    /// Address the relayed sockets bind to; the node's console sockets must
    /// reach it. Empty: the node's first non-loopback IPv4 address.
    pub relay_peer_address: String,
    /// Local UDP ports for relayed allocations (one per client).
    pub relay_port_min: u16,
    pub relay_port_max: u16,
    /// TURN realm.
    pub relay_realm: String,
    /// Seconds a session's relay credentials may be used for a new
    /// allocation (they are revoked when the session ends).
    pub relay_credential_lifetime_secs: u32,
    /// Concurrent relayed clients.
    pub relay_max_allocations: u32,
    /// Clients use the relay only (no direct attempts): hides the client
    /// address from the node's media sockets and proves the WAN path.
    pub force_relay: bool,
    /// Ask the router for a port mapping (UPnP IGD or NAT-PMP/PCP) for the
    /// relay port while WAN access is on. Opt-in.
    pub upnp: bool,
    /// Lease asked for a mapping (seconds); renewed at half-time.
    pub upnp_lease_secs: u32,
    /// Router to ask (`ip` or `ip:port` for NAT-PMP). Empty: the default
    /// route's gateway.
    pub upnp_gateway: String,
    /// Where UPnP discovery (SSDP) is sent.
    pub upnp_ssdp_address: String,
    /// The node's upload capacity (kbit/s). Relayed (WAN) sessions never
    /// exceed it; 0: unknown, the link estimate alone limits them.
    pub uplink_kbps: u32,
    /// Fit bitrate, resolution and frame rate to the measured link
    /// (congestion control estimate) instead of fixed profile rates.
    pub adaptive: bool,
    /// Lowest video bitrate adaptation goes to (kbit/s).
    pub min_kbps: u32,
    /// Picture quality floor in thousandths of a bit per pixel: below it the
    /// resolution (gaming: first) or the frame rate (desktop) is lowered.
    pub min_millibits_per_pixel: u32,
    /// Lowest frame rate desktop/console adaptation goes to.
    pub min_fps: u32,
    /// Milliseconds between two downward changes (an encoder change costs a
    /// keyframe).
    pub down_interval_ms: u32,
    /// Milliseconds the estimate must stay higher before stepping up.
    pub up_interval_ms: u32,
    /// Video packets kept for retransmission on a client's NACK (and for
    /// bandwidth probing); larger covers longer round trips at high rates.
    pub nack_history_packets: u32,
    /// Send FlexFEC (flexfec-03) repair packets for the video when the client
    /// offers FlexFEC: one lost packet per protected set is rebuilt at once
    /// instead of costing a retransmission round trip.
    pub fec: bool,
    /// Repair packets per 100 video packets (at least one per frame).
    pub fec_overhead_percent: u32,
    /// Protect only while the measured round trip is at least this long
    /// (ms); below it a retransmission is cheaper. 0: always.
    pub fec_min_rtt_ms: u32,
    /// Spread video packets over time at the link estimate (WebRTC pacer)
    /// while link adaptation runs. Off: each frame leaves at once.
    pub pace_video: bool,
}

impl Default for WanSettings {
    fn default() -> Self {
        WanSettings {
            enabled: false,
            relay_listen_address: "0.0.0.0".into(),
            relay_port: 3478,
            relay_tcp: true,
            relay_reply_from_request_address: true,
            relay_public_address: String::new(),
            relay_public_port: 0,
            relay_peer_address: String::new(),
            relay_port_min: 61300,
            relay_port_max: 61399,
            relay_realm: "horcrux".into(),
            relay_credential_lifetime_secs: 120,
            relay_max_allocations: 64,
            force_relay: false,
            upnp: false,
            upnp_lease_secs: 3600,
            upnp_gateway: String::new(),
            upnp_ssdp_address: "239.255.255.250:1900".into(),
            uplink_kbps: 0,
            adaptive: true,
            min_kbps: 250,
            min_millibits_per_pixel: 50,
            min_fps: 10,
            down_interval_ms: 1000,
            up_interval_ms: 4000,
            nack_history_packets: 4096,
            fec: true,
            fec_overhead_percent: 25,
            fec_min_rtt_ms: 40,
            pace_video: true,
        }
    }
}

impl WanSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self
            .relay_listen_address
            .parse::<std::net::IpAddr>()
            .is_err()
        {
            return Err("wan.relay_listen_address must be an IP address".into());
        }
        if self.relay_port == 0 {
            return Err("wan.relay_port must not be 0".into());
        }
        if !self.relay_peer_address.is_empty()
            && self.relay_peer_address.parse::<std::net::IpAddr>().is_err()
        {
            return Err("wan.relay_peer_address must be empty or an IP address".into());
        }
        if self.relay_public_address.len() > 253
            || !self
                .relay_public_address
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || ".-:[]".contains(c))
        {
            return Err("wan.relay_public_address must be a host name or an IP address".into());
        }
        if self.relay_port_min == 0 || self.relay_port_min > self.relay_port_max {
            return Err("wan.relay_port_min must be non-zero and not above relay_port_max".into());
        }
        if (self.relay_port_min..=self.relay_port_max).contains(&self.relay_port) {
            return Err("wan.relay_port must lie outside relay_port_min..relay_port_max".into());
        }
        if self.relay_realm.is_empty()
            || self.relay_realm.len() > 64
            || !self
                .relay_realm
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c))
        {
            return Err("wan.relay_realm must be 1-64 letters, digits, '.', '-' or '_'".into());
        }
        if !(10..=86_400).contains(&self.relay_credential_lifetime_secs) {
            return Err("wan.relay_credential_lifetime_secs must be 10..=86400".into());
        }
        if !(1..=4096).contains(&self.relay_max_allocations) {
            return Err("wan.relay_max_allocations must be 1..=4096".into());
        }
        if !(120..=86_400).contains(&self.upnp_lease_secs) {
            return Err("wan.upnp_lease_secs must be 120..=86400".into());
        }
        if !self.upnp_gateway.is_empty()
            && self.upnp_gateway.parse::<std::net::IpAddr>().is_err()
            && self.upnp_gateway.parse::<std::net::SocketAddr>().is_err()
        {
            return Err("wan.upnp_gateway must be empty, an IP address or ip:port".into());
        }
        if self
            .upnp_ssdp_address
            .parse::<std::net::SocketAddr>()
            .is_err()
        {
            return Err("wan.upnp_ssdp_address must be ip:port".into());
        }
        if self.uplink_kbps != 0 && self.uplink_kbps < 100 {
            return Err("wan.uplink_kbps must be 0 (unknown) or at least 100".into());
        }
        if !(50..=100_000).contains(&self.min_kbps) {
            return Err("wan.min_kbps must be 50..=100000".into());
        }
        if !(1..=2000).contains(&self.min_millibits_per_pixel) {
            return Err("wan.min_millibits_per_pixel must be 1..=2000".into());
        }
        if !(100..=65_536).contains(&self.nack_history_packets) {
            return Err("wan.nack_history_packets must be 100..=65536".into());
        }
        if !(1..=100).contains(&self.fec_overhead_percent) {
            return Err("wan.fec_overhead_percent must be 1..=100".into());
        }
        if self.fec_min_rtt_ms > 10_000 {
            return Err("wan.fec_min_rtt_ms must be at most 10000".into());
        }
        if !(1..=120).contains(&self.min_fps) {
            return Err("wan.min_fps must be 1..=120".into());
        }
        if !(100..=60_000).contains(&self.down_interval_ms)
            || !(100..=600_000).contains(&self.up_interval_ms)
        {
            return Err(
                "wan.down_interval_ms must be 100..=60000 and up_interval_ms 100..=600000".into(),
            );
        }
        Ok(())
    }
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
    /// Encode software H.264 in-process through the system libx264, so
    /// link adaptation changes the bitrate (and asks for keyframes) without
    /// restarting the encoder. Off, or no library found: the FFmpeg program.
    pub x264_in_process: bool,
    /// libx264 names or paths tried in order for the in-process encoder.
    pub x264_library: Vec<String>,
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
    /// Guest tools CD image attached to console VMs (read-only, virtio-scsi)
    /// so the tools can be installed from inside the guest. Empty or a
    /// missing file: no CD.
    pub guest_tools_iso: String,
    /// Install the guest tools automatically through the QEMU guest agent
    /// when a viewer opens and the tools do not answer (Windows guests: the
    /// MSI from the tools CD).
    pub guest_tools_auto_install: bool,
    /// Largest clipboard text carried in either direction (bytes).
    pub clipboard_max_bytes: u32,
    /// Opus bitrate for guest audio (kbit/s).
    pub audio_bitrate_kbps: u32,
    /// WAN access (relay, port mapping) and link adaptation.
    pub wan: WanSettings,
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
            x264_in_process: true,
            x264_library: vec![
                "libx264.so.165".into(),
                "libx264.so.164".into(),
                "libx264.so".into(),
            ],
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
            guest_tools_iso: "/usr/share/horcrux/guest-tools/horcrux-guest-tools.iso".into(),
            guest_tools_auto_install: true,
            clipboard_max_bytes: 1 << 20,
            audio_bitrate_kbps: 128,
            wan: WanSettings::default(),
            desktop: desktop(),
            gaming: gaming(),
            console: console(),
        }
    }
}

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
        if self.x264_library.len() > 16
            || self
                .x264_library
                .iter()
                .any(|l| l.trim().is_empty() || l.len() > 4096 || l.contains('\0'))
        {
            return Err(
                "x264_library must list at most 16 non-empty library names or paths".into(),
            );
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
        if !(1..=64 * 1024 * 1024).contains(&self.clipboard_max_bytes) {
            return Err("clipboard_max_bytes must be 1..=67108864".into());
        }
        if !(6..=510).contains(&self.audio_bitrate_kbps) {
            return Err("audio_bitrate_kbps must be 6..=510".into());
        }
        self.wan.validate()?;
        if (self.wan.relay_port_min..=self.wan.relay_port_max).contains(&self.udp_port_min)
            || (self.wan.relay_port_min..=self.wan.relay_port_max).contains(&self.udp_port_max)
            || (self.udp_port_min..=self.udp_port_max).contains(&self.wan.relay_port_min)
        {
            return Err(
                "wan.relay_port_min..relay_port_max must not overlap udp_port_min..udp_port_max"
                    .into(),
            );
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
        assert!(bad(&|s| s.wan.relay_listen_address = "any".into()));
        assert!(bad(&|s| s.wan.relay_public_address = "evil host; x".into()));
        assert!(bad(&|s| s.wan.relay_port = 61310));
        assert!(bad(&|s| s.wan.relay_port_min = 61250));
        assert!(bad(&|s| s.wan.uplink_kbps = 10));
        assert!(bad(&|s| s.wan.relay_credential_lifetime_secs = 0));
    }

    #[test]
    fn wan_defaults_off_and_partial_documents_keep_defaults() {
        let s = ConsoleSettings::default();
        assert!(!s.wan.enabled && !s.wan.upnp && !s.wan.force_relay && s.wan.adaptive);
        let partial: ConsoleSettings =
            serde_json::from_str(r#"{"wan":{"enabled":true,"uplink_kbps":50000}}"#).unwrap();
        assert!(partial.wan.enabled);
        assert_eq!(partial.wan.uplink_kbps, 50000);
        assert_eq!(partial.wan.relay_port, 3478);
        partial.validate().unwrap();
    }

    #[test]
    fn profiles_parse() {
        for p in ConsoleProfile::ALL {
            assert_eq!(ConsoleProfile::parse(p.name()), Some(p));
        }
        assert_eq!(ConsoleProfile::parse("tv"), None);
    }
}
