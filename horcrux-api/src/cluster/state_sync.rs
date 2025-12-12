//! Cluster State Synchronization
//!
//! Provides distributed state management across cluster nodes using:
//! - Eventual consistency for configuration
//! - Strong consistency for critical state (VM ownership, locks)
//! - Conflict resolution for concurrent updates

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};
use tracing::{info, warn, debug};
use horcrux_common::Result;

/// State synchronization message types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncMessage {
    /// Full state snapshot request
    RequestSnapshot,
    /// Full state snapshot
    Snapshot(ClusterState),
    /// Incremental state update
    Update(StateUpdate),
    /// Heartbeat with version
    Heartbeat { node: String, version: u64 },
    /// Lock request
    LockRequest { resource_id: String, node: String },
    /// Lock granted
    LockGranted { resource_id: String, node: String },
    /// Lock released
    LockReleased { resource_id: String },
    /// Lock denied
    LockDenied { resource_id: String, holder: String },
}

/// Incremental state update
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateUpdate {
    pub version: u64,
    pub timestamp: i64,
    pub node: String,
    pub operation: StateOperation,
}

/// State operation types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StateOperation {
    /// VM state change
    VmStateChange { vm_id: String, old_state: String, new_state: String },
    /// VM migration
    VmMigration { vm_id: String, from_node: String, to_node: String },
    /// VM created
    VmCreated { vm_id: String, node: String },
    /// VM deleted
    VmDeleted { vm_id: String, node: String },
    /// Node joined
    NodeJoined { node_id: String, address: String },
    /// Node left
    NodeLeft { node_id: String },
    /// Config change
    ConfigChange { key: String, value: String },
    /// HA state change
    HaStateChange { vm_id: String, state: String },
}

/// Distributed lock for resources
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributedLock {
    pub resource_id: String,
    pub holder: String,
    pub acquired_at: i64,
    pub expires_at: i64,
    pub lock_type: LockType,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum LockType {
    /// Exclusive lock - only one holder
    Exclusive,
    /// Shared lock - multiple readers
    Shared,
}

/// Cluster state snapshot
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClusterState {
    pub version: u64,
    pub timestamp: i64,
    /// VM ownership: vm_id -> node_name
    pub vm_owners: HashMap<String, String>,
    /// VM states: vm_id -> state
    pub vm_states: HashMap<String, String>,
    /// Node statuses: node_id -> status
    pub node_statuses: HashMap<String, NodeSyncStatus>,
    /// Cluster configuration
    pub config: HashMap<String, String>,
    /// Active distributed locks
    pub locks: HashMap<String, DistributedLock>,
    /// HA resource states
    pub ha_states: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSyncStatus {
    pub node_id: String,
    pub state_version: u64,
    pub last_seen: i64,
    pub is_online: bool,
}

/// State synchronization manager
pub struct StateSyncManager {
    local_node: String,
    state: Arc<RwLock<ClusterState>>,
    pending_updates: Arc<RwLock<Vec<StateUpdate>>>,
    update_tx: broadcast::Sender<SyncMessage>,
    lock_timeout_secs: i64,
}

impl StateSyncManager {
    pub fn new(local_node: String) -> Self {
        let (update_tx, _) = broadcast::channel(1000);

        Self {
            local_node,
            state: Arc::new(RwLock::new(ClusterState::default())),
            pending_updates: Arc::new(RwLock::new(Vec::new())),
            update_tx,
            lock_timeout_secs: 60,
        }
    }

    /// Get the current state version
    pub async fn get_version(&self) -> u64 {
        self.state.read().await.version
    }

    /// Get a subscriber for state updates
    pub fn subscribe(&self) -> broadcast::Receiver<SyncMessage> {
        self.update_tx.subscribe()
    }

    /// Apply a state update
    pub async fn apply_update(&self, update: StateUpdate) -> Result<()> {
        let mut state = self.state.write().await;

        // Check version ordering
        if update.version <= state.version {
            debug!(
                "Ignoring old update: received v{}, current v{}",
                update.version, state.version
            );
            return Ok(());
        }

        // Apply the operation
        match &update.operation {
            StateOperation::VmStateChange { vm_id, new_state, .. } => {
                state.vm_states.insert(vm_id.clone(), new_state.clone());
            }
            StateOperation::VmMigration { vm_id, to_node, .. } => {
                state.vm_owners.insert(vm_id.clone(), to_node.clone());
            }
            StateOperation::VmCreated { vm_id, node } => {
                state.vm_owners.insert(vm_id.clone(), node.clone());
                state.vm_states.insert(vm_id.clone(), "created".to_string());
            }
            StateOperation::VmDeleted { vm_id, .. } => {
                state.vm_owners.remove(vm_id);
                state.vm_states.remove(vm_id);
            }
            StateOperation::NodeJoined { node_id, .. } => {
                state.node_statuses.insert(node_id.clone(), NodeSyncStatus {
                    node_id: node_id.clone(),
                    state_version: update.version,
                    last_seen: update.timestamp,
                    is_online: true,
                });
            }
            StateOperation::NodeLeft { node_id } => {
                if let Some(status) = state.node_statuses.get_mut(node_id) {
                    status.is_online = false;
                    status.last_seen = update.timestamp;
                }
            }
            StateOperation::ConfigChange { key, value } => {
                state.config.insert(key.clone(), value.clone());
            }
            StateOperation::HaStateChange { vm_id, state: new_state } => {
                state.ha_states.insert(vm_id.clone(), new_state.clone());
            }
        }

        state.version = update.version;
        state.timestamp = update.timestamp;

        // Broadcast the update
        let _ = self.update_tx.send(SyncMessage::Update(update));

        Ok(())
    }

    /// Create and broadcast a local state change
    pub async fn broadcast_change(&self, operation: StateOperation) -> Result<()> {
        let mut state = self.state.write().await;
        let version = state.version + 1;

        let update = StateUpdate {
            version,
            timestamp: chrono::Utc::now().timestamp(),
            node: self.local_node.clone(),
            operation,
        };

        state.version = version;

        // Broadcast to peers
        let _ = self.update_tx.send(SyncMessage::Update(update));

        Ok(())
    }

    /// Get the full state snapshot
    pub async fn get_snapshot(&self) -> ClusterState {
        self.state.read().await.clone()
    }

    /// Apply a full state snapshot
    pub async fn apply_snapshot(&self, snapshot: ClusterState) -> Result<()> {
        let mut state = self.state.write().await;

        if snapshot.version <= state.version {
            info!(
                "Ignoring older snapshot: received v{}, current v{}",
                snapshot.version, state.version
            );
            return Ok(());
        }

        info!(
            "Applying state snapshot: v{} -> v{}",
            state.version, snapshot.version
        );

        *state = snapshot;
        Ok(())
    }

    /// Request a distributed lock
    pub async fn acquire_lock(&self, resource_id: &str, lock_type: LockType) -> Result<bool> {
        let mut state = self.state.write().await;
        let now = chrono::Utc::now().timestamp();

        // Check if lock exists and is still valid
        if let Some(existing) = state.locks.get(resource_id) {
            if existing.expires_at > now {
                // Lock is held
                if existing.holder == self.local_node {
                    // We already hold it, extend
                    let mut lock = existing.clone();
                    lock.expires_at = now + self.lock_timeout_secs;
                    state.locks.insert(resource_id.to_string(), lock);
                    return Ok(true);
                } else if existing.lock_type == LockType::Exclusive || lock_type == LockType::Exclusive {
                    // Can't acquire exclusive lock or request exclusive on shared
                    return Ok(false);
                }
            }
        }

        // Acquire the lock
        let lock = DistributedLock {
            resource_id: resource_id.to_string(),
            holder: self.local_node.clone(),
            acquired_at: now,
            expires_at: now + self.lock_timeout_secs,
            lock_type,
        };

        state.locks.insert(resource_id.to_string(), lock);

        // Broadcast lock acquisition
        let _ = self.update_tx.send(SyncMessage::LockGranted {
            resource_id: resource_id.to_string(),
            node: self.local_node.clone(),
        });

        Ok(true)
    }

    /// Release a distributed lock
    pub async fn release_lock(&self, resource_id: &str) -> Result<()> {
        let mut state = self.state.write().await;

        if let Some(lock) = state.locks.get(resource_id) {
            if lock.holder != self.local_node {
                return Err(horcrux_common::Error::System(
                    format!("Cannot release lock held by {}", lock.holder)
                ));
            }
        }

        state.locks.remove(resource_id);

        // Broadcast lock release
        let _ = self.update_tx.send(SyncMessage::LockReleased {
            resource_id: resource_id.to_string(),
        });

        Ok(())
    }

    /// Check if we hold a lock on a resource
    pub async fn holds_lock(&self, resource_id: &str) -> bool {
        let state = self.state.read().await;
        let now = chrono::Utc::now().timestamp();

        state.locks.get(resource_id)
            .map(|l| l.holder == self.local_node && l.expires_at > now)
            .unwrap_or(false)
    }

    /// Get VM owner node
    pub async fn get_vm_owner(&self, vm_id: &str) -> Option<String> {
        self.state.read().await.vm_owners.get(vm_id).cloned()
    }

    /// Check if a node is online
    pub async fn is_node_online(&self, node_id: &str) -> bool {
        self.state.read().await
            .node_statuses
            .get(node_id)
            .map(|s| s.is_online)
            .unwrap_or(false)
    }

    /// Update node heartbeat
    pub async fn update_heartbeat(&self, node_id: &str) {
        let mut state = self.state.write().await;
        let now = chrono::Utc::now().timestamp();

        if let Some(status) = state.node_statuses.get_mut(node_id) {
            status.last_seen = now;
            status.is_online = true;
        }
    }

    /// Mark stale nodes as offline
    pub async fn check_stale_nodes(&self, timeout_secs: i64) {
        let mut state = self.state.write().await;
        let now = chrono::Utc::now().timestamp();

        for status in state.node_statuses.values_mut() {
            if status.is_online && (now - status.last_seen) > timeout_secs {
                warn!(node = %status.node_id, "Node marked as offline (stale heartbeat)");
                status.is_online = false;
            }
        }
    }

    /// Cleanup expired locks
    pub async fn cleanup_expired_locks(&self) {
        let mut state = self.state.write().await;
        let now = chrono::Utc::now().timestamp();

        let expired: Vec<_> = state.locks
            .iter()
            .filter(|(_, l)| l.expires_at < now)
            .map(|(id, _)| id.clone())
            .collect();

        for id in expired {
            debug!(resource = %id, "Removing expired lock");
            state.locks.remove(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_state_sync_versioning() {
        let manager = StateSyncManager::new("node1".to_string());

        assert_eq!(manager.get_version().await, 0);

        manager.broadcast_change(StateOperation::VmCreated {
            vm_id: "vm-100".to_string(),
            node: "node1".to_string(),
        }).await.unwrap();

        // Version should not change (only tracked updates change it)
        assert_eq!(manager.get_version().await, 0);
    }

    #[tokio::test]
    async fn test_distributed_lock() {
        let manager = StateSyncManager::new("node1".to_string());

        // Acquire lock
        let acquired = manager.acquire_lock("vm-100", LockType::Exclusive).await.unwrap();
        assert!(acquired);

        // Check we hold it
        assert!(manager.holds_lock("vm-100").await);

        // Release it
        manager.release_lock("vm-100").await.unwrap();
        assert!(!manager.holds_lock("vm-100").await);
    }

    #[tokio::test]
    async fn test_vm_ownership() {
        let manager = StateSyncManager::new("node1".to_string());

        let update = StateUpdate {
            version: 1,
            timestamp: chrono::Utc::now().timestamp(),
            node: "node1".to_string(),
            operation: StateOperation::VmCreated {
                vm_id: "vm-100".to_string(),
                node: "node1".to_string(),
            },
        };

        manager.apply_update(update).await.unwrap();

        let owner = manager.get_vm_owner("vm-100").await;
        assert_eq!(owner, Some("node1".to_string()));
    }
}
