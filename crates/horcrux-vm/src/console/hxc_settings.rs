//! Storage of the node's Horcrux Console settings.
//!
//! Defaults come from `[console]` in /etc/horcrux/config.toml (itself
//! defaulted field by field). Changes made through `PUT
//! /api/console/settings` or a declarative `console:` section are written
//! to `<data_dir>/console.json` and win over the file's section from then
//! on; deleting that file goes back to config.toml.
use horcrux_common::console::ConsoleSettings;
use std::path::PathBuf;
use std::sync::RwLock;

static CURRENT: RwLock<Option<ConsoleSettings>> = RwLock::new(None);

/// Where API/declarative changes are kept.
pub fn store_path() -> PathBuf {
    if let Ok(p) = std::env::var("HORCRUX_CONSOLE_SETTINGS") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    horcrux_core::config::HorcruxConfig::load()
        .paths
        .data_dir
        .join("console.json")
}

fn load_uncached() -> ConsoleSettings {
    let path = store_path();
    if let Ok(bytes) = std::fs::read(&path) {
        match serde_json::from_slice::<ConsoleSettings>(&bytes) {
            Ok(s) if s.validate().is_ok() => return s,
            Ok(_) | Err(_) => tracing::warn!(
                "{} is not valid console settings; using config.toml [console]",
                path.display()
            ),
        }
    }
    horcrux_core::config::HorcruxConfig::load().console
}

/// The node's effective console settings.
pub fn get() -> ConsoleSettings {
    if let Ok(g) = CURRENT.read() {
        if let Some(s) = g.as_ref() {
            return s.clone();
        }
    }
    let s = load_uncached();
    if let Ok(mut g) = CURRENT.write() {
        *g = Some(s.clone());
    }
    s
}

/// Validate, persist (atomically, mode 0600) and activate new settings.
pub fn set(s: ConsoleSettings) -> Result<ConsoleSettings, String> {
    s.validate()?;
    let path = store_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_vec_pretty(&s).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, json).map_err(|e| format!("{}: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    if let Ok(mut g) = CURRENT.write() {
        *g = Some(s.clone());
    }
    Ok(s)
}

/// Forget the cached value (tests, or after the file changed underneath).
pub fn reload() -> ConsoleSettings {
    if let Ok(mut g) = CURRENT.write() {
        *g = None;
    }
    get()
}
