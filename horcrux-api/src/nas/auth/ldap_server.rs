//! LDAP Server module
//!
//! Manages OpenLDAP slapd server for local directory services.

use horcrux_common::{Error, Result};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use std::collections::HashMap;

/// LDAP Server Configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LdapServerConfig {
    /// Base DN for the directory
    pub base_dn: String,
    /// Organization name
    pub organization: String,
    /// Admin DN
    pub admin_dn: String,
    /// Admin password (hashed)
    #[serde(skip_serializing)]
    pub admin_password: String,
    /// Listen URIs
    pub listen_uris: Vec<String>,
    /// Enable TLS
    pub tls_enabled: bool,
    /// TLS certificate path
    pub tls_cert: Option<String>,
    /// TLS key path
    pub tls_key: Option<String>,
    /// TLS CA certificate path
    pub tls_ca_cert: Option<String>,
    /// Log level
    pub log_level: String,
    /// Database backend (mdb, hdb)
    pub database_backend: String,
    /// Database directory
    pub database_dir: String,
    /// Enable memberOf overlay
    pub memberof_enabled: bool,
    /// Enable refint overlay
    pub refint_enabled: bool,
    /// Schema files to load
    pub schemas: Vec<String>,
}

impl Default for LdapServerConfig {
    fn default() -> Self {
        Self {
            base_dn: "dc=horcrux,dc=local".to_string(),
            organization: "Horcrux NAS".to_string(),
            admin_dn: "cn=admin,dc=horcrux,dc=local".to_string(),
            admin_password: String::new(),
            listen_uris: vec!["ldap:///".to_string(), "ldapi:///".to_string()],
            tls_enabled: false,
            tls_cert: None,
            tls_key: None,
            tls_ca_cert: None,
            log_level: "stats".to_string(),
            database_backend: "mdb".to_string(),
            database_dir: "/var/lib/ldap".to_string(),
            memberof_enabled: true,
            refint_enabled: true,
            schemas: vec![
                "core".to_string(),
                "cosine".to_string(),
                "inetorgperson".to_string(),
                "nis".to_string(),
            ],
        }
    }
}

/// LDAP Server Manager
pub struct LdapServerManager {
    config: LdapServerConfig,
    slapd_conf: String,
}

impl LdapServerManager {
    /// Create a new LDAP server manager
    pub fn new() -> Self {
        Self {
            config: LdapServerConfig::default(),
            slapd_conf: "/etc/ldap/slapd.d".to_string(),
        }
    }

    /// Set configuration
    pub fn set_config(&mut self, config: LdapServerConfig) {
        self.config = config;
    }

    /// Initialize LDAP server with base configuration
    pub async fn initialize(&self, admin_password: &str) -> Result<()> {
        // Create database directory
        tokio::fs::create_dir_all(&self.config.database_dir).await
            .map_err(|e| Error::Internal(format!("Failed to create database dir: {}", e)))?;

        // Generate password hash
        let password_hash = self.hash_password(admin_password).await?;

        // Generate initial LDIF
        let init_ldif = self.generate_init_ldif(&password_hash);

        // Write temp file
        let temp_path = "/tmp/horcrux_ldap_init.ldif";
        tokio::fs::write(temp_path, &init_ldif).await
            .map_err(|e| Error::Internal(format!("Failed to write init LDIF: {}", e)))?;

        // Stop slapd if running
        let _ = self.stop().await;

        // Remove existing config
        let _ = tokio::fs::remove_dir_all(&self.slapd_conf).await;
        tokio::fs::create_dir_all(&self.slapd_conf).await
            .map_err(|e| Error::Internal(format!("Failed to create slapd.d: {}", e)))?;

        // Initialize with slapadd
        let output = Command::new("slapadd")
            .args(["-F", &self.slapd_conf, "-n", "0", "-l", temp_path])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("slapadd failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("slapadd failed: {}", stderr)));
        }

        // Clean up temp file
        let _ = tokio::fs::remove_file(temp_path).await;

        // Set permissions
        let _ = Command::new("chown")
            .args(["-R", "openldap:openldap", &self.slapd_conf])
            .output()
            .await;

        let _ = Command::new("chown")
            .args(["-R", "openldap:openldap", &self.config.database_dir])
            .output()
            .await;

        // Start slapd
        self.start().await
    }

    /// Generate initialization LDIF
    fn generate_init_ldif(&self, password_hash: &str) -> String {
        let mut ldif = String::new();

        // Config database
        ldif.push_str(&format!(r#"dn: cn=config
objectClass: olcGlobal
cn: config
olcLogLevel: {}

dn: cn=schema,cn=config
objectClass: olcSchemaConfig
cn: schema

"#, self.config.log_level));

        // Include schemas
        for schema in &self.config.schemas {
            ldif.push_str(&format!("include: file:///etc/ldap/schema/{}.ldif\n", schema));
        }

        ldif.push_str(&format!(r#"
dn: olcDatabase={{0}}config,cn=config
objectClass: olcDatabaseConfig
olcDatabase: {{0}}config
olcRootDN: cn=admin,cn=config
olcRootPW: {}

dn: olcDatabase={{1}}{},cn=config
objectClass: olcDatabaseConfig
objectClass: olcMdbConfig
olcDatabase: {{1}}{}
olcSuffix: {}
olcRootDN: {}
olcRootPW: {}
olcDbDirectory: {}
olcDbIndex: objectClass eq
olcDbIndex: uid eq
olcDbIndex: cn eq
olcDbIndex: gidNumber eq
olcDbIndex: uidNumber eq
olcDbIndex: memberUid eq
olcDbIndex: member eq

"#,
            password_hash,
            self.config.database_backend,
            self.config.database_backend,
            self.config.base_dn,
            self.config.admin_dn,
            password_hash,
            self.config.database_dir,
        ));

        // Add memberOf overlay if enabled
        if self.config.memberof_enabled {
            ldif.push_str(&format!(r#"dn: olcOverlay={{0}}memberof,olcDatabase={{1}}{},cn=config
objectClass: olcOverlayConfig
objectClass: olcMemberOf
olcOverlay: {{0}}memberof
olcMemberOfRefInt: TRUE
olcMemberOfGroupOC: groupOfNames
olcMemberOfMemberAD: member
olcMemberOfMemberOfAD: memberOf

"#, self.config.database_backend));
        }

        ldif
    }

    /// Hash password using slappasswd
    async fn hash_password(&self, password: &str) -> Result<String> {
        let output = Command::new("slappasswd")
            .args(["-s", password])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("slappasswd failed: {}", e)))?;

        if !output.status.success() {
            return Err(Error::Internal("Failed to hash password".to_string()));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Create base directory structure
    pub async fn create_base_structure(&self) -> Result<()> {
        let base_ldif = format!(r#"dn: {}
objectClass: dcObject
objectClass: organization
dc: {}
o: {}

dn: ou=users,{}
objectClass: organizationalUnit
ou: users

dn: ou=groups,{}
objectClass: organizationalUnit
ou: groups

dn: ou=services,{}
objectClass: organizationalUnit
ou: services

"#,
            self.config.base_dn,
            self.config.base_dn.split(',').next().unwrap_or("dc=local").replace("dc=", ""),
            self.config.organization,
            self.config.base_dn,
            self.config.base_dn,
            self.config.base_dn,
        );

        self.ldap_add(&base_ldif).await
    }

    /// Add a user to LDAP
    pub async fn add_user(&self, user: &LdapUserEntry) -> Result<()> {
        let ldif = format!(r#"dn: uid={},ou=users,{}
objectClass: inetOrgPerson
objectClass: posixAccount
objectClass: shadowAccount
uid: {}
cn: {}
sn: {}
uidNumber: {}
gidNumber: {}
homeDirectory: {}
loginShell: {}
userPassword: {}
"#,
            user.uid,
            self.config.base_dn,
            user.uid,
            user.cn,
            user.sn,
            user.uid_number,
            user.gid_number,
            user.home_directory,
            user.login_shell,
            user.password_hash,
        );

        self.ldap_add(&ldif).await
    }

    /// Delete a user from LDAP
    pub async fn delete_user(&self, uid: &str) -> Result<()> {
        let dn = format!("uid={},ou=users,{}", uid, self.config.base_dn);
        self.ldap_delete(&dn).await
    }

    /// Modify user password
    pub async fn set_user_password(&self, uid: &str, password: &str) -> Result<()> {
        let password_hash = self.hash_password(password).await?;
        let ldif = format!(r#"dn: uid={},ou=users,{}
changetype: modify
replace: userPassword
userPassword: {}
"#,
            uid,
            self.config.base_dn,
            password_hash,
        );

        self.ldap_modify(&ldif).await
    }

    /// Add a group to LDAP
    pub async fn add_group(&self, group: &LdapGroupEntry) -> Result<()> {
        let mut ldif = format!(r#"dn: cn={},ou=groups,{}
objectClass: posixGroup
cn: {}
gidNumber: {}
"#,
            group.cn,
            self.config.base_dn,
            group.cn,
            group.gid_number,
        );

        for member in &group.members {
            ldif.push_str(&format!("memberUid: {}\n", member));
        }

        self.ldap_add(&ldif).await
    }

    /// Delete a group from LDAP
    pub async fn delete_group(&self, cn: &str) -> Result<()> {
        let dn = format!("cn={},ou=groups,{}", cn, self.config.base_dn);
        self.ldap_delete(&dn).await
    }

    /// Add member to group
    pub async fn add_group_member(&self, group_cn: &str, uid: &str) -> Result<()> {
        let ldif = format!(r#"dn: cn={},ou=groups,{}
changetype: modify
add: memberUid
memberUid: {}
"#,
            group_cn,
            self.config.base_dn,
            uid,
        );

        self.ldap_modify(&ldif).await
    }

    /// Remove member from group
    pub async fn remove_group_member(&self, group_cn: &str, uid: &str) -> Result<()> {
        let ldif = format!(r#"dn: cn={},ou=groups,{}
changetype: modify
delete: memberUid
memberUid: {}
"#,
            group_cn,
            self.config.base_dn,
            uid,
        );

        self.ldap_modify(&ldif).await
    }

    /// Internal ldapadd wrapper
    async fn ldap_add(&self, ldif: &str) -> Result<()> {
        let temp_path = "/tmp/horcrux_ldap_add.ldif";
        tokio::fs::write(temp_path, ldif).await
            .map_err(|e| Error::Internal(format!("Failed to write LDIF: {}", e)))?;

        let output = Command::new("ldapadd")
            .args([
                "-x",
                "-H", "ldapi:///",
                "-D", &self.config.admin_dn,
                "-w", &self.config.admin_password,
                "-f", temp_path,
            ])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("ldapadd failed: {}", e)))?;

        let _ = tokio::fs::remove_file(temp_path).await;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("ldapadd failed: {}", stderr)));
        }

        Ok(())
    }

    /// Internal ldapmodify wrapper
    async fn ldap_modify(&self, ldif: &str) -> Result<()> {
        let temp_path = "/tmp/horcrux_ldap_modify.ldif";
        tokio::fs::write(temp_path, ldif).await
            .map_err(|e| Error::Internal(format!("Failed to write LDIF: {}", e)))?;

        let output = Command::new("ldapmodify")
            .args([
                "-x",
                "-H", "ldapi:///",
                "-D", &self.config.admin_dn,
                "-w", &self.config.admin_password,
                "-f", temp_path,
            ])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("ldapmodify failed: {}", e)))?;

        let _ = tokio::fs::remove_file(temp_path).await;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("ldapmodify failed: {}", stderr)));
        }

        Ok(())
    }

    /// Internal ldapdelete wrapper
    async fn ldap_delete(&self, dn: &str) -> Result<()> {
        let output = Command::new("ldapdelete")
            .args([
                "-x",
                "-H", "ldapi:///",
                "-D", &self.config.admin_dn,
                "-w", &self.config.admin_password,
                dn,
            ])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("ldapdelete failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("ldapdelete failed: {}", stderr)));
        }

        Ok(())
    }

    /// Start slapd service
    pub async fn start(&self) -> Result<()> {
        // Try systemd first
        let output = Command::new("systemctl")
            .args(["start", "slapd"])
            .output()
            .await;

        if let Ok(out) = output {
            if out.status.success() {
                return Ok(());
            }
        }

        // Try OpenRC
        let output = Command::new("rc-service")
            .args(["slapd", "start"])
            .output()
            .await;

        if let Ok(out) = output {
            if out.status.success() {
                return Ok(());
            }
        }

        Err(Error::Internal("Failed to start slapd".to_string()))
    }

    /// Stop slapd service
    pub async fn stop(&self) -> Result<()> {
        let output = Command::new("systemctl")
            .args(["stop", "slapd"])
            .output()
            .await;

        if let Ok(out) = output {
            if out.status.success() {
                return Ok(());
            }
        }

        let output = Command::new("rc-service")
            .args(["slapd", "stop"])
            .output()
            .await;

        if let Ok(out) = output {
            if out.status.success() {
                return Ok(());
            }
        }

        Err(Error::Internal("Failed to stop slapd".to_string()))
    }

    /// Restart slapd service
    pub async fn restart(&self) -> Result<()> {
        self.stop().await?;
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        self.start().await
    }

    /// Check if slapd is running
    pub async fn is_running(&self) -> bool {
        let output = Command::new("systemctl")
            .args(["is-active", "slapd"])
            .output()
            .await;

        if let Ok(out) = output {
            if out.status.success() {
                return true;
            }
        }

        let output = Command::new("rc-service")
            .args(["slapd", "status"])
            .output()
            .await;

        if let Ok(out) = output {
            return out.status.success();
        }

        false
    }

    /// Get server status
    pub async fn get_status(&self) -> Result<LdapServerStatus> {
        let running = self.is_running().await;

        Ok(LdapServerStatus {
            running,
            base_dn: self.config.base_dn.clone(),
            admin_dn: self.config.admin_dn.clone(),
            listen_uris: self.config.listen_uris.clone(),
            tls_enabled: self.config.tls_enabled,
            database_backend: self.config.database_backend.clone(),
        })
    }

    /// Backup LDAP database
    pub async fn backup(&self, output_path: &str) -> Result<()> {
        let output = Command::new("slapcat")
            .args(["-F", &self.slapd_conf, "-l", output_path])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("slapcat failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("slapcat failed: {}", stderr)));
        }

        Ok(())
    }

    /// Restore LDAP database from backup
    pub async fn restore(&self, input_path: &str) -> Result<()> {
        // Stop slapd
        let _ = self.stop().await;

        // Clear database
        let _ = tokio::fs::remove_dir_all(&self.config.database_dir).await;
        tokio::fs::create_dir_all(&self.config.database_dir).await
            .map_err(|e| Error::Internal(format!("Failed to create database dir: {}", e)))?;

        // Restore
        let output = Command::new("slapadd")
            .args(["-F", &self.slapd_conf, "-l", input_path])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("slapadd failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("slapadd failed: {}", stderr)));
        }

        // Fix permissions
        let _ = Command::new("chown")
            .args(["-R", "openldap:openldap", &self.config.database_dir])
            .output()
            .await;

        // Start slapd
        self.start().await
    }
}

/// LDAP user entry for creation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LdapUserEntry {
    pub uid: String,
    pub cn: String,
    pub sn: String,
    pub uid_number: u32,
    pub gid_number: u32,
    pub home_directory: String,
    pub login_shell: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
}

/// LDAP group entry for creation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LdapGroupEntry {
    pub cn: String,
    pub gid_number: u32,
    pub members: Vec<String>,
}

/// LDAP server status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LdapServerStatus {
    pub running: bool,
    pub base_dn: String,
    pub admin_dn: String,
    pub listen_uris: Vec<String>,
    pub tls_enabled: bool,
    pub database_backend: String,
}

impl Default for LdapServerManager {
    fn default() -> Self {
        Self::new()
    }
}
