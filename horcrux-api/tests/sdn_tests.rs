//! SDN (Software Defined Networking) Module Tests
//! Tests for zones, VNets, subnets, IPAM, and network policies

use horcrux_api::sdn::{
    SdnManager, Zone, ZoneType, VNet, VNetType, Subnet, DhcpRange, IpAllocation,
};
use horcrux_api::sdn::policy::{
    NetworkPolicy, NetworkPolicyManager, PolicyType, LabelSelector, LabelExpression,
    LabelOperator, IngressRule, EgressRule, PeerSelector, NetworkPolicyPort, Protocol,
};
use std::collections::HashMap;
use std::net::IpAddr;

// ============== Zone Tests ==============

#[test]
fn test_create_simple_zone() {
    let mut sdn = SdnManager::new();

    let zone = Zone {
        id: "zone-simple".to_string(),
        name: "Simple Zone".to_string(),
        zone_type: ZoneType::Simple,
        description: "A simple VLAN zone".to_string(),
        nodes: vec!["node1".to_string()],
    };

    assert!(sdn.create_zone(zone).is_ok());
    assert_eq!(sdn.list_zones().len(), 1);
}

#[test]
fn test_create_vxlan_zone() {
    let mut sdn = SdnManager::new();

    let zone = Zone {
        id: "zone-vxlan".to_string(),
        name: "VXLAN Zone".to_string(),
        zone_type: ZoneType::Vxlan,
        description: "A VXLAN overlay zone".to_string(),
        nodes: vec!["node1".to_string(), "node2".to_string()],
    };

    assert!(sdn.create_zone(zone).is_ok());
}

#[test]
fn test_vxlan_zone_requires_nodes() {
    let mut sdn = SdnManager::new();

    let zone = Zone {
        id: "zone-vxlan-empty".to_string(),
        name: "Empty VXLAN Zone".to_string(),
        zone_type: ZoneType::Vxlan,
        description: "".to_string(),
        nodes: vec![], // No nodes - should fail
    };

    assert!(sdn.create_zone(zone).is_err());
}

#[test]
fn test_evpn_zone_not_implemented() {
    let mut sdn = SdnManager::new();

    let zone = Zone {
        id: "zone-evpn".to_string(),
        name: "EVPN Zone".to_string(),
        zone_type: ZoneType::Evpn,
        description: "".to_string(),
        nodes: vec!["node1".to_string()],
    };

    let result = sdn.create_zone(zone);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not yet implemented"));
}

#[test]
fn test_duplicate_zone_id() {
    let mut sdn = SdnManager::new();

    let zone = Zone {
        id: "zone-dup".to_string(),
        name: "Zone 1".to_string(),
        zone_type: ZoneType::Simple,
        description: "".to_string(),
        nodes: vec![],
    };

    assert!(sdn.create_zone(zone.clone()).is_ok());
    assert!(sdn.create_zone(zone).is_err());
}

// ============== VNet Tests ==============

#[test]
fn test_create_vlan_vnet() {
    let mut sdn = SdnManager::new();

    // Create zone first
    let zone = Zone {
        id: "zone1".to_string(),
        name: "Test Zone".to_string(),
        zone_type: ZoneType::Simple,
        description: "".to_string(),
        nodes: vec![],
    };
    sdn.create_zone(zone).unwrap();

    let vnet = VNet {
        id: "vnet1".to_string(),
        zone_id: "zone1".to_string(),
        name: "Web Network".to_string(),
        tag: 100,
        vnet_type: VNetType::Vlan,
        subnets: vec![],
        bridge: "vmbr0".to_string(),
    };

    assert!(sdn.create_vnet(vnet).is_ok());
    assert_eq!(sdn.list_vnets().len(), 1);
}

#[test]
fn test_vlan_tag_validation() {
    let mut sdn = SdnManager::new();

    let zone = Zone {
        id: "zone1".to_string(),
        name: "Test".to_string(),
        zone_type: ZoneType::Simple,
        description: "".to_string(),
        nodes: vec![],
    };
    sdn.create_zone(zone).unwrap();

    // Invalid VLAN tag (0)
    let vnet = VNet {
        id: "vnet-invalid-0".to_string(),
        zone_id: "zone1".to_string(),
        name: "Invalid".to_string(),
        tag: 0,
        vnet_type: VNetType::Vlan,
        subnets: vec![],
        bridge: "vmbr0".to_string(),
    };
    assert!(sdn.create_vnet(vnet).is_err());

    // Invalid VLAN tag (4095+)
    let vnet = VNet {
        id: "vnet-invalid-5000".to_string(),
        zone_id: "zone1".to_string(),
        name: "Invalid".to_string(),
        tag: 5000,
        vnet_type: VNetType::Vlan,
        subnets: vec![],
        bridge: "vmbr0".to_string(),
    };
    assert!(sdn.create_vnet(vnet).is_err());

    // Valid VLAN tag
    let vnet = VNet {
        id: "vnet-valid".to_string(),
        zone_id: "zone1".to_string(),
        name: "Valid".to_string(),
        tag: 100,
        vnet_type: VNetType::Vlan,
        subnets: vec![],
        bridge: "vmbr0".to_string(),
    };
    assert!(sdn.create_vnet(vnet).is_ok());
}

#[test]
fn test_vxlan_vni_validation() {
    let mut sdn = SdnManager::new();

    let zone = Zone {
        id: "zone1".to_string(),
        name: "Test".to_string(),
        zone_type: ZoneType::Vxlan,
        description: "".to_string(),
        nodes: vec!["node1".to_string()],
    };
    sdn.create_zone(zone).unwrap();

    // Valid VXLAN VNI
    let vnet = VNet {
        id: "vnet-vxlan".to_string(),
        zone_id: "zone1".to_string(),
        name: "VXLAN Net".to_string(),
        tag: 10000,
        vnet_type: VNetType::Vxlan,
        subnets: vec![],
        bridge: "vmbr0".to_string(),
    };
    assert!(sdn.create_vnet(vnet).is_ok());

    // Invalid VNI (too large)
    let vnet = VNet {
        id: "vnet-vxlan-invalid".to_string(),
        zone_id: "zone1".to_string(),
        name: "Invalid VXLAN".to_string(),
        tag: 20000000, // > 16777215
        vnet_type: VNetType::Vxlan,
        subnets: vec![],
        bridge: "vmbr0".to_string(),
    };
    assert!(sdn.create_vnet(vnet).is_err());
}

#[test]
fn test_vnet_requires_zone() {
    let mut sdn = SdnManager::new();

    let vnet = VNet {
        id: "vnet-no-zone".to_string(),
        zone_id: "nonexistent".to_string(),
        name: "Orphan VNet".to_string(),
        tag: 100,
        vnet_type: VNetType::Vlan,
        subnets: vec![],
        bridge: "vmbr0".to_string(),
    };

    assert!(sdn.create_vnet(vnet).is_err());
}

#[test]
fn test_tag_conflict_in_zone() {
    let mut sdn = SdnManager::new();

    let zone = Zone {
        id: "zone1".to_string(),
        name: "Test".to_string(),
        zone_type: ZoneType::Simple,
        description: "".to_string(),
        nodes: vec![],
    };
    sdn.create_zone(zone).unwrap();

    let vnet1 = VNet {
        id: "vnet1".to_string(),
        zone_id: "zone1".to_string(),
        name: "First".to_string(),
        tag: 100,
        vnet_type: VNetType::Vlan,
        subnets: vec![],
        bridge: "vmbr0".to_string(),
    };
    assert!(sdn.create_vnet(vnet1).is_ok());

    // Same tag in same zone should fail
    let vnet2 = VNet {
        id: "vnet2".to_string(),
        zone_id: "zone1".to_string(),
        name: "Second".to_string(),
        tag: 100, // Duplicate tag
        vnet_type: VNetType::Vlan,
        subnets: vec![],
        bridge: "vmbr0".to_string(),
    };
    assert!(sdn.create_vnet(vnet2).is_err());
}

#[test]
fn test_list_vnets_in_zone() {
    let mut sdn = SdnManager::new();

    // Create two zones
    sdn.create_zone(Zone {
        id: "zone1".to_string(),
        name: "Zone 1".to_string(),
        zone_type: ZoneType::Simple,
        description: "".to_string(),
        nodes: vec![],
    }).unwrap();

    sdn.create_zone(Zone {
        id: "zone2".to_string(),
        name: "Zone 2".to_string(),
        zone_type: ZoneType::Simple,
        description: "".to_string(),
        nodes: vec![],
    }).unwrap();

    // Create VNets in each zone
    sdn.create_vnet(VNet {
        id: "vnet-z1-1".to_string(),
        zone_id: "zone1".to_string(),
        name: "Z1 Net 1".to_string(),
        tag: 100,
        vnet_type: VNetType::Vlan,
        subnets: vec![],
        bridge: "vmbr0".to_string(),
    }).unwrap();

    sdn.create_vnet(VNet {
        id: "vnet-z1-2".to_string(),
        zone_id: "zone1".to_string(),
        name: "Z1 Net 2".to_string(),
        tag: 200,
        vnet_type: VNetType::Vlan,
        subnets: vec![],
        bridge: "vmbr0".to_string(),
    }).unwrap();

    sdn.create_vnet(VNet {
        id: "vnet-z2-1".to_string(),
        zone_id: "zone2".to_string(),
        name: "Z2 Net 1".to_string(),
        tag: 100,
        vnet_type: VNetType::Vlan,
        subnets: vec![],
        bridge: "vmbr1".to_string(),
    }).unwrap();

    assert_eq!(sdn.list_vnets().len(), 3);
    assert_eq!(sdn.list_vnets_in_zone("zone1").len(), 2);
    assert_eq!(sdn.list_vnets_in_zone("zone2").len(), 1);
    assert_eq!(sdn.list_vnets_in_zone("nonexistent").len(), 0);
}

// ============== Subnet Tests ==============

#[test]
fn test_create_subnet() {
    let mut sdn = SdnManager::new();

    sdn.create_zone(Zone {
        id: "zone1".to_string(),
        name: "Test".to_string(),
        zone_type: ZoneType::Simple,
        description: "".to_string(),
        nodes: vec![],
    }).unwrap();

    sdn.create_vnet(VNet {
        id: "vnet1".to_string(),
        zone_id: "zone1".to_string(),
        name: "Test Net".to_string(),
        tag: 100,
        vnet_type: VNetType::Vlan,
        subnets: vec![],
        bridge: "vmbr0".to_string(),
    }).unwrap();

    let subnet = Subnet {
        id: "subnet1".to_string(),
        vnet_id: "vnet1".to_string(),
        cidr: "10.0.1.0/24".to_string(),
        gateway: Some("10.0.1.1".parse().unwrap()),
        dns_servers: vec!["8.8.8.8".parse().unwrap()],
        dhcp_range: Some(DhcpRange {
            start: "10.0.1.100".parse().unwrap(),
            end: "10.0.1.200".parse().unwrap(),
        }),
    };

    assert!(sdn.create_subnet(subnet).is_ok());
    assert_eq!(sdn.list_subnets().len(), 1);
}

#[test]
fn test_subnet_requires_vnet() {
    let mut sdn = SdnManager::new();

    let subnet = Subnet {
        id: "subnet-orphan".to_string(),
        vnet_id: "nonexistent".to_string(),
        cidr: "10.0.1.0/24".to_string(),
        gateway: None,
        dns_servers: vec![],
        dhcp_range: None,
    };

    assert!(sdn.create_subnet(subnet).is_err());
}

#[test]
fn test_invalid_cidr() {
    let mut sdn = SdnManager::new();

    sdn.create_zone(Zone {
        id: "zone1".to_string(),
        name: "Test".to_string(),
        zone_type: ZoneType::Simple,
        description: "".to_string(),
        nodes: vec![],
    }).unwrap();

    sdn.create_vnet(VNet {
        id: "vnet1".to_string(),
        zone_id: "zone1".to_string(),
        name: "Test".to_string(),
        tag: 100,
        vnet_type: VNetType::Vlan,
        subnets: vec![],
        bridge: "vmbr0".to_string(),
    }).unwrap();

    // Missing prefix
    let subnet = Subnet {
        id: "subnet-invalid".to_string(),
        vnet_id: "vnet1".to_string(),
        cidr: "10.0.1.0".to_string(), // No /prefix
        gateway: None,
        dns_servers: vec![],
        dhcp_range: None,
    };

    assert!(sdn.create_subnet(subnet).is_err());
}

// ============== IPAM Tests ==============

#[test]
fn test_ip_allocation() {
    let mut sdn = SdnManager::new();

    // Setup
    sdn.create_zone(Zone {
        id: "zone1".to_string(),
        name: "Test".to_string(),
        zone_type: ZoneType::Simple,
        description: "".to_string(),
        nodes: vec![],
    }).unwrap();

    sdn.create_vnet(VNet {
        id: "vnet1".to_string(),
        zone_id: "zone1".to_string(),
        name: "Test".to_string(),
        tag: 100,
        vnet_type: VNetType::Vlan,
        subnets: vec![],
        bridge: "vmbr0".to_string(),
    }).unwrap();

    sdn.create_subnet(Subnet {
        id: "subnet1".to_string(),
        vnet_id: "vnet1".to_string(),
        cidr: "10.0.1.0/24".to_string(),
        gateway: Some("10.0.1.1".parse().unwrap()),
        dns_servers: vec![],
        dhcp_range: None,
    }).unwrap();

    // Allocate IP
    let allocation = sdn.allocate_ip("subnet1", Some("vm-100".to_string()), None);
    assert!(allocation.is_ok());

    let alloc = allocation.unwrap();
    assert_eq!(alloc.subnet_id, "subnet1");
    assert_eq!(alloc.assigned_to, Some("vm-100".to_string()));
}

#[test]
fn test_preferred_ip_allocation() {
    let mut sdn = SdnManager::new();

    // Setup
    sdn.create_zone(Zone {
        id: "zone1".to_string(),
        name: "Test".to_string(),
        zone_type: ZoneType::Simple,
        description: "".to_string(),
        nodes: vec![],
    }).unwrap();

    sdn.create_vnet(VNet {
        id: "vnet1".to_string(),
        zone_id: "zone1".to_string(),
        name: "Test".to_string(),
        tag: 100,
        vnet_type: VNetType::Vlan,
        subnets: vec![],
        bridge: "vmbr0".to_string(),
    }).unwrap();

    sdn.create_subnet(Subnet {
        id: "subnet1".to_string(),
        vnet_id: "vnet1".to_string(),
        cidr: "10.0.1.0/24".to_string(),
        gateway: None,
        dns_servers: vec![],
        dhcp_range: None,
    }).unwrap();

    // Allocate specific IP
    let preferred: IpAddr = "10.0.1.50".parse().unwrap();
    let allocation = sdn.allocate_ip("subnet1", None, Some(preferred));
    assert!(allocation.is_ok());

    let alloc = allocation.unwrap();
    assert_eq!(alloc.ip, preferred);
}

#[test]
fn test_ip_release() {
    let mut sdn = SdnManager::new();

    // Setup
    sdn.create_zone(Zone {
        id: "zone1".to_string(),
        name: "Test".to_string(),
        zone_type: ZoneType::Simple,
        description: "".to_string(),
        nodes: vec![],
    }).unwrap();

    sdn.create_vnet(VNet {
        id: "vnet1".to_string(),
        zone_id: "zone1".to_string(),
        name: "Test".to_string(),
        tag: 100,
        vnet_type: VNetType::Vlan,
        subnets: vec![],
        bridge: "vmbr0".to_string(),
    }).unwrap();

    sdn.create_subnet(Subnet {
        id: "subnet1".to_string(),
        vnet_id: "vnet1".to_string(),
        cidr: "10.0.1.0/24".to_string(),
        gateway: None,
        dns_servers: vec![],
        dhcp_range: None,
    }).unwrap();

    // Allocate and release
    let alloc = sdn.allocate_ip("subnet1", None, None).unwrap();
    assert!(sdn.release_ip(&alloc.ip).is_ok());

    // Can now reallocate the same IP
    let result = sdn.allocate_ip("subnet1", None, Some(alloc.ip));
    assert!(result.is_ok());
}

#[test]
fn test_duplicate_ip_allocation() {
    let mut sdn = SdnManager::new();

    // Setup
    sdn.create_zone(Zone {
        id: "zone1".to_string(),
        name: "Test".to_string(),
        zone_type: ZoneType::Simple,
        description: "".to_string(),
        nodes: vec![],
    }).unwrap();

    sdn.create_vnet(VNet {
        id: "vnet1".to_string(),
        zone_id: "zone1".to_string(),
        name: "Test".to_string(),
        tag: 100,
        vnet_type: VNetType::Vlan,
        subnets: vec![],
        bridge: "vmbr0".to_string(),
    }).unwrap();

    sdn.create_subnet(Subnet {
        id: "subnet1".to_string(),
        vnet_id: "vnet1".to_string(),
        cidr: "10.0.1.0/24".to_string(),
        gateway: None,
        dns_servers: vec![],
        dhcp_range: None,
    }).unwrap();

    // Allocate IP
    let alloc = sdn.allocate_ip("subnet1", None, None).unwrap();

    // Try to allocate same IP again
    let result = sdn.allocate_ip("subnet1", None, Some(alloc.ip));
    assert!(result.is_err());
}

// ============== Network Policy Tests ==============

#[test]
fn test_create_network_policy() {
    let mut manager = NetworkPolicyManager::new();

    let policy = NetworkPolicy {
        id: "policy-1".to_string(),
        name: "Allow Web Traffic".to_string(),
        namespace: "default".to_string(),
        pod_selector: LabelSelector {
            match_labels: HashMap::from([("app".to_string(), "web".to_string())]),
            match_expressions: vec![],
        },
        policy_types: vec![PolicyType::Ingress],
        ingress: vec![IngressRule {
            from: vec![PeerSelector::IpBlock {
                cidr: "0.0.0.0/0".to_string(),
                except: vec!["10.0.0.0/8".to_string()],
            }],
            ports: vec![NetworkPolicyPort {
                protocol: Protocol::TCP,
                port: Some(80),
                end_port: None,
            }],
        }],
        egress: vec![],
        enabled: true,
    };

    assert!(manager.create_policy(policy).is_ok());
    assert_eq!(manager.list_policies().len(), 1);
}

#[test]
fn test_policy_empty_name() {
    let mut manager = NetworkPolicyManager::new();

    let policy = NetworkPolicy {
        id: "policy-empty".to_string(),
        name: "".to_string(), // Empty name
        namespace: "default".to_string(),
        pod_selector: LabelSelector {
            match_labels: HashMap::new(),
            match_expressions: vec![],
        },
        policy_types: vec![],
        ingress: vec![],
        egress: vec![],
        enabled: true,
    };

    assert!(manager.create_policy(policy).is_err());
}

#[test]
fn test_delete_policy() {
    let mut manager = NetworkPolicyManager::new();

    let policy = NetworkPolicy {
        id: "policy-delete".to_string(),
        name: "To Delete".to_string(),
        namespace: "default".to_string(),
        pod_selector: LabelSelector {
            match_labels: HashMap::new(),
            match_expressions: vec![],
        },
        policy_types: vec![],
        ingress: vec![],
        egress: vec![],
        enabled: true,
    };

    manager.create_policy(policy).unwrap();
    assert_eq!(manager.list_policies().len(), 1);

    manager.delete_policy("policy-delete").unwrap();
    assert!(manager.list_policies().is_empty());
}

#[test]
fn test_list_policies_in_namespace() {
    let mut manager = NetworkPolicyManager::new();

    // Create policies in different namespaces
    for (id, ns) in [("p1", "ns1"), ("p2", "ns1"), ("p3", "ns2")] {
        manager.create_policy(NetworkPolicy {
            id: id.to_string(),
            name: format!("Policy {}", id),
            namespace: ns.to_string(),
            pod_selector: LabelSelector {
                match_labels: HashMap::new(),
                match_expressions: vec![],
            },
            policy_types: vec![],
            ingress: vec![],
            egress: vec![],
            enabled: true,
        }).unwrap();
    }

    assert_eq!(manager.list_policies_in_namespace("ns1").len(), 2);
    assert_eq!(manager.list_policies_in_namespace("ns2").len(), 1);
    assert_eq!(manager.list_policies_in_namespace("ns3").len(), 0);
}

#[test]
fn test_get_policy() {
    let mut manager = NetworkPolicyManager::new();

    let policy = NetworkPolicy {
        id: "policy-get".to_string(),
        name: "Get Me".to_string(),
        namespace: "default".to_string(),
        pod_selector: LabelSelector {
            match_labels: HashMap::new(),
            match_expressions: vec![],
        },
        policy_types: vec![],
        ingress: vec![],
        egress: vec![],
        enabled: true,
    };

    manager.create_policy(policy).unwrap();

    assert!(manager.get_policy("policy-get").is_some());
    assert!(manager.get_policy("nonexistent").is_none());
}

#[test]
fn test_update_pod_policies() {
    let mut manager = NetworkPolicyManager::new();

    // Create policy that matches pods with app=web
    let policy = NetworkPolicy {
        id: "policy-web".to_string(),
        name: "Web Policy".to_string(),
        namespace: "default".to_string(),
        pod_selector: LabelSelector {
            match_labels: HashMap::from([("app".to_string(), "web".to_string())]),
            match_expressions: vec![],
        },
        policy_types: vec![PolicyType::Ingress],
        ingress: vec![],
        egress: vec![],
        enabled: true,
    };

    manager.create_policy(policy).unwrap();

    // Update policies for a pod with matching labels
    let labels = HashMap::from([("app".to_string(), "web".to_string())]);
    manager.update_pod_policies("pod-1", &labels, "default");

    // Update policies for a pod without matching labels
    let other_labels = HashMap::from([("app".to_string(), "db".to_string())]);
    manager.update_pod_policies("pod-2", &other_labels, "default");
}

#[test]
fn test_protocol_serialization() {
    let protocols = vec![Protocol::TCP, Protocol::UDP, Protocol::SCTP];

    for protocol in protocols {
        let json = serde_json::to_string(&protocol).unwrap();
        let deserialized: Protocol = serde_json::from_str(&json).unwrap();
        assert_eq!(protocol, deserialized);
    }
}

#[test]
fn test_label_operator_serialization() {
    let operators = vec![
        LabelOperator::In,
        LabelOperator::NotIn,
        LabelOperator::Exists,
        LabelOperator::DoesNotExist,
    ];

    for op in operators {
        let json = serde_json::to_string(&op).unwrap();
        let deserialized: LabelOperator = serde_json::from_str(&json).unwrap();
        assert_eq!(op, deserialized);
    }
}
