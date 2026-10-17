//! VM template management
//! Provides template creation, cloning (full and linked/COW)
use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Template metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Template {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub source_vm_id: String,
    pub created: i64, // Unix timestamp
    pub disk_path: PathBuf,
    pub storage_type: StorageType,
    pub memory: u64, // MB
    pub cpus: u32,
    pub os_type: OsType,
    pub cloudinit_template: Option<String>, // Default cloud-init config
}

/// Storage type for templates
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StorageType {
    Zfs,
    Ceph,
    Lvm,
    Directory,
}

/// Operating system type
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OsType {
    Linux,
    Windows,
    Other,
}

/// Clone type
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloneType {
    Full,   // Complete copy
    Linked, // COW/snapshot-based clone
}

/// Clone request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloneRequest {
    pub new_vm_id: String,
    pub new_vm_name: String,
    pub clone_type: CloneType,
    pub storage_pool: Option<String>, // Target storage pool
    pub cloudinit_config: Option<crate::cloudinit::CloudInitConfig>,
}

/// Template manager
pub struct TemplateManager {
    templates: Arc<RwLock<HashMap<String, Template>>>,
    zfs_backend: Option<Arc<crate::storage::zfs::ZfsManager>>,
    ceph_backend: Option<Arc<crate::storage::ceph::CephManager>>,
    lvm_backend: Option<Arc<crate::storage::lvm::LvmManager>>,
    /// Directory-storage VM disks live here; template copies go in
    /// `<vm_storage>/templates`.
    vm_storage: PathBuf,
}

impl Default for TemplateManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TemplateManager {
    pub fn new() -> Self {
        Self {
            templates: Arc::new(RwLock::new(HashMap::new())),
            zfs_backend: None,
            ceph_backend: None,
            lvm_backend: None,
            vm_storage: PathBuf::from("/var/lib/horcrux/vms"),
        }
    }

    /// Template manager whose directory-storage disks live under the
    /// configured VM storage path (HORCRUX_DATA_DIR / paths.vm_storage)
    /// instead of the hardcoded /var/lib/horcrux/vms.
    pub fn with_vm_storage(vm_storage: PathBuf) -> Self {
        Self {
            vm_storage,
            ..Self::new()
        }
    }

    /// Where a cloned VM's disk is written for directory storage.
    pub fn clone_disk_path(&self, new_vm_id: &str) -> PathBuf {
        self.vm_storage.join(format!("{}.qcow2", new_vm_id))
    }

    pub fn with_backends(
        zfs: Option<Arc<crate::storage::zfs::ZfsManager>>,
        ceph: Option<Arc<crate::storage::ceph::CephManager>>,
        lvm: Option<Arc<crate::storage::lvm::LvmManager>>,
    ) -> Self {
        Self {
            templates: Arc::new(RwLock::new(HashMap::new())),
            zfs_backend: zfs,
            ceph_backend: ceph,
            lvm_backend: lvm,
            vm_storage: PathBuf::from("/var/lib/horcrux/vms"),
        }
    }

    /// Convert a VM to a template
    #[allow(clippy::too_many_arguments)]
    pub async fn create_template(
        &self,
        vm_id: &str,
        name: String,
        description: Option<String>,
        disk_path: PathBuf,
        storage_type: StorageType,
        memory: u64,
        cpus: u32,
        os_type: OsType,
    ) -> Result<Template> {
        tracing::info!("Converting VM {} to template {}", vm_id, name);

        // Generate template ID
        let template_id = format!("template-{}", uuid::Uuid::new_v4());
        let created = chrono::Utc::now().timestamp();

        // Directory storage: the template gets its OWN copy of the disk.
        // It used to keep pointing at the source VM's disk, so deleting the
        // template deleted the VM's disk, and deleting the VM broke the
        // template. A missing source disk is an error, not a silent no-op.
        let disk_path = if storage_type == StorageType::Directory {
            if !disk_path.is_file() {
                return Err(horcrux_common::Error::Validation(format!(
                    "Source disk {} does not exist",
                    disk_path.display()
                )));
            }
            let template_dir = self.vm_storage.join("templates");
            tokio::fs::create_dir_all(&template_dir).await?;
            let dest = template_dir.join(format!("{}.qcow2", template_id));
            tokio::fs::copy(&disk_path, &dest).await?;
            dest
        } else {
            disk_path
        };

        // Create template metadata
        let template = Template {
            id: template_id.clone(),
            name: name.clone(),
            description,
            source_vm_id: vm_id.to_string(),
            created,
            disk_path: disk_path.clone(),
            storage_type: storage_type.clone(),
            memory,
            cpus,
            os_type,
            cloudinit_template: None,
        };

        // Mark the VM disk as read-only or move it to templates directory
        self.prepare_template_disk(&template).await?;

        // Store template metadata
        let mut templates = self.templates.write().await;
        templates.insert(template_id.clone(), template.clone());

        tracing::info!("Template created: {} ({})", name, template_id);
        Ok(template)
    }

    /// Clone a template to create a new VM
    pub async fn clone_template(&self, template_id: &str, request: CloneRequest) -> Result<String> {
        let templates = self.templates.read().await;
        let template = templates.get(template_id).ok_or_else(|| {
            horcrux_common::Error::NotFound(format!("Template {} not found", template_id))
        })?;

        tracing::info!(
            "Cloning template {} to VM {} ({:?})",
            template_id,
            request.new_vm_id,
            request.clone_type
        );

        // Clone based on storage type and clone type
        match (&template.storage_type, &request.clone_type) {
            (StorageType::Zfs, CloneType::Linked) => {
                self.clone_zfs_linked(template, &request).await?;
            }
            (StorageType::Zfs, CloneType::Full) => {
                self.clone_zfs_full(template, &request).await?;
            }
            (StorageType::Ceph, CloneType::Linked) => {
                self.clone_ceph_linked(template, &request).await?;
            }
            (StorageType::Ceph, CloneType::Full) => {
                self.clone_ceph_full(template, &request).await?;
            }
            (StorageType::Lvm, CloneType::Linked) => {
                self.clone_lvm_linked(template, &request).await?;
            }
            (StorageType::Lvm, CloneType::Full) => {
                self.clone_lvm_full(template, &request).await?;
            }
            (StorageType::Directory, _) => {
                self.clone_directory(template, &request).await?;
            }
        }

        tracing::info!("Clone created successfully: {}", request.new_vm_id);
        Ok(request.new_vm_id.clone())
    }

    /// List all templates
    pub async fn list_templates(&self) -> Vec<Template> {
        let templates = self.templates.read().await;
        templates.values().cloned().collect()
    }

    /// Get template by ID
    pub async fn get_template(&self, template_id: &str) -> Result<Template> {
        let templates = self.templates.read().await;
        templates.get(template_id).cloned().ok_or_else(|| {
            horcrux_common::Error::NotFound(format!("Template {} not found", template_id))
        })
    }

    /// Delete a template
    pub async fn delete_template(&self, template_id: &str) -> Result<()> {
        let mut templates = self.templates.write().await;

        if let Some(template) = templates.remove(template_id) {
            // Delete the template's own disk copy (directory storage). Never
            // touch a path outside <vm_storage>/templates: for ZFS/Ceph/LVM
            // disk_path names the source VM's volume, which is not ours.
            if template
                .disk_path
                .starts_with(self.vm_storage.join("templates"))
                && template.disk_path.exists()
            {
                tokio::fs::remove_file(&template.disk_path).await.ok();
            }
            tracing::info!("Template deleted: {}", template_id);
            Ok(())
        } else {
            Err(horcrux_common::Error::NotFound(format!(
                "Template {} not found",
                template_id
            )))
        }
    }

    // Private helper methods

    async fn prepare_template_disk(&self, template: &Template) -> Result<()> {
        // For directory-based storage, we might want to move the disk to a templates directory
        // For snapshot-based storage (ZFS/Ceph/LVM), we create a snapshot
        match template.storage_type {
            StorageType::Zfs => {
                if let Some(zfs) = &self.zfs_backend {
                    // Create a template snapshot
                    let snapshot_name = format!("template-{}", template.id);
                    // Extract pool and dataset from path
                    // For simplicity, using a placeholder - real implementation would parse the path
                    zfs.create_snapshot("tank/vms", &template.source_vm_id, &snapshot_name)
                        .await?;
                }
            }
            StorageType::Ceph => {
                if let Some(ceph) = &self.ceph_backend {
                    let snapshot_name = format!("template-{}", template.id);
                    ceph.create_snapshot("rbd/vms", &template.source_vm_id, &snapshot_name)
                        .await?;
                }
            }
            StorageType::Lvm => {
                if let Some(lvm) = &self.lvm_backend {
                    let snapshot_name = format!("template-{}", template.id);
                    lvm.create_snapshot("vg0", &template.source_vm_id, &snapshot_name)
                        .await?;
                }
            }
            StorageType::Directory => {
                // Already copied into <vm_storage>/templates by create_template.
            }
        }

        Ok(())
    }

    async fn clone_zfs_linked(&self, template: &Template, request: &CloneRequest) -> Result<()> {
        let zfs = self.zfs_backend.as_ref().ok_or_else(|| {
            horcrux_common::Error::System("ZFS backend not configured".to_string())
        })?;

        let snapshot_name = format!("template-{}", template.id);
        let clone_name = request.new_vm_id.clone();

        // Clone from snapshot (COW)
        zfs.clone_snapshot(
            "tank/vms",
            &template.source_vm_id,
            &snapshot_name,
            &clone_name,
        )
        .await?;

        Ok(())
    }

    async fn clone_zfs_full(&self, template: &Template, request: &CloneRequest) -> Result<()> {
        let zfs = self.zfs_backend.as_ref().ok_or_else(|| {
            horcrux_common::Error::System("ZFS backend not configured".to_string())
        })?;

        // Create a new zvol and copy data
        zfs.create_volume("tank/vms", &request.new_vm_id, template.memory / 1024)
            .await?;

        // Use zfs send/receive for full copy
        let snapshot_name = format!("template-{}", template.id);
        let send_cmd = format!(
            "zfs send tank/vms/{}@{} | zfs receive tank/vms/{}",
            template.source_vm_id, snapshot_name, request.new_vm_id
        );

        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&send_cmd)
            .output()
            .await?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "Failed to clone ZFS volume: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        Ok(())
    }

    async fn clone_ceph_linked(&self, template: &Template, request: &CloneRequest) -> Result<()> {
        let ceph = self.ceph_backend.as_ref().ok_or_else(|| {
            horcrux_common::Error::System("Ceph backend not configured".to_string())
        })?;

        let snapshot_name = format!("template-{}", template.id);

        // Protect snapshot (required for cloning)
        let protect_cmd = format!(
            "rbd snap protect rbd/vms/{}@{}",
            template.source_vm_id, snapshot_name
        );
        tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&protect_cmd)
            .output()
            .await?;

        // Clone from snapshot (COW)
        ceph.clone_snapshot(
            "rbd/vms",
            &template.source_vm_id,
            &snapshot_name,
            &request.new_vm_id,
        )
        .await?;

        Ok(())
    }

    async fn clone_ceph_full(&self, template: &Template, request: &CloneRequest) -> Result<()> {
        let ceph = self.ceph_backend.as_ref().ok_or_else(|| {
            horcrux_common::Error::System("Ceph backend not configured".to_string())
        })?;

        // Create new RBD image
        ceph.create_volume("rbd/vms", &request.new_vm_id, template.memory / 1024)
            .await?;

        // Copy data using rbd export/import
        let snapshot_name = format!("template-{}", template.id);
        let copy_cmd = format!(
            "rbd export rbd/vms/{}@{} - | rbd import - rbd/vms/{}",
            template.source_vm_id, snapshot_name, request.new_vm_id
        );

        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&copy_cmd)
            .output()
            .await?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "Failed to clone Ceph volume: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        Ok(())
    }

    async fn clone_lvm_linked(&self, template: &Template, request: &CloneRequest) -> Result<()> {
        let lvm = self.lvm_backend.as_ref().ok_or_else(|| {
            horcrux_common::Error::System("LVM backend not configured".to_string())
        })?;

        let snapshot_name = format!("template-{}", template.id);

        // LVM snapshots are already COW, so we create a snapshot of the snapshot
        lvm.create_snapshot("vg0", &snapshot_name, &request.new_vm_id)
            .await?;

        Ok(())
    }

    async fn clone_lvm_full(&self, template: &Template, request: &CloneRequest) -> Result<()> {
        let lvm = self.lvm_backend.as_ref().ok_or_else(|| {
            horcrux_common::Error::System("LVM backend not configured".to_string())
        })?;

        // Create new LV
        lvm.create_volume("vg0", &request.new_vm_id, template.memory / 1024)
            .await?;

        // Copy data using dd
        let snapshot_name = format!("template-{}", template.id);
        let copy_cmd = format!(
            "dd if=/dev/vg0/{} of=/dev/vg0/{} bs=4M",
            snapshot_name, request.new_vm_id
        );

        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&copy_cmd)
            .output()
            .await?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "Failed to clone LVM volume: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        Ok(())
    }

    async fn clone_directory(&self, template: &Template, request: &CloneRequest) -> Result<()> {
        // qemu-img is invoked directly with separate arguments. This used to
        // build a `sh -c "qemu-img ... <paths>"` string from the template's
        // disk path (client-supplied) and the new VM id, so a crafted value
        // could run arbitrary shell commands.
        tokio::fs::create_dir_all(&self.vm_storage).await?;
        let dest_path = self.clone_disk_path(&request.new_vm_id);
        if dest_path.exists() {
            return Err(horcrux_common::Error::Conflict(format!(
                "Disk for VM {} already exists",
                request.new_vm_id
            )));
        }

        let mut cmd = tokio::process::Command::new("qemu-img");
        match request.clone_type {
            CloneType::Linked => {
                cmd.arg("create")
                    .arg("-f")
                    .arg("qcow2")
                    .arg("-b")
                    .arg(&template.disk_path)
                    .arg("-F")
                    .arg("qcow2")
                    .arg(&dest_path);
            }
            CloneType::Full => {
                cmd.arg("convert")
                    .arg("-O")
                    .arg("qcow2")
                    .arg(&template.disk_path)
                    .arg(&dest_path);
            }
        }
        let output = cmd.output().await?;
        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "qemu-img failed to clone template: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hx-tpl-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Real qcow2 source disk (needs qemu-img, as in CI's Test job).
    fn source_disk(dir: &std::path::Path) -> PathBuf {
        let p = dir.join("vm-100.qcow2");
        let ok = std::process::Command::new("qemu-img")
            .args(["create", "-f", "qcow2"])
            .arg(&p)
            .arg("16M")
            .status()
            .expect("qemu-img must be installed")
            .success();
        assert!(ok);
        p
    }

    #[tokio::test]
    async fn directory_template_owns_a_copy_and_clones_from_it() {
        let dir = scratch("own");
        let manager = TemplateManager::with_vm_storage(dir.join("vms"));
        let src = source_disk(&dir);

        let template = manager
            .create_template(
                "vm-100",
                "Ubuntu 22.04 Template".to_string(),
                Some("Base Ubuntu server template".to_string()),
                src.clone(),
                StorageType::Directory,
                4096,
                2,
                OsType::Linux,
            )
            .await
            .unwrap();
        assert_eq!(template.name, "Ubuntu 22.04 Template");
        assert_eq!((template.memory, template.cpus), (4096, 2));
        assert_ne!(
            template.disk_path, src,
            "template must not share the VM's disk"
        );
        assert!(template.disk_path.starts_with(dir.join("vms/templates")));
        assert!(template.disk_path.is_file());

        for (id, kind) in [
            ("clone-full", CloneType::Full),
            ("clone-linked", CloneType::Linked),
        ] {
            let new_id = manager
                .clone_template(
                    &template.id,
                    CloneRequest {
                        new_vm_id: id.to_string(),
                        new_vm_name: id.to_string(),
                        clone_type: kind,
                        storage_pool: None,
                        cloudinit_config: None,
                    },
                )
                .await
                .unwrap();
            assert_eq!(new_id, id);
            assert!(manager.clone_disk_path(id).is_file(), "{id} disk written");
        }

        // Deleting the template removes ITS copy, never the source VM disk.
        manager.delete_template(&template.id).await.unwrap();
        assert!(
            src.is_file(),
            "source VM disk must survive template deletion"
        );
        assert!(!template.disk_path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn missing_source_disk_and_unknown_template_are_errors() {
        let dir = scratch("err");
        let manager = TemplateManager::with_vm_storage(dir.join("vms"));
        let r = manager
            .create_template(
                "vm-x",
                "t".to_string(),
                None,
                dir.join("does-not-exist.qcow2"),
                StorageType::Directory,
                512,
                1,
                OsType::Linux,
            )
            .await;
        assert!(
            matches!(r, Err(horcrux_common::Error::Validation(_))),
            "{r:?}"
        );
        assert!(manager.list_templates().await.is_empty());
        assert!(matches!(
            manager.get_template("nope").await,
            Err(horcrux_common::Error::NotFound(_))
        ));
        // A path with shell metacharacters is just a (missing) file name.
        let r = manager
            .create_template(
                "vm-x",
                "t".to_string(),
                None,
                dir.join("x;touch pwned"),
                StorageType::Directory,
                512,
                1,
                OsType::Linux,
            )
            .await;
        assert!(r.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
