//! VM migration between cluster nodes: job bookkeeping, progress and
//! cancellation.
//!
//! The work itself is a [`MigrationExecutor`] supplied by the caller (the
//! API's peer-to-peer QEMU migration in `horcrux-api/src/api/vm_migration.rs`:
//! offline disk copy, or live with QMP `migrate` + `drive-mirror`). The
//! manager runs it in a background task, lets the user cancel it, and runs
//! the executor's abort step after a cancel or a failure so nothing is left
//! half-moved. (It used to drive `virsh` over ssh, which can't see Horcrux's
//! own QEMU VMs - no libvirt domain exists for them - so every migration
//! failed or, worse, reported success without moving anything.)
pub mod block_migration;
pub mod health_check;
pub mod qemu_monitor;
pub mod rollback;

use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Migration type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MigrationType {
    Live,    // Live migration (minimal downtime)
    Offline, // Offline migration (VM must be stopped)
    Online,  // Online migration with brief pause
}

/// Migration state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MigrationState {
    Pending,
    Preparing,
    Transferring,
    Syncing,
    Finalizing,
    Completed,
    Failed,
    Cancelled,
}

/// Migration job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationJob {
    pub id: String,
    pub vm_id: String,
    pub source_node: String,
    pub target_node: String,
    pub migration_type: MigrationType,
    pub state: MigrationState,
    pub progress: f32, // 0.0 - 100.0
    pub started: DateTime<Utc>,
    pub completed: Option<DateTime<Utc>>,
    pub bandwidth_limit: Option<u64>, // MB/s
    pub error: Option<String>,
    pub transferred_bytes: u64,
    pub total_bytes: u64,
    /// The job fails (and is rolled back) when it runs longer (None/0: no
    /// limit).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

/// Migration configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationConfig {
    pub vm_id: String,
    pub target_node: String,
    pub migration_type: MigrationType,
    pub bandwidth_limit: Option<u64>, // MB/s, None = unlimited
    pub force: bool,                  // Force migration even if checks fail
    pub with_local_disks: bool,       // Migrate local disks (requires shared storage otherwise)
    /// Fail and roll back a migration still running after this many
    /// seconds (None or 0: no limit).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

/// Migration statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationStats {
    pub duration_seconds: u64,
    pub downtime_ms: u64,
    pub transferred_gb: f64,
    pub average_speed_mbps: f64,
    pub memory_dirty_rate: f64, // MB/s
}

/// Handle a migration task reports its progress through.
#[derive(Clone)]
pub struct JobReporter {
    jobs: Arc<RwLock<HashMap<String, MigrationJob>>>,
    job_id: String,
}

impl JobReporter {
    pub fn job_id(&self) -> &str {
        &self.job_id
    }

    /// Record the current phase and progress (0-100). A cancelled job
    /// keeps its cancelled state.
    pub async fn update(&self, state: MigrationState, progress: f32) {
        MigrationManager::update_job_state(&self.jobs, &self.job_id, state, progress).await;
    }

    /// Record bytes moved so far out of the total.
    pub async fn bytes(&self, transferred: u64, total: u64) {
        if let Some(job) = self.jobs.write().await.get_mut(&self.job_id) {
            job.transferred_bytes = transferred;
            job.total_bytes = total;
        }
    }
}

/// The work of one migration: `run` does it; `abort` (after a failure, a
/// cancel or a timeout, with the reason) undoes whatever `run` got done and
/// says what it could not undo (recorded as the job's error). They usually
/// share state through an `Arc` captured by both closures.
pub struct MigrationExecutor {
    pub run: Box<dyn FnOnce(JobReporter) -> BoxFuture<'static, Result<()>> + Send>,
    pub abort:
        Box<dyn FnOnce(String) -> BoxFuture<'static, std::result::Result<(), String>> + Send>,
}

/// Migration manager
pub struct MigrationManager {
    jobs: Arc<RwLock<HashMap<String, MigrationJob>>>,
    bandwidth_limit: Arc<RwLock<Option<u64>>>, // Global limit
    max_concurrent: Arc<RwLock<usize>>,
    rollback_manager: Arc<RwLock<rollback::RollbackManager>>,
    auto_rollback_enabled: Arc<RwLock<bool>>,
    health_check_enabled: Arc<RwLock<bool>>,
    health_reports: Arc<RwLock<HashMap<String, health_check::HealthCheckReport>>>,
    /// Cancel signal of each running job's task (see `cancel_migration`).
    cancels: Arc<RwLock<HashMap<String, tokio::sync::watch::Sender<bool>>>>,
}

impl Default for MigrationManager {
    fn default() -> Self {
        Self::new()
    }
}

impl MigrationManager {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            bandwidth_limit: Arc::new(RwLock::new(None)),
            max_concurrent: Arc::new(RwLock::new(1)), // Default: 1 concurrent migration
            rollback_manager: Arc::new(RwLock::new(rollback::RollbackManager::new())),
            auto_rollback_enabled: Arc::new(RwLock::new(true)), // Auto-rollback enabled by default
            health_check_enabled: Arc::new(RwLock::new(true)),  // Health checks enabled by default
            health_reports: Arc::new(RwLock::new(HashMap::new())),
            cancels: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Enable or disable automatic rollback on migration failure (the
    /// executor's abort step).
    pub async fn set_auto_rollback(&self, enabled: bool) {
        let mut auto_rollback = self.auto_rollback_enabled.write().await;
        *auto_rollback = enabled;
        tracing::info!(
            "Automatic migration rollback {}",
            if enabled { "enabled" } else { "disabled" }
        );
    }

    /// Check if auto-rollback is enabled
    pub async fn is_auto_rollback_enabled(&self) -> bool {
        *self.auto_rollback_enabled.read().await
    }

    /// Enable or disable post-migration health checks
    pub async fn set_health_checks(&self, enabled: bool) {
        let mut health_check = self.health_check_enabled.write().await;
        *health_check = enabled;
        tracing::info!(
            "Post-migration health checks {}",
            if enabled { "enabled" } else { "disabled" }
        );
    }

    /// Check if health checks are enabled
    pub async fn is_health_check_enabled(&self) -> bool {
        *self.health_check_enabled.read().await
    }

    /// The bandwidth limit (MB/s) a migration with `config` runs at: its
    /// own, else the global one.
    pub async fn effective_bandwidth(&self, config: &MigrationConfig) -> Option<u64> {
        config
            .bandwidth_limit
            .or(*self.bandwidth_limit.read().await)
    }

    /// Start a migration job running `executor` in the background.
    pub async fn start_migration(
        &self,
        config: MigrationConfig,
        source_node: String,
        executor: MigrationExecutor,
    ) -> Result<String> {
        let active_count = self.count_active_migrations().await;
        let max_concurrent = *self.max_concurrent.read().await;
        if active_count >= max_concurrent {
            return Err(horcrux_common::Error::System(format!(
                "Maximum concurrent migrations ({}) reached",
                max_concurrent
            )));
        }
        if self
            .list_active()
            .await
            .iter()
            .any(|j| j.vm_id == config.vm_id)
        {
            return Err(horcrux_common::Error::Conflict(format!(
                "VM {} is already being migrated",
                config.vm_id
            )));
        }

        let job_id = format!(
            "migration-{}-{}",
            config.vm_id,
            Utc::now().timestamp_millis()
        );
        let bandwidth_limit = self.effective_bandwidth(&config).await;
        let job = MigrationJob {
            id: job_id.clone(),
            vm_id: config.vm_id.clone(),
            source_node: source_node.clone(),
            target_node: config.target_node.clone(),
            migration_type: config.migration_type.clone(),
            state: MigrationState::Pending,
            progress: 0.0,
            started: Utc::now(),
            completed: None,
            bandwidth_limit,
            error: None,
            transferred_bytes: 0,
            total_bytes: 0,
            timeout_secs: config.timeout_secs.filter(|s| *s > 0),
        };
        let timeout = job.timeout_secs;
        self.jobs.write().await.insert(job_id.clone(), job);

        let jobs = self.jobs.clone();
        let reporter = JobReporter {
            jobs: jobs.clone(),
            job_id: job_id.clone(),
        };
        let auto_rollback = *self.auto_rollback_enabled.read().await;
        let (cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);
        self.cancels.write().await.insert(job_id.clone(), cancel_tx);
        let cancels = self.cancels.clone();
        let task_job_id = job_id.clone();

        tokio::spawn(async move {
            let MigrationExecutor { run, abort } = executor;
            // Cancelling drops the run future between (or in the middle
            // of) its steps: no later step runs and in-flight child
            // processes die with it (kill_on_drop); then abort undoes the
            // steps already done.
            // The job's timeout stops it like a cancel (and always rolls
            // back: its run was dropped in the middle of a step).
            let deadline = async move {
                match timeout {
                    Some(secs) => tokio::time::sleep(std::time::Duration::from_secs(secs)).await,
                    None => std::future::pending::<()>().await,
                }
            };
            let mut timed_out = false;
            let result = tokio::select! {
                r = run(reporter) => r,
                _ = cancel_rx.wait_for(|c| *c) => {
                    tracing::info!("Migration {} cancelled: remaining steps skipped", task_job_id);
                    Err(horcrux_common::Error::System("cancelled by the user".into()))
                }
                _ = deadline => {
                    timed_out = true;
                    Err(horcrux_common::Error::System(format!(
                        "timed out: still running after its {} s timeout",
                        timeout.unwrap_or(0)
                    )))
                }
            };
            let cancelled = jobs
                .read()
                .await
                .get(&task_job_id)
                .map(|j| j.state == MigrationState::Cancelled)
                .unwrap_or(false);
            let mut rollback_error = None;
            if let Err(e) = &result {
                if cancelled || timed_out || auto_rollback {
                    tracing::warn!("Migration {} {}: rolling back", task_job_id, e);
                    if let Err(left) = abort(e.to_string()).await {
                        tracing::error!("Migration {} rollback incomplete: {}", task_job_id, left);
                        rollback_error = Some(left);
                    }
                } else {
                    tracing::warn!(
                        "Migration {} failed ({}) and automatic rollback is disabled",
                        task_job_id,
                        e
                    );
                }
            }
            cancels.write().await.remove(&task_job_id);
            let mut jobs_lock = jobs.write().await;
            let Some(job) = jobs_lock.get_mut(&task_job_id) else {
                return;
            };
            if job.state == MigrationState::Cancelled {
                // Cancelled stays cancelled; a rollback that could not
                // undo everything says so.
                if let Some(left) = rollback_error {
                    job.error = Some(format!("cancelled, but the rollback is incomplete: {left}"));
                }
                return;
            }
            job.completed = Some(Utc::now());
            match result {
                Ok(()) => {
                    job.state = MigrationState::Completed;
                    job.progress = 100.0;
                    // Everything arrived: nothing remains (the last
                    // progress sample was taken before the end).
                    let total = job.total_bytes.max(job.transferred_bytes);
                    job.total_bytes = total;
                    job.transferred_bytes = total;
                }
                Err(e) => {
                    job.state = MigrationState::Failed;
                    job.error = Some(match rollback_error {
                        Some(left) => format!("{e}; rollback incomplete: {left}"),
                        None => e.to_string(),
                    });
                }
            }
        });

        tracing::info!(
            "Started {} migration of VM {} from {} to {}",
            match config.migration_type {
                MigrationType::Live => "live",
                MigrationType::Offline => "offline",
                MigrationType::Online => "online",
            },
            config.vm_id,
            source_node,
            config.target_node
        );
        Ok(job_id)
    }

    /// Update job state
    async fn update_job_state(
        jobs: &Arc<RwLock<HashMap<String, MigrationJob>>>,
        job_id: &str,
        state: MigrationState,
        progress: f32,
    ) {
        let mut jobs_lock = jobs.write().await;
        if let Some(job) = jobs_lock.get_mut(job_id) {
            // A cancelled job stays cancelled: the background task used to
            // flip it back to Transferring/Completed a moment later.
            if job.state == MigrationState::Cancelled {
                return;
            }
            job.state = state;
            job.progress = progress;
        }
    }

    /// Get migration job status
    pub async fn get_job(&self, job_id: &str) -> Option<MigrationJob> {
        self.jobs.read().await.get(job_id).cloned()
    }

    /// Most recently started migration job for a VM.
    pub async fn latest_job_for_vm(&self, vm_id: &str) -> Option<MigrationJob> {
        self.jobs
            .read()
            .await
            .values()
            .filter(|j| j.vm_id == vm_id)
            .max_by_key(|j| j.started)
            .cloned()
    }

    /// List all migration jobs
    pub async fn list_jobs(&self) -> Vec<MigrationJob> {
        self.jobs.read().await.values().cloned().collect()
    }

    /// List active migrations
    pub async fn list_active(&self) -> Vec<MigrationJob> {
        self.jobs
            .read()
            .await
            .values()
            .filter(|j| {
                !matches!(
                    j.state,
                    MigrationState::Completed | MigrationState::Failed | MigrationState::Cancelled
                )
            })
            .cloned()
            .collect()
    }

    /// Cancel migration: the job is marked cancelled at once, its task
    /// stops between (or in the middle of) its steps and the executor's
    /// abort undoes what was done so far (the VM keeps running on the
    /// source, the target's half-received copy is removed).
    pub async fn cancel_migration(&self, job_id: &str) -> Result<()> {
        let mut jobs = self.jobs.write().await;

        let job = jobs.get_mut(job_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("Migration job {} not found", job_id))
        })?;

        if matches!(
            job.state,
            MigrationState::Completed | MigrationState::Failed
        ) {
            return Err(horcrux_common::Error::System(
                "Cannot cancel completed or failed migration".to_string(),
            ));
        }

        job.state = MigrationState::Cancelled;
        job.completed = Some(Utc::now());
        drop(jobs);

        if let Some(tx) = self.cancels.read().await.get(job_id) {
            let _ = tx.send(true);
        }

        tracing::info!("Cancelled migration job {}", job_id);
        Ok(())
    }

    /// Count active migrations
    async fn count_active_migrations(&self) -> usize {
        self.jobs
            .read()
            .await
            .values()
            .filter(|j| {
                !matches!(
                    j.state,
                    MigrationState::Completed | MigrationState::Failed | MigrationState::Cancelled
                )
            })
            .count()
    }

    /// Set global bandwidth limit
    pub async fn set_bandwidth_limit(&self, limit_mbps: Option<u64>) {
        let mut bw = self.bandwidth_limit.write().await;
        *bw = limit_mbps;
        tracing::info!(
            "Set global migration bandwidth limit to {:?} MB/s",
            limit_mbps
        );
    }

    /// Set max concurrent migrations
    pub async fn set_max_concurrent(&self, max: usize) {
        let mut max_concurrent = self.max_concurrent.write().await;
        *max_concurrent = max;
        tracing::info!("Set max concurrent migrations to {}", max);
    }

    /// Get migration statistics
    pub async fn get_statistics(&self, job_id: &str) -> Result<MigrationStats> {
        let job = self.get_job(job_id).await.ok_or_else(|| {
            horcrux_common::Error::System(format!("Migration job {} not found", job_id))
        })?;

        let duration = if let Some(completed) = job.completed {
            (completed - job.started).num_seconds() as u64
        } else {
            (Utc::now() - job.started).num_seconds() as u64
        };

        let transferred_gb = job.transferred_bytes as f64 / (1024.0 * 1024.0 * 1024.0);

        let average_speed_mbps = if duration > 0 {
            (job.transferred_bytes as f64 / (1024.0 * 1024.0)) / duration as f64
        } else {
            0.0
        };

        Ok(MigrationStats {
            duration_seconds: duration,
            downtime_ms: match job.migration_type {
                MigrationType::Live => 100,   // Typical live migration downtime
                MigrationType::Online => 500, // Brief pause
                MigrationType::Offline => duration * 1000, // Full downtime
            },
            transferred_gb,
            average_speed_mbps,
            memory_dirty_rate: 0.0, // Would be calculated from actual migration
        })
    }

    /// Get rollback plan for a migration job
    pub async fn get_rollback(&self, migration_job_id: &str) -> Option<rollback::RollbackPlan> {
        self.rollback_manager
            .read()
            .await
            .get_rollback(migration_job_id)
            .cloned()
    }

    /// List all rollback plans
    pub async fn list_rollbacks(&self) -> Vec<rollback::RollbackPlan> {
        self.rollback_manager
            .read()
            .await
            .list_rollbacks()
            .into_iter()
            .cloned()
            .collect()
    }

    /// Manually trigger rollback for a failed migration
    pub async fn manual_rollback(
        &self,
        migration_job_id: &str,
    ) -> Result<rollback::RollbackSummary> {
        let job = self.get_job(migration_job_id).await.ok_or_else(|| {
            horcrux_common::Error::System(format!("Migration job {} not found", migration_job_id))
        })?;

        if job.state != MigrationState::Failed {
            return Err(horcrux_common::Error::System(format!(
                "Can only rollback failed migrations. Current state: {:?}",
                job.state
            )));
        }

        let mut rb_manager = self.rollback_manager.write().await;
        rb_manager
            .rollback_migration(
                migration_job_id.to_string(),
                &job.vm_id,
                job.source_node,
                job.target_node,
            )
            .await
    }

    /// Get health check report for a migration
    pub async fn get_health_report(&self, job_id: &str) -> Option<health_check::HealthCheckReport> {
        self.health_reports.read().await.get(job_id).cloned()
    }

    /// List all health check reports
    pub async fn list_health_reports(&self) -> Vec<health_check::HealthCheckReport> {
        self.health_reports.read().await.values().cloned().collect()
    }

    /// Get health check summary for a migration
    pub async fn get_health_summary(
        &self,
        job_id: &str,
    ) -> Option<health_check::HealthCheckSummary> {
        self.get_health_report(job_id)
            .await
            .map(|r| r.get_summary())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn config(vm: &str) -> MigrationConfig {
        MigrationConfig {
            vm_id: vm.to_string(),
            target_node: "node2".to_string(),
            migration_type: MigrationType::Live,
            bandwidth_limit: None,
            force: false,
            with_local_disks: false,
            timeout_secs: None,
        }
    }

    /// An executor whose run sleeps `secs` (then succeeds or fails) and
    /// counts aborts.
    fn executor(secs: u64, ok: bool, aborts: Arc<AtomicUsize>) -> MigrationExecutor {
        MigrationExecutor {
            run: Box::new(move |r: JobReporter| {
                Box::pin(async move {
                    r.update(MigrationState::Transferring, 40.0).await;
                    r.bytes(10, 100).await;
                    tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
                    if ok {
                        Ok(())
                    } else {
                        Err(horcrux_common::Error::System("target refused".into()))
                    }
                })
            }),
            abort: Box::new(move |_reason| {
                Box::pin(async move {
                    aborts.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            }),
        }
    }

    async fn wait_done(m: &MigrationManager, id: &str) -> MigrationJob {
        for _ in 0..100 {
            let j = m.get_job(id).await.unwrap();
            if j.completed.is_some() && m.cancels.read().await.is_empty() {
                return j;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("job {id} did not finish");
    }

    #[tokio::test]
    async fn successful_run_completes_the_job() {
        let m = MigrationManager::new();
        let aborts = Arc::new(AtomicUsize::new(0));
        let id = m
            .start_migration(
                config("100"),
                "node1".into(),
                executor(0, true, aborts.clone()),
            )
            .await
            .unwrap();
        let j = wait_done(&m, &id).await;
        assert_eq!(j.state, MigrationState::Completed);
        assert_eq!(j.progress, 100.0);
        // The last sample said 10 of 100; a completed job moved it all.
        assert_eq!((j.transferred_bytes, j.total_bytes), (100, 100));
        assert_eq!(aborts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn failed_run_is_rolled_back_and_reported() {
        let m = MigrationManager::new();
        let aborts = Arc::new(AtomicUsize::new(0));
        let id = m
            .start_migration(
                config("100"),
                "node1".into(),
                executor(0, false, aborts.clone()),
            )
            .await
            .unwrap();
        let j = wait_done(&m, &id).await;
        assert_eq!(j.state, MigrationState::Failed);
        assert!(j.error.unwrap().contains("target refused"));
        assert_eq!(aborts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn concurrent_migration_limit_and_one_job_per_vm() {
        let m = MigrationManager::new();
        m.set_max_concurrent(2).await;
        let a = Arc::new(AtomicUsize::new(0));
        assert!(m
            .start_migration(config("100"), "node1".into(), executor(5, true, a.clone()))
            .await
            .is_ok());
        let again = m
            .start_migration(config("100"), "node1".into(), executor(5, true, a.clone()))
            .await;
        assert!(matches!(again, Err(horcrux_common::Error::Conflict(_))));
        assert!(m
            .start_migration(config("101"), "node1".into(), executor(5, true, a.clone()))
            .await
            .is_ok());
        assert!(m
            .start_migration(config("102"), "node1".into(), executor(5, true, a))
            .await
            .is_err());
    }

    /// A cancel must stop the work, not just the displayed state: the
    /// running step's future is dropped (its child process killed), no
    /// later step runs, the abort step runs once, and the job stays
    /// cancelled.
    #[tokio::test]
    async fn cancel_stops_the_run_and_rolls_back() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("after");
        let m2 = marker.clone();
        let aborts = Arc::new(AtomicUsize::new(0));
        let a2 = aborts.clone();
        let exec = MigrationExecutor {
            run: Box::new(move |_r| {
                Box::pin(async move {
                    // A step that hangs (like a copy to a stuck target)...
                    let _ = tokio::process::Command::new("sleep")
                        .arg("3")
                        .kill_on_drop(true)
                        .status()
                        .await;
                    // ...and the step after it, which must never run.
                    std::fs::write(&m2, b"ran").unwrap();
                    Ok(())
                })
            }),
            abort: Box::new(move |reason| {
                Box::pin(async move {
                    assert!(reason.contains("cancelled"), "{reason}");
                    a2.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            }),
        };
        let m = MigrationManager::new();
        let id = m
            .start_migration(config("qa-src"), "node1".into(), exec)
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        m.cancel_migration(&id).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        assert!(!marker.exists(), "a step ran after the cancel");
        assert_eq!(aborts.load(Ordering::SeqCst), 1);
        let j = m.get_job(&id).await.unwrap();
        assert_eq!(j.state, MigrationState::Cancelled);
        assert!(m.cancels.read().await.is_empty());
        assert!(m.list_active().await.is_empty(), "the slot is free again");
        // A finished job can't be cancelled; an unknown one is an error.
        assert!(m.cancel_migration("nope").await.is_err());
    }

    /// The job's timeout is enforced: a run still going after it is
    /// stopped (no later step runs), rolled back, and the job fails with
    /// the reason; a run within its timeout completes.
    #[tokio::test]
    async fn timeout_fails_and_rolls_back_the_job() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("after");
        let m2 = marker.clone();
        let aborts = Arc::new(AtomicUsize::new(0));
        let a2 = aborts.clone();
        let exec = MigrationExecutor {
            run: Box::new(move |r: JobReporter| {
                Box::pin(async move {
                    r.update(MigrationState::Transferring, 6.0).await;
                    // A target that stopped answering: no progress.
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    std::fs::write(&m2, b"ran").unwrap();
                    Ok(())
                })
            }),
            abort: Box::new(move |reason| {
                Box::pin(async move {
                    assert!(reason.contains("timed out"), "{reason}");
                    a2.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            }),
        };
        let m = MigrationManager::new();
        // Even with automatic rollback off, a timeout rolls back.
        m.set_auto_rollback(false).await;
        let mut c = config("slow");
        c.timeout_secs = Some(1);
        let id = m.start_migration(c, "node1".into(), exec).await.unwrap();
        assert_eq!(m.get_job(&id).await.unwrap().timeout_secs, Some(1));
        let j = wait_done(&m, &id).await;
        assert_eq!(j.state, MigrationState::Failed);
        assert!(
            j.error.as_deref().unwrap().contains("timed out"),
            "{:?}",
            j.error
        );
        assert_eq!(aborts.load(Ordering::SeqCst), 1);
        assert!(!marker.exists());

        // timeout 0 = no limit; a quick run under a limit completes.
        let mut c = config("quick");
        c.timeout_secs = Some(0);
        let a = Arc::new(AtomicUsize::new(0));
        let id = m
            .start_migration(c, "node1".into(), executor(0, true, a.clone()))
            .await
            .unwrap();
        assert_eq!(m.get_job(&id).await.unwrap().timeout_secs, None);
        assert_eq!(wait_done(&m, &id).await.state, MigrationState::Completed);
        let mut c = config("quick2");
        c.timeout_secs = Some(30);
        let id = m
            .start_migration(c, "node1".into(), executor(0, true, a.clone()))
            .await
            .unwrap();
        assert_eq!(wait_done(&m, &id).await.state, MigrationState::Completed);
        assert_eq!(a.load(Ordering::SeqCst), 0);
    }

    /// A rollback that can't undo everything (e.g. a disk mirror to a dead
    /// target still attached) is reported on the job, also a cancelled one.
    #[tokio::test]
    async fn incomplete_rollback_is_reported() {
        let failing = |secs: u64, ok: bool| MigrationExecutor {
            run: Box::new(move |_r| {
                Box::pin(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
                    if ok {
                        Ok(())
                    } else {
                        Err(horcrux_common::Error::System("target gone".into()))
                    }
                })
            }),
            abort: Box::new(move |_reason| {
                Box::pin(async move { Err("disk mirror mig-drive-disk0 still attached".into()) })
            }),
        };
        let m = MigrationManager::new();
        let id = m
            .start_migration(config("a"), "node1".into(), failing(0, false))
            .await
            .unwrap();
        let j = wait_done(&m, &id).await;
        assert_eq!(j.state, MigrationState::Failed);
        let e = j.error.unwrap();
        assert!(
            e.contains("target gone")
                && e.contains("rollback incomplete")
                && e.contains("still attached"),
            "{e}"
        );

        let id = m
            .start_migration(config("b"), "node1".into(), failing(30, true))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        m.cancel_migration(&id).await.unwrap();
        wait_done(&m, &id).await;
        let mut j = m.get_job(&id).await.unwrap();
        for _ in 0..50 {
            if j.error.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            j = m.get_job(&id).await.unwrap();
        }
        assert_eq!(j.state, MigrationState::Cancelled);
        assert!(j.error.unwrap().contains("still attached"));
    }
}
