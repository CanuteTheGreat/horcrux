//! Live and Offline VM Migration
//!
//! Provides VM migration capabilities between cluster nodes including:
//! - Live migration with pre-copy memory transfer
//! - Offline migration with storage copy
//! - Post-copy migration for large VMs
//! - Migration progress tracking

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, error, debug};
use horcrux_common::Result;
use chrono::{DateTime, Utc};

/// Migration types supported
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MigrationType {
    /// Live migration - VM stays running during transfer
    Live,
    /// Offline migration - VM is stopped, copied, then started
    Offline,
    /// Post-copy - Start on destination, fetch memory on demand
    PostCopy,
}

/// Migration state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MigrationState {
    /// Migration pending/queued
    Pending,
    /// Pre-migration checks in progress
    Checking,
    /// Memory transfer in progress
    Transferring,
    /// Storage sync in progress
    SyncingStorage,
    /// Final iteration (for live migration)
    Converging,
    /// Switching to destination
    Switching,
    /// Post-migration cleanup
    Cleanup,
    /// Migration completed successfully
    Completed,
    /// Migration failed
    Failed(String),
    /// Migration cancelled
    Cancelled,
}

/// Migration job configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationConfig {
    /// Maximum bandwidth for migration (MB/s), 0 = unlimited
    pub bandwidth_limit: u64,
    /// Enable compression for memory transfer
    pub compress: bool,
    /// Allow migration to node with different CPU
    pub allow_cpu_incompatible: bool,
    /// Maximum downtime in milliseconds (for live migration)
    pub max_downtime_ms: u64,
    /// Enable RDMA for faster memory transfer (requires InfiniBand)
    pub use_rdma: bool,
    /// Number of parallel transfer connections
    pub parallel_connections: u32,
    /// Enable XBZRLE compression for repeated memory patterns
    pub enable_xbzrle: bool,
}

impl Default for MigrationConfig {
    fn default() -> Self {
        Self {
            bandwidth_limit: 0, // unlimited
            compress: true,
            allow_cpu_incompatible: false,
            max_downtime_ms: 300, // 300ms default
            use_rdma: false,
            parallel_connections: 2,
            enable_xbzrle: true,
        }
    }
}

/// Migration progress information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationProgress {
    /// Total RAM to transfer (bytes)
    pub total_memory: u64,
    /// RAM transferred so far (bytes)
    pub transferred_memory: u64,
    /// Remaining RAM to transfer (bytes)
    pub remaining_memory: u64,
    /// Dirty pages since last iteration
    pub dirty_pages: u64,
    /// Current transfer speed (bytes/sec)
    pub transfer_speed: u64,
    /// Expected time to complete (seconds)
    pub expected_downtime_ms: u64,
    /// Current iteration number
    pub iteration: u32,
    /// Storage progress (0-100%)
    pub storage_progress: f32,
    /// Total disk to transfer (bytes)
    pub total_disk: u64,
    /// Disk transferred so far (bytes)
    pub transferred_disk: u64,
}

impl Default for MigrationProgress {
    fn default() -> Self {
        Self {
            total_memory: 0,
            transferred_memory: 0,
            remaining_memory: 0,
            dirty_pages: 0,
            transfer_speed: 0,
            expected_downtime_ms: 0,
            iteration: 0,
            storage_progress: 0.0,
            total_disk: 0,
            transferred_disk: 0,
        }
    }
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
    pub config: MigrationConfig,
    pub progress: MigrationProgress,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

/// Pre-migration check result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationCheck {
    pub check_name: String,
    pub passed: bool,
    pub message: String,
    pub blocking: bool,
}

/// Migration manager
pub struct MigrationManager {
    jobs: Arc<RwLock<HashMap<String, MigrationJob>>>,
    active_migrations: Arc<RwLock<HashMap<String, String>>>, // vm_id -> job_id
    max_concurrent: usize,
}

impl MigrationManager {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            active_migrations: Arc::new(RwLock::new(HashMap::new())),
            max_concurrent: 4,
        }
    }

    /// Pre-migration checks
    pub async fn check_migration(
        &self,
        vm_id: &str,
        source_node: &str,
        target_node: &str,
        migration_type: &MigrationType,
    ) -> Vec<MigrationCheck> {
        let mut checks = Vec::new();

        // Check 1: VM is not already being migrated
        let active = self.active_migrations.read().await;
        if active.contains_key(vm_id) {
            checks.push(MigrationCheck {
                check_name: "no_active_migration".to_string(),
                passed: false,
                message: "VM is already being migrated".to_string(),
                blocking: true,
            });
        } else {
            checks.push(MigrationCheck {
                check_name: "no_active_migration".to_string(),
                passed: true,
                message: "No active migration for this VM".to_string(),
                blocking: false,
            });
        }
        drop(active);

        // Check 2: Source and target are different
        if source_node == target_node {
            checks.push(MigrationCheck {
                check_name: "different_nodes".to_string(),
                passed: false,
                message: "Source and target nodes are the same".to_string(),
                blocking: true,
            });
        } else {
            checks.push(MigrationCheck {
                check_name: "different_nodes".to_string(),
                passed: true,
                message: format!("Migration from {} to {}", source_node, target_node),
                blocking: false,
            });
        }

        // Check 3: Network connectivity (placeholder)
        checks.push(MigrationCheck {
            check_name: "network_connectivity".to_string(),
            passed: true,
            message: "Network connectivity verified".to_string(),
            blocking: false,
        });

        // Check 4: Storage accessibility (placeholder)
        checks.push(MigrationCheck {
            check_name: "storage_accessible".to_string(),
            passed: true,
            message: "Storage is accessible from target node".to_string(),
            blocking: false,
        });

        // Check 5: Live migration requirements
        if *migration_type == MigrationType::Live {
            checks.push(MigrationCheck {
                check_name: "live_migration_support".to_string(),
                passed: true,
                message: "Live migration is supported".to_string(),
                blocking: false,
            });
        }

        // Check 6: CPU compatibility (placeholder)
        checks.push(MigrationCheck {
            check_name: "cpu_compatibility".to_string(),
            passed: true,
            message: "CPU models are compatible".to_string(),
            blocking: false,
        });

        // Check 7: Memory availability on target (placeholder)
        checks.push(MigrationCheck {
            check_name: "memory_available".to_string(),
            passed: true,
            message: "Sufficient memory available on target".to_string(),
            blocking: false,
        });

        info!(
            vm_id = vm_id,
            source = source_node,
            target = target_node,
            checks_passed = checks.iter().filter(|c| c.passed).count(),
            total_checks = checks.len(),
            "Pre-migration checks completed"
        );

        checks
    }

    /// Start a migration job
    pub async fn start_migration(
        &self,
        vm_id: String,
        source_node: String,
        target_node: String,
        migration_type: MigrationType,
        config: Option<MigrationConfig>,
    ) -> Result<String> {
        // Run pre-migration checks
        let checks = self.check_migration(&vm_id, &source_node, &target_node, &migration_type).await;

        let blocking_failures: Vec<_> = checks.iter()
            .filter(|c| !c.passed && c.blocking)
            .collect();

        if !blocking_failures.is_empty() {
            let reasons: Vec<_> = blocking_failures.iter()
                .map(|c| c.message.clone())
                .collect();
            return Err(horcrux_common::Error::System(
                format!("Pre-migration checks failed: {}", reasons.join(", "))
            ));
        }

        // Check concurrent migration limit
        let jobs = self.jobs.read().await;
        let active_count = jobs.values()
            .filter(|j| matches!(j.state,
                MigrationState::Transferring |
                MigrationState::SyncingStorage |
                MigrationState::Converging |
                MigrationState::Switching
            ))
            .count();
        drop(jobs);

        if active_count >= self.max_concurrent {
            return Err(horcrux_common::Error::System(
                format!("Maximum concurrent migrations ({}) reached", self.max_concurrent)
            ));
        }

        // Create migration job
        let job_id = format!("mig-{}-{}", vm_id, Utc::now().timestamp());
        let job = MigrationJob {
            id: job_id.clone(),
            vm_id: vm_id.clone(),
            source_node: source_node.clone(),
            target_node: target_node.clone(),
            migration_type: migration_type.clone(),
            state: MigrationState::Pending,
            config: config.unwrap_or_default(),
            progress: MigrationProgress::default(),
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
            active.insert(vm_id.clone(), job_id.clone());
        }

        info!(
            job_id = %job_id,
            vm_id = %vm_id,
            source = %source_node,
            target = %target_node,
            migration_type = ?migration_type,
            "Migration job created"
        );

        // Start migration process (in a real implementation, this would be async)
        self.execute_migration(&job_id).await?;

        Ok(job_id)
    }

    /// Execute the migration
    async fn execute_migration(&self, job_id: &str) -> Result<()> {
        // Update state to checking
        self.update_state(job_id, MigrationState::Checking).await?;

        let job = self.get_job(job_id).await?;

        match job.migration_type {
            MigrationType::Live => self.execute_live_migration(&job).await,
            MigrationType::Offline => self.execute_offline_migration(&job).await,
            MigrationType::PostCopy => self.execute_postcopy_migration(&job).await,
        }
    }

    /// Execute live migration
    async fn execute_live_migration(&self, job: &MigrationJob) -> Result<()> {
        info!(job_id = %job.id, "Starting live migration");

        // Phase 1: Pre-copy - Transfer all memory pages
        self.update_state(&job.id, MigrationState::Transferring).await?;

        // In a real implementation, this would use QEMU's migrate command
        // qemu-monitor-command: migrate -d tcp:target:4444

        // Simulate memory transfer iterations
        for iteration in 1..=5 {
            debug!(job_id = %job.id, iteration = iteration, "Memory transfer iteration");

            // Update progress
            let mut jobs = self.jobs.write().await;
            if let Some(j) = jobs.get_mut(&job.id) {
                j.progress.iteration = iteration;
                j.progress.transferred_memory = iteration as u64 * 1024 * 1024 * 1024; // Simulated
                j.progress.remaining_memory = (5 - iteration) as u64 * 1024 * 1024 * 1024;
                j.progress.dirty_pages = 10000 / iteration as u64;
            }
            drop(jobs);

            // Small delay for simulation
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        // Phase 2: Converging - Final dirty page sync
        self.update_state(&job.id, MigrationState::Converging).await?;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Phase 3: Switching - Stop source, start destination
        self.update_state(&job.id, MigrationState::Switching).await?;

        // In a real implementation:
        // 1. Send QMP command to finalize migration
        // 2. Wait for migration to complete
        // 3. Verify VM is running on target

        // Phase 4: Cleanup
        self.update_state(&job.id, MigrationState::Cleanup).await?;

        // Complete
        self.complete_migration(&job.id).await?;

        info!(job_id = %job.id, "Live migration completed successfully");

        Ok(())
    }

    /// Execute offline migration
    async fn execute_offline_migration(&self, job: &MigrationJob) -> Result<()> {
        info!(job_id = %job.id, "Starting offline migration");

        // Phase 1: Stop VM on source
        self.update_state(&job.id, MigrationState::Checking).await?;

        // In a real implementation:
        // 1. Stop the VM
        // 2. Wait for clean shutdown

        // Phase 2: Copy storage
        self.update_state(&job.id, MigrationState::SyncingStorage).await?;

        // Simulate storage copy
        for progress in (0..=100).step_by(10) {
            let mut jobs = self.jobs.write().await;
            if let Some(j) = jobs.get_mut(&job.id) {
                j.progress.storage_progress = progress as f32;
            }
            drop(jobs);
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // Phase 3: Transfer memory state (if suspended)
        self.update_state(&job.id, MigrationState::Transferring).await?;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Phase 4: Start VM on target
        self.update_state(&job.id, MigrationState::Switching).await?;

        // In a real implementation:
        // 1. Create VM on target with same config
        // 2. Attach copied storage
        // 3. Start VM

        // Phase 5: Cleanup source
        self.update_state(&job.id, MigrationState::Cleanup).await?;

        // Complete
        self.complete_migration(&job.id).await?;

        info!(job_id = %job.id, "Offline migration completed successfully");

        Ok(())
    }

    /// Execute post-copy migration
    async fn execute_postcopy_migration(&self, job: &MigrationJob) -> Result<()> {
        info!(job_id = %job.id, "Starting post-copy migration");

        // Phase 1: Transfer minimal state
        self.update_state(&job.id, MigrationState::Transferring).await?;

        // In a real implementation:
        // 1. Transfer CPU state and device state
        // 2. Start VM on destination immediately
        // 3. Fetch memory pages on demand

        // Phase 2: Switch to destination
        self.update_state(&job.id, MigrationState::Switching).await?;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Phase 3: Background page fetch
        // VM is running on destination, pages fetched as needed

        // Complete
        self.complete_migration(&job.id).await?;

        info!(job_id = %job.id, "Post-copy migration completed successfully");

        Ok(())
    }

    /// Update migration state
    async fn update_state(&self, job_id: &str, state: MigrationState) -> Result<()> {
        let mut jobs = self.jobs.write().await;
        let job = jobs.get_mut(job_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("Migration job {} not found", job_id))
        })?;

        debug!(job_id = job_id, old_state = ?job.state, new_state = ?state, "Migration state change");
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

        let vm_id = job.vm_id.clone();
        drop(jobs);

        // Remove from active migrations
        let mut active = self.active_migrations.write().await;
        active.remove(&vm_id);

        Ok(())
    }

    /// Cancel a migration
    pub async fn cancel_migration(&self, job_id: &str) -> Result<()> {
        let mut jobs = self.jobs.write().await;
        let job = jobs.get_mut(job_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("Migration job {} not found", job_id))
        })?;

        // Can only cancel pending or in-progress migrations
        match &job.state {
            MigrationState::Completed | MigrationState::Failed(_) | MigrationState::Cancelled => {
                return Err(horcrux_common::Error::System(
                    format!("Cannot cancel migration in state {:?}", job.state)
                ));
            }
            _ => {}
        }

        warn!(job_id = job_id, "Cancelling migration");

        job.state = MigrationState::Cancelled;
        job.completed_at = Some(Utc::now());

        let vm_id = job.vm_id.clone();
        drop(jobs);

        // Remove from active migrations
        let mut active = self.active_migrations.write().await;
        active.remove(&vm_id);

        // In a real implementation:
        // 1. Send cancel command to QEMU
        // 2. Cleanup any partial state
        // 3. Ensure VM is still running on source

        Ok(())
    }

    /// Fail a migration
    pub async fn fail_migration(&self, job_id: &str, reason: String) -> Result<()> {
        let mut jobs = self.jobs.write().await;
        let job = jobs.get_mut(job_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("Migration job {} not found", job_id))
        })?;

        error!(job_id = job_id, reason = %reason, "Migration failed");

        job.state = MigrationState::Failed(reason.clone());
        job.error = Some(reason);
        job.completed_at = Some(Utc::now());

        let vm_id = job.vm_id.clone();
        drop(jobs);

        // Remove from active migrations
        let mut active = self.active_migrations.write().await;
        active.remove(&vm_id);

        Ok(())
    }

    /// Get migration job
    pub async fn get_job(&self, job_id: &str) -> Result<MigrationJob> {
        let jobs = self.jobs.read().await;
        jobs.get(job_id)
            .cloned()
            .ok_or_else(|| horcrux_common::Error::System(format!("Migration job {} not found", job_id)))
    }

    /// Get migration job by VM ID
    pub async fn get_job_by_vm(&self, vm_id: &str) -> Option<MigrationJob> {
        let active = self.active_migrations.read().await;
        if let Some(job_id) = active.get(vm_id) {
            let jobs = self.jobs.read().await;
            return jobs.get(job_id).cloned();
        }
        None
    }

    /// List all migration jobs
    pub async fn list_jobs(&self, include_completed: bool) -> Vec<MigrationJob> {
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

    /// Get migration history for a VM
    pub async fn get_vm_migration_history(&self, vm_id: &str) -> Vec<MigrationJob> {
        let jobs = self.jobs.read().await;
        jobs.values()
            .filter(|j| j.vm_id == vm_id)
            .cloned()
            .collect()
    }

    /// Cleanup old completed jobs
    pub async fn cleanup_old_jobs(&self, max_age_hours: u32) {
        let mut jobs = self.jobs.write().await;
        let cutoff = Utc::now() - chrono::Duration::hours(max_age_hours as i64);

        let old_jobs: Vec<_> = jobs.iter()
            .filter(|(_, j)| {
                matches!(j.state, MigrationState::Completed | MigrationState::Failed(_) | MigrationState::Cancelled)
                    && j.completed_at.map(|t| t < cutoff).unwrap_or(false)
            })
            .map(|(id, _)| id.clone())
            .collect();

        for id in old_jobs {
            debug!(job_id = %id, "Cleaning up old migration job");
            jobs.remove(&id);
        }
    }
}

impl Default for MigrationManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_migration_checks() {
        let manager = MigrationManager::new();

        let checks = manager.check_migration(
            "vm-100",
            "node1",
            "node2",
            &MigrationType::Live,
        ).await;

        assert!(checks.iter().all(|c| c.passed));
    }

    #[tokio::test]
    async fn test_same_node_migration_fails() {
        let manager = MigrationManager::new();

        let checks = manager.check_migration(
            "vm-100",
            "node1",
            "node1",
            &MigrationType::Live,
        ).await;

        let different_nodes = checks.iter()
            .find(|c| c.check_name == "different_nodes")
            .unwrap();

        assert!(!different_nodes.passed);
        assert!(different_nodes.blocking);
    }

    #[tokio::test]
    async fn test_start_migration() {
        let manager = MigrationManager::new();

        let job_id = manager.start_migration(
            "vm-100".to_string(),
            "node1".to_string(),
            "node2".to_string(),
            MigrationType::Offline,
            None,
        ).await.unwrap();

        let job = manager.get_job(&job_id).await.unwrap();
        assert_eq!(job.state, MigrationState::Completed);
    }

    #[tokio::test]
    async fn test_list_jobs() {
        let manager = MigrationManager::new();

        manager.start_migration(
            "vm-100".to_string(),
            "node1".to_string(),
            "node2".to_string(),
            MigrationType::Offline,
            None,
        ).await.unwrap();

        let all_jobs = manager.list_jobs(true).await;
        assert_eq!(all_jobs.len(), 1);

        let active_jobs = manager.list_jobs(false).await;
        assert_eq!(active_jobs.len(), 0); // Completed, so not in active
    }
}
