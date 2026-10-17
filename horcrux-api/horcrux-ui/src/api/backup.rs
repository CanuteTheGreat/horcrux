//! Backup & restore API client (mirrors horcrux-api `backup` types; checked
//! against the live API by tests/api_contract.rs).
use super::{delete_json, fetch_json, post_json, put_json, ApiError};
use serde::{Deserialize, Serialize};

/// A backup in the catalog (`GET /api/backups`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Backup {
    pub id: String,
    /// "vm" or "container".
    pub target_type: String,
    pub target_id: String,
    pub target_name: String,
    /// Unix time the backup was taken.
    pub timestamp: i64,
    /// Archive size in bytes.
    pub size: u64,
    pub mode: String,
    pub compression: String,
    pub path: String,
    #[serde(default)]
    pub storage: String,
    #[serde(default)]
    pub config_included: bool,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub protected: bool,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub job_id: Option<String>,
    #[serde(default)]
    pub verify_state: Option<String>,
    #[serde(default)]
    pub verified_at: Option<i64>,
    #[serde(default)]
    pub verify_error: Option<String>,
}

/// `POST /api/backups`
#[derive(Debug, Clone, Default, Serialize)]
pub struct CreateBackupRequest {
    pub target_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_type: Option<String>,
    pub storage: String,
    pub mode: String,
    pub compression: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    pub protected: bool,
}

/// `PUT /api/backups/:id`
#[derive(Debug, Clone, Default, Serialize)]
pub struct UpdateBackupRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protected: Option<bool>,
}

/// `POST /api/backups/:id/restore`
#[derive(Debug, Clone, Default, Serialize)]
pub struct RestoreRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub force: bool,
    pub start: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreResult {
    pub backup_id: String,
    pub target_type: String,
    pub target_id: String,
    pub name: String,
    pub replaced: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyResult {
    pub backup_id: String,
    pub ok: bool,
    pub checked_at: i64,
    pub files_checked: usize,
    pub error: Option<String>,
}

/// keep-* retention options (Proxmox prune semantics).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Retention {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_last: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_hourly: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_daily: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_weekly: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_monthly: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_yearly: Option<u32>,
}

impl Retention {
    /// "last 3, daily 7" style summary ("keep all" when empty).
    pub fn summary(&self) -> String {
        let parts: Vec<String> = [
            ("last", self.keep_last),
            ("hourly", self.keep_hourly),
            ("daily", self.keep_daily),
            ("weekly", self.keep_weekly),
            ("monthly", self.keep_monthly),
            ("yearly", self.keep_yearly),
        ]
        .iter()
        .filter_map(|(n, v)| v.filter(|v| *v > 0).map(|v| format!("{n} {v}")))
        .collect();
        if parts.is_empty() {
            "keep all".to_string()
        } else {
            parts.join(", ")
        }
    }

    /// Parse the six keep-* form fields (blank = unset).
    pub fn from_fields(fields: [&str; 6]) -> Result<Self, String> {
        let p = |s: &str, name: &str| -> Result<Option<u32>, String> {
            let s = s.trim();
            if s.is_empty() {
                return Ok(None);
            }
            s.parse::<u32>()
                .map(Some)
                .map_err(|_| format!("{name} must be a whole number"))
        };
        Ok(Self {
            keep_last: p(fields[0], "Keep last")?,
            keep_hourly: p(fields[1], "Keep hourly")?,
            keep_daily: p(fields[2], "Keep daily")?,
            keep_weekly: p(fields[3], "Keep weekly")?,
            keep_monthly: p(fields[4], "Keep monthly")?,
            keep_yearly: p(fields[5], "Keep yearly")?,
        })
    }
}

/// `POST /api/backups/prune`
#[derive(Debug, Clone, Serialize)]
pub struct PruneRequest {
    pub target_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage: Option<String>,
    #[serde(flatten)]
    pub policy: Retention,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PruneDecision {
    pub backup_id: String,
    pub timestamp: i64,
    /// "keep", "remove" or "protected".
    pub mark: String,
}

/// `GET /api/backup-storages`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupStorage {
    pub id: String,
    pub path: String,
    pub kind: String,
    pub available_gb: Option<u64>,
    pub total_gb: Option<u64>,
}

/// Scheduled backup job (`/api/backup-jobs`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BackupJob {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    pub schedule: String,
    pub targets: Vec<String>,
    #[serde(default)]
    pub storage: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub compression: String,
    #[serde(default)]
    pub retention: Retention,
    #[serde(default)]
    pub notify: Option<String>,
    #[serde(default, skip_serializing)]
    pub last_run: Option<i64>,
    #[serde(default, skip_serializing)]
    pub last_status: Option<String>,
    #[serde(default, skip_serializing)]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing)]
    pub next_run: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunStarted {
    pub job_id: String,
    pub started: bool,
}

pub async fn list_backups() -> Result<Vec<Backup>, ApiError> {
    fetch_json("/backups").await
}

pub async fn create_backup(req: &CreateBackupRequest) -> Result<Backup, ApiError> {
    post_json("/backups", req).await
}

pub async fn update_backup(id: &str, req: &UpdateBackupRequest) -> Result<Backup, ApiError> {
    put_json(&format!("/backups/{}", id), req).await
}

pub async fn delete_backup(id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/backups/{}", id)).await
}

pub async fn restore_backup(id: &str, req: &RestoreRequest) -> Result<RestoreResult, ApiError> {
    post_json(&format!("/backups/{}/restore", id), req).await
}

pub async fn verify_backup(id: &str) -> Result<VerifyResult, ApiError> {
    post_json(&format!("/backups/{}/verify", id), &serde_json::json!({})).await
}

pub async fn prune_backups(req: &PruneRequest) -> Result<Vec<PruneDecision>, ApiError> {
    post_json("/backups/prune", req).await
}

pub async fn list_backup_storages() -> Result<Vec<BackupStorage>, ApiError> {
    fetch_json("/backup-storages").await
}

pub async fn list_backup_jobs() -> Result<Vec<BackupJob>, ApiError> {
    fetch_json("/backup-jobs").await
}

pub async fn create_backup_job(job: &BackupJob) -> Result<BackupJob, ApiError> {
    post_json("/backup-jobs", job).await
}

pub async fn update_backup_job(id: &str, job: &BackupJob) -> Result<BackupJob, ApiError> {
    put_json(&format!("/backup-jobs/{}", id), job).await
}

pub async fn delete_backup_job(id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/backup-jobs/{}", id)).await
}

pub async fn run_backup_job(id: &str) -> Result<RunStarted, ApiError> {
    post_json(&format!("/backup-jobs/{}/run", id), &serde_json::json!({})).await
}

/// Human-readable byte count.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{} B", bytes)
    } else {
        format!("{:.1} {}", v, UNITS[u])
    }
}

/// Local date/time of a unix timestamp.
pub fn format_time(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| ts.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_fields_and_summary() {
        let r = Retention::from_fields(["3", "", "7", " ", "", ""]).unwrap();
        assert_eq!(r.keep_last, Some(3));
        assert_eq!(r.keep_daily, Some(7));
        assert_eq!(r.summary(), "last 3, daily 7");
        assert_eq!(Retention::default().summary(), "keep all");
        assert!(Retention::from_fields(["x", "", "", "", "", ""]).is_err());
    }

    #[test]
    fn prune_request_flattens_policy() {
        let v = serde_json::to_value(PruneRequest {
            target_id: "vm1".into(),
            storage: None,
            policy: Retention {
                keep_last: Some(2),
                ..Default::default()
            },
            dry_run: true,
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({"target_id": "vm1", "keep_last": 2, "dry_run": true})
        );
    }

    #[test]
    fn bytes() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }
}
