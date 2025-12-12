//! Cluster Membership Protocol
//!
//! Provides join/leave protocol for cluster nodes including:
//! - Node discovery and registration
//! - Join request validation
//! - Graceful node departure
//! - Node eviction for unresponsive nodes

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};
use tracing::{info, warn, error, debug};
use horcrux_common::Result;
use chrono::{DateTime, Utc};

use super::node::{Node, NodeStatus, Architecture};

/// Join request from a node wanting to join the cluster
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinRequest {
    pub node_name: String,
    pub node_address: String,
    pub node_port: u16,
    pub architecture: Architecture,
    pub api_version: String,
    pub cluster_token: Option<String>,
    pub cpu_cores: u32,
    pub memory_total: u64,
    pub storage_total: u64,
    pub capabilities: Vec<String>,
}

/// Join response from the cluster
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum JoinResponse {
    Accepted {
        node_id: u32,
        cluster_name: String,
        members: Vec<MemberInfo>,
        cluster_token: String,
    },
    Rejected {
        reason: String,
    },
    Pending {
        request_id: String,
        message: String,
    },
}

/// Information about a cluster member
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberInfo {
    pub node_id: u32,
    pub name: String,
    pub address: String,
    pub port: u16,
    pub is_master: bool,
    pub status: NodeStatus,
    pub architecture: Architecture,
    pub joined_at: DateTime<Utc>,
}

/// Leave request types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LeaveReason {
    /// Graceful shutdown
    Graceful,
    /// Maintenance mode
    Maintenance,
    /// User-initiated removal
    Manual,
    /// Evicted by cluster
    Evicted(String),
    /// Lost quorum
    QuorumLost,
}

/// Membership event for broadcasting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MembershipEvent {
    /// Node joined the cluster
    NodeJoined {
        node_id: u32,
        node_name: String,
        address: String,
        timestamp: DateTime<Utc>,
    },
    /// Node left the cluster
    NodeLeft {
        node_id: u32,
        node_name: String,
        reason: LeaveReason,
        timestamp: DateTime<Utc>,
    },
    /// Node status changed
    NodeStatusChanged {
        node_id: u32,
        node_name: String,
        old_status: NodeStatus,
        new_status: NodeStatus,
        timestamp: DateTime<Utc>,
    },
    /// Master election
    MasterElected {
        node_id: u32,
        node_name: String,
        timestamp: DateTime<Utc>,
    },
    /// Quorum state changed
    QuorumChanged {
        has_quorum: bool,
        total_votes: u32,
        active_votes: u32,
        timestamp: DateTime<Utc>,
    },
}

/// Pending join request
#[derive(Debug, Clone)]
pub struct PendingJoin {
    pub request: JoinRequest,
    pub received_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// Cluster membership manager
pub struct MembershipManager {
    cluster_name: Arc<RwLock<Option<String>>>,
    cluster_token: Arc<RwLock<Option<String>>>,
    members: Arc<RwLock<HashMap<u32, MemberInfo>>>,
    pending_joins: Arc<RwLock<HashMap<String, PendingJoin>>>,
    local_node_id: Arc<RwLock<Option<u32>>>,
    master_node_id: Arc<RwLock<Option<u32>>>,
    next_node_id: Arc<RwLock<u32>>,
    event_tx: broadcast::Sender<MembershipEvent>,
    require_approval: bool,
    max_nodes: u32,
}

impl MembershipManager {
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(100);

        Self {
            cluster_name: Arc::new(RwLock::new(None)),
            cluster_token: Arc::new(RwLock::new(None)),
            members: Arc::new(RwLock::new(HashMap::new())),
            pending_joins: Arc::new(RwLock::new(HashMap::new())),
            local_node_id: Arc::new(RwLock::new(None)),
            master_node_id: Arc::new(RwLock::new(None)),
            next_node_id: Arc::new(RwLock::new(1)),
            event_tx,
            require_approval: false,
            max_nodes: 32, // Default max cluster size
        }
    }

    /// Initialize as first node (create cluster)
    pub async fn initialize_cluster(&self, cluster_name: String, local_node: &Node) -> Result<String> {
        let mut name = self.cluster_name.write().await;
        if name.is_some() {
            return Err(horcrux_common::Error::System(
                "Already part of a cluster".to_string()
            ));
        }

        // Generate cluster token
        let token = self.generate_token();

        // Set cluster info
        *name = Some(cluster_name.clone());
        drop(name);

        let mut token_guard = self.cluster_token.write().await;
        *token_guard = Some(token.clone());
        drop(token_guard);

        // Set local node as first member and master
        let mut next_id = self.next_node_id.write().await;
        let node_id = *next_id;
        *next_id += 1;
        drop(next_id);

        let member = MemberInfo {
            node_id,
            name: local_node.name.clone(),
            address: local_node.ip.clone(),
            port: 8443, // Default API port
            is_master: true,
            status: NodeStatus::Online,
            architecture: local_node.architecture.clone(),
            joined_at: Utc::now(),
        };

        let mut members = self.members.write().await;
        members.insert(node_id, member.clone());
        drop(members);

        let mut local_id = self.local_node_id.write().await;
        *local_id = Some(node_id);
        drop(local_id);

        let mut master_id = self.master_node_id.write().await;
        *master_id = Some(node_id);
        drop(master_id);

        info!(
            cluster = %cluster_name,
            node_id = node_id,
            node_name = %local_node.name,
            "Cluster initialized"
        );

        // Broadcast event
        let _ = self.event_tx.send(MembershipEvent::NodeJoined {
            node_id,
            node_name: local_node.name.clone(),
            address: local_node.ip.clone(),
            timestamp: Utc::now(),
        });

        Ok(token)
    }

    /// Process join request from another node
    pub async fn handle_join_request(&self, request: JoinRequest) -> JoinResponse {
        // Verify cluster exists
        let cluster_name = self.cluster_name.read().await;
        let cluster_name = match cluster_name.as_ref() {
            Some(n) => n.clone(),
            None => {
                return JoinResponse::Rejected {
                    reason: "No cluster configured on this node".to_string(),
                };
            }
        };
        drop(cluster_name);

        // Verify cluster token if provided
        if let Some(ref provided_token) = request.cluster_token {
            let expected_token = self.cluster_token.read().await;
            if expected_token.as_ref() != Some(provided_token) {
                warn!(node = %request.node_name, "Invalid cluster token");
                return JoinResponse::Rejected {
                    reason: "Invalid cluster token".to_string(),
                };
            }
        } else if self.cluster_token.read().await.is_some() {
            return JoinResponse::Rejected {
                reason: "Cluster token required".to_string(),
            };
        }

        // Check if node name already exists
        let members = self.members.read().await;
        if members.values().any(|m| m.name == request.node_name) {
            return JoinResponse::Rejected {
                reason: format!("Node name {} already exists in cluster", request.node_name),
            };
        }

        // Check cluster size limit
        if members.len() >= self.max_nodes as usize {
            return JoinResponse::Rejected {
                reason: format!("Cluster has reached maximum size ({})", self.max_nodes),
            };
        }
        drop(members);

        // If approval required, queue the request
        if self.require_approval {
            let request_id = format!("join-{}-{}", request.node_name, Utc::now().timestamp());
            let pending = PendingJoin {
                request: request.clone(),
                received_at: Utc::now(),
                expires_at: Utc::now() + chrono::Duration::hours(24),
            };

            let mut pending_joins = self.pending_joins.write().await;
            pending_joins.insert(request_id.clone(), pending);

            info!(
                request_id = %request_id,
                node = %request.node_name,
                "Join request queued for approval"
            );

            return JoinResponse::Pending {
                request_id,
                message: "Join request requires administrator approval".to_string(),
            };
        }

        // Auto-accept: Add node to cluster
        self.accept_join_internal(&request).await
    }

    /// Accept a pending join request
    pub async fn accept_pending_join(&self, request_id: &str) -> Result<JoinResponse> {
        let mut pending = self.pending_joins.write().await;
        let pending_join = pending.remove(request_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("Pending join request {} not found", request_id))
        })?;
        drop(pending);

        Ok(self.accept_join_internal(&pending_join.request).await)
    }

    /// Reject a pending join request
    pub async fn reject_pending_join(&self, request_id: &str, reason: &str) -> Result<()> {
        let mut pending = self.pending_joins.write().await;
        let pending_join = pending.remove(request_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("Pending join request {} not found", request_id))
        })?;

        warn!(
            request_id = request_id,
            node = %pending_join.request.node_name,
            reason = reason,
            "Join request rejected"
        );

        Ok(())
    }

    /// Internal join acceptance logic
    async fn accept_join_internal(&self, request: &JoinRequest) -> JoinResponse {
        let cluster_name = self.cluster_name.read().await.clone().unwrap();
        let cluster_token = self.cluster_token.read().await.clone().unwrap();

        // Assign node ID
        let mut next_id = self.next_node_id.write().await;
        let node_id = *next_id;
        *next_id += 1;
        drop(next_id);

        // Create member info
        let member = MemberInfo {
            node_id,
            name: request.node_name.clone(),
            address: request.node_address.clone(),
            port: request.node_port,
            is_master: false,
            status: NodeStatus::Online,
            architecture: request.architecture.clone(),
            joined_at: Utc::now(),
        };

        // Add to members
        let mut members = self.members.write().await;
        members.insert(node_id, member.clone());

        // Get all members for response
        let member_list: Vec<MemberInfo> = members.values().cloned().collect();
        drop(members);

        info!(
            node_id = node_id,
            node_name = %request.node_name,
            address = %request.node_address,
            "Node joined cluster"
        );

        // Broadcast event
        let _ = self.event_tx.send(MembershipEvent::NodeJoined {
            node_id,
            node_name: request.node_name.clone(),
            address: request.node_address.clone(),
            timestamp: Utc::now(),
        });

        JoinResponse::Accepted {
            node_id,
            cluster_name,
            members: member_list,
            cluster_token,
        }
    }

    /// Handle node leaving the cluster
    pub async fn handle_leave(&self, node_id: u32, reason: LeaveReason) -> Result<()> {
        let mut members = self.members.write().await;

        let member = members.remove(&node_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("Node {} not found in cluster", node_id))
        })?;

        info!(
            node_id = node_id,
            node_name = %member.name,
            reason = ?reason,
            "Node left cluster"
        );

        drop(members);

        // Broadcast event
        let _ = self.event_tx.send(MembershipEvent::NodeLeft {
            node_id,
            node_name: member.name,
            reason,
            timestamp: Utc::now(),
        });

        // Check if master left
        let master_id = self.master_node_id.read().await;
        if *master_id == Some(node_id) {
            drop(master_id);
            self.elect_new_master().await?;
        }

        Ok(())
    }

    /// Initiate graceful leave (for local node)
    pub async fn leave_cluster(&self) -> Result<()> {
        let local_id = self.local_node_id.read().await;
        let node_id = local_id.ok_or_else(|| {
            horcrux_common::Error::System("Not part of a cluster".to_string())
        })?;
        drop(local_id);

        info!(node_id = node_id, "Initiating graceful cluster leave");

        // Remove self from members
        self.handle_leave(node_id, LeaveReason::Graceful).await?;

        // Clear local state
        *self.cluster_name.write().await = None;
        *self.cluster_token.write().await = None;
        *self.local_node_id.write().await = None;
        *self.master_node_id.write().await = None;

        Ok(())
    }

    /// Evict a node from the cluster (master only)
    pub async fn evict_node(&self, node_id: u32, reason: &str) -> Result<()> {
        // Verify we are master
        let local_id = self.local_node_id.read().await;
        let master_id = self.master_node_id.read().await;

        if *local_id != *master_id {
            return Err(horcrux_common::Error::System(
                "Only master can evict nodes".to_string()
            ));
        }
        drop(local_id);
        drop(master_id);

        // Cannot evict self
        let local = self.local_node_id.read().await;
        if *local == Some(node_id) {
            return Err(horcrux_common::Error::System(
                "Cannot evict self from cluster".to_string()
            ));
        }
        drop(local);

        warn!(node_id = node_id, reason = reason, "Evicting node from cluster");

        self.handle_leave(node_id, LeaveReason::Evicted(reason.to_string())).await
    }

    /// Update node status
    pub async fn update_node_status(&self, node_id: u32, status: NodeStatus) -> Result<()> {
        let mut members = self.members.write().await;

        let member = members.get_mut(&node_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("Node {} not found", node_id))
        })?;

        let old_status = member.status.clone();
        if old_status == status {
            return Ok(());
        }

        member.status = status.clone();
        let node_name = member.name.clone();
        drop(members);

        debug!(
            node_id = node_id,
            old_status = ?old_status,
            new_status = ?status,
            "Node status updated"
        );

        // Broadcast event
        let _ = self.event_tx.send(MembershipEvent::NodeStatusChanged {
            node_id,
            node_name,
            old_status,
            new_status: status,
            timestamp: Utc::now(),
        });

        Ok(())
    }

    /// Elect new master when current master leaves or fails
    async fn elect_new_master(&self) -> Result<()> {
        let members = self.members.read().await;

        // Simple election: pick the online node with lowest ID
        let new_master = members.values()
            .filter(|m| m.status == NodeStatus::Online)
            .min_by_key(|m| m.node_id);

        let new_master = match new_master {
            Some(m) => m.clone(),
            None => {
                error!("No online nodes available for master election");
                return Err(horcrux_common::Error::System(
                    "No online nodes available for master election".to_string()
                ));
            }
        };
        drop(members);

        info!(
            node_id = new_master.node_id,
            node_name = %new_master.name,
            "New master elected"
        );

        // Update master status
        let mut master_id = self.master_node_id.write().await;
        *master_id = Some(new_master.node_id);
        drop(master_id);

        let mut members = self.members.write().await;
        // Clear old master flag
        for m in members.values_mut() {
            m.is_master = false;
        }
        // Set new master
        if let Some(m) = members.get_mut(&new_master.node_id) {
            m.is_master = true;
        }
        drop(members);

        // Broadcast event
        let _ = self.event_tx.send(MembershipEvent::MasterElected {
            node_id: new_master.node_id,
            node_name: new_master.name,
            timestamp: Utc::now(),
        });

        Ok(())
    }

    /// Subscribe to membership events
    pub fn subscribe(&self) -> broadcast::Receiver<MembershipEvent> {
        self.event_tx.subscribe()
    }

    /// Get cluster name
    pub async fn get_cluster_name(&self) -> Option<String> {
        self.cluster_name.read().await.clone()
    }

    /// Get all members
    pub async fn get_members(&self) -> Vec<MemberInfo> {
        self.members.read().await.values().cloned().collect()
    }

    /// Get member by ID
    pub async fn get_member(&self, node_id: u32) -> Option<MemberInfo> {
        self.members.read().await.get(&node_id).cloned()
    }

    /// Get local node ID
    pub async fn get_local_node_id(&self) -> Option<u32> {
        *self.local_node_id.read().await
    }

    /// Get master node ID
    pub async fn get_master_node_id(&self) -> Option<u32> {
        *self.master_node_id.read().await
    }

    /// Check if local node is master
    pub async fn is_master(&self) -> bool {
        let local = self.local_node_id.read().await;
        let master = self.master_node_id.read().await;
        *local == *master && local.is_some()
    }

    /// Get pending join requests
    pub async fn get_pending_joins(&self) -> Vec<PendingJoin> {
        self.pending_joins.read().await.values().cloned().collect()
    }

    /// Clean up expired pending requests
    pub async fn cleanup_expired_pending(&self) {
        let mut pending = self.pending_joins.write().await;
        let now = Utc::now();

        let expired: Vec<_> = pending.iter()
            .filter(|(_, p)| p.expires_at < now)
            .map(|(id, _)| id.clone())
            .collect();

        for id in expired {
            debug!(request_id = %id, "Removing expired pending join request");
            pending.remove(&id);
        }
    }

    /// Generate a secure cluster token
    fn generate_token(&self) -> String {
        use std::time::{SystemTime, UNIX_EPOCH};

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        // Simple token generation (in production, use cryptographic random)
        format!("hx-{:x}-{:x}",
            timestamp,
            std::process::id() as u128 ^ timestamp
        )
    }

    /// Set whether join requests require manual approval
    pub fn set_require_approval(&mut self, require: bool) {
        self.require_approval = require;
    }

    /// Set maximum cluster size
    pub fn set_max_nodes(&mut self, max: u32) {
        self.max_nodes = max;
    }
}

impl Default for MembershipManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_initialize_cluster() {
        let manager = MembershipManager::new();

        let node = Node::new_local(1, "node1".to_string(), "192.168.1.1".to_string());
        let token = manager.initialize_cluster("test-cluster".to_string(), &node).await.unwrap();

        assert!(!token.is_empty());
        assert_eq!(manager.get_cluster_name().await, Some("test-cluster".to_string()));
        assert!(manager.is_master().await);

        let members = manager.get_members().await;
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].name, "node1");
    }

    #[tokio::test]
    async fn test_join_request() {
        let manager = MembershipManager::new();

        // Initialize cluster first
        let node = Node::new_local(1, "node1".to_string(), "192.168.1.1".to_string());
        let token = manager.initialize_cluster("test-cluster".to_string(), &node).await.unwrap();

        // Create join request
        let request = JoinRequest {
            node_name: "node2".to_string(),
            node_address: "192.168.1.2".to_string(),
            node_port: 8443,
            architecture: Architecture::X86_64,
            api_version: "1.0".to_string(),
            cluster_token: Some(token),
            cpu_cores: 8,
            memory_total: 16 * 1024 * 1024 * 1024,
            storage_total: 100 * 1024 * 1024 * 1024,
            capabilities: vec!["live_migration".to_string()],
        };

        let response = manager.handle_join_request(request).await;

        match response {
            JoinResponse::Accepted { node_id, members, .. } => {
                assert_eq!(node_id, 2);
                assert_eq!(members.len(), 2);
            }
            _ => panic!("Expected accepted response"),
        }
    }

    #[tokio::test]
    async fn test_invalid_token() {
        let manager = MembershipManager::new();

        let node = Node::new_local(1, "node1".to_string(), "192.168.1.1".to_string());
        manager.initialize_cluster("test-cluster".to_string(), &node).await.unwrap();

        let request = JoinRequest {
            node_name: "node2".to_string(),
            node_address: "192.168.1.2".to_string(),
            node_port: 8443,
            architecture: Architecture::X86_64,
            api_version: "1.0".to_string(),
            cluster_token: Some("invalid-token".to_string()),
            cpu_cores: 8,
            memory_total: 16 * 1024 * 1024 * 1024,
            storage_total: 100 * 1024 * 1024 * 1024,
            capabilities: vec![],
        };

        let response = manager.handle_join_request(request).await;

        match response {
            JoinResponse::Rejected { reason } => {
                assert!(reason.contains("Invalid cluster token"));
            }
            _ => panic!("Expected rejected response"),
        }
    }

    #[tokio::test]
    async fn test_leave_cluster() {
        let manager = MembershipManager::new();

        let node = Node::new_local(1, "node1".to_string(), "192.168.1.1".to_string());
        manager.initialize_cluster("test-cluster".to_string(), &node).await.unwrap();

        manager.leave_cluster().await.unwrap();

        assert!(manager.get_cluster_name().await.is_none());
        assert_eq!(manager.get_members().await.len(), 0);
    }
}
