///! Snapshot Replication Module
///!
///! Provides cross-node snapshot replication using:
///! - ZFS send/receive for incremental transfers
///! - SSH tunneling for secure transfer
///! - Bandwidth throttling
///! - Progress tracking
///! - Automatic cleanup of old replicas

use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::AsyncBufReadExt;
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
}

impl ReplicationManager {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            active_replications: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a new replication job
    pub async fn create_job(&self, mut job: ReplicationJob) -> Result<ReplicationJob> {
        let now = chrono::Utc::now().timestamp();
        job.created_at = now;
        job.next_run = Self::calculate_next_run(&job.schedule, now);

        let mut jobs = self.jobs.write().await;

        if jobs.contains_key(&job.id) {
            return Err(horcrux_common::Error::InvalidConfig(
                format!("Replication job {} already exists", job.id)
            ));
        }

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

        if jobs.remove(job_id).is_none() {
            return Err(horcrux_common::Error::System(
                format!("Replication job {} not found", job_id)
            ));
        }

        info!("Deleted replication job: {}", job_id);
        Ok(())
    }

    /// Execute a replication job
    pub async fn execute_replication(&self, job_id: &str) -> Result<ReplicationState> {
        let job = self.get_job(job_id).await
            .ok_or_else(|| horcrux_common::Error::System(
                format!("Replication job {} not found", job_id)
            ))?;

        if !job.enabled {
            return Err(horcrux_common::Error::InvalidConfig(
                format!("Replication job {} is disabled", job_id)
            ));
        }

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

        let mut active = self.active_replications.write().await;
        active.insert(job.id.clone(), state.clone());
        drop(active);

        // Execute replication in background
        let manager = Arc::new(self.clone());
        let job_clone = job.clone();
        let job_id_clone = job_id.to_string();

        tokio::spawn(async move {
            if let Err(e) = manager.perform_replication(job_clone).await {
                error!("Replication failed for job {}: {}", job_id_clone, e);

                let mut active = manager.active_replications.write().await;
                if let Some(state) = active.get_mut(&job_id_clone) {
                    state.status = ReplicationStatus::Failed;
                    state.error = Some(e.to_string());
                }
            }
        });

        Ok(state)
    }

    /// Perform the actual replication
    async fn perform_replication(&self, job: ReplicationJob) -> Result<()> {
        info!("Starting replication: {} -> {}", job.source_vm_id, job.target_node);

        // Update state to transferring
        self.update_state(&job.id, |state| {
            state.status = ReplicationStatus::Transferring;
            state.progress_percent = 10;
        }).await;

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
        }).await;

        // Apply retention policy on target
        self.apply_retention_policy(&job).await?;

        // Update state to completed
        self.update_state(&job.id, |state| {
            state.status = ReplicationStatus::Completed;
            state.progress_percent = 100;
        }).await;

        // Update job last_run and next_run
        let mut jobs = self.jobs.write().await;
        if let Some(job_mut) = jobs.get_mut(&job.id) {
            let now = chrono::Utc::now().timestamp();
            job_mut.last_run = Some(now);
            job_mut.next_run = Self::calculate_next_run(&job_mut.schedule, now);
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
            .arg(&format!("{}/{}", job.target_pool, job.source_vm_id))
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(
                format!("Failed to check snapshots on target: {}", e)
            ))?;

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
        let mut recv_args = vec!["zfs", "receive", "-F", &target_path];

        let bandwidth_limit = if let Some(limit) = job.bandwidth_limit_mbps {
            format!("{}m", limit)
        } else {
            String::new()
        };

        let mut ssh_cmd = if !bandwidth_limit.is_empty() {
            // Use pv for bandwidth throttling
            let mut cmd = Command::new("ssh");
            cmd.arg(&job.target_node)
                .arg(&format!("pv -L {} | zfs receive -F {}", bandwidth_limit, target_path));
            cmd
        } else {
            let mut cmd = Command::new("ssh");
            cmd.arg(&job.target_node)
                .args(&recv_args);
            cmd
        };

        // Pipe send to receive
        let mut send_child = send_cmd
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| horcrux_common::Error::System(
                format!("Failed to start zfs send: {}", e)
            ))?;

        let mut send_stdout = send_child.stdout.take()
            .ok_or_else(|| horcrux_common::Error::System("Failed to capture send stdout".to_string()))?;

        let mut ssh_child = ssh_cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| horcrux_common::Error::System(
                format!("Failed to start ssh receive: {}", e)
            ))?;

        let mut ssh_stdin = ssh_child.stdin.take()
            .ok_or_else(|| horcrux_common::Error::System("Failed to capture ssh stdin".to_string()))?;

        // Copy data from send to ssh
        tokio::io::copy(&mut send_stdout, &mut ssh_stdin).await
            .map_err(|e| horcrux_common::Error::System(
                format!("Failed to copy data: {}", e)
            ))?;

        // Close stdin to signal completion
        drop(ssh_stdin);

        // Wait for both processes
        let send_status = send_child.wait().await
            .map_err(|e| horcrux_common::Error::System(
                format!("Failed to wait for send: {}", e)
            ))?;

        let output = ssh_child.wait_with_output().await
            .map_err(|e| horcrux_common::Error::System(
                format!("Failed to wait for ssh: {}", e)
            ))?;

        if !send_status.success() {
            return Err(horcrux_common::Error::System(
                "ZFS send failed".to_string()
            ));
        }

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(
                format!("Replication failed: {}", stderr)
            ));
        }

        Ok(())
    }

    /// Perform incremental replication
    async fn perform_incremental_replication(&self, job: &ReplicationJob) -> Result<()> {
        info!("Performing incremental replication for {}", job.source_vm_id);

        // Get the last replicated snapshot on target
        let last_snapshot = self.get_last_replicated_snapshot(job).await?;

        let source_path = format!("{}@{}", job.source_vm_id, job.source_snapshot);
        let incremental_from = format!("{}@{}", job.source_vm_id, last_snapshot);
        let target_path = format!("{}/{}", job.target_pool, job.source_vm_id);

        // Build incremental ZFS send command
        let mut send_cmd = Command::new("zfs");
        send_cmd.arg("send")
            .arg("-i")
            .arg(&incremental_from)
            .arg(&source_path);

        // Build SSH receive command
        let mut ssh_cmd = Command::new("ssh");
        ssh_cmd.arg(&job.target_node)
            .arg("zfs")
            .arg("receive")
            .arg("-F")
            .arg(&target_path);

        // Execute
        let mut send_child = send_cmd
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| horcrux_common::Error::System(
                format!("Failed to start incremental zfs send: {}", e)
            ))?;

        let mut send_stdout = send_child.stdout.take()
            .ok_or_else(|| horcrux_common::Error::System("Failed to capture send stdout".to_string()))?;

        let mut ssh_child = ssh_cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| horcrux_common::Error::System(
                format!("Failed to start ssh receive: {}", e)
            ))?;

        let mut ssh_stdin = ssh_child.stdin.take()
            .ok_or_else(|| horcrux_common::Error::System("Failed to capture ssh stdin".to_string()))?;

        // Copy data from send to ssh
        tokio::io::copy(&mut send_stdout, &mut ssh_stdin).await
            .map_err(|e| horcrux_common::Error::System(
                format!("Failed to copy data: {}", e)
            ))?;

        // Close stdin to signal completion
        drop(ssh_stdin);

        // Wait for both processes
        let send_status = send_child.wait().await
            .map_err(|e| horcrux_common::Error::System(
                format!("Failed to wait for send: {}", e)
            ))?;

        let output = ssh_child.wait_with_output().await
            .map_err(|e| horcrux_common::Error::System(
                format!("Failed to wait for ssh: {}", e)
            ))?;

        if !send_status.success() {
            return Err(horcrux_common::Error::System(
                "Incremental ZFS send failed".to_string()
            ));
        }

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(
                format!("Incremental replication failed: {}", stderr)
            ));
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
            .arg(&format!("{}/{}", job.target_pool, job.source_vm_id))
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(
                format!("Failed to get snapshots on target: {}", e)
            ))?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(
                "Failed to query target snapshots".to_string()
            ));
        }

        let snapshots = String::from_utf8_lossy(&output.stdout);
        let last = snapshots.lines().last()
            .ok_or_else(|| horcrux_common::Error::System(
                "No snapshots found on target".to_string()
            ))?;

        // Extract snapshot name from "pool/dataset@snapshot" format
        let snapshot_name = last.split('@').nth(1)
            .ok_or_else(|| horcrux_common::Error::System(
                "Invalid snapshot format".to_string()
            ))?;

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
            .arg(&format!("{}/{}", job.target_pool, job.source_vm_id))
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(
                format!("Failed to list snapshots for retention: {}", e)
            ))?;

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
                    .unwrap_or_else(|| chrono::Utc::now());
                let mut next = dt.date_naive().and_hms_opt(*hour as u32, 0, 0)
                    .unwrap_or(dt.naive_utc());
                if next <= dt.naive_utc() {
                    next = (dt + chrono::Duration::days(1))
                        .date_naive()
                        .and_hms_opt(*hour as u32, 0, 0)
                        .unwrap_or(dt.naive_utc());
                }
                chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(next, chrono::Utc).timestamp()
            }
            ReplicationSchedule::Weekly { day, hour } => {
                // Similar to daily but weekly
                from + (7 * 24 * 3600)
            }
            ReplicationSchedule::Manual => i64::MAX, // Never auto-run
        }
    }
}

// Make Clone available for Arc wrapping
impl Clone for ReplicationManager {
    fn clone(&self) -> Self {
        Self {
            jobs: Arc::clone(&self.jobs),
            active_replications: Arc::clone(&self.active_replications),
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
}
