//! Automatic stop / start actions: node default and per-guest overrides,
//! stored in `config_objects` (kinds `guest_power_node`, `guest_power`).
use super::engine::GuestKind;
use crate::db::config_store::ConfigStore;
use crate::AppState;
use horcrux_common::Result;
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
        check_dir(self.save_dir.as_deref())
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
    pub stop_action: Option<StopAction>,
    pub start_action: Option<StartAction>,
    pub start_delay_secs: Option<u32>,
    /// Lower starts first (default 1000).
    pub start_order: Option<i32>,
    pub save_dir: Option<String>,
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
        };
        let e = combine(&node, &o, "/d".into());
        assert_eq!(e.stop_action, StopAction::Poweroff);
        assert_eq!(e.start_action, StartAction::Nothing);
        assert_eq!((e.start_delay_secs, e.start_order), (30, 1));
        assert_eq!(e.save_dir, PathBuf::from("/mnt/slow"));
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
