///! Virtual machine management module
///! Handles QEMU/KVM, LXD, and Incus virtual machine lifecycle

pub mod qemu;
pub mod lxd;
pub mod incus;
pub mod vgpu;

pub use qemu::{QemuManager, QemuVm};

use horcrux_common::{Result, VmConfig};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::db::Database;

/// Virtual machine manager
pub struct VmManager {
    vms: Arc<RwLock<HashMap<String, QemuVm>>>,
    qemu: QemuManager,
    db: Option<Arc<Database>>,
}

impl VmManager {
    pub fn new() -> Self {
        Self {
            vms: Arc::new(RwLock::new(HashMap::new())),
            qemu: QemuManager::new(),
            db: None,
        }
    }

    /// Create VmManager with database support
    pub fn with_database(db: Arc<Database>) -> Self {
        Self {
            vms: Arc::new(RwLock::new(HashMap::new())),
            qemu: QemuManager::new(),
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
            return Err(horcrux_common::Error::InvalidConfig(
                format!("VM with ID {} already exists", config.id)
            ));
        }

        // Create the VM
        let vm = self.qemu.create_vm(&config).await?;
        let vm_config = vm.to_config();

        // Save to database if available
        if let Some(db) = &self.db {
            db.create_vm(&vm_config).await?;
        }

        vms.insert(config.id.clone(), vm);
        Ok(vm_config)
    }

    /// Start a virtual machine
    pub async fn start_vm(&self, id: &str) -> Result<VmConfig> {
        let vms = self.vms.read().await;
        let vm = vms.get(id)
            .ok_or_else(|| horcrux_common::Error::VmNotFound(id.to_string()))?;

        self.qemu.start_vm(vm).await?;
        Ok(vm.to_config())
    }

    /// Stop a virtual machine
    pub async fn stop_vm(&self, id: &str) -> Result<VmConfig> {
        let vms = self.vms.read().await;
        let vm = vms.get(id)
            .ok_or_else(|| horcrux_common::Error::VmNotFound(id.to_string()))?;

        self.qemu.stop_vm(vm).await?;
        Ok(vm.to_config())
    }

    /// Delete a virtual machine
    pub async fn delete_vm(&self, id: &str) -> Result<()> {
        let mut vms = self.vms.write().await;

        if let Some(vm) = vms.remove(id) {
            self.qemu.delete_vm(&vm).await?;

            // Delete from database if available
            if let Some(db) = &self.db {
                db.delete_vm(id).await?;
            }

            Ok(())
        } else {
            Err(horcrux_common::Error::VmNotFound(id.to_string()))
        }
    }
}
