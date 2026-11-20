//! Settings of the desktop client: a TOML file with a default for every
//! value, each overridable on the command line. Per-user file:
//! `$XDG_CONFIG_HOME/horcrux/console.toml` (Linux and other Unix),
//! `%APPDATA%\horcrux\console.toml` (Windows),
//! `~/Library/Application Support/horcrux/console.toml` (macOS).
use clap::Parser;
use horcrux_media::decoder::{default_avcodec, default_avutil};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Node URL (`https://host:8006`).
    pub node: String,
    pub user: String,
    /// Environment variable holding the password (asked on the terminal
    /// when it is not set).
    pub password_env: String,
    /// VM to view.
    pub vm: String,
    /// Console profile to ask for (`desktop`, `gaming`, `console`); empty:
    /// the VM's own.
    pub profile: String,
    /// PEM file with the node's CA (System > Certificates > CA, or
    /// `/api/tls/ca.pem`).
    pub ca_file: String,
    /// Or: the SHA-256 fingerprint (hex) of the node's certificate.
    pub cert_sha256: String,
    /// H.264 decoder: `auto` (the system's own: libavcodec on Linux, Media
    /// Foundation on Windows, VideoToolbox on macOS), `avcodec`,
    /// `mediafoundation` or `videotoolbox`.
    pub decoder: String,
    /// Decode on the GPU where the decoder can (DXVA, VideoToolbox).
    pub hardware_decode: bool,
    /// `avcodec` decoder: FFmpeg libraries, tried in order.
    pub avcodec: Vec<String>,
    pub avutil: Vec<String>,
    /// Key that releases a captured mouse (W3C `code` name).
    pub release_key: String,
    /// Mouse capture with raw relative motion: `auto` (gaming profile with
    /// guest tools: on click), `on`, `off`.
    pub relative_mouse: String,
    pub fullscreen: bool,
    pub gamepads: bool,
    /// How often gamepads are read where the system is polled (XInput on
    /// Windows, GameController on macOS); Linux reads evdev events.
    pub gamepad_poll_hz: u32,
    /// Refresh rate reported to the node (frames are paced to it); 0: the
    /// monitor's.
    pub refresh_hz: u32,
    /// Initial window size.
    pub width: u32,
    pub height: u32,
    /// Milliseconds to wait for the relay or the media connection.
    pub connect_timeout_ms: u64,
    /// Connection window: remember the last node, user, VM and profile (and
    /// certificates trusted there) in `recent.toml` next to this file.
    pub remember_connection: bool,
    /// Connection window text size (1-4); 0: from the monitor's scale.
    pub ui_scale: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            node: String::new(),
            user: "root".into(),
            password_env: "HORCRUX_PASSWORD".into(),
            vm: String::new(),
            profile: String::new(),
            ca_file: String::new(),
            cert_sha256: String::new(),
            decoder: "auto".into(),
            hardware_decode: true,
            avcodec: default_avcodec(),
            avutil: default_avutil(),
            release_key: "ControlRight".into(),
            relative_mouse: "auto".into(),
            fullscreen: false,
            gamepads: true,
            gamepad_poll_hz: 250,
            refresh_hz: 0,
            width: 1280,
            height: 800,
            connect_timeout_ms: 15_000,
            remember_connection: true,
            ui_scale: 0,
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "horcrux-console",
    version,
    about = "Horcrux Console desktop client"
)]
pub struct Args {
    /// Settings file (TOML).
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long)]
    pub node: Option<String>,
    #[arg(long)]
    pub user: Option<String>,
    #[arg(long)]
    pub vm: Option<String>,
    #[arg(long)]
    pub profile: Option<String>,
    #[arg(long)]
    pub ca_file: Option<String>,
    #[arg(long)]
    pub cert_sha256: Option<String>,
    #[arg(long)]
    pub fullscreen: bool,
    #[arg(long)]
    pub relative_mouse: Option<String>,
    #[arg(long)]
    pub refresh_hz: Option<u32>,
    /// No window: connect, decode, run the scripted checks below, print a
    /// JSON report (CI e2e and diagnostics).
    #[arg(long)]
    pub headless: bool,
    /// Headless: keystroke trials (key-to-picture latency).
    #[arg(long, default_value_t = 0)]
    pub trials: u32,
    /// Headless: seconds to run before the trials.
    #[arg(long, default_value_t = 5)]
    pub warmup_secs: u64,
    /// Headless: also send one gamepad state and one relative motion.
    #[arg(long)]
    pub exercise_input: bool,
    /// Headless: type this text (US layout; a newline or `\n` is Enter) after the
    /// warm-up, before the trials (e.g. to log in on a test guest).
    #[arg(long)]
    pub type_text: Option<String>,
    /// Headless: write the last decoded picture to this file (PPM).
    #[arg(long)]
    pub snapshot: Option<String>,
    /// Print the default settings file and exit.
    #[arg(long)]
    pub print_defaults: bool,
    /// Always open the connection window (even when node and vm are set).
    #[arg(long)]
    pub connect_window: bool,
    /// A `horcrux-console://HOST[:PORT]/VM?profile=P&user=U` link (the web
    /// UI's "Open in the desktop app"): fills in the connection window,
    /// which then asks for the password.
    pub url: Option<String>,
}

/// The per-user settings directory (`horcrux` under the system's place
/// for application settings).
pub fn settings_dir() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Library").join("Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }?;
    Some(base.join("horcrux"))
}

pub fn default_path() -> Option<std::path::PathBuf> {
    Some(settings_dir()?.join("console.toml"))
}

/// URL scheme of desktop links.
pub const URL_SCHEME: &str = "horcrux-console";

/// What a `horcrux-console://` link asks for.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Link {
    /// `https://host:port` (links always use HTTPS).
    pub node: String,
    pub vm: String,
    pub profile: Option<String>,
    pub user: Option<String>,
}

fn vm_id_ok(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 128
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

/// Parse `horcrux-console://HOST[:PORT]/VM[?profile=P&user=U]`. Strict:
/// any other part (credentials, fragments, unknown keys, odd characters) is
/// refused, because any web page can open such a link.
pub fn parse_link(url: &str) -> Result<Link, String> {
    let rest = url
        .strip_prefix(URL_SCHEME)
        .and_then(|r| r.strip_prefix("://"))
        .ok_or_else(|| format!("not a {URL_SCHEME}:// link"))?;
    if rest.contains('#') || rest.contains('@') || rest.contains('\\') {
        return Err("link has parts a console link never has".into());
    }
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (hostport, vm) = path.split_once('/').ok_or("link has no VM")?;
    let vm = vm.trim_end_matches('/');
    if !vm_id_ok(vm) {
        return Err("link's VM is not a VM id".into());
    }
    let u = reqwest::Url::parse(&format!("https://{hostport}/"))
        .map_err(|e| format!("link's host: {e}"))?;
    let host = u.host_str().ok_or("link has no host")?;
    if host.is_empty() || !u.username().is_empty() || u.password().is_some() {
        return Err("link's host is not a host".into());
    }
    let mut link = Link {
        node: format!("https://{hostport}"),
        vm: vm.to_string(),
        ..Default::default()
    };
    for kv in query.split('&').filter(|kv| !kv.is_empty()) {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        let v = urlencoding_decode(v)?;
        match k {
            "profile" if ["desktop", "gaming", "console", ""].contains(&v.as_str()) => {
                link.profile = Some(v)
            }
            "user"
                if !v.is_empty()
                    && v.len() <= 128
                    && v.chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_.@".contains(c)) =>
            {
                link.user = Some(v)
            }
            _ => return Err(format!("link parameter {k:?} is not allowed")),
        }
    }
    Ok(link)
}

fn urlencoding_decode(v: &str) -> Result<String, String> {
    let b = v.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let h = std::str::from_utf8(&b[i + 1..i + 3]).map_err(|e| e.to_string())?;
            out.push(u8::from_str_radix(h, 16).map_err(|_| "bad %-escape in link")?);
            i += 3;
        } else if b[i] == b'%' {
            return Err("bad %-escape in link".into());
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|e| e.to_string())
}

/// The connection window's memory (`recent.toml`): last connection and
/// certificates the user chose to trust, per node.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Recent {
    pub node: String,
    pub user: String,
    pub vm: String,
    pub profile: String,
    /// node URL -> SHA-256 fingerprint the user trusted.
    pub trusted: std::collections::BTreeMap<String, String>,
}

impl Recent {
    pub fn path(settings_file: Option<&std::path::Path>) -> Option<std::path::PathBuf> {
        match settings_file {
            Some(p) => Some(p.with_file_name("recent.toml")),
            None => Some(settings_dir()?.join("recent.toml")),
        }
    }
    pub fn load(path: &std::path::Path) -> Recent {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default()
    }
    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, toml::to_string(self).map_err(|e| e.to_string())?)
            .map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
    }
}

impl Settings {
    pub fn load(args: &Args) -> Result<Settings, String> {
        let mut s = match Self::file(args) {
            Some(p) if p.exists() => {
                let text =
                    std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
                toml::from_str::<Settings>(&text).map_err(|e| format!("{}: {e}", p.display()))?
            }
            Some(p) if args.config.is_some() => {
                return Err(format!("{}: no such file", p.display()))
            }
            _ => Settings::default(),
        };
        let set = |dst: &mut String, v: &Option<String>| {
            if let Some(v) = v {
                *dst = v.clone();
            }
        };
        set(&mut s.node, &args.node);
        set(&mut s.user, &args.user);
        set(&mut s.vm, &args.vm);
        set(&mut s.profile, &args.profile);
        set(&mut s.ca_file, &args.ca_file);
        set(&mut s.cert_sha256, &args.cert_sha256);
        set(&mut s.relative_mouse, &args.relative_mouse);
        if args.fullscreen {
            s.fullscreen = true;
        }
        if let Some(r) = args.refresh_hz {
            s.refresh_hz = r;
        }
        s.validate_values()?;
        Ok(s)
    }

    /// Where the settings were read from (or would be).
    pub fn file(args: &Args) -> Option<std::path::PathBuf> {
        args.config
            .clone()
            .map(std::path::PathBuf::from)
            .or_else(default_path)
    }

    /// Node and VM are set: connect without the window.
    pub fn has_target(&self) -> bool {
        !self.node.is_empty() && !self.vm.is_empty()
    }

    /// Everything, including a node and a VM to connect to.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_target()?;
        self.validate_values()
    }

    pub fn validate_target(&self) -> Result<(), String> {
        if !(self.node.starts_with("https://") || self.node.starts_with("http://"))
            || reqwest::Url::parse(&self.node)
                .ok()
                .and_then(|u| u.host_str().map(|h| !h.is_empty()))
                != Some(true)
        {
            return Err("node must be a URL such as https://host:8006".into());
        }
        if !vm_id_ok(&self.vm) {
            return Err("vm must be a VM id".into());
        }
        Ok(())
    }

    /// The values that have to be right whether or not a target is set.
    pub fn validate_values(&self) -> Result<(), String> {
        if !self.vm.is_empty() && !vm_id_ok(&self.vm) {
            return Err("vm must be a VM id".into());
        }
        if !["auto", "avcodec", "mediafoundation", "videotoolbox"].contains(&self.decoder.as_str())
        {
            return Err("decoder must be auto, avcodec, mediafoundation or videotoolbox".into());
        }
        if self.gamepad_poll_hz == 0 || self.gamepad_poll_hz > 1000 {
            return Err("gamepad_poll_hz must be 1 to 1000".into());
        }
        if self.ui_scale > 4 {
            return Err("ui_scale must be 0 (auto) to 4".into());
        }
        if !["", "desktop", "gaming", "console"].contains(&self.profile.as_str()) {
            return Err("profile must be desktop, gaming or console".into());
        }
        if !["auto", "on", "off"].contains(&self.relative_mouse.as_str()) {
            return Err("relative_mouse must be auto, on or off".into());
        }
        if !self.cert_sha256.is_empty()
            && (self.cert_sha256.replace(':', "").len() != 64
                || !self
                    .cert_sha256
                    .replace(':', "")
                    .chars()
                    .all(|c| c.is_ascii_hexdigit()))
        {
            return Err("cert_sha256 must be 64 hex digits".into());
        }
        if self.refresh_hz > 1000 || self.width < 64 || self.height < 64 {
            return Err("refresh_hz at most 1000, width/height at least 64".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The packaged example (packaging/linux/horcrux-console.toml) is the
    /// defaults: regenerate it with `horcrux-console --print-defaults`.
    #[test]
    fn packaged_example_is_the_defaults() {
        let text = include_str!("../../packaging/linux/horcrux-console.toml");
        let body: String = text
            .lines()
            .filter(|l| !l.starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            toml::from_str::<Settings>(&body).unwrap(),
            Settings::default()
        );
        assert_eq!(
            body.trim(),
            toml::to_string(&Settings::default()).unwrap().trim()
        );
    }

    #[test]
    fn defaults_round_trip_and_validate() {
        let d = Settings::default();
        let t = toml::to_string(&d).unwrap();
        assert_eq!(toml::from_str::<Settings>(&t).unwrap(), d);
        let mut s = d.clone();
        assert!(s.validate().is_err(), "no node");
        s.node = "https://n:8006".into();
        s.vm = "vm-1".into();
        assert!(s.validate().is_ok());
        s.cert_sha256 = "zz".into();
        assert!(s.validate().is_err());
        s.cert_sha256 = "ab".repeat(32);
        assert!(s.validate().is_ok());
        s.relative_mouse = "maybe".into();
        assert!(s.validate().is_err());
        // a partial file keeps the other defaults
        let p: Settings = toml::from_str("node = \"https://x\"\nvm = \"a\"\n").unwrap();
        assert_eq!(p.release_key, "ControlRight");
        // values are checked without a target (connection window start)
        let mut w = Settings::default();
        assert!(w.validate_values().is_ok() && !w.has_target());
        w.decoder = "ffmpeg.dll".into();
        assert!(w.validate_values().is_err());
        w.decoder = "mediafoundation".into();
        w.gamepad_poll_hz = 0;
        assert!(w.validate_values().is_err());
    }

    #[test]
    fn console_links() {
        let l = parse_link("horcrux-console://node.example:8006/vm-101?profile=gaming&user=alice")
            .unwrap();
        assert_eq!(
            l,
            Link {
                node: "https://node.example:8006".into(),
                vm: "vm-101".into(),
                profile: Some("gaming".into()),
                user: Some("alice".into()),
            }
        );
        let l = parse_link("horcrux-console://[fd00::1]:8006/a_b.c/").unwrap();
        assert_eq!(
            (l.node.as_str(), l.vm.as_str()),
            ("https://[fd00::1]:8006", "a_b.c")
        );
        assert_eq!(
            parse_link("horcrux-console://n/v?user=bob%40corp")
                .unwrap()
                .user,
            Some("bob@corp".into())
        );
        for bad in [
            "https://n/v",
            "horcrux-console://n",
            "horcrux-console://n/",
            "horcrux-console://u:p@n/v",
            "horcrux-console://n/v#x",
            "horcrux-console://n/v?password=x",
            "horcrux-console://n/v?profile=evil",
            "horcrux-console://n/v/../w",
            "horcrux-console://n/v%2F..",
            "horcrux-console://n/v?user=a%2",
            "horcrux-console://n/v?user=a%20b",
        ] {
            assert!(parse_link(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn recent_round_trip() {
        let dir = std::env::temp_dir().join(format!("hxc-recent-{}", std::process::id()));
        let p = dir.join("recent.toml");
        assert_eq!(Recent::load(&p), Recent::default());
        let mut r = Recent {
            node: "https://n:8006".into(),
            vm: "v".into(),
            ..Default::default()
        };
        r.trusted.insert("https://n:8006".into(), "ab".repeat(32));
        r.save(&p).unwrap();
        assert_eq!(Recent::load(&p), r);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            Recent::path(Some(std::path::Path::new("/x/console.toml"))),
            Some(std::path::PathBuf::from("/x/recent.toml"))
        );
    }
}
