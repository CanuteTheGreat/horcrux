///! Incus container integration
///! Incus (LXD fork) can manage both VMs and containers - this module handles containers

use super::Container;
use horcrux_common::{ContainerConfig, ContainerRuntime, ContainerStatus, Result};
use tokio::process::Command;
use tracing::{error, info};

/// Incus container manager
pub struct IncusContainerManager {}

impl IncusContainerManager {
    pub fn new() -> Self {
        Self {}
    }

    /// Create a new Incus container
    pub async fn create_container(&self, config: &ContainerConfig) -> Result<Container> {
        info!("Creating Incus container: {} (ID: {})", config.name, config.id);

        // Launch container instance with Incus
        let output = Command::new("incus")
            .arg("init")
            .arg("images:ubuntu/22.04") // Default image
            .arg(&config.name)
            .arg("-c")
            .arg(format!("limits.cpu={}", config.cpus))
            .arg("-c")
            .arg(format!("limits.memory={}MB", config.memory))
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run incus init: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("incus init failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create Incus container: {}",
                stderr
            )));
        }

        info!("Incus container {} created successfully", config.id);

        Ok(Container {
            id: config.id.clone(),
            name: config.name.clone(),
            runtime: ContainerRuntime::Incus,
            memory: config.memory,
            cpus: config.cpus,
            rootfs: format!("/var/lib/incus/containers/{}", config.name),
            status: ContainerStatus::Stopped,
        })
    }

    /// Start an Incus container
    pub async fn start_container(&self, container: &Container) -> Result<()> {
        info!("Starting Incus container: {} (ID: {})", container.name, container.id);

        if container.status == ContainerStatus::Running {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Container {} is already running",
                container.id
            )));
        }

        let output = Command::new("incus")
            .arg("start")
            .arg(&container.name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run incus start: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("incus start failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to start Incus container: {}",
                stderr
            )));
        }

        info!("Incus container {} started successfully", container.id);
        Ok(())
    }

    /// Stop an Incus container
    pub async fn stop_container(&self, container: &Container) -> Result<()> {
        info!("Stopping Incus container: {} (ID: {})", container.name, container.id);

        if container.status == ContainerStatus::Stopped {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Container {} is already stopped",
                container.id
            )));
        }

        let output = Command::new("incus")
            .arg("stop")
            .arg(&container.name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run incus stop: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("incus stop failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to stop Incus container: {}",
                stderr
            )));
        }

        info!("Incus container {} stopped successfully", container.id);
        Ok(())
    }

    /// Delete an Incus container
    pub async fn delete_container(&self, container: &Container) -> Result<()> {
        info!("Deleting Incus container: {} (ID: {})", container.name, container.id);

        if container.status == ContainerStatus::Running {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Cannot delete running container {}. Stop it first.",
                container.id
            )));
        }

        let output = Command::new("incus")
            .arg("delete")
            .arg(&container.name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run incus delete: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("incus delete failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to delete Incus container: {}",
                stderr
            )));
        }

        info!("Incus container {} deleted successfully", container.id);
        Ok(())
    }
}
