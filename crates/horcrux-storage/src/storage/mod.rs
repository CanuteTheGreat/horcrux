pub mod btrfs;
pub mod ceph;
pub mod cifs;
pub mod directory;
pub mod glusterfs;
pub mod iscsi;
pub mod lvm;
pub mod migration;
pub mod nfs;
pub mod s3;
pub mod thin_provision;
/// Storage backend management
/// Supports ZFS, Ceph, LVM, iSCSI, directory-based, GlusterFS, BtrFS, and S3 storage
pub mod zfs;

use crate::db::config_store::{self, kinds, ConfigStore};
use crate::db::Database;
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
    pub available: u64, // Available space in GB
    pub total: u64,     // Total space in GB
    pub enabled: bool,
}

/// (total, available) GB of the filesystem holding `path`, via statvfs(2).
pub fn fs_capacity_gb(path: &str) -> Option<(u64, u64)> {
    let st = nix::sys::statvfs::statvfs(path).ok()?;
    let frag = st.fragment_size() as u64;
    let gb = 1024 * 1024 * 1024;
    Some((
        st.blocks() as u64 * frag / gb,
        st.blocks_available() as u64 * frag / gb,
    ))
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
    /// Durable copy of `pools`; `None` for a purely in-memory manager.
    store: Option<ConfigStore>,
}

impl Default for StorageManager {
    fn default() -> Self {
        Self::new()
    }
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
            store: None,
        }
    }

    /// Storage manager backed by the database: loads the persisted pool
    /// definitions and writes every add/remove through, so pools survive a
    /// restart. Stored pools are not re-validated (a remote mount may be
    /// briefly unavailable at boot); filesystem capacity is re-measured.
    pub async fn with_database(db: Arc<Database>) -> Result<Self> {
        let store = ConfigStore::new(&db);
        let mut mgr = Self::new();
        {
            let mut pools = mgr.pools.write().await;
            for mut pool in store
                .load_values::<StoragePool>(kinds::STORAGE_POOL)
                .await?
            {
                Self::refresh_capacity(&mut pool);
                pools.insert(pool.id.clone(), pool);
            }
            tracing::info!("Loaded {} storage pool(s) from database", pools.len());
        }
        mgr.store = Some(store);
        Ok(mgr)
    }

    /// Measure real capacity for filesystem-backed pools.
    fn refresh_capacity(pool: &mut StoragePool) {
        if matches!(
            pool.storage_type,
            StorageType::Directory | StorageType::Nfs | StorageType::Cifs | StorageType::BtrFs
        ) {
            if let Some((total, available)) = fs_capacity_gb(&pool.path) {
                pool.total = total;
                pool.available = available;
            }
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
                    return Err(horcrux_common::Error::InvalidConfig(
                        "S3 path cannot be empty".to_string(),
                    ));
                }

                if !pool.path.starts_with("s3://") {
                    return Err(horcrux_common::Error::InvalidConfig(format!(
                        "S3 path must start with 's3://', got: {}",
                        pool.path
                    )));
                }

                let bucket_part = pool.path.strip_prefix("s3://").unwrap();
                if bucket_part.is_empty() {
                    return Err(horcrux_common::Error::InvalidConfig(
                        "S3 path must specify bucket name after 's3://'".to_string(),
                    ));
                }

                // Validate bucket name format (basic check)
                let bucket_name = bucket_part.split('/').next().unwrap_or("");
                if bucket_name.len() < 3 || bucket_name.len() > 63 {
                    return Err(horcrux_common::Error::InvalidConfig(
                        "S3 bucket name must be between 3 and 63 characters".to_string(),
                    ));
                }

                // Delegate detailed validation to S3 manager
                self.s3.validate_pool(&pool).await?;
            }
        }

        // Filesystem-backed pools: measure real capacity. The API used to
        // store available=0/total=0 ("will be updated by backend", never
        // was), so every volume create in a directory pool failed with
        // "Not enough space ... Available: 0GB".
        let mut pool = pool;
        Self::refresh_capacity(&mut pool);

        config_store::put_opt(&self.store, kinds::STORAGE_POOL, &pool.id, &pool).await?;
        let pool_clone = pool.clone();
        pools.insert(pool.id.clone(), pool);
        Ok(pool_clone)
    }

    /// Remove a storage pool
    pub async fn remove_pool(&self, id: &str) -> Result<()> {
        let mut pools = self.pools.write().await;

        if !pools.contains_key(id) {
            return Err(horcrux_common::Error::System(format!(
                "Storage pool {} not found",
                id
            )));
        }
        config_store::delete_opt(&self.store, kinds::STORAGE_POOL, id).await?;
        pools.remove(id);

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
        let pool = pools.get(pool_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("Storage pool {} not found", pool_id))
        })?;

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

                self.iscsi
                    .create_volume(&target, current_lun, size_gb)
                    .await?
            }
            StorageType::Cifs => {
                // For CIFS, create file in mounted share
                self.cifs
                    .create_volume(&pool.path, volume_name, size_gb)
                    .await?
            }
            StorageType::Nfs => {
                // For NFS, create file in NFS mount
                self.nfs
                    .create_volume(&pool.path, volume_name, size_gb)
                    .await?
            }
            StorageType::GlusterFs => {
                // For GlusterFS, create file in gluster volume
                self.glusterfs
                    .create_volume(&pool.path, volume_name, size_gb)
                    .await?
            }
            StorageType::BtrFs => {
                // For BtrFS, create subvolume
                self.btrfs
                    .create_volume(&pool.path, volume_name, size_gb)
                    .await?
            }
            StorageType::S3 => {
                // S3 doesn't support block volumes, use for backup storage instead
                return Err(horcrux_common::Error::InvalidConfig(
                    "S3 storage is for backups/objects only, not for VM volumes".to_string(),
                ));
            }
        };

        Ok(volume_path)
    }

    /// Delete a volume from a storage pool
    #[allow(dead_code)]
    pub async fn delete_volume(&self, pool_id: &str, volume_name: &str) -> Result<()> {
        let pools = self.pools.read().await;
        let pool = pools.get(pool_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("Storage pool {} not found", pool_id))
        })?;

        match pool.storage_type {
            StorageType::Zfs => self.zfs.delete_volume(&pool.path, volume_name).await?,
            StorageType::Ceph => self.ceph.delete_volume(&pool.path, volume_name).await?,
            StorageType::Lvm => self.lvm.delete_volume(&pool.path, volume_name).await?,
            StorageType::Directory => {
                self.directory
                    .delete_volume(&pool.path, volume_name)
                    .await?
            }
            StorageType::Iscsi => {}
            StorageType::Cifs => {}
            StorageType::Nfs => {}
            StorageType::GlusterFs => {}
            StorageType::BtrFs => {}
            StorageType::S3 => {}
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
        let pool = pools.get(pool_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("Storage pool {} not found", pool_id))
        })?;

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
                self.glusterfs
                    .create_snapshot(&volume_path, snapshot_name)
                    .await?;
            }
            StorageType::BtrFs => {
                // BtrFS snapshot takes source_path, snapshot_name, and readonly flag
                let source_path = format!("{}/{}", pool.path, volume_name);
                self.btrfs
                    .create_snapshot(&source_path, snapshot_name, false)
                    .await?;
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
        let pool = pools.get(pool_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("Storage pool {} not found", pool_id))
        })?;

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
                // GlusterFS restore takes volume_path and snapshot_name, mirroring create_snapshot
                let volume_path = format!("{}/{}", pool.path, volume_name);
                self.glusterfs
                    .restore_snapshot(&volume_path, snapshot_name)
                    .await?
            }
            StorageType::BtrFs => {
                // BtrFS restore_snapshot takes snapshot_path and target_path
                let snapshot_path = format!("{}/{}@{}", pool.path, volume_name, snapshot_name);
                let target_path = format!("{}/{}", pool.path, volume_name);
                self.btrfs
                    .restore_snapshot(&snapshot_path, &target_path)
                    .await?
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("hx-storage-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn pool(id: &str, t: StorageType, path: &str) -> StoragePool {
        StoragePool {
            id: id.into(),
            name: id.into(),
            storage_type: t,
            path: path.into(),
            available: 0,
            total: 0,
            enabled: true,
        }
    }

    /// (total, available) GB per `df` (1K blocks, floored to GB).
    fn df_gb(path: &str) -> (u64, u64) {
        let out = std::process::Command::new("df")
            .args(["-k", "--output=size,avail", path])
            .output()
            .expect("df");
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        let mut nums = text
            .lines()
            .nth(1)
            .expect("df row")
            .split_whitespace()
            .map(|n| n.parse::<u64>().unwrap() / (1024 * 1024));
        (nums.next().unwrap(), nums.next().unwrap())
    }

    #[test]
    fn capacity_matches_df() {
        // A filesystem with a few GB at least (the build tree), so the GB
        // arithmetic is visible.
        let path = env!("CARGO_MANIFEST_DIR");
        let (total, avail) = fs_capacity_gb(path).expect("statvfs");
        let (df_total, df_avail) = df_gb(path);
        assert_eq!(total, df_total);
        // Free space can move while other builds write to the disk.
        assert!(avail.abs_diff(df_avail) <= 1, "{avail} vs df {df_avail}");
        assert!(total > 0 && avail <= total);
        assert_eq!(fs_capacity_gb("/nonexistent/hx-storage-test"), None);
    }

    #[tokio::test]
    async fn directory_pool_lifecycle_with_real_capacity() {
        let d = scratch("dir");
        let m = StorageManager::new();
        let p = m
            .add_pool(pool("dirpool", StorageType::Directory, d.to_str().unwrap()))
            .await
            .unwrap();
        let (total, avail) = fs_capacity_gb(d.to_str().unwrap()).unwrap();
        assert_eq!(p.total, total, "capacity measured");
        assert!(
            p.available.abs_diff(avail) <= 1,
            "{} vs {avail}",
            p.available
        );
        let listed = m.list_pools().await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "dirpool");
        assert_eq!(m.get_pool("dirpool").await.unwrap().total, total);
        assert!(m
            .add_pool(pool("dirpool", StorageType::Directory, d.to_str().unwrap()))
            .await
            .is_err());
        assert!(m.remove_pool("nope").await.is_err());
        m.remove_pool("dirpool").await.unwrap();
        assert!(m.list_pools().await.is_empty());
        assert!(m.get_pool("dirpool").await.is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn volumes_respect_available_space_and_pool_state() {
        let d = scratch("vol");
        let m = StorageManager::new();
        let mut p = pool("v", StorageType::Directory, d.to_str().unwrap());
        p.available = 2;
        m.pools.write().await.insert("v".into(), p);
        // Exactly the available space fits (qcow2 is sparse).
        let path = m.create_volume("v", "disk0", 2).await.unwrap();
        assert_eq!(path, format!("{}/disk0.qcow2", d.display()));
        assert!(std::path::Path::new(&path).is_file());
        // One GB more does not.
        assert!(m.create_volume("v", "disk1", 3).await.is_err());
        assert!(!d.join("disk1.qcow2").exists());
        // Disabled pools and unknown pools refuse.
        m.pools.write().await.get_mut("v").unwrap().enabled = false;
        assert!(m.create_volume("v", "disk2", 1).await.is_err());
        assert!(m.create_volume("missing", "disk3", 1).await.is_err());
        // S3 pools hold backups, not VM disks.
        let mut s3 = pool("s3", StorageType::S3, "s3://bucket");
        s3.available = 100;
        m.pools.write().await.insert("s3".into(), s3);
        assert!(m.create_volume("s3", "disk4", 1).await.is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn s3_bucket_names_are_3_to_63_characters() {
        let m = StorageManager::new();
        for (i, (path, ok)) in [
            ("s3://ab".to_string(), false),
            ("s3://abc".to_string(), true),
            (format!("s3://{}", "b".repeat(63)), true),
            (format!("s3://{}", "b".repeat(64)), false),
            ("s3://endpoint.example/abc".to_string(), true),
            ("s3://".to_string(), false),
            ("http://abc".to_string(), false),
            (String::new(), false),
        ]
        .into_iter()
        .enumerate()
        {
            let r = m
                .add_pool(pool(&format!("s3-{i}"), StorageType::S3, &path))
                .await;
            assert_eq!(r.is_ok(), ok, "{path:?}: {r:?}");
        }
    }

    #[tokio::test]
    async fn pools_survive_a_restart_through_the_database() {
        let d = scratch("db");
        let db = Arc::new(Database::new("sqlite::memory:").await.unwrap());
        db.migrate().await.unwrap();
        let m = StorageManager::with_database(db.clone()).await.unwrap();
        m.add_pool(pool("kept", StorageType::Directory, d.to_str().unwrap()))
            .await
            .unwrap();
        m.add_pool(pool("gone", StorageType::Directory, d.to_str().unwrap()))
            .await
            .unwrap();
        m.remove_pool("gone").await.unwrap();
        drop(m);

        let again = StorageManager::with_database(db).await.unwrap();
        let pools = again.list_pools().await;
        assert_eq!(pools.len(), 1, "{pools:?}");
        assert_eq!(pools[0].id, "kept");
        // Capacity is re-measured on load.
        let (total, _) = fs_capacity_gb(d.to_str().unwrap()).unwrap();
        assert_eq!(pools[0].total, total);
        let _ = std::fs::remove_dir_all(&d);
    }
}
