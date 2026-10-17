//! Backup/restore HTTP API (lane D).
//!
//! `routes(&state)` connects the backup manager to the VM, container and
//! storage managers, starts the backup job scheduler and returns the
//! `/api/backups*`, `/api/backup-jobs*` and `/api/backup-storages` routes.
use crate::alerts::{Alert, AlertSeverity, AlertStatus};
use crate::backup::retention::PruneDecision;
use crate::backup::{
    Backup, BackupConfig, BackupJob, BackupRuntime, BackupUpdate, JobHook, JobRunReport,
    PruneRequest, RestoreRequest, RestoreResult, RetentionPolicy, VerifyResult,
};
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

type St = State<Arc<AppState>>;

/// Wire the backup manager to the runtime managers, start the scheduler
/// and build the routes.
pub fn routes(state: &Arc<AppState>) -> Router<Arc<AppState>> {
    let hook_state = state.clone();
    let hook: JobHook = Arc::new(move |job: BackupJob, report: JobRunReport| {
        let st = hook_state.clone();
        Box::pin(async move { job_finished(st, job, report).await })
    });
    let lxc_path = std::env::var("HORCRUX_LXC_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/var/lib/lxc"));
    let attached = state.backup_manager.attach_runtime(BackupRuntime {
        vm_manager: state.vm_manager.clone(),
        container_manager: state.container_manager.clone(),
        storage_manager: Some(state.storage_manager.clone()),
        vm_storage: state.config.paths.vm_storage.clone(),
        lxc_path,
        on_job_finished: Some(hook),
    });
    if attached.is_ok() {
        let tick = std::env::var("HORCRUX_BACKUP_SCHEDULER_TICK_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|s| *s > 0)
            .unwrap_or(15);
        // Clean up after a backup interrupted by an API crash/kill before
        // the scheduler can start new ones.
        let bm = state.backup_manager.clone();
        tokio::spawn(async move {
            bm.sweep_interrupted().await;
            bm.start_scheduler(std::time::Duration::from_secs(tick));
        });
    }

    Router::new()
        .route("/api/backups", get(list_backups).post(create_backup))
        .route("/api/backups/prune", post(prune_backups))
        .route(
            "/api/backups/:id",
            get(get_backup).put(update_backup).delete(delete_backup),
        )
        .route("/api/backups/:id/restore", post(restore_backup))
        .route("/api/backups/:id/verify", post(verify_backup))
        .route("/api/backups/retention/:target_id", post(apply_retention))
        .route("/api/backup-storages", get(list_backup_storages))
        .route("/api/backup-jobs", get(list_jobs).post(create_job))
        .route(
            "/api/backup-jobs/:id",
            get(get_job).put(update_job).delete(delete_job),
        )
        .route("/api/backup-jobs/:id/run", post(run_job))
}

/// Send the job result to the configured notification channels (per the
/// job's `notify` setting) and to WebSocket clients.
async fn job_finished(state: Arc<AppState>, job: BackupJob, report: JobRunReport) {
    for r in &report.results {
        if let Some(bid) = &r.backup_id {
            if let Ok(b) = state.backup_manager.get_backup(bid).await {
                state.ws_state.broadcast_backup_completed(
                    b.target_id.clone(),
                    b.id.clone(),
                    b.size,
                    (report.finished_at - report.started_at).max(0) as u64,
                );
            }
        }
    }
    let send = match job.notify.as_deref() {
        Some("always") => true,
        Some("failure") => !report.ok,
        _ => false,
    };
    if !send {
        return;
    }
    let failed: Vec<String> = report
        .results
        .iter()
        .filter_map(|r| r.error.as_ref().map(|e| format!("{}: {e}", r.target_id)))
        .collect();
    let message = if report.ok {
        format!(
            "Backup job {} finished OK: {} guest(s) backed up",
            job.id,
            report.results.len()
        )
    } else {
        format!(
            "Backup job {} FAILED for {} of {} guest(s): {}",
            job.id,
            failed.len(),
            report.results.len(),
            failed.join("; ")
        )
    };
    let alert = Alert {
        id: uuid::Uuid::new_v4().to_string(),
        rule_id: format!("backup-job-{}", job.id),
        rule_name: format!("Backup job {}", job.id),
        severity: if report.ok {
            AlertSeverity::Info
        } else {
            AlertSeverity::Critical
        },
        status: if report.ok {
            AlertStatus::Resolved
        } else {
            AlertStatus::Firing
        },
        message,
        target: job.targets.join(","),
        metric_value: failed.len() as f64,
        threshold: 0.0,
        fired_at: report.finished_at,
        resolved_at: None,
        acknowledged_at: None,
        acknowledged_by: None,
    };
    // Legacy alert channels and the Monitoring > Notifications channels.
    state.alert_manager.send_notifications(&alert).await;
}

#[derive(Deserialize)]
struct ListQuery {
    target_id: Option<String>,
}

async fn list_backups(
    State(state): St,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<Backup>>, ApiError> {
    Ok(Json(state.backup_manager.list_backups(q.target_id).await?))
}

async fn get_backup(State(state): St, Path(id): Path<String>) -> Result<Json<Backup>, ApiError> {
    Ok(Json(state.backup_manager.get_backup(&id).await?))
}

async fn create_backup(
    State(state): St,
    Json(payload): Json<BackupConfig>,
) -> Result<(StatusCode, Json<Backup>), ApiError> {
    let started = std::time::Instant::now();
    let backup = state.backup_manager.create_backup(payload).await?;
    state.ws_state.broadcast_backup_completed(
        backup.target_id.clone(),
        backup.id.clone(),
        backup.size,
        started.elapsed().as_secs(),
    );
    Ok((StatusCode::CREATED, Json(backup)))
}

async fn update_backup(
    State(state): St,
    Path(id): Path<String>,
    Json(update): Json<BackupUpdate>,
) -> Result<Json<Backup>, ApiError> {
    Ok(Json(state.backup_manager.update_backup(&id, update).await?))
}

async fn delete_backup(State(state): St, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    state.backup_manager.delete_backup(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn restore_backup(
    State(state): St,
    Path(id): Path<String>,
    body: Option<Json<RestoreRequest>>,
) -> Result<Json<RestoreResult>, ApiError> {
    let req = body.map(|Json(r)| r).unwrap_or_default();
    Ok(Json(state.backup_manager.restore_backup(&id, req).await?))
}

async fn verify_backup(
    State(state): St,
    Path(id): Path<String>,
) -> Result<Json<VerifyResult>, ApiError> {
    Ok(Json(state.backup_manager.verify_backup(&id).await?))
}

async fn prune_backups(
    State(state): St,
    Json(req): Json<PruneRequest>,
) -> Result<Json<Vec<PruneDecision>>, ApiError> {
    Ok(Json(state.backup_manager.prune(&req).await?))
}

async fn apply_retention(
    State(state): St,
    Path(target_id): Path<String>,
    Json(policy): Json<RetentionPolicy>,
) -> Result<Json<Vec<PruneDecision>>, ApiError> {
    Ok(Json(
        state
            .backup_manager
            .apply_retention(&target_id, &policy)
            .await?,
    ))
}

/// A place backups can be written to.
#[derive(Serialize)]
struct BackupStorage {
    /// Value to pass as `storage`.
    id: String,
    /// Directory archives are written to.
    path: String,
    /// "default" or the pool's storage type.
    kind: String,
    available_gb: Option<u64>,
    total_gb: Option<u64>,
}

async fn list_backup_storages(State(state): St) -> Json<Vec<BackupStorage>> {
    let mut out = Vec::new();
    let local = state.backup_manager.default_storage().to_path_buf();
    let cap = crate::storage::fs_capacity_gb(&local.to_string_lossy()).or_else(|| {
        local
            .parent()
            .and_then(|p| crate::storage::fs_capacity_gb(&p.to_string_lossy()))
    });
    out.push(BackupStorage {
        id: "local".into(),
        path: local.to_string_lossy().into(),
        kind: "default".into(),
        total_gb: cap.map(|c| c.0),
        available_gb: cap.map(|c| c.1),
    });
    for pool in state.storage_manager.list_pools().await {
        if let Ok((id, dir)) = state.backup_manager.resolve_storage(&pool.id).await {
            out.push(BackupStorage {
                id,
                path: dir.to_string_lossy().into(),
                kind: format!("{:?}", pool.storage_type).to_lowercase(),
                total_gb: Some(pool.total),
                available_gb: Some(pool.available),
            });
        }
    }
    Json(out)
}

async fn list_jobs(State(state): St) -> Result<Json<Vec<BackupJob>>, ApiError> {
    Ok(Json(state.backup_manager.list_jobs().await?))
}

async fn get_job(State(state): St, Path(id): Path<String>) -> Result<Json<BackupJob>, ApiError> {
    Ok(Json(state.backup_manager.get_job(&id).await?))
}

async fn create_job(
    State(state): St,
    Json(job): Json<BackupJob>,
) -> Result<(StatusCode, Json<BackupJob>), ApiError> {
    Ok((
        StatusCode::CREATED,
        Json(state.backup_manager.create_job(job).await?),
    ))
}

async fn update_job(
    State(state): St,
    Path(id): Path<String>,
    Json(job): Json<BackupJob>,
) -> Result<Json<BackupJob>, ApiError> {
    Ok(Json(state.backup_manager.update_job(&id, job).await?))
}

async fn delete_job(State(state): St, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    state.backup_manager.delete_job(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct RunStarted {
    job_id: String,
    started: bool,
}

/// Start a job in the background (202); its outcome lands on the job
/// (`last_run`/`last_status`/`last_error`) and in the backup list.
async fn run_job(
    State(state): St,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<RunStarted>), ApiError> {
    let job = state.backup_manager.get_job(&id).await?;
    let mgr = state.backup_manager.clone();
    let job_id = job.id.clone();
    tokio::spawn(async move {
        if let Err(e) = mgr.run_job(&job_id).await {
            tracing::error!("backup job {job_id}: {e}");
        }
    });
    Ok((
        StatusCode::ACCEPTED,
        Json(RunStarted {
            job_id: job.id,
            started: true,
        }),
    ))
}
