//! Firewall management module
//! Provides distributed firewall with datacenter, node, VM, and container level rules
mod nftables;
pub mod objects;
mod security_groups;

/// Prefix of the kernel log lines nft writes for rules with `log` set.
pub const LOG_PREFIX: &str = "hx-fw: ";

use crate::db::config_store::{self, kinds, ConfigStore};
use crate::db::Database;
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
    pub source: Option<String>, // IP/CIDR or security group
    pub dest: Option<String>,   // IP/CIDR or security group
    pub sport: Option<String>,  // Source port or range
    pub dport: Option<String>,  // Dest port or range
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
    In,  // Incoming
    Out, // Outgoing
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

impl FirewallScope {
    /// Stable string form used as the persisted scope key
    /// (`datacenter`, `node:<name>`, `vm:<id>`, `container:<id>`).
    pub fn key(&self) -> String {
        match self {
            FirewallScope::Datacenter => "datacenter".to_string(),
            FirewallScope::Node(n) => format!("node:{}", n),
            FirewallScope::Vm(id) => format!("vm:{}", id),
            FirewallScope::Container(id) => format!("container:{}", id),
        }
    }

    /// Inverse of [`FirewallScope::key`].
    pub fn from_key(key: &str) -> Option<Self> {
        if key == "datacenter" {
            return Some(FirewallScope::Datacenter);
        }
        let (kind, id) = key.split_once(':')?;
        match kind {
            "node" => Some(FirewallScope::Node(id.to_string())),
            "vm" => Some(FirewallScope::Vm(id.to_string())),
            "container" => Some(FirewallScope::Container(id.to_string())),
            _ => None,
        }
    }
}

/// Persisted form of one rule: the rule plus the scope it belongs to.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredRule {
    scope: String,
    rule: FirewallRule,
}

fn stored_rule_id(scope: &FirewallScope, rule_id: &str) -> String {
    format!("{}/{}", scope.key(), rule_id)
}

/// Firewall manager
pub struct FirewallManager {
    datacenter_rules: Arc<RwLock<Vec<FirewallRule>>>,
    node_rules: Arc<RwLock<HashMap<String, Vec<FirewallRule>>>>,
    vm_rules: Arc<RwLock<HashMap<String, Vec<FirewallRule>>>>,
    container_rules: Arc<RwLock<HashMap<String, Vec<FirewallRule>>>>,
    security_groups: Arc<RwLock<HashMap<String, SecurityGroup>>>,
    nftables: nftables::NftablesManager,
    /// Host chains (`dc`, `node`) that have been applied, so object changes
    /// can refresh them.
    applied_host: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// Durable copy of rules and security groups; `None` = in-memory only.
    store: Option<ConfigStore>,
}

impl Default for FirewallManager {
    fn default() -> Self {
        Self::new()
    }
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
            applied_host: Arc::default(),
            store: None,
        }
    }

    /// Firewall manager backed by the database: loads persisted rules and
    /// security groups and writes every change through.
    ///
    /// Call [`FirewallManager::reapply_guest_rules`] after construction to
    /// reinstall VM/container chains into nftables: those rules take effect
    /// as soon as they are added, so after a host reboot the kernel ruleset
    /// must be rebuilt from the stored copy. Datacenter/node rules are only
    /// installed by an explicit apply, exactly as before.
    pub async fn with_database(db: Arc<Database>) -> Result<Self> {
        let store = ConfigStore::new(&db);
        let mut mgr = Self::new();
        let mut count = 0usize;
        for (id, stored) in store.load::<StoredRule>(kinds::FIREWALL_RULE).await? {
            let Some(scope) = FirewallScope::from_key(&stored.scope) else {
                tracing::warn!("Skipping firewall rule {} with unknown scope", id);
                continue;
            };
            mgr.push_rule(&scope, stored.rule).await;
            count += 1;
        }
        {
            let mut groups = mgr.security_groups.write().await;
            for group in store
                .load_values::<SecurityGroup>(kinds::SECURITY_GROUP)
                .await?
            {
                groups.insert(group.name.clone(), group);
            }
            tracing::info!(
                "Loaded {} firewall rule(s) and {} security group(s) from database",
                count,
                groups.len()
            );
        }
        let ipsets = store
            .load_values::<objects::IpSet>(kinds::FIREWALL_IPSET)
            .await?;
        let aliases = store
            .load_values::<objects::Alias>(kinds::FIREWALL_ALIAS)
            .await?;
        let options = store
            .load_values::<objects::FwOptions>(kinds::FIREWALL_OPTIONS)
            .await?
            .into_iter()
            .next();
        {
            let mut o = mgr
                .nftables
                .objects
                .write()
                .unwrap_or_else(|e| e.into_inner());
            for set in ipsets {
                o.ipsets.insert(set.name.clone(), set);
            }
            for a in aliases {
                o.aliases.insert(a.name.clone(), a);
            }
            if let Some(opt) = options {
                o.options = opt;
            }
        }
        mgr.store = Some(store);
        Ok(mgr)
    }

    /// Reinstall every stored VM/container rule set into nftables. Failures
    /// (e.g. no nft binary or no CAP_NET_ADMIN) are logged, not fatal.
    pub async fn reapply_guest_rules(&self) {
        let mut scopes: Vec<FirewallScope> = Vec::new();
        for (id, rules) in self.vm_rules.read().await.iter() {
            if !rules.is_empty() {
                scopes.push(FirewallScope::Vm(id.clone()));
            }
        }
        for (id, rules) in self.container_rules.read().await.iter() {
            if !rules.is_empty() {
                scopes.push(FirewallScope::Container(id.clone()));
            }
        }
        for scope in scopes {
            if let Err(e) = self.apply_rules(scope.clone()).await {
                tracing::warn!(
                    "Failed to reapply firewall rules for {}: {}",
                    scope.key(),
                    e
                );
            }
        }
    }

    /// Ids of the containers that have firewall rules.
    pub async fn containers_with_rules(&self) -> Vec<String> {
        self.container_rules
            .read()
            .await
            .iter()
            .filter(|(_, r)| !r.is_empty())
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Append a rule to the in-memory list for a scope (no nftables, no DB).
    async fn push_rule(&self, scope: &FirewallScope, rule: FirewallRule) {
        match scope {
            FirewallScope::Datacenter => self.datacenter_rules.write().await.push(rule),
            FirewallScope::Node(n) => self
                .node_rules
                .write()
                .await
                .entry(n.clone())
                .or_default()
                .push(rule),
            FirewallScope::Vm(id) => self
                .vm_rules
                .write()
                .await
                .entry(id.clone())
                .or_default()
                .push(rule),
            FirewallScope::Container(id) => self
                .container_rules
                .write()
                .await
                .entry(id.clone())
                .or_default()
                .push(rule),
        }
    }

    /// Add a firewall rule
    pub async fn add_rule(&self, scope: FirewallScope, rule: FirewallRule) -> Result<()> {
        self.nftables
            .objects
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .check_rule_refs(&rule.source, &rule.dest, &rule.dport)?;
        // Rule ids are the handle for delete and the persistence key, so they
        // must be unique within a scope (a duplicate used to be appended
        // silently and then removed together with the original).
        if self
            .list_rules(scope.clone())
            .await
            .iter()
            .any(|r| r.id == rule.id)
        {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Firewall rule {} already exists in scope {}",
                rule.id,
                scope.key()
            )));
        }
        config_store::put_opt(
            &self.store,
            kinds::FIREWALL_RULE,
            &stored_rule_id(&scope, &rule.id),
            &StoredRule {
                scope: scope.key(),
                rule: rule.clone(),
            },
        )
        .await?;
        match scope {
            FirewallScope::Datacenter => {
                let mut rules = self.datacenter_rules.write().await;
                rules.push(rule.clone());
            }
            FirewallScope::Node(node) => {
                let mut rules = self.node_rules.write().await;
                rules
                    .entry(node)
                    .or_insert_with(Vec::new)
                    .push(rule.clone());
            }
            FirewallScope::Vm(vm_id) => {
                let mut rules = self.vm_rules.write().await;
                rules
                    .entry(vm_id.clone())
                    .or_insert_with(Vec::new)
                    .push(rule.clone());

                drop(rules);
                // Reinstall this VM's whole rule set (bridge family, matched
                // on the VM's tap devices).
                let all = self.list_rules(FirewallScope::Vm(vm_id.clone())).await;
                self.nftables.sync_vm_rules(&vm_id, &all).await?;
            }
            FirewallScope::Container(ct_id) => {
                let mut rules = self.container_rules.write().await;
                rules
                    .entry(ct_id.clone())
                    .or_insert_with(Vec::new)
                    .push(rule.clone());

                drop(rules);
                // Reinstall this container's whole rule set (bridge family,
                // matched on the container's stable host-side veths).
                let all = self
                    .list_rules(FirewallScope::Container(ct_id.clone()))
                    .await;
                self.nftables.sync_ct_rules(&ct_id, &all).await?;
            }
        }

        Ok(())
    }

    /// Remove a firewall rule
    pub async fn remove_rule(&self, scope: FirewallScope, rule_id: &str) -> Result<()> {
        config_store::delete_opt(
            &self.store,
            kinds::FIREWALL_RULE,
            &stored_rule_id(&scope, rule_id),
        )
        .await?;
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

                drop(rules);
                let all = self.list_rules(FirewallScope::Vm(vm_id.clone())).await;
                self.nftables.sync_vm_rules(&vm_id, &all).await?;
            }
            FirewallScope::Container(ct_id) => {
                let mut rules = self.container_rules.write().await;
                if let Some(ct_rules) = rules.get_mut(&ct_id) {
                    ct_rules.retain(|r| r.id != rule_id);
                }

                drop(rules);
                let all = self
                    .list_rules(FirewallScope::Container(ct_id.clone()))
                    .await;
                self.nftables.sync_ct_rules(&ct_id, &all).await?;
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

        config_store::put_opt(&self.store, kinds::SECURITY_GROUP, &group.name, &group).await?;
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
        let group = groups.get(group_name).ok_or_else(|| {
            horcrux_common::Error::System(format!("Security group {} not found", group_name))
        })?;

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
        groups.get(name).cloned().ok_or_else(|| {
            horcrux_common::Error::System(format!("Security group {} not found", name))
        })
    }

    /// Apply firewall rules for a specific scope to nftables.
    ///
    /// Datacenter/node rules go into host input/output chains; VM and
    /// container rules replace that guest's chain. This used to skip
    /// datacenter/node rules entirely and then flush the whole table, so
    /// "Apply" installed nothing (and wiped any VM/container rules).
    pub async fn apply_rules(&self, scope: FirewallScope) -> Result<()> {
        let rules = self.list_rules(scope.clone()).await;
        match &scope {
            FirewallScope::Datacenter => {
                self.nftables.apply_host_rules("dc", &rules).await?;
                self.applied_host.lock().unwrap().insert("dc".into());
            }
            FirewallScope::Node(_) => {
                self.nftables.apply_host_rules("node", &rules).await?;
                self.applied_host.lock().unwrap().insert("node".into());
            }
            FirewallScope::Vm(vm_id) => self.nftables.sync_vm_rules(vm_id, &rules).await?,
            FirewallScope::Container(ct_id) => self.nftables.sync_ct_rules(ct_id, &rules).await?,
        }
        Ok(())
    }

    // ---- IP sets, aliases, host options -------------------------------

    pub fn list_ipsets(&self) -> Vec<objects::IpSet> {
        let o = self
            .nftables
            .objects
            .read()
            .unwrap_or_else(|e| e.into_inner());
        o.ipsets.values().cloned().collect()
    }

    pub fn list_aliases(&self) -> Vec<objects::Alias> {
        let o = self
            .nftables
            .objects
            .read()
            .unwrap_or_else(|e| e.into_inner());
        o.aliases.values().cloned().collect()
    }

    pub fn options(&self) -> objects::FwOptions {
        self.nftables
            .objects
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .options
            .clone()
    }

    /// Create or replace an IP set (entries are validated), then refresh the
    /// rules that use it in the kernel.
    pub async fn put_ipset(&self, set: objects::IpSet) -> Result<()> {
        if !objects::valid_name(&set.name) {
            return Err(horcrux_common::Error::InvalidConfig(
                "IP set name: letters, digits, '-' and '_', starting with a letter".into(),
            ));
        }
        if let Some(bad) = set.entries.iter().find(|e| !objects::valid_cidr(e)) {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "{bad:?} is not an IPv4 address or CIDR"
            )));
        }
        config_store::put_opt(&self.store, kinds::FIREWALL_IPSET, &set.name, &set).await?;
        self.nftables
            .objects
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .ipsets
            .insert(set.name.clone(), set);
        self.reapply_after_object_change().await;
        Ok(())
    }

    /// Delete an IP set that no rule refers to.
    pub async fn delete_ipset(&self, name: &str) -> Result<bool> {
        if let Some(scope) = self.rule_using(&format!("+{name}")).await {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "IP set {name} is used by a rule in scope {scope}"
            )));
        }
        let existed = self
            .nftables
            .objects
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .ipsets
            .remove(name)
            .is_some();
        config_store::delete_opt(&self.store, kinds::FIREWALL_IPSET, name).await?;
        Ok(existed)
    }

    pub async fn put_alias(&self, alias: objects::Alias) -> Result<()> {
        if !objects::valid_name(&alias.name) || alias.name == "any" {
            return Err(horcrux_common::Error::InvalidConfig(
                "alias name: letters, digits, '-' and '_', starting with a letter".into(),
            ));
        }
        if !objects::valid_cidr(&alias.cidr) {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "{:?} is not an IPv4 address or CIDR",
                alias.cidr
            )));
        }
        config_store::put_opt(&self.store, kinds::FIREWALL_ALIAS, &alias.name, &alias).await?;
        self.nftables
            .objects
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .aliases
            .insert(alias.name.clone(), alias);
        self.reapply_after_object_change().await;
        Ok(())
    }

    pub async fn delete_alias(&self, name: &str) -> Result<bool> {
        if let Some(scope) = self.rule_using(name).await {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "alias {name} is used by a rule in scope {scope}"
            )));
        }
        let existed = self
            .nftables
            .objects
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .aliases
            .remove(name)
            .is_some();
        config_store::delete_opt(&self.store, kinds::FIREWALL_ALIAS, name).await?;
        Ok(existed)
    }

    /// Set the host default policies; applied right away to host chains that
    /// are already installed.
    pub async fn set_options(&self, opt: objects::FwOptions) -> Result<()> {
        opt.validate()?;
        config_store::put_opt(&self.store, kinds::FIREWALL_OPTIONS, "options", &opt).await?;
        self.nftables
            .objects
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .options = opt;
        self.reapply_after_object_change().await;
        Ok(())
    }

    /// The scope of a rule whose source or dest is exactly `reference`.
    async fn rule_using(&self, reference: &str) -> Option<String> {
        let mut scopes = vec![FirewallScope::Datacenter];
        scopes.extend(
            self.node_rules
                .read()
                .await
                .keys()
                .cloned()
                .map(FirewallScope::Node),
        );
        scopes.extend(
            self.vm_rules
                .read()
                .await
                .keys()
                .cloned()
                .map(FirewallScope::Vm),
        );
        scopes.extend(
            self.container_rules
                .read()
                .await
                .keys()
                .cloned()
                .map(FirewallScope::Container),
        );
        for scope in scopes {
            for r in self.list_rules(scope.clone()).await {
                let hit = |a: &Option<String>| a.as_deref().map(str::trim) == Some(reference);
                if hit(&r.source) || hit(&r.dest) {
                    return Some(scope.key());
                }
            }
        }
        None
    }

    /// Guest rule sets and already-applied host chains are rebuilt so the
    /// kernel matches the objects. Failures (no nft, no CAP_NET_ADMIN) are
    /// logged: the objects are stored either way and apply on the next apply.
    async fn reapply_after_object_change(&self) {
        self.reapply_guest_rules().await;
        let host: Vec<String> = self.applied_host.lock().unwrap().iter().cloned().collect();
        for prefix in host {
            let scope = if prefix == "dc" {
                FirewallScope::Datacenter
            } else {
                // node rules of every node that has some
                let nodes: Vec<String> = self.node_rules.read().await.keys().cloned().collect();
                for n in nodes {
                    let _ = self.apply_rules(FirewallScope::Node(n)).await;
                }
                continue;
            };
            if let Err(e) = self.apply_rules(scope).await {
                tracing::warn!("Failed to refresh the datacenter firewall: {e}");
            }
        }
    }

    /// Forget every rule of a VM or container that is gone (deleted, or
    /// migrated to another node): stored rules, and its chains and jumps in
    /// the kernel. Otherwise they'd be re-installed at every start and a
    /// new guest that gets the same id would inherit them.
    pub async fn remove_guest_rules(&self, scope: FirewallScope) -> Result<()> {
        let (kind, id) = match &scope {
            FirewallScope::Vm(id) => (nftables::GuestKind::Vm, id.clone()),
            FirewallScope::Container(id) => (nftables::GuestKind::Container, id.clone()),
            _ => {
                return Err(horcrux_common::Error::Validation(
                    "only VM and container firewalls belong to a guest".into(),
                ))
            }
        };
        for rule in self.list_rules(scope.clone()).await {
            config_store::delete_opt(
                &self.store,
                kinds::FIREWALL_RULE,
                &stored_rule_id(&scope, &rule.id),
            )
            .await?;
        }
        match &scope {
            FirewallScope::Vm(id) => {
                self.vm_rules.write().await.remove(id);
            }
            FirewallScope::Container(id) => {
                self.container_rules.write().await.remove(id);
            }
            _ => {}
        }
        if let Err(e) = self.nftables.remove_guest(kind, &id).await {
            tracing::warn!("firewall of {} not removed from nftables: {e}", scope.key());
        }
        Ok(())
    }
}

/// Persistence through the config store (moved from db/config_store.rs so
/// the database layer no longer depends on this module).
#[cfg(test)]
#[allow(unused_imports)]
mod config_store_persistence_tests {
    use crate::db::config_store::{kinds, ConfigStore};
    use crate::db::Database;

    async fn store() -> (tempfile::TempDir, Database, ConfigStore) {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}", dir.path().join("t.db").display());
        let db = Database::new(&url).await.unwrap();
        db.migrate().await.unwrap();
        let s = ConfigStore::new(&db);
        (dir, db, s)
    }

    #[tokio::test]
    async fn firewall_rules_reload_into_their_scopes() {
        use crate::firewall::{
            Direction, FirewallAction, FirewallManager, FirewallRule, FirewallScope,
        };
        let (_dir, db, _s) = store().await;
        let db = std::sync::Arc::new(db);
        let rule = |id: &str| FirewallRule {
            id: id.into(),
            enabled: true,
            action: FirewallAction::Accept,
            direction: Direction::In,
            protocol: None,
            source: None,
            dest: None,
            sport: None,
            dport: Some("22".into()),
            comment: None,
            log: false,
            position: 0,
        };
        {
            let fw = FirewallManager::with_database(db.clone()).await.unwrap();
            fw.add_rule(FirewallScope::Datacenter, rule("a"))
                .await
                .unwrap();
            fw.add_rule(FirewallScope::Datacenter, rule("b"))
                .await
                .unwrap();
            fw.add_rule(FirewallScope::Node("n1".into()), rule("a"))
                .await
                .unwrap();
            // Duplicate id within a scope is rejected.
            assert!(fw
                .add_rule(FirewallScope::Datacenter, rule("a"))
                .await
                .is_err());
            fw.remove_rule(FirewallScope::Datacenter, "a")
                .await
                .unwrap();
        }
        let fw = FirewallManager::with_database(db).await.unwrap();
        let dc: Vec<String> = fw
            .list_rules(FirewallScope::Datacenter)
            .await
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(dc, vec!["b".to_string()]);
        assert_eq!(
            fw.list_rules(FirewallScope::Node("n1".into())).await.len(),
            1
        );
        for scope in [
            FirewallScope::Datacenter,
            FirewallScope::Node("x:y".into()),
            FirewallScope::Vm("v".into()),
            FirewallScope::Container("c".into()),
        ] {
            assert_eq!(FirewallScope::from_key(&scope.key()), Some(scope));
        }
    }
}
