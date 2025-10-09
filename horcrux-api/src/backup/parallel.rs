//! Parallel backup restore
//!
//! Provides faster backup recovery by restoring multiple
//! disks/volumes in parallel. Proxmox VE 9.0 feature.

use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tracing::{error, info};

/// Parallel restore manager
pub struct ParallelRestoreManager {
    max_parallel: usize,
}

impl ParallelRestoreManager {
    pub fn new(max_parallel: usize) -> Self {
        Self {
            max_parallel: max_parallel.max(1).min(8), // 1-8 parallel streams
        }
    }

    /// Restore backup with parallel disk recovery
    pub async fn restore_parallel(
        &self,
        backup_id: &str,
        volumes: Vec<VolumeRestore>,
    ) -> Result<RestoreResult> {
        info!(
            "Starting parallel restore of {} with {} volumes",
            backup_id,
            volumes.len()
        );

        let start_time = std::time::Instant::now();
        let semaphore = Arc::new(Semaphore::new(self.max_parallel));
        let mut join_set = JoinSet::new();

        let total_size: u64 = volumes.iter().map(|v| v.size_bytes).sum();
        let mut completed_size = Arc::new(tokio::sync::RwLock::new(0u64));

        // Spawn restore tasks for each volume
        for volume in volumes {
            let sem = semaphore.clone();
            let completed = completed_size.clone();

            join_set.spawn(async move {
                // Acquire semaphore permit
                let _permit = sem.acquire().await.unwrap();

                info!("Restoring volume: {}", volume.name);

                // Simulate volume restore (in production, call actual restore logic)
                let result = Self::restore_volume(&volume).await;

                // Update progress
                if result.is_ok() {
                    let mut comp = completed.write().await;
                    *comp += volume.size_bytes;
                }

                result.map(|_| volume.name.clone())
            });
        }

        // Collect results
        let mut restored_volumes = Vec::new();
        let mut errors = Vec::new();

        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(Ok(volume_name)) => {
                    info!("Successfully restored volume: {}", volume_name);
                    restored_volumes.push(volume_name);
                }
                Ok(Err(e)) => {
                    error!("Volume restore failed: {}", e);
                    errors.push(e.to_string());
                }
                Err(e) => {
                    error!("Task join error: {}", e);
                    errors.push(format!("Task error: {}", e));
                }
            }
        }

        let elapsed = start_time.elapsed();
        let completed_total = *completed_size.read().await;

        let throughput_mbps = if elapsed.as_secs() > 0 {
            (completed_total as f64 / 1024.0 / 1024.0) / elapsed.as_secs_f64()
        } else {
            0.0
        };

        info!(
            "Parallel restore completed: {} volumes in {:.2}s ({:.2} MB/s)",
            restored_volumes.len(),
            elapsed.as_secs_f64(),
            throughput_mbps
        );

        Ok(RestoreResult {
            backup_id: backup_id.to_string(),
            restored_volumes,
            failed_volumes: errors,
            total_size_bytes: total_size,
            duration_secs: elapsed.as_secs_f64(),
            throughput_mbps,
        })
    }

    /// Restore a single volume (placeholder for actual implementation)
    async fn restore_volume(volume: &VolumeRestore) -> Result<()> {
        // In production, this would:
        // 1. Read backup data from source
        // 2. Decompress if needed
        // 3. Write to target storage
        // 4. Verify checksums

        // Simulate restore time based on size
        let restore_time_ms = (volume.size_bytes / 100_000_000).max(100); // ~100MB/s
        tokio::time::sleep(std::time::Duration::from_millis(restore_time_ms)).await;

        Ok(())
    }

    /// Calculate optimal parallel stream count based on hardware
    pub fn calculate_optimal_streams(total_size_gb: u64, available_bandwidth_gbps: f64) -> usize {
        // Simple heuristic:
        // - Small backups (<100GB): 2-4 streams
        // - Medium backups (100-500GB): 4-6 streams
        // - Large backups (>500GB): 6-8 streams
        // - Limited by available bandwidth

        let size_based = if total_size_gb < 100 {
            2
        } else if total_size_gb < 500 {
            4
        } else {
            6
        };

        // Limit by bandwidth (assume 1 stream needs ~1 Gbps)
        let bandwidth_based = (available_bandwidth_gbps.ceil() as usize).min(8);

        size_based.min(bandwidth_based)
    }
}

/// Volume restore information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeRestore {
    pub name: String,
    pub source_path: String,
    pub target_path: String,
    pub size_bytes: u64,
    pub checksum: Option<String>,
}

/// Restore result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreResult {
    pub backup_id: String,
    pub restored_volumes: Vec<String>,
    pub failed_volumes: Vec<String>,
    pub total_size_bytes: u64,
    pub duration_secs: f64,
    pub throughput_mbps: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_parallel_restore() {
        let manager = ParallelRestoreManager::new(4);

        let volumes = vec![
            VolumeRestore {
                name: "disk0".to_string(),
                source_path: "/backup/disk0".to_string(),
                target_path: "/vms/disk0".to_string(),
                size_bytes: 10_000_000_000, // 10GB
                checksum: None,
            },
            VolumeRestore {
                name: "disk1".to_string(),
                source_path: "/backup/disk1".to_string(),
                target_path: "/vms/disk1".to_string(),
                size_bytes: 5_000_000_000, // 5GB
                checksum: None,
            },
        ];

        let result = manager.restore_parallel("backup-001", volumes).await.unwrap();

        assert_eq!(result.restored_volumes.len(), 2);
        assert!(result.failed_volumes.is_empty());
        assert_eq!(result.total_size_bytes, 15_000_000_000);
    }

    #[test]
    fn test_calculate_optimal_streams() {
        assert_eq!(
            ParallelRestoreManager::calculate_optimal_streams(50, 10.0),
            2
        );
        assert_eq!(
            ParallelRestoreManager::calculate_optimal_streams(200, 10.0),
            4
        );
        assert_eq!(
            ParallelRestoreManager::calculate_optimal_streams(1000, 10.0),
            6
        );
    }
}
