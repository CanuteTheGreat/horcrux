///! Console access module
///! Provides VNC and SPICE console access to VMs via WebSocket proxy

mod vnc;
mod websocket;

use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Console connection type
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConsoleType {
    Vnc,
    Spice,
    Serial,
}

/// Console connection info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsoleInfo {
    pub vm_id: String,
    pub console_type: ConsoleType,
    pub host: String,
    pub port: u16,
    pub ticket: String,
    pub ws_port: u16,
}

/// Console ticket for authentication
#[derive(Debug, Clone)]
pub struct ConsoleTicket {
    pub ticket_id: String,
    pub vm_id: String,
    pub console_type: ConsoleType,
    pub vnc_port: u16,
    pub created_at: i64,
    pub expires_at: i64,
}

/// Console manager
pub struct ConsoleManager {
    active_tickets: Arc<RwLock<HashMap<String, ConsoleTicket>>>,
    vnc_manager: vnc::VncManager,
    ws_proxy: Arc<websocket::WebSocketProxy>,
}

impl ConsoleManager {
    pub fn new() -> Self {
        Self {
            active_tickets: Arc::new(RwLock::new(HashMap::new())),
            vnc_manager: vnc::VncManager::new(),
            ws_proxy: Arc::new(websocket::WebSocketProxy::new()),
        }
    }

    /// Create a console connection for a VM
    pub async fn create_console(&self, vm_id: &str, console_type: ConsoleType) -> Result<ConsoleInfo> {
        // Generate authentication ticket
        let ticket = self.generate_ticket(vm_id, &console_type).await?;

        match console_type {
            ConsoleType::Vnc => {
                // Get or start VNC server for this VM
                let vnc_port = self.vnc_manager.get_vnc_port(vm_id).await?;

                // Start WebSocket proxy
                let ws_port = self.ws_proxy.start_proxy(&ticket.ticket_id, "127.0.0.1", vnc_port).await?;

                Ok(ConsoleInfo {
                    vm_id: vm_id.to_string(),
                    console_type: ConsoleType::Vnc,
                    host: "127.0.0.1".to_string(),
                    port: vnc_port,
                    ticket: ticket.ticket_id.clone(),
                    ws_port,
                })
            }
            ConsoleType::Spice => {
                // TODO: Implement SPICE support
                Err(horcrux_common::Error::System("SPICE not yet implemented".to_string()))
            }
            ConsoleType::Serial => {
                // TODO: Implement serial console support
                Err(horcrux_common::Error::System("Serial console not yet implemented".to_string()))
            }
        }
    }

    /// Verify a console ticket
    pub async fn verify_ticket(&self, ticket_id: &str) -> Result<ConsoleTicket> {
        let tickets = self.active_tickets.read().await;
        let ticket = tickets
            .get(ticket_id)
            .ok_or_else(|| horcrux_common::Error::System("Invalid console ticket".to_string()))?;

        // Check if ticket is expired
        let now = chrono::Utc::now().timestamp();
        if now > ticket.expires_at {
            return Err(horcrux_common::Error::System("Console ticket expired".to_string()));
        }

        Ok(ticket.clone())
    }

    /// Generate a new console ticket
    async fn generate_ticket(&self, vm_id: &str, console_type: &ConsoleType) -> Result<ConsoleTicket> {
        let ticket_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp();
        let expires_at = now + 300; // 5 minutes

        // For VNC, we need to get the port from QEMU
        let vnc_port = match console_type {
            ConsoleType::Vnc => self.vnc_manager.ensure_vnc_enabled(vm_id).await?,
            _ => 0,
        };

        let ticket = ConsoleTicket {
            ticket_id: ticket_id.clone(),
            vm_id: vm_id.to_string(),
            console_type: console_type.clone(),
            vnc_port,
            created_at: now,
            expires_at,
        };

        let mut tickets = self.active_tickets.write().await;
        tickets.insert(ticket_id, ticket.clone());

        Ok(ticket)
    }

    /// Clean up expired tickets
    pub async fn cleanup_expired_tickets(&self) {
        let now = chrono::Utc::now().timestamp();
        let mut tickets = self.active_tickets.write().await;
        tickets.retain(|_, ticket| ticket.expires_at > now);
    }

    /// Get VNC websocket URL for a VM
    pub async fn get_vnc_websocket(&self, vm_id: &str) -> Result<String> {
        let info = self.create_console(vm_id, ConsoleType::Vnc).await?;
        Ok(format!("ws://{}:{}/{}", info.host, info.ws_port, info.ticket))
    }
}
