///! CNI (Container Network Interface) implementation
///! Provides Kubernetes-style networking for containers
///! Implements CNI spec version 1.0.0

use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// CNI plugin configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CniConfig {
    pub cni_version: String,
    pub name: String,
    pub plugin_type: CniPluginType,
    pub bridge: Option<String>,
    pub ipam: IpamConfig,
    pub dns: Option<DnsConfig>,
    pub capabilities: HashMap<String, bool>,
}

/// CNI plugin types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CniPluginType {
    Bridge,
    Macvlan,
    Ipvlan,
    Vlan,
    Vxlan,
    Ptp,      // Point-to-point
    Host,     // Host networking
    Loopback,
}

/// IPAM (IP Address Management) configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpamConfig {
    pub ipam_type: String, // "host-local", "dhcp", "static"
    pub subnet: Option<String>,
    pub range_start: Option<IpAddr>,
    pub range_end: Option<IpAddr>,
    pub gateway: Option<IpAddr>,
    pub routes: Vec<RouteConfig>,
}

/// DNS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsConfig {
    pub nameservers: Vec<IpAddr>,
    pub domain: Option<String>,
    pub search: Vec<String>,
    pub options: Vec<String>,
}

/// Route configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteConfig {
    pub dst: String,
    pub gw: Option<IpAddr>,
}

/// CNI network attachment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CniAttachment {
    pub container_id: String,
    pub network_name: String,
    pub interface_name: String,
    pub ip_address: IpAddr,
    pub mac_address: String,
    pub gateway: Option<IpAddr>,
}

/// CNI result (returned from ADD operation)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CniResult {
    pub cni_version: String,
    pub interfaces: Vec<CniInterface>,
    pub ips: Vec<CniIpConfig>,
    pub routes: Vec<RouteConfig>,
    pub dns: Option<DnsConfig>,
}

/// CNI interface info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CniInterface {
    pub name: String,
    pub mac: String,
    pub sandbox: Option<String>,
}

/// CNI IP configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CniIpConfig {
    pub address: String, // CIDR notation
    pub gateway: Option<IpAddr>,
    pub interface: Option<u32>,
}

/// CNI Manager
pub struct CniManager {
    cni_bin_dir: PathBuf,
    cni_conf_dir: PathBuf,
    networks: HashMap<String, CniConfig>,
    attachments: HashMap<String, Vec<CniAttachment>>,
}

impl CniManager {
    pub fn new(cni_bin_dir: PathBuf, cni_conf_dir: PathBuf) -> Self {
        Self {
            cni_bin_dir,
            cni_conf_dir,
            networks: HashMap::new(),
            attachments: HashMap::new(),
        }
    }

    /// Create a new CNI network
    pub async fn create_network(&mut self, config: CniConfig) -> Result<()> {
        // Write network configuration file
        let conf_file = self.cni_conf_dir.join(format!("{}.conflist", config.name));

        let conf_list = serde_json::json!({
            "cniVersion": config.cni_version,
            "name": config.name,
            "plugins": [
                {
                    "type": format!("{:?}", config.plugin_type).to_lowercase(),
                    "bridge": config.bridge,
                    "ipam": config.ipam,
                }
            ]
        });

        tokio::fs::write(&conf_file, serde_json::to_string_pretty(&conf_list)
            .map_err(|e| horcrux_common::Error::System(format!("Failed to serialize CNI config: {}", e)))?)
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to write CNI config: {}", e)))?;

        self.networks.insert(config.name.clone(), config);
        tracing::info!("Created CNI network: {}", conf_file.display());

        Ok(())
    }

    /// Add container to network (CNI ADD operation)
    pub async fn add_container(
        &mut self,
        container_id: &str,
        network_name: &str,
        interface_name: &str,
        netns_path: &str,
    ) -> Result<CniResult> {
        let network = self.networks.get(network_name)
            .ok_or_else(|| horcrux_common::Error::System(format!("Network {} not found", network_name)))?;

        // Prepare CNI environment variables
        let env_vars = vec![
            ("CNI_COMMAND", "ADD"),
            ("CNI_CONTAINERID", container_id),
            ("CNI_NETNS", netns_path),
            ("CNI_IFNAME", interface_name),
            ("CNI_PATH", self.cni_bin_dir.to_str().unwrap()),
        ];

        // Call CNI plugin
        let plugin_path = self.cni_bin_dir.join(format!("{:?}", network.plugin_type).to_lowercase());
        let config_json = serde_json::to_string(&network)
            .map_err(|e| horcrux_common::Error::System(format!("Failed to serialize CNI config: {}", e)))?;

        let mut cmd = Command::new(&plugin_path);
        for (key, value) in env_vars {
            cmd.env(key, value);
        }

        let mut child = cmd
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| horcrux_common::Error::System(format!("Failed to spawn CNI plugin: {}", e)))?;

        // Write config to stdin and close it
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(config_json.as_bytes()).await?;
            drop(stdin); // Close stdin
        }

        let result = child.wait_with_output().await?;

        if !result.status.success() {
            let stderr = String::from_utf8_lossy(&result.stderr);
            return Err(horcrux_common::Error::System(format!("CNI plugin failed: {}", stderr)));
        }

        // Parse CNI result
        let stdout = String::from_utf8_lossy(&result.stdout);
        let cni_result: CniResult = serde_json::from_str(&stdout)
            .map_err(|e| horcrux_common::Error::System(format!("Failed to parse CNI result: {}", e)))?;

        // Store attachment
        let ip_address = cni_result.ips.first()
            .and_then(|ip| ip.address.split('/').next())
            .and_then(|ip_str| ip_str.parse().ok())
            .ok_or_else(|| horcrux_common::Error::System("No IP address in CNI result".to_string()))?;

        let mac_address = cni_result.interfaces.first()
            .map(|iface| iface.mac.clone())
            .unwrap_or_else(|| "00:00:00:00:00:00".to_string());

        let attachment = CniAttachment {
            container_id: container_id.to_string(),
            network_name: network_name.to_string(),
            interface_name: interface_name.to_string(),
            ip_address,
            mac_address,
            gateway: cni_result.ips.first().and_then(|ip| ip.gateway),
        };

        self.attachments
            .entry(container_id.to_string())
            .or_insert_with(Vec::new)
            .push(attachment);

        tracing::info!(
            "Attached container {} to network {} with IP {}",
            container_id,
            network_name,
            ip_address
        );

        Ok(cni_result)
    }

    /// Remove container from network (CNI DEL operation)
    pub async fn del_container(
        &mut self,
        container_id: &str,
        network_name: &str,
        interface_name: &str,
        netns_path: &str,
    ) -> Result<()> {
        let network = self.networks.get(network_name)
            .ok_or_else(|| horcrux_common::Error::System(format!("Network {} not found", network_name)))?;

        // Prepare CNI environment variables
        let env_vars = vec![
            ("CNI_COMMAND", "DEL"),
            ("CNI_CONTAINERID", container_id),
            ("CNI_NETNS", netns_path),
            ("CNI_IFNAME", interface_name),
            ("CNI_PATH", self.cni_bin_dir.to_str().unwrap()),
        ];

        // Call CNI plugin
        let plugin_path = self.cni_bin_dir.join(format!("{:?}", network.plugin_type).to_lowercase());
        let config_json = serde_json::to_string(&network)
            .map_err(|e| horcrux_common::Error::System(format!("Failed to serialize CNI config: {}", e)))?;

        let mut cmd = Command::new(&plugin_path);
        for (key, value) in env_vars {
            cmd.env(key, value);
        }

        let mut child = cmd
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| horcrux_common::Error::System(format!("Failed to spawn CNI plugin: {}", e)))?;

        // Write config to stdin and close it
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(config_json.as_bytes()).await?;
            drop(stdin); // Close stdin
        }

        let result = child.wait_with_output().await?;

        if !result.status.success() {
            let stderr = String::from_utf8_lossy(&result.stderr);
            tracing::warn!("CNI DEL warning: {}", stderr);
            // Don't fail on DEL errors - best effort cleanup
        }

        // Remove attachment
        if let Some(attachments) = self.attachments.get_mut(container_id) {
            attachments.retain(|a| a.network_name != network_name);
        }

        tracing::info!("Detached container {} from network {}", container_id, network_name);

        Ok(())
    }

    /// Check CNI plugin health (CNI CHECK operation)
    pub async fn check_container(
        &self,
        container_id: &str,
        network_name: &str,
        interface_name: &str,
        netns_path: &str,
    ) -> Result<()> {
        let network = self.networks.get(network_name)
            .ok_or_else(|| horcrux_common::Error::System(format!("Network {} not found", network_name)))?;

        let env_vars = vec![
            ("CNI_COMMAND", "CHECK"),
            ("CNI_CONTAINERID", container_id),
            ("CNI_NETNS", netns_path),
            ("CNI_IFNAME", interface_name),
            ("CNI_PATH", self.cni_bin_dir.to_str().unwrap()),
        ];

        let plugin_path = self.cni_bin_dir.join(format!("{:?}", network.plugin_type).to_lowercase());
        let config_json = serde_json::to_string(&network)
            .map_err(|e| horcrux_common::Error::System(format!("Failed to serialize CNI config: {}", e)))?;

        let mut cmd = Command::new(&plugin_path);
        for (key, value) in env_vars {
            cmd.env(key, value);
        }

        let mut child = cmd
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| horcrux_common::Error::System(format!("Failed to spawn CNI plugin: {}", e)))?;

        // Write config to stdin and close it
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(config_json.as_bytes()).await?;
            drop(stdin); // Close stdin
        }

        let result = child.wait_with_output().await?;

        if !result.status.success() {
            let stderr = String::from_utf8_lossy(&result.stderr);
            return Err(horcrux_common::Error::System(format!("CNI CHECK failed: {}", stderr)));
        }

        Ok(())
    }

    /// List all networks
    pub fn list_networks(&self) -> Vec<CniConfig> {
        self.networks.values().cloned().collect()
    }

    /// Get network by name
    pub fn get_network(&self, name: &str) -> Option<&CniConfig> {
        self.networks.get(name)
    }

    /// Delete a network
    pub async fn delete_network(&mut self, name: &str) -> Result<()> {
        self.networks.remove(name);

        // Remove config file
        let conf_file = self.cni_conf_dir.join(format!("{}.conflist", name));
        if conf_file.exists() {
            tokio::fs::remove_file(&conf_file).await?;
        }

        tracing::info!("Deleted CNI network: {}", name);
        Ok(())
    }

    /// List container attachments
    pub fn list_attachments(&self, container_id: &str) -> Vec<CniAttachment> {
        self.attachments
            .get(container_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Get CNI plugin capabilities
    pub async fn get_capabilities(&self, plugin_type: &CniPluginType) -> Result<Vec<String>> {
        let plugin_path = self.cni_bin_dir.join(format!("{:?}", plugin_type).to_lowercase());

        if !plugin_path.exists() {
            return Err(horcrux_common::Error::System(format!(
                "CNI plugin {:?} not found at {}",
                plugin_type,
                plugin_path.display()
            )));
        }

        // Most CNI plugins support these basic capabilities
        Ok(vec![
            "portMappings".to_string(),
            "bandwidth".to_string(),
            "ipRanges".to_string(),
        ])
    }

    /// Create default bridge network
    pub async fn create_default_network(&mut self) -> Result<()> {
        let default_config = CniConfig {
            cni_version: "1.0.0".to_string(),
            name: "horcrux-default".to_string(),
            plugin_type: CniPluginType::Bridge,
            bridge: Some("cni0".to_string()),
            ipam: IpamConfig {
                ipam_type: "host-local".to_string(),
                subnet: Some("10.88.0.0/16".to_string()),
                range_start: Some("10.88.0.10".parse().unwrap()),
                range_end: Some("10.88.255.254".parse().unwrap()),
                gateway: Some("10.88.0.1".parse().unwrap()),
                routes: vec![
                    RouteConfig {
                        dst: "0.0.0.0/0".to_string(),
                        gw: None,
                    }
                ],
            },
            dns: Some(DnsConfig {
                nameservers: vec!["8.8.8.8".parse().unwrap(), "8.8.4.4".parse().unwrap()],
                domain: Some("horcrux.local".to_string()),
                search: vec!["horcrux.local".to_string()],
                options: vec![],
            }),
            capabilities: HashMap::from([
                ("portMappings".to_string(), true),
                ("bandwidth".to_string(), true),
            ]),
        };

        self.create_network(default_config).await?;
        tracing::info!("Created default CNI network: horcrux-default");

        Ok(())
    }
}

impl Default for IpamConfig {
    fn default() -> Self {
        Self {
            ipam_type: "host-local".to_string(),
            subnet: None,
            range_start: None,
            range_end: None,
            gateway: None,
            routes: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cni_config_serialization() {
        let config = CniConfig {
            cni_version: "1.0.0".to_string(),
            name: "test-network".to_string(),
            plugin_type: CniPluginType::Bridge,
            bridge: Some("cni0".to_string()),
            ipam: IpamConfig::default(),
            dns: None,
            capabilities: HashMap::new(),
        };

        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("test-network"));
    }

    #[test]
    fn test_route_config() {
        let route = RouteConfig {
            dst: "0.0.0.0/0".to_string(),
            gw: Some("10.0.0.1".parse().unwrap()),
        };

        assert_eq!(route.dst, "0.0.0.0/0");
    }
}
