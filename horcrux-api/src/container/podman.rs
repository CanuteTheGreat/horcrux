///! Podman container integration
///! Podman is a daemonless alternative to Docker

use super::Container;
use horcrux_common::{ContainerConfig, ContainerRuntime, ContainerStatus, Result};
use tokio::process::Command;
use tracing::{error, info};

/// Podman container manager
pub struct PodmanManager {}

impl PodmanManager {
    pub fn new() -> Self {
        Self {}
    }

    /// Create a new Podman container
    pub async fn create_container(&self, config: &ContainerConfig) -> Result<Container> {
        info!("Creating Podman container: {} (ID: {})", config.name, config.id);

        // Create and configure Podman container
        let mut cmd = Command::new("podman");
        cmd.arg("create")
            .arg("--name")
            .arg(&config.name)
            .arg("--memory")
            .arg(format!("{}m", config.memory))
            .arg("--cpus")
            .arg(config.cpus.to_string())
            .arg(&config.rootfs); // rootfs is the image name for Podman

        let output = cmd.output().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to run podman create: {}", e))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("podman create failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create Podman container: {}",
                stderr
            )));
        }

        let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        info!("Podman container {} created successfully (Podman ID: {})", config.id, container_id);

        Ok(Container {
            id: config.id.clone(),
            name: config.name.clone(),
            runtime: ContainerRuntime::Podman,
            memory: config.memory,
            cpus: config.cpus,
            rootfs: config.rootfs.clone(),
            status: ContainerStatus::Stopped,
        })
    }

    /// Start a Podman container
    pub async fn start_container(&self, container: &Container) -> Result<()> {
        info!("Starting Podman container: {} (ID: {})", container.name, container.id);

        if container.status == ContainerStatus::Running {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Container {} is already running",
                container.id
            )));
        }

        let output = Command::new("podman")
            .arg("start")
            .arg(&container.name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run podman start: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("podman start failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to start Podman container: {}",
                stderr
            )));
        }

        info!("Podman container {} started successfully", container.id);
        Ok(())
    }

    /// Stop a Podman container
    pub async fn stop_container(&self, container: &Container) -> Result<()> {
        info!("Stopping Podman container: {} (ID: {})", container.name, container.id);

        if container.status == ContainerStatus::Stopped {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Container {} is already stopped",
                container.id
            )));
        }

        let output = Command::new("podman")
            .arg("stop")
            .arg(&container.name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run podman stop: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("podman stop failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to stop Podman container: {}",
                stderr
            )));
        }

        info!("Podman container {} stopped successfully", container.id);
        Ok(())
    }

    /// Delete a Podman container
    pub async fn delete_container(&self, container: &Container) -> Result<()> {
        info!("Deleting Podman container: {} (ID: {})", container.name, container.id);

        if container.status == ContainerStatus::Running {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Cannot delete running container {}. Stop it first.",
                container.id
            )));
        }

        let output = Command::new("podman")
            .arg("rm")
            .arg(&container.name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run podman rm: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("podman rm failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to delete Podman container: {}",
                stderr
            )));
        }

        info!("Podman container {} deleted successfully", container.id);
        Ok(())
    }

    /// Check if Podman is available
    pub fn check_podman_available() -> bool {
        std::process::Command::new("podman")
            .arg("--version")
            .output()
            .is_ok()
    }

    /// Get Podman version
    pub async fn get_podman_version() -> Result<String> {
        let output = Command::new("podman")
            .arg("--version")
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run podman: {}", e))
            })?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(
                "Podman not found or not working".to_string(),
            ));
        }

        let version = String::from_utf8_lossy(&output.stdout);
        Ok(version.trim().to_string())
    }
}
