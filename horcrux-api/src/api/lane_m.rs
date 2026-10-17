//! Lane M: Helm (Kubernetes), Monitoring > Notifications channels and
//! Monitoring > Metrics Explorer backends for pages the web UI already
//! routes.

use crate::AppState;
use axum::Router;
use std::sync::Arc;

/// Build the lane's routes and start its background work (metrics sampler).
pub async fn routes(state: &Arc<AppState>) -> anyhow::Result<Router<Arc<AppState>>> {
    let channels = Arc::new(
        crate::notification_channels::NotificationChannelStore::load(state.database.clone())
            .await
            .map_err(|e| anyhow::anyhow!("loading notification channels: {:?}", e))?,
    );
    state.alert_manager.attach_channel_store(channels.clone());

    let metrics = Arc::new(crate::metrics_explorer::MetricsStore::new());
    crate::metrics_explorer::start_sampler(
        metrics.clone(),
        state.monitoring_manager.clone(),
        state.config.paths.data_dir.clone(),
    );

    let router = Router::new()
        .merge(super::notifications::routes(channels))
        .merge(super::metrics::routes(metrics));
    #[cfg(feature = "kubernetes")]
    let router = router
        .merge(super::helm::routes(state))
        .merge(super::k8s_config::routes());
    Ok(router)
}
