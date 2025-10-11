//! SDN Fabrics Module
//!
//! Implements spine-leaf network architecture with multi-path support,
//! automatic failover, and routing protocol integration (OpenFabric, OSPF).
//!
//! Proxmox VE 9.0 Feature: SDN Fabrics for scalable network architectures

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Network fabric configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fabric {
    pub id: String,
    pub name: String,
    pub fabric_type: FabricType,
    pub spine_nodes: Vec<String>,   // Spine layer nodes
    pub leaf_nodes: Vec<String>,    // Leaf layer nodes
    pub routing_protocol: RoutingProtocol,
    pub redundancy: RedundancyConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FabricType {
    /// Traditional spine-leaf (2-tier Clos)
    SpineLeaf,
    /// Multi-tier spine-leaf (3+ tiers)
    MultiTier,
    /// Collapsed spine (single tier, all nodes peer)
    Collapsed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RoutingProtocol {
    /// OpenFabric - Open routing protocol for data center fabrics
    OpenFabric(OpenFabricConfig),
    /// OSPF - Open Shortest Path First
    Ospf(OspfConfig),
    /// BGP - Border Gateway Protocol (for EVPN)
    Bgp(BgpConfig),
    /// Static routing
    Static,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenFabricConfig {
    pub area: String,           // IS-IS area
    pub tier: u8,               // Tier in the fabric (0=spine, 1=leaf)
    pub flooding_reduction: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OspfConfig {
    pub area_id: String,        // OSPF area ID (e.g., "0.0.0.0")
    pub network_type: OspfNetworkType,
    pub hello_interval: u16,    // seconds
    pub dead_interval: u16,     // seconds
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum OspfNetworkType {
    PointToPoint,
    Broadcast,
    NonBroadcast,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BgpConfig {
    pub asn: u32,               // Autonomous System Number
    pub router_id: String,      // BGP router ID
    pub neighbors: Vec<BgpNeighbor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BgpNeighbor {
    pub ip: String,
    pub asn: u32,
}

/// Redundancy and failover configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedundancyConfig {
    pub uplinks_per_leaf: u8,   // Number of uplinks from each leaf to spines
    pub lacp_enabled: bool,     // Link Aggregation Control Protocol
    pub auto_failover: bool,    // Automatic NIC failover
    pub fast_convergence: bool, // Fast routing convergence
}

/// Fabric link between nodes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FabricLink {
    pub id: String,
    pub source_node: String,
    pub source_interface: String,
    pub dest_node: String,
    pub dest_interface: String,
    pub link_type: LinkType,
    pub bandwidth_gbps: u32,
    pub status: LinkStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum LinkType {
    Spine,      // Spine to leaf
    Leaf,       // Leaf to compute/storage
    Peer,       // Spine to spine or leaf to leaf
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum LinkStatus {
    Up,
    Down,
    Degraded,
}

/// Fabric manager for managing spine-leaf architectures
pub struct FabricManager {
    fabrics: HashMap<String, Fabric>,
    links: HashMap<String, FabricLink>,
    routing_tables: HashMap<String, RoutingTable>,
}

impl FabricManager {
    pub fn new() -> Self {
        FabricManager {
            fabrics: HashMap::new(),
            links: HashMap::new(),
            routing_tables: HashMap::new(),
        }
    }

    /// Create a new fabric
    pub fn create_fabric(&mut self, fabric: Fabric) -> Result<(), String> {
        if self.fabrics.contains_key(&fabric.id) {
            return Err(format!("Fabric {} already exists", fabric.id));
        }

        // Validate fabric configuration
        self.validate_fabric(&fabric)?;

        // Initialize routing for the fabric
        self.initialize_routing(&fabric)?;

        self.fabrics.insert(fabric.id.clone(), fabric);
        Ok(())
    }

    /// Add a link to the fabric
    pub fn add_link(&mut self, link: FabricLink) -> Result<(), String> {
        if self.links.contains_key(&link.id) {
            return Err(format!("Link {} already exists", link.id));
        }

        // Validate link
        self.validate_link(&link)?;

        self.links.insert(link.id.clone(), link);
        Ok(())
    }

    /// Calculate routing paths (ECMP - Equal-Cost Multi-Path)
    pub fn calculate_paths(
        &self,
        fabric_id: &str,
        source: &str,
        destination: &str,
    ) -> Result<Vec<Vec<String>>, String> {
        let fabric = self.fabrics.get(fabric_id)
            .ok_or_else(|| format!("Fabric {} not found", fabric_id))?;

        match fabric.fabric_type {
            FabricType::SpineLeaf => {
                // In spine-leaf, all paths go through spines
                self.calculate_spine_leaf_paths(fabric, source, destination)
            }
            FabricType::MultiTier => {
                self.calculate_multi_tier_paths(fabric, source, destination)
            }
            FabricType::Collapsed => {
                // Direct paths in collapsed fabric
                Ok(vec![vec![source.to_string(), destination.to_string()]])
            }
        }
    }

    /// Handle link failure and trigger failover
    pub fn handle_link_failure(&mut self, link_id: &str) -> Result<(), String> {
        let link = self.links.get_mut(link_id)
            .ok_or_else(|| format!("Link {} not found", link_id))?;

        link.status = LinkStatus::Down;

        // Find affected fabric
        let fabric_id = self.find_fabric_for_link(link_id)?;

        // Check fabric configuration without holding borrow
        let (auto_failover, needs_routing_update) = {
            let fabric = self.fabrics.get(&fabric_id)
                .ok_or_else(|| format!("Fabric {} not found", fabric_id))?;

            let auto_failover = fabric.redundancy.auto_failover;
            let needs_routing = matches!(
                fabric.routing_protocol,
                RoutingProtocol::OpenFabric(_) | RoutingProtocol::Ospf(_)
            );
            (auto_failover, needs_routing)
        };

        if auto_failover {
            // Trigger automatic failover
            self.trigger_failover(&fabric_id, link_id)?;
        }

        // Recalculate routing if using dynamic protocols
        if needs_routing_update {
            self.recalculate_routing(&fabric_id)?;
        }

        Ok(())
    }

    /// Get fabric statistics
    pub fn get_fabric_stats(&self, fabric_id: &str) -> Result<FabricStats, String> {
        let fabric = self.fabrics.get(fabric_id)
            .ok_or_else(|| format!("Fabric {} not found", fabric_id))?;

        let fabric_links: Vec<_> = self.links.values()
            .filter(|l| {
                fabric.spine_nodes.contains(&l.source_node)
                    || fabric.spine_nodes.contains(&l.dest_node)
                    || fabric.leaf_nodes.contains(&l.source_node)
                    || fabric.leaf_nodes.contains(&l.dest_node)
            })
            .collect();

        let total_links = fabric_links.len();
        let up_links = fabric_links.iter().filter(|l| l.status == LinkStatus::Up).count();
        let down_links = fabric_links.iter().filter(|l| l.status == LinkStatus::Down).count();

        let total_bandwidth: u32 = fabric_links.iter()
            .filter(|l| l.status == LinkStatus::Up)
            .map(|l| l.bandwidth_gbps)
            .sum();

        Ok(FabricStats {
            fabric_id: fabric_id.to_string(),
            total_nodes: fabric.spine_nodes.len() + fabric.leaf_nodes.len(),
            spine_nodes: fabric.spine_nodes.len(),
            leaf_nodes: fabric.leaf_nodes.len(),
            total_links,
            up_links,
            down_links,
            total_bandwidth_gbps: total_bandwidth,
            health_percentage: (up_links as f64 / total_links as f64) * 100.0,
        })
    }

    // Helper functions

    fn validate_fabric(&self, fabric: &Fabric) -> Result<(), String> {
        match fabric.fabric_type {
            FabricType::SpineLeaf => {
                if fabric.spine_nodes.is_empty() {
                    return Err("Spine-leaf fabric requires at least one spine node".to_string());
                }
                if fabric.leaf_nodes.is_empty() {
                    return Err("Spine-leaf fabric requires at least one leaf node".to_string());
                }
            }
            FabricType::Collapsed => {
                if fabric.spine_nodes.len() + fabric.leaf_nodes.len() < 2 {
                    return Err("Collapsed fabric requires at least two nodes".to_string());
                }
            }
            _ => {}
        }

        Ok(())
    }

    fn validate_link(&self, _link: &FabricLink) -> Result<(), String> {
        // Validate link bandwidth
        // Validate interface names
        // Check for duplicate links
        Ok(())
    }

    fn initialize_routing(&mut self, fabric: &Fabric) -> Result<(), String> {
        match &fabric.routing_protocol {
            RoutingProtocol::OpenFabric(_config) => {
                // Initialize OpenFabric (IS-IS based) routing (config will be used for area/tier settings)
                for node in fabric.spine_nodes.iter().chain(fabric.leaf_nodes.iter()) {
                    let rt = RoutingTable {
                        node_id: node.clone(),
                        protocol: "OpenFabric".to_string(),
                        routes: vec![],
                    };
                    self.routing_tables.insert(node.clone(), rt);
                }
            }
            RoutingProtocol::Ospf(_config) => {
                // Initialize OSPF routing (config will be used for area ID and timers)
                for node in fabric.spine_nodes.iter().chain(fabric.leaf_nodes.iter()) {
                    let rt = RoutingTable {
                        node_id: node.clone(),
                        protocol: "OSPF".to_string(),
                        routes: vec![],
                    };
                    self.routing_tables.insert(node.clone(), rt);
                }
            }
            _ => {}
        }

        Ok(())
    }

    fn calculate_spine_leaf_paths(
        &self,
        fabric: &Fabric,
        source: &str,
        destination: &str,
    ) -> Result<Vec<Vec<String>>, String> {
        let mut paths = Vec::new();

        // In spine-leaf, traffic goes: leaf -> spine -> leaf
        // Each spine provides an equal-cost path (ECMP)
        for spine in &fabric.spine_nodes {
            let path = vec![
                source.to_string(),
                spine.clone(),
                destination.to_string(),
            ];
            paths.push(path);
        }

        Ok(paths)
    }

    fn calculate_multi_tier_paths(
        &self,
        _fabric: &Fabric,
        source: &str,
        destination: &str,
    ) -> Result<Vec<Vec<String>>, String> {
        // Simplified multi-tier path calculation
        Ok(vec![vec![source.to_string(), destination.to_string()]])
    }

    fn find_fabric_for_link(&self, link_id: &str) -> Result<String, String> {
        let link = self.links.get(link_id)
            .ok_or_else(|| format!("Link {} not found", link_id))?;

        for (fabric_id, fabric) in &self.fabrics {
            if fabric.spine_nodes.contains(&link.source_node)
                || fabric.leaf_nodes.contains(&link.source_node) {
                return Ok(fabric_id.clone());
            }
        }

        Err("No fabric found for link".to_string())
    }

    fn trigger_failover(&mut self, fabric_id: &str, failed_link_id: &str) -> Result<(), String> {
        let _fabric = self.fabrics.get(fabric_id)
            .ok_or_else(|| format!("Fabric {} not found", fabric_id))?;

        let _failed_link = self.links.get(failed_link_id)
            .ok_or_else(|| format!("Link {} not found", failed_link_id))?;

        tracing::warn!(
            "Triggering failover for link {} in fabric {}",
            failed_link_id,
            fabric_id
        );

        // Mark link as down
        if let Some(link) = self.links.get_mut(failed_link_id) {
            link.status = LinkStatus::Down;
        }

        // Recalculate all affected routing tables
        self.recalculate_routing(fabric_id)?;

        tracing::info!("Failover complete for fabric {}", fabric_id);
        Ok(())
    }

    fn recalculate_routing(&mut self, fabric_id: &str) -> Result<(), String> {
        let fabric = self.fabrics.get(fabric_id)
            .ok_or_else(|| format!("Fabric {} not found", fabric_id))?;

        tracing::debug!("Recalculating routing for fabric {}", fabric_id);

        // Get all active links
        let active_links: Vec<_> = self.links.values()
            .filter(|link| link.status == LinkStatus::Up)
            .collect();

        // Recalculate routes for each node
        let all_nodes: Vec<_> = fabric.spine_nodes.iter().chain(fabric.leaf_nodes.iter()).cloned().collect();

        for node in &all_nodes {
            // Calculate new routes for this node
            let mut new_routes = Vec::new();

            for target in &all_nodes {
                if target == node {
                    continue;
                }

                // Find paths using Dijkstra-like algorithm
                if let Ok(paths) = self.calculate_spine_leaf_paths(fabric, node, target) {
                    // Add routes for all valid paths (ECMP)
                    for path in paths {
                        if path.len() >= 2 && Self::path_uses_only_active_links_static(&path, &active_links) {
                            let next_hop = path.get(1).unwrap().clone();

                            new_routes.push(Route {
                                destination: target.clone(),
                                next_hop,
                                metric: (path.len() - 1) as u32,
                            });
                        }
                    }
                }
            }

            // Update routing table with new routes
            if let Some(routing_table) = self.routing_tables.get_mut(node) {
                routing_table.routes = new_routes;
            }
        }

        tracing::info!("Routing recalculation complete for fabric {}", fabric_id);
        Ok(())
    }

    /// Check if a path uses only active links (static version)
    fn path_uses_only_active_links_static(path: &[String], active_links: &[&FabricLink]) -> bool {
        for i in 0..path.len() - 1 {
            let source = &path[i];
            let dest = &path[i + 1];

            // Check if link exists and is active
            let has_active_link = active_links.iter().any(|link| {
                (&link.source_node == source && &link.dest_node == dest) ||
                (&link.source_node == dest && &link.dest_node == source)
            });

            if !has_active_link {
                return false;
            }
        }
        true
    }

    /// List all fabrics
    pub fn list_fabrics(&self) -> Vec<&Fabric> {
        self.fabrics.values().collect()
    }

    /// Get fabric by ID
    pub fn get_fabric(&self, fabric_id: &str) -> Option<&Fabric> {
        self.fabrics.get(fabric_id)
    }

    /// List links in a fabric
    pub fn list_fabric_links(&self, fabric_id: &str) -> Result<Vec<&FabricLink>, String> {
        let fabric = self.fabrics.get(fabric_id)
            .ok_or_else(|| format!("Fabric {} not found", fabric_id))?;

        let links: Vec<_> = self.links.values()
            .filter(|l| {
                fabric.spine_nodes.contains(&l.source_node)
                    || fabric.spine_nodes.contains(&l.dest_node)
                    || fabric.leaf_nodes.contains(&l.source_node)
                    || fabric.leaf_nodes.contains(&l.dest_node)
            })
            .collect();

        Ok(links)
    }
}

/// Routing table for a node
#[derive(Debug, Clone)]
pub struct RoutingTable {
    pub node_id: String,
    pub protocol: String,
    pub routes: Vec<Route>,
}

#[derive(Debug, Clone)]
pub struct Route {
    pub destination: String,
    pub next_hop: String,
    pub metric: u32,
}

/// Fabric statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FabricStats {
    pub fabric_id: String,
    pub total_nodes: usize,
    pub spine_nodes: usize,
    pub leaf_nodes: usize,
    pub total_links: usize,
    pub up_links: usize,
    pub down_links: usize,
    pub total_bandwidth_gbps: u32,
    pub health_percentage: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_spine_leaf_fabric() {
        let mut fm = FabricManager::new();

        let fabric = Fabric {
            id: "fabric1".to_string(),
            name: "Production Fabric".to_string(),
            fabric_type: FabricType::SpineLeaf,
            spine_nodes: vec!["spine1".to_string(), "spine2".to_string()],
            leaf_nodes: vec!["leaf1".to_string(), "leaf2".to_string()],
            routing_protocol: RoutingProtocol::OpenFabric(OpenFabricConfig {
                area: "49.0001".to_string(),
                tier: 0,
                flooding_reduction: true,
            }),
            redundancy: RedundancyConfig {
                uplinks_per_leaf: 2,
                lacp_enabled: true,
                auto_failover: true,
                fast_convergence: true,
            },
        };

        assert!(fm.create_fabric(fabric).is_ok());
        assert_eq!(fm.list_fabrics().len(), 1);
    }

    #[test]
    fn test_calculate_ecmp_paths() {
        let mut fm = FabricManager::new();

        let fabric = Fabric {
            id: "fabric1".to_string(),
            name: "Test".to_string(),
            fabric_type: FabricType::SpineLeaf,
            spine_nodes: vec!["spine1".to_string(), "spine2".to_string()],
            leaf_nodes: vec!["leaf1".to_string(), "leaf2".to_string()],
            routing_protocol: RoutingProtocol::Static,
            redundancy: RedundancyConfig {
                uplinks_per_leaf: 2,
                lacp_enabled: false,
                auto_failover: false,
                fast_convergence: false,
            },
        };

        fm.create_fabric(fabric).unwrap();

        // Calculate paths from leaf1 to leaf2
        let paths = fm.calculate_paths("fabric1", "leaf1", "leaf2").unwrap();

        // Should have 2 paths (one through each spine) for ECMP
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn test_link_failover() {
        let mut fm = FabricManager::new();

        let fabric = Fabric {
            id: "fabric1".to_string(),
            name: "Test".to_string(),
            fabric_type: FabricType::SpineLeaf,
            spine_nodes: vec!["spine1".to_string()],
            leaf_nodes: vec!["leaf1".to_string()],
            routing_protocol: RoutingProtocol::Static,
            redundancy: RedundancyConfig {
                uplinks_per_leaf: 1,
                lacp_enabled: false,
                auto_failover: true,
                fast_convergence: false,
            },
        };

        fm.create_fabric(fabric).unwrap();

        let link = FabricLink {
            id: "link1".to_string(),
            source_node: "leaf1".to_string(),
            source_interface: "eth0".to_string(),
            dest_node: "spine1".to_string(),
            dest_interface: "eth0".to_string(),
            link_type: LinkType::Spine,
            bandwidth_gbps: 10,
            status: LinkStatus::Up,
        };

        fm.add_link(link).unwrap();

        // Simulate link failure
        assert!(fm.handle_link_failure("link1").is_ok());
        assert_eq!(fm.links.get("link1").unwrap().status, LinkStatus::Down);
    }
}
