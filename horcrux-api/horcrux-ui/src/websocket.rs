///! WebSocket client for real-time updates
///! Provides live VM status, metrics, and event notifications

use leptos::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{CloseEvent, ErrorEvent, MessageEvent, WebSocket};

/// WebSocket event types (matching server-side definitions)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum WsEvent {
    VmStatusChanged {
        vm_id: String,
        old_status: String,
        new_status: String,
        timestamp: String,
    },
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
    NodeMetrics {
        hostname: String,
        cpu_usage: f64,
        memory_usage: f64,
        disk_usage: f64,
        load_average: [f64; 3],
        timestamp: String,
    },
    VmCreated {
        vm_id: String,
        name: String,
        user: String,
        timestamp: String,
    },
    VmDeleted {
        vm_id: String,
        name: String,
        user: String,
        timestamp: String,
    },
    BackupCompleted {
        vm_id: String,
        backup_id: String,
        size_bytes: u64,
        duration_seconds: u64,
        timestamp: String,
    },
    MigrationStarted {
        vm_id: String,
        source_node: String,
        target_node: String,
        timestamp: String,
    },
    MigrationProgress {
        vm_id: String,
        progress: u8,
        transferred_bytes: u64,
        total_bytes: u64,
        timestamp: String,
    },
    MigrationCompleted {
        vm_id: String,
        target_node: String,
        duration_seconds: u64,
        timestamp: String,
    },
    AlertTriggered {
        alert_id: String,
        rule_name: String,
        severity: String,
        target: String,
        message: String,
        timestamp: String,
    },
    AlertResolved {
        alert_id: String,
        rule_name: String,
        target: String,
        timestamp: String,
    },
    ContainerStatusChanged {
        container_id: String,
        old_status: String,
        new_status: String,
        timestamp: String,
    },
    ContainerDeleted {
        container_id: String,
        timestamp: String,
    },
    Notification {
        level: String,
        title: String,
        message: String,
        timestamp: String,
    },
    Ping {
        timestamp: String,
    },
    Subscribed {
        topics: Vec<String>,
        timestamp: String,
    },
    Error {
        code: String,
        message: String,
        timestamp: String,
    },
}

/// WebSocket subscription request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsSubscription {
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

/// Hook to use WebSocket in components
pub fn use_websocket(topics: Vec<String>) -> (ReadSignal<Option<WsEvent>>, ReadSignal<bool>) {
    let (events, set_events) = create_signal(None);
    let (connected, set_connected) = create_signal(false);

    create_effect(move |_| {
        let ws = match WebSocket::new("ws://localhost:8006/api/ws") {
            Ok(ws) => ws,
            Err(e) => {
                logging::log!("Failed to create WebSocket: {:?}", e);
                return;
            }
        };

        ws.set_binary_type(web_sys::BinaryType::Arraybuffer);

        let cloned_ws = ws.clone();
        let topics_clone = topics.clone();

        // Handle connection open
        let onopen_callback = Closure::wrap(Box::new(move |_| {
            logging::log!("WebSocket connected");
            set_connected.set(true);

            // Send subscription request
            let subscription = WsSubscription { topics: topics_clone.clone() };
            if let Ok(json) = serde_json::to_string(&subscription) {
                let _ = cloned_ws.send_with_str(&json);
            }
        }) as Box<dyn FnMut(JsValue)>);
        ws.set_onopen(Some(onopen_callback.as_ref().unchecked_ref()));
        onopen_callback.forget();

        // Handle messages
        let onmessage_callback = Closure::wrap(Box::new(move |e: MessageEvent| {
            if let Ok(txt) = e.data().dyn_into::<js_sys::JsString>() {
                let message = String::from(txt);

                match serde_json::from_str::<WsEvent>(&message) {
                    Ok(event) => {
                        logging::log!("Received WebSocket event: {:?}", event);
                        set_events.set(Some(event));
                    }
                    Err(err) => {
                        logging::log!("Failed to parse WebSocket message: {}", err);
                    }
                }
            }
        }) as Box<dyn FnMut(MessageEvent)>);
        ws.set_onmessage(Some(onmessage_callback.as_ref().unchecked_ref()));
        onmessage_callback.forget();

        // Handle errors
        let onerror_callback = Closure::wrap(Box::new(move |e: ErrorEvent| {
            logging::log!("WebSocket error: {:?}", e.message());
            set_connected.set(false);
        }) as Box<dyn FnMut(ErrorEvent)>);
        ws.set_onerror(Some(onerror_callback.as_ref().unchecked_ref()));
        onerror_callback.forget();

        // Handle close
        let onclose_callback = Closure::wrap(Box::new(move |e: CloseEvent| {
            logging::log!("WebSocket closed: code={}, reason={}", e.code(), e.reason());
            set_connected.set(false);
        }) as Box<dyn FnMut(CloseEvent)>);
        ws.set_onclose(Some(onclose_callback.as_ref().unchecked_ref()));
        onclose_callback.forget();

        // Cleanup on unmount
        on_cleanup(move || {
            let _ = ws.close();
        });
    });

    (events, connected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ws_subscription_serialization() {
        let subscription = WsSubscription {
            topics: vec![
                TOPIC_VM_STATUS.to_string(),
                TOPIC_VM_METRICS.to_string(),
            ],
        };

        let json = serde_json::to_string(&subscription).unwrap();
        assert!(json.contains("topics"));
        assert!(json.contains("vm:status"));
        assert!(json.contains("vm:metrics"));
    }

    #[test]
    fn test_ws_event_deserialization() {
        let json = r#"{
            "type": "VmStatusChanged",
            "data": {
                "vm_id": "vm-100",
                "old_status": "stopped",
                "new_status": "running",
                "timestamp": "2025-10-12T10:30:00Z"
            }
        }"#;

        let event: WsEvent = serde_json::from_str(json).unwrap();
        match event {
            WsEvent::VmStatusChanged { vm_id, old_status, new_status, .. } => {
                assert_eq!(vm_id, "vm-100");
                assert_eq!(old_status, "stopped");
                assert_eq!(new_status, "running");
            }
            _ => panic!("Expected VmStatusChanged event"),
        }
    }

    #[test]
    fn test_node_metrics_deserialization() {
        let json = r#"{
            "type": "NodeMetrics",
            "data": {
                "hostname": "node1",
                "cpu_usage": 45.5,
                "memory_usage": 60.2,
                "disk_usage": 75.0,
                "load_average": [1.5, 2.0, 1.8],
                "timestamp": "2025-10-12T10:30:00Z"
            }
        }"#;

        let event: WsEvent = serde_json::from_str(json).unwrap();
        match event {
            WsEvent::NodeMetrics { hostname, cpu_usage, memory_usage, .. } => {
                assert_eq!(hostname, "node1");
                assert_eq!(cpu_usage, 45.5);
                assert_eq!(memory_usage, 60.2);
            }
            _ => panic!("Expected NodeMetrics event"),
        }
    }
}
