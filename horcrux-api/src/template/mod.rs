///! VM template management
///! Provides template creation, cloning (full and linked/COW)

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
    pub created: i64,  // Unix timestamp
    pub disk_path: PathBuf,
    pub storage_type: StorageType,
    pub memory: u64,   // MB
    pub cpus: u32,
    pub os_type: OsType,
    pub cloudinit_template: Option<String>,  // Default cloud-init config
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
    Full,    // Complete copy
    Linked,  // COW/snapshot-based clone
}

/// Clone request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloneRequest {
    pub new_vm_id: String,
    pub new_vm_name: String,
    pub clone_type: CloneType,
    pub storage_pool: Option<String>,  // Target storage pool
    pub cloudinit_config: Option<crate::cloudinit::CloudInitConfig>,
}

/// Template manager
pub struct TemplateManager {
    templates: Arc<RwLock<HashMap<String, Template>>>,
    zfs_backend: Option<Arc<crate::storage::zfs::ZfsManager>>,
    ceph_backend: Option<Arc<crate::storage::ceph::CephManager>>,
    lvm_backend: Option<Arc<crate::storage::lvm::LvmManager>>,
}

impl TemplateManager {
    pub fn new() -> Self {
        Self {
            templates: Arc::new(RwLock::new(HashMap::new())),
            zfs_backend: None,
            ceph_backend: None,
            lvm_backend: None,
        }
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
        }
    }

    /// Convert a VM to a template
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
    pub async fn clone_template(
        &self,
        template_id: &str,
        request: CloneRequest,
    ) -> Result<String> {
        let templates = self.templates.read().await;
        let template = templates
            .get(template_id)
            .ok_or_else(|| horcrux_common::Error::System(format!("Template {} not found", template_id)))?;

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
        templates
            .get(template_id)
            .cloned()
            .ok_or_else(|| horcrux_common::Error::System(format!("Template {} not found", template_id)))
    }

    /// Delete a template
    pub async fn delete_template(&self, template_id: &str) -> Result<()> {
        let mut templates = self.templates.write().await;

        if let Some(template) = templates.remove(template_id) {
            // Delete template disk if it exists
            if template.disk_path.exists() {
                tokio::fs::remove_file(&template.disk_path).await.ok();
            }
            tracing::info!("Template deleted: {}", template_id);
            Ok(())
        } else {
            Err(horcrux_common::Error::System(format!("Template {} not found", template_id)))
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
                    zfs.create_snapshot("tank/vms", &template.source_vm_id, &snapshot_name).await?;
                }
            }
            StorageType::Ceph => {
                if let Some(ceph) = &self.ceph_backend {
                    let snapshot_name = format!("template-{}", template.id);
                    ceph.create_snapshot("rbd/vms", &template.source_vm_id, &snapshot_name).await?;
                }
            }
            StorageType::Lvm => {
                if let Some(lvm) = &self.lvm_backend {
                    let snapshot_name = format!("template-{}", template.id);
                    lvm.create_snapshot("vg0", &template.source_vm_id, &snapshot_name).await?;
                }
            }
            StorageType::Directory => {
                // For directory storage, copy the disk to templates directory
                let template_dir = PathBuf::from("/var/lib/horcrux/templates");
                tokio::fs::create_dir_all(&template_dir).await?;

                let dest = template_dir.join(format!("{}.qcow2", template.id));
                tokio::fs::copy(&template.disk_path, &dest).await?;
            }
        }

        Ok(())
    }

    async fn clone_zfs_linked(&self, template: &Template, request: &CloneRequest) -> Result<()> {
        let zfs = self.zfs_backend.as_ref()
            .ok_or_else(|| horcrux_common::Error::System("ZFS backend not configured".to_string()))?;

        let snapshot_name = format!("template-{}", template.id);
        let clone_name = request.new_vm_id.clone();

        // Clone from snapshot (COW)
        zfs.clone_snapshot("tank/vms", &template.source_vm_id, &snapshot_name, &clone_name).await?;

        Ok(())
    }

    async fn clone_zfs_full(&self, template: &Template, request: &CloneRequest) -> Result<()> {
        let zfs = self.zfs_backend.as_ref()
            .ok_or_else(|| horcrux_common::Error::System("ZFS backend not configured".to_string()))?;

        // Create a new zvol and copy data
        zfs.create_volume("tank/vms", &request.new_vm_id, template.memory / 1024).await?;

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
        let ceph = self.ceph_backend.as_ref()
            .ok_or_else(|| horcrux_common::Error::System("Ceph backend not configured".to_string()))?;

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
        ceph.clone_snapshot("rbd/vms", &template.source_vm_id, &snapshot_name, &request.new_vm_id).await?;

        Ok(())
    }

    async fn clone_ceph_full(&self, template: &Template, request: &CloneRequest) -> Result<()> {
        let ceph = self.ceph_backend.as_ref()
            .ok_or_else(|| horcrux_common::Error::System("Ceph backend not configured".to_string()))?;

        // Create new RBD image
        ceph.create_volume("rbd/vms", &request.new_vm_id, template.memory / 1024).await?;

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
        let lvm = self.lvm_backend.as_ref()
            .ok_or_else(|| horcrux_common::Error::System("LVM backend not configured".to_string()))?;

        let snapshot_name = format!("template-{}", template.id);

        // LVM snapshots are already COW, so we create a snapshot of the snapshot
        lvm.create_snapshot("vg0", &snapshot_name, &request.new_vm_id).await?;

        Ok(())
    }

    async fn clone_lvm_full(&self, template: &Template, request: &CloneRequest) -> Result<()> {
        let lvm = self.lvm_backend.as_ref()
            .ok_or_else(|| horcrux_common::Error::System("LVM backend not configured".to_string()))?;

        // Create new LV
        lvm.create_volume("vg0", &request.new_vm_id, template.memory / 1024).await?;

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
        // For directory storage, use qemu-img to create a backing file (linked) or full copy
        let vm_dir = PathBuf::from("/var/lib/horcrux/vms");
        tokio::fs::create_dir_all(&vm_dir).await?;

        let dest_path = vm_dir.join(format!("{}.qcow2", request.new_vm_id));

        match request.clone_type {
            CloneType::Linked => {
                // Create a qcow2 with backing file (COW)
                let cmd = format!(
                    "qemu-img create -f qcow2 -b {} -F qcow2 {}",
                    template.disk_path.display(),
                    dest_path.display()
                );

                let output = tokio::process::Command::new("sh")
                    .arg("-c")
                    .arg(&cmd)
                    .output()
                    .await?;

                if !output.status.success() {
                    return Err(horcrux_common::Error::System(format!(
                        "Failed to create linked clone: {}",
                        String::from_utf8_lossy(&output.stderr)
                    )));
                }
            }
            CloneType::Full => {
                // Full copy using qemu-img convert
                let cmd = format!(
                    "qemu-img convert -O qcow2 {} {}",
                    template.disk_path.display(),
                    dest_path.display()
                );

                let output = tokio::process::Command::new("sh")
                    .arg("-c")
                    .arg(&cmd)
                    .output()
                    .await?;

                if !output.status.success() {
                    return Err(horcrux_common::Error::System(format!(
                        "Failed to create full clone: {}",
                        String::from_utf8_lossy(&output.stderr)
                    )));
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_template() {
        let manager = TemplateManager::new();

        let template = manager
            .create_template(
                "vm-100",
                "Ubuntu 22.04 Template".to_string(),
                Some("Base Ubuntu server template".to_string()),
                PathBuf::from("/var/lib/horcrux/vms/vm-100.qcow2"),
                StorageType::Directory,
                4096,
                2,
                OsType::Linux,
            )
            .await;

        assert!(template.is_ok());
        let template = template.unwrap();
        assert_eq!(template.name, "Ubuntu 22.04 Template");
        assert_eq!(template.memory, 4096);
        assert_eq!(template.cpus, 2);
    }

    #[tokio::test]
    async fn test_list_templates() {
        let manager = TemplateManager::new();

        manager
            .create_template(
                "vm-100",
                "Template 1".to_string(),
                None,
                PathBuf::from("/tmp/vm-100.qcow2"),
                StorageType::Directory,
                2048,
                1,
                OsType::Linux,
            )
            .await
            .ok();

        let templates = manager.list_templates().await;
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].name, "Template 1");
    }
}
