pub mod clone;
pub mod clone_progress;
pub mod cross_node_clone;
pub mod display;
pub mod incus;
pub mod lxd;
/// Virtual machine management module
/// Handles QEMU/KVM, LXD, and Incus virtual machine lifecycle
pub mod qemu;
pub mod replication;
pub mod snapshot;
pub mod snapshot_quota;
pub mod snapshot_scheduler;
pub mod vgpu;

pub use qemu::{QemuManager, QemuVm};

use crate::db::Database;
use horcrux_common::{Result, VmConfig, VmHypervisor, VmStatus};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Virtual machine manager
pub struct VmManager {
    vms: Arc<RwLock<HashMap<String, QemuVm>>>,
    qemu: QemuManager,
    lxd: lxd::LxdManager,
    incus: incus::IncusManager,
    db: Option<Arc<Database>>,
}

impl Default for VmManager {
    fn default() -> Self {
        Self::new()
    }
}

impl VmManager {
    pub fn new() -> Self {
        Self {
            vms: Arc::new(RwLock::new(HashMap::new())),
            qemu: QemuManager::new(),
            lxd: lxd::LxdManager::new(),
            incus: incus::IncusManager::new(),
            db: None,
        }
    }

    /// Create VmManager with database support
    pub fn with_database(db: Arc<Database>) -> Self {
        Self {
            vms: Arc::new(RwLock::new(HashMap::new())),
            qemu: QemuManager::new(),
            lxd: lxd::LxdManager::new(),
            incus: incus::IncusManager::new(),
            db: Some(db),
        }
    }

    /// Create VmManager with database support and an explicit VM storage
    /// path (honors `HORCRUX_DATA_DIR`/`HORCRUX_VM_STORAGE` instead of the
    /// hardcoded `/var/lib/horcrux/vms` default, which doesn't exist and
    /// isn't writable in unprivileged environments like CI runners).
    pub fn with_database_and_storage_path(db: Arc<Database>, vm_storage_path: PathBuf) -> Self {
        Self {
            vms: Arc::new(RwLock::new(HashMap::new())),
            qemu: QemuManager::with_storage_path(vm_storage_path),
            lxd: lxd::LxdManager::new(),
            incus: incus::IncusManager::new(),
            db: Some(db),
        }
    }

    /// List all virtual machines
    pub async fn list_vms(&self) -> Vec<VmConfig> {
        // Try database first if available
        if let Some(db) = &self.db {
            if let Ok(vms) = db.list_vms().await {
                return vms;
            }
        }

        // Fallback to in-memory
        let vms = self.vms.read().await;
        vms.values().map(|vm| vm.to_config()).collect()
    }

    /// Get a specific VM by ID
    pub async fn get_vm(&self, id: &str) -> Result<VmConfig> {
        // Try database first if available
        if let Some(db) = &self.db {
            if let Ok(vm) = db.get_vm(id).await {
                return Ok(vm);
            }
        }

        // Fallback to in-memory
        let vms = self.vms.read().await;
        vms.get(id)
            .map(|vm| vm.to_config())
            .ok_or_else(|| horcrux_common::Error::VmNotFound(id.to_string()))
    }

    /// Create a new virtual machine
    pub async fn create_vm(&self, config: VmConfig) -> Result<VmConfig> {
        let mut vms = self.vms.write().await;

        // Check if VM with this ID already exists
        if vms.contains_key(&config.id) {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "VM with ID {} already exists",
                config.id
            )));
        }

        config
            .display
            .validate(&config.architecture)
            .map_err(horcrux_common::Error::Validation)?;

        // Create the VM
        // Dispatch on the requested hypervisor. The LXD/Incus VM backends
        // existed but were never called: an "lxd" or "incus" VM silently
        // became a QEMU VM labelled "qemu".
        let vm = match config.hypervisor {
            VmHypervisor::Qemu => self.qemu.create_vm(&config).await?,
            VmHypervisor::Lxd => {
                if !lxd::LxdManager::check_lxd_available() {
                    return Err(horcrux_common::Error::Validation(
                        "LXD is not installed on this host (USE=lxd needs app-containers/lxd)"
                            .to_string(),
                    ));
                }
                self.lxd.create_vm(&config).await?
            }
            VmHypervisor::Incus => {
                if !incus::IncusManager::check_incus_available() {
                    return Err(horcrux_common::Error::Validation(
                        "Incus is not installed on this host (USE=incus needs app-containers/incus)"
                            .to_string(),
                    ));
                }
                self.incus.create_vm(&config).await?
            }
        };
        let vm_config = vm.to_config();

        // Save to database if available
        if let Some(db) = &self.db {
            db.create_vm(&vm_config).await?;
        }

        vms.insert(config.id.clone(), vm);
        Ok(vm_config)
    }

    /// Change a stopped VM's display protocol / video card / sound card.
    /// Takes effect at the next start.
    pub async fn set_display(
        &self,
        id: &str,
        display: horcrux_common::VmDisplay,
    ) -> Result<VmConfig> {
        let mut vms = self.vms.write().await;
        self.hydrate(&mut vms, id).await?;
        let vm = vms
            .get_mut(id)
            .ok_or_else(|| horcrux_common::Error::VmNotFound(id.to_string()))?;
        if vm.hypervisor != VmHypervisor::Qemu {
            return Err(horcrux_common::Error::Validation(
                "Display settings apply to QEMU VMs; LXD/Incus manage their own console".into(),
            ));
        }
        if vm.status == VmStatus::Running {
            return Err(horcrux_common::Error::Conflict(
                "Stop the VM before changing its display settings".into(),
            ));
        }
        display
            .validate(&vm.architecture)
            .map_err(horcrux_common::Error::Validation)?;
        vm.display = display;
        let config = vm.to_config();
        if let Some(db) = &self.db {
            db.update_vm(&config).await?;
        }
        Ok(config)
    }

    /// Register a VM whose disk already exists (e.g. cloned from a
    /// template) without creating a new disk.
    pub async fn import_vm(&self, config: VmConfig) -> Result<VmConfig> {
        let mut vms = self.vms.write().await;
        let known_in_db = match &self.db {
            Some(db) => db.get_vm(&config.id).await.is_ok(),
            None => false,
        };
        if vms.contains_key(&config.id) || known_in_db {
            return Err(horcrux_common::Error::Conflict(format!(
                "VM with ID {} already exists",
                config.id
            )));
        }
        let vm = self.qemu.vm_from_config(&config);
        let vm_config = vm.to_config();
        if let Some(db) = &self.db {
            db.create_vm(&vm_config).await?;
        }
        vms.insert(config.id.clone(), vm);
        Ok(vm_config)
    }

    /// Make sure `id` is in the in-memory map, loading it from the database
    /// if needed. VMs are persisted, but start/stop used to look only at
    /// the in-memory map, so after an API restart every listed VM failed
    /// with "VM not found" when started or stopped.
    async fn hydrate(&self, vms: &mut HashMap<String, qemu::QemuVm>, id: &str) -> Result<()> {
        if vms.contains_key(id) {
            return Ok(());
        }
        if let Some(db) = &self.db {
            if let Ok(config) = db.get_vm(id).await {
                vms.insert(id.to_string(), self.qemu.vm_from_config(&config));
                return Ok(());
            }
        }
        Err(horcrux_common::Error::VmNotFound(id.to_string()))
    }

    /// Start a virtual machine
    pub async fn start_vm(&self, id: &str) -> Result<VmConfig> {
        let mut vms = self.vms.write().await;
        self.hydrate(&mut vms, id).await?;
        let vm = vms
            .get_mut(id)
            .ok_or_else(|| horcrux_common::Error::VmNotFound(id.to_string()))?;

        match vm.hypervisor {
            VmHypervisor::Qemu => self.qemu.start_vm(vm).await?,
            VmHypervisor::Lxd => self.lxd.start_vm(vm).await?,
            VmHypervisor::Incus => self.incus.start_vm(vm).await?,
        }
        vm.status = VmStatus::Running;
        let config = vm.to_config();

        // Persist the new status if available
        if let Some(db) = &self.db {
            db.update_vm(&config).await?;
        }

        Ok(config)
    }

    /// Stop a virtual machine
    pub async fn stop_vm(&self, id: &str) -> Result<VmConfig> {
        let mut vms = self.vms.write().await;
        self.hydrate(&mut vms, id).await?;
        let vm = vms
            .get_mut(id)
            .ok_or_else(|| horcrux_common::Error::VmNotFound(id.to_string()))?;

        match vm.hypervisor {
            VmHypervisor::Qemu => self.qemu.stop_vm(vm).await?,
            VmHypervisor::Lxd => self.lxd.stop_vm(vm).await?,
            VmHypervisor::Incus => self.incus.stop_vm(vm).await?,
        }
        vm.status = VmStatus::Stopped;
        let config = vm.to_config();

        // Persist the new status if available
        if let Some(db) = &self.db {
            db.update_vm(&config).await?;
        }

        Ok(config)
    }

    /// Delete a virtual machine
    pub async fn delete_vm(&self, id: &str) -> Result<()> {
        let mut vms = self.vms.write().await;
        // Load DB-only VMs too, so their disk/process are cleaned up, not
        // just the row.
        let _ = self.hydrate(&mut vms, id).await;

        if let Some(vm) = vms.remove(id) {
            // Best-effort: the on-disk/hypervisor resources may already be
            // gone (e.g. after a server restart with a stale in-memory
            // entry); don't let that failure prevent the DB row cleanup.
            let _ = match vm.hypervisor {
                VmHypervisor::Qemu => self.qemu.delete_vm(&vm).await,
                VmHypervisor::Lxd => self.lxd.delete_vm(&vm).await,
                VmHypervisor::Incus => self.incus.delete_vm(&vm).await,
            };

            if let Some(db) = &self.db {
                db.delete_vm(id).await?;
            }

            Ok(())
        } else if let Some(db) = &self.db {
            // Not tracked in memory (e.g. loaded from a previous process),
            // but may still have a row in the database. Delete it there so
            // callers don't get a false VmNotFound for a VM that still
            // exists persistently.
            db.delete_vm(id).await
        } else {
            Err(horcrux_common::Error::VmNotFound(id.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horcrux_common::{VmArchitecture, VmConfig, VmHypervisor, VmStatus};

    fn create_test_vm_config(id: &str, name: &str) -> VmConfig {
        VmConfig {
            id: id.to_string(),
            name: name.to_string(),
            hypervisor: VmHypervisor::Qemu,
            memory: 2048,
            cpus: 2,
            disk_size: 20 * 1024 * 1024 * 1024, // 20GB
            status: VmStatus::Stopped,
            architecture: VmArchitecture::X86_64,
            disks: Vec::new(),
            display: Default::default(),
        }
    }

    #[tokio::test]
    async fn test_vm_manager_new() {
        let manager = VmManager::new();
        let vms = manager.list_vms().await;
        assert_eq!(vms.len(), 0, "New manager should have no VMs");
    }

    #[tokio::test]
    async fn test_list_vms_empty() {
        let manager = VmManager::new();
        let vms = manager.list_vms().await;
        assert_eq!(vms.len(), 0);
    }

    #[tokio::test]
    async fn test_get_vm_not_found() {
        let manager = VmManager::new();
        let result = manager.get_vm("non-existent").await;
        assert!(result.is_err(), "Getting non-existent VM should fail");
        match result {
            Err(horcrux_common::Error::VmNotFound(id)) => {
                assert_eq!(id, "non-existent");
            }
            _ => panic!("Expected VmNotFound error"),
        }
    }

    #[tokio::test]
    async fn test_delete_vm_not_found() {
        let manager = VmManager::new();
        let result = manager.delete_vm("non-existent").await;
        assert!(result.is_err(), "Deleting non-existent VM should fail");
        match result {
            Err(horcrux_common::Error::VmNotFound(id)) => {
                assert_eq!(id, "non-existent");
            }
            _ => panic!("Expected VmNotFound error"),
        }
    }

    #[tokio::test]
    async fn test_start_vm_not_found() {
        let manager = VmManager::new();
        let result = manager.start_vm("non-existent").await;
        assert!(result.is_err(), "Starting non-existent VM should fail");
        match result {
            Err(horcrux_common::Error::VmNotFound(id)) => {
                assert_eq!(id, "non-existent");
            }
            _ => panic!("Expected VmNotFound error"),
        }
    }

    #[tokio::test]
    async fn test_stop_vm_not_found() {
        let manager = VmManager::new();
        let result = manager.stop_vm("non-existent").await;
        assert!(result.is_err(), "Stopping non-existent VM should fail");
        match result {
            Err(horcrux_common::Error::VmNotFound(id)) => {
                assert_eq!(id, "non-existent");
            }
            _ => panic!("Expected VmNotFound error"),
        }
    }

    /// VMs are persisted, so a new manager over the same database (= an
    /// API restart) must still be able to operate on them. Requires
    /// `qemu-img` (installed in CI's Test job).
    #[tokio::test]
    async fn vm_is_usable_after_manager_restart() {
        let dir = std::env::temp_dir().join(format!("hx-vm-restart-{}", std::process::id()));
        let db = Arc::new(Database::new("sqlite::memory:").await.unwrap());
        db.migrate().await.unwrap();

        let first = VmManager::with_database_and_storage_path(db.clone(), dir.clone());
        let mut cfg = create_test_vm_config("restart-vm", "restart");
        cfg.disk_size = 1;
        first.create_vm(cfg).await.expect("create (needs qemu-img)");
        let disk = dir.join("restart-vm.qcow2");
        assert!(disk.exists());
        drop(first);

        let second = VmManager::with_database_and_storage_path(db.clone(), dir.clone());
        if let Err(horcrux_common::Error::VmNotFound(_)) = second.stop_vm("restart-vm").await {
            panic!("a persisted VM must not be 'not found' after a restart");
        }
        second.delete_vm("restart-vm").await.unwrap();
        assert!(
            !disk.exists(),
            "delete must remove the disk of a VM loaded from the DB"
        );
        assert!(second.get_vm("restart-vm").await.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
