//! Declarative configuration client: desired spec, drift, GitOps, history
//! (mirrors horcrux-api `api::declarative`; checked against the live API by
//! tests/api_contract.rs).
use super::{delete_json, fetch_json, post_json, put_json, ApiError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Desired {
    #[serde(default)]
    pub configured: bool,
    #[serde(default)]
    pub spec: String,
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub updated_by: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub commit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriftItem {
    pub kind: String,
    pub name: String,
    pub field: String,
    pub expected: String,
    pub actual: String,
    pub action: String,
    #[serde(default)]
    pub blocked: bool,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DriftReport {
    #[serde(default)]
    pub configured: bool,
    #[serde(default)]
    pub checked_at: String,
    #[serde(default)]
    pub desired_hash: Option<String>,
    #[serde(default)]
    pub desired_source: Option<String>,
    #[serde(default)]
    pub in_sync: bool,
    #[serde(default)]
    pub count: u32,
    #[serde(default)]
    pub blocked: u32,
    #[serde(default)]
    pub items: Vec<DriftItem>,
    #[serde(default)]
    pub errors: Vec<String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldDiff {
    pub field: String,
    pub expected: String,
    pub actual: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub kind: String,
    pub name: String,
    pub action: String,
    #[serde(default)]
    pub details: Vec<String>,
    #[serde(default)]
    pub fields: Vec<FieldDiff>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    #[serde(default)]
    pub prune: bool,
    #[serde(default)]
    pub changes: Vec<Change>,
    #[serde(default)]
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeResult {
    pub kind: String,
    pub name: String,
    pub action: String,
    #[serde(default)]
    pub details: Vec<String>,
    pub status: String,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ApplyReport {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub plan: Plan,
    #[serde(default)]
    pub results: Vec<ChangeResult>,
    #[serde(default)]
    pub history_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ValidateResult {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub in_sync: bool,
    #[serde(default)]
    pub errors: Vec<String>,
    #[serde(default)]
    pub items: Vec<DriftItem>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PutDesiredResult {
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub in_sync: bool,
    #[serde(default)]
    pub drift_count: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DiffTexts {
    #[serde(default)]
    pub configured: bool,
    #[serde(default)]
    pub same: bool,
    #[serde(default)]
    pub desired: String,
    #[serde(default)]
    pub live: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub drift_interval_secs: u64,
    pub history_limit: u64,
    pub drift_alert: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Signing {
    #[serde(default)]
    pub format: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_signers: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_signers_file: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Credentials {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default)]
    pub secret: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitConfig {
    #[serde(default = "yes")]
    pub enabled: bool,
    pub url: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub interval_secs: u64,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub verify_signatures: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signing: Option<Signing>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<Credentials>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_name: Option<String>,
}

fn yes() -> bool {
    true
}

impl Default for GitConfig {
    fn default() -> Self {
        GitConfig {
            enabled: true,
            url: String::new(),
            branch: "main".into(),
            path: "horcrux.yaml".into(),
            interval_secs: 300,
            mode: "approve".into(),
            verify_signatures: false,
            signing: None,
            credentials: None,
            ssh_key: None,
            webhook_secret: None,
            node_name: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GitConfigDoc {
    #[serde(default)]
    pub configured: bool,
    #[serde(default)]
    pub config: Option<GitConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pending {
    pub hash: String,
    pub commit: String,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub plan: Plan,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GitStatus {
    #[serde(default)]
    pub configured: bool,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub verify_signatures: bool,
    #[serde(default)]
    pub interval_secs: Option<u64>,
    #[serde(default)]
    pub node: Option<String>,
    #[serde(default)]
    pub last_commit: Option<String>,
    #[serde(default)]
    pub last_commit_subject: Option<String>,
    #[serde(default)]
    pub last_commit_author: Option<String>,
    #[serde(default)]
    pub last_commit_time: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub signer: Option<String>,
    #[serde(default)]
    pub last_sync: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub last_result: Option<String>,
    #[serde(default)]
    pub last_applied_commit: Option<String>,
    #[serde(default)]
    pub rejected_commit: Option<String>,
    #[serde(default)]
    pub pending: Vec<Pending>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncOutcome {
    pub commit: String,
    pub result: String,
    #[serde(default)]
    pub in_sync: bool,
    #[serde(default)]
    pub applied: bool,
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub pending: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistorySummary {
    pub id: String,
    pub time: String,
    pub actor: String,
    pub source: String,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub spec_hash: String,
    pub ok: bool,
    #[serde(default)]
    pub changes: u32,
    #[serde(default)]
    pub failed: u32,
    #[serde(default)]
    pub rollback_of: Option<String>,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HistoryList {
    #[serde(default)]
    pub total: u32,
    #[serde(default)]
    pub items: Vec<HistorySummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub time: String,
    pub actor: String,
    pub source: String,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub spec_hash: String,
    #[serde(default)]
    pub spec: String,
    pub ok: bool,
    #[serde(default)]
    pub plan: Plan,
    #[serde(default)]
    pub results: Vec<ChangeResult>,
    #[serde(default)]
    pub rollback_of: Option<String>,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RollbackResult {
    pub rollback_of: String,
    pub restored_from: String,
    #[serde(default)]
    pub dry_run: bool,
    pub report: ApplyReport,
}

#[derive(Debug, Clone, Deserialize)]
struct Export {
    spec: String,
}

const P: &str = "/declarative";

pub async fn get_desired() -> Result<Desired, ApiError> {
    fetch_json(&format!("{P}/desired")).await
}
pub async fn put_desired(spec: &str) -> Result<PutDesiredResult, ApiError> {
    put_json(
        &format!("{P}/desired"),
        &json!({"spec": spec, "source": "ui"}),
    )
    .await
}
pub async fn clear_desired() -> Result<(), ApiError> {
    delete_json(&format!("{P}/desired")).await
}
pub async fn validate(spec: &str) -> Result<ValidateResult, ApiError> {
    post_json(&format!("{P}/desired/validate"), &json!({"spec": spec})).await
}
pub async fn apply_desired(dry_run: bool) -> Result<ApplyReport, ApiError> {
    post_json(
        &format!("{P}/desired/apply"),
        &json!({"dry_run": dry_run, "source": "ui"}),
    )
    .await
}
/// The live state as spec text (to start a desired spec from).
pub async fn export_text() -> Result<String, ApiError> {
    let e: Export = fetch_json(&format!("{P}/export")).await?;
    Ok(e.spec)
}
pub async fn get_drift(refresh: bool) -> Result<DriftReport, ApiError> {
    fetch_json(&format!(
        "{P}/drift{}",
        if refresh { "?refresh=true" } else { "" }
    ))
    .await
}
pub async fn check_drift() -> Result<DriftReport, ApiError> {
    post_json(&format!("{P}/drift/check"), &json!({})).await
}
pub async fn drift_diff() -> Result<DiffTexts, ApiError> {
    fetch_json(&format!("{P}/drift/diff")).await
}
pub async fn get_settings() -> Result<Settings, ApiError> {
    fetch_json(&format!("{P}/settings")).await
}
pub async fn put_settings(s: &Settings) -> Result<Settings, ApiError> {
    put_json(&format!("{P}/settings"), s).await
}
pub async fn git_config() -> Result<GitConfigDoc, ApiError> {
    fetch_json(&format!("{P}/gitops/config")).await
}
pub async fn put_git_config(c: &GitConfig) -> Result<GitConfigDoc, ApiError> {
    put_json(&format!("{P}/gitops/config"), c).await
}
pub async fn delete_git_config() -> Result<(), ApiError> {
    delete_json(&format!("{P}/gitops/config")).await
}
pub async fn git_status() -> Result<GitStatus, ApiError> {
    fetch_json(&format!("{P}/gitops/status")).await
}
pub async fn git_sync() -> Result<SyncOutcome, ApiError> {
    post_json(&format!("{P}/gitops/sync"), &json!({})).await
}
pub async fn git_approve(hash: &str) -> Result<ApplyReport, ApiError> {
    post_json(
        &format!("{P}/gitops/approve/{}", urlencoding::encode(hash)),
        &json!({}),
    )
    .await
}
pub async fn git_reject(hash: &str, reason: &str) -> Result<Value, ApiError> {
    post_json(
        &format!("{P}/gitops/reject/{}", urlencoding::encode(hash)),
        &json!({"reason": reason}),
    )
    .await
}
pub async fn history(limit: u32) -> Result<HistoryList, ApiError> {
    fetch_json(&format!("{P}/history?limit={limit}")).await
}
pub async fn history_entry(id: &str) -> Result<HistoryEntry, ApiError> {
    fetch_json(&format!("{P}/history/{}", urlencoding::encode(id))).await
}
pub async fn rollback(
    id: &str,
    dry_run: bool,
    set_desired: bool,
) -> Result<RollbackResult, ApiError> {
    post_json(
        &format!("{P}/history/{}/rollback", urlencoding::encode(id)),
        &json!({"dry_run": dry_run, "set_desired": set_desired}),
    )
    .await
}
