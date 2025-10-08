//! VXLAN (Virtual Extensible LAN) Implementation
//!
//! Provides overlay networking across physical network boundaries.
//! VXLAN encapsulates Layer 2 frames in UDP packets for tunneling.

use std::process::Command;
use std::net::IpAddr;

/// VXLAN configuration
#[derive(Debug, Clone)]
pub struct VxlanConfig {
    pub vni: u32,          // VXLAN Network Identifier (0-16777215)
    pub local_ip: IpAddr,  // Local tunnel endpoint
    pub group: Option<IpAddr>,  // Multicast group (optional)
    pub remote_ips: Vec<IpAddr>, // Remote tunnel endpoints (for unicast)
    pub port: u16,         // UDP port (default: 4789)
    pub bridge: String,    // Bridge to attach to
}

pub struct VxlanManager;

impl VxlanManager {
    /// Create a VXLAN interface
    pub fn create_vxlan(config: &VxlanConfig) -> Result<(), String> {
        // Validate VNI
        if config.vni > 16777215 {
            return Err(format!("Invalid VXLAN VNI: {} (must be 0-16777215)", config.vni));
        }

        let vxlan_iface = format!("vxlan{}", config.vni);

        // Build ip command based on unicast or multicast mode
        let mut args = vec![
            "link", "add",
            &vxlan_iface,
            "type", "vxlan",
            "id", &config.vni.to_string(),
            "dstport", &config.port.to_string(),
        ];

        let local_ip_str = config.local_ip.to_string();
        args.extend(&["local", &local_ip_str]);

        // Multicast mode
        let group_str;
        if let Some(group) = &config.group {
            group_str = group.to_string();
            args.extend(&["group", &group_str]);
        }

        // Add dev parameter (required for multicast)
        args.extend(&["dev", "eth0"]); // TODO: Make this configurable

        // Create VXLAN interface
        let output = Command::new("ip")
            .args(&args)
            .output()
            .map_err(|e| format!("Failed to execute ip command: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to create VXLAN interface: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        // Add remote tunnel endpoints (for unicast VXLAN)
        for remote_ip in &config.remote_ips {
            Self::add_remote_endpoint(&vxlan_iface, remote_ip)?;
        }

        // Bring interface up
        let output = Command::new("ip")
            .args(&["link", "set", "dev", &vxlan_iface, "up"])
            .output()
            .map_err(|e| format!("Failed to bring up VXLAN interface: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to bring up VXLAN interface: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        // Add to bridge
        let output = Command::new("ip")
            .args(&["link", "set", "dev", &vxlan_iface, "master", &config.bridge])
            .output()
            .map_err(|e| format!("Failed to add VXLAN to bridge: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to add VXLAN to bridge: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(())
    }

    /// Delete a VXLAN interface
    pub fn delete_vxlan(vni: u32) -> Result<(), String> {
        let vxlan_iface = format!("vxlan{}", vni);

        let output = Command::new("ip")
            .args(&["link", "delete", &vxlan_iface])
            .output()
            .map_err(|e| format!("Failed to delete VXLAN interface: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("Cannot find device") {
                return Err(format!("Failed to delete VXLAN interface: {}", stderr));
            }
        }

        Ok(())
    }

    /// Add a remote tunnel endpoint (for unicast VXLAN)
    pub fn add_remote_endpoint(vxlan_iface: &str, remote_ip: &IpAddr) -> Result<(), String> {
        let remote_str = remote_ip.to_string();

        // bridge fdb append 00:00:00:00:00:00 dev vxlan100 dst <remote_ip>
        let output = Command::new("bridge")
            .args(&[
                "fdb", "append",
                "00:00:00:00:00:00",
                "dev", vxlan_iface,
                "dst", &remote_str,
            ])
            .output()
            .map_err(|e| format!("Failed to add remote endpoint: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to add remote endpoint: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(())
    }

    /// Remove a remote tunnel endpoint
    pub fn remove_remote_endpoint(vxlan_iface: &str, remote_ip: &IpAddr) -> Result<(), String> {
        let remote_str = remote_ip.to_string();

        let output = Command::new("bridge")
            .args(&[
                "fdb", "delete",
                "00:00:00:00:00:00",
                "dev", vxlan_iface,
                "dst", &remote_str,
            ])
            .output()
            .map_err(|e| format!("Failed to remove remote endpoint: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to remove remote endpoint: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(())
    }

    /// List all VXLAN interfaces
    pub fn list_vxlans() -> Result<Vec<String>, String> {
        let output = Command::new("ip")
            .args(&["-d", "link", "show", "type", "vxlan"])
            .output()
            .map_err(|e| format!("Failed to list VXLANs: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to list VXLANs: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let vxlans: Vec<String> = stdout
            .lines()
            .filter(|line| line.contains("vxlan"))
            .filter_map(|line| {
                line.split(':').nth(1).map(|s| s.trim().split('@').next().unwrap().to_string())
            })
            .collect();

        Ok(vxlans)
    }

    /// Get VXLAN interface details
    pub fn get_vxlan_info(vni: u32) -> Result<VxlanInfo, String> {
        let vxlan_iface = format!("vxlan{}", vni);

        let output = Command::new("ip")
            .args(&["-d", "-j", "link", "show", &vxlan_iface])
            .output()
            .map_err(|e| format!("Failed to get VXLAN info: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "VXLAN interface not found: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        // Parse JSON output (simplified - in production, use serde_json)
        let stdout = String::from_utf8_lossy(&output.stdout);

        Ok(VxlanInfo {
            vni,
            interface: vxlan_iface,
            state: if stdout.contains("UP") { "up".to_string() } else { "down".to_string() },
        })
    }
}

/// VXLAN interface information
#[derive(Debug, Clone)]
pub struct VxlanInfo {
    pub vni: u32,
    pub interface: String,
    pub state: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vni_validation() {
        let config = VxlanConfig {
            vni: 16777216, // Too large
            local_ip: "192.168.1.1".parse().unwrap(),
            group: None,
            remote_ips: vec![],
            port: 4789,
            bridge: "vmbr0".to_string(),
        };

        assert!(VxlanManager::create_vxlan(&config).is_err());
    }

    #[test]
    fn test_vxlan_interface_name() {
        let vni = 100;
        let expected = "vxlan100";
        assert_eq!(format!("vxlan{}", vni), expected);
    }
}
