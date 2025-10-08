///! Directory-based storage backend
///! Simple file-based storage using qcow2 or raw files

use super::StoragePool;
use horcrux_common::Result;
use tokio::process::Command;
use tracing::{error, info};

/// Directory storage manager
pub struct DirectoryManager {}

impl DirectoryManager {
    pub fn new() -> Self {
        Self {}
    }

    /// Validate a directory storage path
    pub async fn validate_pool(&self, pool: &StoragePool) -> Result<()> {
        // Check if directory exists
        if !tokio::fs::metadata(&pool.path).await.is_ok() {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Directory {} does not exist",
                pool.path
            )));
        }

        Ok(())
    }

    /// Create a qcow2 image file
    pub async fn create_volume(
        &self,
        dir_path: &str,
        volume_name: &str,
        size_gb: u64,
    ) -> Result<String> {
        info!("Creating directory volume: {}/{}.qcow2 ({}GB)", dir_path, volume_name, size_gb);

        let volume_path = format!("{}/{}.qcow2", dir_path, volume_name);

        let output = Command::new("qemu-img")
            .arg("create")
            .arg("-f")
            .arg("qcow2")
            .arg(&volume_path)
            .arg(format!("{}G", size_gb))
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to create volume: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Failed to create volume: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to create volume: {}",
                stderr
            )));
        }

        Ok(volume_path)
    }

    /// Delete a volume file
    pub async fn delete_volume(&self, dir_path: &str, volume_name: &str) -> Result<()> {
        info!("Deleting directory volume: {}/{}.qcow2", dir_path, volume_name);

        let volume_path = format!("{}/{}.qcow2", dir_path, volume_name);

        tokio::fs::remove_file(&volume_path).await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to delete volume: {}", e))
        })?;

        Ok(())
    }

    /// Check if directory storage is available
    pub fn check_directory_available() -> bool {
        // Directory storage just needs qemu-img
        std::process::Command::new("qemu-img")
            .arg("--version")
            .output()
            .is_ok()
    }
}
