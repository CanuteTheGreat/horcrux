//! Podman container integration
//! Podman is a daemonless alternative to Docker
use super::Container;
use horcrux_common::{ContainerConfig, ContainerRuntime, ContainerStatus, Result};
use tokio::process::Command;
use tracing::{error, info};

/// A container as `podman ps` lists it.
#[derive(Debug, Clone)]
pub struct PodmanSummary {
    pub name: String,
    pub status: ContainerStatus,
    pub image: String,
    pub created_at: Option<String>,
    pub horcrux_id: Option<String>,
}

/// Podman container manager
pub struct PodmanManager {}

impl Default for PodmanManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PodmanManager {
    pub fn new() -> Self {
        Self {}
    }

    /// Every container `podman` knows (running or not), for reconciling the
    /// API's map after a restart. Err when podman is not usable here.
    pub async fn list_live(&self) -> Result<Vec<PodmanSummary>> {
        let out = Command::new("podman")
            .args(["ps", "-a", "--format", "json"])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("podman ps: {e}")))?;
        if !out.status.success() {
            return Err(horcrux_common::Error::System(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ));
        }
        let rows: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap_or_default();
        Ok(rows
            .iter()
            .map(|r| {
                let name = r["Names"]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|n| n.as_str())
                    .or_else(|| r["Names"].as_str())
                    .unwrap_or_default()
                    .to_string();
                let state = r["State"].as_str().unwrap_or("").to_ascii_lowercase();
                PodmanSummary {
                    status: match state.as_str() {
                        "running" => ContainerStatus::Running,
                        "paused" => ContainerStatus::Paused,
                        "exited" | "created" | "stopped" | "configured" => ContainerStatus::Stopped,
                        _ => ContainerStatus::Unknown,
                    },
                    image: r["Image"].as_str().unwrap_or_default().to_string(),
                    created_at: match &r["Created"] {
                        serde_json::Value::String(s) => Some(s.clone()),
                        serde_json::Value::Number(n) => n
                            .as_i64()
                            .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
                            .map(|t| t.to_rfc3339()),
                        _ => None,
                    },
                    horcrux_id: r["Labels"][super::HORCRUX_ID_LABEL]
                        .as_str()
                        .map(str::to_string),
                    name,
                }
            })
            .collect())
    }

    /// Create a new Podman container
    pub async fn create_container(&self, config: &ContainerConfig) -> Result<Container> {
        info!(
            "Creating Podman container: {} (ID: {})",
            config.name, config.id
        );

        // Create and configure Podman container
        let mut cmd = Command::new("podman");
        cmd.arg("create")
            .arg("--name")
            .arg(&config.name)
            .arg("--label")
            .arg(format!("{}={}", super::HORCRUX_ID_LABEL, config.id))
            .arg("--memory")
            .arg(format!("{}m", config.memory))
            .arg("--cpus")
            .arg(config.cpus.to_string())
            .args(super::oci_option_args(&config.options))
            .arg(&config.rootfs) // rootfs is the image name for Podman
            .args(&config.options.command);

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
        info!(
            "Podman container {} created successfully (Podman ID: {})",
            config.id, container_id
        );

        Ok(Container {
            id: config.id.clone(),
            name: config.name.clone(),
            runtime: ContainerRuntime::Podman,
            memory: config.memory,
            cpus: config.cpus,
            rootfs: config.rootfs.clone(),
            status: ContainerStatus::Stopped,
            options: config.options.clone(),
            created_at: Some(chrono::Utc::now().to_rfc3339()),
            lxc: None,
        })
    }

    /// Start a Podman container
    pub async fn start_container(&self, container: &Container) -> Result<()> {
        info!(
            "Starting Podman container: {} (ID: {})",
            container.name, container.id
        );

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
        info!(
            "Stopping Podman container: {} (ID: {})",
            container.name, container.id
        );

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
        info!(
            "Deleting Podman container: {} (ID: {})",
            container.name, container.id
        );

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
            .map_err(|e| horcrux_common::Error::System(format!("Failed to run podman: {}", e)))?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(
                "Podman not found or not working".to_string(),
            ));
        }

        let version = String::from_utf8_lossy(&output.stdout);
        Ok(version.trim().to_string())
    }

    /// Pause a container
    pub async fn pause_container(&self, container: &Container) -> Result<()> {
        let output = Command::new("podman")
            .arg("pause")
            .arg(&container.name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run podman pause: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to pause container: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Resume a container
    pub async fn resume_container(&self, container: &Container) -> Result<()> {
        let output = Command::new("podman")
            .arg("unpause")
            .arg(&container.name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run podman unpause: {}", e))
            })?;

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
        let output = Command::new("podman")
            .arg("inspect")
            .arg("--format")
            .arg("{{.State.Status}}")
            .arg(name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run podman inspect: {}", e))
            })?;

        if !output.status.success() {
            return Err(horcrux_common::Error::ContainerNotFound(name.to_string()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let status = stdout.trim();

        match status {
            "running" => Ok(ContainerStatus::Running),
            "paused" => Ok(ContainerStatus::Paused),
            "exited" | "stopped" => Ok(ContainerStatus::Stopped),
            _ => Ok(ContainerStatus::Unknown),
        }
    }

    /// Execute command in container
    pub async fn exec_command(&self, name: &str, command: &[String]) -> Result<String> {
        let mut cmd = Command::new("podman");
        cmd.arg("exec").arg(name);

        for arg in command {
            cmd.arg(arg);
        }

        let output = cmd.output().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to run podman exec: {}", e))
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
    pub async fn clone_container(&self, source: &str, target: &str, _snapshot: bool) -> Result<()> {
        // Podman doesn't have native clone, so we commit and create new container
        let output = Command::new("podman")
            .arg("commit")
            .arg(source)
            .arg(format!("{}-image", target.to_lowercase()))
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to commit container: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to commit container: {}",
                stderr
            )));
        }

        Ok(())
    }
}
