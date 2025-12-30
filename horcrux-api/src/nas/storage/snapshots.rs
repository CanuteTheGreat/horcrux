//! Snapshot management
//!
//! Handles ZFS snapshots, Btrfs snapshots, and LVM snapshots with
//! automatic scheduling, retention policies, and lifecycle management.

use horcrux_common::{Error, Result};
use crate::nas::storage::NasSnapshot;
use tokio::process::Command;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Snapshot type based on backend
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SnapshotType {
    /// ZFS snapshot
    Zfs,
    /// Btrfs snapshot (subvolume)
    Btrfs,
    /// LVM snapshot
    Lvm,
}

/// Retention period type
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum RetentionPeriod {
    /// Keep X most recent
    Count(u32),
    /// Keep for X hours
    Hours(u32),
    /// Keep for X days
    Days(u32),
    /// Keep for X weeks
    Weeks(u32),
    /// Keep for X months
    Months(u32),
    /// Keep for X years
    Years(u32),
}

/// Snapshot retention policy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionPolicy {
    /// Policy name
    pub name: String,
    /// Keep X hourly snapshots
    pub keep_hourly: Option<u32>,
    /// Keep X daily snapshots
    pub keep_daily: Option<u32>,
    /// Keep X weekly snapshots
    pub keep_weekly: Option<u32>,
    /// Keep X monthly snapshots
    pub keep_monthly: Option<u32>,
    /// Keep X yearly snapshots
    pub keep_yearly: Option<u32>,
    /// Minimum age before deletion (days)
    pub min_age_days: Option<u32>,
    /// Maximum age for any snapshot (days)
    pub max_age_days: Option<u32>,
    /// Protect snapshots with holds
    pub protect_holds: bool,
    /// Protect manually created snapshots
    pub protect_manual: bool,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            keep_hourly: Some(24),
            keep_daily: Some(7),
            keep_weekly: Some(4),
            keep_monthly: Some(12),
            keep_yearly: Some(2),
            min_age_days: Some(1),
            max_age_days: Some(365),
            protect_holds: true,
            protect_manual: true,
        }
    }
}

/// Snapshot schedule configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotSchedule {
    /// Schedule ID
    pub id: String,
    /// Schedule name
    pub name: String,
    /// Target dataset/volume pattern (supports wildcards)
    pub target: String,
    /// Create recursive snapshots
    pub recursive: bool,
    /// Cron expression for schedule
    pub cron_expression: String,
    /// Snapshot name prefix
    pub prefix: String,
    /// Retention policy to apply
    pub retention: RetentionPolicy,
    /// Whether schedule is enabled
    pub enabled: bool,
    /// Last run timestamp
    pub last_run: Option<i64>,
    /// Last run status
    pub last_status: Option<String>,
}

/// Snapshot operation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotOperationResult {
    /// Operation succeeded
    pub success: bool,
    /// Snapshots created
    pub created: Vec<String>,
    /// Snapshots deleted
    pub deleted: Vec<String>,
    /// Errors encountered
    pub errors: Vec<String>,
    /// Duration in milliseconds
    pub duration_ms: u64,
}

/// Snapshot manager
pub struct SnapshotManager {
    /// Detected snapshot type capability
    pub capabilities: Vec<SnapshotType>,
}

impl SnapshotManager {
    /// Create a new snapshot manager
    pub fn new() -> Self {
        Self {
            capabilities: Vec::new(),
        }
    }

    /// Detect available snapshot capabilities
    pub async fn detect_capabilities(&mut self) -> Result<Vec<SnapshotType>> {
        let mut caps = Vec::new();

        // Check ZFS
        if Command::new("which").arg("zfs").output().await.map(|o| o.status.success()).unwrap_or(false) {
            caps.push(SnapshotType::Zfs);
        }

        // Check Btrfs
        if Command::new("which").arg("btrfs").output().await.map(|o| o.status.success()).unwrap_or(false) {
            caps.push(SnapshotType::Btrfs);
        }

        // Check LVM
        if Command::new("which").arg("lvcreate").output().await.map(|o| o.status.success()).unwrap_or(false) {
            caps.push(SnapshotType::Lvm);
        }

        self.capabilities = caps.clone();
        Ok(caps)
    }

    /// Detect snapshot type for a given path/dataset
    pub async fn detect_type(&self, target: &str) -> Result<SnapshotType> {
        // Check if it's a ZFS dataset
        let zfs_check = Command::new("zfs")
            .args(["list", "-H", target])
            .output()
            .await;

        if zfs_check.map(|o| o.status.success()).unwrap_or(false) {
            return Ok(SnapshotType::Zfs);
        }

        // Check if path is on Btrfs
        let btrfs_check = Command::new("btrfs")
            .args(["subvolume", "show", target])
            .output()
            .await;

        if btrfs_check.map(|o| o.status.success()).unwrap_or(false) {
            return Ok(SnapshotType::Btrfs);
        }

        // Check if it's an LVM logical volume
        let lvm_check = Command::new("lvs")
            .args(["--noheadings", target])
            .output()
            .await;

        if lvm_check.map(|o| o.status.success()).unwrap_or(false) {
            return Ok(SnapshotType::Lvm);
        }

        Err(Error::NotFound(format!(
            "Cannot determine snapshot type for {}",
            target
        )))
    }
}

impl Default for SnapshotManager {
    fn default() -> Self {
        Self::new()
    }
}

/// List snapshots for a dataset
pub async fn list_snapshots(dataset: &str) -> Result<Vec<NasSnapshot>> {
    // Try ZFS first
    #[cfg(feature = "nas-zfs")]
    {
        if let Ok(snapshots) = list_zfs_snapshots(dataset).await {
            if !snapshots.is_empty() {
                return Ok(snapshots);
            }
        }
    }

    // Try Btrfs
    #[cfg(feature = "nas-btrfs")]
    {
        if let Ok(snapshots) = list_btrfs_snapshots(dataset).await {
            if !snapshots.is_empty() {
                return Ok(snapshots);
            }
        }
    }

    // Try LVM
    #[cfg(feature = "nas-lvm")]
    {
        if let Ok(snapshots) = list_lvm_snapshots(dataset).await {
            if !snapshots.is_empty() {
                return Ok(snapshots);
            }
        }
    }

    Ok(Vec::new())
}

/// List all snapshots (recursive)
pub async fn list_all_snapshots() -> Result<Vec<NasSnapshot>> {
    let mut all_snapshots = Vec::new();

    #[cfg(feature = "nas-zfs")]
    {
        // List all ZFS snapshots
        let output = Command::new("zfs")
            .args([
                "list", "-H", "-t", "snapshot",
                "-o", "name,used,refer,creation",
                "-s", "creation",
            ])
            .output()
            .await;

        if let Ok(output) = output {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for snapshot in parse_zfs_snapshot_list(&stdout) {
                    all_snapshots.push(snapshot);
                }
            }
        }
    }

    Ok(all_snapshots)
}

/// List ZFS snapshots
#[cfg(feature = "nas-zfs")]
async fn list_zfs_snapshots(dataset: &str) -> Result<Vec<NasSnapshot>> {
    let output = Command::new("zfs")
        .args([
            "list", "-H", "-t", "snapshot", "-r",
            "-o", "name,used,refer,creation",
            "-s", "creation",
            dataset,
        ])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("zfs list failed: {}", e)))?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut snapshots = Vec::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 4 {
            let full_name = parts[0].to_string();

            // Parse snapshot name from full_name (dataset@snapshot)
            let (parent_dataset, snap_name) = if let Some(idx) = full_name.find('@') {
                (full_name[..idx].to_string(), full_name[idx + 1..].to_string())
            } else {
                continue;
            };

            let used_bytes = super::parse_size(parts[1]).unwrap_or(0);
            let referenced_bytes = super::parse_size(parts[2]).unwrap_or(0);

            // Parse creation time (ZFS outputs in a specific format)
            let created_at = parse_zfs_timestamp(parts[3]).unwrap_or(0);

            // Check for holds
            let hold = check_snapshot_hold(&full_name).await.unwrap_or(false);

            snapshots.push(NasSnapshot {
                id: full_name.replace(['/', '@'], "_"),
                name: snap_name,
                full_name,
                dataset: parent_dataset,
                used_bytes,
                referenced_bytes,
                hold,
                created_at,
            });
        }
    }

    Ok(snapshots)
}

/// Parse ZFS timestamp format
fn parse_zfs_timestamp(ts: &str) -> Option<i64> {
    // ZFS outputs timestamps like "Mon Oct 21 14:30 2024"
    // For simplicity, we'll just use the current approach
    // In production, use chrono to parse properly
    chrono::DateTime::parse_from_str(ts, "%a %b %d %H:%M %Y")
        .ok()
        .map(|dt| dt.timestamp())
        .or_else(|| {
            // Try Unix timestamp format
            ts.parse::<i64>().ok()
        })
}

/// Check if a snapshot has a hold
#[cfg(feature = "nas-zfs")]
async fn check_snapshot_hold(snapshot: &str) -> Result<bool> {
    let output = Command::new("zfs")
        .args(["holds", "-H", snapshot])
        .output()
        .await?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(!stdout.trim().is_empty())
    } else {
        Ok(false)
    }
}

/// Create a snapshot
pub async fn create_snapshot(dataset: &str, name: &str) -> Result<NasSnapshot> {
    #[cfg(feature = "nas-zfs")]
    {
        create_zfs_snapshot(dataset, name).await
    }

    #[cfg(not(feature = "nas-zfs"))]
    {
        let _ = (dataset, name);
        Err(Error::Internal("ZFS not enabled".to_string()))
    }
}

/// Create a ZFS snapshot
#[cfg(feature = "nas-zfs")]
async fn create_zfs_snapshot(dataset: &str, name: &str) -> Result<NasSnapshot> {
    let full_name = format!("{}@{}", dataset, name);

    let output = Command::new("zfs")
        .args(["snapshot", &full_name])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("zfs snapshot failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Internal(format!(
            "zfs snapshot failed: {}",
            stderr
        )));
    }

    // Get the created snapshot
    let snapshots = list_zfs_snapshots(dataset).await?;
    snapshots
        .into_iter()
        .find(|s| s.full_name == full_name)
        .ok_or_else(|| Error::Internal("Snapshot created but not found".to_string()))
}

/// Delete a snapshot
pub async fn delete_snapshot(snapshot: &str) -> Result<()> {
    #[cfg(feature = "nas-zfs")]
    {
        let output = Command::new("zfs")
            .args(["destroy", snapshot])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("zfs destroy failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "zfs destroy failed: {}",
                stderr
            )));
        }
    }

    #[cfg(not(feature = "nas-zfs"))]
    {
        let _ = snapshot;
    }

    Ok(())
}

/// Rollback to a snapshot
pub async fn rollback_snapshot(snapshot: &str) -> Result<()> {
    #[cfg(feature = "nas-zfs")]
    {
        let output = Command::new("zfs")
            .args(["rollback", "-r", snapshot])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("zfs rollback failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "zfs rollback failed: {}",
                stderr
            )));
        }
    }

    #[cfg(not(feature = "nas-zfs"))]
    {
        let _ = snapshot;
    }

    Ok(())
}

/// Clone a snapshot to a new dataset
pub async fn clone_snapshot(snapshot: &str, target: &str) -> Result<()> {
    #[cfg(feature = "nas-zfs")]
    {
        let output = Command::new("zfs")
            .args(["clone", snapshot, target])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("zfs clone failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "zfs clone failed: {}",
                stderr
            )));
        }
    }

    #[cfg(not(feature = "nas-zfs"))]
    {
        let _ = (snapshot, target);
    }

    Ok(())
}

/// Hold a snapshot (prevent deletion)
pub async fn hold_snapshot(snapshot: &str, tag: &str) -> Result<()> {
    #[cfg(feature = "nas-zfs")]
    {
        let output = Command::new("zfs")
            .args(["hold", tag, snapshot])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("zfs hold failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "zfs hold failed: {}",
                stderr
            )));
        }
    }

    #[cfg(not(feature = "nas-zfs"))]
    {
        let _ = (snapshot, tag);
    }

    Ok(())
}

/// Release a snapshot hold
pub async fn release_hold(snapshot: &str, tag: &str) -> Result<()> {
    #[cfg(feature = "nas-zfs")]
    {
        let output = Command::new("zfs")
            .args(["release", tag, snapshot])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("zfs release failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "zfs release failed: {}",
                stderr
            )));
        }
    }

    #[cfg(not(feature = "nas-zfs"))]
    {
        let _ = (snapshot, tag);
    }

    Ok(())
}

/// Generate automatic snapshot name based on type
pub fn auto_snapshot_name(prefix: &str) -> String {
    let now = chrono::Utc::now();
    format!("{}-{}", prefix, now.format("%Y%m%d-%H%M%S"))
}

/// Parse ZFS snapshot list output
#[cfg(feature = "nas-zfs")]
fn parse_zfs_snapshot_list(output: &str) -> Vec<NasSnapshot> {
    let mut snapshots = Vec::new();

    for line in output.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 4 {
            let full_name = parts[0].to_string();

            let (parent_dataset, snap_name) = if let Some(idx) = full_name.find('@') {
                (full_name[..idx].to_string(), full_name[idx + 1..].to_string())
            } else {
                continue;
            };

            let used_bytes = super::parse_size(parts[1]).unwrap_or(0);
            let referenced_bytes = super::parse_size(parts[2]).unwrap_or(0);
            let created_at = parse_zfs_timestamp(parts[3]).unwrap_or(0);

            snapshots.push(NasSnapshot {
                id: full_name.replace(['/', '@'], "_"),
                name: snap_name,
                full_name,
                dataset: parent_dataset,
                used_bytes,
                referenced_bytes,
                hold: false,
                created_at,
            });
        }
    }

    snapshots
}

// ============================================================================
// Btrfs Snapshot Functions
// ============================================================================

/// List Btrfs snapshots in a directory
#[cfg(feature = "nas-btrfs")]
pub async fn list_btrfs_snapshots(subvolume: &str) -> Result<Vec<NasSnapshot>> {
    // Btrfs snapshots are stored in .snapshots directory by convention
    let snapshot_dir = format!("{}/.snapshots", subvolume);
    let mut snapshots = Vec::new();

    // List snapshots using btrfs subvolume list
    let output = Command::new("btrfs")
        .args([
            "subvolume", "list", "-s",
            "-o", subvolume,
        ])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("btrfs subvolume list failed: {}", e)))?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    for line in stdout.lines() {
        // Parse: ID xxx gen yyy top level zzz path <path>
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 9 {
            let path = parts[8..].join(" ");

            // Get snapshot info
            if let Ok(info) = get_btrfs_snapshot_info(&format!("{}/{}", subvolume, path)).await {
                snapshots.push(info);
            }
        }
    }

    Ok(snapshots)
}

/// Get Btrfs snapshot info
#[cfg(feature = "nas-btrfs")]
async fn get_btrfs_snapshot_info(path: &str) -> Result<NasSnapshot> {
    let output = Command::new("btrfs")
        .args(["subvolume", "show", path])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("btrfs subvolume show failed: {}", e)))?;

    if !output.status.success() {
        return Err(Error::Internal("Failed to get snapshot info".to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut name = String::new();
    let mut created_at = 0i64;

    for line in stdout.lines() {
        let line = line.trim();
        if line.starts_with("Name:") {
            name = line.strip_prefix("Name:").unwrap_or("").trim().to_string();
        } else if line.starts_with("Creation time:") {
            let ts = line.strip_prefix("Creation time:").unwrap_or("").trim();
            created_at = chrono::DateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S %z")
                .map(|dt| dt.timestamp())
                .unwrap_or(0);
        }
    }

    // Get disk usage
    let du_output = Command::new("du")
        .args(["-sb", path])
        .output()
        .await
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0)
        })
        .unwrap_or(0);

    Ok(NasSnapshot {
        id: path.replace('/', "_"),
        name: name.clone(),
        full_name: path.to_string(),
        dataset: Path::new(path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
        used_bytes: du_output,
        referenced_bytes: du_output,
        hold: false,
        created_at,
    })
}

/// Create a Btrfs snapshot
#[cfg(feature = "nas-btrfs")]
pub async fn create_btrfs_snapshot(source: &str, dest: &str, readonly: bool) -> Result<NasSnapshot> {
    let mut args = vec!["subvolume", "snapshot"];

    if readonly {
        args.push("-r");
    }

    args.push(source);
    args.push(dest);

    let output = Command::new("btrfs")
        .args(&args)
        .output()
        .await
        .map_err(|e| Error::Internal(format!("btrfs snapshot failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Internal(format!("btrfs snapshot failed: {}", stderr)));
    }

    get_btrfs_snapshot_info(dest).await
}

/// Delete a Btrfs snapshot
#[cfg(feature = "nas-btrfs")]
pub async fn delete_btrfs_snapshot(path: &str) -> Result<()> {
    let output = Command::new("btrfs")
        .args(["subvolume", "delete", path])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("btrfs subvolume delete failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Internal(format!(
            "btrfs subvolume delete failed: {}",
            stderr
        )));
    }

    Ok(())
}

// ============================================================================
// LVM Snapshot Functions
// ============================================================================

/// List LVM snapshots
#[cfg(feature = "nas-lvm")]
pub async fn list_lvm_snapshots(volume_group: &str) -> Result<Vec<NasSnapshot>> {
    let output = Command::new("lvs")
        .args([
            "--noheadings",
            "-o", "lv_name,origin,lv_size,lv_time,lv_attr",
            volume_group,
        ])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("lvs failed: {}", e)))?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut snapshots = Vec::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 4 {
            let name = parts[0].to_string();
            let origin = parts[1].to_string();

            // Skip if not a snapshot (origin is empty)
            if origin.is_empty() {
                continue;
            }

            let size = super::parse_size(parts[2]).unwrap_or(0);
            let created_at = parse_lvm_timestamp(parts.get(3).unwrap_or(&""))
                .unwrap_or(0);

            snapshots.push(NasSnapshot {
                id: format!("{}_{}", volume_group.replace('/', "_"), name),
                name: name.clone(),
                full_name: format!("{}/{}", volume_group, name),
                dataset: format!("{}/{}", volume_group, origin),
                used_bytes: size,
                referenced_bytes: size,
                hold: false,
                created_at,
            });
        }
    }

    Ok(snapshots)
}

/// Parse LVM timestamp
fn parse_lvm_timestamp(ts: &str) -> Option<i64> {
    chrono::DateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S %z")
        .ok()
        .map(|dt| dt.timestamp())
        .or_else(|| ts.parse::<i64>().ok())
}

/// Create an LVM snapshot
#[cfg(feature = "nas-lvm")]
pub async fn create_lvm_snapshot(
    source_lv: &str,
    snapshot_name: &str,
    size: &str,
) -> Result<NasSnapshot> {
    let output = Command::new("lvcreate")
        .args([
            "--snapshot",
            "--name", snapshot_name,
            "--size", size,
            source_lv,
        ])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("lvcreate failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Internal(format!("lvcreate failed: {}", stderr)));
    }

    // Extract VG from source_lv path
    let vg = Path::new(source_lv)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| source_lv.to_string());

    // Get the created snapshot
    let snapshots = list_lvm_snapshots(&vg).await?;
    snapshots
        .into_iter()
        .find(|s| s.name == snapshot_name)
        .ok_or_else(|| Error::Internal("Snapshot created but not found".to_string()))
}

/// Delete an LVM snapshot
#[cfg(feature = "nas-lvm")]
pub async fn delete_lvm_snapshot(snapshot_path: &str) -> Result<()> {
    let output = Command::new("lvremove")
        .args(["-f", snapshot_path])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("lvremove failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Internal(format!("lvremove failed: {}", stderr)));
    }

    Ok(())
}

// ============================================================================
// Retention Policy Execution
// ============================================================================

/// Apply retention policy to snapshots
pub async fn apply_retention_policy(
    dataset: &str,
    policy: &RetentionPolicy,
    prefix: Option<&str>,
) -> Result<SnapshotOperationResult> {
    let start_time = std::time::Instant::now();
    let mut result = SnapshotOperationResult {
        success: true,
        created: Vec::new(),
        deleted: Vec::new(),
        errors: Vec::new(),
        duration_ms: 0,
    };

    // Get all snapshots for the dataset
    let mut snapshots = list_snapshots(dataset).await?;

    // Filter by prefix if specified
    if let Some(prefix) = prefix {
        snapshots.retain(|s| s.name.starts_with(prefix));
    }

    // Sort by creation time (newest first)
    snapshots.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    let now = chrono::Utc::now().timestamp();

    // Categorize snapshots by time bucket
    let mut hourly: Vec<&NasSnapshot> = Vec::new();
    let mut daily: Vec<&NasSnapshot> = Vec::new();
    let mut weekly: Vec<&NasSnapshot> = Vec::new();
    let mut monthly: Vec<&NasSnapshot> = Vec::new();
    let mut yearly: Vec<&NasSnapshot> = Vec::new();

    let mut seen_hours = std::collections::HashSet::new();
    let mut seen_days = std::collections::HashSet::new();
    let mut seen_weeks = std::collections::HashSet::new();
    let mut seen_months = std::collections::HashSet::new();
    let mut seen_years = std::collections::HashSet::new();

    for snapshot in &snapshots {
        let dt = chrono::DateTime::from_timestamp(snapshot.created_at, 0)
            .unwrap_or_else(|| chrono::Utc::now());

        let hour_key = dt.format("%Y%m%d%H").to_string();
        let day_key = dt.format("%Y%m%d").to_string();
        let week_key = format!("{}{}", dt.format("%Y").to_string(), dt.iso_week().week());
        let month_key = dt.format("%Y%m").to_string();
        let year_key = dt.format("%Y").to_string();

        if !seen_hours.contains(&hour_key) {
            seen_hours.insert(hour_key);
            hourly.push(snapshot);
        }
        if !seen_days.contains(&day_key) {
            seen_days.insert(day_key);
            daily.push(snapshot);
        }
        if !seen_weeks.contains(&week_key) {
            seen_weeks.insert(week_key);
            weekly.push(snapshot);
        }
        if !seen_months.contains(&month_key) {
            seen_months.insert(month_key);
            monthly.push(snapshot);
        }
        if !seen_years.contains(&year_key) {
            seen_years.insert(year_key);
            yearly.push(snapshot);
        }
    }

    // Determine which snapshots to keep
    let mut keep_set = std::collections::HashSet::new();

    if let Some(keep) = policy.keep_hourly {
        for snap in hourly.iter().take(keep as usize) {
            keep_set.insert(&snap.full_name);
        }
    }
    if let Some(keep) = policy.keep_daily {
        for snap in daily.iter().take(keep as usize) {
            keep_set.insert(&snap.full_name);
        }
    }
    if let Some(keep) = policy.keep_weekly {
        for snap in weekly.iter().take(keep as usize) {
            keep_set.insert(&snap.full_name);
        }
    }
    if let Some(keep) = policy.keep_monthly {
        for snap in monthly.iter().take(keep as usize) {
            keep_set.insert(&snap.full_name);
        }
    }
    if let Some(keep) = policy.keep_yearly {
        for snap in yearly.iter().take(keep as usize) {
            keep_set.insert(&snap.full_name);
        }
    }

    // Delete snapshots that should not be kept
    for snapshot in &snapshots {
        let age_days = (now - snapshot.created_at) / 86400;

        // Check minimum age
        if let Some(min_age) = policy.min_age_days {
            if age_days < min_age as i64 {
                continue; // Too young to delete
            }
        }

        // Check if in keep set
        if keep_set.contains(&snapshot.full_name) {
            continue;
        }

        // Check if held
        if policy.protect_holds && snapshot.hold {
            continue;
        }

        // Check if manual snapshot
        if policy.protect_manual && !snapshot.name.contains('-') {
            continue;
        }

        // Check max age
        if let Some(max_age) = policy.max_age_days {
            if age_days > max_age as i64 {
                // Must delete regardless
            }
        }

        // Delete the snapshot
        match delete_snapshot(&snapshot.full_name).await {
            Ok(_) => {
                result.deleted.push(snapshot.full_name.clone());
            }
            Err(e) => {
                result.errors.push(format!(
                    "Failed to delete {}: {}",
                    snapshot.full_name, e
                ));
                result.success = false;
            }
        }
    }

    result.duration_ms = start_time.elapsed().as_millis() as u64;
    Ok(result)
}

/// Execute a snapshot schedule
pub async fn execute_schedule(schedule: &SnapshotSchedule) -> Result<SnapshotOperationResult> {
    let start_time = std::time::Instant::now();
    let mut result = SnapshotOperationResult {
        success: true,
        created: Vec::new(),
        deleted: Vec::new(),
        errors: Vec::new(),
        duration_ms: 0,
    };

    // Generate snapshot name
    let snap_name = auto_snapshot_name(&schedule.prefix);

    // Create snapshot(s)
    if schedule.recursive {
        // Get all child datasets
        let datasets = get_child_datasets(&schedule.target).await?;

        for dataset in datasets {
            match create_snapshot(&dataset, &snap_name).await {
                Ok(snap) => {
                    result.created.push(snap.full_name);
                }
                Err(e) => {
                    result.errors.push(format!(
                        "Failed to create snapshot for {}: {}",
                        dataset, e
                    ));
                    result.success = false;
                }
            }
        }
    } else {
        match create_snapshot(&schedule.target, &snap_name).await {
            Ok(snap) => {
                result.created.push(snap.full_name);
            }
            Err(e) => {
                result.errors.push(format!(
                    "Failed to create snapshot for {}: {}",
                    schedule.target, e
                ));
                result.success = false;
            }
        }
    }

    // Apply retention policy
    let retention_result = apply_retention_policy(
        &schedule.target,
        &schedule.retention,
        Some(&schedule.prefix),
    )
    .await?;

    result.deleted.extend(retention_result.deleted);
    result.errors.extend(retention_result.errors);
    if !retention_result.success {
        result.success = false;
    }

    result.duration_ms = start_time.elapsed().as_millis() as u64;
    Ok(result)
}

/// Get child datasets for recursive operations
async fn get_child_datasets(parent: &str) -> Result<Vec<String>> {
    let output = Command::new("zfs")
        .args(["list", "-H", "-r", "-o", "name", parent])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("zfs list failed: {}", e)))?;

    if !output.status.success() {
        return Ok(vec![parent.to_string()]);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let datasets: Vec<String> = stdout
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    Ok(datasets)
}

/// Create recursive snapshot
pub async fn create_recursive_snapshot(dataset: &str, name: &str) -> Result<Vec<NasSnapshot>> {
    #[cfg(feature = "nas-zfs")]
    {
        let full_name = format!("{}@{}", dataset, name);

        let output = Command::new("zfs")
            .args(["snapshot", "-r", &full_name])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("zfs snapshot -r failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "zfs snapshot -r failed: {}",
                stderr
            )));
        }

        // Get all created snapshots
        let datasets = get_child_datasets(dataset).await?;
        let mut snapshots = Vec::new();

        for ds in datasets {
            let snap_full_name = format!("{}@{}", ds, name);
            if let Ok(snaps) = list_zfs_snapshots(&ds).await {
                if let Some(snap) = snaps.into_iter().find(|s| s.full_name == snap_full_name) {
                    snapshots.push(snap);
                }
            }
        }

        Ok(snapshots)
    }

    #[cfg(not(feature = "nas-zfs"))]
    {
        let _ = (dataset, name);
        Err(Error::Internal("ZFS not enabled".to_string()))
    }
}

/// Get snapshot space usage summary
pub async fn get_snapshot_usage(dataset: &str) -> Result<SnapshotUsageSummary> {
    let snapshots = list_snapshots(dataset).await?;

    let total_count = snapshots.len() as u32;
    let total_used: u64 = snapshots.iter().map(|s| s.used_bytes).sum();
    let total_referenced: u64 = snapshots.iter().map(|s| s.referenced_bytes).sum();

    let oldest = snapshots.iter().map(|s| s.created_at).min();
    let newest = snapshots.iter().map(|s| s.created_at).max();

    // Count by prefix
    let mut by_prefix: HashMap<String, u32> = HashMap::new();
    for snap in &snapshots {
        let prefix = snap.name.split('-').next().unwrap_or("manual").to_string();
        *by_prefix.entry(prefix).or_insert(0) += 1;
    }

    Ok(SnapshotUsageSummary {
        dataset: dataset.to_string(),
        total_count,
        total_used_bytes: total_used,
        total_referenced_bytes: total_referenced,
        oldest_snapshot: oldest,
        newest_snapshot: newest,
        count_by_prefix: by_prefix,
    })
}

/// Snapshot usage summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotUsageSummary {
    /// Dataset name
    pub dataset: String,
    /// Total snapshot count
    pub total_count: u32,
    /// Total space used by snapshots
    pub total_used_bytes: u64,
    /// Total referenced space
    pub total_referenced_bytes: u64,
    /// Oldest snapshot timestamp
    pub oldest_snapshot: Option<i64>,
    /// Newest snapshot timestamp
    pub newest_snapshot: Option<i64>,
    /// Count by prefix
    pub count_by_prefix: HashMap<String, u32>,
}

/// Compare two snapshots (diff)
#[cfg(feature = "nas-zfs")]
pub async fn diff_snapshots(snapshot1: &str, snapshot2: &str) -> Result<Vec<SnapshotDiff>> {
    let output = Command::new("zfs")
        .args(["diff", "-H", snapshot1, snapshot2])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("zfs diff failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Internal(format!("zfs diff failed: {}", stderr)));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut diffs = Vec::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(2, '\t').collect();
        if parts.len() >= 2 {
            let change_type = match parts[0] {
                "+" => SnapshotChangeType::Added,
                "-" => SnapshotChangeType::Removed,
                "M" => SnapshotChangeType::Modified,
                "R" => SnapshotChangeType::Renamed,
                _ => continue,
            };

            diffs.push(SnapshotDiff {
                change_type,
                path: parts[1].to_string(),
                new_path: None,
            });
        }
    }

    Ok(diffs)
}

/// Snapshot diff entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotDiff {
    /// Type of change
    pub change_type: SnapshotChangeType,
    /// Path that changed
    pub path: String,
    /// New path (for renames)
    pub new_path: Option<String>,
}

/// Snapshot change type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SnapshotChangeType {
    Added,
    Removed,
    Modified,
    Renamed,
}

/// Send snapshot to file (for backup)
#[cfg(feature = "nas-zfs")]
pub async fn send_snapshot_to_file(
    snapshot: &str,
    output_path: &str,
    compressed: bool,
    incremental_base: Option<&str>,
) -> Result<u64> {
    let mut args = vec!["send"];

    if compressed {
        args.push("-c");
    }

    if let Some(base) = incremental_base {
        args.push("-i");
        args.push(base);
    }

    args.push(snapshot);

    let command = if compressed {
        format!(
            "zfs {} | gzip > {}",
            args.join(" "),
            output_path
        )
    } else {
        format!(
            "zfs {} > {}",
            args.join(" "),
            output_path
        )
    };

    let output = Command::new("sh")
        .args(["-c", &command])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("zfs send failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Internal(format!("zfs send failed: {}", stderr)));
    }

    // Get file size
    let metadata = tokio::fs::metadata(output_path).await.map_err(|e| {
        Error::Internal(format!("Failed to get file size: {}", e))
    })?;

    Ok(metadata.len())
}

/// Receive snapshot from file
#[cfg(feature = "nas-zfs")]
pub async fn receive_snapshot_from_file(
    input_path: &str,
    target_dataset: &str,
    force: bool,
) -> Result<()> {
    let mut args = vec!["receive"];

    if force {
        args.push("-F");
    }

    args.push(target_dataset);

    let command = if input_path.ends_with(".gz") {
        format!(
            "gunzip -c {} | zfs {}",
            input_path,
            args.join(" ")
        )
    } else {
        format!(
            "zfs {} < {}",
            args.join(" "),
            input_path
        )
    };

    let output = Command::new("sh")
        .args(["-c", &command])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("zfs receive failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Internal(format!("zfs receive failed: {}", stderr)));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auto_snapshot_name() {
        let name = auto_snapshot_name("hourly");
        assert!(name.starts_with("hourly-"));
        assert!(name.len() > 7);
    }

    #[test]
    fn test_retention_policy_default() {
        let policy = RetentionPolicy::default();
        assert_eq!(policy.keep_hourly, Some(24));
        assert_eq!(policy.keep_daily, Some(7));
        assert_eq!(policy.keep_weekly, Some(4));
        assert_eq!(policy.keep_monthly, Some(12));
        assert_eq!(policy.keep_yearly, Some(2));
    }
}
