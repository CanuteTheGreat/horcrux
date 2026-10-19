//! LXD integration for virtual machines
//! LXD can manage both VMs and containers - this module handles VMs
//!
//! Note: This module is future-ready but not yet integrated into the main API.
//! It will be activated when LXD VM management is added to the platform.
use super::QemuVm;
use horcrux_common::{Result, VmConfig, VmStatus};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::{error, info};

/// LXD VM representation
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LxdInstance {
    name: String,
    status: String,
    #[serde(rename = "type")]
    instance_type: String,
}

/// LXD manager for virtual machines
#[allow(dead_code)]
pub struct LxdManager {}

#[allow(dead_code)]
impl Default for LxdManager {
    fn default() -> Self {
        Self::new()
    }
}

impl LxdManager {
    pub fn new() -> Self {
        Self {}
    }

    /// `lxc init` arguments for a VM: the chosen image (`distro/release`
    /// from the public `images:` remote, `remote:alias`, or a local alias /
    /// fingerprint - the same rules as the container form), Ubuntu 22.04
    /// when none was given.
    pub fn init_args(config: &VmConfig, image: Option<&str>) -> Vec<String> {
        vec![
            "init".to_string(),
            "--vm".to_string(),
            crate::console::lxd::instance_image(image.unwrap_or("")),
            config.name.clone(),
            "-c".to_string(),
            format!("limits.cpu={}", config.cpus),
            "-c".to_string(),
            format!("limits.memory={}MB", config.memory),
            "-d".to_string(),
            format!("root,size={}GB", config.disk_size),
        ]
    }

    /// Create a new LXD virtual machine from `image` (see `init_args`).
    pub async fn create_vm(&self, config: &VmConfig, image: Option<&str>) -> Result<QemuVm> {
        info!(
            "Creating LXD VM: {} (ID: {}) from {}",
            config.name,
            config.id,
            crate::console::lxd::instance_image(image.unwrap_or(""))
        );

        let output = Command::new("lxc")
            .args(Self::init_args(config, image))
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run lxc init: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("lxc init failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create LXD VM: {}",
                stderr
            )));
        }

        info!("LXD VM {} created successfully", config.id);

        Ok(QemuVm {
            id: config.id.clone(),
            name: config.name.clone(),
            memory: config.memory,
            cpus: config.cpus,
            disk_path: std::path::PathBuf::from(format!(
                "/var/lib/lxd/virtual-machines/{}",
                config.name
            )),
            disk_size: config.disk_size,
            status: VmStatus::Stopped,
            _pid: None,
            hypervisor: horcrux_common::VmHypervisor::Lxd,
            architecture: config.architecture.clone(),
            display: config.display,
            hardware: Default::default(),
            disks: Vec::new(),
        })
    }

    /// Start an LXD virtual machine
    pub async fn start_vm(&self, vm: &QemuVm) -> Result<()> {
        info!("Starting LXD VM: {} (ID: {})", vm.name, vm.id);

        if vm.status == VmStatus::Running {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "VM {} is already running",
                vm.id
            )));
        }

        let output = Command::new("lxc")
            .arg("start")
            .arg(&vm.name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run lxc start: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("lxc start failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to start LXD VM: {}",
                stderr
            )));
        }

        info!("LXD VM {} started successfully", vm.id);
        Ok(())
    }

    /// Stop an LXD virtual machine
    pub async fn stop_vm(&self, vm: &QemuVm) -> Result<()> {
        info!("Stopping LXD VM: {} (ID: {})", vm.name, vm.id);

        if vm.status == VmStatus::Stopped {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "VM {} is already stopped",
                vm.id
            )));
        }

        let output = Command::new("lxc")
            .arg("stop")
            .arg(&vm.name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run lxc stop: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("lxc stop failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to stop LXD VM: {}",
                stderr
            )));
        }

        info!("LXD VM {} stopped successfully", vm.id);
        Ok(())
    }

    /// Delete an LXD virtual machine
    pub async fn delete_vm(&self, vm: &QemuVm) -> Result<()> {
        info!("Deleting LXD VM: {} (ID: {})", vm.name, vm.id);

        if vm.status == VmStatus::Running {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Cannot delete running VM {}. Stop it first.",
                vm.id
            )));
        }

        let output = Command::new("lxc")
            .arg("delete")
            .arg(&vm.name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run lxc delete: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("lxc delete failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to delete LXD VM: {}",
                stderr
            )));
        }

        info!("LXD VM {} deleted successfully", vm.id);
        Ok(())
    }

    /// Check if LXD is available
    pub fn check_lxd_available() -> bool {
        std::process::Command::new("lxc")
            .arg("version")
            .output()
            .is_ok()
    }

    /// Get LXD version
    pub async fn get_lxd_version() -> Result<String> {
        let output = Command::new("lxc")
            .arg("version")
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run lxc: {}", e)))?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(
                "LXD not found or not working".to_string(),
            ));
        }

        let version = String::from_utf8_lossy(&output.stdout);
        Ok(version.trim().to_string())
    }
}

#[cfg(test)]
mod image_tests {
    use super::*;

    fn cfg() -> VmConfig {
        VmConfig {
            id: "vm-1".into(),
            name: "vm1".into(),
            hypervisor: horcrux_common::VmHypervisor::Lxd,
            memory: 1024,
            cpus: 2,
            disk_size: 10,
            status: VmStatus::Stopped,
            architecture: Default::default(),
            disks: Vec::new(),
            display: Default::default(),
            hardware: Default::default(),
        }
    }

    #[test]
    fn vm_uses_the_chosen_image() {
        let m = LxdManager::init_args;
        assert_eq!(m(&cfg(), Some("debian/12"))[2], "images:debian/12");
        assert_eq!(
            m(&cfg(), Some("images:alpine/3.20"))[2],
            "images:alpine/3.20"
        );
        assert_eq!(m(&cfg(), Some("my-local-image"))[2], "my-local-image");
        assert_eq!(m(&cfg(), None)[2], "images:ubuntu/22.04");
        assert_eq!(m(&cfg(), Some("  "))[2], "images:ubuntu/22.04");
        let args = m(&cfg(), Some("debian/12"));
        assert_eq!(&args[..2], &["init", "--vm"]);
        assert_eq!(args[3], "vm1");
        assert!(args.contains(&"limits.memory=1024MB".to_string()));
    }
}
