//! HTTP route modules, one per work lane, each merged into the protected
//! router by a single line in app/mod.rs (the router, formerly main.rs).

pub mod alert_rules;
pub mod auth;
#[cfg(feature = "backup")]
pub mod backup;
#[cfg(feature = "cluster")]
pub mod cluster_join;
pub mod dashboards;
#[cfg(feature = "cluster")]
pub mod ha_loop;
#[cfg(feature = "kubernetes")]
pub mod helm;
pub mod host_net;
#[cfg(feature = "kubernetes")]
pub mod k8s_config;
pub mod lane_m;
pub mod metrics;
pub mod migration_jobs;
pub mod migration_peer;
pub mod node_disks;
pub mod node_summary;
pub mod notifications;
pub mod prometheus_export;
#[cfg(feature = "s3-gateway")]
pub mod s3_gateway;
pub mod sdn_cni;
pub mod storage_volumes;
pub mod tls;
pub mod vm_hw;
pub mod vm_import;
pub mod vm_migration;

use crate::AppState;
use axum::Router;
use std::sync::Arc;

/// Access control + certificate routes that need a login (app/mod.rs wraps
/// them in the auth and RBAC middleware with every other protected route).
pub fn access_routes() -> Router<Arc<AppState>> {
    auth::protected_routes().merge(tls::routes())
}
