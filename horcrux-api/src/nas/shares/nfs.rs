//! NFS Server module
//!
//! Manages NFS exports for Unix/Linux file sharing.

use horcrux_common::{Error, Result};
use crate::nas::shares::{NasShare, NfsExportConfig, NfsClient, NfsSecurity};
use crate::nas::AccessLevel;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

/// NFS global configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NfsGlobalConfig {
    /// Number of NFS server threads
    pub threads: u32,
    /// Enable NFSv3
    pub nfsv3: bool,
    /// Enable NFSv4
    pub nfsv4: bool,
    /// Enable NFSv4.1
    pub nfsv41: bool,
    /// Enable NFSv4.2
    pub nfsv42: bool,
    /// NFSv4 domain
    pub nfsv4_domain: Option<String>,
    /// Enable UDP
    pub udp: bool,
    /// Enable TCP
    pub tcp: bool,
    /// Mount port (0 for random)
    pub mountd_port: u16,
    /// Statd port (0 for random)
    pub statd_port: u16,
    /// Lockd port (0 for random)
    pub lockd_port: u16,
}

impl Default for NfsGlobalConfig {
    fn default() -> Self {
        Self {
            threads: 8,
            nfsv3: true,
            nfsv4: true,
            nfsv41: true,
            nfsv42: true,
            nfsv4_domain: None,
            udp: false,
            tcp: true,
            mountd_port: 0,
            statd_port: 0,
            lockd_port: 0,
        }
    }
}

/// NFS client info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NfsClientInfo {
    /// Client address
    pub address: String,
    /// Exported path
    pub export_path: String,
    /// Mount time
    pub mount_time: Option<i64>,
}

/// NFS Server Manager
pub struct NfsServerManager {
    exports_path: String,
    global_config: NfsGlobalConfig,
}

impl NfsServerManager {
    /// Create a new NFS server manager
    pub fn new() -> Self {
        Self {
            exports_path: "/etc/exports".to_string(),
            global_config: NfsGlobalConfig::default(),
        }
    }

    /// Set global configuration
    pub fn set_global_config(&mut self, config: NfsGlobalConfig) {
        self.global_config = config;
    }

    /// Add an export from a NAS share
    pub async fn add_export(&self, share: &NasShare) -> Result<()> {
        let mut exports = self.read_exports().await?;

        // Generate export line
        let export_line = self.generate_export_line(share);

        // Check if export already exists
        let has_export = exports.lines().any(|l| l.starts_with(&share.path));
        if !has_export {
            exports.push_str(&export_line);
            exports.push('\n');
        }

        self.write_exports(&exports).await?;
        self.refresh_exports().await?;

        Ok(())
    }

    /// Remove an export
    pub async fn remove_export(&self, share: &NasShare) -> Result<()> {
        let exports = self.read_exports().await?;

        // Filter out the export line
        let new_exports: String = exports
            .lines()
            .filter(|l| !l.starts_with(&share.path))
            .collect::<Vec<_>>()
            .join("\n");

        self.write_exports(&new_exports).await?;
        self.refresh_exports().await?;

        Ok(())
    }

    /// Generate complete exports file
    pub fn generate_exports(&self, shares: &[NasShare]) -> String {
        let mut exports = String::new();

        exports.push_str("# Horcrux NAS exports - auto-generated\n");
        exports.push_str("# Do not edit manually\n\n");

        for share in shares {
            if share.enabled && share.nfs_config.is_some() {
                exports.push_str(&self.generate_export_line(share));
                exports.push('\n');
            }
        }

        exports
    }

    /// Generate a single export line
    fn generate_export_line(&self, share: &NasShare) -> String {
        let config = share.nfs_config.as_ref().cloned().unwrap_or_default();

        let mut line = share.path.clone();

        for client in &config.clients {
            line.push(' ');
            line.push_str(&self.format_client(client, &config));
        }

        line
    }

    /// Format a client entry
    fn format_client(&self, client: &NfsClient, config: &NfsExportConfig) -> String {
        let mut options = Vec::new();

        // Access mode
        match client.access {
            AccessLevel::ReadWrite => options.push("rw"),
            AccessLevel::ReadOnly => options.push("ro"),
            AccessLevel::NoAccess => return String::new(),
        }

        // Sync mode
        if config.async_writes {
            options.push("async");
        } else {
            options.push("sync");
        }

        // Squash options
        if config.all_squash {
            options.push("all_squash");
        } else if config.root_squash {
            options.push("root_squash");
        } else {
            options.push("no_root_squash");
        }

        // Security
        let sec = match config.security {
            NfsSecurity::Sys => "sec=sys",
            NfsSecurity::Krb5 => "sec=krb5",
            NfsSecurity::Krb5i => "sec=krb5i",
            NfsSecurity::Krb5p => "sec=krb5p",
        };
        options.push(sec);

        // Secure port requirement
        if client.secure {
            options.push("secure");
        } else {
            options.push("insecure");
        }

        // Anonymous UID/GID
        if let Some(anonuid) = config.anonuid {
            options.push(&format!("anonuid={}", anonuid));
        }
        if let Some(anongid) = config.anongid {
            options.push(&format!("anongid={}", anongid));
        }

        // Subtree checking (disabled for better performance)
        options.push("no_subtree_check");

        format!("{}({})", client.host, options.join(","))
    }

    /// Read current exports file
    async fn read_exports(&self) -> Result<String> {
        match tokio::fs::read_to_string(&self.exports_path).await {
            Ok(content) => Ok(content),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(Error::Internal(format!(
                "Failed to read exports: {}",
                e
            ))),
        }
    }

    /// Write exports file
    async fn write_exports(&self, content: &str) -> Result<()> {
        tokio::fs::write(&self.exports_path, content)
            .await
            .map_err(|e| Error::Internal(format!("Failed to write exports: {}", e)))
    }

    /// Refresh exports (exportfs -ra)
    pub async fn refresh_exports(&self) -> Result<()> {
        let output = Command::new("exportfs")
            .args(["-ra"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("exportfs failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "exportfs failed: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// List current exports
    pub async fn list_exports(&self) -> Result<Vec<String>> {
        let output = Command::new("exportfs")
            .args(["-v"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("exportfs failed: {}", e)))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.lines().map(|s| s.to_string()).collect())
    }

    /// Get connected NFS clients
    pub async fn get_clients(&self) -> Result<Vec<NfsClientInfo>> {
        let mut clients = Vec::new();

        // Read from /proc/fs/nfsd/clients if available
        let clients_dir = std::path::Path::new("/proc/fs/nfsd/clients");
        if clients_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(clients_dir) {
                for entry in entries.flatten() {
                    if let Ok(info_path) = entry.path().join("info").canonicalize() {
                        if let Ok(content) = std::fs::read_to_string(&info_path) {
                            // Parse client info
                            let mut address = String::new();
                            for line in content.lines() {
                                if line.starts_with("address:") {
                                    address = line.split(':').nth(1).unwrap_or("").trim().to_string();
                                }
                            }
                            if !address.is_empty() {
                                clients.push(NfsClientInfo {
                                    address,
                                    export_path: String::new(),
                                    mount_time: None,
                                });
                            }
                        }
                    }
                }
            }
        }

        // Fallback: parse showmount output
        if clients.is_empty() {
            let output = Command::new("showmount")
                .args(["--all", "--no-headers"])
                .output()
                .await
                .ok();

            if let Some(output) = output {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    for line in stdout.lines() {
                        let parts: Vec<&str> = line.split(':').collect();
                        if parts.len() >= 2 {
                            clients.push(NfsClientInfo {
                                address: parts[0].to_string(),
                                export_path: parts[1].to_string(),
                                mount_time: None,
                            });
                        }
                    }
                }
            }
        }

        Ok(clients)
    }

    /// Get NFS server statistics
    pub async fn get_stats(&self) -> Result<NfsStats> {
        // Read from /proc/net/rpc/nfsd
        let content = tokio::fs::read_to_string("/proc/net/rpc/nfsd")
            .await
            .unwrap_or_default();

        let mut stats = NfsStats::default();

        for line in content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.is_empty() {
                continue;
            }

            match parts[0] {
                "io" if parts.len() >= 3 => {
                    stats.bytes_read = parts[1].parse().unwrap_or(0);
                    stats.bytes_written = parts[2].parse().unwrap_or(0);
                }
                "th" if parts.len() >= 2 => {
                    stats.threads = parts[1].parse().unwrap_or(0);
                }
                "net" if parts.len() >= 5 => {
                    stats.tcp_connections = parts[2].parse().unwrap_or(0);
                    stats.udp_connections = parts[3].parse().unwrap_or(0);
                }
                _ => {}
            }
        }

        Ok(stats)
    }
}

impl Default for NfsServerManager {
    fn default() -> Self {
        Self::new()
    }
}

impl NfsServerManager {
    /// Write complete exports file
    pub async fn write_full_exports(&self, shares: &[NasShare]) -> Result<()> {
        let exports = self.generate_exports(shares);

        // Backup existing exports
        if tokio::fs::metadata(&self.exports_path).await.is_ok() {
            let backup_path = format!("{}.bak", self.exports_path);
            let _ = tokio::fs::copy(&self.exports_path, &backup_path).await;
        }

        self.write_exports(&exports).await?;
        self.refresh_exports().await
    }

    /// Parse existing exports file
    pub async fn parse_exports(&self) -> Result<Vec<ParsedExport>> {
        let content = self.read_exports().await?;
        let mut exports = Vec::new();

        for line in content.lines() {
            let line = line.trim();
            // Skip comments and empty lines
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            // Parse export line: /path client1(opts) client2(opts)
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.is_empty() {
                continue;
            }

            let path = parts[0].to_string();
            let mut clients = Vec::new();

            for client_spec in &parts[1..] {
                if let Some((host, opts)) = client_spec.split_once('(') {
                    let options = opts.trim_end_matches(')');
                    clients.push(ParsedClient {
                        host: host.to_string(),
                        options: options.split(',').map(|s| s.to_string()).collect(),
                    });
                } else {
                    clients.push(ParsedClient {
                        host: client_spec.to_string(),
                        options: Vec::new(),
                    });
                }
            }

            exports.push(ParsedExport { path, clients });
        }

        Ok(exports)
    }

    /// Get service status
    pub async fn get_status(&self) -> Result<NfsServiceStatus> {
        // Check if nfsd is running
        let nfsd_running = Self::check_process("nfsd").await ||
                          Self::check_nfs_threads().await;
        let mountd_running = Self::check_process("rpc.mountd").await;
        let statd_running = Self::check_process("rpc.statd").await;
        let idmapd_running = Self::check_process("rpc.idmapd").await;

        // Get version
        let version = Self::get_nfs_version().await.unwrap_or_else(|| "unknown".to_string());

        // Count clients
        let clients = self.get_clients().await.unwrap_or_default();
        let exports = self.list_exports().await.unwrap_or_default();

        Ok(NfsServiceStatus {
            nfsd_running,
            mountd_running,
            statd_running,
            idmapd_running,
            version,
            active_clients: clients.len() as u32,
            active_exports: exports.len() as u32,
            threads: self.global_config.threads,
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

    async fn check_nfs_threads() -> bool {
        tokio::fs::metadata("/proc/fs/nfsd/threads")
            .await
            .map(|m| m.is_file())
            .unwrap_or(false)
    }

    async fn get_nfs_version() -> Option<String> {
        let output = Command::new("rpcinfo")
            .args(["-p"])
            .output()
            .await
            .ok()?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // Look for nfs version
            for line in stdout.lines() {
                if line.contains("nfs") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2 {
                        return Some(format!("NFSv{}", parts[1]));
                    }
                }
            }
        }
        None
    }

    /// Start NFS services
    pub async fn start(&self) -> Result<()> {
        // Write NFS config
        self.write_nfs_config().await?;

        // Try systemd first
        let systemd_result = Command::new("systemctl")
            .args(["start", "nfs-server"])
            .output()
            .await;

        if let Ok(output) = systemd_result {
            if output.status.success() {
                return Ok(());
            }
        }

        // Try OpenRC
        let openrc_result = Command::new("rc-service")
            .args(["nfs", "start"])
            .output()
            .await;

        if let Ok(output) = openrc_result {
            if output.status.success() {
                return Ok(());
            }
        }

        // Direct start
        let _ = Command::new("rpc.statd").output().await;
        let _ = Command::new("rpc.mountd").output().await;
        let _ = Command::new("rpc.nfsd")
            .arg(self.global_config.threads.to_string())
            .output()
            .await;

        Ok(())
    }

    /// Stop NFS services
    pub async fn stop(&self) -> Result<()> {
        // Try systemd first
        let _ = Command::new("systemctl")
            .args(["stop", "nfs-server"])
            .output()
            .await;

        // Try OpenRC
        let _ = Command::new("rc-service")
            .args(["nfs", "stop"])
            .output()
            .await;

        // Direct stop
        let _ = Command::new("rpc.nfsd").arg("0").output().await;
        let _ = Command::new("pkill").arg("rpc.mountd").output().await;

        Ok(())
    }

    /// Restart NFS services
    pub async fn restart(&self) -> Result<()> {
        self.stop().await?;
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        self.start().await
    }

    /// Write NFS server configuration
    async fn write_nfs_config(&self) -> Result<()> {
        let g = &self.global_config;

        // Write /etc/nfs.conf (for newer nfs-utils)
        let nfs_conf = format!(r#"# Horcrux NAS NFS configuration
[nfsd]
threads = {}
vers3 = {}
vers4 = {}
vers4.1 = {}
vers4.2 = {}
tcp = {}
udp = {}

[mountd]
port = {}

[statd]
port = {}

[lockd]
port = {}
"#,
            g.threads,
            if g.nfsv3 { "y" } else { "n" },
            if g.nfsv4 { "y" } else { "n" },
            if g.nfsv41 { "y" } else { "n" },
            if g.nfsv42 { "y" } else { "n" },
            if g.tcp { "y" } else { "n" },
            if g.udp { "y" } else { "n" },
            g.mountd_port,
            g.statd_port,
            g.lockd_port,
        );

        let _ = tokio::fs::write("/etc/nfs.conf", &nfs_conf).await;

        // Write idmapd.conf if NFSv4 domain is set
        if let Some(ref domain) = g.nfsv4_domain {
            let idmapd_conf = format!(r#"[General]
Domain = {}

[Mapping]
Nobody-User = nobody
Nobody-Group = nogroup

[Translation]
Method = nsswitch
"#, domain);
            let _ = tokio::fs::write("/etc/idmapd.conf", &idmapd_conf).await;
        }

        Ok(())
    }

    /// Configure NFSv4 domain
    pub async fn set_nfsv4_domain(&mut self, domain: &str) -> Result<()> {
        self.global_config.nfsv4_domain = Some(domain.to_string());
        self.write_nfs_config().await?;

        // Restart idmapd
        let _ = Command::new("nfsidmap").args(["-c"]).output().await;

        Ok(())
    }

    /// Unexport a specific path
    pub async fn unexport(&self, path: &str) -> Result<()> {
        let output = Command::new("exportfs")
            .args(["-u", path])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("exportfs failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to unexport: {}", stderr)));
        }

        Ok(())
    }

    /// Export a specific path to specific client
    pub async fn export_to_client(&self, path: &str, client: &str, options: &str) -> Result<()> {
        let spec = format!("{}:{}", client, path);
        let output = Command::new("exportfs")
            .args(["-o", options, &spec])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("exportfs failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to export: {}", stderr)));
        }

        Ok(())
    }

    /// Get NFS operation statistics
    pub async fn get_operation_stats(&self) -> Result<NfsOperationStats> {
        let content = tokio::fs::read_to_string("/proc/net/rpc/nfsd")
            .await
            .unwrap_or_default();

        let mut stats = NfsOperationStats::default();

        for line in content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.is_empty() {
                continue;
            }

            match parts[0] {
                "proc3" if parts.len() >= 23 => {
                    // NFSv3 operations
                    stats.v3_null = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                    stats.v3_getattr = parts.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
                    stats.v3_setattr = parts.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
                    stats.v3_lookup = parts.get(5).and_then(|s| s.parse().ok()).unwrap_or(0);
                    stats.v3_access = parts.get(6).and_then(|s| s.parse().ok()).unwrap_or(0);
                    stats.v3_readlink = parts.get(7).and_then(|s| s.parse().ok()).unwrap_or(0);
                    stats.v3_read = parts.get(8).and_then(|s| s.parse().ok()).unwrap_or(0);
                    stats.v3_write = parts.get(9).and_then(|s| s.parse().ok()).unwrap_or(0);
                    stats.v3_create = parts.get(10).and_then(|s| s.parse().ok()).unwrap_or(0);
                    stats.v3_mkdir = parts.get(11).and_then(|s| s.parse().ok()).unwrap_or(0);
                    stats.v3_remove = parts.get(13).and_then(|s| s.parse().ok()).unwrap_or(0);
                    stats.v3_rmdir = parts.get(14).and_then(|s| s.parse().ok()).unwrap_or(0);
                    stats.v3_rename = parts.get(15).and_then(|s| s.parse().ok()).unwrap_or(0);
                }
                "proc4ops" if parts.len() >= 40 => {
                    // NFSv4 operations
                    stats.v4_access = parts.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
                    stats.v4_close = parts.get(5).and_then(|s| s.parse().ok()).unwrap_or(0);
                    stats.v4_commit = parts.get(6).and_then(|s| s.parse().ok()).unwrap_or(0);
                    stats.v4_create = parts.get(7).and_then(|s| s.parse().ok()).unwrap_or(0);
                    stats.v4_getattr = parts.get(10).and_then(|s| s.parse().ok()).unwrap_or(0);
                    stats.v4_lookup = parts.get(16).and_then(|s| s.parse().ok()).unwrap_or(0);
                    stats.v4_open = parts.get(19).and_then(|s| s.parse().ok()).unwrap_or(0);
                    stats.v4_read = parts.get(26).and_then(|s| s.parse().ok()).unwrap_or(0);
                    stats.v4_remove = parts.get(28).and_then(|s| s.parse().ok()).unwrap_or(0);
                    stats.v4_rename = parts.get(29).and_then(|s| s.parse().ok()).unwrap_or(0);
                    stats.v4_setattr = parts.get(35).and_then(|s| s.parse().ok()).unwrap_or(0);
                    stats.v4_write = parts.get(39).and_then(|s| s.parse().ok()).unwrap_or(0);
                }
                _ => {}
            }
        }

        Ok(stats)
    }

    /// Flush NFS caches
    pub async fn flush_caches(&self) -> Result<()> {
        // Flush idmapd cache
        let _ = Command::new("nfsidmap").args(["-c"]).output().await;

        // Clear auth cache
        let _ = tokio::fs::write("/proc/net/rpc/auth.unix.ip/flush", "1").await;

        Ok(())
    }

    /// Set number of NFS threads
    pub async fn set_threads(&mut self, threads: u32) -> Result<()> {
        self.global_config.threads = threads;

        // Write to /proc if available
        let _ = tokio::fs::write("/proc/fs/nfsd/threads", threads.to_string()).await;

        Ok(())
    }

    /// Enable/disable NFS versions
    pub fn set_nfs_versions(&mut self, v3: bool, v4: bool, v41: bool, v42: bool) {
        self.global_config.nfsv3 = v3;
        self.global_config.nfsv4 = v4;
        self.global_config.nfsv41 = v41;
        self.global_config.nfsv42 = v42;
    }

    /// Get export by path
    pub async fn get_export(&self, path: &str) -> Result<Option<ParsedExport>> {
        let exports = self.parse_exports().await?;
        Ok(exports.into_iter().find(|e| e.path == path))
    }

    /// Update export clients
    pub async fn update_export_clients(&self, path: &str, clients: Vec<ParsedClient>) -> Result<()> {
        let mut exports = self.parse_exports().await?;

        // Find and update the export
        let mut found = false;
        for export in &mut exports {
            if export.path == path {
                export.clients = clients.clone();
                found = true;
                break;
            }
        }

        if !found {
            exports.push(ParsedExport {
                path: path.to_string(),
                clients,
            });
        }

        // Regenerate exports file
        let content = self.format_parsed_exports(&exports);
        self.write_exports(&content).await?;
        self.refresh_exports().await
    }

    fn format_parsed_exports(&self, exports: &[ParsedExport]) -> String {
        let mut content = String::new();
        content.push_str("# Horcrux NAS exports - auto-generated\n");
        content.push_str("# Do not edit manually\n\n");

        for export in exports {
            content.push_str(&export.path);
            for client in &export.clients {
                content.push(' ');
                content.push_str(&client.host);
                if !client.options.is_empty() {
                    content.push('(');
                    content.push_str(&client.options.join(","));
                    content.push(')');
                }
            }
            content.push('\n');
        }

        content
    }
}

/// Parsed export entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedExport {
    pub path: String,
    pub clients: Vec<ParsedClient>,
}

/// Parsed client entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedClient {
    pub host: String,
    pub options: Vec<String>,
}

/// NFS service status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NfsServiceStatus {
    pub nfsd_running: bool,
    pub mountd_running: bool,
    pub statd_running: bool,
    pub idmapd_running: bool,
    pub version: String,
    pub active_clients: u32,
    pub active_exports: u32,
    pub threads: u32,
}

/// NFS operation statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NfsOperationStats {
    // NFSv3 operations
    pub v3_null: u64,
    pub v3_getattr: u64,
    pub v3_setattr: u64,
    pub v3_lookup: u64,
    pub v3_access: u64,
    pub v3_readlink: u64,
    pub v3_read: u64,
    pub v3_write: u64,
    pub v3_create: u64,
    pub v3_mkdir: u64,
    pub v3_remove: u64,
    pub v3_rmdir: u64,
    pub v3_rename: u64,

    // NFSv4 operations
    pub v4_access: u64,
    pub v4_close: u64,
    pub v4_commit: u64,
    pub v4_create: u64,
    pub v4_getattr: u64,
    pub v4_lookup: u64,
    pub v4_open: u64,
    pub v4_read: u64,
    pub v4_remove: u64,
    pub v4_rename: u64,
    pub v4_setattr: u64,
    pub v4_write: u64,
}

/// NFS server statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NfsStats {
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub threads: u32,
    pub tcp_connections: u32,
    pub udp_connections: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_global_config() {
        let config = NfsGlobalConfig::default();
        assert_eq!(config.threads, 8);
        assert!(config.nfsv4);
        assert!(config.tcp);
    }

    #[test]
    fn test_format_client() {
        let manager = NfsServerManager::new();
        let client = NfsClient {
            host: "192.168.1.0/24".to_string(),
            access: AccessLevel::ReadWrite,
            secure: true,
        };
        let config = NfsExportConfig::default();

        let result = manager.format_client(&client, &config);
        assert!(result.contains("192.168.1.0/24"));
        assert!(result.contains("rw"));
        assert!(result.contains("sync") || result.contains("async"));
    }

    #[test]
    fn test_generate_export_line() {
        let manager = NfsServerManager::new();

        let mut share = NasShare::new(
            "test".to_string(),
            "Test Share".to_string(),
            "/mnt/test".to_string(),
        );
        share.nfs_config = Some(NfsExportConfig::default());

        let line = manager.generate_export_line(&share);
        assert!(line.starts_with("/mnt/test"));
    }
}
