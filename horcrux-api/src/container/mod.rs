#[cfg(any(feature = "docker", feature = "podman"))]
pub mod cnetwork;
#[cfg(feature = "docker")]
pub mod docker;
#[cfg(any(feature = "docker", feature = "podman"))]
pub mod image;
#[cfg(feature = "incus")]
pub mod incus;
/// Container management module
/// Handles LXC, LXD, Incus, Docker, and Podman container lifecycle
#[cfg(feature = "lxc")]
pub mod lxc;
#[cfg(feature = "lxd")]
pub mod lxd;
#[cfg(feature = "podman")]
pub mod podman;
#[cfg(any(feature = "docker", feature = "podman"))]
pub mod stack;
#[cfg(any(feature = "docker", feature = "podman"))]
pub mod volume;
#[cfg(any(feature = "docker", feature = "podman"))]
pub mod ws;
#[cfg(any(feature = "docker", feature = "podman"))]
pub mod ws_handlers;

use crate::db::Database;
use horcrux_common::{
    ContainerConfig, ContainerOptions, ContainerRuntime, ContainerStatus, Result,
};
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
    /// Docker/Podman creation options the container was created with.
    pub options: ContainerOptions,
    /// RFC 3339 creation time (set by Horcrux on create, or taken from
    /// the runtime for containers discovered on the host).
    pub created_at: Option<String>,
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
            options: self.options.clone(),
            created_at: self.created_at.clone(),
        }
    }
}

/// Label Horcrux puts on every Docker/Podman container it creates, holding
/// the Horcrux container id (the runtime only knows the container name).
/// Shared by docker.rs and podman.rs, either of which may be compiled out.
pub const HORCRUX_ID_LABEL: &str = "horcrux.id";

/// `docker/podman create` flags for the user-supplied options (everything
/// that goes before the image argument; `command` goes after it).
pub(crate) fn oci_option_args(opts: &ContainerOptions) -> Vec<String> {
    let mut args = Vec::new();
    for e in &opts.env {
        args.push("--env".to_string());
        args.push(e.clone());
    }
    for p in &opts.ports {
        args.push("--publish".to_string());
        args.push(p.clone());
    }
    for v in &opts.volumes {
        args.push("--volume".to_string());
        args.push(v.clone());
    }
    if let Some(net) = &opts.network {
        args.push("--network".to_string());
        args.push(net.clone());
        for a in &opts.network_aliases {
            args.push("--network-alias".to_string());
            args.push(a.clone());
        }
    }
    args
}

/// Docker/Podman container names: `[a-zA-Z0-9][a-zA-Z0-9_.-]*`.
pub(crate) fn is_valid_oci_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
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
    #[cfg(feature = "lxc")]
    lxc_manager: lxc::LxcManager,
    #[cfg(feature = "lxd")]
    lxd_manager: lxd::LxdContainerManager,
    #[cfg(feature = "incus")]
    incus_manager: incus::IncusContainerManager,
    #[cfg(feature = "docker")]
    docker_manager: docker::DockerManager,
    #[cfg(feature = "podman")]
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
            #[cfg(feature = "lxc")]
            lxc_manager: lxc::LxcManager::new(),
            #[cfg(feature = "lxd")]
            lxd_manager: lxd::LxdContainerManager::new(),
            #[cfg(feature = "incus")]
            incus_manager: incus::IncusContainerManager::new(),
            #[cfg(feature = "docker")]
            docker_manager: docker::DockerManager::new(),
            #[cfg(feature = "podman")]
            podman_manager: podman::PodmanManager::new(),
            _db: None,
        }
    }

    /// Create ContainerManager with database support
    pub fn with_database(db: Arc<Database>) -> Self {
        Self {
            containers: Arc::new(RwLock::new(HashMap::new())),
            creating: Arc::new(Mutex::new(HashSet::new())),
            #[cfg(feature = "lxc")]
            lxc_manager: lxc::LxcManager::new(),
            #[cfg(feature = "lxd")]
            lxd_manager: lxd::LxdContainerManager::new(),
            #[cfg(feature = "incus")]
            incus_manager: incus::IncusContainerManager::new(),
            #[cfg(feature = "docker")]
            docker_manager: docker::DockerManager::new(),
            #[cfg(feature = "podman")]
            podman_manager: podman::PodmanManager::new(),
            _db: Some(db),
        }
    }

    /// List all containers.
    ///
    /// Docker containers are reconciled against the daemon first, so the
    /// list shows what really exists: status changes made outside Horcrux
    /// (`docker stop`, a crash, an OOM kill) are picked up, containers
    /// removed out-of-band disappear, and containers that exist on the
    /// host but were never created through this manager (started with the
    /// docker CLI, or created before an API restart - the map is in-memory)
    /// are adopted so they can be managed from the UI.
    pub async fn list_containers(&self) -> Vec<ContainerConfig> {
        self.reconcile_docker().await;
        let containers = self.containers.read().await;
        let mut list: Vec<ContainerConfig> = containers.values().map(|c| c.to_config()).collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        list
    }

    /// Bring the Docker part of the in-memory map in line with the daemon.
    /// A no-op when the Docker API is unreachable (no daemon on this host).
    #[cfg(not(feature = "docker"))]
    pub async fn reconcile_docker(&self) {
        // USE=-docker: no Docker backend compiled in, nothing to reconcile.
    }

    /// Bring the Docker part of the in-memory map in line with the daemon.
    /// A no-op when the Docker API is unreachable (no daemon on this host).
    #[cfg(feature = "docker")]
    pub async fn reconcile_docker(&self) {
        let live = match self.docker_manager.list_containers_api().await {
            Ok(live) => live,
            Err(_) => return,
        };
        // Ids with a create/delete/clone in flight are left alone: the
        // runtime may already (or still) have the container while the map
        // does not (yet/any more), and the in-flight call fixes the map.
        let in_flight: HashSet<String> = self
            .creating
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();

        let mut containers = self.containers.write().await;
        let mut seen_names = HashSet::new();
        for c in &live {
            seen_names.insert(c.name.clone());
            let hid = c.horcrux_id.clone().unwrap_or_else(|| c.name.clone());
            if in_flight.contains(&hid) {
                continue;
            }
            if let Some(tracked) = containers
                .values_mut()
                .find(|t| t.runtime == ContainerRuntime::Docker && t.name == c.name)
            {
                tracked.status = c.status.clone();
                if tracked.created_at.is_none() {
                    tracked.created_at = c.created_at.clone();
                }
                continue;
            }
            if containers.contains_key(&hid) {
                // Same id tracked under another runtime/name: don't clobber it.
                continue;
            }
            containers.insert(
                hid.clone(),
                Container {
                    id: hid,
                    name: c.name.clone(),
                    runtime: ContainerRuntime::Docker,
                    memory: 0,
                    cpus: 0,
                    rootfs: c.image.clone(),
                    status: c.status.clone(),
                    options: ContainerOptions::default(),
                    created_at: c.created_at.clone(),
                },
            );
        }
        containers.retain(|id, t| {
            t.runtime != ContainerRuntime::Docker
                || seen_names.contains(&t.name)
                || in_flight.contains(id)
        });
    }

    /// Get a specific container by ID
    pub async fn get_container(&self, id: &str) -> Result<ContainerConfig> {
        self.reconcile_docker().await;
        let containers = self.containers.read().await;
        containers
            .get(id)
            .map(|c| c.to_config())
            .ok_or_else(|| horcrux_common::Error::ContainerNotFound(id.to_string()))
    }

    /// Create a new container
    pub async fn create_container(&self, config: ContainerConfig) -> Result<ContainerConfig> {
        if config.id.trim().is_empty() || config.name.trim().is_empty() {
            return Err(horcrux_common::Error::Validation(
                "Container id and name are required".to_string(),
            ));
        }
        if config.rootfs.trim().is_empty() {
            return Err(horcrux_common::Error::Validation(
                "Container image/rootfs is required".to_string(),
            ));
        }
        config
            .options
            .validate()
            .map_err(horcrux_common::Error::Validation)?;
        match config.runtime {
            ContainerRuntime::Docker | ContainerRuntime::Podman => {
                if !is_valid_oci_name(&config.name) {
                    return Err(horcrux_common::Error::Validation(format!(
                        "Invalid container name '{}': use letters, digits, '_', '.' or '-' \
                         (starting with a letter or digit)",
                        config.name
                    )));
                }
            }
            _ => {
                if !config.options.is_empty() {
                    return Err(horcrux_common::Error::Validation(
                        "env/ports/volumes/command/network are only supported for the \
                         docker and podman runtimes"
                            .to_string(),
                    ));
                }
            }
        }
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
        let container: Container = match config.runtime {
            #[cfg(feature = "lxc")]
            ContainerRuntime::Lxc => self.lxc_manager.create_container(&config).await?,
            #[cfg(feature = "lxd")]
            ContainerRuntime::Lxd => self.lxd_manager.create_container(&config).await?,
            #[cfg(feature = "incus")]
            ContainerRuntime::Incus => self.incus_manager.create_container(&config).await?,
            #[cfg(feature = "docker")]
            ContainerRuntime::Docker => self.docker_manager.create_container(&config).await?,
            #[cfg(feature = "podman")]
            ContainerRuntime::Podman => self.podman_manager.create_container(&config).await?,
            #[allow(unreachable_patterns)]
            _ => return Err(crate::features::runtime_not_compiled(&config.runtime)),
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

    /// Track a container that already exists in its runtime without
    /// creating it (e.g. an LXC/LXD/Incus container just restored from a
    /// backup).
    pub async fn register_existing(&self, config: ContainerConfig) -> Result<ContainerConfig> {
        let mut containers = self.containers.write().await;
        if containers.contains_key(&config.id) {
            return Err(horcrux_common::Error::Conflict(format!(
                "Container with ID {} already exists",
                config.id
            )));
        }
        let container = Container {
            id: config.id.clone(),
            name: config.name.clone(),
            runtime: config.runtime.clone(),
            memory: config.memory,
            cpus: config.cpus,
            rootfs: config.rootfs.clone(),
            status: config.status.clone(),
            options: config.options.clone(),
            created_at: config.created_at.clone(),
        };
        let out = container.to_config();
        containers.insert(config.id.clone(), container);
        Ok(out)
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
            #[cfg(feature = "lxc")]
            ContainerRuntime::Lxc => self.lxc_manager.start_container(&container).await?,
            #[cfg(feature = "lxd")]
            ContainerRuntime::Lxd => self.lxd_manager.start_container(&container).await?,
            #[cfg(feature = "incus")]
            ContainerRuntime::Incus => self.incus_manager.start_container(&container).await?,
            #[cfg(feature = "docker")]
            ContainerRuntime::Docker => self.docker_manager.start_container(&container).await?,
            #[cfg(feature = "podman")]
            ContainerRuntime::Podman => self.podman_manager.start_container(&container).await?,
            #[allow(unreachable_patterns)]
            _ => return Err(crate::features::runtime_not_compiled(&container.runtime)),
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
            #[cfg(feature = "lxc")]
            ContainerRuntime::Lxc => self.lxc_manager.stop_container(&container).await?,
            #[cfg(feature = "lxd")]
            ContainerRuntime::Lxd => self.lxd_manager.stop_container(&container).await?,
            #[cfg(feature = "incus")]
            ContainerRuntime::Incus => self.incus_manager.stop_container(&container).await?,
            #[cfg(feature = "docker")]
            ContainerRuntime::Docker => self.docker_manager.stop_container(&container).await?,
            #[cfg(feature = "podman")]
            ContainerRuntime::Podman => self.podman_manager.stop_container(&container).await?,
            #[allow(unreachable_patterns)]
            _ => return Err(crate::features::runtime_not_compiled(&container.runtime)),
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
        let (container, _in_flight) = {
            let mut containers = self.containers.write().await;
            let container = containers
                .remove(id)
                .ok_or_else(|| horcrux_common::Error::ContainerNotFound(id.to_string()))?;
            // Mark the id in flight (under the map lock) so a concurrent
            // reconcile doesn't re-adopt the container while `docker rm`
            // is still running.
            let in_flight = CreateReservation::try_new(&self.creating, id);
            (container, in_flight)
        };

        let result: Result<()> = match container.runtime {
            #[cfg(feature = "lxc")]
            ContainerRuntime::Lxc => self.lxc_manager.delete_container(&container).await,
            #[cfg(feature = "lxd")]
            ContainerRuntime::Lxd => self.lxd_manager.delete_container(&container).await,
            #[cfg(feature = "incus")]
            ContainerRuntime::Incus => self.incus_manager.delete_container(&container).await,
            #[cfg(feature = "docker")]
            ContainerRuntime::Docker => self.docker_manager.delete_container(&container).await,
            #[cfg(feature = "podman")]
            ContainerRuntime::Podman => self.podman_manager.delete_container(&container).await,
            #[allow(unreachable_patterns)]
            _ => Err(crate::features::runtime_not_compiled(&container.runtime)),
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
            #[cfg(feature = "lxc")]
            ContainerRuntime::Lxc => self.lxc_manager.pause_container(&container).await?,
            #[cfg(feature = "lxd")]
            ContainerRuntime::Lxd => self.lxd_manager.pause_container(&container).await?,
            #[cfg(feature = "incus")]
            ContainerRuntime::Incus => self.incus_manager.pause_container(&container).await?,
            #[cfg(feature = "docker")]
            ContainerRuntime::Docker => self.docker_manager.pause_container(&container).await?,
            #[cfg(feature = "podman")]
            ContainerRuntime::Podman => self.podman_manager.pause_container(&container).await?,
            #[allow(unreachable_patterns)]
            _ => return Err(crate::features::runtime_not_compiled(&container.runtime)),
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
            #[cfg(feature = "lxc")]
            ContainerRuntime::Lxc => self.lxc_manager.resume_container(&container).await?,
            #[cfg(feature = "lxd")]
            ContainerRuntime::Lxd => self.lxd_manager.resume_container(&container).await?,
            #[cfg(feature = "incus")]
            ContainerRuntime::Incus => self.incus_manager.resume_container(&container).await?,
            #[cfg(feature = "docker")]
            ContainerRuntime::Docker => self.docker_manager.resume_container(&container).await?,
            #[cfg(feature = "podman")]
            ContainerRuntime::Podman => self.podman_manager.resume_container(&container).await?,
            #[allow(unreachable_patterns)]
            _ => return Err(crate::features::runtime_not_compiled(&container.runtime)),
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

        let status: ContainerStatus = match container.runtime {
            #[cfg(feature = "lxc")]
            ContainerRuntime::Lxc => {
                self.lxc_manager
                    .get_container_status(&container.name)
                    .await?
            }
            #[cfg(feature = "lxd")]
            ContainerRuntime::Lxd => {
                self.lxd_manager
                    .get_container_status(&container.name)
                    .await?
            }
            #[cfg(feature = "incus")]
            ContainerRuntime::Incus => {
                self.incus_manager
                    .get_container_status(&container.name)
                    .await?
            }
            #[cfg(feature = "docker")]
            ContainerRuntime::Docker => {
                self.docker_manager
                    .get_container_status(&container.name)
                    .await?
            }
            #[cfg(feature = "podman")]
            ContainerRuntime::Podman => {
                self.podman_manager
                    .get_container_status(&container.name)
                    .await?
            }
            #[allow(unreachable_patterns)]
            _ => return Err(crate::features::runtime_not_compiled(&container.runtime)),
        };

        Ok(status)
    }

    /// Execute command in container
    pub async fn exec_command(&self, id: &str, command: Vec<String>) -> Result<String> {
        let containers = self.containers.read().await;
        let container = containers
            .get(id)
            .ok_or_else(|| horcrux_common::Error::ContainerNotFound(id.to_string()))?;

        let output: String = match container.runtime {
            #[cfg(feature = "lxc")]
            ContainerRuntime::Lxc => {
                self.lxc_manager
                    .exec_command(&container.name, &command)
                    .await?
            }
            #[cfg(feature = "lxd")]
            ContainerRuntime::Lxd => {
                self.lxd_manager
                    .exec_command(&container.name, &command)
                    .await?
            }
            #[cfg(feature = "incus")]
            ContainerRuntime::Incus => {
                self.incus_manager
                    .exec_command(&container.name, &command)
                    .await?
            }
            #[cfg(feature = "docker")]
            ContainerRuntime::Docker => {
                self.docker_manager
                    .exec_command(&container.name, &command)
                    .await?
            }
            #[cfg(feature = "podman")]
            ContainerRuntime::Podman => {
                self.podman_manager
                    .exec_command(&container.name, &command)
                    .await?
            }
            #[allow(unreachable_patterns)]
            _ => return Err(crate::features::runtime_not_compiled(&container.runtime)),
        };

        Ok(output)
    }

    /// Clone a container.
    ///
    /// The clone is registered with the manager like any created container,
    /// so it shows up in the list and can be started/stopped/deleted. For
    /// Docker/Podman the runtime "clone" is `commit` to a `<target>-image`
    /// image, after which a real container is created from that image (it
    /// used to stop at the commit, so the "clone" never existed as a
    /// container and nothing appeared in the UI).
    pub async fn clone_container(
        &self,
        source_id: &str,
        target_id: &str,
        target_name: &str,
        snapshot: bool,
    ) -> Result<ContainerConfig> {
        let source_container = {
            let containers = self.containers.read().await;
            if containers.contains_key(target_id) {
                return Err(horcrux_common::Error::InvalidConfig(format!(
                    "Container with ID {} already exists",
                    target_id
                )));
            }
            containers
                .get(source_id)
                .cloned()
                .ok_or_else(|| horcrux_common::Error::ContainerNotFound(source_id.to_string()))?
        };
        let _reservation =
            CreateReservation::try_new(&self.creating, target_id).ok_or_else(|| {
                horcrux_common::Error::InvalidConfig(format!(
                    "Container with ID {} is already being created",
                    target_id
                ))
            })?;

        crate::features::require_container_runtime(&source_container.runtime)?;
        let clone: Container = match source_container.runtime {
            ContainerRuntime::Lxc | ContainerRuntime::Lxd | ContainerRuntime::Incus => {
                match source_container.runtime {
                    #[cfg(feature = "lxc")]
                    ContainerRuntime::Lxc => {
                        self.lxc_manager
                            .clone_container(&source_container.name, target_name, snapshot)
                            .await?
                    }
                    #[cfg(feature = "lxd")]
                    ContainerRuntime::Lxd => {
                        self.lxd_manager
                            .clone_container(&source_container.name, target_name, snapshot)
                            .await?
                    }
                    #[cfg(feature = "incus")]
                    ContainerRuntime::Incus => {
                        self.incus_manager
                            .clone_container(&source_container.name, target_name, snapshot)
                            .await?
                    }
                    #[allow(unreachable_patterns)]
                    _ => {
                        return Err(crate::features::runtime_not_compiled(
                            &source_container.runtime,
                        ))
                    }
                }
                Container {
                    id: target_id.to_string(),
                    name: target_name.to_string(),
                    runtime: source_container.runtime.clone(),
                    memory: source_container.memory,
                    cpus: source_container.cpus,
                    rootfs: format!(
                        "{}/{}",
                        source_container
                            .rootfs
                            .rsplit_once('/')
                            .map(|(dir, _)| dir)
                            .unwrap_or("/var/lib"),
                        target_name
                    ),
                    status: ContainerStatus::Stopped,
                    options: source_container.options.clone(),
                    created_at: Some(chrono::Utc::now().to_rfc3339()),
                }
            }
            ContainerRuntime::Docker | ContainerRuntime::Podman => {
                match source_container.runtime {
                    #[cfg(feature = "docker")]
                    ContainerRuntime::Docker => {
                        self.docker_manager
                            .clone_container(&source_container.name, target_name, snapshot)
                            .await?;
                    }
                    #[cfg(feature = "podman")]
                    ContainerRuntime::Podman => {
                        self.podman_manager
                            .clone_container(&source_container.name, target_name, snapshot)
                            .await?;
                    }
                    #[allow(unreachable_patterns)]
                    _ => {
                        return Err(crate::features::runtime_not_compiled(
                            &source_container.runtime,
                        ))
                    }
                }
                let config = ContainerConfig {
                    id: target_id.to_string(),
                    name: target_name.to_string(),
                    runtime: source_container.runtime.clone(),
                    memory: source_container.memory,
                    cpus: source_container.cpus,
                    rootfs: format!("{}-image", target_name.to_lowercase()),
                    status: ContainerStatus::Stopped,
                    // Published host ports would collide with the source.
                    options: ContainerOptions {
                        ports: Vec::new(),
                        ..source_container.options.clone()
                    },
                    created_at: None,
                };
                match source_container.runtime {
                    #[cfg(feature = "docker")]
                    ContainerRuntime::Docker => {
                        self.docker_manager.create_container(&config).await?
                    }
                    #[cfg(feature = "podman")]
                    ContainerRuntime::Podman => {
                        self.podman_manager.create_container(&config).await?
                    }
                    #[allow(unreachable_patterns)]
                    _ => {
                        return Err(crate::features::runtime_not_compiled(
                            &source_container.runtime,
                        ))
                    }
                }
            }
        };

        let cfg = clone.to_config();
        self.containers
            .write()
            .await
            .insert(target_id.to_string(), clone);
        Ok(cfg)
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

    #[test]
    fn oci_option_args_maps_every_option_to_runtime_flags() {
        let opts = ContainerOptions {
            env: vec!["A=1".into()],
            ports: vec!["8080:80".into()],
            volumes: vec!["data:/data:ro".into()],
            command: vec!["sleep".into(), "60".into()],
            network: Some("app_default".into()),
            network_aliases: vec!["web".into()],
        };
        assert_eq!(
            oci_option_args(&opts),
            vec![
                "--env",
                "A=1",
                "--publish",
                "8080:80",
                "--volume",
                "data:/data:ro",
                "--network",
                "app_default",
                "--network-alias",
                "web",
            ]
        );
        // command is NOT a flag: it goes after the image argument.
        assert!(!oci_option_args(&opts).contains(&"sleep".to_string()));
        assert!(oci_option_args(&ContainerOptions::default()).is_empty());
    }

    #[test]
    fn oci_names_follow_docker_rules() {
        assert!(is_valid_oci_name("web-1"));
        assert!(is_valid_oci_name("stack_web.2"));
        assert!(!is_valid_oci_name(""));
        assert!(!is_valid_oci_name("-rm"));
        assert!(!is_valid_oci_name("Test Container"));
    }

    #[tokio::test]
    async fn create_rejects_invalid_input_before_touching_a_runtime() {
        let mgr = ContainerManager::new();
        let base = ContainerConfig {
            id: "ct-x".into(),
            name: "ct-x".into(),
            runtime: ContainerRuntime::Docker,
            memory: 64,
            cpus: 1,
            rootfs: "busybox:latest".into(),
            ..Default::default()
        };
        let cases = vec![
            ContainerConfig {
                rootfs: String::new(),
                ..base.clone()
            },
            ContainerConfig {
                name: "bad name".into(),
                ..base.clone()
            },
            ContainerConfig {
                options: ContainerOptions {
                    ports: vec!["eighty".into()],
                    ..Default::default()
                },
                ..base.clone()
            },
            ContainerConfig {
                runtime: ContainerRuntime::Lxc,
                options: ContainerOptions {
                    env: vec!["A=1".into()],
                    ..Default::default()
                },
                ..base.clone()
            },
        ];
        for c in cases {
            match mgr.create_container(c.clone()).await {
                Err(horcrux_common::Error::Validation(_)) => {}
                other => panic!("expected Validation error for {c:?}, got {other:?}"),
            }
        }
    }
}
