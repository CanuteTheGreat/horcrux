//! Automatic stop / start actions: node default and per-guest overrides,
//! stored in `config_objects` (kinds `guest_power_node`, `guest_power`).
use super::engine::GuestKind;
use crate::db::config_store::ConfigStore;
use crate::AppState;
use horcrux_common::Result;
pub use horcrux_vm::vm::gpucap::DeviceSaveMode;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

const NODE_KIND: &str = "guest_power_node";
const GUEST_KIND: &str = "guest_power";

/// What happens to a running guest when the host shuts down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopAction {
    /// Save RAM + device state to disk (falls back to Shutdown when it
    /// cannot be saved).
    Save,
    /// Clean guest shutdown (ACPI / init), then power off after the timeout.
    Shutdown,
    /// Power off immediately.
    Poweroff,
    /// Do not touch the guest: it dies with the host (like libvirt's
    /// `on_shutdown = ignore`; the guest is started again at boot only if its
    /// start action says so).
    None,
}

/// Where Save falls back to when the state cannot be saved (passthrough
/// devices, no space, timeout, unencrypted target, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SaveFallback {
    /// Clean guest shutdown (ACPI + guest agent), then power off after the
    /// shutdown timeout.
    #[default]
    Shutdown,
    /// Power off at once.
    Poweroff,
}

/// Where saved memory may be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SaveEncryption {
    /// Only onto encrypted storage (native-encrypted ZFS with its key
    /// loaded, or LUKS); anything else falls back to the save fallback.
    #[default]
    Require,
    /// Administrator accepts plaintext RAM images on the chosen storage.
    AllowUnencrypted,
}

/// What happens to a guest when the host boots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartAction {
    /// Resume saved state, or start if it was running when the host went down.
    ResumeIfRunning,
    /// Resume saved state if there is any, otherwise always start.
    Always,
    /// Leave the guest stopped (saved state is kept).
    Nothing,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NodePolicy {
    /// What happens to running guests when the host goes down or restarts
    /// (`host_shutdown_action` is accepted as an alias).
    #[serde(alias = "host_shutdown_action")]
    pub stop_action: StopAction,
    pub start_action: StartAction,
    /// Seconds after the boot pass began before a guest is started.
    pub start_delay_secs: u32,
    /// Where saved states go (a directory on the chosen storage); empty =
    /// `<data dir>/saved-state/data`.
    pub save_dir: Option<String>,
    /// Per-guest limit for saving at host shutdown.
    pub save_timeout_secs: u64,
    /// Per-guest limit for a clean shutdown (then power off).
    pub shutdown_timeout_secs: u64,
    /// Guests handled at the same time at host shutdown.
    pub max_parallel: usize,
    /// In a cluster, host shutdown first moves running VMs to other nodes
    /// (live migration); only what cannot move is handled by the stop action.
    pub drain_on_shutdown: bool,
    /// How long the shutdown drain may take per move before it gives up on
    /// that guest (which is then saved locally).
    pub drain_timeout_secs: u64,
    /// What a guest gets when its state cannot be saved.
    pub save_fallback: SaveFallback,
    /// Saved memory may hold secrets: require encrypted storage.
    pub save_encryption: SaveEncryption,
    /// Seconds between two resumes at boot, on top of each guest's own start
    /// delay (0 = none).
    pub resume_stagger_secs: u32,
    /// Extra seconds added to the computed worst case before the init system
    /// may give up on the stop pass.
    pub stop_margin_secs: u64,
    /// A VM that owns hardware QEMU cannot save (passthrough GPU) hibernates
    /// itself through the Horcrux guest agent when the host shuts down
    /// (guest tools installed and hibernation enabled in the guest), instead
    /// of being shut down. Off = such VMs follow the save fallback.
    pub guest_hibernate: bool,
    /// Per-guest limit for the guest to write its hibernation image and power
    /// the VM off (then the save fallback applies).
    pub hibernate_timeout_secs: u64,
    /// Limit for one question to the guest agent (readiness check, request).
    pub hibernate_probe_secs: u64,
    /// Where the Horcrux guest agent lives in a Linux guest.
    pub guest_agent_path_linux: String,
    /// Where the Horcrux guest agent lives in a Windows guest.
    pub guest_agent_path_windows: String,
    /// Whether a VM that owns host devices (SR-IOV functions, vGPU) is
    /// saved when the kernel says their drivers can migrate state.
    pub device_save: DeviceSaveMode,
}

impl Default for NodePolicy {
    fn default() -> Self {
        Self {
            stop_action: StopAction::Save,
            start_action: StartAction::ResumeIfRunning,
            start_delay_secs: 0,
            save_dir: None,
            save_timeout_secs: 300,
            shutdown_timeout_secs: 120,
            max_parallel: 4,
            drain_on_shutdown: true,
            drain_timeout_secs: 900,
            save_fallback: SaveFallback::Shutdown,
            save_encryption: SaveEncryption::Require,
            resume_stagger_secs: 0,
            stop_margin_secs: 60,
            guest_hibernate: true,
            hibernate_timeout_secs: 300,
            hibernate_probe_secs: 15,
            guest_agent_path_linux: "/usr/bin/horcrux-guest-agent".into(),
            guest_agent_path_windows:
                r"C:\Program Files\Horcrux\Guest Tools\horcrux-guest-agent.exe".into(),
            device_save: DeviceSaveMode::Auto,
        }
    }
}

impl NodePolicy {
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.max_parallel == 0 || self.max_parallel > 64 {
            return Err("max_parallel must be 1-64".into());
        }
        if !(5..=86400).contains(&self.save_timeout_secs) {
            return Err("save_timeout_secs must be 5-86400".into());
        }
        if !(5..=86400).contains(&self.shutdown_timeout_secs) {
            return Err("shutdown_timeout_secs must be 5-86400".into());
        }
        if self.start_delay_secs > 86400 {
            return Err("start_delay_secs must be at most 86400".into());
        }
        if !(30..=86400).contains(&self.drain_timeout_secs) {
            return Err("drain_timeout_secs must be 30-86400".into());
        }
        if self.resume_stagger_secs > 3600 {
            return Err("resume_stagger_secs must be at most 3600".into());
        }
        if self.stop_margin_secs > 86400 {
            return Err("stop_margin_secs must be at most 86400".into());
        }
        if !(5..=86400).contains(&self.hibernate_timeout_secs) {
            return Err("hibernate_timeout_secs must be 5-86400".into());
        }
        if !(2..=300).contains(&self.hibernate_probe_secs) {
            return Err("hibernate_probe_secs must be 2-300".into());
        }
        for (k, v) in [
            ("guest_agent_path_linux", &self.guest_agent_path_linux),
            ("guest_agent_path_windows", &self.guest_agent_path_windows),
        ] {
            if v.trim().is_empty() || v.contains('\0') || v.contains('\n') {
                return Err(format!("{k} must be a path"));
            }
        }
        check_dir(self.save_dir.as_deref())
    }

    /// Worst-case seconds the host stop pass can take for `guests` running
    /// guests: waves of `max_parallel`, each bounded by the save timeout plus
    /// the fallback shutdown timeout (plus the drain when the node drains
    /// first), and a margin. The stop client's deadline and the UI's
    /// init-system warning both come from this number.
    pub fn stop_budget_secs(&self, guests: usize) -> u64 {
        let par = self.max_parallel.max(1);
        let waves = guests.div_ceil(par).max(1) as u64;
        // a guest is saved OR hibernates by itself (never both), then the
        // fallback shutdown if that failed
        let first = if self.guest_hibernate {
            self.save_timeout_secs.max(self.hibernate_timeout_secs)
        } else {
            self.save_timeout_secs
        };
        let per_wave = first
            + self.shutdown_timeout_secs
            + if self.drain_on_shutdown {
                self.drain_timeout_secs
            } else {
                0
            };
        waves * per_wave + self.stop_margin_secs
    }
}

fn check_dir(d: Option<&str>) -> std::result::Result<(), String> {
    match d {
        Some(d) if !d.trim().is_empty() => {
            if !d.starts_with('/') {
                return Err("the saved-state directory must be an absolute path".into());
            }
            if d.contains(['\'', '\n', '\0']) || d.split('/').any(|p| p == "..") {
                return Err("the saved-state directory has invalid characters".into());
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Per-guest settings; `None` = inherit the node default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GuestOverride {
    #[serde(alias = "host_shutdown_action")]
    pub stop_action: Option<StopAction>,
    pub start_action: Option<StartAction>,
    pub start_delay_secs: Option<u32>,
    /// Lower starts first (default 1000).
    pub start_order: Option<i32>,
    pub save_dir: Option<String>,
    /// Hibernate through the guest agent when state cannot be saved
    /// (passthrough hardware); `None` = the node default.
    pub guest_hibernate: Option<bool>,
}

impl GuestOverride {
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.start_delay_secs.is_some_and(|d| d > 86400) {
            return Err("start_delay_secs must be at most 86400".into());
        }
        check_dir(self.save_dir.as_deref())
    }
}

/// The settings that apply to one guest.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Effective {
    pub stop_action: StopAction,
    pub start_action: StartAction,
    pub start_delay_secs: u32,
    pub start_order: i32,
    pub save_dir: PathBuf,
    pub save_timeout_secs: u64,
    pub shutdown_timeout_secs: u64,
    pub save_fallback: SaveFallback,
    pub save_encryption: SaveEncryption,
    pub guest_hibernate: bool,
    pub hibernate_timeout_secs: u64,
    pub hibernate_probe_secs: u64,
    pub guest_agent_path_linux: String,
    pub guest_agent_path_windows: String,
    pub device_save: DeviceSaveMode,
}

/// Combine node default and override (pure, unit-tested).
pub fn combine(node: &NodePolicy, o: &GuestOverride, default_dir: PathBuf) -> Effective {
    let dir = |d: &Option<String>| {
        d.as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
    };
    Effective {
        stop_action: o.stop_action.unwrap_or(node.stop_action),
        start_action: o.start_action.unwrap_or(node.start_action),
        start_delay_secs: o.start_delay_secs.unwrap_or(node.start_delay_secs),
        start_order: o.start_order.unwrap_or(1000),
        save_dir: dir(&o.save_dir)
            .or_else(|| dir(&node.save_dir))
            .unwrap_or(default_dir),
        save_timeout_secs: node.save_timeout_secs,
        shutdown_timeout_secs: node.shutdown_timeout_secs,
        save_fallback: node.save_fallback,
        save_encryption: node.save_encryption,
        guest_hibernate: o.guest_hibernate.unwrap_or(node.guest_hibernate),
        hibernate_timeout_secs: node.hibernate_timeout_secs,
        hibernate_probe_secs: node.hibernate_probe_secs,
        guest_agent_path_linux: node.guest_agent_path_linux.clone(),
        guest_agent_path_windows: node.guest_agent_path_windows.clone(),
        device_save: node.device_save,
    }
}

pub fn default_save_dir(state: &AppState) -> PathBuf {
    state.config.paths.data_dir.join("saved-state").join("data")
}

fn key(k: GuestKind, id: &str) -> String {
    format!("{}:{}", k.prefix(), id)
}

pub async fn load_node(state: &Arc<AppState>) -> NodePolicy {
    ConfigStore::new(&state.database)
        .load::<NodePolicy>(NODE_KIND)
        .await
        .ok()
        .and_then(|v| v.into_iter().next())
        .map(|(_, p)| p)
        .unwrap_or_default()
}

pub async fn save_node(state: &Arc<AppState>, p: &NodePolicy) -> Result<()> {
    ConfigStore::new(&state.database)
        .put(NODE_KIND, "default", p)
        .await
}

pub async fn load_override(state: &Arc<AppState>, k: GuestKind, id: &str) -> GuestOverride {
    let want = key(k, id);
    ConfigStore::new(&state.database)
        .load::<GuestOverride>(GUEST_KIND)
        .await
        .ok()
        .and_then(|v| v.into_iter().find(|(i, _)| *i == want))
        .map(|(_, o)| o)
        .unwrap_or_default()
}

pub async fn save_override(
    state: &Arc<AppState>,
    k: GuestKind,
    id: &str,
    o: &GuestOverride,
) -> Result<()> {
    let store = ConfigStore::new(&state.database);
    if *o == GuestOverride::default() {
        store.delete(GUEST_KIND, &key(k, id)).await.map(|_| ())
    } else {
        store.put(GUEST_KIND, &key(k, id), o).await
    }
}

pub async fn effective(state: &Arc<AppState>, k: GuestKind, id: &str) -> Effective {
    combine(
        &load_node(state).await,
        &load_override(state, k, id).await,
        default_save_dir(state),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_hyper_v() {
        let n = NodePolicy::default();
        assert_eq!(n.stop_action, StopAction::Save);
        assert_eq!(n.start_action, StartAction::ResumeIfRunning);
        n.validate().unwrap();
    }

    #[test]
    fn override_beats_node_default() {
        let node = NodePolicy {
            stop_action: StopAction::Shutdown,
            start_delay_secs: 5,
            save_dir: Some("/mnt/fast".into()),
            ..Default::default()
        };
        let none = combine(&node, &GuestOverride::default(), "/d".into());
        assert_eq!(none.stop_action, StopAction::Shutdown);
        assert_eq!(none.start_delay_secs, 5);
        assert_eq!(none.save_dir, PathBuf::from("/mnt/fast"));
        assert_eq!(none.start_order, 1000);
        let o = GuestOverride {
            stop_action: Some(StopAction::Poweroff),
            start_action: Some(StartAction::Nothing),
            start_delay_secs: Some(30),
            start_order: Some(1),
            save_dir: Some("/mnt/slow".into()),
            guest_hibernate: Some(false),
        };
        let e = combine(&node, &o, "/d".into());
        assert_eq!(e.stop_action, StopAction::Poweroff);
        assert_eq!(e.start_action, StartAction::Nothing);
        assert_eq!((e.start_delay_secs, e.start_order), (30, 1));
        assert_eq!(e.save_dir, PathBuf::from("/mnt/slow"));
        assert!(!e.guest_hibernate && none.guest_hibernate);
        let dflt = combine(
            &NodePolicy::default(),
            &GuestOverride::default(),
            "/d".into(),
        );
        assert_eq!(dflt.save_dir, PathBuf::from("/d"));
    }

    #[test]
    fn validation_rejects_bad_values() {
        let n = NodePolicy {
            max_parallel: 0,
            ..Default::default()
        };
        assert!(n.validate().is_err());
        let n = NodePolicy {
            save_dir: Some("relative/dir".into()),
            ..Default::default()
        };
        assert!(n.validate().is_err());
        let o = GuestOverride {
            save_dir: Some("/a/../b".into()),
            ..Default::default()
        };
        assert!(o.validate().is_err());
    }

    #[test]
    fn host_shutdown_action_is_an_alias_and_none_exists() {
        let n: NodePolicy = serde_json::from_str(r#"{"host_shutdown_action":"none"}"#).unwrap();
        assert_eq!(n.stop_action, StopAction::None);
        let o: GuestOverride =
            serde_json::from_str(r#"{"host_shutdown_action":"shutdown"}"#).unwrap();
        assert_eq!(o.stop_action, Some(StopAction::Shutdown));
        assert_eq!(
            serde_json::to_string(&StopAction::None).unwrap(),
            "\"none\""
        );
    }

    #[test]
    fn defaults_are_safe() {
        let n = NodePolicy::default();
        assert_eq!(n.save_encryption, SaveEncryption::Require);
        assert_eq!(n.save_fallback, SaveFallback::Shutdown);
        let e = combine(&n, &GuestOverride::default(), "/d".into());
        assert_eq!(e.save_encryption, SaveEncryption::Require);
    }

    #[test]
    fn stop_budget_grows_with_waves_and_timeouts() {
        let n = NodePolicy {
            max_parallel: 2,
            save_timeout_secs: 100,
            shutdown_timeout_secs: 50,
            drain_on_shutdown: false,
            stop_margin_secs: 10,
            guest_hibernate: false,
            ..Default::default()
        };
        assert_eq!(n.stop_budget_secs(0), 160);
        assert_eq!(n.stop_budget_secs(2), 160);
        assert_eq!(n.stop_budget_secs(3), 310);
        let d = NodePolicy {
            drain_on_shutdown: true,
            drain_timeout_secs: 30,
            ..n
        };
        assert_eq!(d.stop_budget_secs(2), 190);
    }

    #[test]
    fn hibernate_defaults_on_and_is_bounded_by_the_budget() {
        let n = NodePolicy::default();
        assert!(n.guest_hibernate);
        n.validate().unwrap();
        // hibernate replaces the save (the longer of the two counts), the
        // fallback shutdown still follows
        let b = NodePolicy {
            max_parallel: 1,
            save_timeout_secs: 100,
            shutdown_timeout_secs: 50,
            hibernate_timeout_secs: 400,
            drain_on_shutdown: false,
            stop_margin_secs: 0,
            ..Default::default()
        };
        assert_eq!(b.stop_budget_secs(1), 450);
        let off = NodePolicy {
            guest_hibernate: false,
            ..b
        };
        assert_eq!(off.stop_budget_secs(1), 150);
        let bad = NodePolicy {
            hibernate_timeout_secs: 1,
            ..Default::default()
        };
        assert!(bad.validate().is_err());
        let bad = NodePolicy {
            guest_agent_path_linux: " ".into(),
            ..Default::default()
        };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn serde_names_are_stable() {
        assert_eq!(
            serde_json::to_string(&StopAction::Poweroff).unwrap(),
            "\"poweroff\""
        );
        assert_eq!(
            serde_json::to_string(&StartAction::ResumeIfRunning).unwrap(),
            "\"resume_if_running\""
        );
        let o: GuestOverride = serde_json::from_str("{}").unwrap();
        assert_eq!(o, GuestOverride::default());
    }
}
