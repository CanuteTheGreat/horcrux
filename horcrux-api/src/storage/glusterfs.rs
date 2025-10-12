//! GlusterFS distributed storage backend
//!
//! Provides GlusterFS volume support with native replication and distribution

#![allow(dead_code)]

use horcrux_common::Result;
use tokio::process::Command as AsyncCommand;
use serde::{Deserialize, Serialize};
use super::StoragePool;

/// GlusterFS manager
pub struct GlusterFsManager {}

/// GlusterFS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlusterFsConfig {
    pub server: String,          // GlusterFS server hostname/IP
    pub volume: String,          // GlusterFS volume name
    pub path: String,            // Path within volume
    pub transport: Transport,    // Transport protocol
    pub backup_volfile_servers: Vec<String>,  // Backup servers for redundancy
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Transport {
    Tcp,
    Rdma,
    TcpRdma,
}

impl Transport {
    fn as_str(&self) -> &str {
        match self {
            Transport::Tcp => "tcp",
            Transport::Rdma => "rdma",
            Transport::TcpRdma => "tcp,rdma",
        }
    }
}

/// GlusterFS volume type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VolumeType {
    Distribute,      // Distributed (no redundancy)
    Replicate,       // Replicated (mirrored)
    DistributeReplicate,  // Distributed-Replicate
    Disperse,        // Erasure coded
}

/// GlusterFS volume info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlusterFsVolumeInfo {
    pub name: String,
    pub volume_type: VolumeType,
    pub status: String,
    pub brick_count: usize,
    pub replica_count: Option<usize>,
    pub disperse_count: Option<usize>,
    pub bricks: Vec<String>,
}

impl GlusterFsManager {
    pub fn new() -> Self {
        Self {}
    }

    /// Validate GlusterFS pool configuration
    pub async fn validate_pool(&self, pool: &StoragePool) -> Result<()> {
        if pool.path.is_empty() {
            return Err(horcrux_common::Error::System(
                "GlusterFS volume path is required".to_string()
            ));
        }

        Ok(())
    }

    /// Mount GlusterFS volume
    pub async fn mount_volume(&self, config: &GlusterFsConfig, mount_point: &str) -> Result<()> {
        tracing::info!(
            "Mounting GlusterFS volume {}:{} to {}",
            config.server, config.volume, mount_point
        );

        // Create mount point if it doesn't exist
        tokio::fs::create_dir_all(mount_point).await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to create mount point: {}", e))
        })?;

        // Build mount options
        let mut opts = vec![
            format!("transport={}", config.transport.as_str()),
            "log-level=WARNING".to_string(),
        ];

        // Add backup volfile servers for HA
        if !config.backup_volfile_servers.is_empty() {
            opts.push(format!(
                "backup-volfile-servers={}",
                config.backup_volfile_servers.join(":")
            ));
        }

        let options = opts.join(",");

        // Mount using glusterfs FUSE client
        let output = AsyncCommand::new("mount")
            .args(&[
                "-t", "glusterfs",
                "-o", &options,
                &format!("{}:/{}", config.server, config.volume),
                mount_point,
            ])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Mount command failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(
                format!("Failed to mount GlusterFS: {}", stderr)
            ));
        }

        tracing::info!("Successfully mounted GlusterFS volume");

        // Test mount by creating a test file
        self.test_mount(mount_point).await?;

        Ok(())
    }

    /// Unmount GlusterFS volume
    pub async fn unmount_volume(&self, mount_point: &str) -> Result<()> {
        tracing::info!("Unmounting GlusterFS volume from {}", mount_point);

        let output = AsyncCommand::new("umount")
            .arg(mount_point)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Unmount failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(
                format!("Failed to unmount: {}", stderr)
            ));
        }

        Ok(())
    }

    /// Test mount by creating a test file
    async fn test_mount(&self, mount_point: &str) -> Result<()> {
        use std::path::PathBuf;

        let test_file = PathBuf::from(mount_point).join(".horcrux_test");

        tokio::fs::write(&test_file, b"test").await.map_err(|e| {
            horcrux_common::Error::System(format!("Mount test write failed: {}", e))
        })?;

        tokio::fs::remove_file(&test_file).await.map_err(|e| {
            horcrux_common::Error::System(format!("Mount test cleanup failed: {}", e))
        })?;

        Ok(())
    }

    /// Get GlusterFS volume information
    pub async fn get_volume_info(&self, server: &str, volume: &str) -> Result<GlusterFsVolumeInfo> {
        let output = AsyncCommand::new("gluster")
            .args(&["--remote-host", server, "volume", "info", volume])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to get volume info: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(
                format!("Volume info failed: {}", stderr)
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        self.parse_volume_info(volume, &stdout)
    }

    /// Parse gluster volume info output
    fn parse_volume_info(&self, volume_name: &str, output: &str) -> Result<GlusterFsVolumeInfo> {
        let mut volume_type = VolumeType::Distribute;
        let mut status = String::new();
        let mut brick_count = 0;
        let replica_count = None;
        let disperse_count = None;
        let mut bricks = Vec::new();

        for line in output.lines() {
            let line = line.trim();

            if line.starts_with("Type:") {
                let type_str = line.split(':').nth(1).unwrap_or("").trim();
                volume_type = match type_str {
                    "Distribute" => VolumeType::Distribute,
                    "Replicate" => VolumeType::Replicate,
                    "Distributed-Replicate" => VolumeType::DistributeReplicate,
                    "Disperse" => VolumeType::Disperse,
                    _ => VolumeType::Distribute,
                };
            } else if line.starts_with("Status:") {
                status = line.split(':').nth(1).unwrap_or("").trim().to_string();
            } else if line.starts_with("Number of Bricks:") {
                let count_str = line.split(':').nth(1).unwrap_or("0").trim();
                // Handle formats like "Number of Bricks: 2 x 2 = 4"
                let parts: Vec<&str> = count_str.split('=').collect();
                if parts.len() > 1 {
                    brick_count = parts[1].trim().parse().unwrap_or(0);
                } else {
                    brick_count = count_str.parse().unwrap_or(0);
                }
            } else if line.starts_with("Brick") {
                if let Some(brick_path) = line.split(':').nth(1) {
                    bricks.push(brick_path.trim().to_string());
                }
            }
        }

        Ok(GlusterFsVolumeInfo {
            name: volume_name.to_string(),
            volume_type,
            status,
            brick_count,
            replica_count,
            disperse_count,
            bricks,
        })
    }

    /// Create VM volume on GlusterFS
    pub async fn create_volume(
        &self,
        mount_point: &str,
        volume_name: &str,
        size_gb: u64,
    ) -> Result<String> {
        use std::path::PathBuf;

        tracing::info!("Creating {}GB volume {} on GlusterFS", size_gb, volume_name);

        let volume_path = PathBuf::from(mount_point).join(format!("{}.qcow2", volume_name));

        // Create qcow2 image
        let output = AsyncCommand::new("qemu-img")
            .args(&[
                "create",
                "-f", "qcow2",
                volume_path.to_str().unwrap(),
                &format!("{}G", size_gb),
            ])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to create volume: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(
                format!("Volume creation failed: {}", stderr)
            ));
        }

        Ok(volume_path.to_string_lossy().to_string())
    }

    /// Delete volume
    pub async fn delete_volume(&self, volume_path: &str) -> Result<()> {
        tracing::info!("Deleting GlusterFS volume: {}", volume_path);

        tokio::fs::remove_file(volume_path).await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to delete volume: {}", e))
        })?;

        Ok(())
    }

    /// Create snapshot
    pub async fn create_snapshot(
        &self,
        volume_path: &str,
        snapshot_name: &str,
    ) -> Result<String> {
        tracing::info!("Creating snapshot {} of {}", snapshot_name, volume_path);

        // Use qcow2 internal snapshots
        let output = AsyncCommand::new("qemu-img")
            .args(&[
                "snapshot",
                "-c", snapshot_name,
                volume_path,
            ])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Snapshot creation failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(
                format!("Failed to create snapshot: {}", stderr)
            ));
        }

        Ok(snapshot_name.to_string())
    }

    /// List GlusterFS volumes on server
    pub async fn list_volumes(server: &str) -> Result<Vec<String>> {
        let output = AsyncCommand::new("gluster")
            .args(&["--remote-host", server, "volume", "list"])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to list volumes: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(
                format!("Volume list failed: {}", stderr)
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let volumes = stdout
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();

        Ok(volumes)
    }

    /// Check if GlusterFS is available
    pub fn check_glusterfs_available() -> bool {
        std::process::Command::new("gluster")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Get GlusterFS version
    pub async fn get_glusterfs_version() -> Result<String> {
        let output = AsyncCommand::new("gluster")
            .arg("--version")
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to get version: {}", e)))?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System("Failed to get GlusterFS version".to_string()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let version = stdout
            .lines()
            .next()
            .unwrap_or("unknown")
            .to_string();

        Ok(version)
    }
}

impl GlusterFsConfig {
    /// Parse GlusterFS path format: server:/volume/path
    pub fn parse(path: &str) -> Result<Self> {
        let parts: Vec<&str> = path.split(':').collect();

        if parts.len() != 2 {
            return Err(horcrux_common::Error::System(
                "Invalid GlusterFS path format. Expected: server:/volume/path".to_string()
            ));
        }

        let server = parts[0].to_string();
        let volume_path = parts[1].to_string();

        // Extract volume name (first component of path)
        let volume_parts: Vec<&str> = volume_path.trim_start_matches('/').split('/').collect();
        let volume = volume_parts[0].to_string();
        let path = if volume_parts.len() > 1 {
            "/".to_string() + &volume_parts[1..].join("/")
        } else {
            "/".to_string()
        };

        Ok(Self {
            server,
            volume,
            path,
            transport: Transport::Tcp,
            backup_volfile_servers: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_glusterfs_path() {
        let config = GlusterFsConfig::parse("gluster1:/vmdata/vms").unwrap();
        assert_eq!(config.server, "gluster1");
        assert_eq!(config.volume, "vmdata");
        assert_eq!(config.path, "/vms");
    }

    #[test]
    fn test_parse_glusterfs_path_root() {
        let config = GlusterFsConfig::parse("gluster1:/vmdata").unwrap();
        assert_eq!(config.server, "gluster1");
        assert_eq!(config.volume, "vmdata");
        assert_eq!(config.path, "/");
    }

    #[test]
    fn test_invalid_glusterfs_path() {
        let result = GlusterFsConfig::parse("invalid-path");
        assert!(result.is_err());
    }
}
