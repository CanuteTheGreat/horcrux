pub mod cnetwork;
pub mod docker;
pub mod image;
pub mod incus;
/// Container management module
/// Handles LXC, LXD, Incus, Docker, and Podman container lifecycle
pub mod lxc;
pub mod lxd;
pub mod podman;
pub mod stack;
pub mod volume;
pub mod ws;
pub mod ws_handlers;

use crate::db::Database;
use horcrux_common::{ContainerConfig, ContainerRuntime, ContainerStatus, Result};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;

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

/// Claim on a container id whose runtime create is in flight. Released on
/// drop, so an early return / error path can never leave the id stuck.
struct CreateReservation {
    set: Arc<Mutex<HashSet<String>>>,
    id: String,
}

impl CreateReservation {
    /// Returns None if `id` is already being created.
    fn try_new(set: &Arc<Mutex<HashSet<String>>>, id: &str) -> Option<Self> {
        let mut guard = set.lock().unwrap_or_else(|e| e.into_inner());
        if !guard.insert(id.to_string()) {
            return None;
        }
        Some(Self {
            set: set.clone(),
            id: id.to_string(),
        })
    }
}

impl Drop for CreateReservation {
    fn drop(&mut self) {
        self.set
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}

/// Container manager
pub struct ContainerManager {
    containers: Arc<RwLock<HashMap<String, Container>>>,
    /// Ids with a runtime create in flight (see `create_container`).
    creating: Arc<Mutex<HashSet<String>>>,
    lxc_manager: lxc::LxcManager,
    lxd_manager: lxd::LxdContainerManager,
    incus_manager: incus::IncusContainerManager,
    docker_manager: docker::DockerManager,
    podman_manager: podman::PodmanManager,
    _db: Option<Arc<Database>>, // Reserved for future persistent container storage
}

impl Default for ContainerManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ContainerManager {
    pub fn new() -> Self {
        Self {
            containers: Arc::new(RwLock::new(HashMap::new())),
            creating: Arc::new(Mutex::new(HashSet::new())),
            lxc_manager: lxc::LxcManager::new(),
            lxd_manager: lxd::LxdContainerManager::new(),
            incus_manager: incus::IncusContainerManager::new(),
            docker_manager: docker::DockerManager::new(),
            podman_manager: podman::PodmanManager::new(),
            _db: None,
        }
    }

    /// Create ContainerManager with database support
    pub fn with_database(db: Arc<Database>) -> Self {
        Self {
            containers: Arc::new(RwLock::new(HashMap::new())),
            creating: Arc::new(Mutex::new(HashSet::new())),
            lxc_manager: lxc::LxcManager::new(),
            lxd_manager: lxd::LxdContainerManager::new(),
            incus_manager: incus::IncusContainerManager::new(),
            docker_manager: docker::DockerManager::new(),
            podman_manager: podman::PodmanManager::new(),
            _db: Some(db),
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
        // Reserve the id under the map lock, then run the runtime call
        // (`docker create`, `lxc-create`, ...) WITHOUT holding it. This used
        // to hold the global write lock across the runtime call, which
        // serialized every container create/delete/list/get on the host
        // behind one slow CLI invocation - under concurrent load requests
        // queued past client timeouts (nightly stress test, 2026-10-16).
        let reservation = {
            let containers = self.containers.read().await;
            if containers.contains_key(&config.id) {
                return Err(horcrux_common::Error::InvalidConfig(format!(
                    "Container with ID {} already exists",
                    config.id
                )));
            }
            CreateReservation::try_new(&self.creating, &config.id).ok_or_else(|| {
                horcrux_common::Error::InvalidConfig(format!(
                    "Container with ID {} is already being created",
                    config.id
                ))
            })?
        };

        // Create the container based on runtime
        let container = match config.runtime {
            ContainerRuntime::Lxc => self.lxc_manager.create_container(&config).await?,
            ContainerRuntime::Lxd => self.lxd_manager.create_container(&config).await?,
            ContainerRuntime::Incus => self.incus_manager.create_container(&config).await?,
            ContainerRuntime::Docker => self.docker_manager.create_container(&config).await?,
            ContainerRuntime::Podman => self.podman_manager.create_container(&config).await?,
        };

        let container_config = container.to_config();
        let mut containers = self.containers.write().await;
        containers.insert(config.id.clone(), container);
        // Release the reservation while still holding the write lock, so a
        // concurrent create of the same id (which checks map + reservation
        // under the read lock) can never see neither.
        drop(reservation);
        Ok(container_config)
    }

    /// Start a container
    pub async fn start_container(&self, id: &str) -> Result<ContainerConfig> {
        // Clone the container and drop the read lock before calling out to
        // the runtime (which can take a while), then take a short write
        // lock just to flip status. Previously this never updated the
        // stored status at all: every subsequent call that checks
        // container.status (e.g. DockerManager::stop_container's "already
        // stopped" guard) kept seeing the pre-start Stopped value forever,
        // so a container could be started but could then never be stopped
        // again through this API - the stop call would always fail with
        // "Container is already stopped" even while genuinely running.
        let container = {
            let containers = self.containers.read().await;
            containers
                .get(id)
                .cloned()
                .ok_or_else(|| horcrux_common::Error::ContainerNotFound(id.to_string()))?
        };

        match container.runtime {
            ContainerRuntime::Lxc => self.lxc_manager.start_container(&container).await?,
            ContainerRuntime::Lxd => self.lxd_manager.start_container(&container).await?,
            ContainerRuntime::Incus => self.incus_manager.start_container(&container).await?,
            ContainerRuntime::Docker => self.docker_manager.start_container(&container).await?,
            ContainerRuntime::Podman => self.podman_manager.start_container(&container).await?,
        }

        let mut containers = self.containers.write().await;
        if let Some(stored) = containers.get_mut(id) {
            stored.status = ContainerStatus::Running;
        }
        Ok(ContainerConfig {
            status: ContainerStatus::Running,
            ..container.to_config()
        })
    }

    /// Stop a container
    pub async fn stop_container(&self, id: &str) -> Result<ContainerConfig> {
        let container = {
            let containers = self.containers.read().await;
            containers
                .get(id)
                .cloned()
                .ok_or_else(|| horcrux_common::Error::ContainerNotFound(id.to_string()))?
        };

        match container.runtime {
            ContainerRuntime::Lxc => self.lxc_manager.stop_container(&container).await?,
            ContainerRuntime::Lxd => self.lxd_manager.stop_container(&container).await?,
            ContainerRuntime::Incus => self.incus_manager.stop_container(&container).await?,
            ContainerRuntime::Docker => self.docker_manager.stop_container(&container).await?,
            ContainerRuntime::Podman => self.podman_manager.stop_container(&container).await?,
        }

        let mut containers = self.containers.write().await;
        if let Some(stored) = containers.get_mut(id) {
            stored.status = ContainerStatus::Stopped;
        }
        Ok(ContainerConfig {
            status: ContainerStatus::Stopped,
            ..container.to_config()
        })
    }

    /// Delete a container
    pub async fn delete_container(&self, id: &str) -> Result<()> {
        // Claim the entry under a short lock (a concurrent delete of the same
        // id gets NotFound), then call the runtime without holding the lock.
        let container = self
            .containers
            .write()
            .await
            .remove(id)
            .ok_or_else(|| horcrux_common::Error::ContainerNotFound(id.to_string()))?;

        let result = match container.runtime {
            ContainerRuntime::Lxc => self.lxc_manager.delete_container(&container).await,
            ContainerRuntime::Lxd => self.lxd_manager.delete_container(&container).await,
            ContainerRuntime::Incus => self.incus_manager.delete_container(&container).await,
            ContainerRuntime::Docker => self.docker_manager.delete_container(&container).await,
            ContainerRuntime::Podman => self.podman_manager.delete_container(&container).await,
        };

        if let Err(e) = result {
            // The runtime still has it: keep tracking it instead of silently
            // forgetting a container that exists (previously it was dropped
            // from the map before the runtime call, success or not).
            self.containers
                .write()
                .await
                .entry(id.to_string())
                .or_insert(container);
            return Err(e);
        }
        Ok(())
    }

    /// Pause a container
    pub async fn pause_container(&self, id: &str) -> Result<ContainerConfig> {
        let container = {
            let containers = self.containers.read().await;
            containers
                .get(id)
                .cloned()
                .ok_or_else(|| horcrux_common::Error::ContainerNotFound(id.to_string()))?
        };

        match container.runtime {
            ContainerRuntime::Lxc => self.lxc_manager.pause_container(&container).await?,
            ContainerRuntime::Lxd => self.lxd_manager.pause_container(&container).await?,
            ContainerRuntime::Incus => self.incus_manager.pause_container(&container).await?,
            ContainerRuntime::Docker => self.docker_manager.pause_container(&container).await?,
            ContainerRuntime::Podman => self.podman_manager.pause_container(&container).await?,
        }

        let mut containers = self.containers.write().await;
        if let Some(stored) = containers.get_mut(id) {
            stored.status = ContainerStatus::Paused;
        }
        Ok(ContainerConfig {
            status: ContainerStatus::Paused,
            ..container.to_config()
        })
    }

    /// Resume a container
    pub async fn resume_container(&self, id: &str) -> Result<ContainerConfig> {
        let container = {
            let containers = self.containers.read().await;
            containers
                .get(id)
                .cloned()
                .ok_or_else(|| horcrux_common::Error::ContainerNotFound(id.to_string()))?
        };

        match container.runtime {
            ContainerRuntime::Lxc => self.lxc_manager.resume_container(&container).await?,
            ContainerRuntime::Lxd => self.lxd_manager.resume_container(&container).await?,
            ContainerRuntime::Incus => self.incus_manager.resume_container(&container).await?,
            ContainerRuntime::Docker => self.docker_manager.resume_container(&container).await?,
            ContainerRuntime::Podman => self.podman_manager.resume_container(&container).await?,
        }

        let mut containers = self.containers.write().await;
        if let Some(stored) = containers.get_mut(id) {
            stored.status = ContainerStatus::Running;
        }
        Ok(ContainerConfig {
            status: ContainerStatus::Running,
            ..container.to_config()
        })
    }

    /// Get container status
    pub async fn get_container_status(&self, id: &str) -> Result<ContainerStatus> {
        let containers = self.containers.read().await;
        let container = containers
            .get(id)
            .ok_or_else(|| horcrux_common::Error::ContainerNotFound(id.to_string()))?;

        let status = match container.runtime {
            ContainerRuntime::Lxc => {
                self.lxc_manager
                    .get_container_status(&container.name)
                    .await?
            }
            ContainerRuntime::Lxd => {
                self.lxd_manager
                    .get_container_status(&container.name)
                    .await?
            }
            ContainerRuntime::Incus => {
                self.incus_manager
                    .get_container_status(&container.name)
                    .await?
            }
            ContainerRuntime::Docker => {
                self.docker_manager
                    .get_container_status(&container.name)
                    .await?
            }
            ContainerRuntime::Podman => {
                self.podman_manager
                    .get_container_status(&container.name)
                    .await?
            }
        };

        Ok(status)
    }

    /// Execute command in container
    pub async fn exec_command(&self, id: &str, command: Vec<String>) -> Result<String> {
        let containers = self.containers.read().await;
        let container = containers
            .get(id)
            .ok_or_else(|| horcrux_common::Error::ContainerNotFound(id.to_string()))?;

        let output = match container.runtime {
            ContainerRuntime::Lxc => {
                self.lxc_manager
                    .exec_command(&container.name, &command)
                    .await?
            }
            ContainerRuntime::Lxd => {
                self.lxd_manager
                    .exec_command(&container.name, &command)
                    .await?
            }
            ContainerRuntime::Incus => {
                self.incus_manager
                    .exec_command(&container.name, &command)
                    .await?
            }
            ContainerRuntime::Docker => {
                self.docker_manager
                    .exec_command(&container.name, &command)
                    .await?
            }
            ContainerRuntime::Podman => {
                self.podman_manager
                    .exec_command(&container.name, &command)
                    .await?
            }
        };

        Ok(output)
    }

    /// Clone a container
    pub async fn clone_container(
        &self,
        source_id: &str,
        target_id: &str,
        target_name: &str,
        snapshot: bool,
    ) -> Result<ContainerConfig> {
        let containers = self.containers.read().await;
        let source_container = containers
            .get(source_id)
            .ok_or_else(|| horcrux_common::Error::ContainerNotFound(source_id.to_string()))?;

        match source_container.runtime {
            ContainerRuntime::Lxc => {
                self.lxc_manager
                    .clone_container(&source_container.name, target_name, snapshot)
                    .await?;
            }
            ContainerRuntime::Lxd => {
                self.lxd_manager
                    .clone_container(&source_container.name, target_name, snapshot)
                    .await?;
            }
            ContainerRuntime::Incus => {
                self.incus_manager
                    .clone_container(&source_container.name, target_name, snapshot)
                    .await?;
            }
            ContainerRuntime::Docker => {
                self.docker_manager
                    .clone_container(&source_container.name, target_name, snapshot)
                    .await?;
            }
            ContainerRuntime::Podman => {
                self.podman_manager
                    .clone_container(&source_container.name, target_name, snapshot)
                    .await?;
            }
        }

        // Create a new container config for the clone
        let cloned_config = ContainerConfig {
            id: target_id.to_string(),
            name: target_name.to_string(),
            runtime: source_container.runtime.clone(),
            memory: source_container.memory,
            cpus: source_container.cpus,
            rootfs: format!(
                "{}/{}",
                source_container
                    .rootfs
                    .rsplit('/')
                    .nth(1)
                    .unwrap_or("/var/lib"),
                target_name
            ),
            status: ContainerStatus::Stopped,
        };

        Ok(cloned_config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_reservation_is_exclusive_and_released_on_drop() {
        let set = Arc::new(Mutex::new(HashSet::new()));
        let first = CreateReservation::try_new(&set, "ct-1").expect("first claim");
        assert!(CreateReservation::try_new(&set, "ct-1").is_none());
        // Other ids are independent.
        let other = CreateReservation::try_new(&set, "ct-2").expect("other id");
        drop(first);
        assert!(CreateReservation::try_new(&set, "ct-1").is_some());
        drop(other);
        assert!(set.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn delete_unknown_container_is_not_found() {
        let mgr = ContainerManager::new();
        match mgr.delete_container("does-not-exist").await {
            Err(horcrux_common::Error::ContainerNotFound(id)) => assert_eq!(id, "does-not-exist"),
            other => panic!("expected ContainerNotFound, got {other:?}"),
        }
    }
}
