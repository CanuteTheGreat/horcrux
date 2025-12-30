//! LDAP Client module
//!
//! Provides LDAP client functionality for authenticating users and groups
//! against external LDAP directories.

use horcrux_common::{Error, Result};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

/// LDAP Configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LdapConfig {
    /// LDAP server URI (ldap:// or ldaps://)
    pub uri: String,
    /// Base DN for searches
    pub base_dn: String,
    /// Bind DN (for authenticated binds)
    pub bind_dn: Option<String>,
    /// Bind password
    #[serde(skip_serializing)]
    pub bind_password: Option<String>,
    /// User search base (relative to base_dn)
    pub user_base: String,
    /// Group search base (relative to base_dn)
    pub group_base: String,
    /// User object class
    pub user_object_class: String,
    /// Group object class
    pub group_object_class: String,
    /// Username attribute
    pub uid_attribute: String,
    /// Group name attribute
    pub group_attribute: String,
    /// Member attribute (for group membership)
    pub member_attribute: String,
    /// Use TLS/StartTLS
    pub use_tls: bool,
    /// TLS CA certificate path
    pub tls_ca_cert: Option<String>,
    /// Skip TLS verification (insecure)
    pub tls_skip_verify: bool,
    /// Connection timeout in seconds
    pub timeout: u32,
}

impl Default for LdapConfig {
    fn default() -> Self {
        Self {
            uri: "ldap://localhost:389".to_string(),
            base_dn: "dc=example,dc=com".to_string(),
            bind_dn: None,
            bind_password: None,
            user_base: "ou=users".to_string(),
            group_base: "ou=groups".to_string(),
            user_object_class: "posixAccount".to_string(),
            group_object_class: "posixGroup".to_string(),
            uid_attribute: "uid".to_string(),
            group_attribute: "cn".to_string(),
            member_attribute: "memberUid".to_string(),
            use_tls: false,
            tls_ca_cert: None,
            tls_skip_verify: false,
            timeout: 10,
        }
    }
}

/// LDAP User entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LdapUser {
    /// Distinguished Name
    pub dn: String,
    /// Username (uid)
    pub uid: String,
    /// Unix UID number
    pub uid_number: u32,
    /// Primary GID number
    pub gid_number: u32,
    /// Common name
    pub cn: String,
    /// Home directory
    pub home_directory: Option<String>,
    /// Login shell
    pub login_shell: Option<String>,
    /// Email address
    pub mail: Option<String>,
    /// Display name
    pub display_name: Option<String>,
}

/// LDAP Group entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LdapGroup {
    /// Distinguished Name
    pub dn: String,
    /// Group name (cn)
    pub cn: String,
    /// Unix GID number
    pub gid_number: u32,
    /// Member UIDs
    pub members: Vec<String>,
    /// Description
    pub description: Option<String>,
}

/// LDAP Client Manager
pub struct LdapClient {
    config: LdapConfig,
}

impl LdapClient {
    /// Create a new LDAP client
    pub fn new(config: LdapConfig) -> Self {
        Self { config }
    }

    /// Test connection to LDAP server
    pub async fn test_connection(&self) -> Result<bool> {
        let mut args = vec![
            "-x".to_string(),
            "-H".to_string(),
            self.config.uri.clone(),
            "-b".to_string(),
            "".to_string(),
            "-s".to_string(),
            "base".to_string(),
        ];

        if let Some(ref bind_dn) = self.config.bind_dn {
            args.push("-D".to_string());
            args.push(bind_dn.clone());
        }

        if let Some(ref password) = self.config.bind_password {
            args.push("-w".to_string());
            args.push(password.clone());
        }

        if self.config.use_tls {
            args.push("-ZZ".to_string());
        }

        let output = Command::new("ldapsearch")
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("ldapsearch failed: {}", e)))?;

        Ok(output.status.success())
    }

    /// Search for users
    pub async fn search_users(&self, filter: Option<&str>) -> Result<Vec<LdapUser>> {
        let user_filter = filter.unwrap_or(&format!("(objectClass={})", self.config.user_object_class));
        let search_base = format!("{},{}", self.config.user_base, self.config.base_dn);

        let output = self.ldap_search(&search_base, user_filter, &[
            "dn", "uid", "uidNumber", "gidNumber", "cn", "homeDirectory",
            "loginShell", "mail", "displayName"
        ]).await?;

        Ok(Self::parse_user_entries(&output))
    }

    /// Get a specific user by UID
    pub async fn get_user(&self, uid: &str) -> Result<LdapUser> {
        let filter = format!("(&(objectClass={})({}={}))",
            self.config.user_object_class,
            self.config.uid_attribute,
            uid
        );
        let search_base = format!("{},{}", self.config.user_base, self.config.base_dn);

        let output = self.ldap_search(&search_base, &filter, &[
            "dn", "uid", "uidNumber", "gidNumber", "cn", "homeDirectory",
            "loginShell", "mail", "displayName"
        ]).await?;

        let users = Self::parse_user_entries(&output);
        users.into_iter().next()
            .ok_or_else(|| Error::NotFound(format!("User '{}' not found", uid)))
    }

    /// Search for groups
    pub async fn search_groups(&self, filter: Option<&str>) -> Result<Vec<LdapGroup>> {
        let group_filter = filter.unwrap_or(&format!("(objectClass={})", self.config.group_object_class));
        let search_base = format!("{},{}", self.config.group_base, self.config.base_dn);

        let output = self.ldap_search(&search_base, group_filter, &[
            "dn", "cn", "gidNumber", "memberUid", "description"
        ]).await?;

        Ok(Self::parse_group_entries(&output))
    }

    /// Get a specific group by CN
    pub async fn get_group(&self, cn: &str) -> Result<LdapGroup> {
        let filter = format!("(&(objectClass={})({}={}))",
            self.config.group_object_class,
            self.config.group_attribute,
            cn
        );
        let search_base = format!("{},{}", self.config.group_base, self.config.base_dn);

        let output = self.ldap_search(&search_base, &filter, &[
            "dn", "cn", "gidNumber", "memberUid", "description"
        ]).await?;

        let groups = Self::parse_group_entries(&output);
        groups.into_iter().next()
            .ok_or_else(|| Error::NotFound(format!("Group '{}' not found", cn)))
    }

    /// Authenticate a user with password
    pub async fn authenticate(&self, uid: &str, password: &str) -> Result<bool> {
        // First, find the user's DN
        let user = self.get_user(uid).await?;

        // Attempt bind with user's credentials
        let args = vec![
            "-x",
            "-H", &self.config.uri,
            "-D", &user.dn,
            "-w", password,
            "-b", &self.config.base_dn,
            "-s", "base",
        ];

        let output = Command::new("ldapsearch")
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("ldapsearch failed: {}", e)))?;

        Ok(output.status.success())
    }

    /// Get groups for a user
    pub async fn get_user_groups(&self, uid: &str) -> Result<Vec<String>> {
        let filter = format!("(&(objectClass={})({}={}))",
            self.config.group_object_class,
            self.config.member_attribute,
            uid
        );
        let search_base = format!("{},{}", self.config.group_base, self.config.base_dn);

        let output = self.ldap_search(&search_base, &filter, &["cn"]).await?;

        let mut groups = Vec::new();
        for line in output.lines() {
            if line.starts_with("cn: ") {
                groups.push(line[4..].to_string());
            }
        }

        Ok(groups)
    }

    /// Internal ldapsearch wrapper
    async fn ldap_search(&self, base: &str, filter: &str, attrs: &[&str]) -> Result<String> {
        let mut args = vec![
            "-x".to_string(),
            "-H".to_string(),
            self.config.uri.clone(),
            "-b".to_string(),
            base.to_string(),
            "-LLL".to_string(),
        ];

        if let Some(ref bind_dn) = self.config.bind_dn {
            args.push("-D".to_string());
            args.push(bind_dn.clone());
        }

        if let Some(ref password) = self.config.bind_password {
            args.push("-w".to_string());
            args.push(password.clone());
        }

        if self.config.use_tls {
            args.push("-ZZ".to_string());
        }

        args.push(filter.to_string());
        args.extend(attrs.iter().map(|a| a.to_string()));

        let output = Command::new("ldapsearch")
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("ldapsearch failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("LDAP search failed: {}", stderr)));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Parse LDIF output into user entries
    fn parse_user_entries(ldif: &str) -> Vec<LdapUser> {
        let mut users = Vec::new();
        let mut current: Option<LdapUser> = None;

        for line in ldif.lines() {
            let line = line.trim();

            if line.is_empty() {
                if let Some(user) = current.take() {
                    users.push(user);
                }
                continue;
            }

            if line.starts_with("dn: ") {
                current = Some(LdapUser {
                    dn: line[4..].to_string(),
                    uid: String::new(),
                    uid_number: 0,
                    gid_number: 0,
                    cn: String::new(),
                    home_directory: None,
                    login_shell: None,
                    mail: None,
                    display_name: None,
                });
            }

            if let Some(ref mut user) = current {
                if line.starts_with("uid: ") {
                    user.uid = line[5..].to_string();
                } else if line.starts_with("uidNumber: ") {
                    user.uid_number = line[11..].parse().unwrap_or(0);
                } else if line.starts_with("gidNumber: ") {
                    user.gid_number = line[11..].parse().unwrap_or(0);
                } else if line.starts_with("cn: ") {
                    user.cn = line[4..].to_string();
                } else if line.starts_with("homeDirectory: ") {
                    user.home_directory = Some(line[15..].to_string());
                } else if line.starts_with("loginShell: ") {
                    user.login_shell = Some(line[12..].to_string());
                } else if line.starts_with("mail: ") {
                    user.mail = Some(line[6..].to_string());
                } else if line.starts_with("displayName: ") {
                    user.display_name = Some(line[13..].to_string());
                }
            }
        }

        if let Some(user) = current {
            users.push(user);
        }

        users
    }

    /// Parse LDIF output into group entries
    fn parse_group_entries(ldif: &str) -> Vec<LdapGroup> {
        let mut groups = Vec::new();
        let mut current: Option<LdapGroup> = None;

        for line in ldif.lines() {
            let line = line.trim();

            if line.is_empty() {
                if let Some(group) = current.take() {
                    groups.push(group);
                }
                continue;
            }

            if line.starts_with("dn: ") {
                current = Some(LdapGroup {
                    dn: line[4..].to_string(),
                    cn: String::new(),
                    gid_number: 0,
                    members: Vec::new(),
                    description: None,
                });
            }

            if let Some(ref mut group) = current {
                if line.starts_with("cn: ") {
                    group.cn = line[4..].to_string();
                } else if line.starts_with("gidNumber: ") {
                    group.gid_number = line[11..].parse().unwrap_or(0);
                } else if line.starts_with("memberUid: ") {
                    group.members.push(line[11..].to_string());
                } else if line.starts_with("description: ") {
                    group.description = Some(line[13..].to_string());
                }
            }
        }

        if let Some(group) = current {
            groups.push(group);
        }

        groups
    }

    /// Configure NSS to use LDAP
    pub async fn configure_nss(&self) -> Result<()> {
        // Generate nslcd.conf for nss-pam-ldapd
        let config = format!(r#"# Horcrux NAS LDAP NSS configuration
uid nslcd
gid nslcd
uri {}
base {}
binddn {}
bindpw {}
ssl {}
tls_cacertfile {}
scope sub
filter passwd (objectClass={})
filter shadow (objectClass={})
filter group (objectClass={})
"#,
            self.config.uri,
            self.config.base_dn,
            self.config.bind_dn.as_deref().unwrap_or(""),
            self.config.bind_password.as_deref().unwrap_or(""),
            if self.config.use_tls { "start_tls" } else { "off" },
            self.config.tls_ca_cert.as_deref().unwrap_or("/etc/ssl/certs/ca-certificates.crt"),
            self.config.user_object_class,
            self.config.user_object_class,
            self.config.group_object_class,
        );

        tokio::fs::write("/etc/nslcd.conf", config).await
            .map_err(|e| Error::Internal(format!("Failed to write nslcd.conf: {}", e)))?;

        // Restart nslcd
        let _ = Command::new("systemctl")
            .args(["restart", "nslcd"])
            .output()
            .await;

        Ok(())
    }

    /// Get LDAP client status
    pub async fn get_status(&self) -> Result<LdapClientStatus> {
        let connected = self.test_connection().await.unwrap_or(false);
        let user_count = if connected {
            self.search_users(None).await.map(|u| u.len()).unwrap_or(0) as u32
        } else {
            0
        };
        let group_count = if connected {
            self.search_groups(None).await.map(|g| g.len()).unwrap_or(0) as u32
        } else {
            0
        };

        Ok(LdapClientStatus {
            connected,
            server_uri: self.config.uri.clone(),
            base_dn: self.config.base_dn.clone(),
            user_count,
            group_count,
            tls_enabled: self.config.use_tls,
        })
    }
}

/// LDAP client status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LdapClientStatus {
    pub connected: bool,
    pub server_uri: String,
    pub base_dn: String,
    pub user_count: u32,
    pub group_count: u32,
    pub tls_enabled: bool,
}

impl Default for LdapClient {
    fn default() -> Self {
        Self::new(LdapConfig::default())
    }
}

impl LdapClient {
    /// Configure PAM to use LDAP authentication
    pub async fn configure_pam(&self) -> Result<()> {
        // Create PAM LDAP configuration
        let pam_config = format!(r#"# Horcrux NAS PAM LDAP configuration
base {}
uri {}
binddn {}
bindpw {}
pam_password md5
ssl {}
tls_cacertfile {}
"#,
            self.config.base_dn,
            self.config.uri,
            self.config.bind_dn.as_deref().unwrap_or(""),
            self.config.bind_password.as_deref().unwrap_or(""),
            if self.config.use_tls { "start_tls" } else { "off" },
            self.config.tls_ca_cert.as_deref().unwrap_or("/etc/ssl/certs/ca-certificates.crt"),
        );

        tokio::fs::write("/etc/pam_ldap.conf", pam_config).await
            .map_err(|e| Error::Internal(format!("Failed to write pam_ldap.conf: {}", e)))?;

        // Configure /etc/nsswitch.conf for LDAP
        let nsswitch = r#"passwd:     files ldap
group:      files ldap
shadow:     files ldap
hosts:      files dns
networks:   files
protocols:  files
services:   files
ethers:     files
rpc:        files
"#;
        tokio::fs::write("/etc/nsswitch.conf", nsswitch).await
            .map_err(|e| Error::Internal(format!("Failed to write nsswitch.conf: {}", e)))?;

        Ok(())
    }

    /// Sync LDAP users to local system
    pub async fn sync_users(&self) -> Result<SyncResult> {
        let ldap_users = self.search_users(None).await?;
        let mut created = 0;
        let mut updated = 0;
        let mut errors = Vec::new();

        for user in ldap_users {
            // Check if user exists
            let output = Command::new("id")
                .arg(&user.uid)
                .output()
                .await;

            if output.map(|o| o.status.success()).unwrap_or(false) {
                // Update existing user
                let result = Command::new("usermod")
                    .args([
                        "-u", &user.uid_number.to_string(),
                        "-g", &user.gid_number.to_string(),
                        "-c", &user.cn,
                        "-d", user.home_directory.as_deref().unwrap_or(&format!("/home/{}", user.uid)),
                        "-s", user.login_shell.as_deref().unwrap_or("/bin/bash"),
                        &user.uid,
                    ])
                    .output()
                    .await;

                if result.map(|o| o.status.success()).unwrap_or(false) {
                    updated += 1;
                } else {
                    errors.push(format!("Failed to update user: {}", user.uid));
                }
            } else {
                // Create new user
                let result = Command::new("useradd")
                    .args([
                        "-u", &user.uid_number.to_string(),
                        "-g", &user.gid_number.to_string(),
                        "-c", &user.cn,
                        "-d", user.home_directory.as_deref().unwrap_or(&format!("/home/{}", user.uid)),
                        "-s", user.login_shell.as_deref().unwrap_or("/bin/bash"),
                        "-m",
                        &user.uid,
                    ])
                    .output()
                    .await;

                if result.map(|o| o.status.success()).unwrap_or(false) {
                    created += 1;
                } else {
                    errors.push(format!("Failed to create user: {}", user.uid));
                }
            }
        }

        Ok(SyncResult {
            created,
            updated,
            deleted: 0,
            errors,
        })
    }

    /// Sync LDAP groups to local system
    pub async fn sync_groups(&self) -> Result<SyncResult> {
        let ldap_groups = self.search_groups(None).await?;
        let mut created = 0;
        let mut updated = 0;
        let mut errors = Vec::new();

        for group in ldap_groups {
            // Check if group exists
            let output = Command::new("getent")
                .args(["group", &group.cn])
                .output()
                .await;

            if output.map(|o| o.status.success()).unwrap_or(false) {
                // Update existing group (mainly members)
                let result = Command::new("groupmod")
                    .args(["-g", &group.gid_number.to_string(), &group.cn])
                    .output()
                    .await;

                if result.map(|o| o.status.success()).unwrap_or(false) {
                    // Sync members
                    for member in &group.members {
                        let _ = Command::new("usermod")
                            .args(["-aG", &group.cn, member])
                            .output()
                            .await;
                    }
                    updated += 1;
                } else {
                    errors.push(format!("Failed to update group: {}", group.cn));
                }
            } else {
                // Create new group
                let result = Command::new("groupadd")
                    .args(["-g", &group.gid_number.to_string(), &group.cn])
                    .output()
                    .await;

                if result.map(|o| o.status.success()).unwrap_or(false) {
                    // Add members
                    for member in &group.members {
                        let _ = Command::new("usermod")
                            .args(["-aG", &group.cn, member])
                            .output()
                            .await;
                    }
                    created += 1;
                } else {
                    errors.push(format!("Failed to create group: {}", group.cn));
                }
            }
        }

        Ok(SyncResult {
            created,
            updated,
            deleted: 0,
            errors,
        })
    }

    /// Search users by attribute
    pub async fn search_users_by(&self, attr: &str, value: &str) -> Result<Vec<LdapUser>> {
        let filter = format!("(&(objectClass={})({}={}))",
            self.config.user_object_class,
            attr,
            value
        );
        self.search_users(Some(&filter)).await
    }

    /// Check if user exists in LDAP
    pub async fn user_exists(&self, uid: &str) -> Result<bool> {
        match self.get_user(uid).await {
            Ok(_) => Ok(true),
            Err(Error::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Check if group exists in LDAP
    pub async fn group_exists(&self, cn: &str) -> Result<bool> {
        match self.get_group(cn).await {
            Ok(_) => Ok(true),
            Err(Error::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Get user by UID number
    pub async fn get_user_by_uid_number(&self, uid_number: u32) -> Result<LdapUser> {
        let filter = format!("(&(objectClass={})(uidNumber={}))",
            self.config.user_object_class,
            uid_number
        );
        let search_base = format!("{},{}", self.config.user_base, self.config.base_dn);

        let output = self.ldap_search(&search_base, &filter, &[
            "dn", "uid", "uidNumber", "gidNumber", "cn", "homeDirectory",
            "loginShell", "mail", "displayName"
        ]).await?;

        let users = Self::parse_user_entries(&output);
        users.into_iter().next()
            .ok_or_else(|| Error::NotFound(format!("User with UID {} not found", uid_number)))
    }

    /// Get group by GID number
    pub async fn get_group_by_gid_number(&self, gid_number: u32) -> Result<LdapGroup> {
        let filter = format!("(&(objectClass={})(gidNumber={}))",
            self.config.group_object_class,
            gid_number
        );
        let search_base = format!("{},{}", self.config.group_base, self.config.base_dn);

        let output = self.ldap_search(&search_base, &filter, &[
            "dn", "cn", "gidNumber", "memberUid", "description"
        ]).await?;

        let groups = Self::parse_group_entries(&output);
        groups.into_iter().next()
            .ok_or_else(|| Error::NotFound(format!("Group with GID {} not found", gid_number)))
    }
}

/// Sync result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncResult {
    pub created: u32,
    pub updated: u32,
    pub deleted: u32,
    pub errors: Vec<String>,
}
