//! Kerberos Authentication module
//!
//! Manages MIT Kerberos for authentication services.

use horcrux_common::{Error, Result};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use std::collections::HashMap;

/// Kerberos Configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KerberosConfig {
    /// Default realm
    pub default_realm: String,
    /// KDC servers (realm -> [servers])
    pub realms: HashMap<String, RealmConfig>,
    /// Domain to realm mappings
    pub domain_realm: HashMap<String, String>,
    /// DNS lookup for KDC
    pub dns_lookup_kdc: bool,
    /// DNS lookup for realm
    pub dns_lookup_realm: bool,
    /// Ticket lifetime
    pub ticket_lifetime: String,
    /// Renewable lifetime
    pub renew_lifetime: String,
    /// Forwardable tickets
    pub forwardable: bool,
    /// Proxiable tickets
    pub proxiable: bool,
}

/// Realm configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealmConfig {
    /// KDC servers
    pub kdc: Vec<String>,
    /// Admin server
    pub admin_server: Option<String>,
    /// Default domain
    pub default_domain: Option<String>,
}

impl Default for KerberosConfig {
    fn default() -> Self {
        Self {
            default_realm: "HORCRUX.LOCAL".to_string(),
            realms: HashMap::new(),
            domain_realm: HashMap::new(),
            dns_lookup_kdc: true,
            dns_lookup_realm: true,
            ticket_lifetime: "24h".to_string(),
            renew_lifetime: "7d".to_string(),
            forwardable: true,
            proxiable: true,
        }
    }
}

/// Kerberos ticket information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KerberosTicket {
    /// Principal name
    pub principal: String,
    /// Issue time
    pub issue_time: i64,
    /// Expiration time
    pub expire_time: i64,
    /// Renew until time
    pub renew_until: Option<i64>,
    /// Ticket flags
    pub flags: Vec<String>,
}

/// Kerberos principal
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KerberosPrincipal {
    /// Principal name
    pub name: String,
    /// Principal type (user, service, host)
    pub principal_type: String,
    /// Policy name
    pub policy: Option<String>,
    /// Last password change
    pub last_password_change: Option<i64>,
    /// Password expiration
    pub password_expiration: Option<i64>,
    /// Maximum ticket life
    pub max_ticket_life: Option<String>,
    /// Maximum renewable life
    pub max_renewable_life: Option<String>,
}

/// Kerberos Manager
pub struct KerberosManager {
    config: KerberosConfig,
    krb5_conf: String,
    keytab_path: String,
}

impl KerberosManager {
    /// Create a new Kerberos manager
    pub fn new() -> Self {
        Self {
            config: KerberosConfig::default(),
            krb5_conf: "/etc/krb5.conf".to_string(),
            keytab_path: "/etc/krb5.keytab".to_string(),
        }
    }

    /// Set configuration
    pub fn set_config(&mut self, config: KerberosConfig) {
        self.config = config;
    }

    /// Generate krb5.conf content
    pub fn generate_config(&self) -> String {
        let mut config = String::new();

        // [libdefaults]
        config.push_str("[libdefaults]\n");
        config.push_str(&format!("    default_realm = {}\n", self.config.default_realm));
        config.push_str(&format!("    dns_lookup_kdc = {}\n", self.config.dns_lookup_kdc));
        config.push_str(&format!("    dns_lookup_realm = {}\n", self.config.dns_lookup_realm));
        config.push_str(&format!("    ticket_lifetime = {}\n", self.config.ticket_lifetime));
        config.push_str(&format!("    renew_lifetime = {}\n", self.config.renew_lifetime));
        config.push_str(&format!("    forwardable = {}\n", self.config.forwardable));
        config.push_str(&format!("    proxiable = {}\n", self.config.proxiable));
        config.push_str("    default_ccache_name = KEYRING:persistent:%{uid}\n");
        config.push('\n');

        // [realms]
        config.push_str("[realms]\n");
        for (realm, realm_config) in &self.config.realms {
            config.push_str(&format!("    {} = {{\n", realm));
            for kdc in &realm_config.kdc {
                config.push_str(&format!("        kdc = {}\n", kdc));
            }
            if let Some(ref admin) = realm_config.admin_server {
                config.push_str(&format!("        admin_server = {}\n", admin));
            }
            if let Some(ref domain) = realm_config.default_domain {
                config.push_str(&format!("        default_domain = {}\n", domain));
            }
            config.push_str("    }\n");
        }
        config.push('\n');

        // [domain_realm]
        if !self.config.domain_realm.is_empty() {
            config.push_str("[domain_realm]\n");
            for (domain, realm) in &self.config.domain_realm {
                config.push_str(&format!("    {} = {}\n", domain, realm));
                config.push_str(&format!("    .{} = {}\n", domain, realm));
            }
            config.push('\n');
        }

        config
    }

    /// Write krb5.conf
    pub async fn write_config(&self) -> Result<()> {
        let config = self.generate_config();
        tokio::fs::write(&self.krb5_conf, config).await
            .map_err(|e| Error::Internal(format!("Failed to write krb5.conf: {}", e)))
    }

    /// Test Kerberos configuration
    pub async fn test_config(&self) -> Result<bool> {
        let output = Command::new("kinit")
            .args(["--version"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("kinit failed: {}", e)))?;

        Ok(output.status.success())
    }

    /// Obtain TGT for a principal
    pub async fn kinit(&self, principal: &str, password: &str) -> Result<()> {
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

    /// Obtain TGT using keytab
    pub async fn kinit_keytab(&self, principal: &str, keytab: Option<&str>) -> Result<()> {
        let keytab_path = keytab.unwrap_or(&self.keytab_path);

        let output = Command::new("kinit")
            .args(["-k", "-t", keytab_path, principal])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("kinit failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Authentication(format!("kinit failed: {}", stderr)));
        }

        Ok(())
    }

    /// Destroy credentials
    pub async fn kdestroy(&self) -> Result<()> {
        let output = Command::new("kdestroy")
            .output()
            .await
            .map_err(|e| Error::Internal(format!("kdestroy failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("kdestroy failed: {}", stderr)));
        }

        Ok(())
    }

    /// List current tickets
    pub async fn klist(&self) -> Result<Vec<KerberosTicket>> {
        let output = Command::new("klist")
            .args(["-l"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("klist failed: {}", e)))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(Self::parse_klist(&stdout))
    }

    /// Parse klist output
    fn parse_klist(output: &str) -> Vec<KerberosTicket> {
        let mut tickets = Vec::new();

        for line in output.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with("Principal") || line.starts_with("---") {
                continue;
            }

            // Parse ticket entry (simplified)
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                tickets.push(KerberosTicket {
                    principal: parts.last().unwrap_or(&"").to_string(),
                    issue_time: chrono::Utc::now().timestamp(),
                    expire_time: chrono::Utc::now().timestamp() + 86400,
                    renew_until: None,
                    flags: Vec::new(),
                });
            }
        }

        tickets
    }

    /// Create a keytab entry
    pub async fn create_keytab(&self, principal: &str, password: &str, keytab: Option<&str>) -> Result<()> {
        let keytab_path = keytab.unwrap_or(&self.keytab_path);

        // Use ktutil to create keytab
        let ktutil_input = format!(
            "add_entry -password -p {} -k 1 -e aes256-cts-hmac-sha1-96\n{}\nwrite_kt {}\nquit\n",
            principal,
            password,
            keytab_path
        );

        let mut child = Command::new("ktutil")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| Error::Internal(format!("ktutil failed: {}", e)))?;

        if let Some(stdin) = child.stdin.as_mut() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(ktutil_input.as_bytes()).await
                .map_err(|e| Error::Internal(format!("Failed to write to ktutil: {}", e)))?;
        }

        let output = child.wait_with_output().await
            .map_err(|e| Error::Internal(format!("ktutil failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("ktutil failed: {}", stderr)));
        }

        // Set permissions
        let _ = Command::new("chmod")
            .args(["600", keytab_path])
            .output()
            .await;

        Ok(())
    }

    /// List keytab entries
    pub async fn list_keytab(&self, keytab: Option<&str>) -> Result<Vec<String>> {
        let keytab_path = keytab.unwrap_or(&self.keytab_path);

        let output = Command::new("klist")
            .args(["-k", keytab_path])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("klist failed: {}", e)))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut principals = Vec::new();

        for line in stdout.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with("Keytab") || line.starts_with("KVNO") || line.starts_with("---") {
                continue;
            }

            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                principals.push(parts.last().unwrap_or(&"").to_string());
            }
        }

        Ok(principals)
    }

    /// Verify keytab can authenticate
    pub async fn verify_keytab(&self, principal: &str, keytab: Option<&str>) -> Result<bool> {
        match self.kinit_keytab(principal, keytab).await {
            Ok(_) => {
                let _ = self.kdestroy().await;
                Ok(true)
            }
            Err(_) => Ok(false)
        }
    }

    /// Get Kerberos status
    pub async fn get_status(&self) -> Result<KerberosStatus> {
        let config_exists = tokio::fs::metadata(&self.krb5_conf).await.is_ok();
        let keytab_exists = tokio::fs::metadata(&self.keytab_path).await.is_ok();
        let tickets = self.klist().await.unwrap_or_default();

        Ok(KerberosStatus {
            configured: config_exists,
            default_realm: self.config.default_realm.clone(),
            keytab_exists,
            active_tickets: tickets.len() as u32,
            realms: self.config.realms.keys().cloned().collect(),
        })
    }

    /// Configure PAM for Kerberos authentication
    pub async fn configure_pam(&self) -> Result<()> {
        // Generate pam_krb5.conf
        let pam_config = format!(r#"# Horcrux Kerberos PAM configuration
[pam]
    krb5_auth = true
    krb5_ccache_type = KEYRING
    krb5_validate = true
    ticket_lifetime = {}
    renew_lifetime = {}
"#,
            self.config.ticket_lifetime,
            self.config.renew_lifetime,
        );

        tokio::fs::write("/etc/security/pam_krb5.conf", pam_config).await
            .map_err(|e| Error::Internal(format!("Failed to write pam_krb5.conf: {}", e)))?;

        Ok(())
    }

    /// Add a realm to configuration
    pub fn add_realm(&mut self, realm: &str, kdc: Vec<String>, admin_server: Option<String>) {
        self.config.realms.insert(realm.to_string(), RealmConfig {
            kdc,
            admin_server,
            default_domain: None,
        });
    }

    /// Remove a realm from configuration
    pub fn remove_realm(&mut self, realm: &str) {
        self.config.realms.remove(realm);
    }

    /// Add domain to realm mapping
    pub fn add_domain_mapping(&mut self, domain: &str, realm: &str) {
        self.config.domain_realm.insert(domain.to_string(), realm.to_string());
    }
}

/// Kerberos status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KerberosStatus {
    pub configured: bool,
    pub default_realm: String,
    pub keytab_exists: bool,
    pub active_tickets: u32,
    pub realms: Vec<String>,
}

impl Default for KerberosManager {
    fn default() -> Self {
        Self::new()
    }
}

impl KerberosManager {
    /// Add entry to keytab with specific encryption types
    pub async fn add_keytab_entry(
        &self,
        principal: &str,
        password: &str,
        keytab: Option<&str>,
        enctypes: &[&str],
    ) -> Result<()> {
        let keytab_path = keytab.unwrap_or(&self.keytab_path);
        let enctypes = if enctypes.is_empty() {
            vec!["aes256-cts-hmac-sha1-96", "aes128-cts-hmac-sha1-96", "arcfour-hmac"]
        } else {
            enctypes.to_vec()
        };

        let mut ktutil_input = String::new();
        for enctype in &enctypes {
            ktutil_input.push_str(&format!(
                "add_entry -password -p {} -k 1 -e {}\n{}\n",
                principal, enctype, password
            ));
        }
        ktutil_input.push_str(&format!("write_kt {}\nquit\n", keytab_path));

        let mut child = Command::new("ktutil")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| Error::Internal(format!("ktutil failed: {}", e)))?;

        if let Some(stdin) = child.stdin.as_mut() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(ktutil_input.as_bytes()).await
                .map_err(|e| Error::Internal(format!("Failed to write to ktutil: {}", e)))?;
        }

        let output = child.wait_with_output().await
            .map_err(|e| Error::Internal(format!("ktutil failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("ktutil failed: {}", stderr)));
        }

        // Set proper permissions
        let _ = Command::new("chmod")
            .args(["600", keytab_path])
            .output()
            .await;

        Ok(())
    }

    /// Remove entry from keytab
    pub async fn remove_keytab_entry(&self, principal: &str, keytab: Option<&str>) -> Result<()> {
        let keytab_path = keytab.unwrap_or(&self.keytab_path);

        // List current entries and find slots to delete
        let entries = self.list_keytab(Some(keytab_path)).await?;
        let mut slots_to_delete: Vec<usize> = Vec::new();

        for (i, entry) in entries.iter().enumerate() {
            if entry.contains(principal) {
                slots_to_delete.push(i + 1); // ktutil uses 1-based indexing
            }
        }

        if slots_to_delete.is_empty() {
            return Ok(());
        }

        // Delete slots in reverse order to avoid index shifting
        slots_to_delete.reverse();

        let mut ktutil_input = format!("read_kt {}\n", keytab_path);
        for slot in &slots_to_delete {
            ktutil_input.push_str(&format!("delete_entry {}\n", slot));
        }
        ktutil_input.push_str(&format!("write_kt {}\nquit\n", keytab_path));

        let mut child = Command::new("ktutil")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| Error::Internal(format!("ktutil failed: {}", e)))?;

        if let Some(stdin) = child.stdin.as_mut() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(ktutil_input.as_bytes()).await.ok();
        }

        let _ = child.wait().await;
        Ok(())
    }

    /// Merge keytabs
    pub async fn merge_keytab(&self, src: &str, dst: &str) -> Result<()> {
        let ktutil_input = format!(
            "read_kt {}\nread_kt {}\nwrite_kt {}\nquit\n",
            dst, src, dst
        );

        let mut child = Command::new("ktutil")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| Error::Internal(format!("ktutil failed: {}", e)))?;

        if let Some(stdin) = child.stdin.as_mut() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(ktutil_input.as_bytes()).await.ok();
        }

        child.wait().await
            .map_err(|e| Error::Internal(format!("ktutil failed: {}", e)))?;

        Ok(())
    }

    /// Create service principal keytab
    pub async fn create_service_keytab(
        &self,
        service: &str,
        hostname: &str,
        realm: &str,
        password: &str,
        keytab: &str,
    ) -> Result<()> {
        let principal = format!("{}/{}@{}", service, hostname, realm);
        self.add_keytab_entry(&principal, password, Some(keytab), &[]).await
    }

    /// Generate random key for keytab (requires kadmin access)
    pub async fn kadmin_create_keytab(
        &self,
        principal: &str,
        keytab: &str,
        admin_principal: &str,
        admin_password: &str,
    ) -> Result<()> {
        let kadmin_input = format!(
            "ktadd -k {} {}\nquit\n",
            keytab, principal
        );

        let mut child = Command::new("kadmin")
            .args(["-p", admin_principal])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| Error::Internal(format!("kadmin failed: {}", e)))?;

        if let Some(stdin) = child.stdin.as_mut() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(format!("{}\n{}", admin_password, kadmin_input).as_bytes()).await.ok();
        }

        let output = child.wait_with_output().await
            .map_err(|e| Error::Internal(format!("kadmin failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("kadmin failed: {}", stderr)));
        }

        Ok(())
    }

    /// Renew TGT
    pub async fn krenew(&self) -> Result<()> {
        let output = Command::new("kinit")
            .args(["-R"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("kinit -R failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("kinit -R failed: {}", stderr)));
        }

        Ok(())
    }

    /// Get ticket details
    pub async fn get_ticket_details(&self) -> Result<TicketDetails> {
        let output = Command::new("klist")
            .args(["-e", "-f"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("klist failed: {}", e)))?;

        if !output.status.success() {
            return Err(Error::NotFound("No valid tickets".to_string()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(Self::parse_ticket_details(&stdout))
    }

    fn parse_ticket_details(output: &str) -> TicketDetails {
        let mut details = TicketDetails {
            principal: String::new(),
            cache_name: String::new(),
            tickets: Vec::new(),
        };

        let mut current_ticket: Option<TicketInfo> = None;

        for line in output.lines() {
            let line = line.trim();

            if line.starts_with("Ticket cache:") {
                details.cache_name = line.replace("Ticket cache:", "").trim().to_string();
            } else if line.starts_with("Default principal:") {
                details.principal = line.replace("Default principal:", "").trim().to_string();
            } else if line.contains("@") && !line.starts_with("Valid") && !line.starts_with("renew") {
                if let Some(ticket) = current_ticket.take() {
                    details.tickets.push(ticket);
                }

                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 5 {
                    current_ticket = Some(TicketInfo {
                        service_principal: parts.last().unwrap_or(&"").to_string(),
                        valid_starting: format!("{} {}", parts.get(0).unwrap_or(&""), parts.get(1).unwrap_or(&"")),
                        expires: format!("{} {}", parts.get(2).unwrap_or(&""), parts.get(3).unwrap_or(&"")),
                        renew_until: None,
                        flags: Vec::new(),
                        encryption: String::new(),
                    });
                }
            } else if line.starts_with("renew until") {
                if let Some(ref mut ticket) = current_ticket {
                    ticket.renew_until = Some(line.replace("renew until", "").trim().to_string());
                }
            } else if line.starts_with("Etype") {
                if let Some(ref mut ticket) = current_ticket {
                    ticket.encryption = line.to_string();
                }
            } else if line.starts_with("Flags:") {
                if let Some(ref mut ticket) = current_ticket {
                    ticket.flags = line.replace("Flags:", "")
                        .split_whitespace()
                        .map(|s| s.to_string())
                        .collect();
                }
            }
        }

        if let Some(ticket) = current_ticket {
            details.tickets.push(ticket);
        }

        details
    }
}

/// Detailed ticket information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TicketDetails {
    pub principal: String,
    pub cache_name: String,
    pub tickets: Vec<TicketInfo>,
}

/// Individual ticket information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TicketInfo {
    pub service_principal: String,
    pub valid_starting: String,
    pub expires: String,
    pub renew_until: Option<String>,
    pub flags: Vec<String>,
    pub encryption: String,
}
