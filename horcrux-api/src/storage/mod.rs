///! Storage backend management
///! Supports ZFS, Ceph, LVM, iSCSI, and directory-based storage

pub mod zfs;
pub mod ceph;
pub mod lvm;
pub mod iscsi;
pub mod directory;

use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Storage backend type
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageType {
    Zfs,
    Ceph,
    Lvm,
    Iscsi,
    Directory,
}

/// Storage pool configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoragePool {
    pub id: String,
    pub name: String,
    pub storage_type: StorageType,
    pub path: String,
    pub available: u64,  // Available space in GB
    pub total: u64,      // Total space in GB
    pub enabled: bool,
}

/// Storage manager
pub struct StorageManager {
    pools: Arc<RwLock<HashMap<String, StoragePool>>>,
    zfs: zfs::ZfsManager,
    ceph: ceph::CephManager,
    lvm: lvm::LvmManager,
    directory: directory::DirectoryManager,
}

impl StorageManager {
    pub fn new() -> Self {
        Self {
            pools: Arc::new(RwLock::new(HashMap::new())),
            zfs: zfs::ZfsManager::new(),
            ceph: ceph::CephManager::new(),
            lvm: lvm::LvmManager::new(),
            directory: directory::DirectoryManager::new(),
        }
    }

    /// List all storage pools
    pub async fn list_pools(&self) -> Vec<StoragePool> {
        let pools = self.pools.read().await;
        pools.values().cloned().collect()
    }

    /// Get a specific storage pool
    pub async fn get_pool(&self, id: &str) -> Result<StoragePool> {
        let pools = self.pools.read().await;
        pools
            .get(id)
            .cloned()
            .ok_or_else(|| horcrux_common::Error::System(format!("Storage pool {} not found", id)))
    }

    /// Add a storage pool
    pub async fn add_pool(&self, pool: StoragePool) -> Result<StoragePool> {
        let mut pools = self.pools.write().await;

        if pools.contains_key(&pool.id) {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Storage pool {} already exists",
                pool.id
            )));
        }

        // Validate the pool based on type
        match pool.storage_type {
            StorageType::Zfs => self.zfs.validate_pool(&pool).await?,
            StorageType::Ceph => self.ceph.validate_pool(&pool).await?,
            StorageType::Lvm => self.lvm.validate_pool(&pool).await?,
            StorageType::Directory => self.directory.validate_pool(&pool).await?,
        }

        let pool_clone = pool.clone();
        pools.insert(pool.id.clone(), pool);
        Ok(pool_clone)
    }

    /// Remove a storage pool
    pub async fn remove_pool(&self, id: &str) -> Result<()> {
        let mut pools = self.pools.write().await;

        if pools.remove(id).is_none() {
            return Err(horcrux_common::Error::System(format!(
                "Storage pool {} not found",
                id
            )));
        }

        Ok(())
    }

    /// Create a volume in a storage pool
    pub async fn create_volume(
        &self,
        pool_id: &str,
        volume_name: &str,
        size_gb: u64,
    ) -> Result<String> {
        let pools = self.pools.read().await;
        let pool = pools
            .get(pool_id)
            .ok_or_else(|| horcrux_common::Error::System(format!("Storage pool {} not found", pool_id)))?;

        if !pool.enabled {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Storage pool {} is disabled",
                pool_id
            )));
        }

        if size_gb > pool.available {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Not enough space in pool {}. Requested: {}GB, Available: {}GB",
                pool_id, size_gb, pool.available
            )));
        }

        let volume_path = match pool.storage_type {
            StorageType::Zfs => {
                self.zfs
                    .create_volume(&pool.path, volume_name, size_gb)
                    .await?
            }
            StorageType::Ceph => {
                self.ceph
                    .create_volume(&pool.path, volume_name, size_gb)
                    .await?
            }
            StorageType::Lvm => {
                self.lvm
                    .create_volume(&pool.path, volume_name, size_gb)
                    .await?
            }
            StorageType::Directory => {
                self.directory
                    .create_volume(&pool.path, volume_name, size_gb)
                    .await?
            }
        };

        Ok(volume_path)
    }

    /// Delete a volume from a storage pool
    pub async fn delete_volume(&self, pool_id: &str, volume_name: &str) -> Result<()> {
        let pools = self.pools.read().await;
        let pool = pools
            .get(pool_id)
            .ok_or_else(|| horcrux_common::Error::System(format!("Storage pool {} not found", pool_id)))?;

        match pool.storage_type {
            StorageType::Zfs => self.zfs.delete_volume(&pool.path, volume_name).await?,
            StorageType::Ceph => self.ceph.delete_volume(&pool.path, volume_name).await?,
            StorageType::Lvm => self.lvm.delete_volume(&pool.path, volume_name).await?,
            StorageType::Directory => self.directory.delete_volume(&pool.path, volume_name).await?,
        }

        Ok(())
    }

    /// Create a snapshot of a volume
    pub async fn create_snapshot(
        &self,
        pool_id: &str,
        volume_name: &str,
        snapshot_name: &str,
    ) -> Result<()> {
        let pools = self.pools.read().await;
        let pool = pools
            .get(pool_id)
            .ok_or_else(|| horcrux_common::Error::System(format!("Storage pool {} not found", pool_id)))?;

        match pool.storage_type {
            StorageType::Zfs => {
                self.zfs
                    .create_snapshot(&pool.path, volume_name, snapshot_name)
                    .await?
            }
            StorageType::Ceph => {
                self.ceph
                    .create_snapshot(&pool.path, volume_name, snapshot_name)
                    .await?
            }
            StorageType::Lvm => {
                self.lvm
                    .create_snapshot(&pool.path, volume_name, snapshot_name)
                    .await?
            }
            StorageType::Directory => {
                return Err(horcrux_common::Error::InvalidConfig(
                    "Directory storage does not support snapshots".to_string(),
                ))
            }
        }

        Ok(())
    }

    /// Restore a volume from a snapshot
    pub async fn restore_snapshot(
        &self,
        pool_id: &str,
        volume_name: &str,
        snapshot_name: &str,
    ) -> Result<()> {
        let pools = self.pools.read().await;
        let pool = pools
            .get(pool_id)
            .ok_or_else(|| horcrux_common::Error::System(format!("Storage pool {} not found", pool_id)))?;

        match pool.storage_type {
            StorageType::Zfs => {
                self.zfs
                    .restore_snapshot(&pool.path, volume_name, snapshot_name)
                    .await?
            }
            StorageType::Ceph => {
                self.ceph
                    .restore_snapshot(&pool.path, volume_name, snapshot_name)
                    .await?
            }
            StorageType::Lvm => {
                self.lvm
                    .restore_snapshot(&pool.path, volume_name, snapshot_name)
                    .await?
            }
            StorageType::Directory => {
                return Err(horcrux_common::Error::InvalidConfig(
                    "Directory storage does not support snapshots".to_string(),
                ))
            }
        }

        Ok(())
    }
}
