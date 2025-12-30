//! AFP/Netatalk file sharing module
//!
//! Manages Netatalk configuration for macOS file sharing and Time Machine support.

use horcrux_common::{Error, Result};
use crate::nas::shares::{AfpShareConfig, NasShare};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::process::Command;

/// AFP global configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AfpGlobalConfig {
    /// Hostname (optional, defaults to system hostname)
    pub hostname: Option<String>,
    /// UAM list (authentication methods)
    pub uam_list: String,
    /// Allow password saving
    pub save_password: bool,
    /// Allow password changes
    pub set_password: bool,
    /// Mimic Mac model for Finder
    pub mimic_model: String,
    /// Log level
    pub log_level: String,
    /// Enable Zeroconf/Bonjour
    pub zeroconf: bool,
    /// Enable Spotlight search
    pub spotlight: bool,
    /// FCE listener address
    pub fce_listener: Option<String>,
    /// FCE coalesce time
    pub fce_coalesce: Option<u32>,
    /// Extra parameters
    pub extra_parameters: HashMap<String, String>,
}

impl Default for AfpGlobalConfig {
    fn default() -> Self {
        Self {
            hostname: None,
            uam_list: "uams_dhx2.so".to_string(),
            save_password: true,
            set_password: false,
            mimic_model: "RackMac".to_string(),
            log_level: "default:warn".to_string(),
            zeroconf: true,
            spotlight: false,
            fce_listener: None,
            fce_coalesce: None,
            extra_parameters: HashMap::new(),
        }
    }
}

/// AFP share for Time Machine support
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AfpShare {
    /// Share name
    pub name: String,
    /// Share path
    pub path: String,
    /// Time Machine enabled
    pub time_machine: bool,
    /// Volume size limit (MB)
    pub volume_size_limit: Option<u64>,
}

/// AFP Manager for Netatalk
pub struct AfpManager {
    config_path: String,
    global_config: AfpGlobalConfig,
    shares: Vec<AfpShare>,
}

impl AfpManager {
    /// Create a new AFP manager
    pub fn new() -> Self {
        Self {
            config_path: "/etc/netatalk/afp.conf".to_string(),
            global_config: AfpGlobalConfig::default(),
            shares: Vec::new(),
        }
    }

    /// Get global configuration
    pub fn config(&self) -> &AfpGlobalConfig {
        &self.global_config
    }

    /// Get configured shares
    pub fn shares(&self) -> &[AfpShare] {
        &self.shares
    }

    /// Disconnect a session by PID
    pub async fn disconnect_session(&self, pid: u32) -> Result<()> {
        // Send SIGTERM to the AFP session process
        let output = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to kill AFP session: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to terminate AFP session {}: {}", pid, stderr)));
        }

        Ok(())
    }

    /// Add a share to AFP configuration
    pub async fn add_share(&self, share: &NasShare) -> Result<()> {
        let config = self.read_config().await?;
        let share_section = self.generate_share_section(share);

        let mut new_config = config;
        new_config.push_str(&share_section);

        self.write_config(&new_config).await?;
        self.reload().await?;

        Ok(())
    }

    /// Remove a share from AFP configuration
    pub async fn remove_share(&self, share: &NasShare) -> Result<()> {
        let config = self.read_config().await?;
        let new_config = self.remove_share_section(&config, &share.name);

        self.write_config(&new_config).await?;
        self.reload().await?;

        Ok(())
    }

    /// Generate complete afp.conf
    pub fn generate_config(&self, shares: &[NasShare]) -> String {
        let mut config = String::new();

        config.push_str(&self.generate_global_section());
        config.push_str("\n");

        // Time Machine preset
        config.push_str("; Preset for Time Machine volumes\n");
        config.push_str("[TimeMachine]\n");
        config.push_str("   time machine = yes\n\n");

        for share in shares {
            if share.enabled && share.afp_config.is_some() {
                config.push_str(&self.generate_share_section(share));
            }
        }

        config
    }

    /// Generate global section
    fn generate_global_section(&self) -> String {
        let mut section = String::new();
        let g = &self.global_config;

        section.push_str("[Global]\n");

        // Use system hostname if not specified
        let hostname = g.hostname.clone().unwrap_or_else(|| {
            hostname::get()
                .map(|h| h.to_string_lossy().to_string())
                .unwrap_or_else(|_| "horcrux".to_string())
        });
        section.push_str(&format!("   hostname = {}\n", hostname));

        section.push_str(&format!("   mimic model = {}\n", g.mimic_model));
        section.push_str(&format!("   uam list = {}\n", g.uam_list));
        section.push_str(&format!("   save password = {}\n", if g.save_password { "yes" } else { "no" }));
        section.push_str(&format!("   set password = {}\n", if g.set_password { "yes" } else { "no" }));
        section.push_str(&format!("   zeroconf = {}\n", if g.zeroconf { "yes" } else { "no" }));
        section.push_str(&format!("   spotlight = {}\n", if g.spotlight { "yes" } else { "no" }));
        section.push_str(&format!("   log level = {}\n", g.log_level));

        if let Some(ref listener) = g.fce_listener {
            section.push_str(&format!("   fce listener = {}\n", listener));
        }

        if let Some(coalesce) = g.fce_coalesce {
            section.push_str(&format!("   fce coalesce = {}\n", coalesce));
        }

        // Add extra parameters
        for (key, value) in &g.extra_parameters {
            section.push_str(&format!("   {} = {}\n", key, value));
        }

        section
    }

    /// Generate share section
    fn generate_share_section(&self, share: &NasShare) -> String {
        let mut section = String::new();
        let config = share.afp_config.as_ref().cloned().unwrap_or_default();

        section.push_str(&format!("\n[{}]\n", share.name));
        section.push_str(&format!("   path = {}\n", share.path));

        if config.time_machine {
            section.push_str("   time machine = yes\n");
            if let Some(quota) = config.time_machine_quota_gb {
                // Convert GB to bytes
                let quota_bytes = quota * 1024 * 1024 * 1024;
                section.push_str(&format!("   vol size limit = {}\n", quota_bytes / 1024 / 1024));
            }
        }

        if !config.valid_users.is_empty() {
            section.push_str(&format!("   valid users = {}\n", config.valid_users.join(" ")));
        }

        if !config.rolist.is_empty() {
            section.push_str(&format!("   rolist = {}\n", config.rolist.join(" ")));
        }

        if !config.rwlist.is_empty() {
            section.push_str(&format!("   rwlist = {}\n", config.rwlist.join(" ")));
        }

        section
    }

    /// Remove share section
    fn remove_share_section(&self, config: &str, share_name: &str) -> String {
        let mut result = String::new();
        let mut skip_section = false;
        let section_header = format!("[{}]", share_name);

        for line in config.lines() {
            let trimmed = line.trim();

            if trimmed.eq_ignore_ascii_case(&section_header) {
                skip_section = true;
                continue;
            }

            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                skip_section = false;
            }

            if !skip_section {
                result.push_str(line);
                result.push('\n');
            }
        }

        result
    }

    /// Read config file
    async fn read_config(&self) -> Result<String> {
        match tokio::fs::read_to_string(&self.config_path).await {
            Ok(content) => Ok(content),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(Error::Internal(format!(
                "Failed to read afp.conf: {}",
                e
            ))),
        }
    }

    /// Write config file
    async fn write_config(&self, content: &str) -> Result<()> {
        tokio::fs::write(&self.config_path, content)
            .await
            .map_err(|e| Error::Internal(format!("Failed to write afp.conf: {}", e)))
    }

    /// Reload Netatalk
    pub async fn reload(&self) -> Result<()> {
        if !self.is_running().await {
            return Ok(());
        }
        // Send SIGHUP to netatalk
        let output = Command::new("pkill")
            .args(["-HUP", "netatalk"])
            .output()
            .await;

        match output {
            Ok(out) if out.status.success() => Ok(()),
            Ok(_) => Ok(()), // netatalk might not be running
            Err(e) => Err(Error::Internal(format!("Failed to reload Netatalk: {}", e))),
        }
    }

    /// Start Netatalk
    pub async fn start(&self) -> Result<()> {
        crate::nas::services::manage_service(
            &crate::nas::services::NasService::Netatalk,
            crate::nas::services::ServiceAction::Start,
        )
        .await
    }

    /// Stop Netatalk
    pub async fn stop(&self) -> Result<()> {
        crate::nas::services::manage_service(
            &crate::nas::services::NasService::Netatalk,
            crate::nas::services::ServiceAction::Stop,
        )
        .await
    }

    /// Check if Netatalk is running
    pub async fn is_running(&self) -> bool {
        let output = Command::new("pgrep")
            .arg("netatalk")
            .output()
            .await;

        match output {
            Ok(out) => out.status.success(),
            Err(_) => false,
        }
    }

    /// Get Netatalk status
    pub async fn get_status(&self) -> Result<AfpStatus> {
        let running = self.is_running().await;
        let config_exists = tokio::fs::metadata(&self.config_path).await.is_ok();

        Ok(AfpStatus {
            running,
            config_path: self.config_path.clone(),
            config_exists,
            hostname: self.global_config.hostname.clone(),
            spotlight_enabled: self.global_config.spotlight,
        })
    }

    /// Set global configuration
    pub fn set_global_config(&mut self, config: AfpGlobalConfig) {
        self.global_config = config;
    }
}

/// AFP service status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AfpStatus {
    pub running: bool,
    pub config_path: String,
    pub config_exists: bool,
    pub hostname: String,
    pub spotlight_enabled: bool,
}

impl Default for AfpManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AfpManager {
    /// Write complete configuration file
    pub async fn write_full_config(&self, shares: &[NasShare]) -> Result<()> {
        let config = self.generate_config(shares);

        // Backup existing config
        if tokio::fs::metadata(&self.config_path).await.is_ok() {
            let backup_path = format!("{}.bak", self.config_path);
            let _ = tokio::fs::copy(&self.config_path, &backup_path).await;
        }

        // Ensure directory exists
        if let Some(parent) = std::path::Path::new(&self.config_path).parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }

        self.write_config(&config).await?;
        self.reload().await
    }

    /// Get connected AFP clients
    pub async fn get_connections(&self) -> Result<Vec<AfpConnection>> {
        let output = Command::new("afpstats")
            .output()
            .await;

        if let Ok(out) = output {
            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                return Ok(Self::parse_afpstats(&stdout));
            }
        }

        // Fallback: try macusers
        let output = Command::new("macusers")
            .output()
            .await;

        if let Ok(out) = output {
            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                return Ok(Self::parse_macusers(&stdout));
            }
        }

        Ok(Vec::new())
    }

    fn parse_afpstats(output: &str) -> Vec<AfpConnection> {
        let mut connections = Vec::new();

        for line in output.lines().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4 {
                connections.push(AfpConnection {
                    pid: parts.first().and_then(|s| s.parse().ok()).unwrap_or(0),
                    username: parts.get(1).unwrap_or(&"").to_string(),
                    volume: parts.get(2).unwrap_or(&"").to_string(),
                    client_ip: parts.get(3).unwrap_or(&"").to_string(),
                    connected_at: None,
                });
            }
        }

        connections
    }

    fn parse_macusers(output: &str) -> Vec<AfpConnection> {
        let mut connections = Vec::new();

        for line in output.lines().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 {
                connections.push(AfpConnection {
                    pid: parts.first().and_then(|s| s.parse().ok()).unwrap_or(0),
                    username: parts.get(1).unwrap_or(&"").to_string(),
                    volume: parts.get(2).unwrap_or(&"").to_string(),
                    client_ip: String::new(),
                    connected_at: None,
                });
            }
        }

        connections
    }

    /// Disconnect a specific client
    pub async fn disconnect_client(&self, pid: u32) -> Result<()> {
        let output = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to disconnect client: {}", e)))?;

        if !output.status.success() {
            return Err(Error::Internal("Failed to disconnect client".to_string()));
        }

        Ok(())
    }

    /// Configure Bonjour/mDNS advertisement
    pub async fn configure_bonjour(&self, enabled: bool, name: Option<&str>) -> Result<()> {
        if enabled {
            // Register with Avahi/mDNSResponder
            let service_name = name.unwrap_or(&self.global_config.hostname);
            let avahi_service = format!(r#"<?xml version="1.0" standalone='no'?>
<!DOCTYPE service-group SYSTEM "avahi-service.dtd">
<service-group>
  <name>{}</name>
  <service>
    <type>_afpovertcp._tcp</type>
    <port>548</port>
  </service>
  <service>
    <type>_device-info._tcp</type>
    <port>0</port>
    <txt-record>model=Xserve</txt-record>
  </service>
</service-group>
"#, service_name);

            let avahi_path = "/etc/avahi/services/afpd.service";
            let _ = tokio::fs::write(avahi_path, avahi_service).await;

            // Reload avahi
            let _ = Command::new("systemctl")
                .args(["reload", "avahi-daemon"])
                .output()
                .await;
        } else {
            // Remove service file
            let _ = tokio::fs::remove_file("/etc/avahi/services/afpd.service").await;
        }

        Ok(())
    }

    /// Configure Time Machine advertisement for a share
    pub async fn configure_time_machine_bonjour(&self, share_name: &str, enabled: bool) -> Result<()> {
        if enabled {
            let avahi_service = format!(r#"<?xml version="1.0" standalone='no'?>
<!DOCTYPE service-group SYSTEM "avahi-service.dtd">
<service-group>
  <name>{}</name>
  <service>
    <type>_adisk._tcp</type>
    <port>9</port>
    <txt-record>sys=waMA=00:00:00:00:00:00,adVF=0x100</txt-record>
    <txt-record>dk0=adVN={},adVF=0x82</txt-record>
  </service>
  <service>
    <type>_afpovertcp._tcp</type>
    <port>548</port>
  </service>
</service-group>
"#, share_name, share_name);

            let avahi_path = format!("/etc/avahi/services/timemachine-{}.service", share_name);
            let _ = tokio::fs::write(&avahi_path, avahi_service).await;

            let _ = Command::new("systemctl")
                .args(["reload", "avahi-daemon"])
                .output()
                .await;
        } else {
            let avahi_path = format!("/etc/avahi/services/timemachine-{}.service", share_name);
            let _ = tokio::fs::remove_file(&avahi_path).await;
        }

        Ok(())
    }

    /// Get Time Machine backup status for a volume
    pub async fn get_time_machine_status(&self, volume_path: &str) -> Result<TimeMachineStatus> {
        // Check if sparse bundle exists
        let sparse_bundles = Self::find_sparse_bundles(volume_path).await?;

        let mut total_size_bytes = 0u64;
        let mut backups = Vec::new();

        for bundle in sparse_bundles {
            // Get bundle size
            let output = Command::new("du")
                .args(["-sb", &bundle])
                .output()
                .await;

            if let Ok(out) = output {
                if out.status.success() {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    if let Some(size_str) = stdout.split_whitespace().next() {
                        if let Ok(size) = size_str.parse::<u64>() {
                            total_size_bytes += size;
                        }
                    }
                }
            }

            // Extract client name from bundle name
            let bundle_name = std::path::Path::new(&bundle)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");

            backups.push(TimeMachineBackup {
                bundle_path: bundle,
                client_name: bundle_name.to_string(),
                last_backup: None,
            });
        }

        Ok(TimeMachineStatus {
            volume_path: volume_path.to_string(),
            total_size_bytes,
            backups,
        })
    }

    async fn find_sparse_bundles(path: &str) -> Result<Vec<String>> {
        let output = Command::new("find")
            .args([path, "-maxdepth", "2", "-name", "*.sparsebundle", "-type", "d"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to find sparse bundles: {}", e)))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.lines().map(|s| s.to_string()).collect())
    }

    /// Clean up old Time Machine backups
    pub async fn cleanup_time_machine_backups(&self, volume_path: &str, max_age_days: u32) -> Result<u32> {
        let output = Command::new("find")
            .args([
                volume_path,
                "-maxdepth", "2",
                "-name", "*.sparsebundle",
                "-type", "d",
                "-mtime", &format!("+{}", max_age_days),
            ])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to find old backups: {}", e)))?;

        if !output.status.success() {
            return Ok(0);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let old_bundles: Vec<&str> = stdout.lines().collect();
        let count = old_bundles.len() as u32;

        for bundle in old_bundles {
            let _ = tokio::fs::remove_dir_all(bundle).await;
        }

        Ok(count)
    }

    /// Get Netatalk version
    pub async fn get_version(&self) -> Result<String> {
        let output = Command::new("netatalk")
            .args(["-V"])
            .output()
            .await;

        if let Ok(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout);
            for line in stdout.lines() {
                if line.contains("netatalk") {
                    return Ok(line.trim().to_string());
                }
            }
        }

        // Try afpd
        let output = Command::new("afpd")
            .args(["-V"])
            .output()
            .await;

        if let Ok(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout);
            return Ok(stdout.lines().next().unwrap_or("unknown").to_string());
        }

        Ok("unknown".to_string())
    }

    /// Restart Netatalk service
    pub async fn restart(&self) -> Result<()> {
        self.stop().await?;
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        self.start().await
    }

    /// Configure Spotlight indexing for a volume
    pub async fn configure_spotlight(&self, volume_path: &str, enabled: bool) -> Result<()> {
        if enabled {
            // Start spotlight indexer for this path
            let _ = Command::new("dbd")
                .args(["-r", volume_path])
                .output()
                .await;
        } else {
            // Remove spotlight index
            let index_path = format!("{}/.AppleDB", volume_path);
            let _ = tokio::fs::remove_dir_all(&index_path).await;
        }

        Ok(())
    }

    /// Parse current configuration file
    pub async fn parse_config(&self) -> Result<Vec<ParsedAfpShare>> {
        let content = self.read_config().await?;
        let mut shares = Vec::new();
        let mut current_share: Option<ParsedAfpShare> = None;

        for line in content.lines() {
            let line = line.trim();

            // Skip comments
            if line.starts_with(';') || line.starts_with('#') {
                continue;
            }

            // Check for section header
            if line.starts_with('[') && line.ends_with(']') {
                // Save previous share
                if let Some(share) = current_share.take() {
                    if share.name != "Global" && share.name != "TimeMachine" {
                        shares.push(share);
                    }
                }

                let name = &line[1..line.len()-1];
                current_share = Some(ParsedAfpShare {
                    name: name.to_string(),
                    path: String::new(),
                    time_machine: false,
                    valid_users: Vec::new(),
                    options: std::collections::HashMap::new(),
                });
                continue;
            }

            // Parse options
            if let Some(ref mut share) = current_share {
                if let Some((key, value)) = line.split_once('=') {
                    let key = key.trim().to_lowercase();
                    let value = value.trim().to_string();

                    match key.as_str() {
                        "path" => share.path = value,
                        "time machine" => share.time_machine = value == "yes",
                        "valid users" => {
                            share.valid_users = value.split_whitespace()
                                .map(|s| s.to_string())
                                .collect();
                        }
                        _ => {
                            share.options.insert(key, value);
                        }
                    }
                }
            }
        }

        // Save last share
        if let Some(share) = current_share {
            if share.name != "Global" && share.name != "TimeMachine" {
                shares.push(share);
            }
        }

        Ok(shares)
    }

    /// Enable AFP globally
    pub async fn enable(&self) -> Result<()> {
        // Enable service
        let _ = Command::new("systemctl")
            .args(["enable", "netatalk"])
            .output()
            .await;

        let _ = Command::new("rc-update")
            .args(["add", "netatalk", "default"])
            .output()
            .await;

        self.start().await
    }

    /// Disable AFP globally
    pub async fn disable(&self) -> Result<()> {
        self.stop().await?;

        // Disable service
        let _ = Command::new("systemctl")
            .args(["disable", "netatalk"])
            .output()
            .await;

        let _ = Command::new("rc-update")
            .args(["del", "netatalk"])
            .output()
            .await;

        Ok(())
    }
}

/// AFP connection info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AfpConnection {
    pub pid: u32,
    pub username: String,
    pub volume: String,
    pub client_ip: String,
    pub connected_at: Option<i64>,
}

/// Time Machine backup status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeMachineStatus {
    pub volume_path: String,
    pub total_size_bytes: u64,
    pub backups: Vec<TimeMachineBackup>,
}

/// Individual Time Machine backup
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeMachineBackup {
    pub bundle_path: String,
    pub client_name: String,
    pub last_backup: Option<i64>,
}

/// Parsed AFP share from config
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedAfpShare {
    pub name: String,
    pub path: String,
    pub time_machine: bool,
    pub valid_users: Vec<String>,
    pub options: std::collections::HashMap<String, String>,
}
