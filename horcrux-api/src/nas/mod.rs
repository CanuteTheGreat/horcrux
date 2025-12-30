//! NAS (Network Attached Storage) management module
//!
//! Provides comprehensive file sharing services including:
//! - SMB/Samba, NFS, AFP, WebDAV, FTP/SFTP
//! - Local and directory service authentication (LDAP, AD, Kerberos)
//! - Time Machine, S3 Gateway, iSCSI Target, Rsync server
//! - ZFS/Btrfs/mdraid storage management with snapshots and replication
//!
//! # Feature Flags
//!
//! - `nas` - Core NAS functionality (required for all NAS features)
//! - `smb` - SMB/Samba file sharing
//! - `nfs-server` - NFS server exports
//! - `afp` - AFP/Netatalk for macOS
//! - `webdav` - WebDAV file access
//! - `ftp` - FTP/SFTP server
//! - `nas-auth` - Core authentication module
//! - `ldap` - LDAP client integration
//! - `ldap-server` - LDAP server (OpenLDAP)
//! - `kerberos` - Kerberos authentication
//! - `ad` - Active Directory integration
//! - `timemachine` - macOS Time Machine support
//! - `s3-gateway` - S3-compatible API
//! - `iscsi-target` - iSCSI block storage
//! - `rsync-server` - Rsync daemon

pub mod shares;
pub mod auth;
pub mod services;
pub mod storage;
pub mod monitoring;
pub mod scheduler;

use horcrux_common::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

// Re-export core types
pub use shares::{NasShare, ShareProtocol, ShareAccess, SharePermissions};
pub use auth::{NasUser, NasGroup, AclEntry};
pub use services::NasService;
pub use storage::{NasPool, NasDataset, NasSnapshot};

/// Share protocol type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    /// SMB/CIFS (Windows/Samba)
    Smb,
    /// NFS (Network File System)
    Nfs,
    /// AFP (Apple Filing Protocol)
    Afp,
    /// WebDAV (HTTP-based)
    WebDav,
    /// FTP (File Transfer Protocol)
    Ftp,
    /// SFTP (SSH File Transfer Protocol)
    Sftp,
}

impl std::fmt::Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Protocol::Smb => write!(f, "smb"),
            Protocol::Nfs => write!(f, "nfs"),
            Protocol::Afp => write!(f, "afp"),
            Protocol::WebDav => write!(f, "webdav"),
            Protocol::Ftp => write!(f, "ftp"),
            Protocol::Sftp => write!(f, "sftp"),
        }
    }
}

/// Access level for shares
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccessLevel {
    /// Read-only access
    ReadOnly,
    /// Read and write access
    ReadWrite,
    /// No access (deny)
    NoAccess,
}

impl Default for AccessLevel {
    fn default() -> Self {
        AccessLevel::ReadOnly
    }
}

/// Case sensitivity mode for file names
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaseSensitivity {
    /// Auto-detect based on filesystem
    Auto,
    /// Case-sensitive (Unix-style)
    Sensitive,
    /// Case-insensitive (Windows-style)
    Insensitive,
}

impl Default for CaseSensitivity {
    fn default() -> Self {
        CaseSensitivity::Auto
    }
}

/// NAS service status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceStatus {
    /// Service name
    pub service: NasService,
    /// Whether the service is running
    pub running: bool,
    /// Whether the service is enabled to start on boot
    pub enabled: bool,
    /// Process ID if running
    pub pid: Option<u32>,
    /// Uptime in seconds
    pub uptime_seconds: Option<u64>,
    /// Number of active connections
    pub connections: u32,
    /// Last error message if any
    pub last_error: Option<String>,
}

/// Quota configuration for users/groups/shares
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaConfig {
    /// Per-user quotas
    pub user_quotas: HashMap<String, QuotaLimit>,
    /// Per-group quotas
    pub group_quotas: HashMap<String, QuotaLimit>,
    /// Dataset-level quota in GB (ZFS quota)
    pub dataset_quota_gb: Option<u64>,
    /// Dataset-level refquota in GB (ZFS refquota)
    pub dataset_refquota_gb: Option<u64>,
}

impl Default for QuotaConfig {
    fn default() -> Self {
        Self {
            user_quotas: HashMap::new(),
            group_quotas: HashMap::new(),
            dataset_quota_gb: None,
            dataset_refquota_gb: None,
        }
    }
}

/// Quota limits
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaLimit {
    /// Soft limit in GB (warning threshold)
    pub soft_limit_gb: Option<u64>,
    /// Hard limit in GB (cannot exceed)
    pub hard_limit_gb: u64,
    /// Soft inode limit
    pub inode_soft: Option<u64>,
    /// Hard inode limit
    pub inode_hard: Option<u64>,
    /// Grace period in days after exceeding soft limit
    pub grace_period_days: Option<u32>,
}

/// NAS global configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NasConfig {
    /// Whether NAS is enabled
    pub enabled: bool,
    /// Hostname for NAS services
    pub hostname: String,
    /// Workgroup/Domain name
    pub workgroup: String,
    /// Server description
    pub description: String,
    /// UID range start for NAS users
    pub uid_start: u32,
    /// GID range start for NAS groups
    pub gid_start: u32,
    /// Default share path base
    pub share_base_path: String,
    /// Enable guest access by default
    pub guest_enabled: bool,
    /// Guest account username
    pub guest_account: String,
}

impl Default for NasConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            hostname: hostname::get()
                .map(|h| h.to_string_lossy().to_string())
                .unwrap_or_else(|_| "horcrux".to_string()),
            workgroup: "WORKGROUP".to_string(),
            description: "Horcrux NAS Server".to_string(),
            uid_start: 10000,
            gid_start: 10000,
            share_base_path: "/mnt/nas".to_string(),
            guest_enabled: false,
            guest_account: "nobody".to_string(),
        }
    }
}

/// NAS Manager - main orchestrator for all NAS functionality
pub struct NasManager {
    /// NAS configuration
    config: Arc<RwLock<NasConfig>>,
    /// Shares registry
    shares: Arc<RwLock<HashMap<String, shares::NasShare>>>,
    /// Users registry
    users: Arc<RwLock<HashMap<String, auth::NasUser>>>,
    /// Groups registry
    groups: Arc<RwLock<HashMap<String, auth::NasGroup>>>,
    /// SMB manager
    #[cfg(feature = "smb")]
    smb_manager: shares::smb::SmbManager,
    /// NFS server manager
    #[cfg(feature = "nfs-server")]
    nfs_manager: shares::nfs::NfsServerManager,
    /// AFP manager
    #[cfg(feature = "afp")]
    afp_manager: shares::afp::AfpManager,
    /// WebDAV manager
    #[cfg(feature = "webdav")]
    webdav_manager: shares::webdav::WebDavManager,
    /// FTP manager
    #[cfg(feature = "ftp")]
    ftp_manager: shares::ftp::FtpManager,
    /// Authentication manager
    auth_manager: auth::AuthManager,
    /// S3 gateway manager
    #[cfg(feature = "s3-gateway")]
    s3_gateway: services::s3::S3GatewayManager,
    /// iSCSI target manager
    #[cfg(feature = "iscsi-target")]
    iscsi_target: services::iscsi::IscsiTargetManager,
    /// Rsync server manager
    #[cfg(feature = "rsync-server")]
    rsync_server: services::rsync::RsyncManager,
    /// Storage manager
    storage_manager: storage::StorageManager,
}

impl NasManager {
    /// Create a new NAS manager
    pub fn new(config: NasConfig) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            shares: Arc::new(RwLock::new(HashMap::new())),
            users: Arc::new(RwLock::new(HashMap::new())),
            groups: Arc::new(RwLock::new(HashMap::new())),
            #[cfg(feature = "smb")]
            smb_manager: shares::smb::SmbManager::new(),
            #[cfg(feature = "nfs-server")]
            nfs_manager: shares::nfs::NfsServerManager::new(),
            #[cfg(feature = "afp")]
            afp_manager: shares::afp::AfpManager::new(),
            #[cfg(feature = "webdav")]
            webdav_manager: shares::webdav::WebDavManager::new(),
            #[cfg(feature = "ftp")]
            ftp_manager: shares::ftp::FtpManager::new(),
            auth_manager: auth::AuthManager::new(),
            #[cfg(feature = "s3-gateway")]
            s3_gateway: services::s3::S3GatewayManager::new(),
            #[cfg(feature = "iscsi-target")]
            iscsi_target: services::iscsi::IscsiTargetManager::new(),
            #[cfg(feature = "rsync-server")]
            rsync_server: services::rsync::RsyncManager::new(),
            storage_manager: storage::StorageManager::new(),
        }
    }

    /// Get NAS configuration
    pub async fn get_config(&self) -> NasConfig {
        self.config.read().await.clone()
    }

    /// Update NAS configuration
    pub async fn update_config(&self, config: NasConfig) -> Result<()> {
        *self.config.write().await = config;
        Ok(())
    }

    // === Share Management ===

    /// Create a new share
    pub async fn create_share(&self, share: shares::NasShare) -> Result<shares::NasShare> {
        let mut shares = self.shares.write().await;

        // Validate share doesn't already exist
        if shares.contains_key(&share.id) {
            return Err(Error::AlreadyExists(format!(
                "Share with ID {} already exists",
                share.id
            )));
        }

        // Create the share directory if it doesn't exist
        if !std::path::Path::new(&share.path).exists() {
            tokio::fs::create_dir_all(&share.path).await.map_err(|e| {
                Error::Internal(format!(
                    "Failed to create share directory {}: {}",
                    share.path, e
                ))
            })?;
        }

        shares.insert(share.id.clone(), share.clone());

        // Apply protocol-specific configurations
        self.apply_share_config(&share).await?;

        Ok(share)
    }

    /// Update an existing share
    pub async fn update_share(&self, id: &str, share: shares::NasShare) -> Result<shares::NasShare> {
        let mut shares = self.shares.write().await;

        if !shares.contains_key(id) {
            return Err(Error::NotFound(format!(
                "Share with ID {} not found",
                id
            )));
        }

        shares.insert(id.to_string(), share.clone());
        self.apply_share_config(&share).await?;

        Ok(share)
    }

    /// Delete a share
    pub async fn delete_share(&self, id: &str) -> Result<()> {
        let mut shares = self.shares.write().await;

        if let Some(share) = shares.remove(id) {
            self.remove_share_config(&share).await?;
        } else {
            return Err(Error::NotFound(format!(
                "Share with ID {} not found",
                id
            )));
        }

        Ok(())
    }

    /// Get a share by ID
    pub async fn get_share(&self, id: &str) -> Result<shares::NasShare> {
        let shares = self.shares.read().await;

        shares.get(id).cloned().ok_or_else(|| {
            Error::NotFound(format!("Share with ID {} not found", id))
        })
    }

    /// List all shares
    pub async fn list_shares(&self) -> Vec<shares::NasShare> {
        let shares = self.shares.read().await;
        shares.values().cloned().collect()
    }

    /// Enable a share
    pub async fn enable_share(&self, id: &str) -> Result<()> {
        let mut shares = self.shares.write().await;

        if let Some(share) = shares.get_mut(id) {
            share.enabled = true;
            let share_clone = share.clone();
            drop(shares);
            self.apply_share_config(&share_clone).await?;
        } else {
            return Err(Error::NotFound(format!(
                "Share with ID {} not found",
                id
            )));
        }

        Ok(())
    }

    /// Disable a share
    pub async fn disable_share(&self, id: &str) -> Result<()> {
        let mut shares = self.shares.write().await;

        if let Some(share) = shares.get_mut(id) {
            share.enabled = false;
            let share_clone = share.clone();
            drop(shares);
            self.remove_share_config(&share_clone).await?;
        } else {
            return Err(Error::NotFound(format!(
                "Share with ID {} not found",
                id
            )));
        }

        Ok(())
    }

    // === User Management ===

    /// Create a new NAS user
    pub async fn create_user(&self, user: auth::NasUser) -> Result<auth::NasUser> {
        let mut users = self.users.write().await;

        if users.contains_key(&user.id) {
            return Err(Error::AlreadyExists(format!(
                "User with ID {} already exists",
                user.id
            )));
        }

        // Sync user to system
        self.auth_manager.create_system_user(&user).await?;

        users.insert(user.id.clone(), user.clone());
        Ok(user)
    }

    /// Delete a NAS user
    pub async fn delete_user(&self, id: &str) -> Result<()> {
        let mut users = self.users.write().await;

        if let Some(user) = users.remove(id) {
            self.auth_manager.delete_system_user(&user).await?;
        } else {
            return Err(Error::NotFound(format!(
                "User with ID {} not found",
                id
            )));
        }

        Ok(())
    }

    /// Get a user by ID
    pub async fn get_user(&self, id: &str) -> Result<auth::NasUser> {
        let users = self.users.read().await;

        users.get(id).cloned().ok_or_else(|| {
            Error::NotFound(format!("User with ID {} not found", id))
        })
    }

    /// List all NAS users
    pub async fn list_users(&self) -> Vec<auth::NasUser> {
        let users = self.users.read().await;
        users.values().cloned().collect()
    }

    /// Set user password
    pub async fn set_user_password(&self, id: &str, password: &str) -> Result<()> {
        let users = self.users.read().await;

        if let Some(user) = users.get(id) {
            self.auth_manager.set_password(user, password).await?;
        } else {
            return Err(Error::NotFound(format!(
                "User with ID {} not found",
                id
            )));
        }

        Ok(())
    }

    // === Group Management ===

    /// Create a new NAS group
    pub async fn create_group(&self, group: auth::NasGroup) -> Result<auth::NasGroup> {
        let mut groups = self.groups.write().await;

        if groups.contains_key(&group.id) {
            return Err(Error::AlreadyExists(format!(
                "Group with ID {} already exists",
                group.id
            )));
        }

        self.auth_manager.create_system_group(&group).await?;

        groups.insert(group.id.clone(), group.clone());
        Ok(group)
    }

    /// Delete a NAS group
    pub async fn delete_group(&self, id: &str) -> Result<()> {
        let mut groups = self.groups.write().await;

        if let Some(group) = groups.remove(id) {
            self.auth_manager.delete_system_group(&group).await?;
        } else {
            return Err(Error::NotFound(format!(
                "Group with ID {} not found",
                id
            )));
        }

        Ok(())
    }

    /// List all NAS groups
    pub async fn list_groups(&self) -> Vec<auth::NasGroup> {
        let groups = self.groups.read().await;
        groups.values().cloned().collect()
    }

    // === Service Management ===

    /// Start a NAS service
    pub async fn start_service(&self, service: NasService) -> Result<()> {
        services::manage_service(&service, services::ServiceAction::Start).await
    }

    /// Stop a NAS service
    pub async fn stop_service(&self, service: NasService) -> Result<()> {
        services::manage_service(&service, services::ServiceAction::Stop).await
    }

    /// Restart a NAS service
    pub async fn restart_service(&self, service: NasService) -> Result<()> {
        services::manage_service(&service, services::ServiceAction::Restart).await
    }

    /// Get service status
    pub async fn get_service_status(&self, service: NasService) -> Result<ServiceStatus> {
        services::get_service_status(&service).await
    }

    /// List all NAS services and their status
    pub async fn list_services(&self) -> Vec<ServiceStatus> {
        let mut statuses = Vec::new();

        for service in NasService::all() {
            if let Ok(status) = self.get_service_status(service).await {
                statuses.push(status);
            }
        }

        statuses
    }

    // === Internal helpers ===

    /// Apply share configuration to all enabled protocols
    async fn apply_share_config(&self, share: &shares::NasShare) -> Result<()> {
        if !share.enabled {
            return Ok(());
        }

        for protocol in &share.protocols {
            match protocol {
                #[cfg(feature = "smb")]
                Protocol::Smb => {
                    self.smb_manager.add_share(share).await?;
                }
                #[cfg(feature = "nfs-server")]
                Protocol::Nfs => {
                    self.nfs_manager.add_export(share).await?;
                }
                #[cfg(feature = "afp")]
                Protocol::Afp => {
                    self.afp_manager.add_share(share).await?;
                }
                #[cfg(feature = "webdav")]
                Protocol::WebDav => {
                    self.webdav_manager.add_share(share).await?;
                }
                #[cfg(feature = "ftp")]
                Protocol::Ftp | Protocol::Sftp => {
                    self.ftp_manager.add_share(share).await?;
                }
                #[allow(unreachable_patterns)]
                _ => {}
            }
        }

        Ok(())
    }

    /// Remove share configuration from all protocols
    async fn remove_share_config(&self, share: &shares::NasShare) -> Result<()> {
        for protocol in &share.protocols {
            match protocol {
                #[cfg(feature = "smb")]
                Protocol::Smb => {
                    self.smb_manager.remove_share(share).await?;
                }
                #[cfg(feature = "nfs-server")]
                Protocol::Nfs => {
                    self.nfs_manager.remove_export(share).await?;
                }
                #[cfg(feature = "afp")]
                Protocol::Afp => {
                    self.afp_manager.remove_share(share).await?;
                }
                #[cfg(feature = "webdav")]
                Protocol::WebDav => {
                    self.webdav_manager.remove_share(share).await?;
                }
                #[cfg(feature = "ftp")]
                Protocol::Ftp | Protocol::Sftp => {
                    self.ftp_manager.remove_share(share).await?;
                }
                #[allow(unreachable_patterns)]
                _ => {}
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_display() {
        assert_eq!(Protocol::Smb.to_string(), "smb");
        assert_eq!(Protocol::Nfs.to_string(), "nfs");
        assert_eq!(Protocol::Afp.to_string(), "afp");
    }

    #[test]
    fn test_default_config() {
        let config = NasConfig::default();
        assert!(config.enabled);
        assert_eq!(config.workgroup, "WORKGROUP");
        assert_eq!(config.uid_start, 10000);
    }

    #[test]
    fn test_quota_default() {
        let quota = QuotaConfig::default();
        assert!(quota.user_quotas.is_empty());
        assert!(quota.dataset_quota_gb.is_none());
    }
}
