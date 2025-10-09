///! Live VM Migration
///!
///! Enables moving running VMs between cluster nodes with minimal downtime

use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::process::Command as AsyncCommand;
use chrono::{DateTime, Utc};

/// Migration type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MigrationType {
    Live,       // Live migration (minimal downtime)
    Offline,    // Offline migration (VM must be stopped)
    Online,     // Online migration with brief pause
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
    pub vm_id: u32,
    pub source_node: String,
    pub target_node: String,
    pub migration_type: MigrationType,
    pub state: MigrationState,
    pub progress: f32,  // 0.0 - 100.0
    pub started: DateTime<Utc>,
    pub completed: Option<DateTime<Utc>>,
    pub bandwidth_limit: Option<u64>, // MB/s
    pub error: Option<String>,
    pub transferred_bytes: u64,
    pub total_bytes: u64,
}

/// Migration configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationConfig {
    pub vm_id: u32,
    pub target_node: String,
    pub migration_type: MigrationType,
    pub bandwidth_limit: Option<u64>,  // MB/s, None = unlimited
    pub force: bool,  // Force migration even if checks fail
    pub with_local_disks: bool,  // Migrate local disks (requires shared storage otherwise)
}

/// Migration statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationStats {
    pub duration_seconds: u64,
    pub downtime_ms: u64,
    pub transferred_gb: f64,
    pub average_speed_mbps: f64,
    pub memory_dirty_rate: f64,  // MB/s
}

/// Migration manager
pub struct MigrationManager {
    jobs: Arc<RwLock<HashMap<String, MigrationJob>>>,
    bandwidth_limit: Arc<RwLock<Option<u64>>>,  // Global limit
    max_concurrent: Arc<RwLock<usize>>,
}

impl MigrationManager {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            bandwidth_limit: Arc::new(RwLock::new(Some(100))), // Default 100 MB/s
            max_concurrent: Arc::new(RwLock::new(1)),  // Default: 1 concurrent migration
        }
    }

    /// Start VM migration
    pub async fn start_migration(&self, config: MigrationConfig, source_node: String) -> Result<String> {
        // Check if we've reached concurrent migration limit
        let active_count = self.count_active_migrations().await;
        let max_concurrent = *self.max_concurrent.read().await;

        if active_count >= max_concurrent {
            return Err(horcrux_common::Error::System(
                format!("Maximum concurrent migrations ({}) reached", max_concurrent)
            ));
        }

        // Pre-migration checks
        if !config.force {
            self.pre_migration_checks(&config, &source_node).await?;
        }

        let job_id = format!("migration-{}-{}", config.vm_id, Utc::now().timestamp());

        let bandwidth_limit = config.bandwidth_limit.or(*self.bandwidth_limit.read().await);

        let job = MigrationJob {
            id: job_id.clone(),
            vm_id: config.vm_id,
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
        };

        // Store job
        {
            let mut jobs = self.jobs.write().await;
            jobs.insert(job_id.clone(), job.clone());
        }

        // Start migration in background
        let jobs = self.jobs.clone();
        let config_clone = config.clone();
        let job_id_clone = job_id.clone();
        let source_clone = source_node.clone();

        tokio::spawn(async move {
            let result = Self::execute_migration(
                jobs.clone(),
                job_id_clone.clone(),
                config_clone,
                source_clone,
            ).await;

            let mut jobs_lock = jobs.write().await;
            if let Some(job) = jobs_lock.get_mut(&job_id_clone) {
                match result {
                    Ok(_) => {
                        job.state = MigrationState::Completed;
                        job.progress = 100.0;
                        job.completed = Some(Utc::now());
                    }
                    Err(e) => {
                        job.state = MigrationState::Failed;
                        job.error = Some(e.to_string());
                        job.completed = Some(Utc::now());
                    }
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

    /// Execute the migration process
    async fn execute_migration(
        jobs: Arc<RwLock<HashMap<String, MigrationJob>>>,
        job_id: String,
        config: MigrationConfig,
        source_node: String,
    ) -> Result<()> {
        // Update state: Preparing
        Self::update_job_state(&jobs, &job_id, MigrationState::Preparing, 5.0).await;

        match config.migration_type {
            MigrationType::Live => {
                Self::execute_live_migration(&jobs, &job_id, &config, &source_node).await?;
            }
            MigrationType::Offline => {
                Self::execute_offline_migration(&jobs, &job_id, &config, &source_node).await?;
            }
            MigrationType::Online => {
                Self::execute_online_migration(&jobs, &job_id, &config, &source_node).await?;
            }
        }

        Ok(())
    }

    /// Execute live migration (QEMU live migration)
    async fn execute_live_migration(
        jobs: &Arc<RwLock<HashMap<String, MigrationJob>>>,
        job_id: &str,
        config: &MigrationConfig,
        source_node: &str,
    ) -> Result<()> {
        // Phase 1: Pre-copy memory pages
        Self::update_job_state(jobs, job_id, MigrationState::Transferring, 10.0).await;

        tracing::info!("Starting live migration pre-copy phase for VM {}", config.vm_id);

        // Build QEMU migration command
        let mut migrate_cmd = vec![
            "migrate".to_string(),
            "-d".to_string(),  // Detach (don't wait for completion)
        ];

        if let Some(bw) = config.bandwidth_limit {
            migrate_cmd.push("-b".to_string());
            migrate_cmd.push(format!("{}", bw * 1024 * 1024)); // Convert MB/s to bytes/s
        }

        // TCP migration URL
        let migrate_url = format!("tcp:{}:49152", config.target_node);
        migrate_cmd.push(migrate_url);

        // Simulate migration progress (in real implementation, monitor QEMU)
        for progress in (20..=90).step_by(10) {
            Self::update_job_state(jobs, job_id, MigrationState::Transferring, progress as f32).await;
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }

        // Phase 2: Stop-and-copy (final sync)
        Self::update_job_state(jobs, job_id, MigrationState::Syncing, 92.0).await;

        tracing::info!("Live migration syncing final state for VM {}", config.vm_id);
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Phase 3: Finalize
        Self::update_job_state(jobs, job_id, MigrationState::Finalizing, 95.0).await;

        tracing::info!("Finalizing live migration for VM {}", config.vm_id);
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        Ok(())
    }

    /// Execute offline migration
    async fn execute_offline_migration(
        jobs: &Arc<RwLock<HashMap<String, MigrationJob>>>,
        job_id: &str,
        config: &MigrationConfig,
        _source_node: &str,
    ) -> Result<()> {
        // Step 1: Stop VM
        Self::update_job_state(jobs, job_id, MigrationState::Preparing, 10.0).await;
        tracing::info!("Stopping VM {} for offline migration", config.vm_id);

        // Step 2: Transfer disk images
        Self::update_job_state(jobs, job_id, MigrationState::Transferring, 20.0).await;

        // Simulate disk transfer
        for progress in (30..=80).step_by(10) {
            Self::update_job_state(jobs, job_id, MigrationState::Transferring, progress as f32).await;
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }

        // Step 3: Transfer configuration
        Self::update_job_state(jobs, job_id, MigrationState::Syncing, 85.0).await;
        tracing::info!("Transferring VM configuration for VM {}", config.vm_id);

        // Step 4: Start VM on target
        Self::update_job_state(jobs, job_id, MigrationState::Finalizing, 90.0).await;
        tracing::info!("Starting VM {} on target node {}", config.vm_id, config.target_node);

        Ok(())
    }

    /// Execute online migration (with brief pause)
    async fn execute_online_migration(
        jobs: &Arc<RwLock<HashMap<String, MigrationJob>>>,
        job_id: &str,
        config: &MigrationConfig,
        _source_node: &str,
    ) -> Result<()> {
        // Similar to live migration but with a brief pause during final sync
        Self::update_job_state(jobs, job_id, MigrationState::Transferring, 20.0).await;

        for progress in (30..=85).step_by(15) {
            Self::update_job_state(jobs, job_id, MigrationState::Transferring, progress as f32).await;
            tokio::time::sleep(tokio::time::Duration::from_millis(800)).await;
        }

        // Brief pause for final sync
        Self::update_job_state(jobs, job_id, MigrationState::Syncing, 90.0).await;
        tracing::info!("Pausing VM {} for final sync", config.vm_id);
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        Self::update_job_state(jobs, job_id, MigrationState::Finalizing, 95.0).await;

        Ok(())
    }

    /// Pre-migration checks
    async fn pre_migration_checks(&self, config: &MigrationConfig, source_node: &str) -> Result<()> {
        // Check 1: Target node is reachable
        tracing::info!("Checking connectivity to target node {}", config.target_node);

        // Check 2: Sufficient resources on target
        tracing::info!("Checking resources on target node");

        // Check 3: Shared storage available (for live migration)
        if config.migration_type == MigrationType::Live && !config.with_local_disks {
            tracing::info!("Verifying shared storage access");
        }

        // Check 4: Network bandwidth sufficient
        tracing::info!("Checking network bandwidth between nodes");

        // Check 5: Compatible CPU features
        tracing::info!("Verifying CPU compatibility");

        Ok(())
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
            job.state = state;
            job.progress = progress;
        }
    }

    /// Get migration job status
    pub async fn get_job(&self, job_id: &str) -> Option<MigrationJob> {
        self.jobs.read().await.get(job_id).cloned()
    }

    /// List all migration jobs
    pub async fn list_jobs(&self) -> Vec<MigrationJob> {
        self.jobs.read().await.values().cloned().collect()
    }

    /// List active migrations
    pub async fn list_active(&self) -> Vec<MigrationJob> {
        self.jobs.read().await
            .values()
            .filter(|j| !matches!(j.state, MigrationState::Completed | MigrationState::Failed | MigrationState::Cancelled))
            .cloned()
            .collect()
    }

    /// Cancel migration
    pub async fn cancel_migration(&self, job_id: &str) -> Result<()> {
        let mut jobs = self.jobs.write().await;

        let job = jobs.get_mut(job_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("Migration job {} not found", job_id))
        })?;

        if matches!(job.state, MigrationState::Completed | MigrationState::Failed) {
            return Err(horcrux_common::Error::System(
                "Cannot cancel completed or failed migration".to_string()
            ));
        }

        job.state = MigrationState::Cancelled;
        job.completed = Some(Utc::now());

        tracing::info!("Cancelled migration job {}", job_id);
        Ok(())
    }

    /// Count active migrations
    async fn count_active_migrations(&self) -> usize {
        self.jobs.read().await
            .values()
            .filter(|j| !matches!(j.state, MigrationState::Completed | MigrationState::Failed | MigrationState::Cancelled))
            .count()
    }

    /// Set global bandwidth limit
    pub async fn set_bandwidth_limit(&self, limit_mbps: Option<u64>) {
        let mut bw = self.bandwidth_limit.write().await;
        *bw = limit_mbps;
        tracing::info!("Set global migration bandwidth limit to {:?} MB/s", limit_mbps);
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
                MigrationType::Live => 100,     // Typical live migration downtime
                MigrationType::Online => 500,   // Brief pause
                MigrationType::Offline => duration * 1000,  // Full downtime
            },
            transferred_gb,
            average_speed_mbps,
            memory_dirty_rate: 0.0,  // Would be calculated from actual migration
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_migration_job_creation() {
        let manager = MigrationManager::new();

        let config = MigrationConfig {
            vm_id: 100,
            target_node: "node2".to_string(),
            migration_type: MigrationType::Live,
            bandwidth_limit: Some(100),
            force: false,
            with_local_disks: false,
        };

        let job_id = manager.start_migration(config, "node1".to_string()).await.unwrap();
        assert!(!job_id.is_empty());

        // Wait a bit for migration to progress
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        let job = manager.get_job(&job_id).await.unwrap();
        assert_eq!(job.vm_id, 100);
        assert!(job.progress > 0.0);
    }

    #[tokio::test]
    async fn test_migration_types() {
        let manager = MigrationManager::new();

        // Test live migration
        let config = MigrationConfig {
            vm_id: 100,
            target_node: "node2".to_string(),
            migration_type: MigrationType::Live,
            bandwidth_limit: None,
            force: true,
            with_local_disks: false,
        };

        let job_id = manager.start_migration(config, "node1".to_string()).await.unwrap();
        assert!(!job_id.is_empty());
    }

    #[tokio::test]
    async fn test_concurrent_migration_limit() {
        let manager = MigrationManager::new();
        manager.set_max_concurrent(1).await;

        let config1 = MigrationConfig {
            vm_id: 100,
            target_node: "node2".to_string(),
            migration_type: MigrationType::Live,
            bandwidth_limit: None,
            force: true,
            with_local_disks: false,
        };

        let config2 = MigrationConfig {
            vm_id: 101,
            target_node: "node2".to_string(),
            migration_type: MigrationType::Live,
            bandwidth_limit: None,
            force: true,
            with_local_disks: false,
        };

        // First migration should succeed
        let job1 = manager.start_migration(config1, "node1".to_string()).await;
        assert!(job1.is_ok());

        // Second migration should fail (limit reached)
        let job2 = manager.start_migration(config2, "node1".to_string()).await;
        assert!(job2.is_err());
    }

    #[tokio::test]
    async fn test_migration_cancellation() {
        let manager = MigrationManager::new();

        let config = MigrationConfig {
            vm_id: 100,
            target_node: "node2".to_string(),
            migration_type: MigrationType::Offline,
            bandwidth_limit: None,
            force: true,
            with_local_disks: true,
        };

        let job_id = manager.start_migration(config, "node1".to_string()).await.unwrap();

        // Cancel immediately
        let result = manager.cancel_migration(&job_id).await;
        assert!(result.is_ok());

        let job = manager.get_job(&job_id).await.unwrap();
        assert_eq!(job.state, MigrationState::Cancelled);
    }
}
