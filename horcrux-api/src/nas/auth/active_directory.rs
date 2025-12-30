//! Active Directory Integration module
//!
//! Manages Active Directory domain join and integration via Samba/Winbind.

use horcrux_common::{Error, Result};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use std::collections::HashMap;

/// Active Directory Configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdConfig {
    /// AD domain name (e.g., CORP.EXAMPLE.COM)
    pub domain: String,
    /// Workgroup/NetBIOS name
    pub workgroup: String,
    /// AD domain controller
    pub domain_controller: Option<String>,
    /// ID mapping backend (rid, ad, autorid)
    pub idmap_backend: String,
    /// ID range start
    pub idmap_range_start: u32,
    /// ID range end
    pub idmap_range_end: u32,
    /// Default shell for AD users
    pub default_shell: String,
    /// Home directory template
    pub home_dir_template: String,
    /// Enable offline authentication
    pub offline_auth: bool,
    /// Kerberos realm (usually same as domain)
    pub kerberos_realm: String,
    /// Use RFC2307 attributes from AD
    pub use_rfc2307: bool,
}

impl Default for AdConfig {
    fn default() -> Self {
        Self {
            domain: String::new(),
            workgroup: String::new(),
            domain_controller: None,
            idmap_backend: "rid".to_string(),
            idmap_range_start: 10000,
            idmap_range_end: 999999,
            default_shell: "/bin/bash".to_string(),
            home_dir_template: "/home/%U".to_string(),
            offline_auth: true,
            kerberos_realm: String::new(),
            use_rfc2307: false,
        }
    }
}

/// AD Domain Join Status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdJoinStatus {
    /// Whether joined to a domain
    pub joined: bool,
    /// Domain name
    pub domain: Option<String>,
    /// Domain controller
    pub domain_controller: Option<String>,
    /// Machine account name
    pub machine_account: Option<String>,
    /// Join timestamp
    pub joined_at: Option<i64>,
    /// Winbind running
    pub winbind_running: bool,
}

/// AD User from winbind
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdUser {
    /// Username (DOMAIN\user or user@domain)
    pub username: String,
    /// Unix UID
    pub uid: u32,
    /// Primary GID
    pub gid: u32,
    /// Full name
    pub full_name: Option<String>,
    /// Home directory
    pub home_directory: String,
    /// Login shell
    pub shell: String,
    /// SID
    pub sid: Option<String>,
}

/// AD Group from winbind
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdGroup {
    /// Group name (DOMAIN\group)
    pub name: String,
    /// Unix GID
    pub gid: u32,
    /// SID
    pub sid: Option<String>,
    /// Members (usernames)
    pub members: Vec<String>,
}

/// Active Directory Manager
pub struct ActiveDirectoryManager {
    config: AdConfig,
    smb_conf: String,
    krb5_conf: String,
}

impl ActiveDirectoryManager {
    /// Create a new AD manager
    pub fn new() -> Self {
        Self {
            config: AdConfig::default(),
            smb_conf: "/etc/samba/smb.conf".to_string(),
            krb5_conf: "/etc/krb5.conf".to_string(),
        }
    }

    /// Set configuration
    pub fn set_config(&mut self, config: AdConfig) {
        self.config = config;
    }

    /// Join Active Directory domain
    pub async fn join_domain(&self, admin_user: &str, admin_password: &str) -> Result<()> {
        // First, configure Samba and Kerberos
        self.write_smb_conf().await?;
        self.write_krb5_conf().await?;

        // Obtain Kerberos ticket
        let principal = format!("{}@{}", admin_user, self.config.kerberos_realm);
        self.kinit(&principal, admin_password).await?;

        // Join domain using net ads join
        let mut args = vec![
            "ads".to_string(),
            "join".to_string(),
            "-U".to_string(),
            format!("{}%{}", admin_user, admin_password),
        ];

        if let Some(ref dc) = self.config.domain_controller {
            args.push("-S".to_string());
            args.push(dc.clone());
        }

        let output = Command::new("net")
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("net ads join failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Domain join failed: {}", stderr)));
        }

        // Start winbind
        self.start_winbind().await?;

        // Configure NSS
        self.configure_nss().await?;

        // Configure PAM
        self.configure_pam().await?;

        Ok(())
    }

    /// Leave Active Directory domain
    pub async fn leave_domain(&self, admin_user: &str, admin_password: &str) -> Result<()> {
        let output = Command::new("net")
            .args([
                "ads", "leave",
                "-U", &format!("{}%{}", admin_user, admin_password),
            ])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("net ads leave failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Domain leave failed: {}", stderr)));
        }

        // Stop winbind
        let _ = self.stop_winbind().await;

        Ok(())
    }

    /// Check domain join status
    pub async fn get_join_status(&self) -> Result<AdJoinStatus> {
        let output = Command::new("net")
            .args(["ads", "testjoin"])
            .output()
            .await;

        let joined = match output {
            Ok(out) => out.status.success(),
            Err(_) => false,
        };

        let (domain, domain_controller, machine_account) = if joined {
            let info = self.get_domain_info().await.unwrap_or_default();
            (
                info.get("Domain").cloned(),
                info.get("DC").cloned(),
                info.get("Account").cloned(),
            )
        } else {
            (None, None, None)
        };

        let winbind_running = self.is_winbind_running().await;

        Ok(AdJoinStatus {
            joined,
            domain,
            domain_controller,
            machine_account,
            joined_at: None,
            winbind_running,
        })
    }

    /// Get domain information
    async fn get_domain_info(&self) -> Result<HashMap<String, String>> {
        let output = Command::new("net")
            .args(["ads", "info"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("net ads info failed: {}", e)))?;

        let mut info = HashMap::new();
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if let Some((key, value)) = line.split_once(':') {
                    info.insert(key.trim().to_string(), value.trim().to_string());
                }
            }
        }

        Ok(info)
    }

    /// List AD users via winbind
    pub async fn list_users(&self) -> Result<Vec<AdUser>> {
        let output = Command::new("wbinfo")
            .args(["-u"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("wbinfo failed: {}", e)))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut users = Vec::new();

        for line in stdout.lines() {
            let username = line.trim();
            if !username.is_empty() {
                if let Ok(user) = self.get_user_info(username).await {
                    users.push(user);
                }
            }
        }

        Ok(users)
    }

    /// Get user info via getent
    async fn get_user_info(&self, username: &str) -> Result<AdUser> {
        let output = Command::new("getent")
            .args(["passwd", username])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("getent failed: {}", e)))?;

        if !output.status.success() {
            return Err(Error::NotFound(format!("User '{}' not found", username)));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = stdout.trim().split(':').collect();

        if parts.len() < 7 {
            return Err(Error::Internal("Invalid passwd entry".to_string()));
        }

        Ok(AdUser {
            username: parts[0].to_string(),
            uid: parts[2].parse().unwrap_or(0),
            gid: parts[3].parse().unwrap_or(0),
            full_name: if parts[4].is_empty() { None } else { Some(parts[4].to_string()) },
            home_directory: parts[5].to_string(),
            shell: parts[6].to_string(),
            sid: None,
        })
    }

    /// List AD groups via winbind
    pub async fn list_groups(&self) -> Result<Vec<AdGroup>> {
        let output = Command::new("wbinfo")
            .args(["-g"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("wbinfo failed: {}", e)))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut groups = Vec::new();

        for line in stdout.lines() {
            let name = line.trim();
            if !name.is_empty() {
                if let Ok(group) = self.get_group_info(name).await {
                    groups.push(group);
                }
            }
        }

        Ok(groups)
    }

    /// Get group info via getent
    async fn get_group_info(&self, name: &str) -> Result<AdGroup> {
        let output = Command::new("getent")
            .args(["group", name])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("getent failed: {}", e)))?;

        if !output.status.success() {
            return Err(Error::NotFound(format!("Group '{}' not found", name)));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = stdout.trim().split(':').collect();

        if parts.len() < 4 {
            return Err(Error::Internal("Invalid group entry".to_string()));
        }

        let members: Vec<String> = if parts[3].is_empty() {
            Vec::new()
        } else {
            parts[3].split(',').map(|s| s.to_string()).collect()
        };

        Ok(AdGroup {
            name: parts[0].to_string(),
            gid: parts[2].parse().unwrap_or(0),
            sid: None,
            members,
        })
    }

    /// Get groups for a user
    pub async fn get_user_groups(&self, username: &str) -> Result<Vec<String>> {
        let output = Command::new("wbinfo")
            .args(["--user-groups", username])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("wbinfo failed: {}", e)))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
    }

    /// Authenticate user against AD
    pub async fn authenticate(&self, username: &str, password: &str) -> Result<bool> {
        let output = Command::new("wbinfo")
            .args(["-a", &format!("{}%{}", username, password)])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("wbinfo failed: {}", e)))?;

        Ok(output.status.success())
    }

    /// Convert SID to Unix ID
    pub async fn sid_to_uid(&self, sid: &str) -> Result<u32> {
        let output = Command::new("wbinfo")
            .args(["-S", sid])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("wbinfo failed: {}", e)))?;

        if !output.status.success() {
            return Err(Error::NotFound("SID not found".to_string()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout.trim().parse()
            .map_err(|_| Error::Internal("Invalid UID".to_string()))
    }

    /// Convert Unix ID to SID
    pub async fn uid_to_sid(&self, uid: u32) -> Result<String> {
        let output = Command::new("wbinfo")
            .args(["-U", &uid.to_string()])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("wbinfo failed: {}", e)))?;

        if !output.status.success() {
            return Err(Error::NotFound("UID not found".to_string()));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Generate smb.conf for AD
    fn generate_smb_conf(&self) -> String {
        let mut config = String::new();

        config.push_str("[global]\n");
        config.push_str(&format!("   workgroup = {}\n", self.config.workgroup));
        config.push_str(&format!("   realm = {}\n", self.config.kerberos_realm));
        config.push_str("   security = ADS\n");
        config.push_str("   encrypt passwords = yes\n");
        config.push_str(&format!("   idmap config * : backend = {}\n", self.config.idmap_backend));
        config.push_str(&format!("   idmap config * : range = {}-{}\n",
            self.config.idmap_range_start,
            self.config.idmap_range_end
        ));
        config.push_str(&format!("   idmap config {} : backend = {}\n",
            self.config.workgroup,
            self.config.idmap_backend
        ));
        config.push_str(&format!("   idmap config {} : range = {}-{}\n",
            self.config.workgroup,
            self.config.idmap_range_start,
            self.config.idmap_range_end
        ));

        if self.config.use_rfc2307 {
            config.push_str(&format!("   idmap config {} : schema_mode = rfc2307\n", self.config.workgroup));
        }

        config.push_str(&format!("   template shell = {}\n", self.config.default_shell));
        config.push_str(&format!("   template homedir = {}\n", self.config.home_dir_template));
        config.push_str("   winbind use default domain = yes\n");
        config.push_str("   winbind enum users = yes\n");
        config.push_str("   winbind enum groups = yes\n");

        if self.config.offline_auth {
            config.push_str("   winbind offline logon = yes\n");
        }

        config.push_str("   winbind refresh tickets = yes\n");
        config.push_str("   kerberos method = secrets and keytab\n");
        config.push_str("   dedicated keytab file = /etc/krb5.keytab\n");

        if let Some(ref dc) = self.config.domain_controller {
            config.push_str(&format!("   password server = {}\n", dc));
        }

        config
    }

    /// Write smb.conf
    async fn write_smb_conf(&self) -> Result<()> {
        let config = self.generate_smb_conf();
        tokio::fs::write(&self.smb_conf, config).await
            .map_err(|e| Error::Internal(format!("Failed to write smb.conf: {}", e)))
    }

    /// Generate krb5.conf for AD
    fn generate_krb5_conf(&self) -> String {
        format!(r#"[libdefaults]
    default_realm = {}
    dns_lookup_kdc = true
    dns_lookup_realm = true
    ticket_lifetime = 24h
    renew_lifetime = 7d
    forwardable = true
    rdns = false
    default_ccache_name = KEYRING:persistent:%{{uid}}

[realms]
    {} = {{
        kdc = {}
        admin_server = {}
        default_domain = {}
    }}

[domain_realm]
    .{} = {}
    {} = {}
"#,
            self.config.kerberos_realm,
            self.config.kerberos_realm,
            self.config.domain_controller.as_deref().unwrap_or(&self.config.domain),
            self.config.domain_controller.as_deref().unwrap_or(&self.config.domain),
            self.config.domain.to_lowercase(),
            self.config.domain.to_lowercase(),
            self.config.kerberos_realm,
            self.config.domain.to_lowercase(),
            self.config.kerberos_realm,
        )
    }

    /// Write krb5.conf
    async fn write_krb5_conf(&self) -> Result<()> {
        let config = self.generate_krb5_conf();
        tokio::fs::write(&self.krb5_conf, config).await
            .map_err(|e| Error::Internal(format!("Failed to write krb5.conf: {}", e)))
    }

    /// Obtain Kerberos ticket
    async fn kinit(&self, principal: &str, password: &str) -> Result<()> {
        let mut child = Command::new("kinit")
            .arg(principal)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| Error::Internal(format!("kinit failed: {}", e)))?;

        if let Some(stdin) = child.stdin.as_mut() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(password.as_bytes()).await
                .map_err(|e| Error::Internal(format!("Failed to write password: {}", e)))?;
            stdin.write_all(b"\n").await.ok();
        }

        let output = child.wait_with_output().await
            .map_err(|e| Error::Internal(format!("kinit failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Authentication(format!("kinit failed: {}", stderr)));
        }

        Ok(())
    }

    /// Configure NSS for winbind
    async fn configure_nss(&self) -> Result<()> {
        // Read current nsswitch.conf
        let content = tokio::fs::read_to_string("/etc/nsswitch.conf").await
            .unwrap_or_default();

        let mut new_content = String::new();
        let mut passwd_done = false;
        let mut group_done = false;

        for line in content.lines() {
            if line.starts_with("passwd:") && !line.contains("winbind") {
                new_content.push_str(&format!("{} winbind\n", line.trim()));
                passwd_done = true;
            } else if line.starts_with("group:") && !line.contains("winbind") {
                new_content.push_str(&format!("{} winbind\n", line.trim()));
                group_done = true;
            } else {
                new_content.push_str(line);
                new_content.push('\n');
            }
        }

        if !passwd_done {
            new_content.push_str("passwd: files winbind\n");
        }
        if !group_done {
            new_content.push_str("group: files winbind\n");
        }

        tokio::fs::write("/etc/nsswitch.conf", new_content).await
            .map_err(|e| Error::Internal(format!("Failed to write nsswitch.conf: {}", e)))
    }

    /// Configure PAM for winbind
    async fn configure_pam(&self) -> Result<()> {
        // This is simplified - real implementation would modify PAM config files
        // using pam-auth-update or similar
        Ok(())
    }

    /// Start winbind service
    async fn start_winbind(&self) -> Result<()> {
        let output = Command::new("systemctl")
            .args(["start", "winbind"])
            .output()
            .await;

        if let Ok(out) = output {
            if out.status.success() {
                return Ok(());
            }
        }

        let output = Command::new("rc-service")
            .args(["winbind", "start"])
            .output()
            .await;

        if let Ok(out) = output {
            if out.status.success() {
                return Ok(());
            }
        }

        Err(Error::Internal("Failed to start winbind".to_string()))
    }

    /// Stop winbind service
    async fn stop_winbind(&self) -> Result<()> {
        let _ = Command::new("systemctl")
            .args(["stop", "winbind"])
            .output()
            .await;

        let _ = Command::new("rc-service")
            .args(["winbind", "stop"])
            .output()
            .await;

        Ok(())
    }

    /// Check if winbind is running
    async fn is_winbind_running(&self) -> bool {
        let output = Command::new("systemctl")
            .args(["is-active", "winbind"])
            .output()
            .await;

        if let Ok(out) = output {
            if out.status.success() {
                return true;
            }
        }

        let output = Command::new("rc-service")
            .args(["winbind", "status"])
            .output()
            .await;

        if let Ok(out) = output {
            return out.status.success();
        }

        false
    }

    /// Refresh winbind cache
    pub async fn refresh_cache(&self) -> Result<()> {
        let _ = Command::new("net")
            .args(["cache", "flush"])
            .output()
            .await;

        Ok(())
    }

    /// Get AD status
    pub async fn get_status(&self) -> Result<AdStatus> {
        let join_status = self.get_join_status().await?;

        Ok(AdStatus {
            joined: join_status.joined,
            domain: join_status.domain,
            domain_controller: join_status.domain_controller,
            winbind_running: join_status.winbind_running,
            kerberos_realm: self.config.kerberos_realm.clone(),
            idmap_backend: self.config.idmap_backend.clone(),
        })
    }
}

/// AD status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdStatus {
    pub joined: bool,
    pub domain: Option<String>,
    pub domain_controller: Option<String>,
    pub winbind_running: bool,
    pub kerberos_realm: String,
    pub idmap_backend: String,
}

impl Default for ActiveDirectoryManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ActiveDirectoryManager {
    /// Register DNS records for the machine account
    pub async fn register_dns(&self) -> Result<()> {
        let output = Command::new("net")
            .args(["ads", "dns", "register"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("DNS registration failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("DNS registration failed: {}", stderr)));
        }

        Ok(())
    }

    /// Unregister DNS records
    pub async fn unregister_dns(&self) -> Result<()> {
        let output = Command::new("net")
            .args(["ads", "dns", "unregister"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("DNS unregistration failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("DNS unregistration failed: {}", stderr)));
        }

        Ok(())
    }

    /// Rotate machine account password
    pub async fn rotate_machine_password(&self) -> Result<()> {
        let output = Command::new("net")
            .args(["ads", "changetrustpw"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Password rotation failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Password rotation failed: {}", stderr)));
        }

        Ok(())
    }

    /// Get machine keytab from AD
    pub async fn get_machine_keytab(&self, keytab_path: &str) -> Result<()> {
        let output = Command::new("net")
            .args(["ads", "keytab", "create", "-P"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Keytab creation failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Keytab creation failed: {}", stderr)));
        }

        // Copy to specified path if different from default
        if keytab_path != "/etc/krb5.keytab" {
            tokio::fs::copy("/etc/krb5.keytab", keytab_path).await
                .map_err(|e| Error::Internal(format!("Failed to copy keytab: {}", e)))?;
        }

        Ok(())
    }

    /// Add service principal to keytab
    pub async fn add_keytab_principal(&self, principal: &str) -> Result<()> {
        let output = Command::new("net")
            .args(["ads", "keytab", "add", principal])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Add keytab principal failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Add keytab principal failed: {}", stderr)));
        }

        Ok(())
    }

    /// List keytab principals
    pub async fn list_keytab_principals(&self) -> Result<Vec<String>> {
        let output = Command::new("net")
            .args(["ads", "keytab", "list"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("List keytab failed: {}", e)))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.lines()
            .filter(|l| l.contains("@"))
            .map(|l| l.trim().to_string())
            .collect())
    }

    /// Lookup user by SID
    pub async fn lookup_user_by_sid(&self, sid: &str) -> Result<AdUser> {
        let output = Command::new("wbinfo")
            .args(["--sid-to-name", sid])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("wbinfo failed: {}", e)))?;

        if !output.status.success() {
            return Err(Error::NotFound(format!("SID '{}' not found", sid)));
        }

        let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
        // Extract the actual username if format is DOMAIN\user
        let username = name.split('\\').last().unwrap_or(&name);

        self.get_user_info(username).await
    }

    /// Lookup group by SID
    pub async fn lookup_group_by_sid(&self, sid: &str) -> Result<AdGroup> {
        let output = Command::new("wbinfo")
            .args(["--sid-to-name", sid])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("wbinfo failed: {}", e)))?;

        if !output.status.success() {
            return Err(Error::NotFound(format!("SID '{}' not found", sid)));
        }

        let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let groupname = name.split('\\').last().unwrap_or(&name);

        self.get_group_info(groupname).await
    }

    /// Get user SID
    pub async fn get_user_sid(&self, username: &str) -> Result<String> {
        let output = Command::new("wbinfo")
            .args(["--name-to-sid", username])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("wbinfo failed: {}", e)))?;

        if !output.status.success() {
            return Err(Error::NotFound(format!("User '{}' not found", username)));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        // Output format: SID type
        let sid = stdout.split_whitespace().next().unwrap_or("").to_string();
        Ok(sid)
    }

    /// Get group SID
    pub async fn get_group_sid(&self, groupname: &str) -> Result<String> {
        let output = Command::new("wbinfo")
            .args(["--name-to-sid", groupname])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("wbinfo failed: {}", e)))?;

        if !output.status.success() {
            return Err(Error::NotFound(format!("Group '{}' not found", groupname)));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let sid = stdout.split_whitespace().next().unwrap_or("").to_string();
        Ok(sid)
    }

    /// Test trust relationship
    pub async fn test_trust(&self) -> Result<TrustStatus> {
        let output = Command::new("wbinfo")
            .args(["--check-secret"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("wbinfo failed: {}", e)))?;

        let secret_ok = output.status.success();

        let output = Command::new("wbinfo")
            .args(["-p"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("wbinfo failed: {}", e)))?;

        let ping_ok = output.status.success();

        Ok(TrustStatus {
            secret_valid: secret_ok,
            dc_reachable: ping_ok,
        })
    }

    /// List trusted domains
    pub async fn list_trusts(&self) -> Result<Vec<TrustedDomain>> {
        let output = Command::new("wbinfo")
            .args(["--all-domains"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("wbinfo failed: {}", e)))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut domains = Vec::new();

        for line in stdout.lines() {
            let name = line.trim();
            if !name.is_empty() {
                domains.push(TrustedDomain {
                    name: name.to_string(),
                    trust_type: "Unknown".to_string(),
                    is_transitive: false,
                });
            }
        }

        Ok(domains)
    }

    /// Ping domain controller
    pub async fn ping_dc(&self) -> Result<DcPingResult> {
        let start = std::time::Instant::now();

        let output = Command::new("wbinfo")
            .args(["-p"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("wbinfo failed: {}", e)))?;

        let latency_ms = start.elapsed().as_millis() as u64;
        let success = output.status.success();

        // Get DC name
        let dc_output = Command::new("wbinfo")
            .args(["--dsgetdcname", &self.config.domain])
            .output()
            .await;

        let dc_name = if let Ok(out) = dc_output {
            let stdout = String::from_utf8_lossy(&out.stdout);
            stdout.lines()
                .find(|l| l.contains("DC name"))
                .and_then(|l| l.split(':').nth(1))
                .map(|s| s.trim().to_string())
        } else {
            None
        };

        Ok(DcPingResult {
            success,
            latency_ms,
            dc_name,
        })
    }

    /// Configure NTP to sync with AD domain
    pub async fn configure_time_sync(&self) -> Result<()> {
        // Try to get DC as NTP server
        let dc = self.config.domain_controller.as_ref()
            .cloned()
            .unwrap_or_else(|| self.config.domain.clone());

        // Write chrony or ntp config
        let chrony_config = format!(r#"# AD Domain Controller time sync
server {} iburst prefer
driftfile /var/lib/chrony/drift
makestep 1.0 3
rtcsync
"#, dc);

        // Try chrony first
        if let Err(_) = tokio::fs::write("/etc/chrony.conf", &chrony_config).await {
            // Fall back to ntp.conf format
            let ntp_config = format!(r#"# AD Domain Controller time sync
server {} iburst prefer
driftfile /var/lib/ntp/drift
"#, dc);

            tokio::fs::write("/etc/ntp.conf", &ntp_config).await
                .map_err(|e| Error::Internal(format!("Failed to write NTP config: {}", e)))?;
        }

        // Restart time service
        let _ = Command::new("systemctl").args(["restart", "chronyd"]).output().await;
        let _ = Command::new("systemctl").args(["restart", "ntpd"]).output().await;
        let _ = Command::new("rc-service").args(["chronyd", "restart"]).output().await;
        let _ = Command::new("rc-service").args(["ntpd", "restart"]).output().await;

        Ok(())
    }

    /// Get effective permissions for a user on a path
    pub async fn get_effective_permissions(&self, username: &str, path: &str) -> Result<EffectivePermissions> {
        // Get user's SID
        let user_sid = self.get_user_sid(username).await.ok();

        // Get user's groups
        let groups = self.get_user_groups(username).await.unwrap_or_default();

        // Check file permissions using test command
        let can_read = Command::new("sudo")
            .args(["-u", username, "test", "-r", path])
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);

        let can_write = Command::new("sudo")
            .args(["-u", username, "test", "-w", path])
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);

        let can_execute = Command::new("sudo")
            .args(["-u", username, "test", "-x", path])
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);

        Ok(EffectivePermissions {
            username: username.to_string(),
            path: path.to_string(),
            can_read,
            can_write,
            can_execute,
            groups,
            user_sid,
        })
    }

    /// Join domain with additional options
    pub async fn join_domain_advanced(&self, options: DomainJoinOptions) -> Result<()> {
        // Configure files first
        self.write_smb_conf().await?;
        self.write_krb5_conf().await?;

        // Configure time sync if requested
        if options.sync_time {
            let _ = self.configure_time_sync().await;
        }

        // Obtain Kerberos ticket
        let principal = format!("{}@{}", options.admin_user, self.config.kerberos_realm);
        self.kinit(&principal, &options.admin_password).await?;

        // Build net ads join command
        let mut args = vec![
            "ads".to_string(),
            "join".to_string(),
            "-U".to_string(),
            format!("{}%{}", options.admin_user, options.admin_password),
        ];

        if let Some(ref dc) = self.config.domain_controller {
            args.push("-S".to_string());
            args.push(dc.clone());
        }

        if let Some(ref ou) = options.computer_ou {
            args.push("createcomputer=".to_string() + ou);
        }

        if options.no_dns_update {
            args.push("--no-dns-updates".to_string());
        }

        let output = Command::new("net")
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("net ads join failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Domain join failed: {}", stderr)));
        }

        // Register DNS if requested
        if options.register_dns {
            let _ = self.register_dns().await;
        }

        // Start winbind
        self.start_winbind().await?;

        // Configure NSS
        self.configure_nss().await?;

        // Configure PAM with advanced options
        if options.create_home_dirs {
            self.configure_pam_advanced(true).await?;
        } else {
            self.configure_pam().await?;
        }

        Ok(())
    }

    /// Configure PAM with additional options
    async fn configure_pam_advanced(&self, create_home: bool) -> Result<()> {
        let pam_winbind = format!(r#"# Horcrux AD PAM configuration
auth        sufficient    pam_winbind.so
account     sufficient    pam_winbind.so
password    sufficient    pam_winbind.so
session     optional      pam_winbind.so
"#);

        tokio::fs::write("/etc/pam.d/horcrux-ad", &pam_winbind).await
            .map_err(|e| Error::Internal(format!("Failed to write PAM config: {}", e)))?;

        if create_home {
            // Configure mkhomedir
            let mkhomedir = "session     optional      pam_mkhomedir.so skel=/etc/skel umask=0077\n";
            let mut content = tokio::fs::read_to_string("/etc/pam.d/horcrux-ad").await
                .unwrap_or_default();
            content.push_str(mkhomedir);
            tokio::fs::write("/etc/pam.d/horcrux-ad", content).await
                .map_err(|e| Error::Internal(format!("Failed to write PAM config: {}", e)))?;
        }

        Ok(())
    }

    /// Verify join prerequisites
    pub async fn verify_prerequisites(&self) -> Result<JoinPrerequisites> {
        let mut prereqs = JoinPrerequisites {
            dns_resolves: false,
            dc_reachable: false,
            ports_open: Vec::new(),
            time_synced: false,
            samba_installed: false,
            winbind_installed: false,
            krb5_installed: false,
            errors: Vec::new(),
        };

        // Check if samba is installed
        prereqs.samba_installed = Command::new("which")
            .arg("net")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);

        if !prereqs.samba_installed {
            prereqs.errors.push("Samba (net command) not installed".to_string());
        }

        // Check if winbind is installed
        prereqs.winbind_installed = Command::new("which")
            .arg("wbinfo")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);

        if !prereqs.winbind_installed {
            prereqs.errors.push("Winbind not installed".to_string());
        }

        // Check if krb5 is installed
        prereqs.krb5_installed = Command::new("which")
            .arg("kinit")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);

        if !prereqs.krb5_installed {
            prereqs.errors.push("MIT Kerberos not installed".to_string());
        }

        // Check DNS resolution
        let dc = self.config.domain_controller.as_ref()
            .cloned()
            .unwrap_or_else(|| self.config.domain.clone());

        let dns_check = Command::new("host")
            .arg(&dc)
            .output()
            .await;

        prereqs.dns_resolves = dns_check.map(|o| o.status.success()).unwrap_or(false);
        if !prereqs.dns_resolves {
            prereqs.errors.push(format!("Cannot resolve {}", dc));
        }

        // Check DC reachability (ping)
        let ping_check = Command::new("ping")
            .args(["-c", "1", "-W", "3", &dc])
            .output()
            .await;

        prereqs.dc_reachable = ping_check.map(|o| o.status.success()).unwrap_or(false);
        if !prereqs.dc_reachable {
            prereqs.errors.push(format!("Cannot reach {}", dc));
        }

        // Check required ports (simplified - just check if nc is available)
        for port in [88, 389, 445, 464] {
            let nc_check = Command::new("nc")
                .args(["-z", "-w", "3", &dc, &port.to_string()])
                .output()
                .await;

            if nc_check.map(|o| o.status.success()).unwrap_or(false) {
                prereqs.ports_open.push(port);
            }
        }

        // Check time sync (compare with DC if possible)
        let time_check = Command::new("ntpdate")
            .args(["-q", &dc])
            .output()
            .await;

        prereqs.time_synced = time_check.map(|o| {
            let stdout = String::from_utf8_lossy(&o.stdout);
            // If offset is less than 5 minutes, we're good
            !stdout.contains("offset") ||
            stdout.lines().any(|l| {
                if let Some(offset_str) = l.split("offset").nth(1) {
                    if let Ok(offset) = offset_str.trim().split_whitespace().next()
                        .unwrap_or("0").parse::<f64>()
                    {
                        return offset.abs() < 300.0;
                    }
                }
                true
            })
        }).unwrap_or(true);

        if !prereqs.time_synced {
            prereqs.errors.push("Time not synchronized with DC (>5 min offset)".to_string());
        }

        Ok(prereqs)
    }
}

/// Trust status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustStatus {
    pub secret_valid: bool,
    pub dc_reachable: bool,
}

/// Trusted domain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustedDomain {
    pub name: String,
    pub trust_type: String,
    pub is_transitive: bool,
}

/// DC ping result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DcPingResult {
    pub success: bool,
    pub latency_ms: u64,
    pub dc_name: Option<String>,
}

/// Effective permissions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectivePermissions {
    pub username: String,
    pub path: String,
    pub can_read: bool,
    pub can_write: bool,
    pub can_execute: bool,
    pub groups: Vec<String>,
    pub user_sid: Option<String>,
}

/// Domain join options
#[derive(Debug, Clone)]
pub struct DomainJoinOptions {
    pub admin_user: String,
    pub admin_password: String,
    pub computer_ou: Option<String>,
    pub sync_time: bool,
    pub register_dns: bool,
    pub no_dns_update: bool,
    pub create_home_dirs: bool,
}

/// Join prerequisites check
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinPrerequisites {
    pub dns_resolves: bool,
    pub dc_reachable: bool,
    pub ports_open: Vec<u16>,
    pub time_synced: bool,
    pub samba_installed: bool,
    pub winbind_installed: bool,
    pub krb5_installed: bool,
    pub errors: Vec<String>,
}
