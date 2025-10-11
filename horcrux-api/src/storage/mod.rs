///! Storage backend management
///! Supports ZFS, Ceph, LVM, iSCSI, directory-based, GlusterFS, BtrFS, and S3 storage

pub mod zfs;
pub mod ceph;
pub mod lvm;
pub mod iscsi;
pub mod directory;
pub mod cifs;
pub mod nfs;
pub mod glusterfs;
pub mod btrfs;
pub mod s3;

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
    Cifs,
    Nfs,
    GlusterFs,
    BtrFs,
    S3,
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
    iscsi: iscsi::IscsiManager,
    cifs: cifs::CifsManager,
    nfs: nfs::NfsManager,
    glusterfs: glusterfs::GlusterFsManager,
    btrfs: btrfs::BtrFsManager,
    s3: s3::S3Manager,
    // Track next LUN ID for each iSCSI target
    iscsi_lun_counters: Arc<RwLock<HashMap<String, u32>>>,
}

impl StorageManager {
    pub fn new() -> Self {
        Self {
            pools: Arc::new(RwLock::new(HashMap::new())),
            zfs: zfs::ZfsManager::new(),
            ceph: ceph::CephManager::new(),
            lvm: lvm::LvmManager::new(),
            directory: directory::DirectoryManager::new(),
            iscsi: iscsi::IscsiManager::new(),
            cifs: cifs::CifsManager::new(),
            nfs: nfs::NfsManager::new(),
            glusterfs: glusterfs::GlusterFsManager::new(),
            btrfs: btrfs::BtrFsManager::new(),
            s3: s3::S3Manager::new(),
            iscsi_lun_counters: Arc::new(RwLock::new(HashMap::new())),
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
            StorageType::Iscsi => self.iscsi.validate_pool(&pool).await?,
            StorageType::Cifs => self.cifs.validate_pool(&pool).await?,
            StorageType::Nfs => self.nfs.validate_pool(&pool).await?,
            StorageType::GlusterFs => self.glusterfs.validate_pool(&pool).await?,
            StorageType::BtrFs => self.btrfs.validate_pool(&pool).await?,
            StorageType::S3 => {
                // S3 validation: verify path contains valid bucket configuration
                // Format expected: "s3://bucket-name" or "s3://endpoint/bucket-name"
                if pool.path.is_empty() {
                    return Err(horcrux_common::Error::InvalidConfig("S3 path cannot be empty".to_string()));
                }

                if !pool.path.starts_with("s3://") {
                    return Err(horcrux_common::Error::InvalidConfig(
                        format!("S3 path must start with 's3://', got: {}", pool.path)
                    ));
                }

                let bucket_part = pool.path.strip_prefix("s3://").unwrap();
                if bucket_part.is_empty() {
                    return Err(horcrux_common::Error::InvalidConfig(
                        "S3 path must specify bucket name after 's3://'".to_string()
                    ));
                }

                // Validate bucket name format (basic check)
                let bucket_name = bucket_part.split('/').next().unwrap_or("");
                if bucket_name.len() < 3 || bucket_name.len() > 63 {
                    return Err(horcrux_common::Error::InvalidConfig(
                        "S3 bucket name must be between 3 and 63 characters".to_string()
                    ));
                }

                // Delegate detailed validation to S3 manager
                self.s3.validate_pool(&pool).await?;
            }
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
            StorageType::Iscsi => {
                // For iSCSI, parse target and create volume
                let target = iscsi::IscsiTarget::parse(&pool.path)?;

                // Get and increment LUN ID for this target
                let target_key = format!("{}@{}", target.portal, target.iqn);
                let mut lun_counters = self.iscsi_lun_counters.write().await;
                let lun_id = lun_counters.entry(target_key).or_insert(0);
                let current_lun = *lun_id;
                *lun_id += 1;
                drop(lun_counters);

                self.iscsi.create_volume(&target, current_lun, size_gb).await?
            }
            StorageType::Cifs => {
                // For CIFS, create file in mounted share
                self.cifs.create_volume(&pool.path, volume_name, size_gb).await?
            }
            StorageType::Nfs => {
                // For NFS, create file in NFS mount
                self.nfs.create_volume(&pool.path, volume_name, size_gb).await?
            }
            StorageType::GlusterFs => {
                // For GlusterFS, create file in gluster volume
                self.glusterfs.create_volume(&pool.path, volume_name, size_gb).await?
            }
            StorageType::BtrFs => {
                // For BtrFS, create subvolume
                self.btrfs.create_volume(&pool.path, volume_name, size_gb).await?
            }
            StorageType::S3 => {
                // S3 doesn't support block volumes, use for backup storage instead
                return Err(horcrux_common::Error::InvalidConfig(
                    "S3 storage is for backups/objects only, not for VM volumes".to_string()
                ))
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
            StorageType::Iscsi => {},
            StorageType::Cifs => {},
            StorageType::Nfs => {},
            StorageType::GlusterFs => {},
            StorageType::BtrFs => {},
            StorageType::S3 => {},
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
            StorageType::Iscsi => {
                return Err(horcrux_common::Error::InvalidConfig(
                    "iSCSI storage does not support snapshots".to_string(),
                ))
            }
            StorageType::Cifs => {
                return Err(horcrux_common::Error::InvalidConfig(
                    "CIFS storage does not support snapshots".to_string(),
                ))
            }
            StorageType::Nfs => {
                return Err(horcrux_common::Error::InvalidConfig(
                    "NFS storage does not support snapshots".to_string(),
                ))
            }
            StorageType::GlusterFs => {
                // GlusterFS snapshot takes volume_path and snapshot_name
                let volume_path = format!("{}/{}", pool.path, volume_name);
                self.glusterfs.create_snapshot(&volume_path, snapshot_name).await?;
            }
            StorageType::BtrFs => {
                // BtrFS snapshot takes source_path, snapshot_name, and readonly flag
                let source_path = format!("{}/{}", pool.path, volume_name);
                self.btrfs.create_snapshot(&source_path, snapshot_name, false).await?;
            }
            StorageType::S3 => {
                return Err(horcrux_common::Error::InvalidConfig(
                    "S3 does not support snapshots".to_string(),
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
            StorageType::Iscsi => {
                return Err(horcrux_common::Error::InvalidConfig(
                    "iSCSI storage does not support snapshots".to_string(),
                ))
            }
            StorageType::Cifs => {
                return Err(horcrux_common::Error::InvalidConfig(
                    "CIFS storage does not support snapshots".to_string(),
                ))
            }
            StorageType::Nfs => {
                return Err(horcrux_common::Error::InvalidConfig(
                    "NFS storage does not support snapshots".to_string(),
                ))
            }
            StorageType::GlusterFs => {
                return Err(horcrux_common::Error::InvalidConfig(
                    "GlusterFS snapshot restore not yet implemented".to_string(),
                ))
            }
            StorageType::BtrFs => {
                // BtrFS restore_snapshot takes snapshot_path and target_path
                let snapshot_path = format!("{}/{}@{}", pool.path, volume_name, snapshot_name);
                let target_path = format!("{}/{}", pool.path, volume_name);
                self.btrfs.restore_snapshot(&snapshot_path, &target_path).await?
            }
            StorageType::S3 => {
                return Err(horcrux_common::Error::InvalidConfig(
                    "S3 does not support snapshots".to_string(),
                ))
            }
        }

        Ok(())
    }
}
