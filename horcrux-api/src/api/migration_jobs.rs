//! HA > Migration Center: list, start and cancel migration jobs.
//!
//! The page calls `/api/migration/jobs` (GET/POST/DELETE) and
//! `/api/migration/bulk`; only `/api/migrate/:vm_id` used to exist, so the
//! page answered "Failed to load migration jobs: HTTP 404". These routes sit
//! on the same `MigrationManager` the per-VM route uses (the peer-to-peer
//! QEMU migration in `vm_migration.rs`) and translate its jobs into the
//! shape the page renders.
//! Permissions come from the `migration` area in `auth::privileges`
//! (VM.Audit on /vms to list, VM.Migrate on /vms to start or cancel).

use crate::error::ApiError;
use crate::migration::{self, MigrationJob, MigrationState, MigrationType};
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/migration/jobs", get(list_jobs).post(create_job))
        .route("/api/migration/jobs/:job_id", delete(cancel_job))
        .route("/api/migration/bulk", post(create_bulk))
}

/// A migration job as the Migration Center page reads it
/// (`horcrux-ui` `api::MigrationJob`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MigrationJobView {
    pub job_id: String,
    pub resource_type: String,
    pub resource_id: String,
    pub source_node: String,
    pub target_node: String,
    pub migration_type: String,
    pub status: String,
    pub progress: u32,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
    pub error: Option<String>,
    pub bandwidth_limit: Option<u64>,
    pub timeout: u32,
    pub force: bool,
    pub with_local_disks: bool,
    pub estimated_duration: Option<u32>,
    pub transferred_bytes: u64,
    pub remaining_bytes: Option<u64>,
}

/// Page status for a manager state: pending, running, completed, failed,
/// cancelled (the page shows Cancel for pending/running).
pub fn status_name(state: &MigrationState) -> &'static str {
    match state {
        MigrationState::Pending => "pending",
        MigrationState::Preparing
        | MigrationState::Transferring
        | MigrationState::Syncing
        | MigrationState::Finalizing => "running",
        MigrationState::Completed => "completed",
        MigrationState::Failed => "failed",
        MigrationState::Cancelled => "cancelled",
    }
}

fn type_name(t: &MigrationType) -> &'static str {
    match t {
        MigrationType::Live => "live",
        MigrationType::Online => "online",
        MigrationType::Offline => "offline",
    }
}

pub fn view(job: &MigrationJob) -> MigrationJobView {
    MigrationJobView {
        job_id: job.id.clone(),
        resource_type: "vm".into(),
        resource_id: job.vm_id.clone(),
        source_node: job.source_node.clone(),
        target_node: job.target_node.clone(),
        migration_type: type_name(&job.migration_type).into(),
        status: status_name(&job.state).into(),
        progress: job.progress.clamp(0.0, 100.0).round() as u32,
        start_time: job.started,
        end_time: job.completed,
        error: job.error.clone(),
        bandwidth_limit: job.bandwidth_limit,
        timeout: job.timeout_secs.unwrap_or(0).min(u32::MAX as u64) as u32,
        force: false,
        with_local_disks: false,
        estimated_duration: None,
        transferred_bytes: job.transferred_bytes,
        remaining_bytes: remaining_bytes(job),
    }
}

/// Bytes still to move: none once the job completed (the last progress
/// sample is taken before the end), unknown before a total is known.
fn remaining_bytes(job: &MigrationJob) -> Option<u64> {
    if job.state == MigrationState::Completed {
        return Some(0);
    }
    (job.total_bytes > 0).then(|| job.total_bytes.saturating_sub(job.transferred_bytes))
}

async fn list_jobs(State(state): State<Arc<AppState>>) -> Json<Vec<MigrationJobView>> {
    let mut jobs = state.migration_manager.list_jobs().await;
    jobs.sort_by_key(|j| std::cmp::Reverse(j.started));
    Json(jobs.iter().map(view).collect())
}

/// What the page's "Migrate" dialog sends (the full UI job; only these
/// fields matter, the rest is filled in by the server).
#[derive(Debug, Deserialize)]
pub struct CreateJobRequest {
    #[serde(default = "default_resource_type")]
    pub resource_type: String,
    pub resource_id: String,
    pub target_node: String,
    #[serde(default)]
    pub migration_type: Option<String>,
    #[serde(default)]
    pub bandwidth_limit: Option<u64>,
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub with_local_disks: bool,
    /// Seconds after which the job fails and is rolled back (0: no limit).
    #[serde(default)]
    pub timeout: Option<u64>,
}

fn default_resource_type() -> String {
    "vm".into()
}

pub fn parse_type(s: Option<&str>) -> Result<MigrationType, ApiError> {
    match s.unwrap_or("online") {
        "live" => Ok(MigrationType::Live),
        "online" => Ok(MigrationType::Online),
        "offline" => Ok(MigrationType::Offline),
        other => Err(ApiError::BadRequest(format!(
            "Invalid migration type: {} (live, online or offline)",
            other
        ))),
    }
}

async fn start_one(
    state: &Arc<AppState>,
    resource_type: &str,
    config: migration::MigrationConfig,
) -> Result<MigrationJob, ApiError> {
    let resource_id = config.vm_id.clone();
    let resource_id = resource_id.as_str();
    let target_node = config.target_node.clone();
    let target_node = target_node.as_str();
    if resource_type != "vm" {
        return Err(ApiError::BadRequest(format!(
            "Only VMs can be migrated (got resource type '{}')",
            resource_type
        )));
    }
    if target_node.trim().is_empty() {
        return Err(ApiError::BadRequest("target_node is required".into()));
    }
    state.vm_manager.get_vm(resource_id).await?;
    let source_node = state
        .cluster_manager
        .get_local_node_name()
        .await
        .unwrap_or_else(|_| "localhost".to_string());
    if source_node == target_node {
        return Err(ApiError::BadRequest(format!(
            "VM {} is already on node {}",
            resource_id, target_node
        )));
    }
    let job_id = super::vm_migration::start(state, config).await?;
    state.ws_state.broadcast_migration_started(
        resource_id.to_string(),
        source_node,
        target_node.to_string(),
    );
    state
        .migration_manager
        .get_job(&job_id)
        .await
        .ok_or_else(|| ApiError::Internal(format!("migration job {} vanished", job_id)))
}

async fn create_job(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateJobRequest>,
) -> Result<(StatusCode, Json<MigrationJobView>), ApiError> {
    let migration_type = parse_type(req.migration_type.as_deref())?;
    let config = migration::MigrationConfig {
        vm_id: req.resource_id,
        target_node: req.target_node,
        migration_type,
        bandwidth_limit: req.bandwidth_limit,
        force: req.force,
        with_local_disks: req.with_local_disks,
        timeout_secs: req.timeout.filter(|t| *t > 0),
    };
    let job = start_one(&state, &req.resource_type, config).await?;
    Ok((StatusCode::CREATED, Json(view(&job))))
}

async fn cancel_job(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let job = state
        .migration_manager
        .get_job(&job_id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Migration job {} not found", job_id)))?;
    if job.state == MigrationState::Cancelled {
        return Ok(StatusCode::NO_CONTENT);
    }
    state
        .migration_manager
        .cancel_migration(&job_id)
        .await
        .map_err(|e| ApiError::Conflict(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

/// The page's bulk dialog (`horcrux-ui` `api::BulkMigrationJob`).
#[derive(Debug, Serialize, Deserialize)]
pub struct BulkJob {
    #[serde(default)]
    pub job_id: String,
    pub target_node: String,
    pub resources: Vec<(String, String)>,
    #[serde(default)]
    pub max_workers: u32,
    #[serde(default)]
    pub migration_type: String,
    #[serde(default)]
    pub status: String,
    #[serde(default = "Utc::now")]
    pub start_time: DateTime<Utc>,
    #[serde(default)]
    pub end_time: Option<DateTime<Utc>>,
    #[serde(default)]
    pub completed_count: usize,
    #[serde(default)]
    pub failed_count: usize,
    #[serde(default)]
    pub total_count: usize,
}

/// Start a migration per resource. The manager runs one migration at a time
/// by default, so resources beyond that limit are reported as failed rather
/// than silently dropped; the response says how many started.
async fn create_bulk(
    State(state): State<Arc<AppState>>,
    Json(req): Json<BulkJob>,
) -> Result<(StatusCode, Json<BulkJob>), ApiError> {
    if req.resources.is_empty() {
        return Err(ApiError::BadRequest("No resources selected".into()));
    }
    let migration_type = parse_type(Some(if req.migration_type.is_empty() {
        "online"
    } else {
        req.migration_type.as_str()
    }))?;
    let mut started = 0usize;
    let mut errors = Vec::new();
    for (rtype, rid) in &req.resources {
        let config = migration::MigrationConfig {
            vm_id: rid.clone(),
            target_node: req.target_node.clone(),
            migration_type: migration_type.clone(),
            bandwidth_limit: None,
            force: false,
            with_local_disks: false,
            timeout_secs: None,
        };
        match start_one(&state, rtype, config).await {
            Ok(_) => started += 1,
            Err(e) => errors.push(format!("{} {}: {:?}", rtype, rid, e)),
        }
    }
    if started == 0 {
        return Err(ApiError::BadRequest(format!(
            "No migration could be started: {}",
            errors.join("; ")
        )));
    }
    let total = req.resources.len();
    Ok((
        StatusCode::CREATED,
        Json(BulkJob {
            job_id: format!("bulk-migration-{}", Utc::now().timestamp()),
            target_node: req.target_node,
            resources: req.resources,
            max_workers: req.max_workers,
            migration_type: type_name(&migration_type).into(),
            status: if errors.is_empty() {
                "running".into()
            } else {
                "partial".into()
            },
            start_time: Utc::now(),
            end_time: None,
            completed_count: 0,
            failed_count: errors.len(),
            total_count: total,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(state: MigrationState) -> MigrationJob {
        MigrationJob {
            id: "migration-vm1-1".into(),
            vm_id: "vm1".into(),
            source_node: "a".into(),
            target_node: "b".into(),
            migration_type: MigrationType::Online,
            state,
            progress: 42.4,
            started: Utc::now(),
            completed: None,
            bandwidth_limit: Some(100),
            error: None,
            transferred_bytes: 10,
            total_bytes: 30,
            timeout_secs: Some(300),
        }
    }

    #[test]
    fn manager_jobs_render_in_page_shape() {
        let v = view(&job(MigrationState::Transferring));
        assert_eq!(v.job_id, "migration-vm1-1");
        assert_eq!(v.resource_type, "vm");
        assert_eq!(v.resource_id, "vm1");
        assert_eq!(v.status, "running");
        assert_eq!(v.migration_type, "online");
        assert_eq!(v.progress, 42);
        assert_eq!(v.remaining_bytes, Some(20));
        assert_eq!(view(&job(MigrationState::Cancelled)).status, "cancelled");
        assert_eq!(view(&job(MigrationState::Pending)).status, "pending");
        assert_eq!(v.timeout, 300, "the job's timeout is shown");
    }

    /// A completed job has nothing left to move, whatever the last
    /// progress sample said (QA: 100% with 0.1 GB "remaining").
    #[test]
    fn completed_job_has_no_remaining_bytes() {
        let mut j = job(MigrationState::Completed);
        j.transferred_bytes = 127_596_322;
        j.total_bytes = 285_810_688;
        assert_eq!(view(&j).remaining_bytes, Some(0));
        j.state = MigrationState::Failed;
        assert_eq!(view(&j).remaining_bytes, Some(158_214_366));
        j.total_bytes = 0;
        j.state = MigrationState::Transferring;
        assert_eq!(view(&j).remaining_bytes, None);
    }

    /// The Migrate dialog's Timeout field reaches the job (it used to be
    /// dropped); 0 means no limit.
    #[test]
    fn dialog_timeout_is_kept() {
        let req: CreateJobRequest = serde_json::from_value(serde_json::json!({
            "resource_id": "vm1", "target_node": "b", "timeout": 300,
            "job_id": "", "status": "pending", "progress": 0
        }))
        .unwrap();
        assert_eq!(req.timeout, Some(300));
        let req: CreateJobRequest =
            serde_json::from_value(serde_json::json!({"resource_id": "vm1", "target_node": "b"}))
                .unwrap();
        assert_eq!(req.timeout, None);
    }

    #[test]
    fn page_migration_types_parse() {
        assert_eq!(parse_type(Some("online")).unwrap(), MigrationType::Online);
        assert_eq!(parse_type(Some("offline")).unwrap(), MigrationType::Offline);
        assert_eq!(parse_type(None).unwrap(), MigrationType::Online);
        assert!(parse_type(Some("teleport")).is_err());
    }
}
