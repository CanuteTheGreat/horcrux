//! Settings of the desktop client: a TOML file
//! (`$XDG_CONFIG_HOME/horcrux/console.toml` by default) with a default for
//! every value, each overridable on the command line.
use clap::Parser;
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
    /// FFmpeg libraries for H.264 decoding, tried in order.
    pub avcodec: Vec<String>,
    pub avutil: Vec<String>,
    /// Key that releases a captured mouse (W3C `code` name).
    pub release_key: String,
    /// Mouse capture with raw relative motion: `auto` (gaming profile with
    /// guest tools: on click), `on`, `off`.
    pub relative_mouse: String,
    pub fullscreen: bool,
    pub gamepads: bool,
    /// Refresh rate reported to the node (frames are paced to it); 0: the
    /// monitor's.
    pub refresh_hz: u32,
    /// Initial window size.
    pub width: u32,
    pub height: u32,
    /// Milliseconds to wait for the relay or the media connection.
    pub connect_timeout_ms: u64,
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
            avcodec: horcrux_media::avcodec::default_avcodec(),
            avutil: horcrux_media::avcodec::default_avutil(),
            release_key: "ControlRight".into(),
            relative_mouse: "auto".into(),
            fullscreen: false,
            gamepads: true,
            refresh_hz: 0,
            width: 1280,
            height: 800,
            connect_timeout_ms: 15_000,
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
    /// Print the default settings file and exit.
    #[arg(long)]
    pub print_defaults: bool,
}

pub fn default_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config"))
        })?;
    Some(base.join("horcrux").join("console.toml"))
}

impl Settings {
    pub fn load(args: &Args) -> Result<Settings, String> {
        let path = args
            .config
            .clone()
            .map(std::path::PathBuf::from)
            .or_else(default_path);
        let mut s = match path {
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
        s.validate()?;
        Ok(s)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(self.node.starts_with("https://") || self.node.starts_with("http://")) {
            return Err("node must be a URL such as https://host:8006".into());
        }
        if self.vm.is_empty()
            || !self
                .vm
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        {
            return Err("vm must be a VM id".into());
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
    }
}
