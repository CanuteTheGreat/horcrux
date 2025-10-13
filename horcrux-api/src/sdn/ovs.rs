///! Open vSwitch (OVS) Implementation
///!
///! Provides advanced software-defined networking with OpenFlow support.
///! OVS is a production-grade multilayer virtual switch.

use std::process::Command;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// OVS bridge configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OvsBridge {
    pub name: String,
    pub datapath_type: DatapathType,
    pub fail_mode: Option<FailMode>,
    pub protocols: Vec<String>,  // e.g., ["OpenFlow10", "OpenFlow13"]
    pub controller: Option<String>,  // Controller address (e.g., "tcp:127.0.0.1:6633")
}

/// OVS datapath type
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatapathType {
    System,    // Kernel datapath
    Netdev,    // Userspace datapath (DPDK)
}

/// Bridge fail mode
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FailMode {
    Standalone,  // Act as learning switch
    Secure,      // Drop all packets without controller
}

/// OVS port configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OvsPort {
    pub name: String,
    pub port_type: PortType,
    pub tag: Option<u16>,  // VLAN tag (1-4095)
    pub trunks: Vec<u16>,  // Trunk VLANs
    pub options: HashMap<String, String>,  // Type-specific options
}

/// OVS port type
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PortType {
    Internal,  // Internal port
    Patch,     // Patch port (connects bridges)
    Vxlan,     // VXLAN tunnel
    Gre,       // GRE tunnel
    Geneve,    // Geneve tunnel
    System,    // System device (e.g., eth0)
}

/// OVS flow rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OvsFlow {
    pub table: u8,
    pub priority: u16,
    pub matches: HashMap<String, String>,
    pub actions: Vec<String>,
}

pub struct OvsManager;

impl OvsManager {
    /// Check if OVS is installed and running
    pub fn check_ovs_available() -> bool {
        Command::new("ovs-vsctl")
            .arg("--version")
            .output()
            .is_ok()
    }

    /// Get OVS version
    pub fn get_ovs_version() -> Result<String, String> {
        let output = Command::new("ovs-vsctl")
            .arg("--version")
            .output()
            .map_err(|e| format!("Failed to get OVS version: {}", e))?;

        if !output.status.success() {
            return Err("OVS not found or not running".to_string());
        }

        let version = String::from_utf8_lossy(&output.stdout);
        Ok(version.lines().next().unwrap_or("Unknown").to_string())
    }

    /// Create an OVS bridge
    pub fn create_bridge(config: &OvsBridge) -> Result<(), String> {
        // Create bridge
        let mut args = vec!["add-br", &config.name];

        // Add datapath type
        if matches!(config.datapath_type, DatapathType::Netdev) {
            args.extend(&["--", "set", "bridge", &config.name, "datapath_type=netdev"]);
        }

        let output = Command::new("ovs-vsctl")
            .args(&args)
            .output()
            .map_err(|e| format!("Failed to create OVS bridge: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("already exists") {
                return Err(format!("Failed to create OVS bridge: {}", stderr));
            }
        }

        // Set fail mode
        if let Some(fail_mode) = &config.fail_mode {
            let mode_str = match fail_mode {
                FailMode::Standalone => "standalone",
                FailMode::Secure => "secure",
            };

            let output = Command::new("ovs-vsctl")
                .args(&["set-fail-mode", &config.name, mode_str])
                .output()
                .map_err(|e| format!("Failed to set fail mode: {}", e))?;

            if !output.status.success() {
                return Err(format!(
                    "Failed to set fail mode: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }

        // Set protocols
        if !config.protocols.is_empty() {
            let protocols = config.protocols.join(",");
            let protocols_arg = format!("protocols={}", protocols);

            let output = Command::new("ovs-vsctl")
                .args(&["set", "bridge", &config.name, &protocols_arg])
                .output()
                .map_err(|e| format!("Failed to set protocols: {}", e))?;

            if !output.status.success() {
                return Err(format!(
                    "Failed to set protocols: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }

        // Set controller
        if let Some(controller) = &config.controller {
            let output = Command::new("ovs-vsctl")
                .args(&["set-controller", &config.name, controller])
                .output()
                .map_err(|e| format!("Failed to set controller: {}", e))?;

            if !output.status.success() {
                return Err(format!(
                    "Failed to set controller: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }

        Ok(())
    }

    /// Delete an OVS bridge
    pub fn delete_bridge(bridge_name: &str) -> Result<(), String> {
        let output = Command::new("ovs-vsctl")
            .args(&["del-br", bridge_name])
            .output()
            .map_err(|e| format!("Failed to delete OVS bridge: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("no bridge named") {
                return Err(format!("Failed to delete OVS bridge: {}", stderr));
            }
        }

        Ok(())
    }

    /// Add a port to an OVS bridge
    pub fn add_port(bridge_name: &str, port: &OvsPort) -> Result<(), String> {
        let mut cmd = Command::new("ovs-vsctl");
        cmd.args(&["add-port", bridge_name, &port.name]);

        // Set port type
        match &port.port_type {
            PortType::Internal => {
                cmd.args(&["--", "set", "interface", &port.name, "type=internal"]);
            }
            PortType::Patch => {
                if let Some(peer) = port.options.get("peer") {
                    cmd.args(&[
                        "--",
                        "set",
                        "interface",
                        &port.name,
                        "type=patch",
                        &format!("options:peer={}", peer),
                    ]);
                } else {
                    return Err("Patch port requires 'peer' option".to_string());
                }
            }
            PortType::Vxlan => {
                if let Some(remote_ip) = port.options.get("remote_ip") {
                    let remote_ip_opt = format!("options:remote_ip={}", remote_ip);
                    let mut args: Vec<&str> = vec![
                        "--",
                        "set",
                        "interface",
                        &port.name,
                        "type=vxlan",
                        &remote_ip_opt,
                    ];

                    // Add optional key (VNI)
                    let key_str;
                    if let Some(key) = port.options.get("key") {
                        key_str = format!("options:key={}", key);
                        args.push(&key_str);
                    }

                    cmd.args(&args);
                } else {
                    return Err("VXLAN port requires 'remote_ip' option".to_string());
                }
            }
            PortType::Gre => {
                if let Some(remote_ip) = port.options.get("remote_ip") {
                    cmd.args(&[
                        "--",
                        "set",
                        "interface",
                        &port.name,
                        "type=gre",
                        &format!("options:remote_ip={}", remote_ip),
                    ]);
                } else {
                    return Err("GRE port requires 'remote_ip' option".to_string());
                }
            }
            PortType::Geneve => {
                if let Some(remote_ip) = port.options.get("remote_ip") {
                    cmd.args(&[
                        "--",
                        "set",
                        "interface",
                        &port.name,
                        "type=geneve",
                        &format!("options:remote_ip={}", remote_ip),
                    ]);
                } else {
                    return Err("Geneve port requires 'remote_ip' option".to_string());
                }
            }
            PortType::System => {
                // System ports don't need type specification
            }
        }

        let output = cmd
            .output()
            .map_err(|e| format!("Failed to add port: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to add port: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        // Set VLAN tag if specified
        if let Some(tag) = port.tag {
            Self::set_port_vlan(bridge_name, &port.name, tag)?;
        }

        // Set trunk VLANs if specified
        if !port.trunks.is_empty() {
            Self::set_port_trunks(bridge_name, &port.name, &port.trunks)?;
        }

        Ok(())
    }

    /// Remove a port from an OVS bridge
    pub fn delete_port(bridge_name: &str, port_name: &str) -> Result<(), String> {
        let output = Command::new("ovs-vsctl")
            .args(&["del-port", bridge_name, port_name])
            .output()
            .map_err(|e| format!("Failed to delete port: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("no port named") {
                return Err(format!("Failed to delete port: {}", stderr));
            }
        }

        Ok(())
    }

    /// Set VLAN tag on a port
    pub fn set_port_vlan(_bridge_name: &str, port_name: &str, vlan: u16) -> Result<(), String> {
        if vlan == 0 || vlan > 4095 {
            return Err(format!("Invalid VLAN tag: {} (must be 1-4095)", vlan));
        }

        let output = Command::new("ovs-vsctl")
            .args(&[
                "set",
                "port",
                port_name,
                &format!("tag={}", vlan),
            ])
            .output()
            .map_err(|e| format!("Failed to set VLAN tag: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to set VLAN tag: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(())
    }

    /// Set trunk VLANs on a port
    pub fn set_port_trunks(_bridge_name: &str, port_name: &str, trunks: &[u16]) -> Result<(), String> {
        let trunks_str: Vec<String> = trunks.iter().map(|v| v.to_string()).collect();
        let trunks_arg = format!("trunks=[{}]", trunks_str.join(","));

        let output = Command::new("ovs-vsctl")
            .args(&["set", "port", port_name, &trunks_arg])
            .output()
            .map_err(|e| format!("Failed to set trunk VLANs: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to set trunk VLANs: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(())
    }

    /// List all OVS bridges
    pub fn list_bridges() -> Result<Vec<String>, String> {
        let output = Command::new("ovs-vsctl")
            .arg("list-br")
            .output()
            .map_err(|e| format!("Failed to list bridges: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to list bridges: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let bridges: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        Ok(bridges)
    }

    /// List ports on a bridge
    pub fn list_ports(bridge_name: &str) -> Result<Vec<String>, String> {
        let output = Command::new("ovs-vsctl")
            .args(&["list-ports", bridge_name])
            .output()
            .map_err(|e| format!("Failed to list ports: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to list ports: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let ports: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        Ok(ports)
    }

    /// Add an OpenFlow rule
    pub fn add_flow(bridge_name: &str, flow: &OvsFlow) -> Result<(), String> {
        let mut flow_str = format!("table={},priority={}", flow.table, flow.priority);

        // Add matches
        for (key, value) in &flow.matches {
            flow_str.push_str(&format!(",{}={}", key, value));
        }

        // Add actions
        if !flow.actions.is_empty() {
            flow_str.push_str(&format!(",actions={}", flow.actions.join(",")));
        }

        let output = Command::new("ovs-ofctl")
            .args(&["add-flow", bridge_name, &flow_str])
            .output()
            .map_err(|e| format!("Failed to add flow: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to add flow: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(())
    }

    /// Delete all flows from a bridge
    pub fn delete_flows(bridge_name: &str) -> Result<(), String> {
        let output = Command::new("ovs-ofctl")
            .args(&["del-flows", bridge_name])
            .output()
            .map_err(|e| format!("Failed to delete flows: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to delete flows: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(())
    }

    /// Show flows on a bridge
    pub fn show_flows(bridge_name: &str) -> Result<String, String> {
        let output = Command::new("ovs-ofctl")
            .args(&["dump-flows", bridge_name])
            .output()
            .map_err(|e| format!("Failed to show flows: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to show flows: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Get bridge information
    pub fn get_bridge_info(bridge_name: &str) -> Result<BridgeInfo, String> {
        let output = Command::new("ovs-vsctl")
            .args(&["show", bridge_name])
            .output()
            .map_err(|e| format!("Failed to get bridge info: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to get bridge info: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let info_str = String::from_utf8_lossy(&output.stdout);
        let ports = Self::list_ports(bridge_name)?;

        Ok(BridgeInfo {
            name: bridge_name.to_string(),
            ports,
            info: info_str.to_string(),
        })
    }
}

/// Bridge information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeInfo {
    pub name: String,
    pub ports: Vec<String>,
    pub info: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vlan_validation() {
        assert!(OvsManager::set_port_vlan("br0", "port1", 0).is_err());
        assert!(OvsManager::set_port_vlan("br0", "port1", 4096).is_err());
    }

    #[test]
    fn test_datapath_type_serialization() {
        let dt = DatapathType::System;
        let json = serde_json::to_string(&dt).unwrap();
        assert_eq!(json, "\"system\"");
    }

    #[test]
    fn test_fail_mode_serialization() {
        let fm = FailMode::Standalone;
        let json = serde_json::to_string(&fm).unwrap();
        assert_eq!(json, "\"standalone\"");
    }
}
