//! Storage pools served by third-party plugins (`StorageType::Plugin`).
//!
//! The pool record keeps the validated, non-secret config; secret fields go
//! to the secrets store (config store kind `storage_plugin_secret`, never
//! returned by the API). Every volume operation is one plugin call with its
//! own timeout (see `plugin.rs`).
use super::plugin::sdk::{self, Op, PoolContext, VolumeArgs};
use super::{StorageManager, StoragePool, StorageType, VolumeInfo};
use crate::db::config_store::{self, kinds};
use horcrux_common::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const GB: u64 = 1024 * 1024 * 1024;

/// A snapshot of a pool volume (`GET .../volumes/:name/snapshots`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeSnapshot {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<i64>,
}

fn to_info(v: sdk::Volume) -> VolumeInfo {
    VolumeInfo {
        name: v.name,
        path: v.path,
        format: v.format,
        size_bytes: v.size_bytes,
        allocated_bytes: v.allocated_bytes,
    }
}

impl StorageManager {
    /// The plugin host (plugin list, settings).
    pub fn plugin_host(&self) -> &super::plugin::PluginHost {
        &self.plugins
    }

    async fn plugin_ctx(&self, pool: &StoragePool) -> PoolContext {
        PoolContext {
            id: pool.id.clone(),
            name: pool.name.clone(),
            config: pool.plugin_config.clone().unwrap_or_default(),
            secrets: self
                .plugin_secrets
                .read()
                .await
                .get(&pool.id)
                .cloned()
                .unwrap_or_default(),
            state_dir: self
                .plugins
                .pool_state_dir(&pool.id)
                .to_string_lossy()
                .to_string(),
        }
    }

    /// Activate a plugin pool (pool add, manager start): returns the local
    /// directory it reported.
    async fn plugin_activate(&self, id: &str, pool: &StoragePool) -> Result<Option<String>> {
        let ctx = self.plugin_ctx(pool).await;
        let r: sdk::ActivateResult = self
            .plugins
            .call_as(id, Op::Activate, Some(ctx), VolumeArgs::default())
            .await?;
        Ok(r.local_dir)
    }

    /// Add a pool of plugin `plugin_id`. `raw` is the form/spec config
    /// (secrets included); it is validated against the plugin's schema
    /// (400 listing every bad field), the plugin activates the pool, and
    /// only then is anything stored.
    pub async fn add_plugin_pool(
        &self,
        name: &str,
        plugin_id: &str,
        raw: BTreeMap<String, serde_json::Value>,
        enabled: bool,
    ) -> Result<StoragePool> {
        if name.trim().is_empty() {
            return Err(Error::InvalidConfig("pool name is required".into()));
        }
        let desc = self.plugins.describe(plugin_id).await?;
        let v = sdk::validate_config(&desc.schema, &raw).map_err(|errs| {
            Error::InvalidConfig(format!(
                "{} pool settings: {}",
                desc.name,
                errs.iter()
                    .map(|e| match desc.schema.iter().find(|f| f.key == e.key) {
                        Some(f) => format!("{} ({}) {}", f.label, f.key, e.message),
                        None => e.to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            ))
        })?;
        let mut pool = StoragePool {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            storage_type: StorageType::Plugin(plugin_id.to_string()),
            path: String::new(),
            available: 0,
            total: 0,
            enabled,
            source: None,
            plugin_config: Some(v.config),
        };
        self.plugin_secrets
            .write()
            .await
            .insert(pool.id.clone(), v.secrets.clone());
        let activated = self.plugin_activate(plugin_id, &pool).await;
        let local_dir = match activated {
            Ok(d) => d,
            Err(e) => {
                self.plugin_secrets.write().await.remove(&pool.id);
                let _ = std::fs::remove_dir_all(self.plugins.pool_state_dir(&pool.id));
                return Err(e);
            }
        };
        // Only a real local directory becomes the pool path (VM disks and the
        // content browser use it); otherwise volumes are reached through the
        // plugin alone.
        if let Some(d) = local_dir.filter(|d| std::path::Path::new(d).is_absolute()) {
            pool.path = d;
        }
        self.plugin_measure(plugin_id, &mut pool).await;
        if let Err(e) = async {
            config_store::put_opt(
                &self.store,
                kinds::STORAGE_PLUGIN_SECRET,
                &pool.id,
                &v.secrets,
            )
            .await?;
            config_store::put_opt(&self.store, kinds::STORAGE_POOL, &pool.id, &pool).await
        }
        .await
        {
            let ctx = self.plugin_ctx(&pool).await;
            let _ = self
                .plugins
                .call(plugin_id, Op::Deactivate, Some(ctx), VolumeArgs::default())
                .await;
            self.plugin_secrets.write().await.remove(&pool.id);
            return Err(e);
        }
        self.pools
            .write()
            .await
            .insert(pool.id.clone(), pool.clone());
        Ok(pool)
    }

    /// Capacity from the plugin's `status` (kept when it fails).
    async fn plugin_measure(&self, plugin_id: &str, pool: &mut StoragePool) {
        let ctx = self.plugin_ctx(pool).await;
        match self
            .plugins
            .call_as::<sdk::StatusResult>(plugin_id, Op::Status, Some(ctx), VolumeArgs::default())
            .await
        {
            Ok(st) if st.active => {
                pool.total = st.total_bytes / GB;
                pool.available = st.available_bytes / GB;
            }
            Ok(st) => tracing::warn!(
                "Storage pool {}: plugin {plugin_id} reports it inactive: {}",
                pool.name,
                st.message.unwrap_or_default()
            ),
            Err(e) => tracing::warn!("Storage pool {}: status: {e}", pool.name),
        }
    }

    /// Re-measure plugin pools (all, or `only`) in parallel.
    pub(super) async fn refresh_plugin_pools(&self, only: Option<&str>) {
        let targets: Vec<StoragePool> = self
            .pools
            .read()
            .await
            .values()
            .filter(|p| p.storage_type.plugin_id().is_some())
            .filter(|p| only.is_none_or(|id| p.id == id))
            .cloned()
            .collect();
        for mut p in targets {
            let id = p.storage_type.plugin_id().unwrap_or_default().to_string();
            self.plugin_measure(&id, &mut p).await;
            if let Some(live) = self.pools.write().await.get_mut(&p.id) {
                live.total = p.total;
                live.available = p.available;
            }
        }
    }

    /// Re-activate plugin pools at start (background; a storage server may
    /// be briefly down at boot).
    pub(super) async fn spawn_plugin_activation(&self) {
        let pools: Vec<StoragePool> = self
            .pools
            .read()
            .await
            .values()
            .filter(|p| p.enabled && p.storage_type.plugin_id().is_some())
            .cloned()
            .collect();
        for p in pools {
            let id = p.storage_type.plugin_id().unwrap_or_default().to_string();
            let ctx = self.plugin_ctx(&p).await;
            let host = self.plugins.clone();
            tokio::spawn(async move {
                if let Err(e) = host
                    .call(&id, Op::Activate, Some(ctx), VolumeArgs::default())
                    .await
                {
                    tracing::warn!("Storage pool {}: activating ({id}): {e}", p.name);
                }
            });
        }
    }

    /// Plugin side of removing a pool: deactivate, drop secrets and state.
    pub(super) async fn plugin_remove(&self, pool: &StoragePool) {
        let Some(id) = pool.storage_type.plugin_id() else {
            return;
        };
        let ctx = self.plugin_ctx(pool).await;
        if let Err(e) = self
            .plugins
            .call(id, Op::Deactivate, Some(ctx), VolumeArgs::default())
            .await
        {
            tracing::warn!("Storage pool {}: deactivating ({id}): {e}", pool.name);
        }
        self.plugin_secrets.write().await.remove(&pool.id);
        if let Err(e) =
            config_store::delete_opt(&self.store, kinds::STORAGE_PLUGIN_SECRET, &pool.id).await
        {
            tracing::warn!("Storage pool {}: deleting secrets: {e}", pool.name);
        }
        // only remove the state dir when nothing is mounted under it any more
        let dir = self.plugins.pool_state_dir(&pool.id);
        let dir_s = dir.to_string_lossy().to_string();
        let busy = std::fs::read_to_string("/proc/self/mounts")
            .map(|m| {
                m.lines()
                    .filter_map(|l| l.split(' ').nth(1))
                    .any(|mp| mp.starts_with(&dir_s))
            })
            .unwrap_or(false);
        if !busy {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    async fn plugin_pool(&self, pool_id: &str) -> Result<(String, StoragePool)> {
        let pool = self.get_pool_or_404(pool_id).await?;
        match pool.storage_type.plugin_id() {
            Some(id) => Ok((id.to_string(), pool)),
            None => Err(Error::InvalidConfig(format!(
                "pool {} is not a plugin pool",
                pool.name
            ))),
        }
    }

    async fn pcall<T: serde::de::DeserializeOwned>(
        &self,
        id: &str,
        pool: &StoragePool,
        op: Op,
        args: VolumeArgs,
    ) -> Result<T> {
        let ctx = self.plugin_ctx(pool).await;
        self.plugins.call_as(id, op, Some(ctx), args).await
    }

    fn vargs(name: &str) -> VolumeArgs {
        VolumeArgs {
            name: name.to_string(),
            ..Default::default()
        }
    }

    pub(super) async fn plugin_create_volume(
        &self,
        id: &str,
        pool: &StoragePool,
        name: &str,
        size_gb: u64,
    ) -> Result<String> {
        if !pool.enabled {
            return Err(Error::InvalidConfig(format!(
                "Storage pool {} is disabled",
                pool.name
            )));
        }
        if size_gb == 0 {
            return Err(Error::InvalidConfig(
                "Volume size must be at least 1 GB".to_string(),
            ));
        }
        // the plugin knows the real space; check only what it reported
        if pool.total > 0 && size_gb > pool.available {
            return Err(Error::InvalidConfig(format!(
                "Not enough space in pool {}. Requested: {}GB, Available: {}GB",
                pool.name, size_gb, pool.available
            )));
        }
        let v: sdk::Volume = self
            .pcall(
                id,
                pool,
                Op::CreateVolume,
                VolumeArgs {
                    name: name.to_string(),
                    size_bytes: Some(size_gb * GB),
                    ..Default::default()
                },
            )
            .await?;
        Ok(v.path)
    }

    pub(super) async fn plugin_list_volumes(
        &self,
        id: &str,
        pool: &StoragePool,
    ) -> Result<Vec<VolumeInfo>> {
        let v: Vec<sdk::Volume> = self
            .pcall(id, pool, Op::ListVolumes, VolumeArgs::default())
            .await?;
        let mut out: Vec<VolumeInfo> = v
            .into_iter()
            .filter(|v| sdk::valid_volume_name(&v.name))
            .map(to_info)
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub(super) async fn plugin_delete_volume(
        &self,
        id: &str,
        pool: &StoragePool,
        name: &str,
    ) -> Result<()> {
        self.pcall::<serde_json::Value>(id, pool, Op::DeleteVolume, Self::vargs(name))
            .await
            .map(|_| ())
    }

    pub(super) async fn plugin_resize_volume(
        &self,
        id: &str,
        pool: &StoragePool,
        name: &str,
        new_size_gb: u64,
    ) -> Result<()> {
        let cur = self
            .plugin_list_volumes(id, pool)
            .await?
            .into_iter()
            .find(|v| v.name == name)
            .ok_or_else(|| {
                Error::NotFound(format!("Volume {} not found in pool {}", name, pool.name))
            })?;
        if new_size_gb.saturating_mul(GB) < cur.size_bytes {
            return Err(Error::InvalidConfig(format!(
                "Volume {} is {} GB; shrinking is not supported",
                name,
                cur.size_bytes.div_ceil(GB)
            )));
        }
        let grow = (new_size_gb * GB)
            .saturating_sub(cur.size_bytes)
            .div_ceil(GB);
        if pool.total > 0 && grow > pool.available {
            return Err(Error::InvalidConfig(format!(
                "Not enough space in pool {}. Requested: +{}GB, Available: {}GB",
                pool.name, grow, pool.available
            )));
        }
        self.pcall::<sdk::Volume>(
            id,
            pool,
            Op::ResizeVolume,
            VolumeArgs {
                name: name.to_string(),
                size_bytes: Some(new_size_gb * GB),
                ..Default::default()
            },
        )
        .await
        .map(|_| ())
    }

    fn snap_args(name: &str, snapshot: &str) -> VolumeArgs {
        VolumeArgs {
            name: name.to_string(),
            snapshot: Some(snapshot.to_string()),
            ..Default::default()
        }
    }

    pub(super) async fn plugin_snapshot(
        &self,
        id: &str,
        pool: &StoragePool,
        op: Op,
        name: &str,
        snapshot: &str,
    ) -> Result<()> {
        self.pcall::<serde_json::Value>(id, pool, op, Self::snap_args(name, snapshot))
            .await
            .map(|_| ())
    }

    /// Snapshots of a volume (plugin pools whose plugin declares snapshots).
    pub async fn list_snapshots(&self, pool_id: &str, volume: &str) -> Result<Vec<VolumeSnapshot>> {
        super::check_names(&[volume])?;
        let (id, pool) = self.plugin_pool(pool_id).await?;
        let s: Vec<sdk::Snapshot> = self
            .pcall(id.as_str(), &pool, Op::ListSnapshots, Self::vargs(volume))
            .await?;
        Ok(s.into_iter()
            .map(|s| VolumeSnapshot {
                name: s.name,
                created: s.created,
            })
            .collect())
    }

    /// Delete a volume snapshot (plugin pools).
    pub async fn delete_snapshot(&self, pool_id: &str, volume: &str, snapshot: &str) -> Result<()> {
        super::check_names(&[volume, snapshot])?;
        let (id, pool) = self.plugin_pool(pool_id).await?;
        self.plugin_snapshot(&id, &pool, Op::DeleteSnapshot, volume, snapshot)
            .await
    }

    /// What a VM disk on `name` points at; asks the plugin for plugin pools.
    pub async fn volume_location_of(&self, pool: &StoragePool, name: &str) -> Result<String> {
        super::check_names(&[name])?;
        match pool.storage_type.plugin_id() {
            Some(id) => self
                .pcall::<sdk::PathResult>(id, pool, Op::VolumePath, Self::vargs(name))
                .await
                .map(|p| p.path),
            None => Self::volume_location(pool, name),
        }
    }

    /// Whether VM disks may be created in this plugin pool's directory.
    pub async fn plugin_holds_vm_disks(&self, pool: &StoragePool) -> bool {
        match pool.storage_type.plugin_id() {
            Some(id) => self
                .plugins
                .describe(id)
                .await
                .map(|d| d.capabilities.vm_disk_dir)
                .unwrap_or(false),
            None => false,
        }
    }

    /// Load secrets of plugin pools (database-backed manager start).
    pub(super) async fn load_plugin_secrets(&self) -> Result<()> {
        if let Some(store) = &self.store {
            let rows = store
                .load::<BTreeMap<String, String>>(kinds::STORAGE_PLUGIN_SECRET)
                .await?;
            let mut g = self.plugin_secrets.write().await;
            for (id, s) in rows {
                g.insert(id, s);
            }
        }
        Ok(())
    }
}
