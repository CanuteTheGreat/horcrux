///! Directory-based storage backend
///! Simple file-based storage using qcow2 or raw files

#![allow(dead_code)]

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
        use std::path::Path;

        let path = Path::new(&pool.path);

        // 1. Verify path exists
        let metadata = tokio::fs::metadata(&pool.path)
            .await
            .map_err(|e| horcrux_common::Error::InvalidConfig(
                format!("Path does not exist or is not accessible: {} - {}", pool.path, e)
            ))?;

        // 2. Verify it's a directory
        if !metadata.is_dir() {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Path is not a directory: {}",
                pool.path
            )));
        }

        // 3. Verify write permissions by creating and deleting a test file
        let test_file = path.join(".horcrux_test");
        tokio::fs::write(&test_file, b"test")
            .await
            .map_err(|e| horcrux_common::Error::InvalidConfig(
                format!("No write permission for directory {}: {}", pool.path, e)
            ))?;

        tokio::fs::remove_file(&test_file)
            .await
            .map_err(|e| horcrux_common::Error::InvalidConfig(
                format!("Failed to clean up test file: {}", e)
            ))?;

        // 4. Verify qemu-img is available (needed for creating volumes)
        if !Self::check_directory_available() {
            return Err(horcrux_common::Error::InvalidConfig(
                "qemu-img not found. Directory storage requires qemu-img to be installed.".to_string()
            ));
        }

        info!("Directory storage validation passed for: {}", pool.path);
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
