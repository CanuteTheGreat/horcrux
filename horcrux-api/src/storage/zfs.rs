//! ZFS storage backend
//! Provides ZFS pool and volume management with snapshots and clones

#![allow(dead_code)]

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

    /// List all ZFS pools
    pub async fn list_pools(&self) -> Result<Vec<ZfsPoolInfo>> {
        let output = Command::new("zpool")
            .arg("list")
            .arg("-H")
            .arg("-o")
            .arg("name,size,alloc,free,health,cap")
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to list ZFS pools: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to list ZFS pools: {}",
                stderr
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let pools = stdout.lines()
            .filter_map(|line| {
                let parts: Vec<_> = line.split('\t').collect();
                if parts.len() >= 6 {
                    Some(ZfsPoolInfo {
                        name: parts[0].to_string(),
                        size: parts[1].to_string(),
                        allocated: parts[2].to_string(),
                        free: parts[3].to_string(),
                        health: parts[4].to_string(),
                        capacity_percent: parts[5].trim_end_matches('%').parse().unwrap_or(0),
                    })
                } else {
                    None
                }
            })
            .collect();

        Ok(pools)
    }

    /// Get pool status and health details
    pub async fn get_pool_status(&self, pool_name: &str) -> Result<String> {
        let output = Command::new("zpool")
            .arg("status")
            .arg(pool_name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to get pool status: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to get pool status: {}",
                stderr
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Start a scrub on a ZFS pool
    pub async fn start_scrub(&self, pool_name: &str) -> Result<()> {
        info!("Starting scrub on pool: {}", pool_name);

        let output = Command::new("zpool")
            .arg("scrub")
            .arg(pool_name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to start scrub: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to start scrub: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Stop a running scrub
    pub async fn stop_scrub(&self, pool_name: &str) -> Result<()> {
        info!("Stopping scrub on pool: {}", pool_name);

        let output = Command::new("zpool")
            .arg("scrub")
            .arg("-s")
            .arg(pool_name)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to stop scrub: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to stop scrub: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// List snapshots for a volume
    pub async fn list_snapshots(&self, volume_path: &str) -> Result<Vec<ZfsSnapshotInfo>> {
        let output = Command::new("zfs")
            .arg("list")
            .arg("-t")
            .arg("snapshot")
            .arg("-H")
            .arg("-r")
            .arg("-o")
            .arg("name,used,refer,creation")
            .arg(volume_path)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to list snapshots: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to list snapshots: {}",
                stderr
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let snapshots = stdout.lines()
            .filter_map(|line| {
                let parts: Vec<_> = line.split('\t').collect();
                if parts.len() >= 4 {
                    let name = parts[0].to_string();
                    let snapshot_name = name.split('@').last()
                        .unwrap_or(&name)
                        .to_string();

                    Some(ZfsSnapshotInfo {
                        full_name: name,
                        name: snapshot_name,
                        used: parts[1].to_string(),
                        referenced: parts[2].to_string(),
                        creation: parts[3].to_string(),
                    })
                } else {
                    None
                }
            })
            .collect();

        Ok(snapshots)
    }

    /// Delete a snapshot
    pub async fn delete_snapshot(
        &self,
        pool_path: &str,
        volume_name: &str,
        snapshot_name: &str,
    ) -> Result<()> {
        info!(
            "Deleting ZFS snapshot: {}/{}@{}",
            pool_path, volume_name, snapshot_name
        );

        let snapshot_path = format!("{}/{}@{}", pool_path, volume_name, snapshot_name);

        let output = Command::new("zfs")
            .arg("destroy")
            .arg(&snapshot_path)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to delete snapshot: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to delete snapshot: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Send a snapshot to stdout (for piping to another host)
    pub async fn send_snapshot(
        &self,
        snapshot_path: &str,
        incremental_base: Option<&str>,
    ) -> Result<tokio::process::Child> {
        info!("Sending ZFS snapshot: {}", snapshot_path);

        let mut cmd = Command::new("zfs");
        cmd.arg("send");

        if let Some(base) = incremental_base {
            cmd.arg("-i").arg(base);
        }

        cmd.arg(snapshot_path)
            .stdout(std::process::Stdio::piped());

        let child = cmd.spawn().map_err(|e| {
            horcrux_common::Error::System(format!("Failed to start ZFS send: {}", e))
        })?;

        Ok(child)
    }

    /// Receive a snapshot from stdin
    pub async fn receive_snapshot(&self, target_path: &str) -> Result<tokio::process::Child> {
        info!("Receiving ZFS snapshot to: {}", target_path);

        let child = Command::new("zfs")
            .arg("receive")
            .arg("-F")
            .arg(target_path)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to start ZFS receive: {}", e))
            })?;

        Ok(child)
    }

    /// Resize a volume
    pub async fn resize_volume(
        &self,
        pool_path: &str,
        volume_name: &str,
        new_size_gb: u64,
    ) -> Result<()> {
        info!(
            "Resizing ZFS volume: {}/{} to {}GB",
            pool_path, volume_name, new_size_gb
        );

        let volume_path = format!("{}/{}", pool_path, volume_name);

        let output = Command::new("zfs")
            .arg("set")
            .arg(format!("volsize={}G", new_size_gb))
            .arg(&volume_path)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to resize volume: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to resize volume: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Get volume properties
    pub async fn get_volume_properties(&self, volume_path: &str) -> Result<ZfsVolumeProperties> {
        let output = Command::new("zfs")
            .arg("get")
            .arg("-H")
            .arg("-p")
            .arg("volsize,used,available,compression,compressratio,referenced")
            .arg(volume_path)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to get volume properties: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to get volume properties: {}",
                stderr
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut props = ZfsVolumeProperties::default();

        for line in stdout.lines() {
            let parts: Vec<_> = line.split('\t').collect();
            if parts.len() >= 3 {
                match parts[1] {
                    "volsize" => props.volsize = parts[2].parse().unwrap_or(0),
                    "used" => props.used = parts[2].parse().unwrap_or(0),
                    "available" => props.available = parts[2].parse().unwrap_or(0),
                    "compression" => props.compression = parts[2].to_string(),
                    "compressratio" => props.compress_ratio = parts[2].to_string(),
                    "referenced" => props.referenced = parts[2].parse().unwrap_or(0),
                    _ => {}
                }
            }
        }

        Ok(props)
    }

    /// Enable compression on a volume
    pub async fn set_compression(
        &self,
        volume_path: &str,
        compression: &str, // lz4, zstd, gzip, off
    ) -> Result<()> {
        info!("Setting compression {} on: {}", compression, volume_path);

        let output = Command::new("zfs")
            .arg("set")
            .arg(format!("compression={}", compression))
            .arg(volume_path)
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to set compression: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to set compression: {}",
                stderr
            )));
        }

        Ok(())
    }
}

/// ZFS pool information
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ZfsPoolInfo {
    pub name: String,
    pub size: String,
    pub allocated: String,
    pub free: String,
    pub health: String,
    pub capacity_percent: u32,
}

/// ZFS snapshot information
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ZfsSnapshotInfo {
    pub full_name: String,
    pub name: String,
    pub used: String,
    pub referenced: String,
    pub creation: String,
}

/// ZFS volume properties
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ZfsVolumeProperties {
    pub volsize: u64,
    pub used: u64,
    pub available: u64,
    pub compression: String,
    pub compress_ratio: String,
    pub referenced: u64,
}
