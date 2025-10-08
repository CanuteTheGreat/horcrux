///! Cloud-init integration for automated VM provisioning
///! Generates cloud-init ISO images with user-data and meta-data

use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Cloud-init configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudInitConfig {
    pub hostname: String,
    pub user: Option<UserConfig>,
    pub ssh_keys: Vec<String>,
    pub network: Option<NetworkConfig>,
    pub packages: Vec<String>,
    pub runcmd: Vec<String>,
}

/// User configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConfig {
    pub name: String,
    pub password: Option<String>,  // Hashed password
    pub plain_password: Option<String>,  // Will be hashed
    pub sudo: bool,
    pub shell: Option<String>,
}

/// Network configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub version: u8,  // Version 1 or 2
    pub ethernets: Vec<EthernetConfig>,
}

/// Ethernet interface configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EthernetConfig {
    pub name: String,  // e.g., "eth0"
    pub dhcp4: bool,
    pub addresses: Vec<String>,  // CIDR notation
    pub gateway4: Option<String>,
    pub nameservers: Vec<String>,
}

/// Cloud-init manager
pub struct CloudInitManager {
    iso_dir: PathBuf,
}

impl CloudInitManager {
    pub fn new(iso_dir: PathBuf) -> Self {
        Self { iso_dir }
    }

    /// Generate cloud-init ISO image
    pub async fn generate_iso(&self, vm_id: &str, config: &CloudInitConfig) -> Result<PathBuf> {
        tracing::info!("Generating cloud-init ISO for VM {}", vm_id);

        // Create temporary directory for cloud-init files
        let temp_dir = self.iso_dir.join(format!("cloudinit-{}", vm_id));
        tokio::fs::create_dir_all(&temp_dir).await?;

        // Generate meta-data
        let meta_data = self.generate_meta_data(&config.hostname);
        let meta_data_path = temp_dir.join("meta-data");
        tokio::fs::write(&meta_data_path, meta_data).await?;

        // Generate user-data
        let user_data = self.generate_user_data(config).await?;
        let user_data_path = temp_dir.join("user-data");
        tokio::fs::write(&user_data_path, user_data).await?;

        // Generate network-config if provided
        if let Some(network) = &config.network {
            let network_config = self.generate_network_config(network);
            let network_config_path = temp_dir.join("network-config");
            tokio::fs::write(&network_config_path, network_config).await?;
        }

        // Create ISO image using genisoimage or mkisofs
        let iso_path = self.iso_dir.join(format!("cloudinit-{}.iso", vm_id));
        self.create_iso(&temp_dir, &iso_path).await?;

        // Clean up temporary directory
        tokio::fs::remove_dir_all(&temp_dir).await.ok();

        tracing::info!("Cloud-init ISO created: {}", iso_path.display());
        Ok(iso_path)
    }

    /// Generate meta-data file
    fn generate_meta_data(&self, hostname: &str) -> String {
        format!(
            "instance-id: {}\nlocal-hostname: {}\n",
            uuid::Uuid::new_v4(),
            hostname
        )
    }

    /// Generate user-data file (cloud-config format)
    async fn generate_user_data(&self, config: &CloudInitConfig) -> Result<String> {
        let mut user_data = String::from("#cloud-config\n");

        // Hostname
        user_data.push_str(&format!("hostname: {}\n", config.hostname));
        user_data.push_str(&format!("fqdn: {}.local\n", config.hostname));
        user_data.push_str("manage_etc_hosts: true\n\n");

        // User configuration
        if let Some(user) = &config.user {
            user_data.push_str("users:\n");
            user_data.push_str("  - name: ");
            user_data.push_str(&user.name);
            user_data.push_str("\n");

            if user.sudo {
                user_data.push_str("    sudo: ALL=(ALL) NOPASSWD:ALL\n");
            }

            user_data.push_str("    groups: sudo\n");

            if let Some(shell) = &user.shell {
                user_data.push_str(&format!("    shell: {}\n", shell));
            } else {
                user_data.push_str("    shell: /bin/bash\n");
            }

            // Password (prefer hashed, but support plain for convenience)
            if let Some(hashed) = &user.password {
                user_data.push_str(&format!("    passwd: {}\n", hashed));
            } else if let Some(plain) = &user.plain_password {
                // Hash the password using SHA-512
                let hashed = self.hash_password(plain).await?;
                user_data.push_str(&format!("    passwd: {}\n", hashed));
            }

            user_data.push_str("    lock_passwd: false\n");
        }

        // SSH keys
        if !config.ssh_keys.is_empty() {
            user_data.push_str("\nssh_authorized_keys:\n");
            for key in &config.ssh_keys {
                user_data.push_str(&format!("  - {}\n", key));
            }
        }

        // Packages to install
        if !config.packages.is_empty() {
            user_data.push_str("\npackages:\n");
            for package in &config.packages {
                user_data.push_str(&format!("  - {}\n", package));
            }
        }

        // Commands to run
        if !config.runcmd.is_empty() {
            user_data.push_str("\nruncmd:\n");
            for cmd in &config.runcmd {
                user_data.push_str(&format!("  - {}\n", cmd));
            }
        }

        // SSH configuration
        user_data.push_str("\nssh_pwauth: true\n");
        user_data.push_str("disable_root: false\n");
        user_data.push_str("package_update: true\n");
        user_data.push_str("package_upgrade: true\n");

        Ok(user_data)
    }

    /// Generate network-config file (Netplan format)
    fn generate_network_config(&self, network: &NetworkConfig) -> String {
        let mut config = format!("version: {}\n", network.version);

        if !network.ethernets.is_empty() {
            config.push_str("ethernets:\n");

            for eth in &network.ethernets {
                config.push_str(&format!("  {}:\n", eth.name));

                if eth.dhcp4 {
                    config.push_str("    dhcp4: true\n");
                } else {
                    config.push_str("    dhcp4: false\n");

                    if !eth.addresses.is_empty() {
                        config.push_str("    addresses:\n");
                        for addr in &eth.addresses {
                            config.push_str(&format!("      - {}\n", addr));
                        }
                    }

                    if let Some(gateway) = &eth.gateway4 {
                        config.push_str(&format!("    gateway4: {}\n", gateway));
                    }

                    if !eth.nameservers.is_empty() {
                        config.push_str("    nameservers:\n");
                        config.push_str("      addresses:\n");
                        for ns in &eth.nameservers {
                            config.push_str(&format!("        - {}\n", ns));
                        }
                    }
                }
            }
        }

        config
    }

    /// Hash password using SHA-512
    async fn hash_password(&self, password: &str) -> Result<String> {
        // Use mkpasswd or openssl to hash the password
        let output = tokio::process::Command::new("mkpasswd")
            .arg("--method=SHA-512")
            .arg(password)
            .output()
            .await;

        match output {
            Ok(out) if out.status.success() => {
                let hashed = String::from_utf8_lossy(&out.stdout).trim().to_string();
                Ok(hashed)
            }
            _ => {
                // Fallback to openssl if mkpasswd not available
                let output = tokio::process::Command::new("openssl")
                    .arg("passwd")
                    .arg("-6")
                    .arg(password)
                    .output()
                    .await?;

                if output.status.success() {
                    let hashed = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    Ok(hashed)
                } else {
                    Err(horcrux_common::Error::System(
                        "Failed to hash password. Install mkpasswd or openssl.".to_string(),
                    ))
                }
            }
        }
    }

    /// Create ISO image from directory
    async fn create_iso(&self, source_dir: &PathBuf, output_iso: &PathBuf) -> Result<()> {
        // Try genisoimage first, then mkisofs, then xorriso
        let commands = vec!["genisoimage", "mkisofs", "xorriso"];

        for cmd in commands {
            let result = if cmd == "xorriso" {
                tokio::process::Command::new(cmd)
                    .arg("-as")
                    .arg("mkisofs")
                    .arg("-output")
                    .arg(output_iso)
                    .arg("-volid")
                    .arg("cidata")
                    .arg("-joliet")
                    .arg("-rock")
                    .arg(source_dir)
                    .output()
                    .await
            } else {
                tokio::process::Command::new(cmd)
                    .arg("-output")
                    .arg(output_iso)
                    .arg("-volid")
                    .arg("cidata")
                    .arg("-joliet")
                    .arg("-rock")
                    .arg(source_dir)
                    .output()
                    .await
            };

            match result {
                Ok(output) if output.status.success() => {
                    return Ok(());
                }
                Ok(_) => continue,
                Err(_) => continue,
            }
        }

        Err(horcrux_common::Error::System(
            "Failed to create ISO. Install genisoimage, mkisofs, or xorriso.".to_string(),
        ))
    }

    /// Delete cloud-init ISO
    pub async fn delete_iso(&self, vm_id: &str) -> Result<()> {
        let iso_path = self.iso_dir.join(format!("cloudinit-{}.iso", vm_id));
        if iso_path.exists() {
            tokio::fs::remove_file(&iso_path).await?;
            tracing::info!("Deleted cloud-init ISO: {}", iso_path.display());
        }
        Ok(())
    }

    /// Get ISO path for VM
    pub fn get_iso_path(&self, vm_id: &str) -> PathBuf {
        self.iso_dir.join(format!("cloudinit-{}.iso", vm_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_generate_meta_data() {
        let manager = CloudInitManager::new(PathBuf::from("/tmp/cloudinit"));
        let meta_data = manager.generate_meta_data("test-vm");

        assert!(meta_data.contains("local-hostname: test-vm"));
        assert!(meta_data.contains("instance-id:"));
    }

    #[tokio::test]
    async fn test_generate_user_data() {
        let manager = CloudInitManager::new(PathBuf::from("/tmp/cloudinit"));

        let config = CloudInitConfig {
            hostname: "test-vm".to_string(),
            user: Some(UserConfig {
                name: "ubuntu".to_string(),
                password: None,
                plain_password: None,
                sudo: true,
                shell: Some("/bin/bash".to_string()),
            }),
            ssh_keys: vec!["ssh-rsa AAAA...".to_string()],
            network: None,
            packages: vec!["curl".to_string(), "vim".to_string()],
            runcmd: vec!["echo 'Hello World'".to_string()],
        };

        let user_data = manager.generate_user_data(&config).await.unwrap();

        assert!(user_data.contains("#cloud-config"));
        assert!(user_data.contains("hostname: test-vm"));
        assert!(user_data.contains("name: ubuntu"));
        assert!(user_data.contains("sudo: ALL=(ALL) NOPASSWD:ALL"));
        assert!(user_data.contains("ssh-rsa AAAA..."));
        assert!(user_data.contains("- curl"));
        assert!(user_data.contains("- vim"));
    }

    #[test]
    fn test_generate_network_config() {
        let manager = CloudInitManager::new(PathBuf::from("/tmp/cloudinit"));

        let network = NetworkConfig {
            version: 2,
            ethernets: vec![EthernetConfig {
                name: "eth0".to_string(),
                dhcp4: false,
                addresses: vec!["192.168.1.100/24".to_string()],
                gateway4: Some("192.168.1.1".to_string()),
                nameservers: vec!["8.8.8.8".to_string(), "8.8.4.4".to_string()],
            }],
        };

        let config = manager.generate_network_config(&network);

        assert!(config.contains("version: 2"));
        assert!(config.contains("eth0:"));
        assert!(config.contains("dhcp4: false"));
        assert!(config.contains("192.168.1.100/24"));
        assert!(config.contains("gateway4: 192.168.1.1"));
        assert!(config.contains("8.8.8.8"));
    }
}
