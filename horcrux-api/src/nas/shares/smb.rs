//! SMB/Samba file sharing module
//!
//! Manages Samba configuration and shares for Windows/cross-platform file sharing.

use horcrux_common::{Error, Result};
use crate::nas::shares::{NasShare, SmbShareConfig};
use crate::nas::CaseSensitivity;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::process::Command;

/// SMB global configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmbGlobalConfig {
    /// Workgroup name
    pub workgroup: String,
    /// Server description
    pub server_string: String,
    /// NetBIOS name
    pub netbios_name: Option<String>,
    /// Security mode (user, share, domain, ads)
    pub security: String,
    /// Map to guest account
    pub map_to_guest: String,
    /// Log level (0-10)
    pub log_level: u8,
    /// Enable macOS optimizations (fruit VFS)
    pub fruit_enabled: bool,
    /// Enable Spotlight search
    pub spotlight_enabled: bool,
    /// Minimum SMB protocol version
    pub min_protocol: String,
    /// Maximum SMB protocol version
    pub max_protocol: String,
    /// Enable local master browser
    pub local_master: bool,
    /// Enable domain master browser
    pub domain_master: bool,
    /// Enable WINS support
    pub wins_support: bool,
    /// Extra global parameters
    pub extra_parameters: HashMap<String, String>,
}

impl Default for SmbGlobalConfig {
    fn default() -> Self {
        Self {
            workgroup: "WORKGROUP".to_string(),
            server_string: "Horcrux NAS Server".to_string(),
            netbios_name: None,
            security: "user".to_string(),
            map_to_guest: "Bad User".to_string(),
            log_level: 1,
            fruit_enabled: true,
            spotlight_enabled: false,
            min_protocol: "SMB2".to_string(),
            max_protocol: "SMB3".to_string(),
            local_master: true,
            domain_master: false,
            wins_support: false,
            extra_parameters: HashMap::new(),
        }
    }
}

/// SMB connection info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmbConnection {
    /// Process ID
    pub pid: u32,
    /// Username
    pub username: String,
    /// Connected share
    pub share: String,
    /// Machine name/IP
    pub machine: String,
    /// Protocol version
    pub protocol: String,
    /// Connected since
    pub connected_at: i64,
}

/// SMB open file/lock info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmbLock {
    /// Process ID
    pub pid: u32,
    /// Username
    pub username: String,
    /// Share name
    pub share: String,
    /// File path
    pub path: String,
    /// Lock type (R, W, RW)
    pub lock_type: String,
    /// Oplock type
    pub oplock: String,
}

/// SMB Manager
pub struct SmbManager {
    config_path: String,
    global_config: SmbGlobalConfig,
}

impl SmbManager {
    /// Create a new SMB manager
    pub fn new() -> Self {
        Self {
            config_path: "/etc/samba/smb.conf".to_string(),
            global_config: SmbGlobalConfig::default(),
        }
    }

    /// Set global configuration
    pub fn set_global_config(&mut self, config: SmbGlobalConfig) {
        self.global_config = config;
    }

    /// Get global configuration
    pub fn get_global_config(&self) -> &SmbGlobalConfig {
        &self.global_config
    }

    /// Add a share to Samba configuration
    pub async fn add_share(&self, share: &NasShare) -> Result<()> {
        // Read current config, add share, write back
        let mut config = self.read_config().await?;

        // Generate share section
        let share_config = self.generate_share_section(share);
        config.push_str(&share_config);

        self.write_config(&config).await?;
        self.reload().await?;

        Ok(())
    }

    /// Remove a share from Samba configuration
    pub async fn remove_share(&self, share: &NasShare) -> Result<()> {
        let config = self.read_config().await?;

        // Remove the share section
        let new_config = self.remove_share_section(&config, &share.name);

        self.write_config(&new_config).await?;
        self.reload().await?;

        Ok(())
    }

    /// Generate complete smb.conf from shares
    pub fn generate_config(&self, shares: &[NasShare]) -> String {
        let mut config = String::new();

        // Global section
        config.push_str(&self.generate_global_section());

        // Share sections
        for share in shares {
            if share.enabled {
                config.push_str(&self.generate_share_section(share));
            }
        }

        config
    }

    /// Generate global section
    fn generate_global_section(&self) -> String {
        let mut section = String::new();
        let g = &self.global_config;

        section.push_str("[global]\n");
        section.push_str(&format!("   workgroup = {}\n", g.workgroup));
        section.push_str(&format!("   server string = {}\n", g.server_string));

        if let Some(ref netbios) = g.netbios_name {
            section.push_str(&format!("   netbios name = {}\n", netbios));
        }

        section.push_str(&format!("   security = {}\n", g.security));
        section.push_str(&format!("   map to guest = {}\n", g.map_to_guest));
        section.push_str(&format!("   log level = {}\n", g.log_level));

        // Protocol versions
        section.push_str(&format!("   server min protocol = {}\n", g.min_protocol));
        section.push_str(&format!("   server max protocol = {}\n", g.max_protocol));

        // Browsing
        section.push_str(&format!("   local master = {}\n", if g.local_master { "yes" } else { "no" }));
        section.push_str(&format!("   domain master = {}\n", if g.domain_master { "yes" } else { "no" }));
        section.push_str(&format!("   wins support = {}\n", if g.wins_support { "yes" } else { "no" }));

        // Disable printing
        section.push_str("   load printers = no\n");
        section.push_str("   printing = bsd\n");
        section.push_str("   printcap name = /dev/null\n");
        section.push_str("   disable spoolss = yes\n");

        // macOS optimizations
        if g.fruit_enabled {
            section.push_str("\n   # macOS optimizations\n");
            section.push_str("   vfs objects = fruit streams_xattr\n");
            section.push_str("   fruit:metadata = stream\n");
            section.push_str("   fruit:model = MacSamba\n");
            section.push_str("   fruit:posix_rename = yes\n");
            section.push_str("   fruit:veto_appledouble = no\n");
            section.push_str("   fruit:nfs_aces = no\n");
            section.push_str("   fruit:wipe_intentionally_left_blank_rfork = yes\n");
            section.push_str("   fruit:delete_empty_adfiles = yes\n");
        }

        // Spotlight
        if g.spotlight_enabled {
            section.push_str("   spotlight = yes\n");
        }

        // Extra parameters
        for (key, value) in &g.extra_parameters {
            section.push_str(&format!("   {} = {}\n", key, value));
        }

        section.push('\n');
        section
    }

    /// Generate share section
    fn generate_share_section(&self, share: &NasShare) -> String {
        let mut section = String::new();

        section.push_str(&format!("\n[{}]\n", share.name));
        section.push_str(&format!("   path = {}\n", share.path));

        if let Some(ref desc) = share.description {
            section.push_str(&format!("   comment = {}\n", desc));
        }

        // Get SMB-specific config or use defaults
        let smb_config = share.smb_config.as_ref().cloned().unwrap_or_default();

        section.push_str(&format!("   browseable = {}\n", if smb_config.browseable { "yes" } else { "no" }));
        section.push_str(&format!("   read only = {}\n", if smb_config.read_only { "yes" } else { "no" }));
        section.push_str(&format!("   guest ok = {}\n", if smb_config.guest_ok { "yes" } else { "no" }));

        // Valid users
        if !smb_config.valid_users.is_empty() {
            section.push_str(&format!("   valid users = {}\n", smb_config.valid_users.join(" ")));
        }

        // Valid groups (prefix with @)
        if !smb_config.valid_groups.is_empty() {
            let groups: Vec<String> = smb_config.valid_groups.iter()
                .map(|g| format!("@{}", g))
                .collect();
            section.push_str(&format!("   valid users = {}\n", groups.join(" ")));
        }

        // Hosts allow/deny
        if !smb_config.hosts_allow.is_empty() {
            section.push_str(&format!("   hosts allow = {}\n", smb_config.hosts_allow.join(" ")));
        }
        if !smb_config.hosts_deny.is_empty() {
            section.push_str(&format!("   hosts deny = {}\n", smb_config.hosts_deny.join(" ")));
        }

        // VFS objects
        if !smb_config.vfs_objects.is_empty() {
            section.push_str(&format!("   vfs objects = {}\n", smb_config.vfs_objects.join(" ")));
        }

        // Recycle bin
        if smb_config.recycle_bin {
            section.push_str("   vfs objects = recycle\n");
            section.push_str("   recycle:repository = .recycle\n");
            section.push_str("   recycle:keeptree = yes\n");
            section.push_str("   recycle:versions = yes\n");
        }

        // Audit logging
        if smb_config.audit_logging {
            section.push_str("   vfs objects = full_audit\n");
            section.push_str("   full_audit:prefix = %u|%I|%m|%S\n");
            section.push_str("   full_audit:success = mkdir rmdir read write rename unlink\n");
            section.push_str("   full_audit:failure = none\n");
            section.push_str("   full_audit:facility = local5\n");
            section.push_str("   full_audit:priority = notice\n");
        }

        // Oplocks
        section.push_str(&format!("   oplocks = {}\n", if smb_config.oplocks { "yes" } else { "no" }));

        // Case sensitivity
        match smb_config.case_sensitive {
            CaseSensitivity::Sensitive => {
                section.push_str("   case sensitive = yes\n");
            }
            CaseSensitivity::Insensitive => {
                section.push_str("   case sensitive = no\n");
            }
            CaseSensitivity::Auto => {
                section.push_str("   case sensitive = auto\n");
            }
        }

        // Extra parameters
        for (key, value) in &smb_config.extra_parameters {
            section.push_str(&format!("   {} = {}\n", key, value));
        }

        section
    }

    /// Remove a share section from config
    fn remove_share_section(&self, config: &str, share_name: &str) -> String {
        let mut result = String::new();
        let mut skip_section = false;
        let section_header = format!("[{}]", share_name);

        for line in config.lines() {
            let trimmed = line.trim();

            // Check if this is the section we want to remove
            if trimmed.eq_ignore_ascii_case(&section_header) {
                skip_section = true;
                continue;
            }

            // Check if we've hit a new section
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

    /// Read current config file
    async fn read_config(&self) -> Result<String> {
        tokio::fs::read_to_string(&self.config_path)
            .await
            .map_err(|e| Error::Internal(format!("Failed to read smb.conf: {}", e)))
    }

    /// Write config file
    async fn write_config(&self, config: &str) -> Result<()> {
        tokio::fs::write(&self.config_path, config)
            .await
            .map_err(|e| Error::Internal(format!("Failed to write smb.conf: {}", e)))
    }

    /// Test configuration syntax
    pub async fn test_config(&self) -> Result<bool> {
        let output = Command::new("testparm")
            .args(["-s", "--suppress-prompt"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("testparm failed: {}", e)))?;

        Ok(output.status.success())
    }

    /// Reload Samba configuration
    pub async fn reload(&self) -> Result<()> {
        // Send SIGHUP to smbd
        let output = Command::new("smbcontrol")
            .args(["all", "reload-config"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("smbcontrol failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to reload Samba: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Get active SMB connections
    pub async fn get_connections(&self) -> Result<Vec<SmbConnection>> {
        // Try JSON output first (Samba 4.x)
        let output = Command::new("smbstatus")
            .args(["--shares", "--json"])
            .output()
            .await;

        if let Ok(out) = output {
            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout) {
                    return Ok(Self::parse_connections_json(&json));
                }
            }
        }

        // Fallback to parseable output
        let output = Command::new("smbstatus")
            .args(["--shares", "--parseable"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("smbstatus failed: {}", e)))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(Self::parse_connections_parseable(&stdout))
    }

    fn parse_connections_json(json: &serde_json::Value) -> Vec<SmbConnection> {
        let mut connections = Vec::new();

        // Handle different Samba JSON formats
        if let Some(sessions) = json.get("sessions").and_then(|s| s.as_object()) {
            for (_session_id, session) in sessions {
                if let Some(tcons) = session.get("tcons").and_then(|t| t.as_object()) {
                    for (_tcon_id, tcon) in tcons {
                        let pid = session.get("session_id")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u32;
                        let username = session.get("username")
                            .and_then(|v| v.as_str())
                            .unwrap_or("").to_string();
                        let machine = session.get("remote_machine")
                            .and_then(|v| v.as_str())
                            .unwrap_or("").to_string();
                        let share = tcon.get("service")
                            .and_then(|v| v.as_str())
                            .unwrap_or("").to_string();
                        let protocol = session.get("signing")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| "SMB3".to_string());

                        connections.push(SmbConnection {
                            pid,
                            username,
                            share,
                            machine,
                            protocol,
                            connected_at: chrono::Utc::now().timestamp(),
                        });
                    }
                }
            }
        }

        connections
    }

    fn parse_connections_parseable(output: &str) -> Vec<SmbConnection> {
        let mut connections = Vec::new();

        for line in output.lines().skip(1) {
            let parts: Vec<&str> = line.split('\\').collect();
            if parts.len() >= 4 {
                connections.push(SmbConnection {
                    pid: parts.first().and_then(|s| s.parse().ok()).unwrap_or(0),
                    username: parts.get(1).unwrap_or(&"").to_string(),
                    share: parts.get(2).unwrap_or(&"").to_string(),
                    machine: parts.get(3).unwrap_or(&"").to_string(),
                    protocol: "SMB".to_string(),
                    connected_at: chrono::Utc::now().timestamp(),
                });
            }
        }

        connections
    }

    /// Get open files/locks
    pub async fn get_locks(&self) -> Result<Vec<SmbLock>> {
        // Try JSON output first
        let output = Command::new("smbstatus")
            .args(["--locks", "--json"])
            .output()
            .await;

        if let Ok(out) = output {
            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout) {
                    return Ok(Self::parse_locks_json(&json));
                }
            }
        }

        // Fallback to parseable output
        let output = Command::new("smbstatus")
            .args(["--locks", "--parseable"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("smbstatus failed: {}", e)))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(Self::parse_locks_parseable(&stdout))
    }

    fn parse_locks_json(json: &serde_json::Value) -> Vec<SmbLock> {
        let mut locks = Vec::new();

        if let Some(locked_files) = json.get("locked_files").and_then(|l| l.as_array()) {
            for file in locked_files {
                locks.push(SmbLock {
                    pid: file.get("pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                    username: file.get("username").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    share: file.get("service_path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    path: file.get("filename").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    lock_type: file.get("lock_type").and_then(|v| v.as_str()).unwrap_or("RW").to_string(),
                });
            }
        }

        locks
    }

    fn parse_locks_parseable(output: &str) -> Vec<SmbLock> {
        let mut locks = Vec::new();

        for line in output.lines().skip(1) {
            let parts: Vec<&str> = line.split('\\').collect();
            if parts.len() >= 5 {
                locks.push(SmbLock {
                    pid: parts.first().and_then(|s| s.parse().ok()).unwrap_or(0),
                    username: parts.get(1).unwrap_or(&"").to_string(),
                    share: parts.get(2).unwrap_or(&"").to_string(),
                    path: parts.get(3).unwrap_or(&"").to_string(),
                    lock_type: parts.get(4).unwrap_or(&"RW").to_string(),
                });
            }
        }

        locks
    }

    /// Disconnect a client session
    pub async fn disconnect_session(&self, pid: u32) -> Result<()> {
        let output = Command::new("smbcontrol")
            .args([&pid.to_string(), "close-share", "*"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("smbcontrol failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to disconnect session: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Add SMB user password
    pub async fn add_user(&self, username: &str, password: &str) -> Result<()> {
        let mut child = Command::new("smbpasswd")
            .args(["-a", "-s", username])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| Error::Internal(format!("smbpasswd failed: {}", e)))?;

        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin
                .write_all(format!("{}\n{}\n", password, password).as_bytes())
                .await?;
        }

        let status = child.wait().await?;
        if !status.success() {
            return Err(Error::Internal("smbpasswd failed".to_string()));
        }

        Ok(())
    }

    /// Enable SMB user
    pub async fn enable_user(&self, username: &str) -> Result<()> {
        let output = Command::new("smbpasswd")
            .args(["-e", username])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to enable user: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Disable SMB user
    pub async fn disable_user(&self, username: &str) -> Result<()> {
        let output = Command::new("smbpasswd")
            .args(["-d", username])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to disable user: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Delete SMB user
    pub async fn delete_user(&self, username: &str) -> Result<()> {
        let output = Command::new("smbpasswd")
            .args(["-x", username])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to delete user: {}",
                stderr
            )));
        }

        Ok(())
    }
}

impl Default for SmbManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SmbManager {
    /// Write complete configuration to disk
    pub async fn write_full_config(&self, shares: &[NasShare]) -> Result<()> {
        let config = self.generate_config(shares);

        // Backup existing config
        if tokio::fs::metadata(&self.config_path).await.is_ok() {
            let backup_path = format!("{}.bak", self.config_path);
            let _ = tokio::fs::copy(&self.config_path, &backup_path).await;
        }

        self.write_config(&config).await?;

        // Test configuration before reloading
        if !self.test_config().await? {
            // Restore backup if test fails
            let backup_path = format!("{}.bak", self.config_path);
            if tokio::fs::metadata(&backup_path).await.is_ok() {
                let _ = tokio::fs::copy(&backup_path, &self.config_path).await;
            }
            return Err(Error::Validation("Invalid Samba configuration".to_string()));
        }

        self.reload().await
    }

    /// Generate share section with shadow copy support for ZFS
    pub fn generate_share_with_shadow_copy(&self, share: &NasShare, zfs_dataset: Option<&str>) -> String {
        let mut section = self.generate_share_section(share);

        // Add ZFS shadow copy support if dataset is provided
        if let Some(dataset) = zfs_dataset {
            section.push_str("\n   # ZFS Shadow Copy Support\n");
            section.push_str("   vfs objects = shadow_copy2\n");
            section.push_str("   shadow:snapdir = .zfs/snapshot\n");
            section.push_str("   shadow:sort = desc\n");
            section.push_str("   shadow:format = %Y-%m-%d_%H-%M-%S\n");
            section.push_str("   shadow:localtime = no\n");
            section.push_str(&format!("   shadow:basedir = {}\n", share.path));
            section.push_str(&format!("   # ZFS dataset: {}\n", dataset));
        }

        section
    }

    /// Generate AD-integrated global configuration
    pub fn generate_ad_global_section(&self, ad_config: &AdSmbConfig) -> String {
        let mut section = String::new();
        let g = &self.global_config;

        section.push_str("[global]\n");
        section.push_str(&format!("   workgroup = {}\n", ad_config.workgroup));
        section.push_str(&format!("   realm = {}\n", ad_config.realm));
        section.push_str("   security = ADS\n");
        section.push_str("   encrypt passwords = yes\n");

        if let Some(ref netbios) = g.netbios_name {
            section.push_str(&format!("   netbios name = {}\n", netbios));
        }

        section.push_str(&format!("   server string = {}\n", g.server_string));

        // ID mapping
        section.push_str(&format!("   idmap config * : backend = {}\n", ad_config.idmap_backend));
        section.push_str(&format!("   idmap config * : range = {}-{}\n",
            ad_config.idmap_range_start, ad_config.idmap_range_end));
        section.push_str(&format!("   idmap config {} : backend = {}\n",
            ad_config.workgroup, ad_config.idmap_backend));
        section.push_str(&format!("   idmap config {} : range = {}-{}\n",
            ad_config.workgroup, ad_config.idmap_range_start, ad_config.idmap_range_end));

        if ad_config.use_rfc2307 {
            section.push_str(&format!("   idmap config {} : schema_mode = rfc2307\n", ad_config.workgroup));
        }

        // Winbind settings
        section.push_str(&format!("   template shell = {}\n", ad_config.template_shell));
        section.push_str(&format!("   template homedir = {}\n", ad_config.template_homedir));
        section.push_str("   winbind use default domain = yes\n");
        section.push_str("   winbind enum users = yes\n");
        section.push_str("   winbind enum groups = yes\n");

        if ad_config.offline_logon {
            section.push_str("   winbind offline logon = yes\n");
        }

        section.push_str("   winbind refresh tickets = yes\n");
        section.push_str("   kerberos method = secrets and keytab\n");
        section.push_str("   dedicated keytab file = /etc/krb5.keytab\n");

        if let Some(ref dc) = ad_config.password_server {
            section.push_str(&format!("   password server = {}\n", dc));
        }

        // Protocol versions
        section.push_str(&format!("   server min protocol = {}\n", g.min_protocol));
        section.push_str(&format!("   server max protocol = {}\n", g.max_protocol));

        // Disable printing
        section.push_str("   load printers = no\n");
        section.push_str("   printing = bsd\n");
        section.push_str("   printcap name = /dev/null\n");
        section.push_str("   disable spoolss = yes\n");

        // macOS optimizations
        if g.fruit_enabled {
            section.push_str("\n   # macOS optimizations\n");
            section.push_str("   vfs objects = fruit streams_xattr\n");
            section.push_str("   fruit:metadata = stream\n");
            section.push_str("   fruit:model = MacSamba\n");
            section.push_str("   fruit:posix_rename = yes\n");
            section.push_str("   fruit:nfs_aces = no\n");
        }

        section.push('\n');
        section
    }

    /// Get service status
    pub async fn get_status(&self) -> Result<SmbServiceStatus> {
        // Check if smbd is running
        let smbd_running = Self::check_process("smbd").await;
        let nmbd_running = Self::check_process("nmbd").await;
        let winbindd_running = Self::check_process("winbindd").await;

        // Get version
        let version = Self::get_samba_version().await.unwrap_or_else(|_| "unknown".to_string());

        // Count connections
        let connections = self.get_connections().await.unwrap_or_default();
        let locks = self.get_locks().await.unwrap_or_default();

        Ok(SmbServiceStatus {
            smbd_running,
            nmbd_running,
            winbindd_running,
            version,
            active_connections: connections.len() as u32,
            open_files: locks.len() as u32,
        })
    }

    async fn check_process(name: &str) -> bool {
        Command::new("pgrep")
            .arg(name)
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    async fn get_samba_version() -> Result<String> {
        let output = Command::new("smbd")
            .arg("--version")
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get Samba version: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.trim().to_string())
    }

    /// Start Samba services
    pub async fn start(&self) -> Result<()> {
        // Try systemd first
        let systemd_result = Command::new("systemctl")
            .args(["start", "smb", "nmb"])
            .output()
            .await;

        if let Ok(output) = systemd_result {
            if output.status.success() {
                return Ok(());
            }
        }

        // Try OpenRC
        let openrc_result = Command::new("rc-service")
            .args(["samba", "start"])
            .output()
            .await;

        if let Ok(output) = openrc_result {
            if output.status.success() {
                return Ok(());
            }
        }

        // Direct start
        let _ = Command::new("smbd").arg("-D").output().await;
        let _ = Command::new("nmbd").arg("-D").output().await;

        Ok(())
    }

    /// Stop Samba services
    pub async fn stop(&self) -> Result<()> {
        // Try systemd first
        let _ = Command::new("systemctl")
            .args(["stop", "smb", "nmb"])
            .output()
            .await;

        // Try OpenRC
        let _ = Command::new("rc-service")
            .args(["samba", "stop"])
            .output()
            .await;

        // Direct kill
        let _ = Command::new("pkill").arg("smbd").output().await;
        let _ = Command::new("pkill").arg("nmbd").output().await;

        Ok(())
    }

    /// Restart Samba services
    pub async fn restart(&self) -> Result<()> {
        self.stop().await?;
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        self.start().await
    }

    /// List all shares from current config
    pub async fn list_shares(&self) -> Result<Vec<String>> {
        let config = self.read_config().await?;
        let mut shares = Vec::new();

        for line in config.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                let name = &trimmed[1..trimmed.len()-1];
                if name != "global" && name != "printers" && name != "print$" {
                    shares.push(name.to_string());
                }
            }
        }

        Ok(shares)
    }

    /// Get share configuration from current config
    pub async fn get_share_config(&self, share_name: &str) -> Result<HashMap<String, String>> {
        let config = self.read_config().await?;
        let mut in_section = false;
        let mut params = HashMap::new();
        let section_header = format!("[{}]", share_name);

        for line in config.lines() {
            let trimmed = line.trim();

            if trimmed.eq_ignore_ascii_case(&section_header) {
                in_section = true;
                continue;
            }

            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                if in_section {
                    break;
                }
                continue;
            }

            if in_section && trimmed.contains('=') {
                if let Some((key, value)) = trimmed.split_once('=') {
                    params.insert(key.trim().to_string(), value.trim().to_string());
                }
            }
        }

        if params.is_empty() && !in_section {
            return Err(Error::NotFound(format!("Share '{}' not found", share_name)));
        }

        Ok(params)
    }

    /// Update a specific share parameter
    pub async fn update_share_param(&self, share_name: &str, key: &str, value: &str) -> Result<()> {
        let config = self.read_config().await?;
        let mut result = String::new();
        let mut in_section = false;
        let mut param_updated = false;
        let section_header = format!("[{}]", share_name);

        for line in config.lines() {
            let trimmed = line.trim();

            if trimmed.eq_ignore_ascii_case(&section_header) {
                in_section = true;
                result.push_str(line);
                result.push('\n');
                continue;
            }

            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                // If we were in the section and didn't find the param, add it
                if in_section && !param_updated {
                    result.push_str(&format!("   {} = {}\n", key, value));
                    param_updated = true;
                }
                in_section = false;
            }

            if in_section && trimmed.starts_with(key) && trimmed.contains('=') {
                result.push_str(&format!("   {} = {}\n", key, value));
                param_updated = true;
                continue;
            }

            result.push_str(line);
            result.push('\n');
        }

        // If section was at the end and param wasn't updated
        if in_section && !param_updated {
            result.push_str(&format!("   {} = {}\n", key, value));
        }

        self.write_config(&result).await?;
        self.reload().await
    }

    /// Break a file lock
    pub async fn break_lock(&self, pid: u32, share: &str) -> Result<()> {
        let output = Command::new("smbcontrol")
            .args([&pid.to_string(), "close-share", share])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("smbcontrol failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to break lock: {}", stderr)));
        }

        Ok(())
    }

    /// Set SMB user password (for existing users)
    pub async fn set_user_password(&self, username: &str, password: &str) -> Result<()> {
        let mut child = Command::new("smbpasswd")
            .args(["-s", username])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| Error::Internal(format!("smbpasswd failed: {}", e)))?;

        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin
                .write_all(format!("{}\n{}\n", password, password).as_bytes())
                .await?;
        }

        let status = child.wait().await?;
        if !status.success() {
            return Err(Error::Internal("smbpasswd failed".to_string()));
        }

        Ok(())
    }

    /// List SMB users from passdb
    pub async fn list_users(&self) -> Result<Vec<SmbUser>> {
        let output = Command::new("pdbedit")
            .args(["-L", "-v"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("pdbedit failed: {}", e)))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut users = Vec::new();
        let mut current_user: Option<SmbUser> = None;

        for line in stdout.lines() {
            if line.starts_with("Unix username:") {
                if let Some(user) = current_user.take() {
                    users.push(user);
                }
                let username = line.replace("Unix username:", "").trim().to_string();
                current_user = Some(SmbUser {
                    username,
                    uid: 0,
                    full_name: None,
                    flags: Vec::new(),
                    password_last_set: None,
                });
            } else if let Some(ref mut user) = current_user {
                if line.starts_with("Unix user ID:") {
                    user.uid = line.replace("Unix user ID:", "").trim().parse().unwrap_or(0);
                } else if line.starts_with("Full Name:") {
                    let name = line.replace("Full Name:", "").trim().to_string();
                    if !name.is_empty() {
                        user.full_name = Some(name);
                    }
                } else if line.starts_with("Account Flags:") {
                    let flags = line.replace("Account Flags:", "").trim().to_string();
                    user.flags = flags.chars()
                        .filter(|c| c.is_alphabetic())
                        .map(|c| c.to_string())
                        .collect();
                } else if line.starts_with("Password last set:") {
                    let time_str = line.replace("Password last set:", "").trim().to_string();
                    user.password_last_set = Some(time_str);
                }
            }
        }

        if let Some(user) = current_user {
            users.push(user);
        }

        Ok(users)
    }
}

/// AD-integrated SMB configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdSmbConfig {
    pub workgroup: String,
    pub realm: String,
    pub idmap_backend: String,
    pub idmap_range_start: u32,
    pub idmap_range_end: u32,
    pub use_rfc2307: bool,
    pub template_shell: String,
    pub template_homedir: String,
    pub offline_logon: bool,
    pub password_server: Option<String>,
}

impl Default for AdSmbConfig {
    fn default() -> Self {
        Self {
            workgroup: "WORKGROUP".to_string(),
            realm: "EXAMPLE.COM".to_string(),
            idmap_backend: "rid".to_string(),
            idmap_range_start: 10000,
            idmap_range_end: 999999,
            use_rfc2307: false,
            template_shell: "/bin/bash".to_string(),
            template_homedir: "/home/%U".to_string(),
            offline_logon: true,
            password_server: None,
        }
    }
}

/// SMB service status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmbServiceStatus {
    pub smbd_running: bool,
    pub nmbd_running: bool,
    pub winbindd_running: bool,
    pub version: String,
    pub active_connections: u32,
    pub open_files: u32,
}

/// SMB user from passdb
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmbUser {
    pub username: String,
    pub uid: u32,
    pub full_name: Option<String>,
    pub flags: Vec<String>,
    pub password_last_set: Option<String>,
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_global_config() {
        let config = SmbGlobalConfig::default();
        assert_eq!(config.workgroup, "WORKGROUP");
        assert_eq!(config.security, "user");
        assert!(config.fruit_enabled);
    }

    #[test]
    fn test_generate_global_section() {
        let manager = SmbManager::new();
        let section = manager.generate_global_section();

        assert!(section.contains("[global]"));
        assert!(section.contains("workgroup = WORKGROUP"));
        assert!(section.contains("security = user"));
    }

    #[test]
    fn test_remove_share_section() {
        let manager = SmbManager::new();
        let config = r#"[global]
   workgroup = WORKGROUP

[share1]
   path = /mnt/share1

[share2]
   path = /mnt/share2
"#;

        let result = manager.remove_share_section(config, "share1");
        assert!(!result.contains("[share1]"));
        assert!(result.contains("[share2]"));
        assert!(result.contains("[global]"));
    }
}
