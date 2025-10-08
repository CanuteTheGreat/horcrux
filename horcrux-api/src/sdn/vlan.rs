//! VLAN (Virtual LAN) Implementation
//!
//! Provides VLAN tagging and bridge management for network isolation.
//! Uses Linux bridge interfaces with VLAN filtering.

use std::process::Command;

/// VLAN configuration
#[derive(Debug, Clone)]
pub struct VlanConfig {
    pub tag: u16,          // VLAN tag (1-4094)
    pub parent_iface: String,  // Parent interface (e.g., "eth0")
    pub bridge: String,    // Bridge name (e.g., "vmbr0")
}

pub struct VlanManager;

impl VlanManager {
    /// Create a VLAN interface
    pub fn create_vlan(config: &VlanConfig) -> Result<(), String> {
        // Validate VLAN tag
        if config.tag < 1 || config.tag > 4094 {
            return Err(format!("Invalid VLAN tag: {} (must be 1-4094)", config.tag));
        }

        let vlan_iface = format!("{}.{}", config.parent_iface, config.tag);

        // Create VLAN interface
        // ip link add link eth0 name eth0.100 type vlan id 100
        let output = Command::new("ip")
            .args(&[
                "link", "add",
                "link", &config.parent_iface,
                "name", &vlan_iface,
                "type", "vlan",
                "id", &config.tag.to_string(),
            ])
            .output()
            .map_err(|e| format!("Failed to execute ip command: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to create VLAN interface: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        // Bring interface up
        let output = Command::new("ip")
            .args(&["link", "set", "dev", &vlan_iface, "up"])
            .output()
            .map_err(|e| format!("Failed to bring up VLAN interface: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to bring up VLAN interface: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        // Add VLAN interface to bridge
        Self::add_to_bridge(&vlan_iface, &config.bridge)?;

        Ok(())
    }

    /// Delete a VLAN interface
    pub fn delete_vlan(parent_iface: &str, tag: u16) -> Result<(), String> {
        let vlan_iface = format!("{}.{}", parent_iface, tag);

        let output = Command::new("ip")
            .args(&["link", "delete", &vlan_iface])
            .output()
            .map_err(|e| format!("Failed to delete VLAN interface: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Ignore "Cannot find device" errors
            if !stderr.contains("Cannot find device") {
                return Err(format!("Failed to delete VLAN interface: {}", stderr));
            }
        }

        Ok(())
    }

    /// Create a Linux bridge
    pub fn create_bridge(bridge_name: &str) -> Result<(), String> {
        // Create bridge
        let output = Command::new("ip")
            .args(&["link", "add", "name", bridge_name, "type", "bridge"])
            .output()
            .map_err(|e| format!("Failed to create bridge: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Ignore "File exists" errors (bridge already exists)
            if !stderr.contains("File exists") {
                return Err(format!("Failed to create bridge: {}", stderr));
            }
        }

        // Enable VLAN filtering on bridge
        let output = Command::new("ip")
            .args(&[
                "link", "set", "dev", bridge_name,
                "type", "bridge", "vlan_filtering", "1"
            ])
            .output()
            .map_err(|e| format!("Failed to enable VLAN filtering: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to enable VLAN filtering: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        // Bring bridge up
        let output = Command::new("ip")
            .args(&["link", "set", "dev", bridge_name, "up"])
            .output()
            .map_err(|e| format!("Failed to bring up bridge: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to bring up bridge: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(())
    }

    /// Delete a Linux bridge
    pub fn delete_bridge(bridge_name: &str) -> Result<(), String> {
        // Bring bridge down first
        let _ = Command::new("ip")
            .args(&["link", "set", "dev", bridge_name, "down"])
            .output();

        let output = Command::new("ip")
            .args(&["link", "delete", bridge_name])
            .output()
            .map_err(|e| format!("Failed to delete bridge: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("Cannot find device") {
                return Err(format!("Failed to delete bridge: {}", stderr));
            }
        }

        Ok(())
    }

    /// Add interface to bridge
    pub fn add_to_bridge(iface: &str, bridge: &str) -> Result<(), String> {
        let output = Command::new("ip")
            .args(&["link", "set", "dev", iface, "master", bridge])
            .output()
            .map_err(|e| format!("Failed to add interface to bridge: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to add interface to bridge: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(())
    }

    /// Remove interface from bridge
    pub fn remove_from_bridge(iface: &str) -> Result<(), String> {
        let output = Command::new("ip")
            .args(&["link", "set", "dev", iface, "nomaster"])
            .output()
            .map_err(|e| format!("Failed to remove interface from bridge: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to remove interface from bridge: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(())
    }

    /// List all VLAN interfaces
    pub fn list_vlans() -> Result<Vec<String>, String> {
        let output = Command::new("ip")
            .args(&["-d", "link", "show", "type", "vlan"])
            .output()
            .map_err(|e| format!("Failed to list VLANs: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to list VLANs: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let vlans: Vec<String> = stdout
            .lines()
            .filter(|line| line.contains("vlan"))
            .filter_map(|line| {
                // Parse interface name from output like: "5: eth0.100@eth0: <BROADCAST..."
                line.split(':').nth(1).map(|s| s.trim().split('@').next().unwrap().to_string())
            })
            .collect();

        Ok(vlans)
    }

    /// List all bridges
    pub fn list_bridges() -> Result<Vec<String>, String> {
        let output = Command::new("ip")
            .args(&["link", "show", "type", "bridge"])
            .output()
            .map_err(|e| format!("Failed to list bridges: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to list bridges: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let bridges: Vec<String> = stdout
            .lines()
            .filter_map(|line| {
                if line.contains("bridge") {
                    line.split(':').nth(1).map(|s| s.trim().split('@').next().unwrap().to_string())
                } else {
                    None
                }
            })
            .collect();

        Ok(bridges)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vlan_tag_validation() {
        let config = VlanConfig {
            tag: 0, // Invalid
            parent_iface: "eth0".to_string(),
            bridge: "vmbr0".to_string(),
        };

        assert!(VlanManager::create_vlan(&config).is_err());

        let config = VlanConfig {
            tag: 5000, // Invalid
            parent_iface: "eth0".to_string(),
            bridge: "vmbr0".to_string(),
        };

        assert!(VlanManager::create_vlan(&config).is_err());
    }
}
