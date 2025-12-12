//! Storage Migration Module
//!
//! Provides volume migration capabilities between storage pools including:
//! - Live volume migration (using QEMU block-commit)
//! - Offline volume copy
//! - Cross-backend migration (ZFS -> Ceph, etc.)
//! - Migration progress tracking

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, error, debug};
use horcrux_common::Result;
use chrono::{DateTime, Utc};

use super::StorageType;

/// Migration mode
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MigrationMode {
    /// Live migration using QEMU drive-mirror
    Live,
    /// Offline copy using qemu-img convert
    Offline,
    /// ZFS send/receive for ZFS-to-ZFS
    ZfsSendReceive,
    /// Ceph RBD migration for Ceph-to-Ceph
    CephRbdMigrate,
}

/// Storage migration state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MigrationState {
    /// Migration pending
    Pending,
    /// Pre-migration checks
    Checking,
    /// Copying data
    Copying,
    /// Synchronizing final changes
    Syncing,
    /// Switching to new storage
    Switching,
    /// Cleaning up old storage
    Cleanup,
    /// Migration complete
    Completed,
    /// Migration failed
    Failed(String),
    /// Migration cancelled
    Cancelled,
}

/// Migration job configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageMigrationConfig {
    /// Delete source volume after successful migration
    pub delete_source: bool,
    /// Maximum bandwidth in MB/s (0 = unlimited)
    pub bandwidth_limit: u64,
    /// Target format (qcow2, raw, etc.)
    pub target_format: Option<String>,
    /// Enable compression during transfer
    pub compress: bool,
    /// Enable sparse allocation on target
    pub sparse: bool,
}

impl Default for StorageMigrationConfig {
    fn default() -> Self {
        Self {
            delete_source: false,
            bandwidth_limit: 0,
            target_format: None,
            compress: true,
            sparse: true,
        }
    }
}

/// Storage migration progress
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MigrationProgress {
    /// Total bytes to transfer
    pub total_bytes: u64,
    /// Bytes transferred
    pub transferred_bytes: u64,
    /// Current transfer speed (bytes/sec)
    pub speed: u64,
    /// Estimated time remaining (seconds)
    pub eta_seconds: u64,
    /// Percentage complete
    pub percent: f32,
}

/// Storage migration job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageMigrationJob {
    pub id: String,
    pub volume_name: String,
    pub source_pool: String,
    pub target_pool: String,
    pub source_type: StorageType,
    pub target_type: StorageType,
    pub mode: MigrationMode,
    pub state: MigrationState,
    pub config: StorageMigrationConfig,
    pub progress: MigrationProgress,
    pub vm_id: Option<String>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

/// Storage migration manager
pub struct StorageMigrationManager {
    jobs: Arc<RwLock<HashMap<String, StorageMigrationJob>>>,
    active_migrations: Arc<RwLock<HashMap<String, String>>>, // volume_name -> job_id
}

impl StorageMigrationManager {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            active_migrations: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Determine the best migration mode for given source and target
    pub fn determine_migration_mode(
        source_type: &StorageType,
        target_type: &StorageType,
        is_vm_running: bool,
    ) -> MigrationMode {
        // ZFS to ZFS: use ZFS send/receive
        if *source_type == StorageType::Zfs && *target_type == StorageType::Zfs {
            return MigrationMode::ZfsSendReceive;
        }

        // Ceph to Ceph: use RBD migration
        if *source_type == StorageType::Ceph && *target_type == StorageType::Ceph {
            return MigrationMode::CephRbdMigrate;
        }

        // VM running: use live migration
        if is_vm_running {
            return MigrationMode::Live;
        }

        // Default to offline copy
        MigrationMode::Offline
    }

    /// Start a storage migration
    pub async fn start_migration(
        &self,
        volume_name: String,
        source_pool: String,
        target_pool: String,
        source_type: StorageType,
        target_type: StorageType,
        vm_id: Option<String>,
        config: Option<StorageMigrationConfig>,
    ) -> Result<String> {
        // Check if volume is already being migrated
        let active = self.active_migrations.read().await;
        if active.contains_key(&volume_name) {
            return Err(horcrux_common::Error::System(
                format!("Volume {} is already being migrated", volume_name)
            ));
        }
        drop(active);

        // Determine migration mode
        let mode = Self::determine_migration_mode(&source_type, &target_type, vm_id.is_some());

        // Create job
        let job_id = format!("smig-{}-{}", volume_name, Utc::now().timestamp());
        let job = StorageMigrationJob {
            id: job_id.clone(),
            volume_name: volume_name.clone(),
            source_pool,
            target_pool,
            source_type,
            target_type,
            mode: mode.clone(),
            state: MigrationState::Pending,
            config: config.unwrap_or_default(),
            progress: MigrationProgress::default(),
            vm_id,
            started_at: Utc::now(),
            completed_at: None,
            error: None,
        };

        // Register job
        {
            let mut jobs = self.jobs.write().await;
            jobs.insert(job_id.clone(), job);
        }
        {
            let mut active = self.active_migrations.write().await;
            active.insert(volume_name.clone(), job_id.clone());
        }

        info!(
            job_id = %job_id,
            volume = %volume_name,
            mode = ?mode,
            "Storage migration started"
        );

        // Execute migration
        self.execute_migration(&job_id).await?;

        Ok(job_id)
    }

    /// Execute the migration based on mode
    async fn execute_migration(&self, job_id: &str) -> Result<()> {
        self.update_state(job_id, MigrationState::Checking).await?;

        let job = self.get_job(job_id).await?;

        match job.mode {
            MigrationMode::Live => self.execute_live_migration(&job).await,
            MigrationMode::Offline => self.execute_offline_migration(&job).await,
            MigrationMode::ZfsSendReceive => self.execute_zfs_migration(&job).await,
            MigrationMode::CephRbdMigrate => self.execute_ceph_migration(&job).await,
        }
    }

    /// Execute live migration using QEMU drive-mirror
    async fn execute_live_migration(&self, job: &StorageMigrationJob) -> Result<()> {
        info!(job_id = %job.id, "Executing live storage migration");

        // Phase 1: Start drive-mirror
        self.update_state(&job.id, MigrationState::Copying).await?;

        // In real implementation:
        // 1. QMP: drive-mirror to target
        // 2. Monitor mirror progress
        // 3. When ready, complete the job

        // Simulate copying
        for progress in (0..=100).step_by(10) {
            let mut jobs = self.jobs.write().await;
            if let Some(j) = jobs.get_mut(&job.id) {
                j.progress.percent = progress as f32;
                j.progress.transferred_bytes = (progress as u64) * 1024 * 1024 * 1024 / 100;
            }
            drop(jobs);
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // Phase 2: Sync final changes
        self.update_state(&job.id, MigrationState::Syncing).await?;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Phase 3: Switch to new storage
        self.update_state(&job.id, MigrationState::Switching).await?;

        // QMP: block-job-complete to switch to mirror

        // Phase 4: Cleanup
        self.update_state(&job.id, MigrationState::Cleanup).await?;

        self.complete_migration(&job.id).await?;

        info!(job_id = %job.id, "Live storage migration completed");

        Ok(())
    }

    /// Execute offline migration using qemu-img convert
    async fn execute_offline_migration(&self, job: &StorageMigrationJob) -> Result<()> {
        info!(job_id = %job.id, "Executing offline storage migration");

        self.update_state(&job.id, MigrationState::Copying).await?;

        // In real implementation:
        // qemu-img convert -p -O qcow2 source.img target.img

        // Simulate copying
        for progress in (0..=100).step_by(5) {
            let mut jobs = self.jobs.write().await;
            if let Some(j) = jobs.get_mut(&job.id) {
                j.progress.percent = progress as f32;
            }
            drop(jobs);
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }

        self.update_state(&job.id, MigrationState::Cleanup).await?;

        // Delete source if configured
        if job.config.delete_source {
            debug!(job_id = %job.id, "Deleting source volume");
            // In real implementation: delete source
        }

        self.complete_migration(&job.id).await?;

        info!(job_id = %job.id, "Offline storage migration completed");

        Ok(())
    }

    /// Execute ZFS send/receive migration
    async fn execute_zfs_migration(&self, job: &StorageMigrationJob) -> Result<()> {
        info!(job_id = %job.id, "Executing ZFS send/receive migration");

        self.update_state(&job.id, MigrationState::Copying).await?;

        // In real implementation:
        // 1. Create snapshot of source
        // 2. zfs send source@snap | zfs receive target
        // 3. Optionally sync incrementally
        // 4. Final switch

        // Simulate ZFS transfer
        for progress in (0..=100).step_by(10) {
            let mut jobs = self.jobs.write().await;
            if let Some(j) = jobs.get_mut(&job.id) {
                j.progress.percent = progress as f32;
            }
            drop(jobs);
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        }

        self.update_state(&job.id, MigrationState::Cleanup).await?;
        self.complete_migration(&job.id).await?;

        info!(job_id = %job.id, "ZFS migration completed");

        Ok(())
    }

    /// Execute Ceph RBD migration
    async fn execute_ceph_migration(&self, job: &StorageMigrationJob) -> Result<()> {
        info!(job_id = %job.id, "Executing Ceph RBD migration");

        self.update_state(&job.id, MigrationState::Copying).await?;

        // In real implementation:
        // rbd migration execute pool/image@snap pool/new-image

        // Simulate Ceph migration
        for progress in (0..=100).step_by(10) {
            let mut jobs = self.jobs.write().await;
            if let Some(j) = jobs.get_mut(&job.id) {
                j.progress.percent = progress as f32;
            }
            drop(jobs);
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        }

        self.update_state(&job.id, MigrationState::Cleanup).await?;
        self.complete_migration(&job.id).await?;

        info!(job_id = %job.id, "Ceph RBD migration completed");

        Ok(())
    }

    /// Update migration state
    async fn update_state(&self, job_id: &str, state: MigrationState) -> Result<()> {
        let mut jobs = self.jobs.write().await;
        let job = jobs.get_mut(job_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("Migration job {} not found", job_id))
        })?;

        debug!(job_id = job_id, old_state = ?job.state, new_state = ?state, "Storage migration state change");
        job.state = state;

        Ok(())
    }

    /// Complete a migration
    async fn complete_migration(&self, job_id: &str) -> Result<()> {
        let mut jobs = self.jobs.write().await;
        let job = jobs.get_mut(job_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("Migration job {} not found", job_id))
        })?;

        job.state = MigrationState::Completed;
        job.completed_at = Some(Utc::now());
        job.progress.percent = 100.0;

        let volume_name = job.volume_name.clone();
        drop(jobs);

        // Remove from active migrations
        let mut active = self.active_migrations.write().await;
        active.remove(&volume_name);

        Ok(())
    }

    /// Cancel a migration
    pub async fn cancel_migration(&self, job_id: &str) -> Result<()> {
        let mut jobs = self.jobs.write().await;
        let job = jobs.get_mut(job_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("Migration job {} not found", job_id))
        })?;

        match &job.state {
            MigrationState::Completed | MigrationState::Failed(_) | MigrationState::Cancelled => {
                return Err(horcrux_common::Error::System(
                    format!("Cannot cancel migration in state {:?}", job.state)
                ));
            }
            _ => {}
        }

        warn!(job_id = job_id, "Cancelling storage migration");

        job.state = MigrationState::Cancelled;
        job.completed_at = Some(Utc::now());

        let volume_name = job.volume_name.clone();
        drop(jobs);

        let mut active = self.active_migrations.write().await;
        active.remove(&volume_name);

        Ok(())
    }

    /// Fail a migration
    pub async fn fail_migration(&self, job_id: &str, reason: String) -> Result<()> {
        let mut jobs = self.jobs.write().await;
        let job = jobs.get_mut(job_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("Migration job {} not found", job_id))
        })?;

        error!(job_id = job_id, reason = %reason, "Storage migration failed");

        job.state = MigrationState::Failed(reason.clone());
        job.error = Some(reason);
        job.completed_at = Some(Utc::now());

        let volume_name = job.volume_name.clone();
        drop(jobs);

        let mut active = self.active_migrations.write().await;
        active.remove(&volume_name);

        Ok(())
    }

    /// Get migration job
    pub async fn get_job(&self, job_id: &str) -> Result<StorageMigrationJob> {
        let jobs = self.jobs.read().await;
        jobs.get(job_id)
            .cloned()
            .ok_or_else(|| horcrux_common::Error::System(format!("Migration job {} not found", job_id)))
    }

    /// List all migration jobs
    pub async fn list_jobs(&self, include_completed: bool) -> Vec<StorageMigrationJob> {
        let jobs = self.jobs.read().await;
        jobs.values()
            .filter(|j| {
                include_completed || !matches!(j.state,
                    MigrationState::Completed |
                    MigrationState::Failed(_) |
                    MigrationState::Cancelled
                )
            })
            .cloned()
            .collect()
    }

    /// Get migration for a specific volume
    pub async fn get_volume_migration(&self, volume_name: &str) -> Option<StorageMigrationJob> {
        let active = self.active_migrations.read().await;
        if let Some(job_id) = active.get(volume_name) {
            let jobs = self.jobs.read().await;
            return jobs.get(job_id).cloned();
        }
        None
    }
}

impl Default for StorageMigrationManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_migration_mode_selection() {
        // ZFS to ZFS should use send/receive
        assert_eq!(
            StorageMigrationManager::determine_migration_mode(
                &StorageType::Zfs,
                &StorageType::Zfs,
                false
            ),
            MigrationMode::ZfsSendReceive
        );

        // Ceph to Ceph should use RBD migrate
        assert_eq!(
            StorageMigrationManager::determine_migration_mode(
                &StorageType::Ceph,
                &StorageType::Ceph,
                false
            ),
            MigrationMode::CephRbdMigrate
        );

        // Running VM should use live migration
        assert_eq!(
            StorageMigrationManager::determine_migration_mode(
                &StorageType::Lvm,
                &StorageType::Directory,
                true
            ),
            MigrationMode::Live
        );

        // Different backends, offline should use offline copy
        assert_eq!(
            StorageMigrationManager::determine_migration_mode(
                &StorageType::Zfs,
                &StorageType::Lvm,
                false
            ),
            MigrationMode::Offline
        );
    }

    #[tokio::test]
    async fn test_start_migration() {
        let manager = StorageMigrationManager::new();

        let job_id = manager.start_migration(
            "vm-100-disk-0".to_string(),
            "pool1".to_string(),
            "pool2".to_string(),
            StorageType::Directory,
            StorageType::Directory,
            None,
            None,
        ).await.unwrap();

        let job = manager.get_job(&job_id).await.unwrap();
        assert_eq!(job.state, MigrationState::Completed);
    }
}
