///! BtrFS storage backend
///!
///! Provides BtrFS filesystem support with native snapshots and subvolumes
///!
///! Note: This module is complete but not yet fully integrated into the storage manager.
///! It will be activated when BtrFS backend support is enabled in the platform.

use horcrux_common::Result;
use tokio::process::Command as AsyncCommand;
use serde::{Deserialize, Serialize};
use super::StoragePool;
use std::path::PathBuf;

/// BtrFS manager
#[allow(dead_code)]
pub struct BtrFsManager {}

/// BtrFS snapshot
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BtrFsSnapshot {
    pub name: String,
    pub path: String,
    pub created: String,
    pub is_readonly: bool,
}

/// BtrFS subvolume info
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubvolumeInfo {
    pub id: u64,
    pub path: String,
    pub parent_id: u64,
}

#[allow(dead_code)]
impl BtrFsManager {
    pub fn new() -> Self {
        Self {}
    }

    /// Validate BtrFS pool
    pub async fn validate_pool(&self, pool: &StoragePool) -> Result<()> {
        if pool.path.is_empty() {
            return Err(horcrux_common::Error::System(
                "BtrFS path is required".to_string()
            ));
        }

        // Verify it's a BtrFS filesystem
        self.verify_btrfs(&pool.path).await?;

        Ok(())
    }

    /// Verify path is on BtrFS
    async fn verify_btrfs(&self, path: &str) -> Result<()> {
        let output = AsyncCommand::new("stat")
            .args(&["-f", "-c", "%T", path])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to check filesystem: {}", e)))?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(
                format!("Path {} is not accessible", path)
            ));
        }

        let fs_type = String::from_utf8_lossy(&output.stdout).trim().to_string();

        if fs_type != "btrfs" {
            return Err(horcrux_common::Error::System(
                format!("Path {} is not on BtrFS filesystem (found: {})", path, fs_type)
            ));
        }

        Ok(())
    }

    /// Create BtrFS subvolume for VM
    pub async fn create_subvolume(&self, base_path: &str, name: &str) -> Result<String> {
        let subvol_path = PathBuf::from(base_path).join(name);

        tracing::info!("Creating BtrFS subvolume: {}", subvol_path.display());

        let output = AsyncCommand::new("btrfs")
            .args(&["subvolume", "create", subvol_path.to_str().unwrap()])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to create subvolume: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(
                format!("Subvolume creation failed: {}", stderr)
            ));
        }

        Ok(subvol_path.to_string_lossy().to_string())
    }

    /// Delete BtrFS subvolume
    pub async fn delete_subvolume(&self, subvol_path: &str) -> Result<()> {
        tracing::info!("Deleting BtrFS subvolume: {}", subvol_path);

        let output = AsyncCommand::new("btrfs")
            .args(&["subvolume", "delete", subvol_path])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to delete subvolume: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(
                format!("Subvolume deletion failed: {}", stderr)
            ));
        }

        Ok(())
    }

    /// Create VM volume (qcow2 on subvolume)
    pub async fn create_volume(
        &self,
        base_path: &str,
        volume_name: &str,
        size_gb: u64,
    ) -> Result<String> {
        // Create subvolume for the VM
        let subvol_name = format!("vm-{}", volume_name);
        let subvol_path = self.create_subvolume(base_path, &subvol_name).await?;

        // Create qcow2 image in subvolume
        let image_path = PathBuf::from(&subvol_path).join(format!("{}.qcow2", volume_name));

        tracing::info!("Creating {}GB volume at {}", size_gb, image_path.display());

        let output = AsyncCommand::new("qemu-img")
            .args(&[
                "create",
                "-f", "qcow2",
                image_path.to_str().unwrap(),
                &format!("{}G", size_gb),
            ])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to create volume: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Clean up subvolume on failure
            let _ = self.delete_subvolume(&subvol_path).await;
            return Err(horcrux_common::Error::System(
                format!("Volume creation failed: {}", stderr)
            ));
        }

        Ok(image_path.to_string_lossy().to_string())
    }

    /// Delete volume and its subvolume
    pub async fn delete_volume(&self, volume_path: &str) -> Result<()> {
        let path = PathBuf::from(volume_path);

        // Delete the qcow2 image
        if path.exists() {
            tokio::fs::remove_file(&path).await.map_err(|e| {
                horcrux_common::Error::System(format!("Failed to delete volume: {}", e))
            })?;
        }

        // Delete the subvolume
        if let Some(parent) = path.parent() {
            self.delete_subvolume(parent.to_str().unwrap()).await?;
        }

        Ok(())
    }

    /// Create BtrFS snapshot (read-only by default)
    pub async fn create_snapshot(
        &self,
        source_path: &str,
        snapshot_name: &str,
        readonly: bool,
    ) -> Result<String> {
        let source = PathBuf::from(source_path);
        let parent = source.parent().ok_or_else(|| {
            horcrux_common::Error::System("Invalid source path".to_string())
        })?;

        let snapshot_path = parent.join(format!("{}-snapshot-{}",
            source.file_name().unwrap().to_str().unwrap(),
            snapshot_name
        ));

        tracing::info!(
            "Creating BtrFS snapshot: {} -> {} (readonly: {})",
            source_path,
            snapshot_path.display(),
            readonly
        );

        let mut args = vec!["subvolume", "snapshot"];

        if readonly {
            args.push("-r");
        }

        args.push(source_path);
        args.push(snapshot_path.to_str().unwrap());

        let output = AsyncCommand::new("btrfs")
            .args(&args)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to create snapshot: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(
                format!("Snapshot creation failed: {}", stderr)
            ));
        }

        Ok(snapshot_path.to_string_lossy().to_string())
    }

    /// List snapshots for a subvolume
    pub async fn list_snapshots(&self, base_path: &str) -> Result<Vec<BtrFsSnapshot>> {
        let output = AsyncCommand::new("btrfs")
            .args(&["subvolume", "list", "-s", base_path])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to list snapshots: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(
                format!("Snapshot list failed: {}", stderr)
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut snapshots = Vec::new();

        for line in stdout.lines().skip(1) {  // Skip header
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 9 {
                snapshots.push(BtrFsSnapshot {
                    name: parts[8].to_string(),
                    path: format!("{}/{}", base_path, parts[8]),
                    created: format!("{} {}", parts[6], parts[7]),
                    is_readonly: line.contains("ro"),
                });
            }
        }

        Ok(snapshots)
    }

    /// Restore from snapshot
    pub async fn restore_snapshot(
        &self,
        snapshot_path: &str,
        target_path: &str,
    ) -> Result<()> {
        tracing::info!("Restoring from snapshot {} to {}", snapshot_path, target_path);

        // Delete target if it exists
        if PathBuf::from(target_path).exists() {
            self.delete_subvolume(target_path).await?;
        }

        // Create writable snapshot at target location
        let output = AsyncCommand::new("btrfs")
            .args(&[
                "subvolume", "snapshot",
                snapshot_path,
                target_path,
            ])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to restore snapshot: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(
                format!("Snapshot restore failed: {}", stderr)
            ));
        }

        Ok(())
    }

    /// Get filesystem usage
    pub async fn get_usage(&self, path: &str) -> Result<(u64, u64)> {
        let output = AsyncCommand::new("btrfs")
            .args(&["filesystem", "usage", "-b", path])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to get usage: {}", e)))?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System("Failed to get filesystem usage".to_string()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        let mut total = 0u64;
        let mut used = 0u64;

        for line in stdout.lines() {
            if line.contains("Device size:") {
                if let Some(size_str) = line.split_whitespace().last() {
                    total = size_str.parse().unwrap_or(0);
                }
            } else if line.contains("Used:") {
                if let Some(size_str) = line.split_whitespace().last() {
                    used = size_str.parse().unwrap_or(0);
                }
            }
        }

        Ok((used, total))
    }

    /// Enable/disable compression
    pub async fn set_compression(&self, path: &str, algorithm: Option<&str>) -> Result<()> {
        let value = match algorithm {
            Some(algo) => format!("compress={}", algo),  // zlib, lzo, zstd
            None => "compress=no".to_string(),
        };

        tracing::info!("Setting BtrFS compression on {}: {}", path, value);

        let output = AsyncCommand::new("btrfs")
            .args(&["property", "set", path, "compression", &value])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to set compression: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(
                format!("Compression setting failed: {}", stderr)
            ));
        }

        Ok(())
    }

    /// Defragment volume
    pub async fn defragment(&self, path: &str) -> Result<()> {
        tracing::info!("Defragmenting BtrFS volume: {}", path);

        let output = AsyncCommand::new("btrfs")
            .args(&["filesystem", "defragment", "-r", path])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Defragmentation failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(
                format!("Defragmentation failed: {}", stderr)
            ));
        }

        Ok(())
    }

    /// Check if BtrFS is available
    pub fn check_btrfs_available() -> bool {
        std::process::Command::new("btrfs")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Get BtrFS version
    pub async fn get_btrfs_version() -> Result<String> {
        let output = AsyncCommand::new("btrfs")
            .arg("--version")
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to get version: {}", e)))?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System("Failed to get BtrFS version".to_string()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let version = stdout.lines().next().unwrap_or("unknown").to_string();

        Ok(version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_btrfs_available() {
        // This test just checks if btrfs command is available
        let available = BtrFsManager::check_btrfs_available();
        // Don't assert - btrfs may not be installed on test system
        println!("BtrFS available: {}", available);
    }
}
