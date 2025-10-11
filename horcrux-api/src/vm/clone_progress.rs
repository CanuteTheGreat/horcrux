///! Clone Progress Tracking and Cancellation
///!
///! Provides real-time progress tracking for VM cloning operations
///! with support for cancellation of in-progress clones

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use uuid::Uuid;

use horcrux_common::Result;

/// Clone job state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CloneJobState {
    /// Job is queued but not yet started
    Queued,
    /// Job is currently running
    Running,
    /// Job completed successfully
    Completed,
    /// Job failed with an error
    Failed,
    /// Job was cancelled by user
    Cancelled,
}

/// Clone operation stage
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CloneStage {
    /// Preparing clone operation
    Preparing,
    /// Cloning disk image
    CloningDisk,
    /// Generating new MAC addresses
    GeneratingMacs,
    /// Applying network configuration
    ConfiguringNetwork,
    /// Creating cloud-init configuration
    CreatingCloudInit,
    /// Finalizing clone
    Finalizing,
}

/// Clone job information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloneJob {
    /// Unique job ID
    pub id: String,
    /// Source VM ID
    pub source_vm_id: String,
    /// Target VM ID (clone)
    pub target_vm_id: String,
    /// Target VM name
    pub target_vm_name: String,
    /// Current job state
    pub state: CloneJobState,
    /// Current stage of cloning
    pub stage: Option<CloneStage>,
    /// Progress percentage (0-100)
    pub progress: u8,
    /// Estimated total size in bytes
    pub total_size_bytes: Option<u64>,
    /// Bytes copied so far
    pub copied_bytes: Option<u64>,
    /// Job creation timestamp
    pub created_at: DateTime<Utc>,
    /// Job start timestamp
    pub started_at: Option<DateTime<Utc>>,
    /// Job completion timestamp
    pub completed_at: Option<DateTime<Utc>>,
    /// Error message if failed
    pub error_message: Option<String>,
    /// Whether cancellation was requested
    pub cancellation_requested: bool,
}

impl CloneJob {
    /// Create a new clone job
    pub fn new(source_vm_id: String, target_vm_id: String, target_vm_name: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            source_vm_id,
            target_vm_id,
            target_vm_name,
            state: CloneJobState::Queued,
            stage: None,
            progress: 0,
            total_size_bytes: None,
            copied_bytes: None,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            error_message: None,
            cancellation_requested: false,
        }
    }

    /// Mark job as started
    pub fn start(&mut self) {
        self.state = CloneJobState::Running;
        self.started_at = Some(Utc::now());
    }

    /// Update progress
    pub fn update_progress(&mut self, stage: CloneStage, progress: u8) {
        self.stage = Some(stage);
        self.progress = progress.min(100);
    }

    /// Update bytes copied
    pub fn update_bytes(&mut self, total: u64, copied: u64) {
        self.total_size_bytes = Some(total);
        self.copied_bytes = Some(copied);

        // Update progress percentage based on bytes
        if total > 0 {
            self.progress = ((copied as f64 / total as f64) * 100.0) as u8;
        }
    }

    /// Mark job as completed
    pub fn complete(&mut self) {
        self.state = CloneJobState::Completed;
        self.progress = 100;
        self.completed_at = Some(Utc::now());
    }

    /// Mark job as failed
    pub fn fail(&mut self, error: String) {
        self.state = CloneJobState::Failed;
        self.error_message = Some(error);
        self.completed_at = Some(Utc::now());
    }

    /// Mark job as cancelled
    pub fn cancel(&mut self) {
        self.state = CloneJobState::Cancelled;
        self.completed_at = Some(Utc::now());
    }

    /// Request cancellation
    pub fn request_cancellation(&mut self) {
        self.cancellation_requested = true;
    }

    /// Check if cancellation was requested
    pub fn is_cancellation_requested(&self) -> bool {
        self.cancellation_requested
    }

    /// Get elapsed time in seconds
    pub fn elapsed_seconds(&self) -> Option<i64> {
        self.started_at.map(|start| {
            let end = self.completed_at.unwrap_or_else(|| Utc::now());
            (end - start).num_seconds()
        })
    }

    /// Get estimated time remaining in seconds
    pub fn estimated_remaining_seconds(&self) -> Option<i64> {
        if self.progress == 0 || self.progress == 100 {
            return None;
        }

        self.elapsed_seconds().map(|elapsed| {
            let total_estimated = (elapsed as f64 / (self.progress as f64 / 100.0)) as i64;
            total_estimated - elapsed
        })
    }
}

/// Clone job manager
pub struct CloneJobManager {
    jobs: Arc<RwLock<HashMap<String, CloneJob>>>,
    _max_completed_jobs: usize,  // Reserved for configurable job history limit
}

impl CloneJobManager {
    /// Create a new clone job manager
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            _max_completed_jobs: 100, // Keep last 100 completed jobs
        }
    }

    /// Create a new clone job
    pub async fn create_job(
        &self,
        source_vm_id: String,
        target_vm_id: String,
        target_vm_name: String,
    ) -> String {
        let job = CloneJob::new(source_vm_id, target_vm_id, target_vm_name);
        let job_id = job.id.clone();

        let mut jobs = self.jobs.write().await;
        jobs.insert(job_id.clone(), job);

        info!("Created clone job: {}", job_id);
        job_id
    }

    /// Get a clone job by ID
    pub async fn get_job(&self, job_id: &str) -> Option<CloneJob> {
        let jobs = self.jobs.read().await;
        jobs.get(job_id).cloned()
    }

    /// List all clone jobs
    pub async fn list_jobs(&self) -> Vec<CloneJob> {
        let jobs = self.jobs.read().await;
        jobs.values().cloned().collect()
    }

    /// List active (queued or running) jobs
    pub async fn list_active_jobs(&self) -> Vec<CloneJob> {
        let jobs = self.jobs.read().await;
        jobs.values()
            .filter(|job| {
                matches!(job.state, CloneJobState::Queued | CloneJobState::Running)
            })
            .cloned()
            .collect()
    }

    /// List completed jobs
    pub async fn list_completed_jobs(&self) -> Vec<CloneJob> {
        let jobs = self.jobs.read().await;
        jobs.values()
            .filter(|job| {
                matches!(
                    job.state,
                    CloneJobState::Completed | CloneJobState::Failed | CloneJobState::Cancelled
                )
            })
            .cloned()
            .collect()
    }

    /// Start a job
    pub async fn start_job(&self, job_id: &str) -> Result<()> {
        let mut jobs = self.jobs.write().await;

        if let Some(job) = jobs.get_mut(job_id) {
            job.start();
            info!("Started clone job: {}", job_id);
            Ok(())
        } else {
            Err(horcrux_common::Error::System(format!(
                "Clone job {} not found",
                job_id
            )))
        }
    }

    /// Update job progress
    pub async fn update_progress(
        &self,
        job_id: &str,
        stage: CloneStage,
        progress: u8,
    ) -> Result<()> {
        let mut jobs = self.jobs.write().await;

        if let Some(job) = jobs.get_mut(job_id) {
            debug!(
                "Updated clone job {} progress: {:?} {}%",
                job_id, stage, progress
            );
            job.update_progress(stage, progress);
            Ok(())
        } else {
            Err(horcrux_common::Error::System(format!(
                "Clone job {} not found",
                job_id
            )))
        }
    }

    /// Update bytes copied
    pub async fn update_bytes(
        &self,
        job_id: &str,
        total_bytes: u64,
        copied_bytes: u64,
    ) -> Result<()> {
        let mut jobs = self.jobs.write().await;

        if let Some(job) = jobs.get_mut(job_id) {
            job.update_bytes(total_bytes, copied_bytes);
            debug!(
                "Updated clone job {} bytes: {}/{} ({}%)",
                job_id, copied_bytes, total_bytes, job.progress
            );
            Ok(())
        } else {
            Err(horcrux_common::Error::System(format!(
                "Clone job {} not found",
                job_id
            )))
        }
    }

    /// Complete a job
    pub async fn complete_job(&self, job_id: &str) -> Result<()> {
        let mut jobs = self.jobs.write().await;

        if let Some(job) = jobs.get_mut(job_id) {
            job.complete();
            info!("Completed clone job: {}", job_id);

            // Clean up old completed jobs
            self.cleanup_old_jobs_internal(&mut jobs).await;

            Ok(())
        } else {
            Err(horcrux_common::Error::System(format!(
                "Clone job {} not found",
                job_id
            )))
        }
    }

    /// Fail a job
    pub async fn fail_job(&self, job_id: &str, error: String) -> Result<()> {
        let mut jobs = self.jobs.write().await;

        if let Some(job) = jobs.get_mut(job_id) {
            job.fail(error.clone());
            warn!("Failed clone job {}: {}", job_id, error);

            // Clean up old completed jobs
            self.cleanup_old_jobs_internal(&mut jobs).await;

            Ok(())
        } else {
            Err(horcrux_common::Error::System(format!(
                "Clone job {} not found",
                job_id
            )))
        }
    }

    /// Request cancellation of a job
    pub async fn request_cancellation(&self, job_id: &str) -> Result<()> {
        let mut jobs = self.jobs.write().await;

        if let Some(job) = jobs.get_mut(job_id) {
            if job.state == CloneJobState::Running || job.state == CloneJobState::Queued {
                job.request_cancellation();
                info!("Requested cancellation of clone job: {}", job_id);
                Ok(())
            } else {
                Err(horcrux_common::Error::System(format!(
                    "Cannot cancel job {} in state {:?}",
                    job_id, job.state
                )))
            }
        } else {
            Err(horcrux_common::Error::System(format!(
                "Clone job {} not found",
                job_id
            )))
        }
    }

    /// Cancel a job (called by the clone operation when it detects cancellation request)
    pub async fn cancel_job(&self, job_id: &str) -> Result<()> {
        let mut jobs = self.jobs.write().await;

        if let Some(job) = jobs.get_mut(job_id) {
            job.cancel();
            info!("Cancelled clone job: {}", job_id);

            // Clean up old completed jobs
            self.cleanup_old_jobs_internal(&mut jobs).await;

            Ok(())
        } else {
            Err(horcrux_common::Error::System(format!(
                "Clone job {} not found",
                job_id
            )))
        }
    }

    /// Check if a job should be cancelled
    pub async fn should_cancel(&self, job_id: &str) -> bool {
        let jobs = self.jobs.read().await;
        jobs.get(job_id)
            .map(|job| job.is_cancellation_requested())
            .unwrap_or(false)
    }

    /// Delete a job
    pub async fn delete_job(&self, job_id: &str) -> Result<()> {
        let mut jobs = self.jobs.write().await;

        if let Some(job) = jobs.get(job_id) {
            // Only allow deleting completed/failed/cancelled jobs
            if matches!(
                job.state,
                CloneJobState::Completed | CloneJobState::Failed | CloneJobState::Cancelled
            ) {
                jobs.remove(job_id);
                info!("Deleted clone job: {}", job_id);
                Ok(())
            } else {
                Err(horcrux_common::Error::System(format!(
                    "Cannot delete active job {}",
                    job_id
                )))
            }
        } else {
            Err(horcrux_common::Error::System(format!(
                "Clone job {} not found",
                job_id
            )))
        }
    }

    /// Clean up old completed jobs (internal, requires write lock)
    async fn cleanup_old_jobs_internal(
        &self,
        jobs: &mut HashMap<String, CloneJob>,
    ) {
        let mut completed_jobs: Vec<_> = jobs
            .values()
            .filter(|job| {
                matches!(
                    job.state,
                    CloneJobState::Completed | CloneJobState::Failed | CloneJobState::Cancelled
                )
            })
            .cloned()
            .collect();

        if completed_jobs.len() > self.max_completed_jobs {
            // Sort by completion time (oldest first)
            completed_jobs.sort_by_key(|job| job.completed_at);

            // Remove oldest jobs
            let to_remove = completed_jobs.len() - self.max_completed_jobs;
            for job in completed_jobs.iter().take(to_remove) {
                jobs.remove(&job.id);
                debug!("Cleaned up old clone job: {}", job.id);
            }
        }
    }

    /// Clean up old completed jobs (public API)
    pub async fn cleanup_old_jobs(&self) {
        let mut jobs = self.jobs.write().await;
        self.cleanup_old_jobs_internal(&mut jobs).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_clone_job() {
        let manager = CloneJobManager::new();

        let job_id = manager
            .create_job(
                "vm-100".to_string(),
                "vm-101".to_string(),
                "cloned-vm".to_string(),
            )
            .await;

        let job = manager.get_job(&job_id).await.unwrap();
        assert_eq!(job.source_vm_id, "vm-100");
        assert_eq!(job.target_vm_id, "vm-101");
        assert_eq!(job.state, CloneJobState::Queued);
        assert_eq!(job.progress, 0);
    }

    #[tokio::test]
    async fn test_job_lifecycle() {
        let manager = CloneJobManager::new();

        let job_id = manager
            .create_job(
                "vm-100".to_string(),
                "vm-101".to_string(),
                "cloned-vm".to_string(),
            )
            .await;

        // Start job
        manager.start_job(&job_id).await.ok();
        let job = manager.get_job(&job_id).await.unwrap();
        assert_eq!(job.state, CloneJobState::Running);
        assert!(job.started_at.is_some());

        // Update progress
        manager
            .update_progress(&job_id, CloneStage::CloningDisk, 50)
            .await
            .ok();
        let job = manager.get_job(&job_id).await.unwrap();
        assert_eq!(job.progress, 50);
        assert_eq!(job.stage, Some(CloneStage::CloningDisk));

        // Complete job
        manager.complete_job(&job_id).await.ok();
        let job = manager.get_job(&job_id).await.unwrap();
        assert_eq!(job.state, CloneJobState::Completed);
        assert_eq!(job.progress, 100);
        assert!(job.completed_at.is_some());
    }

    #[tokio::test]
    async fn test_job_cancellation() {
        let manager = CloneJobManager::new();

        let job_id = manager
            .create_job(
                "vm-100".to_string(),
                "vm-101".to_string(),
                "cloned-vm".to_string(),
            )
            .await;

        manager.start_job(&job_id).await.ok();

        // Request cancellation
        manager.request_cancellation(&job_id).await.ok();
        assert!(manager.should_cancel(&job_id).await);

        // Cancel job
        manager.cancel_job(&job_id).await.ok();
        let job = manager.get_job(&job_id).await.unwrap();
        assert_eq!(job.state, CloneJobState::Cancelled);
    }

    #[tokio::test]
    async fn test_update_bytes() {
        let manager = CloneJobManager::new();

        let job_id = manager
            .create_job(
                "vm-100".to_string(),
                "vm-101".to_string(),
                "cloned-vm".to_string(),
            )
            .await;

        manager.start_job(&job_id).await.ok();

        // Update bytes
        let total_bytes = 10_000_000_000u64; // 10 GB
        let copied_bytes = 5_000_000_000u64; // 5 GB

        manager
            .update_bytes(&job_id, total_bytes, copied_bytes)
            .await
            .ok();

        let job = manager.get_job(&job_id).await.unwrap();
        assert_eq!(job.total_size_bytes, Some(total_bytes));
        assert_eq!(job.copied_bytes, Some(copied_bytes));
        assert_eq!(job.progress, 50);
    }

    #[tokio::test]
    async fn test_list_jobs() {
        let manager = CloneJobManager::new();

        // Create multiple jobs
        let job1 = manager
            .create_job(
                "vm-100".to_string(),
                "vm-101".to_string(),
                "clone1".to_string(),
            )
            .await;

        let job2 = manager
            .create_job(
                "vm-200".to_string(),
                "vm-201".to_string(),
                "clone2".to_string(),
            )
            .await;

        manager.start_job(&job1).await.ok();
        manager.complete_job(&job1).await.ok();

        let active_jobs = manager.list_active_jobs().await;
        assert_eq!(active_jobs.len(), 1);
        assert_eq!(active_jobs[0].id, job2);

        let completed_jobs = manager.list_completed_jobs().await;
        assert_eq!(completed_jobs.len(), 1);
        assert_eq!(completed_jobs[0].id, job1);
    }

    #[tokio::test]
    async fn test_job_elapsed_time() {
        let manager = CloneJobManager::new();

        let job_id = manager
            .create_job(
                "vm-100".to_string(),
                "vm-101".to_string(),
                "cloned-vm".to_string(),
            )
            .await;

        manager.start_job(&job_id).await.ok();

        // Wait a bit
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let job = manager.get_job(&job_id).await.unwrap();
        let elapsed = job.elapsed_seconds();
        assert!(elapsed.is_some());
        assert!(elapsed.unwrap() >= 0);
    }

    #[tokio::test]
    async fn test_estimated_remaining_time() {
        let manager = CloneJobManager::new();

        let job_id = manager
            .create_job(
                "vm-100".to_string(),
                "vm-101".to_string(),
                "cloned-vm".to_string(),
            )
            .await;

        manager.start_job(&job_id).await.ok();

        // Wait and update progress
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        manager
            .update_progress(&job_id, CloneStage::CloningDisk, 25)
            .await
            .ok();

        let job = manager.get_job(&job_id).await.unwrap();
        let remaining = job.estimated_remaining_seconds();
        assert!(remaining.is_some());
    }

    #[tokio::test]
    async fn test_cleanup_old_jobs() {
        let mut manager = CloneJobManager::new();
        manager.max_completed_jobs = 2; // Only keep 2 completed jobs

        // Create and complete 5 jobs
        for i in 0..5 {
            let job_id = manager
                .create_job(
                    format!("vm-{}", i),
                    format!("vm-{}-clone", i),
                    format!("clone-{}", i),
                )
                .await;

            manager.start_job(&job_id).await.ok();
            manager.complete_job(&job_id).await.ok();

            // Small delay to ensure different completion times
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        // Should only have 2 completed jobs left
        let completed_jobs = manager.list_completed_jobs().await;
        assert_eq!(completed_jobs.len(), 2);
    }
}
