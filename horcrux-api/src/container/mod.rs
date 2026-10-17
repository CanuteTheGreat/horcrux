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
#[cfg(feature = "lxc")]
pub mod lxc_config;
#[cfg(feature = "lxc")]
pub mod lxc_template;
#[cfg(feature = "lxd")]
pub mod lxd;
#[cfg(feature = "podman")]
pub mod podman;
#[cfg(any(feature = "docker", feature = "podman"))]
pub mod stack;
#[cfg(any(feature = "docker", feature = "podman"))]
pub mod volume;
#[cfg(any(
    feature = "docker",
    feature = "podman",
    feature = "lxc",
    feature = "lxd",
    feature = "incus"
))]
pub mod ws;
#[cfg(any(
    feature = "docker",
    feature = "podman",
    feature = "lxc",
    feature = "lxd",
    feature = "incus"
))]
pub mod ws_handlers;

use crate::db::Database;
use horcrux_common::{
    ContainerConfig, ContainerOptions, ContainerRuntime, ContainerStatus, LxcOptions, Result,
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
    /// LXC settings (LXC containers created by Horcrux only).
    pub lxc: Option<LxcOptions>,
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
            lxc: self.lxc.clone(),
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

/// Container ids with a runtime create/delete/clone in flight, plus a log
/// of which ids (and names) were added to / removed from the map, numbered
/// by a counter.
///
/// The reconcilers list the runtime, then fix the map. A create or delete
/// that completes between the listing and the map update makes the listing
/// stale for that container: one created meanwhile is missing from it (and
/// used to be dropped from the map - a failed stack deploy then could not
/// roll back the service it had just started), one deleted meanwhile is
/// still in it (and used to be re-adopted as a ghost). A reconciler leaves
/// every id/name touched since it started listing alone.
#[derive(Default)]
struct InFlight {
    ids: HashSet<String>,
    epoch: u64,
    log: std::collections::VecDeque<(u64, String)>,
}

/// Bounded: a reconcile that started more than this many changes ago
/// treats every entry as touched.
const IN_FLIGHT_LOG: usize = 4096;

impl InFlight {
    fn touch(&mut self, key: &str) {
        self.epoch += 1;
        self.log.push_back((self.epoch, key.to_string()));
        while self.log.len() > IN_FLIGHT_LOG {
            self.log.pop_front();
        }
    }

    #[cfg_attr(
        not(any(
            feature = "docker",
            feature = "lxc",
            feature = "lxd",
            feature = "incus"
        )),
        allow(dead_code)
    )]
    /// Ids/names touched after `since`; None if the log no longer reaches
    /// back that far.
    fn touched_since(&self, since: u64) -> Option<HashSet<String>> {
        if self.epoch == since {
            return Some(HashSet::new());
        }
        match self.log.front() {
            Some((first, _)) if *first <= since + 1 => Some(
                self.log
                    .iter()
                    .filter(|(e, _)| *e > since)
                    .map(|(_, k)| k.clone())
                    .collect(),
            ),
            _ => None,
        }
    }
}

/// What a reconciler must leave alone: ids in flight now, and ids/names
/// whose map membership changed while it was listing (`None` = all).
#[cfg_attr(
    not(any(
        feature = "docker",
        feature = "lxc",
        feature = "lxd",
        feature = "incus"
    )),
    allow(dead_code)
)]
struct InFlightSnapshot {
    ids: HashSet<String>,
    touched: Option<HashSet<String>>,
}

#[cfg_attr(
    not(any(
        feature = "docker",
        feature = "lxc",
        feature = "lxd",
        feature = "incus"
    )),
    allow(dead_code)
)]
impl InFlightSnapshot {
    fn skip(&self, key: &str) -> bool {
        self.ids.contains(key) || self.touched.as_ref().is_none_or(|t| t.contains(key))
    }
}

#[cfg_attr(
    not(any(
        feature = "docker",
        feature = "lxc",
        feature = "lxd",
        feature = "incus"
    )),
    allow(dead_code)
)]
fn in_flight_snapshot(set: &Mutex<InFlight>, since: u64) -> InFlightSnapshot {
    let g = set.lock().unwrap_or_else(|e| e.into_inner());
    InFlightSnapshot {
        ids: g.ids.clone(),
        touched: g.touched_since(since),
    }
}

#[cfg_attr(
    not(any(
        feature = "docker",
        feature = "lxc",
        feature = "lxd",
        feature = "incus"
    )),
    allow(dead_code)
)]
fn in_flight_epoch(set: &Mutex<InFlight>) -> u64 {
    set.lock().unwrap_or_else(|e| e.into_inner()).epoch
}

fn in_flight_touch(set: &Mutex<InFlight>, key: &str) {
    set.lock().unwrap_or_else(|e| e.into_inner()).touch(key);
}

/// Claim on a container id whose runtime create is in flight. Released on
/// drop, so an early return / error path can never leave the id stuck.
struct CreateReservation {
    set: Arc<Mutex<InFlight>>,
    id: String,
}

impl CreateReservation {
    /// Returns None if `id` is already being created.
    fn try_new(set: &Arc<Mutex<InFlight>>, id: &str) -> Option<Self> {
        let mut guard = set.lock().unwrap_or_else(|e| e.into_inner());
        if !guard.ids.insert(id.to_string()) {
            return None;
        }
        guard.touch(id);
        Some(Self {
            set: set.clone(),
            id: id.to_string(),
        })
    }
}

impl Drop for CreateReservation {
    fn drop(&mut self) {
        let mut guard = self.set.lock().unwrap_or_else(|e| e.into_inner());
        guard.ids.remove(&self.id);
        let id = std::mem::take(&mut self.id);
        guard.touch(&id);
    }
}

/// Image source for an LXD/Incus container from the form's image field:
/// `remote:alias` as given, `distro/release` from the public `images:`
/// remote, anything else (a local alias or fingerprint) as given; empty
/// means Ubuntu 22.04.
#[cfg(any(feature = "lxd", feature = "incus"))]
pub fn instance_image(rootfs: &str) -> String {
    let r = rootfs.trim();
    if r.is_empty() {
        "images:ubuntu/22.04".to_string()
    } else if r.contains(':') || !r.contains('/') || r.starts_with('/') {
        r.to_string()
    } else {
        format!("images:{r}")
    }
}

/// Container manager
pub struct ContainerManager {
    containers: Arc<RwLock<HashMap<String, Container>>>,
    /// Ids with a runtime create in flight (see `create_container`).
    creating: Arc<Mutex<InFlight>>,
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
            creating: Arc::new(Mutex::new(InFlight::default())),
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
            creating: Arc::new(Mutex::new(InFlight::default())),
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
        self.reconcile_lxc().await;
        self.reconcile_instances().await;
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
        let listed_at = in_flight_epoch(&self.creating);
        let live = match self.docker_manager.list_containers_api().await {
            Ok(live) => live,
            Err(_) => return,
        };
        // Ids with a create/delete/clone in flight are left alone: the
        // runtime may already (or still) have the container while the map
        // does not (yet/any more), and the in-flight call fixes the map.
        let mut containers = self.containers.write().await;
        let skip = in_flight_snapshot(&self.creating, listed_at);
        let mut seen_names = HashSet::new();
        for c in &live {
            seen_names.insert(c.name.clone());
            let hid = c.horcrux_id.clone().unwrap_or_else(|| c.name.clone());
            if skip.skip(&hid) || skip.skip(&c.name) {
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
                    lxc: None,
                },
            );
        }
        containers.retain(|id, t| {
            t.runtime != ContainerRuntime::Docker
                || seen_names.contains(&t.name)
                || skip.skip(id)
                || skip.skip(&t.name)
        });
    }

    /// Without LXC compiled in there is nothing to reconcile.
    #[cfg(not(feature = "lxc"))]
    pub async fn reconcile_lxc(&self) {}

    /// Bring the LXC part of the map in line with the host's lxcpath, so
    /// LXC containers survive API restarts (their settings live next to
    /// them in horcrux.json) and ones created with lxc-create show up.
    #[cfg(feature = "lxc")]
    pub async fn reconcile_lxc(&self) {
        let listed_at = in_flight_epoch(&self.creating);
        let live = match self.lxc_manager.discover().await {
            Ok(l) => l,
            Err(e) => {
                tracing::debug!("LXC reconcile skipped: {e}");
                return;
            }
        };
        let mut containers = self.containers.write().await;
        let skip = in_flight_snapshot(&self.creating, listed_at);
        let mut seen = HashSet::new();
        for c in live {
            seen.insert(c.name.clone());
            if skip.skip(&c.id) || skip.skip(&c.name) {
                continue;
            }
            if let Some(t) = containers
                .values_mut()
                .find(|t| t.runtime == ContainerRuntime::Lxc && t.name == c.name)
            {
                t.status = c.status.clone();
                if c.lxc.is_some() {
                    t.lxc = c.lxc.clone();
                    t.memory = c.memory;
                    t.cpus = c.cpus;
                }
                continue;
            }
            if containers.contains_key(&c.id) {
                continue;
            }
            containers.insert(c.id.clone(), c);
        }
        containers.retain(|id, t| {
            t.runtime != ContainerRuntime::Lxc
                || seen.contains(&t.name)
                || skip.skip(id)
                || skip.skip(&t.name)
        });
    }

    #[cfg(not(any(feature = "lxd", feature = "incus")))]
    pub async fn reconcile_instances(&self) {}

    /// Bring the LXD/Incus part of the map in line with the daemons, so
    /// their containers survive API restarts (they used to vanish from the
    /// list, taking their Console/Terminal links with them), status changes
    /// made elsewhere show up, and deleted ones disappear. A daemon that
    /// can't be reached leaves its entries untouched.
    #[cfg(any(feature = "lxd", feature = "incus"))]
    pub async fn reconcile_instances(&self) {
        use crate::console::lxd::{Backend, Client};
        #[allow(unused_mut)]
        let mut daemons: Vec<(ContainerRuntime, Backend)> = Vec::new();
        #[cfg(feature = "lxd")]
        daemons.push((ContainerRuntime::Lxd, Backend::Lxd));
        #[cfg(feature = "incus")]
        daemons.push((ContainerRuntime::Incus, Backend::Incus));
        for (runtime, backend) in daemons {
            let client = Client::quick(backend);
            if !client.available() {
                continue;
            }
            let listed_at = in_flight_epoch(&self.creating);
            let live = match client.containers().await {
                Ok(l) => l,
                Err(e) => {
                    tracing::debug!("{} reconcile skipped: {e}", backend.label());
                    continue;
                }
            };
            let mut containers = self.containers.write().await;
            let skip = in_flight_snapshot(&self.creating, listed_at);
            let mut seen = HashSet::new();
            for c in live {
                seen.insert(c.name.clone());
                let status = match c.status.to_ascii_lowercase().as_str() {
                    "running" => ContainerStatus::Running,
                    "stopped" => ContainerStatus::Stopped,
                    "frozen" => ContainerStatus::Paused,
                    _ => ContainerStatus::Unknown,
                };
                if let Some(t) = containers
                    .values_mut()
                    .find(|t| t.runtime == runtime && t.name == c.name)
                {
                    t.status = status;
                    continue;
                }
                if containers.contains_key(&c.name) || skip.skip(&c.name) {
                    continue;
                }
                containers.insert(
                    c.name.clone(),
                    Container {
                        id: c.name.clone(),
                        name: c.name.clone(),
                        runtime: runtime.clone(),
                        memory: c.memory_mb,
                        cpus: c.cpus,
                        rootfs: String::new(),
                        status,
                        options: Default::default(),
                        created_at: c.created_at.clone(),
                        lxc: None,
                    },
                );
            }
            containers.retain(|id, t| {
                t.runtime != runtime
                    || seen.contains(&t.name)
                    || skip.skip(id)
                    || skip.skip(&t.name)
            });
        }
    }

    /// Get a specific container by ID
    pub async fn get_container(&self, id: &str) -> Result<ContainerConfig> {
        self.reconcile_docker().await;
        self.reconcile_lxc().await;
        self.reconcile_instances().await;
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
                if config.lxc.is_some() {
                    return Err(horcrux_common::Error::Validation(
                        "lxc settings are only valid with the lxc runtime".to_string(),
                    ));
                }
                if !is_valid_oci_name(&config.name) {
                    return Err(horcrux_common::Error::Validation(format!(
                        "Invalid container name '{}': use letters, digits, '_', '.' or '-' \
                         (starting with a letter or digit)",
                        config.name
                    )));
                }
            }
            _ => {
                if config.lxc.is_some() && config.runtime != ContainerRuntime::Lxc {
                    return Err(horcrux_common::Error::Validation(
                        "lxc settings are only valid with the lxc runtime".to_string(),
                    ));
                }
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
        // Result-valued match + `?` (not `return` in the fallback arm), so
        // builds without any container runtime have no unreachable code.
        let container: Container = match config.runtime {
            #[cfg(feature = "lxc")]
            ContainerRuntime::Lxc => self.lxc_manager.create_container(&config).await,
            #[cfg(feature = "lxd")]
            ContainerRuntime::Lxd => self.lxd_manager.create_container(&config).await,
            #[cfg(feature = "incus")]
            ContainerRuntime::Incus => self.incus_manager.create_container(&config).await,
            #[cfg(feature = "docker")]
            ContainerRuntime::Docker => self.docker_manager.create_container(&config).await,
            #[cfg(feature = "podman")]
            ContainerRuntime::Podman => self.podman_manager.create_container(&config).await,
            #[allow(unreachable_patterns)]
            _ => Err(crate::features::runtime_not_compiled(&config.runtime)),
        }?;

        let container_config = container.to_config();
        let mut containers = self.containers.write().await;
        in_flight_touch(&self.creating, &config.name);
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
            lxc: config.lxc.clone(),
        };
        let out = container.to_config();
        containers.insert(config.id.clone(), container);
        in_flight_touch(&self.creating, &config.id);
        in_flight_touch(&self.creating, &config.name);
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

        // Result-valued match + `?` (not `return` in the fallback arm), so
        // builds without any container runtime have no unreachable code.
        match container.runtime {
            #[cfg(feature = "lxc")]
            ContainerRuntime::Lxc => self.lxc_manager.start_container(&container).await,
            #[cfg(feature = "lxd")]
            ContainerRuntime::Lxd => self.lxd_manager.start_container(&container).await,
            #[cfg(feature = "incus")]
            ContainerRuntime::Incus => self.incus_manager.start_container(&container).await,
            #[cfg(feature = "docker")]
            ContainerRuntime::Docker => self.docker_manager.start_container(&container).await,
            #[cfg(feature = "podman")]
            ContainerRuntime::Podman => self.podman_manager.start_container(&container).await,
            #[allow(unreachable_patterns)]
            _ => Err(crate::features::runtime_not_compiled(&container.runtime)),
        }?;

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

        // Result-valued match + `?` (not `return` in the fallback arm), so
        // builds without any container runtime have no unreachable code.
        match container.runtime {
            #[cfg(feature = "lxc")]
            ContainerRuntime::Lxc => self.lxc_manager.stop_container(&container).await,
            #[cfg(feature = "lxd")]
            ContainerRuntime::Lxd => self.lxd_manager.stop_container(&container).await,
            #[cfg(feature = "incus")]
            ContainerRuntime::Incus => self.incus_manager.stop_container(&container).await,
            #[cfg(feature = "docker")]
            ContainerRuntime::Docker => self.docker_manager.stop_container(&container).await,
            #[cfg(feature = "podman")]
            ContainerRuntime::Podman => self.podman_manager.stop_container(&container).await,
            #[allow(unreachable_patterns)]
            _ => Err(crate::features::runtime_not_compiled(&container.runtime)),
        }?;

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
            in_flight_touch(&self.creating, &container.name);
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
        // Its name is gone from (or back in) the runtime now: a reconcile
        // that listed before this point must not act on it.
        in_flight_touch(&self.creating, &container.name);

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

        // Result-valued match + `?` (not `return` in the fallback arm), so
        // builds without any container runtime have no unreachable code.
        match container.runtime {
            #[cfg(feature = "lxc")]
            ContainerRuntime::Lxc => self.lxc_manager.pause_container(&container).await,
            #[cfg(feature = "lxd")]
            ContainerRuntime::Lxd => self.lxd_manager.pause_container(&container).await,
            #[cfg(feature = "incus")]
            ContainerRuntime::Incus => self.incus_manager.pause_container(&container).await,
            #[cfg(feature = "docker")]
            ContainerRuntime::Docker => self.docker_manager.pause_container(&container).await,
            #[cfg(feature = "podman")]
            ContainerRuntime::Podman => self.podman_manager.pause_container(&container).await,
            #[allow(unreachable_patterns)]
            _ => Err(crate::features::runtime_not_compiled(&container.runtime)),
        }?;

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

        // Result-valued match + `?` (not `return` in the fallback arm), so
        // builds without any container runtime have no unreachable code.
        match container.runtime {
            #[cfg(feature = "lxc")]
            ContainerRuntime::Lxc => self.lxc_manager.resume_container(&container).await,
            #[cfg(feature = "lxd")]
            ContainerRuntime::Lxd => self.lxd_manager.resume_container(&container).await,
            #[cfg(feature = "incus")]
            ContainerRuntime::Incus => self.incus_manager.resume_container(&container).await,
            #[cfg(feature = "docker")]
            ContainerRuntime::Docker => self.docker_manager.resume_container(&container).await,
            #[cfg(feature = "podman")]
            ContainerRuntime::Podman => self.podman_manager.resume_container(&container).await,
            #[allow(unreachable_patterns)]
            _ => Err(crate::features::runtime_not_compiled(&container.runtime)),
        }?;

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

        // Result-valued match + `?` (not `return` in the fallback arm), so
        // builds without any container runtime have no unreachable code.
        let status: ContainerStatus = match container.runtime {
            #[cfg(feature = "lxc")]
            ContainerRuntime::Lxc => self.lxc_manager.get_container_status(&container.name).await,
            #[cfg(feature = "lxd")]
            ContainerRuntime::Lxd => self.lxd_manager.get_container_status(&container.name).await,
            #[cfg(feature = "incus")]
            ContainerRuntime::Incus => {
                self.incus_manager
                    .get_container_status(&container.name)
                    .await
            }
            #[cfg(feature = "docker")]
            ContainerRuntime::Docker => {
                self.docker_manager
                    .get_container_status(&container.name)
                    .await
            }
            #[cfg(feature = "podman")]
            ContainerRuntime::Podman => {
                self.podman_manager
                    .get_container_status(&container.name)
                    .await
            }
            #[allow(unreachable_patterns)]
            _ => Err(crate::features::runtime_not_compiled(&container.runtime)),
        }?;

        Ok(status)
    }

    /// Execute command in container
    // Without any container runtime compiled in, every arm is the
    // "not compiled in" error and the arguments go unused.
    #[cfg_attr(
        not(any(
            feature = "lxc",
            feature = "lxd",
            feature = "incus",
            feature = "docker",
            feature = "podman"
        )),
        allow(unused_variables)
    )]
    pub async fn exec_command(&self, id: &str, command: Vec<String>) -> Result<String> {
        let containers = self.containers.read().await;
        let container = containers
            .get(id)
            .ok_or_else(|| horcrux_common::Error::ContainerNotFound(id.to_string()))?;

        // Result-valued match + `?` (not `return` in the fallback arm), so
        // builds without any container runtime have no unreachable code.
        let output: String = match container.runtime {
            #[cfg(feature = "lxc")]
            ContainerRuntime::Lxc => {
                self.lxc_manager
                    .exec_command(&container.name, &command)
                    .await
            }
            #[cfg(feature = "lxd")]
            ContainerRuntime::Lxd => {
                self.lxd_manager
                    .exec_command(&container.name, &command)
                    .await
            }
            #[cfg(feature = "incus")]
            ContainerRuntime::Incus => {
                self.incus_manager
                    .exec_command(&container.name, &command)
                    .await
            }
            #[cfg(feature = "docker")]
            ContainerRuntime::Docker => {
                self.docker_manager
                    .exec_command(&container.name, &command)
                    .await
            }
            #[cfg(feature = "podman")]
            ContainerRuntime::Podman => {
                self.podman_manager
                    .exec_command(&container.name, &command)
                    .await
            }
            #[allow(unreachable_patterns)]
            _ => Err(crate::features::runtime_not_compiled(&container.runtime)),
        }?;

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
    // Without any container runtime compiled in, every arm is the
    // "not compiled in" error and the arguments go unused.
    #[cfg_attr(
        not(any(
            feature = "lxc",
            feature = "lxd",
            feature = "incus",
            feature = "docker",
            feature = "podman"
        )),
        allow(unused_variables)
    )]
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
            #[cfg(feature = "lxc")]
            ContainerRuntime::Lxc => {
                self.lxc_manager
                    .clone_container(&source_container.name, target_id, target_name, snapshot)
                    .await?
            }
            #[allow(unreachable_patterns)]
            ContainerRuntime::Lxc | ContainerRuntime::Lxd | ContainerRuntime::Incus => {
                // A Result rather than `return` in the fallback arm: with
                // neither lxd nor incus compiled in, the code below must not
                // become unreachable (USE-flag builds use -D warnings).
                let cloned: Result<()> = match source_container.runtime {
                    #[cfg(feature = "lxd")]
                    ContainerRuntime::Lxd => {
                        self.lxd_manager
                            .clone_container(&source_container.name, target_name, snapshot)
                            .await
                    }
                    #[cfg(feature = "incus")]
                    ContainerRuntime::Incus => {
                        self.incus_manager
                            .clone_container(&source_container.name, target_name, snapshot)
                            .await
                    }
                    #[allow(unreachable_patterns)]
                    _ => Err(crate::features::runtime_not_compiled(
                        &source_container.runtime,
                    )),
                };
                cloned?;
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
                    lxc: None,
                }
            }
            ContainerRuntime::Docker | ContainerRuntime::Podman => {
                let cloned: Result<()> = match source_container.runtime {
                    #[cfg(feature = "docker")]
                    ContainerRuntime::Docker => {
                        self.docker_manager
                            .clone_container(&source_container.name, target_name, snapshot)
                            .await
                    }
                    #[cfg(feature = "podman")]
                    ContainerRuntime::Podman => {
                        self.podman_manager
                            .clone_container(&source_container.name, target_name, snapshot)
                            .await
                    }
                    #[allow(unreachable_patterns)]
                    _ => Err(crate::features::runtime_not_compiled(
                        &source_container.runtime,
                    )),
                };
                cloned?;
                #[cfg_attr(
                    not(any(feature = "docker", feature = "podman")),
                    allow(unused_variables)
                )]
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
                    lxc: None,
                };
                match source_container.runtime {
                    #[cfg(feature = "docker")]
                    ContainerRuntime::Docker => self.docker_manager.create_container(&config).await,
                    #[cfg(feature = "podman")]
                    ContainerRuntime::Podman => self.podman_manager.create_container(&config).await,
                    #[allow(unreachable_patterns)]
                    _ => Err(crate::features::runtime_not_compiled(
                        &source_container.runtime,
                    )),
                }?
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

/// LXC-only operations (pct shutdown/reboot/set/resize/snapshot).
#[cfg(feature = "lxc")]
impl ContainerManager {
    /// The LXC backend (templates, host info, web terminal argv).
    pub fn lxc(&self) -> &lxc::LxcManager {
        &self.lxc_manager
    }

    /// Look up an LXC container by id (after reconciling with the host).
    pub async fn lxc_container(&self, id: &str) -> Result<Container> {
        self.reconcile_lxc().await;
        let c = self
            .containers
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| horcrux_common::Error::ContainerNotFound(id.to_string()))?;
        if c.runtime != ContainerRuntime::Lxc {
            return Err(horcrux_common::Error::Validation(format!(
                "container {id} uses the {:?} runtime; this operation is for LXC containers",
                c.runtime
            )));
        }
        Ok(c)
    }

    async fn set_status(&self, id: &str, status: ContainerStatus) -> Option<ContainerConfig> {
        let mut containers = self.containers.write().await;
        containers.get_mut(id).map(|c| {
            c.status = status;
            c.to_config()
        })
    }

    async fn refresh_lxc_entry(&self, id: &str, meta: &lxc::LxcMeta) {
        if let Some(c) = self.containers.write().await.get_mut(id) {
            c.memory = meta.memory;
            c.cpus = meta.cpus;
            c.lxc = Some(meta.lxc.clone());
        }
    }

    /// Clean shutdown (`pct shutdown`).
    pub async fn shutdown_container(
        &self,
        id: &str,
        timeout: u32,
        force: bool,
    ) -> Result<ContainerConfig> {
        let c = self.lxc_container(id).await?;
        self.lxc_manager
            .shutdown_container(&c.name, timeout, force)
            .await?;
        Ok(self
            .set_status(id, ContainerStatus::Stopped)
            .await
            .unwrap_or_else(|| c.to_config()))
    }

    /// Reboot (`pct reboot`), applying pending configuration.
    pub async fn reboot_container(&self, id: &str, timeout: u32) -> Result<ContainerConfig> {
        let c = self.lxc_container(id).await?;
        self.lxc_manager.reboot_container(&c, timeout).await?;
        Ok(self
            .set_status(id, ContainerStatus::Running)
            .await
            .unwrap_or_else(|| c.to_config()))
    }

    pub async fn lxc_config(&self, id: &str) -> Result<lxc::LxcConfigView> {
        let c = self.lxc_container(id).await?;
        let meta = self.lxc_manager.read_meta(&c.name)?;
        Ok(self.lxc_manager.config_view(&meta))
    }

    /// Change settings (`pct set`).
    pub async fn update_lxc_config(
        &self,
        id: &str,
        memory: Option<u64>,
        cpus: Option<u32>,
        opts: LxcOptions,
    ) -> Result<lxc::LxcConfigView> {
        let c = self.lxc_container(id).await?;
        let meta = self
            .lxc_manager
            .update_config(&c.name, memory, cpus, opts)
            .await?;
        self.refresh_lxc_entry(id, &meta).await;
        Ok(self.lxc_manager.config_view(&meta))
    }

    /// Grow the root disk (`pct resize`).
    pub async fn resize_container(&self, id: &str, disk_gb: u64) -> Result<lxc::LxcConfigView> {
        let c = self.lxc_container(id).await?;
        let meta = self.lxc_manager.resize(&c.name, disk_gb).await?;
        self.refresh_lxc_entry(id, &meta).await;
        Ok(self.lxc_manager.config_view(&meta))
    }

    pub async fn list_snapshots(&self, id: &str) -> Result<Vec<horcrux_common::LxcSnapshot>> {
        let c = self.lxc_container(id).await?;
        self.lxc_manager.list_snapshots(&c.name).await
    }

    pub async fn create_snapshot(
        &self,
        id: &str,
        name: &str,
        comment: Option<String>,
    ) -> Result<horcrux_common::LxcSnapshot> {
        let c = self.lxc_container(id).await?;
        self.lxc_manager
            .create_snapshot(&c.name, name, comment)
            .await
    }

    pub async fn rollback_snapshot(&self, id: &str, name: &str) -> Result<ContainerConfig> {
        let c = self.lxc_container(id).await?;
        self.lxc_manager.rollback_snapshot(&c, name).await?;
        let meta = self.lxc_manager.read_meta(&c.name)?;
        self.refresh_lxc_entry(id, &meta).await;
        let status = self
            .lxc_manager
            .get_container_status(&c.name)
            .await
            .unwrap_or(ContainerStatus::Unknown);
        Ok(self
            .set_status(id, status)
            .await
            .unwrap_or_else(|| c.to_config()))
    }

    pub async fn delete_snapshot(&self, id: &str, name: &str) -> Result<()> {
        let c = self.lxc_container(id).await?;
        self.lxc_manager.delete_snapshot(&c.name, name).await
    }
}

#[cfg(all(test, any(feature = "lxd", feature = "incus")))]
mod instance_image_tests {
    #[test]
    fn image_sources() {
        assert_eq!(super::instance_image("alpine/3.20"), "images:alpine/3.20");
        assert_eq!(
            super::instance_image("images:debian/12"),
            "images:debian/12"
        );
        assert_eq!(super::instance_image("my-local"), "my-local");
        assert_eq!(super::instance_image(" "), "images:ubuntu/22.04");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_reservation_is_exclusive_and_released_on_drop() {
        let set = Arc::new(Mutex::new(InFlight::default()));
        let first = CreateReservation::try_new(&set, "ct-1").expect("first claim");
        assert!(CreateReservation::try_new(&set, "ct-1").is_none());
        // Other ids are independent.
        let other = CreateReservation::try_new(&set, "ct-2").expect("other id");
        drop(first);
        assert!(CreateReservation::try_new(&set, "ct-1").is_some());
        drop(other);
        assert!(set.lock().unwrap().ids.is_empty());
    }

    #[test]
    fn in_flight_log_reports_ids_touched_since_a_listing() {
        let set = Arc::new(Mutex::new(InFlight::default()));
        let before = in_flight_epoch(&set);
        // Nothing happened: nothing to skip.
        assert!(!in_flight_snapshot(&set, before).skip("a"));
        // A create that started and finished while a reconcile was listing.
        drop(CreateReservation::try_new(&set, "stack_good").unwrap());
        in_flight_touch(&set, "stack_good_name");
        let snap = in_flight_snapshot(&set, before);
        assert!(snap.skip("stack_good"), "created during the listing");
        assert!(snap.skip("stack_good_name"));
        assert!(!snap.skip("other"), "untouched ids are reconciled");
        // A later reconcile starts after the create: nothing to skip.
        let after = in_flight_epoch(&set);
        assert!(!in_flight_snapshot(&set, after).skip("stack_good"));
        // A listing older than the bounded log skips everything.
        for i in 0..(IN_FLIGHT_LOG + 5) {
            in_flight_touch(&set, &format!("c{i}"));
        }
        assert!(in_flight_snapshot(&set, after).skip("anything"));
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
