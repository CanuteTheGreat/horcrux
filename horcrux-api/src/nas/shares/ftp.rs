//! FTP/SFTP file sharing module
//!
//! Manages ProFTPD configuration with TLS/SSL, virtual users,
//! quotas, bandwidth limiting, and SFTP via OpenSSH.

use horcrux_common::{Error, Result};
use crate::nas::shares::{FtpShareConfig, NasShare};
use tokio::process::Command;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// FTP/FTPS protocol mode
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FtpProtocol {
    /// Plain FTP (port 21)
    Ftp,
    /// FTP with explicit TLS (FTPES, port 21)
    FtpExplicitTls,
    /// FTP with implicit TLS (FTPS, port 990)
    FtpImplicitTls,
    /// SFTP via OpenSSH (port 22)
    Sftp,
}

impl Default for FtpProtocol {
    fn default() -> Self {
        Self::FtpExplicitTls
    }
}

/// TLS/SSL configuration for FTPS
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FtpTlsConfig {
    /// Enable TLS
    pub enabled: bool,
    /// Path to SSL certificate
    pub certificate: String,
    /// Path to SSL private key
    pub certificate_key: String,
    /// Path to CA chain (optional)
    pub ca_chain: Option<String>,
    /// Require TLS for all connections
    pub required: bool,
    /// Require TLS for data transfers
    pub required_data: bool,
    /// Minimum TLS version
    pub min_version: String,
    /// Cipher suite
    pub ciphers: Option<String>,
}

impl Default for FtpTlsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            certificate: "/etc/ssl/certs/proftpd.crt".to_string(),
            certificate_key: "/etc/ssl/private/proftpd.key".to_string(),
            ca_chain: None,
            required: false,
            required_data: false,
            min_version: "TLSv1.2".to_string(),
            ciphers: None,
        }
    }
}

/// Passive mode configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FtpPassiveConfig {
    /// Enable passive mode
    pub enabled: bool,
    /// Passive port range start
    pub port_min: u16,
    /// Passive port range end
    pub port_max: u16,
    /// External/NAT IP address (for clients behind NAT)
    pub external_address: Option<String>,
    /// Use resolved address for passive mode
    pub resolve_address: bool,
}

impl Default for FtpPassiveConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port_min: 49152,
            port_max: 65534,
            external_address: None,
            resolve_address: false,
        }
    }
}

/// Bandwidth limiting configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FtpBandwidthConfig {
    /// Maximum download rate per user (KB/s, 0 = unlimited)
    pub max_download_rate: u64,
    /// Maximum upload rate per user (KB/s, 0 = unlimited)
    pub max_upload_rate: u64,
    /// Maximum site-wide download rate (KB/s, 0 = unlimited)
    pub site_download_rate: u64,
    /// Maximum site-wide upload rate (KB/s, 0 = unlimited)
    pub site_upload_rate: u64,
}

impl Default for FtpBandwidthConfig {
    fn default() -> Self {
        Self {
            max_download_rate: 0,
            max_upload_rate: 0,
            site_download_rate: 0,
            site_upload_rate: 0,
        }
    }
}

/// Virtual user configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FtpVirtualUser {
    /// Username
    pub username: String,
    /// Password hash (use create_password_hash to generate)
    pub password_hash: String,
    /// User's UID
    pub uid: u32,
    /// User's GID
    pub gid: u32,
    /// Home directory
    pub home_dir: String,
    /// Shell (usually /bin/false)
    pub shell: String,
    /// Whether user is enabled
    pub enabled: bool,
    /// User quota in bytes (0 = unlimited)
    pub quota_bytes: u64,
    /// Max files quota (0 = unlimited)
    pub quota_files: u64,
    /// Maximum download rate (KB/s, 0 = unlimited)
    pub max_download_rate: u64,
    /// Maximum upload rate (KB/s, 0 = unlimited)
    pub max_upload_rate: u64,
}

/// Extended FTP server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FtpServerConfig {
    /// Server name displayed to clients
    pub server_name: String,
    /// Listen address (empty = all interfaces)
    pub listen_address: String,
    /// Listen port
    pub port: u16,
    /// Maximum concurrent connections
    pub max_clients: u32,
    /// Maximum connections per host
    pub max_clients_per_host: u32,
    /// Maximum login attempts
    pub max_login_attempts: u32,
    /// Idle timeout in seconds
    pub timeout_idle: u32,
    /// No-transfer timeout in seconds
    pub timeout_no_transfer: u32,
    /// Data transfer timeout in seconds
    pub timeout_stalled: u32,
    /// TLS configuration
    pub tls: FtpTlsConfig,
    /// Passive mode configuration
    pub passive: FtpPassiveConfig,
    /// Bandwidth limits
    pub bandwidth: FtpBandwidthConfig,
    /// Use virtual users
    pub use_virtual_users: bool,
    /// Virtual user database path
    pub virtual_users_file: String,
    /// Enable anonymous access
    pub anonymous_enabled: bool,
    /// Anonymous user root directory
    pub anonymous_root: Option<String>,
    /// Anonymous can upload
    pub anonymous_upload: bool,
    /// Allow root login
    pub allow_root_login: bool,
    /// Require valid shell
    pub require_valid_shell: bool,
    /// Umask for new files
    pub umask: String,
    /// Default chroot to home directory
    pub default_chroot: bool,
    /// Enable extended logging
    pub extended_log: bool,
    /// Log file path
    pub log_file: String,
    /// Transfer log path
    pub transfer_log: String,
    /// Allow FXP (server-to-server) transfers
    pub allow_fxp: bool,
    /// Hide files matching pattern
    pub hide_files: Option<String>,
    /// Deny files matching pattern
    pub deny_filter: Option<String>,
    /// Custom directives
    pub custom_directives: Option<String>,
}

impl Default for FtpServerConfig {
    fn default() -> Self {
        Self {
            server_name: "Horcrux FTP Server".to_string(),
            listen_address: String::new(),
            port: 21,
            max_clients: 100,
            max_clients_per_host: 10,
            max_login_attempts: 3,
            timeout_idle: 600,
            timeout_no_transfer: 600,
            timeout_stalled: 300,
            tls: FtpTlsConfig::default(),
            passive: FtpPassiveConfig::default(),
            bandwidth: FtpBandwidthConfig::default(),
            use_virtual_users: false,
            virtual_users_file: "/etc/proftpd/ftpd.passwd".to_string(),
            anonymous_enabled: false,
            anonymous_root: None,
            anonymous_upload: false,
            allow_root_login: false,
            require_valid_shell: false,
            umask: "022".to_string(),
            default_chroot: true,
            extended_log: true,
            log_file: "/var/log/proftpd/proftpd.log".to_string(),
            transfer_log: "/var/log/proftpd/xferlog".to_string(),
            allow_fxp: false,
            hide_files: Some("^\\.".to_string()),
            deny_filter: Some("\\*.*/".to_string()),
            custom_directives: None,
        }
    }
}

/// FTP connection information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FtpConnection {
    /// Process ID
    pub pid: u32,
    /// Connected user
    pub username: String,
    /// Client IP address
    pub client_ip: String,
    /// Login time
    pub login_time: String,
    /// Idle time in seconds
    pub idle_seconds: u32,
    /// Current directory
    pub current_dir: String,
    /// Current action (IDLE, RETR, STOR, etc.)
    pub action: String,
    /// File being transferred (if any)
    pub current_file: Option<String>,
}

/// FTP transfer statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FtpTransferStats {
    /// Total bytes uploaded
    pub bytes_uploaded: u64,
    /// Total bytes downloaded
    pub bytes_downloaded: u64,
    /// Total files uploaded
    pub files_uploaded: u64,
    /// Total files downloaded
    pub files_downloaded: u64,
    /// Failed logins
    pub failed_logins: u64,
    /// Successful logins
    pub successful_logins: u64,
}

/// FTP service status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FtpStatus {
    /// Is ProFTPD daemon running
    pub running: bool,
    /// ProFTPD version
    pub version: String,
    /// Config path
    pub config_path: String,
    /// Config file exists
    pub config_exists: bool,
    /// Config syntax valid
    pub config_valid: bool,
    /// TLS enabled
    pub tls_enabled: bool,
    /// Active connections count
    pub active_connections: u32,
    /// Server uptime in seconds
    pub uptime_seconds: u64,
}

/// SFTP-specific configuration for OpenSSH
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SftpConfig {
    /// Subsystem enabled
    pub enabled: bool,
    /// Chroot directory for SFTP users
    pub chroot_directory: Option<String>,
    /// Force command (internal-sftp)
    pub force_command: bool,
    /// Allowed users/groups (Match block)
    pub allowed_groups: Vec<String>,
    /// Disable shell access for SFTP users
    pub disable_shell: bool,
    /// Log level
    pub log_level: String,
}

impl Default for SftpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            chroot_directory: None,
            force_command: true,
            allowed_groups: vec!["sftpusers".to_string()],
            disable_shell: true,
            log_level: "INFO".to_string(),
        }
    }
}

/// Simplified FTP global configuration for API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FtpGlobalConfig {
    /// Server name displayed to clients
    pub server_name: String,
    /// Listen port
    pub port: u16,
    /// Passive port range (start, end)
    pub passive_port_range: (u16, u16),
    /// Maximum concurrent connections
    pub max_clients: u32,
    /// Maximum connections per host
    pub max_clients_per_host: u32,
    /// Idle timeout in seconds
    pub timeout_idle: u32,
    /// Login timeout in seconds
    pub timeout_login: u32,
    /// No-transfer timeout in seconds
    pub timeout_no_transfer: u32,
    /// Enable anonymous access
    pub allow_anonymous: bool,
    /// Anonymous user root directory
    pub anonymous_root: Option<String>,
    /// TLS enabled
    pub tls_enabled: bool,
    /// TLS certificate file
    pub tls_cert_file: Option<String>,
    /// TLS key file
    pub tls_key_file: Option<String>,
    /// Require TLS for all connections
    pub tls_required: bool,
    /// Chroot local users to home directory
    pub chroot_local_users: bool,
    /// Require valid shell
    pub require_valid_shell: bool,
    /// Use sendfile for transfers
    pub use_sendfile: bool,
    /// Extra parameters
    pub extra_parameters: HashMap<String, String>,
}

impl Default for FtpGlobalConfig {
    fn default() -> Self {
        Self {
            server_name: "Horcrux FTP Server".to_string(),
            port: 21,
            passive_port_range: (49152, 65534),
            max_clients: 100,
            max_clients_per_host: 10,
            timeout_idle: 600,
            timeout_login: 300,
            timeout_no_transfer: 900,
            allow_anonymous: false,
            anonymous_root: None,
            tls_enabled: false,
            tls_cert_file: None,
            tls_key_file: None,
            tls_required: false,
            chroot_local_users: true,
            require_valid_shell: true,
            use_sendfile: true,
            extra_parameters: HashMap::new(),
        }
    }
}

/// FTP Manager for ProFTPD and SFTP
pub struct FtpManager {
    /// ProFTPD main config path
    proftpd_conf: String,
    /// ProFTPD conf.d directory
    proftpd_conf_d: String,
    /// Virtual users file
    virtual_users_file: String,
    /// SFTP config file for OpenSSH
    sshd_config: String,
    /// Use systemd or OpenRC
    use_systemd: bool,
    /// Global configuration
    global_config: FtpGlobalConfig,
}

impl FtpManager {
    /// Create a new FTP manager
    pub fn new() -> Self {
        let use_systemd = Path::new("/run/systemd/system").exists();

        Self {
            proftpd_conf: "/etc/proftpd/proftpd.conf".to_string(),
            proftpd_conf_d: "/etc/proftpd/conf.d".to_string(),
            virtual_users_file: "/etc/proftpd/ftpd.passwd".to_string(),
            sshd_config: "/etc/ssh/sshd_config".to_string(),
            use_systemd,
            global_config: FtpGlobalConfig::default(),
        }
    }

    /// Get global configuration
    pub fn config(&self) -> &FtpGlobalConfig {
        &self.global_config
    }

    /// Set global configuration
    pub fn set_global_config(&mut self, config: FtpGlobalConfig) {
        self.global_config = config;
    }

    /// Disconnect a session by PID
    pub async fn disconnect_session(&self, pid: u32) -> Result<()> {
        // Send SIGTERM to the FTP session process
        let output = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to kill FTP session: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to terminate FTP session {}: {}", pid, stderr)));
        }

        Ok(())
    }

    /// Create with custom paths
    pub fn with_paths(
        proftpd_conf: &str,
        proftpd_conf_d: &str,
        virtual_users_file: &str,
    ) -> Self {
        let use_systemd = Path::new("/run/systemd/system").exists();

        Self {
            proftpd_conf: proftpd_conf.to_string(),
            proftpd_conf_d: proftpd_conf_d.to_string(),
            virtual_users_file: virtual_users_file.to_string(),
            sshd_config: "/etc/ssh/sshd_config".to_string(),
            use_systemd,
            global_config: FtpGlobalConfig::default(),
        }
    }

    /// Initialize required directories
    pub async fn init(&self) -> Result<()> {
        tokio::fs::create_dir_all(&self.proftpd_conf_d).await.map_err(|e| {
            Error::Internal(format!("Failed to create proftpd conf.d: {}", e))
        })?;

        tokio::fs::create_dir_all("/var/log/proftpd").await.ok();

        Ok(())
    }

    /// Write full ProFTPD configuration
    pub async fn write_full_config(
        &self,
        server_config: &FtpServerConfig,
        shares: &[NasShare],
    ) -> Result<()> {
        // Initialize directories
        self.init().await?;

        // Backup existing config
        if Path::new(&self.proftpd_conf).exists() {
            let backup_path = format!("{}.bak.{}", self.proftpd_conf, chrono::Utc::now().timestamp());
            tokio::fs::copy(&self.proftpd_conf, &backup_path).await.ok();
        }

        let config = self.generate_full_config(server_config, shares);
        self.write_config(&config).await?;

        // Validate config
        self.test_config().await?;

        Ok(())
    }

    /// Generate full ProFTPD configuration
    fn generate_full_config(&self, config: &FtpServerConfig, shares: &[NasShare]) -> String {
        let mut conf = String::new();

        // Header
        conf.push_str("# ProFTPD Configuration\n");
        conf.push_str("# Generated by Horcrux NAS Manager\n");
        conf.push_str("# Do not edit manually\n\n");

        // Server identity
        conf.push_str(&format!("ServerName\t\t\t\"{}\"\n", config.server_name));
        conf.push_str("ServerType\t\t\tstandalone\n");
        conf.push_str("ServerIdent\t\t\ton \"Horcrux FTP Server Ready\"\n\n");

        // Network settings
        if !config.listen_address.is_empty() {
            conf.push_str(&format!("DefaultAddress\t\t\t{}\n", config.listen_address));
        }
        conf.push_str(&format!("Port\t\t\t\t{}\n", config.port));
        conf.push_str("UseIPv6\t\t\t\ton\n");
        conf.push_str("UseReverseDNS\t\t\toff\n\n");

        // Connection limits
        conf.push_str(&format!("MaxClients\t\t\t{}\n", config.max_clients));
        conf.push_str(&format!("MaxClientsPerHost\t\t{}\n", config.max_clients_per_host));
        conf.push_str(&format!("MaxLoginAttempts\t\t{}\n", config.max_login_attempts));
        conf.push_str(&format!("MaxConnectionsPerHost\t\t{}\n\n", config.max_clients_per_host));

        // Timeouts
        conf.push_str(&format!("TimeoutIdle\t\t\t{}\n", config.timeout_idle));
        conf.push_str(&format!("TimeoutNoTransfer\t\t{}\n", config.timeout_no_transfer));
        conf.push_str(&format!("TimeoutStalled\t\t\t{}\n", config.timeout_stalled));
        conf.push_str("TimeoutLogin\t\t\t120\n\n");

        // User settings
        conf.push_str("User\t\t\t\tnobody\n");
        conf.push_str("Group\t\t\t\tnogroup\n");
        conf.push_str(&format!("Umask\t\t\t\t{}\n", config.umask));

        if config.default_chroot {
            conf.push_str("DefaultRoot\t\t\t~\n");
        }

        if !config.require_valid_shell {
            conf.push_str("RequireValidShell\t\toff\n");
        }

        if !config.allow_root_login {
            conf.push_str("RootLogin\t\t\toff\n");
        }

        conf.push('\n');

        // Passive mode
        if config.passive.enabled {
            conf.push_str(&format!(
                "PassivePorts\t\t\t{} {}\n",
                config.passive.port_min, config.passive.port_max
            ));

            if let Some(ref addr) = config.passive.external_address {
                conf.push_str(&format!("MasqueradeAddress\t\t{}\n", addr));
            }
        }
        conf.push('\n');

        // Bandwidth limits
        if config.bandwidth.max_download_rate > 0 {
            conf.push_str(&format!(
                "TransferRate RETR {} user *\n",
                config.bandwidth.max_download_rate * 1024
            ));
        }
        if config.bandwidth.max_upload_rate > 0 {
            conf.push_str(&format!(
                "TransferRate STOR {} user *\n",
                config.bandwidth.max_upload_rate * 1024
            ));
        }
        if config.bandwidth.site_download_rate > 0 {
            conf.push_str(&format!(
                "TransferRate RETR {} server *\n",
                config.bandwidth.site_download_rate * 1024
            ));
        }
        if config.bandwidth.site_upload_rate > 0 {
            conf.push_str(&format!(
                "TransferRate STOR {} server *\n",
                config.bandwidth.site_upload_rate * 1024
            ));
        }
        conf.push('\n');

        // Security settings
        if !config.allow_fxp {
            conf.push_str("AllowForeignAddress\t\toff\n");
        }

        if let Some(ref pattern) = config.deny_filter {
            conf.push_str(&format!("DenyFilter\t\t\t\"{}\"\n", pattern));
        }

        conf.push_str("ListOptions\t\t\t\"-la\"\n");
        conf.push_str("AllowOverwrite\t\t\ton\n\n");

        // TLS Configuration
        if config.tls.enabled {
            conf.push_str("<IfModule mod_tls.c>\n");
            conf.push_str("  TLSEngine\t\t\ton\n");
            conf.push_str("  TLSLog\t\t\t/var/log/proftpd/tls.log\n");
            conf.push_str(&format!(
                "  TLSProtocol\t\t\t{} TLSv1.3\n",
                config.tls.min_version
            ));

            if let Some(ref ciphers) = config.tls.ciphers {
                conf.push_str(&format!("  TLSCipherSuite\t\t{}\n", ciphers));
            } else {
                conf.push_str("  TLSCipherSuite\t\tHIGH:!aNULL:!MD5\n");
            }

            conf.push_str(&format!(
                "  TLSRSACertificateFile\t\t{}\n",
                config.tls.certificate
            ));
            conf.push_str(&format!(
                "  TLSRSACertificateKeyFile\t{}\n",
                config.tls.certificate_key
            ));

            if let Some(ref ca) = config.tls.ca_chain {
                conf.push_str(&format!("  TLSCACertificateFile\t\t{}\n", ca));
            }

            if config.tls.required {
                conf.push_str("  TLSRequired\t\t\ton\n");
            } else {
                conf.push_str("  TLSRequired\t\t\toff\n");
            }

            conf.push_str("  TLSVerifyClient\t\toff\n");
            conf.push_str("  TLSRenegotiate\t\trequired off\n");
            conf.push_str("</IfModule>\n\n");
        }

        // Virtual users
        if config.use_virtual_users {
            conf.push_str("<IfModule mod_auth_file.c>\n");
            conf.push_str(&format!(
                "  AuthUserFile\t\t\t{}\n",
                config.virtual_users_file
            ));
            conf.push_str("  AuthOrder\t\t\tmod_auth_file.c\n");
            conf.push_str("</IfModule>\n\n");
        }

        // Quotas
        conf.push_str("<IfModule mod_quotatab.c>\n");
        conf.push_str("  QuotaEngine\t\t\ton\n");
        conf.push_str("  QuotaLog\t\t\t/var/log/proftpd/quota.log\n");
        conf.push_str("  <IfModule mod_quotatab_file.c>\n");
        conf.push_str("    QuotaLimitTable\t\tfile:/etc/proftpd/ftpquota.limittab\n");
        conf.push_str("    QuotaTallyTable\t\tfile:/etc/proftpd/ftpquota.tallytab\n");
        conf.push_str("  </IfModule>\n");
        conf.push_str("</IfModule>\n\n");

        // Logging
        conf.push_str(&format!(
            "SystemLog\t\t\t{}\n",
            config.log_file
        ));
        conf.push_str(&format!(
            "TransferLog\t\t\t{}\n",
            config.transfer_log
        ));

        if config.extended_log {
            conf.push_str("LogFormat\t\t\tdefault \"%h %l %u %t \\\"%r\\\" %s %b\"\n");
            conf.push_str("LogFormat\t\t\tauth \"%v [%P] %h %t \\\"%r\\\" %s\"\n");
            conf.push_str("LogFormat\t\t\twrite \"%h %l %u %t \\\"%r\\\" %s %b\"\n");
            conf.push_str("ExtendedLog\t\t\t/var/log/proftpd/access.log WRITE,READ default\n");
            conf.push_str("ExtendedLog\t\t\t/var/log/proftpd/auth.log AUTH auth\n");
        }
        conf.push('\n');

        // Hide files
        if let Some(ref pattern) = config.hide_files {
            conf.push_str(&format!("<Directory /*>\n"));
            conf.push_str(&format!("  HideFiles\t\t\t({})\n", pattern));
            conf.push_str("</Directory>\n\n");
        }

        // Anonymous access
        if config.anonymous_enabled {
            if let Some(ref root) = config.anonymous_root {
                conf.push_str(&format!("<Anonymous {}>\n", root));
                conf.push_str("  User\t\t\t\tftp\n");
                conf.push_str("  Group\t\t\t\tftp\n");
                conf.push_str("  UserAlias\t\t\tanonymous ftp\n");
                conf.push_str("  MaxClients\t\t\t10 \"Maximum anonymous users reached\"\n");
                conf.push_str("  DisplayLogin\t\t\twelcome.msg\n");
                conf.push_str("  DisplayChdir\t\t\t.message\n");

                if config.anonymous_upload {
                    conf.push_str("  <Directory uploads/*>\n");
                    conf.push_str("    <Limit STOR MKD>\n");
                    conf.push_str("      AllowAll\n");
                    conf.push_str("    </Limit>\n");
                    conf.push_str("  </Directory>\n");
                } else {
                    conf.push_str("  <Limit WRITE>\n");
                    conf.push_str("    DenyAll\n");
                    conf.push_str("  </Limit>\n");
                }

                conf.push_str("</Anonymous>\n\n");
            }
        }

        // Share directories
        for share in shares {
            if share.enabled {
                let share_config = share.ftp_config.as_ref().cloned().unwrap_or_default();
                conf.push_str(&self.generate_share_section(share, &share_config));
            }
        }

        // Custom directives
        if let Some(ref custom) = config.custom_directives {
            conf.push_str("\n# Custom directives\n");
            conf.push_str(custom);
            conf.push('\n');
        }

        // Include conf.d
        conf.push_str(&format!("\nInclude {}/\n", self.proftpd_conf_d));

        conf
    }

    /// Add an FTP share
    pub async fn add_share(&self, share: &NasShare) -> Result<()> {
        let config = self.read_config().await?;
        let share_section = self.generate_share_section(
            share,
            &share.ftp_config.clone().unwrap_or_default(),
        );

        let mut new_config = config;
        new_config.push_str(&share_section);

        self.write_config(&new_config).await?;
        self.test_config().await?;
        self.reload().await?;

        Ok(())
    }

    /// Remove an FTP share
    pub async fn remove_share(&self, share: &NasShare) -> Result<()> {
        let config = self.read_config().await?;
        let new_config = self.remove_share_section(&config, &share.name);

        self.write_config(&new_config).await?;
        self.test_config().await?;
        self.reload().await?;

        Ok(())
    }

    /// Generate share section
    fn generate_share_section(&self, share: &NasShare, config: &FtpShareConfig) -> String {
        let mut section = String::new();

        section.push_str(&format!("\n# Share: {}\n", share.name));
        section.push_str(&format!("<Directory {}>\n", share.path));

        // Permissions based on share settings
        if share.read_only {
            section.push_str("  <Limit WRITE>\n");
            section.push_str("    DenyAll\n");
            section.push_str("  </Limit>\n");
        } else {
            section.push_str("  <Limit READ WRITE DIRS>\n");
            section.push_str("    AllowAll\n");
            section.push_str("  </Limit>\n");
        }

        // User restrictions
        if !share.allowed_users.is_empty() {
            let users = share.allowed_users.join(" ");
            section.push_str(&format!("  <Limit ALL>\n"));
            section.push_str(&format!("    AllowUser {}\n", users));
            section.push_str("    DenyAll\n");
            section.push_str("  </Limit>\n");
        }

        // Hide files if specified
        if let Some(ref pattern) = config.hide_pattern {
            section.push_str(&format!("  HideFiles ({})\n", pattern));
        }

        section.push_str("</Directory>\n");

        // Chroot configuration per share
        if config.chroot {
            section.push_str(&format!("<Directory {}/*>\n", share.path));
            section.push_str(&format!("  DefaultRoot {}\n", share.path));
            section.push_str("</Directory>\n");
        }

        // Anonymous access if enabled
        if config.anonymous {
            section.push_str(&format!(
                r#"
<Anonymous {}>
  User ftp
  Group ftp
  UserAlias anonymous ftp
  MaxClients 10 "Maximum anonymous users reached"
  <Limit WRITE>
    DenyAll
  </Limit>
</Anonymous>
"#,
                share.path
            ));
        }

        section
    }

    /// Remove share section
    fn remove_share_section(&self, config: &str, share_name: &str) -> String {
        let mut result = String::new();
        let mut skip_section = false;
        let mut depth = 0;
        let marker = format!("# Share: {}", share_name);

        for line in config.lines() {
            if line.trim() == marker {
                skip_section = true;
                continue;
            }

            if skip_section {
                // Track nested blocks
                if line.contains('<') && !line.contains("</") {
                    depth += 1;
                }
                if line.contains("</") {
                    depth -= 1;
                    if depth <= 0 {
                        skip_section = false;
                        depth = 0;
                    }
                }
                continue;
            }

            result.push_str(line);
            result.push('\n');
        }

        result
    }

    /// Read config file
    async fn read_config(&self) -> Result<String> {
        match tokio::fs::read_to_string(&self.proftpd_conf).await {
            Ok(content) => Ok(content),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(Error::Internal(format!(
                "Failed to read proftpd.conf: {}",
                e
            ))),
        }
    }

    /// Write config file
    async fn write_config(&self, content: &str) -> Result<()> {
        // Ensure directory exists
        if let Some(parent) = Path::new(&self.proftpd_conf).parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }

        tokio::fs::write(&self.proftpd_conf, content)
            .await
            .map_err(|e| {
                Error::Internal(format!("Failed to write proftpd.conf: {}", e))
            })
    }

    /// Test ProFTPD configuration syntax
    pub async fn test_config(&self) -> Result<()> {
        let output = Command::new("proftpd")
            .args(["-t", "-c", &self.proftpd_conf])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Config test failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Validation(format!(
                "ProFTPD configuration error: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Reload ProFTPD
    pub async fn reload(&self) -> Result<()> {
        if !self.is_running().await {
            return Ok(());
        }

        let output = Command::new("pkill")
            .args(["-HUP", "proftpd"])
            .output()
            .await;

        match output {
            Ok(out) if out.status.success() => Ok(()),
            Ok(_) => Ok(()), // proftpd might not be running
            Err(e) => Err(Error::Internal(format!("Failed to reload ProFTPD: {}", e))),
        }
    }

    /// Start ProFTPD
    pub async fn start(&self) -> Result<()> {
        let (cmd, args) = if self.use_systemd {
            ("systemctl", vec!["start", "proftpd"])
        } else {
            ("rc-service", vec!["proftpd", "start"])
        };

        let output = Command::new(cmd)
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to start ProFTPD: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to start ProFTPD: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Stop ProFTPD
    pub async fn stop(&self) -> Result<()> {
        let (cmd, args) = if self.use_systemd {
            ("systemctl", vec!["stop", "proftpd"])
        } else {
            ("rc-service", vec!["proftpd", "stop"])
        };

        let output = Command::new(cmd)
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to stop ProFTPD: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to stop ProFTPD: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Restart ProFTPD
    pub async fn restart(&self) -> Result<()> {
        let (cmd, args) = if self.use_systemd {
            ("systemctl", vec!["restart", "proftpd"])
        } else {
            ("rc-service", vec!["proftpd", "restart"])
        };

        let output = Command::new(cmd)
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to restart ProFTPD: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to restart ProFTPD: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Check if ProFTPD is running
    pub async fn is_running(&self) -> bool {
        let output = Command::new("pgrep")
            .args(["-x", "proftpd"])
            .output()
            .await;

        match output {
            Ok(out) => out.status.success(),
            Err(_) => false,
        }
    }

    /// Get ProFTPD status
    pub async fn get_status(&self) -> Result<FtpStatus> {
        let running = self.is_running().await;
        let config_exists = Path::new(&self.proftpd_conf).exists();

        // Get version
        let version = Command::new("proftpd")
            .arg("--version")
            .output()
            .await
            .map(|o| {
                let stdout = String::from_utf8_lossy(&o.stdout);
                stdout
                    .lines()
                    .next()
                    .unwrap_or("unknown")
                    .trim()
                    .to_string()
            })
            .unwrap_or_else(|_| "unknown".to_string());

        // Test config
        let config_valid = self.test_config().await.is_ok();

        // Check TLS
        let tls_enabled = if config_exists {
            let config = self.read_config().await.unwrap_or_default();
            config.contains("TLSEngine") && config.contains("on")
        } else {
            false
        };

        // Get active connections
        let active_connections = self.get_connections().await.map(|c| c.len() as u32).unwrap_or(0);

        // Get uptime
        let uptime_seconds = self.get_uptime().await.unwrap_or(0);

        Ok(FtpStatus {
            running,
            version,
            config_path: self.proftpd_conf.clone(),
            config_exists,
            config_valid,
            tls_enabled,
            active_connections,
            uptime_seconds,
        })
    }

    /// Get active connections using ftpwho
    pub async fn get_connections(&self) -> Result<Vec<FtpConnection>> {
        let output = Command::new("ftpwho")
            .args(["-v"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("ftpwho failed: {}", e)))?;

        if !output.status.success() {
            return Ok(vec![]);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let connections = self.parse_ftpwho_output(&stdout);

        Ok(connections)
    }

    /// Parse ftpwho output
    fn parse_ftpwho_output(&self, output: &str) -> Vec<FtpConnection> {
        let mut connections = Vec::new();

        for line in output.lines().skip(1) {
            // Skip header
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 6 {
                connections.push(FtpConnection {
                    pid: parts[0].parse().unwrap_or(0),
                    username: parts[1].to_string(),
                    client_ip: parts[2].to_string(),
                    login_time: parts[3].to_string(),
                    idle_seconds: parts[4].parse().unwrap_or(0),
                    current_dir: parts.get(5).map(|s| s.to_string()).unwrap_or_default(),
                    action: parts.get(6).map(|s| s.to_string()).unwrap_or_else(|| "IDLE".to_string()),
                    current_file: parts.get(7).map(|s| s.to_string()),
                });
            }
        }

        connections
    }

    /// Get connection count using ftpcount
    pub async fn get_connection_count(&self) -> Result<u32> {
        let output = Command::new("ftpcount")
            .output()
            .await
            .map_err(|e| Error::Internal(format!("ftpcount failed: {}", e)))?;

        if !output.status.success() {
            return Ok(0);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        // Parse output like "Master 192.168.1.1 - 5 users"
        for line in stdout.lines() {
            if line.contains("users") {
                if let Some(count) = line.split_whitespace().rev().nth(1) {
                    return Ok(count.parse().unwrap_or(0));
                }
            }
        }

        Ok(0)
    }

    /// Kill a specific connection
    pub async fn kill_connection(&self, pid: u32) -> Result<()> {
        let output = Command::new("ftpkill")
            .arg(pid.to_string())
            .output()
            .await
            .map_err(|e| Error::Internal(format!("ftpkill failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to kill session: {}", stderr)));
        }

        Ok(())
    }

    /// Add a virtual user
    pub async fn add_virtual_user(&self, user: &FtpVirtualUser) -> Result<()> {
        // Create home directory
        tokio::fs::create_dir_all(&user.home_dir).await.map_err(|e| {
            Error::Internal(format!("Failed to create home directory: {}", e))
        })?;

        // Use ftpasswd to add user
        let output = Command::new("ftpasswd")
            .args([
                "--passwd",
                "--file",
                &self.virtual_users_file,
                "--name",
                &user.username,
                "--uid",
                &user.uid.to_string(),
                "--gid",
                &user.gid.to_string(),
                "--home",
                &user.home_dir,
                "--shell",
                &user.shell,
                "--stdin",
            ])
            .stdin(std::process::Stdio::piped())
            .output()
            .await;

        // If ftpasswd is not available, write directly to file
        if output.is_err() || !output.as_ref().unwrap().status.success() {
            self.write_virtual_user_manually(user).await?;
        }

        // Set home directory ownership
        Command::new("chown")
            .args([
                "-R",
                &format!("{}:{}", user.uid, user.gid),
                &user.home_dir,
            ])
            .output()
            .await
            .ok();

        Ok(())
    }

    /// Write virtual user to passwd file manually
    async fn write_virtual_user_manually(&self, user: &FtpVirtualUser) -> Result<()> {
        let mut content = String::new();

        if Path::new(&self.virtual_users_file).exists() {
            content = tokio::fs::read_to_string(&self.virtual_users_file)
                .await
                .unwrap_or_default();

            // Remove existing entry for this user
            content = content
                .lines()
                .filter(|line| {
                    !line.starts_with(&format!("{}:", user.username))
                })
                .collect::<Vec<_>>()
                .join("\n");

            if !content.is_empty() && !content.ends_with('\n') {
                content.push('\n');
            }
        }

        // Add new entry
        // Format: username:password_hash:uid:gid:gecos:home:shell
        content.push_str(&format!(
            "{}:{}:{}:{}:{}:{}:{}\n",
            user.username,
            user.password_hash,
            user.uid,
            user.gid,
            user.username, // gecos
            user.home_dir,
            user.shell
        ));

        // Ensure directory exists
        if let Some(parent) = Path::new(&self.virtual_users_file).parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }

        tokio::fs::write(&self.virtual_users_file, content)
            .await
            .map_err(|e| Error::Internal(format!("Failed to write virtual users file: {}", e)))?;

        Ok(())
    }

    /// Remove a virtual user
    pub async fn remove_virtual_user(&self, username: &str) -> Result<()> {
        if !Path::new(&self.virtual_users_file).exists() {
            return Ok(());
        }

        let content = tokio::fs::read_to_string(&self.virtual_users_file)
            .await
            .map_err(|e| Error::Internal(format!("Failed to read virtual users file: {}", e)))?;

        let new_content: String = content
            .lines()
            .filter(|line| !line.starts_with(&format!("{}:", username)))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";

        tokio::fs::write(&self.virtual_users_file, new_content)
            .await
            .map_err(|e| Error::Internal(format!("Failed to write virtual users file: {}", e)))?;

        Ok(())
    }

    /// List virtual users
    pub async fn list_virtual_users(&self) -> Result<Vec<String>> {
        if !Path::new(&self.virtual_users_file).exists() {
            return Ok(vec![]);
        }

        let content = tokio::fs::read_to_string(&self.virtual_users_file)
            .await
            .map_err(|e| Error::Internal(format!("Failed to read virtual users file: {}", e)))?;

        let users: Vec<String> = content
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .filter_map(|line| line.split(':').next().map(|s| s.to_string()))
            .collect();

        Ok(users)
    }

    /// Create password hash for virtual user
    pub async fn create_password_hash(&self, password: &str) -> Result<String> {
        // Use OpenSSL to create MD5 crypt hash (compatible with ProFTPD)
        let output = Command::new("openssl")
            .args(["passwd", "-1", password])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to create password hash: {}", e)))?;

        if !output.status.success() {
            return Err(Error::Internal("Failed to create password hash".to_string()));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Get uptime in seconds
    async fn get_uptime(&self) -> Result<u64> {
        let output = Command::new("ps")
            .args(["-eo", "pid,etime,comm"])
            .output()
            .await;

        if let Ok(output) = output {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if line.contains("proftpd") {
                    // Parse etime format: [[DD-]HH:]MM:SS
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2 {
                        return Ok(self.parse_etime(parts[1]));
                    }
                }
            }
        }

        Ok(0)
    }

    /// Parse etime format to seconds
    fn parse_etime(&self, etime: &str) -> u64 {
        let mut seconds = 0u64;

        // Format: [[DD-]HH:]MM:SS
        if etime.contains('-') {
            let day_parts: Vec<&str> = etime.splitn(2, '-').collect();
            if let Ok(days) = day_parts[0].parse::<u64>() {
                seconds += days * 86400;
            }
            if day_parts.len() > 1 {
                seconds += self.parse_hms(day_parts[1]);
            }
        } else {
            seconds = self.parse_hms(etime);
        }

        seconds
    }

    /// Parse HH:MM:SS or MM:SS to seconds
    fn parse_hms(&self, hms: &str) -> u64 {
        let parts: Vec<u64> = hms
            .split(':')
            .filter_map(|p| p.parse().ok())
            .collect();

        match parts.len() {
            3 => parts[0] * 3600 + parts[1] * 60 + parts[2],
            2 => parts[0] * 60 + parts[1],
            1 => parts[0],
            _ => 0,
        }
    }

    /// Get transfer statistics from xferlog
    pub async fn get_transfer_stats(&self, log_path: Option<&str>) -> Result<FtpTransferStats> {
        let path = log_path.unwrap_or("/var/log/proftpd/xferlog");

        if !Path::new(path).exists() {
            return Ok(FtpTransferStats {
                bytes_uploaded: 0,
                bytes_downloaded: 0,
                files_uploaded: 0,
                files_downloaded: 0,
                failed_logins: 0,
                successful_logins: 0,
            });
        }

        let content = tokio::fs::read_to_string(path).await.map_err(|e| {
            Error::Internal(format!("Failed to read xferlog: {}", e))
        })?;

        let mut stats = FtpTransferStats {
            bytes_uploaded: 0,
            bytes_downloaded: 0,
            files_uploaded: 0,
            files_downloaded: 0,
            failed_logins: 0,
            successful_logins: 0,
        };

        for line in content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 9 {
                // xferlog format: ... bytes direction type flags duration filename ...
                if let Ok(bytes) = parts[7].parse::<u64>() {
                    match parts[11] {
                        "i" => {
                            stats.bytes_uploaded += bytes;
                            stats.files_uploaded += 1;
                        }
                        "o" => {
                            stats.bytes_downloaded += bytes;
                            stats.files_downloaded += 1;
                        }
                        _ => {}
                    }
                }
            }
        }

        Ok(stats)
    }

    /// Configure SFTP via OpenSSH
    pub async fn configure_sftp(&self, config: &SftpConfig) -> Result<()> {
        let sshd_content = tokio::fs::read_to_string(&self.sshd_config)
            .await
            .map_err(|e| Error::Internal(format!("Failed to read sshd_config: {}", e)))?;

        let mut new_content = String::new();
        let mut in_match_block = false;
        let mut sftp_added = false;

        for line in sshd_content.lines() {
            // Skip existing SFTP Match blocks
            if line.contains("Match Group") && line.contains("sftpusers") {
                in_match_block = true;
                continue;
            }

            if in_match_block {
                if line.starts_with("Match") || line.trim().is_empty() {
                    in_match_block = false;
                } else {
                    continue;
                }
            }

            // Replace existing Subsystem sftp
            if line.trim().starts_with("Subsystem") && line.contains("sftp") {
                if config.enabled && !sftp_added {
                    new_content.push_str("Subsystem sftp internal-sftp\n");
                    sftp_added = true;
                }
                continue;
            }

            new_content.push_str(line);
            new_content.push('\n');
        }

        // Add SFTP configuration if enabled
        if config.enabled && !sftp_added {
            new_content.push_str("\n# SFTP Configuration - Managed by Horcrux\n");
            new_content.push_str("Subsystem sftp internal-sftp\n");
        }

        // Add Match block for SFTP users
        if config.enabled && !config.allowed_groups.is_empty() {
            new_content.push_str(&format!(
                "\nMatch Group {}\n",
                config.allowed_groups.join(",")
            ));

            if let Some(ref chroot) = config.chroot_directory {
                new_content.push_str(&format!("    ChrootDirectory {}\n", chroot));
            }

            if config.force_command {
                new_content.push_str("    ForceCommand internal-sftp\n");
            }

            if config.disable_shell {
                new_content.push_str("    X11Forwarding no\n");
                new_content.push_str("    AllowTcpForwarding no\n");
            }
        }

        // Backup and write
        let backup_path = format!("{}.bak.{}", self.sshd_config, chrono::Utc::now().timestamp());
        tokio::fs::copy(&self.sshd_config, &backup_path).await.ok();

        tokio::fs::write(&self.sshd_config, new_content)
            .await
            .map_err(|e| Error::Internal(format!("Failed to write sshd_config: {}", e)))?;

        // Test config
        let output = Command::new("sshd")
            .args(["-t"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("sshd test failed: {}", e)))?;

        if !output.status.success() {
            // Restore backup
            tokio::fs::copy(&backup_path, &self.sshd_config).await.ok();
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Validation(format!(
                "sshd configuration error: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Reload SSH daemon for SFTP changes
    pub async fn reload_sshd(&self) -> Result<()> {
        let (cmd, args) = if self.use_systemd {
            ("systemctl", vec!["reload", "sshd"])
        } else {
            ("rc-service", vec!["sshd", "reload"])
        };

        let output = Command::new(cmd)
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to reload sshd: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to reload sshd: {}", stderr)));
        }

        Ok(())
    }

    /// Create SFTP group for chroot users
    pub async fn create_sftp_group(&self, group_name: &str) -> Result<()> {
        let output = Command::new("groupadd")
            .args(["-f", group_name])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to create group: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Ignore "already exists" errors
            if !stderr.contains("already exists") {
                return Err(Error::Internal(format!("Failed to create group: {}", stderr)));
            }
        }

        Ok(())
    }

    /// Add user to SFTP group
    pub async fn add_user_to_sftp_group(&self, username: &str, group_name: &str) -> Result<()> {
        let output = Command::new("usermod")
            .args(["-aG", group_name, username])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to add user to group: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to add user to group: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Generate TLS certificate for FTP
    pub async fn generate_tls_cert(
        &self,
        common_name: &str,
        days: u32,
    ) -> Result<FtpTlsConfig> {
        let cert_dir = "/etc/ssl/certs";
        let key_dir = "/etc/ssl/private";

        tokio::fs::create_dir_all(cert_dir).await.ok();
        tokio::fs::create_dir_all(key_dir).await.ok();

        let cert_path = format!("{}/proftpd.crt", cert_dir);
        let key_path = format!("{}/proftpd.key", key_dir);

        // Generate self-signed certificate
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

        // Set secure permissions on key
        Command::new("chmod")
            .args(["600", &key_path])
            .output()
            .await
            .ok();

        Ok(FtpTlsConfig {
            enabled: true,
            certificate: cert_path,
            certificate_key: key_path,
            ca_chain: None,
            required: false,
            required_data: false,
            min_version: "TLSv1.2".to_string(),
            ciphers: None,
        })
    }

    /// Enable FTP service at boot
    pub async fn enable(&self) -> Result<()> {
        let (cmd, args) = if self.use_systemd {
            ("systemctl", vec!["enable", "proftpd"])
        } else {
            ("rc-update", vec!["add", "proftpd", "default"])
        };

        let output = Command::new(cmd)
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to enable proftpd: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to enable proftpd: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Disable FTP service at boot
    pub async fn disable(&self) -> Result<()> {
        let (cmd, args) = if self.use_systemd {
            ("systemctl", vec!["disable", "proftpd"])
        } else {
            ("rc-update", vec!["del", "proftpd", "default"])
        };

        let output = Command::new(cmd)
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to disable proftpd: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to disable proftpd: {}",
                stderr
            )));
        }

        Ok(())
    }
}

impl Default for FtpManager {
    fn default() -> Self {
        Self::new()
    }
}
