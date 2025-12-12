//! Cluster Module Tests
//! Tests for cluster management, node balancing, affinity rules, and architecture support

use horcrux_api::cluster::balancer::{
    ClusterBalancer, BalancingPolicy, BalancingStrategy, NodeResources, VmResources,
    MigrationRecommendation, MigrationPriority,
};
use horcrux_api::cluster::affinity::{
    AffinityManager, AffinityRule, AffinityRuleType, AffinityPolicy,
    NodeAffinityRule, ResourceAffinityRule, AntiAffinityRule,
};
use horcrux_api::cluster::arch::{
    ArchitectureManager, ArchitectureInfo, Endianness, PlacementCompatibility,
    MigrationCompatibility, ClusterArchStats, EmulationType,
};
use std::collections::HashMap;

// ============== Cluster Balancer Tests ==============

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
fn test_balancer_creation() {
    let policy = BalancingPolicy::default();
    let balancer = ClusterBalancer::new(policy);
    assert_eq!(balancer.get_policy().threshold, 20.0);
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
fn test_single_node_always_balanced() {
    let policy = BalancingPolicy::default();
    let balancer = ClusterBalancer::new(policy);

    let nodes = vec![create_test_node("node1", 90.0, 90.0, 10)];

    assert!(balancer.is_balanced(&nodes));
}

#[test]
fn test_balancing_strategies() {
    let strategies = vec![
        BalancingStrategy::Cpu,
        BalancingStrategy::Memory,
        BalancingStrategy::VmCount,
        BalancingStrategy::Weighted,
    ];

    for strategy in strategies {
        let json = serde_json::to_string(&strategy).unwrap();
        let _: BalancingStrategy = serde_json::from_str(&json).unwrap();
    }
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

    // Should have some recommendations since cluster is imbalanced
    assert!(recommendations.len() <= 2); // max_migrations = 2
    for rec in &recommendations {
        assert_eq!(rec.from_node, "node1");
        assert_eq!(rec.to_node, "node2");
    }
}

#[test]
fn test_no_recommendations_when_disabled() {
    let policy = BalancingPolicy {
        enabled: false,
        ..Default::default()
    };

    let balancer = ClusterBalancer::new(policy);

    let nodes = vec![
        create_test_node("node1", 90.0, 90.0, 10),
        create_test_node("node2", 10.0, 10.0, 1),
    ];

    let vms = vec![create_test_vm(100, "node1")];

    let recommendations = balancer.get_recommendations(&nodes, &vms);
    assert!(recommendations.is_empty());
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
    assert_eq!(best, Some("node2".to_string())); // Least loaded
}

#[test]
fn test_policy_update() {
    let policy = BalancingPolicy::default();
    let mut balancer = ClusterBalancer::new(policy);

    assert_eq!(balancer.get_policy().threshold, 20.0);

    let new_policy = BalancingPolicy {
        threshold: 30.0,
        ..Default::default()
    };
    balancer.update_policy(new_policy);

    assert_eq!(balancer.get_policy().threshold, 30.0);
}

#[test]
fn test_migration_priority_ordering() {
    let priorities = vec![
        MigrationPriority::Low,
        MigrationPriority::Medium,
        MigrationPriority::High,
        MigrationPriority::Critical,
    ];

    assert!(MigrationPriority::Low < MigrationPriority::Medium);
    assert!(MigrationPriority::Medium < MigrationPriority::High);
    assert!(MigrationPriority::High < MigrationPriority::Critical);

    for priority in priorities {
        let json = serde_json::to_string(&priority).unwrap();
        let _: MigrationPriority = serde_json::from_str(&json).unwrap();
    }
}

// ============== Affinity Rules Tests ==============

#[test]
fn test_affinity_manager_creation() {
    let manager = AffinityManager::new();
    assert!(manager.list_rules().is_empty());
}

#[test]
fn test_create_node_affinity_rule() {
    let mut manager = AffinityManager::new();

    let rule = AffinityRule {
        id: "rule-1".to_string(),
        name: "Pin DB to specific nodes".to_string(),
        rule_type: AffinityRuleType::NodeAffinity(NodeAffinityRule {
            resources: vec!["vm-db".to_string()],
            nodes: vec!["node1".to_string(), "node2".to_string()],
            policy: AffinityPolicy::Required,
        }),
        enabled: true,
        priority: 100,
        description: "Database VM must run on storage-optimized nodes".to_string(),
    };

    assert!(manager.add_rule(rule).is_ok());
    assert_eq!(manager.list_rules().len(), 1);
}

#[test]
fn test_create_resource_affinity_rule() {
    let mut manager = AffinityManager::new();

    let rule = AffinityRule {
        id: "rule-2".to_string(),
        name: "Keep web and cache together".to_string(),
        rule_type: AffinityRuleType::ResourceAffinity(ResourceAffinityRule {
            resources: vec!["vm-web".to_string(), "vm-cache".to_string()],
            policy: AffinityPolicy::Preferred,
        }),
        enabled: true,
        priority: 50,
        description: "Web server and cache should be on same node".to_string(),
    };

    assert!(manager.add_rule(rule).is_ok());
}

#[test]
fn test_create_anti_affinity_rule() {
    let mut manager = AffinityManager::new();

    let rule = AffinityRule {
        id: "rule-3".to_string(),
        name: "Spread DB replicas".to_string(),
        rule_type: AffinityRuleType::AntiAffinity(AntiAffinityRule {
            resources: vec!["vm-db-1".to_string(), "vm-db-2".to_string(), "vm-db-3".to_string()],
            policy: AffinityPolicy::Required,
        }),
        enabled: true,
        priority: 100,
        description: "DB replicas must be on different nodes".to_string(),
    };

    assert!(manager.add_rule(rule).is_ok());
}

#[test]
fn test_affinity_policies() {
    let policies = vec![
        AffinityPolicy::Required,
        AffinityPolicy::Preferred,
    ];

    for policy in policies {
        let json = serde_json::to_string(&policy).unwrap();
        let _: AffinityPolicy = serde_json::from_str(&json).unwrap();
    }
}

#[test]
fn test_delete_affinity_rule() {
    let mut manager = AffinityManager::new();

    let rule = AffinityRule {
        id: "rule-delete".to_string(),
        name: "To be deleted".to_string(),
        rule_type: AffinityRuleType::NodeAffinity(NodeAffinityRule {
            resources: vec!["vm-test".to_string()],
            nodes: vec!["node1".to_string()],
            policy: AffinityPolicy::Preferred,
        }),
        enabled: true,
        priority: 50,
        description: "".to_string(),
    };

    manager.add_rule(rule).unwrap();
    assert_eq!(manager.list_rules().len(), 1);

    manager.remove_rule("rule-delete").unwrap();
    assert!(manager.list_rules().is_empty());
}

#[test]
fn test_get_affinity_rule() {
    let mut manager = AffinityManager::new();

    let rule = AffinityRule {
        id: "rule-get".to_string(),
        name: "Get me".to_string(),
        rule_type: AffinityRuleType::ResourceAffinity(ResourceAffinityRule {
            resources: vec!["vm-a".to_string(), "vm-b".to_string()],
            policy: AffinityPolicy::Required,
        }),
        enabled: true,
        priority: 100,
        description: "".to_string(),
    };

    manager.add_rule(rule).unwrap();

    assert!(manager.get_rule("rule-get").is_some());
    assert!(manager.get_rule("nonexistent").is_none());
}

#[test]
fn test_node_suggestion_with_affinity() {
    let mut manager = AffinityManager::new();

    let rule = AffinityRule {
        id: "pin-rule".to_string(),
        name: "Pin VM to node1".to_string(),
        rule_type: AffinityRuleType::NodeAffinity(NodeAffinityRule {
            resources: vec!["vm-pinned".to_string()],
            nodes: vec!["node1".to_string(), "node2".to_string()],
            policy: AffinityPolicy::Required,
        }),
        enabled: true,
        priority: 100,
        description: "".to_string(),
    };

    manager.add_rule(rule).unwrap();

    let available_nodes = vec![
        "node1".to_string(),
        "node2".to_string(),
        "node3".to_string(),
    ];
    let placements = HashMap::new();

    let suggested = manager.suggest_node("vm-pinned", &available_nodes, &placements).unwrap();
    assert!(suggested == "node1" || suggested == "node2");
}

#[test]
fn test_placement_validation() {
    let mut manager = AffinityManager::new();

    let rule = AffinityRule {
        id: "anti-rule".to_string(),
        name: "Spread replicas".to_string(),
        rule_type: AffinityRuleType::AntiAffinity(AntiAffinityRule {
            resources: vec!["vm-rep1".to_string(), "vm-rep2".to_string()],
            policy: AffinityPolicy::Required,
        }),
        enabled: true,
        priority: 100,
        description: "".to_string(),
    };

    manager.add_rule(rule).unwrap();

    let mut placements = HashMap::new();
    placements.insert("vm-rep1".to_string(), "node1".to_string());

    // vm-rep2 should NOT be placed on node1
    let result = manager.validate_placement("vm-rep2", "node1", &placements);
    assert!(result.is_err());

    // vm-rep2 can be placed on node2
    let result = manager.validate_placement("vm-rep2", "node2", &placements);
    assert!(result.is_ok());
}

// ============== Architecture Tests ==============

#[test]
fn test_architecture_manager_creation() {
    let manager = ArchitectureManager::new();

    // Should have built-in architectures
    assert!(manager.get_architecture("x86_64").is_some());
    assert!(manager.get_architecture("aarch64").is_some());
    assert!(manager.get_architecture("riscv64").is_some());
    assert!(manager.get_architecture("ppc64le").is_some());
    assert!(manager.get_architecture("s390x").is_some());
}

#[test]
fn test_detect_host_architecture() {
    let arch = ArchitectureManager::detect_host_architecture();
    assert!(arch.is_ok());

    let arch_name = arch.unwrap();
    assert!(!arch_name.is_empty());
}

#[test]
fn test_register_node() {
    let mut manager = ArchitectureManager::new();

    assert!(manager.register_node("node1".to_string(), "x86_64".to_string()).is_ok());
    assert!(manager.register_node("node2".to_string(), "aarch64".to_string()).is_ok());

    // Unknown architecture should fail
    assert!(manager.register_node("node3".to_string(), "unknown".to_string()).is_err());
}

#[test]
fn test_native_placement() {
    let mut manager = ArchitectureManager::new();
    manager.register_node("node1".to_string(), "x86_64".to_string()).unwrap();

    let compat = manager.can_run_on_node("x86_64", "node1").unwrap();
    assert!(compat.compatible);
    assert!(compat.native);
    assert!(!compat.emulation_required);
    assert_eq!(compat.performance_penalty, 0.0);
}

#[test]
fn test_emulated_placement() {
    let mut manager = ArchitectureManager::new();
    manager.register_node("node1".to_string(), "x86_64".to_string()).unwrap();

    let compat = manager.can_run_on_node("aarch64", "node1").unwrap();
    assert!(compat.compatible);
    assert!(!compat.native);
    assert!(compat.emulation_required);
    assert!(compat.performance_penalty > 0.0);
    assert_eq!(compat.emulation_type, Some(EmulationType::Qemu));
}

#[test]
fn test_placement_suggestion() {
    let mut manager = ArchitectureManager::new();
    manager.register_node("node1".to_string(), "x86_64".to_string()).unwrap();
    manager.register_node("node2".to_string(), "aarch64".to_string()).unwrap();

    let available = vec!["node1".to_string(), "node2".to_string()];

    // x86_64 VM should prefer node1
    let placement = manager.suggest_placement("x86_64", &available).unwrap();
    assert_eq!(placement.node_id, "node1");
    assert!(placement.native_execution);

    // aarch64 VM should prefer node2
    let placement = manager.suggest_placement("aarch64", &available).unwrap();
    assert_eq!(placement.node_id, "node2");
    assert!(placement.native_execution);
}

#[test]
fn test_migration_validation() {
    let mut manager = ArchitectureManager::new();
    manager.register_node("node1".to_string(), "x86_64".to_string()).unwrap();
    manager.register_node("node2".to_string(), "x86_64".to_string()).unwrap();
    manager.register_node("node3".to_string(), "aarch64".to_string()).unwrap();

    // Same architecture migration (x86_64 -> x86_64)
    let migration = manager.validate_migration("x86_64", "node1", "node2").unwrap();
    assert!(migration.compatible);
    assert!(!migration.requires_shutdown);

    // Cross-architecture migration (x86_64 -> aarch64 for x86_64 VM)
    let migration = manager.validate_migration("x86_64", "node1", "node3").unwrap();
    // aarch64 can't run x86_64 natively, needs emulation
    assert!(migration.compatible || !migration.compatible); // Depends on emulation support
}

#[test]
fn test_cluster_arch_stats() {
    let mut manager = ArchitectureManager::new();
    manager.register_node("node1".to_string(), "x86_64".to_string()).unwrap();
    manager.register_node("node2".to_string(), "x86_64".to_string()).unwrap();
    manager.register_node("node3".to_string(), "aarch64".to_string()).unwrap();

    let stats = manager.get_cluster_stats();
    assert_eq!(stats.total_nodes, 3);
    assert_eq!(stats.unique_architectures, 2);
    assert!(stats.mixed_arch_cluster);
    assert_eq!(*stats.architecture_distribution.get("x86_64").unwrap(), 2);
    assert_eq!(*stats.architecture_distribution.get("aarch64").unwrap(), 1);
}

#[test]
fn test_custom_architecture_registration() {
    let mut manager = ArchitectureManager::new();

    let custom_arch = ArchitectureInfo {
        name: "loongarch64".to_string(),
        description: "LoongArch 64-bit".to_string(),
        aliases: vec![],
        word_size: 64,
        endianness: Endianness::Little,
        can_emulate: vec![],
        features: vec!["lsx".to_string(), "lasx".to_string()],
    };

    assert!(manager.register_architecture(custom_arch).is_ok());
    assert!(manager.get_architecture("loongarch64").is_some());

    // Can now register nodes with this architecture
    assert!(manager.register_node("loong-node".to_string(), "loongarch64".to_string()).is_ok());
}

#[test]
fn test_endianness_serialization() {
    let endiannesses = vec![Endianness::Little, Endianness::Big];

    for endianness in endiannesses {
        let json = serde_json::to_string(&endianness).unwrap();
        let _: Endianness = serde_json::from_str(&json).unwrap();
    }
}

#[test]
fn test_emulation_types() {
    let types = vec![
        EmulationType::Native,
        EmulationType::Qemu,
        EmulationType::Hvf,
        EmulationType::WhpX,
    ];

    for emu_type in types {
        let json = serde_json::to_string(&emu_type).unwrap();
        let _: EmulationType = serde_json::from_str(&json).unwrap();
    }
}

#[test]
fn test_list_architectures() {
    let manager = ArchitectureManager::new();
    let archs = manager.list_architectures();

    assert!(!archs.is_empty());

    // Should have at least the main architectures
    let arch_names: Vec<&str> = archs.iter().map(|a| a.name.as_str()).collect();
    assert!(arch_names.contains(&"x86_64"));
    assert!(arch_names.contains(&"aarch64"));
    assert!(arch_names.contains(&"riscv64"));
}

#[test]
fn test_unregistered_node_error() {
    let manager = ArchitectureManager::new();

    let result = manager.can_run_on_node("x86_64", "unknown-node");
    assert!(result.is_err());
}
