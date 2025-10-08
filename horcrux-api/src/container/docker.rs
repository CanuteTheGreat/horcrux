///! Docker container integration

use super::Container;
use horcrux_common::{ContainerConfig, ContainerRuntime, ContainerStatus, Result};
use tokio::process::Command;
use tracing::{error, info};

/// Docker container manager
pub struct DockerManager {}

impl DockerManager {
    pub fn new() -> Self {
        Self {}
    }

    /// Create a new Docker container
    pub async fn create_container(&self, config: &ContainerConfig) -> Result<Container> {
        info!("Creating Docker container: {} (ID: {})", config.name, config.id);

        // Create and configure Docker container
        let mut cmd = Command::new("docker");
        cmd.arg("create")
            .arg("--name")
            .arg(&config.name)
            .arg("--memory")
            .arg(format!("{}m", config.memory))
            .arg("--cpus")
            .arg(config.cpus.to_string())
            .arg(&config.rootfs); // rootfs is the image name for Docker

        let output = cmd.output().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to run docker create: {}", e))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("docker create failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create Docker container: {}",
                stderr
            )));
        }

        let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        info!("Docker container {} created successfully (Docker ID: {})", config.id, container_id);

        Ok(Container {
            id: config.id.clone(),
            name: config.name.clone(),
            runtime: ContainerRuntime::Docker,
            memory: config.memory,
            cpus: config.cpus,
            rootfs: config.rootfs.clone(),
            status: ContainerStatus::Stopped,
        })
    }

    /// Start a Docker container
    pub async fn start_container(&self, container: &Container) -> Result<()> {
        info!("Starting Docker container: {} (ID: {})", container.name, container.id);

        if container.status == ContainerStatus::Running {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Container {} is already running",
                container.id
            )));
        }

        let output = Command::new("docker")
            .arg("start")
            .arg(&container.name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run docker start: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("docker start failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to start Docker container: {}",
                stderr
            )));
        }

        info!("Docker container {} started successfully", container.id);
        Ok(())
    }

    /// Stop a Docker container
    pub async fn stop_container(&self, container: &Container) -> Result<()> {
        info!("Stopping Docker container: {} (ID: {})", container.name, container.id);

        if container.status == ContainerStatus::Stopped {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Container {} is already stopped",
                container.id
            )));
        }

        let output = Command::new("docker")
            .arg("stop")
            .arg(&container.name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run docker stop: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("docker stop failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to stop Docker container: {}",
                stderr
            )));
        }

        info!("Docker container {} stopped successfully", container.id);
        Ok(())
    }

    /// Delete a Docker container
    pub async fn delete_container(&self, container: &Container) -> Result<()> {
        info!("Deleting Docker container: {} (ID: {})", container.name, container.id);

        // Docker allows removing running containers with -f, but we'll enforce stopping first
        if container.status == ContainerStatus::Running {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Cannot delete running container {}. Stop it first.",
                container.id
            )));
        }

        let output = Command::new("docker")
            .arg("rm")
            .arg(&container.name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run docker rm: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("docker rm failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to delete Docker container: {}",
                stderr
            )));
        }

        info!("Docker container {} deleted successfully", container.id);
        Ok(())
    }

    /// Check if Docker is available
    pub fn check_docker_available() -> bool {
        std::process::Command::new("docker")
            .arg("--version")
            .output()
            .is_ok()
    }

    /// Get Docker version
    pub async fn get_docker_version() -> Result<String> {
        let output = Command::new("docker")
            .arg("--version")
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run docker: {}", e))
            })?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(
                "Docker not found or not working".to_string(),
            ));
        }

        let version = String::from_utf8_lossy(&output.stdout);
        Ok(version.trim().to_string())
    }
}
