///! Incus integration for virtual machines
///! Incus (LXD fork) can manage both VMs and containers - this module handles VMs

use super::QemuVm;
use horcrux_common::{Result, VmConfig, VmStatus};
use tokio::process::Command;
use tracing::{error, info};

/// Incus manager for virtual machines
pub struct IncusManager {}

impl IncusManager {
    pub fn new() -> Self {
        Self {}
    }

    /// Create a new Incus virtual machine
    pub async fn create_vm(&self, config: &VmConfig) -> Result<QemuVm> {
        info!("Creating Incus VM: {} (ID: {})", config.name, config.id);

        // Launch VM instance with Incus
        let output = Command::new("incus")
            .arg("init")
            .arg("--vm")
            .arg("images:ubuntu/22.04") // Default image
            .arg(&config.name)
            .arg("-c")
            .arg(format!("limits.cpu={}", config.cpus))
            .arg("-c")
            .arg(format!("limits.memory={}MB", config.memory))
            .arg("-d")
            .arg(format!("root,size={}GB", config.disk_size))
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run incus init: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("incus init failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create Incus VM: {}",
                stderr
            )));
        }

        info!("Incus VM {} created successfully", config.id);

        Ok(QemuVm {
            id: config.id.clone(),
            name: config.name.clone(),
            memory: config.memory,
            cpus: config.cpus,
            disk_path: std::path::PathBuf::from(format!("/var/lib/incus/virtual-machines/{}", config.name)),
            disk_size: config.disk_size,
            status: VmStatus::Stopped,
            pid: None,
        })
    }

    /// Start an Incus virtual machine
    pub async fn start_vm(&self, vm: &QemuVm) -> Result<()> {
        info!("Starting Incus VM: {} (ID: {})", vm.name, vm.id);

        if vm.status == VmStatus::Running {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "VM {} is already running",
                vm.id
            )));
        }

        let output = Command::new("incus")
            .arg("start")
            .arg(&vm.name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run incus start: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("incus start failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to start Incus VM: {}",
                stderr
            )));
        }

        info!("Incus VM {} started successfully", vm.id);
        Ok(())
    }

    /// Stop an Incus virtual machine
    pub async fn stop_vm(&self, vm: &QemuVm) -> Result<()> {
        info!("Stopping Incus VM: {} (ID: {})", vm.name, vm.id);

        if vm.status == VmStatus::Stopped {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "VM {} is already stopped",
                vm.id
            )));
        }

        let output = Command::new("incus")
            .arg("stop")
            .arg(&vm.name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run incus stop: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("incus stop failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to stop Incus VM: {}",
                stderr
            )));
        }

        info!("Incus VM {} stopped successfully", vm.id);
        Ok(())
    }

    /// Delete an Incus virtual machine
    pub async fn delete_vm(&self, vm: &QemuVm) -> Result<()> {
        info!("Deleting Incus VM: {} (ID: {})", vm.name, vm.id);

        if vm.status == VmStatus::Running {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Cannot delete running VM {}. Stop it first.",
                vm.id
            )));
        }

        let output = Command::new("incus")
            .arg("delete")
            .arg(&vm.name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run incus delete: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("incus delete failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to delete Incus VM: {}",
                stderr
            )));
        }

        info!("Incus VM {} deleted successfully", vm.id);
        Ok(())
    }

    /// Check if Incus is available
    pub fn check_incus_available() -> bool {
        std::process::Command::new("incus")
            .arg("version")
            .output()
            .is_ok()
    }

    /// Get Incus version
    pub async fn get_incus_version() -> Result<String> {
        let output = Command::new("incus")
            .arg("version")
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run incus: {}", e)))?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(
                "Incus not found or not working".to_string(),
            ));
        }

        let version = String::from_utf8_lossy(&output.stdout);
        Ok(version.trim().to_string())
    }
}
