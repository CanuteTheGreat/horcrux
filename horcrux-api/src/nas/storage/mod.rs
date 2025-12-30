//! NAS Storage module
//!
//! Manages storage pools, datasets, snapshots, and replication for NAS:
//! - ZFS pools and datasets
//! - Btrfs subvolumes
//! - mdraid arrays
//! - LVM thin volumes
//! - Periodic snapshots
//! - Replication tasks

pub mod pools;
pub mod datasets;
pub mod snapshots;
pub mod replication;
pub mod quotas;

use horcrux_common::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::process::Command;

/// Storage backend type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageType {
    /// ZFS pool
    Zfs,
    /// Btrfs filesystem
    Btrfs,
    /// Linux software RAID (mdadm)
    Mdraid,
    /// LVM logical volume
    Lvm,
    /// Directory (simple path)
    Directory,
}

impl std::fmt::Display for StorageType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageType::Zfs => write!(f, "zfs"),
            StorageType::Btrfs => write!(f, "btrfs"),
            StorageType::Mdraid => write!(f, "mdraid"),
            StorageType::Lvm => write!(f, "lvm"),
            StorageType::Directory => write!(f, "directory"),
        }
    }
}

/// RAID level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RaidLevel {
    /// No redundancy (stripe)
    Raid0,
    /// Mirroring
    Raid1,
    /// Single parity
    Raid5,
    /// Double parity
    Raid6,
    /// Striped mirrors
    Raid10,
    /// ZFS single parity
    RaidZ,
    /// ZFS double parity
    RaidZ2,
    /// ZFS triple parity
    RaidZ3,
    /// ZFS mirror
    Mirror,
    /// Single disk
    Single,
}

impl std::fmt::Display for RaidLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RaidLevel::Raid0 => write!(f, "raid0"),
            RaidLevel::Raid1 => write!(f, "raid1"),
            RaidLevel::Raid5 => write!(f, "raid5"),
            RaidLevel::Raid6 => write!(f, "raid6"),
            RaidLevel::Raid10 => write!(f, "raid10"),
            RaidLevel::RaidZ => write!(f, "raidz"),
            RaidLevel::RaidZ2 => write!(f, "raidz2"),
            RaidLevel::RaidZ3 => write!(f, "raidz3"),
            RaidLevel::Mirror => write!(f, "mirror"),
            RaidLevel::Single => write!(f, "single"),
        }
    }
}

/// Pool health status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PoolHealth {
    /// Pool is healthy
    Online,
    /// Pool is degraded (missing disk)
    Degraded,
    /// Pool has faulted device
    Faulted,
    /// Pool is offline
    Offline,
    /// Pool is unavailable
    Unavailable,
    /// Pool is being resilvered
    Resilvering,
    /// Pool is being scrubbed
    Scrubbing,
    /// Unknown status
    Unknown,
}

/// NAS storage pool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NasPool {
    /// Unique identifier
    pub id: String,
    /// Pool name
    pub name: String,
    /// Storage type (ZFS, Btrfs, etc.)
    pub storage_type: StorageType,
    /// RAID level
    pub raid_level: RaidLevel,
    /// Member disks/devices
    pub devices: Vec<String>,
    /// Mount path
    pub mount_path: String,
    /// Total capacity in bytes
    pub total_bytes: u64,
    /// Used space in bytes
    pub used_bytes: u64,
    /// Available space in bytes
    pub available_bytes: u64,
    /// Pool health
    pub health: PoolHealth,
    /// Whether the pool is imported/mounted
    pub online: bool,
    /// Compression enabled (ZFS/Btrfs)
    pub compression: Option<String>,
    /// Deduplication enabled (ZFS)
    pub dedup: bool,
    /// Properties
    pub properties: HashMap<String, String>,
    /// Creation timestamp
    pub created_at: i64,
}

impl NasPool {
    /// Get usage percentage
    pub fn usage_percent(&self) -> f64 {
        if self.total_bytes == 0 {
            0.0
        } else {
            (self.used_bytes as f64 / self.total_bytes as f64) * 100.0
        }
    }
}

/// NAS dataset (ZFS dataset, Btrfs subvolume, LVM volume)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NasDataset {
    /// Unique identifier
    pub id: String,
    /// Dataset name
    pub name: String,
    /// Full path (e.g., pool/dataset)
    pub full_name: String,
    /// Parent pool ID
    pub pool_id: String,
    /// Mount path
    pub mount_path: String,
    /// Dataset type
    pub dataset_type: DatasetType,
    /// Used space in bytes
    pub used_bytes: u64,
    /// Referenced space in bytes
    pub referenced_bytes: u64,
    /// Available space in bytes
    pub available_bytes: u64,
    /// Quota in bytes (None = unlimited)
    pub quota_bytes: Option<u64>,
    /// Reference quota in bytes (ZFS refquota)
    pub refquota_bytes: Option<u64>,
    /// Compression setting
    pub compression: Option<String>,
    /// Record size (ZFS)
    pub recordsize: Option<u32>,
    /// Access time updates
    pub atime: bool,
    /// Sync behavior
    pub sync: String,
    /// Properties
    pub properties: HashMap<String, String>,
    /// Creation timestamp
    pub created_at: i64,
}

/// Dataset type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatasetType {
    /// Filesystem dataset
    Filesystem,
    /// Block volume (zvol)
    Volume,
    /// Snapshot
    Snapshot,
    /// Bookmark (ZFS)
    Bookmark,
}

/// NAS snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NasSnapshot {
    /// Unique identifier
    pub id: String,
    /// Snapshot name
    pub name: String,
    /// Full path (e.g., pool/dataset@snapshot)
    pub full_name: String,
    /// Parent dataset full name
    pub dataset: String,
    /// Used space in bytes
    pub used_bytes: u64,
    /// Referenced space in bytes
    pub referenced_bytes: u64,
    /// Whether this is a hold (cannot be destroyed)
    pub hold: bool,
    /// Creation timestamp
    pub created_at: i64,
}

/// Replication task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationTask {
    /// Unique identifier
    pub id: String,
    /// Task name
    pub name: String,
    /// Source dataset
    pub source_dataset: String,
    /// Target host
    pub target_host: String,
    /// Target dataset
    pub target_dataset: String,
    /// Replication direction
    pub direction: ReplicationDirection,
    /// Transport method
    pub transport: ReplicationTransport,
    /// Schedule (cron expression)
    pub schedule: String,
    /// Recursive replication
    pub recursive: bool,
    /// Retention policy
    pub retention: Option<RetentionPolicy>,
    /// Compress during transfer
    pub compression: bool,
    /// Bandwidth limit in KB/s (None = unlimited)
    pub bandwidth_limit: Option<u32>,
    /// Whether the task is enabled
    pub enabled: bool,
    /// Last run timestamp
    pub last_run: Option<i64>,
    /// Last run status
    pub last_status: Option<String>,
    /// Creation timestamp
    pub created_at: i64,
}

/// Replication direction
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplicationDirection {
    Push,
    Pull,
}

/// Replication transport
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplicationTransport {
    Ssh,
    Local,
    Netcat,
}

/// Retention policy for snapshots
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionPolicy {
    /// Keep N hourly snapshots
    pub hourly: Option<u32>,
    /// Keep N daily snapshots
    pub daily: Option<u32>,
    /// Keep N weekly snapshots
    pub weekly: Option<u32>,
    /// Keep N monthly snapshots
    pub monthly: Option<u32>,
    /// Keep N yearly snapshots
    pub yearly: Option<u32>,
    /// Keep all snapshots newer than N days
    pub keep_days: Option<u32>,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            hourly: Some(24),
            daily: Some(7),
            weekly: Some(4),
            monthly: Some(12),
            yearly: Some(2),
            keep_days: Some(7),
        }
    }
}

/// Storage manager
pub struct StorageManager {
    pools: HashMap<String, NasPool>,
}

impl StorageManager {
    /// Create a new storage manager
    pub fn new() -> Self {
        Self {
            pools: HashMap::new(),
        }
    }

    /// List all ZFS pools
    #[cfg(feature = "nas-zfs")]
    pub async fn list_zfs_pools(&self) -> Result<Vec<NasPool>> {
        pools::list_zfs_pools().await
    }

    /// Create a new ZFS pool
    #[cfg(feature = "nas-zfs")]
    pub async fn create_zfs_pool(
        &self,
        name: &str,
        raid_level: RaidLevel,
        devices: &[String],
    ) -> Result<NasPool> {
        pools::create_zfs_pool(name, raid_level, devices).await
    }

    /// Destroy a ZFS pool
    #[cfg(feature = "nas-zfs")]
    pub async fn destroy_zfs_pool(&self, name: &str) -> Result<()> {
        pools::destroy_zfs_pool(name).await
    }

    /// List datasets in a pool
    pub async fn list_datasets(&self, pool: &str) -> Result<Vec<NasDataset>> {
        datasets::list_datasets(pool).await
    }

    /// Create a dataset
    pub async fn create_dataset(&self, pool: &str, name: &str) -> Result<NasDataset> {
        datasets::create_dataset(pool, name).await
    }

    /// List snapshots
    pub async fn list_snapshots(&self, dataset: &str) -> Result<Vec<NasSnapshot>> {
        snapshots::list_snapshots(dataset).await
    }

    /// Create a snapshot
    pub async fn create_snapshot(&self, dataset: &str, name: &str) -> Result<NasSnapshot> {
        snapshots::create_snapshot(dataset, name).await
    }

    /// Start pool scrub
    #[cfg(feature = "nas-zfs")]
    pub async fn scrub_pool(&self, pool: &str) -> Result<()> {
        pools::scrub_zfs_pool(pool).await
    }
}

impl Default for StorageManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Check if a command exists
pub async fn command_exists(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Parse size string to bytes (e.g., "100G" -> 107374182400)
pub fn parse_size(size: &str) -> Option<u64> {
    let size = size.trim();
    if size.is_empty() {
        return None;
    }

    let (num_str, suffix) = if size.chars().last()?.is_alphabetic() {
        let idx = size.len() - 1;
        (&size[..idx], size.chars().last()?)
    } else {
        (size, ' ')
    };

    let num: f64 = num_str.parse().ok()?;
    let multiplier = match suffix.to_ascii_uppercase() {
        'K' => 1024.0,
        'M' => 1024.0 * 1024.0,
        'G' => 1024.0 * 1024.0 * 1024.0,
        'T' => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        'P' => 1024.0 * 1024.0 * 1024.0 * 1024.0 * 1024.0,
        ' ' => 1.0,
        _ => return None,
    };

    Some((num * multiplier) as u64)
}

/// Format bytes to human-readable size
pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;
    const PB: u64 = TB * 1024;

    if bytes >= PB {
        format!("{:.2}P", bytes as f64 / PB as f64)
    } else if bytes >= TB {
        format!("{:.2}T", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2}G", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2}M", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2}K", bytes as f64 / KB as f64)
    } else {
        format!("{}B", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_size() {
        assert_eq!(parse_size("1024"), Some(1024));
        assert_eq!(parse_size("1K"), Some(1024));
        assert_eq!(parse_size("1M"), Some(1024 * 1024));
        assert_eq!(parse_size("1G"), Some(1024 * 1024 * 1024));
        assert_eq!(parse_size("1T"), Some(1024 * 1024 * 1024 * 1024));
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(1024), "1.00K");
        assert_eq!(format_size(1024 * 1024), "1.00M");
        assert_eq!(format_size(1024 * 1024 * 1024), "1.00G");
    }

    #[test]
    fn test_pool_usage() {
        let pool = NasPool {
            id: "test".to_string(),
            name: "test".to_string(),
            storage_type: StorageType::Zfs,
            raid_level: RaidLevel::RaidZ,
            devices: vec![],
            mount_path: "/mnt/test".to_string(),
            total_bytes: 1000,
            used_bytes: 500,
            available_bytes: 500,
            health: PoolHealth::Online,
            online: true,
            compression: None,
            dedup: false,
            properties: HashMap::new(),
            created_at: 0,
        };

        assert_eq!(pool.usage_percent(), 50.0);
    }
}
