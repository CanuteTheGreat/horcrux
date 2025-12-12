///! Clustering support module
///! Provides multi-node cluster management, HA, and VM migration using Corosync

pub mod corosync;
pub mod node;
pub mod affinity;
pub mod balancer;
pub mod arch;

use horcrux_common::Result;
pub use node::{Node, NodeStatus};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Cluster configuration
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ClusterConfig {
    pub name: String,
    pub nodes: Vec<Node>,
    pub quorum_votes: u32,
}

/// Cluster manager
pub struct ClusterManager {
    _config: Arc<RwLock<Option<ClusterConfig>>>,  // Reserved for runtime cluster config updates
    nodes: Arc<RwLock<HashMap<String, Node>>>,
    corosync: corosync::CorosyncManager,
    local_node_name: Arc<RwLock<Option<String>>>,
}

#[allow(dead_code)]
impl ClusterManager {
    pub fn new() -> Self {
        Self {
            _config: Arc::new(RwLock::new(None)),
            nodes: Arc::new(RwLock::new(HashMap::new())),
            corosync: corosync::CorosyncManager::new(),
            local_node_name: Arc::new(RwLock::new(None)),
        }
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

        *config = Some(ClusterConfig {
            name: cluster_name,
            nodes: vec![node],
            quorum_votes: 1,
        });

        let mut nodes_map = self.nodes.write().await;
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
        *config = Some(ClusterConfig {
            name: cluster_name,
            nodes: vec![],
            quorum_votes: 1,
        });

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

        nodes.insert(node.name.clone(), node);

        Ok(())
    }

    /// Remove a node from the cluster
    pub async fn remove_node(&self, node_name: &str) -> Result<()> {
        let mut nodes = self.nodes.write().await;

        let node = nodes
            .get(node_name)
            .ok_or_else(|| horcrux_common::Error::InvalidConfig(format!("Node {} not found", node_name)))?;

        if node.is_local {
            return Err(horcrux_common::Error::InvalidConfig(
                "Cannot remove local node from cluster".to_string(),
            ));
        }

        // Remove from Corosync
        self.corosync.remove_node(node).await?;

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
        let online_nodes = nodes.values().filter(|n| n.status == NodeStatus::Online).count();

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

        let target = nodes
            .get(target_node)
            .ok_or_else(|| horcrux_common::Error::InvalidConfig(format!("Target node {} not found", target_node)))?;

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

    /// Find best node for a VM based on architecture and resources
    pub async fn find_best_node(&self, vm_arch: &node::Architecture, required_memory: u64, required_cores: u32) -> Result<String> {
        let nodes = self.nodes.read().await;

        let mut candidates: Vec<_> = nodes
            .values()
            .filter(|n| {
                n.is_online()
                    && n.can_run_architecture(vm_arch)
                    && n.memory_total >= required_memory
                    && n.cpu_cores >= required_cores
            })
            .collect();

        if candidates.is_empty() {
            return Err(horcrux_common::Error::System(
                "No suitable node found for VM placement".to_string(),
            ));
        }

        // Prefer native architecture nodes
        candidates.sort_by(|a, b| {
            let a_native = a.is_native_for(vm_arch);
            let b_native = b.is_native_for(vm_arch);

            match (a_native, b_native) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => {
                    // If both native or both emulated, prefer more resources
                    let a_score = (a.memory_total / required_memory) + (a.cpu_cores as u64 / required_cores as u64);
                    let b_score = (b.memory_total / required_memory) + (b.cpu_cores as u64 / required_cores as u64);
                    b_score.cmp(&a_score)
                }
            }
        });

        Ok(candidates[0].name.clone())
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

/// Architecture summary for the cluster
#[derive(Debug, Clone, serde::Serialize)]
pub struct ArchitectureSummary {
    pub x86_64_nodes: usize,
    pub aarch64_nodes: usize,
    pub other_nodes: usize,
    pub is_mixed: bool,
}
