//! WebSocket ticket authentication for container logs/exec/events.
//!
//! Mirrors `console::ConsoleManager`'s ticket pattern exactly (short-lived,
//! single-purpose, server-generated opaque ticket IDs checked on WebSocket
//! upgrade) rather than inventing a new auth scheme for containers. The
//! normal `auth_middleware`-protected REST endpoint issues a ticket; the
//! WebSocket endpoint (which can't carry an `Authorization` header from a
//! browser `new WebSocket(...)` call) is authenticated by that ticket alone.
use horcrux_common::{ContainerRuntime, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// What kind of container WebSocket session a ticket authorizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContainerWsKind {
    Logs,
    Exec,
    Events,
}

#[derive(Debug, Clone)]
pub struct ContainerWsTicket {
    pub ticket_id: String,
    pub container_id: String,
    pub runtime: ContainerRuntime,
    pub kind: ContainerWsKind,
    /// Shell/command to run for `Exec` tickets; unused otherwise.
    pub exec_command: Vec<String>,
    pub created_at: i64,
    pub expires_at: i64,
}

/// Manages short-lived WebSocket auth tickets for container logs/exec/events,
/// exactly like `ConsoleManager::active_tickets` does for VM consoles.
pub struct ContainerWsManager {
    active_tickets: Arc<RwLock<HashMap<String, ContainerWsTicket>>>,
}

impl Default for ContainerWsManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ContainerWsManager {
    pub fn new() -> Self {
        Self {
            active_tickets: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn create_ticket(
        &self,
        container_id: &str,
        runtime: ContainerRuntime,
        kind: ContainerWsKind,
        exec_command: Vec<String>,
    ) -> ContainerWsTicket {
        let now = chrono::Utc::now().timestamp();
        let ticket = ContainerWsTicket {
            ticket_id: uuid::Uuid::new_v4().to_string(),
            container_id: container_id.to_string(),
            runtime,
            kind,
            exec_command,
            created_at: now,
            // Exec/logs/events sessions can run long (an attached shell, a
            // `-f` log tail); 5 minutes matches the console ticket TTL for
            // *establishing* the connection - once the WebSocket is
            // upgraded the ticket isn't re-checked, same as console.
            expires_at: now + 300,
        };
        self.active_tickets
            .write()
            .await
            .insert(ticket.ticket_id.clone(), ticket.clone());
        ticket
    }

    pub async fn verify_ticket(&self, ticket_id: &str) -> Result<ContainerWsTicket> {
        let tickets = self.active_tickets.read().await;
        let ticket = tickets.get(ticket_id).ok_or_else(|| {
            horcrux_common::Error::System("Invalid container WebSocket ticket".to_string())
        })?;
        let now = chrono::Utc::now().timestamp();
        if now > ticket.expires_at {
            return Err(horcrux_common::Error::System(
                "Container WebSocket ticket expired".to_string(),
            ));
        }
        Ok(ticket.clone())
    }

    /// Periodic cleanup of expired tickets (same idea as console, invoked
    /// lazily rather than on a background timer to avoid adding a new task).
    pub async fn sweep_expired(&self) {
        let now = chrono::Utc::now().timestamp();
        self.active_tickets
            .write()
            .await
            .retain(|_, t| t.expires_at >= now);
    }
}
