use serde::{Deserialize, Serialize};

/// Node resource usage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeResources {
    pub node_name: String,
    pub cpu_usage: f32,        // Percentage 0-100
    pub memory_usage: f32,     // Percentage 0-100
    pub disk_usage: f32,       // Percentage 0-100
    pub network_usage: f32,    // Mbps
    pub vm_count: usize,
    pub total_cpu_cores: usize,
    pub total_memory_gb: usize,
    pub total_disk_gb: usize,
}

/// VM resource requirements
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmResources {
    pub vm_id: u32,
    pub cpu_cores: usize,
    pub memory_gb: usize,
    pub disk_gb: usize,
    pub current_node: String,
    pub can_migrate: bool,
}

/// Balancing strategy
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum BalancingStrategy {
    /// Balance by CPU usage
    Cpu,
    /// Balance by memory usage
    Memory,
    /// Balance by VM count
    VmCount,
    /// Balance by weighted score
    Weighted,
}

/// Balancing policy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalancingPolicy {
    pub enabled: bool,
    pub strategy: BalancingStrategy,
    pub threshold: f32,        // Trigger rebalancing if difference > threshold
    pub min_gain: f32,         // Minimum improvement required to migrate
    pub max_migrations: usize, // Max VMs to migrate per cycle
    pub aggressive: bool,      // More aggressive balancing
}

impl Default for BalancingPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            strategy: BalancingStrategy::Weighted,
            threshold: 20.0,  // 20% difference
            min_gain: 5.0,    // 5% improvement
            max_migrations: 3,
            aggressive: false,
        }
    }
}

/// Migration recommendation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationRecommendation {
    pub vm_id: u32,
    pub from_node: String,
    pub to_node: String,
    pub reason: String,
    pub score_improvement: f32,
    pub priority: MigrationPriority,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum MigrationPriority {
    Low,
    Medium,
    High,
    Critical,
}

/// Cluster balancer
pub struct ClusterBalancer {
    policy: BalancingPolicy,
}

impl ClusterBalancer {
    pub fn new(policy: BalancingPolicy) -> Self {
        Self { policy }
    }

    /// Calculate node load score
    fn calculate_node_score(&self, node: &NodeResources) -> f32 {
        match self.policy.strategy {
            BalancingStrategy::Cpu => node.cpu_usage,
            BalancingStrategy::Memory => node.memory_usage,
            BalancingStrategy::VmCount => {
                (node.vm_count as f32 / 10.0) * 100.0  // Normalize to percentage
            }
            BalancingStrategy::Weighted => {
                // Weighted average: CPU 40%, Memory 40%, VM count 20%
                (node.cpu_usage * 0.4) +
                (node.memory_usage * 0.4) +
                ((node.vm_count as f32 / 10.0) * 100.0 * 0.2)
            }
        }
    }

    /// Check if cluster is balanced
    pub fn is_balanced(&self, nodes: &[NodeResources]) -> bool {
        if nodes.len() < 2 {
            return true;  // Single node is always "balanced"
        }

        let scores: Vec<f32> = nodes.iter()
            .map(|n| self.calculate_node_score(n))
            .collect();

        let max_score = scores.iter().cloned().fold(f32::MIN, f32::max);
        let min_score = scores.iter().cloned().fold(f32::MAX, f32::min);

        let difference = max_score - min_score;

        difference <= self.policy.threshold
    }

    /// Get balancing recommendations
    pub fn get_recommendations(
        &self,
        nodes: &[NodeResources],
        vms: &[VmResources],
    ) -> Vec<MigrationRecommendation> {
        if !self.policy.enabled || nodes.len() < 2 {
            return Vec::new();
        }

        // Calculate node scores
        let mut node_scores: Vec<(String, f32)> = nodes.iter()
            .map(|n| (n.node_name.clone(), self.calculate_node_score(n)))
            .collect();

        node_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        // Check if balancing is needed
        let max_score = node_scores[0].1;
        let min_score = node_scores[node_scores.len() - 1].1;

        if max_score - min_score <= self.policy.threshold {
            return Vec::new();  // Already balanced
        }

        tracing::info!(
            "Cluster imbalance detected: max={:.1}%, min={:.1}%, diff={:.1}%",
            max_score, min_score, max_score - min_score
        );

        // Generate migration recommendations
        let mut recommendations = Vec::new();
        let mut migrations_count = 0;

        // Get VMs from overloaded nodes
        let overloaded_node = &node_scores[0].0;
        let underloaded_node = &node_scores[node_scores.len() - 1].0;

        for vm in vms {
            if migrations_count >= self.policy.max_migrations {
                break;
            }

            if !vm.can_migrate {
                continue;
            }

            if vm.current_node != *overloaded_node {
                continue;
            }

            // Calculate improvement if we migrate this VM
            let improvement = self.calculate_migration_improvement(
                nodes,
                vm,
                overloaded_node,
                underloaded_node,
            );

            if improvement >= self.policy.min_gain {
                let priority = if improvement > 20.0 {
                    MigrationPriority::High
                } else if improvement > 10.0 {
                    MigrationPriority::Medium
                } else {
                    MigrationPriority::Low
                };

                recommendations.push(MigrationRecommendation {
                    vm_id: vm.vm_id,
                    from_node: overloaded_node.clone(),
                    to_node: underloaded_node.clone(),
                    reason: format!(
                        "Rebalance cluster: {:.1}% improvement",
                        improvement
                    ),
                    score_improvement: improvement,
                    priority,
                });

                migrations_count += 1;
            }
        }

        // Sort by priority and improvement
        recommendations.sort_by(|a, b| {
            b.priority.cmp(&a.priority)
                .then(b.score_improvement.partial_cmp(&a.score_improvement).unwrap())
        });

        recommendations
    }

    /// Calculate improvement from migrating a VM
    fn calculate_migration_improvement(
        &self,
        nodes: &[NodeResources],
        vm: &VmResources,
        from_node: &str,
        to_node: &str,
    ) -> f32 {
        let source = nodes.iter().find(|n| n.node_name == from_node);
        let target = nodes.iter().find(|n| n.node_name == to_node);

        if source.is_none() || target.is_none() {
            return 0.0;
        }

        let source = source.unwrap();
        let target = target.unwrap();

        // Calculate current imbalance
        let current_source_score = self.calculate_node_score(source);
        let current_target_score = self.calculate_node_score(target);
        let current_diff = (current_source_score - current_target_score).abs();

        // Estimate scores after migration
        let cpu_delta = (vm.cpu_cores as f32 / source.total_cpu_cores as f32) * 100.0;
        let mem_delta = (vm.memory_gb as f32 / source.total_memory_gb as f32) * 100.0;

        let new_source_score = current_source_score - ((cpu_delta + mem_delta) / 2.0);
        let new_target_score = current_target_score + ((cpu_delta + mem_delta) / 2.0);
        let new_diff = (new_source_score - new_target_score).abs();

        // Improvement is reduction in imbalance
        current_diff - new_diff
    }

    /// Find best node for VM placement
    pub fn find_best_node(
        &self,
        nodes: &[NodeResources],
        vm: &VmResources,
    ) -> Option<String> {
        if nodes.is_empty() {
            return None;
        }

        let mut best_node: Option<&NodeResources> = None;
        let mut best_score = f32::MAX;

        for node in nodes {
            // Check if node has sufficient resources
            let cpu_available = node.total_cpu_cores as f32 * (1.0 - node.cpu_usage / 100.0);
            let mem_available = node.total_memory_gb as f32 * (1.0 - node.memory_usage / 100.0);

            if cpu_available < vm.cpu_cores as f32 || mem_available < vm.memory_gb as f32 {
                continue;  // Not enough resources
            }

            // Score this node (lower is better for placement)
            let score = self.calculate_node_score(node);

            if score < best_score {
                best_score = score;
                best_node = Some(node);
            }
        }

        best_node.map(|n| n.node_name.clone())
    }

    /// Update balancing policy
    pub fn update_policy(&mut self, policy: BalancingPolicy) {
        self.policy = policy;
    }

    /// Get current policy
    pub fn get_policy(&self) -> &BalancingPolicy {
        &self.policy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_node(name: &str, cpu: f32, memory: f32, vm_count: usize) -> NodeResources {
        NodeResources {
            node_name: name.to_string(),
            cpu_usage: cpu,
            memory_usage: memory,
            disk_usage: 50.0,
            network_usage: 100.0,
            vm_count,
            total_cpu_cores: 16,
            total_memory_gb: 64,
            total_disk_gb: 1000,
        }
    }

    fn create_test_vm(id: u32, node: &str) -> VmResources {
        VmResources {
            vm_id: id,
            cpu_cores: 2,
            memory_gb: 4,
            disk_gb: 50,
            current_node: node.to_string(),
            can_migrate: true,
        }
    }

    #[test]
    fn test_balanced_cluster() {
        let policy = BalancingPolicy::default();
        let balancer = ClusterBalancer::new(policy);

        let nodes = vec![
            create_test_node("node1", 50.0, 50.0, 5),
            create_test_node("node2", 52.0, 51.0, 5),
            create_test_node("node3", 49.0, 50.0, 5),
        ];

        assert!(balancer.is_balanced(&nodes));
    }

    #[test]
    fn test_imbalanced_cluster() {
        let policy = BalancingPolicy::default();
        let balancer = ClusterBalancer::new(policy);

        let nodes = vec![
            create_test_node("node1", 90.0, 85.0, 10),
            create_test_node("node2", 30.0, 35.0, 3),
        ];

        assert!(!balancer.is_balanced(&nodes));
    }

    #[test]
    fn test_migration_recommendations() {
        let policy = BalancingPolicy {
            enabled: true,
            strategy: BalancingStrategy::Weighted,
            threshold: 20.0,
            min_gain: 5.0,
            max_migrations: 2,
            aggressive: false,
        };

        let balancer = ClusterBalancer::new(policy);

        let nodes = vec![
            create_test_node("node1", 80.0, 75.0, 8),
            create_test_node("node2", 30.0, 35.0, 2),
        ];

        let vms = vec![
            create_test_vm(100, "node1"),
            create_test_vm(101, "node1"),
            create_test_vm(102, "node1"),
        ];

        let recommendations = balancer.get_recommendations(&nodes, &vms);

        assert!(!recommendations.is_empty());
        assert!(recommendations.len() <= 2);  // max_migrations = 2
        assert_eq!(recommendations[0].from_node, "node1");
        assert_eq!(recommendations[0].to_node, "node2");
    }

    #[test]
    fn test_best_node_placement() {
        let policy = BalancingPolicy::default();
        let balancer = ClusterBalancer::new(policy);

        let nodes = vec![
            create_test_node("node1", 80.0, 75.0, 8),
            create_test_node("node2", 30.0, 35.0, 2),
            create_test_node("node3", 50.0, 50.0, 5),
        ];

        let vm = create_test_vm(100, "");

        let best = balancer.find_best_node(&nodes, &vm);

        assert_eq!(best, Some("node2".to_string()));  // Least loaded
    }

    #[test]
    fn test_different_strategies() {
        let mut policy = BalancingPolicy::default();

        // Test CPU strategy
        policy.strategy = BalancingStrategy::Cpu;
        let balancer = ClusterBalancer::new(policy.clone());
        let node = create_test_node("test", 75.0, 50.0, 5);
        assert_eq!(balancer.calculate_node_score(&node), 75.0);

        // Test Memory strategy
        policy.strategy = BalancingStrategy::Memory;
        let balancer = ClusterBalancer::new(policy.clone());
        assert_eq!(balancer.calculate_node_score(&node), 50.0);

        // Test Weighted strategy
        policy.strategy = BalancingStrategy::Weighted;
        let balancer = ClusterBalancer::new(policy);
        let score = balancer.calculate_node_score(&node);
        assert!(score > 50.0 && score < 75.0);  // Weighted average
    }
}
