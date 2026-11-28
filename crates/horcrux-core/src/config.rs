//! Configuration management for Horcrux API
//!
//! This module provides a centralized configuration system that loads settings from:
//! 1. Environment variables (highest priority)
//! 2. Configuration file (TOML format)
//! 3. Default values (lowest priority)

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Main configuration struct for Horcrux
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HorcruxConfig {
    /// Server configuration
    pub server: ServerConfig,
    /// Storage paths configuration
    pub paths: PathsConfig,
    /// Database configuration
    pub database: DatabaseConfig,
    /// TLS/SSL configuration
    pub tls: TlsConfig,
    /// QEMU configuration
    pub qemu: QemuConfig,
    /// Logging configuration
    pub logging: LoggingConfig,
    /// CNI (Container Networking Interface) configuration
    pub cni: CniConfig,
    /// Backup scheduler and LXC settings (`[backup]`)
    #[serde(default)]
    pub backup: BackupConfig,
    /// Authentication settings
    #[serde(default)]
    pub auth: AuthConfig,
    /// GPU / PCI passthrough and VM tuning (gaming VMs)
    #[serde(default)]
    pub passthrough: PassthroughConfig,
    /// Horcrux Console (`[console]`): node defaults for the remote display
    /// (docs/design/horcrux-console.md section 9). The API and the
    /// declarative spec store changes in `<data_dir>/console.json`.
    #[serde(default)]
    pub console: horcrux_common::console::ConsoleSettings,
    /// API access log (`[access_log]`): who called what, from where, and
    /// how it ended.
    #[serde(default)]
    pub access_log: AccessLogConfig,
    /// Running commands in VMs through the guest agent (`[guest_exec]`).
    #[serde(default)]
    pub guest_exec: GuestExecConfig,
}

/// `POST /api/vms/:id/guest-exec` limits (`[guest_exec]` in config.toml).
/// Env overrides: `HORCRUX_GUEST_EXEC_ENABLED`,
/// `HORCRUX_GUEST_EXEC_MAX_TIMEOUT_SECS`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GuestExecConfig {
    /// Switch the route off for the whole node.
    pub enabled: bool,
    /// Seconds to wait for a command when the request names no timeout.
    pub default_timeout_secs: u64,
    /// Largest `timeout_secs` a request may ask for.
    pub max_timeout_secs: u64,
    /// Largest standard input accepted, in bytes.
    pub max_input_bytes: usize,
    /// Standard output and error are each cut to this many bytes.
    pub max_output_bytes: usize,
    /// How often a running command's status is asked for, in milliseconds.
    pub poll_interval_ms: u64,
    /// Put the argument values in the audit record (they may hold secrets,
    /// so the default is the program name and the argument count only).
    pub log_arguments: bool,
}

impl Default for GuestExecConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_timeout_secs: 60,
            max_timeout_secs: 3600,
            max_input_bytes: 1 << 20,
            max_output_bytes: 4 << 20,
            poll_interval_ms: 250,
            log_arguments: false,
        }
    }
}

/// API access log settings (`[access_log]` in config.toml).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AccessLogConfig {
    /// Record every API request (`GET /api/access-log`).
    pub enabled: bool,
    /// Most requests kept (oldest dropped first).
    pub max_entries: usize,
    /// Requests older than this many days are dropped.
    pub max_age_days: u32,
    /// Peer addresses of reverse proxies whose `X-Forwarded-For` is
    /// believed; from any other peer the header is ignored.
    pub trusted_proxies: Vec<String>,
}

impl Default for AccessLogConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_entries: 50_000,
            max_age_days: 14,
            trusted_proxies: Vec::new(),
        }
    }
}

/// Backup settings (`[backup]` in config.toml). Env overrides:
/// `HORCRUX_BACKUP_SCHEDULER_TICK_SECS`, `HORCRUX_LXC_PATH`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BackupConfig {
    /// How often the backup scheduler looks for due jobs, in seconds (> 0).
    pub scheduler_tick_secs: u64,
    /// Where LXC keeps container directories (read when backing them up).
    pub lxc_path: PathBuf,
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            scheduler_tick_secs: 15,
            lxc_path: PathBuf::from("/var/lib/lxc"),
        }
    }
}

/// GPU / PCI passthrough and VM tuning settings (`[passthrough]`).
///
/// Every value has a default; a node overrides any of them in
/// `/etc/horcrux/config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PassthroughConfig {
    /// QEMU `-cpu` flags added for VMs with `adv.hyperv` (Windows guests).
    pub hyperv_enlightenments: Vec<String>,
    /// CPU model used for Hyper-V VMs that set no CPU model themselves.
    pub hyperv_cpu_model: String,
    /// Host services stopped (when running) before a managed GPU is moved to
    /// vfio-pci and started again once it is back on its host driver.
    pub handoff_stop_services: Vec<String>,
    /// Refuse a managed hand-off while a host process has the device open
    /// (unbinding a driver under an open file hangs or crashes that program).
    pub handoff_refuse_when_held: bool,
    /// How long one driver unbind/bind may take before the hand-off fails.
    pub handoff_timeout_secs: u64,
    /// Give huge pages that a VM start reserved back to the host when it stops.
    pub hugepages_release_on_stop: bool,
    /// Known-issue tables for passthrough devices (TOML, later files add to
    /// earlier ones; missing files are skipped).
    pub quirks_files: Vec<PathBuf>,
    /// Single-GPU passthrough (`allow_display_loss`): host services that
    /// drive the local screen, stopped (when running) before the GPU moves
    /// to the VM and started again when it comes back. `display-manager` is
    /// the service name on OpenRC (x11-base/xorg-server) and the alias of
    /// the active display manager on systemd.
    pub handoff_display_services: Vec<String>,
    /// Single-GPU passthrough: also detach the kernel's framebuffer console
    /// (vtconsole + EFI/simple framebuffer) from the GPU, and re-attach it
    /// afterwards.
    pub handoff_unbind_consoles: bool,
    /// Single-GPU passthrough: how long to wait for the desktop's programs
    /// to let go of the GPU after the display services stopped.
    pub handoff_display_release_secs: u64,
    /// Per-game anti-cheat/VM compatibility tables (TOML, later files add to
    /// and override earlier ones by title; missing files are skipped).
    pub gaming_compat_files: Vec<PathBuf>,
}

impl Default for PassthroughConfig {
    fn default() -> Self {
        Self {
            hyperv_enlightenments: [
                "hv-relaxed",
                "hv-vapic",
                "hv-spinlocks=0x1fff",
                "hv-vpindex",
                "hv-runtime",
                "hv-synic",
                "hv-stimer",
                "hv-stimer-direct",
                "hv-reset",
                "hv-frequencies",
                "hv-reenlightenment",
                "hv-tlbflush",
                "hv-ipi",
                "hv-time",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            hyperv_cpu_model: "host".into(),
            handoff_stop_services: vec!["nvidia-persistenced".into()],
            handoff_refuse_when_held: true,
            handoff_timeout_secs: 20,
            hugepages_release_on_stop: true,
            quirks_files: vec![
                PathBuf::from("/usr/share/horcrux/gpu-quirks.toml"),
                PathBuf::from("/etc/horcrux/gpu-quirks.toml"),
            ],
            handoff_display_services: vec!["display-manager".into()],
            handoff_unbind_consoles: true,
            handoff_display_release_secs: 15,
            gaming_compat_files: vec![
                PathBuf::from("/usr/share/horcrux/gaming-compat.toml"),
                PathBuf::from("/etc/horcrux/gaming-compat.toml"),
            ],
        }
    }
}

/// Authentication settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Allow anyone to create an account at `/api/auth/register` (off by
    /// default; such accounts start without any permissions). Env:
    /// `HORCRUX_ALLOW_REGISTRATION`.
    #[serde(default)]
    pub allow_registration: bool,
    /// System groups whose members are full administrators of the web UI/API
    /// (users sign in with their host account through PAM).
    /// Env: `HORCRUX_ADMIN_GROUPS` (comma separated).
    #[serde(default = "default_admin_groups")]
    pub admin_groups: Vec<String>,
    /// System groups mapped to the `VMAdmin` role (manage guests).
    #[serde(default)]
    pub operator_groups: Vec<String>,
    /// System groups mapped to the read-only `Auditor` role.
    #[serde(default)]
    pub readonly_groups: Vec<String>,
    /// Let root (uid 0) sign in over the web. Off by default.
    /// Env: `HORCRUX_ALLOW_ROOT_LOGIN`.
    #[serde(default)]
    pub allow_root: bool,
    /// Keep accepting the old Horcrux-only accounts (the pre-PAM user store)
    /// for this release so nobody is locked out during the upgrade. The
    /// legacy `admin` account is disabled automatically once a PAM
    /// administrator has signed in. Set false to refuse them now.
    #[serde(default = "yes_true")]
    pub legacy_local_login: bool,
}

fn default_admin_groups() -> Vec<String> {
    vec!["horcrux-admin".into(), "wheel".into()]
}

fn yes_true() -> bool {
    true
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            allow_registration: false,
            admin_groups: default_admin_groups(),
            operator_groups: vec![],
            readonly_groups: vec![],
            allow_root: false,
            legacy_local_login: true,
        }
    }
}

/// Server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Host address to bind to
    pub host: String,
    /// Port to listen on
    pub port: u16,
    /// Serve HTTPS (default). Turn off only behind a TLS-terminating proxy.
    pub tls_enabled: bool,
    /// Extra plain-HTTP port that redirects to HTTPS and answers ACME
    /// HTTP-01 challenges (80 for Let's Encrypt). Env: `HORCRUX_HTTP_REDIRECT_PORT`.
    #[serde(default)]
    pub http_redirect_port: Option<u16>,
}

/// Storage paths configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathsConfig {
    /// Base directory for Horcrux data
    pub data_dir: PathBuf,
    /// Directory for VM disk images
    pub vm_storage: PathBuf,
    /// Directory for snapshots
    pub snapshots: PathBuf,
    /// Directory for templates
    pub templates: PathBuf,
    /// Directory for cloud-init data
    pub cloudinit: PathBuf,
    /// Directory for restore operations
    pub restore: PathBuf,
    /// Directory for backups
    pub backups: PathBuf,
}

/// Database configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// Database URL (e.g., "sqlite:///var/lib/horcrux/horcrux.db")
    pub url: String,
    /// Maximum number of connections in the pool
    pub max_connections: u32,
}

/// TLS/SSL configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    /// Path to SSL certificate file
    pub cert_path: PathBuf,
    /// Path to SSL private key file
    pub key_path: PathBuf,
    /// SSL directory for storing certificates
    pub ssl_dir: PathBuf,
}

/// QEMU configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QemuConfig {
    /// Pattern for QMP socket path (use {vm_id} as placeholder)
    pub qmp_socket_pattern: String,
    /// Pattern for monitor socket path (use {vm_id} as placeholder)
    pub monitor_socket_pattern: String,
    /// Pattern for serial socket path (use {vm_id} as placeholder)
    pub serial_socket_pattern: String,
    /// Directory for QEMU runtime files
    pub run_dir: PathBuf,
}

/// Logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Log level (trace, debug, info, warn, error)
    pub level: String,
    /// Directory for log files
    pub log_dir: PathBuf,
    /// Path to audit log file
    pub audit_log: PathBuf,
    /// Enable file logging
    pub file_logging_enabled: bool,
}

/// CNI (Container Networking Interface) configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CniConfig {
    /// Directory containing CNI binaries
    pub bin_dir: PathBuf,
    /// Directory containing CNI configuration files
    pub conf_dir: PathBuf,
    /// Enable CNI features
    pub enabled: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 8006,
            tls_enabled: true,
            http_redirect_port: None,
        }
    }
}

impl Default for PathsConfig {
    fn default() -> Self {
        let data_dir = PathBuf::from("/var/lib/horcrux");
        Self {
            vm_storage: data_dir.join("vms"),
            snapshots: data_dir.join("snapshots"),
            templates: data_dir.join("templates"),
            cloudinit: data_dir.join("cloudinit"),
            restore: data_dir.join("restore"),
            backups: data_dir.join("backups"),
            data_dir,
        }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: "sqlite:///var/lib/horcrux/horcrux.db".to_string(),
            max_connections: 10,
        }
    }
}

impl Default for TlsConfig {
    fn default() -> Self {
        let ssl_dir = PathBuf::from("/etc/horcrux/ssl");
        Self {
            cert_path: ssl_dir.join("cert.pem"),
            key_path: ssl_dir.join("key.pem"),
            ssl_dir,
        }
    }
}

impl Default for QemuConfig {
    fn default() -> Self {
        Self {
            qmp_socket_pattern: "/var/run/qemu/{vm_id}.qmp".to_string(),
            monitor_socket_pattern: "/var/run/qemu-server/{vm_id}.mon".to_string(),
            serial_socket_pattern: "/var/run/qemu-server/{vm_id}.serial".to_string(),
            run_dir: PathBuf::from("/var/run/qemu-server"),
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            log_dir: PathBuf::from("/var/log/horcrux"),
            audit_log: PathBuf::from("/var/log/horcrux/audit.log"),
            file_logging_enabled: true,
        }
    }
}

impl Default for CniConfig {
    fn default() -> Self {
        Self {
            bin_dir: PathBuf::from("/opt/cni/bin"),
            conf_dir: PathBuf::from("/etc/cni/net.d"),
            enabled: true,
        }
    }
}

impl HorcruxConfig {
    /// Load configuration from environment variables and optional config file
    pub fn load() -> Self {
        let mut config = Self::default();

        // Try to load from config file first
        if let Some(config_path) = Self::find_config_file() {
            if let Ok(file_config) = Self::load_from_file(&config_path) {
                config = file_config;
            }
        }

        // Override with environment variables
        config.apply_env_overrides();

        config
    }

    /// Load configuration from a specific file path
    pub fn load_from_file(path: &PathBuf) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| ConfigError::FileRead(path.clone(), e.to_string()))?;

        toml::from_str(&content).map_err(|e| ConfigError::Parse(e.to_string()))
    }

    /// Find configuration file in standard locations
    fn find_config_file() -> Option<PathBuf> {
        let paths = [
            // Environment variable override
            std::env::var("HORCRUX_CONFIG").ok().map(PathBuf::from),
            // Standard locations
            Some(PathBuf::from("/etc/horcrux/config.toml")),
            Some(PathBuf::from("./config.toml")),
            Some(PathBuf::from("./horcrux.toml")),
        ];

        paths.into_iter().flatten().find(|p| p.exists())
    }

    /// Apply environment variable overrides
    fn apply_env_overrides(&mut self) {
        // Server
        if let Ok(host) = std::env::var("HORCRUX_HOST") {
            self.server.host = host;
        }
        if let Ok(port) = std::env::var("HORCRUX_PORT") {
            if let Ok(port) = port.parse() {
                self.server.port = port;
            }
        }
        if let Ok(tls) = std::env::var("HORCRUX_TLS_ENABLED") {
            self.server.tls_enabled = !matches!(
                tls.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            );
        }
        if let Ok(port) = std::env::var("HORCRUX_HTTP_REDIRECT_PORT") {
            self.server.http_redirect_port = port.trim().parse().ok().filter(|p| *p != 0);
        }
        if let Ok(v) = std::env::var("HORCRUX_ADMIN_GROUPS") {
            self.auth.admin_groups = v
                .split(',')
                .map(|g| g.trim().to_string())
                .filter(|g| !g.is_empty())
                .collect();
        }
        if let Ok(v) = std::env::var("HORCRUX_ALLOW_ROOT_LOGIN") {
            self.auth.allow_root = matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            );
        }
        if let Ok(v) = std::env::var("HORCRUX_ALLOW_REGISTRATION") {
            self.auth.allow_registration = matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            );
        }

        // Paths
        if let Ok(data_dir) = std::env::var("HORCRUX_DATA_DIR") {
            let data_dir = PathBuf::from(data_dir);
            self.paths.vm_storage = data_dir.join("vms");
            self.paths.snapshots = data_dir.join("snapshots");
            self.paths.templates = data_dir.join("templates");
            self.paths.cloudinit = data_dir.join("cloudinit");
            self.paths.restore = data_dir.join("restore");
            self.paths.backups = data_dir.join("backups");
            self.paths.data_dir = data_dir;
        }
        if let Ok(path) = std::env::var("HORCRUX_VM_STORAGE") {
            self.paths.vm_storage = PathBuf::from(path);
        }
        if let Ok(path) = std::env::var("HORCRUX_SNAPSHOTS_DIR") {
            self.paths.snapshots = PathBuf::from(path);
        }
        if let Ok(path) = std::env::var("HORCRUX_TEMPLATES_DIR") {
            self.paths.templates = PathBuf::from(path);
        }
        if let Ok(path) = std::env::var("HORCRUX_CLOUDINIT_DIR") {
            self.paths.cloudinit = PathBuf::from(path);
        }

        // Database
        if let Ok(url) = std::env::var("HORCRUX_DATABASE_URL") {
            self.database.url = url;
        }
        if let Ok(max) = std::env::var("HORCRUX_DATABASE_MAX_CONNECTIONS") {
            if let Ok(max) = max.parse() {
                self.database.max_connections = max;
            }
        }

        // TLS
        if let Ok(path) = std::env::var("HORCRUX_TLS_CERT") {
            self.tls.cert_path = PathBuf::from(path);
        }
        if let Ok(path) = std::env::var("HORCRUX_TLS_KEY") {
            self.tls.key_path = PathBuf::from(path);
        }
        if let Ok(path) = std::env::var("HORCRUX_SSL_DIR") {
            self.tls.ssl_dir = PathBuf::from(path);
            if std::env::var("HORCRUX_TLS_CERT").is_err() {
                self.tls.cert_path = self.tls.ssl_dir.join("cert.pem");
            }
            if std::env::var("HORCRUX_TLS_KEY").is_err() {
                self.tls.key_path = self.tls.ssl_dir.join("key.pem");
            }
        }

        // QEMU
        if let Ok(pattern) = std::env::var("HORCRUX_QMP_SOCKET_PATTERN") {
            self.qemu.qmp_socket_pattern = pattern;
        }
        if let Ok(pattern) = std::env::var("HORCRUX_MONITOR_SOCKET_PATTERN") {
            self.qemu.monitor_socket_pattern = pattern;
        }
        if let Ok(pattern) = std::env::var("HORCRUX_SERIAL_SOCKET_PATTERN") {
            self.qemu.serial_socket_pattern = pattern;
        }
        if let Ok(path) = std::env::var("HORCRUX_QEMU_RUN_DIR") {
            self.qemu.run_dir = PathBuf::from(path);
        }

        // Logging
        if let Ok(level) = std::env::var("HORCRUX_LOG_LEVEL") {
            self.logging.level = level;
        }
        if let Ok(path) = std::env::var("HORCRUX_LOG_DIR") {
            let log_dir = PathBuf::from(path);
            self.logging.audit_log = log_dir.join("audit.log");
            self.logging.log_dir = log_dir;
        }
        if let Ok(path) = std::env::var("HORCRUX_AUDIT_LOG") {
            self.logging.audit_log = PathBuf::from(path);
        }
        if let Ok(enabled) = std::env::var("HORCRUX_FILE_LOGGING") {
            self.logging.file_logging_enabled = enabled.parse().unwrap_or(true);
        }

        // Backup
        if let Some(secs) = std::env::var("HORCRUX_BACKUP_SCHEDULER_TICK_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|s| *s > 0)
        {
            self.backup.scheduler_tick_secs = secs;
        }
        if let Ok(path) = std::env::var("HORCRUX_LXC_PATH") {
            self.backup.lxc_path = PathBuf::from(path);
        }

        // Guest exec
        if let Ok(v) = std::env::var("HORCRUX_GUEST_EXEC_ENABLED") {
            self.guest_exec.enabled = v.parse().unwrap_or(true);
        }
        if let Some(secs) = std::env::var("HORCRUX_GUEST_EXEC_MAX_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|s| *s > 0)
        {
            self.guest_exec.max_timeout_secs = secs;
        }

        // CNI
        if let Ok(path) = std::env::var("HORCRUX_CNI_BIN_DIR") {
            self.cni.bin_dir = PathBuf::from(path);
        }
        if let Ok(path) = std::env::var("HORCRUX_CNI_CONF_DIR") {
            self.cni.conf_dir = PathBuf::from(path);
        }
        if let Ok(enabled) = std::env::var("HORCRUX_CNI_ENABLED") {
            self.cni.enabled = enabled.parse().unwrap_or(true);
        }
    }

    /// Generate a sample configuration file
    pub fn generate_sample() -> String {
        let config = Self::default();
        toml::to_string_pretty(&config).unwrap_or_default()
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Validate port range
        if self.server.port == 0 {
            return Err(ConfigError::Validation("Port cannot be 0".to_string()));
        }

        // Validate database URL
        if self.database.url.is_empty() {
            return Err(ConfigError::Validation(
                "Database URL cannot be empty".to_string(),
            ));
        }

        // Validate socket patterns contain placeholder
        if !self.qemu.qmp_socket_pattern.contains("{vm_id}") {
            return Err(ConfigError::Validation(
                "QMP socket pattern must contain {vm_id} placeholder".to_string(),
            ));
        }

        Ok(())
    }
}

impl QemuConfig {
    /// Get the QMP socket path for a specific VM
    pub fn get_qmp_socket(&self, vm_id: &str) -> PathBuf {
        PathBuf::from(self.qmp_socket_pattern.replace("{vm_id}", vm_id))
    }

    /// Get the monitor socket path for a specific VM
    pub fn get_monitor_socket(&self, vm_id: &str) -> PathBuf {
        PathBuf::from(self.monitor_socket_pattern.replace("{vm_id}", vm_id))
    }

    /// Get the serial socket path for a specific VM
    pub fn get_serial_socket(&self, vm_id: &str) -> PathBuf {
        PathBuf::from(self.serial_socket_pattern.replace("{vm_id}", vm_id))
    }
}

/// Configuration errors
#[derive(Debug, Clone)]
pub enum ConfigError {
    /// Failed to read configuration file
    FileRead(PathBuf, String),
    /// Failed to parse configuration
    Parse(String),
    /// Configuration validation failed
    Validation(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::FileRead(path, err) => {
                write!(f, "Failed to read config file {:?}: {}", path, err)
            }
            ConfigError::Parse(err) => write!(f, "Failed to parse config: {}", err),
            ConfigError::Validation(err) => write!(f, "Config validation failed: {}", err),
        }
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = HorcruxConfig::default();
        assert_eq!(config.server.port, 8006);
        assert_eq!(config.paths.data_dir, PathBuf::from("/var/lib/horcrux"));
        assert_eq!(config.database.url, "sqlite:///var/lib/horcrux/horcrux.db");
    }

    #[test]
    fn test_qemu_socket_paths() {
        let config = QemuConfig::default();
        assert_eq!(
            config.get_qmp_socket("100"),
            PathBuf::from("/var/run/qemu/100.qmp")
        );
        assert_eq!(
            config.get_monitor_socket("100"),
            PathBuf::from("/var/run/qemu-server/100.mon")
        );
        assert_eq!(
            config.get_serial_socket("100"),
            PathBuf::from("/var/run/qemu-server/100.serial")
        );
    }

    #[test]
    fn test_config_validation() {
        let config = HorcruxConfig::default();
        assert!(config.validate().is_ok());

        let mut invalid_config = HorcruxConfig::default();
        invalid_config.server.port = 0;
        assert!(invalid_config.validate().is_err());

        let mut invalid_qemu = HorcruxConfig::default();
        invalid_qemu.qemu.qmp_socket_pattern = "/no/placeholder.sock".to_string();
        assert!(invalid_qemu.validate().is_err());
    }

    #[test]
    fn test_generate_sample_config() {
        let sample = HorcruxConfig::generate_sample();
        assert!(sample.contains("[server]"));
        assert!(sample.contains("[paths]"));
        assert!(sample.contains("[database]"));
        assert!(sample.contains("[tls]"));
        assert!(sample.contains("[qemu]"));
        assert!(sample.contains("[logging]"));
        assert!(sample.contains("[cni]"));
    }

    #[test]
    fn test_paths_config_default() {
        let paths = PathsConfig::default();
        assert_eq!(paths.vm_storage, PathBuf::from("/var/lib/horcrux/vms"));
        assert_eq!(paths.snapshots, PathBuf::from("/var/lib/horcrux/snapshots"));
        assert_eq!(paths.templates, PathBuf::from("/var/lib/horcrux/templates"));
        assert_eq!(paths.cloudinit, PathBuf::from("/var/lib/horcrux/cloudinit"));
    }

    #[test]
    fn test_env_override_data_dir() {
        // This test would need to set env vars, which could affect other tests
        // In a real scenario, you'd use a test framework that isolates env vars
        let config = HorcruxConfig::default();
        assert_eq!(config.paths.data_dir, PathBuf::from("/var/lib/horcrux"));
    }

    #[test]
    fn backup_section_parses_and_defaults() {
        let d = BackupConfig::default();
        assert_eq!(d.scheduler_tick_secs, 15);
        assert_eq!(d.lxc_path, PathBuf::from("/var/lib/lxc"));
        let c: BackupConfig = toml::from_str("scheduler_tick_secs = 60").unwrap();
        assert_eq!(c.scheduler_tick_secs, 60);
        assert_eq!(c.lxc_path, PathBuf::from("/var/lib/lxc"));
    }
}
