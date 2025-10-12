//! High Availability (HA) Manager for Horcrux
//!
//! Provides automatic failover and resource management for VMs across cluster nodes

#![allow(dead_code)]

use horcrux_common::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

/// HA resource state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HaState {
    Started,    // Resource is running
    Stopped,    // Resource is intentionally stopped
    Migrating,  // Resource is being migrated
    Error,      // Resource failed and needs attention
    Disabled,   // HA disabled for this resource
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
}

/// HA group configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HaGroup {
    pub name: String,
    pub nodes: Vec<String>,
    pub restricted: bool,   // Only use nodes in group
    pub no_failback: bool,  // Don't migrate back to preferred node
}

/// HA resource configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HaConfig {
    pub vm_id: u32,
    pub group: String,
    pub max_restart: u32,  // Max restart attempts on same node
    pub max_relocate: u32, // Max relocations to other nodes
    pub state: HaState,    // Requested state
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
}

impl HaManager {
    pub fn new() -> Self {
        Self {
            resources: Arc::new(RwLock::new(HashMap::new())),
            groups: Arc::new(RwLock::new(HashMap::new())),
            events: Arc::new(RwLock::new(Vec::new())),
            enabled: Arc::new(RwLock::new(false)),
        }
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
            return Err(horcrux_common::Error::System(
                format!("HA group {} already exists", group.name)
            ));
        }

        tracing::info!("Adding HA group: {} with nodes {:?}", group.name, group.nodes);
        groups.insert(group.name.clone(), group);
        Ok(())
    }

    /// Remove HA group
    pub async fn remove_group(&self, name: &str) -> Result<()> {
        // Check if any resources use this group
        let resources = self.resources.read().await;
        let using_group = resources.values()
            .any(|r| r.group == name);

        if using_group {
            return Err(horcrux_common::Error::System(
                format!("Cannot remove group {}: resources still using it", name)
            ));
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
                return Err(horcrux_common::Error::System(
                    format!("HA group {} does not exist", config.group)
                ));
            }
        }

        let mut resources = self.resources.write().await;

        if resources.contains_key(&config.vm_id) {
            return Err(horcrux_common::Error::System(
                format!("VM {} already has HA enabled", config.vm_id)
            ));
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
        };

        tracing::info!("Added HA resource: VM {} to group {}", config.vm_id, config.group);
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
            Err(horcrux_common::Error::System(
                format!("VM {} not managed by HA", vm_id)
            ))
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
                    vm_id, resource.max_relocate
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
                }).await;

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
                message: format!("Restart attempt {}/{}", resource.restart_count, resource.max_restart),
            }).await;

            tracing::info!("Attempting restart {}/{} for VM {}",
                resource.restart_count, resource.max_restart, vm_id);

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
                }).await;

                tracing::info!("Migrating VM {} to {} after restart failures", vm_id, target);
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
        }).await;

        tracing::error!("VM {} exceeded all recovery limits, setting to error state", vm_id);
        Ok(HaAction::None)
    }

    /// Find best node for VM migration
    async fn find_best_node_for_migration(&self, resource: &HaResource) -> Result<Option<String>> {
        let groups = self.groups.read().await;

        let group = groups.get(&resource.group).ok_or_else(|| {
            horcrux_common::Error::System(format!("HA group {} not found", resource.group))
        })?;

        // Prefer the preferred node if available
        if let Some(ref preferred) = resource.preferred_node {
            if group.nodes.contains(preferred) {
                return Ok(Some(preferred.clone()));
            }
        }

        // Otherwise pick first available node (not current node)
        for node in &group.nodes {
            if Some(node.as_str()) != resource.current_node.as_deref() {
                return Ok(Some(node.clone()));
            }
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
    pub async fn update_resource_state(&self, vm_id: u32, new_state: HaState, node: Option<String>) -> Result<()> {
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

        tracing::info!("Updated HA resource VM {} state: {:?} -> {:?}", vm_id, old_state, new_state);

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
}
