//! Snapshot Replication Module
//!
//! Provides cross-node snapshot replication using:
//! - ZFS send/receive for incremental transfers
//! - SSH tunneling for secure transfer
//! - Bandwidth throttling
//! - Progress tracking
//! - Automatic cleanup of old replicas
use crate::db::config_store::{self, kinds, ConfigStore};
use crate::db::Database;
use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

/// Replication job configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationJob {
    pub id: String,
    pub name: String,
    pub source_vm_id: String,
    pub source_snapshot: String,
    pub target_node: String,
    pub target_pool: String,
    pub schedule: ReplicationSchedule,
    pub bandwidth_limit_mbps: Option<u32>,
    pub retention_count: u32,
    pub enabled: bool,
    pub last_run: Option<i64>,
    pub next_run: i64,
    pub created_at: i64,
    /// Outcome of the most recent run: `running`, `ok` or `failed`
    /// (`None` = never run). Persisted so a background failure stays
    /// visible on the job after the request that started it has returned.
    #[serde(default)]
    pub last_status: Option<String>,
    /// Error message of the most recent failed run (cleared by a success).
    #[serde(default)]
    pub last_error: Option<String>,
}

/// Replication schedule
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplicationSchedule {
    Hourly,
    Daily { hour: u8 },
    Weekly { day: u8, hour: u8 },
    Manual, // Only replicate on demand
}

/// Replication state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationState {
    pub job_id: String,
    pub status: ReplicationStatus,
    pub progress_percent: u8,
    pub transferred_bytes: u64,
    pub total_bytes: u64,
    pub bandwidth_mbps: f64,
    pub started_at: i64,
    pub estimated_completion: Option<i64>,
    pub error: Option<String>,
}

/// Replication status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ReplicationStatus {
    Idle,
    Preparing,
    Transferring,
    Finalizing,
    Completed,
    Failed,
}

/// Snapshot replication manager
pub struct ReplicationManager {
    jobs: Arc<RwLock<HashMap<String, ReplicationJob>>>,
    active_replications: Arc<RwLock<HashMap<String, ReplicationState>>>,
    /// Durable copy of `jobs`; `None` = in-memory only.
    store: Option<ConfigStore>,
}

impl Default for ReplicationManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ReplicationManager {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            active_replications: Arc::new(RwLock::new(HashMap::new())),
            store: None,
        }
    }

    /// Replication manager backed by the database: loads persisted jobs
    /// (including last/next run times) and writes every change through.
    pub async fn with_database(db: Arc<Database>) -> Result<Self> {
        let store = ConfigStore::new(&db);
        let mut mgr = Self::new();
        {
            let mut jobs = mgr.jobs.write().await;
            for job in store
                .load_values::<ReplicationJob>(kinds::REPLICATION_JOB)
                .await?
            {
                jobs.insert(job.id.clone(), job);
            }
            // A run that was in flight when the process stopped never
            // finished: show it as failed instead of "running" forever.
            for job in jobs.values_mut() {
                if job.last_status.as_deref() == Some("running") {
                    job.last_status = Some("failed".to_string());
                    job.last_error =
                        Some("interrupted: the API restarted during the run".to_string());
                    config_store::put_opt(
                        &Some(store.clone()),
                        kinds::REPLICATION_JOB,
                        &job.id,
                        &*job,
                    )
                    .await?;
                }
            }
            info!("Loaded {} replication job(s) from database", jobs.len());
        }
        mgr.store = Some(store);
        Ok(mgr)
    }

    /// Create a new replication job
    pub async fn create_job(&self, mut job: ReplicationJob) -> Result<ReplicationJob> {
        let now = chrono::Utc::now().timestamp();
        job.created_at = now;
        job.next_run = Self::calculate_next_run(&job.schedule, now);

        let mut jobs = self.jobs.write().await;

        if jobs.contains_key(&job.id) {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Replication job {} already exists",
                job.id
            )));
        }

        config_store::put_opt(&self.store, kinds::REPLICATION_JOB, &job.id, &job).await?;
        jobs.insert(job.id.clone(), job.clone());
        info!("Created replication job: {}", job.name);

        Ok(job)
    }

    /// Get a replication job
    pub async fn get_job(&self, job_id: &str) -> Option<ReplicationJob> {
        let jobs = self.jobs.read().await;
        jobs.get(job_id).cloned()
    }

    /// List all replication jobs
    pub async fn list_jobs(&self) -> Vec<ReplicationJob> {
        let jobs = self.jobs.read().await;
        jobs.values().cloned().collect()
    }

    /// Delete a replication job
    pub async fn delete_job(&self, job_id: &str) -> Result<()> {
        let mut jobs = self.jobs.write().await;

        if !jobs.contains_key(job_id) {
            return Err(horcrux_common::Error::NotFound(format!(
                "Replication job {} not found",
                job_id
            )));
        }
        config_store::delete_opt(&self.store, kinds::REPLICATION_JOB, job_id).await?;
        jobs.remove(job_id);

        info!("Deleted replication job: {}", job_id);
        Ok(())
    }

    /// Check, before anything is started, that a run can possibly work:
    /// `ssh` and `zfs` are installed on this node and the target is usable.
    /// Reported to the caller of "execute" instead of failing later in the
    /// background.
    fn check_preconditions(job: &ReplicationJob) -> Result<()> {
        let target = job.target_node.trim();
        if target.is_empty() || target.starts_with('-') || target.contains(char::is_whitespace) {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Replication job {} has an invalid target node '{}'",
                job.id, job.target_node
            )));
        }
        for bin in ["zfs", "ssh"] {
            if !binary_in_path(bin) {
                return Err(horcrux_common::Error::InvalidConfig(format!(
                    "Cannot replicate: the '{}' command is not installed on this node{}",
                    bin,
                    if bin == "zfs" {
                        " (replication uses ZFS send/receive)"
                    } else {
                        " (needed to reach the target node)"
                    }
                )));
            }
        }
        Ok(())
    }

    /// Execute a replication job
    pub async fn execute_replication(&self, job_id: &str) -> Result<ReplicationState> {
        let job = self.get_job(job_id).await.ok_or_else(|| {
            horcrux_common::Error::NotFound(format!("Replication job {} not found", job_id))
        })?;

        if !job.enabled {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Replication job {} is disabled",
                job_id
            )));
        }

        Self::check_preconditions(&job)?;

        // Create initial state
        let state = ReplicationState {
            job_id: job.id.clone(),
            status: ReplicationStatus::Preparing,
            progress_percent: 0,
            transferred_bytes: 0,
            total_bytes: 0,
            bandwidth_mbps: 0.0,
            started_at: chrono::Utc::now().timestamp(),
            estimated_completion: None,
            error: None,
        };

        {
            let mut active = self.active_replications.write().await;
            if let Some(existing) = active.get(&job.id) {
                if matches!(
                    existing.status,
                    ReplicationStatus::Preparing
                        | ReplicationStatus::Transferring
                        | ReplicationStatus::Finalizing
                ) {
                    return Err(horcrux_common::Error::Conflict(format!(
                        "Replication job {} is already running",
                        job_id
                    )));
                }
            }
            active.insert(job.id.clone(), state.clone());
        }
        self.record_outcome(&job.id, Some("running"), None).await;

        // Execute replication in background
        let manager = Arc::new(self.clone());
        let job_clone = job.clone();

        tokio::spawn(async move {
            let m = manager.clone();
            manager
                .run_and_record(&job_clone.id, || m.perform_replication(job_clone.clone()))
                .await;
        });

        Ok(state)
    }

    /// Run `op` (the replication itself) and persist how it ended on the
    /// job: `last_status` ok/failed, `last_error`, plus the live state.
    async fn run_and_record<F, Fut>(&self, job_id: &str, op: F)
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
        match op().await {
            Ok(()) => {
                self.record_outcome(job_id, Some("ok"), None).await;
            }
            Err(e) => {
                error!("Replication failed for job {}: {}", job_id, e);
                let msg = e.to_string();
                self.update_state(job_id, |state| {
                    state.status = ReplicationStatus::Failed;
                    state.error = Some(msg.clone());
                })
                .await;
                self.record_outcome(job_id, Some("failed"), Some(msg)).await;
            }
        }
    }

    /// Persist the run outcome on the job (best effort: a store error is
    /// logged, the in-memory job is still updated).
    async fn record_outcome(&self, job_id: &str, status: Option<&str>, err: Option<String>) {
        let mut jobs = self.jobs.write().await;
        if let Some(job) = jobs.get_mut(job_id) {
            job.last_status = status.map(str::to_string);
            job.last_error = err;
            if let Err(e) =
                config_store::put_opt(&self.store, kinds::REPLICATION_JOB, &job.id, &*job).await
            {
                warn!(
                    "Failed to persist replication job {} outcome: {}",
                    job_id, e
                );
            }
        }
    }

    /// Perform the actual replication
    async fn perform_replication(&self, job: ReplicationJob) -> Result<()> {
        info!(
            "Starting replication: {} -> {}",
            job.source_vm_id, job.target_node
        );

        // Update state to transferring
        self.update_state(&job.id, |state| {
            state.status = ReplicationStatus::Transferring;
            state.progress_percent = 10;
        })
        .await;

        // Determine if this is incremental or full replication
        let is_incremental = self.check_previous_snapshot(&job).await?;

        // Execute ZFS send/receive
        if is_incremental {
            self.perform_incremental_replication(&job).await?;
        } else {
            self.perform_full_replication(&job).await?;
        }

        // Update state to finalizing
        self.update_state(&job.id, |state| {
            state.status = ReplicationStatus::Finalizing;
            state.progress_percent = 90;
        })
        .await;

        // Apply retention policy on target
        self.apply_retention_policy(&job).await?;

        // Update state to completed
        self.update_state(&job.id, |state| {
            state.status = ReplicationStatus::Completed;
            state.progress_percent = 100;
        })
        .await;

        // Update job last_run and next_run
        let mut jobs = self.jobs.write().await;
        if let Some(job_mut) = jobs.get_mut(&job.id) {
            let now = chrono::Utc::now().timestamp();
            job_mut.last_run = Some(now);
            job_mut.next_run = Self::calculate_next_run(&job_mut.schedule, now);
            if let Err(e) =
                config_store::put_opt(&self.store, kinds::REPLICATION_JOB, &job_mut.id, job_mut)
                    .await
            {
                warn!(
                    "Failed to persist replication job {} run time: {}",
                    job.id, e
                );
            }
        }

        info!("Replication completed: {}", job.name);
        Ok(())
    }

    /// Check if a previous snapshot exists for incremental replication
    async fn check_previous_snapshot(&self, job: &ReplicationJob) -> Result<bool> {
        // Query target node for existing snapshots
        let output = Command::new("ssh")
            .arg(&job.target_node)
            .arg("zfs")
            .arg("list")
            .arg("-H")
            .arg("-t")
            .arg("snapshot")
            .arg("-o")
            .arg("name")
            .arg(format!("{}/{}", job.target_pool, job.source_vm_id))
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to check snapshots on target: {}", e))
            })?;

        Ok(output.status.success() && !output.stdout.is_empty())
    }

    /// Perform full replication (initial sync)
    async fn perform_full_replication(&self, job: &ReplicationJob) -> Result<()> {
        info!("Performing full replication for {}", job.source_vm_id);

        let source_path = format!("{}@{}", job.source_vm_id, job.source_snapshot);
        let target_path = format!("{}/{}", job.target_pool, job.source_vm_id);

        // Build ZFS send command
        let mut send_cmd = Command::new("zfs");
        send_cmd.arg("send").arg(&source_path);

        // Build SSH receive command with bandwidth limit
        let recv_args = vec!["zfs", "receive", "-F", &target_path];

        let bandwidth_limit = if let Some(limit) = job.bandwidth_limit_mbps {
            format!("{}m", limit)
        } else {
            String::new()
        };

        let mut ssh_cmd = if !bandwidth_limit.is_empty() {
            // Use pv for bandwidth throttling
            let mut cmd = Command::new("ssh");
            cmd.arg(&job.target_node).arg(format!(
                "pv -L {} | zfs receive -F {}",
                bandwidth_limit, target_path
            ));
            cmd
        } else {
            let mut cmd = Command::new("ssh");
            cmd.arg(&job.target_node).args(&recv_args);
            cmd
        };

        // Pipe send to receive
        let mut send_child = send_cmd.stdout(Stdio::piped()).spawn().map_err(|e| {
            horcrux_common::Error::System(format!("Failed to start zfs send: {}", e))
        })?;

        let mut send_stdout = send_child.stdout.take().ok_or_else(|| {
            horcrux_common::Error::System("Failed to capture send stdout".to_string())
        })?;

        let mut ssh_child = ssh_cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to start ssh receive: {}", e))
            })?;

        let mut ssh_stdin = ssh_child.stdin.take().ok_or_else(|| {
            horcrux_common::Error::System("Failed to capture ssh stdin".to_string())
        })?;

        // Copy data from send to ssh
        tokio::io::copy(&mut send_stdout, &mut ssh_stdin)
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to copy data: {}", e)))?;

        // Close stdin to signal completion
        drop(ssh_stdin);

        // Wait for both processes
        let send_status = send_child.wait().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to wait for send: {}", e))
        })?;

        let output = ssh_child
            .wait_with_output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to wait for ssh: {}", e)))?;

        if !send_status.success() {
            return Err(horcrux_common::Error::System("ZFS send failed".to_string()));
        }

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Replication failed: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Perform incremental replication
    async fn perform_incremental_replication(&self, job: &ReplicationJob) -> Result<()> {
        info!(
            "Performing incremental replication for {}",
            job.source_vm_id
        );

        // Get the last replicated snapshot on target
        let last_snapshot = self.get_last_replicated_snapshot(job).await?;

        let source_path = format!("{}@{}", job.source_vm_id, job.source_snapshot);
        let incremental_from = format!("{}@{}", job.source_vm_id, last_snapshot);
        let target_path = format!("{}/{}", job.target_pool, job.source_vm_id);

        // Build incremental ZFS send command
        let mut send_cmd = Command::new("zfs");
        send_cmd
            .arg("send")
            .arg("-i")
            .arg(&incremental_from)
            .arg(&source_path);

        // Build SSH receive command
        let mut ssh_cmd = Command::new("ssh");
        ssh_cmd
            .arg(&job.target_node)
            .arg("zfs")
            .arg("receive")
            .arg("-F")
            .arg(&target_path);

        // Execute
        let mut send_child = send_cmd.stdout(Stdio::piped()).spawn().map_err(|e| {
            horcrux_common::Error::System(format!("Failed to start incremental zfs send: {}", e))
        })?;

        let mut send_stdout = send_child.stdout.take().ok_or_else(|| {
            horcrux_common::Error::System("Failed to capture send stdout".to_string())
        })?;

        let mut ssh_child = ssh_cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to start ssh receive: {}", e))
            })?;

        let mut ssh_stdin = ssh_child.stdin.take().ok_or_else(|| {
            horcrux_common::Error::System("Failed to capture ssh stdin".to_string())
        })?;

        // Copy data from send to ssh
        tokio::io::copy(&mut send_stdout, &mut ssh_stdin)
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to copy data: {}", e)))?;

        // Close stdin to signal completion
        drop(ssh_stdin);

        // Wait for both processes
        let send_status = send_child.wait().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to wait for send: {}", e))
        })?;

        let output = ssh_child
            .wait_with_output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to wait for ssh: {}", e)))?;

        if !send_status.success() {
            return Err(horcrux_common::Error::System(
                "Incremental ZFS send failed".to_string(),
            ));
        }

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Incremental replication failed: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Get the last replicated snapshot name
    async fn get_last_replicated_snapshot(&self, job: &ReplicationJob) -> Result<String> {
        let output = Command::new("ssh")
            .arg(&job.target_node)
            .arg("zfs")
            .arg("list")
            .arg("-H")
            .arg("-t")
            .arg("snapshot")
            .arg("-o")
            .arg("name")
            .arg("-s")
            .arg("creation")
            .arg(format!("{}/{}", job.target_pool, job.source_vm_id))
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to get snapshots on target: {}", e))
            })?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(
                "Failed to query target snapshots".to_string(),
            ));
        }

        let snapshots = String::from_utf8_lossy(&output.stdout);
        let last = snapshots.lines().last().ok_or_else(|| {
            horcrux_common::Error::System("No snapshots found on target".to_string())
        })?;

        // Extract snapshot name from "pool/dataset@snapshot" format
        let snapshot_name = last
            .split('@')
            .nth(1)
            .ok_or_else(|| horcrux_common::Error::System("Invalid snapshot format".to_string()))?;

        Ok(snapshot_name.to_string())
    }

    /// Apply retention policy to replicated snapshots
    async fn apply_retention_policy(&self, job: &ReplicationJob) -> Result<()> {
        info!("Applying retention policy (keep {})", job.retention_count);

        // Get all snapshots on target
        let output = Command::new("ssh")
            .arg(&job.target_node)
            .arg("zfs")
            .arg("list")
            .arg("-H")
            .arg("-t")
            .arg("snapshot")
            .arg("-o")
            .arg("name")
            .arg("-s")
            .arg("creation")
            .arg(format!("{}/{}", job.target_pool, job.source_vm_id))
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!(
                    "Failed to list snapshots for retention: {}",
                    e
                ))
            })?;

        if !output.status.success() {
            return Ok(()); // No snapshots to clean up
        }

        let snapshots: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|s| s.to_string())
            .collect();

        // Delete old snapshots beyond retention count
        if snapshots.len() > job.retention_count as usize {
            let to_delete = &snapshots[..snapshots.len() - job.retention_count as usize];

            for snapshot in to_delete {
                info!("Deleting old snapshot: {}", snapshot);

                let delete_output = Command::new("ssh")
                    .arg(&job.target_node)
                    .arg("zfs")
                    .arg("destroy")
                    .arg(snapshot)
                    .output()
                    .await;

                if let Err(e) = delete_output {
                    warn!("Failed to delete snapshot {}: {}", snapshot, e);
                }
            }
        }

        Ok(())
    }

    /// Update replication state
    async fn update_state<F>(&self, job_id: &str, updater: F)
    where
        F: FnOnce(&mut ReplicationState),
    {
        let mut active = self.active_replications.write().await;
        if let Some(state) = active.get_mut(job_id) {
            updater(state);
        }
    }

    /// Get replication state
    pub async fn get_state(&self, job_id: &str) -> Option<ReplicationState> {
        let active = self.active_replications.read().await;
        active.get(job_id).cloned()
    }

    /// Calculate next run time
    fn calculate_next_run(schedule: &ReplicationSchedule, from: i64) -> i64 {
        match schedule {
            ReplicationSchedule::Hourly => from + 3600,
            ReplicationSchedule::Daily { hour } => {
                let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(from, 0)
                    .unwrap_or_else(chrono::Utc::now);
                let mut next = dt
                    .date_naive()
                    .and_hms_opt(*hour as u32, 0, 0)
                    .unwrap_or(dt.naive_utc());
                if next <= dt.naive_utc() {
                    next = (dt + chrono::Duration::days(1))
                        .date_naive()
                        .and_hms_opt(*hour as u32, 0, 0)
                        .unwrap_or(dt.naive_utc());
                }
                chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(next, chrono::Utc)
                    .timestamp()
            }
            ReplicationSchedule::Weekly {
                day: _day,
                hour: _hour,
            } => {
                // Similar to daily but weekly (day and hour will be used in proper scheduling implementation)
                from + (7 * 24 * 3600)
            }
            ReplicationSchedule::Manual => i64::MAX, // Never auto-run
        }
    }
}

/// True if `name` is an executable file in one of the `PATH` directories.
fn binary_in_path(name: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|dir| {
                std::fs::metadata(dir.join(name))
                    .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

// Make Clone available for Arc wrapping
impl Clone for ReplicationManager {
    fn clone(&self) -> Self {
        Self {
            jobs: Arc::clone(&self.jobs),
            active_replications: Arc::clone(&self.active_replications),
            store: self.store.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_replication_job() {
        let manager = ReplicationManager::new();

        let job = ReplicationJob {
            id: "rep-1".to_string(),
            name: "VM 100 to Node2".to_string(),
            source_vm_id: "100".to_string(),
            source_snapshot: "daily-20251010".to_string(),
            target_node: "node2".to_string(),
            target_pool: "tank/replicas".to_string(),
            schedule: ReplicationSchedule::Daily { hour: 2 },
            bandwidth_limit_mbps: Some(100),
            retention_count: 7,
            enabled: true,
            last_run: None,
            next_run: 0,
            created_at: 0,
            last_status: None,
            last_error: None,
        };

        let result = manager.create_job(job).await;
        assert!(result.is_ok());

        let jobs = manager.list_jobs().await;
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, "rep-1");
    }

    #[test]
    fn test_calculate_next_run() {
        let now = chrono::Utc::now().timestamp();

        // Hourly should be 1 hour from now
        let next = ReplicationManager::calculate_next_run(&ReplicationSchedule::Hourly, now);
        assert!(next > now);
        assert!(next <= now + 3601);

        // Manual should never auto-run
        let next = ReplicationManager::calculate_next_run(&ReplicationSchedule::Manual, now);
        assert_eq!(next, i64::MAX);
    }

    fn test_job(id: &str) -> ReplicationJob {
        ReplicationJob {
            id: id.to_string(),
            name: id.to_string(),
            source_vm_id: "100".to_string(),
            source_snapshot: "snap".to_string(),
            target_node: "node2".to_string(),
            target_pool: "tank".to_string(),
            schedule: ReplicationSchedule::Manual,
            bandwidth_limit_mbps: None,
            retention_count: 7,
            enabled: true,
            last_run: None,
            next_run: 0,
            created_at: 0,
            last_status: None,
            last_error: None,
        }
    }

    #[tokio::test]
    async fn failed_run_is_persisted_on_the_job_and_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}", dir.path().join("repl.db").display());
        let db = Arc::new(Database::new(&url).await.unwrap());
        db.migrate().await.unwrap();
        let mgr = ReplicationManager::with_database(db.clone()).await.unwrap();
        mgr.create_job(test_job("rep-fail")).await.unwrap();

        mgr.record_outcome("rep-fail", Some("running"), None).await;
        mgr.run_and_record("rep-fail", || async {
            Err(horcrux_common::Error::System(
                "Failed to start zfs send: No such file or directory".to_string(),
            ))
        })
        .await;

        let job = mgr.get_job("rep-fail").await.unwrap();
        assert_eq!(job.last_status.as_deref(), Some("failed"));
        assert!(job.last_error.as_deref().unwrap().contains("zfs send"));

        // durable: a new manager on the same database sees the failure
        let reloaded = ReplicationManager::with_database(db.clone()).await.unwrap();
        let job = reloaded.get_job("rep-fail").await.unwrap();
        assert_eq!(job.last_status.as_deref(), Some("failed"));
        assert!(job.last_error.as_deref().unwrap().contains("zfs send"));

        // a later success clears the error
        reloaded
            .run_and_record("rep-fail", || async { Ok(()) })
            .await;
        let job = reloaded.get_job("rep-fail").await.unwrap();
        assert_eq!(job.last_status.as_deref(), Some("ok"));
        assert!(job.last_error.is_none());

        // a run left "running" by a restart is reported as failed on load
        reloaded
            .record_outcome("rep-fail", Some("running"), None)
            .await;
        let after_restart = ReplicationManager::with_database(db).await.unwrap();
        let job = after_restart.get_job("rep-fail").await.unwrap();
        assert_eq!(job.last_status.as_deref(), Some("failed"));
        assert!(job.last_error.as_deref().unwrap().contains("interrupted"));
    }

    #[tokio::test]
    async fn unknown_job_is_not_found_and_bad_target_is_rejected() {
        let mgr = ReplicationManager::new();
        assert!(matches!(
            mgr.execute_replication("nope").await,
            Err(horcrux_common::Error::NotFound(_))
        ));
        assert!(matches!(
            mgr.delete_job("nope").await,
            Err(horcrux_common::Error::NotFound(_))
        ));
        let mut job = test_job("bad-target");
        job.target_node = "-oProxyCommand=x".to_string();
        assert!(matches!(
            ReplicationManager::check_preconditions(&job),
            Err(horcrux_common::Error::InvalidConfig(_))
        ));
    }
}
