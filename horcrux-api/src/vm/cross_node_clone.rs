//! Cross-Node VM Cloning
//!
//! Provides functionality to clone VMs from one node to another in a cluster.
//! Uses SSH for secure disk transfer and supports all major storage backends.

#![allow(dead_code)]

use horcrux_common::{Result, VmConfig};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::process::Command;
use tracing::{info, warn};

use super::clone::CloneOptions;

/// Cross-node clone configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossNodeCloneConfig {
    pub source_node: String,
    pub target_node: String,
    pub source_vm_id: String,
    pub clone_options: CloneOptions,
    pub ssh_port: Option<u16>,
    pub ssh_user: Option<String>,
    pub compression_enabled: bool,
    pub bandwidth_limit_mbps: Option<u32>,
    /// Target LVM volume group (defaults to "vg0")
    pub target_volume_group: Option<String>,
}

/// Cross-node clone job status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossNodeCloneJob {
    pub job_id: String,
    pub config: CrossNodeCloneConfig,
    pub state: CloneJobState,
    pub progress_percent: f32,
    pub transferred_bytes: u64,
    pub total_bytes: u64,
    pub transfer_rate_mbps: f64,
    pub started_at: i64,
    pub completed_at: Option<i64>,
    pub error: Option<String>,
}

/// Clone job state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CloneJobState {
    Preparing,
    TransferringDisks,
    CreatingVm,
    Completed,
    Failed,
}

/// Cross-node clone manager
pub struct CrossNodeCloneManager {
    storage_path: PathBuf,
}

impl CrossNodeCloneManager {
    pub fn new(storage_path: String) -> Self {
        Self {
            storage_path: PathBuf::from(storage_path),
        }
    }

    /// Clone a VM from one node to another
    pub async fn clone_cross_node(
        &self,
        source_vm: &VmConfig,
        config: CrossNodeCloneConfig,
    ) -> Result<VmConfig> {
        info!(
            "Starting cross-node clone of VM {} from {} to {}",
            source_vm.id, config.source_node, config.target_node
        );

        // Verify SSH connectivity
        self.verify_ssh_connectivity(&config).await?;

        // Create target directories
        self.create_target_directories(&config).await?;

        // Transfer all disks
        for (idx, disk) in source_vm.disks.iter().enumerate() {
            info!("Transferring disk {} ({}/{})", disk.path, idx + 1, source_vm.disks.len());
            self.transfer_disk(disk, &config, idx).await?;
        }

        // Create VM configuration on target node
        let cloned_vm = self.create_target_vm_config(source_vm, &config).await?;

        info!(
            "Cross-node clone completed: VM {} cloned to {} on node {}",
            source_vm.id, cloned_vm.id, config.target_node
        );

        Ok(cloned_vm)
    }

    /// Verify SSH connectivity to both source and target nodes
    async fn verify_ssh_connectivity(&self, config: &CrossNodeCloneConfig) -> Result<()> {
        info!("Verifying SSH connectivity to source and target nodes");

        let ssh_user = config.ssh_user.as_deref().unwrap_or("root");
        let ssh_port = config.ssh_port.unwrap_or(22);

        // Test source node
        let source_test = Command::new("ssh")
            .arg("-p")
            .arg(ssh_port.to_string())
            .arg(format!("{}@{}", ssh_user, config.source_node))
            .arg("echo")
            .arg("connected")
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to connect to source node: {}", e))
            })?;

        if !source_test.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "Cannot connect to source node {}: {}",
                config.source_node,
                String::from_utf8_lossy(&source_test.stderr)
            )));
        }

        // Test target node
        let target_test = Command::new("ssh")
            .arg("-p")
            .arg(ssh_port.to_string())
            .arg(format!("{}@{}", ssh_user, config.target_node))
            .arg("echo")
            .arg("connected")
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to connect to target node: {}", e))
            })?;

        if !target_test.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "Cannot connect to target node {}: {}",
                config.target_node,
                String::from_utf8_lossy(&target_test.stderr)
            )));
        }

        info!("SSH connectivity verified successfully");
        Ok(())
    }

    /// Create necessary directories on target node
    async fn create_target_directories(&self, config: &CrossNodeCloneConfig) -> Result<()> {
        let ssh_user = config.ssh_user.as_deref().unwrap_or("root");
        let ssh_port = config.ssh_port.unwrap_or(22);

        info!("Creating target directories on {}", config.target_node);

        let output = Command::new("ssh")
            .arg("-p")
            .arg(ssh_port.to_string())
            .arg(format!("{}@{}", ssh_user, config.target_node))
            .arg("mkdir")
            .arg("-p")
            .arg(self.storage_path.to_string_lossy().to_string())
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to create target directories: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create directories on target: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Transfer a disk from source to target node
    async fn transfer_disk(
        &self,
        disk: &horcrux_common::VmDisk,
        config: &CrossNodeCloneConfig,
        disk_index: usize,
    ) -> Result<()> {
        let ssh_user = config.ssh_user.as_deref().unwrap_or("root");
        let ssh_port = config.ssh_port.unwrap_or(22);

        // Generate target disk path
        let new_vm_id = config.clone_options.id.as_ref()
            .ok_or_else(|| horcrux_common::Error::System("Clone ID must be provided for cross-node clone".to_string()))?;

        let extension = std::path::Path::new(&disk.path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("qcow2");

        let target_filename = if disk_index == 0 {
            format!("{}.{}", new_vm_id, extension)
        } else {
            format!("{}-disk{}.{}", new_vm_id, disk_index, extension)
        };

        let target_path = self.storage_path.join(&target_filename);

        info!(
            "Transferring {} from {} to {} on {}",
            disk.path, config.source_node, target_path.display(), config.target_node
        );

        // Build SSH command with compression and bandwidth limiting
        let mut ssh_cmd = format!("ssh -p {}", ssh_port);

        if config.compression_enabled {
            ssh_cmd.push_str(" -C"); // Enable SSH compression
        }

        // Use different transfer methods based on storage type
        if disk.path.ends_with(".qcow2") || disk.path.ends_with(".raw") || disk.path.ends_with(".img") {
            // File-based disk - use rsync or scp
            self.transfer_file_disk(disk, config, &target_path, ssh_user, ssh_port).await?;
        } else if disk.path.starts_with("/dev/zvol/") {
            // ZFS volume - use zfs send/receive
            self.transfer_zfs_disk(disk, config, &target_path, ssh_user, ssh_port).await?;
        } else if disk.path.starts_with("/dev/") && disk.path.contains("/lv") {
            // LVM volume - use dd over SSH
            self.transfer_lvm_disk(disk, config, &target_path, ssh_user, ssh_port).await?;
        } else if !disk.path.starts_with("/dev/") && disk.path.contains('/') {
            // Ceph RBD - use rbd export/import
            self.transfer_ceph_disk(disk, config, &target_path, ssh_user, ssh_port).await?;
        } else {
            // Default to file-based transfer
            self.transfer_file_disk(disk, config, &target_path, ssh_user, ssh_port).await?;
        }

        info!("Disk transfer completed: {}", disk.path);
        Ok(())
    }

    /// Transfer file-based disk (qcow2, raw, etc.)
    async fn transfer_file_disk(
        &self,
        disk: &horcrux_common::VmDisk,
        config: &CrossNodeCloneConfig,
        target_path: &PathBuf,
        ssh_user: &str,
        ssh_port: u16,
    ) -> Result<()> {
        let source_ssh = format!("{}@{}", ssh_user, config.source_node);
        let target_ssh = format!("{}@{}", ssh_user, config.target_node);

        // Use rsync for efficient transfer with progress
        let mut rsync_cmd = Command::new("rsync");
        rsync_cmd
            .arg("-av")
            .arg("--progress")
            .arg("-e")
            .arg(format!("ssh -p {}", ssh_port));

        // Add bandwidth limit if specified
        if let Some(limit_mbps) = config.bandwidth_limit_mbps {
            let limit_kbps = limit_mbps * 1024;
            rsync_cmd.arg(format!("--bwlimit={}", limit_kbps));
        }

        // Enable compression if requested
        if config.compression_enabled {
            rsync_cmd.arg("-z");
        }

        // Source: ssh to source node and read the file
        rsync_cmd.arg(format!("{}:{}", source_ssh, disk.path));

        // Target: ssh to target node and write the file
        rsync_cmd.arg(format!("{}:{}", target_ssh, target_path.display()));

        let output = rsync_cmd.output().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to execute rsync: {}", e))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Rsync transfer failed: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Transfer ZFS volume using zfs send/receive
    async fn transfer_zfs_disk(
        &self,
        disk: &horcrux_common::VmDisk,
        config: &CrossNodeCloneConfig,
        target_path: &PathBuf,
        ssh_user: &str,
        ssh_port: u16,
    ) -> Result<()> {
        let zvol_name = disk.path.strip_prefix("/dev/zvol/")
            .ok_or_else(|| horcrux_common::Error::System("Invalid ZFS path".to_string()))?;

        // Create snapshot on source
        let snapshot_name = format!("{}@cross-clone-{}", zvol_name, uuid::Uuid::new_v4());

        info!("Creating ZFS snapshot: {}", snapshot_name);

        let source_ssh = format!("{}@{}", ssh_user, config.source_node);

        let snapshot_cmd = Command::new("ssh")
            .arg("-p")
            .arg(ssh_port.to_string())
            .arg(&source_ssh)
            .arg("zfs")
            .arg("snapshot")
            .arg(&snapshot_name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to create snapshot: {}", e))
            })?;

        if !snapshot_cmd.status.success() {
            let stderr = String::from_utf8_lossy(&snapshot_cmd.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create ZFS snapshot: {}",
                stderr
            )));
        }

        // Extract target zvol name from target_path
        let target_zvol = target_path.to_string_lossy();

        // Send snapshot to target node
        info!("Transferring ZFS snapshot to target node");

        let target_ssh = format!("{}@{}", ssh_user, config.target_node);

        // Build zfs send | ssh | zfs receive pipeline
        let send_cmd = format!(
            "ssh -p {} {} zfs send {} | ssh -p {} {} zfs receive -F {}",
            ssh_port, source_ssh, snapshot_name,
            ssh_port, target_ssh, target_zvol
        );

        let output = Command::new("sh")
            .arg("-c")
            .arg(&send_cmd)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to transfer ZFS volume: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("ZFS transfer warning: {}", stderr);
        }

        // Clean up snapshot on source
        let _ = Command::new("ssh")
            .arg("-p")
            .arg(ssh_port.to_string())
            .arg(&source_ssh)
            .arg("zfs")
            .arg("destroy")
            .arg(&snapshot_name)
            .output()
            .await;

        Ok(())
    }

    /// Transfer LVM volume using dd over SSH
    async fn transfer_lvm_disk(
        &self,
        disk: &horcrux_common::VmDisk,
        config: &CrossNodeCloneConfig,
        target_path: &PathBuf,
        ssh_user: &str,
        ssh_port: u16,
    ) -> Result<()> {
        info!("Transferring LVM volume via dd over SSH");

        let source_ssh = format!("{}@{}", ssh_user, config.source_node);
        let target_ssh = format!("{}@{}", ssh_user, config.target_node);

        // Get source LV size
        let size_cmd = format!(
            "ssh -p {} {} lvs --noheadings -o lv_size --units g {}",
            ssh_port, source_ssh, disk.path
        );

        let size_output = Command::new("sh")
            .arg("-c")
            .arg(&size_cmd)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to get LV size: {}", e))
            })?;

        if !size_output.status.success() {
            return Err(horcrux_common::Error::System(
                "Failed to determine LV size".to_string()
            ));
        }

        let size_str = String::from_utf8_lossy(&size_output.stdout);
        let size = size_str.trim().trim_end_matches('G');

        // Create target LV
        let target_vg = config.target_volume_group.as_deref().unwrap_or("vg0");
        let target_lv_name = target_path.file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| horcrux_common::Error::System("Invalid target path".to_string()))?;

        let create_cmd = format!(
            "ssh -p {} {} lvcreate -L {}G -n {} {}",
            ssh_port, target_ssh, size, target_lv_name, target_vg
        );

        let _ = Command::new("sh")
            .arg("-c")
            .arg(&create_cmd)
            .output()
            .await;

        // Transfer using dd with compression
        let dd_cmd = if config.compression_enabled {
            format!(
                "ssh -p {} {} dd if={} bs=4M status=progress | gzip -c | ssh -p {} {} 'gunzip -c | dd of=/dev/{}/{} bs=4M'",
                ssh_port, source_ssh, disk.path,
                ssh_port, target_ssh, target_vg, target_lv_name
            )
        } else {
            format!(
                "ssh -p {} {} dd if={} bs=4M status=progress | ssh -p {} {} dd of=/dev/{}/{} bs=4M",
                ssh_port, source_ssh, disk.path,
                ssh_port, target_ssh, target_vg, target_lv_name
            )
        };

        let output = Command::new("sh")
            .arg("-c")
            .arg(&dd_cmd)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to transfer LVM volume: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "LVM transfer failed: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Transfer Ceph RBD volume
    async fn transfer_ceph_disk(
        &self,
        disk: &horcrux_common::VmDisk,
        config: &CrossNodeCloneConfig,
        target_path: &PathBuf,
        ssh_user: &str,
        ssh_port: u16,
    ) -> Result<()> {
        info!("Transferring Ceph RBD volume");

        let source_ssh = format!("{}@{}", ssh_user, config.source_node);
        let target_ssh = format!("{}@{}", ssh_user, config.target_node);

        // RBD export/import pipeline
        let rbd_cmd = if config.compression_enabled {
            format!(
                "ssh -p {} {} rbd export {} - | gzip -c | ssh -p {} {} 'gunzip -c | rbd import - {}'",
                ssh_port, source_ssh, disk.path,
                ssh_port, target_ssh, target_path.display()
            )
        } else {
            format!(
                "ssh -p {} {} rbd export {} - | ssh -p {} {} rbd import - {}",
                ssh_port, source_ssh, disk.path,
                ssh_port, target_ssh, target_path.display()
            )
        };

        let output = Command::new("sh")
            .arg("-c")
            .arg(&rbd_cmd)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to transfer RBD volume: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "RBD transfer failed: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Create VM configuration on target node
    async fn create_target_vm_config(
        &self,
        source_vm: &VmConfig,
        config: &CrossNodeCloneConfig,
    ) -> Result<VmConfig> {
        let new_vm_id = config.clone_options.id.clone()
            .ok_or_else(|| horcrux_common::Error::System("Clone ID required".to_string()))?;

        // Build new VM config with updated disk paths
        let mut cloned_vm = VmConfig {
            id: new_vm_id.clone(),
            name: config.clone_options.name.clone(),
            hypervisor: source_vm.hypervisor.clone(),
            memory: source_vm.memory,
            cpus: source_vm.cpus,
            disk_size: source_vm.disk_size,
            status: horcrux_common::VmStatus::Stopped,
            architecture: source_vm.architecture.clone(),
            disks: Vec::new(),
        };

        // Update disk paths for target node
        for (idx, disk) in source_vm.disks.iter().enumerate() {
            let extension = std::path::Path::new(&disk.path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("qcow2");

            let target_filename = if idx == 0 {
                format!("{}.{}", new_vm_id, extension)
            } else {
                format!("{}-disk{}.{}", new_vm_id, idx, extension)
            };

            let target_disk_path = self.storage_path.join(&target_filename);

            cloned_vm.disks.push(horcrux_common::VmDisk {
                path: target_disk_path.to_string_lossy().to_string(),
                size_gb: disk.size_gb,
                disk_type: disk.disk_type.clone(),
                cache: disk.cache.clone(),
            });
        }

        Ok(cloned_vm)
    }

    /// Estimate transfer size for a VM
    pub async fn estimate_transfer_size(&self, vm: &VmConfig, source_node: &str) -> Result<u64> {
        let mut total_size = 0u64;

        for disk in &vm.disks {
            // Try to get actual disk size from source node
            let size = self.get_disk_size(&disk.path, source_node).await
                .unwrap_or((disk.size_gb as u64) * 1024 * 1024 * 1024);
            total_size += size;
        }

        Ok(total_size)
    }

    /// Get disk size from remote node
    async fn get_disk_size(&self, disk_path: &str, node: &str) -> Result<u64> {
        let output = Command::new("ssh")
            .arg(node)
            .arg("stat")
            .arg("-c")
            .arg("%s")
            .arg(disk_path)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to get disk size: {}", e)))?;

        if output.status.success() {
            let size_str = String::from_utf8_lossy(&output.stdout);
            let size = size_str.trim().parse::<u64>()
                .map_err(|e| horcrux_common::Error::System(format!("Failed to parse size: {}", e)))?;
            Ok(size)
        } else {
            Err(horcrux_common::Error::System("Failed to retrieve disk size".to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::clone::CloneMode;

    #[test]
    fn test_cross_node_clone_manager_new() {
        let manager = CrossNodeCloneManager::new("/var/lib/horcrux/vms".to_string());
        assert_eq!(manager.storage_path, PathBuf::from("/var/lib/horcrux/vms"));
    }

    #[test]
    fn test_clone_job_state_equality() {
        assert_eq!(CloneJobState::Preparing, CloneJobState::Preparing);
        assert_eq!(CloneJobState::TransferringDisks, CloneJobState::TransferringDisks);
        assert_eq!(CloneJobState::CreatingVm, CloneJobState::CreatingVm);
        assert_eq!(CloneJobState::Completed, CloneJobState::Completed);
        assert_eq!(CloneJobState::Failed, CloneJobState::Failed);
        assert_ne!(CloneJobState::Preparing, CloneJobState::Completed);
    }

    #[test]
    fn test_cross_node_clone_config() {
        let clone_options = CloneOptions {
            name: "cross-node-vm".to_string(),
            id: Some("vm-200".to_string()),
            mode: CloneMode::Full,
            start: false,
            mac_addresses: None,
            description: Some("Cross-node clone test".to_string()),
            network_config: None,
        };

        let config = CrossNodeCloneConfig {
            source_node: "node1.example.com".to_string(),
            target_node: "node2.example.com".to_string(),
            source_vm_id: "vm-100".to_string(),
            clone_options,
            ssh_port: Some(22),
            ssh_user: Some("root".to_string()),
            compression_enabled: true,
            bandwidth_limit_mbps: Some(100),
        };

        assert_eq!(config.source_node, "node1.example.com");
        assert_eq!(config.target_node, "node2.example.com");
        assert_eq!(config.ssh_port, Some(22));
        assert!(config.compression_enabled);
        assert_eq!(config.bandwidth_limit_mbps, Some(100));
    }

    #[test]
    fn test_cross_node_clone_job() {
        let clone_options = CloneOptions {
            name: "test-vm".to_string(),
            id: Some("vm-300".to_string()),
            mode: CloneMode::Full,
            start: false,
            mac_addresses: None,
            description: None,
            network_config: None,
        };

        let config = CrossNodeCloneConfig {
            source_node: "node1".to_string(),
            target_node: "node2".to_string(),
            source_vm_id: "vm-100".to_string(),
            clone_options,
            ssh_port: None,
            ssh_user: None,
            compression_enabled: false,
            bandwidth_limit_mbps: None,
        };

        let job = CrossNodeCloneJob {
            job_id: "job-123".to_string(),
            config,
            state: CloneJobState::Preparing,
            progress_percent: 0.0,
            transferred_bytes: 0,
            total_bytes: 10_000_000_000,
            transfer_rate_mbps: 0.0,
            started_at: chrono::Utc::now().timestamp(),
            completed_at: None,
            error: None,
        };

        assert_eq!(job.job_id, "job-123");
        assert_eq!(job.state, CloneJobState::Preparing);
        assert_eq!(job.progress_percent, 0.0);
        assert!(job.error.is_none());
        assert!(job.completed_at.is_none());
    }
}
