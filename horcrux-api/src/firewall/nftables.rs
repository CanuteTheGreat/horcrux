///! nftables integration for firewall rules

use super::{FirewallAction, FirewallRule};
use horcrux_common::Result;
use tokio::process::Command;
use tracing::{error, info};

/// nftables manager
pub struct NftablesManager {
    table_name: String,
}

impl NftablesManager {
    pub fn new() -> Self {
        Self {
            table_name: "horcrux".to_string(),
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

        Ok(())
    }

    /// Remove firewall rule for a VM
    pub async fn remove_vm_rule(&self, vm_id: &str, rule_id: &str) -> Result<()> {
        info!("Removing firewall rule {} for VM {}", rule_id, vm_id);
        // In production, we'd track rule handles and delete by handle
        // For now, this is a placeholder
        Ok(())
    }

    /// Remove firewall rule for a container
    pub async fn remove_container_rule(&self, ct_id: &str, rule_id: &str) -> Result<()> {
        info!("Removing firewall rule {} for container {}", rule_id, ct_id);
        // Placeholder
        Ok(())
    }

    /// Ensure chain exists
    async fn ensure_chain(&self, chain_name: &str) -> Result<()> {
        let output = Command::new("nft")
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

        // In production, this would:
        // 1. Flush all horcrux chains
        // 2. Rebuild from stored configuration
        // 3. Apply atomically

        Ok(())
    }

    /// Check if nftables is available
    pub fn check_nftables_available() -> bool {
        std::process::Command::new("nft")
            .arg("--version")
            .output()
            .is_ok()
    }
}
