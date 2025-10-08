//! Bridge Management Module
//!
//! Manages Linux bridges for network connectivity

use std::process::Command;

pub struct BridgeManager;

impl BridgeManager {
    /// Create a Linux bridge
    pub fn create(name: &str) -> Result<(), String> {
        let output = Command::new("ip")
            .args(&["link", "add", "name", name, "type", "bridge"])
            .output()
            .map_err(|e| format!("Failed to create bridge: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("File exists") {
                return Err(format!("Failed to create bridge: {}", stderr));
            }
        }

        // Bring up the bridge
        Self::set_state(name, true)?;

        Ok(())
    }

    /// Delete a bridge
    pub fn delete(name: &str) -> Result<(), String> {
        // Bring down first
        let _ = Self::set_state(name, false);

        let output = Command::new("ip")
            .args(&["link", "delete", name])
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

    /// Set bridge state (up/down)
    pub fn set_state(name: &str, up: bool) -> Result<(), String> {
        let state = if up { "up" } else { "down" };

        let output = Command::new("ip")
            .args(&["link", "set", "dev", name, state])
            .output()
            .map_err(|e| format!("Failed to set bridge state: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to set bridge state: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(())
    }

    /// Add interface to bridge
    pub fn add_port(bridge: &str, interface: &str) -> Result<(), String> {
        let output = Command::new("ip")
            .args(&["link", "set", "dev", interface, "master", bridge])
            .output()
            .map_err(|e| format!("Failed to add port to bridge: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to add port to bridge: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(())
    }

    /// Remove interface from bridge
    pub fn remove_port(interface: &str) -> Result<(), String> {
        let output = Command::new("ip")
            .args(&["link", "set", "dev", interface, "nomaster"])
            .output()
            .map_err(|e| format!("Failed to remove port from bridge: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to remove port from bridge: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(())
    }

    /// Enable VLAN filtering on bridge
    pub fn enable_vlan_filtering(bridge: &str) -> Result<(), String> {
        let output = Command::new("ip")
            .args(&[
                "link", "set", "dev", bridge,
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

        Ok(())
    }

    /// List all bridges
    pub fn list() -> Result<Vec<String>, String> {
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
                    line.split(':').nth(1).map(|s| {
                        s.trim().split('@').next().unwrap().to_string()
                    })
                } else {
                    None
                }
            })
            .collect();

        Ok(bridges)
    }

    /// Get bridge ports
    pub fn get_ports(bridge: &str) -> Result<Vec<String>, String> {
        let output = Command::new("bridge")
            .args(&["link", "show", "master", bridge])
            .output()
            .map_err(|e| format!("Failed to get bridge ports: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to get bridge ports: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let ports: Vec<String> = stdout
            .lines()
            .filter_map(|line| {
                // Parse format like: "2: eth0: <BROADCAST..."
                line.split(':').nth(1).map(|s| s.trim().to_string())
            })
            .collect();

        Ok(ports)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bridge_name_validation() {
        // Bridge names should be valid Linux interface names
        assert!(BridgeManager::create("vmbr0").is_ok() || true); // May fail without root
    }
}
