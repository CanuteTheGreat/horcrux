//! Startup construction of the database-backed configuration managers.
//!
//! Each manager keeps its objects in memory for fast access and writes every
//! change through to the `config_objects` table (see `db::config_store`);
//! here they are built and loaded from the database before the API starts.

use crate::alerts::AlertManager;
#[cfg(feature = "backup")]
use crate::backup::BackupManager;
use crate::cluster::ClusterManager;
use crate::firewall::FirewallManager;
#[cfg(feature = "cluster")]
use crate::ha;
use crate::template::TemplateManager;
use crate::{config, db, storage};
use std::sync::Arc;

/// Database-backed configuration managers built at startup.
pub struct PersistedManagers {
    pub storage: Arc<storage::StorageManager>,
    pub firewall: Arc<FirewallManager>,
    #[cfg(feature = "cluster")]
    pub ha: Arc<ha::HaManager>,
    pub alert: Arc<AlertManager>,
    pub template: Arc<TemplateManager>,
    pub cluster: Arc<ClusterManager>,
    #[cfg(feature = "backup")]
    pub backup: Arc<BackupManager>,
}

/// First start: give the node a directory storage pool named `local`
/// (`<data_dir>/storage/local`), so ISOs and VM disks have a place to go
/// without any setup. Done once: an operator who removes it keeps it removed.
pub async fn ensure_default_storage(
    database: &Arc<db::Database>,
    mgr: &Arc<storage::StorageManager>,
    cfg: &config::HorcruxConfig,
) {
    use crate::db::config_store::ConfigStore;
    const SEED_KIND: &str = "install_seed";
    const SEED_ID: &str = "default_storage";
    let store = ConfigStore::new(database);
    if store
        .load::<bool>(SEED_KIND)
        .await
        .map(|v| v.iter().any(|(id, _)| id == SEED_ID))
        .unwrap_or(true)
    {
        return;
    }
    // Nodes that already have pools were set up by hand: nothing to add.
    if mgr.list_pools().await.is_empty() {
        let dir = cfg.paths.data_dir.join("storage").join("local");
        if let Err(e) = tokio::fs::create_dir_all(dir.join("template/iso")).await {
            tracing::warn!("default storage pool: creating {}: {e}", dir.display());
            return;
        }
        let pool = storage::StoragePool {
            id: crate::api::vm_hw::DEFAULT_POOL_ID.to_string(),
            name: "local".to_string(),
            storage_type: storage::StorageType::Directory,
            path: dir.to_string_lossy().to_string(),
            available: 0,
            total: 0,
            enabled: true,
            source: None,
            lvm: None,
            plugin_config: None,
        };
        match mgr.add_pool(pool).await {
            Ok(p) => tracing::info!("created default storage pool {} at {}", p.name, p.path),
            Err(e) => {
                tracing::warn!("default storage pool not created: {e}");
                return;
            }
        }
    }
    let _ = store.put(SEED_KIND, SEED_ID, &true).await;
}

/// Build every DB-backed configuration manager, loading its persisted state.
/// A database error here is fatal: starting with an empty configuration
/// would silently drop the operator's pools, rules and jobs.
pub async fn load_persisted_managers(
    database: &Arc<db::Database>,
    cfg: &config::HorcruxConfig,
) -> PersistedManagers {
    let firewall = FirewallManager::with_database(database.clone())
        .await
        .expect("Failed to load firewall configuration");
    // VM/container rules are live as soon as they are added; rebuild their
    // nftables chains from the stored copy (the kernel ruleset does not
    // survive a host reboot).
    firewall.reapply_guest_rules().await;
    let storage_mgr = Arc::new(
        storage::StorageManager::with_database(database.clone())
            .await
            .expect("Failed to load storage pools"),
    );
    ensure_default_storage(database, &storage_mgr, cfg).await;
    PersistedManagers {
        storage: storage_mgr,
        firewall: Arc::new(firewall),
        #[cfg(feature = "cluster")]
        ha: Arc::new(
            ha::HaManager::with_database(database.clone())
                .await
                .expect("Failed to load HA configuration"),
        ),
        alert: Arc::new(
            AlertManager::with_database(database.clone())
                .await
                .expect("Failed to load alert rules"),
        ),
        template: Arc::new(
            TemplateManager::with_database(database.clone(), cfg.paths.vm_storage.clone())
                .await
                .expect("Failed to load templates"),
        ),
        cluster: Arc::new(
            ClusterManager::with_database(database.clone())
                .await
                .expect("Failed to load cluster nodes"),
        ),
        #[cfg(feature = "backup")]
        backup: Arc::new(
            BackupManager::with_database(
                database.clone(),
                cfg.paths.backups.clone(),
                cfg.paths.restore.clone(),
            )
            .await
            .expect("Failed to load backup jobs"),
        ),
    }
}
