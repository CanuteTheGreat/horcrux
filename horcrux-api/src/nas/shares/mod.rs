//! Share management module
//!
//! Provides abstractions and implementations for NAS file shares
//! across multiple protocols (SMB, NFS, AFP, WebDAV, FTP).

#[cfg(feature = "smb")]
pub mod smb;
#[cfg(feature = "nfs-server")]
pub mod nfs;
#[cfg(feature = "afp")]
pub mod afp;
#[cfg(feature = "webdav")]
pub mod webdav;
#[cfg(feature = "ftp")]
pub mod ftp;

use crate::nas::{AccessLevel, CaseSensitivity, Protocol, QuotaConfig};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// NAS Share definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NasShare {
    /// Unique identifier
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Filesystem path
    pub path: String,
    /// Storage pool reference
    pub pool_id: Option<String>,
    /// ZFS dataset or Btrfs subvolume name
    pub dataset: Option<String>,
    /// Enabled protocols for this share
    pub protocols: Vec<Protocol>,
    /// Whether the share is active
    pub enabled: bool,
    /// Description
    pub description: Option<String>,
    /// Owner user
    pub owner_user: String,
    /// Owner group
    pub owner_group: String,
    /// Share permissions
    pub permissions: SharePermissions,
    /// SMB-specific configuration
    #[cfg(feature = "smb")]
    pub smb_config: Option<SmbShareConfig>,
    /// NFS-specific configuration
    #[cfg(feature = "nfs-server")]
    pub nfs_config: Option<NfsExportConfig>,
    /// AFP-specific configuration
    #[cfg(feature = "afp")]
    pub afp_config: Option<AfpShareConfig>,
    /// WebDAV-specific configuration
    #[cfg(feature = "webdav")]
    pub webdav_config: Option<WebDavConfig>,
    /// FTP-specific configuration
    #[cfg(feature = "ftp")]
    pub ftp_config: Option<FtpShareConfig>,
    /// Quota configuration
    pub quota: Option<QuotaConfig>,
    /// Creation timestamp (Unix epoch)
    pub created_at: i64,
    /// Last update timestamp (Unix epoch)
    pub updated_at: i64,
}

impl NasShare {
    /// Create a new share with minimal configuration
    pub fn new(id: String, name: String, path: String) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            id,
            name,
            path,
            pool_id: None,
            dataset: None,
            protocols: Vec::new(),
            enabled: true,
            description: None,
            owner_user: "root".to_string(),
            owner_group: "root".to_string(),
            permissions: SharePermissions::default(),
            #[cfg(feature = "smb")]
            smb_config: None,
            #[cfg(feature = "nfs-server")]
            nfs_config: None,
            #[cfg(feature = "afp")]
            afp_config: None,
            #[cfg(feature = "webdav")]
            webdav_config: None,
            #[cfg(feature = "ftp")]
            ftp_config: None,
            quota: None,
            created_at: now,
            updated_at: now,
        }
    }
}

/// Share access level (re-export for convenience)
pub use crate::nas::AccessLevel as ShareAccess;

/// Share permissions configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharePermissions {
    /// Unix permission mode (e.g., 0o755)
    pub mode: u32,
    /// Whether ACLs are enabled
    pub acl_enabled: bool,
    /// ACL entries
    pub acl_entries: Vec<AclEntry>,
}

impl Default for SharePermissions {
    fn default() -> Self {
        Self {
            mode: 0o755,
            acl_enabled: false,
            acl_entries: Vec::new(),
        }
    }
}

/// ACL entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AclEntry {
    /// Entry type (allow, deny, audit)
    pub entry_type: AclType,
    /// Principal (user:name, group:name, or special)
    pub principal: String,
    /// Permissions
    pub permissions: AclPermissions,
    /// Flags
    pub flags: AclFlags,
}

/// ACL type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AclType {
    Allow,
    Deny,
    Audit,
    Alarm,
}

/// ACL permissions
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AclPermissions {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
    pub append: bool,
    pub delete: bool,
    pub delete_child: bool,
    pub read_attributes: bool,
    pub write_attributes: bool,
    pub read_acl: bool,
    pub write_acl: bool,
    pub take_ownership: bool,
}

/// ACL flags
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AclFlags {
    pub file_inherit: bool,
    pub directory_inherit: bool,
    pub no_propagate_inherit: bool,
    pub inherit_only: bool,
}

/// Share protocol (re-export)
pub use crate::nas::Protocol as ShareProtocol;

// === Protocol-specific configurations ===

/// SMB share configuration
#[cfg(feature = "smb")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmbShareConfig {
    /// Allow guest access
    pub guest_ok: bool,
    /// Show in browse list
    pub browseable: bool,
    /// Read-only share
    pub read_only: bool,
    /// Valid users
    pub valid_users: Vec<String>,
    /// Valid groups
    pub valid_groups: Vec<String>,
    /// Allowed hosts
    pub hosts_allow: Vec<String>,
    /// Denied hosts
    pub hosts_deny: Vec<String>,
    /// VFS objects to load
    pub vfs_objects: Vec<String>,
    /// Enable macOS Fruit VFS
    pub fruit_enabled: bool,
    /// Enable recycle bin
    pub recycle_bin: bool,
    /// Enable audit logging
    pub audit_logging: bool,
    /// Enable oplocks
    pub oplocks: bool,
    /// Case sensitivity mode
    pub case_sensitive: CaseSensitivity,
    /// Extra smb.conf parameters
    pub extra_parameters: HashMap<String, String>,
}

#[cfg(feature = "smb")]
impl Default for SmbShareConfig {
    fn default() -> Self {
        Self {
            guest_ok: false,
            browseable: true,
            read_only: false,
            valid_users: Vec::new(),
            valid_groups: Vec::new(),
            hosts_allow: Vec::new(),
            hosts_deny: Vec::new(),
            vfs_objects: vec!["fruit".to_string(), "streams_xattr".to_string()],
            fruit_enabled: true,
            recycle_bin: false,
            audit_logging: false,
            oplocks: true,
            case_sensitive: CaseSensitivity::Auto,
            extra_parameters: HashMap::new(),
        }
    }
}

/// NFS export configuration
#[cfg(feature = "nfs-server")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NfsExportConfig {
    /// Client access entries
    pub clients: Vec<NfsClient>,
    /// Security flavor
    pub security: NfsSecurity,
    /// Enable async writes
    pub async_writes: bool,
    /// Squash root to nobody
    pub root_squash: bool,
    /// Squash all users to nobody
    pub all_squash: bool,
    /// Anonymous UID
    pub anonuid: Option<u32>,
    /// Anonymous GID
    pub anongid: Option<u32>,
}

#[cfg(feature = "nfs-server")]
impl Default for NfsExportConfig {
    fn default() -> Self {
        Self {
            clients: vec![NfsClient::default()],
            security: NfsSecurity::Sys,
            async_writes: true,
            root_squash: true,
            all_squash: false,
            anonuid: None,
            anongid: None,
        }
    }
}

/// NFS client configuration
#[cfg(feature = "nfs-server")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NfsClient {
    /// Host pattern (IP, CIDR, hostname, or *)
    pub host: String,
    /// Access level
    pub access: AccessLevel,
    /// Require secure port (< 1024)
    pub secure: bool,
}

#[cfg(feature = "nfs-server")]
impl Default for NfsClient {
    fn default() -> Self {
        Self {
            host: "*".to_string(),
            access: AccessLevel::ReadWrite,
            secure: true,
        }
    }
}

/// NFS security flavor
#[cfg(feature = "nfs-server")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NfsSecurity {
    /// AUTH_SYS (default Unix auth)
    Sys,
    /// Kerberos authentication only
    Krb5,
    /// Kerberos with integrity checking
    Krb5i,
    /// Kerberos with privacy (encryption)
    Krb5p,
}

/// AFP share configuration
#[cfg(feature = "afp")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AfpShareConfig {
    /// Enable as Time Machine target
    pub time_machine: bool,
    /// Time Machine quota in GB
    pub time_machine_quota_gb: Option<u64>,
    /// Valid users
    pub valid_users: Vec<String>,
    /// Read-only users
    pub rolist: Vec<String>,
    /// Read-write users
    pub rwlist: Vec<String>,
}

#[cfg(feature = "afp")]
impl Default for AfpShareConfig {
    fn default() -> Self {
        Self {
            time_machine: false,
            time_machine_quota_gb: None,
            valid_users: Vec::new(),
            rolist: Vec::new(),
            rwlist: Vec::new(),
        }
    }
}

/// WebDAV configuration
#[cfg(feature = "webdav")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebDavConfig {
    /// Port to listen on
    pub port: u16,
    /// Enable SSL/TLS
    pub ssl: bool,
    /// Require authentication
    pub auth_required: bool,
    /// Read-only mode
    pub read_only: bool,
}

#[cfg(feature = "webdav")]
impl Default for WebDavConfig {
    fn default() -> Self {
        Self {
            port: 8080,
            ssl: true,
            auth_required: true,
            read_only: false,
        }
    }
}

/// FTP/SFTP share configuration
#[cfg(feature = "ftp")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FtpShareConfig {
    /// Allow FTP access
    pub allow_ftp: bool,
    /// Allow SFTP access
    pub allow_sftp: bool,
    /// Allow anonymous access
    pub anonymous: bool,
    /// Local root directory override
    pub local_root: Option<String>,
    /// Chroot users to share directory
    pub chroot: bool,
    /// Passive port range
    pub passive_port_range: Option<(u16, u16)>,
}

#[cfg(feature = "ftp")]
impl Default for FtpShareConfig {
    fn default() -> Self {
        Self {
            allow_ftp: true,
            allow_sftp: true,
            anonymous: false,
            local_root: None,
            chroot: true,
            passive_port_range: Some((40000, 40100)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_share() {
        let share = NasShare::new(
            "share1".to_string(),
            "My Share".to_string(),
            "/mnt/nas/share1".to_string(),
        );

        assert_eq!(share.id, "share1");
        assert_eq!(share.name, "My Share");
        assert!(share.enabled);
        assert!(share.protocols.is_empty());
    }

    #[test]
    fn test_default_permissions() {
        let perms = SharePermissions::default();
        assert_eq!(perms.mode, 0o755);
        assert!(!perms.acl_enabled);
    }
}
