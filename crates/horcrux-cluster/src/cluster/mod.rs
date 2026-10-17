pub mod affinity;
pub mod arch;
pub mod balancer;
/// Clustering support module
/// Provides multi-node cluster management, HA, and VM migration using Corosync
pub mod corosync;
pub mod membership;
pub mod migration;
pub mod node;
pub mod state_sync;
pub mod usage;

use crate::db::config_store::{self, kinds, ConfigStore};
use crate::db::Database;
use horcrux_common::Result;
pub use node::{Node, NodeStatus};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
pub use usage::NodeUsage;

/// Cluster configuration
#[allow(dead_code)]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ClusterConfig {
    pub name: String,
    pub nodes: Vec<Node>,
    pub quorum_votes: u32,
}

/// Cluster manager
pub struct ClusterManager {
    _config: Arc<RwLock<Option<ClusterConfig>>>, // Reserved for runtime cluster config updates
    nodes: Arc<RwLock<HashMap<String, Node>>>,
    corosync: corosync::CorosyncManager,
    local_node_name: Arc<RwLock<Option<String>>>,
    /// This host's live usage sampler and its latest sample (served at
    /// `GET /api/cluster/local-usage` for the other nodes' usage monitors).
    usage_sampler: Arc<usage::LocalUsageSampler>,
    local_usage: Arc<RwLock<Option<NodeUsage>>>,
    /// Durable copy of the node list and cluster config; `None` = in-memory.
    store: Option<ConfigStore>,
}

/// Id of the single persisted cluster config document.
const CLUSTER_CONFIG_ID: &str = "local";

#[allow(dead_code)]
impl Default for ClusterManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ClusterManager {
    pub fn new() -> Self {
        Self {
            _config: Arc::new(RwLock::new(None)),
            nodes: Arc::new(RwLock::new(HashMap::new())),
            corosync: corosync::CorosyncManager::new(),
            local_node_name: Arc::new(RwLock::new(None)),
            usage_sampler: Arc::new(usage::LocalUsageSampler::new()),
            local_usage: Arc::new(RwLock::new(None)),
            store: None,
        }
    }

    /// Cluster manager backed by the database: loads the persisted node list
    /// (and cluster config, if a cluster was created/joined) and writes every
    /// change through. Live fields (status, CPU/memory usage) are reset to
    /// unknown on load; the usage monitor refills them from the nodes.
    pub async fn with_database(db: Arc<Database>) -> Result<Self> {
        let store = ConfigStore::new(&db);
        let mut mgr = Self::new();
        {
            let mut nodes = mgr.nodes.write().await;
            for mut node in store.load_values::<Node>(kinds::CLUSTER_NODE).await? {
                node.status = NodeStatus::Unknown;
                node.cpu_usage = None;
                node.memory_used = None;
                nodes.insert(node.name.clone(), node);
            }
            if let Some(cfg) = store
                .load_values::<ClusterConfig>(kinds::CLUSTER_CONFIG)
                .await?
                .into_iter()
                .next()
            {
                *mgr._config.write().await = Some(cfg);
            }
            tracing::info!("Loaded {} cluster node(s) from database", nodes.len());
        }
        mgr.store = Some(store);
        Ok(mgr)
    }

    async fn persist_config(&self, config: &ClusterConfig) -> Result<()> {
        config_store::put_opt(
            &self.store,
            kinds::CLUSTER_CONFIG,
            CLUSTER_CONFIG_ID,
            config,
        )
        .await
    }

    /// This host's latest live usage sample, taking one now if the usage
    /// monitor hasn't produced one yet. `None` if procfs is unreadable.
    pub async fn local_usage(&self) -> Option<NodeUsage> {
        if let Some(sample) = self.local_usage.read().await.clone() {
            return Some(sample);
        }
        let sample = self.usage_sampler.sample().await;
        if let Some(ref s) = sample {
            *self.local_usage.write().await = Some(s.clone());
        }
        sample
    }

    /// Start the background usage monitor: every [`usage::POLL_INTERVAL`]
    /// it samples this host, then polls every registered node for its live
    /// CPU/memory usage (this host directly, the rest over HTTP on
    /// `api_port`) and records the result -- including online/offline
    /// status -- on the node. This is the cluster's node heartbeat.
    pub fn start_usage_monitor(self: &Arc<Self>, api_port: u16) {
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            // Peers serve HTTPS with their own node-local CA, which this node
            // doesn't hold, so the channel is encrypted but the peer isn't
            // authenticated by certificate; the request only carries a 30 s,
            // read-only, single-route service token (see middleware::auth).
            let client = match reqwest::Client::builder()
                .connect_timeout(usage::FETCH_TIMEOUT)
                .timeout(usage::FETCH_TIMEOUT)
                .danger_accept_invalid_certs(true)
                .build()
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("Node usage monitor disabled: HTTP client: {}", e);
                    return;
                }
            };
            let mut ticker = tokio::time::interval(usage::POLL_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            tracing::info!(
                "Starting cluster node usage monitor (interval: {}s)",
                usage::POLL_INTERVAL.as_secs()
            );
            loop {
                ticker.tick().await;
                manager.poll_node_usage_once(&client, api_port).await;
            }
        });
    }

    /// One usage-monitor round (see [`Self::start_usage_monitor`]).
    pub async fn poll_node_usage_once(&self, client: &reqwest::Client, api_port: u16) {
        let local_sample = self.usage_sampler.sample().await;
        *self.local_usage.write().await = local_sample.clone();

        let targets: Vec<Node> = self.nodes.read().await.values().cloned().collect();
        if targets.is_empty() {
            return;
        }

        let local_hostname = self.get_local_node_name().await.unwrap_or_default();
        // Short-lived service token for the other nodes' local-usage
        // endpoint. Nodes of one cluster share JWT_SECRET; a node that
        // doesn't will answer 401, which is reported as "up, usage unknown".
        // The token is route-scoped (GET /api/cluster/local-usage only, see
        // middleware::auth) and expires in seconds, since it is sent to every
        // registered peer address.
        let token = crate::middleware::auth::generate_cluster_node_token(&local_hostname).ok();

        let polls = targets.iter().map(|node| {
            let local_sample = local_sample.clone();
            let token = token.clone();
            let local_hostname = local_hostname.clone();
            async move {
                let outcome = if usage::is_this_host(node, &local_hostname) {
                    match local_sample {
                        Some(sample) => usage::PollOutcome::Usage(sample),
                        // We're obviously up; we just can't read procfs.
                        None => usage::PollOutcome::ReachableNoUsage,
                    }
                } else {
                    usage::fetch_remote_usage(client, &node.api_url(api_port), token.as_deref())
                        .await
                };
                (node.name.clone(), outcome)
            }
        });
        let results = futures::future::join_all(polls).await;

        let now = chrono::Utc::now().timestamp();
        let mut nodes = self.nodes.write().await;
        for (name, outcome) in results {
            // The node may have been removed while we were polling.
            if let Some(node) = nodes.get_mut(&name) {
                let was = node.status.clone();
                usage::apply_poll(node, outcome, now);
                if node.status != was {
                    tracing::info!("Cluster node {} is now {:?}", name, node.status);
                }
            }
        }
    }

    /// Cluster-wide summary for `GET /api/cluster/status`. Works whether or
    /// not a Corosync cluster has been initialised: `name` is the Corosync
    /// cluster name when there is one, and `has_quorum` is `None` when
    /// quorum can't be determined (corosync-quorumtool not installed).
    pub async fn status_report(&self, total_vms: usize) -> ClusterStatusReport {
        let name = self._config.read().await.as_ref().map(|c| c.name.clone());
        let nodes = self.list_nodes().await;
        let has_quorum = self.corosync.check_quorum().await.ok();
        ClusterStatusReport::from_nodes(name, &nodes, has_quorum, total_vms)
    }

    /// Get the local node name
    pub async fn get_local_node_name(&self) -> Result<String> {
        let local_name = self.local_node_name.read().await;
        if let Some(name) = local_name.as_ref() {
            return Ok(name.clone());
        }
        drop(local_name);

        // Try to get from hostname
        let hostname = hostname::get()
            .map_err(|e| horcrux_common::Error::System(format!("Failed to get hostname: {}", e)))?
            .to_string_lossy()
            .to_string();

        // Cache it
        let mut local_name = self.local_node_name.write().await;
        *local_name = Some(hostname.clone());

        Ok(hostname)
    }

    /// Initialize a new cluster
    #[allow(dead_code)]
    pub async fn create_cluster(&self, cluster_name: String, node_name: String) -> Result<()> {
        let mut config = self._config.write().await;

        if config.is_some() {
            return Err(horcrux_common::Error::InvalidConfig(
                "Cluster already exists".to_string(),
            ));
        }

        // Create initial node with auto-detected architecture
        let node = Node::new_local(1, node_name.clone(), "127.0.0.1".to_string());

        // Initialize Corosync
        self.corosync.init_cluster(&cluster_name, &node).await?;

        let mut nodes = HashMap::new();
        nodes.insert(node_name.clone(), node.clone());

        let new_config = ClusterConfig {
            name: cluster_name,
            nodes: vec![node.clone()],
            quorum_votes: 1,
        };
        self.persist_config(&new_config).await?;
        *config = Some(new_config);

        let mut nodes_map = self.nodes.write().await;
        if let Some(store) = &self.store {
            for old in nodes_map.keys() {
                store.delete(kinds::CLUSTER_NODE, old).await?;
            }
            store.put(kinds::CLUSTER_NODE, &node.name, &node).await?;
        }
        *nodes_map = nodes;

        Ok(())
    }

    /// Join an existing cluster
    #[allow(dead_code)]
    pub async fn join_cluster(
        &self,
        cluster_name: String,
        node_name: String,
        master_ip: String,
    ) -> Result<()> {
        let config = self._config.read().await;

        if config.is_some() {
            return Err(horcrux_common::Error::InvalidConfig(
                "Already part of a cluster".to_string(),
            ));
        }

        drop(config);

        // Join via Corosync
        self.corosync
            .join_cluster(&cluster_name, &node_name, &master_ip)
            .await?;

        // Fetch cluster configuration from master
        // (In a real implementation, this would be an API call to the master node)
        let mut config = self._config.write().await;
        let new_config = ClusterConfig {
            name: cluster_name,
            nodes: vec![],
            quorum_votes: 1,
        };
        self.persist_config(&new_config).await?;
        *config = Some(new_config);

        Ok(())
    }

    /// Add a node to the cluster
    pub async fn add_node(&self, node: Node) -> Result<()> {
        let mut nodes = self.nodes.write().await;

        if nodes.contains_key(&node.name) {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Node {} already exists in cluster",
                node.name
            )));
        }

        // Add to Corosync
        self.corosync.add_node(&node).await?;

        config_store::put_opt(&self.store, kinds::CLUSTER_NODE, &node.name, &node).await?;
        nodes.insert(node.name.clone(), node);

        Ok(())
    }

    /// Remove a node from the cluster
    pub async fn remove_node(&self, node_name: &str) -> Result<()> {
        let mut nodes = self.nodes.write().await;

        let node = nodes.get(node_name).ok_or_else(|| {
            horcrux_common::Error::NotFound(format!("Node {} not found", node_name))
        })?;

        if node.is_local {
            return Err(horcrux_common::Error::Conflict(
                "Cannot remove local node from cluster".to_string(),
            ));
        }

        // Remove from Corosync
        self.corosync.remove_node(node).await?;

        config_store::delete_opt(&self.store, kinds::CLUSTER_NODE, node_name).await?;
        nodes.remove(node_name);

        Ok(())
    }

    /// List all nodes in the cluster
    pub async fn list_nodes(&self) -> Vec<Node> {
        let nodes = self.nodes.read().await;
        nodes.values().cloned().collect()
    }

    /// Get cluster status
    pub async fn get_cluster_status(&self) -> Result<ClusterStatus> {
        let config = self._config.read().await;

        let config = config
            .as_ref()
            .ok_or_else(|| horcrux_common::Error::System("Not part of a cluster".to_string()))?;

        let nodes = self.nodes.read().await;
        let online_nodes = nodes
            .values()
            .filter(|n| n.status == NodeStatus::Online)
            .count();

        let quorum = self.corosync.check_quorum().await?;

        Ok(ClusterStatus {
            name: config.name.clone(),
            total_nodes: nodes.len(),
            online_nodes,
            has_quorum: quorum,
            quorum_votes: config.quorum_votes,
        })
    }

    /// Check if cluster has quorum
    pub async fn has_quorum(&self) -> Result<bool> {
        self.corosync.check_quorum().await
    }

    /// Migrate VM to another node
    pub async fn migrate_vm(
        &self,
        vm_id: &str,
        target_node: &str,
        vm_arch: &node::Architecture,
        live: bool,
    ) -> Result<()> {
        let nodes = self.nodes.read().await;

        let target = nodes.get(target_node).ok_or_else(|| {
            horcrux_common::Error::InvalidConfig(format!("Target node {} not found", target_node))
        })?;

        if target.status != NodeStatus::Online {
            return Err(horcrux_common::Error::System(format!(
                "Target node {} is not online",
                target_node
            )));
        }

        // Check architecture compatibility
        if !target.can_run_architecture(vm_arch) {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Target node {} ({:?}) cannot run VMs with architecture {:?}",
                target_node, target.architecture, vm_arch
            )));
        }

        // Warn if migration will require emulation
        if !target.is_native_for(vm_arch) {
            tracing::warn!(
                "Migration of VM {} to node {} will require emulation (VM: {:?}, Node: {:?})",
                vm_id,
                target_node,
                vm_arch,
                target.architecture
            );
        }

        // In a real implementation:
        // 1. Acquire distributed lock for VM
        // 2. If live migration: use QEMU live migration
        // 3. If offline: stop VM, copy disk, start on target
        // 4. Update cluster configuration
        // 5. Release lock

        tracing::info!(
            "Migrating VM {} to node {} (live: {}, native: {})",
            vm_id,
            target_node,
            live,
            target.is_native_for(vm_arch)
        );

        // Placeholder - actual implementation would be more complex
        Ok(())
    }

    /// Find best node for a VM based on architecture and resources.
    ///
    /// `min_priority`, when set, is a hard filter: a node below this
    /// placement weight is never a candidate at all, regardless of how idle
    /// it is. This is what lets a VM be pinned to only the higher-priority
    /// (newer/faster) tier of a mixed-age cluster -- the reverse (preferring
    /// the lower tier) works the same way with a low cutoff plus simply not
    /// having any high-priority nodes with room, since priority is also a
    /// soft preference among the remaining candidates (see below).
    pub async fn find_best_node(
        &self,
        vm_arch: &node::Architecture,
        required_memory: u64,
        required_cores: u32,
        min_priority: Option<u32>,
    ) -> Result<String> {
        let nodes = self.nodes.read().await;

        let mut candidates: Vec<_> = nodes
            .values()
            .filter(|n| {
                n.is_online()
                    && n.can_run_architecture(vm_arch)
                    && n.memory_available() >= required_memory
                    && n.cpu_cores >= required_cores
                    && n.priority >= min_priority.unwrap_or(0)
            })
            .collect();

        if candidates.is_empty() {
            return Err(horcrux_common::Error::System(
                "No suitable node found for VM placement".to_string(),
            ));
        }

        // Prefer native architecture nodes, then higher placement weight
        // (priority), then more free resources as the final tiebreaker.
        candidates.sort_by(|a, b| {
            let a_native = a.is_native_for(vm_arch);
            let b_native = b.is_native_for(vm_arch);

            match (a_native, b_native) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => b.priority.cmp(&a.priority).then_with(|| {
                    // If both native or both emulated and equal priority,
                    // prefer more resources.
                    // Free RAM is the live figure when the node reports it.
                    let a_score = (a.memory_available() / required_memory.max(1))
                        + (a.cpu_cores as u64 / required_cores.max(1) as u64);
                    let b_score = (b.memory_available() / required_memory.max(1))
                        + (b.cpu_cores as u64 / required_cores.max(1) as u64);
                    b_score.cmp(&a_score)
                }),
            }
        });

        Ok(candidates[0].name.clone())
    }

    /// Set a node's placement/HA weight (0-1000 by convention, not enforced
    /// here). Higher means preferred for new VM placement and tried first
    /// for HA failover; lower means "only use this node when nothing
    /// better-weighted has room" -- the intended way to mark an older/
    /// slower host as lower priority in a mixed-age cluster.
    pub async fn set_node_priority(&self, node_name: &str, priority: u32) -> Result<()> {
        let mut nodes = self.nodes.write().await;
        let node = nodes.get_mut(node_name).ok_or_else(|| {
            horcrux_common::Error::NotFound(format!("Node {} not found", node_name))
        })?;
        let mut updated = node.clone();
        updated.priority = priority;
        config_store::put_opt(&self.store, kinds::CLUSTER_NODE, node_name, &updated).await?;
        node.priority = priority;
        Ok(())
    }

    /// Get nodes that support a specific architecture
    pub async fn get_nodes_for_architecture(&self, arch: &node::Architecture) -> Vec<Node> {
        let nodes = self.nodes.read().await;
        nodes
            .values()
            .filter(|n| n.is_online() && n.can_run_architecture(arch))
            .cloned()
            .collect()
    }

    /// Get cluster architecture summary
    pub async fn get_architecture_summary(&self) -> ArchitectureSummary {
        let nodes = self.nodes.read().await;

        let mut x86_64_count = 0;
        let mut aarch64_count = 0;
        let mut other_count = 0;

        for node in nodes.values() {
            match node.architecture {
                node::Architecture::X86_64 => x86_64_count += 1,
                node::Architecture::Aarch64 => aarch64_count += 1,
                _ => other_count += 1,
            }
        }

        ArchitectureSummary {
            x86_64_nodes: x86_64_count,
            aarch64_nodes: aarch64_count,
            other_nodes: other_count,
            is_mixed: (x86_64_count > 0 && aarch64_count > 0)
                || (x86_64_count > 0 && other_count > 0)
                || (aarch64_count > 0 && other_count > 0),
        }
    }

    /// Enable HA for a VM
    pub async fn enable_ha(&self, vm_id: &str, priority: u32) -> Result<()> {
        // Configure Pacemaker resource for VM
        // This would create a resource that monitors the VM and fails it over if the node dies

        tracing::info!("Enabling HA for VM {} with priority {}", vm_id, priority);

        // Placeholder - would integrate with Pacemaker
        Ok(())
    }

    /// Disable HA for a VM
    pub async fn disable_ha(&self, vm_id: &str) -> Result<()> {
        tracing::info!("Disabling HA for VM {}", vm_id);

        // Placeholder - would remove Pacemaker resource
        Ok(())
    }
}

/// Cluster status information
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ClusterStatus {
    pub name: String,
    pub total_nodes: usize,
    pub online_nodes: usize,
    pub has_quorum: bool,
    pub quorum_votes: u32,
}

/// Body of `GET /api/cluster/status`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClusterStatusReport {
    /// Corosync cluster name, `null` if no Corosync cluster is configured.
    pub name: Option<String>,
    pub total_nodes: usize,
    pub online_nodes: usize,
    /// Corosync quorum, `null` when it can't be determined.
    pub has_quorum: Option<bool>,
    /// VMs managed by the API server answering the request.
    pub total_vms: usize,
    /// Sum of `cpu_cores` over all nodes.
    pub total_cpus: u32,
    /// Sum of `memory_total` over all nodes, bytes.
    pub total_memory: u64,
    /// Sum of live `memory_used` over the nodes currently reporting usage,
    /// bytes; `null` if no node is reporting.
    pub memory_used: Option<u64>,
    /// How many nodes are currently reporting live usage.
    pub nodes_reporting_usage: usize,
}

impl ClusterStatusReport {
    pub fn from_nodes(
        name: Option<String>,
        nodes: &[Node],
        has_quorum: Option<bool>,
        total_vms: usize,
    ) -> Self {
        let reporting: Vec<u64> = nodes.iter().filter_map(|n| n.memory_used).collect();
        Self {
            name,
            total_nodes: nodes.len(),
            online_nodes: nodes.iter().filter(|n| n.is_online()).count(),
            has_quorum,
            total_vms,
            total_cpus: nodes.iter().map(|n| n.cpu_cores).sum(),
            total_memory: nodes.iter().map(|n| n.memory_total).sum(),
            memory_used: if reporting.is_empty() {
                None
            } else {
                Some(reporting.iter().sum())
            },
            nodes_reporting_usage: reporting.len(),
        }
    }
}

/// Architecture summary for the cluster
#[derive(Debug, Clone, serde::Serialize)]
pub struct ArchitectureSummary {
    pub x86_64_nodes: usize,
    pub aarch64_nodes: usize,
    pub other_nodes: usize,
    pub is_mixed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use node::{AddNodeRequest, Architecture};

    fn node(name: &str, cores: u32, mem_total: u64, mem_used: Option<u64>, online: bool) -> Node {
        let req: AddNodeRequest = serde_json::from_value(serde_json::json!({
            "address": "10.0.0.1",
            "architecture": "x86_64",
            "cpu_cores": cores,
            "memory_total": mem_total,
            "status": if online { "online" } else { "offline" },
        }))
        .unwrap();
        let mut n = req.into_node(name.to_string(), 1);
        n.memory_used = mem_used;
        n.cpu_usage = mem_used.map(|_| 10.0);
        n
    }

    #[test]
    fn status_report_wire_shape() {
        let nodes = vec![
            node("a", 8, 16 << 30, Some(4 << 30), true),
            node("b", 4, 8 << 30, None, false),
        ];
        let report = ClusterStatusReport::from_nodes(None, &nodes, None, 3);
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "name": null,
                "total_nodes": 2,
                "online_nodes": 1,
                "has_quorum": null,
                "total_vms": 3,
                "total_cpus": 12,
                "total_memory": 25769803776u64,
                "memory_used": 4294967296u64,
                "nodes_reporting_usage": 1
            })
        );
    }

    #[test]
    fn status_report_memory_used_is_null_when_nobody_reports() {
        let nodes = vec![node("a", 8, 16 << 30, None, true)];
        let report = ClusterStatusReport::from_nodes(Some("c1".into()), &nodes, Some(true), 0);
        assert_eq!(
            report.memory_used, None,
            "unknown must not be reported as 0"
        );
        assert_eq!(report.nodes_reporting_usage, 0);
        assert_eq!(report.name.as_deref(), Some("c1"));
        assert_eq!(report.has_quorum, Some(true));
    }

    #[tokio::test]
    async fn remove_node_removes_and_reports_not_found() {
        let mgr = ClusterManager::new();
        mgr.add_node(node("gone", 2, 1 << 30, None, true))
            .await
            .unwrap();
        mgr.remove_node("gone").await.unwrap();
        assert!(mgr.list_nodes().await.is_empty());
        match mgr.remove_node("gone").await {
            Err(horcrux_common::Error::NotFound(_)) => {}
            other => panic!("expected NotFound, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn remove_node_refuses_local_node() {
        let mgr = ClusterManager::new();
        let mut local = node("me", 2, 1 << 30, None, true);
        local.is_local = true;
        mgr.add_node(local).await.unwrap();
        assert!(matches!(
            mgr.remove_node("me").await,
            Err(horcrux_common::Error::Conflict(_))
        ));
        assert_eq!(mgr.list_nodes().await.len(), 1);
    }

    #[tokio::test]
    async fn find_best_node_uses_live_free_memory() {
        let mgr = ClusterManager::new();
        // "big" has more total RAM but almost all of it is in use.
        mgr.add_node(node("big", 8, 64 << 30, Some(62 << 30), true))
            .await
            .unwrap();
        mgr.add_node(node("small", 8, 16 << 30, Some(2 << 30), true))
            .await
            .unwrap();
        let picked = mgr
            .find_best_node(&Architecture::X86_64, 4 << 30, 2, None)
            .await
            .unwrap();
        assert_eq!(picked, "small");
    }

    #[tokio::test]
    async fn poll_round_samples_this_host_and_marks_dead_nodes_offline() {
        if !std::path::Path::new("/proc/stat").exists() {
            return;
        }
        let mgr = ClusterManager::new();
        let mut here = node("here", 0, 0, None, false);
        here.ip = "127.0.0.1".into();
        mgr.add_node(here).await.unwrap();
        // TEST-NET-1 (RFC 5737) is never routed: unreachable -> offline.
        let mut dead = node("dead", 2, 1 << 30, None, true);
        dead.ip = "192.0.2.1".into();
        mgr.add_node(dead).await.unwrap();

        let client = reqwest::Client::new();
        mgr.poll_node_usage_once(&client, 9).await;

        let nodes: HashMap<String, Node> = mgr
            .list_nodes()
            .await
            .into_iter()
            .map(|n| (n.name.clone(), n))
            .collect();
        let here = &nodes["here"];
        assert_eq!(here.status, NodeStatus::Online);
        assert!(here.cpu_usage.is_some());
        assert!(here.memory_used.is_some());
        assert!(here.memory_total > 0 && here.cpu_cores > 0);
        assert!(mgr.local_usage().await.is_some());

        let dead = &nodes["dead"];
        assert_eq!(dead.status, NodeStatus::Offline);
        assert_eq!(dead.cpu_usage, None);
        assert_eq!(dead.memory_used, None);
    }
}
