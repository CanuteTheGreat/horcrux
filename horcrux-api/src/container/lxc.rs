///! LXC container integration

use super::Container;
use horcrux_common::{ContainerConfig, ContainerRuntime, ContainerStatus, Result};
use std::path::PathBuf;
use tokio::process::Command;
use tracing::{debug, error, info};

/// LXC container manager
pub struct LxcManager {
    storage_path: PathBuf,
}

impl LxcManager {
    pub fn new() -> Self {
        Self {
            storage_path: PathBuf::from("/var/lib/lxc"),
        }
    }

    /// Create a new LXC container
    pub async fn create_container(&self, config: &ContainerConfig) -> Result<Container> {
        info!("Creating LXC container: {} (ID: {})", config.name, config.id);

        // Create container using lxc-create
        let output = Command::new("lxc-create")
            .arg("-n")
            .arg(&config.name)
            .arg("-t")
            .arg("download")
            .arg("--")
            .arg("-d")
            .arg("alpine") // Default to Alpine Linux
            .arg("-r")
            .arg("3.19")
            .arg("-a")
            .arg("amd64")
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run lxc-create: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("lxc-create failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create LXC container: {}",
                stderr
            )));
        }

        // Configure memory and CPU limits
        self.configure_limits(&config.name, config.memory, config.cpus)
            .await?;

        info!("LXC container {} created successfully", config.id);

        Ok(Container {
            id: config.id.clone(),
            name: config.name.clone(),
            runtime: ContainerRuntime::Lxc,
            memory: config.memory,
            cpus: config.cpus,
            rootfs: self.storage_path.join(&config.name).to_string_lossy().to_string(),
            status: ContainerStatus::Stopped,
        })
    }

    /// Configure LXC container resource limits
    async fn configure_limits(&self, name: &str, memory: u64, cpus: u32) -> Result<()> {
        let config_path = self.storage_path.join(name).join("config");

        // Append resource limits to config
        let limits = format!(
            "\n# Resource limits\nlxc.cgroup2.memory.max = {}M\nlxc.cgroup2.cpu.max = {}\n",
            memory, cpus
        );

        tokio::fs::write(&config_path, limits)
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!(
                    "Failed to configure container limits: {}",
                    e
                ))
            })?;

        Ok(())
    }

    /// Start an LXC container
    pub async fn start_container(&self, container: &Container) -> Result<()> {
        info!("Starting LXC container: {} (ID: {})", container.name, container.id);

        if container.status == ContainerStatus::Running {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Container {} is already running",
                container.id
            )));
        }

        let output = Command::new("lxc-start")
            .arg("-n")
            .arg(&container.name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run lxc-start: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("lxc-start failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to start LXC container: {}",
                stderr
            )));
        }

        info!("LXC container {} started successfully", container.id);
        Ok(())
    }

    /// Stop an LXC container
    pub async fn stop_container(&self, container: &Container) -> Result<()> {
        info!("Stopping LXC container: {} (ID: {})", container.name, container.id);

        if container.status == ContainerStatus::Stopped {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Container {} is already stopped",
                container.id
            )));
        }

        let output = Command::new("lxc-stop")
            .arg("-n")
            .arg(&container.name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run lxc-stop: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("lxc-stop failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to stop LXC container: {}",
                stderr
            )));
        }

        info!("LXC container {} stopped successfully", container.id);
        Ok(())
    }

    /// Delete an LXC container
    pub async fn delete_container(&self, container: &Container) -> Result<()> {
        info!("Deleting LXC container: {} (ID: {})", container.name, container.id);

        // Ensure container is stopped first
        if container.status == ContainerStatus::Running {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Cannot delete running container {}. Stop it first.",
                container.id
            )));
        }

        let output = Command::new("lxc-destroy")
            .arg("-n")
            .arg(&container.name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run lxc-destroy: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("lxc-destroy failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to delete LXC container: {}",
                stderr
            )));
        }

        info!("LXC container {} deleted successfully", container.id);
        Ok(())
    }

    /// Check if LXC is available
    pub fn check_lxc_available() -> bool {
        std::process::Command::new("lxc-start")
            .arg("--version")
            .output()
            .is_ok()
    }

    /// Get LXC version
    pub async fn get_lxc_version() -> Result<String> {
        let output = Command::new("lxc-start")
            .arg("--version")
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run lxc-start: {}", e))
            })?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(
                "LXC not found or not working".to_string(),
            ));
        }

        let version = String::from_utf8_lossy(&output.stdout);
        Ok(version.trim().to_string())
    }
}
