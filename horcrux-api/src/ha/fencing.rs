//! Node Fencing (STONITH) Implementation
//!
//! Provides automatic fencing of failed nodes to prevent split-brain scenarios
//! and ensure data integrity in HA environments.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, error};
use horcrux_common::Result;

/// Fencing agent types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FencingAgentType {
    /// IPMI/BMC-based fencing
    Ipmi,
    /// Fence via VM hypervisor (for nested VMs)
    Hypervisor,
    /// Fence via SNMP power switch
    SnmpPdu,
    /// Fence via SSH command
    Ssh,
    /// Fence via iLO
    Ilo,
    /// Fence via DRAC
    Drac,
    /// Fence via libvirt (for VM testing)
    Libvirt,
    /// Manual fencing (requires operator intervention)
    Manual,
    /// Watchdog timer
    Watchdog,
}

/// Fencing device configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FencingDevice {
    pub id: String,
    pub agent_type: FencingAgentType,
    pub node: String,
    pub address: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub port: Option<u16>,
    pub options: HashMap<String, String>,
    pub priority: u32,
}

/// Fencing operation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FencingResult {
    Success,
    Failed(String),
    Timeout,
    Pending,
    ManualRequired,
}

/// Fencing event for audit trail
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FencingEvent {
    pub timestamp: i64,
    pub node: String,
    pub device_id: String,
    pub action: FencingAction,
    pub result: FencingResult,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FencingAction {
    Off,
    On,
    Reboot,
    Status,
}

/// Fencing manager for node isolation
pub struct FencingManager {
    devices: Arc<RwLock<HashMap<String, FencingDevice>>>,
    events: Arc<RwLock<Vec<FencingEvent>>>,
    enabled: Arc<RwLock<bool>>,
    timeout_secs: u32,
}

impl FencingManager {
    pub fn new() -> Self {
        Self {
            devices: Arc::new(RwLock::new(HashMap::new())),
            events: Arc::new(RwLock::new(Vec::new())),
            enabled: Arc::new(RwLock::new(true)),
            timeout_secs: 60,
        }
    }

    /// Enable fencing
    pub async fn enable(&self) {
        *self.enabled.write().await = true;
        info!("Fencing enabled");
    }

    /// Disable fencing (dangerous - use with caution)
    pub async fn disable(&self) {
        *self.enabled.write().await = false;
        warn!("Fencing disabled - cluster is vulnerable to split-brain");
    }

    /// Check if fencing is enabled
    pub async fn is_enabled(&self) -> bool {
        *self.enabled.read().await
    }

    /// Add a fencing device
    pub async fn add_device(&self, device: FencingDevice) -> Result<()> {
        let mut devices = self.devices.write().await;

        if devices.contains_key(&device.id) {
            return Err(horcrux_common::Error::System(
                format!("Fencing device {} already exists", device.id)
            ));
        }

        info!(
            device_id = %device.id,
            node = %device.node,
            agent = ?device.agent_type,
            "Adding fencing device"
        );

        devices.insert(device.id.clone(), device);
        Ok(())
    }

    /// Remove a fencing device
    pub async fn remove_device(&self, device_id: &str) -> Result<()> {
        let mut devices = self.devices.write().await;
        devices.remove(device_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("Fencing device {} not found", device_id))
        })?;
        info!(device_id = device_id, "Removed fencing device");
        Ok(())
    }

    /// List all fencing devices
    pub async fn list_devices(&self) -> Vec<FencingDevice> {
        self.devices.read().await.values().cloned().collect()
    }

    /// Get devices for a specific node
    pub async fn get_node_devices(&self, node: &str) -> Vec<FencingDevice> {
        let devices = self.devices.read().await;
        let mut node_devices: Vec<_> = devices
            .values()
            .filter(|d| d.node == node)
            .cloned()
            .collect();

        // Sort by priority
        node_devices.sort_by_key(|d| d.priority);
        node_devices
    }

    /// Fence a node (power off)
    pub async fn fence_node(&self, node: &str, reason: &str) -> FencingResult {
        if !self.is_enabled().await {
            warn!(node = node, "Fencing requested but fencing is disabled");
            return FencingResult::Failed("Fencing is disabled".to_string());
        }

        info!(node = node, reason = reason, "Initiating node fencing");

        let devices = self.get_node_devices(node).await;

        if devices.is_empty() {
            error!(node = node, "No fencing devices configured for node");
            return FencingResult::ManualRequired;
        }

        // Try each device in priority order
        for device in devices {
            let result = self.execute_fence(&device, FencingAction::Off).await;

            self.log_event(FencingEvent {
                timestamp: chrono::Utc::now().timestamp(),
                node: node.to_string(),
                device_id: device.id.clone(),
                action: FencingAction::Off,
                result: result.clone(),
                reason: reason.to_string(),
            }).await;

            match result {
                FencingResult::Success => {
                    info!(node = node, device = %device.id, "Node successfully fenced");
                    return FencingResult::Success;
                }
                FencingResult::Failed(ref err) => {
                    warn!(
                        node = node,
                        device = %device.id,
                        error = %err,
                        "Fencing failed, trying next device"
                    );
                }
                _ => {}
            }
        }

        error!(node = node, "All fencing devices failed");
        FencingResult::ManualRequired
    }

    /// Check node power status
    pub async fn check_node_status(&self, node: &str) -> FencingResult {
        let devices = self.get_node_devices(node).await;

        if devices.is_empty() {
            return FencingResult::Failed("No fencing devices".to_string());
        }

        // Use first device for status check
        if let Some(device) = devices.first() {
            return self.execute_fence(device, FencingAction::Status).await;
        }

        FencingResult::Failed("No devices available".to_string())
    }

    /// Execute a fencing operation
    async fn execute_fence(&self, device: &FencingDevice, action: FencingAction) -> FencingResult {
        match device.agent_type {
            FencingAgentType::Ipmi => self.fence_ipmi(device, &action).await,
            FencingAgentType::Ssh => self.fence_ssh(device, &action).await,
            FencingAgentType::Libvirt => self.fence_libvirt(device, &action).await,
            FencingAgentType::Watchdog => self.fence_watchdog(device, &action).await,
            FencingAgentType::Manual => FencingResult::ManualRequired,
            _ => {
                warn!(agent = ?device.agent_type, "Fencing agent not implemented");
                FencingResult::Failed(format!("Agent {:?} not implemented", device.agent_type))
            }
        }
    }

    /// IPMI fencing implementation
    async fn fence_ipmi(&self, device: &FencingDevice, action: &FencingAction) -> FencingResult {
        let action_cmd = match action {
            FencingAction::Off => "power off",
            FencingAction::On => "power on",
            FencingAction::Reboot => "power reset",
            FencingAction::Status => "power status",
        };

        let mut cmd = tokio::process::Command::new("ipmitool");
        cmd.arg("-H").arg(&device.address);

        if let Some(ref user) = device.username {
            cmd.arg("-U").arg(user);
        }
        if let Some(ref pass) = device.password {
            cmd.arg("-P").arg(pass);
        }

        cmd.args(action_cmd.split_whitespace());

        match tokio::time::timeout(
            std::time::Duration::from_secs(self.timeout_secs as u64),
            cmd.output(),
        ).await {
            Ok(Ok(output)) => {
                if output.status.success() {
                    FencingResult::Success
                } else {
                    FencingResult::Failed(
                        String::from_utf8_lossy(&output.stderr).to_string()
                    )
                }
            }
            Ok(Err(e)) => FencingResult::Failed(format!("Command failed: {}", e)),
            Err(_) => FencingResult::Timeout,
        }
    }

    /// SSH fencing implementation
    async fn fence_ssh(&self, device: &FencingDevice, action: &FencingAction) -> FencingResult {
        let cmd = match action {
            FencingAction::Off => device.options.get("off_cmd")
                .cloned()
                .unwrap_or_else(|| "poweroff".to_string()),
            FencingAction::On => return FencingResult::Failed("SSH cannot power on".to_string()),
            FencingAction::Reboot => device.options.get("reboot_cmd")
                .cloned()
                .unwrap_or_else(|| "reboot".to_string()),
            FencingAction::Status => "uptime".to_string(),
        };

        let mut ssh_cmd = tokio::process::Command::new("ssh");

        if let Some(ref user) = device.username {
            ssh_cmd.arg("-l").arg(user);
        }

        if let Some(port) = device.port {
            ssh_cmd.arg("-p").arg(port.to_string());
        }

        ssh_cmd.arg("-o").arg("StrictHostKeyChecking=no")
            .arg("-o").arg("ConnectTimeout=10")
            .arg(&device.address)
            .arg(&cmd);

        match tokio::time::timeout(
            std::time::Duration::from_secs(self.timeout_secs as u64),
            ssh_cmd.output(),
        ).await {
            Ok(Ok(output)) => {
                if output.status.success() || matches!(action, FencingAction::Off | FencingAction::Reboot) {
                    // For shutdown commands, connection closure is expected
                    FencingResult::Success
                } else {
                    FencingResult::Failed(
                        String::from_utf8_lossy(&output.stderr).to_string()
                    )
                }
            }
            Ok(Err(e)) => FencingResult::Failed(format!("SSH failed: {}", e)),
            Err(_) => FencingResult::Timeout,
        }
    }

    /// Libvirt fencing implementation (for testing)
    async fn fence_libvirt(&self, device: &FencingDevice, action: &FencingAction) -> FencingResult {
        let domain = device.options.get("domain")
            .cloned()
            .unwrap_or_else(|| device.node.clone());

        let virsh_cmd = match action {
            FencingAction::Off => "destroy",
            FencingAction::On => "start",
            FencingAction::Reboot => "reboot",
            FencingAction::Status => "domstate",
        };

        let mut cmd = tokio::process::Command::new("virsh");

        if !device.address.is_empty() {
            cmd.arg("-c").arg(&device.address);
        }

        cmd.arg(virsh_cmd).arg(&domain);

        match cmd.output().await {
            Ok(output) => {
                if output.status.success() {
                    FencingResult::Success
                } else {
                    FencingResult::Failed(
                        String::from_utf8_lossy(&output.stderr).to_string()
                    )
                }
            }
            Err(e) => FencingResult::Failed(format!("virsh failed: {}", e)),
        }
    }

    /// Watchdog timer fencing
    async fn fence_watchdog(&self, _device: &FencingDevice, action: &FencingAction) -> FencingResult {
        match action {
            FencingAction::Off | FencingAction::Reboot => {
                // Trigger watchdog reset by writing to /dev/watchdog
                match tokio::fs::write("/dev/watchdog", b"V").await {
                    Ok(_) => FencingResult::Success,
                    Err(e) => FencingResult::Failed(format!("Watchdog trigger failed: {}", e)),
                }
            }
            FencingAction::Status => {
                if tokio::fs::metadata("/dev/watchdog").await.is_ok() {
                    FencingResult::Success
                } else {
                    FencingResult::Failed("Watchdog device not available".to_string())
                }
            }
            _ => FencingResult::Failed("Invalid action for watchdog".to_string()),
        }
    }

    /// Log a fencing event
    async fn log_event(&self, event: FencingEvent) {
        let mut events = self.events.write().await;
        events.push(event);

        // Keep last 100 events
        if events.len() > 100 {
            let drain_count = events.len() - 100;
            events.drain(0..drain_count);
        }
    }

    /// Get fencing event history
    pub async fn get_events(&self, node: Option<&str>, limit: usize) -> Vec<FencingEvent> {
        let events = self.events.read().await;

        let filtered: Vec<_> = match node {
            Some(n) => events.iter().filter(|e| e.node == n).cloned().collect(),
            None => events.clone(),
        };

        filtered.into_iter().rev().take(limit).collect()
    }
}

impl Default for FencingManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fencing_device_management() {
        let manager = FencingManager::new();

        let device = FencingDevice {
            id: "fence-node1".to_string(),
            agent_type: FencingAgentType::Ipmi,
            node: "node1".to_string(),
            address: "192.168.1.100".to_string(),
            username: Some("admin".to_string()),
            password: Some("password".to_string()),
            port: None,
            options: HashMap::new(),
            priority: 1,
        };

        manager.add_device(device).await.unwrap();

        let devices = manager.list_devices().await;
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].id, "fence-node1");

        let node_devices = manager.get_node_devices("node1").await;
        assert_eq!(node_devices.len(), 1);
    }

    #[tokio::test]
    async fn test_fencing_enable_disable() {
        let manager = FencingManager::new();

        assert!(manager.is_enabled().await);

        manager.disable().await;
        assert!(!manager.is_enabled().await);

        manager.enable().await;
        assert!(manager.is_enabled().await);
    }
}
