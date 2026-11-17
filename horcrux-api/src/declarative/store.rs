//! Persistence of the declarative engine's own state: the stored desired
//! spec, the apply history, GitOps configuration and pending approvals.
//! Everything lives in the node's config store (SQLite `config_objects`),
//! so it survives restarts and is included in config backups.
use super::exec::ChangeResult;
use super::Plan;
use crate::db::config_store::ConfigStore;
use crate::AppState;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

const K_DESIRED: &str = "declarative_desired";
const K_SETTINGS: &str = "declarative_settings";
const K_HISTORY: &str = "declarative_history";
const K_GITCFG: &str = "declarative_gitops_cfg";
const K_GITSTATE: &str = "declarative_gitops_state";
const K_PENDING: &str = "declarative_gitops_pending";
const ONE: &str = "node";

fn store(state: &Arc<AppState>) -> ConfigStore {
    ConfigStore::new(&state.database)
}

pub fn sha256_hex(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

// --------------------------------------------------------------- settings

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    /// Seconds between drift checks (5..=86400).
    #[serde(default = "d_interval")]
    pub drift_interval_secs: u64,
    /// Apply records kept (10..=5000); the oldest are dropped.
    #[serde(default = "d_history")]
    pub history_limit: usize,
    /// Feed the drift count to the alert system (rule "Configuration drift").
    #[serde(default = "d_true")]
    pub drift_alert: bool,
}

fn d_interval() -> u64 {
    300
}
fn d_history() -> usize {
    200
}
fn d_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            drift_interval_secs: d_interval(),
            history_limit: d_history(),
            drift_alert: true,
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<(), String> {
        if !(5..=86_400).contains(&self.drift_interval_secs) {
            return Err("drift_interval_secs must be 5-86400".into());
        }
        if !(10..=5000).contains(&self.history_limit) {
            return Err("history_limit must be 10-5000".into());
        }
        Ok(())
    }
}

pub async fn settings(state: &Arc<AppState>) -> Settings {
    store(state)
        .load::<Settings>(K_SETTINGS)
        .await
        .ok()
        .and_then(|v| v.into_iter().next())
        .map(|(_, s)| s)
        .unwrap_or_default()
}

pub async fn put_settings(state: &Arc<AppState>, s: &Settings) -> Result<(), String> {
    s.validate()?;
    store(state)
        .put(K_SETTINGS, ONE, s)
        .await
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- desired

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Desired {
    pub spec: String,
    pub hash: String,
    pub updated_at: String,
    pub updated_by: String,
    /// api | cli | ui | gitops
    pub source: String,
    /// Commit the spec came from (GitOps).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

pub async fn desired(state: &Arc<AppState>) -> Option<Desired> {
    store(state)
        .load::<Desired>(K_DESIRED)
        .await
        .ok()
        .and_then(|v| v.into_iter().next())
        .map(|(_, d)| d)
}

pub async fn put_desired(
    state: &Arc<AppState>,
    spec: &str,
    by: &str,
    source: &str,
    commit: Option<String>,
) -> Result<Desired, String> {
    let d = Desired {
        spec: spec.to_string(),
        hash: sha256_hex(spec),
        updated_at: now(),
        updated_by: by.to_string(),
        source: source.to_string(),
        commit,
    };
    store(state)
        .put(K_DESIRED, ONE, &d)
        .await
        .map_err(|e| e.to_string())?;
    Ok(d)
}

pub async fn clear_desired(state: &Arc<AppState>) -> Result<bool, String> {
    store(state)
        .delete(K_DESIRED, ONE)
        .await
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- history

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub time: String,
    pub ts_ms: i64,
    pub actor: String,
    /// api | cli | ui | gitops | rollback
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    pub spec_hash: String,
    pub spec: String,
    pub prune: bool,
    pub ok: bool,
    pub plan: Plan,
    pub results: Vec<ChangeResult>,
    /// The entry this one rolled back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_of: Option<String>,
    pub summary: String,
}

/// List item without the bulky parts.
#[derive(Debug, Clone, Serialize)]
pub struct HistorySummary {
    pub id: String,
    pub time: String,
    pub actor: String,
    pub source: String,
    pub commit: Option<String>,
    pub spec_hash: String,
    pub ok: bool,
    pub changes: usize,
    pub failed: usize,
    pub rollback_of: Option<String>,
    pub summary: String,
}

impl HistoryEntry {
    pub fn summarize(&self) -> HistorySummary {
        HistorySummary {
            id: self.id.clone(),
            time: self.time.clone(),
            actor: self.actor.clone(),
            source: self.source.clone(),
            commit: self.commit.clone(),
            spec_hash: self.spec_hash.clone(),
            ok: self.ok,
            changes: self.plan.changes.len(),
            failed: self
                .results
                .iter()
                .filter(|r| r.status != "applied")
                .count(),
            rollback_of: self.rollback_of.clone(),
            summary: self.summary.clone(),
        }
    }
}

pub fn new_history_id() -> (String, i64) {
    let ms = chrono::Utc::now().timestamp_millis();
    let r = uuid::Uuid::new_v4().simple().to_string();
    (format!("{ms:013}-{}", &r[..4]), ms)
}

/// Newest first.
pub async fn history(state: &Arc<AppState>) -> Vec<HistoryEntry> {
    let mut v: Vec<HistoryEntry> = store(state)
        .load::<HistoryEntry>(K_HISTORY)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    v.sort_by(|a, b| b.id.cmp(&a.id));
    v
}

pub async fn history_entry(state: &Arc<AppState>, id: &str) -> Option<HistoryEntry> {
    history(state).await.into_iter().find(|e| e.id == id)
}

/// Add an entry and drop the oldest beyond the retention cap.
pub async fn record_history(state: &Arc<AppState>, e: &HistoryEntry) -> Result<(), String> {
    let st = store(state);
    st.put(K_HISTORY, &e.id, e)
        .await
        .map_err(|x| x.to_string())?;
    let cap = settings(state).await.history_limit;
    let all = history(state).await;
    for old in all.iter().skip(cap) {
        let _ = st.delete(K_HISTORY, &old.id).await;
    }
    Ok(())
}

// ----------------------------------------------------------------- gitops

pub async fn gitops_config(state: &Arc<AppState>) -> Option<super::gitops::GitConfig> {
    store(state)
        .load(K_GITCFG)
        .await
        .ok()
        .and_then(|v| v.into_iter().next())
        .map(|(_, c)| c)
}

pub async fn put_gitops_config(
    state: &Arc<AppState>,
    c: &super::gitops::GitConfig,
) -> Result<(), String> {
    store(state)
        .put(K_GITCFG, ONE, c)
        .await
        .map_err(|e| e.to_string())
}

pub async fn delete_gitops_config(state: &Arc<AppState>) -> Result<bool, String> {
    store(state)
        .delete(K_GITCFG, ONE)
        .await
        .map_err(|e| e.to_string())
}

pub async fn gitops_state(state: &Arc<AppState>) -> super::gitops::GitState {
    store(state)
        .load(K_GITSTATE)
        .await
        .ok()
        .and_then(|v| v.into_iter().next())
        .map(|(_, c)| c)
        .unwrap_or_default()
}

pub async fn put_gitops_state(
    state: &Arc<AppState>,
    s: &super::gitops::GitState,
) -> Result<(), String> {
    store(state)
        .put(K_GITSTATE, ONE, s)
        .await
        .map_err(|e| e.to_string())
}

pub async fn pending(state: &Arc<AppState>) -> Vec<super::gitops::Pending> {
    let mut v: Vec<super::gitops::Pending> = store(state)
        .load(K_PENDING)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(_, p)| p)
        .collect();
    v.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    v
}

pub async fn put_pending(state: &Arc<AppState>, p: &super::gitops::Pending) -> Result<(), String> {
    store(state)
        .put(K_PENDING, &p.hash, p)
        .await
        .map_err(|e| e.to_string())
}

pub async fn delete_pending(state: &Arc<AppState>, hash: &str) {
    let _ = store(state).delete(K_PENDING, hash).await;
}
