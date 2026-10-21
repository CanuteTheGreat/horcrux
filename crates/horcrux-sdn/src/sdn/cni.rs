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
    #[serde(default)]
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
    #[serde(default)]
    pub routes: Vec<RouteConfig>,
}

/// DNS configuration.
///
/// Every key is optional per the CNI spec ("dns" in a result is an object
/// whose members may all be absent). The bridge plugin returns `"dns": {}`,
/// and requiring `nameservers` made every successful ADD come back as a
/// 500 "missing field `nameservers`" (daily QA 2026-10-21) - after the
/// plugin had already wired the veth and taken an IPAM lease, so the
/// attachment was never recorded and the lease leaked.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DnsConfig {
    #[serde(default)]
    pub nameservers: Vec<IpAddr>,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub search: Vec<String>,
    #[serde(default)]
    pub options: Vec<String>,
}

/// Route configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteConfig {
    pub dst: String,
    #[serde(default)]
    pub gw: Option<IpAddr>,
}

fn default_namespace() -> String {
    "default".to_string()
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
    /// Network namespace the plugin ran in (used again for DEL, so a
    /// detach needs only the container and the network).
    #[serde(default)]
    pub netns_path: String,
    /// The Horcrux container this attachment belongs to, when it was made
    /// from the container picker (its netns is looked up again on detach,
    /// since the container may have restarted with a new PID).
    #[serde(default)]
    pub guest: Option<String>,
    /// Network-policy namespace and labels of this endpoint (what a
    /// policy's `namespace` / `pod_selector` match against).
    #[serde(default = "default_namespace")]
    pub namespace: String,
    #[serde(default)]
    pub labels: HashMap<String, String>,
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
    #[serde(default)]
    pub dns: Option<DnsConfig>,
}

/// CNI interface info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CniInterface {
    pub name: String,
    #[serde(default)]
    pub mac: String,
    #[serde(default)]
    pub sandbox: Option<String>,
}

/// CNI IP configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CniIpConfig {
    pub address: String, // CIDR notation
    #[serde(default)]
    pub gateway: Option<IpAddr>,
    #[serde(default)]
    pub interface: Option<u32>,
}

/// Parse `a.b.c.d/n` (or an IPv6 prefix) into (address, prefix length).
pub fn parse_cidr(cidr: &str) -> Result<(IpAddr, u8)> {
    let bad = || {
        horcrux_common::Error::Validation(format!(
            "Invalid CIDR {:?}: expected e.g. 10.244.0.0/24",
            cidr
        ))
    };
    let (addr, prefix) = cidr.trim().split_once('/').ok_or_else(bad)?;
    let addr: IpAddr = addr.parse().map_err(|_| bad())?;
    let prefix: u8 = prefix.parse().map_err(|_| bad())?;
    let max = if addr.is_ipv4() { 32 } else { 128 };
    if prefix > max {
        return Err(bad());
    }
    Ok((addr, prefix))
}

/// Whether `ip` lies inside `net/prefix` (same address family only).
pub fn cidr_contains(net: IpAddr, prefix: u8, ip: IpAddr) -> bool {
    match (net, ip) {
        (IpAddr::V4(n), IpAddr::V4(a)) => {
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - prefix as u32)
            };
            (u32::from(n) & mask) == (u32::from(a) & mask)
        }
        (IpAddr::V6(n), IpAddr::V6(a)) => {
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX << (128 - prefix as u32)
            };
            (u128::from(n) & mask) == (u128::from(a) & mask)
        }
        _ => false,
    }
}

fn invalid(msg: String) -> horcrux_common::Error {
    horcrux_common::Error::Validation(msg)
}

/// Check a network definition before anything is written to disk: a safe
/// name (it becomes `<conf_dir>/<name>.conflist` and host-local's lease
/// directory), a real interface name for the bridge, and IPAM addresses
/// that make sense together (subnet in CIDR form, gateway and range inside
/// it, range start <= end). Bad input is `Error::Validation` (HTTP 400).
pub fn validate_network(config: &CniConfig) -> Result<()> {
    crate::validation::validate_cni_network_name(&config.name)?;
    if let Some(bridge) = &config.bridge {
        if !horcrux_common::valid_ifname(bridge) {
            return Err(invalid(format!(
                "Invalid bridge device {:?}: 1-15 of letters, digits, '-', '_', '.'",
                bridge
            )));
        }
    }
    let ipam = &config.ipam;
    if !matches!(ipam.ipam_type.as_str(), "host-local" | "dhcp" | "static") {
        return Err(invalid(format!(
            "Unsupported IPAM type {:?} (host-local, dhcp or static)",
            ipam.ipam_type
        )));
    }
    let subnet = match &ipam.subnet {
        Some(s) => Some(parse_cidr(s)?),
        None if ipam.ipam_type == "host-local" => {
            return Err(invalid(
                "A subnet (CIDR, e.g. 10.244.0.0/24) is required for host-local IPAM".into(),
            ))
        }
        None => None,
    };
    if let Some((net, prefix)) = subnet {
        let max = if net.is_ipv4() { 32 } else { 128 };
        if prefix >= max - 1 {
            return Err(invalid(format!(
                "Subnet /{} has no room for a gateway and containers",
                prefix
            )));
        }
        let inside = |what: &str, ip: IpAddr| -> Result<()> {
            if !cidr_contains(net, prefix, ip) {
                return Err(invalid(format!(
                    "{} {} is not inside subnet {}/{}",
                    what, ip, net, prefix
                )));
            }
            Ok(())
        };
        if let Some(gw) = ipam.gateway {
            inside("Gateway", gw)?;
        }
        if let Some(start) = ipam.range_start {
            inside("Range start", start)?;
        }
        if let Some(end) = ipam.range_end {
            inside("Range end", end)?;
        }
    } else if ipam.gateway.is_some() || ipam.range_start.is_some() || ipam.range_end.is_some() {
        return Err(invalid(
            "Gateway and range need a subnet to be checked against".into(),
        ));
    }
    if let (Some(s), Some(e)) = (ipam.range_start, ipam.range_end) {
        let ordered = match (s, e) {
            (IpAddr::V4(a), IpAddr::V4(b)) => u32::from(a) <= u32::from(b),
            (IpAddr::V6(a), IpAddr::V6(b)) => u128::from(a) <= u128::from(b),
            _ => false,
        };
        if !ordered {
            return Err(invalid(format!(
                "Range start {} must not be after range end {}",
                s, e
            )));
        }
    }
    for r in &ipam.routes {
        parse_cidr(&r.dst)?;
    }
    Ok(())
}

/// Attachment inputs that end up in plugin env vars and file paths
/// (host-local keeps one lease file per IP naming the container id).
fn validate_attach_args(container_id: &str, interface_name: &str, netns_path: &str) -> Result<()> {
    crate::validation::validate_cni_name("Container id", container_id)?;
    if !horcrux_common::valid_ifname(interface_name) {
        return Err(invalid(format!(
            "Invalid interface name {:?}: 1-15 of letters, digits, '-', '_', '.'",
            interface_name
        )));
    }
    let ok_prefix = ["/proc/", "/var/run/netns/", "/run/netns/"]
        .iter()
        .any(|p| netns_path.starts_with(p));
    if !ok_prefix || netns_path.contains("..") || netns_path.contains('\0') {
        return Err(invalid(format!(
            "Invalid network namespace path {:?}: must be /proc/<pid>/ns/net or /run/netns/<name>",
            netns_path
        )));
    }
    Ok(())
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

    /// `<conf_dir>/<name>.conflist`, only for a valid network name: the
    /// path builder re-checks the name itself so no caller can make it
    /// write or delete a file outside the CNI conf dir.
    pub fn conf_path(&self, name: &str) -> Result<PathBuf> {
        crate::validation::validate_cni_network_name(name)?;
        Ok(self.cni_conf_dir.join(format!("{}.conflist", name)))
    }

    async fn write_conflist(&self, config: &CniConfig) -> Result<PathBuf> {
        let conf_file = self.conf_path(&config.name)?;

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
            "plugins": [build_netconf(config)]
        });

        tokio::fs::write(
            &conf_file,
            serde_json::to_string_pretty(&conf_list).map_err(|e| {
                horcrux_common::Error::System(format!("Failed to serialize CNI config: {}", e))
            })?,
        )
        .await
        .map_err(|e| horcrux_common::Error::System(format!("Failed to write CNI config: {}", e)))?;
        Ok(conf_file)
    }

    /// Create a new CNI network (validated first; an existing name is a
    /// conflict instead of silently replacing a network in use).
    pub async fn create_network(&mut self, config: CniConfig) -> Result<()> {
        validate_network(&config)?;
        if self.networks.contains_key(&config.name) {
            return Err(horcrux_common::Error::AlreadyExists(format!(
                "CNI network {} already exists",
                config.name
            )));
        }
        let conf_file = self.write_conflist(&config).await?;
        self.networks.insert(config.name.clone(), config);
        tracing::info!("Created CNI network: {}", conf_file.display());

        Ok(())
    }

    /// Run one CNI plugin command (ADD/DEL/CHECK) for `network`.
    async fn run_plugin(
        &self,
        command: &str,
        network: &CniConfig,
        container_id: &str,
        interface_name: &str,
        netns_path: &str,
    ) -> Result<std::process::Output> {
        let cni_bin_dir_str = self.cni_bin_dir.to_str().ok_or_else(|| {
            horcrux_common::Error::System(format!(
                "CNI bin dir path is not valid UTF-8: {:?}",
                self.cni_bin_dir
            ))
        })?;
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

        let mut child = Command::new(&plugin_path)
            .env("CNI_COMMAND", command)
            .env("CNI_CONTAINERID", container_id)
            .env("CNI_NETNS", netns_path)
            .env("CNI_IFNAME", interface_name)
            .env("CNI_PATH", cni_bin_dir_str)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| {
                horcrux_common::Error::System(format!(
                    "Failed to spawn CNI plugin {}: {}",
                    plugin_path.display(),
                    e
                ))
            })?;

        // Write config to stdin and close it
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(config_json.as_bytes()).await?;
            drop(stdin); // Close stdin
        }

        Ok(child.wait_with_output().await?)
    }

    /// Add container to network (CNI ADD operation)
    pub async fn add_container(
        &mut self,
        container_id: &str,
        network_name: &str,
        interface_name: &str,
        netns_path: &str,
    ) -> Result<CniResult> {
        self.attach(
            container_id,
            network_name,
            interface_name,
            netns_path,
            None,
            default_namespace(),
            HashMap::new(),
        )
        .await
        .map(|(result, _)| result)
    }

    /// CNI ADD plus bookkeeping: validates the inputs, refuses a second
    /// attachment of the same container to the same network (or a clash on
    /// the interface name), runs the plugin, and records the attachment
    /// (with its netns, policy namespace and labels). If the plugin's
    /// result can't be used, the plugin is undone with DEL so no veth or
    /// IPAM lease is left behind.
    #[allow(clippy::too_many_arguments)]
    pub async fn attach(
        &mut self,
        container_id: &str,
        network_name: &str,
        interface_name: &str,
        netns_path: &str,
        guest: Option<String>,
        namespace: String,
        labels: HashMap<String, String>,
    ) -> Result<(CniResult, CniAttachment)> {
        crate::validation::validate_cni_network_name(network_name)?;
        validate_attach_args(container_id, interface_name, netns_path)?;
        crate::validation::validate_cni_name("Policy namespace", &namespace)?;
        let network = self.networks.get(network_name).cloned().ok_or_else(|| {
            horcrux_common::Error::NotFound(format!("Network {} not found", network_name))
        })?;
        if let Some(existing) = self.attachments.get(container_id) {
            if existing.iter().any(|a| a.network_name == network_name) {
                return Err(horcrux_common::Error::AlreadyExists(format!(
                    "{} is already attached to network {}",
                    container_id, network_name
                )));
            }
            if existing.iter().any(|a| a.interface_name == interface_name) {
                return Err(horcrux_common::Error::AlreadyExists(format!(
                    "{} already has an interface {} (attached to another network)",
                    container_id, interface_name
                )));
            }
        }
        if !std::path::Path::new(netns_path).exists() {
            return Err(horcrux_common::Error::NotFound(format!(
                "Network namespace {} does not exist (is the container running?)",
                netns_path
            )));
        }

        let result = self
            .run_plugin("ADD", &network, container_id, interface_name, netns_path)
            .await?;

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

        // Parse the CNI result. The plugin has already wired the interface
        // and taken an IPAM lease at this point: anything that stops us
        // recording the attachment must undo that with DEL.
        let stdout = String::from_utf8_lossy(&result.stdout).to_string();
        let parsed = serde_json::from_str::<CniResult>(&stdout)
            .map_err(|e| format!("Failed to parse CNI result: {}", e))
            .and_then(|r| {
                let ip = r
                    .ips
                    .first()
                    .and_then(|ip| ip.address.split('/').next())
                    .and_then(|ip_str| ip_str.parse::<IpAddr>().ok())
                    .ok_or_else(|| "No IP address in CNI result".to_string())?;
                Ok((r, ip))
            });
        let (cni_result, ip_address) = match parsed {
            Ok(v) => v,
            Err(msg) => {
                let _ = self
                    .run_plugin("DEL", &network, container_id, interface_name, netns_path)
                    .await;
                return Err(horcrux_common::Error::System(format!(
                    "{} (the plugin's changes were rolled back): {}",
                    msg, stdout
                )));
            }
        };

        // The container-side interface (it has a sandbox) carries the MAC
        // the container uses; fall back to the first interface listed.
        let mac_address = cni_result
            .interfaces
            .iter()
            .find(|i| i.sandbox.as_deref().is_some_and(|s| !s.is_empty()))
            .or_else(|| cni_result.interfaces.first())
            .map(|iface| iface.mac.clone())
            .unwrap_or_else(|| "00:00:00:00:00:00".to_string());

        let attachment = CniAttachment {
            container_id: container_id.to_string(),
            network_name: network_name.to_string(),
            interface_name: interface_name.to_string(),
            ip_address,
            mac_address,
            gateway: cni_result.ips.first().and_then(|ip| ip.gateway),
            netns_path: netns_path.to_string(),
            guest,
            namespace,
            labels,
        };

        self.attachments
            .entry(container_id.to_string())
            .or_default()
            .push(attachment.clone());

        tracing::info!(
            "Attached container {} to network {} with IP {}",
            container_id,
            network_name,
            ip_address
        );

        Ok((cni_result, attachment))
    }

    /// The recorded attachment of `container_id` to `network_name`.
    pub fn get_attachment(&self, container_id: &str, network_name: &str) -> Option<CniAttachment> {
        self.attachments
            .get(container_id)?
            .iter()
            .find(|a| a.network_name == network_name)
            .cloned()
    }

    /// Interface name and netns for DEL/CHECK: the caller's, else the ones
    /// recorded at attach time.
    fn resolve_args(
        &self,
        container_id: &str,
        network_name: &str,
        interface_name: &str,
        netns_path: &str,
    ) -> Result<(String, String, bool)> {
        let recorded = self.get_attachment(container_id, network_name);
        let pick = |given: &str, rec: Option<String>| {
            if given.is_empty() {
                rec.unwrap_or_default()
            } else {
                given.to_string()
            }
        };
        let ifname = pick(
            interface_name,
            recorded.as_ref().map(|a| a.interface_name.clone()),
        );
        let netns = pick(netns_path, recorded.as_ref().map(|a| a.netns_path.clone()));
        if recorded.is_none() && (ifname.is_empty() || netns.is_empty()) {
            return Err(horcrux_common::Error::NotFound(format!(
                "{} is not attached to network {}",
                container_id, network_name
            )));
        }
        validate_attach_args(container_id, &ifname, &netns)?;
        Ok((ifname, netns, recorded.is_some()))
    }

    /// Remove container from network (CNI DEL operation). The interface
    /// name and netns default to the ones recorded at attach time; DEL also
    /// releases the IPAM (host-local) lease. A failing DEL is an error and
    /// the attachment stays recorded, so it can be retried.
    pub async fn del_container(
        &mut self,
        container_id: &str,
        network_name: &str,
        interface_name: &str,
        netns_path: &str,
    ) -> Result<()> {
        crate::validation::validate_cni_network_name(network_name)?;
        let (ifname, netns, _) =
            self.resolve_args(container_id, network_name, interface_name, netns_path)?;
        let network = self.networks.get(network_name).cloned().ok_or_else(|| {
            horcrux_common::Error::NotFound(format!("Network {} not found", network_name))
        })?;

        let result = self
            .run_plugin("DEL", &network, container_id, &ifname, &netns)
            .await?;

        if !result.status.success() {
            let stdout = String::from_utf8_lossy(&result.stdout);
            let stderr = String::from_utf8_lossy(&result.stderr);
            return Err(horcrux_common::Error::System(format!(
                "CNI DEL failed (exit: {:?}): stdout={} stderr={}",
                result.status.code(),
                stdout,
                stderr
            )));
        }

        if let Some(attachments) = self.attachments.get_mut(container_id) {
            attachments.retain(|a| a.network_name != network_name);
            if attachments.is_empty() {
                self.attachments.remove(container_id);
            }
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
        crate::validation::validate_cni_network_name(network_name)?;
        let (ifname, netns, _) =
            self.resolve_args(container_id, network_name, interface_name, netns_path)?;
        let network = self.networks.get(network_name).ok_or_else(|| {
            horcrux_common::Error::NotFound(format!("Network {} not found", network_name))
        })?;

        let result = self
            .run_plugin("CHECK", network, container_id, &ifname, &netns)
            .await?;

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
        let mut v: Vec<CniConfig> = self.networks.values().cloned().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    /// Get network by name
    pub fn get_network(&self, name: &str) -> Option<&CniConfig> {
        self.networks.get(name)
    }

    /// Delete a network
    pub async fn delete_network(&mut self, name: &str) -> Result<()> {
        let conf_file = self.conf_path(name)?;
        if !self.networks.contains_key(name) {
            return Err(horcrux_common::Error::NotFound(format!(
                "Network {} not found",
                name
            )));
        }
        // Refuse to delete a network that still has containers attached to
        // it (mirrors Docker/CNI-chain-manager behavior). Without this
        // check, deleting an in-use network silently removed the
        // .conflist a plugin needs to run DEL later, orphaning the
        // veth/bridge interfaces those containers still hold on the host
        // and the IPAM leases they hold.
        let mut still_attached: Vec<&str> = self
            .attachments
            .iter()
            .filter(|(_, atts)| atts.iter().any(|a| a.network_name == name))
            .map(|(container_id, _)| container_id.as_str())
            .collect();
        still_attached.sort();
        if !still_attached.is_empty() {
            return Err(horcrux_common::Error::Conflict(format!(
                "Cannot delete network {}: {} container(s) still attached ({}). Detach them first.",
                name,
                still_attached.len(),
                still_attached.join(", ")
            )));
        }

        self.networks.remove(name);

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

    /// Every recorded attachment (all containers, all networks).
    pub fn all_attachments(&self) -> Vec<CniAttachment> {
        let mut v: Vec<CniAttachment> = self.attachments.values().flatten().cloned().collect();
        v.sort_by(|a, b| {
            (a.network_name.as_str(), a.container_id.as_str())
                .cmp(&(b.network_name.as_str(), b.container_id.as_str()))
        });
        v
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

    /// Create default bridge network (no-op when it already exists).
    pub async fn create_default_network(&mut self) -> Result<()> {
        if self.networks.contains_key("horcrux-default") {
            return Ok(());
        }
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

    fn net(name: &str) -> CniConfig {
        CniConfig {
            cni_version: "1.0.0".to_string(),
            name: name.to_string(),
            plugin_type: CniPluginType::Bridge,
            bridge: Some("cnitest0".to_string()),
            ipam: IpamConfig {
                subnet: Some("10.251.0.0/24".to_string()),
                gateway: Some("10.251.0.1".parse().unwrap()),
                ..IpamConfig::default()
            },
            dns: None,
            capabilities: HashMap::new(),
        }
    }

    #[test]
    fn test_cni_config_serialization() {
        let json = serde_json::to_string(&net("test-network")).unwrap();
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

    /// What the real bridge plugin (containernetworking/plugins 1.x) prints
    /// on ADD: `"dns": {}` with no members. Daily QA 2026-10-21: this was
    /// rejected with "missing field `nameservers`" after the plugin had
    /// already attached the container.
    const BRIDGE_ADD_RESULT: &str = r#"{
        "cniVersion": "1.0.0",
        "interfaces": [
            {"name": "qa-br0", "mac": "4a:18:00:2a:42:2f"},
            {"name": "veth598864ce", "mac": "9e:77:35:05:f4:55"},
            {"name": "eth0", "mac": "6a:0b:3c:11:22:33", "sandbox": "/proc/1234/ns/net"}
        ],
        "ips": [{"interface": 2, "address": "10.251.0.2/24", "gateway": "10.251.0.1"}],
        "routes": [{"dst": "0.0.0.0/0"}],
        "dns": {}
    }"#;

    #[test]
    fn bridge_plugin_result_with_empty_dns_parses() {
        let r: CniResult = serde_json::from_str(BRIDGE_ADD_RESULT).unwrap();
        assert_eq!(r.ips[0].address, "10.251.0.2/24");
        assert!(r.dns.unwrap().nameservers.is_empty());
        let r: CniResult = serde_json::from_str(r#"{"cniVersion":"1.0.0"}"#).unwrap();
        assert!(r.dns.is_none() && r.ips.is_empty());
    }

    #[test]
    fn network_definitions_are_validated() {
        assert!(validate_network(&net("qa-sdn")).is_ok());
        let mut bad = net("qa-sdn");
        bad.ipam.subnet = Some("not-a-cidr".into());
        assert!(matches!(
            validate_network(&bad),
            Err(horcrux_common::Error::Validation(_))
        ));
        for s in ["10.0.0.0/33", "10.0.0.0", "10.0.0.300/24", "10.0.0.0/31"] {
            let mut b = net("x");
            b.ipam.subnet = Some(s.into());
            b.ipam.gateway = None;
            assert!(validate_network(&b).is_err(), "{s}");
        }
        let mut gw_out = net("x");
        gw_out.ipam.gateway = Some("192.168.1.1".parse().unwrap());
        assert!(validate_network(&gw_out).is_err());
        let mut range_rev = net("x");
        range_rev.ipam.range_start = Some("10.251.0.200".parse().unwrap());
        range_rev.ipam.range_end = Some("10.251.0.10".parse().unwrap());
        assert!(validate_network(&range_rev).is_err());
        let mut no_subnet = net("x");
        no_subnet.ipam.subnet = None;
        no_subnet.ipam.gateway = None;
        assert!(validate_network(&no_subnet).is_err());
        let mut bad_bridge = net("x");
        bad_bridge.bridge = Some("../../etc".into());
        assert!(validate_network(&bad_bridge).is_err());
        let mut v6 = net("v6");
        v6.ipam.subnet = Some("fd00:10::/64".into());
        v6.ipam.gateway = Some("fd00:10::1".parse().unwrap());
        assert!(validate_network(&v6).is_ok());
    }

    /// Daily QA 2026-10-21: `{"name":"../../../tmp/qa-sdn-escape"}` wrote
    /// /tmp/qa-sdn-escape.conflist as root. Nothing may be written or
    /// removed outside the conf dir, whatever the name.
    #[tokio::test]
    async fn network_names_cannot_escape_the_conf_dir() {
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join("a/b/conf");
        let mut m = CniManager::new(dir.path().join("bin"), conf.clone());
        let escape = dir.path().join("a/escape.conflist");
        for name in ["../escape", "../../a/escape", "/tmp/x", "a/b", ".."] {
            let err = m.create_network(net(name)).await.unwrap_err();
            assert!(
                matches!(err, horcrux_common::Error::Validation(_)),
                "{name}: {err:?}"
            );
            assert!(m.delete_network(name).await.is_err());
        }
        assert!(!escape.exists());
        std::fs::create_dir_all(dir.path().join("a")).unwrap();
        std::fs::write(&escape, "keep").unwrap();
        assert!(m.delete_network("../escape").await.is_err());
        assert!(escape.exists(), "delete must not remove files outside");
        assert!(m.conf_path("../escape").is_err());
    }

    /// A stand-in CNI plugin: ADD prints the real bridge plugin's result
    /// (`"dns": {}`), DEL/CHECK succeed; every call is logged.
    fn fake_plugin(bin: &std::path::Path, log: &std::path::Path, add_output: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(bin).unwrap();
        let script = format!(
            "#!/bin/sh\ncat >/dev/null\necho \"$CNI_COMMAND $CNI_CONTAINERID $CNI_IFNAME $CNI_NETNS\" >> {}\n\
             if [ \"$CNI_COMMAND\" = ADD ]; then cat <<'EOF'\n{}\nEOF\nfi\nexit 0\n",
            log.display(),
            add_output
        );
        let p = bin.join("bridge");
        std::fs::write(&p, script).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[tokio::test]
    async fn attach_records_refuses_delete_and_detach_runs_del() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        fake_plugin(&dir.path().join("bin"), &log, BRIDGE_ADD_RESULT);
        let mut m = CniManager::new(dir.path().join("bin"), dir.path().join("conf"));
        m.create_network(net("qa-sdn")).await.unwrap();
        assert!(matches!(
            m.create_network(net("qa-sdn")).await,
            Err(horcrux_common::Error::AlreadyExists(_))
        ));

        let netns = "/proc/self/ns/net";
        let (_, att) = m
            .attach(
                "ct-a",
                "qa-sdn",
                "eth1",
                netns,
                Some("ct-a".into()),
                "default".into(),
                HashMap::from([("app".to_string(), "web".to_string())]),
            )
            .await
            .expect("attach with the bridge plugin's real result");
        assert_eq!(att.ip_address.to_string(), "10.251.0.2");
        assert_eq!(att.mac_address, "6a:0b:3c:11:22:33");
        assert_eq!(m.list_attachments("ct-a").len(), 1);
        assert_eq!(m.all_attachments().len(), 1);

        // Same container + network again: conflict, plugin not run.
        assert!(matches!(
            m.add_container("ct-a", "qa-sdn", "eth2", netns).await,
            Err(horcrux_common::Error::AlreadyExists(_))
        ));
        // Bad inputs never reach the plugin.
        assert!(m
            .add_container("../x", "qa-sdn", "eth1", netns)
            .await
            .is_err());
        assert!(m
            .add_container("ct-b", "qa-sdn", "eth1", "/tmp/../proc/1/ns/net")
            .await
            .is_err());
        assert!(matches!(
            m.add_container("ct-b", "nope", "eth1", netns).await,
            Err(horcrux_common::Error::NotFound(_))
        ));

        // Deleting the network while ct-a is attached is refused.
        let err = m.delete_network("qa-sdn").await.unwrap_err();
        assert!(matches!(err, horcrux_common::Error::Conflict(_)), "{err:?}");
        assert!(err.to_string().contains("ct-a"));
        assert!(m.get_network("qa-sdn").is_some());
        assert!(dir.path().join("conf/qa-sdn.conflist").exists());

        // Detach needs only container + network: DEL gets the recorded
        // ifname and netns (host-local releases the lease on DEL).
        m.del_container("ct-a", "qa-sdn", "", "").await.unwrap();
        assert!(m.list_attachments("ct-a").is_empty());
        let calls = std::fs::read_to_string(&log).unwrap();
        assert!(calls.contains(&format!("DEL ct-a eth1 {netns}")), "{calls}");
        assert!(!calls.contains("ADD ct-b"), "{calls}");
        assert!(matches!(
            m.del_container("ct-a", "qa-sdn", "", "").await,
            Err(horcrux_common::Error::NotFound(_))
        ));

        m.delete_network("qa-sdn").await.unwrap();
        assert!(m.get_network("qa-sdn").is_none());
        assert!(!dir.path().join("conf/qa-sdn.conflist").exists());
    }

    /// A result we can't use after a successful ADD is rolled back with DEL
    /// instead of leaving the veth and the IPAM lease behind.
    #[tokio::test]
    async fn unusable_add_result_is_rolled_back() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        fake_plugin(
            &dir.path().join("bin"),
            &log,
            r#"{"cniVersion":"1.0.0","ips":[]}"#,
        );
        let mut m = CniManager::new(dir.path().join("bin"), dir.path().join("conf"));
        m.create_network(net("qa-sdn")).await.unwrap();
        let err = m
            .add_container("ct-a", "qa-sdn", "eth1", "/proc/self/ns/net")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("rolled back"), "{err}");
        assert!(m.list_attachments("ct-a").is_empty());
        let calls = std::fs::read_to_string(&log).unwrap();
        assert!(calls.contains("DEL ct-a eth1"), "{calls}");
    }
}
