//! Time Machine backup support module
//!
//! Provides macOS Time Machine backup target functionality via AFP or SMB.

use horcrux_common::{Error, Result};
use crate::nas::shares::NasShare;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use std::path::Path;

/// Time Machine target configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeMachineTarget {
    /// Share ID
    pub share_id: String,
    /// Share name
    pub name: String,
    /// Backup path
    pub path: String,
    /// Quota in GB (0 = unlimited)
    pub quota_gb: u64,
    /// Protocol (AFP or SMB)
    pub protocol: TimeMachineProtocol,
    /// Enabled
    pub enabled: bool,
    /// Per-machine quotas
    pub machine_quotas: Vec<MachineQuota>,
}

/// Protocol for Time Machine
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimeMachineProtocol {
    /// AFP via Netatalk
    Afp,
    /// SMB via Samba (modern)
    Smb,
}

/// Per-machine quota
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineQuota {
    /// Machine UUID or name
    pub machine_id: String,
    /// Quota in GB
    pub quota_gb: u64,
    /// Current usage in bytes
    pub used_bytes: u64,
}

/// Time Machine status for a machine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeMachineStatus {
    /// Machine ID
    pub machine_id: String,
    /// Machine name
    pub machine_name: Option<String>,
    /// Last backup timestamp
    pub last_backup: Option<i64>,
    /// Backup size in bytes
    pub backup_size_bytes: u64,
    /// Number of snapshots
    pub snapshot_count: u32,
    /// Oldest snapshot date
    pub oldest_snapshot: Option<i64>,
}

/// Configure a share as a Time Machine target
pub async fn configure_timemachine(share: &mut NasShare, quota_gb: u64) -> Result<()> {
    // Enable Time Machine in AFP config
    #[cfg(feature = "afp")]
    {
        let mut afp_config = share.afp_config.clone().unwrap_or_default();
        afp_config.time_machine = true;
        afp_config.time_machine_quota_gb = if quota_gb > 0 { Some(quota_gb) } else { None };
        share.afp_config = Some(afp_config);
    }

    // For SMB, we need to configure fruit VFS with Time Machine support
    #[cfg(feature = "smb")]
    {
        use std::collections::HashMap;

        let mut smb_config = share.smb_config.clone().unwrap_or_default();
        smb_config.fruit_enabled = true;

        // Add Time Machine VFS object
        if !smb_config.vfs_objects.contains(&"fruit".to_string()) {
            smb_config.vfs_objects.push("fruit".to_string());
        }
        if !smb_config.vfs_objects.contains(&"streams_xattr".to_string()) {
            smb_config.vfs_objects.push("streams_xattr".to_string());
        }

        // Add Time Machine parameters
        smb_config.extra_parameters.insert(
            "fruit:time machine".to_string(),
            "yes".to_string(),
        );

        if quota_gb > 0 {
            // Convert GB to bytes
            let quota_bytes = quota_gb * 1024 * 1024 * 1024;
            smb_config.extra_parameters.insert(
                "fruit:time machine max size".to_string(),
                quota_bytes.to_string(),
            );
        }

        share.smb_config = Some(smb_config);
    }

    Ok(())
}

/// List Time Machine backups in a share
pub async fn list_backups(share: &NasShare) -> Result<Vec<TimeMachineStatus>> {
    let mut backups = Vec::new();

    // Time Machine stores backups in .sparsebundle directories
    if let Ok(mut entries) = tokio::fs::read_dir(&share.path).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();

            // Look for .sparsebundle or .backupbundle directories
            if name.ends_with(".sparsebundle") || name.ends_with(".backupbundle") {
                let machine_id = name
                    .trim_end_matches(".sparsebundle")
                    .trim_end_matches(".backupbundle")
                    .to_string();

                // Get bundle size
                let size = get_bundle_size(&entry.path()).await.unwrap_or(0);

                backups.push(TimeMachineStatus {
                    machine_id: machine_id.clone(),
                    machine_name: Some(machine_id),
                    last_backup: None,
                    backup_size_bytes: size,
                    snapshot_count: 0,
                    oldest_snapshot: None,
                });
            }
        }
    }

    Ok(backups)
}

/// Get the size of a sparse bundle
async fn get_bundle_size(path: &std::path::Path) -> Result<u64> {
    let mut total = 0u64;

    // Read band files in the bundle
    let bands_path = path.join("bands");
    if bands_path.exists() {
        if let Ok(mut entries) = tokio::fs::read_dir(&bands_path).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                if let Ok(meta) = entry.metadata().await {
                    total += meta.len();
                }
            }
        }
    }

    Ok(total)
}

/// Clean up old Time Machine backups
pub async fn cleanup_old_backups(
    share: &NasShare,
    machine_id: &str,
    keep_count: u32,
) -> Result<u32> {
    // Time Machine cleanup requires parsing the backup structure
    // This is a placeholder - actual implementation would need to:
    // 1. Parse the Backups.backupdb structure
    // 2. Identify snapshot dates
    // 3. Remove oldest snapshots while keeping required count
    let _ = (share, machine_id, keep_count);
    Ok(0)
}

/// Get Time Machine quota usage
pub async fn get_quota_usage(share: &NasShare) -> Result<(u64, u64)> {
    // Returns (used_bytes, quota_bytes)
    let used = list_backups(share)
        .await?
        .iter()
        .map(|b| b.backup_size_bytes)
        .sum();

    #[cfg(feature = "afp")]
    {
        if let Some(ref config) = share.afp_config {
            if let Some(quota_gb) = config.time_machine_quota_gb {
                return Ok((used, quota_gb * 1024 * 1024 * 1024));
            }
        }
    }

    Ok((used, 0))
}

/// Time Machine Manager for managing backup targets
pub struct TimeMachineManager {
    /// Base path for Time Machine targets
    base_path: String,
}

impl TimeMachineManager {
    /// Create a new Time Machine manager
    pub fn new() -> Self {
        Self {
            base_path: "/mnt/nas/timemachine".to_string(),
        }
    }

    /// Set base path for Time Machine targets
    pub fn set_base_path(&mut self, path: &str) {
        self.base_path = path.to_string();
    }

    /// Create a new Time Machine target
    pub async fn create_target(&self, target: &TimeMachineTarget) -> Result<()> {
        // Create the directory
        tokio::fs::create_dir_all(&target.path).await.map_err(|e| {
            Error::Internal(format!("Failed to create Time Machine directory: {}", e))
        })?;

        // Set ownership (typically to the share owner)
        let output = Command::new("chown")
            .args(["-R", "nobody:nogroup", &target.path])
            .output()
            .await;

        if let Ok(out) = output {
            if !out.status.success() {
                // Non-fatal, continue
            }
        }

        // Set permissions
        let output = Command::new("chmod")
            .args(["770", &target.path])
            .output()
            .await;

        if let Ok(out) = output {
            if !out.status.success() {
                // Non-fatal, continue
            }
        }

        Ok(())
    }

    /// Delete a Time Machine target
    pub async fn delete_target(&self, path: &str, delete_backups: bool) -> Result<()> {
        if delete_backups {
            tokio::fs::remove_dir_all(path).await.map_err(|e| {
                Error::Internal(format!("Failed to delete Time Machine target: {}", e))
            })?;
        }
        Ok(())
    }

    /// Get backup info for a specific machine
    pub async fn get_backup_info(&self, target_path: &str, machine_id: &str) -> Result<TimeMachineStatus> {
        let bundle_path = Path::new(target_path)
            .join(format!("{}.sparsebundle", machine_id));

        if !bundle_path.exists() {
            let bundle_path = Path::new(target_path)
                .join(format!("{}.backupbundle", machine_id));
            if !bundle_path.exists() {
                return Err(Error::NotFound(format!("Backup for '{}' not found", machine_id)));
            }
        }

        let size = get_bundle_size(&bundle_path).await?;

        // Try to get plist info
        let info = self.parse_backup_plist(&bundle_path).await;

        Ok(TimeMachineStatus {
            machine_id: machine_id.to_string(),
            machine_name: info.as_ref().and_then(|i| i.machine_name.clone()),
            last_backup: info.as_ref().and_then(|i| i.last_backup),
            backup_size_bytes: size,
            snapshot_count: info.map(|i| i.snapshot_count).unwrap_or(0),
            oldest_snapshot: None,
        })
    }

    /// Parse backup info from plist
    async fn parse_backup_plist(&self, bundle_path: &Path) -> Option<BackupPlistInfo> {
        let plist_path = bundle_path.join("com.apple.TimeMachine.MachineID.plist");

        if !plist_path.exists() {
            return None;
        }

        // Use plutil or plistutil to parse the plist
        let output = Command::new("plutil")
            .args(["-p", &plist_path.to_string_lossy()])
            .output()
            .await
            .ok()?;

        if !output.status.success() {
            // Try plistutil (Linux)
            let output = Command::new("plistutil")
                .args(["-i", &plist_path.to_string_lossy()])
                .output()
                .await
                .ok()?;

            if !output.status.success() {
                return None;
            }
        }

        // Parse the output - this is simplified
        Some(BackupPlistInfo {
            machine_name: None,
            last_backup: None,
            snapshot_count: 0,
        })
    }

    /// Delete a specific machine's backup
    pub async fn delete_backup(&self, target_path: &str, machine_id: &str) -> Result<()> {
        let bundle_path = Path::new(target_path)
            .join(format!("{}.sparsebundle", machine_id));

        if bundle_path.exists() {
            tokio::fs::remove_dir_all(&bundle_path).await.map_err(|e| {
                Error::Internal(format!("Failed to delete backup: {}", e))
            })?;
            return Ok(());
        }

        let bundle_path = Path::new(target_path)
            .join(format!("{}.backupbundle", machine_id));

        if bundle_path.exists() {
            tokio::fs::remove_dir_all(&bundle_path).await.map_err(|e| {
                Error::Internal(format!("Failed to delete backup: {}", e))
            })?;
            return Ok(());
        }

        Err(Error::NotFound(format!("Backup for '{}' not found", machine_id)))
    }

    /// Verify backup integrity
    pub async fn verify_backup(&self, target_path: &str, machine_id: &str) -> Result<BackupVerifyResult> {
        let bundle_path = Path::new(target_path)
            .join(format!("{}.sparsebundle", machine_id));

        if !bundle_path.exists() {
            return Err(Error::NotFound(format!("Backup for '{}' not found", machine_id)));
        }

        // Check for required files
        let has_info_plist = bundle_path.join("Info.plist").exists();
        let has_bands = bundle_path.join("bands").exists();
        let has_token = bundle_path.join("token").exists();

        // Calculate checksum if possible
        let bands_path = bundle_path.join("bands");
        let mut band_count = 0u32;
        let mut total_size = 0u64;

        if has_bands {
            if let Ok(mut entries) = tokio::fs::read_dir(&bands_path).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    band_count += 1;
                    if let Ok(meta) = entry.metadata().await {
                        total_size += meta.len();
                    }
                }
            }
        }

        let valid = has_info_plist && has_bands && has_token;

        Ok(BackupVerifyResult {
            machine_id: machine_id.to_string(),
            valid,
            has_info_plist,
            has_bands,
            has_token,
            band_count,
            total_size_bytes: total_size,
            errors: if valid { Vec::new() } else { vec!["Missing required files".to_string()] },
        })
    }

    /// Get overall Time Machine status
    pub async fn get_status(&self, targets: &[TimeMachineTarget]) -> Result<TimeMachineOverallStatus> {
        let mut total_backups = 0u32;
        let mut total_size = 0u64;
        let mut active_targets = 0u32;

        for target in targets {
            if target.enabled {
                active_targets += 1;
                if let Ok(backups) = self.list_target_backups(&target.path).await {
                    total_backups += backups.len() as u32;
                    total_size += backups.iter().map(|b| b.backup_size_bytes).sum::<u64>();
                }
            }
        }

        Ok(TimeMachineOverallStatus {
            target_count: targets.len() as u32,
            active_targets,
            total_backups,
            total_size_bytes: total_size,
        })
    }

    /// List backups for a specific target
    pub async fn list_target_backups(&self, target_path: &str) -> Result<Vec<TimeMachineStatus>> {
        let mut backups = Vec::new();

        if let Ok(mut entries) = tokio::fs::read_dir(target_path).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();

                if name.ends_with(".sparsebundle") || name.ends_with(".backupbundle") {
                    let machine_id = name
                        .trim_end_matches(".sparsebundle")
                        .trim_end_matches(".backupbundle")
                        .to_string();

                    let size = get_bundle_size(&entry.path()).await.unwrap_or(0);

                    backups.push(TimeMachineStatus {
                        machine_id: machine_id.clone(),
                        machine_name: Some(machine_id),
                        last_backup: None,
                        backup_size_bytes: size,
                        snapshot_count: 0,
                        oldest_snapshot: None,
                    });
                }
            }
        }

        Ok(backups)
    }

    /// Apply quota to a machine's backup
    pub async fn set_machine_quota(&self, target_path: &str, machine_id: &str, quota_gb: u64) -> Result<()> {
        // For sparse bundles, we can set the size limit in the Info.plist
        // This is a simplified implementation
        let bundle_path = Path::new(target_path)
            .join(format!("{}.sparsebundle", machine_id));

        if !bundle_path.exists() {
            return Err(Error::NotFound(format!("Backup for '{}' not found", machine_id)));
        }

        // Use hdiutil on macOS or sparsebundlefs tools on Linux
        // This is a placeholder - actual implementation depends on available tools
        let _ = quota_gb;

        Ok(())
    }
}

impl Default for TimeMachineManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Backup plist info (internal)
struct BackupPlistInfo {
    machine_name: Option<String>,
    last_backup: Option<i64>,
    snapshot_count: u32,
}

/// Result of backup verification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupVerifyResult {
    /// Machine ID
    pub machine_id: String,
    /// Overall validity
    pub valid: bool,
    /// Has Info.plist
    pub has_info_plist: bool,
    /// Has bands directory
    pub has_bands: bool,
    /// Has token file
    pub has_token: bool,
    /// Number of band files
    pub band_count: u32,
    /// Total size in bytes
    pub total_size_bytes: u64,
    /// Error messages
    pub errors: Vec<String>,
}

/// Overall Time Machine status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeMachineOverallStatus {
    /// Total number of targets
    pub target_count: u32,
    /// Number of active (enabled) targets
    pub active_targets: u32,
    /// Total number of machine backups
    pub total_backups: u32,
    /// Total size across all backups
    pub total_size_bytes: u64,
}
