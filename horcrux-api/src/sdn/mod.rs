//! Software Defined Networking (SDN) Module
//!
//! Provides network virtualization capabilities:
//! - VLANs (Virtual LANs)
//! - VXLAN (Virtual Extensible LAN) overlay networks
//! - Network zones for isolation
//! - IPAM (IP Address Management)
//! - Bridge management
//!
//! Architecture:
//! - Zones: Top-level network containers (can span multiple nodes)
//! - VNets: Virtual networks within zones (VLAN or VXLAN)
//! - Subnets: IP address ranges within VNets
//! - IPAM: Track IP allocation and assignment

#![allow(dead_code)]

pub mod vlan;
pub mod vxlan;
pub mod ovs;
pub mod templates;
pub mod ipam;
pub mod zones;
pub mod bridge;
pub mod fabric;
pub mod cni;
pub mod policy;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};

/// Network zone - top level container for virtual networks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Zone {
    pub id: String,
    pub name: String,
    pub zone_type: ZoneType,
    pub description: String,
    pub nodes: Vec<String>, // Node IDs that are part of this zone
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ZoneType {
    Simple,   // Basic zone with VLANs
    Vxlan,    // VXLAN overlay network
    Evpn,     // EVPN (Ethernet VPN) - advanced, future
}

/// Virtual network (VNet) - actual network within a zone
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VNet {
    pub id: String,
    pub zone_id: String,
    pub name: String,
    pub tag: u32,         // VLAN tag (1-4094) or VXLAN VNI
    pub vnet_type: VNetType,
    pub subnets: Vec<Subnet>,
    pub bridge: String,   // Linux bridge name
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum VNetType {
    Vlan,
    Vxlan,
}

/// Subnet within a VNet
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subnet {
    pub id: String,
    pub vnet_id: String,
    pub cidr: String,     // e.g., "10.0.1.0/24"
    pub gateway: Option<IpAddr>,
    pub dns_servers: Vec<IpAddr>,
    pub dhcp_range: Option<DhcpRange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DhcpRange {
    pub start: IpAddr,
    pub end: IpAddr,
}

/// IP allocation record for IPAM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpAllocation {
    pub ip: IpAddr,
    pub subnet_id: String,
    pub assigned_to: Option<String>, // VM/container ID
    pub hostname: Option<String>,
    pub mac_address: Option<String>,
    pub allocated_at: i64,
}

/// SDN configuration manager
pub struct SdnManager {
    zones: HashMap<String, Zone>,
    vnets: HashMap<String, VNet>,
    subnets: HashMap<String, Subnet>,
    allocations: HashMap<IpAddr, IpAllocation>,
}

impl SdnManager {
    pub fn new() -> Self {
        SdnManager {
            zones: HashMap::new(),
            vnets: HashMap::new(),
            subnets: HashMap::new(),
            allocations: HashMap::new(),
        }
    }

    /// Create a new zone
    pub fn create_zone(&mut self, zone: Zone) -> Result<(), String> {
        if self.zones.contains_key(&zone.id) {
            return Err(format!("Zone {} already exists", zone.id));
        }

        // Validate zone type
        match zone.zone_type {
            ZoneType::Simple => {
                // Simple zones are always valid
            }
            ZoneType::Vxlan => {
                // VXLAN requires multicast or controller
                if zone.nodes.is_empty() {
                    return Err("VXLAN zone requires at least one node".to_string());
                }
            }
            ZoneType::Evpn => {
                return Err("EVPN zones not yet implemented".to_string());
            }
        }

        self.zones.insert(zone.id.clone(), zone);
        Ok(())
    }

    /// Create a virtual network
    pub fn create_vnet(&mut self, vnet: VNet) -> Result<(), String> {
        // Validate zone exists
        if !self.zones.contains_key(&vnet.zone_id) {
            return Err(format!("Zone {} not found", vnet.zone_id));
        }

        // Validate tag range
        match vnet.vnet_type {
            VNetType::Vlan => {
                if vnet.tag < 1 || vnet.tag > 4094 {
                    return Err(format!("Invalid VLAN tag: {} (must be 1-4094)", vnet.tag));
                }
            }
            VNetType::Vxlan => {
                if vnet.tag > 16777215 {
                    return Err(format!("Invalid VXLAN VNI: {} (must be 0-16777215)", vnet.tag));
                }
            }
        }

        // Check for tag conflicts in the same zone
        for existing_vnet in self.vnets.values() {
            if existing_vnet.zone_id == vnet.zone_id && existing_vnet.tag == vnet.tag {
                return Err(format!("Tag {} already in use in zone {}", vnet.tag, vnet.zone_id));
            }
        }

        self.vnets.insert(vnet.id.clone(), vnet);
        Ok(())
    }

    /// Create a subnet
    pub fn create_subnet(&mut self, subnet: Subnet) -> Result<(), String> {
        // Validate VNet exists
        if !self.vnets.contains_key(&subnet.vnet_id) {
            return Err(format!("VNet {} not found", subnet.vnet_id));
        }

        // Validate CIDR format
        if !self.validate_cidr(&subnet.cidr) {
            return Err(format!("Invalid CIDR: {}", subnet.cidr));
        }

        self.subnets.insert(subnet.id.clone(), subnet);
        Ok(())
    }

    /// Allocate an IP address
    pub fn allocate_ip(
        &mut self,
        subnet_id: &str,
        assigned_to: Option<String>,
        preferred_ip: Option<IpAddr>,
    ) -> Result<IpAllocation, String> {
        let subnet = self.subnets.get(subnet_id)
            .ok_or_else(|| format!("Subnet {} not found", subnet_id))?
            .clone();

        // Parse CIDR to get network range
        let (network, prefix_len) = self.parse_cidr(&subnet.cidr)?;

        // If preferred IP specified, try to allocate it
        if let Some(ip) = preferred_ip {
            if self.allocations.contains_key(&ip) {
                return Err(format!("IP {} already allocated", ip));
            }

            if !self.ip_in_subnet(&ip, &network, prefix_len) {
                return Err(format!("IP {} not in subnet {}", ip, subnet.cidr));
            }

            let allocation = IpAllocation {
                ip,
                subnet_id: subnet_id.to_string(),
                assigned_to,
                hostname: None,
                mac_address: None,
                allocated_at: chrono::Utc::now().timestamp(),
            };

            self.allocations.insert(ip, allocation.clone());
            return Ok(allocation);
        }

        // Find next available IP
        let available_ip = self.find_next_available_ip(&network, prefix_len)?;

        let allocation = IpAllocation {
            ip: available_ip,
            subnet_id: subnet_id.to_string(),
            assigned_to,
            hostname: None,
            mac_address: None,
            allocated_at: chrono::Utc::now().timestamp(),
        };

        self.allocations.insert(available_ip, allocation.clone());
        Ok(allocation)
    }

    /// Release an IP address
    pub fn release_ip(&mut self, ip: &IpAddr) -> Result<(), String> {
        if self.allocations.remove(ip).is_none() {
            return Err(format!("IP {} not allocated", ip));
        }
        Ok(())
    }

    /// Get all zones
    pub fn list_zones(&self) -> Vec<Zone> {
        self.zones.values().cloned().collect()
    }

    /// Get all VNets
    pub fn list_vnets(&self) -> Vec<VNet> {
        self.vnets.values().cloned().collect()
    }

    /// Get VNets in a specific zone
    pub fn list_vnets_in_zone(&self, zone_id: &str) -> Vec<VNet> {
        self.vnets.values()
            .filter(|v| v.zone_id == zone_id)
            .cloned()
            .collect()
    }

    /// Get all subnets
    pub fn list_subnets(&self) -> Vec<Subnet> {
        self.subnets.values().cloned().collect()
    }

    /// Get allocations in a subnet
    pub fn list_allocations(&self, subnet_id: &str) -> Vec<IpAllocation> {
        self.allocations.values()
            .filter(|a| a.subnet_id == subnet_id)
            .cloned()
            .collect()
    }

    // Helper functions

    fn validate_cidr(&self, cidr: &str) -> bool {
        cidr.contains('/') && cidr.split('/').count() == 2
    }

    fn parse_cidr(&self, cidr: &str) -> Result<(Ipv4Addr, u8), String> {
        let parts: Vec<&str> = cidr.split('/').collect();
        if parts.len() != 2 {
            return Err("Invalid CIDR format".to_string());
        }

        let network: Ipv4Addr = parts[0].parse()
            .map_err(|_| "Invalid IP address".to_string())?;
        let prefix_len: u8 = parts[1].parse()
            .map_err(|_| "Invalid prefix length".to_string())?;

        if prefix_len > 32 {
            return Err("Prefix length must be 0-32".to_string());
        }

        Ok((network, prefix_len))
    }

    fn ip_in_subnet(&self, ip: &IpAddr, network: &Ipv4Addr, prefix_len: u8) -> bool {
        if let IpAddr::V4(ipv4) = ip {
            let ip_u32 = u32::from(*ipv4);
            let network_u32 = u32::from(*network);
            let mask = if prefix_len == 0 { 0 } else { !0u32 << (32 - prefix_len) };

            (ip_u32 & mask) == (network_u32 & mask)
        } else {
            false // IPv6 not yet supported
        }
    }

    fn find_next_available_ip(&self, network: &Ipv4Addr, prefix_len: u8) -> Result<IpAddr, String> {
        let network_u32 = u32::from(*network);
        let host_bits = 32 - prefix_len;
        let max_hosts = (1u32 << host_bits) - 2; // Exclude network and broadcast

        for i in 1..=max_hosts {
            let ip_u32 = network_u32 + i;
            let ip = IpAddr::V4(Ipv4Addr::from(ip_u32));

            if !self.allocations.contains_key(&ip) {
                return Ok(ip);
            }
        }

        Err("No available IPs in subnet".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_zone() {
        let mut sdn = SdnManager::new();

        let zone = Zone {
            id: "zone1".to_string(),
            name: "Production Zone".to_string(),
            zone_type: ZoneType::Simple,
            description: "Main production zone".to_string(),
            nodes: vec!["node1".to_string()],
        };

        assert!(sdn.create_zone(zone).is_ok());
        assert_eq!(sdn.list_zones().len(), 1);
    }

    #[test]
    fn test_create_vnet() {
        let mut sdn = SdnManager::new();

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
    }

    #[test]
    fn test_invalid_vlan_tag() {
        let mut sdn = SdnManager::new();

        let zone = Zone {
            id: "zone1".to_string(),
            name: "Test".to_string(),
            zone_type: ZoneType::Simple,
            description: "".to_string(),
            nodes: vec![],
        };
        sdn.create_zone(zone).unwrap();

        let vnet = VNet {
            id: "vnet1".to_string(),
            zone_id: "zone1".to_string(),
            name: "Invalid".to_string(),
            tag: 5000, // Invalid VLAN tag
            vnet_type: VNetType::Vlan,
            subnets: vec![],
            bridge: "vmbr0".to_string(),
        };

        assert!(sdn.create_vnet(vnet).is_err());
    }

    #[test]
    fn test_ip_allocation() {
        let mut sdn = SdnManager::new();

        // Setup zone, vnet, and subnet
        let zone = Zone {
            id: "zone1".to_string(),
            name: "Test".to_string(),
            zone_type: ZoneType::Simple,
            description: "".to_string(),
            nodes: vec![],
        };
        sdn.create_zone(zone).unwrap();

        let vnet = VNet {
            id: "vnet1".to_string(),
            zone_id: "zone1".to_string(),
            name: "Test Network".to_string(),
            tag: 100,
            vnet_type: VNetType::Vlan,
            subnets: vec![],
            bridge: "vmbr0".to_string(),
        };
        sdn.create_vnet(vnet).unwrap();

        let subnet = Subnet {
            id: "subnet1".to_string(),
            vnet_id: "vnet1".to_string(),
            cidr: "10.0.1.0/24".to_string(),
            gateway: Some("10.0.1.1".parse().unwrap()),
            dns_servers: vec![],
            dhcp_range: None,
        };
        sdn.create_subnet(subnet).unwrap();

        // Allocate IP
        let allocation = sdn.allocate_ip("subnet1", Some("vm-100".to_string()), None);
        assert!(allocation.is_ok());

        let alloc = allocation.unwrap();
        assert_eq!(alloc.subnet_id, "subnet1");
        assert_eq!(alloc.assigned_to, Some("vm-100".to_string()));

        // Try to allocate the same IP again
        let result = sdn.allocate_ip("subnet1", None, Some(alloc.ip));
        assert!(result.is_err());

        // Release IP
        assert!(sdn.release_ip(&alloc.ip).is_ok());

        // Should be able to allocate again
        let result = sdn.allocate_ip("subnet1", None, Some(alloc.ip));
        assert!(result.is_ok());
    }
}
