///! Docker container integration

use super::Container;
use bollard::Docker;
use bollard::container::{ListContainersOptions, StatsOptions};
use bollard::models::ContainerStateStatusEnum;
use horcrux_common::{ContainerConfig, ContainerRuntime, ContainerStatus, Result};
use std::sync::Arc;
use tokio::process::Command;
use tracing::{debug, error, info, warn};

/// Docker container manager
pub struct DockerManager {
    /// Docker API client (optional - falls back to CLI if unavailable)
    docker: Option<Arc<Docker>>,
}

impl DockerManager {
    pub fn new() -> Self {
        // Try to connect to Docker API
        let docker = match Docker::connect_with_local_defaults() {
            Ok(docker) => {
                info!("Docker API client initialized successfully");
                Some(Arc::new(docker))
            }
            Err(e) => {
                warn!("Failed to connect to Docker API: {}. Falling back to CLI.", e);
                None
            }
        };

        Self { docker }
    }

    /// Get Docker API client if available
    fn get_docker_client(&self) -> Option<&Docker> {
        self.docker.as_ref().map(|d| d.as_ref())
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

    /// Pause a container
    pub async fn pause_container(&self, container: &Container) -> Result<()> {
        let output = Command::new("docker")
            .arg("pause")
            .arg(&container.name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run docker pause: {}", e))
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
        let output = Command::new("docker")
            .arg("unpause")
            .arg(&container.name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run docker unpause: {}", e))
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
        let output = Command::new("docker")
            .arg("inspect")
            .arg("--format")
            .arg("{{.State.Status}}")
            .arg(name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run docker inspect: {}", e))
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
        let mut cmd = Command::new("docker");
        cmd.arg("exec").arg(name);

        for arg in command {
            cmd.arg(arg);
        }

        let output = cmd.output().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to run docker exec: {}", e))
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
        // Docker doesn't have native clone, so we commit and create new container
        let output = Command::new("docker")
            .arg("commit")
            .arg(source)
            .arg(format!("{}-image", target))
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

    /// List all containers using Docker API
    pub async fn list_containers_api(&self) -> Result<Vec<(String, String, ContainerStatus)>> {
        let docker = self.get_docker_client().ok_or_else(|| {
            horcrux_common::Error::System("Docker API not available".to_string())
        })?;

        let options = Some(ListContainersOptions::<String> {
            all: true,
            ..Default::default()
        });

        let containers = docker
            .list_containers(options)
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to list containers: {}", e)))?;

        let mut result = Vec::new();
        for container in containers {
            let id = container.id.unwrap_or_default();
            let name = container
                .names
                .and_then(|names| names.first().map(|n| n.trim_start_matches('/').to_string()))
                .unwrap_or_else(|| id.clone());

            let status = match container.state.as_deref() {
                Some("running") => ContainerStatus::Running,
                Some("paused") => ContainerStatus::Paused,
                Some("exited") | Some("stopped") => ContainerStatus::Stopped,
                _ => ContainerStatus::Unknown,
            };

            result.push((id, name, status));
        }

        debug!("Listed {} containers via Docker API", result.len());
        Ok(result)
    }

    /// Get container statistics using Docker API
    pub async fn get_container_stats_api(&self, container_id: &str) -> Result<DockerContainerStats> {
        let docker = self.get_docker_client().ok_or_else(|| {
            horcrux_common::Error::System("Docker API not available".to_string())
        })?;

        let stats_options = StatsOptions {
            stream: false,
            one_shot: true,
        };

        let mut stats_stream = docker.stats(container_id, Some(stats_options));

        use futures::StreamExt;
        if let Some(stats_result) = stats_stream.next().await {
            let stats = stats_result.map_err(|e| {
                horcrux_common::Error::System(format!("Failed to get container stats: {}", e))
            })?;

            // Parse CPU stats
            let cpu_delta = stats.cpu_stats.cpu_usage.total_usage
                - stats.precpu_stats.cpu_usage.total_usage;
            let system_delta = stats.cpu_stats.system_cpu_usage.unwrap_or(0)
                - stats.precpu_stats.system_cpu_usage.unwrap_or(0);
            let num_cpus = stats.cpu_stats.online_cpus.unwrap_or(1) as f64;

            let cpu_percent = if system_delta > 0 {
                (cpu_delta as f64 / system_delta as f64) * num_cpus * 100.0
            } else {
                0.0
            };

            // Parse memory stats
            let memory_usage = stats.memory_stats.usage.unwrap_or(0);
            let memory_limit = stats.memory_stats.limit.unwrap_or(0);

            // Parse network stats
            let mut network_rx_bytes = 0u64;
            let mut network_tx_bytes = 0u64;

            if let Some(networks) = stats.networks {
                for (_interface, net_stats) in networks {
                    network_rx_bytes += net_stats.rx_bytes;
                    network_tx_bytes += net_stats.tx_bytes;
                }
            }

            // Parse block I/O stats
            let mut block_read_bytes = 0u64;
            let mut block_write_bytes = 0u64;

            if let Some(blkio_stats) = stats.blkio_stats.io_service_bytes_recursive {
                for entry in blkio_stats {
                    match entry.op.as_str() {
                        "Read" => block_read_bytes += entry.value,
                        "Write" => block_write_bytes += entry.value,
                        _ => {}
                    }
                }
            }

            debug!(
                "Container {} stats: CPU: {:.2}%, Memory: {} / {} bytes",
                container_id, cpu_percent, memory_usage, memory_limit
            );

            return Ok(DockerContainerStats {
                cpu_usage_percent: cpu_percent,
                memory_usage_bytes: memory_usage,
                memory_limit_bytes: memory_limit,
                network_rx_bytes,
                network_tx_bytes,
                block_read_bytes,
                block_write_bytes,
            });
        }

        Err(horcrux_common::Error::System(format!(
            "No stats available for container {}",
            container_id
        )))
    }

    /// Get container info using Docker API
    pub async fn inspect_container_api(&self, container_id: &str) -> Result<DockerContainerInfo> {
        let docker = self.get_docker_client().ok_or_else(|| {
            horcrux_common::Error::System("Docker API not available".to_string())
        })?;

        let container = docker
            .inspect_container(container_id, None)
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to inspect container: {}", e)))?;

        let status = match container.state.and_then(|s| s.status) {
            Some(ContainerStateStatusEnum::RUNNING) => ContainerStatus::Running,
            Some(ContainerStateStatusEnum::PAUSED) => ContainerStatus::Paused,
            Some(ContainerStateStatusEnum::EXITED) | Some(ContainerStateStatusEnum::DEAD) => {
                ContainerStatus::Stopped
            }
            _ => ContainerStatus::Unknown,
        };

        Ok(DockerContainerInfo {
            id: container.id.unwrap_or_default(),
            name: container.name.unwrap_or_default().trim_start_matches('/').to_string(),
            status,
            image: container.config.and_then(|c| c.image).unwrap_or_default(),
        })
    }
}

/// Docker container statistics
#[derive(Debug, Clone)]
pub struct DockerContainerStats {
    pub cpu_usage_percent: f64,
    pub memory_usage_bytes: u64,
    pub memory_limit_bytes: u64,
    pub network_rx_bytes: u64,
    pub network_tx_bytes: u64,
    pub block_read_bytes: u64,
    pub block_write_bytes: u64,
}

/// Docker container information
#[derive(Debug, Clone)]
pub struct DockerContainerInfo {
    pub id: String,
    pub name: String,
    pub status: ContainerStatus,
    pub image: String,
}
