//! Rsync server module
//!
//! Manages rsyncd for efficient backup and sync operations.

use horcrux_common::{Error, Result};
use crate::nas::shares::NasShare;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

/// Rsync module configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsyncModule {
    /// Module name
    pub name: String,
    /// Path
    pub path: String,
    /// Description
    pub comment: Option<String>,
    /// Read-only
    pub read_only: bool,
    /// List in module list
    pub list: bool,
    /// Allowed users
    pub auth_users: Vec<String>,
    /// Secrets file path
    pub secrets_file: Option<String>,
    /// Allowed hosts
    pub hosts_allow: Vec<String>,
    /// Denied hosts
    pub hosts_deny: Vec<String>,
    /// Max connections
    pub max_connections: u32,
    /// Transfer log
    pub transfer_logging: bool,
}

impl Default for RsyncModule {
    fn default() -> Self {
        Self {
            name: String::new(),
            path: String::new(),
            comment: None,
            read_only: true,
            list: true,
            auth_users: Vec::new(),
            secrets_file: None,
            hosts_allow: Vec::new(),
            hosts_deny: Vec::new(),
            max_connections: 10,
            transfer_logging: true,
        }
    }
}

/// Rsync server global configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsyncGlobalConfig {
    /// Port
    pub port: u16,
    /// PID file
    pub pid_file: String,
    /// Lock file
    pub lock_file: String,
    /// Log file
    pub log_file: String,
    /// UID
    pub uid: String,
    /// GID
    pub gid: String,
    /// Use chroot
    pub use_chroot: bool,
    /// Max connections
    pub max_connections: u32,
}

impl Default for RsyncGlobalConfig {
    fn default() -> Self {
        Self {
            port: 873,
            pid_file: "/var/run/rsyncd.pid".to_string(),
            lock_file: "/var/run/rsyncd.lock".to_string(),
            log_file: "/var/log/rsyncd.log".to_string(),
            uid: "nobody".to_string(),
            gid: "nogroup".to_string(),
            use_chroot: true,
            max_connections: 100,
        }
    }
}

/// Rsync Manager
pub struct RsyncManager {
    config_path: String,
    global_config: RsyncGlobalConfig,
}

impl RsyncManager {
    /// Create a new rsync manager
    pub fn new() -> Self {
        Self {
            config_path: "/etc/rsyncd.conf".to_string(),
            global_config: RsyncGlobalConfig::default(),
        }
    }

    /// Create module from NAS share
    pub fn module_from_share(share: &NasShare) -> RsyncModule {
        RsyncModule {
            name: share.name.clone(),
            path: share.path.clone(),
            comment: share.description.clone(),
            read_only: false,
            list: true,
            auth_users: Vec::new(),
            secrets_file: None,
            hosts_allow: Vec::new(),
            hosts_deny: Vec::new(),
            max_connections: 10,
            transfer_logging: true,
        }
    }

    /// Generate complete rsyncd.conf
    pub fn generate_config(&self, modules: &[RsyncModule]) -> String {
        let mut config = String::new();

        // Global section
        config.push_str(&self.generate_global_section());

        // Module sections
        for module in modules {
            config.push_str(&self.generate_module_section(module));
        }

        config
    }

    /// Generate global section
    fn generate_global_section(&self) -> String {
        let g = &self.global_config;
        format!(
            r#"# Horcrux NAS rsync configuration
pid file = {}
lock file = {}
log file = {}
port = {}
uid = {}
gid = {}
use chroot = {}
max connections = {}

"#,
            g.pid_file,
            g.lock_file,
            g.log_file,
            g.port,
            g.uid,
            g.gid,
            if g.use_chroot { "yes" } else { "no" },
            g.max_connections,
        )
    }

    /// Generate module section
    fn generate_module_section(&self, module: &RsyncModule) -> String {
        let mut section = String::new();

        section.push_str(&format!("[{}]\n", module.name));
        section.push_str(&format!("    path = {}\n", module.path));

        if let Some(ref comment) = module.comment {
            section.push_str(&format!("    comment = {}\n", comment));
        }

        section.push_str(&format!(
            "    read only = {}\n",
            if module.read_only { "yes" } else { "no" }
        ));
        section.push_str(&format!(
            "    list = {}\n",
            if module.list { "yes" } else { "no" }
        ));

        if !module.auth_users.is_empty() {
            section.push_str(&format!("    auth users = {}\n", module.auth_users.join(",")));
            if let Some(ref secrets) = module.secrets_file {
                section.push_str(&format!("    secrets file = {}\n", secrets));
            }
        }

        if !module.hosts_allow.is_empty() {
            section.push_str(&format!("    hosts allow = {}\n", module.hosts_allow.join(" ")));
        }
        if !module.hosts_deny.is_empty() {
            section.push_str(&format!("    hosts deny = {}\n", module.hosts_deny.join(" ")));
        }

        section.push_str(&format!("    max connections = {}\n", module.max_connections));

        if module.transfer_logging {
            section.push_str("    transfer logging = yes\n");
        }

        section.push('\n');
        section
    }

    /// Write configuration
    pub async fn write_config(&self, modules: &[RsyncModule]) -> Result<()> {
        let config = self.generate_config(modules);
        tokio::fs::write(&self.config_path, config)
            .await
            .map_err(|e| {
                Error::Internal(format!("Failed to write rsyncd.conf: {}", e))
            })
    }

    /// Reload rsync daemon
    pub async fn reload(&self) -> Result<()> {
        // rsync doesn't support reload, restart instead
        crate::nas::services::manage_service(
            &crate::nas::services::NasService::Rsyncd,
            crate::nas::services::ServiceAction::Restart,
        )
        .await
    }

    /// Start rsync daemon
    pub async fn start(&self) -> Result<()> {
        crate::nas::services::manage_service(
            &crate::nas::services::NasService::Rsyncd,
            crate::nas::services::ServiceAction::Start,
        )
        .await
    }

    /// Stop rsync daemon
    pub async fn stop(&self) -> Result<()> {
        crate::nas::services::manage_service(
            &crate::nas::services::NasService::Rsyncd,
            crate::nas::services::ServiceAction::Stop,
        )
        .await
    }

    /// Check if rsyncd is running
    pub async fn is_running(&self) -> bool {
        let output = Command::new("systemctl")
            .args(["is-active", "rsyncd"])
            .output()
            .await;

        if let Ok(out) = output {
            if out.status.success() {
                return true;
            }
        }

        // Try OpenRC
        let output = Command::new("rc-service")
            .args(["rsyncd", "status"])
            .output()
            .await;

        if let Ok(out) = output {
            return out.status.success();
        }

        false
    }

    /// Get rsync service status
    pub async fn get_status(&self) -> Result<RsyncStatus> {
        let running = self.is_running().await;
        let modules = self.list_modules().await.unwrap_or_default();

        Ok(RsyncStatus {
            running,
            port: self.global_config.port,
            module_count: modules.len() as u32,
            chroot_enabled: self.global_config.use_chroot,
        })
    }

    /// List configured modules by parsing rsyncd.conf
    pub async fn list_modules(&self) -> Result<Vec<RsyncModule>> {
        let config = match tokio::fs::read_to_string(&self.config_path).await {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(Error::Internal(format!(
                    "Failed to read rsyncd.conf: {}",
                    e
                )))
            }
        };

        Ok(Self::parse_modules(&config))
    }

    /// Parse modules from rsyncd.conf content
    fn parse_modules(config: &str) -> Vec<RsyncModule> {
        let mut modules = Vec::new();
        let mut current_module: Option<RsyncModule> = None;

        for line in config.lines() {
            let line = line.trim();

            // Skip empty lines and comments
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            // Module header: [modulename]
            if line.starts_with('[') && line.ends_with(']') {
                if let Some(module) = current_module.take() {
                    modules.push(module);
                }
                let name = &line[1..line.len() - 1];
                current_module = Some(RsyncModule {
                    name: name.to_string(),
                    ..Default::default()
                });
                continue;
            }

            // Parse key = value lines inside a module
            if let Some(ref mut module) = current_module {
                if let Some((key, value)) = line.split_once('=') {
                    let key = key.trim();
                    let value = value.trim();

                    match key {
                        "path" => module.path = value.to_string(),
                        "comment" => module.comment = Some(value.to_string()),
                        "read only" => module.read_only = value == "yes",
                        "list" => module.list = value == "yes",
                        "auth users" => {
                            module.auth_users = value.split(',').map(|s| s.trim().to_string()).collect();
                        }
                        "secrets file" => module.secrets_file = Some(value.to_string()),
                        "hosts allow" => {
                            module.hosts_allow = value.split_whitespace().map(|s| s.to_string()).collect();
                        }
                        "hosts deny" => {
                            module.hosts_deny = value.split_whitespace().map(|s| s.to_string()).collect();
                        }
                        "max connections" => {
                            module.max_connections = value.parse().unwrap_or(10);
                        }
                        "transfer logging" => module.transfer_logging = value == "yes",
                        _ => {} // Ignore unknown options
                    }
                }
            }
        }

        if let Some(module) = current_module {
            modules.push(module);
        }

        modules
    }

    /// Get a specific module by name
    pub async fn get_module(&self, name: &str) -> Result<RsyncModule> {
        let modules = self.list_modules().await?;
        for module in modules {
            if module.name == name {
                return Ok(module);
            }
        }
        Err(Error::NotFound(format!("Rsync module '{}' not found", name)))
    }

    /// Add a module to the configuration
    pub async fn add_module(&self, module: &RsyncModule) -> Result<()> {
        let mut modules = self.list_modules().await?;

        // Check if module already exists
        if modules.iter().any(|m| m.name == module.name) {
            return Err(Error::AlreadyExists(format!(
                "Rsync module '{}' already exists",
                module.name
            )));
        }

        modules.push(module.clone());
        self.write_config(&modules).await?;
        self.reload().await
    }

    /// Update an existing module
    pub async fn update_module(&self, module: &RsyncModule) -> Result<()> {
        let mut modules = self.list_modules().await?;

        let found = modules.iter_mut().find(|m| m.name == module.name);
        if let Some(existing) = found {
            *existing = module.clone();
        } else {
            return Err(Error::NotFound(format!(
                "Rsync module '{}' not found",
                module.name
            )));
        }

        self.write_config(&modules).await?;
        self.reload().await
    }

    /// Delete a module
    pub async fn delete_module(&self, name: &str) -> Result<()> {
        let modules = self.list_modules().await?;
        let new_modules: Vec<RsyncModule> = modules.into_iter().filter(|m| m.name != name).collect();

        self.write_config(&new_modules).await?;
        self.reload().await
    }

    /// Create a secrets file for a module
    pub async fn create_secrets_file(&self, module_name: &str, users: &[(String, String)]) -> Result<String> {
        let secrets_path = format!("/etc/rsyncd.secrets.{}", module_name);

        let mut content = String::new();
        for (user, password) in users {
            content.push_str(&format!("{}:{}\n", user, password));
        }

        tokio::fs::write(&secrets_path, &content).await.map_err(|e| {
            Error::Internal(format!("Failed to write secrets file: {}", e))
        })?;

        // Set permissions to 600
        let output = Command::new("chmod")
            .args(["600", &secrets_path])
            .output()
            .await;

        if let Ok(out) = output {
            if !out.status.success() {
                return Err(Error::Internal("Failed to set secrets file permissions".to_string()));
            }
        }

        Ok(secrets_path)
    }

    /// Add a user to an existing secrets file
    pub async fn add_user_to_secrets(&self, secrets_path: &str, user: &str, password: &str) -> Result<()> {
        let mut content = tokio::fs::read_to_string(secrets_path)
            .await
            .unwrap_or_default();

        // Remove existing entry for this user if any
        let lines: Vec<&str> = content
            .lines()
            .filter(|line| !line.starts_with(&format!("{}:", user)))
            .collect();

        content = lines.join("\n");
        if !content.is_empty() {
            content.push('\n');
        }
        content.push_str(&format!("{}:{}\n", user, password));

        tokio::fs::write(secrets_path, content).await.map_err(|e| {
            Error::Internal(format!("Failed to write secrets file: {}", e))
        })
    }

    /// Remove a user from a secrets file
    pub async fn remove_user_from_secrets(&self, secrets_path: &str, user: &str) -> Result<()> {
        let content = tokio::fs::read_to_string(secrets_path)
            .await
            .map_err(|e| Error::Internal(format!("Failed to read secrets file: {}", e)))?;

        let lines: Vec<&str> = content
            .lines()
            .filter(|line| !line.starts_with(&format!("{}:", user)))
            .collect();

        let new_content = lines.join("\n") + "\n";

        tokio::fs::write(secrets_path, new_content).await.map_err(|e| {
            Error::Internal(format!("Failed to write secrets file: {}", e))
        })
    }

    /// Set global configuration
    pub fn set_global_config(&mut self, config: RsyncGlobalConfig) {
        self.global_config = config;
    }
}

/// Rsync service status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsyncStatus {
    /// Service running
    pub running: bool,
    /// Listen port
    pub port: u16,
    /// Number of configured modules
    pub module_count: u32,
    /// Chroot enabled
    pub chroot_enabled: bool,
}

impl Default for RsyncManager {
    fn default() -> Self {
        Self::new()
    }
}
