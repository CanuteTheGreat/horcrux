///! VM Snapshot Management
///!
///! Provides snapshot functionality for virtual machines:
///! - Create snapshots (memory + disk state)
///! - List snapshots with metadata
///! - Restore to previous snapshot
///! - Delete snapshots
///! - Snapshot trees and rollback

use horcrux_common::{Result, VmConfig, VmStatus};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::fs;
use tokio::process::Command;

/// VM snapshot metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmSnapshot {
    pub id: String,
    pub vm_id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: i64,
    pub parent_snapshot: Option<String>,
    pub vm_state: VmSnapshotState,
    pub disk_snapshots: Vec<DiskSnapshot>,
    pub memory_snapshot: Option<String>,
    pub config_backup: VmConfig,
}

/// VM state at snapshot time
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum VmSnapshotState {
    Running,  // Live snapshot with memory
    Stopped,  // Disk-only snapshot
    Paused,   // VM was paused during snapshot
}

/// Disk snapshot information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskSnapshot {
    pub disk_id: String,
    pub storage_type: StorageType,
    pub snapshot_name: String,
    pub snapshot_path: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum StorageType {
    Zfs,
    Lvm,
    Qcow2,
    Btrfs,
    Ceph,
}

/// Snapshot tree node for visualization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotTreeNode {
    pub snapshot: VmSnapshot,
    pub children: Vec<SnapshotTreeNode>,
    pub is_current: bool,
}

/// VM Snapshot Manager
pub struct VmSnapshotManager {
    snapshots: HashMap<String, VmSnapshot>,
    snapshot_dir: String,
}

impl VmSnapshotManager {
    pub fn new(snapshot_dir: String) -> Self {
        Self {
            snapshots: HashMap::new(),
            snapshot_dir,
        }
    }

    /// Create a VM snapshot
    pub async fn create_snapshot(
        &mut self,
        vm_config: &VmConfig,
        snapshot_name: String,
        description: Option<String>,
        include_memory: bool,
    ) -> Result<VmSnapshot> {
        let snapshot_id = uuid::Uuid::new_v4().to_string();
        let timestamp = chrono::Utc::now().timestamp();

        tracing::info!(
            "Creating snapshot '{}' for VM {} (include_memory: {})",
            snapshot_name,
            vm_config.id,
            include_memory
        );

        // Determine VM state
        let vm_state = match vm_config.status {
            VmStatus::Running => {
                if include_memory {
                    VmSnapshotState::Running
                } else {
                    // Need to pause VM for consistent disk snapshot
                    self.pause_vm(&vm_config.id).await?;
                    VmSnapshotState::Paused
                }
            }
            VmStatus::Stopped => VmSnapshotState::Stopped,
            VmStatus::Paused => VmSnapshotState::Paused,
            _ => {
                return Err(horcrux_common::Error::System(
                    "Cannot snapshot VM in current state".to_string(),
                ));
            }
        };

        // Create disk snapshots
        let mut disk_snapshots = Vec::new();
        for (idx, disk) in vm_config.disks.iter().enumerate() {
            let disk_snapshot = self
                .create_disk_snapshot(
                    &vm_config.id,
                    &disk.path,
                    &snapshot_name,
                    idx,
                )
                .await?;
            disk_snapshots.push(disk_snapshot);
        }

        // Create memory snapshot if requested and VM is running
        let memory_snapshot = if include_memory && vm_state == VmSnapshotState::Running {
            Some(self.create_memory_snapshot(&vm_config.id, &snapshot_id).await?)
        } else {
            None
        };

        // Resume VM if we paused it
        if vm_state == VmSnapshotState::Paused && vm_config.status == VmStatus::Running {
            self.resume_vm(&vm_config.id).await?;
        }

        let snapshot = VmSnapshot {
            id: snapshot_id.clone(),
            vm_id: vm_config.id.clone(),
            name: snapshot_name,
            description,
            created_at: timestamp,
            parent_snapshot: None, // TODO: Track snapshot lineage
            vm_state,
            disk_snapshots,
            memory_snapshot,
            config_backup: vm_config.clone(),
        };

        // Save snapshot metadata
        self.save_snapshot_metadata(&snapshot).await?;
        self.snapshots.insert(snapshot_id, snapshot.clone());

        tracing::info!("Snapshot created successfully: {}", snapshot.id);
        Ok(snapshot)
    }

    /// Create a snapshot of a single disk
    async fn create_disk_snapshot(
        &self,
        _vm_id: &str,
        disk_path: &str,
        snapshot_name: &str,
        disk_idx: usize,
    ) -> Result<DiskSnapshot> {
        // Detect storage type from disk path
        let storage_type = self.detect_storage_type(disk_path)?;

        let snapshot_id = format!("{}_{}", snapshot_name, disk_idx);

        match storage_type {
            StorageType::Zfs => {
                self.create_zfs_snapshot(disk_path, &snapshot_id).await?;
            }
            StorageType::Lvm => {
                self.create_lvm_snapshot(disk_path, &snapshot_id).await?;
            }
            StorageType::Qcow2 => {
                self.create_qcow2_snapshot(disk_path, &snapshot_id).await?;
            }
            StorageType::Btrfs => {
                self.create_btrfs_snapshot(disk_path, &snapshot_id).await?;
            }
            StorageType::Ceph => {
                self.create_ceph_snapshot(disk_path, &snapshot_id).await?;
            }
        }

        let size = self.get_disk_size(disk_path).await.unwrap_or(0);

        Ok(DiskSnapshot {
            disk_id: format!("disk-{}", disk_idx),
            storage_type,
            snapshot_name: snapshot_id.clone(),
            snapshot_path: disk_path.to_string(),
            size_bytes: size,
        })
    }

    /// Create ZFS snapshot
    async fn create_zfs_snapshot(&self, zvol_path: &str, snapshot_name: &str) -> Result<()> {
        // Extract dataset from /dev/zvol/pool/dataset
        let dataset = zvol_path
            .strip_prefix("/dev/zvol/")
            .ok_or_else(|| horcrux_common::Error::System("Invalid ZFS path".to_string()))?;

        let snapshot_path = format!("{}@{}", dataset, snapshot_name);

        let output = Command::new("zfs")
            .arg("snapshot")
            .arg(&snapshot_path)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create ZFS snapshot: {}",
                stderr
            )));
        }

        tracing::info!("Created ZFS snapshot: {}", snapshot_path);
        Ok(())
    }

    /// Create LVM snapshot
    async fn create_lvm_snapshot(&self, lv_path: &str, snapshot_name: &str) -> Result<()> {
        // LVM path format: /dev/vg_name/lv_name
        let output = Command::new("lvcreate")
            .arg("-s")
            .arg("-n")
            .arg(snapshot_name)
            .arg("-L")
            .arg("10G") // Snapshot size - should be configurable
            .arg(lv_path)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create LVM snapshot: {}",
                stderr
            )));
        }

        tracing::info!("Created LVM snapshot: {}", snapshot_name);
        Ok(())
    }

    /// Create QCOW2 internal snapshot
    async fn create_qcow2_snapshot(&self, qcow2_path: &str, snapshot_name: &str) -> Result<()> {
        let output = Command::new("qemu-img")
            .arg("snapshot")
            .arg("-c")
            .arg(snapshot_name)
            .arg(qcow2_path)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create QCOW2 snapshot: {}",
                stderr
            )));
        }

        tracing::info!("Created QCOW2 snapshot: {}", snapshot_name);
        Ok(())
    }

    /// Create Btrfs snapshot
    async fn create_btrfs_snapshot(&self, subvol_path: &str, snapshot_name: &str) -> Result<()> {
        let snapshot_path = format!("{}.snap_{}", subvol_path, snapshot_name);

        let output = Command::new("btrfs")
            .arg("subvolume")
            .arg("snapshot")
            .arg("-r") // Read-only snapshot
            .arg(subvol_path)
            .arg(&snapshot_path)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create Btrfs snapshot: {}",
                stderr
            )));
        }

        tracing::info!("Created Btrfs snapshot: {}", snapshot_path);
        Ok(())
    }

    /// Create Ceph RBD snapshot
    async fn create_ceph_snapshot(&self, rbd_path: &str, snapshot_name: &str) -> Result<()> {
        // RBD path format: pool/image
        let output = Command::new("rbd")
            .arg("snap")
            .arg("create")
            .arg(format!("{}@{}", rbd_path, snapshot_name))
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create Ceph snapshot: {}",
                stderr
            )));
        }

        tracing::info!("Created Ceph RBD snapshot: {}", snapshot_name);
        Ok(())
    }

    /// Create memory snapshot (save VM RAM state)
    async fn create_memory_snapshot(&self, vm_id: &str, snapshot_id: &str) -> Result<String> {
        let memory_file = format!("{}/{}-{}.mem", self.snapshot_dir, vm_id, snapshot_id);

        // Use QEMU monitor to save memory state
        let monitor_path = format!("/var/run/qemu-{}.mon", vm_id);

        let save_command = format!("migrate \"exec:gzip -c > {}\"", memory_file);

        let output = Command::new("socat")
            .arg("-")
            .arg(format!("UNIX-CONNECT:{}", monitor_path))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()?
            .stdin
            .ok_or_else(|| horcrux_common::Error::System("Failed to get stdin".to_string()))?;

        use tokio::io::AsyncWriteExt;
        let mut stdin = output;
        stdin.write_all(save_command.as_bytes()).await?;
        stdin.write_all(b"\n").await?;

        tracing::info!("Created memory snapshot: {}", memory_file);
        Ok(memory_file)
    }

    /// List all snapshots for a VM
    pub fn list_snapshots(&self, vm_id: &str) -> Vec<VmSnapshot> {
        self.snapshots
            .values()
            .filter(|s| s.vm_id == vm_id)
            .cloned()
            .collect()
    }

    /// Get a specific snapshot
    pub fn get_snapshot(&self, snapshot_id: &str) -> Option<&VmSnapshot> {
        self.snapshots.get(snapshot_id)
    }

    /// Delete a snapshot
    pub async fn delete_snapshot(&mut self, snapshot_id: &str) -> Result<()> {
        let snapshot = self
            .snapshots
            .get(snapshot_id)
            .ok_or_else(|| horcrux_common::Error::System("Snapshot not found".to_string()))?
            .clone();

        tracing::info!("Deleting snapshot: {}", snapshot_id);

        // Delete disk snapshots
        for disk_snap in &snapshot.disk_snapshots {
            self.delete_disk_snapshot(disk_snap).await?;
        }

        // Delete memory snapshot if exists
        if let Some(mem_file) = &snapshot.memory_snapshot {
            if let Err(e) = fs::remove_file(mem_file).await {
                tracing::warn!("Failed to delete memory snapshot file: {}", e);
            }
        }

        // Delete metadata file
        let metadata_file = format!("{}/{}.json", self.snapshot_dir, snapshot_id);
        if let Err(e) = fs::remove_file(&metadata_file).await {
            tracing::warn!("Failed to delete snapshot metadata: {}", e);
        }

        self.snapshots.remove(snapshot_id);
        tracing::info!("Snapshot deleted: {}", snapshot_id);
        Ok(())
    }

    /// Delete a disk snapshot
    async fn delete_disk_snapshot(&self, disk_snap: &DiskSnapshot) -> Result<()> {
        match disk_snap.storage_type {
            StorageType::Zfs => {
                let dataset = disk_snap
                    .snapshot_path
                    .strip_prefix("/dev/zvol/")
                    .unwrap_or(&disk_snap.snapshot_path);
                let snapshot_path = format!("{}@{}", dataset, disk_snap.snapshot_name);

                Command::new("zfs")
                    .arg("destroy")
                    .arg(&snapshot_path)
                    .output()
                    .await?;
            }
            StorageType::Lvm => {
                let vg_name = disk_snap.snapshot_path.split('/').nth(2).unwrap_or("vg");
                let snapshot_path = format!("/dev/{}/{}", vg_name, disk_snap.snapshot_name);

                Command::new("lvremove")
                    .arg("-f")
                    .arg(&snapshot_path)
                    .output()
                    .await?;
            }
            StorageType::Qcow2 => {
                Command::new("qemu-img")
                    .arg("snapshot")
                    .arg("-d")
                    .arg(&disk_snap.snapshot_name)
                    .arg(&disk_snap.snapshot_path)
                    .output()
                    .await?;
            }
            StorageType::Btrfs => {
                let snapshot_path = format!("{}.snap_{}", disk_snap.snapshot_path, disk_snap.snapshot_name);
                Command::new("btrfs")
                    .arg("subvolume")
                    .arg("delete")
                    .arg(&snapshot_path)
                    .output()
                    .await?;
            }
            StorageType::Ceph => {
                Command::new("rbd")
                    .arg("snap")
                    .arg("rm")
                    .arg(format!("{}@{}", disk_snap.snapshot_path, disk_snap.snapshot_name))
                    .output()
                    .await?;
            }
        }

        Ok(())
    }

    /// Restore VM to a snapshot
    pub async fn restore_snapshot(
        &self,
        snapshot_id: &str,
        restore_memory: bool,
    ) -> Result<()> {
        let snapshot = self
            .snapshots
            .get(snapshot_id)
            .ok_or_else(|| horcrux_common::Error::System("Snapshot not found".to_string()))?;

        tracing::info!("Restoring snapshot: {}", snapshot_id);

        // Stop VM if running
        self.stop_vm(&snapshot.vm_id).await?;

        // Restore disk snapshots
        for disk_snap in &snapshot.disk_snapshots {
            self.restore_disk_snapshot(disk_snap).await?;
        }

        // Restore memory if requested and available
        if restore_memory && snapshot.memory_snapshot.is_some() {
            // Start VM and restore memory state
            tracing::info!("Restoring memory state (not yet implemented)");
            // TODO: Implement memory restoration via QEMU migration
        }

        tracing::info!("Snapshot restored: {}", snapshot_id);
        Ok(())
    }

    /// Restore a disk snapshot
    async fn restore_disk_snapshot(&self, disk_snap: &DiskSnapshot) -> Result<()> {
        match disk_snap.storage_type {
            StorageType::Zfs => {
                let dataset = disk_snap
                    .snapshot_path
                    .strip_prefix("/dev/zvol/")
                    .unwrap_or(&disk_snap.snapshot_path);
                let snapshot_path = format!("{}@{}", dataset, disk_snap.snapshot_name);

                Command::new("zfs")
                    .arg("rollback")
                    .arg("-r")
                    .arg(&snapshot_path)
                    .output()
                    .await?;
            }
            StorageType::Lvm => {
                // LVM restore requires merge operation
                let vg_name = disk_snap.snapshot_path.split('/').nth(2).unwrap_or("vg");
                let snapshot_path = format!("/dev/{}/{}", vg_name, disk_snap.snapshot_name);

                Command::new("lvconvert")
                    .arg("--merge")
                    .arg(&snapshot_path)
                    .output()
                    .await?;
            }
            StorageType::Qcow2 => {
                Command::new("qemu-img")
                    .arg("snapshot")
                    .arg("-a")
                    .arg(&disk_snap.snapshot_name)
                    .arg(&disk_snap.snapshot_path)
                    .output()
                    .await?;
            }
            StorageType::Btrfs => {
                // Btrfs requires deleting current and renaming snapshot
                tracing::warn!("Btrfs snapshot restoration not fully implemented");
            }
            StorageType::Ceph => {
                Command::new("rbd")
                    .arg("snap")
                    .arg("rollback")
                    .arg(format!("{}@{}", disk_snap.snapshot_path, disk_snap.snapshot_name))
                    .output()
                    .await?;
            }
        }

        Ok(())
    }

    /// Build snapshot tree for visualization
    pub fn build_snapshot_tree(&self, vm_id: &str) -> Vec<SnapshotTreeNode> {
        let snapshots = self.list_snapshots(vm_id);

        if snapshots.is_empty() {
            return Vec::new();
        }

        // Build tree structure from parent relationships
        self.build_tree_recursive(&snapshots, None)
    }

    /// Recursively build snapshot tree from parent-child relationships
    fn build_tree_recursive(
        &self,
        all_snapshots: &[VmSnapshot],
        parent_id: Option<&str>,
    ) -> Vec<SnapshotTreeNode> {
        all_snapshots
            .iter()
            .filter(|s| s.parent_snapshot.as_deref() == parent_id)
            .map(|snapshot| {
                // Recursively build children
                let children = self.build_tree_recursive(all_snapshots, Some(&snapshot.id));

                SnapshotTreeNode {
                    snapshot: snapshot.clone(),
                    children,
                    is_current: self.is_current_snapshot(&snapshot.id),
                }
            })
            .collect()
    }

    /// Check if a snapshot is the currently active one
    fn is_current_snapshot(&self, snapshot_id: &str) -> bool {
        // In a real implementation, this would check which snapshot
        // the VM is currently running from. For now, check if it's
        // the most recent snapshot without children.
        if self.snapshots.get(snapshot_id).is_some() {
            // A snapshot is "current" if no other snapshots have it as parent
            !self.snapshots.values().any(|s| {
                s.parent_snapshot.as_ref().map(|p| p.as_str()) == Some(snapshot_id)
            })
        } else {
            false
        }
    }

    // Helper methods

    fn detect_storage_type(&self, disk_path: &str) -> Result<StorageType> {
        if disk_path.starts_with("/dev/zvol/") {
            Ok(StorageType::Zfs)
        } else if disk_path.starts_with("/dev/") && disk_path.contains("/lv") {
            Ok(StorageType::Lvm)
        } else if disk_path.ends_with(".qcow2") {
            Ok(StorageType::Qcow2)
        } else if disk_path.contains("btrfs") {
            Ok(StorageType::Btrfs)
        } else if disk_path.contains("rbd") {
            Ok(StorageType::Ceph)
        } else {
            Err(horcrux_common::Error::System(format!(
                "Cannot detect storage type for: {}",
                disk_path
            )))
        }
    }

    async fn get_disk_size(&self, disk_path: &str) -> Result<u64> {
        let metadata = fs::metadata(disk_path).await?;
        Ok(metadata.len())
    }

    async fn pause_vm(&self, vm_id: &str) -> Result<()> {
        // TODO: Implement via QEMU monitor
        tracing::info!("Pausing VM: {}", vm_id);
        Ok(())
    }

    async fn resume_vm(&self, vm_id: &str) -> Result<()> {
        // TODO: Implement via QEMU monitor
        tracing::info!("Resuming VM: {}", vm_id);
        Ok(())
    }

    async fn stop_vm(&self, vm_id: &str) -> Result<()> {
        // TODO: Implement via QEMU monitor
        tracing::info!("Stopping VM: {}", vm_id);
        Ok(())
    }

    async fn save_snapshot_metadata(&self, snapshot: &VmSnapshot) -> Result<()> {
        fs::create_dir_all(&self.snapshot_dir).await?;

        let metadata_file = format!("{}/{}.json", self.snapshot_dir, snapshot.id);
        let json = serde_json::to_string_pretty(snapshot)
            .map_err(|e| horcrux_common::Error::System(format!("Failed to serialize snapshot: {}", e)))?;

        fs::write(&metadata_file, json).await?;
        Ok(())
    }

    /// Load snapshots from disk on startup
    pub async fn load_snapshots(&mut self) -> Result<()> {
        if !fs::try_exists(&self.snapshot_dir).await.unwrap_or(false) {
            fs::create_dir_all(&self.snapshot_dir).await?;
            return Ok(());
        }

        let mut entries = fs::read_dir(&self.snapshot_dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Ok(content) = fs::read_to_string(&path).await {
                    if let Ok(snapshot) = serde_json::from_str::<VmSnapshot>(&content) {
                        self.snapshots.insert(snapshot.id.clone(), snapshot);
                    }
                }
            }
        }

        tracing::info!("Loaded {} snapshots from disk", self.snapshots.len());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horcrux_common::VmArchitecture;

    fn create_test_vm_config() -> VmConfig {
        VmConfig {
            id: "test-vm-100".to_string(),
            name: "test-vm".to_string(),
            hypervisor: horcrux_common::VmHypervisor::Qemu,
            memory: 2048,
            cpus: 2,
            disk_size: 20 * 1024 * 1024 * 1024,
            status: VmStatus::Running,
            architecture: VmArchitecture::X86_64,
            disks: vec![],
        }
    }

    #[test]
    fn test_snapshot_manager_new() {
        let manager = VmSnapshotManager::new("/tmp/snapshots".to_string());
        assert_eq!(manager.snapshot_dir, "/tmp/snapshots");
        assert_eq!(manager.snapshots.len(), 0);
    }

    #[test]
    fn test_detect_storage_type() {
        let manager = VmSnapshotManager::new("/tmp/snapshots".to_string());

        assert_eq!(
            manager.detect_storage_type("/dev/zvol/tank/vm-100-disk-0").unwrap(),
            StorageType::Zfs
        );

        assert_eq!(
            manager.detect_storage_type("/dev/vg0/lv-vm-100").unwrap(),
            StorageType::Lvm
        );

        assert_eq!(
            manager.detect_storage_type("/var/lib/vz/images/100/vm-100-disk-0.qcow2").unwrap(),
            StorageType::Qcow2
        );

        assert_eq!(
            manager.detect_storage_type("/mnt/btrfs/vm-100-disk-0").unwrap(),
            StorageType::Btrfs
        );

        assert_eq!(
            manager.detect_storage_type("/dev/rbd0").unwrap(),
            StorageType::Ceph
        );
    }

    #[test]
    fn test_detect_storage_type_invalid() {
        let manager = VmSnapshotManager::new("/tmp/snapshots".to_string());
        let result = manager.detect_storage_type("");
        assert!(result.is_err());
    }

    #[test]
    fn test_list_snapshots_empty() {
        let manager = VmSnapshotManager::new("/tmp/snapshots".to_string());
        let snapshots = manager.list_snapshots("vm-100");
        assert_eq!(snapshots.len(), 0);
    }

    #[test]
    fn test_get_snapshot_not_found() {
        let manager = VmSnapshotManager::new("/tmp/snapshots".to_string());
        let result = manager.get_snapshot("non-existent");
        assert!(result.is_none());
    }

    #[test]
    fn test_vm_snapshot_state_equality() {
        assert_eq!(VmSnapshotState::Running, VmSnapshotState::Running);
        assert_eq!(VmSnapshotState::Stopped, VmSnapshotState::Stopped);
        assert_eq!(VmSnapshotState::Paused, VmSnapshotState::Paused);
        assert_ne!(VmSnapshotState::Running, VmSnapshotState::Stopped);
    }

    #[test]
    fn test_storage_type_equality() {
        assert_eq!(StorageType::Zfs, StorageType::Zfs);
        assert_eq!(StorageType::Lvm, StorageType::Lvm);
        assert_eq!(StorageType::Qcow2, StorageType::Qcow2);
        assert_eq!(StorageType::Btrfs, StorageType::Btrfs);
        assert_eq!(StorageType::Ceph, StorageType::Ceph);
        assert_ne!(StorageType::Zfs, StorageType::Lvm);
    }

    #[tokio::test]
    async fn test_create_snapshot_stopped_vm() {
        let mut manager = VmSnapshotManager::new("/tmp/test-snapshots".to_string());
        let mut vm_config = create_test_vm_config();
        vm_config.status = VmStatus::Stopped;

        let result = manager.create_snapshot(
            &vm_config,
            "test-snapshot".to_string(),
            Some("Test description".to_string()),
            false,
        ).await;

        assert!(result.is_ok());
        let snapshot = result.unwrap();
        assert_eq!(snapshot.vm_id, "test-vm-100");
        assert_eq!(snapshot.name, "test-snapshot");
        assert_eq!(snapshot.description, Some("Test description".to_string()));
        assert_eq!(snapshot.vm_state, VmSnapshotState::Stopped);
        assert!(snapshot.memory_snapshot.is_none());
    }

    #[tokio::test]
    async fn test_create_snapshot_running_vm_no_memory() {
        let mut manager = VmSnapshotManager::new("/tmp/test-snapshots".to_string());
        let vm_config = create_test_vm_config();

        let result = manager.create_snapshot(
            &vm_config,
            "live-snapshot".to_string(),
            None,
            false,
        ).await;

        assert!(result.is_ok());
        let snapshot = result.unwrap();
        // VM is paused for consistent disk snapshot when running without memory
        assert_eq!(snapshot.vm_state, VmSnapshotState::Paused);
        assert!(snapshot.memory_snapshot.is_none());
    }

    #[tokio::test]
    async fn test_list_snapshots_filters_by_vm() {
        let mut manager = VmSnapshotManager::new("/tmp/test-snapshots".to_string());

        let mut vm_config1 = create_test_vm_config();
        vm_config1.id = "vm-100".to_string();
        vm_config1.status = VmStatus::Stopped;

        let mut vm_config2 = create_test_vm_config();
        vm_config2.id = "vm-200".to_string();
        vm_config2.status = VmStatus::Stopped;

        // Create snapshots for different VMs
        let _ = manager.create_snapshot(&vm_config1, "snap1".to_string(), None, false).await;
        let _ = manager.create_snapshot(&vm_config1, "snap2".to_string(), None, false).await;
        let _ = manager.create_snapshot(&vm_config2, "snap3".to_string(), None, false).await;

        let vm100_snapshots = manager.list_snapshots("vm-100");
        let vm200_snapshots = manager.list_snapshots("vm-200");

        assert_eq!(vm100_snapshots.len(), 2);
        assert_eq!(vm200_snapshots.len(), 1);
    }

    #[tokio::test]
    async fn test_delete_snapshot() {
        let mut manager = VmSnapshotManager::new("/tmp/test-snapshots".to_string());
        let mut vm_config = create_test_vm_config();
        vm_config.status = VmStatus::Stopped;

        let snapshot = manager.create_snapshot(
            &vm_config,
            "delete-test".to_string(),
            None,
            false,
        ).await.unwrap();

        let snapshot_id = snapshot.id.clone();

        // Verify snapshot exists
        assert!(manager.get_snapshot(&snapshot_id).is_some());

        // Delete snapshot
        let result = manager.delete_snapshot(&snapshot_id).await;
        assert!(result.is_ok());

        // Verify snapshot is gone
        assert!(manager.get_snapshot(&snapshot_id).is_none());
    }

    #[tokio::test]
    async fn test_delete_nonexistent_snapshot() {
        let mut manager = VmSnapshotManager::new("/tmp/test-snapshots".to_string());
        let result = manager.delete_snapshot("non-existent-id").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_snapshot_tree_node_structure() {
        let vm_config = create_test_vm_config();
        let snapshot = VmSnapshot {
            id: "snap-1".to_string(),
            vm_id: "vm-100".to_string(),
            name: "root-snapshot".to_string(),
            description: None,
            created_at: 1234567890,
            parent_snapshot: None,
            vm_state: VmSnapshotState::Stopped,
            disk_snapshots: vec![],
            memory_snapshot: None,
            config_backup: vm_config,
        };

        let tree_node = SnapshotTreeNode {
            snapshot: snapshot.clone(),
            children: vec![],
            is_current: true,
        };

        assert_eq!(tree_node.snapshot.id, "snap-1");
        assert_eq!(tree_node.children.len(), 0);
        assert!(tree_node.is_current);
    }

    #[test]
    fn test_disk_snapshot_structure() {
        let disk_snapshot = DiskSnapshot {
            disk_id: "disk-0".to_string(),
            storage_type: StorageType::Qcow2,
            snapshot_name: "snap-001".to_string(),
            snapshot_path: "/var/lib/vz/images/100/snap-001.qcow2".to_string(),
            size_bytes: 1073741824, // 1GB
        };

        assert_eq!(disk_snapshot.disk_id, "disk-0");
        assert_eq!(disk_snapshot.storage_type, StorageType::Qcow2);
        assert_eq!(disk_snapshot.size_bytes, 1073741824);
    }

    #[tokio::test]
    async fn test_snapshot_metadata_persistence() {
        use tokio::fs;

        let test_dir = "/tmp/test-snapshot-metadata";
        let mut manager = VmSnapshotManager::new(test_dir.to_string());
        let mut vm_config = create_test_vm_config();
        vm_config.status = VmStatus::Stopped;

        // Create snapshot
        let snapshot = manager.create_snapshot(
            &vm_config,
            "persist-test".to_string(),
            Some("Testing persistence".to_string()),
            false,
        ).await.unwrap();

        let snapshot_id = snapshot.id.clone();

        // Verify metadata file exists
        let metadata_file = format!("{}/{}.json", test_dir, snapshot_id);
        assert!(fs::try_exists(&metadata_file).await.unwrap_or(false));

        // Clean up
        let _ = fs::remove_dir_all(test_dir).await;
    }
}
