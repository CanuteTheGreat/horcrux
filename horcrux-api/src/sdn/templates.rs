///! Network Templates
///!
///! Reusable network configurations for VMs and containers

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Network template
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkTemplate {
    pub id: String,
    pub name: String,
    pub description: String,
    pub template_type: TemplateType,
    pub config: NetworkConfig,
    pub tags: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Template type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TemplateType {
    Bridge,     // Standard Linux bridge
    OvsBridge,  // Open vSwitch bridge
    Macvlan,    // MACVLAN interface
    Ipvlan,     // IPVLAN interface
    Vxlan,      // VXLAN overlay
    Custom,     // Custom configuration
}

/// Network configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// Bridge name
    pub bridge: Option<String>,

    /// VLAN configuration
    pub vlan: Option<VlanConfig>,

    /// IP configuration
    pub ip_config: IpConfig,

    /// Firewall rules
    pub firewall: Option<FirewallConfig>,

    /// QoS settings
    pub qos: Option<QosConfig>,

    /// Additional options
    pub options: HashMap<String, String>,
}

/// VLAN configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VlanConfig {
    /// VLAN ID (1-4095)
    pub vlan_id: u16,

    /// Trunk mode (allow multiple VLANs)
    pub trunk: bool,

    /// Allowed VLANs in trunk mode
    pub allowed_vlans: Vec<u16>,

    /// Native VLAN (untagged)
    pub native_vlan: Option<u16>,
}

/// IP configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpConfig {
    /// IP address mode
    pub mode: IpMode,

    /// Static IP address (if mode is Static)
    pub address: Option<String>,

    /// Network mask/prefix
    pub netmask: Option<String>,

    /// Default gateway
    pub gateway: Option<String>,

    /// DNS servers
    pub dns_servers: Vec<String>,

    /// DNS search domains
    pub dns_search: Vec<String>,

    /// MTU size
    pub mtu: Option<u16>,
}

/// IP address assignment mode
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum IpMode {
    Static,   // Static IP assignment
    Dhcp,     // DHCP from network
    Manual,   // Manually configured later
}

/// Firewall configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirewallConfig {
    /// Enable firewall
    pub enabled: bool,

    /// Default policy
    pub default_policy: FirewallPolicy,

    /// Firewall rules
    pub rules: Vec<FirewallRule>,

    /// Enable MAC address filtering
    pub mac_filter: bool,

    /// Enable IP spoofing protection
    pub anti_spoof: bool,
}

/// Firewall policy
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "UPPERCASE")]
pub enum FirewallPolicy {
    Accept,
    Drop,
    Reject,
}

/// Firewall rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirewallRule {
    /// Rule action
    pub action: FirewallPolicy,

    /// Protocol (tcp, udp, icmp, etc.)
    pub protocol: Option<String>,

    /// Source address/network
    pub source: Option<String>,

    /// Destination address/network
    pub dest: Option<String>,

    /// Source port
    pub sport: Option<String>,

    /// Destination port
    pub dport: Option<String>,

    /// Rule comment
    pub comment: Option<String>,
}

/// Quality of Service configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QosConfig {
    /// Enable QoS
    pub enabled: bool,

    /// Bandwidth limit (Mbps)
    pub rate_limit: Option<u32>,

    /// Burst size (MB)
    pub burst: Option<u32>,

    /// Priority (0-7, higher is better)
    pub priority: Option<u8>,

    /// Traffic class
    pub class: Option<String>,
}

/// Network template manager
pub struct TemplateManager {
    templates: HashMap<String, NetworkTemplate>,
}

impl TemplateManager {
    pub fn new() -> Self {
        Self {
            templates: HashMap::new(),
        }
    }

    /// Create a new network template
    pub fn create_template(&mut self, template: NetworkTemplate) -> Result<(), String> {
        if self.templates.contains_key(&template.id) {
            return Err(format!("Template {} already exists", template.id));
        }

        self.validate_template(&template)?;
        self.templates.insert(template.id.clone(), template);
        Ok(())
    }

    /// Get a template by ID
    pub fn get_template(&self, id: &str) -> Option<&NetworkTemplate> {
        self.templates.get(id)
    }

    /// List all templates
    pub fn list_templates(&self) -> Vec<&NetworkTemplate> {
        self.templates.values().collect()
    }

    /// List templates by type
    pub fn list_templates_by_type(&self, template_type: &TemplateType) -> Vec<&NetworkTemplate> {
        self.templates
            .values()
            .filter(|t| &t.template_type == template_type)
            .collect()
    }

    /// List templates by tag
    pub fn list_templates_by_tag(&self, tag: &str) -> Vec<&NetworkTemplate> {
        self.templates
            .values()
            .filter(|t| t.tags.contains(&tag.to_string()))
            .collect()
    }

    /// Update a template
    pub fn update_template(&mut self, id: &str, template: NetworkTemplate) -> Result<(), String> {
        if !self.templates.contains_key(id) {
            return Err(format!("Template {} not found", id));
        }

        self.validate_template(&template)?;
        self.templates.insert(id.to_string(), template);
        Ok(())
    }

    /// Delete a template
    pub fn delete_template(&mut self, id: &str) -> Result<(), String> {
        if self.templates.remove(id).is_none() {
            return Err(format!("Template {} not found", id));
        }
        Ok(())
    }

    /// Validate a network template
    fn validate_template(&self, template: &NetworkTemplate) -> Result<(), String> {
        // Validate VLAN if present
        if let Some(vlan) = &template.config.vlan {
            if vlan.vlan_id == 0 || vlan.vlan_id > 4095 {
                return Err(format!("Invalid VLAN ID: {} (must be 1-4095)", vlan.vlan_id));
            }

            for allowed_vlan in &vlan.allowed_vlans {
                if *allowed_vlan == 0 || *allowed_vlan > 4095 {
                    return Err(format!("Invalid allowed VLAN: {} (must be 1-4095)", allowed_vlan));
                }
            }

            if let Some(native) = vlan.native_vlan {
                if native == 0 || native > 4095 {
                    return Err(format!("Invalid native VLAN: {} (must be 1-4095)", native));
                }
            }
        }

        // Validate IP config
        if template.config.ip_config.mode == IpMode::Static {
            if template.config.ip_config.address.is_none() {
                return Err("Static IP mode requires an address".to_string());
            }
            if template.config.ip_config.netmask.is_none() {
                return Err("Static IP mode requires a netmask".to_string());
            }
        }

        // Validate MTU if present
        if let Some(mtu) = template.config.ip_config.mtu {
            if mtu < 68 || mtu > 9000 {
                return Err(format!("Invalid MTU: {} (must be 68-9000)", mtu));
            }
        }

        // Validate QoS if present
        if let Some(qos) = &template.config.qos {
            if qos.enabled {
                if qos.rate_limit.is_some() && qos.rate_limit.unwrap() == 0 {
                    return Err("Rate limit must be greater than 0".to_string());
                }
                if let Some(priority) = qos.priority {
                    if priority > 7 {
                        return Err(format!("Invalid priority: {} (must be 0-7)", priority));
                    }
                }
            }
        }

        Ok(())
    }

    /// Apply a template to create network configuration for a VM/container
    pub fn apply_template(&self, template_id: &str, instance_id: &str) -> Result<AppliedConfig, String> {
        let template = self.get_template(template_id)
            .ok_or_else(|| format!("Template {} not found", template_id))?;

        // Generate configuration based on template
        let config = AppliedConfig {
            instance_id: instance_id.to_string(),
            template_id: template_id.to_string(),
            bridge: template.config.bridge.clone(),
            vlan_id: template.config.vlan.as_ref().map(|v| v.vlan_id),
            ip_address: template.config.ip_config.address.clone(),
            gateway: template.config.ip_config.gateway.clone(),
            dns_servers: template.config.ip_config.dns_servers.clone(),
            mtu: template.config.ip_config.mtu,
            firewall_enabled: template.config.firewall.as_ref().map(|f| f.enabled).unwrap_or(false),
            qos_enabled: template.config.qos.as_ref().map(|q| q.enabled).unwrap_or(false),
        };

        Ok(config)
    }

    /// Create default templates
    pub fn create_default_templates(&mut self) -> Result<(), String> {
        // Default bridged network
        let bridged = NetworkTemplate {
            id: "default-bridge".to_string(),
            name: "Default Bridged Network".to_string(),
            description: "Standard bridged network with DHCP".to_string(),
            template_type: TemplateType::Bridge,
            config: NetworkConfig {
                bridge: Some("vmbr0".to_string()),
                vlan: None,
                ip_config: IpConfig {
                    mode: IpMode::Dhcp,
                    address: None,
                    netmask: None,
                    gateway: None,
                    dns_servers: vec![],
                    dns_search: vec![],
                    mtu: Some(1500),
                },
                firewall: None,
                qos: None,
                options: HashMap::new(),
            },
            tags: vec!["default".to_string(), "dhcp".to_string()],
            created_at: chrono::Utc::now().timestamp(),
            updated_at: chrono::Utc::now().timestamp(),
        };
        self.create_template(bridged)?;

        // VLAN-aware network
        let vlan_network = NetworkTemplate {
            id: "vlan-network".to_string(),
            name: "VLAN Network".to_string(),
            description: "VLAN-tagged network".to_string(),
            template_type: TemplateType::Bridge,
            config: NetworkConfig {
                bridge: Some("vmbr0".to_string()),
                vlan: Some(VlanConfig {
                    vlan_id: 100,
                    trunk: false,
                    allowed_vlans: vec![],
                    native_vlan: None,
                }),
                ip_config: IpConfig {
                    mode: IpMode::Dhcp,
                    address: None,
                    netmask: None,
                    gateway: None,
                    dns_servers: vec![],
                    dns_search: vec![],
                    mtu: Some(1500),
                },
                firewall: None,
                qos: None,
                options: HashMap::new(),
            },
            tags: vec!["vlan".to_string()],
            created_at: chrono::Utc::now().timestamp(),
            updated_at: chrono::Utc::now().timestamp(),
        };
        self.create_template(vlan_network)?;

        Ok(())
    }
}

/// Applied network configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedConfig {
    pub instance_id: String,
    pub template_id: String,
    pub bridge: Option<String>,
    pub vlan_id: Option<u16>,
    pub ip_address: Option<String>,
    pub gateway: Option<String>,
    pub dns_servers: Vec<String>,
    pub mtu: Option<u16>,
    pub firewall_enabled: bool,
    pub qos_enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_template() {
        let mut manager = TemplateManager::new();

        let template = NetworkTemplate {
            id: "test1".to_string(),
            name: "Test Template".to_string(),
            description: "Test".to_string(),
            template_type: TemplateType::Bridge,
            config: NetworkConfig {
                bridge: Some("vmbr0".to_string()),
                vlan: None,
                ip_config: IpConfig {
                    mode: IpMode::Dhcp,
                    address: None,
                    netmask: None,
                    gateway: None,
                    dns_servers: vec![],
                    dns_search: vec![],
                    mtu: Some(1500),
                },
                firewall: None,
                qos: None,
                options: HashMap::new(),
            },
            tags: vec![],
            created_at: 0,
            updated_at: 0,
        };

        assert!(manager.create_template(template).is_ok());
        assert_eq!(manager.list_templates().len(), 1);
    }

    #[test]
    fn test_invalid_vlan() {
        let mut manager = TemplateManager::new();

        let template = NetworkTemplate {
            id: "test1".to_string(),
            name: "Test".to_string(),
            description: "".to_string(),
            template_type: TemplateType::Bridge,
            config: NetworkConfig {
                bridge: Some("vmbr0".to_string()),
                vlan: Some(VlanConfig {
                    vlan_id: 5000, // Invalid
                    trunk: false,
                    allowed_vlans: vec![],
                    native_vlan: None,
                }),
                ip_config: IpConfig {
                    mode: IpMode::Dhcp,
                    address: None,
                    netmask: None,
                    gateway: None,
                    dns_servers: vec![],
                    dns_search: vec![],
                    mtu: Some(1500),
                },
                firewall: None,
                qos: None,
                options: HashMap::new(),
            },
            tags: vec![],
            created_at: 0,
            updated_at: 0,
        };

        assert!(manager.create_template(template).is_err());
    }

    #[test]
    fn test_static_ip_validation() {
        let mut manager = TemplateManager::new();

        let template = NetworkTemplate {
            id: "test1".to_string(),
            name: "Test".to_string(),
            description: "".to_string(),
            template_type: TemplateType::Bridge,
            config: NetworkConfig {
                bridge: Some("vmbr0".to_string()),
                vlan: None,
                ip_config: IpConfig {
                    mode: IpMode::Static,
                    address: None, // Missing required address
                    netmask: None,
                    gateway: None,
                    dns_servers: vec![],
                    dns_search: vec![],
                    mtu: Some(1500),
                },
                firewall: None,
                qos: None,
                options: HashMap::new(),
            },
            tags: vec![],
            created_at: 0,
            updated_at: 0,
        };

        assert!(manager.create_template(template).is_err());
    }

    #[test]
    fn test_apply_template() {
        let mut manager = TemplateManager::new();

        let template = NetworkTemplate {
            id: "test1".to_string(),
            name: "Test".to_string(),
            description: "".to_string(),
            template_type: TemplateType::Bridge,
            config: NetworkConfig {
                bridge: Some("vmbr0".to_string()),
                vlan: Some(VlanConfig {
                    vlan_id: 100,
                    trunk: false,
                    allowed_vlans: vec![],
                    native_vlan: None,
                }),
                ip_config: IpConfig {
                    mode: IpMode::Dhcp,
                    address: None,
                    netmask: None,
                    gateway: None,
                    dns_servers: vec!["8.8.8.8".to_string()],
                    dns_search: vec![],
                    mtu: Some(1500),
                },
                firewall: None,
                qos: None,
                options: HashMap::new(),
            },
            tags: vec![],
            created_at: 0,
            updated_at: 0,
        };

        manager.create_template(template).unwrap();

        let config = manager.apply_template("test1", "vm-100");
        assert!(config.is_ok());

        let applied = config.unwrap();
        assert_eq!(applied.instance_id, "vm-100");
        assert_eq!(applied.vlan_id, Some(100));
        assert_eq!(applied.dns_servers, vec!["8.8.8.8".to_string()]);
    }

    #[test]
    fn test_default_templates() {
        let mut manager = TemplateManager::new();
        assert!(manager.create_default_templates().is_ok());
        assert!(manager.list_templates().len() >= 2);
    }
}
