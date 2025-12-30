//! iSCSI Target module
//!
//! Manages iSCSI targets for block-level storage access.
//! Supports both tgtd (SCSI target framework) and LIO (Linux-IO Target).

use horcrux_common::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::process::Command;

/// iSCSI backend implementation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IscsiBackend {
    /// tgtd - SCSI Target Framework
    Tgtd,
    /// LIO - Linux-IO Target (kernel-based)
    Lio,
}

impl Default for IscsiBackend {
    fn default() -> Self {
        // Prefer LIO if available (kernel-based, better performance)
        Self::Tgtd
    }
}

/// Global iSCSI configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IscsiGlobalConfig {
    /// Backend implementation
    pub backend: IscsiBackend,
    /// iSNS server address (for automatic discovery)
    pub isns_server: Option<String>,
    /// Discovery authentication enabled
    pub discovery_auth: bool,
    /// Discovery CHAP username
    pub discovery_username: Option<String>,
    /// Discovery CHAP password
    pub discovery_password: Option<String>,
    /// Default portal group tag
    pub default_portal_tag: u32,
    /// Enable header digest
    pub header_digest: bool,
    /// Enable data digest
    pub data_digest: bool,
    /// Max connections per session
    pub max_connections: u32,
    /// Max burst length (bytes)
    pub max_burst_length: u32,
    /// First burst length (bytes)
    pub first_burst_length: u32,
    /// Max receive data segment length (bytes)
    pub max_recv_data_segment_length: u32,
    /// Max outstanding R2T
    pub max_outstanding_r2t: u32,
    /// Immediate data enabled
    pub immediate_data: bool,
    /// Initial R2T disabled
    pub initial_r2t: bool,
    /// Default time to wait (seconds)
    pub default_time_to_wait: u32,
    /// Default time to retain (seconds)
    pub default_time_to_retain: u32,
}

impl Default for IscsiGlobalConfig {
    fn default() -> Self {
        Self {
            backend: IscsiBackend::default(),
            isns_server: None,
            discovery_auth: false,
            discovery_username: None,
            discovery_password: None,
            default_portal_tag: 1,
            header_digest: false,
            data_digest: false,
            max_connections: 8,
            max_burst_length: 16776192,
            first_burst_length: 262144,
            max_recv_data_segment_length: 262144,
            max_outstanding_r2t: 1,
            immediate_data: true,
            initial_r2t: true,
            default_time_to_wait: 2,
            default_time_to_retain: 20,
        }
    }
}

/// iSCSI Target configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IscsiTarget {
    /// Target IQN (iSCSI Qualified Name)
    pub iqn: String,
    /// Target alias (friendly name)
    pub alias: Option<String>,
    /// Enabled
    pub enabled: bool,
    /// LUNs attached to this target
    pub luns: Vec<IscsiLun>,
    /// Initiator ACLs
    pub acls: Vec<IscsiAcl>,
    /// CHAP authentication
    pub chap: Option<ChapAuth>,
    /// Mutual CHAP (bidirectional)
    pub mutual_chap: Option<ChapAuth>,
    /// Portal groups
    pub portals: Vec<IscsiPortal>,
    /// Target parameters
    pub params: Option<TargetParams>,
    /// Created at
    pub created_at: i64,
    /// Modified at
    pub modified_at: i64,
    /// Description
    pub description: Option<String>,
}

impl Default for IscsiTarget {
    fn default() -> Self {
        Self {
            iqn: String::new(),
            alias: None,
            enabled: true,
            luns: vec![],
            acls: vec![],
            chap: None,
            mutual_chap: None,
            portals: vec![IscsiPortal::default()],
            params: None,
            created_at: chrono::Utc::now().timestamp(),
            modified_at: chrono::Utc::now().timestamp(),
            description: None,
        }
    }
}

/// iSCSI Portal (network endpoint)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IscsiPortal {
    /// IP address to listen on
    pub ip: String,
    /// TCP port (default 3260)
    pub port: u16,
    /// Portal group tag
    pub tag: u32,
}

impl Default for IscsiPortal {
    fn default() -> Self {
        Self {
            ip: "0.0.0.0".to_string(),
            port: 3260,
            tag: 1,
        }
    }
}

/// Target-specific parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetParams {
    /// Maximum connections
    pub max_connections: Option<u32>,
    /// Immediate data
    pub immediate_data: Option<bool>,
    /// Initial R2T
    pub initial_r2t: Option<bool>,
    /// Max burst length
    pub max_burst_length: Option<u32>,
    /// First burst length
    pub first_burst_length: Option<u32>,
    /// Max outstanding R2T
    pub max_outstanding_r2t: Option<u32>,
    /// Max receive data segment
    pub max_recv_data_segment: Option<u32>,
    /// Data sequence in order
    pub data_pdu_in_order: Option<bool>,
    /// Data sequence in order
    pub data_sequence_in_order: Option<bool>,
    /// Error recovery level
    pub error_recovery_level: Option<u32>,
    /// Queue depth
    pub queue_depth: Option<u32>,
}

impl Default for TargetParams {
    fn default() -> Self {
        Self {
            max_connections: None,
            immediate_data: None,
            initial_r2t: None,
            max_burst_length: None,
            first_burst_length: None,
            max_outstanding_r2t: None,
            max_recv_data_segment: None,
            data_pdu_in_order: None,
            data_sequence_in_order: None,
            error_recovery_level: None,
            queue_depth: None,
        }
    }
}

/// iSCSI LUN
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IscsiLun {
    /// LUN ID
    pub lun_id: u32,
    /// Backing store path (zvol or file)
    pub path: String,
    /// LUN type
    pub lun_type: LunType,
    /// Size in bytes
    pub size_bytes: u64,
    /// Sector size (512 or 4096)
    pub sector_size: u32,
    /// Read-only
    pub read_only: bool,
    /// Write cache enabled
    pub write_cache: bool,
    /// Serial number
    pub serial: Option<String>,
    /// Vendor ID
    pub vendor_id: Option<String>,
    /// Product ID
    pub product_id: Option<String>,
    /// SCSI device type
    pub device_type: ScsiDeviceType,
    /// Thin provisioning (for file-backed)
    pub thin_provisioning: bool,
}

impl Default for IscsiLun {
    fn default() -> Self {
        Self {
            lun_id: 0,
            path: String::new(),
            lun_type: LunType::Block,
            size_bytes: 0,
            sector_size: 512,
            read_only: false,
            write_cache: true,
            serial: None,
            vendor_id: Some("HORCRUX".to_string()),
            product_id: Some("iSCSI LUN".to_string()),
            device_type: ScsiDeviceType::Disk,
            thin_provisioning: false,
        }
    }
}

/// LUN backing store type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LunType {
    /// Block device (zvol, LVM LV)
    Block,
    /// File-backed (sparse file)
    File,
    /// RAMDISK (for testing)
    Ramdisk,
    /// Pass-through to existing block device
    Passthrough,
}

/// SCSI device type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScsiDeviceType {
    /// Disk (TYPE_DISK)
    Disk,
    /// Tape (TYPE_TAPE)
    Tape,
    /// CD/DVD (TYPE_ROM)
    Cdrom,
    /// Pass-through
    Passthrough,
}

/// iSCSI initiator ACL
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IscsiAcl {
    /// Initiator IQN pattern
    pub initiator_iqn: String,
    /// Allowed
    pub allowed: bool,
    /// LUN mapping (optional, if not set all LUNs visible)
    pub lun_mapping: Option<HashMap<u32, u32>>,
    /// Initiator-specific CHAP
    pub chap: Option<ChapAuth>,
}

/// CHAP authentication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChapAuth {
    /// Username
    pub username: String,
    /// Password
    #[serde(skip_serializing)]
    pub password: String,
}

/// iSCSI session info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IscsiSession {
    /// Session ID
    pub sid: u32,
    /// Initiator IQN
    pub initiator: String,
    /// Target IQN
    pub target: String,
    /// Connection info
    pub connections: Vec<IscsiConnection>,
    /// Session state
    pub state: SessionState,
    /// TSIH (Target Session Identifying Handle)
    pub tsih: u32,
    /// Connected at
    pub connected_at: i64,
    /// Read bytes
    pub read_bytes: u64,
    /// Write bytes
    pub write_bytes: u64,
    /// Commands processed
    pub commands: u64,
}

/// iSCSI connection info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IscsiConnection {
    /// Connection ID
    pub cid: u32,
    /// Initiator IP address
    pub initiator_ip: String,
    /// Initiator port
    pub initiator_port: u16,
    /// Target IP address
    pub target_ip: String,
    /// Target port
    pub target_port: u16,
    /// Connection state
    pub state: ConnectionState,
    /// Header digest enabled
    pub header_digest: bool,
    /// Data digest enabled
    pub data_digest: bool,
}

/// Session state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    Active,
    Failed,
    Blocked,
    Unknown,
}

/// Connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionState {
    /// Normal operation
    Logged,
    /// Negotiating
    Login,
    /// Logging out
    Logout,
    /// Cleanup
    Cleanup,
    /// Unknown
    Unknown,
}

/// iSCSI target info (runtime status)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IscsiTargetInfo {
    /// Target ID
    pub tid: u32,
    /// Target IQN
    pub iqn: String,
    /// Alias
    pub alias: Option<String>,
    /// Enabled
    pub enabled: bool,
    /// LUNs attached
    pub luns: Vec<IscsiLunInfo>,
    /// Active sessions
    pub sessions: u32,
    /// Total read bytes
    pub read_bytes: u64,
    /// Total write bytes
    pub write_bytes: u64,
}

/// iSCSI LUN info (runtime status)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IscsiLunInfo {
    /// LUN ID
    pub lun_id: u32,
    /// Backing store path
    pub path: String,
    /// Size in bytes
    pub size_bytes: u64,
    /// Type
    pub lun_type: String,
    /// Online
    pub online: bool,
    /// Read bytes
    pub read_bytes: u64,
    /// Write bytes
    pub write_bytes: u64,
}

/// iSCSI service status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IscsiStatus {
    /// Service running
    pub running: bool,
    /// Backend type
    pub backend: IscsiBackend,
    /// Version
    pub version: Option<String>,
    /// Number of targets
    pub target_count: u32,
    /// Number of active sessions
    pub session_count: u32,
    /// Total LUNs across all targets
    pub total_luns: u32,
    /// Total storage size
    pub total_size_bytes: u64,
    /// iSNS registered
    pub isns_registered: bool,
    /// Listening portals
    pub portals: Vec<IscsiPortal>,
}

/// Create LUN options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateLunOptions {
    /// Size in bytes
    pub size_bytes: u64,
    /// LUN type
    pub lun_type: LunType,
    /// For ZFS zvol: pool name
    pub zfs_pool: Option<String>,
    /// For file-backed: directory
    pub file_dir: Option<String>,
    /// Sparse (thin provisioned)
    pub sparse: bool,
    /// Sector size
    pub sector_size: u32,
    /// Block size (for zvol)
    pub block_size: Option<u32>,
    /// Compression (for zvol)
    pub compression: Option<String>,
}

impl Default for CreateLunOptions {
    fn default() -> Self {
        Self {
            size_bytes: 10 * 1024 * 1024 * 1024, // 10GB
            lun_type: LunType::Block,
            zfs_pool: None,
            file_dir: Some("/var/lib/iscsi".to_string()),
            sparse: true,
            sector_size: 512,
            block_size: None,
            compression: None,
        }
    }
}

/// iSCSI Target Manager
pub struct IscsiTargetManager {
    config: IscsiGlobalConfig,
    config_path: String,
}

impl IscsiTargetManager {
    /// Create a new iSCSI target manager
    pub fn new() -> Self {
        Self {
            config: IscsiGlobalConfig::default(),
            config_path: "/etc/tgt/targets.conf".to_string(),
        }
    }

    /// Create with specific configuration
    pub fn with_config(config: IscsiGlobalConfig) -> Self {
        let config_path = match config.backend {
            IscsiBackend::Tgtd => "/etc/tgt/targets.conf".to_string(),
            IscsiBackend::Lio => "/etc/target/saveconfig.json".to_string(),
        };
        Self { config, config_path }
    }

    /// Set global configuration
    pub fn set_config(&mut self, config: IscsiGlobalConfig) {
        self.config = config;
        self.config_path = match self.config.backend {
            IscsiBackend::Tgtd => "/etc/tgt/targets.conf".to_string(),
            IscsiBackend::Lio => "/etc/target/saveconfig.json".to_string(),
        };
    }

    /// Generate IQN for a target
    pub fn generate_iqn(name: &str) -> String {
        let now = chrono::Utc::now();
        format!(
            "iqn.{}.com.horcrux:{}",
            now.format("%Y-%m"),
            name.to_lowercase().replace(' ', "-").replace('_', "-")
        )
    }

    /// Validate IQN format
    pub fn validate_iqn(iqn: &str) -> Result<()> {
        if !iqn.starts_with("iqn.") && !iqn.starts_with("eui.") && !iqn.starts_with("naa.") {
            return Err(Error::Validation("IQN must start with iqn., eui., or naa.".to_string()));
        }
        if iqn.len() > 223 {
            return Err(Error::Validation("IQN exceeds maximum length of 223 characters".to_string()));
        }
        Ok(())
    }

    /// Detect available backend
    pub async fn detect_backend() -> IscsiBackend {
        // Check for LIO (targetcli)
        let lio = Command::new("which")
            .arg("targetcli")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);

        if lio {
            return IscsiBackend::Lio;
        }

        // Check for tgtd
        let tgtd = Command::new("which")
            .arg("tgtadm")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);

        if tgtd {
            return IscsiBackend::Tgtd;
        }

        // Default to tgtd
        IscsiBackend::Tgtd
    }

    // ==================== Target Operations ====================

    /// Create a target
    pub async fn create_target(&self, target: &IscsiTarget) -> Result<u32> {
        Self::validate_iqn(&target.iqn)?;

        match self.config.backend {
            IscsiBackend::Tgtd => self.create_target_tgtd(target).await,
            IscsiBackend::Lio => self.create_target_lio(target).await,
        }
    }

    /// Create target using tgtd
    async fn create_target_tgtd(&self, target: &IscsiTarget) -> Result<u32> {
        // Find next available TID
        let targets = self.list_targets().await?;
        let tid = targets.iter().map(|t| t.tid).max().unwrap_or(0) + 1;

        // Create target
        let output = Command::new("tgtadm")
            .args([
                "--lld", "iscsi",
                "--mode", "target",
                "--op", "new",
                "--tid", &tid.to_string(),
                "--targetname", &target.iqn,
            ])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("tgtadm failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to create target: {}", stderr)));
        }

        // Add LUNs
        for lun in &target.luns {
            self.add_lun_tgtd(tid, lun).await?;
        }

        // Set ACLs
        if target.acls.is_empty() {
            // Allow all initiators by default
            self.set_acl_tgtd(tid, "ALL").await?;
        } else {
            for acl in &target.acls {
                if acl.allowed {
                    self.set_acl_tgtd(tid, &acl.initiator_iqn).await?;
                }
            }
        }

        // Set CHAP if configured
        if let Some(ref chap) = target.chap {
            self.set_chap_tgtd(tid, chap, false).await?;
        }

        // Persist configuration
        self.save_tgtd_config().await?;

        Ok(tid)
    }

    /// Create target using LIO (targetcli)
    async fn create_target_lio(&self, target: &IscsiTarget) -> Result<u32> {
        // Create target
        let output = Command::new("targetcli")
            .args([
                &format!("/iscsi create {}", target.iqn),
            ])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("targetcli failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to create target: {}", stderr)));
        }

        // Create TPG (Target Portal Group)
        let _ = Command::new("targetcli")
            .args([&format!("/iscsi/{}/tpg1 set attribute authentication=0 generate_node_acls=1 demo_mode_write_protect=0", target.iqn)])
            .output()
            .await;

        // Add LUNs
        for lun in &target.luns {
            self.add_lun_lio(&target.iqn, lun).await?;
        }

        // Add ACLs
        for acl in &target.acls {
            if acl.allowed {
                self.set_acl_lio(&target.iqn, &acl.initiator_iqn).await?;
            }
        }

        // Save configuration
        self.save_lio_config().await?;

        Ok(1) // LIO doesn't use TIDs the same way
    }

    /// Delete a target
    pub async fn delete_target(&self, iqn: &str) -> Result<()> {
        match self.config.backend {
            IscsiBackend::Tgtd => {
                let tid = self.get_tid_for_iqn(iqn).await?;

                // Force close all sessions
                let _ = Command::new("tgtadm")
                    .args([
                        "--lld", "iscsi",
                        "--mode", "target",
                        "--op", "delete",
                        "--force",
                        "--tid", &tid.to_string(),
                    ])
                    .output()
                    .await;

                self.save_tgtd_config().await
            }
            IscsiBackend::Lio => {
                let output = Command::new("targetcli")
                    .args([&format!("/iscsi delete {}", iqn)])
                    .output()
                    .await
                    .map_err(|e| Error::Internal(format!("targetcli failed: {}", e)))?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(Error::Internal(format!("Failed to delete target: {}", stderr)));
                }

                self.save_lio_config().await
            }
        }
    }

    /// Enable/disable a target
    pub async fn set_target_enabled(&self, iqn: &str, enabled: bool) -> Result<()> {
        match self.config.backend {
            IscsiBackend::Tgtd => {
                let tid = self.get_tid_for_iqn(iqn).await?;
                let state = if enabled { "ready" } else { "offline" };

                let output = Command::new("tgtadm")
                    .args([
                        "--lld", "iscsi",
                        "--mode", "target",
                        "--op", "update",
                        "--tid", &tid.to_string(),
                        "--name", "state",
                        "--value", state,
                    ])
                    .output()
                    .await
                    .map_err(|e| Error::Internal(format!("tgtadm failed: {}", e)))?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(Error::Internal(format!("Failed to update target state: {}", stderr)));
                }
                Ok(())
            }
            IscsiBackend::Lio => {
                let enable_str = if enabled { "1" } else { "0" };
                let output = Command::new("targetcli")
                    .args([&format!("/iscsi/{}/tpg1 set attribute demo_mode_write_protect={}", iqn, if enabled { "0" } else { "1" })])
                    .output()
                    .await
                    .map_err(|e| Error::Internal(format!("targetcli failed: {}", e)))?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(Error::Internal(format!("Failed to update target: {}", stderr)));
                }
                Ok(())
            }
        }
    }

    /// List all targets
    pub async fn list_targets(&self) -> Result<Vec<IscsiTargetInfo>> {
        match self.config.backend {
            IscsiBackend::Tgtd => self.list_targets_tgtd().await,
            IscsiBackend::Lio => self.list_targets_lio().await,
        }
    }

    /// List targets using tgtd
    async fn list_targets_tgtd(&self) -> Result<Vec<IscsiTargetInfo>> {
        let output = Command::new("tgtadm")
            .args(["--lld", "iscsi", "--mode", "target", "--op", "show"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("tgtadm failed: {}", e)))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(self.parse_tgtd_targets(&stdout))
    }

    /// List targets using LIO
    async fn list_targets_lio(&self) -> Result<Vec<IscsiTargetInfo>> {
        let output = Command::new("targetcli")
            .args(["ls", "/iscsi", "-d", "1"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("targetcli failed: {}", e)))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(self.parse_lio_targets(&stdout))
    }

    /// Parse tgtd target output
    fn parse_tgtd_targets(&self, output: &str) -> Vec<IscsiTargetInfo> {
        let mut targets = Vec::new();
        let mut current_target: Option<IscsiTargetInfo> = None;
        let mut current_lun: Option<IscsiLunInfo> = None;

        for line in output.lines() {
            let line = line.trim();

            // Parse target header
            if line.starts_with("Target ") && line.contains(':') {
                // Save previous target
                if let Some(lun) = current_lun.take() {
                    if let Some(ref mut target) = current_target {
                        target.luns.push(lun);
                    }
                }
                if let Some(target) = current_target.take() {
                    targets.push(target);
                }

                let parts: Vec<&str> = line.splitn(2, ':').collect();
                if parts.len() == 2 {
                    let tid = parts[0]
                        .strip_prefix("Target ")
                        .and_then(|s| s.trim().parse::<u32>().ok())
                        .unwrap_or(0);
                    let iqn = parts[1].trim().to_string();

                    current_target = Some(IscsiTargetInfo {
                        tid,
                        iqn,
                        alias: None,
                        enabled: true,
                        luns: Vec::new(),
                        sessions: 0,
                        read_bytes: 0,
                        write_bytes: 0,
                    });
                }
            }

            // Parse LUN
            if line.starts_with("LUN:") {
                if let Some(lun) = current_lun.take() {
                    if let Some(ref mut target) = current_target {
                        target.luns.push(lun);
                    }
                }

                if let Some(lun_str) = line.strip_prefix("LUN:") {
                    if let Ok(lun_id) = lun_str.trim().parse::<u32>() {
                        current_lun = Some(IscsiLunInfo {
                            lun_id,
                            path: String::new(),
                            size_bytes: 0,
                            lun_type: "block".to_string(),
                            online: true,
                            read_bytes: 0,
                            write_bytes: 0,
                        });
                    }
                }
            }

            // Parse backing store
            if line.starts_with("Backing store path:") {
                if let Some(ref mut lun) = current_lun {
                    lun.path = line
                        .strip_prefix("Backing store path:")
                        .map(|s| s.trim().to_string())
                        .unwrap_or_default();
                }
            }

            // Parse size
            if line.starts_with("Size:") {
                if let Some(ref mut lun) = current_lun {
                    if let Some(size_str) = line.strip_prefix("Size:") {
                        let size_part = size_str.split(',').next().unwrap_or("0");
                        lun.size_bytes = size_part.trim().parse().unwrap_or(0);
                    }
                }
            }

            // Parse backing store type
            if line.starts_with("Backing store type:") {
                if let Some(ref mut lun) = current_lun {
                    lun.lun_type = line
                        .strip_prefix("Backing store type:")
                        .map(|s| s.trim().to_string())
                        .unwrap_or_else(|| "block".to_string());
                }
            }

            // Count sessions
            if line.starts_with("I_T nexus:") {
                if let Some(ref mut target) = current_target {
                    target.sessions += 1;
                }
            }
        }

        // Don't forget last items
        if let Some(lun) = current_lun {
            if let Some(ref mut target) = current_target {
                target.luns.push(lun);
            }
        }
        if let Some(target) = current_target {
            targets.push(target);
        }

        targets
    }

    /// Parse LIO target output
    fn parse_lio_targets(&self, output: &str) -> Vec<IscsiTargetInfo> {
        let mut targets = Vec::new();
        let mut tid = 0u32;

        for line in output.lines() {
            let line = line.trim();
            if line.contains("iqn.") {
                // Extract IQN from targetcli output
                let parts: Vec<&str> = line.split_whitespace().collect();
                for part in parts {
                    if part.starts_with("iqn.") || part.contains(":iqn.") {
                        let iqn = if part.contains(':') {
                            part.split(':').find(|s| s.starts_with("iqn.")).unwrap_or(part)
                        } else {
                            part
                        };
                        tid += 1;
                        targets.push(IscsiTargetInfo {
                            tid,
                            iqn: iqn.trim_end_matches(']').trim_end_matches('/').to_string(),
                            alias: None,
                            enabled: true,
                            luns: Vec::new(),
                            sessions: 0,
                            read_bytes: 0,
                            write_bytes: 0,
                        });
                        break;
                    }
                }
            }
        }

        targets
    }

    /// Get target by IQN
    pub async fn get_target(&self, iqn: &str) -> Result<IscsiTargetInfo> {
        let targets = self.list_targets().await?;
        targets
            .into_iter()
            .find(|t| t.iqn == iqn)
            .ok_or_else(|| Error::NotFound(format!("Target '{}' not found", iqn)))
    }

    /// Get TID for IQN (tgtd only)
    async fn get_tid_for_iqn(&self, iqn: &str) -> Result<u32> {
        let targets = self.list_targets().await?;
        targets
            .iter()
            .find(|t| t.iqn == iqn)
            .map(|t| t.tid)
            .ok_or_else(|| Error::NotFound(format!("Target '{}' not found", iqn)))
    }

    // ==================== LUN Operations ====================

    /// Add LUN to target
    pub async fn add_lun(&self, iqn: &str, lun: &IscsiLun) -> Result<()> {
        match self.config.backend {
            IscsiBackend::Tgtd => {
                let tid = self.get_tid_for_iqn(iqn).await?;
                self.add_lun_tgtd(tid, lun).await?;
                self.save_tgtd_config().await
            }
            IscsiBackend::Lio => {
                self.add_lun_lio(iqn, lun).await?;
                self.save_lio_config().await
            }
        }
    }

    /// Add LUN using tgtd
    async fn add_lun_tgtd(&self, tid: u32, lun: &IscsiLun) -> Result<()> {
        let mut args = vec![
            "--lld".to_string(), "iscsi".to_string(),
            "--mode".to_string(), "logicalunit".to_string(),
            "--op".to_string(), "new".to_string(),
            "--tid".to_string(), tid.to_string(),
            "--lun".to_string(), lun.lun_id.to_string(),
            "--backing-store".to_string(), lun.path.clone(),
        ];

        // Add type-specific options
        match lun.lun_type {
            LunType::File => {
                args.push("--bstype".to_string());
                args.push("aio".to_string());
            }
            LunType::Block => {
                args.push("--bstype".to_string());
                args.push("aio".to_string());
            }
            LunType::Ramdisk => {
                args.push("--bstype".to_string());
                args.push("rdwr".to_string());
            }
            _ => {}
        }

        let output = Command::new("tgtadm")
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("tgtadm failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to add LUN: {}", stderr)));
        }

        // Set additional parameters
        if lun.read_only {
            let _ = Command::new("tgtadm")
                .args([
                    "--lld", "iscsi",
                    "--mode", "logicalunit",
                    "--op", "update",
                    "--tid", &tid.to_string(),
                    "--lun", &lun.lun_id.to_string(),
                    "--params", "readonly=1",
                ])
                .output()
                .await;
        }

        Ok(())
    }

    /// Add LUN using LIO
    async fn add_lun_lio(&self, iqn: &str, lun: &IscsiLun) -> Result<()> {
        // First create the backstore
        let backstore_name = format!("disk_{}", lun.lun_id);
        let backstore_type = match lun.lun_type {
            LunType::Block => "block",
            LunType::File => "fileio",
            LunType::Ramdisk => "ramdisk",
            LunType::Passthrough => "pscsi",
        };

        // Create backstore
        let backstore_cmd = match lun.lun_type {
            LunType::Block | LunType::Passthrough => {
                format!("/backstores/{} create {} {}", backstore_type, backstore_name, lun.path)
            }
            LunType::File => {
                format!("/backstores/{} create {} {} {}", backstore_type, backstore_name, lun.path, lun.size_bytes)
            }
            LunType::Ramdisk => {
                format!("/backstores/{} create {} {}", backstore_type, backstore_name, lun.size_bytes)
            }
        };

        let output = Command::new("targetcli")
            .args([&backstore_cmd])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("targetcli failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Check if already exists
            if !stderr.contains("already exists") {
                return Err(Error::Internal(format!("Failed to create backstore: {}", stderr)));
            }
        }

        // Add LUN to TPG
        let lun_cmd = format!(
            "/iscsi/{}/tpg1/luns create /backstores/{}/{} {}",
            iqn, backstore_type, backstore_name, lun.lun_id
        );

        let output = Command::new("targetcli")
            .args([&lun_cmd])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("targetcli failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to add LUN: {}", stderr)));
        }

        Ok(())
    }

    /// Remove LUN from target
    pub async fn remove_lun(&self, iqn: &str, lun_id: u32) -> Result<()> {
        match self.config.backend {
            IscsiBackend::Tgtd => {
                let tid = self.get_tid_for_iqn(iqn).await?;

                let output = Command::new("tgtadm")
                    .args([
                        "--lld", "iscsi",
                        "--mode", "logicalunit",
                        "--op", "delete",
                        "--tid", &tid.to_string(),
                        "--lun", &lun_id.to_string(),
                    ])
                    .output()
                    .await
                    .map_err(|e| Error::Internal(format!("tgtadm failed: {}", e)))?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(Error::Internal(format!("Failed to remove LUN: {}", stderr)));
                }

                self.save_tgtd_config().await
            }
            IscsiBackend::Lio => {
                let output = Command::new("targetcli")
                    .args([&format!("/iscsi/{}/tpg1/luns delete lun{}", iqn, lun_id)])
                    .output()
                    .await
                    .map_err(|e| Error::Internal(format!("targetcli failed: {}", e)))?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(Error::Internal(format!("Failed to remove LUN: {}", stderr)));
                }

                self.save_lio_config().await
            }
        }
    }

    /// Create a new LUN backing store (zvol or file)
    pub async fn create_lun_backing(&self, name: &str, options: &CreateLunOptions) -> Result<String> {
        match options.lun_type {
            LunType::Block => {
                // Create ZFS zvol
                if let Some(ref pool) = options.zfs_pool {
                    self.create_zvol(pool, name, options).await
                } else {
                    Err(Error::Validation("ZFS pool required for block LUN".to_string()))
                }
            }
            LunType::File => {
                // Create file
                let dir = options.file_dir.as_deref().unwrap_or("/var/lib/iscsi");
                self.create_file_lun(dir, name, options).await
            }
            _ => Err(Error::Validation("Unsupported LUN type for creation".to_string())),
        }
    }

    /// Create ZFS zvol for LUN
    #[cfg(feature = "nas-zfs")]
    async fn create_zvol(&self, pool: &str, name: &str, options: &CreateLunOptions) -> Result<String> {
        let zvol_name = format!("{}/iscsi-{}", pool, name);
        let size = format!("{}B", options.size_bytes);

        let mut args = vec!["create".to_string()];

        // Sparse (thin provisioned)
        if options.sparse {
            args.push("-s".to_string());
        }

        // Block size
        if let Some(bs) = options.block_size {
            args.push("-b".to_string());
            args.push(bs.to_string());
        }

        // Compression
        if let Some(ref comp) = options.compression {
            args.push("-o".to_string());
            args.push(format!("compression={}", comp));
        }

        // Volume size
        args.push("-V".to_string());
        args.push(size);

        // Volume name
        args.push(zvol_name.clone());

        let output = Command::new("zfs")
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("zfs create failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to create zvol: {}", stderr)));
        }

        Ok(format!("/dev/zvol/{}", zvol_name))
    }

    #[cfg(not(feature = "nas-zfs"))]
    async fn create_zvol(&self, _pool: &str, _name: &str, _options: &CreateLunOptions) -> Result<String> {
        Err(Error::Internal("ZFS support not enabled".to_string()))
    }

    /// Create file-backed LUN
    async fn create_file_lun(&self, dir: &str, name: &str, options: &CreateLunOptions) -> Result<String> {
        // Ensure directory exists
        tokio::fs::create_dir_all(dir).await.map_err(|e| {
            Error::Internal(format!("Failed to create directory: {}", e))
        })?;

        let path = format!("{}/{}.img", dir, name);

        if options.sparse {
            // Create sparse file with truncate
            let output = Command::new("truncate")
                .args(["-s", &options.size_bytes.to_string(), &path])
                .output()
                .await
                .map_err(|e| Error::Internal(format!("truncate failed: {}", e)))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(Error::Internal(format!("Failed to create file: {}", stderr)));
            }
        } else {
            // Create pre-allocated file with fallocate
            let output = Command::new("fallocate")
                .args(["-l", &options.size_bytes.to_string(), &path])
                .output()
                .await
                .map_err(|e| Error::Internal(format!("fallocate failed: {}", e)))?;

            if !output.status.success() {
                // Fallback to dd
                let blocks = options.size_bytes / 1048576; // 1MB blocks
                let output = Command::new("dd")
                    .args([
                        "if=/dev/zero",
                        &format!("of={}", path),
                        "bs=1M",
                        &format!("count={}", blocks),
                    ])
                    .output()
                    .await
                    .map_err(|e| Error::Internal(format!("dd failed: {}", e)))?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(Error::Internal(format!("Failed to create file: {}", stderr)));
                }
            }
        }

        Ok(path)
    }

    /// Delete LUN backing store
    pub async fn delete_lun_backing(&self, path: &str) -> Result<()> {
        if path.starts_with("/dev/zvol/") {
            // ZFS zvol
            let zvol = path.strip_prefix("/dev/zvol/").unwrap();
            let output = Command::new("zfs")
                .args(["destroy", zvol])
                .output()
                .await
                .map_err(|e| Error::Internal(format!("zfs destroy failed: {}", e)))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(Error::Internal(format!("Failed to delete zvol: {}", stderr)));
            }
        } else if std::path::Path::new(path).exists() {
            // File
            tokio::fs::remove_file(path).await.map_err(|e| {
                Error::Internal(format!("Failed to delete file: {}", e))
            })?;
        }

        Ok(())
    }

    // ==================== ACL Operations ====================

    /// Set initiator ACL
    pub async fn set_acl(&self, iqn: &str, initiator_iqn: &str) -> Result<()> {
        match self.config.backend {
            IscsiBackend::Tgtd => {
                let tid = self.get_tid_for_iqn(iqn).await?;
                self.set_acl_tgtd(tid, initiator_iqn).await?;
                self.save_tgtd_config().await
            }
            IscsiBackend::Lio => {
                self.set_acl_lio(iqn, initiator_iqn).await?;
                self.save_lio_config().await
            }
        }
    }

    /// Set ACL using tgtd
    async fn set_acl_tgtd(&self, tid: u32, initiator: &str) -> Result<()> {
        let output = Command::new("tgtadm")
            .args([
                "--lld", "iscsi",
                "--mode", "target",
                "--op", "bind",
                "--tid", &tid.to_string(),
                "--initiator-address", initiator,
            ])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("tgtadm failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to set ACL: {}", stderr)));
        }

        Ok(())
    }

    /// Set ACL using LIO
    async fn set_acl_lio(&self, iqn: &str, initiator_iqn: &str) -> Result<()> {
        let output = Command::new("targetcli")
            .args([&format!("/iscsi/{}/tpg1/acls create {}", iqn, initiator_iqn)])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("targetcli failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to set ACL: {}", stderr)));
        }

        Ok(())
    }

    /// Remove ACL
    pub async fn remove_acl(&self, iqn: &str, initiator_iqn: &str) -> Result<()> {
        match self.config.backend {
            IscsiBackend::Tgtd => {
                let tid = self.get_tid_for_iqn(iqn).await?;

                let output = Command::new("tgtadm")
                    .args([
                        "--lld", "iscsi",
                        "--mode", "target",
                        "--op", "unbind",
                        "--tid", &tid.to_string(),
                        "--initiator-address", initiator_iqn,
                    ])
                    .output()
                    .await
                    .map_err(|e| Error::Internal(format!("tgtadm failed: {}", e)))?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(Error::Internal(format!("Failed to remove ACL: {}", stderr)));
                }

                self.save_tgtd_config().await
            }
            IscsiBackend::Lio => {
                let output = Command::new("targetcli")
                    .args([&format!("/iscsi/{}/tpg1/acls delete {}", iqn, initiator_iqn)])
                    .output()
                    .await
                    .map_err(|e| Error::Internal(format!("targetcli failed: {}", e)))?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(Error::Internal(format!("Failed to remove ACL: {}", stderr)));
                }

                self.save_lio_config().await
            }
        }
    }

    // ==================== CHAP Operations ====================

    /// Set CHAP authentication
    pub async fn set_chap(&self, iqn: &str, chap: &ChapAuth, is_outgoing: bool) -> Result<()> {
        match self.config.backend {
            IscsiBackend::Tgtd => {
                let tid = self.get_tid_for_iqn(iqn).await?;
                self.set_chap_tgtd(tid, chap, is_outgoing).await?;
                self.save_tgtd_config().await
            }
            IscsiBackend::Lio => {
                self.set_chap_lio(iqn, chap, is_outgoing).await?;
                self.save_lio_config().await
            }
        }
    }

    /// Set CHAP using tgtd
    async fn set_chap_tgtd(&self, tid: u32, chap: &ChapAuth, is_outgoing: bool) -> Result<()> {
        // Create account
        let output = Command::new("tgtadm")
            .args([
                "--lld", "iscsi",
                "--mode", "account",
                "--op", "new",
                "--user", &chap.username,
                "--password", &chap.password,
            ])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("tgtadm failed: {}", e)))?;

        // Ignore "already exists" error
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("already exists") {
                return Err(Error::Internal(format!("Failed to create account: {}", stderr)));
            }
        }

        // Bind to target
        let mut args = vec![
            "--lld".to_string(), "iscsi".to_string(),
            "--mode".to_string(), "account".to_string(),
            "--op".to_string(), "bind".to_string(),
            "--tid".to_string(), tid.to_string(),
            "--user".to_string(), chap.username.clone(),
        ];

        if is_outgoing {
            args.push("--outgoing".to_string());
        }

        let output = Command::new("tgtadm")
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("tgtadm failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to bind account: {}", stderr)));
        }

        Ok(())
    }

    /// Set CHAP using LIO
    async fn set_chap_lio(&self, iqn: &str, chap: &ChapAuth, is_outgoing: bool) -> Result<()> {
        let attr = if is_outgoing {
            format!("set auth userid={} password={}", chap.username, chap.password)
        } else {
            format!("set auth userid={} password={}", chap.username, chap.password)
        };

        let output = Command::new("targetcli")
            .args([&format!("/iscsi/{}/tpg1 {}", iqn, attr)])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("targetcli failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to set CHAP: {}", stderr)));
        }

        // Enable authentication
        let _ = Command::new("targetcli")
            .args([&format!("/iscsi/{}/tpg1 set attribute authentication=1", iqn)])
            .output()
            .await;

        Ok(())
    }

    // ==================== Session Operations ====================

    /// List active sessions
    pub async fn list_sessions(&self) -> Result<Vec<IscsiSession>> {
        match self.config.backend {
            IscsiBackend::Tgtd => self.list_sessions_tgtd().await,
            IscsiBackend::Lio => self.list_sessions_lio().await,
        }
    }

    /// List sessions using tgtd
    async fn list_sessions_tgtd(&self) -> Result<Vec<IscsiSession>> {
        let output = Command::new("tgtadm")
            .args(["--lld", "iscsi", "--mode", "target", "--op", "show"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("tgtadm failed: {}", e)))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(self.parse_tgtd_sessions(&stdout))
    }

    /// Parse tgtd session output
    fn parse_tgtd_sessions(&self, output: &str) -> Vec<IscsiSession> {
        let mut sessions = Vec::new();
        let mut current_target: Option<String> = None;
        let mut current_session: Option<IscsiSession> = None;

        for line in output.lines() {
            let line = line.trim();

            // Track current target
            if line.starts_with("Target ") && line.contains(':') {
                if let Some(session) = current_session.take() {
                    sessions.push(session);
                }
                let parts: Vec<&str> = line.splitn(2, ':').collect();
                if parts.len() == 2 {
                    current_target = Some(parts[1].trim().to_string());
                }
            }

            // Parse I_T nexus
            if line.starts_with("I_T nexus:") {
                if let Some(session) = current_session.take() {
                    sessions.push(session);
                }

                if let Some(sid_str) = line.strip_prefix("I_T nexus:") {
                    if let Ok(sid) = sid_str.trim().parse::<u32>() {
                        current_session = Some(IscsiSession {
                            sid,
                            initiator: String::new(),
                            target: current_target.clone().unwrap_or_default(),
                            connections: vec![],
                            state: SessionState::Active,
                            tsih: 0,
                            connected_at: chrono::Utc::now().timestamp(),
                            read_bytes: 0,
                            write_bytes: 0,
                            commands: 0,
                        });
                    }
                }
            }

            // Parse initiator name
            if line.starts_with("Initiator:") {
                if let Some(ref mut session) = current_session {
                    session.initiator = line
                        .strip_prefix("Initiator:")
                        .map(|s| s.trim().to_string())
                        .unwrap_or_default();
                }
            }

            // Parse connection info
            if line.starts_with("Connection:") {
                if let Some(ref mut session) = current_session {
                    session.connections.push(IscsiConnection {
                        cid: 0,
                        initiator_ip: String::new(),
                        initiator_port: 0,
                        target_ip: String::new(),
                        target_port: 3260,
                        state: ConnectionState::Logged,
                        header_digest: false,
                        data_digest: false,
                    });
                }
            }

            // Parse IP address
            if line.starts_with("IP Address:") {
                if let Some(ref mut session) = current_session {
                    if let Some(conn) = session.connections.last_mut() {
                        conn.initiator_ip = line
                            .strip_prefix("IP Address:")
                            .map(|s| s.trim().to_string())
                            .unwrap_or_default();
                    }
                }
            }
        }

        if let Some(session) = current_session {
            sessions.push(session);
        }

        sessions
    }

    /// List sessions using LIO
    async fn list_sessions_lio(&self) -> Result<Vec<IscsiSession>> {
        let output = Command::new("targetcli")
            .args(["sessions", "detail"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("targetcli failed: {}", e)))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        // LIO session parsing would need more complex implementation
        Ok(Vec::new())
    }

    /// Disconnect a session
    pub async fn disconnect_session(&self, iqn: &str, initiator_iqn: &str) -> Result<()> {
        match self.config.backend {
            IscsiBackend::Tgtd => {
                let tid = self.get_tid_for_iqn(iqn).await?;

                let output = Command::new("tgtadm")
                    .args([
                        "--lld", "iscsi",
                        "--mode", "conn",
                        "--op", "delete",
                        "--tid", &tid.to_string(),
                        "--sid", "1", // Would need to find actual SID
                        "--cid", "0",
                    ])
                    .output()
                    .await
                    .map_err(|e| Error::Internal(format!("tgtadm failed: {}", e)))?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(Error::Internal(format!("Failed to disconnect session: {}", stderr)));
                }

                Ok(())
            }
            IscsiBackend::Lio => {
                let output = Command::new("targetcli")
                    .args([&format!("/iscsi/{}/tpg1/acls/{} delete", iqn, initiator_iqn)])
                    .output()
                    .await
                    .map_err(|e| Error::Internal(format!("targetcli failed: {}", e)))?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(Error::Internal(format!("Failed to disconnect: {}", stderr)));
                }

                Ok(())
            }
        }
    }

    // ==================== Service Management ====================

    /// Start iSCSI service
    pub async fn start(&self) -> Result<()> {
        match self.config.backend {
            IscsiBackend::Tgtd => {
                crate::nas::services::manage_service(
                    &crate::nas::services::NasService::Tgtd,
                    crate::nas::services::ServiceAction::Start,
                ).await
            }
            IscsiBackend::Lio => {
                // LIO uses target.service
                let output = Command::new("systemctl")
                    .args(["start", "target"])
                    .output()
                    .await;

                if output.is_err() || !output.unwrap().status.success() {
                    // Try OpenRC
                    let _ = Command::new("rc-service")
                        .args(["target", "start"])
                        .output()
                        .await;
                }
                Ok(())
            }
        }
    }

    /// Stop iSCSI service
    pub async fn stop(&self) -> Result<()> {
        match self.config.backend {
            IscsiBackend::Tgtd => {
                crate::nas::services::manage_service(
                    &crate::nas::services::NasService::Tgtd,
                    crate::nas::services::ServiceAction::Stop,
                ).await
            }
            IscsiBackend::Lio => {
                let output = Command::new("systemctl")
                    .args(["stop", "target"])
                    .output()
                    .await;

                if output.is_err() || !output.unwrap().status.success() {
                    let _ = Command::new("rc-service")
                        .args(["target", "stop"])
                        .output()
                        .await;
                }
                Ok(())
            }
        }
    }

    /// Check if service is running
    pub async fn is_running(&self) -> bool {
        match self.config.backend {
            IscsiBackend::Tgtd => {
                Command::new("systemctl")
                    .args(["is-active", "--quiet", "tgtd"])
                    .status()
                    .await
                    .map(|s| s.success())
                    .unwrap_or(false)
                    ||
                Command::new("rc-service")
                    .args(["tgtd", "status"])
                    .output()
                    .await
                    .map(|o| {
                        let stdout = String::from_utf8_lossy(&o.stdout);
                        stdout.contains("started") || stdout.contains("running")
                    })
                    .unwrap_or(false)
            }
            IscsiBackend::Lio => {
                Command::new("systemctl")
                    .args(["is-active", "--quiet", "target"])
                    .status()
                    .await
                    .map(|s| s.success())
                    .unwrap_or(false)
            }
        }
    }

    /// Get service status
    pub async fn get_status(&self) -> Result<IscsiStatus> {
        let running = self.is_running().await;
        let targets = if running {
            self.list_targets().await.unwrap_or_default()
        } else {
            Vec::new()
        };
        let sessions = if running {
            self.list_sessions().await.unwrap_or_default()
        } else {
            Vec::new()
        };

        let total_luns: u32 = targets.iter().map(|t| t.luns.len() as u32).sum();
        let total_size: u64 = targets.iter()
            .flat_map(|t| t.luns.iter())
            .map(|l| l.size_bytes)
            .sum();

        Ok(IscsiStatus {
            running,
            backend: self.config.backend,
            version: None,
            target_count: targets.len() as u32,
            session_count: sessions.len() as u32,
            total_luns,
            total_size_bytes: total_size,
            isns_registered: self.config.isns_server.is_some(),
            portals: vec![IscsiPortal::default()],
        })
    }

    // ==================== Configuration Persistence ====================

    /// Save tgtd configuration
    async fn save_tgtd_config(&self) -> Result<()> {
        let output = Command::new("tgt-admin")
            .args(["--dump"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("tgt-admin failed: {}", e)))?;

        if !output.status.success() {
            return Err(Error::Internal("tgt-admin --dump failed".to_string()));
        }

        let config = String::from_utf8_lossy(&output.stdout);

        // Ensure directory exists
        if let Some(parent) = std::path::Path::new(&self.config_path).parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }

        tokio::fs::write(&self.config_path, config.as_bytes()).await.map_err(|e| {
            Error::Internal(format!("Failed to save config: {}", e))
        })
    }

    /// Save LIO configuration
    async fn save_lio_config(&self) -> Result<()> {
        let output = Command::new("targetcli")
            .args(["saveconfig"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("targetcli failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to save config: {}", stderr)));
        }

        Ok(())
    }

    /// Restore tgtd configuration
    pub async fn restore_config(&self) -> Result<()> {
        match self.config.backend {
            IscsiBackend::Tgtd => {
                let output = Command::new("tgt-admin")
                    .args(["--execute"])
                    .output()
                    .await
                    .map_err(|e| Error::Internal(format!("tgt-admin failed: {}", e)))?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(Error::Internal(format!("Failed to restore config: {}", stderr)));
                }

                Ok(())
            }
            IscsiBackend::Lio => {
                let output = Command::new("targetcli")
                    .args(["restoreconfig"])
                    .output()
                    .await
                    .map_err(|e| Error::Internal(format!("targetcli failed: {}", e)))?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(Error::Internal(format!("Failed to restore config: {}", stderr)));
                }

                Ok(())
            }
        }
    }
}

impl Default for IscsiTargetManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_iqn() {
        let iqn = IscsiTargetManager::generate_iqn("test_storage");
        assert!(iqn.starts_with("iqn."));
        assert!(iqn.contains("com.horcrux:"));
        assert!(iqn.contains("test-storage"));
    }

    #[test]
    fn test_validate_iqn() {
        assert!(IscsiTargetManager::validate_iqn("iqn.2024-01.com.horcrux:storage").is_ok());
        assert!(IscsiTargetManager::validate_iqn("invalid").is_err());
    }

    #[test]
    fn test_default_config() {
        let config = IscsiGlobalConfig::default();
        assert_eq!(config.max_connections, 8);
        assert!(config.immediate_data);
    }

    #[test]
    fn test_default_lun() {
        let lun = IscsiLun::default();
        assert_eq!(lun.sector_size, 512);
        assert_eq!(lun.device_type, ScsiDeviceType::Disk);
        assert!(!lun.read_only);
    }

    #[test]
    fn test_create_lun_options() {
        let opts = CreateLunOptions::default();
        assert_eq!(opts.size_bytes, 10 * 1024 * 1024 * 1024);
        assert!(opts.sparse);
    }
}
