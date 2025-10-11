//! IPAM (IP Address Management) Module
//!
//! Manages IP address allocation, tracking, and DNS integration.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use serde::{Deserialize, Serialize};

/// IPAM database for tracking IP allocations
pub struct IpamManager {
    allocations: HashMap<String, SubnetAllocations>, // subnet_id -> allocations
}

/// Allocations within a subnet
struct SubnetAllocations {
    subnet_cidr: String,
    used_ips: HashMap<IpAddr, AllocationInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllocationInfo {
    pub ip: IpAddr,
    pub mac_address: Option<String>,
    pub hostname: Option<String>,
    pub assigned_to: Option<String>, // VM/container ID
    pub allocated_at: i64,
    pub allocation_type: AllocationType,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AllocationType {
    Static,   // Manually assigned
    Dynamic,  // DHCP or auto-assigned
    Reserved, // Reserved for special use
}

impl IpamManager {
    pub fn new() -> Self {
        IpamManager {
            allocations: HashMap::new(),
        }
    }

    /// Initialize a subnet for IPAM tracking
    pub fn init_subnet(&mut self, subnet_id: String, cidr: String) {
        self.allocations.insert(
            subnet_id,
            SubnetAllocations {
                subnet_cidr: cidr,
                used_ips: HashMap::new(),
            },
        );
    }

    /// Allocate a specific IP address
    pub fn allocate_specific(
        &mut self,
        subnet_id: &str,
        ip: IpAddr,
        info: AllocationInfo,
    ) -> Result<(), String> {
        let subnet = self.allocations.get_mut(subnet_id)
            .ok_or_else(|| format!("Subnet {} not initialized", subnet_id))?;

        // Check if IP already allocated
        if subnet.used_ips.contains_key(&ip) {
            return Err(format!("IP {} already allocated", ip));
        }

        // Validate IP is in subnet
        if !Self::ip_in_cidr(&ip, &subnet.subnet_cidr)? {
            return Err(format!("IP {} not in subnet {}", ip, subnet.subnet_cidr));
        }

        subnet.used_ips.insert(ip, info);
        Ok(())
    }

    /// Allocate next available IP in subnet
    pub fn allocate_next(
        &mut self,
        subnet_id: &str,
        mut info: AllocationInfo,
    ) -> Result<IpAddr, String> {
        let subnet = self.allocations.get(subnet_id)
            .ok_or_else(|| format!("Subnet {} not initialized", subnet_id))?;

        let (network, prefix_len) = Self::parse_cidr(&subnet.subnet_cidr)?;

        // Find next available IP
        let next_ip = self.find_next_available(subnet_id, &network, prefix_len)?;

        info.ip = next_ip;
        self.allocate_specific(subnet_id, next_ip, info)?;

        Ok(next_ip)
    }

    /// Release an IP address
    pub fn release(&mut self, subnet_id: &str, ip: &IpAddr) -> Result<(), String> {
        let subnet = self.allocations.get_mut(subnet_id)
            .ok_or_else(|| format!("Subnet {} not found", subnet_id))?;

        if subnet.used_ips.remove(ip).is_none() {
            return Err(format!("IP {} not allocated", ip));
        }

        Ok(())
    }

    /// Get allocation info for an IP
    pub fn get_allocation(&self, subnet_id: &str, ip: &IpAddr) -> Option<&AllocationInfo> {
        self.allocations.get(subnet_id)
            .and_then(|subnet| subnet.used_ips.get(ip))
    }

    /// List all allocations in a subnet
    pub fn list_allocations(&self, subnet_id: &str) -> Vec<AllocationInfo> {
        self.allocations.get(subnet_id)
            .map(|subnet| subnet.used_ips.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Get subnet utilization stats
    pub fn get_utilization(&self, subnet_id: &str) -> Result<SubnetUtilization, String> {
        let subnet = self.allocations.get(subnet_id)
            .ok_or_else(|| format!("Subnet {} not found", subnet_id))?;

        let (_, prefix_len) = Self::parse_cidr(&subnet.subnet_cidr)?;
        let total_ips = if prefix_len >= 31 {
            // /31 and /32 special cases
            1u64 << (32 - prefix_len)
        } else {
            // Subtract network and broadcast addresses
            (1u64 << (32 - prefix_len)) - 2
        };

        let used_ips = subnet.used_ips.len() as u64;
        let available_ips = total_ips.saturating_sub(used_ips);
        let utilization_percent = (used_ips as f64 / total_ips as f64) * 100.0;

        Ok(SubnetUtilization {
            subnet_id: subnet_id.to_string(),
            cidr: subnet.subnet_cidr.clone(),
            total_ips,
            used_ips,
            available_ips,
            utilization_percent,
        })
    }

    /// Find IPs by hostname
    pub fn find_by_hostname(&self, hostname: &str) -> Vec<AllocationInfo> {
        self.allocations.values()
            .flat_map(|subnet| {
                subnet.used_ips.values()
                    .filter(|alloc| {
                        alloc.hostname.as_ref().map_or(false, |h| h == hostname)
                    })
                    .cloned()
            })
            .collect()
    }

    /// Find IPs by MAC address
    pub fn find_by_mac(&self, mac: &str) -> Vec<AllocationInfo> {
        self.allocations.values()
            .flat_map(|subnet| {
                subnet.used_ips.values()
                    .filter(|alloc| {
                        alloc.mac_address.as_ref().map_or(false, |m| m == mac)
                    })
                    .cloned()
            })
            .collect()
    }

    // Helper functions

    fn find_next_available(
        &self,
        subnet_id: &str,
        network: &Ipv4Addr,
        prefix_len: u8,
    ) -> Result<IpAddr, String> {
        let subnet = self.allocations.get(subnet_id)
            .ok_or_else(|| format!("Subnet {} not found", subnet_id))?;

        let network_u32 = u32::from(*network);
        let host_bits = 32 - prefix_len;
        let max_hosts = if prefix_len >= 31 {
            1u32 << host_bits
        } else {
            (1u32 << host_bits) - 2
        };

        let start = if prefix_len >= 31 { 0 } else { 1 };

        for i in start..=max_hosts {
            let ip_u32 = network_u32.wrapping_add(i);
            let ip = IpAddr::V4(Ipv4Addr::from(ip_u32));

            if !subnet.used_ips.contains_key(&ip) {
                return Ok(ip);
            }
        }

        Err("No available IPs in subnet".to_string())
    }

    fn parse_cidr(cidr: &str) -> Result<(Ipv4Addr, u8), String> {
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

    fn ip_in_cidr(ip: &IpAddr, cidr: &str) -> Result<bool, String> {
        let (network, prefix_len) = Self::parse_cidr(cidr)?;

        if let IpAddr::V4(ipv4) = ip {
            let ip_u32 = u32::from(*ipv4);
            let network_u32 = u32::from(network);
            let mask = if prefix_len == 0 { 0 } else { !0u32 << (32 - prefix_len) };

            Ok((ip_u32 & mask) == (network_u32 & mask))
        } else {
            Err("IPv6 not yet supported".to_string())
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubnetUtilization {
    pub subnet_id: String,
    pub cidr: String,
    pub total_ips: u64,
    pub used_ips: u64,
    pub available_ips: u64,
    pub utilization_percent: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipam_allocation() {
        let mut ipam = IpamManager::new();
        ipam.init_subnet("subnet1".to_string(), "10.0.1.0/24".to_string());

        let info = AllocationInfo {
            ip: "10.0.1.10".parse().unwrap(),
            mac_address: Some("00:11:22:33:44:55".to_string()),
            hostname: Some("test-host".to_string()),
            assigned_to: Some("vm-100".to_string()),
            allocated_at: 1234567890,
            allocation_type: AllocationType::Static,
        };

        // Allocate specific IP
        assert!(ipam.allocate_specific("subnet1", "10.0.1.10".parse().unwrap(), info.clone()).is_ok());

        // Try to allocate same IP again
        assert!(ipam.allocate_specific("subnet1", "10.0.1.10".parse().unwrap(), info).is_err());

        // Release IP
        assert!(ipam.release("subnet1", &"10.0.1.10".parse().unwrap()).is_ok());
    }

    #[test]
    fn test_next_available() {
        let mut ipam = IpamManager::new();
        ipam.init_subnet("subnet1".to_string(), "192.168.1.0/24".to_string());

        let info = AllocationInfo {
            ip: "0.0.0.0".parse().unwrap(), // Will be set by allocate_next
            mac_address: None,
            hostname: None,
            assigned_to: None,
            allocated_at: 0,
            allocation_type: AllocationType::Dynamic,
        };

        // Allocate first available (should be .1)
        let ip1 = ipam.allocate_next("subnet1", info.clone()).unwrap();
        assert_eq!(ip1, "192.168.1.1".parse::<IpAddr>().unwrap());

        // Allocate next (should be .2)
        let ip2 = ipam.allocate_next("subnet1", info).unwrap();
        assert_eq!(ip2, "192.168.1.2".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn test_utilization() {
        let mut ipam = IpamManager::new();
        ipam.init_subnet("subnet1".to_string(), "10.0.0.0/30".to_string());

        // /30 has 2 usable IPs (.1 and .2)
        let util = ipam.get_utilization("subnet1").unwrap();
        assert_eq!(util.total_ips, 2);
        assert_eq!(util.used_ips, 0);
        assert_eq!(util.available_ips, 2);
    }
}
