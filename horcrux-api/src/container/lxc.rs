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

    /// Get container status
    pub async fn get_container_status(&self, name: &str) -> Result<ContainerStatus> {
        let output = Command::new("lxc-info")
            .arg("-n")
            .arg(name)
            .arg("-s")
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run lxc-info: {}", e))
            })?;

        if !output.status.success() {
            return Err(horcrux_common::Error::ContainerNotFound(name.to_string()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let status = stdout.trim();

        if status.contains("RUNNING") {
            Ok(ContainerStatus::Running)
        } else if status.contains("STOPPED") {
            Ok(ContainerStatus::Stopped)
        } else if status.contains("FROZEN") {
            Ok(ContainerStatus::Paused)
        } else {
            Ok(ContainerStatus::Unknown)
        }
    }

    /// Pause/freeze a container
    pub async fn pause_container(&self, container: &Container) -> Result<()> {
        info!("Pausing LXC container: {} (ID: {})", container.name, container.id);

        let output = Command::new("lxc-freeze")
            .arg("-n")
            .arg(&container.name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run lxc-freeze: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to pause container: {}",
                stderr
            )));
        }

        info!("LXC container {} paused successfully", container.id);
        Ok(())
    }

    /// Resume/unfreeze a container
    pub async fn resume_container(&self, container: &Container) -> Result<()> {
        info!("Resuming LXC container: {} (ID: {})", container.name, container.id);

        let output = Command::new("lxc-unfreeze")
            .arg("-n")
            .arg(&container.name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run lxc-unfreeze: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to resume container: {}",
                stderr
            )));
        }

        info!("LXC container {} resumed successfully", container.id);
        Ok(())
    }

    /// Get container information
    pub async fn get_container_info(&self, name: &str) -> Result<ContainerInfo> {
        let output = Command::new("lxc-info")
            .arg("-n")
            .arg(name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run lxc-info: {}", e))
            })?;

        if !output.status.success() {
            return Err(horcrux_common::Error::ContainerNotFound(name.to_string()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        self.parse_container_info(&stdout)
    }

    /// Parse lxc-info output
    fn parse_container_info(&self, output: &str) -> Result<ContainerInfo> {
        let mut info = ContainerInfo::default();

        for line in output.lines() {
            let parts: Vec<&str> = line.splitn(2, ':').collect();
            if parts.len() != 2 {
                continue;
            }

            let key = parts[0].trim();
            let value = parts[1].trim();

            match key {
                "Name" => info.name = value.to_string(),
                "State" => {
                    info.state = match value {
                        "RUNNING" => ContainerStatus::Running,
                        "STOPPED" => ContainerStatus::Stopped,
                        "FROZEN" => ContainerStatus::Paused,
                        _ => ContainerStatus::Unknown,
                    }
                }
                "PID" => info.pid = value.parse().ok(),
                "IP" => info.ip_address = Some(value.to_string()),
                "CPU use" => info.cpu_usage = value.split_whitespace().next().and_then(|s| s.parse().ok()),
                "Memory use" => {
                    if let Some(mem_str) = value.split_whitespace().next() {
                        info.memory_usage = self.parse_memory(mem_str);
                    }
                }
                _ => {}
            }
        }

        Ok(info)
    }

    /// Parse memory string (e.g., "512.00 MiB" -> bytes)
    fn parse_memory(&self, mem_str: &str) -> Option<u64> {
        let parts: Vec<&str> = mem_str.split_whitespace().collect();
        if parts.len() < 2 {
            return None;
        }

        let value: f64 = parts[0].parse().ok()?;
        let unit = parts[1];

        let bytes = match unit {
            "KiB" => value * 1024.0,
            "MiB" => value * 1024.0 * 1024.0,
            "GiB" => value * 1024.0 * 1024.0 * 1024.0,
            "B" => value,
            _ => return None,
        };

        Some(bytes as u64)
    }

    /// Execute command in container
    pub async fn exec_command(&self, name: &str, command: &[String]) -> Result<String> {
        debug!("Executing command in container {}: {:?}", name, command);

        let mut cmd = Command::new("lxc-attach");
        cmd.arg("-n").arg(name).arg("--");

        for arg in command {
            cmd.arg(arg);
        }

        let output = cmd.output().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to run lxc-attach: {}", e))
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

    /// List all LXC containers
    pub async fn list_all_containers(&self) -> Result<Vec<String>> {
        let output = Command::new("lxc-ls")
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run lxc-ls: {}", e))
            })?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(
                "Failed to list LXC containers".to_string(),
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
    }

    /// Clone/snapshot a container
    pub async fn clone_container(&self, source: &str, target: &str, snapshot: bool) -> Result<()> {
        info!("Cloning LXC container: {} -> {} (snapshot: {})", source, target, snapshot);

        let mut cmd = Command::new("lxc-copy");
        cmd.arg("-n").arg(source).arg("-N").arg(target);

        if snapshot {
            cmd.arg("-s");
        }

        let output = cmd.output().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to run lxc-copy: {}", e))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to clone container: {}",
                stderr
            )));
        }

        info!("Container cloned successfully: {} -> {}", source, target);
        Ok(())
    }
}

/// Container information
#[derive(Debug, Clone, Default)]
pub struct ContainerInfo {
    pub name: String,
    pub state: ContainerStatus,
    pub pid: Option<u32>,
    pub ip_address: Option<String>,
    pub cpu_usage: Option<f64>,
    pub memory_usage: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lxc_manager_creation() {
        let manager = LxcManager::new();
        assert_eq!(manager.storage_path, PathBuf::from("/var/lib/lxc"));
    }

    #[test]
    fn test_parse_memory() {
        let manager = LxcManager::new();
        assert_eq!(manager.parse_memory("512.00 MiB"), Some(536870912));
        assert_eq!(manager.parse_memory("1.50 GiB"), Some(1610612736));
        assert_eq!(manager.parse_memory("100 KiB"), Some(102400));
        assert_eq!(manager.parse_memory("invalid"), None);
    }
}
