//! HTTP route modules, one per work lane, each merged into the protected
//! router by a single line in main.rs.

pub mod auth;
#[cfg(feature = "backup")]
pub mod backup;
#[cfg(feature = "kubernetes")]
pub mod helm;
#[cfg(feature = "kubernetes")]
pub mod k8s_config;
pub mod lane_m;
pub mod metrics;
pub mod notifications;
pub mod tls;
pub mod vm_import;

use crate::AppState;
use axum::Router;
use std::sync::Arc;

/// Access control + certificate routes that need a login (main.rs wraps
/// them in the auth and RBAC middleware with every other protected route).
pub fn access_routes() -> Router<Arc<AppState>> {
    auth::protected_routes().merge(tls::routes())
}
