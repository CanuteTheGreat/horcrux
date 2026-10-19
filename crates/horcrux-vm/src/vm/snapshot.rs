//! VM Snapshot Management
//!
//! Provides snapshot functionality for virtual machines:
//! - Create snapshots (memory + disk state)
//! - List snapshots with metadata
//! - Restore to previous snapshot
//! - Delete snapshots
//! - Snapshot trees and rollback
use horcrux_common::{Result, VmConfig};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
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
    /// RAM/device state is part of the snapshot (a live `savevm`; it lives
    /// inside the qcow2 images, so `memory_snapshot` stays `None`).
    #[serde(default)]
    pub memory_included: bool,
    /// Size of that saved VM state in bytes (`qemu-img info` vm-state-size).
    #[serde(default)]
    pub vm_state_bytes: u64,
}

/// VM state at snapshot time
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum VmSnapshotState {
    Running, // Live snapshot with memory
    Stopped, // Disk-only snapshot
    Paused,  // VM was paused during snapshot
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
    /// Whether a QEMU process is running a VM right now (decides offline
    /// `qemu-img` vs. live QMP handling). Tests swap it to exercise the
    /// live paths against a fake QMP socket.
    process_running: fn(&str) -> bool,
}

impl VmSnapshotManager {
    pub fn new(snapshot_dir: String) -> Self {
        Self {
            snapshots: HashMap::new(),
            snapshot_dir,
            process_running: crate::vm::qemu::QemuManager::process_running,
        }
    }

    /// Kept for callers that pass the configured QMP socket pattern: the
    /// QMP socket of a Horcrux VM always sits next to its first disk
    /// (`vm::display::DisplayRuntime::qmp_socket`), so the pattern is unused.
    pub fn with_qmp_socket_pattern(snapshot_dir: String, _qmp_socket_pattern: String) -> Self {
        Self::new(snapshot_dir)
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

        validate_snapshot_name(&snapshot_name)?;

        // Whether a QEMU process is really running this VM right now (the
        // stored status can be stale, e.g. after a QEMU crash). A running
        // QEMU holds its disk images locked, so `qemu-img snapshot` can't
        // touch them: live snapshots go through the VM's QMP socket.
        let live = vm_config.hypervisor == horcrux_common::VmHypervisor::Qemu
            && (self.process_running)(&vm_config.id);

        let (vm_state, disk_snapshots) = if live {
            self.create_live_snapshot(vm_config, &snapshot_name, include_memory)
                .await?
        } else {
            let mut disk_snapshots = Vec::new();
            for (idx, disk) in vm_config.disks.iter().enumerate() {
                let disk_snapshot = self
                    .create_disk_snapshot(&vm_config.id, &disk.path, &snapshot_name, idx)
                    .await?;
                disk_snapshots.push(disk_snapshot);
            }
            (VmSnapshotState::Stopped, disk_snapshots)
        };
        // RAM state of a live snapshot lives inside the qcow2 images
        // (`savevm`), not in a separate file.
        let memory_snapshot: Option<String> = None;
        let memory_included = vm_state == VmSnapshotState::Running;
        let vm_state_bytes: u64 = disk_snapshots.iter().map(|d| d.size_bytes).sum();

        // Track snapshot lineage: the new snapshot's parent is whichever
        // existing snapshot for this VM is currently the "head" of the chain
        // (the most recently created snapshot with no children yet). This
        // keeps `build_snapshot_tree`/`is_current_snapshot` -- which both
        // already rely on `parent_snapshot` being populated -- working
        // correctly instead of silently seeing a flat list of unrelated
        // root snapshots.
        let parent_snapshot = self
            .list_snapshots(&vm_config.id)
            .into_iter()
            .filter(|s| self.is_current_snapshot(&s.id))
            .max_by_key(|s| s.created_at)
            .map(|s| s.id);

        let snapshot = VmSnapshot {
            id: snapshot_id.clone(),
            vm_id: vm_config.id.clone(),
            name: snapshot_name,
            description,
            created_at: timestamp,
            parent_snapshot,
            vm_state,
            disk_snapshots,
            memory_snapshot,
            config_backup: vm_config.clone(),
            memory_included,
            vm_state_bytes,
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

        // qcow2: what this snapshot holds on its own (its saved VM state;
        // its disk clusters are shared with the image). Others: as before.
        let size = match storage_type {
            StorageType::Qcow2 => qcow2_snapshot_state_size(disk_path, &snapshot_id).await,
            _ => self.get_disk_size(disk_path).await.unwrap_or(0),
        };

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
        run_tool(
            "zfs",
            &["snapshot", snapshot_path.as_str()],
            "create ZFS snapshot",
        )
        .await?;
        tracing::info!("Created ZFS snapshot: {}", snapshot_path);
        Ok(())
    }

    /// Create LVM snapshot
    async fn create_lvm_snapshot(&self, lv_path: &str, snapshot_name: &str) -> Result<()> {
        // LVM path format: /dev/vg_name/lv_name. Snapshot size: 10G
        // (should be configurable).
        run_tool(
            "lvcreate",
            &["-s", "-n", snapshot_name, "-L", "10G", lv_path],
            "create LVM snapshot",
        )
        .await?;
        tracing::info!("Created LVM snapshot: {}", snapshot_name);
        Ok(())
    }

    /// Create QCOW2 internal snapshot (VM stopped).
    async fn create_qcow2_snapshot(&self, qcow2_path: &str, snapshot_name: &str) -> Result<()> {
        qemu_img_snapshot("-c", snapshot_name, qcow2_path).await?;
        tracing::info!("Created QCOW2 snapshot: {}", snapshot_name);
        Ok(())
    }

    /// Create Btrfs snapshot
    async fn create_btrfs_snapshot(&self, subvol_path: &str, snapshot_name: &str) -> Result<()> {
        let snapshot_path = format!("{}.snap_{}", subvol_path, snapshot_name);

        // -r: read-only snapshot
        run_tool(
            "btrfs",
            &[
                "subvolume",
                "snapshot",
                "-r",
                subvol_path,
                snapshot_path.as_str(),
            ],
            "create Btrfs snapshot",
        )
        .await?;
        tracing::info!("Created Btrfs snapshot: {}", snapshot_path);
        Ok(())
    }

    /// Create Ceph RBD snapshot
    async fn create_ceph_snapshot(&self, rbd_path: &str, snapshot_name: &str) -> Result<()> {
        // RBD path format: pool/image
        let spec = format!("{}@{}", rbd_path, snapshot_name);
        run_tool(
            "rbd",
            &["snap", "create", spec.as_str()],
            "create Ceph snapshot",
        )
        .await?;
        tracing::info!("Created Ceph RBD snapshot: {}", snapshot_name);
        Ok(())
    }

    /// QMP socket of a VM (next to its first disk).
    fn qmp_socket_of(vm: &VmConfig) -> Result<PathBuf> {
        let disk = vm.disks.first().ok_or_else(|| {
            horcrux_common::Error::InvalidConfig(format!("VM {} has no disk", vm.id))
        })?;
        Ok(crate::vm::display::DisplayRuntime::qmp_socket(
            std::path::Path::new(&disk.path),
        ))
    }

    /// Snapshot a running QEMU VM through its QMP socket. With RAM:
    /// `savevm` (all disks + device/RAM state under one tag, atomic).
    /// Without: pause, internal snapshot of every disk, resume.
    async fn create_live_snapshot(
        &self,
        vm: &VmConfig,
        name: &str,
        include_memory: bool,
    ) -> Result<(VmSnapshotState, Vec<DiskSnapshot>)> {
        for d in &vm.disks {
            if self.detect_storage_type(&d.path)? != StorageType::Qcow2 {
                return Err(horcrux_common::Error::Conflict(format!(
                    "live snapshots need qcow2 disks ({} is not); stop VM {} first",
                    d.path, vm.id
                )));
            }
        }
        let sock = Self::qmp_socket_of(vm)?;
        let mut disks = Vec::new();
        if include_memory {
            hmp(&sock, &format!("savevm {name}")).await?;
            for (idx, d) in vm.disks.iter().enumerate() {
                disks.push(DiskSnapshot {
                    disk_id: format!("disk-{idx}"),
                    storage_type: StorageType::Qcow2,
                    snapshot_name: name.to_string(),
                    snapshot_path: d.path.clone(),
                    size_bytes: qcow2_snapshot_state_size(&d.path, name).await,
                });
            }
            return Ok((VmSnapshotState::Running, disks));
        }
        let disk0 = std::path::PathBuf::from(&vm.disks[0].path);
        crate::vm::hold::set(&disk0, crate::vm::hold::HoldAction::Resume, "live snapshot").await?;
        if let Err(e) = qmp_exec(&sock, "stop", None).await {
            crate::vm::hold::clear(&disk0).await;
            return Err(e);
        }
        let mut result = Ok(());
        for (idx, d) in vm.disks.iter().enumerate() {
            let snap = format!("{name}_{idx}");
            let r = async {
                let device = block_device_for(&sock, &d.path).await?;
                qmp_exec(
                    &sock,
                    "blockdev-snapshot-internal-sync",
                    Some(serde_json::json!({"device": device, "name": snap})),
                )
                .await
            }
            .await;
            if let Err(e) = r {
                result = Err(e);
                break;
            }
            disks.push(DiskSnapshot {
                disk_id: format!("disk-{idx}"),
                storage_type: StorageType::Qcow2,
                size_bytes: qcow2_snapshot_state_size(&d.path, &snap).await,
                snapshot_name: snap,
                snapshot_path: d.path.clone(),
            });
        }
        // Always resume, even if a disk snapshot failed.
        let resumed = qmp_exec(&sock, "cont", None).await;
        if resumed.is_ok() {
            crate::vm::hold::clear(&disk0).await;
        }
        result?;
        resumed?;
        Ok((VmSnapshotState::Paused, disks))
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

        if (self.process_running)(&snapshot.vm_id) {
            // The running QEMU holds the images: delete through QMP.
            let sock = Self::qmp_socket_of(&snapshot.config_backup)?;
            if snapshot.vm_state == VmSnapshotState::Running {
                hmp(&sock, &format!("delvm {}", snapshot.name)).await?;
            } else {
                for disk_snap in &snapshot.disk_snapshots {
                    let device = block_device_for(&sock, &disk_snap.snapshot_path).await?;
                    qmp_exec(
                        &sock,
                        "blockdev-snapshot-delete-internal-sync",
                        Some(
                            serde_json::json!({"device": device, "name": disk_snap.snapshot_name}),
                        ),
                    )
                    .await?;
                }
            }
        } else {
            for disk_snap in &snapshot.disk_snapshots {
                self.delete_disk_snapshot(disk_snap).await?;
            }
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
                qemu_img_snapshot("-d", &disk_snap.snapshot_name, &disk_snap.snapshot_path).await?;
            }
            StorageType::Btrfs => {
                let snapshot_path = format!(
                    "{}.snap_{}",
                    disk_snap.snapshot_path, disk_snap.snapshot_name
                );
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
                    .arg(format!(
                        "{}@{}",
                        disk_snap.snapshot_path, disk_snap.snapshot_name
                    ))
                    .output()
                    .await?;
            }
        }

        Ok(())
    }

    /// Restore VM to a snapshot.
    ///
    /// A running VM can only be rolled back to a snapshot that includes its
    /// RAM (`loadvm` through QMP; the VM keeps running from that point). A
    /// disk-only snapshot needs the VM stopped first, otherwise the guest
    /// would keep running on top of disks rewritten underneath it.
    pub async fn restore_snapshot(&self, snapshot_id: &str, _restore_memory: bool) -> Result<()> {
        let snapshot = self
            .snapshots
            .get(snapshot_id)
            .ok_or_else(|| horcrux_common::Error::NotFound("Snapshot not found".to_string()))?;

        tracing::info!("Restoring snapshot: {}", snapshot_id);
        let live = (self.process_running)(&snapshot.vm_id);
        if live {
            if snapshot.vm_state != VmSnapshotState::Running {
                return Err(horcrux_common::Error::Conflict(format!(
                    "VM {} is running: stop it before rolling back to the disk-only snapshot '{}'",
                    snapshot.vm_id, snapshot.name
                )));
            }
            let sock = Self::qmp_socket_of(&snapshot.config_backup)?;
            hmp(&sock, &format!("loadvm {}", snapshot.name)).await?;
        } else {
            for disk_snap in &snapshot.disk_snapshots {
                self.restore_disk_snapshot(disk_snap).await?;
            }
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
                qemu_img_snapshot("-a", &disk_snap.snapshot_name, &disk_snap.snapshot_path).await?;
            }
            StorageType::Btrfs => {
                // Btrfs requires deleting current and renaming snapshot
                tracing::warn!("Btrfs snapshot restoration not fully implemented");
            }
            StorageType::Ceph => {
                Command::new("rbd")
                    .arg("snap")
                    .arg("rollback")
                    .arg(format!(
                        "{}@{}",
                        disk_snap.snapshot_path, disk_snap.snapshot_name
                    ))
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
        if self.snapshots.contains_key(snapshot_id) {
            // A snapshot is "current" if no other snapshots have it as parent
            !self
                .snapshots
                .values()
                .any(|s| s.parent_snapshot.as_deref() == Some(snapshot_id))
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

    async fn save_snapshot_metadata(&self, snapshot: &VmSnapshot) -> Result<()> {
        fs::create_dir_all(&self.snapshot_dir).await?;

        let metadata_file = format!("{}/{}.json", self.snapshot_dir, snapshot.id);
        let json = serde_json::to_string_pretty(snapshot).map_err(|e| {
            horcrux_common::Error::System(format!("Failed to serialize snapshot: {}", e))
        })?;

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

/// Snapshot names end up on QEMU/HMP command lines and in qcow2 headers:
/// letters, digits, '-', '_' and '.', 1-64 characters, not starting with '-'.
pub fn validate_snapshot_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(horcrux_common::Error::Validation(format!(
            "invalid snapshot name {name:?}: use 1-64 letters, digits, '-', '_' or '.'"
        )))
    }
}

/// Run a storage tool (zfs, lvcreate, btrfs, rbd). A tool that can't be
/// started or exits non-zero is an error naming what failed and why
/// (the tool's stderr), never a silent success.
async fn run_tool(prog: &str, args: &[&str], what: &str) -> Result<()> {
    let output = Command::new(prog)
        .args(args)
        .output()
        .await
        .map_err(|e| horcrux_common::Error::System(format!("Failed to {what}: {prog}: {e}")))?;
    if !output.status.success() {
        return Err(horcrux_common::Error::System(format!(
            "Failed to {what}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

/// `qemu-img snapshot <op> <name> <image>` (op = -c create, -a apply,
/// -d delete); a failure is an error, never silently ignored.
async fn qemu_img_snapshot(op: &str, name: &str, image: &str) -> Result<()> {
    let output = Command::new("qemu-img")
        .args(["snapshot", op, name, image])
        .output()
        .await?;
    if !output.status.success() {
        return Err(horcrux_common::Error::System(format!(
            "qemu-img snapshot {op} {name} {image}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

/// Run one QMP command on a VM's QMP socket and return its `return`
/// value. Asynchronous events QEMU interleaves (STOP, RESUME, ...) are
/// skipped; a QMP error is returned as an error.
pub async fn qmp_exec(
    sock: &std::path::Path,
    command: &str,
    arguments: Option<serde_json::Value>,
) -> Result<serde_json::Value> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let fut = async {
        let stream = tokio::net::UnixStream::connect(sock).await.map_err(|e| {
            horcrux_common::Error::System(format!("QMP connect {}: {e}", sock.display()))
        })?;
        let (rd, mut wr) = stream.into_split();
        let mut rd = BufReader::new(rd);
        let mut line = String::new();
        rd.read_line(&mut line).await?; // greeting
        for (i, cmd) in [
            serde_json::json!({"execute": "qmp_capabilities"}),
            match &arguments {
                Some(a) => serde_json::json!({"execute": command, "arguments": a}),
                None => serde_json::json!({"execute": command}),
            },
        ]
        .into_iter()
        .enumerate()
        {
            wr.write_all(format!("{cmd}\n").as_bytes()).await?;
            loop {
                line.clear();
                if rd.read_line(&mut line).await? == 0 {
                    return Err(horcrux_common::Error::System(
                        "QMP connection closed".to_string(),
                    ));
                }
                let v: serde_json::Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if let Some(err) = v.get("error") {
                    return Err(horcrux_common::Error::System(format!(
                        "QMP {command}: {}",
                        err["desc"].as_str().unwrap_or("error")
                    )));
                }
                if let Some(r) = v.get("return") {
                    if i == 1 {
                        return Ok(r.clone());
                    }
                    break;
                }
            }
        }
        Err(horcrux_common::Error::System(format!(
            "QMP {command}: no reply"
        )))
    };
    tokio::time::timeout(std::time::Duration::from_secs(600), fut)
        .await
        .map_err(|_| horcrux_common::Error::System(format!("QMP {command}: timed out")))?
}

/// What a qcow2 internal snapshot holds on its own as `qemu-img info`
/// reports it: its saved VM state (`vm-state-size`), 0 for a disk-only
/// snapshot or when the image can't be read.
async fn qcow2_snapshot_state_size(path: &str, name: &str) -> u64 {
    let out = tokio::process::Command::new("qemu-img")
        .args(["info", "--output=json", "-U", path])
        .output()
        .await;
    match out {
        Ok(o) if o.status.success() => serde_json::from_slice::<serde_json::Value>(&o.stdout)
            .map(|v| snapshot_state_size(&v, name))
            .unwrap_or(0),
        _ => 0,
    }
}

/// `vm-state-size` of snapshot `name` in a `qemu-img info --output=json`.
fn snapshot_state_size(info: &serde_json::Value, name: &str) -> u64 {
    info["snapshots"]
        .as_array()
        .and_then(|a| a.iter().find(|s| s["name"] == name))
        .and_then(|s| s["vm-state-size"].as_u64())
        .unwrap_or(0)
}

/// Run an HMP command through QMP. HMP reports failures as text in the
/// return value (e.g. "Error: ..."), so any output is treated as an error
/// for the commands used here (savevm/loadvm/delvm print nothing on success).
async fn hmp(sock: &std::path::Path, command_line: &str) -> Result<()> {
    let out = qmp_exec(
        sock,
        "human-monitor-command",
        Some(serde_json::json!({"command-line": command_line})),
    )
    .await?;
    let text = out.as_str().unwrap_or_default().trim();
    if !text.is_empty() {
        return Err(horcrux_common::Error::System(format!(
            "{command_line}: {text}"
        )));
    }
    Ok(())
}

/// The QEMU block device (drive id) backed by image `path`.
pub async fn block_device_for(sock: &std::path::Path, path: &str) -> Result<String> {
    let blocks = qmp_exec(sock, "query-block", None).await?;
    blocks
        .as_array()
        .into_iter()
        .flatten()
        .find(|b| b["inserted"]["file"].as_str() == Some(path))
        .and_then(|b| b["device"].as_str().map(str::to_string))
        .ok_or_else(|| {
            horcrux_common::Error::System(format!("no QEMU block device backed by {path}"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use horcrux_common::VmArchitecture;
    use horcrux_common::VmStatus;

    #[test]
    fn a_snapshot_reports_its_own_vm_state_size_not_the_image_size() {
        let info = serde_json::json!({
            "virtual-size": 10737418240u64, "actual-size": 718274560u64,
            "snapshots": [
                {"id": "1", "name": "disk-only", "vm-state-size": 0},
                {"id": "2", "name": "qa-snap1", "vm-state-size": 141557760u64}
            ]
        });
        assert_eq!(snapshot_state_size(&info, "qa-snap1"), 141557760);
        assert_eq!(snapshot_state_size(&info, "disk-only"), 0);
        assert_eq!(snapshot_state_size(&info, "missing"), 0);
        assert_eq!(snapshot_state_size(&serde_json::json!({}), "x"), 0);
    }

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
            display: Default::default(),
            hardware: Default::default(),
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
            manager
                .detect_storage_type("/dev/zvol/tank/vm-100-disk-0")
                .unwrap(),
            StorageType::Zfs
        );

        assert_eq!(
            manager.detect_storage_type("/dev/vg0/lv-vm-100").unwrap(),
            StorageType::Lvm
        );

        assert_eq!(
            manager
                .detect_storage_type("/var/lib/vz/images/100/vm-100-disk-0.qcow2")
                .unwrap(),
            StorageType::Qcow2
        );

        assert_eq!(
            manager
                .detect_storage_type("/mnt/btrfs/vm-100-disk-0")
                .unwrap(),
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

        let result = manager
            .create_snapshot(
                &vm_config,
                "test-snapshot".to_string(),
                Some("Test description".to_string()),
                false,
            )
            .await;

        assert!(result.is_ok());
        let snapshot = result.unwrap();
        assert_eq!(snapshot.vm_id, "test-vm-100");
        assert_eq!(snapshot.name, "test-snapshot");
        assert_eq!(snapshot.description, Some("Test description".to_string()));
        assert_eq!(snapshot.vm_state, VmSnapshotState::Stopped);
        assert!(snapshot.memory_snapshot.is_none());
    }

    #[tokio::test]
    async fn status_running_without_a_qemu_process_is_snapshotted_offline() {
        // The stored status said running, but no QEMU process exists for
        // the VM (e.g. it crashed): the snapshot is taken offline instead
        // of trying to pause a VM that isn't there.
        let mut manager = VmSnapshotManager::new("/tmp/test-snapshots".to_string());
        let vm_config = create_test_vm_config();

        let snapshot = manager
            .create_snapshot(&vm_config, "live-snapshot".to_string(), None, false)
            .await
            .unwrap();
        assert_eq!(snapshot.vm_state, VmSnapshotState::Stopped);
        assert!(snapshot.memory_snapshot.is_none());
    }

    #[test]
    fn snapshot_names_are_validated() {
        for ok in ["s1", "before-upgrade", "daily_2026.10.17", &"a".repeat(64)] {
            assert!(validate_snapshot_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-x", "a b", "x;quit", "a/b", &"a".repeat(65)] {
            assert!(validate_snapshot_name(bad).is_err(), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn invalid_snapshot_name_is_refused_before_touching_disks() {
        let mut manager = VmSnapshotManager::new("/tmp/test-snapshots".to_string());
        let vm_config = create_test_vm_config();
        let err = manager
            .create_snapshot(&vm_config, "x; quit".to_string(), None, false)
            .await
            .unwrap_err();
        assert!(
            matches!(err, horcrux_common::Error::Validation(_)),
            "{err:?}"
        );
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
        let _ = manager
            .create_snapshot(&vm_config1, "snap1".to_string(), None, false)
            .await;
        let _ = manager
            .create_snapshot(&vm_config1, "snap2".to_string(), None, false)
            .await;
        let _ = manager
            .create_snapshot(&vm_config2, "snap3".to_string(), None, false)
            .await;

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

        let snapshot = manager
            .create_snapshot(&vm_config, "delete-test".to_string(), None, false)
            .await
            .unwrap();

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
            memory_included: false,
            vm_state_bytes: 0,
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
        let snapshot = manager
            .create_snapshot(
                &vm_config,
                "persist-test".to_string(),
                Some("Testing persistence".to_string()),
                false,
            )
            .await
            .unwrap();

        let snapshot_id = snapshot.id.clone();

        // Verify metadata file exists
        let metadata_file = format!("{}/{}.json", test_dir, snapshot_id);
        assert!(fs::try_exists(&metadata_file).await.unwrap_or(false));

        // Clean up
        let _ = fs::remove_dir_all(test_dir).await;
    }
    fn sh(prog: &str, args: &[&str]) -> String {
        let out = std::process::Command::new(prog)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{prog} {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    /// The byte pattern at offset 0 of a qcow2 image (via qemu-io).
    fn pattern_at_0(img: &str, byte: u8) -> bool {
        std::process::Command::new("qemu-io")
            .args(["-f", "qcow2", "-c", &format!("read -P {byte} 0 64k"), img])
            .output()
            .map(|o| {
                o.status.success()
                    && !String::from_utf8_lossy(&o.stdout).contains("Pattern verification failed")
            })
            .unwrap_or(false)
    }

    /// Offline (stopped VM) qcow2 snapshots for real: internal snapshots in
    /// the image, rollback restores the data, delete removes it from the
    /// image, the parent chain / current marker, and metadata reload.
    #[tokio::test]
    async fn qcow2_snapshot_rollback_delete_and_reload_for_real() {
        let dir = std::env::temp_dir().join(format!("hx-snap-real-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("real-vm.qcow2");
        let img_s = img.to_string_lossy().to_string();
        sh("qemu-img", &["create", "-q", "-f", "qcow2", &img_s, "16M"]);
        sh(
            "qemu-io",
            &["-f", "qcow2", "-c", "write -P 0x11 0 64k", &img_s],
        );
        assert!(pattern_at_0(&img_s, 0x11));

        let meta = dir.join("meta").to_string_lossy().to_string();
        let mut m = VmSnapshotManager::new(meta.clone());
        let mut vm = create_test_vm_config();
        vm.id = format!("snap-real-{}", std::process::id());
        vm.status = VmStatus::Stopped;
        vm.disks = vec![horcrux_common::VmDisk {
            path: img_s.clone(),
            size_gb: 1,
            disk_type: "virtio".into(),
            cache: "none".into(),
            ..Default::default()
        }];

        let s1 = m
            .create_snapshot(&vm, "s1".into(), None, false)
            .await
            .unwrap();
        assert_eq!(s1.vm_state, VmSnapshotState::Stopped);
        assert_eq!(s1.parent_snapshot, None);
        assert_eq!(s1.disk_snapshots.len(), 1);
        let ds = &s1.disk_snapshots[0];
        assert_eq!(ds.storage_type, StorageType::Qcow2);
        // a disk-only snapshot holds no VM state of its own (not the
        // whole image file size)
        assert_eq!(ds.size_bytes, 0);
        assert!(!s1.memory_included);
        assert_eq!(s1.vm_state_bytes, 0);
        let list = sh("qemu-img", &["snapshot", "-l", &img_s]);
        assert!(list.contains(&ds.snapshot_name), "{list}");

        // Change the disk, take s2 (child of s1), then roll back to s1.
        sh(
            "qemu-io",
            &["-f", "qcow2", "-c", "write -P 0x22 0 64k", &img_s],
        );
        let s2 = m
            .create_snapshot(&vm, "s2".into(), None, false)
            .await
            .unwrap();
        assert_eq!(s2.parent_snapshot.as_deref(), Some(s1.id.as_str()));
        assert!(!m.is_current_snapshot(&s1.id), "s1 has a child");
        assert!(m.is_current_snapshot(&s2.id), "s2 is the head");
        assert!(!m.is_current_snapshot("no-such-snapshot"));
        let tree = m.build_snapshot_tree(&vm.id);
        assert_eq!(tree.len(), 1, "one root");
        assert_eq!(tree[0].snapshot.id, s1.id);
        assert!(!tree[0].is_current);
        assert_eq!(tree[0].children.len(), 1);
        assert_eq!(tree[0].children[0].snapshot.id, s2.id);
        assert!(tree[0].children[0].is_current);
        assert!(m.build_snapshot_tree("other-vm").is_empty());

        m.restore_snapshot(&s1.id, false).await.unwrap();
        assert!(pattern_at_0(&img_s, 0x11), "rolled back to s1's data");
        m.restore_snapshot(&s2.id, false).await.unwrap();
        assert!(pattern_at_0(&img_s, 0x22), "forward to s2's data");
        assert!(m.restore_snapshot("no-such-snapshot", false).await.is_err());

        // Metadata reloads into a fresh manager (API restart).
        let mut again = VmSnapshotManager::new(meta.clone());
        again.load_snapshots().await.unwrap();
        assert_eq!(again.list_snapshots(&vm.id).len(), 2);
        assert_eq!(again.get_snapshot(&s2.id).unwrap().name, "s2");

        // Delete s2: gone from the image and from the manager.
        let s2_disk = s2.disk_snapshots[0].snapshot_name.clone();
        m.delete_snapshot(&s2.id).await.unwrap();
        let list = sh("qemu-img", &["snapshot", "-l", &img_s]);
        assert!(!list.contains(&s2_disk), "{list}");
        assert!(list.contains(&ds.snapshot_name), "s1 kept: {list}");
        assert!(m.get_snapshot(&s2.id).is_none());
        assert!(m.is_current_snapshot(&s1.id), "s1 is the head again");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- storage tools: a failure is an error, never a silent success ----

    #[tokio::test]
    async fn run_tool_reports_what_failed_and_why() {
        run_tool("true", &[], "do nothing").await.unwrap();

        let err = run_tool(
            "sh",
            &["-c", "echo pool is busy >&2; exit 3"],
            "create ZFS snapshot",
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("Failed to create ZFS snapshot: pool is busy"),
            "{err}"
        );

        let err = run_tool("hx-no-such-storage-tool", &[], "create LVM snapshot")
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Failed to create LVM snapshot: hx-no-such-storage-tool"),
            "{err}"
        );
    }

    /// Each backend's snapshot of a disk that doesn't exist fails (the tool
    /// is missing on the runner or refuses the bogus target), and the
    /// failure reaches the caller.
    #[tokio::test]
    async fn backend_snapshot_failures_reach_the_caller() {
        let m = VmSnapshotManager::new("/tmp/test-snapshots".to_string());
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("no-such-subvolume");

        let err = m
            .create_zfs_snapshot("/var/lib/not-a-zvol", "s")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Invalid ZFS path"), "{err}");
        assert!(m
            .create_zfs_snapshot("/dev/zvol/hx-no-such-pool/disk", "s")
            .await
            .is_err());
        assert!(m
            .create_lvm_snapshot("/dev/hx-no-such-vg/disk", "s")
            .await
            .is_err());
        assert!(m
            .create_btrfs_snapshot(&missing.to_string_lossy(), "s")
            .await
            .is_err());
        assert!(m
            .create_ceph_snapshot("hx-no-such-pool/disk", "s")
            .await
            .is_err());
    }

    // ---- QMP: a fake QEMU monitor socket ----

    type Seen = std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>;

    /// A fake QEMU QMP endpoint on `sock`. Per connection: greeting,
    /// `qmp_capabilities` -> `{}`, then one command, answered with
    /// `answer(command)` after an async event and a non-JSON line (which
    /// clients must skip). Every command after the handshake is recorded.
    fn fake_qmp(
        sock: &std::path::Path,
        answer: impl Fn(&serde_json::Value) -> serde_json::Value + Send + 'static,
    ) -> Seen {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let _ = std::fs::remove_file(sock);
        let listener = tokio::net::UnixListener::bind(sock).unwrap();
        let seen: Seen = Default::default();
        let log = seen.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (rd, mut wr) = stream.into_split();
                let mut rd = BufReader::new(rd);
                let mut line = String::new();
                let _ = wr
                    .write_all(b"{\"QMP\": {\"version\": {}, \"capabilities\": []}}\n")
                    .await;
                if rd.read_line(&mut line).await.unwrap_or(0) == 0 {
                    continue;
                }
                let _ = wr.write_all(b"{\"return\": {}}\n").await;
                line.clear();
                if rd.read_line(&mut line).await.unwrap_or(0) == 0 {
                    continue;
                }
                let cmd: serde_json::Value = serde_json::from_str(&line).unwrap();
                log.lock().unwrap().push(cmd.clone());
                let reply = format!(
                    "{{\"event\": \"STOP\", \"timestamp\": {{}}}}\nnot json\n{}\n",
                    answer(&cmd)
                );
                let _ = wr.write_all(reply.as_bytes()).await;
            }
        });
        seen
    }

    fn commands(seen: &Seen) -> Vec<serde_json::Value> {
        seen.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn qmp_exec_returns_the_commands_own_reply() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("vm.qmp.sock");
        let seen = fake_qmp(
            &sock,
            |cmd| serde_json::json!({"return": {"ran": cmd["execute"], "args": cmd["arguments"]}}),
        );

        let r = qmp_exec(&sock, "query-status", Some(serde_json::json!({"x": 1})))
            .await
            .unwrap();
        assert_eq!(
            r,
            serde_json::json!({"ran": "query-status", "args": {"x": 1}})
        );
        let r = qmp_exec(&sock, "stop", None).await.unwrap();
        assert_eq!(r, serde_json::json!({"ran": "stop", "args": null}));

        let sent = commands(&seen);
        assert_eq!(sent.len(), 2);
        assert_eq!(
            sent[0],
            serde_json::json!({"execute": "query-status", "arguments": {"x": 1}})
        );
        assert_eq!(sent[1], serde_json::json!({"execute": "stop"}));
    }

    #[tokio::test]
    async fn qmp_errors_and_a_missing_socket_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("vm.qmp.sock");
        fake_qmp(
            &sock,
            |_| serde_json::json!({"error": {"class": "GenericError", "desc": "device is locked"}}),
        );
        let err = qmp_exec(&sock, "cont", None).await.unwrap_err().to_string();
        assert!(err.contains("QMP cont: device is locked"), "{err}");

        let err = qmp_exec(&dir.path().join("absent.sock"), "cont", None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("QMP connect"), "{err}");
    }

    #[tokio::test]
    async fn hmp_output_is_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("vm.qmp.sock");
        let seen = fake_qmp(&sock, |cmd| {
            if cmd["arguments"]["command-line"] == "savevm good" {
                serde_json::json!({"return": ""})
            } else {
                serde_json::json!({"return": "Error: No block device can accept snapshots\r\n"})
            }
        });
        hmp(&sock, "savevm good").await.unwrap();
        let err = hmp(&sock, "savevm bad").await.unwrap_err().to_string();
        assert!(
            err.contains("savevm bad: Error: No block device can accept snapshots"),
            "{err}"
        );
        assert_eq!(
            commands(&seen)[0],
            serde_json::json!({"execute": "human-monitor-command",
                               "arguments": {"command-line": "savevm good"}})
        );
    }

    fn query_block(disk: &str) -> serde_json::Value {
        serde_json::json!({"return": [
            {"device": "ide1-cd0", "inserted": {"file": "/isos/install.iso"}},
            {"device": "floppy0"},
            {"device": "virtio0", "inserted": {"file": disk}},
        ]})
    }

    #[tokio::test]
    async fn block_device_for_finds_the_drive_backed_by_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("vm.qmp.sock");
        fake_qmp(&sock, |_| query_block("/vm/a.qcow2"));
        assert_eq!(
            block_device_for(&sock, "/vm/a.qcow2").await.unwrap(),
            "virtio0"
        );
        let err = block_device_for(&sock, "/vm/b.qcow2")
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("no QEMU block device backed by /vm/b.qcow2"),
            "{err}"
        );
    }

    /// A manager that sees every VM as running, with one snapshot of a VM
    /// whose disk (and so QMP socket) lives in `dir`.
    fn live_manager(dir: &std::path::Path, state: VmSnapshotState) -> (VmSnapshotManager, String) {
        let mut m = VmSnapshotManager::new(dir.join("meta").to_string_lossy().to_string());
        m.process_running = |_| true;
        let mut vm = create_test_vm_config();
        vm.disks = vec![horcrux_common::VmDisk {
            path: dir.join("vm.qcow2").to_string_lossy().to_string(),
            size_gb: 1,
            disk_type: "virtio".into(),
            cache: "none".into(),
            ..Default::default()
        }];
        let disk_snapshots = vec![DiskSnapshot {
            disk_id: "disk-0".into(),
            storage_type: StorageType::Qcow2,
            snapshot_name: "snap_0".into(),
            snapshot_path: vm.disks[0].path.clone(),
            size_bytes: 0,
        }];
        m.snapshots.insert(
            "snap-id".into(),
            VmSnapshot {
                id: "snap-id".into(),
                vm_id: vm.id.clone(),
                name: "snap".into(),
                description: None,
                created_at: 0,
                parent_snapshot: None,
                vm_state: state,
                disk_snapshots,
                memory_snapshot: None,
                config_backup: vm.clone(),
                memory_included: false,
                vm_state_bytes: 0,
            },
        );
        (m, vm.disks[0].path.clone())
    }

    fn fake_vm_qmp(dir: &std::path::Path, disk: String) -> Seen {
        fake_qmp(&dir.join("vm.qmp.sock"), move |cmd| {
            match cmd["execute"].as_str().unwrap_or_default() {
                "query-block" => query_block(&disk),
                _ => serde_json::json!({"return": ""}),
            }
        })
    }

    /// Running VM, snapshot with RAM: rollback and delete go through
    /// `loadvm`/`delvm` on the VM's QMP socket.
    #[tokio::test]
    async fn live_ram_snapshot_rolls_back_and_deletes_through_qmp() {
        let dir = tempfile::tempdir().unwrap();
        let (mut m, disk) = live_manager(dir.path(), VmSnapshotState::Running);
        let seen = fake_vm_qmp(dir.path(), disk);

        m.restore_snapshot("snap-id", true).await.unwrap();
        m.delete_snapshot("snap-id").await.unwrap();
        assert!(m.get_snapshot("snap-id").is_none());

        let lines: Vec<_> = commands(&seen)
            .iter()
            .map(|c| c["arguments"]["command-line"].clone())
            .collect();
        assert_eq!(lines, ["loadvm snap", "delvm snap"]);
    }

    /// Running VM, disk-only snapshot: rollback is refused (the guest would
    /// run on disks rewritten under it) without touching QEMU; delete
    /// removes the internal snapshot from the drive backed by the disk.
    #[tokio::test]
    async fn live_disk_only_snapshot_refuses_rollback_and_deletes_per_drive() {
        let dir = tempfile::tempdir().unwrap();
        let (mut m, disk) = live_manager(dir.path(), VmSnapshotState::Paused);
        let seen = fake_vm_qmp(dir.path(), disk);

        let err = m.restore_snapshot("snap-id", false).await.unwrap_err();
        assert!(matches!(err, horcrux_common::Error::Conflict(_)), "{err:?}");
        assert!(commands(&seen).is_empty(), "rollback refused before QMP");

        m.delete_snapshot("snap-id").await.unwrap();
        assert!(m.get_snapshot("snap-id").is_none());
        assert_eq!(
            commands(&seen),
            [
                serde_json::json!({"execute": "query-block"}),
                serde_json::json!({"execute": "blockdev-snapshot-delete-internal-sync",
                                   "arguments": {"device": "virtio0", "name": "snap_0"}}),
            ]
        );
    }

    /// Only a QEMU VM with a live QEMU process is snapshotted live: an
    /// LXD/Incus VM goes the offline way even when "running".
    #[tokio::test]
    async fn only_running_qemu_vms_are_snapshotted_live() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = VmSnapshotManager::new(dir.path().to_string_lossy().to_string());
        m.process_running = |_| true;
        let mut vm = create_test_vm_config();
        vm.hypervisor = horcrux_common::VmHypervisor::Lxd;
        let snap = m
            .create_snapshot(&vm, "offline".into(), None, false)
            .await
            .unwrap();
        assert_eq!(snap.vm_state, VmSnapshotState::Stopped);

        // The same VM on QEMU takes the live path (which needs a disk for
        // its QMP socket, so this disk-less VM is refused).
        vm.hypervisor = horcrux_common::VmHypervisor::Qemu;
        let err = m
            .create_snapshot(&vm, "live".into(), None, false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("has no disk"), "{err}");
    }
}
