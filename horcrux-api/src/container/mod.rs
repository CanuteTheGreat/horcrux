///! Container management module
///! Handles LXC, LXD, Incus, Docker, and Podman container lifecycle

pub mod lxc;
pub mod lxd;
pub mod incus;
pub mod docker;
pub mod podman;

use horcrux_common::{ContainerConfig, ContainerRuntime, ContainerStatus, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::db::Database;

/// Container instance (runtime-agnostic)
#[derive(Debug, Clone)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub runtime: ContainerRuntime,
    pub memory: u64,
    pub cpus: u32,
    pub rootfs: String,
    pub status: ContainerStatus,
}

impl Container {
    pub fn to_config(&self) -> ContainerConfig {
        ContainerConfig {
            id: self.id.clone(),
            name: self.name.clone(),
            runtime: self.runtime.clone(),
            memory: self.memory,
            cpus: self.cpus,
            rootfs: self.rootfs.clone(),
            status: self.status.clone(),
        }
    }
}

/// Container manager
pub struct ContainerManager {
    containers: Arc<RwLock<HashMap<String, Container>>>,
    lxc_manager: lxc::LxcManager,
    lxd_manager: lxd::LxdContainerManager,
    incus_manager: incus::IncusContainerManager,
    docker_manager: docker::DockerManager,
    podman_manager: podman::PodmanManager,
    db: Option<Arc<Database>>,
}

impl ContainerManager {
    pub fn new() -> Self {
        Self {
            containers: Arc::new(RwLock::new(HashMap::new())),
            lxc_manager: lxc::LxcManager::new(),
            lxd_manager: lxd::LxdContainerManager::new(),
            incus_manager: incus::IncusContainerManager::new(),
            docker_manager: docker::DockerManager::new(),
            podman_manager: podman::PodmanManager::new(),
            db: None,
        }
    }

    /// Create ContainerManager with database support
    pub fn with_database(db: Arc<Database>) -> Self {
        Self {
            containers: Arc::new(RwLock::new(HashMap::new())),
            lxc_manager: lxc::LxcManager::new(),
            lxd_manager: lxd::LxdContainerManager::new(),
            incus_manager: incus::IncusContainerManager::new(),
            docker_manager: docker::DockerManager::new(),
            podman_manager: podman::PodmanManager::new(),
            db: Some(db),
        }
    }

    /// List all containers
    pub async fn list_containers(&self) -> Vec<ContainerConfig> {
        let containers = self.containers.read().await;
        containers.values().map(|c| c.to_config()).collect()
    }

    /// Get a specific container by ID
    pub async fn get_container(&self, id: &str) -> Result<ContainerConfig> {
        let containers = self.containers.read().await;
        containers
            .get(id)
            .map(|c| c.to_config())
            .ok_or_else(|| horcrux_common::Error::ContainerNotFound(id.to_string()))
    }

    /// Create a new container
    pub async fn create_container(&self, config: ContainerConfig) -> Result<ContainerConfig> {
        let mut containers = self.containers.write().await;

        // Check if container with this ID already exists
        if containers.contains_key(&config.id) {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Container with ID {} already exists",
                config.id
            )));
        }

        // Create the container based on runtime
        let container = match config.runtime {
            ContainerRuntime::Lxc => self.lxc_manager.create_container(&config).await?,
            ContainerRuntime::Lxd => self.lxd_manager.create_container(&config).await?,
            ContainerRuntime::Incus => self.incus_manager.create_container(&config).await?,
            ContainerRuntime::Docker => self.docker_manager.create_container(&config).await?,
            ContainerRuntime::Podman => self.podman_manager.create_container(&config).await?,
        };

        let container_config = container.to_config();
        containers.insert(config.id.clone(), container);
        Ok(container_config)
    }

    /// Start a container
    pub async fn start_container(&self, id: &str) -> Result<ContainerConfig> {
        let containers = self.containers.read().await;
        let container = containers
            .get(id)
            .ok_or_else(|| horcrux_common::Error::ContainerNotFound(id.to_string()))?;

        match container.runtime {
            ContainerRuntime::Lxc => self.lxc_manager.start_container(container).await?,
            ContainerRuntime::Lxd => self.lxd_manager.start_container(container).await?,
            ContainerRuntime::Incus => self.incus_manager.start_container(container).await?,
            ContainerRuntime::Docker => self.docker_manager.start_container(container).await?,
            ContainerRuntime::Podman => self.podman_manager.start_container(container).await?,
        }

        Ok(container.to_config())
    }

    /// Stop a container
    pub async fn stop_container(&self, id: &str) -> Result<ContainerConfig> {
        let containers = self.containers.read().await;
        let container = containers
            .get(id)
            .ok_or_else(|| horcrux_common::Error::ContainerNotFound(id.to_string()))?;

        match container.runtime {
            ContainerRuntime::Lxc => self.lxc_manager.stop_container(container).await?,
            ContainerRuntime::Lxd => self.lxd_manager.stop_container(container).await?,
            ContainerRuntime::Incus => self.incus_manager.stop_container(container).await?,
            ContainerRuntime::Docker => self.docker_manager.stop_container(container).await?,
            ContainerRuntime::Podman => self.podman_manager.stop_container(container).await?,
        }

        Ok(container.to_config())
    }

    /// Delete a container
    pub async fn delete_container(&self, id: &str) -> Result<()> {
        let mut containers = self.containers.write().await;

        if let Some(container) = containers.remove(id) {
            match container.runtime {
                ContainerRuntime::Lxc => self.lxc_manager.delete_container(&container).await?,
                ContainerRuntime::Lxd => self.lxd_manager.delete_container(&container).await?,
                ContainerRuntime::Incus => self.incus_manager.delete_container(&container).await?,
                ContainerRuntime::Docker => self.docker_manager.delete_container(&container).await?,
                ContainerRuntime::Podman => self.podman_manager.delete_container(&container).await?,
            }
            Ok(())
        } else {
            Err(horcrux_common::Error::ContainerNotFound(id.to_string()))
        }
    }
}
