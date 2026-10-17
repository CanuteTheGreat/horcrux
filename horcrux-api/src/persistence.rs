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
    PersistedManagers {
        storage: Arc::new(
            storage::StorageManager::with_database(database.clone())
                .await
                .expect("Failed to load storage pools"),
        ),
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
