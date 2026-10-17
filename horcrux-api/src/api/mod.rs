//! HTTP route modules, one per work lane, each merged into the protected
//! router by a single line in main.rs.

#[cfg(feature = "backup")]
pub mod backup;
#[cfg(feature = "kubernetes")]
pub mod helm;
#[cfg(feature = "kubernetes")]
pub mod k8s_config;
pub mod lane_m;
pub mod metrics;
pub mod notifications;
