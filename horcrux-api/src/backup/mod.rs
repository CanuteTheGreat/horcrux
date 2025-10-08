///! Backup and restore system (vzdump equivalent)
///! Provides VM and container backup with scheduling and retention

mod scheduler;
mod retention;
pub mod providers;

use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Backup mode
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackupMode {
    Snapshot,   // Use storage snapshots (ZFS/Ceph/LVM)
    Suspend,    // Suspend VM, copy, resume
    Stop,       // Stop VM, copy, start
}

/// Backup compression
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Compression {
    None,
    Gzip,
    Lzo,
    Zstd,
}

/// Backup configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupConfig {
    pub id: String,
    pub name: String,
    pub target_type: TargetType,  // VM or Container
    pub target_id: String,
    pub storage: String,           // Backup storage location
    pub mode: BackupMode,
    pub compression: Compression,
    pub notes: Option<String>,
}

/// Target type for backup
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetType {
    Vm,
    Container,
}

/// Backup job (scheduled backup)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupJob {
    pub id: String,
    pub enabled: bool,
    pub schedule: String,  // Cron-like schedule
    pub targets: Vec<String>,  // VM/Container IDs
    pub storage: String,
    pub mode: BackupMode,
    pub compression: Compression,
    pub retention: RetentionPolicy,
    pub notify: Option<String>,  // Email for notifications
}

/// Retention policy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionPolicy {
    pub keep_hourly: Option<u32>,
    pub keep_daily: Option<u32>,
    pub keep_weekly: Option<u32>,
    pub keep_monthly: Option<u32>,
    pub keep_yearly: Option<u32>,
}

/// Backup metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Backup {
    pub id: String,
    pub target_type: TargetType,
    pub target_id: String,
    pub target_name: String,
    pub timestamp: i64,
    pub size: u64,  // Bytes
    pub mode: BackupMode,
    pub compression: Compression,
    pub path: PathBuf,
    pub config_included: bool,
    pub notes: Option<String>,
}

/// Backup manager
pub struct BackupManager {
    backups: Arc<RwLock<HashMap<String, Backup>>>,
    jobs: Arc<RwLock<HashMap<String, BackupJob>>>,
    scheduler: scheduler::BackupScheduler,
    retention: retention::RetentionManager,
    zfs_backend: Option<Arc<crate::storage::zfs::ZfsManager>>,
    ceph_backend: Option<Arc<crate::storage::ceph::CephManager>>,
    lvm_backend: Option<Arc<crate::storage::lvm::LvmManager>>,
}

impl BackupManager {
    pub fn new() -> Self {
        Self {
            backups: Arc::new(RwLock::new(HashMap::new())),
            jobs: Arc::new(RwLock::new(HashMap::new())),
            scheduler: scheduler::BackupScheduler::new(),
            retention: retention::RetentionManager::new(),
            zfs_backend: None,
            ceph_backend: None,
            lvm_backend: None,
        }
    }

    /// Create BackupManager with storage backends
    pub fn with_backends(
        zfs: Option<Arc<crate::storage::zfs::ZfsManager>>,
        ceph: Option<Arc<crate::storage::ceph::CephManager>>,
        lvm: Option<Arc<crate::storage::lvm::LvmManager>>,
    ) -> Self {
        Self {
            backups: Arc::new(RwLock::new(HashMap::new())),
            jobs: Arc::new(RwLock::new(HashMap::new())),
            scheduler: scheduler::BackupScheduler::new(),
            retention: retention::RetentionManager::new(),
            zfs_backend: zfs,
            ceph_backend: ceph,
            lvm_backend: lvm,
        }
    }

    /// Create a backup
    pub async fn create_backup(&self, config: BackupConfig) -> Result<Backup> {
        tracing::info!("Creating backup for {} {}", config.target_type.as_str(), config.target_id);

        // Generate backup metadata
        let backup_id = uuid::Uuid::new_v4().to_string();
        let timestamp = chrono::Utc::now().timestamp();

        // Determine backup path
        let filename = format!(
            "vzdump-{}-{}-{}.{}",
            config.target_type.as_str(),
            config.target_id,
            chrono::Utc::now().format("%Y_%m_%d-%H_%M_%S"),
            self.get_file_extension(&config.compression)
        );

        let backup_path = PathBuf::from(&config.storage).join(&filename);

        // Perform the actual backup based on mode
        let size = match config.mode {
            BackupMode::Snapshot => {
                self.backup_with_snapshot(&config, &backup_path).await?
            }
            BackupMode::Suspend => {
                self.backup_with_suspend(&config, &backup_path).await?
            }
            BackupMode::Stop => {
                self.backup_with_stop(&config, &backup_path).await?
            }
        };

        let backup = Backup {
            id: backup_id.clone(),
            target_type: config.target_type,
            target_id: config.target_id,
            target_name: config.name,
            timestamp,
            size,
            mode: config.mode,
            compression: config.compression,
            path: backup_path,
            config_included: true,
            notes: config.notes,
        };

        let mut backups = self.backups.write().await;
        backups.insert(backup_id, backup.clone());

        tracing::info!("Backup created successfully: {}", backup.id);
        Ok(backup)
    }

    /// Restore from backup
    pub async fn restore_backup(&self, backup_id: &str, target_id: Option<String>) -> Result<()> {
        let backups = self.backups.read().await;
        let backup = backups
            .get(backup_id)
            .ok_or_else(|| horcrux_common::Error::System(format!("Backup {} not found", backup_id)))?;

        tracing::info!("Restoring backup {} to {:?}", backup_id, target_id);

        // Determine target ID (use original or new)
        let restore_target = target_id.unwrap_or_else(|| backup.target_id.clone());

        // Extract and restore based on backup type
        match backup.target_type {
            TargetType::Vm => {
                self.restore_vm_backup(backup, &restore_target).await?
            }
            TargetType::Container => {
                self.restore_container_backup(backup, &restore_target).await?
            }
        }

        tracing::info!("Backup restored successfully");
        Ok(())
    }

    /// List all backups
    pub async fn list_backups(&self, target_id: Option<String>) -> Vec<Backup> {
        let backups = self.backups.read().await;

        if let Some(tid) = target_id {
            backups
                .values()
                .filter(|b| b.target_id == tid)
                .cloned()
                .collect()
        } else {
            backups.values().cloned().collect()
        }
    }

    /// Delete a backup
    pub async fn delete_backup(&self, backup_id: &str) -> Result<()> {
        let mut backups = self.backups.write().await;

        if let Some(backup) = backups.remove(backup_id) {
            // Delete the backup file
            tokio::fs::remove_file(&backup.path).await.ok();
            tracing::info!("Backup {} deleted", backup_id);
            Ok(())
        } else {
            Err(horcrux_common::Error::System(format!("Backup {} not found", backup_id)))
        }
    }

    /// Create a backup job
    pub async fn create_job(&self, job: BackupJob) -> Result<()> {
        let mut jobs = self.jobs.write().await;

        if jobs.contains_key(&job.id) {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Backup job {} already exists",
                job.id
            )));
        }

        // Schedule the job
        if job.enabled {
            self.scheduler.schedule_job(&job).await?;
        }

        jobs.insert(job.id.clone(), job);
        Ok(())
    }

    /// List all backup jobs
    pub async fn list_jobs(&self) -> Vec<BackupJob> {
        let jobs = self.jobs.read().await;
        jobs.values().cloned().collect()
    }

    /// Apply retention policy to backups
    pub async fn apply_retention(&self, target_id: &str, policy: &RetentionPolicy) -> Result<()> {
        let backups = self.backups.read().await;
        let target_backups: Vec<&Backup> = backups
            .values()
            .filter(|b| b.target_id == target_id)
            .collect();

        let to_delete = self.retention.apply_policy(&target_backups, policy).await;

        drop(backups);

        // Delete backups that exceed retention
        for backup_id in to_delete {
            self.delete_backup(&backup_id).await?;
        }

        Ok(())
    }

    // Private helper methods

    async fn backup_with_snapshot(&self, config: &BackupConfig, path: &PathBuf) -> Result<u64> {
        // Use storage snapshots (ZFS/Ceph/LVM)
        tracing::info!("Performing snapshot-based backup");

        let snapshot_name = format!("backup-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"));

        // Determine which storage backend to use based on config.storage
        let size = if config.storage.starts_with("zfs:") && self.zfs_backend.is_some() {
            self.backup_with_zfs_snapshot(config, path, &snapshot_name).await?
        } else if config.storage.starts_with("ceph:") && self.ceph_backend.is_some() {
            self.backup_with_ceph_snapshot(config, path, &snapshot_name).await?
        } else if config.storage.starts_with("lvm:") && self.lvm_backend.is_some() {
            self.backup_with_lvm_snapshot(config, path, &snapshot_name).await?
        } else {
            // Fallback to file-based backup
            self.backup_with_file_copy(config, path).await?
        };

        Ok(size)
    }

    async fn backup_with_zfs_snapshot(&self, config: &BackupConfig, path: &PathBuf, snapshot_name: &str) -> Result<u64> {
        let zfs = self.zfs_backend.as_ref().ok_or_else(||
            horcrux_common::Error::System("ZFS backend not configured".to_string()))?;

        // Extract pool and volume from storage path (e.g., "zfs:tank/vms/vm-100")
        let storage_path = config.storage.strip_prefix("zfs:").unwrap_or(&config.storage);

        // Create ZFS snapshot
        zfs.create_snapshot(storage_path, &config.target_id, snapshot_name).await?;

        // Export snapshot to file using zfs send
        let snapshot_path = format!("{}@{}", config.target_id, snapshot_name);
        let export_cmd = format!("zfs send {}/{} | {} > {}",
            storage_path, snapshot_path,
            self.get_compression_cmd(&config.compression),
            path.display()
        );

        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&export_cmd)
            .output()
            .await?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "Failed to export ZFS snapshot: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        // Get backup file size
        let metadata = tokio::fs::metadata(path).await?;
        let size = metadata.len();

        // Clean up snapshot after export
        let destroy_cmd = format!("zfs destroy {}/{}@{}", storage_path, config.target_id, snapshot_name);
        tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&destroy_cmd)
            .output()
            .await?;

        tracing::info!("ZFS snapshot backup completed: {} bytes", size);
        Ok(size)
    }

    async fn backup_with_ceph_snapshot(&self, config: &BackupConfig, path: &PathBuf, snapshot_name: &str) -> Result<u64> {
        let ceph = self.ceph_backend.as_ref().ok_or_else(||
            horcrux_common::Error::System("Ceph backend not configured".to_string()))?;

        // Extract pool from storage path (e.g., "ceph:rbd/vms")
        let storage_path = config.storage.strip_prefix("ceph:").unwrap_or(&config.storage);

        // Create Ceph RBD snapshot
        ceph.create_snapshot(storage_path, &config.target_id, snapshot_name).await?;

        // Export snapshot using rbd export
        let export_cmd = format!("rbd export {}/{}@{} - | {} > {}",
            storage_path, config.target_id, snapshot_name,
            self.get_compression_cmd(&config.compression),
            path.display()
        );

        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&export_cmd)
            .output()
            .await?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "Failed to export Ceph snapshot: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        // Get backup file size
        let metadata = tokio::fs::metadata(path).await?;
        let size = metadata.len();

        // Clean up snapshot
        let rm_snap_cmd = format!("rbd snap rm {}/{}@{}", storage_path, config.target_id, snapshot_name);
        tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&rm_snap_cmd)
            .output()
            .await?;

        tracing::info!("Ceph RBD snapshot backup completed: {} bytes", size);
        Ok(size)
    }

    async fn backup_with_lvm_snapshot(&self, config: &BackupConfig, path: &PathBuf, snapshot_name: &str) -> Result<u64> {
        let lvm = self.lvm_backend.as_ref().ok_or_else(||
            horcrux_common::Error::System("LVM backend not configured".to_string()))?;

        // Extract VG from storage path (e.g., "lvm:vg0")
        let storage_path = config.storage.strip_prefix("lvm:").unwrap_or(&config.storage);

        // Create LVM snapshot
        lvm.create_snapshot(storage_path, &config.target_id, snapshot_name).await?;

        // Export snapshot using dd
        let snapshot_device = format!("/dev/{}/{}", storage_path, snapshot_name);
        let export_cmd = format!("dd if={} bs=4M | {} > {}",
            snapshot_device,
            self.get_compression_cmd(&config.compression),
            path.display()
        );

        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&export_cmd)
            .output()
            .await?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "Failed to export LVM snapshot: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        // Get backup file size
        let metadata = tokio::fs::metadata(path).await?;
        let size = metadata.len();

        // Remove snapshot
        let rm_cmd = format!("lvremove -f {}/{}", storage_path, snapshot_name);
        tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&rm_cmd)
            .output()
            .await?;

        tracing::info!("LVM snapshot backup completed: {} bytes", size);
        Ok(size)
    }

    async fn backup_with_file_copy(&self, config: &BackupConfig, path: &PathBuf) -> Result<u64> {
        // Fallback: Direct file copy with compression
        tracing::info!("Performing file-based backup");

        let source_path = PathBuf::from(&config.storage).join(&config.target_id);

        let copy_cmd = format!("tar -cf - -C {} . | {} > {}",
            source_path.display(),
            self.get_compression_cmd(&config.compression),
            path.display()
        );

        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&copy_cmd)
            .output()
            .await?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "Failed to create backup archive: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        let metadata = tokio::fs::metadata(path).await?;
        Ok(metadata.len())
    }

    fn get_compression_cmd(&self, compression: &Compression) -> String {
        match compression {
            Compression::None => "cat".to_string(),
            Compression::Gzip => "gzip".to_string(),
            Compression::Lzo => "lzop".to_string(),
            Compression::Zstd => "zstd".to_string(),
        }
    }

    async fn backup_with_suspend(&self, config: &BackupConfig, path: &PathBuf) -> Result<u64> {
        // Suspend VM, copy disks, resume
        tracing::info!("Performing suspend-based backup");

        // TODO: Integrate with VM manager to suspend/resume VM
        // For now, just do file copy
        // In production:
        // 1. vm_manager.suspend_vm(&config.target_id).await?
        // 2. Copy disk files
        // 3. vm_manager.resume_vm(&config.target_id).await?

        self.backup_with_file_copy(config, path).await
    }

    async fn backup_with_stop(&self, config: &BackupConfig, path: &PathBuf) -> Result<u64> {
        // Stop VM, copy disks, start
        tracing::info!("Performing stop-based backup");

        // TODO: Integrate with VM manager to stop/start VM
        // For now, just do file copy
        // In production:
        // 1. Check if VM is running
        // 2. vm_manager.stop_vm(&config.target_id).await?
        // 3. Copy disk files
        // 4. vm_manager.start_vm(&config.target_id).await?

        self.backup_with_file_copy(config, path).await
    }

    async fn restore_vm_backup(&self, backup: &Backup, target_id: &str) -> Result<()> {
        tracing::info!("Restoring VM backup to {}", target_id);

        // Determine restore method based on backup path extension
        if backup.path.extension().and_then(|e| e.to_str()) == Some("zst") ||
           backup.path.extension().and_then(|e| e.to_str()) == Some("gz") {
            self.restore_from_compressed_backup(backup, target_id).await?;
        } else {
            self.restore_from_uncompressed_backup(backup, target_id).await?;
        }

        Ok(())
    }

    async fn restore_container_backup(&self, backup: &Backup, target_id: &str) -> Result<()> {
        tracing::info!("Restoring container backup to {}", target_id);

        // Same restore logic as VMs for now
        self.restore_from_compressed_backup(backup, target_id).await?;

        Ok(())
    }

    async fn restore_from_compressed_backup(&self, backup: &Backup, target_id: &str) -> Result<()> {
        let decompress_cmd = match backup.compression {
            Compression::None => "cat",
            Compression::Gzip => "gunzip",
            Compression::Lzo => "lzop -d",
            Compression::Zstd => "unzstd",
        };

        // Create restore directory
        let restore_path = PathBuf::from("/var/lib/horcrux/restore").join(target_id);
        tokio::fs::create_dir_all(&restore_path).await?;

        // Extract backup
        let extract_cmd = format!("{} < {} | tar -xf - -C {}",
            decompress_cmd,
            backup.path.display(),
            restore_path.display()
        );

        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&extract_cmd)
            .output()
            .await?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "Failed to extract backup: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        tracing::info!("Backup extracted to {}", restore_path.display());
        Ok(())
    }

    async fn restore_from_uncompressed_backup(&self, backup: &Backup, target_id: &str) -> Result<()> {
        let restore_path = PathBuf::from("/var/lib/horcrux/restore").join(target_id);
        tokio::fs::create_dir_all(&restore_path).await?;

        // Direct tar extraction
        let extract_cmd = format!("tar -xf {} -C {}",
            backup.path.display(),
            restore_path.display()
        );

        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&extract_cmd)
            .output()
            .await?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "Failed to extract backup: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        Ok(())
    }

    fn get_file_extension(&self, compression: &Compression) -> &str {
        match compression {
            Compression::None => "tar",
            Compression::Gzip => "tar.gz",
            Compression::Lzo => "tar.lzo",
            Compression::Zstd => "tar.zst",
        }
    }
}

impl TargetType {
    fn as_str(&self) -> &str {
        match self {
            TargetType::Vm => "qemu",
            TargetType::Container => "lxc",
        }
    }
}
