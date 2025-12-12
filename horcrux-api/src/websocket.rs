///! WebSocket support for real-time updates
///! Provides live VM status updates, metrics streaming, and event notifications

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
    Extension,
};
use futures::{stream::StreamExt, SinkExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};
use crate::middleware::auth::AuthUser;

/// WebSocket event types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum WsEvent {
    /// VM status changed
    VmStatusChanged {
        vm_id: String,
        old_status: String,
        new_status: String,
        timestamp: String,
    },

    /// VM metrics update
    VmMetrics {
        vm_id: String,
        cpu_usage: f64,
        memory_usage: f64,
        disk_read: u64,
        disk_write: u64,
        network_rx: u64,
        network_tx: u64,
        timestamp: String,
    },

    /// Node metrics update
    NodeMetrics {
        hostname: String,
        cpu_usage: f64,
        memory_usage: f64,
        disk_usage: f64,
        load_average: [f64; 3],
        timestamp: String,
    },

    /// VM created
    VmCreated {
        vm_id: String,
        name: String,
        user: String,
        timestamp: String,
    },

    /// VM deleted
    VmDeleted {
        vm_id: String,
        name: String,
        user: String,
        timestamp: String,
    },

    /// Backup completed
    BackupCompleted {
        vm_id: String,
        backup_id: String,
        size_bytes: u64,
        duration_seconds: u64,
        timestamp: String,
    },

    /// Migration started
    MigrationStarted {
        vm_id: String,
        source_node: String,
        target_node: String,
        timestamp: String,
    },

    /// Migration progress
    MigrationProgress {
        vm_id: String,
        progress: u8,
        transferred_bytes: u64,
        total_bytes: u64,
        timestamp: String,
    },

    /// Migration completed
    MigrationCompleted {
        vm_id: String,
        target_node: String,
        duration_seconds: u64,
        timestamp: String,
    },

    /// Alert triggered
    AlertTriggered {
        alert_id: String,
        rule_name: String,
        severity: String,
        target: String,
        message: String,
        timestamp: String,
    },

    /// Alert resolved
    AlertResolved {
        alert_id: String,
        rule_name: String,
        target: String,
        timestamp: String,
    },

    /// Container status changed
    ContainerStatusChanged {
        container_id: String,
        old_status: String,
        new_status: String,
        timestamp: String,
    },

    /// Container deleted
    ContainerDeleted {
        container_id: String,
        timestamp: String,
    },

    /// Generic notification
    Notification {
        level: String,  // info, warning, error
        title: String,
        message: String,
        timestamp: String,
    },

    /// Heartbeat/ping to keep connection alive
    Ping {
        timestamp: String,
    },

    /// Subscription confirmation
    Subscribed {
        topics: Vec<String>,
        timestamp: String,
    },

    /// Error message
    Error {
        code: String,
        message: String,
        timestamp: String,
    },

    // =========================================================================
    // Kubernetes Events
    // =========================================================================

    /// K8s Pod status changed
    K8sPodStatusChanged {
        cluster_id: String,
        namespace: String,
        pod_name: String,
        old_status: String,
        new_status: String,
        timestamp: String,
    },

    /// K8s Deployment scaled
    K8sDeploymentScaled {
        cluster_id: String,
        namespace: String,
        name: String,
        old_replicas: i32,
        new_replicas: i32,
        timestamp: String,
    },

    /// K8s Event (from the cluster)
    K8sEvent {
        cluster_id: String,
        namespace: String,
        event_type: String,
        reason: String,
        message: String,
        involved_object: String,
        timestamp: String,
    },

    /// K8s Pod log line (for streaming)
    K8sPodLogLine {
        cluster_id: String,
        namespace: String,
        pod_name: String,
        container: String,
        line: String,
        timestamp: String,
    },

    /// K8s Exec output
    K8sExecOutput {
        cluster_id: String,
        namespace: String,
        pod_name: String,
        container: String,
        output: String,
        stream: String, // "stdout" or "stderr"
        timestamp: String,
    },

    /// K8s Node status changed
    K8sNodeStatusChanged {
        cluster_id: String,
        node_name: String,
        old_status: String,
        new_status: String,
        timestamp: String,
    },

    /// K8s Cluster connected
    K8sClusterConnected {
        cluster_id: String,
        cluster_name: String,
        api_server: String,
        timestamp: String,
    },

    /// K8s Cluster disconnected
    K8sClusterDisconnected {
        cluster_id: String,
        cluster_name: String,
        reason: Option<String>,
        timestamp: String,
    },

    /// Helm release status changed
    HelmReleaseStatusChanged {
        cluster_id: String,
        namespace: String,
        release_name: String,
        old_status: String,
        new_status: String,
        timestamp: String,
    },
}

/// WebSocket subscription request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsSubscription {
    /// Topics to subscribe to
    pub topics: Vec<String>,
}

/// Available subscription topics
pub const TOPIC_VM_STATUS: &str = "vm:status";
pub const TOPIC_VM_METRICS: &str = "vm:metrics";
pub const TOPIC_NODE_METRICS: &str = "node:metrics";
pub const TOPIC_VM_EVENTS: &str = "vm:events";
pub const TOPIC_BACKUPS: &str = "backups";
pub const TOPIC_MIGRATIONS: &str = "migrations";
pub const TOPIC_ALERTS: &str = "alerts";
pub const TOPIC_NOTIFICATIONS: &str = "notifications";
// Kubernetes topics
pub const TOPIC_K8S_PODS: &str = "k8s:pods";
pub const TOPIC_K8S_DEPLOYMENTS: &str = "k8s:deployments";
pub const TOPIC_K8S_EVENTS: &str = "k8s:events";
pub const TOPIC_K8S_LOGS: &str = "k8s:logs";
pub const TOPIC_K8S_NODES: &str = "k8s:nodes";
pub const TOPIC_K8S_CLUSTERS: &str = "k8s:clusters";
pub const TOPIC_HELM: &str = "helm:releases";

/// WebSocket state
#[derive(Clone)]
pub struct WsState {
    /// Event broadcaster
    pub tx: broadcast::Sender<WsEvent>,
    /// Active connection count
    connection_count: Arc<std::sync::atomic::AtomicUsize>,
}

impl WsState {
    /// Create new WebSocket state
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(1000);
        Self {
            tx,
            connection_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// Get the number of active connections
    pub fn connection_count(&self) -> usize {
        self.connection_count.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Increment connection count
    pub fn increment_connections(&self) {
        self.connection_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// Decrement connection count
    pub fn decrement_connections(&self) {
        self.connection_count.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// Close all connections by broadcasting shutdown
    pub async fn close_all(&self) {
        // Broadcast a shutdown notification
        self.broadcast(WsEvent::Notification {
            level: "info".to_string(),
            title: "Server Shutdown".to_string(),
            message: "Server is shutting down. Please reconnect shortly.".to_string(),
            timestamp: Self::timestamp(),
        });
    }

    /// Broadcast an event to all connected clients
    pub fn broadcast(&self, event: WsEvent) {
        if let Err(e) = self.tx.send(event) {
            warn!("Failed to broadcast WebSocket event: {}", e);
        }
    }

    /// Get current timestamp in ISO 8601 format
    fn timestamp() -> String {
        chrono::Utc::now().to_rfc3339()
    }

    /// Broadcast VM status change
    pub fn broadcast_vm_status(&self, vm_id: String, old_status: String, new_status: String) {
        self.broadcast(WsEvent::VmStatusChanged {
            vm_id,
            old_status,
            new_status,
            timestamp: Self::timestamp(),
        });
    }

    /// Broadcast VM metrics
    pub fn broadcast_vm_metrics(
        &self,
        vm_id: String,
        cpu_usage: f64,
        memory_usage: f64,
        disk_read: u64,
        disk_write: u64,
        network_rx: u64,
        network_tx: u64,
    ) {
        self.broadcast(WsEvent::VmMetrics {
            vm_id,
            cpu_usage,
            memory_usage,
            disk_read,
            disk_write,
            network_rx,
            network_tx,
            timestamp: Self::timestamp(),
        });
    }

    /// Broadcast node metrics
    pub fn broadcast_node_metrics(
        &self,
        hostname: String,
        cpu_usage: f64,
        memory_usage: f64,
        disk_usage: f64,
        load_average: [f64; 3],
    ) {
        self.broadcast(WsEvent::NodeMetrics {
            hostname,
            cpu_usage,
            memory_usage,
            disk_usage,
            load_average,
            timestamp: Self::timestamp(),
        });
    }

    /// Broadcast VM creation
    pub fn broadcast_vm_created(&self, vm_id: String, name: String, user: String) {
        self.broadcast(WsEvent::VmCreated {
            vm_id,
            name,
            user,
            timestamp: Self::timestamp(),
        });
    }

    /// Broadcast VM deletion
    pub fn broadcast_vm_deleted(&self, vm_id: String, name: String, user: String) {
        self.broadcast(WsEvent::VmDeleted {
            vm_id,
            name,
            user,
            timestamp: Self::timestamp(),
        });
    }

    /// Broadcast backup completion
    pub fn broadcast_backup_completed(
        &self,
        vm_id: String,
        backup_id: String,
        size_bytes: u64,
        duration_seconds: u64,
    ) {
        self.broadcast(WsEvent::BackupCompleted {
            vm_id,
            backup_id,
            size_bytes,
            duration_seconds,
            timestamp: Self::timestamp(),
        });
    }

    /// Broadcast migration started
    pub fn broadcast_migration_started(&self, vm_id: String, source_node: String, target_node: String) {
        self.broadcast(WsEvent::MigrationStarted {
            vm_id,
            source_node,
            target_node,
            timestamp: Self::timestamp(),
        });
    }

    /// Broadcast migration progress
    pub fn broadcast_migration_progress(
        &self,
        vm_id: String,
        progress: u8,
        transferred_bytes: u64,
        total_bytes: u64,
    ) {
        self.broadcast(WsEvent::MigrationProgress {
            vm_id,
            progress,
            transferred_bytes,
            total_bytes,
            timestamp: Self::timestamp(),
        });
    }

    /// Broadcast migration completed
    pub fn broadcast_migration_completed(&self, vm_id: String, target_node: String, duration_seconds: u64) {
        self.broadcast(WsEvent::MigrationCompleted {
            vm_id,
            target_node,
            duration_seconds,
            timestamp: Self::timestamp(),
        });
    }

    /// Broadcast alert triggered
    pub fn broadcast_alert_triggered(
        &self,
        alert_id: String,
        rule_name: String,
        severity: String,
        target: String,
        message: String,
    ) {
        self.broadcast(WsEvent::AlertTriggered {
            alert_id,
            rule_name,
            severity,
            target,
            message,
            timestamp: Self::timestamp(),
        });
    }

    /// Broadcast alert resolved
    pub fn broadcast_alert_resolved(&self, alert_id: String, rule_name: String, target: String) {
        self.broadcast(WsEvent::AlertResolved {
            alert_id,
            rule_name,
            target,
            timestamp: Self::timestamp(),
        });
    }

    /// Broadcast notification
    pub fn broadcast_notification(&self, level: String, title: String, message: String) {
        self.broadcast(WsEvent::Notification {
            level,
            title,
            message,
            timestamp: Self::timestamp(),
        });
    }

    // =========================================================================
    // Kubernetes Broadcast Methods
    // =========================================================================

    /// Broadcast K8s pod status change
    pub fn broadcast_k8s_pod_status(
        &self,
        cluster_id: String,
        namespace: String,
        pod_name: String,
        old_status: String,
        new_status: String,
    ) {
        self.broadcast(WsEvent::K8sPodStatusChanged {
            cluster_id,
            namespace,
            pod_name,
            old_status,
            new_status,
            timestamp: Self::timestamp(),
        });
    }

    /// Broadcast K8s deployment scaled
    pub fn broadcast_k8s_deployment_scaled(
        &self,
        cluster_id: String,
        namespace: String,
        name: String,
        old_replicas: i32,
        new_replicas: i32,
    ) {
        self.broadcast(WsEvent::K8sDeploymentScaled {
            cluster_id,
            namespace,
            name,
            old_replicas,
            new_replicas,
            timestamp: Self::timestamp(),
        });
    }

    /// Broadcast K8s event
    pub fn broadcast_k8s_event(
        &self,
        cluster_id: String,
        namespace: String,
        event_type: String,
        reason: String,
        message: String,
        involved_object: String,
    ) {
        self.broadcast(WsEvent::K8sEvent {
            cluster_id,
            namespace,
            event_type,
            reason,
            message,
            involved_object,
            timestamp: Self::timestamp(),
        });
    }

    /// Broadcast K8s pod log line
    pub fn broadcast_k8s_log_line(
        &self,
        cluster_id: String,
        namespace: String,
        pod_name: String,
        container: String,
        line: String,
    ) {
        self.broadcast(WsEvent::K8sPodLogLine {
            cluster_id,
            namespace,
            pod_name,
            container,
            line,
            timestamp: Self::timestamp(),
        });
    }

    /// Broadcast K8s exec output
    pub fn broadcast_k8s_exec_output(
        &self,
        cluster_id: String,
        namespace: String,
        pod_name: String,
        container: String,
        output: String,
        stream: String,
    ) {
        self.broadcast(WsEvent::K8sExecOutput {
            cluster_id,
            namespace,
            pod_name,
            container,
            output,
            stream,
            timestamp: Self::timestamp(),
        });
    }

    /// Broadcast K8s node status change
    pub fn broadcast_k8s_node_status(
        &self,
        cluster_id: String,
        node_name: String,
        old_status: String,
        new_status: String,
    ) {
        self.broadcast(WsEvent::K8sNodeStatusChanged {
            cluster_id,
            node_name,
            old_status,
            new_status,
            timestamp: Self::timestamp(),
        });
    }

    /// Broadcast K8s cluster connected
    pub fn broadcast_k8s_cluster_connected(
        &self,
        cluster_id: String,
        cluster_name: String,
        api_server: String,
    ) {
        self.broadcast(WsEvent::K8sClusterConnected {
            cluster_id,
            cluster_name,
            api_server,
            timestamp: Self::timestamp(),
        });
    }

    /// Broadcast K8s cluster disconnected
    pub fn broadcast_k8s_cluster_disconnected(
        &self,
        cluster_id: String,
        cluster_name: String,
        reason: Option<String>,
    ) {
        self.broadcast(WsEvent::K8sClusterDisconnected {
            cluster_id,
            cluster_name,
            reason,
            timestamp: Self::timestamp(),
        });
    }

    /// Broadcast Helm release status change
    pub fn broadcast_helm_release_status(
        &self,
        cluster_id: String,
        namespace: String,
        release_name: String,
        old_status: String,
        new_status: String,
    ) {
        self.broadcast(WsEvent::HelmReleaseStatusChanged {
            cluster_id,
            namespace,
            release_name,
            old_status,
            new_status,
            timestamp: Self::timestamp(),
        });
    }
}

/// WebSocket handler
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<crate::AppState>>,
    Extension(auth_user): Extension<AuthUser>,
) -> impl IntoResponse {
    info!(
        user = %auth_user.username,
        "WebSocket connection request"
    );

    let ws_state = state.ws_state.clone();
    ws.on_upgrade(move |socket| handle_socket(socket, ws_state, auth_user))
}

/// Handle WebSocket connection
async fn handle_socket(socket: WebSocket, ws_state: Arc<WsState>, auth_user: AuthUser) {
    let (sender, receiver) = socket.split();
    let sender = Arc::new(tokio::sync::Mutex::new(sender));

    // Create subscription for broadcast events
    let rx = ws_state.tx.subscribe();

    // User's subscribed topics (wrapped in Arc<Mutex> for sharing)
    let subscribed_topics = Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));

    // Send welcome message
    let welcome = WsEvent::Notification {
        level: "info".to_string(),
        title: "Connected".to_string(),
        message: format!("WebSocket connection established for user {}", auth_user.username),
        timestamp: chrono::Utc::now().to_rfc3339(),
    };

    if let Ok(msg) = serde_json::to_string(&welcome) {
        if let Err(e) = sender.lock().await.send(Message::Text(msg)).await {
            error!("Failed to send welcome message: {}", e);
            return;
        }
    }

    let username = auth_user.username.clone();

    // Spawn task to handle incoming messages from client
    let recv_task = tokio::spawn(async move {
        let mut receiver = receiver;
        while let Some(Ok(msg)) = receiver.next().await {
            match msg {
                Message::Text(text) => {
                    debug!(user = %username, "Received text message: {}", text);

                    // Try to parse subscription request
                    if let Ok(sub) = serde_json::from_str::<WsSubscription>(&text) {
                        return Some(sub.topics);
                    }
                }
                Message::Close(_) => {
                    info!(user = %username, "Client closed connection");
                    return None;
                }
                Message::Ping(_) => {
                    // Pong is sent automatically by axum
                }
                _ => {}
            }
        }
        None
    });

    let sender_clone = sender.clone();
    let subscribed_topics_clone = subscribed_topics.clone();
    let _username_clone = auth_user.username.clone();

    // Spawn task to send broadcast events to client
    let send_task = tokio::spawn(async move {
        let mut rx = rx;
        let sender = sender_clone;
        let subscribed_topics = subscribed_topics_clone;

        // Heartbeat interval (30 seconds)
        let mut heartbeat_interval = tokio::time::interval(tokio::time::Duration::from_secs(30));

        loop {
            tokio::select! {
                // Heartbeat
                _ = heartbeat_interval.tick() => {
                    let ping = WsEvent::Ping {
                        timestamp: chrono::Utc::now().to_rfc3339(),
                    };

                    if let Ok(msg) = serde_json::to_string(&ping) {
                        if sender.lock().await.send(Message::Text(msg)).await.is_err() {
                            break;
                        }
                    }
                }

                // Broadcast events
                event = rx.recv() => {
                    if let Ok(event) = event {
                        let topics = subscribed_topics.lock().await;

                        // Check if user is subscribed to this event type
                        let should_send = match &event {
                            WsEvent::VmStatusChanged { .. } => topics.contains(&TOPIC_VM_STATUS.to_string()),
                            WsEvent::VmMetrics { .. } => topics.contains(&TOPIC_VM_METRICS.to_string()),
                            WsEvent::NodeMetrics { .. } => topics.contains(&TOPIC_NODE_METRICS.to_string()),
                            WsEvent::VmCreated { .. } | WsEvent::VmDeleted { .. } => {
                                topics.contains(&TOPIC_VM_EVENTS.to_string())
                            }
                            WsEvent::BackupCompleted { .. } => topics.contains(&TOPIC_BACKUPS.to_string()),
                            WsEvent::MigrationStarted { .. }
                            | WsEvent::MigrationProgress { .. }
                            | WsEvent::MigrationCompleted { .. } => {
                                topics.contains(&TOPIC_MIGRATIONS.to_string())
                            }
                            WsEvent::AlertTriggered { .. } | WsEvent::AlertResolved { .. } => {
                                topics.contains(&TOPIC_ALERTS.to_string())
                            }
                            WsEvent::ContainerStatusChanged { .. } | WsEvent::ContainerDeleted { .. } => {
                                topics.contains(&TOPIC_VM_STATUS.to_string()) // Reuse VM status topic for containers
                            }
                            WsEvent::Notification { .. } => topics.contains(&TOPIC_NOTIFICATIONS.to_string()),
                            // Kubernetes events
                            WsEvent::K8sPodStatusChanged { .. } => {
                                topics.contains(&TOPIC_K8S_PODS.to_string())
                            }
                            WsEvent::K8sDeploymentScaled { .. } => {
                                topics.contains(&TOPIC_K8S_DEPLOYMENTS.to_string())
                            }
                            WsEvent::K8sEvent { .. } => {
                                topics.contains(&TOPIC_K8S_EVENTS.to_string())
                            }
                            WsEvent::K8sPodLogLine { .. } => {
                                topics.contains(&TOPIC_K8S_LOGS.to_string())
                            }
                            WsEvent::K8sExecOutput { .. } => {
                                topics.contains(&TOPIC_K8S_LOGS.to_string())
                            }
                            WsEvent::K8sNodeStatusChanged { .. } => {
                                topics.contains(&TOPIC_K8S_NODES.to_string())
                            }
                            WsEvent::K8sClusterConnected { .. }
                            | WsEvent::K8sClusterDisconnected { .. } => {
                                topics.contains(&TOPIC_K8S_CLUSTERS.to_string())
                            }
                            WsEvent::HelmReleaseStatusChanged { .. } => {
                                topics.contains(&TOPIC_HELM.to_string())
                            }
                            // Always send these
                            WsEvent::Ping { .. }
                            | WsEvent::Subscribed { .. }
                            | WsEvent::Error { .. } => true,
                        };

                        drop(topics);

                        if should_send {
                            if let Ok(msg) = serde_json::to_string(&event) {
                                if sender.lock().await.send(Message::Text(msg)).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    // Wait for subscription from client or timeout
    tokio::select! {
        topics = recv_task => {
            if let Ok(Some(topics)) = topics {
                *subscribed_topics.lock().await = topics.clone();

                // Send subscription confirmation
                let subscribed = WsEvent::Subscribed {
                    topics: topics.clone(),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                };

                if let Ok(msg) = serde_json::to_string(&subscribed) {
                    let _ = sender.lock().await.send(Message::Text(msg)).await;
                }

                info!(
                    user = %auth_user.username,
                    topics = ?topics,
                    "Client subscribed to topics"
                );

                // Continue with send task
                let _ = send_task.await;
            } else {
                send_task.abort();
            }
        }
        _ = tokio::time::sleep(tokio::time::Duration::from_secs(10)) => {
            error!(user = %auth_user.username, "Timeout waiting for subscription");

            let error_event = WsEvent::Error {
                code: "SUBSCRIPTION_TIMEOUT".to_string(),
                message: "No subscription received within 10 seconds".to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
            };

            if let Ok(msg) = serde_json::to_string(&error_event) {
                let _ = sender.lock().await.send(Message::Text(msg)).await;
            }

            send_task.abort();
        }
    }

    info!(user = %auth_user.username, "WebSocket connection closed");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ws_state_creation() {
        let state = WsState::new();
        assert_eq!(state.tx.receiver_count(), 0);
    }

    #[test]
    fn test_broadcast_vm_status() {
        let state = WsState::new();
        let mut rx = state.tx.subscribe();

        state.broadcast_vm_status(
            "vm-100".to_string(),
            "stopped".to_string(),
            "running".to_string(),
        );

        if let Ok(event) = rx.try_recv() {
            match event {
                WsEvent::VmStatusChanged { vm_id, old_status, new_status, .. } => {
                    assert_eq!(vm_id, "vm-100");
                    assert_eq!(old_status, "stopped");
                    assert_eq!(new_status, "running");
                }
                _ => panic!("Expected VmStatusChanged event"),
            }
        }
    }

    #[test]
    fn test_broadcast_notification() {
        let state = WsState::new();
        let mut rx = state.tx.subscribe();

        state.broadcast_notification(
            "warning".to_string(),
            "Test Alert".to_string(),
            "This is a test notification".to_string(),
        );

        if let Ok(event) = rx.try_recv() {
            match event {
                WsEvent::Notification { level, title, message, .. } => {
                    assert_eq!(level, "warning");
                    assert_eq!(title, "Test Alert");
                    assert_eq!(message, "This is a test notification");
                }
                _ => panic!("Expected Notification event"),
            }
        }
    }

    #[test]
    fn test_ws_subscription_deserialization() {
        let json = r#"{"topics": ["vm:status", "vm:metrics"]}"#;
        let sub: WsSubscription = serde_json::from_str(json).unwrap();
        assert_eq!(sub.topics.len(), 2);
        assert!(sub.topics.contains(&"vm:status".to_string()));
        assert!(sub.topics.contains(&"vm:metrics".to_string()));
    }

    #[test]
    fn test_ws_event_serialization() {
        let event = WsEvent::VmStatusChanged {
            vm_id: "vm-100".to_string(),
            old_status: "stopped".to_string(),
            new_status: "running".to_string(),
            timestamp: "2025-10-09T10:30:45Z".to_string(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("VmStatusChanged"));
        assert!(json.contains("vm-100"));
        assert!(json.contains("running"));
    }
}
