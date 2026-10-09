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
        self.pod_metadata
            .insert(pod_id.to_string(), (namespace.to_string(), pod_labels.clone()));
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

        // Ingress rules
        if policy.policy_types.contains(&PolicyType::Ingress) {
            for rule in &policy.ingress {
                for port_spec in &rule.ports {
                    let protocol = match port_spec.protocol {
                        Protocol::TCP => "tcp",
                        Protocol::UDP => "udp",
                        Protocol::SCTP => "sctp",
                    };

                    if let Some(port) = port_spec.port {
                        rules.push(format!(
                            "iptables -A {} -p {} --dport {} -j ACCEPT",
                            chain_name, protocol, port
                        ));
                    }
                }
            }
        }

        // Egress rules
        if policy.policy_types.contains(&PolicyType::Egress) {
            for rule in &policy.egress {
                for port_spec in &rule.ports {
                    let protocol = match port_spec.protocol {
                        Protocol::TCP => "tcp",
                        Protocol::UDP => "udp",
                        Protocol::SCTP => "sctp",
                    };

                    if let Some(port) = port_spec.port {
                        rules.push(format!(
                            "iptables -A {} -p {} --dport {} -j ACCEPT",
                            chain_name, protocol, port
                        ));
                    }
                }
            }
        }

        // Default deny at end of chain
        rules.push(format!("iptables -A {} -j DROP", chain_name));

        rules
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

        // Ingress rules
        if policy.policy_types.contains(&PolicyType::Ingress) {
            for rule in &policy.ingress {
                for port_spec in &rule.ports {
                    let protocol = match port_spec.protocol {
                        Protocol::TCP => "tcp",
                        Protocol::UDP => "udp",
                        Protocol::SCTP => "sctp",
                    };

                    if let Some(port) = port_spec.port {
                        rules.push(format!(
                            "nft add rule inet horcrux policy_{} {} dport {} accept",
                            policy_id, protocol, port
                        ));
                    }
                }
            }
        }

        // Default drop
        rules.push(format!(
            "nft add rule inet horcrux policy_{} drop",
            policy_id
        ));

        rules
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
