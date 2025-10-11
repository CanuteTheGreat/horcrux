///! LXD container integration
///! LXD can manage both VMs and containers - this module handles containers

use super::Container;
use horcrux_common::{ContainerConfig, ContainerRuntime, ContainerStatus, Result};
use tokio::process::Command;
use tracing::{error, info};

/// LXD container manager
pub struct LxdContainerManager {}

impl LxdContainerManager {
    pub fn new() -> Self {
        Self {}
    }

    /// Create a new LXD container
    pub async fn create_container(&self, config: &ContainerConfig) -> Result<Container> {
        info!("Creating LXD container: {} (ID: {})", config.name, config.id);

        // Launch container instance with LXD
        let output = Command::new("lxc")
            .arg("init")
            .arg("images:ubuntu/22.04") // Default image
            .arg(&config.name)
            .arg("-c")
            .arg(format!("limits.cpu={}", config.cpus))
            .arg("-c")
            .arg(format!("limits.memory={}MB", config.memory))
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run lxc init: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("lxc init failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create LXD container: {}",
                stderr
            )));
        }

        info!("LXD container {} created successfully", config.id);

        Ok(Container {
            id: config.id.clone(),
            name: config.name.clone(),
            runtime: ContainerRuntime::Lxd,
            memory: config.memory,
            cpus: config.cpus,
            rootfs: format!("/var/lib/lxd/containers/{}", config.name),
            status: ContainerStatus::Stopped,
        })
    }

    /// Start an LXD container
    pub async fn start_container(&self, container: &Container) -> Result<()> {
        info!("Starting LXD container: {} (ID: {})", container.name, container.id);

        if container.status == ContainerStatus::Running {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Container {} is already running",
                container.id
            )));
        }

        let output = Command::new("lxc")
            .arg("start")
            .arg(&container.name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run lxc start: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("lxc start failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to start LXD container: {}",
                stderr
            )));
        }

        info!("LXD container {} started successfully", container.id);
        Ok(())
    }

    /// Stop an LXD container
    pub async fn stop_container(&self, container: &Container) -> Result<()> {
        info!("Stopping LXD container: {} (ID: {})", container.name, container.id);

        if container.status == ContainerStatus::Stopped {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Container {} is already stopped",
                container.id
            )));
        }

        let output = Command::new("lxc")
            .arg("stop")
            .arg(&container.name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run lxc stop: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("lxc stop failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to stop LXD container: {}",
                stderr
            )));
        }

        info!("LXD container {} stopped successfully", container.id);
        Ok(())
    }

    /// Delete an LXD container
    pub async fn delete_container(&self, container: &Container) -> Result<()> {
        info!("Deleting LXD container: {} (ID: {})", container.name, container.id);

        if container.status == ContainerStatus::Running {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Cannot delete running container {}. Stop it first.",
                container.id
            )));
        }

        let output = Command::new("lxc")
            .arg("delete")
            .arg(&container.name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run lxc delete: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("lxc delete failed: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to delete LXD container: {}",
                stderr
            )));
        }

        info!("LXD container {} deleted successfully", container.id);
        Ok(())
    }

    /// Pause/freeze a container
    pub async fn pause_container(&self, container: &Container) -> Result<()> {
        let output = Command::new("lxc")
            .arg("pause")
            .arg(&container.name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run lxc pause: {}", e)))?;

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
        let output = Command::new("lxc")
            .arg("start")
            .arg(&container.name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run lxc start: {}", e)))?;

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
        let output = Command::new("lxc")
            .arg("info")
            .arg(name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run lxc info: {}", e)))?;

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
        let mut cmd = Command::new("lxc");
        cmd.arg("exec").arg(name).arg("--");

        for arg in command {
            cmd.arg(arg);
        }

        let output = cmd.output().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to run lxc exec: {}", e))
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
        let mut cmd = Command::new("lxc");
        cmd.arg("copy").arg(source).arg(target);

        if snapshot {
            cmd.arg("--instance-only");
        }

        let output = cmd.output().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to run lxc copy: {}", e))
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
