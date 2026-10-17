//! nftables integration for firewall rules
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
    _rule_id: String, // Reserved for future rule queries
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
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to create nftables table: {}", e))
            })?;

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
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to add nftables rule: {}", e))
            })?;

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
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to add nftables rule: {}", e))
            })?;

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
                .map_err(|e| {
                    horcrux_common::Error::System(format!("Failed to delete nftables rule: {}", e))
                })?;

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
                .map_err(|e| {
                    horcrux_common::Error::System(format!("Failed to delete nftables rule: {}", e))
                })?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                warn!("Failed to delete nftables rule: {}", stderr);
                // Don't return error - rule might already be deleted
            }

            info!("Removed firewall rule {} for container {}", rule_id, ct_id);
        } else {
            warn!(
                "Firewall rule {} not found for container {}",
                rule_id, ct_id
            );
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

    /// Build nftables rule from FirewallRule.
    ///
    /// Ports are matched as `tcp dport 80`; a protocol without ports as
    /// `meta l4proto tcp`. This used to emit `tcp tcp dport 80 accept`
    /// (protocol word twice), which nft rejects, so no port rule could
    /// ever be installed.
    pub(crate) fn build_nft_rule(&self, rule: &FirewallRule) -> String {
        let mut parts: Vec<String> = Vec::new();
        let l4 = match rule.protocol {
            Some(super::Protocol::Tcp) => Some("tcp"),
            Some(super::Protocol::Udp) => Some("udp"),
            Some(super::Protocol::Icmp) => Some("icmp"),
            Some(super::Protocol::Icmpv6) => Some("icmpv6"),
            Some(super::Protocol::Any) | None => None,
        };
        let has_ports = rule.sport.is_some() || rule.dport.is_some();
        let any_addr = |a: &Option<String>| {
            a.as_deref()
                .map(str::trim)
                .filter(|a| !a.is_empty() && *a != "0.0.0.0/0" && *a != "any")
                .map(str::to_string)
        };
        if let Some(src) = any_addr(&rule.source) {
            parts.push(format!("ip saddr {}", src));
        }
        if let Some(dst) = any_addr(&rule.dest) {
            parts.push(format!("ip daddr {}", dst));
        }
        match (l4, has_ports) {
            (Some(p @ ("tcp" | "udp")), true) => {
                if let Some(sport) = &rule.sport {
                    parts.push(format!("{} sport {}", p, sport));
                }
                if let Some(dport) = &rule.dport {
                    parts.push(format!("{} dport {}", p, dport));
                }
            }
            (Some(p), _) => parts.push(format!("meta l4proto {}", p)),
            (None, _) => {}
        }
        if rule.log {
            parts.push("log".to_string());
        }
        parts.push(
            match rule.action {
                FirewallAction::Accept => "accept",
                FirewallAction::Reject => "reject",
                FirewallAction::Drop => "drop",
            }
            .to_string(),
        );
        parts.join(" ")
    }

    /// Run one `nft` command; error with its stderr on failure.
    async fn nft(&self, args: &[&str]) -> Result<()> {
        let output = Command::new("nft").args(args).output().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to run nft (nftables): {}", e))
        })?;
        if !output.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "nftables: `nft {}` failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(())
    }

    /// Install host-level (datacenter / node) rules: replace the contents of
    /// the `<prefix>-in` (input hook) and `<prefix>-out` (output hook) base
    /// chains with `rules`. Chains default to `policy accept`, so applying
    /// never locks the host out by itself; VM/container chains are left
    /// alone.
    pub async fn apply_host_rules(&self, prefix: &str, rules: &[FirewallRule]) -> Result<()> {
        self.initialize().await?;
        for (dir, hook) in [("in", "input"), ("out", "output")] {
            let chain = format!("{}-{}", prefix, dir);
            let spec = format!(
                "{{ type filter hook {} priority 0 ; policy accept ; }}",
                hook
            );
            self.nft(&["add", "chain", "inet", &self.table_name, &chain, &spec])
                .await?;
            self.nft(&["flush", "chain", "inet", &self.table_name, &chain])
                .await?;
        }
        for rule in rules.iter().filter(|r| r.enabled) {
            let chain = match rule.direction {
                super::Direction::In => format!("{}-in", prefix),
                super::Direction::Out => format!("{}-out", prefix),
            };
            let expr = self.build_nft_rule(rule);
            self.nft(&["add", "rule", "inet", &self.table_name, &chain, &expr])
                .await?;
        }
        info!(
            "Applied {} {} firewall rule(s) to nftables",
            rules.len(),
            prefix
        );
        Ok(())
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
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to flush nftables table: {}", e))
            })?;

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
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to flush nftables chain: {}", e))
            })?;

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
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to flush nftables chain: {}", e))
            })?;

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
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to list nftables chain: {}", e))
            })?;

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
            .filter(|line| {
                line.trim().starts_with("ip ")
                    || line.trim().starts_with("tcp ")
                    || line.trim().starts_with("udp ")
            })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::firewall::{Direction, Protocol};

    fn rule(protocol: Option<Protocol>, dport: Option<&str>, source: Option<&str>) -> FirewallRule {
        FirewallRule {
            id: "r".into(),
            enabled: true,
            action: FirewallAction::Accept,
            direction: Direction::In,
            protocol,
            source: source.map(str::to_string),
            dest: None,
            sport: None,
            dport: dport.map(str::to_string),
            comment: None,
            log: false,
            position: 0,
        }
    }

    #[test]
    fn builds_valid_nft_expressions() {
        let m = NftablesManager::new();
        assert_eq!(
            m.build_nft_rule(&rule(Some(Protocol::Tcp), Some("8443"), None)),
            "tcp dport 8443 accept"
        );
        assert_eq!(
            m.build_nft_rule(&rule(Some(Protocol::Udp), None, Some("10.0.0.0/8"))),
            "ip saddr 10.0.0.0/8 meta l4proto udp accept"
        );
        // "any" addresses are not emitted as a match
        assert_eq!(
            m.build_nft_rule(&rule(None, None, Some("0.0.0.0/0"))),
            "accept"
        );
    }
}
