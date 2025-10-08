///! Firewall management module
///! Provides distributed firewall with datacenter, node, VM, and container level rules

mod nftables;
mod security_groups;

use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Firewall rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirewallRule {
    pub id: String,
    pub enabled: bool,
    pub action: FirewallAction,
    pub direction: Direction,
    pub protocol: Option<Protocol>,
    pub source: Option<String>,      // IP/CIDR or security group
    pub dest: Option<String>,         // IP/CIDR or security group
    pub sport: Option<String>,        // Source port or range
    pub dport: Option<String>,        // Dest port or range
    pub comment: Option<String>,
    pub log: bool,
    pub position: u32,
}

/// Firewall action
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum FirewallAction {
    Accept,
    Reject,
    Drop,
}

/// Traffic direction
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    In,   // Incoming
    Out,  // Outgoing
}

/// Network protocol
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Tcp,
    Udp,
    Icmp,
    Icmpv6,
    Any,
}

/// Security group (reusable set of rules)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityGroup {
    pub name: String,
    pub description: String,
    pub rules: Vec<FirewallRule>,
}

/// Firewall configuration scope
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirewallScope {
    Datacenter,
    Node(String),
    Vm(String),
    Container(String),
}

/// Firewall manager
pub struct FirewallManager {
    datacenter_rules: Arc<RwLock<Vec<FirewallRule>>>,
    node_rules: Arc<RwLock<HashMap<String, Vec<FirewallRule>>>>,
    vm_rules: Arc<RwLock<HashMap<String, Vec<FirewallRule>>>>,
    container_rules: Arc<RwLock<HashMap<String, Vec<FirewallRule>>>>,
    security_groups: Arc<RwLock<HashMap<String, SecurityGroup>>>,
    nftables: nftables::NftablesManager,
}

impl FirewallManager {
    pub fn new() -> Self {
        Self {
            datacenter_rules: Arc::new(RwLock::new(Vec::new())),
            node_rules: Arc::new(RwLock::new(HashMap::new())),
            vm_rules: Arc::new(RwLock::new(HashMap::new())),
            container_rules: Arc::new(RwLock::new(HashMap::new())),
            security_groups: Arc::new(RwLock::new(HashMap::new())),
            nftables: nftables::NftablesManager::new(),
        }
    }

    /// Add a firewall rule
    pub async fn add_rule(&self, scope: FirewallScope, rule: FirewallRule) -> Result<()> {
        match scope {
            FirewallScope::Datacenter => {
                let mut rules = self.datacenter_rules.write().await;
                rules.push(rule.clone());
            }
            FirewallScope::Node(node) => {
                let mut rules = self.node_rules.write().await;
                rules.entry(node).or_insert_with(Vec::new).push(rule.clone());
            }
            FirewallScope::Vm(vm_id) => {
                let mut rules = self.vm_rules.write().await;
                rules.entry(vm_id.clone()).or_insert_with(Vec::new).push(rule.clone());

                // Apply to nftables for this VM
                self.nftables.add_vm_rule(&vm_id, &rule).await?;
            }
            FirewallScope::Container(ct_id) => {
                let mut rules = self.container_rules.write().await;
                rules.entry(ct_id.clone()).or_insert_with(Vec::new).push(rule.clone());

                // Apply to nftables for this container
                self.nftables.add_container_rule(&ct_id, &rule).await?;
            }
        }

        Ok(())
    }

    /// Remove a firewall rule
    pub async fn remove_rule(&self, scope: FirewallScope, rule_id: &str) -> Result<()> {
        match scope {
            FirewallScope::Datacenter => {
                let mut rules = self.datacenter_rules.write().await;
                rules.retain(|r| r.id != rule_id);
            }
            FirewallScope::Node(node) => {
                let mut rules = self.node_rules.write().await;
                if let Some(node_rules) = rules.get_mut(&node) {
                    node_rules.retain(|r| r.id != rule_id);
                }
            }
            FirewallScope::Vm(vm_id) => {
                let mut rules = self.vm_rules.write().await;
                if let Some(vm_rules) = rules.get_mut(&vm_id) {
                    vm_rules.retain(|r| r.id != rule_id);
                }

                // Remove from nftables
                self.nftables.remove_vm_rule(&vm_id, rule_id).await?;
            }
            FirewallScope::Container(ct_id) => {
                let mut rules = self.container_rules.write().await;
                if let Some(ct_rules) = rules.get_mut(&ct_id) {
                    ct_rules.retain(|r| r.id != rule_id);
                }

                // Remove from nftables
                self.nftables.remove_container_rule(&ct_id, rule_id).await?;
            }
        }

        Ok(())
    }

    /// List rules for a scope
    pub async fn list_rules(&self, scope: FirewallScope) -> Vec<FirewallRule> {
        match scope {
            FirewallScope::Datacenter => {
                let rules = self.datacenter_rules.read().await;
                rules.clone()
            }
            FirewallScope::Node(node) => {
                let rules = self.node_rules.read().await;
                rules.get(&node).cloned().unwrap_or_default()
            }
            FirewallScope::Vm(vm_id) => {
                let rules = self.vm_rules.read().await;
                rules.get(&vm_id).cloned().unwrap_or_default()
            }
            FirewallScope::Container(ct_id) => {
                let rules = self.container_rules.read().await;
                rules.get(&ct_id).cloned().unwrap_or_default()
            }
        }
    }

    /// Create a security group
    pub async fn create_security_group(&self, group: SecurityGroup) -> Result<()> {
        let mut groups = self.security_groups.write().await;

        if groups.contains_key(&group.name) {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Security group {} already exists",
                group.name
            )));
        }

        groups.insert(group.name.clone(), group);
        Ok(())
    }

    /// Apply security group to VM or container
    pub async fn apply_security_group(
        &self,
        group_name: &str,
        target: FirewallScope,
    ) -> Result<()> {
        let groups = self.security_groups.read().await;
        let group = groups
            .get(group_name)
            .ok_or_else(|| horcrux_common::Error::System(format!("Security group {} not found", group_name)))?;

        // Apply all rules from the security group
        for rule in &group.rules {
            self.add_rule(target.clone(), rule.clone()).await?;
        }

        Ok(())
    }

    /// List all security groups
    pub async fn list_security_groups(&self) -> Vec<SecurityGroup> {
        let groups = self.security_groups.read().await;
        groups.values().cloned().collect()
    }

    /// Apply all firewall rules (reload)
    pub async fn apply_all(&self) -> Result<()> {
        // This would rebuild the entire nftables ruleset
        self.nftables.reload_all().await?;
        Ok(())
    }

    /// Delete a firewall rule
    pub async fn delete_rule(&self, scope: FirewallScope, rule_id: &str) -> Result<()> {
        self.remove_rule(scope, rule_id).await
    }

    /// Get security group by name
    pub async fn get_security_group(&self, name: &str) -> Result<SecurityGroup> {
        let groups = self.security_groups.read().await;
        groups
            .get(name)
            .cloned()
            .ok_or_else(|| horcrux_common::Error::System(format!("Security group {} not found", name)))
    }

    /// Apply firewall rules for a specific scope
    pub async fn apply_rules(&self, scope: FirewallScope) -> Result<()> {
        // Apply rules for the specified scope
        let rules = self.list_rules(scope.clone()).await;
        for rule in rules {
            match &scope {
                FirewallScope::Vm(vm_id) => {
                    self.nftables.add_vm_rule(vm_id, &rule).await?;
                }
                FirewallScope::Container(ct_id) => {
                    self.nftables.add_container_rule(ct_id, &rule).await?;
                }
                _ => {
                    // For datacenter and node level, just reload all
                    continue;
                }
            }
        }

        // Reload the full ruleset
        self.nftables.reload_all().await?;
        Ok(())
    }
}
