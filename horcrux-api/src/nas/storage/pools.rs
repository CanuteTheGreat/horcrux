//! Storage pool management
//!
//! Handles creation, destruction, and management of storage pools
//! (ZFS pools, Btrfs volumes, mdraid arrays).

use horcrux_common::{Error, Result};
use crate::nas::storage::{NasPool, PoolHealth, RaidLevel, StorageType};
use std::collections::HashMap;
use tokio::process::Command;

/// List all ZFS pools
#[cfg(feature = "nas-zfs")]
pub async fn list_zfs_pools() -> Result<Vec<NasPool>> {
    let output = Command::new("zpool")
        .args([
            "list", "-H", "-o", "name,size,alloc,free,health,altroot",
        ])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("zpool list failed: {}", e)))?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut pools = Vec::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 5 {
            let name = parts[0].to_string();
            let total_bytes = super::parse_size(parts[1]).unwrap_or(0);
            let used_bytes = super::parse_size(parts[2]).unwrap_or(0);
            let available_bytes = super::parse_size(parts[3]).unwrap_or(0);
            let health = parse_zpool_health(parts[4]);

            // Get more details about the pool
            let (devices, raid_level) = get_zpool_vdevs(&name).await.unwrap_or_default();
            let properties = get_zpool_properties(&name).await.unwrap_or_default();

            let mount_path = format!("/{}", name);

            pools.push(NasPool {
                id: name.clone(),
                name,
                storage_type: StorageType::Zfs,
                raid_level,
                devices,
                mount_path,
                total_bytes,
                used_bytes,
                available_bytes,
                health,
                online: health == PoolHealth::Online,
                compression: properties.get("compression").cloned(),
                dedup: properties.get("dedup").map(|v| v == "on").unwrap_or(false),
                properties,
                created_at: 0, // Would need to parse from zpool history
            });
        }
    }

    Ok(pools)
}

/// Parse ZFS pool health status
fn parse_zpool_health(health: &str) -> PoolHealth {
    match health.to_uppercase().as_str() {
        "ONLINE" => PoolHealth::Online,
        "DEGRADED" => PoolHealth::Degraded,
        "FAULTED" => PoolHealth::Faulted,
        "OFFLINE" => PoolHealth::Offline,
        "UNAVAIL" => PoolHealth::Unavailable,
        _ => PoolHealth::Unknown,
    }
}

/// Get ZFS pool vdevs and determine RAID level
#[cfg(feature = "nas-zfs")]
async fn get_zpool_vdevs(pool: &str) -> Result<(Vec<String>, RaidLevel)> {
    let output = Command::new("zpool")
        .args(["status", "-P", pool])
        .output()
        .await?;

    if !output.status.success() {
        return Ok((Vec::new(), RaidLevel::Single));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut devices = Vec::new();
    let mut raid_level = RaidLevel::Single;
    let mut in_config = false;

    for line in stdout.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("config:") {
            in_config = true;
            continue;
        }

        if in_config {
            if trimmed.starts_with("errors:") {
                break;
            }

            // Detect RAID level from vdev type
            if trimmed.starts_with("raidz3") {
                raid_level = RaidLevel::RaidZ3;
            } else if trimmed.starts_with("raidz2") {
                raid_level = RaidLevel::RaidZ2;
            } else if trimmed.starts_with("raidz") {
                raid_level = RaidLevel::RaidZ;
            } else if trimmed.starts_with("mirror") {
                raid_level = RaidLevel::Mirror;
            }

            // Extract device paths (lines starting with /)
            if trimmed.starts_with('/') {
                let device = trimmed.split_whitespace().next().unwrap_or(trimmed);
                devices.push(device.to_string());
            }
        }
    }

    Ok((devices, raid_level))
}

/// Get ZFS pool properties
#[cfg(feature = "nas-zfs")]
async fn get_zpool_properties(pool: &str) -> Result<HashMap<String, String>> {
    let output = Command::new("zpool")
        .args(["get", "all", "-H", "-o", "property,value", pool])
        .output()
        .await?;

    let mut properties = HashMap::new();

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 2 {
                properties.insert(parts[0].to_string(), parts[1].to_string());
            }
        }
    }

    Ok(properties)
}

/// Create a new ZFS pool
#[cfg(feature = "nas-zfs")]
pub async fn create_zfs_pool(
    name: &str,
    raid_level: RaidLevel,
    devices: &[String],
) -> Result<NasPool> {
    // Validate inputs
    if name.is_empty() {
        return Err(Error::Validation("Pool name cannot be empty".to_string()));
    }
    if devices.is_empty() {
        return Err(Error::Validation("At least one device is required".to_string()));
    }

    // Build zpool create command
    let mut args = vec!["create".to_string()];

    // Add mount options
    args.push("-o".to_string());
    args.push("ashift=12".to_string()); // Optimal for modern drives

    // Add default dataset properties
    args.push("-O".to_string());
    args.push("compression=lz4".to_string());
    args.push("-O".to_string());
    args.push("atime=off".to_string());
    args.push("-O".to_string());
    args.push("xattr=sa".to_string());

    args.push(name.to_string());

    // Add vdev type based on RAID level
    match raid_level {
        RaidLevel::RaidZ => {
            args.push("raidz".to_string());
        }
        RaidLevel::RaidZ2 => {
            args.push("raidz2".to_string());
        }
        RaidLevel::RaidZ3 => {
            args.push("raidz3".to_string());
        }
        RaidLevel::Mirror => {
            args.push("mirror".to_string());
        }
        RaidLevel::Single | RaidLevel::Raid0 => {
            // No prefix for stripe/single
        }
        _ => {
            return Err(Error::Validation(format!(
                "RAID level {:?} not supported for ZFS",
                raid_level
            )));
        }
    }

    // Add devices
    for device in devices {
        args.push(device.clone());
    }

    let output = Command::new("zpool")
        .args(&args)
        .output()
        .await
        .map_err(|e| Error::Internal(format!("zpool create failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Internal(format!(
            "zpool create failed: {}",
            stderr
        )));
    }

    // Return the created pool
    let pools = list_zfs_pools().await?;
    pools
        .into_iter()
        .find(|p| p.name == name)
        .ok_or_else(|| Error::Internal("Pool created but not found".to_string()))
}

/// Destroy a ZFS pool
#[cfg(feature = "nas-zfs")]
pub async fn destroy_zfs_pool(name: &str) -> Result<()> {
    let output = Command::new("zpool")
        .args(["destroy", "-f", name])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("zpool destroy failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Internal(format!(
            "zpool destroy failed: {}",
            stderr
        )));
    }

    Ok(())
}

/// Start a ZFS pool scrub
#[cfg(feature = "nas-zfs")]
pub async fn scrub_zfs_pool(name: &str) -> Result<()> {
    let output = Command::new("zpool")
        .args(["scrub", name])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("zpool scrub failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Internal(format!(
            "zpool scrub failed: {}",
            stderr
        )));
    }

    Ok(())
}

/// Cancel a ZFS pool scrub
#[cfg(feature = "nas-zfs")]
pub async fn cancel_scrub(name: &str) -> Result<()> {
    let output = Command::new("zpool")
        .args(["scrub", "-s", name])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("zpool scrub cancel failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Internal(format!(
            "zpool scrub cancel failed: {}",
            stderr
        )));
    }

    Ok(())
}

/// Get ZFS pool scrub status
#[cfg(feature = "nas-zfs")]
pub async fn get_scrub_status(name: &str) -> Result<Option<ScrubStatus>> {
    let output = Command::new("zpool")
        .args(["status", name])
        .output()
        .await?;

    if !output.status.success() {
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse scrub status from zpool status output
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.contains("scrub in progress") {
            // Parse progress from line like "scan: scrub in progress since..."
            return Ok(Some(ScrubStatus {
                in_progress: true,
                progress_percent: 0.0, // Would need more parsing
                estimated_time_remaining: None,
                errors: 0,
            }));
        }
    }

    Ok(None)
}

/// Scrub status
pub struct ScrubStatus {
    pub in_progress: bool,
    pub progress_percent: f64,
    pub estimated_time_remaining: Option<u64>,
    pub errors: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_health() {
        assert_eq!(parse_zpool_health("ONLINE"), PoolHealth::Online);
        assert_eq!(parse_zpool_health("DEGRADED"), PoolHealth::Degraded);
        assert_eq!(parse_zpool_health("FAULTED"), PoolHealth::Faulted);
        assert_eq!(parse_zpool_health("unknown"), PoolHealth::Unknown);
    }
}
