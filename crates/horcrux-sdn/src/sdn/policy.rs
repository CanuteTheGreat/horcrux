//! Network Policy Enforcement
//! Provides Kubernetes-style network policies for traffic filtering
use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Network policy specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkPolicy {
    pub id: String,
    pub name: String,
    pub namespace: String,
    pub pod_selector: LabelSelector,
    pub policy_types: Vec<PolicyType>,
    pub ingress: Vec<IngressRule>,
    pub egress: Vec<EgressRule>,
    pub enabled: bool,
}

/// Policy type (ingress or egress)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PolicyType {
    Ingress,
    Egress,
}

/// Label selector for pod matching
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LabelSelector {
    pub match_labels: HashMap<String, String>,
    pub match_expressions: Vec<LabelExpression>,
}

/// Label expression for advanced matching
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelExpression {
    pub key: String,
    pub operator: LabelOperator,
    pub values: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum LabelOperator {
    In,
    NotIn,
    Exists,
    DoesNotExist,
}

/// Ingress rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngressRule {
    pub from: Vec<PeerSelector>,
    pub ports: Vec<NetworkPolicyPort>,
}

/// Egress rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EgressRule {
    pub to: Vec<PeerSelector>,
    pub ports: Vec<NetworkPolicyPort>,
}

/// Peer selector (pod, namespace, or IP block)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PeerSelector {
    PodSelector(LabelSelector),
    NamespaceSelector(LabelSelector),
    IpBlock { cidr: String, except: Vec<String> },
}

/// Network policy port specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkPolicyPort {
    pub protocol: Protocol,
    pub port: Option<u16>,
    pub end_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[allow(clippy::upper_case_acronyms)]
pub enum Protocol {
    TCP,
    UDP,
    SCTP,
}

/// Network policy manager
pub struct NetworkPolicyManager {
    policies: HashMap<String, NetworkPolicy>,
    // Mapping from pod ID to applicable policies
    pod_policies: HashMap<String, Vec<String>>,
    // Mapping from namespace to policies
    namespace_policies: HashMap<String, Vec<String>>,
    // Mapping from pod ID to (namespace, labels), used to evaluate peer
    // selectors (`from`/`to`) in is_connection_allowed()
    pod_metadata: HashMap<String, (String, HashMap<String, String>)>,
}

impl Default for NetworkPolicyManager {
    fn default() -> Self {
        Self::new()
    }
}

impl NetworkPolicyManager {
    pub fn new() -> Self {
        Self {
            policies: HashMap::new(),
            pod_policies: HashMap::new(),
            namespace_policies: HashMap::new(),
            pod_metadata: HashMap::new(),
        }
    }

    /// Create a network policy
    pub fn create_policy(&mut self, policy: NetworkPolicy) -> Result<()> {
        // Validate policy
        if policy.name.is_empty() {
            return Err(horcrux_common::Error::System(
                "Policy name cannot be empty".to_string(),
            ));
        }

        // Add to namespace index
        self.namespace_policies
            .entry(policy.namespace.clone())
            .or_default()
            .push(policy.id.clone());

        self.policies.insert(policy.id.clone(), policy);
        tracing::info!("Created network policy: {}", self.policies.len());

        Ok(())
    }

    /// Delete a network policy
    pub fn delete_policy(&mut self, policy_id: &str) -> Result<()> {
        if let Some(policy) = self.policies.remove(policy_id) {
            // Remove from namespace index
            if let Some(ns_policies) = self.namespace_policies.get_mut(&policy.namespace) {
                ns_policies.retain(|id| id != policy_id);
            }

            // Remove from pod index
            self.pod_policies.values_mut().for_each(|policies| {
                policies.retain(|id| id != policy_id);
            });

            tracing::info!("Deleted network policy: {}", policy_id);
        }

        Ok(())
    }

    /// List all policies
    pub fn list_policies(&self) -> Vec<NetworkPolicy> {
        self.policies.values().cloned().collect()
    }

    /// List policies in a namespace
    pub fn list_policies_in_namespace(&self, namespace: &str) -> Vec<NetworkPolicy> {
        if let Some(policy_ids) = self.namespace_policies.get(namespace) {
            policy_ids
                .iter()
                .filter_map(|id| self.policies.get(id).cloned())
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Get a specific policy
    pub fn get_policy(&self, policy_id: &str) -> Option<&NetworkPolicy> {
        self.policies.get(policy_id)
    }

    /// Update policy for a pod (recalculate applicable policies)
    pub fn update_pod_policies(
        &mut self,
        pod_id: &str,
        pod_labels: &HashMap<String, String>,
        namespace: &str,
    ) {
        let applicable_policies: Vec<String> = self
            .policies
            .values()
            .filter(|policy| {
                policy.enabled
                    && policy.namespace == namespace
                    && self.matches_selector(&policy.pod_selector, pod_labels)
            })
            .map(|policy| policy.id.clone())
            .collect();

        self.pod_policies
            .insert(pod_id.to_string(), applicable_policies);
        self.pod_metadata.insert(
            pod_id.to_string(),
            (namespace.to_string(), pod_labels.clone()),
        );
        tracing::debug!(
            "Updated policies for pod {}: {} policies",
            pod_id,
            self.pod_policies.get(pod_id).map(|p| p.len()).unwrap_or(0)
        );
    }

    /// Check if a connection is allowed by network policies
    pub fn is_connection_allowed(
        &self,
        src_pod: &str,
        dst_pod: &str,
        protocol: &Protocol,
        port: u16,
        direction: &PolicyType,
    ) -> bool {
        // Get policies for the destination pod
        let applicable_policies = match self.pod_policies.get(dst_pod) {
            Some(policies) => policies,
            None => return true, // No policies = allow all
        };

        if applicable_policies.is_empty() {
            return true; // No policies = allow all
        }

        // Check each applicable policy
        for policy_id in applicable_policies {
            if let Some(policy) = self.policies.get(policy_id) {
                if !policy.enabled {
                    continue;
                }

                // Check if policy applies to this direction
                if !policy.policy_types.contains(direction) {
                    continue;
                }

                match direction {
                    PolicyType::Ingress => {
                        // Check ingress rules: both the port and the peer
                        // selector (`from`) must match for the rule to grant
                        // access -- previously `from` was never evaluated,
                        // which meant any policy with a port-matching ingress
                        // rule allowed traffic from every source regardless
                        // of its configured peer selectors.
                        for rule in &policy.ingress {
                            if self.matches_port(&rule.ports, protocol, port)
                                && self.matches_peer(&rule.from, src_pod)
                            {
                                return true;
                            }
                        }
                    }
                    PolicyType::Egress => {
                        // Check egress rules (same peer-selector enforcement as ingress)
                        for rule in &policy.egress {
                            if self.matches_port(&rule.ports, protocol, port)
                                && self.matches_peer(&rule.to, src_pod)
                            {
                                return true;
                            }
                        }
                    }
                }
            }
        }

        // If we have policies but no match, deny
        false
    }

    /// Generate iptables rules for a policy
    pub fn generate_iptables_rules(&self, policy_id: &str) -> Vec<String> {
        let policy = match self.policies.get(policy_id) {
            Some(p) => p,
            None => return Vec::new(),
        };

        let mut rules = Vec::new();

        // Chain name based on policy ID
        let chain_name = format!(
            "HORCRUX-POL-{}",
            policy_id.chars().take(8).collect::<String>().to_uppercase()
        );

        // Create custom chain
        rules.push(format!("iptables -N {}", chain_name));

        // Ingress rules: peers in `from` restrict the *source* address.
        if policy.policy_types.contains(&PolicyType::Ingress) {
            for rule in &policy.ingress {
                Self::push_iptables_port_rules(
                    &mut rules,
                    &chain_name,
                    &rule.from,
                    &rule.ports,
                    true,
                );
            }
        }

        // Egress rules: peers in `to` restrict the *destination* address.
        if policy.policy_types.contains(&PolicyType::Egress) {
            for rule in &policy.egress {
                Self::push_iptables_port_rules(
                    &mut rules,
                    &chain_name,
                    &rule.to,
                    &rule.ports,
                    false,
                );
            }
        }

        // Default deny at end of chain
        rules.push(format!("iptables -A {} -j DROP", chain_name));

        rules
    }

    /// Emit iptables rules for one ingress/egress rule's ports, honoring any
    /// `IpBlock` peer selectors as a real `-s`/`-d` address restriction.
    ///
    /// Previously this translation silently dropped `from`/`to` entirely --
    /// a policy scoped to a specific CIDR (e.g. "allow 8080 only from
    /// 10.0.0.0/8") materialized into a rule that accepted the port from
    /// *any* source, because only the port was ever checked. `IpBlock`
    /// peers are concrete addresses we can enforce directly in the
    /// generated rule; `PodSelector`/`NamespaceSelector` peers require
    /// resolving pod IPs, which this layer doesn't have, so those are
    /// called out with an explicit comment instead of being silently
    /// ignored.
    fn push_iptables_port_rules(
        rules: &mut Vec<String>,
        chain_name: &str,
        peers: &[PeerSelector],
        ports: &[NetworkPolicyPort],
        is_ingress: bool,
    ) {
        let addr_flag = if is_ingress { "-s" } else { "-d" };
        let ip_blocks: Vec<(&str, &[String])> = peers
            .iter()
            .filter_map(|p| match p {
                PeerSelector::IpBlock { cidr, except } => Some((cidr.as_str(), except.as_slice())),
                _ => None,
            })
            .collect();
        let has_unresolvable_peer = peers.iter().any(|p| {
            matches!(
                p,
                PeerSelector::PodSelector(_) | PeerSelector::NamespaceSelector(_)
            )
        });

        if has_unresolvable_peer {
            rules.push(format!(
                "# WARNING: chain {} has a Pod/Namespace peer selector that cannot be resolved to concrete addresses at rule-generation time -- the rule(s) below are NOT restricted to that selector and allow the listed port(s) from any address",
                chain_name
            ));
        }

        for port_spec in ports {
            let protocol = match port_spec.protocol {
                Protocol::TCP => "tcp",
                Protocol::UDP => "udp",
                Protocol::SCTP => "sctp",
            };

            let Some(port) = port_spec.port else {
                continue;
            };

            if ip_blocks.is_empty() {
                // No IpBlock peers: either no peer restriction at all, or
                // only unresolvable Pod/Namespace selectors (warned above).
                rules.push(format!(
                    "iptables -A {} -p {} --dport {} -j ACCEPT",
                    chain_name, protocol, port
                ));
            } else {
                for (cidr, except) in &ip_blocks {
                    // Exclusions must be evaluated before the broader CIDR
                    // accept, since iptables chains match top-down.
                    for excluded in *except {
                        rules.push(format!(
                            "iptables -A {} {} {} -p {} --dport {} -j DROP",
                            chain_name, addr_flag, excluded, protocol, port
                        ));
                    }
                    rules.push(format!(
                        "iptables -A {} {} {} -p {} --dport {} -j ACCEPT",
                        chain_name, addr_flag, cidr, protocol, port
                    ));
                }
            }
        }
    }

    /// Generate nftables rules for a policy
    pub fn generate_nftables_rules(&self, policy_id: &str) -> Vec<String> {
        let policy = match self.policies.get(policy_id) {
            Some(p) => p,
            None => return Vec::new(),
        };

        let mut rules = Vec::new();

        // Table and chain setup
        rules.push("nft add table inet horcrux".to_string());
        rules.push(format!("nft add chain inet horcrux policy_{}", policy_id));

        // Ingress rules: peers in `from` restrict the *source* address.
        if policy.policy_types.contains(&PolicyType::Ingress) {
            for rule in &policy.ingress {
                Self::push_nftables_port_rules(
                    &mut rules,
                    policy_id,
                    &rule.from,
                    &rule.ports,
                    true,
                );
            }
        }

        // Egress rules: peers in `to` restrict the *destination* address.
        if policy.policy_types.contains(&PolicyType::Egress) {
            for rule in &policy.egress {
                Self::push_nftables_port_rules(&mut rules, policy_id, &rule.to, &rule.ports, false);
            }
        }

        // Default drop
        rules.push(format!(
            "nft add rule inet horcrux policy_{} drop",
            policy_id
        ));

        rules
    }

    /// nftables equivalent of `push_iptables_port_rules` -- same IpBlock
    /// peer-selector enforcement (`saddr`/`daddr`), same honest warning for
    /// peer selectors that can't be resolved to addresses at this layer.
    fn push_nftables_port_rules(
        rules: &mut Vec<String>,
        policy_id: &str,
        peers: &[PeerSelector],
        ports: &[NetworkPolicyPort],
        is_ingress: bool,
    ) {
        let addr_field = if is_ingress { "saddr" } else { "daddr" };
        let ip_blocks: Vec<(&str, &[String])> = peers
            .iter()
            .filter_map(|p| match p {
                PeerSelector::IpBlock { cidr, except } => Some((cidr.as_str(), except.as_slice())),
                _ => None,
            })
            .collect();
        let has_unresolvable_peer = peers.iter().any(|p| {
            matches!(
                p,
                PeerSelector::PodSelector(_) | PeerSelector::NamespaceSelector(_)
            )
        });

        if has_unresolvable_peer {
            rules.push(format!(
                "# WARNING: policy_{} has a Pod/Namespace peer selector that cannot be resolved to concrete addresses at rule-generation time -- the rule(s) below are NOT restricted to that selector and allow the listed port(s) from any address",
                policy_id
            ));
        }

        for port_spec in ports {
            let protocol = match port_spec.protocol {
                Protocol::TCP => "tcp",
                Protocol::UDP => "udp",
                Protocol::SCTP => "sctp",
            };

            let Some(port) = port_spec.port else {
                continue;
            };

            if ip_blocks.is_empty() {
                rules.push(format!(
                    "nft add rule inet horcrux policy_{} {} dport {} accept",
                    policy_id, protocol, port
                ));
            } else {
                for (cidr, except) in &ip_blocks {
                    for excluded in *except {
                        rules.push(format!(
                            "nft add rule inet horcrux policy_{} ip {} {} {} dport {} drop",
                            policy_id, addr_field, excluded, protocol, port
                        ));
                    }
                    rules.push(format!(
                        "nft add rule inet horcrux policy_{} ip {} {} {} dport {} accept",
                        policy_id, addr_field, cidr, protocol, port
                    ));
                }
            }
        }
    }

    // Helper methods

    fn matches_selector(&self, selector: &LabelSelector, labels: &HashMap<String, String>) -> bool {
        // Check match_labels
        for (key, value) in &selector.match_labels {
            if labels.get(key) != Some(value) {
                return false;
            }
        }

        // Check match_expressions
        for expr in &selector.match_expressions {
            if !self.matches_expression(expr, labels) {
                return false;
            }
        }

        true
    }

    fn matches_expression(&self, expr: &LabelExpression, labels: &HashMap<String, String>) -> bool {
        match expr.operator {
            LabelOperator::In => {
                if let Some(value) = labels.get(&expr.key) {
                    expr.values.contains(value)
                } else {
                    false
                }
            }
            LabelOperator::NotIn => {
                if let Some(value) = labels.get(&expr.key) {
                    !expr.values.contains(value)
                } else {
                    true
                }
            }
            LabelOperator::Exists => labels.contains_key(&expr.key),
            LabelOperator::DoesNotExist => !labels.contains_key(&expr.key),
        }
    }

    /// Evaluate a rule's peer selector list (`from`/`to`) against the
    /// connection's source pod. An empty peer list means "no restriction"
    /// (matches Kubernetes NetworkPolicy semantics: a rule with no `from`/`to`
    /// entries allows all sources/destinations). A non-empty list requires at
    /// least one peer entry to match.
    fn matches_peer(&self, peers: &[PeerSelector], src_pod: &str) -> bool {
        if peers.is_empty() {
            return true;
        }

        let src_meta = self.pod_metadata.get(src_pod);

        for peer in peers {
            match peer {
                PeerSelector::PodSelector(selector) => {
                    if let Some((_namespace, labels)) = src_meta {
                        if self.matches_selector(selector, labels) {
                            return true;
                        }
                    }
                }
                PeerSelector::NamespaceSelector(selector) => {
                    // Without per-namespace label metadata we can only
                    // evaluate the "select all namespaces" case (an empty
                    // selector). A selector with actual label requirements
                    // can't be evaluated yet, so treat it as a non-match
                    // rather than silently allowing traffic -- a false deny
                    // is safer than a false allow for a firewall rule.
                    if selector.match_labels.is_empty() && selector.match_expressions.is_empty() {
                        return true;
                    }
                }
                PeerSelector::IpBlock { .. } => {
                    // Source IP isn't threaded through to this layer yet, so
                    // IP-block peers can't be evaluated here and never match.
                }
            }
        }

        false
    }

    fn matches_port(&self, ports: &[NetworkPolicyPort], protocol: &Protocol, port: u16) -> bool {
        if ports.is_empty() {
            return true; // No port restriction = all ports
        }

        for port_spec in ports {
            if &port_spec.protocol != protocol {
                continue;
            }

            // Check port range
            if let Some(spec_port) = port_spec.port {
                if let Some(end_port) = port_spec.end_port {
                    if port >= spec_port && port <= end_port {
                        return true;
                    }
                } else if port == spec_port {
                    return true;
                }
            } else {
                // No port specified = all ports for this protocol
                return true;
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_policy() {
        let mut manager = NetworkPolicyManager::new();

        let policy = NetworkPolicy {
            id: "policy1".to_string(),
            name: "deny-all".to_string(),
            namespace: "default".to_string(),
            pod_selector: LabelSelector::default(),
            policy_types: vec![PolicyType::Ingress],
            ingress: vec![],
            egress: vec![],
            enabled: true,
        };

        assert!(manager.create_policy(policy).is_ok());
        assert_eq!(manager.list_policies().len(), 1);
    }

    #[test]
    fn test_matches_port() {
        let manager = NetworkPolicyManager::new();

        let ports = vec![
            NetworkPolicyPort {
                protocol: Protocol::TCP,
                port: Some(80),
                end_port: None,
            },
            NetworkPolicyPort {
                protocol: Protocol::TCP,
                port: Some(443),
                end_port: None,
            },
        ];

        assert!(manager.matches_port(&ports, &Protocol::TCP, 80));
        assert!(manager.matches_port(&ports, &Protocol::TCP, 443));
        assert!(!manager.matches_port(&ports, &Protocol::TCP, 8080));
        assert!(!manager.matches_port(&ports, &Protocol::UDP, 80));
    }

    #[test]
    fn test_is_connection_allowed_enforces_pod_selector_peers() {
        let mut manager = NetworkPolicyManager::new();

        let mut from_selector = LabelSelector::default();
        from_selector
            .match_labels
            .insert("role".to_string(), "frontend".to_string());

        let policy = NetworkPolicy {
            id: "policy1".to_string(),
            name: "allow-frontend-to-backend".to_string(),
            namespace: "default".to_string(),
            pod_selector: LabelSelector::default(),
            policy_types: vec![PolicyType::Ingress],
            ingress: vec![IngressRule {
                from: vec![PeerSelector::PodSelector(from_selector)],
                ports: vec![NetworkPolicyPort {
                    protocol: Protocol::TCP,
                    port: Some(8080),
                    end_port: None,
                }],
            }],
            egress: vec![],
            enabled: true,
        };
        manager.create_policy(policy).unwrap();

        let mut backend_labels = HashMap::new();
        backend_labels.insert("role".to_string(), "backend".to_string());
        manager.update_pod_policies("backend-1", &backend_labels, "default");

        let mut frontend_labels = HashMap::new();
        frontend_labels.insert("role".to_string(), "frontend".to_string());
        manager.update_pod_policies("frontend-1", &frontend_labels, "default");

        let mut other_labels = HashMap::new();
        other_labels.insert("role".to_string(), "attacker".to_string());
        manager.update_pod_policies("other-1", &other_labels, "default");

        // Matching peer selector + matching port -> allowed
        assert!(manager.is_connection_allowed(
            "frontend-1",
            "backend-1",
            &Protocol::TCP,
            8080,
            &PolicyType::Ingress,
        ));

        // Port matches but peer selector does not -> must be denied (this is
        // the bug: previously any port match was allowed regardless of `from`)
        assert!(!manager.is_connection_allowed(
            "other-1",
            "backend-1",
            &Protocol::TCP,
            8080,
            &PolicyType::Ingress,
        ));
    }

    #[test]
    fn test_matches_peer_empty_list_allows_all() {
        let manager = NetworkPolicyManager::new();
        assert!(manager.matches_peer(&[], "any-pod"));
    }

    #[test]
    fn test_generate_iptables_rules_restricts_source_for_ip_block_peer() {
        let mut manager = NetworkPolicyManager::new();

        let policy = NetworkPolicy {
            id: "policy1".to_string(),
            name: "allow-subnet".to_string(),
            namespace: "default".to_string(),
            pod_selector: LabelSelector::default(),
            policy_types: vec![PolicyType::Ingress],
            ingress: vec![IngressRule {
                from: vec![PeerSelector::IpBlock {
                    cidr: "10.0.0.0/8".to_string(),
                    except: vec!["10.0.1.0/24".to_string()],
                }],
                ports: vec![NetworkPolicyPort {
                    protocol: Protocol::TCP,
                    port: Some(8080),
                    end_port: None,
                }],
            }],
            egress: vec![],
            enabled: true,
        };
        manager.create_policy(policy).unwrap();

        let rules = manager.generate_iptables_rules("policy1");

        // The except CIDR must be dropped before the broader CIDR is
        // accepted, and both must carry the real source restriction --
        // not just a bare port match.
        let drop_idx = rules
            .iter()
            .position(|r| r.contains("-s 10.0.1.0/24") && r.contains("-j DROP"))
            .expect("expected a DROP rule for the excepted CIDR");
        let accept_idx = rules
            .iter()
            .position(|r| r.contains("-s 10.0.0.0/8") && r.contains("-j ACCEPT"))
            .expect("expected an ACCEPT rule scoped to the allowed CIDR");
        assert!(
            drop_idx < accept_idx,
            "exception must be evaluated before the broader accept"
        );

        // Must not contain an unrestricted accept for this port (the bug
        // this test guards against: port matched from any source).
        assert!(!rules
            .iter()
            .any(|r| r == "iptables -A HORCRUX-POL-POLICY1 -p tcp --dport 8080 -j ACCEPT"));
    }

    #[test]
    fn test_generate_iptables_rules_warns_on_unresolvable_peer_selector() {
        let mut manager = NetworkPolicyManager::new();

        let mut from_selector = LabelSelector::default();
        from_selector
            .match_labels
            .insert("role".to_string(), "frontend".to_string());

        let policy = NetworkPolicy {
            id: "policy2".to_string(),
            name: "allow-frontend".to_string(),
            namespace: "default".to_string(),
            pod_selector: LabelSelector::default(),
            policy_types: vec![PolicyType::Ingress],
            ingress: vec![IngressRule {
                from: vec![PeerSelector::PodSelector(from_selector)],
                ports: vec![NetworkPolicyPort {
                    protocol: Protocol::TCP,
                    port: Some(443),
                    end_port: None,
                }],
            }],
            egress: vec![],
            enabled: true,
        };
        manager.create_policy(policy).unwrap();

        let rules = manager.generate_iptables_rules("policy2");

        assert!(rules.iter().any(|r| r.starts_with("# WARNING:")));
    }

    #[test]
    fn test_generate_nftables_rules_restricts_destination_for_egress_ip_block() {
        let mut manager = NetworkPolicyManager::new();

        let policy = NetworkPolicy {
            id: "policy3".to_string(),
            name: "restrict-egress".to_string(),
            namespace: "default".to_string(),
            pod_selector: LabelSelector::default(),
            policy_types: vec![PolicyType::Egress],
            ingress: vec![],
            egress: vec![EgressRule {
                to: vec![PeerSelector::IpBlock {
                    cidr: "192.168.0.0/16".to_string(),
                    except: vec![],
                }],
                ports: vec![NetworkPolicyPort {
                    protocol: Protocol::TCP,
                    port: Some(443),
                    end_port: None,
                }],
            }],
            enabled: true,
        };
        manager.create_policy(policy).unwrap();

        let rules = manager.generate_nftables_rules("policy3");

        assert!(rules
            .iter()
            .any(|r| r.contains("daddr 192.168.0.0/16") && r.contains("accept")));
        assert!(!rules
            .iter()
            .any(|r| r == "nft add rule inet horcrux policy_policy3 tcp dport 443 accept"));
    }

    #[test]
    fn test_matches_selector() {
        let manager = NetworkPolicyManager::new();

        let mut labels = HashMap::new();
        labels.insert("app".to_string(), "web".to_string());
        labels.insert("env".to_string(), "prod".to_string());

        let mut selector = LabelSelector::default();
        selector
            .match_labels
            .insert("app".to_string(), "web".to_string());

        assert!(manager.matches_selector(&selector, &labels));

        selector
            .match_labels
            .insert("app".to_string(), "db".to_string());
        assert!(!manager.matches_selector(&selector, &labels));
    }
}
