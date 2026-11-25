//! LVM storage backend
//! Provides LVM volume group and logical volume management

#![allow(dead_code)]

use super::StoragePool;
use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::{error, info};

/// LVM storage manager
pub struct LvmManager {}

impl Default for LvmManager {
    fn default() -> Self {
        Self::new()
    }
}

impl LvmManager {
    pub fn new() -> Self {
        Self {}
    }

    /// Validate an LVM volume group
    pub async fn validate_pool(&self, pool: &StoragePool) -> Result<()> {
        // Check if LVM volume group exists
        let output = Command::new("vgs")
            .arg("--noheadings")
            .arg("-o")
            .arg("vg_name")
            .arg(&pool.path)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to check LVM volume group: {}", e))
            })?;

        if !output.status.success() {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "LVM volume group {} does not exist",
                pool.path
            )));
        }

        Ok(())
    }

    /// Create an LVM logical volume
    pub async fn create_volume(
        &self,
        vg_name: &str,
        volume_name: &str,
        size_gb: u64,
    ) -> Result<String> {
        super::check_names(&[volume_name])?;
        info!(
            "Creating LVM volume: {}/{} ({}GB)",
            vg_name, volume_name, size_gb
        );

        let output = Command::new("lvcreate")
            .arg("-L")
            .arg(format!("{}G", size_gb))
            .arg("-n")
            .arg(volume_name)
            .arg(vg_name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to create LVM volume: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Failed to create LVM volume: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create LVM volume: {}",
                stderr
            )));
        }

        Ok(format!("/dev/{}/{}", vg_name, volume_name))
    }

    /// Delete a logical volume the way the pool's options say: on a shared
    /// VG take the LV exclusively first (refused while another node uses
    /// it), zero it (`wipe_removed`) and/or discard its extents
    /// (`discard_removed`), then `lvremove`.
    pub async fn delete_volume_with(
        &self,
        vg_name: &str,
        volume_name: &str,
        opts: &LvmPoolOptions,
    ) -> Result<()> {
        super::check_names(&[volume_name])?;
        let t = std::time::Duration::from_secs(opts.command_timeout_secs.into());
        let sys = |e: String| horcrux_common::Error::System(e);
        if opts.shared || opts.wipe_removed || opts.discard_removed {
            if opts.wipe_removed || opts.discard_removed {
                crate::san::tools::Need::Discard.check(&crate::san::tools::ToolStatus::probe())?;
            }
            crate::san::lvm_shared::activate_exclusive(vg_name, volume_name, t)
                .await
                .map_err(sys)?;
            let dev = format!("/dev/{vg_name}/{volume_name}");
            // Scrubbing a big LV takes a while: give it its own budget.
            let scrub_t = std::time::Duration::from_secs(opts.scrub_timeout_secs.into());
            crate::san::lvm_shared::scrub_before_remove(
                &dev,
                opts.wipe_removed,
                opts.discard_removed,
                scrub_t,
            )
            .await
            .map_err(sys)?;
        }
        self.delete_volume(vg_name, volume_name).await
    }

    /// Delete an LVM logical volume
    pub async fn delete_volume(&self, vg_name: &str, volume_name: &str) -> Result<()> {
        super::check_names(&[volume_name])?;
        info!("Deleting LVM volume: {}/{}", vg_name, volume_name);

        let output = Command::new("lvremove")
            .arg("-f")
            .arg(format!("{}/{}", vg_name, volume_name))
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to delete LVM volume: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Failed to delete LVM volume: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to delete LVM volume: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Create an LVM snapshot (supports thick-provisioned)
    ///
    /// Proxmox VE 9 feature: snapshots on thick-provisioned LVM shared storage
    /// with snapshot chains for volume management
    pub async fn create_snapshot(
        &self,
        vg_name: &str,
        volume_name: &str,
        snapshot_name: &str,
    ) -> Result<()> {
        super::check_names(&[volume_name, snapshot_name])?;
        info!(
            "Creating LVM snapshot: {}/{} -> {}",
            vg_name, volume_name, snapshot_name
        );

        // Create snapshot with same size as origin for thick-provisioned
        // This allows snapshots on FC/iSCSI SAN environments
        let output = Command::new("lvcreate")
            .arg("-s")
            .arg("-n")
            .arg(snapshot_name)
            .arg(format!("{}/{}", vg_name, volume_name))
            .arg("-L") // Thick provision snapshot
            .arg("10G") // Default snapshot size, can be customized
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to create LVM snapshot: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Failed to create LVM snapshot: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create LVM snapshot: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Create snapshot with custom size (thick-provisioned)
    pub async fn create_snapshot_sized(
        &self,
        vg_name: &str,
        volume_name: &str,
        snapshot_name: &str,
        size_gb: u64,
    ) -> Result<()> {
        super::check_names(&[volume_name, snapshot_name])?;
        info!(
            "Creating LVM snapshot: {}/{} -> {} ({}GB)",
            vg_name, volume_name, snapshot_name, size_gb
        );

        let output = Command::new("lvcreate")
            .arg("-s")
            .arg("-n")
            .arg(snapshot_name)
            .arg(format!("{}/{}", vg_name, volume_name))
            .arg("-L")
            .arg(format!("{}G", size_gb))
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to create LVM snapshot: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Failed to create LVM snapshot: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create LVM snapshot: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// List snapshots for a volume (volume chains)
    pub async fn list_snapshots(&self, vg_name: &str, volume_name: &str) -> Result<Vec<String>> {
        super::check_names(&[volume_name])?;
        let output = Command::new("lvs")
            .arg("--noheadings")
            .arg("-o")
            .arg("lv_name,origin")
            .arg(vg_name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to list LVM snapshots: {}", e))
            })?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(
                "Failed to list LVM snapshots".to_string(),
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let snapshots: Vec<String> = stdout
            .lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() == 2 && parts[1] == volume_name {
                    Some(parts[0].to_string())
                } else {
                    None
                }
            })
            .collect();

        Ok(snapshots)
    }

    /// Get snapshot chain info (for volume chains feature)
    pub async fn get_snapshot_chain(
        &self,
        vg_name: &str,
        volume_name: &str,
    ) -> Result<SnapshotChain> {
        super::check_names(&[volume_name])?;
        let snapshots = self.list_snapshots(vg_name, volume_name).await?;

        Ok(SnapshotChain {
            origin: volume_name.to_string(),
            snapshots,
        })
    }

    /// Restore an LVM snapshot (merge)
    pub async fn restore_snapshot(
        &self,
        vg_name: &str,
        _volume_name: &str,
        snapshot_name: &str,
    ) -> Result<()> {
        super::check_names(&[_volume_name, snapshot_name])?;
        info!("Restoring LVM snapshot: {}/{}", vg_name, snapshot_name);

        let output = Command::new("lvconvert")
            .arg("--merge")
            .arg(format!("{}/{}", vg_name, snapshot_name))
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to restore LVM snapshot: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Failed to restore LVM snapshot: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to restore LVM snapshot: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Check if LVM is available
    pub fn check_lvm_available() -> bool {
        std::process::Command::new("vgs")
            .arg("--version")
            .output()
            .is_ok()
    }

    /// Get LVM version
    pub async fn get_lvm_version() -> Result<String> {
        let output = Command::new("lvm")
            .arg("version")
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to get LVM version: {}", e))
            })?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(
                "LVM not found or not working".to_string(),
            ));
        }

        let version = String::from_utf8_lossy(&output.stdout);
        Ok(version.lines().next().unwrap_or("Unknown").to_string())
    }
}

/// Snapshot chain information (Proxmox VE 9 feature)
#[derive(Debug, Clone)]
pub struct SnapshotChain {
    pub origin: String,
    pub snapshots: Vec<String>,
}

/// Per-pool LVM options (`StoragePool::lvm`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LvmPoolOptions {
    /// The VG is a shared (lvmlockd/sanlock) VG on SAN storage: LVs are
    /// activated exclusively on the node that uses them.
    #[serde(default)]
    pub shared: bool,
    /// Zero volumes before removal (`blkdiscard --zeroout`). Default false.
    #[serde(default)]
    pub wipe_removed: bool,
    /// Discard volumes' extents on removal (UNMAP/TRIM). Default false.
    #[serde(default)]
    pub discard_removed: bool,
    /// Deadline for one LVM command (default 60 s).
    #[serde(default = "default_cmd_timeout")]
    pub command_timeout_secs: u32,
    /// Deadline for wiping/discarding one volume (default 3600 s).
    #[serde(default = "default_scrub_timeout")]
    pub scrub_timeout_secs: u32,
}

fn default_cmd_timeout() -> u32 {
    60
}
fn default_scrub_timeout() -> u32 {
    3600
}

impl Default for LvmPoolOptions {
    fn default() -> Self {
        Self {
            shared: false,
            wipe_removed: false,
            discard_removed: false,
            command_timeout_secs: default_cmd_timeout(),
            scrub_timeout_secs: default_scrub_timeout(),
        }
    }
}

impl LvmPoolOptions {
    pub fn validate(&self) -> Result<()> {
        if !(5..=3600).contains(&self.command_timeout_secs) {
            return Err(horcrux_common::Error::InvalidConfig(
                "lvm.command_timeout_secs must be 5..3600".into(),
            ));
        }
        if !(10..=86_400).contains(&self.scrub_timeout_secs) {
            return Err(horcrux_common::Error::InvalidConfig(
                "lvm.scrub_timeout_secs must be 10..86400".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod pool_options_tests {
    use super::*;

    #[test]
    fn defaults_and_validation() {
        let o: LvmPoolOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(o, LvmPoolOptions::default());
        assert!(!o.shared && !o.wipe_removed && !o.discard_removed);
        assert!(o.validate().is_ok());
        let bad = LvmPoolOptions {
            command_timeout_secs: 1,
            ..Default::default()
        };
        assert!(bad.validate().is_err());
    }
}
