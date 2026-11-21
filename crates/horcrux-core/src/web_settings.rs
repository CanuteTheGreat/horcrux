//! Where the web UI / API listens, and how an upgrade keeps old installs working.
//!
//! Defaults (owner rule "stick to standard ports"): HTTPS on 443 and plain
//! HTTP on 80 that only redirects to HTTPS (and answers ACME HTTP-01 and
//! `/api/health*`). Extra HTTPS listeners (for example the historical 8006
//! used by existing API clients, cluster peers and Proxmox-style tools) are
//! opt-in.
//!
//! Layers, each replacing the one before key by key (all values typed,
//! documented, and settable through the API, the UI and the declarative spec):
//!
//! 1. built-in defaults ([`ServerConfig::default`](crate::config::ServerConfig))
//! 2. `[server]` in /etc/horcrux/config.toml
//! 3. environment: `HORCRUX_HOST`, `HORCRUX_PORT`, `HORCRUX_HTTP_REDIRECT_PORT`
//!    (`0` = off), `HORCRUX_EXTRA_HTTPS_PORTS` (comma list, `none` = off)
//! 4. `<data_dir>/web.json`, written by `PUT /api/system/web`, the UI card and a
//!    declarative `web:` section (deleting the file goes back to layers 1-3)
//!
//! Upgrades: the first start of this version on a node that already has a
//! database and never configured a port keeps its historical listener: it
//! writes `web.json` with `port = 8006`, the standard redirect on 80 and an
//! extra HTTPS listener on 443, so existing clients and cluster peers (which
//! reach each other on the configured API port) keep working while the node
//! also answers on the standard ports. Switching the node to 443 only is then
//! one setting (`port`) once every cluster member has been upgraded. A node
//! that configured a port explicitly (config.toml or `HORCRUX_PORT`) keeps it
//! untouched. Fresh installs get the standard defaults.
use crate::config::{HorcruxConfig, ServerConfig};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::{Path, PathBuf};

/// The port older versions listened on by default (Proxmox-compatible).
pub const LEGACY_PORT: u16 = 8006;
/// Marker file in the data dir: the upgrade decision above was already made.
const MARKER: &str = ".web-ports-v2";
const STORE_FILE: &str = "web.json";

/// The listener settings a node keeps in `web.json` (API / UI / declarative).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebSettings {
    /// Address to bind (`0.0.0.0`, `::` or one interface address).
    pub host: String,
    /// Main HTTPS port (also answers plain HTTP there with a redirect).
    pub port: u16,
    /// Plain-HTTP port that only redirects to HTTPS (`null` = off).
    pub http_redirect_port: Option<u16>,
    /// Further HTTPS ports serving the same API (existing clients, peers).
    #[serde(default)]
    pub extra_https_ports: Vec<u16>,
}

impl Default for WebSettings {
    fn default() -> Self {
        WebSettings::from(&ServerConfig::default())
    }
}

impl From<&ServerConfig> for WebSettings {
    fn from(s: &ServerConfig) -> Self {
        WebSettings {
            host: s.host.clone(),
            port: s.port,
            http_redirect_port: s.http_redirect_port,
            extra_https_ports: s.extra_https_ports.clone(),
        }
    }
}

impl WebSettings {
    /// Reject what cannot be bound or would collide.
    pub fn validate(&self) -> Result<(), String> {
        if self.host.parse::<IpAddr>().is_err() {
            return Err(format!("host {:?} is not an IP address", self.host));
        }
        if self.port == 0 {
            return Err("port cannot be 0".into());
        }
        let mut seen = vec![self.port];
        if let Some(p) = self.http_redirect_port {
            if p == 0 {
                return Err("http_redirect_port cannot be 0 (use null to turn it off)".into());
            }
            seen.push(p);
        }
        for p in &self.extra_https_ports {
            if *p == 0 {
                return Err("extra_https_ports cannot contain 0".into());
            }
            if seen.contains(p) {
                return Err(format!(
                    "port {p} is used twice (port, http_redirect_port, extra_https_ports)"
                ));
            }
            seen.push(*p);
        }
        Ok(())
    }

    /// Every HTTPS port, main one first.
    pub fn https_ports(&self) -> Vec<u16> {
        let mut v = vec![self.port];
        v.extend(self.extra_https_ports.iter().copied());
        v
    }

    /// Every TCP port the web service listens on (for firewall rules).
    pub fn all_ports(&self) -> Vec<u16> {
        let mut v = self.https_ports();
        v.extend(self.http_redirect_port);
        v
    }

    /// Put these values into a server config.
    pub fn apply_to(&self, s: &mut ServerConfig) {
        s.host = self.host.clone();
        s.port = self.port;
        s.http_redirect_port = self.http_redirect_port;
        s.extra_https_ports = self.extra_https_ports.clone();
    }
}

/// Parse `HORCRUX_EXTRA_HTTPS_PORTS` (`8006,8443`; empty / `none` / `off` = none).
pub fn parse_port_list(s: &str) -> Vec<u16> {
    match s.trim().to_ascii_lowercase().as_str() {
        "" | "none" | "off" | "0" => Vec::new(),
        v => v
            .split(',')
            .filter_map(|p| p.trim().parse::<u16>().ok())
            .filter(|p| *p != 0)
            .collect(),
    }
}

/// `<data_dir>/web.json`.
pub fn store_path(data_dir: &Path) -> PathBuf {
    data_dir.join(STORE_FILE)
}

/// The stored settings, if the file exists and is valid.
pub fn load_stored(data_dir: &Path) -> Option<WebSettings> {
    let path = store_path(data_dir);
    let bytes = std::fs::read(&path).ok()?;
    match serde_json::from_slice::<WebSettings>(&bytes) {
        Ok(s) if s.validate().is_ok() => Some(s),
        _ => {
            tracing::warn!(
                "{} is not valid web settings; using config.toml / environment",
                path.display()
            );
            None
        }
    }
}

/// Validate and persist (atomically, mode 0644: not secret).
pub fn save(data_dir: &Path, s: &WebSettings) -> Result<(), String> {
    s.validate()?;
    std::fs::create_dir_all(data_dir).map_err(|e| format!("{}: {e}", data_dir.display()))?;
    let path = store_path(data_dir);
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, json).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Remove the stored override (back to config.toml / environment / defaults).
pub fn reset(data_dir: &Path) -> Result<(), String> {
    match std::fs::remove_file(store_path(data_dir)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// The path of a `sqlite:` database URL, if it is one.
fn sqlite_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("sqlite:")?;
    let rest = rest.trim_start_matches("//");
    let rest = rest.split('?').next().unwrap_or(rest);
    if rest.is_empty() || rest == ":memory:" {
        return None;
    }
    Some(PathBuf::from(if rest.starts_with('/') {
        rest.to_string()
    } else {
        format!("/{rest}")
    }))
}

/// Did the node pick a port itself (config.toml `[server] port`, or env)?
fn port_configured_explicitly() -> bool {
    if std::env::var("HORCRUX_PORT").is_ok() {
        return true;
    }
    HorcruxConfig::config_file_value("server", "port").is_some()
}

/// What the first start of this version should do on this node.
#[derive(Debug, PartialEq, Eq)]
pub enum Upgrade {
    /// Decision already made earlier.
    Done,
    /// Fresh node: standard defaults.
    Fresh,
    /// Existing node that never set a port: keep 8006 and add the standards.
    KeepLegacy(WebSettings),
    /// Existing node that set its port itself: untouched.
    KeepExplicit,
}

/// Decide (without writing anything) what the upgrade does.
pub fn plan_upgrade(cfg: &HorcruxConfig) -> Upgrade {
    if cfg.paths.data_dir.join(MARKER).exists() || store_path(&cfg.paths.data_dir).exists() {
        return Upgrade::Done;
    }
    let existing = sqlite_path(&cfg.database.url)
        .map(|p| p.exists())
        .unwrap_or(false);
    if !existing {
        return Upgrade::Fresh;
    }
    if port_configured_explicitly() {
        return Upgrade::KeepExplicit;
    }
    let mut w = WebSettings::from(&cfg.server);
    w.port = LEGACY_PORT;
    let std_port = ServerConfig::default().port;
    w.extra_https_ports = if std_port == LEGACY_PORT {
        Vec::new()
    } else {
        vec![std_port]
    };
    Upgrade::KeepLegacy(w)
}

/// Run [`plan_upgrade`] and record the outcome. Call once at API start, before
/// the database is opened. Returns the settings now in force.
pub fn run_upgrade(cfg: &mut HorcruxConfig) -> Upgrade {
    let plan = plan_upgrade(cfg);
    let dir = cfg.paths.data_dir.clone();
    match &plan {
        Upgrade::Done => {}
        Upgrade::Fresh | Upgrade::KeepExplicit => {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join(MARKER), b"");
        }
        Upgrade::KeepLegacy(w) => match save(&dir, w) {
            Ok(()) => {
                let _ = std::fs::write(dir.join(MARKER), b"");
                tracing::info!(
                        "upgrade: this node keeps its API on port {} and also listens on the standard ports ({:?}); change `port` in Settings > Web listeners when every cluster member is upgraded",
                        w.port,
                        w.extra_https_ports
                    );
                w.apply_to(&mut cfg.server);
            }
            Err(e) => tracing::warn!("upgrade: could not record the legacy API port: {e}"),
        },
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(dir: &Path) -> HorcruxConfig {
        let mut c = HorcruxConfig::default();
        c.paths.data_dir = dir.to_path_buf();
        c.database.url = format!("sqlite://{}/horcrux.db", dir.display());
        c
    }

    #[test]
    fn defaults_are_standard() {
        let w = WebSettings::default();
        assert_eq!(w.port, 443);
        assert_eq!(w.http_redirect_port, Some(80));
        assert!(w.extra_https_ports.is_empty());
        assert!(w.validate().is_ok());
        assert_eq!(w.all_ports(), vec![443, 80]);
    }

    #[test]
    fn validation_rejects_collisions_and_zero() {
        let mut w = WebSettings::default();
        w.extra_https_ports = vec![443];
        assert!(w.validate().is_err());
        w.extra_https_ports = vec![80];
        assert!(w.validate().is_err());
        w.extra_https_ports = vec![8006, 8006];
        assert!(w.validate().is_err());
        w.extra_https_ports = vec![8006];
        assert!(w.validate().is_ok());
        w.port = 0;
        assert!(w.validate().is_err());
        let mut w = WebSettings::default();
        w.host = "not-an-ip".into();
        assert!(w.validate().is_err());
    }

    #[test]
    fn port_list_parsing() {
        assert_eq!(parse_port_list("8006, 8443"), vec![8006, 8443]);
        assert!(parse_port_list("none").is_empty());
        assert!(parse_port_list("").is_empty());
        assert_eq!(parse_port_list("x,8006,0"), vec![8006]);
    }

    #[test]
    fn store_round_trip_and_reset() {
        let d = tempfile::tempdir().unwrap();
        assert!(load_stored(d.path()).is_none());
        let mut w = WebSettings::default();
        w.extra_https_ports = vec![8006];
        save(d.path(), &w).unwrap();
        assert_eq!(load_stored(d.path()), Some(w));
        reset(d.path()).unwrap();
        assert!(load_stored(d.path()).is_none());
        reset(d.path()).unwrap();
    }

    #[test]
    fn fresh_node_gets_defaults_and_marker() {
        let d = tempfile::tempdir().unwrap();
        let mut c = cfg(d.path());
        assert_eq!(plan_upgrade(&c), Upgrade::Fresh);
        run_upgrade(&mut c);
        assert_eq!(c.server.port, 443);
        assert!(d.path().join(MARKER).exists());
        // a database appearing later must not turn it into an "old install"
        std::fs::write(d.path().join("horcrux.db"), b"x").unwrap();
        assert_eq!(plan_upgrade(&c), Upgrade::Done);
    }

    #[test]
    fn existing_node_keeps_legacy_port_and_gains_standard_ones() {
        // This test relies on HORCRUX_PORT / config.toml not pinning a port.
        if port_configured_explicitly() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("horcrux.db"), b"x").unwrap();
        let mut c = cfg(d.path());
        let Upgrade::KeepLegacy(w) = plan_upgrade(&c) else {
            panic!("expected KeepLegacy");
        };
        assert_eq!(w.port, 8006);
        assert_eq!(w.extra_https_ports, vec![443]);
        assert_eq!(w.http_redirect_port, Some(80));
        run_upgrade(&mut c);
        assert_eq!(c.server.port, 8006);
        assert_eq!(load_stored(d.path()).unwrap().port, 8006);
        assert_eq!(plan_upgrade(&c), Upgrade::Done);
    }
}
