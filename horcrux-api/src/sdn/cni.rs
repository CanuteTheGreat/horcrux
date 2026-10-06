//! CNI (Container Network Interface) implementation
//! Provides Kubernetes-style networking for containers
//! Implements CNI spec version 1.0.0
use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
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
    Ptp,  // Point-to-point
    Host, // Host networking
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
    /// Real CNI plugins return this key as "cniVersion" (camelCase) per
    /// the spec, not "cni_version" - without the rename, serde_json
    /// fails to deserialize every real plugin's ADD/CHECK response with
    /// "missing field `cni_version`" even though the field is right
    /// there in the JSON, just spelled the way the spec actually requires.
    #[serde(rename = "cniVersion")]
    pub cni_version: String,
    #[serde(default)]
    pub interfaces: Vec<CniInterface>,
    #[serde(default)]
    pub ips: Vec<CniIpConfig>,
    /// Real plugins (e.g. host-local with no routes configured) omit
    /// this key entirely rather than sending an empty array - without
    /// #[serde(default)] that made deserialization fail with "missing
    /// field `routes`" on every real ADD that didn't configure routes.
    #[serde(default)]
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

/// Translate a CniConfig into the actual on-the-wire NetConf JSON real CNI
/// plugins expect (used both for the on-disk .conflist a plugin chain
/// manager would read, and for the stdin payload piped directly to a
/// plugin binary during ADD/DEL/CHECK). CniConfig/IpamConfig's own Rust
/// field names (plugin_type, ipam_type, range_start, range_end) are kept
/// as-is for API/backwards compatibility, but they are NOT what real
/// plugins understand on the wire - the CNI spec requires "type" (not
/// "plugin_type"/"ipam_type") and "rangeStart"/"rangeEnd" (camelCase, not
/// snake_case). Serializing the Rust structs directly previously produced
/// a NetConf real plugins silently misparsed: `bridge` sees no "type" key
/// matching itself so still runs, but `host-local` sees no usable
/// "rangeStart"/"rangeEnd" and fails with {"code":999,"msg":"cannot
/// convert: no valid IP addresses"}.
fn build_netconf(config: &CniConfig) -> serde_json::Value {
    serde_json::json!({
        "cniVersion": config.cni_version,
        "name": config.name,
        "type": format!("{:?}", config.plugin_type).to_lowercase(),
        "bridge": config.bridge,
        "isGateway": true,
        "ipMasq": true,
        "ipam": {
            "type": config.ipam.ipam_type,
            "subnet": config.ipam.subnet,
            "rangeStart": config.ipam.range_start,
            "rangeEnd": config.ipam.range_end,
            "gateway": config.ipam.gateway,
            "routes": config.ipam.routes,
        },
    })
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

        // The conf dir (default /etc/cni/net.d) may not exist yet on a
        // fresh install or in an unprivileged CI environment - creating it
        // here means callers only need to set HORCRUX_CNI_CONF_DIR to a
        // writable path, not also pre-create it by hand.
        tokio::fs::create_dir_all(&self.cni_conf_dir)
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!(
                    "Failed to create CNI conf directory {}: {}",
                    self.cni_conf_dir.display(),
                    e
                ))
            })?;

        let conf_list = serde_json::json!({
            "cniVersion": config.cni_version,
            "name": config.name,
            "plugins": [build_netconf(&config)]
        });

        tokio::fs::write(
            &conf_file,
            serde_json::to_string_pretty(&conf_list).map_err(|e| {
                horcrux_common::Error::System(format!("Failed to serialize CNI config: {}", e))
            })?,
        )
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
        let network = self.networks.get(network_name).ok_or_else(|| {
            horcrux_common::Error::System(format!("Network {} not found", network_name))
        })?;

        // Prepare CNI environment variables
        let env_vars = vec![
            ("CNI_COMMAND", "ADD"),
            ("CNI_CONTAINERID", container_id),
            ("CNI_NETNS", netns_path),
            ("CNI_IFNAME", interface_name),
            ("CNI_PATH", self.cni_bin_dir.to_str().unwrap()),
        ];

        // Call CNI plugin
        let plugin_path = self
            .cni_bin_dir
            .join(format!("{:?}", network.plugin_type).to_lowercase());
        // The stdin payload a CNI plugin receives must be real NetConf
        // JSON (see build_netconf's doc comment) - serializing `network`
        // (the CniConfig struct) directly used the wrong field names and
        // was why ADD/DEL/CHECK always failed against real plugins even
        // after the on-disk conflist was fixed to use the right schema.
        let config_json = serde_json::to_string(&build_netconf(network)).map_err(|e| {
            horcrux_common::Error::System(format!("Failed to serialize CNI config: {}", e))
        })?;

        let mut cmd = Command::new(&plugin_path);
        for (key, value) in env_vars {
            cmd.env(key, value);
        }

        let mut child = cmd
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to spawn CNI plugin: {}", e))
            })?;

        // Write config to stdin and close it
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(config_json.as_bytes()).await?;
            drop(stdin); // Close stdin
        }

        let result = child.wait_with_output().await?;

        if !result.status.success() {
            // Per the CNI spec, plugins report structured errors as JSON on
            // stdout (not stderr) when they exit non-zero. Surface both so
            // callers/operators can actually see why ADD failed instead of
            // just an empty string.
            let stdout = String::from_utf8_lossy(&result.stdout);
            let stderr = String::from_utf8_lossy(&result.stderr);
            return Err(horcrux_common::Error::System(format!(
                "CNI plugin failed (exit: {:?}): stdout={} stderr={}",
                result.status.code(),
                stdout,
                stderr
            )));
        }

        // Parse CNI result
        let stdout = String::from_utf8_lossy(&result.stdout);
        let cni_result: CniResult = serde_json::from_str(&stdout).map_err(|e| {
            horcrux_common::Error::System(format!("Failed to parse CNI result: {}", e))
        })?;

        // Store attachment
        let ip_address = cni_result
            .ips
            .first()
            .and_then(|ip| ip.address.split('/').next())
            .and_then(|ip_str| ip_str.parse().ok())
            .ok_or_else(|| {
                horcrux_common::Error::System("No IP address in CNI result".to_string())
            })?;

        let mac_address = cni_result
            .interfaces
            .first()
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
            .or_default()
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
        let network = self.networks.get(network_name).ok_or_else(|| {
            horcrux_common::Error::System(format!("Network {} not found", network_name))
        })?;

        // Prepare CNI environment variables
        let env_vars = vec![
            ("CNI_COMMAND", "DEL"),
            ("CNI_CONTAINERID", container_id),
            ("CNI_NETNS", netns_path),
            ("CNI_IFNAME", interface_name),
            ("CNI_PATH", self.cni_bin_dir.to_str().unwrap()),
        ];

        // Call CNI plugin
        let plugin_path = self
            .cni_bin_dir
            .join(format!("{:?}", network.plugin_type).to_lowercase());
        // The stdin payload a CNI plugin receives must be real NetConf
        // JSON (see build_netconf's doc comment) - serializing `network`
        // (the CniConfig struct) directly used the wrong field names and
        // was why ADD/DEL/CHECK always failed against real plugins even
        // after the on-disk conflist was fixed to use the right schema.
        let config_json = serde_json::to_string(&build_netconf(network)).map_err(|e| {
            horcrux_common::Error::System(format!("Failed to serialize CNI config: {}", e))
        })?;

        let mut cmd = Command::new(&plugin_path);
        for (key, value) in env_vars {
            cmd.env(key, value);
        }

        let mut child = cmd
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to spawn CNI plugin: {}", e))
            })?;

        // Write config to stdin and close it
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(config_json.as_bytes()).await?;
            drop(stdin); // Close stdin
        }

        let result = child.wait_with_output().await?;

        if !result.status.success() {
            let stdout = String::from_utf8_lossy(&result.stdout);
            let stderr = String::from_utf8_lossy(&result.stderr);
            tracing::warn!(
                "CNI DEL warning (exit: {:?}): stdout={} stderr={}",
                result.status.code(),
                stdout,
                stderr
            );
            // Don't fail on DEL errors - best effort cleanup
        }

        // Remove attachment
        if let Some(attachments) = self.attachments.get_mut(container_id) {
            attachments.retain(|a| a.network_name != network_name);
        }

        tracing::info!(
            "Detached container {} from network {}",
            container_id,
            network_name
        );

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
        let network = self.networks.get(network_name).ok_or_else(|| {
            horcrux_common::Error::System(format!("Network {} not found", network_name))
        })?;

        let env_vars = vec![
            ("CNI_COMMAND", "CHECK"),
            ("CNI_CONTAINERID", container_id),
            ("CNI_NETNS", netns_path),
            ("CNI_IFNAME", interface_name),
            ("CNI_PATH", self.cni_bin_dir.to_str().unwrap()),
        ];

        let plugin_path = self
            .cni_bin_dir
            .join(format!("{:?}", network.plugin_type).to_lowercase());
        // The stdin payload a CNI plugin receives must be real NetConf
        // JSON (see build_netconf's doc comment) - serializing `network`
        // (the CniConfig struct) directly used the wrong field names and
        // was why ADD/DEL/CHECK always failed against real plugins even
        // after the on-disk conflist was fixed to use the right schema.
        let config_json = serde_json::to_string(&build_netconf(network)).map_err(|e| {
            horcrux_common::Error::System(format!("Failed to serialize CNI config: {}", e))
        })?;

        let mut cmd = Command::new(&plugin_path);
        for (key, value) in env_vars {
            cmd.env(key, value);
        }

        let mut child = cmd
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to spawn CNI plugin: {}", e))
            })?;

        // Write config to stdin and close it
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(config_json.as_bytes()).await?;
            drop(stdin); // Close stdin
        }

        let result = child.wait_with_output().await?;

        if !result.status.success() {
            let stdout = String::from_utf8_lossy(&result.stdout);
            let stderr = String::from_utf8_lossy(&result.stderr);
            return Err(horcrux_common::Error::System(format!(
                "CNI CHECK failed (exit: {:?}): stdout={} stderr={}",
                result.status.code(),
                stdout,
                stderr
            )));
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
        // Refuse to delete a network that still has containers attached to
        // it (mirrors Docker/CNI-chain-manager behavior). Without this
        // check, deleting an in-use network silently removed the
        // .conflist a plugin needs to run DEL later, orphaning the
        // veth/bridge interfaces those containers still hold on the host
        // and leaving stale `attachments` entries pointing at a network
        // that no longer exists (list_attachments would keep reporting
        // them forever, since nothing else ever cleans that map).
        let still_attached: Vec<&str> = self
            .attachments
            .iter()
            .filter(|(_, atts)| atts.iter().any(|a| a.network_name == name))
            .map(|(container_id, _)| container_id.as_str())
            .collect();
        if !still_attached.is_empty() {
            return Err(horcrux_common::Error::System(format!(
                "Cannot delete network {}: {} container(s) still attached ({}).                  Detach them first via del_container.",
                name,
                still_attached.len(),
                still_attached.join(", ")
            )));
        }

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
        let plugin_path = self
            .cni_bin_dir
            .join(format!("{:?}", plugin_type).to_lowercase());

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
                routes: vec![RouteConfig {
                    dst: "0.0.0.0/0".to_string(),
                    gw: None,
                }],
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

    #[tokio::test]
    async fn test_delete_network_refuses_while_attached() {
        let dir = std::env::temp_dir().join(format!("horcrux-cni-test-{}", std::process::id()));
        let mut manager = CniManager::new(dir.join("bin"), dir.join("conf"));

        let config = CniConfig {
            cni_version: "1.0.0".to_string(),
            name: "test-net".to_string(),
            plugin_type: CniPluginType::Bridge,
            bridge: Some("cni0".to_string()),
            ipam: IpamConfig::default(),
            dns: None,
            capabilities: HashMap::new(),
        };
        manager.create_network(config).await.unwrap();

        // Simulate a container already attached to this network (as
        // add_container would have recorded after a real plugin ADD).
        manager.attachments.insert(
            "container-1".to_string(),
            vec![CniAttachment {
                container_id: "container-1".to_string(),
                network_name: "test-net".to_string(),
                interface_name: "eth0".to_string(),
                ip_address: "10.88.0.10".parse().unwrap(),
                mac_address: "00:00:00:00:00:01".to_string(),
                gateway: None,
            }],
        );

        // Deleting a network with a live attachment must fail, not
        // silently remove the network out from under the container.
        let err = manager.delete_network("test-net").await.unwrap_err();
        assert!(err.to_string().contains("container-1"));
        assert!(manager.get_network("test-net").is_some());

        // Once detached, deletion succeeds.
        manager.attachments.remove("container-1");
        manager.delete_network("test-net").await.unwrap();
        assert!(manager.get_network("test-net").is_none());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
