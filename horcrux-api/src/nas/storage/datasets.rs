//! Dataset management
//!
//! Handles ZFS datasets, Btrfs subvolumes, and LVM volumes.

use horcrux_common::{Error, Result};
use crate::nas::storage::{DatasetType, NasDataset};
use std::collections::HashMap;
use tokio::process::Command;

/// List datasets in a pool
pub async fn list_datasets(pool: &str) -> Result<Vec<NasDataset>> {
    #[cfg(feature = "nas-zfs")]
    {
        list_zfs_datasets(pool).await
    }

    #[cfg(not(feature = "nas-zfs"))]
    {
        let _ = pool;
        Ok(Vec::new())
    }
}

/// List ZFS datasets
#[cfg(feature = "nas-zfs")]
async fn list_zfs_datasets(pool: &str) -> Result<Vec<NasDataset>> {
    let output = Command::new("zfs")
        .args([
            "list", "-H", "-r", "-t", "filesystem,volume",
            "-o", "name,used,refer,avail,quota,refquota,compression,recordsize,atime,sync,mountpoint",
            pool,
        ])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("zfs list failed: {}", e)))?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut datasets = Vec::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 11 {
            let full_name = parts[0].to_string();
            let name = full_name.rsplit('/').next().unwrap_or(&full_name).to_string();

            let used_bytes = super::parse_size(parts[1]).unwrap_or(0);
            let referenced_bytes = super::parse_size(parts[2]).unwrap_or(0);
            let available_bytes = super::parse_size(parts[3]).unwrap_or(0);

            let quota_bytes = if parts[4] != "none" && parts[4] != "-" {
                super::parse_size(parts[4])
            } else {
                None
            };

            let refquota_bytes = if parts[5] != "none" && parts[5] != "-" {
                super::parse_size(parts[5])
            } else {
                None
            };

            let compression = if parts[6] != "off" {
                Some(parts[6].to_string())
            } else {
                None
            };

            let recordsize = parts[7].parse().ok();
            let atime = parts[8] == "on";
            let sync = parts[9].to_string();
            let mount_path = parts[10].to_string();

            datasets.push(NasDataset {
                id: full_name.replace('/', "_"),
                name,
                full_name,
                pool_id: pool.to_string(),
                mount_path,
                dataset_type: DatasetType::Filesystem,
                used_bytes,
                referenced_bytes,
                available_bytes,
                quota_bytes,
                refquota_bytes,
                compression,
                recordsize,
                atime,
                sync,
                properties: HashMap::new(),
                created_at: 0,
            });
        }
    }

    Ok(datasets)
}

/// Create a new dataset
pub async fn create_dataset(pool: &str, name: &str) -> Result<NasDataset> {
    #[cfg(feature = "nas-zfs")]
    {
        create_zfs_dataset(pool, name).await
    }

    #[cfg(not(feature = "nas-zfs"))]
    {
        let _ = (pool, name);
        Err(Error::Internal("ZFS not enabled".to_string()))
    }
}

/// Create a ZFS dataset
#[cfg(feature = "nas-zfs")]
async fn create_zfs_dataset(pool: &str, name: &str) -> Result<NasDataset> {
    let full_name = format!("{}/{}", pool, name);

    let output = Command::new("zfs")
        .args(["create", &full_name])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("zfs create failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Internal(format!(
            "zfs create failed: {}",
            stderr
        )));
    }

    // Get the created dataset
    let datasets = list_zfs_datasets(pool).await?;
    datasets
        .into_iter()
        .find(|d| d.full_name == full_name)
        .ok_or_else(|| Error::Internal("Dataset created but not found".to_string()))
}

/// Destroy a dataset
pub async fn destroy_dataset(dataset: &str) -> Result<()> {
    #[cfg(feature = "nas-zfs")]
    {
        let output = Command::new("zfs")
            .args(["destroy", "-r", dataset])
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
        let _ = dataset;
    }

    Ok(())
}

/// Set dataset property
pub async fn set_property(dataset: &str, property: &str, value: &str) -> Result<()> {
    #[cfg(feature = "nas-zfs")]
    {
        let output = Command::new("zfs")
            .args(["set", &format!("{}={}", property, value), dataset])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("zfs set failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "zfs set failed: {}",
                stderr
            )));
        }
    }

    #[cfg(not(feature = "nas-zfs"))]
    {
        let _ = (dataset, property, value);
    }

    Ok(())
}

/// Set dataset quota
pub async fn set_quota(dataset: &str, quota_bytes: u64) -> Result<()> {
    set_property(dataset, "quota", &super::format_size(quota_bytes)).await
}

/// Set dataset refquota
pub async fn set_refquota(dataset: &str, quota_bytes: u64) -> Result<()> {
    set_property(dataset, "refquota", &super::format_size(quota_bytes)).await
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_dataset_names() {
        // Basic test for dataset naming conventions
        let full_name = "tank/data/share1";
        let name = full_name.rsplit('/').next().unwrap();
        assert_eq!(name, "share1");
    }
}
