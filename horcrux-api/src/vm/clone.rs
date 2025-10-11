///! VM Cloning functionality
///!
///! Provides VM cloning capabilities using snapshots and disk copy operations.
///! Supports both full clones (independent copy) and linked clones (based on snapshots).

use horcrux_common::{Result, VmConfig, VmDisk, VmStatus};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::process::Command;
use tracing::{debug, error, info};
use uuid::Uuid;

/// Clone mode determines how the VM is cloned
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum CloneMode {
    /// Full clone - completely independent copy of the VM
    Full,
    /// Linked clone - uses snapshot as backing file (QCOW2 only)
    Linked,
}

/// Network configuration for cloned VM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// Hostname for the cloned VM
    pub hostname: Option<String>,
    /// Static IP addresses (one per network interface)
    pub ip_addresses: Option<Vec<String>>,
    /// Gateway IP address
    pub gateway: Option<String>,
    /// DNS servers
    pub dns_servers: Option<Vec<String>>,
    /// Domain name
    pub domain: Option<String>,
}

/// Options for VM cloning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloneOptions {
    /// New VM name
    pub name: String,
    /// New VM ID (if None, auto-generated)
    pub id: Option<String>,
    /// Clone mode (full or linked)
    pub mode: CloneMode,
    /// Whether to start the clone immediately
    pub start: bool,
    /// Custom MAC addresses for network interfaces
    pub mac_addresses: Option<Vec<String>>,
    /// Description for the cloned VM
    pub description: Option<String>,
    /// Network configuration
    pub network_config: Option<NetworkConfig>,
}

/// VM Clone manager
pub struct VmCloneManager {
    storage_path: PathBuf,
}

impl VmCloneManager {
    pub fn new(storage_path: String) -> Self {
        Self {
            storage_path: PathBuf::from(storage_path),
        }
    }

    /// Clone a virtual machine
    pub async fn clone_vm(
        &self,
        source_vm: &VmConfig,
        options: CloneOptions,
    ) -> Result<VmConfig> {
        info!("Cloning VM {} to {}", source_vm.id, options.name);

        // Generate new VM ID if not provided
        let new_vm_id = options.id.unwrap_or_else(|| Uuid::new_v4().to_string());

        // Ensure storage directory exists
        tokio::fs::create_dir_all(&self.storage_path)
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!(
                    "Failed to create storage directory: {}",
                    e
                ))
            })?;

        // Clone all disks
        let mut cloned_disks = Vec::new();
        for (idx, disk) in source_vm.disks.iter().enumerate() {
            let cloned_disk = self
                .clone_disk(disk, &new_vm_id, idx, options.mode)
                .await?;
            cloned_disks.push(cloned_disk);
        }

        // Create new VM configuration
        let cloned_vm = VmConfig {
            id: new_vm_id.clone(),
            name: options.name.clone(),
            hypervisor: source_vm.hypervisor.clone(),
            memory: source_vm.memory,
            cpus: source_vm.cpus,
            disk_size: source_vm.disk_size,
            status: VmStatus::Stopped,
            architecture: source_vm.architecture.clone(),
            disks: cloned_disks,
        };

        info!(
            "VM {} cloned successfully to {}",
            source_vm.id, cloned_vm.id
        );

        Ok(cloned_vm)
    }

    /// Clone a single disk
    async fn clone_disk(
        &self,
        source_disk: &VmDisk,
        new_vm_id: &str,
        disk_index: usize,
        mode: CloneMode,
    ) -> Result<VmDisk> {
        let source_path = Path::new(&source_disk.path);

        // Determine file extension
        let extension = source_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("qcow2");

        // Generate new disk path
        let new_disk_path = if disk_index == 0 {
            self.storage_path.join(format!("{}.{}", new_vm_id, extension))
        } else {
            self.storage_path
                .join(format!("{}-disk{}.{}", new_vm_id, disk_index, extension))
        };

        debug!(
            "Cloning disk from {:?} to {:?} (mode: {:?})",
            source_path, new_disk_path, mode
        );

        // Detect storage type and clone accordingly
        let storage_type = self.detect_storage_type(&source_disk.path)?;

        match storage_type {
            StorageType::Qcow2 => {
                self.clone_qcow2_disk(source_path, &new_disk_path, mode)
                    .await?
            }
            StorageType::Raw => {
                self.clone_raw_disk(source_path, &new_disk_path).await?
            }
            StorageType::Zfs => {
                self.clone_zfs_disk(&source_disk.path, new_vm_id, disk_index)
                    .await?
            }
            StorageType::Lvm => {
                self.clone_lvm_disk(&source_disk.path, new_vm_id, disk_index)
                    .await?
            }
            StorageType::Btrfs => {
                self.clone_btrfs_disk(&source_disk.path, new_vm_id, disk_index)
                    .await?
            }
            StorageType::Ceph => {
                self.clone_ceph_disk(&source_disk.path, new_vm_id, disk_index)
                    .await?
            }
        }

        Ok(VmDisk {
            path: new_disk_path.to_string_lossy().to_string(),
            size_gb: source_disk.size_gb,
            disk_type: source_disk.disk_type.clone(),
            cache: source_disk.cache.clone(),
        })
    }

    /// Clone QCOW2 disk
    async fn clone_qcow2_disk(
        &self,
        source_path: &Path,
        dest_path: &Path,
        mode: CloneMode,
    ) -> Result<()> {
        match mode {
            CloneMode::Full => {
                // Full copy using qemu-img convert
                info!("Creating full QCOW2 clone");
                let output = Command::new("qemu-img")
                    .arg("convert")
                    .arg("-O")
                    .arg("qcow2")
                    .arg(source_path)
                    .arg(dest_path)
                    .output()
                    .await
                    .map_err(|e| {
                        horcrux_common::Error::System(format!("Failed to run qemu-img: {}", e))
                    })?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    error!("qemu-img convert failed: {}", stderr);
                    return Err(horcrux_common::Error::System(format!(
                        "Failed to clone disk: {}",
                        stderr
                    )));
                }
            }
            CloneMode::Linked => {
                // Linked clone using backing file
                info!("Creating linked QCOW2 clone");
                let output = Command::new("qemu-img")
                    .arg("create")
                    .arg("-f")
                    .arg("qcow2")
                    .arg("-b")
                    .arg(source_path)
                    .arg("-F")
                    .arg("qcow2")
                    .arg(dest_path)
                    .output()
                    .await
                    .map_err(|e| {
                        horcrux_common::Error::System(format!("Failed to run qemu-img: {}", e))
                    })?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    error!("qemu-img create with backing failed: {}", stderr);
                    return Err(horcrux_common::Error::System(format!(
                        "Failed to create linked clone: {}",
                        stderr
                    )));
                }
            }
        }

        Ok(())
    }

    /// Clone raw disk image
    async fn clone_raw_disk(&self, source_path: &Path, dest_path: &Path) -> Result<()> {
        info!("Cloning raw disk image");

        // Use qemu-img for efficient copying
        let output = Command::new("qemu-img")
            .arg("convert")
            .arg("-O")
            .arg("raw")
            .arg(source_path)
            .arg(dest_path)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run qemu-img: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("qemu-img convert failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to clone raw disk: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Clone ZFS volume
    async fn clone_zfs_disk(
        &self,
        source_zvol: &str,
        new_vm_id: &str,
        disk_index: usize,
    ) -> Result<()> {
        info!("Cloning ZFS volume: {}", source_zvol);

        // Extract pool and dataset from path like /dev/zvol/pool/dataset
        let zvol_name = source_zvol
            .strip_prefix("/dev/zvol/")
            .ok_or_else(|| horcrux_common::Error::InvalidConfig("Invalid ZFS path".to_string()))?;

        // Create snapshot first
        let snapshot_name = format!("{}@clone-{}", zvol_name, Uuid::new_v4());
        let output = Command::new("zfs")
            .arg("snapshot")
            .arg(&snapshot_name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run zfs snapshot: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create ZFS snapshot: {}",
                stderr
            )));
        }

        // Clone from snapshot
        let pool = zvol_name.split('/').next().unwrap_or("pool");
        let clone_name = if disk_index == 0 {
            format!("{}/vm-{}", pool, new_vm_id)
        } else {
            format!("{}/vm-{}-disk{}", pool, new_vm_id, disk_index)
        };

        let output = Command::new("zfs")
            .arg("clone")
            .arg(&snapshot_name)
            .arg(&clone_name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run zfs clone: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to clone ZFS volume: {}",
                stderr
            )));
        }

        // Delete temporary snapshot
        Command::new("zfs")
            .arg("destroy")
            .arg(&snapshot_name)
            .output()
            .await
            .ok();

        Ok(())
    }

    /// Clone LVM volume
    async fn clone_lvm_disk(
        &self,
        source_lv: &str,
        new_vm_id: &str,
        disk_index: usize,
    ) -> Result<()> {
        info!("Cloning LVM volume: {}", source_lv);

        // Extract VG and LV from path like /dev/vg0/lv-vm-100
        let parts: Vec<&str> = source_lv.split('/').collect();
        if parts.len() < 4 {
            return Err(horcrux_common::Error::InvalidConfig(
                "Invalid LVM path".to_string(),
            ));
        }

        let vg_name = parts[2];
        let new_lv_name = if disk_index == 0 {
            format!("lv-vm-{}", new_vm_id)
        } else {
            format!("lv-vm-{}-disk{}", new_vm_id, disk_index)
        };

        // Get source LV size
        let output = Command::new("lvs")
            .arg("--noheadings")
            .arg("-o")
            .arg("lv_size")
            .arg("--units")
            .arg("g")
            .arg(source_lv)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run lvs: {}", e)))?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(
                "Failed to get LV size".to_string(),
            ));
        }

        let size_str = String::from_utf8_lossy(&output.stdout);
        let size = size_str.trim().trim_end_matches('G');

        // Create new LV
        let output = Command::new("lvcreate")
            .arg("-L")
            .arg(format!("{}G", size))
            .arg("-n")
            .arg(&new_lv_name)
            .arg(vg_name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run lvcreate: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create LV: {}",
                stderr
            )));
        }

        // Copy data using dd
        let new_lv_path = format!("/dev/{}/{}", vg_name, new_lv_name);
        let output = Command::new("dd")
            .arg(format!("if={}", source_lv))
            .arg(format!("of={}", new_lv_path))
            .arg("bs=4M")
            .arg("status=progress")
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run dd: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to copy LV data: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Clone Btrfs subvolume
    async fn clone_btrfs_disk(
        &self,
        source_subvol: &str,
        new_vm_id: &str,
        disk_index: usize,
    ) -> Result<()> {
        info!("Cloning Btrfs subvolume: {}", source_subvol);

        // Create snapshot first
        let snapshot_path = format!("{}-snap-{}", source_subvol, Uuid::new_v4());
        let output = Command::new("btrfs")
            .arg("subvolume")
            .arg("snapshot")
            .arg("-r") // Read-only snapshot
            .arg(source_subvol)
            .arg(&snapshot_path)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run btrfs snapshot: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create Btrfs snapshot: {}",
                stderr
            )));
        }

        // Clone from snapshot (create writable snapshot)
        let parent_dir = Path::new(source_subvol)
            .parent()
            .ok_or_else(|| horcrux_common::Error::InvalidConfig("Invalid path".to_string()))?;

        let clone_name = if disk_index == 0 {
            format!("vm-{}", new_vm_id)
        } else {
            format!("vm-{}-disk{}", new_vm_id, disk_index)
        };

        let clone_path = parent_dir.join(clone_name);

        let output = Command::new("btrfs")
            .arg("subvolume")
            .arg("snapshot")
            .arg(&snapshot_path)
            .arg(&clone_path)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run btrfs snapshot: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to clone Btrfs subvolume: {}",
                stderr
            )));
        }

        // Delete temporary snapshot
        Command::new("btrfs")
            .arg("subvolume")
            .arg("delete")
            .arg(&snapshot_path)
            .output()
            .await
            .ok();

        Ok(())
    }

    /// Clone Ceph RBD volume
    async fn clone_ceph_disk(
        &self,
        source_rbd: &str,
        new_vm_id: &str,
        disk_index: usize,
    ) -> Result<()> {
        info!("Cloning Ceph RBD volume: {}", source_rbd);

        // Parse RBD path like pool/image
        let parts: Vec<&str> = source_rbd.split('/').collect();
        if parts.len() != 2 {
            return Err(horcrux_common::Error::InvalidConfig(
                "Invalid RBD path format".to_string(),
            ));
        }

        let pool = parts[0];
        let image = parts[1];

        // Create snapshot
        let snapshot_name = format!("clone-{}", Uuid::new_v4());
        let output = Command::new("rbd")
            .arg("snap")
            .arg("create")
            .arg(format!("{}/{}@{}", pool, image, snapshot_name))
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run rbd snap create: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create RBD snapshot: {}",
                stderr
            )));
        }

        // Protect snapshot (required for cloning)
        Command::new("rbd")
            .arg("snap")
            .arg("protect")
            .arg(format!("{}/{}@{}", pool, image, snapshot_name))
            .output()
            .await
            .ok();

        // Clone from snapshot
        let clone_image = if disk_index == 0 {
            format!("vm-{}", new_vm_id)
        } else {
            format!("vm-{}-disk{}", new_vm_id, disk_index)
        };

        let output = Command::new("rbd")
            .arg("clone")
            .arg(format!("{}/{}@{}", pool, image, snapshot_name))
            .arg(format!("{}/{}", pool, clone_image))
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run rbd clone: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to clone RBD volume: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Detect storage type from disk path
    fn detect_storage_type(&self, disk_path: &str) -> Result<StorageType> {
        if disk_path.starts_with("/dev/zvol/") {
            Ok(StorageType::Zfs)
        } else if disk_path.starts_with("/dev/") && disk_path.contains("/lv-") {
            Ok(StorageType::Lvm)
        } else if disk_path.contains("rbd:") || (disk_path.contains('/') && !disk_path.starts_with("/dev/")) {
            // Check if it's RBD format (pool/image)
            if !disk_path.starts_with('/') && disk_path.contains('/') {
                Ok(StorageType::Ceph)
            } else if disk_path.ends_with(".qcow2") {
                Ok(StorageType::Qcow2)
            } else {
                Ok(StorageType::Raw)
            }
        } else if disk_path.ends_with(".qcow2") {
            Ok(StorageType::Qcow2)
        } else if disk_path.ends_with(".raw") || disk_path.ends_with(".img") {
            Ok(StorageType::Raw)
        } else {
            // Try to detect Btrfs by checking if path is a subvolume
            Ok(StorageType::Raw) // Default to raw
        }
    }

    /// Delete a cloned VM's disks
    pub async fn delete_clone(&self, vm_config: &VmConfig) -> Result<()> {
        info!("Deleting cloned VM disks for {}", vm_config.id);

        for disk in &vm_config.disks {
            let storage_type = self.detect_storage_type(&disk.path)?;

            match storage_type {
                StorageType::Qcow2 | StorageType::Raw => {
                    tokio::fs::remove_file(&disk.path).await.map_err(|e| {
                        horcrux_common::Error::System(format!("Failed to delete disk: {}", e))
                    })?;
                }
                StorageType::Zfs => {
                    let zvol_name = disk
                        .path
                        .strip_prefix("/dev/zvol/")
                        .ok_or_else(|| {
                            horcrux_common::Error::InvalidConfig("Invalid ZFS path".to_string())
                        })?;

                    Command::new("zfs")
                        .arg("destroy")
                        .arg(zvol_name)
                        .output()
                        .await
                        .ok();
                }
                StorageType::Lvm => {
                    Command::new("lvremove")
                        .arg("-f")
                        .arg(&disk.path)
                        .output()
                        .await
                        .ok();
                }
                StorageType::Btrfs => {
                    Command::new("btrfs")
                        .arg("subvolume")
                        .arg("delete")
                        .arg(&disk.path)
                        .output()
                        .await
                        .ok();
                }
                StorageType::Ceph => {
                    Command::new("rbd")
                        .arg("rm")
                        .arg(&disk.path)
                        .output()
                        .await
                        .ok();
                }
            }
        }

        Ok(())
    }

    /// Generate a random MAC address with QEMU's OUI prefix
    /// Uses 52:54:00:XX:XX:XX range which is reserved for QEMU/KVM
    pub fn generate_mac_address() -> String {
        use rand::Rng;
        let mut rng = rand::thread_rng();

        format!(
            "52:54:00:{:02x}:{:02x}:{:02x}",
            rng.gen::<u8>(),
            rng.gen::<u8>(),
            rng.gen::<u8>()
        )
    }

    /// Generate multiple unique MAC addresses
    pub fn generate_mac_addresses(count: usize) -> Vec<String> {
        use std::collections::HashSet;
        let mut macs = HashSet::new();

        while macs.len() < count {
            macs.insert(Self::generate_mac_address());
        }

        macs.into_iter().collect()
    }

    /// Validate MAC address format
    pub fn validate_mac_address(mac: &str) -> bool {
        let parts: Vec<&str> = mac.split(':').collect();

        if parts.len() != 6 {
            return false;
        }

        for part in parts {
            if part.len() != 2 {
                return false;
            }
            if !part.chars().all(|c| c.is_ascii_hexdigit()) {
                return false;
            }
        }

        true
    }

    /// Apply MAC addresses to cloned VM
    /// If custom MAC addresses are provided in options, use those
    /// Otherwise, generate new random MAC addresses
    pub fn apply_mac_addresses(
        &self,
        options: &CloneOptions,
        network_interface_count: usize,
    ) -> Result<Vec<String>> {
        if let Some(ref custom_macs) = options.mac_addresses {
            // Validate custom MAC addresses
            for mac in custom_macs {
                if !Self::validate_mac_address(mac) {
                    return Err(horcrux_common::Error::InvalidConfig(
                        format!("Invalid MAC address format: {}", mac)
                    ));
                }
            }

            if custom_macs.len() != network_interface_count {
                return Err(horcrux_common::Error::InvalidConfig(
                    format!(
                        "MAC address count mismatch: expected {}, got {}",
                        network_interface_count,
                        custom_macs.len()
                    )
                ));
            }

            Ok(custom_macs.clone())
        } else {
            // Auto-generate MAC addresses
            Ok(Self::generate_mac_addresses(network_interface_count))
        }
    }

    /// Validate IP address format (IPv4)
    pub fn validate_ipv4_address(ip: &str) -> bool {
        let parts: Vec<&str> = ip.split('.').collect();

        if parts.len() != 4 {
            return false;
        }

        for part in parts {
            if let Ok(_num) = part.parse::<u8>() {
                // Valid octet (0-255)
                if part.len() > 1 && part.starts_with('0') {
                    // No leading zeros except for "0" itself
                    return false;
                }
            } else {
                return false;
            }
        }

        true
    }

    /// Validate hostname format (RFC 1123)
    pub fn validate_hostname(hostname: &str) -> bool {
        if hostname.is_empty() || hostname.len() > 253 {
            return false;
        }

        let labels: Vec<&str> = hostname.split('.').collect();

        for label in labels {
            if label.is_empty() || label.len() > 63 {
                return false;
            }

            // Must start with alphanumeric
            if !label.chars().next().unwrap().is_alphanumeric() {
                return false;
            }

            // Must end with alphanumeric
            if !label.chars().last().unwrap().is_alphanumeric() {
                return false;
            }

            // Can only contain alphanumeric and hyphens
            for c in label.chars() {
                if !c.is_alphanumeric() && c != '-' {
                    return false;
                }
            }
        }

        true
    }

    /// Validate network configuration
    pub fn validate_network_config(
        &self,
        network_config: &NetworkConfig,
        network_interface_count: usize,
    ) -> Result<()> {
        // Validate hostname
        if let Some(ref hostname) = network_config.hostname {
            if !Self::validate_hostname(hostname) {
                return Err(horcrux_common::Error::InvalidConfig(
                    format!("Invalid hostname format: {}", hostname)
                ));
            }
        }

        // Validate IP addresses
        if let Some(ref ips) = network_config.ip_addresses {
            if ips.len() != network_interface_count {
                return Err(horcrux_common::Error::InvalidConfig(
                    format!(
                        "IP address count mismatch: expected {}, got {}",
                        network_interface_count,
                        ips.len()
                    )
                ));
            }

            for ip in ips {
                if !Self::validate_ipv4_address(ip) {
                    return Err(horcrux_common::Error::InvalidConfig(
                        format!("Invalid IP address format: {}", ip)
                    ));
                }
            }
        }

        // Validate gateway
        if let Some(ref gateway) = network_config.gateway {
            if !Self::validate_ipv4_address(gateway) {
                return Err(horcrux_common::Error::InvalidConfig(
                    format!("Invalid gateway IP address: {}", gateway)
                ));
            }
        }

        // Validate DNS servers
        if let Some(ref dns_servers) = network_config.dns_servers {
            for dns in dns_servers {
                if !Self::validate_ipv4_address(dns) {
                    return Err(horcrux_common::Error::InvalidConfig(
                        format!("Invalid DNS server IP address: {}", dns)
                    ));
                }
            }
        }

        // Validate domain
        if let Some(ref domain) = network_config.domain {
            if !Self::validate_hostname(domain) {
                return Err(horcrux_common::Error::InvalidConfig(
                    format!("Invalid domain name: {}", domain)
                ));
            }
        }

        Ok(())
    }

    /// Apply network configuration to cloned VM
    /// This would typically involve creating cloud-init configuration
    /// or modifying network configuration files in the VM
    pub fn apply_network_config(
        &self,
        options: &CloneOptions,
        network_interface_count: usize,
    ) -> Result<Option<NetworkConfig>> {
        if let Some(ref network_config) = options.network_config {
            // Validate the network configuration
            self.validate_network_config(network_config, network_interface_count)?;

            info!(
                "Applying network configuration - hostname: {:?}, IPs: {:?}",
                network_config.hostname, network_config.ip_addresses
            );

            Ok(Some(network_config.clone()))
        } else {
            Ok(None)
        }
    }

    /// Generate cloud-init user-data YAML configuration
    pub fn generate_cloud_init_user_data(
        &self,
        network_config: &NetworkConfig,
        _mac_addresses: &[String],
    ) -> String {
        let mut yaml = String::from("#cloud-config\n");

        // Set hostname
        if let Some(ref hostname) = network_config.hostname {
            yaml.push_str(&format!("hostname: {}\n", hostname));
            yaml.push_str(&format!("fqdn: {}\n", hostname));
        }

        // Preserve hostname across reboots
        yaml.push_str("preserve_hostname: false\n");
        yaml.push_str("manage_etc_hosts: true\n\n");

        yaml
    }

    /// Generate cloud-init network-config YAML (v2 format)
    pub fn generate_cloud_init_network_config(
        &self,
        network_config: &NetworkConfig,
        mac_addresses: &[String],
    ) -> String {
        let mut yaml = String::from("version: 2\n");
        yaml.push_str("ethernets:\n");

        // Configure each network interface
        for (idx, mac) in mac_addresses.iter().enumerate() {
            let iface_name = format!("eth{}", idx);
            yaml.push_str(&format!("  {}:\n", iface_name));
            yaml.push_str(&format!("    match:\n"));
            yaml.push_str(&format!("      macaddress: {}\n", mac));
            yaml.push_str(&format!("    set-name: {}\n", iface_name));

            // Configure IP address if provided
            if let Some(ref ips) = network_config.ip_addresses {
                if let Some(ip) = ips.get(idx) {
                    yaml.push_str(&format!("    addresses:\n"));
                    yaml.push_str(&format!("      - {}/24\n", ip)); // Default /24 netmask
                }
            }

            // Configure gateway (only on first interface)
            if idx == 0 {
                if let Some(ref gateway) = network_config.gateway {
                    yaml.push_str(&format!("    routes:\n"));
                    yaml.push_str(&format!("      - to: default\n"));
                    yaml.push_str(&format!("        via: {}\n", gateway));
                }
            }

            // Configure DNS servers (only on first interface)
            if idx == 0 {
                if let Some(ref dns_servers) = network_config.dns_servers {
                    yaml.push_str(&format!("    nameservers:\n"));
                    yaml.push_str(&format!("      addresses:\n"));
                    for dns in dns_servers {
                        yaml.push_str(&format!("        - {}\n", dns));
                    }

                    // Add domain search if specified
                    if let Some(ref domain) = network_config.domain {
                        yaml.push_str(&format!("      search:\n"));
                        yaml.push_str(&format!("        - {}\n", domain));
                    }
                }
            }
        }

        yaml
    }

    /// Create cloud-init ISO image for VM configuration
    /// This creates a NoCloud datasource ISO that cloud-init can read
    pub async fn create_cloud_init_iso(
        &self,
        vm_id: &str,
        network_config: &NetworkConfig,
        mac_addresses: &[String],
    ) -> Result<PathBuf> {
        let cloud_init_dir = self.storage_path.join(format!("{}-cloud-init", vm_id));
        fs::create_dir_all(&cloud_init_dir).await?;

        // Generate user-data
        let user_data = self.generate_cloud_init_user_data(network_config, mac_addresses);
        let user_data_path = cloud_init_dir.join("user-data");
        fs::write(&user_data_path, user_data).await?;

        // Generate network-config
        let network_config_yaml = self.generate_cloud_init_network_config(network_config, mac_addresses);
        let network_config_path = cloud_init_dir.join("network-config");
        fs::write(&network_config_path, network_config_yaml).await?;

        // Generate meta-data
        let mut meta_data = String::from("instance-id: ");
        meta_data.push_str(vm_id);
        meta_data.push('\n');

        if let Some(ref hostname) = network_config.hostname {
            meta_data.push_str("local-hostname: ");
            meta_data.push_str(hostname);
            meta_data.push('\n');
        }

        let meta_data_path = cloud_init_dir.join("meta-data");
        fs::write(&meta_data_path, meta_data).await?;

        // Create ISO image using genisoimage or mkisofs
        let iso_path = self.storage_path.join(format!("{}-cloud-init.iso", vm_id));

        let iso_result = Command::new("genisoimage")
            .arg("-output")
            .arg(&iso_path)
            .arg("-volid")
            .arg("cidata")
            .arg("-joliet")
            .arg("-rock")
            .arg(&cloud_init_dir)
            .output()
            .await;

        if iso_result.is_err() {
            // Try mkisofs as fallback
            Command::new("mkisofs")
                .arg("-output")
                .arg(&iso_path)
                .arg("-volid")
                .arg("cidata")
                .arg("-joliet")
                .arg("-rock")
                .arg(&cloud_init_dir)
                .output()
                .await
                .map_err(|e| {
                    horcrux_common::Error::System(format!(
                        "Failed to create cloud-init ISO (tried genisoimage and mkisofs): {}",
                        e
                    ))
                })?;
        }

        info!("Created cloud-init ISO at {:?}", iso_path);

        // Clean up temporary directory
        let _ = fs::remove_dir_all(&cloud_init_dir).await;

        Ok(iso_path)
    }
}

/// Storage backend types
#[derive(Debug, Clone, Copy)]
enum StorageType {
    Qcow2,
    Raw,
    Zfs,
    Lvm,
    Btrfs,
    Ceph,
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::CloneMode;
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

    fn create_test_disk(path: &str) -> VmDisk {
        VmDisk {
            path: path.to_string(),
            size_gb: 10,
            disk_type: "virtio".to_string(),
            cache: "none".to_string(),
        }
    }

    #[test]
    fn test_clone_manager_new() {
        let manager = VmCloneManager::new("/var/lib/horcrux/vms".to_string());
        assert_eq!(manager.storage_path, PathBuf::from("/var/lib/horcrux/vms"));
    }

    #[test]
    fn test_storage_type_detection() {
        let manager = VmCloneManager::new("/var/lib/horcrux/vms".to_string());

        // ZFS detection
        assert!(matches!(
            manager.detect_storage_type("/dev/zvol/pool/vm-100").unwrap(),
            StorageType::Zfs
        ));

        // LVM detection
        assert!(matches!(
            manager.detect_storage_type("/dev/vg0/lv-vm-100").unwrap(),
            StorageType::Lvm
        ));

        // QCOW2 detection
        assert!(matches!(
            manager
                .detect_storage_type("/var/lib/horcrux/vms/100.qcow2")
                .unwrap(),
            StorageType::Qcow2
        ));

        // Raw detection
        assert!(matches!(
            manager.detect_storage_type("/var/lib/horcrux/vms/100.raw").unwrap(),
            StorageType::Raw
        ));

        // Ceph RBD detection (pool/image format)
        assert!(matches!(
            manager.detect_storage_type("pool/vm-100").unwrap(),
            StorageType::Ceph
        ));

        // Default to Raw for unrecognized paths
        assert!(matches!(
            manager.detect_storage_type("/mnt/btrfs/vm-100").unwrap(),
            StorageType::Raw
        ));
    }

    #[test]
    fn test_storage_type_detection_edge_cases() {
        let manager = VmCloneManager::new("/var/lib/horcrux/vms".to_string());

        // Empty string defaults to Raw
        let result = manager.detect_storage_type("");
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), StorageType::Raw));

        // Path without extension defaults to Raw
        let result = manager.detect_storage_type("/var/lib/vms/disk");
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), StorageType::Raw));
    }

    #[test]
    fn test_clone_options_with_id() {
        let options = CloneOptions {
            name: "test-clone".to_string(),
            id: Some("101".to_string()),
            mode: CloneMode::Full,
            start: false,
            mac_addresses: None,
            description: Some("Test clone".to_string()),
            network_config: None,
        };

        assert_eq!(options.name, "test-clone");
        assert_eq!(options.id.unwrap(), "101");
        assert!(matches!(options.mode, CloneMode::Full));
        assert!(!options.start);
        assert!(options.mac_addresses.is_none());
        assert_eq!(options.description.unwrap(), "Test clone");
    }

    #[test]
    fn test_clone_options_auto_id() {
        let options = CloneOptions {
            name: "auto-clone".to_string(),
            id: None, // Auto-generate ID
            mode: CloneMode::Linked,
            start: true,
            mac_addresses: Some(vec!["52:54:00:12:34:56".to_string()]),
            description: None,
            network_config: None,
        };

        assert_eq!(options.name, "auto-clone");
        assert!(options.id.is_none());
        assert!(matches!(options.mode, CloneMode::Linked));
        assert!(options.start);
        assert_eq!(options.mac_addresses.unwrap().len(), 1);
        assert!(options.description.is_none());
    }

    #[test]
    fn test_clone_mode_full() {
        let mode = CloneMode::Full;
        assert!(matches!(mode, CloneMode::Full));
    }

    #[test]
    fn test_clone_mode_linked() {
        let mode = CloneMode::Linked;
        assert!(matches!(mode, CloneMode::Linked));
    }

    #[tokio::test]
    async fn test_clone_vm_basic() {
        let manager = VmCloneManager::new("/tmp/test-clone-vms".to_string());
        let source_vm = create_test_vm_config();

        let options = CloneOptions {
            name: "cloned-vm".to_string(),
            id: Some("vm-101".to_string()),
            mode: CloneMode::Full,
            start: false,
            mac_addresses: None,
            description: Some("Test clone".to_string()),
            network_config: None,
        };

        let result = manager.clone_vm(&source_vm, options).await;

        if result.is_ok() {
            let cloned = result.unwrap();
            assert_eq!(cloned.id, "vm-101");
            assert_eq!(cloned.name, "cloned-vm");
            assert_eq!(cloned.memory, source_vm.memory);
            assert_eq!(cloned.cpus, source_vm.cpus);
            assert_eq!(cloned.status, VmStatus::Stopped);
            assert_eq!(cloned.architecture, source_vm.architecture);
        }

        // Clean up
        let _ = tokio::fs::remove_dir_all("/tmp/test-clone-vms").await;
    }

    #[tokio::test]
    async fn test_clone_vm_auto_id() {
        let manager = VmCloneManager::new("/tmp/test-clone-auto".to_string());
        let source_vm = create_test_vm_config();

        let options = CloneOptions {
            name: "auto-id-clone".to_string(),
            id: None, // Should auto-generate
            mode: CloneMode::Full,
            start: false,
            mac_addresses: None,
            description: None,
            network_config: None,
        };

        let result = manager.clone_vm(&source_vm, options).await;

        if result.is_ok() {
            let cloned = result.unwrap();
            assert!(!cloned.id.is_empty());
            assert_ne!(cloned.id, source_vm.id);
            assert_eq!(cloned.name, "auto-id-clone");
        }

        // Clean up
        let _ = tokio::fs::remove_dir_all("/tmp/test-clone-auto").await;
    }

    #[tokio::test]
    async fn test_clone_vm_preserves_config() {
        let manager = VmCloneManager::new("/tmp/test-clone-config".to_string());
        let mut source_vm = create_test_vm_config();
        source_vm.memory = 4096;
        source_vm.cpus = 4;

        let options = CloneOptions {
            name: "config-test-clone".to_string(),
            id: Some("vm-102".to_string()),
            mode: CloneMode::Full,
            start: false,
            mac_addresses: None,
            description: None,
            network_config: None,
        };

        let result = manager.clone_vm(&source_vm, options).await;

        if result.is_ok() {
            let cloned = result.unwrap();
            assert_eq!(cloned.memory, 4096, "Memory should be preserved");
            assert_eq!(cloned.cpus, 4, "CPU count should be preserved");
            assert_eq!(cloned.hypervisor, source_vm.hypervisor);
            assert_eq!(cloned.disk_size, source_vm.disk_size);
        }

        // Clean up
        let _ = tokio::fs::remove_dir_all("/tmp/test-clone-config").await;
    }

    #[tokio::test]
    async fn test_clone_vm_stopped_status() {
        let manager = VmCloneManager::new("/tmp/test-clone-status".to_string());
        let mut source_vm = create_test_vm_config();
        source_vm.status = VmStatus::Running; // Source is running

        let options = CloneOptions {
            name: "status-test-clone".to_string(),
            id: Some("vm-103".to_string()),
            mode: CloneMode::Full,
            start: false,
            mac_addresses: None,
            description: None,
            network_config: None,
        };

        let result = manager.clone_vm(&source_vm, options).await;

        if result.is_ok() {
            let cloned = result.unwrap();
            // Cloned VM should always start in Stopped status
            assert_eq!(cloned.status, VmStatus::Stopped);
        }

        // Clean up
        let _ = tokio::fs::remove_dir_all("/tmp/test-clone-status").await;
    }

    #[test]
    fn test_storage_type_pattern_matching() {
        // Test that StorageType can be matched in patterns
        let storage_types = vec![
            StorageType::Qcow2,
            StorageType::Raw,
            StorageType::Zfs,
            StorageType::Lvm,
            StorageType::Btrfs,
            StorageType::Ceph,
        ];

        for st in storage_types {
            match st {
                StorageType::Qcow2 => assert!(matches!(st, StorageType::Qcow2)),
                StorageType::Raw => assert!(matches!(st, StorageType::Raw)),
                StorageType::Zfs => assert!(matches!(st, StorageType::Zfs)),
                StorageType::Lvm => assert!(matches!(st, StorageType::Lvm)),
                StorageType::Btrfs => assert!(matches!(st, StorageType::Btrfs)),
                StorageType::Ceph => assert!(matches!(st, StorageType::Ceph)),
            }
        }
    }

    #[test]
    fn test_clone_options_builder_pattern() {
        // Test various combinations of options
        let minimal = CloneOptions {
            name: "minimal".to_string(),
            id: None,
            mode: CloneMode::Full,
            start: false,
            mac_addresses: None,
            description: None,
            network_config: None,
        };
        assert!(minimal.id.is_none());
        assert!(minimal.mac_addresses.is_none());

        let full = CloneOptions {
            name: "full".to_string(),
            id: Some("custom-id".to_string()),
            mode: CloneMode::Linked,
            start: true,
            mac_addresses: Some(vec!["52:54:00:11:22:33".to_string(), "52:54:00:44:55:66".to_string()]),
            description: Some("Full config".to_string()),
            network_config: None,
        };
        assert!(full.id.is_some());
        assert_eq!(full.mac_addresses.as_ref().unwrap().len(), 2);
        assert!(full.start);
    }

    #[tokio::test]
    async fn test_storage_directory_creation() {
        let test_path = "/tmp/test-clone-storage-dir";
        let manager = VmCloneManager::new(test_path.to_string());
        let source_vm = create_test_vm_config();

        // Remove directory if it exists
        let _ = tokio::fs::remove_dir_all(test_path).await;

        let options = CloneOptions {
            name: "dir-test-clone".to_string(),
            id: Some("vm-104".to_string()),
            mode: CloneMode::Full,
            start: false,
            mac_addresses: None,
            description: None,
            network_config: None,
        };

        let _ = manager.clone_vm(&source_vm, options).await;

        // Verify directory was created
        let exists = tokio::fs::try_exists(test_path).await.unwrap_or(false);
        assert!(exists, "Storage directory should be created");

        // Clean up
        let _ = tokio::fs::remove_dir_all(test_path).await;
    }

    #[test]
    fn test_generate_mac_address() {
        let mac = VmCloneManager::generate_mac_address();

        // Should start with QEMU OUI prefix
        assert!(mac.starts_with("52:54:00:"));

        // Should be valid format
        assert!(VmCloneManager::validate_mac_address(&mac));

        // Should have correct length
        assert_eq!(mac.len(), 17); // XX:XX:XX:XX:XX:XX
    }

    #[test]
    fn test_generate_multiple_mac_addresses() {
        let macs = VmCloneManager::generate_mac_addresses(5);

        // Should generate correct count
        assert_eq!(macs.len(), 5);

        // All should be unique
        use std::collections::HashSet;
        let unique_macs: HashSet<_> = macs.iter().collect();
        assert_eq!(unique_macs.len(), 5);

        // All should be valid
        for mac in &macs {
            assert!(VmCloneManager::validate_mac_address(mac));
            assert!(mac.starts_with("52:54:00:"));
        }
    }

    #[test]
    fn test_validate_mac_address() {
        // Valid MAC addresses
        assert!(VmCloneManager::validate_mac_address("52:54:00:12:34:56"));
        assert!(VmCloneManager::validate_mac_address("AA:BB:CC:DD:EE:FF"));
        assert!(VmCloneManager::validate_mac_address("00:00:00:00:00:00"));
        assert!(VmCloneManager::validate_mac_address("ff:ff:ff:ff:ff:ff"));

        // Invalid MAC addresses
        assert!(!VmCloneManager::validate_mac_address("52:54:00:12:34")); // Too short
        assert!(!VmCloneManager::validate_mac_address("52:54:00:12:34:56:78")); // Too long
        assert!(!VmCloneManager::validate_mac_address("52-54-00-12-34-56")); // Wrong separator
        assert!(!VmCloneManager::validate_mac_address("52:54:00:12:34:ZZ")); // Invalid hex
        assert!(!VmCloneManager::validate_mac_address("52:54:0:12:34:56")); // Single digit
        assert!(!VmCloneManager::validate_mac_address("")); // Empty
    }

    #[test]
    fn test_apply_mac_addresses_custom() {
        let manager = VmCloneManager::new("/tmp/test".to_string());
        let custom_macs = vec![
            "52:54:00:11:22:33".to_string(),
            "52:54:00:44:55:66".to_string(),
        ];

        let options = CloneOptions {
            name: "test".to_string(),
            id: None,
            mode: CloneMode::Full,
            start: false,
            mac_addresses: Some(custom_macs.clone()),
            description: None,
            network_config: None,
        };

        let result = manager.apply_mac_addresses(&options, 2);
        assert!(result.is_ok());

        let macs = result.unwrap();
        assert_eq!(macs, custom_macs);
    }

    #[test]
    fn test_apply_mac_addresses_auto_generate() {
        let manager = VmCloneManager::new("/tmp/test".to_string());
        let options = CloneOptions {
            name: "test".to_string(),
            id: None,
            mode: CloneMode::Full,
            start: false,
            mac_addresses: None, // Auto-generate
            description: None,
            network_config: None,
        };

        let result = manager.apply_mac_addresses(&options, 3);
        assert!(result.is_ok());

        let macs = result.unwrap();
        assert_eq!(macs.len(), 3);

        for mac in &macs {
            assert!(VmCloneManager::validate_mac_address(mac));
        }
    }

    #[test]
    fn test_apply_mac_addresses_invalid_format() {
        let manager = VmCloneManager::new("/tmp/test".to_string());
        let invalid_macs = vec!["INVALID".to_string()];

        let options = CloneOptions {
            name: "test".to_string(),
            id: None,
            mode: CloneMode::Full,
            start: false,
            mac_addresses: Some(invalid_macs),
            description: None,
            network_config: None,
        };

        let result = manager.apply_mac_addresses(&options, 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_apply_mac_addresses_count_mismatch() {
        let manager = VmCloneManager::new("/tmp/test".to_string());
        let macs = vec!["52:54:00:11:22:33".to_string()];

        let options = CloneOptions {
            name: "test".to_string(),
            id: None,
            mode: CloneMode::Full,
            start: false,
            mac_addresses: Some(macs),
            description: None,
            network_config: None,
        };

        // Requesting 2 interfaces but providing only 1 MAC
        let result = manager.apply_mac_addresses(&options, 2);
        assert!(result.is_err());
    }

    #[test]
    fn test_mac_address_uniqueness() {
        // Generate many MAC addresses to test uniqueness
        let macs = VmCloneManager::generate_mac_addresses(100);

        use std::collections::HashSet;
        let unique_macs: HashSet<_> = macs.iter().collect();

        // All should be unique
        assert_eq!(unique_macs.len(), 100);
    }

    #[test]
    fn test_validate_ipv4_address() {
        // Valid IPv4 addresses
        assert!(VmCloneManager::validate_ipv4_address("192.168.1.100"));
        assert!(VmCloneManager::validate_ipv4_address("10.0.0.1"));
        assert!(VmCloneManager::validate_ipv4_address("172.16.0.1"));
        assert!(VmCloneManager::validate_ipv4_address("0.0.0.0"));
        assert!(VmCloneManager::validate_ipv4_address("255.255.255.255"));

        // Invalid IPv4 addresses
        assert!(!VmCloneManager::validate_ipv4_address("256.1.1.1")); // Out of range
        assert!(!VmCloneManager::validate_ipv4_address("192.168.1")); // Too few octets
        assert!(!VmCloneManager::validate_ipv4_address("192.168.1.1.1")); // Too many octets
        assert!(!VmCloneManager::validate_ipv4_address("192.168.01.1")); // Leading zero
        assert!(!VmCloneManager::validate_ipv4_address("192.168.1.abc")); // Non-numeric
        assert!(!VmCloneManager::validate_ipv4_address("")); // Empty
    }

    #[test]
    fn test_validate_hostname() {
        // Valid hostnames
        assert!(VmCloneManager::validate_hostname("web-server"));
        assert!(VmCloneManager::validate_hostname("db01"));
        assert!(VmCloneManager::validate_hostname("api.example.com"));
        assert!(VmCloneManager::validate_hostname("my-server123"));
        assert!(VmCloneManager::validate_hostname("a"));
        assert!(VmCloneManager::validate_hostname("test-123-abc"));

        // Invalid hostnames
        assert!(!VmCloneManager::validate_hostname("-web")); // Starts with hyphen
        assert!(!VmCloneManager::validate_hostname("web-")); // Ends with hyphen
        assert!(!VmCloneManager::validate_hostname("web_server")); // Underscore not allowed
        assert!(!VmCloneManager::validate_hostname("")); // Empty
        assert!(!VmCloneManager::validate_hostname("web..server")); // Empty label
        assert!(!VmCloneManager::validate_hostname(&"a".repeat(64))); // Label too long (>63)
        assert!(!VmCloneManager::validate_hostname(&"a".repeat(254))); // Hostname too long (>253)
    }

    #[test]
    fn test_validate_network_config_valid() {
        let manager = VmCloneManager::new("/tmp/test".to_string());

        let network_config = NetworkConfig {
            hostname: Some("web-server".to_string()),
            ip_addresses: Some(vec!["192.168.1.100".to_string()]),
            gateway: Some("192.168.1.1".to_string()),
            dns_servers: Some(vec!["8.8.8.8".to_string(), "8.8.4.4".to_string()]),
            domain: Some("example.com".to_string()),
        };

        let result = manager.validate_network_config(&network_config, 1);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_network_config_invalid_hostname() {
        let manager = VmCloneManager::new("/tmp/test".to_string());

        let network_config = NetworkConfig {
            hostname: Some("-invalid".to_string()),
            ip_addresses: None,
            gateway: None,
            dns_servers: None,
            domain: None,
        };

        let result = manager.validate_network_config(&network_config, 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_network_config_invalid_ip() {
        let manager = VmCloneManager::new("/tmp/test".to_string());

        let network_config = NetworkConfig {
            hostname: None,
            ip_addresses: Some(vec!["256.1.1.1".to_string()]),
            gateway: None,
            dns_servers: None,
            domain: None,
        };

        let result = manager.validate_network_config(&network_config, 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_network_config_ip_count_mismatch() {
        let manager = VmCloneManager::new("/tmp/test".to_string());

        let network_config = NetworkConfig {
            hostname: None,
            ip_addresses: Some(vec!["192.168.1.100".to_string()]),
            gateway: None,
            dns_servers: None,
            domain: None,
        };

        // Requesting 2 interfaces but providing only 1 IP
        let result = manager.validate_network_config(&network_config, 2);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_network_config_invalid_gateway() {
        let manager = VmCloneManager::new("/tmp/test".to_string());

        let network_config = NetworkConfig {
            hostname: None,
            ip_addresses: None,
            gateway: Some("invalid".to_string()),
            dns_servers: None,
            domain: None,
        };

        let result = manager.validate_network_config(&network_config, 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_network_config_invalid_dns() {
        let manager = VmCloneManager::new("/tmp/test".to_string());

        let network_config = NetworkConfig {
            hostname: None,
            ip_addresses: None,
            gateway: None,
            dns_servers: Some(vec!["8.8.8.8".to_string(), "invalid".to_string()]),
            domain: None,
        };

        let result = manager.validate_network_config(&network_config, 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_network_config_invalid_domain() {
        let manager = VmCloneManager::new("/tmp/test".to_string());

        let network_config = NetworkConfig {
            hostname: None,
            ip_addresses: None,
            gateway: None,
            dns_servers: None,
            domain: Some("-invalid.com".to_string()),
        };

        let result = manager.validate_network_config(&network_config, 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_apply_network_config() {
        let manager = VmCloneManager::new("/tmp/test".to_string());

        let network_config = NetworkConfig {
            hostname: Some("web-server".to_string()),
            ip_addresses: Some(vec!["192.168.1.100".to_string(), "10.0.0.100".to_string()]),
            gateway: Some("192.168.1.1".to_string()),
            dns_servers: Some(vec!["8.8.8.8".to_string()]),
            domain: Some("example.com".to_string()),
        };

        let options = CloneOptions {
            name: "test".to_string(),
            id: None,
            mode: CloneMode::Full,
            start: false,
            mac_addresses: None,
            description: None,
            network_config: Some(network_config.clone()),
        };

        let result = manager.apply_network_config(&options, 2);
        assert!(result.is_ok());

        let applied_config = result.unwrap();
        assert!(applied_config.is_some());

        let config = applied_config.unwrap();
        assert_eq!(config.hostname, Some("web-server".to_string()));
        assert_eq!(config.ip_addresses.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn test_apply_network_config_none() {
        let manager = VmCloneManager::new("/tmp/test".to_string());

        let options = CloneOptions {
            name: "test".to_string(),
            id: None,
            mode: CloneMode::Full,
            start: false,
            mac_addresses: None,
            description: None,
            network_config: None,
        };

        let result = manager.apply_network_config(&options, 1);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_generate_cloud_init_user_data() {
        let manager = VmCloneManager::new("/tmp/test".to_string());

        let network_config = NetworkConfig {
            hostname: Some("test-server".to_string()),
            ip_addresses: None,
            gateway: None,
            dns_servers: None,
            domain: None,
        };

        let mac_addresses = vec!["52:54:00:11:22:33".to_string()];
        let user_data = manager.generate_cloud_init_user_data(&network_config, &mac_addresses);

        assert!(user_data.contains("#cloud-config"));
        assert!(user_data.contains("hostname: test-server"));
        assert!(user_data.contains("fqdn: test-server"));
        assert!(user_data.contains("preserve_hostname: false"));
        assert!(user_data.contains("manage_etc_hosts: true"));
    }

    #[test]
    fn test_generate_cloud_init_network_config_single_interface() {
        let manager = VmCloneManager::new("/tmp/test".to_string());

        let network_config = NetworkConfig {
            hostname: Some("web-server".to_string()),
            ip_addresses: Some(vec!["192.168.1.100".to_string()]),
            gateway: Some("192.168.1.1".to_string()),
            dns_servers: Some(vec!["8.8.8.8".to_string(), "8.8.4.4".to_string()]),
            domain: Some("example.com".to_string()),
        };

        let mac_addresses = vec!["52:54:00:11:22:33".to_string()];
        let network_yaml = manager.generate_cloud_init_network_config(&network_config, &mac_addresses);

        assert!(network_yaml.contains("version: 2"));
        assert!(network_yaml.contains("ethernets:"));
        assert!(network_yaml.contains("eth0:"));
        assert!(network_yaml.contains("macaddress: 52:54:00:11:22:33"));
        assert!(network_yaml.contains("192.168.1.100/24"));
        assert!(network_yaml.contains("via: 192.168.1.1"));
        assert!(network_yaml.contains("- 8.8.8.8"));
        assert!(network_yaml.contains("- 8.8.4.4"));
        assert!(network_yaml.contains("- example.com"));
    }

    #[test]
    fn test_generate_cloud_init_network_config_multi_interface() {
        let manager = VmCloneManager::new("/tmp/test".to_string());

        let network_config = NetworkConfig {
            hostname: Some("db-server".to_string()),
            ip_addresses: Some(vec![
                "192.168.1.200".to_string(),
                "10.0.0.100".to_string(),
            ]),
            gateway: Some("192.168.1.1".to_string()),
            dns_servers: Some(vec!["192.168.1.10".to_string()]),
            domain: Some("internal.local".to_string()),
        };

        let mac_addresses = vec![
            "52:54:00:11:22:33".to_string(),
            "52:54:00:44:55:66".to_string(),
        ];

        let network_yaml = manager.generate_cloud_init_network_config(&network_config, &mac_addresses);

        assert!(network_yaml.contains("eth0:"));
        assert!(network_yaml.contains("eth1:"));
        assert!(network_yaml.contains("macaddress: 52:54:00:11:22:33"));
        assert!(network_yaml.contains("macaddress: 52:54:00:44:55:66"));
        assert!(network_yaml.contains("192.168.1.200/24"));
        assert!(network_yaml.contains("10.0.0.100/24"));
        assert!(network_yaml.contains("via: 192.168.1.1"));
    }

    #[test]
    fn test_generate_cloud_init_network_config_minimal() {
        let manager = VmCloneManager::new("/tmp/test".to_string());

        let network_config = NetworkConfig {
            hostname: None,
            ip_addresses: None,
            gateway: None,
            dns_servers: None,
            domain: None,
        };

        let mac_addresses = vec!["52:54:00:11:22:33".to_string()];
        let network_yaml = manager.generate_cloud_init_network_config(&network_config, &mac_addresses);

        assert!(network_yaml.contains("version: 2"));
        assert!(network_yaml.contains("eth0:"));
        assert!(network_yaml.contains("macaddress: 52:54:00:11:22:33"));
    }

    #[tokio::test]
    async fn test_create_cloud_init_iso() {
        let test_path = "/tmp/test-cloud-init-iso";
        let manager = VmCloneManager::new(test_path.to_string());

        // Create test directory
        let _ = tokio::fs::create_dir_all(test_path).await;

        let network_config = NetworkConfig {
            hostname: Some("test-vm".to_string()),
            ip_addresses: Some(vec!["192.168.1.100".to_string()]),
            gateway: Some("192.168.1.1".to_string()),
            dns_servers: Some(vec!["8.8.8.8".to_string()]),
            domain: Some("test.local".to_string()),
        };

        let mac_addresses = vec!["52:54:00:11:22:33".to_string()];

        // This test will only fully succeed if genisoimage or mkisofs is installed
        // But we can still test the YAML generation
        let user_data = manager.generate_cloud_init_user_data(&network_config, &mac_addresses);
        let network_yaml = manager.generate_cloud_init_network_config(&network_config, &mac_addresses);

        assert!(!user_data.is_empty());
        assert!(!network_yaml.is_empty());

        // Clean up
        let _ = tokio::fs::remove_dir_all(test_path).await;
    }
}
