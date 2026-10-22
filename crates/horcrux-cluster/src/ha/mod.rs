//! High Availability (HA) Manager for Horcrux
//!
//! Provides automatic failover and resource management for VMs across cluster nodes

#![allow(dead_code)]

pub mod fencing;

use crate::cluster::balancer::{ClusterBalancer, NodeResources, VmResources};
use crate::cluster::node::{
    passthrough_devices_satisfied, Architecture, PassthroughDeviceRequirement,
};
use crate::db::config_store::{self, kinds, ConfigStore};
use crate::db::Database;
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
    pub vm_id: String,
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
    /// Passthrough devices (GPUs, etc.) physically attached to this VM.
    /// Failover/migration must only ever select a node that currently has an
    /// available (not in_use) unit of each required device, matched by
    /// vendor:device ID -- mirrors the architecture field/check exactly.
    /// Defaults to empty for backward compatibility with VMs that don't use
    /// passthrough.
    #[serde(default)]
    pub required_passthrough_devices: Vec<PassthroughDeviceRequirement>,
    /// Bumped every time the cluster moves this resource to another node.
    /// A node that comes back after being replaced sees a higher number on
    /// its peers and gives up its stale copy of the VM (see `ha_loop`).
    #[serde(default)]
    pub placement_seq: u64,
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
    pub vm_id: String,
    pub group: String,
    pub max_restart: u32,  // Max restart attempts on same node
    pub max_relocate: u32, // Max relocations to other nodes
    pub state: HaState,    // Requested state
    /// CPU architecture the VM requires. Defaults to x86_64 for backward
    /// compatibility with callers that don't yet pass it through.
    #[serde(default)]
    pub architecture: Architecture,
    /// Passthrough devices (GPUs, etc.) the VM requires. Defaults to empty
    /// for backward compatibility with callers that don't yet pass it
    /// through.
    #[serde(default)]
    pub required_passthrough_devices: Vec<PassthroughDeviceRequirement>,
}

/// HA event for logging/auditing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HaEvent {
    pub timestamp: DateTime<Utc>,
    pub vm_id: String,
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
    resources: Arc<RwLock<HashMap<String, HaResource>>>,
    groups: Arc<RwLock<HashMap<String, HaGroup>>>,
    events: Arc<RwLock<Vec<HaEvent>>>,
    enabled: Arc<RwLock<bool>>,
    /// Live resource/architecture snapshot per node, used for smart
    /// resource- and architecture-aware failover target selection.
    /// Keyed by node name. Populated via `update_node_resources`.
    node_resources: Arc<RwLock<HashMap<String, NodeResources>>>,
    /// Durable copy of resources, groups and the enabled flag; `None` =
    /// in-memory only.
    store: Option<ConfigStore>,
}

/// Persisted HA-wide settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct HaSettings {
    enabled: bool,
}

const HA_SETTINGS_ID: &str = "global";

impl Default for HaManager {
    fn default() -> Self {
        Self::new()
    }
}

impl HaManager {
    pub fn new() -> Self {
        Self {
            resources: Arc::new(RwLock::new(HashMap::new())),
            groups: Arc::new(RwLock::new(HashMap::new())),
            events: Arc::new(RwLock::new(Vec::new())),
            enabled: Arc::new(RwLock::new(false)),
            node_resources: Arc::new(RwLock::new(HashMap::new())),
            store: None,
        }
    }

    /// HA manager backed by the database: loads persisted groups, resources
    /// (including their last known state/node/counters) and the enabled
    /// flag, and writes every change through.
    pub async fn with_database(db: Arc<Database>) -> Result<Self> {
        let store = ConfigStore::new(&db);
        let mut mgr = Self::new();
        {
            let mut groups = mgr.groups.write().await;
            for g in store.load_values::<HaGroup>(kinds::HA_GROUP).await? {
                groups.insert(g.name.clone(), g);
            }
            let mut resources = mgr.resources.write().await;
            for r in store.load_values::<HaResource>(kinds::HA_RESOURCE).await? {
                resources.insert(r.vm_id.clone(), r);
            }
            if let Some(settings) = store
                .load_values::<HaSettings>(kinds::HA_SETTINGS)
                .await?
                .into_iter()
                .next()
            {
                *mgr.enabled.write().await = settings.enabled;
            }
            tracing::info!(
                "Loaded {} HA group(s) and {} HA resource(s) from database",
                groups.len(),
                resources.len()
            );
        }
        mgr.store = Some(store);
        Ok(mgr)
    }

    /// Write the current in-memory state of one resource (or its absence)
    /// to the database. Used after HA state transitions; a DB failure is
    /// logged rather than undoing a transition that already happened.
    async fn sync_resource(&self, vm_id: &str) {
        if self.store.is_none() {
            return;
        }
        let current = self.resources.read().await.get(vm_id).cloned();
        let res = match current {
            Some(r) => config_store::put_opt(&self.store, kinds::HA_RESOURCE, vm_id, &r).await,
            None => config_store::delete_opt(&self.store, kinds::HA_RESOURCE, vm_id).await,
        };
        if let Err(e) = res {
            tracing::error!("Failed to persist HA resource {}: {}", vm_id, e);
        }
    }

    async fn persist_enabled(&self, enabled: bool) {
        if let Err(e) = config_store::put_opt(
            &self.store,
            kinds::HA_SETTINGS,
            HA_SETTINGS_ID,
            &HaSettings { enabled },
        )
        .await
        {
            tracing::error!("Failed to persist HA enabled flag: {}", e);
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
        self.persist_enabled(true).await;
        tracing::info!("HA management enabled");
    }

    /// Disable HA management
    pub async fn disable(&self) {
        let mut enabled = self.enabled.write().await;
        *enabled = false;
        self.persist_enabled(false).await;
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
            return Err(horcrux_common::Error::Conflict(format!(
                "HA group {} already exists",
                group.name
            )));
        }

        tracing::info!(
            "Adding HA group: {} with nodes {:?}",
            group.name,
            group.nodes
        );
        config_store::put_opt(&self.store, kinds::HA_GROUP, &group.name, &group).await?;
        groups.insert(group.name.clone(), group);
        Ok(())
    }

    /// Remove HA group
    pub async fn remove_group(&self, name: &str) -> Result<()> {
        // Check if any resources use this group
        let resources = self.resources.read().await;
        let using_group = resources.values().any(|r| r.group == name);

        if using_group {
            return Err(horcrux_common::Error::Conflict(format!(
                "Cannot remove group {}: resources still using it",
                name
            )));
        }

        drop(resources);

        let mut groups = self.groups.write().await;
        if !groups.contains_key(name) {
            return Err(horcrux_common::Error::NotFound(format!(
                "HA group {} not found",
                name
            )));
        }
        config_store::delete_opt(&self.store, kinds::HA_GROUP, name).await?;
        groups.remove(name);

        tracing::info!("Removed HA group: {}", name);
        Ok(())
    }

    /// Replace an existing group's node list / flags.
    pub async fn update_group(&self, group: HaGroup) -> Result<()> {
        let mut groups = self.groups.write().await;
        match groups.get_mut(&group.name) {
            Some(existing) => {
                config_store::put_opt(&self.store, kinds::HA_GROUP, &group.name, &group).await?;
                *existing = group;
                Ok(())
            }
            None => Err(horcrux_common::Error::NotFound(format!(
                "HA group {} not found",
                group.name
            ))),
        }
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
                return Err(horcrux_common::Error::NotFound(format!(
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
            vm_id: config.vm_id.clone(),
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
            required_passthrough_devices: config.required_passthrough_devices,
            placement_seq: 0,
        };

        tracing::info!(
            "Added HA resource: VM {} to group {}",
            config.vm_id,
            config.group
        );
        config_store::put_opt(&self.store, kinds::HA_RESOURCE, &config.vm_id, &resource).await?;
        resources.insert(config.vm_id.clone(), resource);

        Ok(())
    }

    /// Remove HA resource
    pub async fn remove_resource(&self, vm_id: &str) -> Result<()> {
        let mut resources = self.resources.write().await;

        if resources.contains_key(vm_id) {
            config_store::delete_opt(&self.store, kinds::HA_RESOURCE, vm_id).await?;
        }
        if resources.remove(vm_id).is_some() {
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
    pub async fn get_resource(&self, vm_id: &str) -> Option<HaResource> {
        self.resources.read().await.get(vm_id).cloned()
    }

    /// Handle node failure - migrate all VMs from failed node
    pub async fn handle_node_failure(&self, failed_node: &str) -> Result<Vec<String>> {
        let result = self.handle_node_failure_inner(failed_node).await;
        // Persist every resource that was on the failed node (state,
        // relocate counters) whether or not relocation succeeded.
        let affected: Vec<String> = self.resources.read().await.keys().cloned().collect();
        for vm_id in affected {
            self.sync_resource(&vm_id).await;
        }
        result
    }

    async fn handle_node_failure_inner(&self, failed_node: &str) -> Result<Vec<String>> {
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
            let target_node = self.find_best_node_for_migration(resource, None).await?;

            if let Some(target) = target_node {
                tracing::info!("Migrating VM {} from {} to {}", vm_id, failed_node, target);

                resource.state = HaState::Migrating;
                resource.relocate_count += 1;
                resource.last_state_change = Utc::now();

                // Log event
                self.log_event(HaEvent {
                    timestamp: Utc::now(),
                    vm_id: vm_id.clone(),
                    event_type: HaEventType::Migrated,
                    old_state: HaState::Started,
                    new_state: HaState::Migrating,
                    node: target.clone(),
                    message: format!("Migrating from failed node {}", failed_node),
                })
                .await;

                migrated_vms.push(vm_id.clone());
            } else {
                tracing::error!("No suitable node found for VM {}", vm_id);
                resource.state = HaState::Error;
            }
        }

        Ok(migrated_vms)
    }

    /// Handle VM failure - attempt restart or migration
    pub async fn handle_vm_failure(&self, vm_id: &str, node: &str) -> Result<HaAction> {
        let result = self.handle_vm_failure_inner(vm_id, node).await;
        self.sync_resource(vm_id).await;
        result
    }

    async fn handle_vm_failure_inner(&self, vm_id: &str, node: &str) -> Result<HaAction> {
        if !self.is_enabled().await {
            return Ok(HaAction::None);
        }

        let mut resources = self.resources.write().await;

        let resource = resources.get_mut(vm_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("VM {} not managed by HA", vm_id))
        })?;

        tracing::warn!("Handling failure of VM {} on node {}", vm_id, node);

        // Try restart first if under limit
        if resource.restart_count < resource.max_restart {
            resource.restart_count += 1;
            resource.last_state_change = Utc::now();

            self.log_event(HaEvent {
                timestamp: Utc::now(),
                vm_id: vm_id.to_string(),
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
            let target_node = self.find_best_node_for_migration(resource, None).await?;

            if let Some(target) = target_node {
                resource.state = HaState::Migrating;
                resource.relocate_count += 1;
                resource.restart_count = 0; // Reset restart counter on migration
                resource.last_state_change = Utc::now();

                self.log_event(HaEvent {
                    timestamp: Utc::now(),
                    vm_id: vm_id.to_string(),
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
            vm_id: vm_id.to_string(),
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
    /// Smart, resource-, architecture-, and passthrough-device-aware target
    /// selection:
    /// - Candidate nodes are restricted to the HA group's node list (unchanged
    ///   behavior), minus the VM's current node.
    /// - The preferred node is honored ONLY if it is present in the group,
    ///   architecture-compatible with the VM, AND has every passthrough
    ///   device the VM requires currently available -- a stale/incompatible
    ///   preferred node is never blindly used.
    /// - Remaining candidates are filtered so a node that cannot run the VM's
    ///   architecture (native or emulated -- see `Architecture::can_run`) OR
    ///   is missing one or more required passthrough devices (GPU, etc. --
    ///   matched by vendor:device ID among the node's currently-available,
    ///   not-in_use units) is never selected, then least-loaded/native-
    ///   preferred scoring is applied via `ClusterBalancer::find_best_node`.
    /// - If no live resource snapshot exists for a candidate node (e.g. in
    ///   tests, or before monitoring has reported in), it falls back to the
    ///   original "first available node" behavior for that node -- but ONLY
    ///   when the node is still architecture-compatible; the architecture and
    ///   passthrough-device checks are applied whenever the node's resource
    ///   snapshot is known. A VM with passthrough requirements but no
    ///   candidate node snapshot at all has no way to be verified compatible,
    ///   so (same as unknown architecture) it still falls through to the
    ///   legacy path -- callers that care about passthrough correctness
    ///   should always populate node_resources via `update_node_resources`.
    async fn find_best_node_for_migration(
        &self,
        resource: &HaResource,
        online: Option<&[String]>,
    ) -> Result<Option<String>> {
        let usable = |n: &str| online.map(|o| o.iter().any(|x| x == n)).unwrap_or(true);
        let groups = self.groups.read().await;

        let group = groups.get(&resource.group).ok_or_else(|| {
            horcrux_common::Error::System(format!("HA group {} not found", resource.group))
        })?;

        let node_resources = self.node_resources.read().await;

        // Prefer the preferred node if available, architecture-compatible,
        // AND has every required passthrough device currently available.
        if let Some(ref preferred) = resource.preferred_node {
            if group.nodes.contains(preferred) && usable(preferred) {
                let compatible = match node_resources.get(preferred) {
                    Some(res) => {
                        res.architecture.can_run(&resource.architecture)
                            && passthrough_devices_satisfied(
                                &res.available_passthrough_devices,
                                &resource.required_passthrough_devices,
                            )
                    }
                    // No resource snapshot known for this node -- don't block
                    // on architecture/passthrough info we don't have.
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
            .filter(|node| usable(node.as_str()))
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
                vm_id: resource.vm_id.clone(),
                cpu_cores: 0,
                memory_gb: 0,
                disk_gb: 0,
                current_node: resource.current_node.clone().unwrap_or_default(),
                can_migrate: true,
                architecture: resource.architecture.clone(),
                required_passthrough_devices: resource.required_passthrough_devices.clone(),
                min_placement_weight: None,
            };

            let balancer = ClusterBalancer::new(Default::default());
            if let Some(target) = balancer.find_best_node(&known_candidates, &vm) {
                return Ok(Some(target));
            }

            // No known-resource node was architecture-compatible, had every
            // required passthrough device available, or had room. Fall
            // through to the unknown-resource nodes below ONLY IF they are
            // not ruled out by name lookup (we simply have no data, so we
            // can't rule them out, but we also must not silently pick one
            // over an incompatible known node -- this mirrors "no resources
            // anywhere" semantics for the known set while still allowing
            // legacy behavior for nodes with no monitoring data at all).
        }

        // Legacy fallback: first available node with no resource/architecture
        // data at all (keeps old behavior for callers/tests that don't feed
        // node_resources into the HA manager).
        if let Some(node) = unknown_candidates.into_iter().next() {
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
    pub async fn get_events(&self, vm_id: Option<&str>, limit: Option<usize>) -> Vec<HaEvent> {
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
        vm_id: &str,
        new_state: HaState,
        node: Option<String>,
    ) -> Result<()> {
        let mut resources = self.resources.write().await;

        let resource = resources.get_mut(vm_id).ok_or_else(|| {
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

        drop(resources);
        self.sync_resource(vm_id).await;
        Ok(())
    }
}

/// One VM the cluster has decided to restart on another node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Relocation {
    pub vm_id: String,
    pub from: String,
    pub target: String,
    /// The resource's `placement_seq` once the move has happened.
    pub seq: u64,
}

impl HaManager {
    /// Decide where every started resource of a dead node restarts, among
    /// the `online` nodes only. Resources that ran out of relocations or
    /// have no eligible node go to `Error` (and an event is logged). Nothing
    /// is moved yet: the caller recovers each plan entry and then calls
    /// [`HaManager::set_placement`].
    pub async fn plan_relocations(&self, failed_node: &str, online: &[String]) -> Vec<Relocation> {
        if !self.is_enabled().await {
            return Vec::new();
        }
        let affected: Vec<HaResource> = self
            .resources
            .read()
            .await
            .values()
            .filter(|r| {
                r.current_node.as_deref() == Some(failed_node)
                    && matches!(r.state, HaState::Started | HaState::Migrating)
            })
            .cloned()
            .collect();
        let mut plan = Vec::new();
        for r in affected {
            let (new_state, target, msg) = if r.relocate_count >= r.max_relocate {
                (
                    Some(HaState::Error),
                    None,
                    format!("exceeded {} relocations", r.max_relocate),
                )
            } else {
                match self.find_best_node_for_migration(&r, Some(online)).await {
                    Ok(Some(t)) => (None, Some(t), String::new()),
                    Ok(None) => (
                        Some(HaState::Error),
                        None,
                        format!("no online node of group {} can run it", r.group),
                    ),
                    Err(e) => (Some(HaState::Error), None, e.to_string()),
                }
            };
            if let Some(st) = new_state {
                let changed = {
                    let mut res = self.resources.write().await;
                    match res.get_mut(&r.vm_id) {
                        Some(x) if x.state != st => {
                            x.state = st.clone();
                            x.last_state_change = Utc::now();
                            true
                        }
                        _ => false,
                    }
                };
                if changed {
                    self.log_event(HaEvent {
                        timestamp: Utc::now(),
                        vm_id: r.vm_id.clone(),
                        event_type: HaEventType::Failed,
                        old_state: r.state.clone(),
                        new_state: st,
                        node: failed_node.to_string(),
                        message: format!("cannot recover from failed node {failed_node}: {msg}"),
                    })
                    .await;
                    self.sync_resource(&r.vm_id).await;
                }
                continue;
            }
            if let Some(target) = target {
                plan.push(Relocation {
                    vm_id: r.vm_id.clone(),
                    from: failed_node.to_string(),
                    target,
                    seq: r.placement_seq + 1,
                });
            }
        }
        plan
    }

    /// Record that `vm_id` now runs on `node` (placement `seq`), started.
    pub async fn set_placement(&self, vm_id: &str, node: &str, seq: u64, relocated: bool) {
        let from = {
            let mut res = self.resources.write().await;
            let Some(r) = res.get_mut(vm_id) else { return };
            if r.placement_seq > seq {
                return;
            }
            let from = r.current_node.clone().unwrap_or_default();
            r.current_node = Some(node.to_string());
            r.placement_seq = seq;
            r.state = HaState::Started;
            r.last_state_change = Utc::now();
            r.restart_count = 0;
            if relocated {
                r.relocate_count += 1;
            }
            from
        };
        if relocated {
            self.log_event(HaEvent {
                timestamp: Utc::now(),
                vm_id: vm_id.to_string(),
                event_type: HaEventType::Migrated,
                old_state: HaState::Started,
                new_state: HaState::Started,
                node: node.to_string(),
                message: format!("recovered on {node} after {from} failed"),
            })
            .await;
        }
        self.sync_resource(vm_id).await;
    }

    /// Resources currently placed on `node`.
    pub async fn resources_on(&self, node: &str) -> Vec<HaResource> {
        self.resources
            .read()
            .await
            .values()
            .filter(|r| r.current_node.as_deref() == Some(node))
            .cloned()
            .collect()
    }

    /// Take over a peer's view of a resource (and its group). Ignored when
    /// the local copy has a newer placement. Returns whether it was applied.
    pub async fn apply_peer_resource(&self, incoming: HaResource, group: Option<HaGroup>) -> bool {
        if let Some(g) = group {
            self.groups.write().await.insert(g.name.clone(), g.clone());
            let _ = config_store::put_opt(&self.store, kinds::HA_GROUP, &g.name, &g).await;
        }
        let vm_id = incoming.vm_id.clone();
        {
            let mut res = self.resources.write().await;
            if let Some(local) = res.get(&vm_id) {
                if local.placement_seq > incoming.placement_seq {
                    return false;
                }
            }
            res.insert(vm_id.clone(), incoming);
        }
        self.sync_resource(&vm_id).await;
        true
    }

    /// Drop resources placed on `node` that are not in `keep` (the node
    /// stopped managing them). Returns the removed ids.
    pub async fn prune_resources_of(&self, node: &str, keep: &[String]) -> Vec<String> {
        let gone: Vec<String> = self
            .resources
            .read()
            .await
            .values()
            .filter(|r| r.current_node.as_deref() == Some(node) && !keep.contains(&r.vm_id))
            .map(|r| r.vm_id.clone())
            .collect();
        for id in &gone {
            self.resources.write().await.remove(id);
            self.sync_resource(id).await;
        }
        gone
    }

    /// Group definition by name.
    pub async fn get_group(&self, name: &str) -> Option<HaGroup> {
        self.groups.read().await.get(name).cloned()
    }

    /// Persist and log a resource-level problem (e.g. recovery refused).
    pub async fn note_failure(&self, vm_id: &str, node: &str, message: String) {
        self.log_event(HaEvent {
            timestamp: Utc::now(),
            vm_id: vm_id.to_string(),
            event_type: HaEventType::Failed,
            old_state: HaState::Started,
            new_state: HaState::Started,
            node: node.to_string(),
            message,
        })
        .await;
    }

    /// Count a failed recovery attempt against the resource's relocation
    /// budget (goes to `Error` once it is used up).
    pub async fn count_failed_relocation(&self, vm_id: &str) {
        {
            let mut res = self.resources.write().await;
            if let Some(r) = res.get_mut(vm_id) {
                r.relocate_count += 1;
                if r.relocate_count >= r.max_relocate {
                    r.state = HaState::Error;
                    r.last_state_change = Utc::now();
                }
            }
        }
        self.sync_resource(vm_id).await;
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
            vm_id: "100".to_string(),
            group: "test-group".to_string(),
            max_restart: 3,
            max_relocate: 2,
            state: HaState::Started,
            architecture: Architecture::X86_64,
            required_passthrough_devices: Vec::new(),
        };
        manager.add_resource(config).await.unwrap();

        let resources = manager.list_resources().await;
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].vm_id, "100");
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
            vm_id: "100".to_string(),
            group: "test-group".to_string(),
            max_restart: 3,
            max_relocate: 2,
            state: HaState::Started,
            architecture: Architecture::X86_64,
            required_passthrough_devices: Vec::new(),
        };
        manager.add_resource(config).await.unwrap();

        // First failure should restart
        let action = manager.handle_vm_failure("100", "node1").await.unwrap();
        assert!(matches!(action, HaAction::Restart));

        // After max restarts, should migrate
        let action = manager.handle_vm_failure("100", "node1").await.unwrap();
        assert!(matches!(action, HaAction::Restart));

        let action = manager.handle_vm_failure("100", "node1").await.unwrap();
        assert!(matches!(action, HaAction::Restart));

        let action = manager.handle_vm_failure("100", "node1").await.unwrap();
        assert!(matches!(action, HaAction::Migrate(_)));
    }

    fn make_node_resources(name: &str, arch: Architecture) -> NodeResources {
        make_node_resources_with_devices(name, arch, Vec::new())
    }

    fn make_node_resources_with_devices(
        name: &str,
        arch: Architecture,
        available_passthrough_devices: Vec<PassthroughDeviceRequirement>,
    ) -> NodeResources {
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
            available_passthrough_devices,
            placement_weight: 100,
        }
    }

    fn gpu_a(device_name: &str) -> PassthroughDeviceRequirement {
        PassthroughDeviceRequirement {
            vendor_id: "10de".to_string(),
            device_id: "2204".to_string(),
            device_name: device_name.to_string(),
        }
    }

    fn gpu_b(device_name: &str) -> PassthroughDeviceRequirement {
        PassthroughDeviceRequirement {
            vendor_id: "1002".to_string(),
            device_id: "73bf".to_string(),
            device_name: device_name.to_string(),
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
            vm_id: "300".to_string(),
            group: "mixed-group".to_string(),
            max_restart: 0,
            max_relocate: 3,
            state: HaState::Started,
            architecture: Architecture::X86_64,
            required_passthrough_devices: Vec::new(),
        };
        manager.add_resource(config).await.unwrap();
        manager
            .update_resource_state("300", HaState::Started, Some("x86-node".to_string()))
            .await
            .unwrap();

        // x86-node is the VM's current node, so failover must pick from
        // {arm-node, riscv-node}. Since arm-node can emulate x86_64 but
        // riscv-node cannot run x86_64 at all, riscv-node must never be
        // selected.
        let migrated = manager.handle_node_failure("x86-node").await.unwrap();
        assert_eq!(migrated, vec!["300".to_string()]);

        let resource = manager.get_resource("300").await.unwrap();
        assert_eq!(resource.state, HaState::Migrating);

        let events = manager.get_events(Some("300"), None).await;
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
            vm_id: "301".to_string(),
            group: "mixed-group".to_string(),
            max_restart: 0,
            max_relocate: 3,
            state: HaState::Started,
            architecture: Architecture::Riscv64,
            required_passthrough_devices: Vec::new(),
        };
        manager.add_resource(config).await.unwrap();
        manager
            .update_resource_state("301", HaState::Started, Some("riscv-node".to_string()))
            .await
            .unwrap();

        let migrated = manager.handle_node_failure("riscv-node").await.unwrap();
        assert_eq!(migrated, vec!["301".to_string()]);

        let events = manager.get_events(Some("301"), None).await;
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
            vm_id: "302".to_string(),
            group: "mixed-group".to_string(),
            max_restart: 0,
            max_relocate: 3,
            state: HaState::Started,
            architecture: Architecture::Riscv64,
            required_passthrough_devices: Vec::new(),
        };
        manager.add_resource(config).await.unwrap();
        manager
            .update_resource_state("302", HaState::Started, Some("riscv-node".to_string()))
            .await
            .unwrap();

        // Only remaining candidate is x86-node, which cannot run riscv64 at
        // all -- there is no legal migration target.
        let migrated = manager.handle_node_failure("riscv-node").await.unwrap();
        assert!(migrated.is_empty());

        let resource = manager.get_resource("302").await.unwrap();
        assert_eq!(resource.state, HaState::Error);
    }

    /// A VM with a GPU passed through must only migrate to a node that has
    /// the same GPU model (vendor:device ID) available -- never to a node
    /// lacking that hardware, even if that node is otherwise idle and
    /// architecture-compatible.
    #[tokio::test]
    async fn test_failover_gpu_vm_only_migrates_to_node_with_matching_gpu() {
        let manager = HaManager::new();
        manager.enable().await;

        let group = HaGroup {
            name: "gpu-group".to_string(),
            nodes: vec![
                "node-with-gpu".to_string(),
                "node-without-gpu".to_string(),
                "node-fails".to_string(),
            ],
            restricted: false,
            no_failback: false,
        };
        manager.add_group(group).await.unwrap();

        manager
            .update_node_resources(make_node_resources_with_devices(
                "node-fails",
                Architecture::X86_64,
                Vec::new(),
            ))
            .await;
        manager
            .update_node_resources(make_node_resources_with_devices(
                "node-without-gpu",
                Architecture::X86_64,
                Vec::new(),
            ))
            .await;
        manager
            .update_node_resources(make_node_resources_with_devices(
                "node-with-gpu",
                Architecture::X86_64,
                vec![gpu_a("NVIDIA RTX A6000")],
            ))
            .await;

        let config = HaConfig {
            vm_id: "400".to_string(),
            group: "gpu-group".to_string(),
            max_restart: 0,
            max_relocate: 3,
            state: HaState::Started,
            architecture: Architecture::X86_64,
            required_passthrough_devices: vec![gpu_a("NVIDIA RTX A6000")],
        };
        manager.add_resource(config).await.unwrap();
        manager
            .update_resource_state("400", HaState::Started, Some("node-fails".to_string()))
            .await
            .unwrap();

        // Only node-with-gpu actually has the required GPU; node-without-gpu
        // must never be selected even though it is architecture-compatible
        // and idle.
        let migrated = manager.handle_node_failure("node-fails").await.unwrap();
        assert_eq!(migrated, vec!["400".to_string()]);

        let events = manager.get_events(Some("400"), None).await;
        let migrate_event = events
            .iter()
            .find(|e| matches!(e.event_type, HaEventType::Migrated))
            .expect("expected a migration event");
        assert_ne!(migrate_event.node, "node-without-gpu");
        assert_eq!(migrate_event.node, "node-with-gpu");
    }

    /// A node whose matching GPU model is already in_use by another VM (and
    /// therefore absent from its `available_passthrough_devices` snapshot)
    /// must NOT be selected as a failover target -- "has the right hardware"
    /// is not enough, it must currently be free.
    #[tokio::test]
    async fn test_failover_gpu_vm_never_migrates_to_node_with_gpu_already_in_use() {
        let manager = HaManager::new();
        manager.enable().await;

        let group = HaGroup {
            name: "gpu-group".to_string(),
            nodes: vec![
                "node-gpu-busy".to_string(),
                "node-gpu-free".to_string(),
                "node-fails".to_string(),
            ],
            restricted: false,
            no_failback: false,
        };
        manager.add_group(group).await.unwrap();

        manager
            .update_node_resources(make_node_resources_with_devices(
                "node-fails",
                Architecture::X86_64,
                Vec::new(),
            ))
            .await;
        // This node physically has the GPU model, but it's already consumed
        // by another VM, so the live snapshot reports zero AVAILABLE units.
        manager
            .update_node_resources(make_node_resources_with_devices(
                "node-gpu-busy",
                Architecture::X86_64,
                Vec::new(),
            ))
            .await;
        manager
            .update_node_resources(make_node_resources_with_devices(
                "node-gpu-free",
                Architecture::X86_64,
                vec![gpu_a("NVIDIA RTX A6000")],
            ))
            .await;

        let config = HaConfig {
            vm_id: "401".to_string(),
            group: "gpu-group".to_string(),
            max_restart: 0,
            max_relocate: 3,
            state: HaState::Started,
            architecture: Architecture::X86_64,
            required_passthrough_devices: vec![gpu_a("NVIDIA RTX A6000")],
        };
        manager.add_resource(config).await.unwrap();
        manager
            .update_resource_state("401", HaState::Started, Some("node-fails".to_string()))
            .await
            .unwrap();

        let migrated = manager.handle_node_failure("node-fails").await.unwrap();
        assert_eq!(migrated, vec!["401".to_string()]);

        let events = manager.get_events(Some("401"), None).await;
        let migrate_event = events
            .iter()
            .find(|e| matches!(e.event_type, HaEventType::Migrated))
            .expect("expected a migration event");
        assert_ne!(migrate_event.node, "node-gpu-busy");
        assert_eq!(migrate_event.node, "node-gpu-free");
    }

    /// If NO node has the required GPU currently available, failover must
    /// cleanly transition the resource to HaState::Error -- never silently
    /// migrate it to a node lacking the hardware (it would simply fail to
    /// boot there, discovered much later and worse).
    #[tokio::test]
    async fn test_failover_gpu_vm_errors_when_no_matching_gpu_available() {
        let manager = HaManager::new();
        manager.enable().await;

        let group = HaGroup {
            name: "gpu-group".to_string(),
            nodes: vec!["node-fails".to_string(), "node-no-gpu".to_string()],
            restricted: false,
            no_failback: false,
        };
        manager.add_group(group).await.unwrap();

        manager
            .update_node_resources(make_node_resources_with_devices(
                "node-fails",
                Architecture::X86_64,
                vec![gpu_a("NVIDIA RTX A6000")],
            ))
            .await;
        manager
            .update_node_resources(make_node_resources_with_devices(
                "node-no-gpu",
                Architecture::X86_64,
                Vec::new(),
            ))
            .await;

        let config = HaConfig {
            vm_id: "402".to_string(),
            group: "gpu-group".to_string(),
            max_restart: 0,
            max_relocate: 3,
            state: HaState::Started,
            architecture: Architecture::X86_64,
            required_passthrough_devices: vec![gpu_a("NVIDIA RTX A6000")],
        };
        manager.add_resource(config).await.unwrap();
        manager
            .update_resource_state("402", HaState::Started, Some("node-fails".to_string()))
            .await
            .unwrap();

        // The only remaining candidate (node-no-gpu) lacks the GPU entirely
        // -- there is no legal migration target.
        let migrated = manager.handle_node_failure("node-fails").await.unwrap();
        assert!(migrated.is_empty());

        let resource = manager.get_resource("402").await.unwrap();
        assert_eq!(resource.state, HaState::Error);
    }

    /// Mixed-filter composition test: a node that IS architecture-compatible
    /// but LACKS the required passthrough device must still be excluded --
    /// proving the architecture filter and the passthrough-device filter
    /// compose correctly (both must pass), not that either one alone is
    /// sufficient.
    #[tokio::test]
    async fn test_failover_excludes_arch_compatible_node_missing_required_gpu() {
        let manager = HaManager::new();
        manager.enable().await;

        let group = HaGroup {
            name: "mixed-group".to_string(),
            nodes: vec![
                "x86-node-fails".to_string(),
                "x86-node-no-gpu".to_string(),
                "arm-node-with-gpu".to_string(),
            ],
            restricted: false,
            no_failback: false,
        };
        manager.add_group(group).await.unwrap();

        manager
            .update_node_resources(make_node_resources_with_devices(
                "x86-node-fails",
                Architecture::X86_64,
                vec![gpu_b("AMD RX 6800")],
            ))
            .await;
        // Architecture-compatible (x86_64 == x86_64) but has no GPU at all.
        manager
            .update_node_resources(make_node_resources_with_devices(
                "x86-node-no-gpu",
                Architecture::X86_64,
                Vec::new(),
            ))
            .await;
        // Has the matching GPU model, but is riscv64-only -- riscv64 can
        // never run an x86_64 guest (see `Architecture::can_run`). Using a
        // riscv64 node here (rather than aarch64, which CAN emulate x86_64)
        // keeps this node architecture-incompatible on its own axis, so the
        // test below proves the real point: x86-node-no-gpu alone (arch-
        // compatible but missing the device) is excluded by the device
        // filter, not just swept up by an unrelated architecture mismatch.
        manager
            .update_node_resources(make_node_resources_with_devices(
                "arm-node-with-gpu",
                Architecture::Riscv64,
                vec![gpu_b("AMD RX 6800")],
            ))
            .await;

        let config = HaConfig {
            vm_id: "403".to_string(),
            group: "mixed-group".to_string(),
            max_restart: 0,
            max_relocate: 3,
            state: HaState::Started,
            architecture: Architecture::X86_64,
            required_passthrough_devices: vec![gpu_b("AMD RX 6800")],
        };
        manager.add_resource(config).await.unwrap();
        manager
            .update_resource_state("403", HaState::Started, Some("x86-node-fails".to_string()))
            .await
            .unwrap();

        // x86-node-no-gpu is architecture-compatible but missing the GPU;
        // arm-node-with-gpu (actually riscv64) has the GPU but is
        // architecture-incompatible. Neither filter alone would reject both
        // -- only their composition (AND) correctly leaves zero valid
        // candidates, proving the two checks compose rather than one
        // overriding the other.
        let migrated = manager.handle_node_failure("x86-node-fails").await.unwrap();
        assert!(migrated.is_empty());

        let resource = manager.get_resource("403").await.unwrap();
        assert_eq!(resource.state, HaState::Error);
    }

    async fn manager_with_vm_on(node: &str, max_relocate: u32) -> HaManager {
        let m = HaManager::new();
        m.add_group(HaGroup {
            name: "g".into(),
            nodes: vec!["n1".into(), "n2".into(), "n3".into()],
            restricted: false,
            no_failback: false,
        })
        .await
        .unwrap();
        m.add_resource(HaConfig {
            vm_id: "vm1".into(),
            group: "g".into(),
            max_restart: 3,
            max_relocate,
            state: HaState::Started,
            architecture: Architecture::X86_64,
            required_passthrough_devices: Vec::new(),
        })
        .await
        .unwrap();
        m.update_resource_state("vm1", HaState::Started, Some(node.into()))
            .await
            .unwrap();
        m.enable().await;
        m
    }

    #[tokio::test]
    async fn plan_relocations_only_uses_online_nodes_and_bumps_seq() {
        let m = manager_with_vm_on("n1", 3).await;
        // n2 is offline too: the only online candidate is n3.
        let plan = m.plan_relocations("n1", &["n3".to_string()]).await;
        assert_eq!(
            plan,
            vec![Relocation {
                vm_id: "vm1".into(),
                from: "n1".into(),
                target: "n3".into(),
                seq: 1
            }]
        );
        // Planning moves nothing by itself.
        assert_eq!(
            m.get_resource("vm1").await.unwrap().current_node.as_deref(),
            Some("n1")
        );
        m.set_placement("vm1", "n3", 1, true).await;
        let r = m.get_resource("vm1").await.unwrap();
        assert_eq!(r.current_node.as_deref(), Some("n3"));
        assert_eq!((r.placement_seq, r.relocate_count), (1, 1));
        assert!(m
            .get_events(Some("vm1"), None)
            .await
            .iter()
            .any(|e| e.message.contains("recovered on n3")));
    }

    #[tokio::test]
    async fn plan_relocations_without_online_candidate_errors_the_resource() {
        let m = manager_with_vm_on("n1", 3).await;
        assert!(m.plan_relocations("n1", &[]).await.is_empty());
        assert_eq!(m.get_resource("vm1").await.unwrap().state, HaState::Error);
        // A resource in Error is not planned again.
        assert!(m
            .plan_relocations("n1", &["n2".to_string()])
            .await
            .is_empty());
    }

    #[tokio::test]
    async fn plan_relocations_respects_max_relocate_and_disabled_ha() {
        let m = manager_with_vm_on("n1", 1).await;
        m.set_placement("vm1", "n1", 0, true).await; // one relocation used
        assert!(m
            .plan_relocations("n1", &["n2".to_string()])
            .await
            .is_empty());
        assert_eq!(m.get_resource("vm1").await.unwrap().state, HaState::Error);
        let m = manager_with_vm_on("n1", 3).await;
        m.disable().await;
        assert!(m
            .plan_relocations("n1", &["n2".to_string()])
            .await
            .is_empty());
    }

    #[tokio::test]
    async fn stale_placement_from_a_returning_node_is_rejected() {
        let m = manager_with_vm_on("n2", 3).await;
        m.set_placement("vm1", "n2", 1, true).await;
        let mut stale = m.get_resource("vm1").await.unwrap();
        stale.current_node = Some("n1".into());
        stale.placement_seq = 0;
        assert!(!m.apply_peer_resource(stale, None).await);
        assert_eq!(
            m.get_resource("vm1").await.unwrap().current_node.as_deref(),
            Some("n2")
        );
        // The newer view wins on the node that was replaced.
        let m1 = manager_with_vm_on("n1", 3).await;
        let newer = m.get_resource("vm1").await.unwrap();
        assert!(m1.apply_peer_resource(newer, None).await);
        assert_eq!(
            m1.get_resource("vm1")
                .await
                .unwrap()
                .current_node
                .as_deref(),
            Some("n2")
        );
    }

    #[tokio::test]
    async fn prune_drops_resources_the_owner_stopped_managing() {
        let m = manager_with_vm_on("n1", 3).await;
        assert!(m.prune_resources_of("n2", &[]).await.is_empty());
        assert_eq!(
            m.prune_resources_of("n1", &[]).await,
            vec!["vm1".to_string()]
        );
        assert!(m.list_resources().await.is_empty());
    }
}

/// Persistence through the config store (moved from db/config_store.rs so
/// the database layer no longer depends on this module).
#[cfg(test)]
#[allow(unused_imports)]
mod config_store_persistence_tests {
    use crate::db::config_store::{kinds, ConfigStore};
    use crate::db::Database;

    async fn store() -> (tempfile::TempDir, Database, ConfigStore) {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}", dir.path().join("t.db").display());
        let db = Database::new(&url).await.unwrap();
        db.migrate().await.unwrap();
        let s = ConfigStore::new(&db);
        (dir, db, s)
    }

    #[tokio::test]
    async fn ha_state_transitions_and_enabled_flag_persist() {
        use crate::ha::{HaConfig, HaGroup, HaManager, HaState};
        let (_dir, db, _s) = store().await;
        let db = std::sync::Arc::new(db);
        {
            let ha = HaManager::with_database(db.clone()).await.unwrap();
            ha.enable().await;
            ha.add_group(HaGroup {
                name: "g".into(),
                nodes: vec!["n1".into(), "n2".into()],
                restricted: false,
                no_failback: false,
            })
            .await
            .unwrap();
            ha.add_resource(HaConfig {
                vm_id: "vm1".into(),
                group: "g".into(),
                max_restart: 3,
                max_relocate: 1,
                state: HaState::Started,
                architecture: Default::default(),
                required_passthrough_devices: Default::default(),
            })
            .await
            .unwrap();
            ha.update_resource_state("vm1", HaState::Started, Some("n1".into()))
                .await
                .unwrap();
            // A VM failure bumps the restart counter; that must persist too.
            ha.handle_vm_failure("vm1", "n1").await.unwrap();
        }
        let ha = HaManager::with_database(db).await.unwrap();
        assert!(ha.is_enabled().await);
        assert_eq!(ha.list_groups().await.len(), 1);
        let r = ha.get_resource("vm1").await.expect("resource reloaded");
        assert_eq!(r.current_node.as_deref(), Some("n1"));
        assert_eq!(r.restart_count, 1);
    }
}
