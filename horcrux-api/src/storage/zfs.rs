///! ZFS storage backend
///! Provides ZFS pool and volume management with snapshots and clones

use super::StoragePool;
use horcrux_common::Result;
use tokio::process::Command;
use tracing::{error, info};

/// ZFS storage manager
pub struct ZfsManager {}

impl ZfsManager {
    pub fn new() -> Self {
        Self {}
    }

    /// Validate a ZFS pool
    pub async fn validate_pool(&self, pool: &StoragePool) -> Result<()> {
        // Check if ZFS pool exists
        let output = Command::new("zfs")
            .arg("list")
            .arg("-H")
            .arg(&pool.path)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to check ZFS pool: {}", e))
            })?;

        if !output.status.success() {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "ZFS pool {} does not exist",
                pool.path
            )));
        }

        Ok(())
    }

    /// Create a ZFS volume (zvol)
    pub async fn create_volume(
        &self,
        pool_path: &str,
        volume_name: &str,
        size_gb: u64,
    ) -> Result<String> {
        info!("Creating ZFS volume: {}/{} ({}GB)", pool_path, volume_name, size_gb);

        let volume_path = format!("{}/{}", pool_path, volume_name);

        // Create zvol
        let output = Command::new("zfs")
            .arg("create")
            .arg("-V")
            .arg(format!("{}G", size_gb))
            .arg(&volume_path)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to create ZFS volume: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Failed to create ZFS volume: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create ZFS volume: {}",
                stderr
            )));
        }

        // Return device path
        Ok(format!("/dev/zvol/{}", volume_path))
    }

    /// Delete a ZFS volume
    pub async fn delete_volume(&self, pool_path: &str, volume_name: &str) -> Result<()> {
        info!("Deleting ZFS volume: {}/{}", pool_path, volume_name);

        let volume_path = format!("{}/{}", pool_path, volume_name);

        let output = Command::new("zfs")
            .arg("destroy")
            .arg(&volume_path)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to delete ZFS volume: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Failed to delete ZFS volume: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to delete ZFS volume: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Create a ZFS snapshot
    pub async fn create_snapshot(
        &self,
        pool_path: &str,
        volume_name: &str,
        snapshot_name: &str,
    ) -> Result<()> {
        info!(
            "Creating ZFS snapshot: {}/{}@{}",
            pool_path, volume_name, snapshot_name
        );

        let snapshot_path = format!("{}/{}@{}", pool_path, volume_name, snapshot_name);

        let output = Command::new("zfs")
            .arg("snapshot")
            .arg(&snapshot_path)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to create ZFS snapshot: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Failed to create ZFS snapshot: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create ZFS snapshot: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Restore a ZFS snapshot
    pub async fn restore_snapshot(
        &self,
        pool_path: &str,
        volume_name: &str,
        snapshot_name: &str,
    ) -> Result<()> {
        info!(
            "Restoring ZFS snapshot: {}/{}@{}",
            pool_path, volume_name, snapshot_name
        );

        let snapshot_path = format!("{}/{}@{}", pool_path, volume_name, snapshot_name);

        let output = Command::new("zfs")
            .arg("rollback")
            .arg(&snapshot_path)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to restore ZFS snapshot: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Failed to restore ZFS snapshot: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to restore ZFS snapshot: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Clone a ZFS snapshot
    pub async fn clone_snapshot(
        &self,
        pool_path: &str,
        volume_name: &str,
        snapshot_name: &str,
        clone_name: &str,
    ) -> Result<String> {
        info!(
            "Cloning ZFS snapshot: {}/{}@{} -> {}",
            pool_path, volume_name, snapshot_name, clone_name
        );

        let snapshot_path = format!("{}/{}@{}", pool_path, volume_name, snapshot_name);
        let clone_path = format!("{}/{}", pool_path, clone_name);

        let output = Command::new("zfs")
            .arg("clone")
            .arg(&snapshot_path)
            .arg(&clone_path)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to clone ZFS snapshot: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Failed to clone ZFS snapshot: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to clone ZFS snapshot: {}",
                stderr
            )));
        }

        Ok(format!("/dev/zvol/{}", clone_path))
    }

    /// Check if ZFS is available
    pub fn check_zfs_available() -> bool {
        std::process::Command::new("zfs")
            .arg("version")
            .output()
            .is_ok()
    }

    /// Get ZFS version
    pub async fn get_zfs_version() -> Result<String> {
        let output = Command::new("zfs")
            .arg("version")
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to get ZFS version: {}", e))
            })?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(
                "ZFS not found or not working".to_string(),
            ));
        }

        let version = String::from_utf8_lossy(&output.stdout);
        Ok(version.lines().next().unwrap_or("Unknown").to_string())
    }
}
