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

/// WebSocket state
#[derive(Clone)]
pub struct WsState {
    /// Event broadcaster
    pub tx: broadcast::Sender<WsEvent>,
}

impl WsState {
    /// Create new WebSocket state
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(1000);
        Self { tx }
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
    let username_clone = auth_user.username.clone();

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
