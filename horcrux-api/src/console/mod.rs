//! Console access module
//! Provides VNC and SPICE console access to VMs via WebSocket proxy

#![allow(dead_code)]

mod vnc;
mod spice;
mod serial;
mod websocket;
pub mod novnc;

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
    spice_manager: spice::SpiceManager,
    serial_manager: serial::SerialManager,
    ws_proxy: Arc<websocket::WebSocketProxy>,
}

impl ConsoleManager {
    pub fn new() -> Self {
        Self {
            active_tickets: Arc::new(RwLock::new(HashMap::new())),
            vnc_manager: vnc::VncManager::new(),
            spice_manager: spice::SpiceManager::new(),
            serial_manager: serial::SerialManager::new(),
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
                // Get or start SPICE server for this VM
                let spice_port = self.spice_manager.get_spice_port(vm_id).await?;

                // Start WebSocket proxy for SPICE
                let ws_port = self.ws_proxy.start_proxy(&ticket.ticket_id, "127.0.0.1", spice_port).await?;

                Ok(ConsoleInfo {
                    vm_id: vm_id.to_string(),
                    console_type: ConsoleType::Spice,
                    host: "127.0.0.1".to_string(),
                    port: spice_port,
                    ticket: ticket.ticket_id.clone(),
                    ws_port,
                })
            }
            ConsoleType::Serial => {
                // Get or enable serial console for this VM
                let socket_path = self.serial_manager.get_serial_socket(vm_id).await?;

                // For serial console, we return the socket path directly
                // Client can connect via WebSocket proxy to the Unix socket
                let ws_port = self.ws_proxy.start_unix_proxy(&ticket.ticket_id, &socket_path).await?;

                Ok(ConsoleInfo {
                    vm_id: vm_id.to_string(),
                    console_type: ConsoleType::Serial,
                    host: "127.0.0.1".to_string(),
                    port: 0, // Serial uses socket, not TCP port
                    ticket: ticket.ticket_id.clone(),
                    ws_port,
                })
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

        // Get the port from QEMU based on console type
        let vnc_port = match console_type {
            ConsoleType::Vnc => self.vnc_manager.ensure_vnc_enabled(vm_id).await?,
            ConsoleType::Spice => self.spice_manager.ensure_spice_enabled(vm_id).await?,
            ConsoleType::Serial => {
                // Ensure serial is enabled and return 0 (serial uses socket, not port)
                let _ = self.serial_manager.ensure_serial_enabled(vm_id).await?;
                0
            }
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

    /// Get SPICE websocket URL for a VM
    pub async fn get_spice_websocket(&self, vm_id: &str) -> Result<String> {
        let info = self.create_console(vm_id, ConsoleType::Spice).await?;
        Ok(format!("ws://{}:{}/{}", info.host, info.ws_port, info.ticket))
    }

    /// Get SPICE connection URI for native SPICE clients
    pub async fn get_spice_uri(&self, vm_id: &str) -> Result<String> {
        let config = self.spice_manager.get_spice_config(vm_id).await?;
        Ok(spice::SpiceManager::generate_spice_uri(
            &config.addr,
            config.port,
            config.password.as_deref(),
        ))
    }

    /// Get Serial console WebSocket URL for a VM
    pub async fn get_serial_websocket(&self, vm_id: &str) -> Result<String> {
        let info = self.create_console(vm_id, ConsoleType::Serial).await?;
        Ok(format!("ws://{}:{}/{}", info.host, info.ws_port, info.ticket))
    }

    /// Send data to serial console
    pub async fn write_serial(&self, vm_id: &str, data: &str) -> Result<()> {
        self.serial_manager.write_serial_input(vm_id, data).await
    }

    /// Read data from serial console
    pub async fn read_serial(&self, vm_id: &str, lines: usize) -> Result<String> {
        self.serial_manager.read_serial_output(vm_id, lines).await
    }
}
