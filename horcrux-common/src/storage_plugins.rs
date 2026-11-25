//! Node settings of third-party storage plugins (`[storage_plugins]` in
//! config.toml, `PUT /api/storage/plugins/settings`, declarative
//! `storage_plugins:`). See docs/storage-plugins.md.
use serde::{Deserialize, Serialize};

/// Default plugin directory: every executable in it is a storage plugin.
pub const DEFAULT_PLUGIN_DIR: &str = "/usr/libexec/horcrux/storage-plugins";

fn d_plugin_dir() -> String {
    DEFAULT_PLUGIN_DIR.into()
}
fn yes() -> bool {
    true
}
fn d_output() -> u32 {
    1024
}

/// Per-operation time limits, in seconds. A plugin process still running at
/// its limit is killed and the call fails with reason `storage_plugin_timeout`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginTimeouts {
    /// `describe` (listing plugins, opening the pool form).
    pub describe_secs: u32,
    /// `activate` (mount / log in; runs at pool add and at every start).
    pub activate_secs: u32,
    pub deactivate_secs: u32,
    /// `status` (capacity, on every pool list).
    pub status_secs: u32,
    /// `list_volumes`, `list_snapshots`.
    pub list_secs: u32,
    pub create_secs: u32,
    pub delete_secs: u32,
    pub resize_secs: u32,
    /// snapshot create / delete / rollback.
    pub snapshot_secs: u32,
    /// `volume_path`.
    pub path_secs: u32,
    /// `map_volume` / `unmap_volume`.
    pub map_secs: u32,
}

impl Default for PluginTimeouts {
    fn default() -> Self {
        Self {
            describe_secs: 10,
            activate_secs: 120,
            deactivate_secs: 60,
            status_secs: 15,
            list_secs: 30,
            create_secs: 600,
            delete_secs: 300,
            resize_secs: 300,
            snapshot_secs: 300,
            path_secs: 15,
            map_secs: 60,
        }
    }
}

impl PluginTimeouts {
    /// (key, value) pairs, for validation and messages.
    pub fn entries(&self) -> [(&'static str, u32); 11] {
        [
            ("describe_secs", self.describe_secs),
            ("activate_secs", self.activate_secs),
            ("deactivate_secs", self.deactivate_secs),
            ("status_secs", self.status_secs),
            ("list_secs", self.list_secs),
            ("create_secs", self.create_secs),
            ("delete_secs", self.delete_secs),
            ("resize_secs", self.resize_secs),
            ("snapshot_secs", self.snapshot_secs),
            ("path_secs", self.path_secs),
            ("map_secs", self.map_secs),
        ]
    }
}

/// The node's storage plugin settings. Every field has a documented default,
/// so an empty section (or none) is valid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoragePluginSettings {
    /// Directory scanned for plugin executables (file name = plugin id).
    /// Default `/usr/libexec/horcrux/storage-plugins`.
    #[serde(default = "d_plugin_dir")]
    pub plugin_dir: String,
    /// Per-pool working directories handed to plugins (mount points, state).
    /// Empty (the default) = `<data_dir>/storage-plugins`.
    pub state_dir: String,
    /// Only run plugins owned by root and not writable by group/others, in a
    /// directory with the same property (default true). Plugins run as the
    /// manager's user, usually root.
    #[serde(default = "yes")]
    pub require_root_owned: bool,
    /// Largest plugin answer read, in KiB (default 1024).
    #[serde(default = "d_output")]
    pub max_output_kib: u32,
    pub timeouts: PluginTimeouts,
}

impl Default for StoragePluginSettings {
    fn default() -> Self {
        Self {
            plugin_dir: d_plugin_dir(),
            state_dir: String::new(),
            require_root_owned: true,
            max_output_kib: d_output(),
            timeouts: PluginTimeouts::default(),
        }
    }
}

fn abs_path(key: &str, p: &str) -> Result<(), String> {
    if !p.starts_with('/') {
        return Err(format!("{key} must be an absolute path"));
    }
    if p.split('/').any(|c| c == "..") || p.chars().any(|c| c.is_control()) {
        return Err(format!("{key} must not contain '..' or control characters"));
    }
    Ok(())
}

impl StoragePluginSettings {
    pub fn validate(&self) -> Result<(), String> {
        abs_path("plugin_dir", &self.plugin_dir)?;
        if !self.state_dir.is_empty() {
            abs_path("state_dir", &self.state_dir)?;
        }
        if !(16..=65536).contains(&self.max_output_kib) {
            return Err("max_output_kib must be between 16 and 65536".into());
        }
        for (k, v) in self.timeouts.entries() {
            if !(1..=86400).contains(&v) {
                return Err(format!("timeouts.{k} must be between 1 and 86400 seconds"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_an_empty_section_parses() {
        let s: StoragePluginSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, StoragePluginSettings::default());
        s.validate().unwrap();
        assert_eq!(s.plugin_dir, DEFAULT_PLUGIN_DIR);
        assert!(s.require_root_owned);
        assert_eq!(s.timeouts.create_secs, 600);
        let s: StoragePluginSettings =
            serde_json::from_str(r#"{"timeouts":{"status_secs":5}}"#).unwrap();
        assert_eq!(s.timeouts.status_secs, 5);
        assert_eq!(s.timeouts.activate_secs, 120);
    }

    #[test]
    fn bad_values_are_refused() {
        let mut s = StoragePluginSettings {
            plugin_dir: "relative".into(),
            ..Default::default()
        };
        assert!(s.validate().unwrap_err().contains("plugin_dir"));
        s.plugin_dir = "/a/../b".into();
        assert!(s.validate().is_err());
        s.plugin_dir = DEFAULT_PLUGIN_DIR.into();
        s.timeouts.create_secs = 0;
        assert!(s.validate().unwrap_err().contains("timeouts.create_secs"));
        s.timeouts.create_secs = 60;
        s.max_output_kib = 1;
        assert!(s.validate().unwrap_err().contains("max_output_kib"));
    }
}
