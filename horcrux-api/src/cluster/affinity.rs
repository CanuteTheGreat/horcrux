//! HA Affinity Rules Module
//!
//! Controls placement of HA resources (VMs and containers) on cluster nodes.
//! Supports both node affinity (pin to specific nodes) and resource affinity
//! (keep resources together or spread them apart).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Affinity rule for HA resource placement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AffinityRule {
    pub id: String,
    pub name: String,
    pub rule_type: AffinityRuleType,
    pub enabled: bool,
    pub priority: u32,  // Higher priority rules are evaluated first
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AffinityRuleType {
    /// Pin resources to specific nodes
    NodeAffinity(NodeAffinityRule),
    /// Keep resources together on same node
    ResourceAffinity(ResourceAffinityRule),
    /// Spread resources across different nodes
    AntiAffinity(AntiAffinityRule),
}

/// Node affinity - pin specific resources to specific nodes
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeAffinityRule {
    pub resources: Vec<String>,  // VM/CT IDs
    pub nodes: Vec<String>,      // Preferred node IDs
    pub policy: AffinityPolicy,
}

/// Resource affinity - keep resources together
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResourceAffinityRule {
    pub resources: Vec<String>,  // VM/CT IDs that should stay together
    pub policy: AffinityPolicy,
}

/// Anti-affinity - spread resources apart
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AntiAffinityRule {
    pub resources: Vec<String>,  // VM/CT IDs that should be separated
    pub policy: AffinityPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AffinityPolicy {
    /// Rule must be satisfied (hard constraint)
    Required,
    /// Rule should be satisfied if possible (soft constraint)
    Preferred,
}

/// Affinity manager for evaluating and applying rules
pub struct AffinityManager {
    rules: HashMap<String, AffinityRule>,
}

impl AffinityManager {
    pub fn new() -> Self {
        AffinityManager {
            rules: HashMap::new(),
        }
    }

    /// Add or update an affinity rule
    pub fn add_rule(&mut self, rule: AffinityRule) -> Result<(), String> {
        // Validate the rule
        self.validate_rule(&rule)?;

        self.rules.insert(rule.id.clone(), rule);
        Ok(())
    }

    /// Remove an affinity rule
    pub fn remove_rule(&mut self, rule_id: &str) -> Result<(), String> {
        if self.rules.remove(rule_id).is_none() {
            return Err(format!("Rule {} not found", rule_id));
        }
        Ok(())
    }

    /// Get a specific rule
    pub fn get_rule(&self, rule_id: &str) -> Option<&AffinityRule> {
        self.rules.get(rule_id)
    }

    /// List all rules
    pub fn list_rules(&self) -> Vec<&AffinityRule> {
        let mut rules: Vec<_> = self.rules.values().collect();
        rules.sort_by(|a, b| b.priority.cmp(&a.priority));
        rules
    }

    /// List rules affecting a specific resource
    pub fn list_rules_for_resource(&self, resource_id: &str) -> Vec<&AffinityRule> {
        self.rules.values()
            .filter(|rule| self.rule_affects_resource(rule, resource_id))
            .collect()
    }

    /// Calculate best node for a resource based on affinity rules
    pub fn suggest_node(
        &self,
        resource_id: &str,
        available_nodes: &[String],
        current_placements: &HashMap<String, String>, // resource_id -> node_id
    ) -> Result<String, String> {
        if available_nodes.is_empty() {
            return Err("No available nodes".to_string());
        }

        // Get all rules affecting this resource, sorted by priority
        let mut relevant_rules: Vec<_> = self.rules.values()
            .filter(|r| r.enabled && self.rule_affects_resource(r, resource_id))
            .collect();
        relevant_rules.sort_by(|a, b| b.priority.cmp(&a.priority));

        // Score each node based on affinity rules
        let mut node_scores: HashMap<String, i32> = HashMap::new();
        for node in available_nodes {
            node_scores.insert(node.clone(), 0);
        }

        for rule in &relevant_rules {
            self.apply_rule_scoring(rule, resource_id, current_placements, &mut node_scores);
        }

        // Check for hard constraints (Required policies)
        for rule in &relevant_rules {
            if let AffinityRuleType::NodeAffinity(na) = &rule.rule_type {
                if na.policy == AffinityPolicy::Required && na.resources.contains(&resource_id.to_string()) {
                    // Must be on one of these nodes
                    let valid_nodes: Vec<_> = available_nodes.iter()
                        .filter(|n| na.nodes.contains(n))
                        .collect();

                    if valid_nodes.is_empty() {
                        return Err(format!("Required node affinity cannot be satisfied for {}", resource_id));
                    }
                }
            }
        }

        // Return node with highest score
        let best_node = node_scores.iter()
            .max_by_key(|(_, score)| *score)
            .map(|(node, _)| node.clone())
            .unwrap_or_else(|| available_nodes[0].clone());

        Ok(best_node)
    }

    /// Validate node placement against affinity rules
    pub fn validate_placement(
        &self,
        resource_id: &str,
        target_node: &str,
        current_placements: &HashMap<String, String>,
    ) -> Result<(), String> {
        let relevant_rules: Vec<_> = self.rules.values()
            .filter(|r| r.enabled && self.rule_affects_resource(r, resource_id))
            .collect();

        for rule in &relevant_rules {
            match &rule.rule_type {
                AffinityRuleType::NodeAffinity(na) => {
                    if na.policy == AffinityPolicy::Required
                        && na.resources.contains(&resource_id.to_string())
                        && !na.nodes.contains(&target_node.to_string()) {
                        return Err(format!(
                            "Required node affinity violated: {} must be on one of {:?}",
                            resource_id, na.nodes
                        ));
                    }
                }
                AffinityRuleType::ResourceAffinity(ra) => {
                    if ra.policy == AffinityPolicy::Required && ra.resources.contains(&resource_id.to_string()) {
                        // Check if other resources in the group are on the same node
                        for other_resource in &ra.resources {
                            if other_resource != resource_id {
                                if let Some(other_node) = current_placements.get(other_resource) {
                                    if other_node != target_node {
                                        return Err(format!(
                                            "Required resource affinity violated: {} must be with {}",
                                            resource_id, other_resource
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
                AffinityRuleType::AntiAffinity(aa) => {
                    if aa.policy == AffinityPolicy::Required && aa.resources.contains(&resource_id.to_string()) {
                        // Check if any other resources in the group are on the same node
                        for other_resource in &aa.resources {
                            if other_resource != resource_id {
                                if let Some(other_node) = current_placements.get(other_resource) {
                                    if other_node == target_node {
                                        return Err(format!(
                                            "Required anti-affinity violated: {} cannot be with {}",
                                            resource_id, other_resource
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    // Helper functions

    fn validate_rule(&self, rule: &AffinityRule) -> Result<(), String> {
        match &rule.rule_type {
            AffinityRuleType::NodeAffinity(na) => {
                if na.resources.is_empty() {
                    return Err("NodeAffinity rule must have at least one resource".to_string());
                }
                if na.nodes.is_empty() {
                    return Err("NodeAffinity rule must have at least one node".to_string());
                }
            }
            AffinityRuleType::ResourceAffinity(ra) => {
                if ra.resources.len() < 2 {
                    return Err("ResourceAffinity rule must have at least two resources".to_string());
                }
            }
            AffinityRuleType::AntiAffinity(aa) => {
                if aa.resources.len() < 2 {
                    return Err("AntiAffinity rule must have at least two resources".to_string());
                }
            }
        }
        Ok(())
    }

    fn rule_affects_resource(&self, rule: &AffinityRule, resource_id: &str) -> bool {
        match &rule.rule_type {
            AffinityRuleType::NodeAffinity(na) => na.resources.contains(&resource_id.to_string()),
            AffinityRuleType::ResourceAffinity(ra) => ra.resources.contains(&resource_id.to_string()),
            AffinityRuleType::AntiAffinity(aa) => aa.resources.contains(&resource_id.to_string()),
        }
    }

    fn apply_rule_scoring(
        &self,
        rule: &AffinityRule,
        resource_id: &str,
        current_placements: &HashMap<String, String>,
        scores: &mut HashMap<String, i32>,
    ) {
        let weight = if matches!(rule.rule_type, AffinityRuleType::NodeAffinity(_)) { 100 }
            else { match &rule.rule_type {
                AffinityRuleType::NodeAffinity(na) if na.policy == AffinityPolicy::Required => 100,
                AffinityRuleType::NodeAffinity(_) => 50,
                AffinityRuleType::ResourceAffinity(ra) if ra.policy == AffinityPolicy::Required => 100,
                AffinityRuleType::ResourceAffinity(_) => 50,
                AffinityRuleType::AntiAffinity(aa) if aa.policy == AffinityPolicy::Required => 100,
                AffinityRuleType::AntiAffinity(_) => 50,
        }};

        match &rule.rule_type {
            AffinityRuleType::NodeAffinity(na) => {
                // Increase score for preferred nodes
                for node in &na.nodes {
                    if let Some(score) = scores.get_mut(node) {
                        *score += weight;
                    }
                }
            }
            AffinityRuleType::ResourceAffinity(ra) => {
                // Increase score for nodes where related resources are already running
                for other_resource in &ra.resources {
                    if other_resource != resource_id {
                        if let Some(node) = current_placements.get(other_resource) {
                            if let Some(score) = scores.get_mut(node) {
                                *score += weight;
                            }
                        }
                    }
                }
            }
            AffinityRuleType::AntiAffinity(aa) => {
                // Decrease score for nodes where related resources are already running
                for other_resource in &aa.resources {
                    if other_resource != resource_id {
                        if let Some(node) = current_placements.get(other_resource) {
                            if let Some(score) = scores.get_mut(node) {
                                *score -= weight;
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_affinity_required() {
        let mut am = AffinityManager::new();

        let rule = AffinityRule {
            id: "rule1".to_string(),
            name: "Pin DB to node1".to_string(),
            rule_type: AffinityRuleType::NodeAffinity(NodeAffinityRule {
                resources: vec!["vm-db".to_string()],
                nodes: vec!["node1".to_string(), "node2".to_string()],
                policy: AffinityPolicy::Required,
            }),
            enabled: true,
            priority: 100,
            description: "".to_string(),
        };

        am.add_rule(rule).unwrap();

        let available_nodes = vec!["node1".to_string(), "node2".to_string(), "node3".to_string()];
        let placements = HashMap::new();

        let suggested = am.suggest_node("vm-db", &available_nodes, &placements).unwrap();
        assert!(suggested == "node1" || suggested == "node2");

        // Validate that placement on node3 would fail
        let result = am.validate_placement("vm-db", "node3", &placements);
        assert!(result.is_err());
    }

    #[test]
    fn test_resource_affinity() {
        let mut am = AffinityManager::new();

        let rule = AffinityRule {
            id: "rule1".to_string(),
            name: "Keep web and db together".to_string(),
            rule_type: AffinityRuleType::ResourceAffinity(ResourceAffinityRule {
                resources: vec!["vm-web".to_string(), "vm-db".to_string()],
                policy: AffinityPolicy::Preferred,
            }),
            enabled: true,
            priority: 50,
            description: "".to_string(),
        };

        am.add_rule(rule).unwrap();

        let available_nodes = vec!["node1".to_string(), "node2".to_string()];
        let mut placements = HashMap::new();
        placements.insert("vm-db".to_string(), "node1".to_string());

        // vm-web should prefer node1 where vm-db is
        let suggested = am.suggest_node("vm-web", &available_nodes, &placements).unwrap();
        assert_eq!(suggested, "node1");
    }

    #[test]
    fn test_anti_affinity() {
        let mut am = AffinityManager::new();

        let rule = AffinityRule {
            id: "rule1".to_string(),
            name: "Spread replicas".to_string(),
            rule_type: AffinityRuleType::AntiAffinity(AntiAffinityRule {
                resources: vec!["vm-replica1".to_string(), "vm-replica2".to_string()],
                policy: AffinityPolicy::Required,
            }),
            enabled: true,
            priority: 100,
            description: "".to_string(),
        };

        am.add_rule(rule).unwrap();

        let available_nodes = vec!["node1".to_string(), "node2".to_string()];
        let mut placements = HashMap::new();
        placements.insert("vm-replica1".to_string(), "node1".to_string());

        // vm-replica2 should not be on node1
        let result = am.validate_placement("vm-replica2", "node1", &placements);
        assert!(result.is_err());

        let result = am.validate_placement("vm-replica2", "node2", &placements);
        assert!(result.is_ok());
    }
}
