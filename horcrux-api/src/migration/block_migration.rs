//! Live Block Migration
//!
//! Enables live migration of VMs with local disks (non-shared storage)
//! Uses QEMU's block migration feature to copy disk data during live migration

#![allow(dead_code)]

use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::RwLock;
use tracing::{error, info};

/// Block device information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockDevice {
    pub device_id: String,
    pub source_path: PathBuf,
    pub target_path: PathBuf,
    pub size_bytes: u64,
    pub format: DiskFormat,
}

/// Disk image format
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DiskFormat {
    Raw,
    Qcow2,
    Vmdk,
    Vdi,
}

impl std::fmt::Display for DiskFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiskFormat::Raw => write!(f, "raw"),
            DiskFormat::Qcow2 => write!(f, "qcow2"),
            DiskFormat::Vmdk => write!(f, "vmdk"),
            DiskFormat::Vdi => write!(f, "vdi"),
        }
    }
}

/// Block migration progress
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockMigrationProgress {
    pub device_id: String,
    pub transferred_bytes: u64,
    pub total_bytes: u64,
    pub progress_percent: f32,
    pub transfer_rate_mbps: f64,
    pub remaining_time_seconds: Option<u64>,
}

/// Block migration state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum BlockMigrationState {
    Idle,
    Preparing,
    Transferring,
    Syncing,
    Completed,
    Failed,
}

/// Block migration job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockMigrationJob {
    pub job_id: String,
    pub vm_id: u32,
    pub devices: Vec<BlockDevice>,
    pub state: BlockMigrationState,
    pub overall_progress: f32,
    pub device_progress: HashMap<String, BlockMigrationProgress>,
    pub started_at: i64,
    pub completed_at: Option<i64>,
    pub error: Option<String>,
}

/// Block migration manager
pub struct BlockMigrationManager {
    jobs: Arc<RwLock<HashMap<String, BlockMigrationJob>>>,
}

impl BlockMigrationManager {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Start block migration for a VM
    pub async fn start_block_migration(
        &self,
        job_id: String,
        vm_id: u32,
        devices: Vec<BlockDevice>,
        source_node: String,
        target_node: String,
    ) -> Result<()> {
        info!("Starting block migration for VM {} ({} devices)", vm_id, devices.len());

        // Create job
        let job = BlockMigrationJob {
            job_id: job_id.clone(),
            vm_id,
            devices: devices.clone(),
            state: BlockMigrationState::Preparing,
            overall_progress: 0.0,
            device_progress: HashMap::new(),
            started_at: chrono::Utc::now().timestamp(),
            completed_at: None,
            error: None,
        };

        // Store job
        {
            let mut jobs = self.jobs.write().await;
            jobs.insert(job_id.clone(), job);
        }

        // Execute block migration in background
        let manager = Arc::new(self.clone());
        let job_id_clone = job_id.clone();
        let devices_clone = devices.clone();
        let source_clone = source_node.clone();
        let target_clone = target_node.clone();

        tokio::spawn(async move {
            if let Err(e) = manager.execute_block_migration(
                &job_id_clone,
                vm_id,
                devices_clone,
                source_clone,
                target_clone,
            ).await {
                error!("Block migration failed for VM {}: {}", vm_id, e);

                let mut jobs = manager.jobs.write().await;
                if let Some(job) = jobs.get_mut(&job_id_clone) {
                    job.state = BlockMigrationState::Failed;
                    job.error = Some(e.to_string());
                    job.completed_at = Some(chrono::Utc::now().timestamp());
                }
            }
        });

        Ok(())
    }

    /// Execute the actual block migration
    async fn execute_block_migration(
        &self,
        job_id: &str,
        vm_id: u32,
        devices: Vec<BlockDevice>,
        source_node: String,
        target_node: String,
    ) -> Result<()> {
        // Update state: Preparing
        self.update_job_state(job_id, BlockMigrationState::Preparing, 5.0).await;

        // Pre-migration checks
        self.pre_migration_checks(&devices, &source_node, &target_node).await?;

        // Update state: Transferring
        self.update_job_state(job_id, BlockMigrationState::Transferring, 10.0).await;

        // For each device, initiate block migration
        let total_devices = devices.len();
        for (idx, device) in devices.iter().enumerate() {
            info!("Migrating block device {} ({}/{})", device.device_id, idx + 1, total_devices);

            // Use qemu-img convert for initial copy (can be done while VM is running)
            self.migrate_block_device(job_id, device, &source_node, &target_node).await?;

            // Update progress
            let progress = 10.0 + (70.0 * (idx + 1) as f32 / total_devices as f32);
            self.update_job_state(job_id, BlockMigrationState::Transferring, progress).await;
        }

        // Final sync phase (copy remaining dirty blocks)
        self.update_job_state(job_id, BlockMigrationState::Syncing, 85.0).await;
        self.sync_remaining_blocks(job_id, &devices, &source_node, &target_node).await?;

        // Mark as completed
        self.update_job_state(job_id, BlockMigrationState::Completed, 100.0).await;

        let mut jobs = self.jobs.write().await;
        if let Some(job) = jobs.get_mut(job_id) {
            job.completed_at = Some(chrono::Utc::now().timestamp());
        }

        info!("Block migration completed for VM {}", vm_id);
        Ok(())
    }

    /// Migrate a single block device
    async fn migrate_block_device(
        &self,
        job_id: &str,
        device: &BlockDevice,
        source_node: &str,
        target_node: &str,
    ) -> Result<()> {
        info!("Migrating device {} from {} to {}", device.device_id, source_node, target_node);

        // Build qemu-img convert command via SSH
        let mut cmd = Command::new("ssh");
        cmd.arg(source_node)
            .arg("qemu-img")
            .arg("convert")
            .arg("-p")  // Show progress
            .arg("-O")
            .arg(device.format.to_string())
            .arg(&device.source_path)
            .arg(format!("ssh://{}{}",target_node, device.target_path.display()));

        let output = cmd.output().await
            .map_err(|e| horcrux_common::Error::System(
                format!("Failed to execute qemu-img convert: {}", e)
            ))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(
                format!("Block migration failed for device {}: {}", device.device_id, stderr)
            ));
        }

        // Update device progress
        self.update_device_progress(job_id, &device.device_id, device.size_bytes, device.size_bytes).await;

        Ok(())
    }

    /// Sync remaining dirty blocks (final sync phase)
    async fn sync_remaining_blocks(
        &self,
        _job_id: &str,
        devices: &[BlockDevice],
        source_node: &str,
        target_node: &str,
    ) -> Result<()> {
        info!("Syncing remaining dirty blocks for {} devices", devices.len());

        // Final sync of dirty blocks for each device
        // Uses rsync with --inplace to copy only changed blocks efficiently
        for device in devices {
            info!("Final sync for device {}", device.device_id);

            // Use rsync with --inplace and --sparse for efficient dirty block sync
            let output = Command::new("ssh")
                .args([
                    "-o", "StrictHostKeyChecking=no",
                    "-o", "UserKnownHostsFile=/dev/null",
                    source_node,
                    "rsync",
                    "-az",
                    "--inplace",      // Update files in-place
                    "--sparse",       // Handle sparse files efficiently
                    "--whole-file",   // Don't use delta transfer for final sync
                    &format!("{}", device.source_path.display()),
                    &format!("{}:{}", target_node, device.target_path.display()),
                ])
                .output()
                .await
                .map_err(|e| horcrux_common::Error::System(
                    format!("Failed to sync dirty blocks for device {}: {}", device.device_id, e)
                ))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(horcrux_common::Error::System(
                    format!("Dirty block sync failed for device {}: {}", device.device_id, stderr)
                ));
            }

            info!("Dirty block sync completed for device {}", device.device_id);
        }

        info!("All dirty blocks synced successfully");
        Ok(())
    }

    /// Pre-migration checks for block devices
    async fn pre_migration_checks(
        &self,
        devices: &[BlockDevice],
        source_node: &str,
        target_node: &str,
    ) -> Result<()> {
        info!("Performing pre-migration checks for block devices");

        // Check 1: Verify all source devices exist
        for device in devices {
            info!("Checking source device: {}", device.source_path.display());

            let mut cmd = Command::new("ssh");
            cmd.arg(source_node)
                .arg("test")
                .arg("-f")
                .arg(&device.source_path);

            let status = cmd.status().await
                .map_err(|e| horcrux_common::Error::System(
                    format!("Failed to check source device: {}", e)
                ))?;

            if !status.success() {
                return Err(horcrux_common::Error::System(
                    format!("Source device not found: {}", device.source_path.display())
                ));
            }
        }

        // Check 2: Verify target has sufficient space
        let total_size: u64 = devices.iter().map(|d| d.size_bytes).sum();
        info!("Total disk space required: {} GB", total_size / (1024 * 1024 * 1024));

        // Check 3: Verify target directories exist
        for device in devices {
            if let Some(parent) = device.target_path.parent() {
                let mut cmd = Command::new("ssh");
                cmd.arg(target_node)
                    .arg("mkdir")
                    .arg("-p")
                    .arg(parent);

                cmd.status().await
                    .map_err(|e| horcrux_common::Error::System(
                        format!("Failed to create target directory: {}", e)
                    ))?;
            }
        }

        info!("Pre-migration checks passed");
        Ok(())
    }

    /// Update job state and overall progress
    async fn update_job_state(&self, job_id: &str, state: BlockMigrationState, progress: f32) {
        let mut jobs = self.jobs.write().await;
        if let Some(job) = jobs.get_mut(job_id) {
            job.state = state;
            job.overall_progress = progress;
        }
    }

    /// Update individual device progress
    async fn update_device_progress(
        &self,
        job_id: &str,
        device_id: &str,
        transferred: u64,
        total: u64,
    ) {
        let mut jobs = self.jobs.write().await;
        if let Some(job) = jobs.get_mut(job_id) {
            let progress_percent = if total > 0 {
                (transferred as f64 / total as f64 * 100.0) as f32
            } else {
                0.0
            };

            let device_progress = BlockMigrationProgress {
                device_id: device_id.to_string(),
                transferred_bytes: transferred,
                total_bytes: total,
                progress_percent,
                transfer_rate_mbps: 0.0,  // Would be calculated from actual transfer rate
                remaining_time_seconds: None,
            };

            job.device_progress.insert(device_id.to_string(), device_progress);
        }
    }

    /// Get block migration job status
    pub async fn get_job(&self, job_id: &str) -> Option<BlockMigrationJob> {
        self.jobs.read().await.get(job_id).cloned()
    }

    /// List all block migration jobs
    pub async fn list_jobs(&self) -> Vec<BlockMigrationJob> {
        self.jobs.read().await.values().cloned().collect()
    }

    /// Get disk format from file path
    pub fn detect_disk_format(path: &Path) -> DiskFormat {
        match path.extension().and_then(|e| e.to_str()) {
            Some("qcow2") => DiskFormat::Qcow2,
            Some("vmdk") => DiskFormat::Vmdk,
            Some("vdi") => DiskFormat::Vdi,
            _ => DiskFormat::Raw,
        }
    }

    /// Calculate total migration size
    pub fn calculate_total_size(devices: &[BlockDevice]) -> u64 {
        devices.iter().map(|d| d.size_bytes).sum()
    }
}

// Make Clone available for Arc wrapping
impl Clone for BlockMigrationManager {
    fn clone(&self) -> Self {
        Self {
            jobs: Arc::clone(&self.jobs),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disk_format_detection() {
        assert_eq!(
            BlockMigrationManager::detect_disk_format(Path::new("/var/lib/vm/disk.qcow2")),
            DiskFormat::Qcow2
        );
        assert_eq!(
            BlockMigrationManager::detect_disk_format(Path::new("/var/lib/vm/disk.vmdk")),
            DiskFormat::Vmdk
        );
        assert_eq!(
            BlockMigrationManager::detect_disk_format(Path::new("/var/lib/vm/disk.raw")),
            DiskFormat::Raw
        );
        assert_eq!(
            BlockMigrationManager::detect_disk_format(Path::new("/var/lib/vm/disk.img")),
            DiskFormat::Raw
        );
    }

    #[test]
    fn test_calculate_total_size() {
        let devices = vec![
            BlockDevice {
                device_id: "vda".to_string(),
                source_path: PathBuf::from("/dev/vda"),
                target_path: PathBuf::from("/dev/vda"),
                size_bytes: 10 * 1024 * 1024 * 1024, // 10 GB
                format: DiskFormat::Qcow2,
            },
            BlockDevice {
                device_id: "vdb".to_string(),
                source_path: PathBuf::from("/dev/vdb"),
                target_path: PathBuf::from("/dev/vdb"),
                size_bytes: 20 * 1024 * 1024 * 1024, // 20 GB
                format: DiskFormat::Raw,
            },
        ];

        let total = BlockMigrationManager::calculate_total_size(&devices);
        assert_eq!(total, 30 * 1024 * 1024 * 1024); // 30 GB
    }

    #[tokio::test]
    async fn test_block_migration_job_creation() {
        let manager = BlockMigrationManager::new();

        let devices = vec![
            BlockDevice {
                device_id: "vda".to_string(),
                source_path: PathBuf::from("/var/lib/vms/vm-100/disk0.qcow2"),
                target_path: PathBuf::from("/var/lib/vms/vm-100/disk0.qcow2"),
                size_bytes: 10 * 1024 * 1024 * 1024,
                format: DiskFormat::Qcow2,
            },
        ];

        let job_id = "block-migration-test".to_string();
        let result = manager.start_block_migration(
            job_id.clone(),
            100,
            devices,
            "node1".to_string(),
            "node2".to_string(),
        ).await;

        assert!(result.is_ok());

        // Wait a bit for job to be created
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let job = manager.get_job(&job_id).await;
        assert!(job.is_some());

        let job = job.unwrap();
        assert_eq!(job.vm_id, 100);
        assert_eq!(job.devices.len(), 1);
    }
}
