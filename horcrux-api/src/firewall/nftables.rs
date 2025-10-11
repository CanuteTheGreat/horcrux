///! nftables integration for firewall rules

use super::{FirewallAction, FirewallRule};
use horcrux_common::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

/// Rule handle tracking
#[derive(Debug, Clone)]
struct RuleHandle {
    _rule_id: String,  // Reserved for future rule queries
    chain_name: String,
    handle: u64,
}

/// nftables manager
pub struct NftablesManager {
    table_name: String,
    rule_handles: Arc<RwLock<HashMap<String, RuleHandle>>>,
    next_handle: Arc<RwLock<u64>>,
}

impl NftablesManager {
    pub fn new() -> Self {
        Self {
            table_name: "horcrux".to_string(),
            rule_handles: Arc::new(RwLock::new(HashMap::new())),
            next_handle: Arc::new(RwLock::new(1)),
        }
    }

    /// Initialize nftables table
    pub async fn initialize(&self) -> Result<()> {
        info!("Initializing nftables table: {}", self.table_name);

        // Create table
        let output = Command::new("nft")
            .arg("add")
            .arg("table")
            .arg("inet")
            .arg(&self.table_name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to create nftables table: {}", e)))?;

        if !output.status.success() && !String::from_utf8_lossy(&output.stderr).contains("exists") {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Failed to create nftables table: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create nftables table: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Add firewall rule for a VM
    pub async fn add_vm_rule(&self, vm_id: &str, rule: &FirewallRule) -> Result<()> {
        info!("Adding firewall rule for VM {}: {}", vm_id, rule.id);

        let chain_name = format!("vm-{}", vm_id);

        // Ensure chain exists
        self.ensure_chain(&chain_name).await?;

        // Build rule
        let nft_rule = self.build_nft_rule(rule);

        // Add rule to chain
        let output = Command::new("nft")
            .arg("add")
            .arg("rule")
            .arg("inet")
            .arg(&self.table_name)
            .arg(&chain_name)
            .arg(&nft_rule)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to add nftables rule: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Failed to add nftables rule: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to add nftables rule: {}",
                stderr
            )));
        }

        // Track rule handle
        let mut next_handle = self.next_handle.write().await;
        let handle = *next_handle;
        *next_handle += 1;
        drop(next_handle);

        let rule_handle = RuleHandle {
            _rule_id: rule.id.clone(),
            chain_name: chain_name.clone(),
            handle,
        };

        let mut handles = self.rule_handles.write().await;
        handles.insert(format!("vm-{}-{}", vm_id, rule.id), rule_handle);

        Ok(())
    }

    /// Add firewall rule for a container
    pub async fn add_container_rule(&self, ct_id: &str, rule: &FirewallRule) -> Result<()> {
        info!("Adding firewall rule for container {}: {}", ct_id, rule.id);

        let chain_name = format!("ct-{}", ct_id);
        self.ensure_chain(&chain_name).await?;

        let nft_rule = self.build_nft_rule(rule);

        let output = Command::new("nft")
            .arg("add")
            .arg("rule")
            .arg("inet")
            .arg(&self.table_name)
            .arg(&chain_name)
            .arg(&nft_rule)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to add nftables rule: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to add nftables rule: {}",
                stderr
            )));
        }

        // Track rule handle
        let mut next_handle = self.next_handle.write().await;
        let handle = *next_handle;
        *next_handle += 1;
        drop(next_handle);

        let rule_handle = RuleHandle {
            _rule_id: rule.id.clone(),
            chain_name: chain_name.clone(),
            handle,
        };

        let mut handles = self.rule_handles.write().await;
        handles.insert(format!("ct-{}-{}", ct_id, rule.id), rule_handle);

        Ok(())
    }

    /// Remove firewall rule for a VM
    pub async fn remove_vm_rule(&self, vm_id: &str, rule_id: &str) -> Result<()> {
        info!("Removing firewall rule {} for VM {}", rule_id, vm_id);

        let key = format!("vm-{}-{}", vm_id, rule_id);
        let mut handles = self.rule_handles.write().await;

        if let Some(rule_handle) = handles.remove(&key) {
            // Delete rule by handle
            let output = Command::new("nft")
                .arg("delete")
                .arg("rule")
                .arg("inet")
                .arg(&self.table_name)
                .arg(&rule_handle.chain_name)
                .arg("handle")
                .arg(rule_handle.handle.to_string())
                .output()
                .await
                .map_err(|e| horcrux_common::Error::System(format!("Failed to delete nftables rule: {}", e)))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                warn!("Failed to delete nftables rule: {}", stderr);
                // Don't return error - rule might already be deleted
            }

            info!("Removed firewall rule {} for VM {}", rule_id, vm_id);
        } else {
            warn!("Firewall rule {} not found for VM {}", rule_id, vm_id);
        }

        Ok(())
    }

    /// Remove firewall rule for a container
    pub async fn remove_container_rule(&self, ct_id: &str, rule_id: &str) -> Result<()> {
        info!("Removing firewall rule {} for container {}", rule_id, ct_id);

        let key = format!("ct-{}-{}", ct_id, rule_id);
        let mut handles = self.rule_handles.write().await;

        if let Some(rule_handle) = handles.remove(&key) {
            // Delete rule by handle
            let output = Command::new("nft")
                .arg("delete")
                .arg("rule")
                .arg("inet")
                .arg(&self.table_name)
                .arg(&rule_handle.chain_name)
                .arg("handle")
                .arg(rule_handle.handle.to_string())
                .output()
                .await
                .map_err(|e| horcrux_common::Error::System(format!("Failed to delete nftables rule: {}", e)))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                warn!("Failed to delete nftables rule: {}", stderr);
                // Don't return error - rule might already be deleted
            }

            info!("Removed firewall rule {} for container {}", rule_id, ct_id);
        } else {
            warn!("Firewall rule {} not found for container {}", rule_id, ct_id);
        }

        Ok(())
    }

    /// Ensure chain exists
    async fn ensure_chain(&self, chain_name: &str) -> Result<()> {
        let _output = Command::new("nft")
            .arg("add")
            .arg("chain")
            .arg("inet")
            .arg(&self.table_name)
            .arg(chain_name)
            .arg("{")
            .arg("type")
            .arg("filter")
            .arg("hook")
            .arg("forward")
            .arg("priority")
            .arg("0")
            .arg(";")
            .arg("policy")
            .arg("accept")
            .arg(";")
            .arg("}")
            .output()
            .await
            .ok();

        // Ignore errors if chain already exists
        Ok(())
    }

    /// Build nftables rule from FirewallRule
    fn build_nft_rule(&self, rule: &FirewallRule) -> String {
        let mut parts = Vec::new();

        // Protocol
        if let Some(ref proto) = rule.protocol {
            let proto_str = match proto {
                super::Protocol::Tcp => "tcp",
                super::Protocol::Udp => "udp",
                super::Protocol::Icmp => "icmp",
                super::Protocol::Icmpv6 => "icmpv6",
                super::Protocol::Any => "",
            };
            if !proto_str.is_empty() {
                parts.push(format!("{} ", proto_str));
            }
        }

        // Source
        if let Some(ref source) = rule.source {
            parts.push(format!("ip saddr {} ", source));
        }

        // Source port
        if let Some(ref sport) = rule.sport {
            if let Some(ref proto) = rule.protocol {
                let proto_str = match proto {
                    super::Protocol::Tcp => "tcp",
                    super::Protocol::Udp => "udp",
                    _ => "",
                };
                if !proto_str.is_empty() {
                    parts.push(format!("{} sport {} ", proto_str, sport));
                }
            }
        }

        // Destination
        if let Some(ref dest) = rule.dest {
            parts.push(format!("ip daddr {} ", dest));
        }

        // Destination port
        if let Some(ref dport) = rule.dport {
            if let Some(ref proto) = rule.protocol {
                let proto_str = match proto {
                    super::Protocol::Tcp => "tcp",
                    super::Protocol::Udp => "udp",
                    _ => "",
                };
                if !proto_str.is_empty() {
                    parts.push(format!("{} dport {} ", proto_str, dport));
                }
            }
        }

        // Action
        let action = match rule.action {
            FirewallAction::Accept => "accept",
            FirewallAction::Reject => "reject",
            FirewallAction::Drop => "drop",
        };
        parts.push(action.to_string());

        parts.concat()
    }

    /// Reload all firewall rules
    pub async fn reload_all(&self) -> Result<()> {
        info!("Reloading all nftables rules");

        // Flush the entire table (removes all chains and rules)
        let output = Command::new("nft")
            .arg("flush")
            .arg("table")
            .arg("inet")
            .arg(&self.table_name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to flush nftables table: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("Failed to flush nftables table: {}", stderr);
        }

        // Clear internal tracking
        let mut handles = self.rule_handles.write().await;
        handles.clear();
        drop(handles);

        let mut next_handle = self.next_handle.write().await;
        *next_handle = 1;
        drop(next_handle);

        info!("All nftables rules flushed and reset");

        Ok(())
    }

    /// Flush chain for specific VM
    pub async fn flush_vm_chain(&self, vm_id: &str) -> Result<()> {
        info!("Flushing firewall chain for VM {}", vm_id);

        let chain_name = format!("vm-{}", vm_id);

        // Flush the chain
        let output = Command::new("nft")
            .arg("flush")
            .arg("chain")
            .arg("inet")
            .arg(&self.table_name)
            .arg(&chain_name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to flush nftables chain: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("Failed to flush nftables chain: {}", stderr);
        }

        // Remove from tracking
        let mut handles = self.rule_handles.write().await;
        handles.retain(|k, _| !k.starts_with(&format!("vm-{}-", vm_id)));

        info!("Flushed firewall chain for VM {}", vm_id);

        Ok(())
    }

    /// Flush chain for specific container
    pub async fn flush_container_chain(&self, ct_id: &str) -> Result<()> {
        info!("Flushing firewall chain for container {}", ct_id);

        let chain_name = format!("ct-{}", ct_id);

        // Flush the chain
        let output = Command::new("nft")
            .arg("flush")
            .arg("chain")
            .arg("inet")
            .arg(&self.table_name)
            .arg(&chain_name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to flush nftables chain: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("Failed to flush nftables chain: {}", stderr);
        }

        // Remove from tracking
        let mut handles = self.rule_handles.write().await;
        handles.retain(|k, _| !k.starts_with(&format!("ct-{}-", ct_id)));

        info!("Flushed firewall chain for container {}", ct_id);

        Ok(())
    }

    /// List all rules in a chain
    pub async fn list_chain_rules(&self, chain_name: &str) -> Result<Vec<String>> {
        let output = Command::new("nft")
            .arg("list")
            .arg("chain")
            .arg("inet")
            .arg(&self.table_name)
            .arg(chain_name)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to list nftables chain: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to list nftables chain: {}",
                stderr
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let rules: Vec<String> = stdout
            .lines()
            .filter(|line| line.trim().starts_with("ip ") || line.trim().starts_with("tcp ") || line.trim().starts_with("udp "))
            .map(|s| s.trim().to_string())
            .collect();

        Ok(rules)
    }

    /// Check if nftables is available
    pub fn check_nftables_available() -> bool {
        std::process::Command::new("nft")
            .arg("--version")
            .output()
            .is_ok()
    }
}
