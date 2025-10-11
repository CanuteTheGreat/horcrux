///! Incus container integration
///! Incus (LXD fork) can manage both VMs and containers - this module handles containers
///!
///! Note: This module is future-ready but not yet integrated into the main API.
///! It will be activated when Incus container management is added to the platform.

use super::Container;
use horcrux_common::{ContainerConfig, ContainerRuntime, ContainerStatus, Result};
use tokio::process::Command;
use tracing::{error, info};

/// Incus container manager
#[allow(dead_code)]
pub struct IncusContainerManager {}

#[allow(dead_code)]
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

    /// Pause/freeze a container
    pub async fn pause_container(&self, container: &Container) -> Result<()> {
        let output = Command::new("incus")
            .arg("pause")
            .arg(&container.name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run incus pause: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to pause container: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Resume/unfreeze a container
    pub async fn resume_container(&self, container: &Container) -> Result<()> {
        let output = Command::new("incus")
            .arg("start")
            .arg(&container.name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run incus start: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to resume container: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Get container status
    pub async fn get_container_status(&self, name: &str) -> Result<ContainerStatus> {
        let output = Command::new("incus")
            .arg("info")
            .arg(name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run incus info: {}", e)))?;

        if !output.status.success() {
            return Err(horcrux_common::Error::ContainerNotFound(name.to_string()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.contains("Status: Running") || stdout.contains("Status: RUNNING") {
            Ok(ContainerStatus::Running)
        } else if stdout.contains("Status: Stopped") || stdout.contains("Status: STOPPED") {
            Ok(ContainerStatus::Stopped)
        } else if stdout.contains("Status: Frozen") || stdout.contains("Status: FROZEN") {
            Ok(ContainerStatus::Paused)
        } else {
            Ok(ContainerStatus::Unknown)
        }
    }

    /// Execute command in container
    pub async fn exec_command(&self, name: &str, command: &[String]) -> Result<String> {
        let mut cmd = Command::new("incus");
        cmd.arg("exec").arg(name).arg("--");

        for arg in command {
            cmd.arg(arg);
        }

        let output = cmd.output().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to run incus exec: {}", e))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Command failed: {}",
                stderr
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Clone a container
    pub async fn clone_container(&self, source: &str, target: &str, snapshot: bool) -> Result<()> {
        let mut cmd = Command::new("incus");
        cmd.arg("copy").arg(source).arg(target);

        if snapshot {
            cmd.arg("--instance-only");
        }

        let output = cmd.output().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to run incus copy: {}", e))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to clone container: {}",
                stderr
            )));
        }

        Ok(())
    }
}
