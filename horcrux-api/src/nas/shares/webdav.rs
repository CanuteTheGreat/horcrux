//! WebDAV file sharing module
//!
//! Manages WebDAV access via nginx with full SSL/TLS support,
//! authentication, CalDAV/CardDAV extensions, and lock management.

use horcrux_common::{Error, Result};
use crate::nas::shares::{NasShare, WebDavConfig};
use tokio::process::Command;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Simplified WebDAV global configuration for API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebdavGlobalConfig {
    /// HTTP listen port
    pub listen_port: u16,
    /// Enable SSL/TLS
    pub ssl_enabled: bool,
    /// HTTPS listen port
    pub ssl_port: u16,
    /// SSL certificate path
    pub ssl_certificate: Option<String>,
    /// SSL certificate key path
    pub ssl_certificate_key: Option<String>,
    /// Authentication type
    pub auth_type: WebdavAuthType,
    /// Authentication realm
    pub realm: String,
    /// Client body temp path
    pub client_body_temp_path: String,
    /// Maximum client body size
    pub client_max_body_size: String,
}

impl Default for WebdavGlobalConfig {
    fn default() -> Self {
        Self {
            listen_port: 8080,
            ssl_enabled: false,
            ssl_port: 8443,
            ssl_certificate: None,
            ssl_certificate_key: None,
            auth_type: WebdavAuthType::Basic,
            realm: "WebDAV".to_string(),
            client_body_temp_path: "/var/lib/nginx/webdav".to_string(),
            client_max_body_size: "10G".to_string(),
        }
    }
}

/// WebDAV authentication type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WebdavAuthType {
    /// No authentication
    None,
    /// HTTP Basic authentication
    Basic,
    /// HTTP Digest authentication
    Digest,
    /// LDAP authentication
    Ldap,
    /// PAM authentication
    Pam,
}

impl Default for WebdavAuthType {
    fn default() -> Self {
        Self::Basic
    }
}

/// WebDAV authentication type (legacy alias)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WebDavAuthType {
    /// No authentication
    None,
    /// HTTP Basic authentication
    Basic,
    /// HTTP Digest authentication
    Digest,
    /// LDAP authentication via nginx-auth-ldap
    Ldap,
    /// PAM authentication
    Pam,
}

impl Default for WebDavAuthType {
    fn default() -> Self {
        Self::Basic
    }
}

/// SSL/TLS configuration for WebDAV
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebDavSslConfig {
    /// Enable SSL/TLS
    pub enabled: bool,
    /// Path to SSL certificate
    pub certificate: String,
    /// Path to SSL private key
    pub certificate_key: String,
    /// Path to CA chain (optional)
    pub ca_chain: Option<String>,
    /// Enable HSTS
    pub hsts_enabled: bool,
    /// HSTS max-age in seconds
    pub hsts_max_age: u64,
    /// Minimum TLS version
    pub min_tls_version: String,
    /// SSL cipher suite
    pub ciphers: Option<String>,
}

impl Default for WebDavSslConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            certificate: String::new(),
            certificate_key: String::new(),
            ca_chain: None,
            hsts_enabled: true,
            hsts_max_age: 31536000,
            min_tls_version: "TLSv1.2".to_string(),
            ciphers: None,
        }
    }
}

/// LDAP configuration for WebDAV authentication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebDavLdapConfig {
    /// LDAP server URL
    pub url: String,
    /// Bind DN for searches
    pub bind_dn: Option<String>,
    /// Bind password
    pub bind_password: Option<String>,
    /// User search base DN
    pub base_dn: String,
    /// User search filter (use %s for username)
    pub search_filter: String,
    /// Group DN for access control (optional)
    pub require_group: Option<String>,
    /// Enable StartTLS
    pub starttls: bool,
    /// Timeout in seconds
    pub timeout: u32,
}

impl Default for WebDavLdapConfig {
    fn default() -> Self {
        Self {
            url: "ldap://localhost:389".to_string(),
            bind_dn: None,
            bind_password: None,
            base_dn: "dc=example,dc=com".to_string(),
            search_filter: "(uid=%s)".to_string(),
            require_group: None,
            starttls: false,
            timeout: 10,
        }
    }
}

/// CalDAV/CardDAV configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DavExtensionsConfig {
    /// Enable CalDAV (calendar)
    pub caldav_enabled: bool,
    /// CalDAV mount path
    pub caldav_path: String,
    /// Enable CardDAV (contacts)
    pub carddav_enabled: bool,
    /// CardDAV mount path
    pub carddav_path: String,
    /// Principal URL path
    pub principal_path: String,
}

impl Default for DavExtensionsConfig {
    fn default() -> Self {
        Self {
            caldav_enabled: false,
            caldav_path: "/caldav".to_string(),
            carddav_enabled: false,
            carddav_path: "/carddav".to_string(),
            principal_path: "/principals/users".to_string(),
        }
    }
}

/// Extended WebDAV share configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtendedWebDavConfig {
    /// Base WebDAV config
    #[serde(flatten)]
    pub base: WebDavConfig,
    /// Authentication type
    pub auth_type: WebDavAuthType,
    /// SSL configuration
    pub ssl: WebDavSslConfig,
    /// LDAP configuration (when auth_type is Ldap)
    pub ldap: Option<WebDavLdapConfig>,
    /// CalDAV/CardDAV extensions
    pub dav_extensions: Option<DavExtensionsConfig>,
    /// Maximum upload size in bytes (0 = unlimited)
    pub max_upload_size: u64,
    /// Enable directory listing
    pub autoindex: bool,
    /// Custom nginx directives
    pub custom_directives: Option<String>,
    /// Rate limit requests per second (0 = disabled)
    pub rate_limit: u32,
    /// Listen port for this virtual host
    pub listen_port: u16,
    /// Server name (hostname)
    pub server_name: Option<String>,
    /// Enable access logging
    pub access_log: bool,
    /// Access log path
    pub access_log_path: Option<String>,
}

impl Default for ExtendedWebDavConfig {
    fn default() -> Self {
        Self {
            base: WebDavConfig::default(),
            auth_type: WebDavAuthType::Basic,
            ssl: WebDavSslConfig::default(),
            ldap: None,
            dav_extensions: None,
            max_upload_size: 0,
            autoindex: true,
            custom_directives: None,
            rate_limit: 0,
            listen_port: 80,
            server_name: None,
            access_log: true,
            access_log_path: None,
        }
    }
}

/// WebDAV user for htpasswd management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebDavUser {
    /// Username
    pub username: String,
    /// Whether user is enabled
    pub enabled: bool,
    /// User's home directory within WebDAV (optional)
    pub home_dir: Option<String>,
    /// User quota in bytes (0 = unlimited)
    pub quota: u64,
}

/// Active WebDAV connection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebDavConnection {
    /// Remote IP address
    pub remote_addr: String,
    /// Request method
    pub method: String,
    /// Request URI
    pub uri: String,
    /// HTTP status code
    pub status: u16,
    /// Bytes transferred
    pub bytes_sent: u64,
    /// Request time
    pub request_time: String,
    /// User agent
    pub user_agent: Option<String>,
    /// Authenticated username
    pub username: Option<String>,
}

/// WebDAV lock information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebDavLock {
    /// Lock token
    pub token: String,
    /// Locked resource path
    pub path: String,
    /// Lock owner
    pub owner: String,
    /// Lock type (write, exclusive)
    pub lock_type: String,
    /// Lock scope (exclusive, shared)
    pub scope: String,
    /// Lock depth (0, 1, infinity)
    pub depth: String,
    /// Lock timeout in seconds
    pub timeout: u64,
    /// Lock creation time
    pub created_at: String,
}

/// WebDAV service status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebDavStatus {
    /// Is nginx running
    pub nginx_running: bool,
    /// Is WebDAV module loaded
    pub dav_module_loaded: bool,
    /// Is DAV_ext module loaded
    pub dav_ext_loaded: bool,
    /// Number of active connections
    pub active_connections: u32,
    /// nginx version
    pub nginx_version: String,
    /// Number of configured shares
    pub configured_shares: u32,
    /// SSL enabled shares count
    pub ssl_enabled_shares: u32,
}

/// WebDAV Manager
pub struct WebDavManager {
    /// nginx configuration directory
    nginx_conf_dir: String,
    /// nginx sites-available directory
    sites_available_dir: String,
    /// nginx sites-enabled directory
    sites_enabled_dir: String,
    /// htpasswd file directory
    htpasswd_dir: String,
    /// Lock database directory
    lock_db_dir: String,
    /// Use systemd or OpenRC
    use_systemd: bool,
    /// Global configuration
    global_config: WebdavGlobalConfig,
}

/// Type alias for lowercase naming convention
pub type WebdavManager = WebDavManager;

impl WebDavManager {
    /// Create a new WebDAV manager
    pub fn new() -> Self {
        // Detect init system
        let use_systemd = Path::new("/run/systemd/system").exists();

        Self {
            nginx_conf_dir: "/etc/nginx/conf.d".to_string(),
            sites_available_dir: "/etc/nginx/sites-available".to_string(),
            sites_enabled_dir: "/etc/nginx/sites-enabled".to_string(),
            htpasswd_dir: "/etc/nginx/htpasswd".to_string(),
            lock_db_dir: "/var/lib/horcrux/webdav/locks".to_string(),
            use_systemd,
            global_config: WebdavGlobalConfig::default(),
        }
    }

    /// Get global configuration
    pub fn config(&self) -> &WebdavGlobalConfig {
        &self.global_config
    }

    /// Set global configuration
    pub fn set_global_config(&mut self, config: WebdavGlobalConfig) {
        self.global_config = config;
    }

    /// Reload nginx configuration
    pub async fn reload(&self) -> Result<()> {
        self.reload_nginx().await
    }

    /// Create with custom paths
    pub fn with_paths(
        nginx_conf_dir: &str,
        sites_available_dir: &str,
        sites_enabled_dir: &str,
        htpasswd_dir: &str,
    ) -> Self {
        let use_systemd = Path::new("/run/systemd/system").exists();

        Self {
            nginx_conf_dir: nginx_conf_dir.to_string(),
            sites_available_dir: sites_available_dir.to_string(),
            sites_enabled_dir: sites_enabled_dir.to_string(),
            htpasswd_dir: htpasswd_dir.to_string(),
            lock_db_dir: "/var/lib/horcrux/webdav/locks".to_string(),
            use_systemd,
            global_config: WebdavGlobalConfig::default(),
        }
    }

    /// Initialize required directories
    pub async fn init(&self) -> Result<()> {
        for dir in [
            &self.nginx_conf_dir,
            &self.sites_available_dir,
            &self.sites_enabled_dir,
            &self.htpasswd_dir,
            &self.lock_db_dir,
        ] {
            tokio::fs::create_dir_all(dir).await.map_err(|e| {
                Error::Internal(format!("Failed to create directory {}: {}", dir, e))
            })?;
        }
        Ok(())
    }

    /// Add a WebDAV share with basic config
    pub async fn add_share(&self, share: &NasShare) -> Result<()> {
        let config = self.generate_nginx_config(share);
        let conf_path = format!("{}/webdav-{}.conf", self.nginx_conf_dir, share.id);

        // Backup existing config
        if Path::new(&conf_path).exists() {
            let backup_path = format!("{}.bak", conf_path);
            tokio::fs::copy(&conf_path, &backup_path).await.ok();
        }

        tokio::fs::write(&conf_path, config).await.map_err(|e| {
            Error::Internal(format!("Failed to write WebDAV config: {}", e))
        })?;

        // Test config before reloading
        self.test_nginx_config().await?;
        self.reload_nginx().await
    }

    /// Add a WebDAV share with extended configuration
    pub async fn add_share_extended(
        &self,
        share: &NasShare,
        extended_config: &ExtendedWebDavConfig,
    ) -> Result<()> {
        let config = self.generate_extended_nginx_config(share, extended_config);
        let conf_path = format!("{}/webdav-{}.conf", self.sites_available_dir, share.id);

        // Backup existing config
        if Path::new(&conf_path).exists() {
            let backup_path = format!("{}.bak", conf_path);
            tokio::fs::copy(&conf_path, &backup_path).await.ok();
        }

        tokio::fs::write(&conf_path, &config).await.map_err(|e| {
            Error::Internal(format!("Failed to write WebDAV config: {}", e))
        })?;

        // Create symlink in sites-enabled
        let enabled_path = format!("{}/webdav-{}.conf", self.sites_enabled_dir, share.id);
        if !Path::new(&enabled_path).exists() {
            tokio::fs::symlink(&conf_path, &enabled_path).await.map_err(|e| {
                Error::Internal(format!("Failed to enable site: {}", e))
            })?;
        }

        // Create htpasswd file for this share if using basic auth
        if extended_config.auth_type == WebDavAuthType::Basic
            || extended_config.auth_type == WebDavAuthType::Digest
        {
            let htpasswd_path = format!("{}/webdav-{}", self.htpasswd_dir, share.id);
            if !Path::new(&htpasswd_path).exists() {
                tokio::fs::write(&htpasswd_path, "").await.ok();
            }
        }

        // Test config before reloading
        self.test_nginx_config().await?;
        self.reload_nginx().await
    }

    /// Remove a WebDAV share
    pub async fn remove_share(&self, share: &NasShare) -> Result<()> {
        // Remove from sites-enabled
        let enabled_path = format!("{}/webdav-{}.conf", self.sites_enabled_dir, share.id);
        if Path::new(&enabled_path).exists() {
            tokio::fs::remove_file(&enabled_path).await.ok();
        }

        // Remove from sites-available
        let available_path = format!("{}/webdav-{}.conf", self.sites_available_dir, share.id);
        if Path::new(&available_path).exists() {
            tokio::fs::remove_file(&available_path).await.ok();
        }

        // Remove from conf.d
        let conf_path = format!("{}/webdav-{}.conf", self.nginx_conf_dir, share.id);
        if Path::new(&conf_path).exists() {
            tokio::fs::remove_file(&conf_path).await.ok();
        }

        // Remove htpasswd file
        let htpasswd_path = format!("{}/webdav-{}", self.htpasswd_dir, share.id);
        if Path::new(&htpasswd_path).exists() {
            tokio::fs::remove_file(&htpasswd_path).await.ok();
        }

        self.reload_nginx().await
    }

    /// Enable an existing share
    pub async fn enable_share(&self, share_id: &str) -> Result<()> {
        let conf_path = format!("{}/webdav-{}.conf", self.sites_available_dir, share_id);
        let enabled_path = format!("{}/webdav-{}.conf", self.sites_enabled_dir, share_id);

        if !Path::new(&conf_path).exists() {
            return Err(Error::NotFound(format!(
                "WebDAV share {} not found",
                share_id
            )));
        }

        if !Path::new(&enabled_path).exists() {
            tokio::fs::symlink(&conf_path, &enabled_path).await.map_err(|e| {
                Error::Internal(format!("Failed to enable share: {}", e))
            })?;
        }

        self.reload_nginx().await
    }

    /// Disable an existing share
    pub async fn disable_share(&self, share_id: &str) -> Result<()> {
        let enabled_path = format!("{}/webdav-{}.conf", self.sites_enabled_dir, share_id);

        if Path::new(&enabled_path).exists() {
            tokio::fs::remove_file(&enabled_path).await.map_err(|e| {
                Error::Internal(format!("Failed to disable share: {}", e))
            })?;
        }

        self.reload_nginx().await
    }

    /// Generate nginx config for WebDAV share
    fn generate_nginx_config(&self, share: &NasShare) -> String {
        let config = share.webdav_config.as_ref().cloned().unwrap_or_default();

        format!(
            r#"# WebDAV share: {}
# Generated by Horcrux NAS Manager
# Do not edit manually

location /webdav/{} {{
    alias {};

    dav_methods PUT DELETE MKCOL COPY MOVE;
    dav_ext_methods PROPFIND OPTIONS LOCK UNLOCK;
    dav_access user:rw group:rw all:r;

    create_full_put_path on;

    autoindex on;
    autoindex_format json;

    {}

    client_max_body_size 0;
    client_body_temp_path /var/lib/nginx/webdav;

    # Lock support
    dav_ext_lock_zone zone=webdav:10m;
}}
"#,
            share.name,
            share.id,
            share.path,
            if config.auth_required {
                format!(
                    r#"auth_basic "WebDAV - {}";
    auth_basic_user_file {}/webdav-{};"#,
                    share.name, self.htpasswd_dir, share.id
                )
            } else {
                String::new()
            }
        )
    }

    /// Generate extended nginx config with SSL, LDAP, rate limiting, etc.
    fn generate_extended_nginx_config(
        &self,
        share: &NasShare,
        config: &ExtendedWebDavConfig,
    ) -> String {
        let mut server_block = String::new();

        // Server block start
        server_block.push_str("# WebDAV Virtual Host\n");
        server_block.push_str(&format!(
            "# Share: {} ({})\n",
            share.name, share.id
        ));
        server_block.push_str("# Generated by Horcrux NAS Manager\n\n");

        // Rate limiting zone
        if config.rate_limit > 0 {
            server_block.push_str(&format!(
                "limit_req_zone $binary_remote_addr zone=webdav_{}:10m rate={}r/s;\n\n",
                share.id, config.rate_limit
            ));
        }

        server_block.push_str("server {\n");

        // Listen directive
        if config.ssl.enabled {
            server_block.push_str(&format!(
                "    listen {} ssl http2;\n",
                config.listen_port
            ));
            server_block.push_str(&format!(
                "    listen [::]{} ssl http2;\n",
                config.listen_port
            ));
        } else {
            server_block.push_str(&format!("    listen {};\n", config.listen_port));
            server_block.push_str(&format!("    listen [::]{};\n", config.listen_port));
        }

        // Server name
        if let Some(ref server_name) = config.server_name {
            server_block.push_str(&format!("    server_name {};\n", server_name));
        } else {
            server_block.push_str("    server_name _;\n");
        }

        server_block.push('\n');

        // SSL configuration
        if config.ssl.enabled {
            server_block.push_str(&format!(
                "    ssl_certificate {};\n",
                config.ssl.certificate
            ));
            server_block.push_str(&format!(
                "    ssl_certificate_key {};\n",
                config.ssl.certificate_key
            ));

            if let Some(ref ca) = config.ssl.ca_chain {
                server_block.push_str(&format!("    ssl_trusted_certificate {};\n", ca));
            }

            server_block.push_str(&format!(
                "    ssl_protocols {} TLSv1.3;\n",
                config.ssl.min_tls_version
            ));

            if let Some(ref ciphers) = config.ssl.ciphers {
                server_block.push_str(&format!("    ssl_ciphers {};\n", ciphers));
            } else {
                server_block.push_str("    ssl_ciphers ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256:ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384;\n");
            }

            server_block.push_str("    ssl_prefer_server_ciphers on;\n");
            server_block.push_str("    ssl_session_cache shared:SSL:10m;\n");
            server_block.push_str("    ssl_session_timeout 1d;\n");
            server_block.push_str("    ssl_session_tickets off;\n");

            if config.ssl.hsts_enabled {
                server_block.push_str(&format!(
                    "    add_header Strict-Transport-Security \"max-age={}\" always;\n",
                    config.ssl.hsts_max_age
                ));
            }

            server_block.push('\n');
        }

        // Access logging
        if config.access_log {
            let log_path = config
                .access_log_path
                .clone()
                .unwrap_or_else(|| format!("/var/log/nginx/webdav-{}.access.log", share.id));
            server_block.push_str(&format!(
                "    access_log {} combined;\n",
                log_path
            ));
        } else {
            server_block.push_str("    access_log off;\n");
        }
        server_block.push_str(&format!(
            "    error_log /var/log/nginx/webdav-{}.error.log;\n\n",
            share.id
        ));

        // Root location
        server_block.push_str("    root /var/www/html;\n\n");

        // WebDAV location
        server_block.push_str(&format!(
            "    location /webdav/{} {{\n",
            share.id
        ));
        server_block.push_str(&format!("        alias {};\n\n", share.path));

        // DAV methods
        server_block.push_str("        dav_methods PUT DELETE MKCOL COPY MOVE;\n");
        server_block.push_str("        dav_ext_methods PROPFIND OPTIONS LOCK UNLOCK;\n");
        server_block.push_str("        dav_access user:rw group:rw all:r;\n");
        server_block.push_str("        create_full_put_path on;\n\n");

        // Autoindex
        if config.autoindex {
            server_block.push_str("        autoindex on;\n");
            server_block.push_str("        autoindex_format json;\n");
            server_block.push_str("        autoindex_exact_size off;\n");
            server_block.push_str("        autoindex_localtime on;\n\n");
        }

        // Authentication
        match config.auth_type {
            WebDavAuthType::None => {}
            WebDavAuthType::Basic => {
                server_block.push_str(&format!(
                    "        auth_basic \"WebDAV - {}\";\n",
                    share.name
                ));
                server_block.push_str(&format!(
                    "        auth_basic_user_file {}/webdav-{};\n\n",
                    self.htpasswd_dir, share.id
                ));
            }
            WebDavAuthType::Digest => {
                server_block.push_str(&format!(
                    "        auth_digest \"WebDAV - {}\";\n",
                    share.name
                ));
                server_block.push_str(&format!(
                    "        auth_digest_user_file {}/webdav-{}.digest;\n\n",
                    self.htpasswd_dir, share.id
                ));
            }
            WebDavAuthType::Ldap => {
                if let Some(ref ldap) = config.ldap {
                    server_block.push_str("        auth_ldap \"WebDAV LDAP Authentication\";\n");
                    server_block.push_str(&format!(
                        "        auth_ldap_servers ldap_{};\n\n",
                        share.id
                    ));
                }
            }
            WebDavAuthType::Pam => {
                server_block.push_str("        auth_pam \"WebDAV\";\n");
                server_block.push_str("        auth_pam_service_name \"nginx\";\n\n");
            }
        }

        // Rate limiting
        if config.rate_limit > 0 {
            server_block.push_str(&format!(
                "        limit_req zone=webdav_{} burst=50 nodelay;\n\n",
                share.id
            ));
        }

        // Upload size
        if config.max_upload_size > 0 {
            server_block.push_str(&format!(
                "        client_max_body_size {};\n",
                config.max_upload_size
            ));
        } else {
            server_block.push_str("        client_max_body_size 0;\n");
        }
        server_block.push_str("        client_body_temp_path /var/lib/nginx/webdav;\n\n");

        // Lock support
        server_block.push_str(&format!(
            "        dav_ext_lock zone=webdav_lock_{};\n\n",
            share.id
        ));

        // Custom directives
        if let Some(ref custom) = config.custom_directives {
            server_block.push_str("        # Custom directives\n");
            for line in custom.lines() {
                server_block.push_str(&format!("        {}\n", line));
            }
            server_block.push('\n');
        }

        server_block.push_str("    }\n");

        // CalDAV location
        if let Some(ref dav_ext) = config.dav_extensions {
            if dav_ext.caldav_enabled {
                server_block.push_str(&format!(
                    "\n    location {} {{\n",
                    dav_ext.caldav_path
                ));
                server_block.push_str(&format!(
                    "        alias {}/calendars;\n",
                    share.path
                ));
                server_block.push_str("        dav_methods PUT DELETE MKCOL COPY MOVE;\n");
                server_block.push_str("        dav_ext_methods PROPFIND OPTIONS LOCK UNLOCK REPORT;\n");
                server_block.push_str("        dav_access user:rw group:rw all:r;\n");
                server_block.push_str("        create_full_put_path on;\n");

                // Copy auth from main config
                match config.auth_type {
                    WebDavAuthType::Basic => {
                        server_block.push_str(&format!(
                            "        auth_basic \"CalDAV - {}\";\n",
                            share.name
                        ));
                        server_block.push_str(&format!(
                            "        auth_basic_user_file {}/webdav-{};\n",
                            self.htpasswd_dir, share.id
                        ));
                    }
                    _ => {}
                }

                server_block.push_str("    }\n");
            }

            if dav_ext.carddav_enabled {
                server_block.push_str(&format!(
                    "\n    location {} {{\n",
                    dav_ext.carddav_path
                ));
                server_block.push_str(&format!(
                    "        alias {}/contacts;\n",
                    share.path
                ));
                server_block.push_str("        dav_methods PUT DELETE MKCOL COPY MOVE;\n");
                server_block.push_str("        dav_ext_methods PROPFIND OPTIONS LOCK UNLOCK REPORT;\n");
                server_block.push_str("        dav_access user:rw group:rw all:r;\n");
                server_block.push_str("        create_full_put_path on;\n");

                // Copy auth from main config
                match config.auth_type {
                    WebDavAuthType::Basic => {
                        server_block.push_str(&format!(
                            "        auth_basic \"CardDAV - {}\";\n",
                            share.name
                        ));
                        server_block.push_str(&format!(
                            "        auth_basic_user_file {}/webdav-{};\n",
                            self.htpasswd_dir, share.id
                        ));
                    }
                    _ => {}
                }

                server_block.push_str("    }\n");
            }
        }

        server_block.push_str("}\n");

        // Add LDAP upstream if needed
        if config.auth_type == WebDavAuthType::Ldap {
            if let Some(ref ldap) = config.ldap {
                server_block.push_str(&format!(
                    "\n# LDAP server for share {}\n",
                    share.id
                ));
                server_block.push_str(&format!("ldap_server ldap_{} {{\n", share.id));
                server_block.push_str(&format!("    url {};\n", ldap.url));

                if let Some(ref bind_dn) = ldap.bind_dn {
                    server_block.push_str(&format!("    binddn \"{}\";\n", bind_dn));
                }
                if let Some(ref bind_pw) = ldap.bind_password {
                    server_block.push_str(&format!("    binddn_passwd \"{}\";\n", bind_pw));
                }

                server_block.push_str(&format!("    base_dn \"{}\";\n", ldap.base_dn));
                server_block.push_str(&format!("    filter \"{}\";\n", ldap.search_filter));

                if let Some(ref group) = ldap.require_group {
                    server_block.push_str(&format!("    require_group \"{}\";\n", group));
                }

                if ldap.starttls {
                    server_block.push_str("    starttls on;\n");
                }

                server_block.push_str(&format!("    connect_timeout {}s;\n", ldap.timeout));
                server_block.push_str("}\n");
            }
        }

        // Add lock zone
        server_block.insert_str(
            0,
            &format!("dav_ext_lock_zone zone=webdav_lock_{}:10m;\n\n", share.id),
        );

        server_block
    }

    /// Write full nginx configuration with all shares
    pub async fn write_full_config(&self, shares: &[NasShare]) -> Result<()> {
        // Initialize directories
        self.init().await?;

        // Backup existing configuration
        let backup_dir = format!("{}/backup.{}", self.nginx_conf_dir, chrono::Utc::now().timestamp());
        tokio::fs::create_dir_all(&backup_dir).await.ok();

        // Remove old WebDAV configs
        let mut entries = tokio::fs::read_dir(&self.nginx_conf_dir).await.map_err(|e| {
            Error::Internal(format!("Failed to read nginx conf dir: {}", e))
        })?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| {
            Error::Internal(format!("Failed to read directory entry: {}", e))
        })? {
            let path = entry.path();
            if let Some(name) = path.file_name() {
                let name = name.to_string_lossy();
                if name.starts_with("webdav-") && name.ends_with(".conf") {
                    // Backup before removing
                    let backup_path = format!("{}/{}", backup_dir, name);
                    tokio::fs::copy(&path, &backup_path).await.ok();
                    tokio::fs::remove_file(&path).await.ok();
                }
            }
        }

        // Write new configs
        for share in shares {
            if share.enabled {
                self.add_share(share).await?;
            }
        }

        // Test configuration
        self.test_nginx_config().await?;

        Ok(())
    }

    /// Test nginx configuration
    pub async fn test_nginx_config(&self) -> Result<()> {
        let output = Command::new("nginx")
            .args(["-t"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("nginx test failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Validation(format!(
                "nginx configuration test failed: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Reload nginx
    async fn reload_nginx(&self) -> Result<()> {
        let output = Command::new("nginx")
            .args(["-s", "reload"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("nginx reload failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "nginx reload failed: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Start nginx service
    pub async fn start(&self) -> Result<()> {
        let (cmd, args) = if self.use_systemd {
            ("systemctl", vec!["start", "nginx"])
        } else {
            ("rc-service", vec!["nginx", "start"])
        };

        let output = Command::new(cmd)
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to start nginx: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to start nginx: {}", stderr)));
        }

        Ok(())
    }

    /// Stop nginx service
    pub async fn stop(&self) -> Result<()> {
        let (cmd, args) = if self.use_systemd {
            ("systemctl", vec!["stop", "nginx"])
        } else {
            ("rc-service", vec!["nginx", "stop"])
        };

        let output = Command::new(cmd)
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to stop nginx: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to stop nginx: {}", stderr)));
        }

        Ok(())
    }

    /// Restart nginx service
    pub async fn restart(&self) -> Result<()> {
        let (cmd, args) = if self.use_systemd {
            ("systemctl", vec!["restart", "nginx"])
        } else {
            ("rc-service", vec!["nginx", "restart"])
        };

        let output = Command::new(cmd)
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to restart nginx: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to restart nginx: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Get WebDAV service status
    pub async fn get_status(&self) -> Result<WebDavStatus> {
        // Check if nginx is running
        let nginx_running = Command::new("pgrep")
            .args(["-x", "nginx"])
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);

        // Get nginx version
        let nginx_version = Command::new("nginx")
            .args(["-v"])
            .output()
            .await
            .map(|o| {
                let stderr = String::from_utf8_lossy(&o.stderr);
                stderr
                    .split('/')
                    .nth(1)
                    .map(|v| v.trim().to_string())
                    .unwrap_or_else(|| "unknown".to_string())
            })
            .unwrap_or_else(|_| "unknown".to_string());

        // Check DAV modules
        let modules_output = Command::new("nginx")
            .args(["-V"])
            .output()
            .await
            .map(|o| String::from_utf8_lossy(&o.stderr).to_string())
            .unwrap_or_default();

        let dav_module_loaded = modules_output.contains("http_dav_module");
        let dav_ext_loaded = modules_output.contains("dav_ext");

        // Count configured shares
        let mut configured_shares = 0u32;
        let mut ssl_enabled_shares = 0u32;

        for dir in [&self.nginx_conf_dir, &self.sites_enabled_dir] {
            if let Ok(mut entries) = tokio::fs::read_dir(dir).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with("webdav-") && name.ends_with(".conf") {
                        configured_shares += 1;

                        // Check if SSL is enabled
                        if let Ok(content) = tokio::fs::read_to_string(entry.path()).await {
                            if content.contains("ssl_certificate") {
                                ssl_enabled_shares += 1;
                            }
                        }
                    }
                }
            }
        }

        // Get active connections
        let active_connections = self.get_connection_count().await.unwrap_or(0);

        Ok(WebDavStatus {
            nginx_running,
            dav_module_loaded,
            dav_ext_loaded,
            active_connections,
            nginx_version,
            configured_shares,
            ssl_enabled_shares,
        })
    }

    /// Get active connection count
    async fn get_connection_count(&self) -> Result<u32> {
        // Try to get from nginx stub_status
        let output = Command::new("curl")
            .args(["-s", "http://127.0.0.1/nginx_status"])
            .output()
            .await;

        if let Ok(output) = output {
            if output.status.success() {
                let content = String::from_utf8_lossy(&output.stdout);
                // Parse "Active connections: N"
                if let Some(line) = content.lines().next() {
                    if let Some(count) = line.split(':').nth(1) {
                        if let Ok(n) = count.trim().parse() {
                            return Ok(n);
                        }
                    }
                }
            }
        }

        // Fallback to counting nginx worker processes
        let output = Command::new("pgrep")
            .args(["-c", "nginx"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to count connections: {}", e)))?;

        let count_str = String::from_utf8_lossy(&output.stdout);
        Ok(count_str.trim().parse().unwrap_or(0))
    }

    /// Add a user to htpasswd file
    pub async fn add_user(
        &self,
        share_id: &str,
        username: &str,
        password: &str,
    ) -> Result<()> {
        let htpasswd_path = format!("{}/webdav-{}", self.htpasswd_dir, share_id);

        // Check if file exists to determine -c flag
        let create_flag = if !Path::new(&htpasswd_path).exists() {
            vec!["-c"]
        } else {
            vec![]
        };

        let mut args = create_flag;
        args.extend(["-b", &htpasswd_path, username, password]);

        let output = Command::new("htpasswd")
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to add user: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to add user: {}", stderr)));
        }

        Ok(())
    }

    /// Remove a user from htpasswd file
    pub async fn remove_user(&self, share_id: &str, username: &str) -> Result<()> {
        let htpasswd_path = format!("{}/webdav-{}", self.htpasswd_dir, share_id);

        if !Path::new(&htpasswd_path).exists() {
            return Err(Error::NotFound("htpasswd file not found".to_string()));
        }

        let output = Command::new("htpasswd")
            .args(["-D", &htpasswd_path, username])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to remove user: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to remove user: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Update a user's password
    pub async fn update_password(
        &self,
        share_id: &str,
        username: &str,
        password: &str,
    ) -> Result<()> {
        let htpasswd_path = format!("{}/webdav-{}", self.htpasswd_dir, share_id);

        if !Path::new(&htpasswd_path).exists() {
            return Err(Error::NotFound("htpasswd file not found".to_string()));
        }

        let output = Command::new("htpasswd")
            .args(["-b", &htpasswd_path, username, password])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to update password: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to update password: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Verify user credentials
    pub async fn verify_user(
        &self,
        share_id: &str,
        username: &str,
        password: &str,
    ) -> Result<bool> {
        let htpasswd_path = format!("{}/webdav-{}", self.htpasswd_dir, share_id);

        if !Path::new(&htpasswd_path).exists() {
            return Ok(false);
        }

        let output = Command::new("htpasswd")
            .args(["-vb", &htpasswd_path, username, password])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to verify user: {}", e)))?;

        Ok(output.status.success())
    }

    /// List users in htpasswd file
    pub async fn list_users(&self, share_id: &str) -> Result<Vec<WebDavUser>> {
        let htpasswd_path = format!("{}/webdav-{}", self.htpasswd_dir, share_id);

        if !Path::new(&htpasswd_path).exists() {
            return Ok(vec![]);
        }

        let content = tokio::fs::read_to_string(&htpasswd_path).await.map_err(|e| {
            Error::Internal(format!("Failed to read htpasswd file: {}", e))
        })?;

        let users: Vec<WebDavUser> = content
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .filter_map(|line| {
                let parts: Vec<&str> = line.splitn(2, ':').collect();
                if parts.len() >= 1 {
                    Some(WebDavUser {
                        username: parts[0].to_string(),
                        enabled: true,
                        home_dir: None,
                        quota: 0,
                    })
                } else {
                    None
                }
            })
            .collect();

        Ok(users)
    }

    /// Parse recent connections from access log
    pub async fn get_recent_connections(
        &self,
        share_id: &str,
        limit: usize,
    ) -> Result<Vec<WebDavConnection>> {
        let log_path = format!("/var/log/nginx/webdav-{}.access.log", share_id);

        if !Path::new(&log_path).exists() {
            return Ok(vec![]);
        }

        let output = Command::new("tail")
            .args(["-n", &limit.to_string(), &log_path])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to read access log: {}", e)))?;

        if !output.status.success() {
            return Ok(vec![]);
        }

        let content = String::from_utf8_lossy(&output.stdout);
        let connections: Vec<WebDavConnection> = content
            .lines()
            .filter_map(|line| self.parse_access_log_line(line))
            .collect();

        Ok(connections)
    }

    /// Parse a single access log line (combined format)
    fn parse_access_log_line(&self, line: &str) -> Option<WebDavConnection> {
        // Format: IP - user [time] "method uri protocol" status bytes "referer" "user-agent"
        let re = regex::Regex::new(
            r#"^(\S+) - (\S+) \[([^\]]+)\] "(\S+) (\S+) [^"]*" (\d+) (\d+) "[^"]*" "([^"]*)""#,
        )
        .ok()?;

        let caps = re.captures(line)?;

        Some(WebDavConnection {
            remote_addr: caps.get(1)?.as_str().to_string(),
            username: {
                let user = caps.get(2)?.as_str();
                if user == "-" {
                    None
                } else {
                    Some(user.to_string())
                }
            },
            request_time: caps.get(3)?.as_str().to_string(),
            method: caps.get(4)?.as_str().to_string(),
            uri: caps.get(5)?.as_str().to_string(),
            status: caps.get(6)?.as_str().parse().unwrap_or(0),
            bytes_sent: caps.get(7)?.as_str().parse().unwrap_or(0),
            user_agent: Some(caps.get(8)?.as_str().to_string()),
        })
    }

    /// Generate SSL certificate using Let's Encrypt
    pub async fn generate_letsencrypt_cert(
        &self,
        domain: &str,
        email: &str,
    ) -> Result<WebDavSslConfig> {
        // Run certbot
        let output = Command::new("certbot")
            .args([
                "certonly",
                "--nginx",
                "-d",
                domain,
                "--email",
                email,
                "--agree-tos",
                "--non-interactive",
            ])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("certbot failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("certbot failed: {}", stderr)));
        }

        Ok(WebDavSslConfig {
            enabled: true,
            certificate: format!("/etc/letsencrypt/live/{}/fullchain.pem", domain),
            certificate_key: format!("/etc/letsencrypt/live/{}/privkey.pem", domain),
            ca_chain: Some(format!("/etc/letsencrypt/live/{}/chain.pem", domain)),
            hsts_enabled: true,
            hsts_max_age: 31536000,
            min_tls_version: "TLSv1.2".to_string(),
            ciphers: None,
        })
    }

    /// Generate self-signed certificate
    pub async fn generate_self_signed_cert(
        &self,
        share_id: &str,
        common_name: &str,
        days: u32,
    ) -> Result<WebDavSslConfig> {
        let cert_dir = "/etc/nginx/ssl";
        tokio::fs::create_dir_all(cert_dir).await.ok();

        let cert_path = format!("{}/webdav-{}.crt", cert_dir, share_id);
        let key_path = format!("{}/webdav-{}.key", cert_dir, share_id);

        // Generate private key and self-signed certificate
        let output = Command::new("openssl")
            .args([
                "req",
                "-x509",
                "-nodes",
                "-days",
                &days.to_string(),
                "-newkey",
                "rsa:2048",
                "-keyout",
                &key_path,
                "-out",
                &cert_path,
                "-subj",
                &format!("/CN={}", common_name),
            ])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("openssl failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("openssl failed: {}", stderr)));
        }

        Ok(WebDavSslConfig {
            enabled: true,
            certificate: cert_path,
            certificate_key: key_path,
            ca_chain: None,
            hsts_enabled: false, // Don't enable HSTS for self-signed
            hsts_max_age: 0,
            min_tls_version: "TLSv1.2".to_string(),
            ciphers: None,
        })
    }

    /// List all WebDAV shares
    pub async fn list_shares(&self) -> Result<Vec<String>> {
        let mut shares = Vec::new();

        for dir in [&self.nginx_conf_dir, &self.sites_available_dir] {
            if let Ok(mut entries) = tokio::fs::read_dir(dir).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with("webdav-") && name.ends_with(".conf") {
                        let share_id = name
                            .strip_prefix("webdav-")
                            .and_then(|s| s.strip_suffix(".conf"))
                            .map(|s| s.to_string());
                        if let Some(id) = share_id {
                            if !shares.contains(&id) {
                                shares.push(id);
                            }
                        }
                    }
                }
            }
        }

        Ok(shares)
    }

    /// Check if a share is enabled
    pub async fn is_share_enabled(&self, share_id: &str) -> Result<bool> {
        let enabled_path = format!("{}/webdav-{}.conf", self.sites_enabled_dir, share_id);
        Ok(Path::new(&enabled_path).exists())
    }

    /// Get share configuration
    pub async fn get_share_config(&self, share_id: &str) -> Result<String> {
        for dir in [&self.sites_available_dir, &self.nginx_conf_dir] {
            let path = format!("{}/webdav-{}.conf", dir, share_id);
            if Path::new(&path).exists() {
                return tokio::fs::read_to_string(&path).await.map_err(|e| {
                    Error::Internal(format!("Failed to read config: {}", e))
                });
            }
        }

        Err(Error::NotFound(format!(
            "WebDAV share {} not found",
            share_id
        )))
    }

    /// Create directories required for CalDAV/CardDAV
    pub async fn setup_caldav_carddav(&self, share_path: &str) -> Result<()> {
        let calendars_path = format!("{}/calendars", share_path);
        let contacts_path = format!("{}/contacts", share_path);

        tokio::fs::create_dir_all(&calendars_path).await.map_err(|e| {
            Error::Internal(format!("Failed to create calendars directory: {}", e))
        })?;

        tokio::fs::create_dir_all(&contacts_path).await.map_err(|e| {
            Error::Internal(format!("Failed to create contacts directory: {}", e))
        })?;

        Ok(())
    }

    /// Set directory permissions for WebDAV share
    pub async fn set_permissions(
        &self,
        path: &str,
        owner: &str,
        group: &str,
        mode: &str,
    ) -> Result<()> {
        // Set ownership
        let output = Command::new("chown")
            .args(["-R", &format!("{}:{}", owner, group), path])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("chown failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("chown failed: {}", stderr)));
        }

        // Set permissions
        let output = Command::new("chmod")
            .args(["-R", mode, path])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("chmod failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("chmod failed: {}", stderr)));
        }

        Ok(())
    }

    /// Enable WebDAV service at boot
    pub async fn enable(&self) -> Result<()> {
        let (cmd, args) = if self.use_systemd {
            ("systemctl", vec!["enable", "nginx"])
        } else {
            ("rc-update", vec!["add", "nginx", "default"])
        };

        let output = Command::new(cmd)
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to enable nginx: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to enable nginx: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Disable WebDAV service at boot
    pub async fn disable(&self) -> Result<()> {
        let (cmd, args) = if self.use_systemd {
            ("systemctl", vec!["disable", "nginx"])
        } else {
            ("rc-update", vec!["del", "nginx", "default"])
        };

        let output = Command::new(cmd)
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to disable nginx: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to disable nginx: {}",
                stderr
            )));
        }

        Ok(())
    }
}

impl Default for WebDavManager {
    fn default() -> Self {
        Self::new()
    }
}
