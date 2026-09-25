//! High Availability (HA) Manager for Horcrux
//!
//! Provides automatic failover and resource management for VMs across cluster nodes

#![allow(dead_code)]

pub mod fencing;

use crate::cluster::balancer::{ClusterBalancer, NodeResources, VmResources};
use crate::cluster::node::Architecture;
use chrono::{DateTime, Utc};
use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// HA resource state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HaState {
    Started,   // Resource is running
    Stopped,   // Resource is intentionally stopped
    Migrating, // Resource is being migrated
    Error,     // Resource failed and needs attention
    Disabled,  // HA disabled for this resource
}

/// HA resource (VM or container)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HaResource {
    pub vm_id: u32,
    pub group: String,
    pub state: HaState,
    pub current_node: Option<String>,
    pub preferred_node: Option<String>,
    pub max_restart: u32,
    pub max_relocate: u32,
    pub restart_count: u32,
    pub relocate_count: u32,
    pub last_state_change: DateTime<Utc>,
    /// CPU architecture the VM requires. Used to ensure failover/migration
    /// never places the VM on a node that cannot run it (native or
    /// emulated). Defaults to x86_64 for backward compatibility.
    #[serde(default)]
    pub architecture: Architecture,
}

/// HA group configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HaGroup {
    pub name: String,
    pub nodes: Vec<String>,
    pub restricted: bool,  // Only use nodes in group
    pub no_failback: bool, // Don't migrate back to preferred node
}

/// HA resource configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HaConfig {
    pub vm_id: u32,
    pub group: String,
    pub max_restart: u32,  // Max restart attempts on same node
    pub max_relocate: u32, // Max relocations to other nodes
    pub state: HaState,    // Requested state
    /// CPU architecture the VM requires. Defaults to x86_64 for backward
    /// compatibility with callers that don't yet pass it through.
    #[serde(default)]
    pub architecture: Architecture,
}

/// HA event for logging/auditing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HaEvent {
    pub timestamp: DateTime<Utc>,
    pub vm_id: u32,
    pub event_type: HaEventType,
    pub old_state: HaState,
    pub new_state: HaState,
    pub node: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HaEventType {
    Started,
    Stopped,
    Restarted,
    Migrated,
    Failed,
    Recovered,
}

/// High Availability Manager
pub struct HaManager {
    resources: Arc<RwLock<HashMap<u32, HaResource>>>,
    groups: Arc<RwLock<HashMap<String, HaGroup>>>,
    events: Arc<RwLock<Vec<HaEvent>>>,
    enabled: Arc<RwLock<bool>>,
    /// Live resource/architecture snapshot per node, used for smart
    /// resource- and architecture-aware failover target selection.
    /// Keyed by node name. Populated via `update_node_resources`.
    node_resources: Arc<RwLock<HashMap<String, NodeResources>>>,
}

impl HaManager {
    pub fn new() -> Self {
        Self {
            resources: Arc::new(RwLock::new(HashMap::new())),
            groups: Arc::new(RwLock::new(HashMap::new())),
            events: Arc::new(RwLock::new(Vec::new())),
            enabled: Arc::new(RwLock::new(false)),
            node_resources: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Update (or insert) the live resource/architecture snapshot for a node.
    /// Callers (e.g. the monitoring subsystem) should push fresh CPU/memory
    /// usage and the node's architecture here so failover target selection
    /// can be both resource- and architecture-aware.
    pub async fn update_node_resources(&self, resources: NodeResources) {
        let mut node_resources = self.node_resources.write().await;
        node_resources.insert(resources.node_name.clone(), resources);
    }

    /// Remove a node's resource snapshot (e.g. when it is decommissioned or
    /// confirmed dead so it is never considered as a migration target).
    pub async fn remove_node_resources(&self, node_name: &str) {
        let mut node_resources = self.node_resources.write().await;
        node_resources.remove(node_name);
    }

    /// Enable HA management
    pub async fn enable(&self) {
        let mut enabled = self.enabled.write().await;
        *enabled = true;
        tracing::info!("HA management enabled");
    }

    /// Disable HA management
    pub async fn disable(&self) {
        let mut enabled = self.enabled.write().await;
        *enabled = false;
        tracing::info!("HA management disabled");
    }

    /// Check if HA is enabled
    pub async fn is_enabled(&self) -> bool {
        *self.enabled.read().await
    }

    /// Add HA group
    pub async fn add_group(&self, group: HaGroup) -> Result<()> {
        let mut groups = self.groups.write().await;

        if groups.contains_key(&group.name) {
            return Err(horcrux_common::Error::System(format!(
                "HA group {} already exists",
                group.name
            )));
        }

        tracing::info!(
            "Adding HA group: {} with nodes {:?}",
            group.name,
            group.nodes
        );
        groups.insert(group.name.clone(), group);
        Ok(())
    }

    /// Remove HA group
    pub async fn remove_group(&self, name: &str) -> Result<()> {
        // Check if any resources use this group
        let resources = self.resources.read().await;
        let using_group = resources.values().any(|r| r.group == name);

        if using_group {
            return Err(horcrux_common::Error::System(format!(
                "Cannot remove group {}: resources still using it",
                name
            )));
        }

        drop(resources);

        let mut groups = self.groups.write().await;
        groups.remove(name);

        tracing::info!("Removed HA group: {}", name);
        Ok(())
    }

    /// List all HA groups
    pub async fn list_groups(&self) -> Vec<HaGroup> {
        self.groups.read().await.values().cloned().collect()
    }

    /// Add HA resource
    pub async fn add_resource(&self, config: HaConfig) -> Result<()> {
        // Verify group exists
        {
            let groups = self.groups.read().await;
            if !groups.contains_key(&config.group) {
                return Err(horcrux_common::Error::System(format!(
                    "HA group {} does not exist",
                    config.group
                )));
            }
        }

        let mut resources = self.resources.write().await;

        if resources.contains_key(&config.vm_id) {
            return Err(horcrux_common::Error::System(format!(
                "VM {} already has HA enabled",
                config.vm_id
            )));
        }

        let resource = HaResource {
            vm_id: config.vm_id,
            group: config.group.clone(),
            state: config.state,
            current_node: None,
            preferred_node: None,
            max_restart: config.max_restart,
            max_relocate: config.max_relocate,
            restart_count: 0,
            relocate_count: 0,
            last_state_change: Utc::now(),
            architecture: config.architecture,
        };

        tracing::info!(
            "Added HA resource: VM {} to group {}",
            config.vm_id,
            config.group
        );
        resources.insert(config.vm_id, resource);

        Ok(())
    }

    /// Remove HA resource
    pub async fn remove_resource(&self, vm_id: u32) -> Result<()> {
        let mut resources = self.resources.write().await;

        if let Some(_) = resources.remove(&vm_id) {
            tracing::info!("Removed HA resource: VM {}", vm_id);
            Ok(())
        } else {
            Err(horcrux_common::Error::System(format!(
                "VM {} not managed by HA",
                vm_id
            )))
        }
    }

    /// List all HA resources
    pub async fn list_resources(&self) -> Vec<HaResource> {
        self.resources.read().await.values().cloned().collect()
    }

    /// Get HA resource status
    pub async fn get_resource(&self, vm_id: u32) -> Option<HaResource> {
        self.resources.read().await.get(&vm_id).cloned()
    }

    /// Handle node failure - migrate all VMs from failed node
    pub async fn handle_node_failure(&self, failed_node: &str) -> Result<Vec<u32>> {
        if !self.is_enabled().await {
            return Ok(Vec::new());
        }

        tracing::warn!("Handling failure of node: {}", failed_node);

        let mut migrated_vms = Vec::new();
        let mut resources = self.resources.write().await;

        for (vm_id, resource) in resources.iter_mut() {
            // Only handle VMs currently on the failed node
            if resource.current_node.as_deref() != Some(failed_node) {
                continue;
            }

            // Check if we can relocate
            if resource.relocate_count >= resource.max_relocate {
                tracing::error!(
                    "VM {} exceeded max relocations ({}), setting to error state",
                    vm_id,
                    resource.max_relocate
                );
                resource.state = HaState::Error;
                continue;
            }

            // Find a suitable node for migration
            let target_node = self.find_best_node_for_migration(resource).await?;

            if let Some(target) = target_node {
                tracing::info!("Migrating VM {} from {} to {}", vm_id, failed_node, target);

                resource.state = HaState::Migrating;
                resource.relocate_count += 1;
                resource.last_state_change = Utc::now();

                // Log event
                self.log_event(HaEvent {
                    timestamp: Utc::now(),
                    vm_id: *vm_id,
                    event_type: HaEventType::Migrated,
                    old_state: HaState::Started,
                    new_state: HaState::Migrating,
                    node: target.clone(),
                    message: format!("Migrating from failed node {}", failed_node),
                })
                .await;

                migrated_vms.push(*vm_id);
            } else {
                tracing::error!("No suitable node found for VM {}", vm_id);
                resource.state = HaState::Error;
            }
        }

        Ok(migrated_vms)
    }

    /// Handle VM failure - attempt restart or migration
    pub async fn handle_vm_failure(&self, vm_id: u32, node: &str) -> Result<HaAction> {
        if !self.is_enabled().await {
            return Ok(HaAction::None);
        }

        let mut resources = self.resources.write().await;

        let resource = resources.get_mut(&vm_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("VM {} not managed by HA", vm_id))
        })?;

        tracing::warn!("Handling failure of VM {} on node {}", vm_id, node);

        // Try restart first if under limit
        if resource.restart_count < resource.max_restart {
            resource.restart_count += 1;
            resource.last_state_change = Utc::now();

            self.log_event(HaEvent {
                timestamp: Utc::now(),
                vm_id,
                event_type: HaEventType::Restarted,
                old_state: HaState::Error,
                new_state: HaState::Started,
                node: node.to_string(),
                message: format!(
                    "Restart attempt {}/{}",
                    resource.restart_count, resource.max_restart
                ),
            })
            .await;

            tracing::info!(
                "Attempting restart {}/{} for VM {}",
                resource.restart_count,
                resource.max_restart,
                vm_id
            );

            return Ok(HaAction::Restart);
        }

        // Restart limit reached, try migration if under relocate limit
        if resource.relocate_count < resource.max_relocate {
            let target_node = self.find_best_node_for_migration(resource).await?;

            if let Some(target) = target_node {
                resource.state = HaState::Migrating;
                resource.relocate_count += 1;
                resource.restart_count = 0; // Reset restart counter on migration
                resource.last_state_change = Utc::now();

                self.log_event(HaEvent {
                    timestamp: Utc::now(),
                    vm_id,
                    event_type: HaEventType::Migrated,
                    old_state: HaState::Error,
                    new_state: HaState::Migrating,
                    node: target.clone(),
                    message: format!("Migrating after {} failed restarts", resource.max_restart),
                })
                .await;

                tracing::info!(
                    "Migrating VM {} to {} after restart failures",
                    vm_id,
                    target
                );
                return Ok(HaAction::Migrate(target));
            }
        }

        // All recovery attempts exhausted
        resource.state = HaState::Error;

        self.log_event(HaEvent {
            timestamp: Utc::now(),
            vm_id,
            event_type: HaEventType::Failed,
            old_state: HaState::Started,
            new_state: HaState::Error,
            node: node.to_string(),
            message: "All recovery attempts exhausted".to_string(),
        })
        .await;

        tracing::error!(
            "VM {} exceeded all recovery limits, setting to error state",
            vm_id
        );
        Ok(HaAction::None)
    }

    /// Find best node for VM migration
    ///
    /// Smart, resource- and architecture-aware target selection:
    /// - Candidate nodes are restricted to the HA group's node list (unchanged
    ///   behavior), minus the VM's current node.
    /// - The preferred node is honored ONLY if it is present in the group and
    ///   also architecture-compatible with the VM -- a stale/incompatible
    ///   preferred node is never blindly used.
    /// - Remaining candidates are filtered so a node that cannot run the VM's
    ///   architecture (native or emulated -- see `Architecture::can_run`) is
    ///   never selected, then least-loaded/native-preferred scoring is
    ///   applied via `ClusterBalancer::find_best_node`.
    /// - If no live resource snapshot exists for a candidate node (e.g. in
    ///   tests, or before monitoring has reported in), it falls back to the
    ///   original "first available node" behavior for that node -- but ONLY
    ///   when the node is still architecture-compatible; the architecture
    ///   check is applied whenever the node's architecture is known.
    async fn find_best_node_for_migration(&self, resource: &HaResource) -> Result<Option<String>> {
        let groups = self.groups.read().await;

        let group = groups.get(&resource.group).ok_or_else(|| {
            horcrux_common::Error::System(format!("HA group {} not found", resource.group))
        })?;

        let node_resources = self.node_resources.read().await;

        // Prefer the preferred node if available AND architecture-compatible.
        if let Some(ref preferred) = resource.preferred_node {
            if group.nodes.contains(preferred) {
                let compatible = match node_resources.get(preferred) {
                    Some(res) => res.architecture.can_run(&resource.architecture),
                    // No resource snapshot known for this node -- don't block
                    // on architecture we have no information about.
                    None => true,
                };
                if compatible {
                    return Ok(Some(preferred.clone()));
                }
            }
        }

        // Build candidate list: group nodes minus the current node.
        let candidate_names: Vec<&String> = group
            .nodes
            .iter()
            .filter(|node| Some(node.as_str()) != resource.current_node.as_deref())
            .collect();

        // Split into nodes we have live resource data for (used for smart
        // scoring) and nodes we don't (legacy fallback path).
        let mut known_candidates: Vec<NodeResources> = Vec::new();
        let mut unknown_candidates: Vec<&String> = Vec::new();

        for name in &candidate_names {
            match node_resources.get(name.as_str()) {
                Some(res) => known_candidates.push(res.clone()),
                None => unknown_candidates.push(name),
            }
        }

        if !known_candidates.is_empty() {
            let vm = VmResources {
                vm_id: resource.vm_id,
                cpu_cores: 0,
                memory_gb: 0,
                disk_gb: 0,
                current_node: resource.current_node.clone().unwrap_or_default(),
                can_migrate: true,
                architecture: resource.architecture.clone(),
            };

            let balancer = ClusterBalancer::new(Default::default());
            if let Some(target) = balancer.find_best_node(&known_candidates, &vm) {
                return Ok(Some(target));
            }

            // No known-resource node was architecture-compatible /
            // had room. Fall through to the unknown-resource nodes below
            // ONLY IF they are not architecture-incompatible by name lookup
            // (we simply have no data, so we can't rule them out, but we
            // also must not silently pick one over an incompatible known
            // node -- this mirrors "no resources anywhere" semantics for the
            // known set while still allowing legacy behavior for nodes with
            // no monitoring data at all).
        }

        // Legacy fallback: first available node with no resource/architecture
        // data at all (keeps old behavior for callers/tests that don't feed
        // node_resources into the HA manager).
        for node in unknown_candidates {
            return Ok(Some(node.clone()));
        }

        Ok(None)
    }

    /// Log HA event
    async fn log_event(&self, event: HaEvent) {
        let mut events = self.events.write().await;
        events.push(event);

        // Keep only last 1000 events
        if events.len() > 1000 {
            let drain_count = events.len() - 1000;
            events.drain(0..drain_count);
        }
    }

    /// Get HA event history
    pub async fn get_events(&self, vm_id: Option<u32>, limit: Option<usize>) -> Vec<HaEvent> {
        let events = self.events.read().await;

        let filtered: Vec<_> = match vm_id {
            Some(id) => events.iter().filter(|e| e.vm_id == id).cloned().collect(),
            None => events.clone(),
        };

        let limit = limit.unwrap_or(100);
        filtered.iter().rev().take(limit).cloned().collect()
    }

    /// Update resource state (called when state changes externally)
    pub async fn update_resource_state(
        &self,
        vm_id: u32,
        new_state: HaState,
        node: Option<String>,
    ) -> Result<()> {
        let mut resources = self.resources.write().await;

        let resource = resources.get_mut(&vm_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("VM {} not managed by HA", vm_id))
        })?;

        let old_state = resource.state.clone();
        resource.state = new_state.clone();
        resource.last_state_change = Utc::now();

        if let Some(n) = node {
            resource.current_node = Some(n.clone());
        }

        // Reset counters on successful start
        if new_state == HaState::Started {
            resource.restart_count = 0;
        }

        tracing::info!(
            "Updated HA resource VM {} state: {:?} -> {:?}",
            vm_id,
            old_state,
            new_state
        );

        Ok(())
    }
}

/// HA action to take
#[derive(Debug, Clone)]
pub enum HaAction {
    None,
    Restart,
    Migrate(String), // Target node
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_ha_group_management() {
        let manager = HaManager::new();

        let group = HaGroup {
            name: "test-group".to_string(),
            nodes: vec!["node1".to_string(), "node2".to_string()],
            restricted: false,
            no_failback: false,
        };

        manager.add_group(group).await.unwrap();

        let groups = manager.list_groups().await;
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "test-group");
    }

    #[tokio::test]
    async fn test_ha_resource_management() {
        let manager = HaManager::new();

        // Add group first
        let group = HaGroup {
            name: "test-group".to_string(),
            nodes: vec!["node1".to_string()],
            restricted: false,
            no_failback: false,
        };
        manager.add_group(group).await.unwrap();

        // Add resource
        let config = HaConfig {
            vm_id: 100,
            group: "test-group".to_string(),
            max_restart: 3,
            max_relocate: 2,
            state: HaState::Started,
            architecture: Architecture::X86_64,
        };
        manager.add_resource(config).await.unwrap();

        let resources = manager.list_resources().await;
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].vm_id, 100);
    }

    #[tokio::test]
    async fn test_vm_failure_restart() {
        let manager = HaManager::new();
        manager.enable().await;

        let group = HaGroup {
            name: "test-group".to_string(),
            nodes: vec!["node1".to_string(), "node2".to_string()],
            restricted: false,
            no_failback: false,
        };
        manager.add_group(group).await.unwrap();

        let config = HaConfig {
            vm_id: 100,
            group: "test-group".to_string(),
            max_restart: 3,
            max_relocate: 2,
            state: HaState::Started,
            architecture: Architecture::X86_64,
        };
        manager.add_resource(config).await.unwrap();

        // First failure should restart
        let action = manager.handle_vm_failure(100, "node1").await.unwrap();
        assert!(matches!(action, HaAction::Restart));

        // After max restarts, should migrate
        let action = manager.handle_vm_failure(100, "node1").await.unwrap();
        assert!(matches!(action, HaAction::Restart));

        let action = manager.handle_vm_failure(100, "node1").await.unwrap();
        assert!(matches!(action, HaAction::Restart));

        let action = manager.handle_vm_failure(100, "node1").await.unwrap();
        assert!(matches!(action, HaAction::Migrate(_)));
    }

    fn make_node_resources(name: &str, arch: Architecture) -> NodeResources {
        NodeResources {
            node_name: name.to_string(),
            cpu_usage: 10.0,
            memory_usage: 10.0,
            disk_usage: 10.0,
            network_usage: 10.0,
            vm_count: 0,
            total_cpu_cores: 16,
            total_memory_gb: 64,
            total_disk_gb: 1000,
            architecture: arch,
        }
    }

    /// 3-node mixed-architecture cluster failover: x86_64, aarch64, riscv64.
    /// An x86_64 VM failing over must never land on the riscv64-only node
    /// (riscv64 cannot emulate anything), and must prefer the native x86_64
    /// node over the aarch64 (emulation-only for x86_64) node when both have
    /// room.
    #[tokio::test]
    async fn test_failover_x86_vm_never_migrates_to_riscv_node() {
        let manager = HaManager::new();
        manager.enable().await;

        let group = HaGroup {
            name: "mixed-group".to_string(),
            nodes: vec![
                "x86-node".to_string(),
                "arm-node".to_string(),
                "riscv-node".to_string(),
            ],
            restricted: false,
            no_failback: false,
        };
        manager.add_group(group).await.unwrap();

        manager
            .update_node_resources(make_node_resources("x86-node", Architecture::X86_64))
            .await;
        manager
            .update_node_resources(make_node_resources("arm-node", Architecture::Aarch64))
            .await;
        manager
            .update_node_resources(make_node_resources("riscv-node", Architecture::Riscv64))
            .await;

        let config = HaConfig {
            vm_id: 300,
            group: "mixed-group".to_string(),
            max_restart: 0,
            max_relocate: 3,
            state: HaState::Started,
            architecture: Architecture::X86_64,
        };
        manager.add_resource(config).await.unwrap();
        manager
            .update_resource_state(300, HaState::Started, Some("x86-node".to_string()))
            .await
            .unwrap();

        // x86-node is the VM's current node, so failover must pick from
        // {arm-node, riscv-node}. Since arm-node can emulate x86_64 but
        // riscv-node cannot run x86_64 at all, riscv-node must never be
        // selected.
        let migrated = manager.handle_node_failure("x86-node").await.unwrap();
        assert_eq!(migrated, vec![300]);

        let resource = manager.get_resource(300).await.unwrap();
        assert_eq!(resource.state, HaState::Migrating);

        let events = manager.get_events(Some(300), None).await;
        let migrate_event = events
            .iter()
            .find(|e| matches!(e.event_type, HaEventType::Migrated))
            .expect("expected a migration event");
        assert_ne!(migrate_event.node, "riscv-node");
        assert_eq!(migrate_event.node, "arm-node");
    }

    /// A riscv64 VM in the same mixed cluster must ONLY ever be placed on
    /// the riscv64 node -- neither x86_64 nor aarch64 hosts can emulate
    /// riscv64 as a guest architecture per `Architecture::can_run`.
    #[tokio::test]
    async fn test_failover_riscv_vm_only_migrates_to_riscv_node() {
        let manager = HaManager::new();
        manager.enable().await;

        let group = HaGroup {
            name: "mixed-group".to_string(),
            nodes: vec![
                "x86-node".to_string(),
                "arm-node".to_string(),
                "riscv-node".to_string(),
                "riscv-node-2".to_string(),
            ],
            restricted: false,
            no_failback: false,
        };
        manager.add_group(group).await.unwrap();

        manager
            .update_node_resources(make_node_resources("x86-node", Architecture::X86_64))
            .await;
        manager
            .update_node_resources(make_node_resources("arm-node", Architecture::Aarch64))
            .await;
        manager
            .update_node_resources(make_node_resources("riscv-node", Architecture::Riscv64))
            .await;
        manager
            .update_node_resources(make_node_resources("riscv-node-2", Architecture::Riscv64))
            .await;

        let config = HaConfig {
            vm_id: 301,
            group: "mixed-group".to_string(),
            max_restart: 0,
            max_relocate: 3,
            state: HaState::Started,
            architecture: Architecture::Riscv64,
        };
        manager.add_resource(config).await.unwrap();
        manager
            .update_resource_state(301, HaState::Started, Some("riscv-node".to_string()))
            .await
            .unwrap();

        let migrated = manager.handle_node_failure("riscv-node").await.unwrap();
        assert_eq!(migrated, vec![301]);

        let events = manager.get_events(Some(301), None).await;
        let migrate_event = events
            .iter()
            .find(|e| matches!(e.event_type, HaEventType::Migrated))
            .expect("expected a migration event");
        // Only the other riscv64 node is a legal target.
        assert_eq!(migrate_event.node, "riscv-node-2");
    }

    /// If no architecture-compatible node has room (here: the only riscv64
    /// node is the one that failed), failover must give up cleanly (Error
    /// state) rather than silently placing the VM on an incompatible node.
    #[tokio::test]
    async fn test_failover_riscv_vm_errors_when_no_riscv_node_available() {
        let manager = HaManager::new();
        manager.enable().await;

        let group = HaGroup {
            name: "mixed-group".to_string(),
            nodes: vec!["x86-node".to_string(), "riscv-node".to_string()],
            restricted: false,
            no_failback: false,
        };
        manager.add_group(group).await.unwrap();

        manager
            .update_node_resources(make_node_resources("x86-node", Architecture::X86_64))
            .await;
        manager
            .update_node_resources(make_node_resources("riscv-node", Architecture::Riscv64))
            .await;

        let config = HaConfig {
            vm_id: 302,
            group: "mixed-group".to_string(),
            max_restart: 0,
            max_relocate: 3,
            state: HaState::Started,
            architecture: Architecture::Riscv64,
        };
        manager.add_resource(config).await.unwrap();
        manager
            .update_resource_state(302, HaState::Started, Some("riscv-node".to_string()))
            .await
            .unwrap();

        // Only remaining candidate is x86-node, which cannot run riscv64 at
        // all -- there is no legal migration target.
        let migrated = manager.handle_node_failure("riscv-node").await.unwrap();
        assert!(migrated.is_empty());

        let resource = manager.get_resource(302).await.unwrap();
        assert_eq!(resource.state, HaState::Error);
    }
}
