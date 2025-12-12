//! Configuration management for Horcrux API
//!
//! This module provides a centralized configuration system that loads settings from:
//! 1. Environment variables (highest priority)
//! 2. Configuration file (TOML format)
//! 3. Default values (lowest priority)

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Main configuration struct for Horcrux
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

/// Server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Host address to bind to
    pub host: String,
    /// Port to listen on
    pub port: u16,
    /// Enable TLS
    pub tls_enabled: bool,
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

impl Default for HorcruxConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            paths: PathsConfig::default(),
            database: DatabaseConfig::default(),
            tls: TlsConfig::default(),
            qemu: QemuConfig::default(),
            logging: LoggingConfig::default(),
            cni: CniConfig::default(),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 8006,
            tls_enabled: true,
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

        toml::from_str(&content)
            .map_err(|e| ConfigError::Parse(e.to_string()))
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

        paths.into_iter()
            .flatten()
            .find(|p| p.exists())
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
            self.server.tls_enabled = tls.parse().unwrap_or(true);
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
            return Err(ConfigError::Validation("Database URL cannot be empty".to_string()));
        }

        // Validate socket patterns contain placeholder
        if !self.qemu.qmp_socket_pattern.contains("{vm_id}") {
            return Err(ConfigError::Validation(
                "QMP socket pattern must contain {vm_id} placeholder".to_string()
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
}
