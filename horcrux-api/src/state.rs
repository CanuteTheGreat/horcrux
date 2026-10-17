//! Application state shared by every HTTP handler.

use std::sync::Arc;

use crate::alerts::AlertManager;
use crate::audit::AuditLogger;
#[cfg(feature = "backup")]
use crate::backup::BackupManager;
use crate::cloudinit::CloudInitManager;
use crate::cluster::ClusterManager;
use crate::console::ConsoleManager;
use crate::firewall::FirewallManager;
#[cfg(feature = "cluster")]
use crate::ha;
#[cfg(feature = "kubernetes")]
use crate::kubernetes;
use crate::monitoring::MonitoringManager;
use crate::observability::OtelManager;
use crate::secrets::VaultManager;
use crate::template::TemplateManager;
use crate::tls::TlsManager;
use crate::vm::VmManager;
use crate::{
    config, container, db, gpu, middleware, migration, prometheus, sdn, storage, vm, webhooks,
    websocket,
};

/// Shared application state
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<config::HorcruxConfig>,
    pub vm_manager: Arc<VmManager>,
    pub container_manager: Arc<container::ContainerManager>,
    #[cfg(feature = "backup")]
    pub backup_manager: Arc<BackupManager>,
    pub cloudinit_manager: Arc<CloudInitManager>,
    pub template_manager: Arc<TemplateManager>,
    pub firewall_manager: Arc<FirewallManager>,
    pub monitoring_manager: Arc<MonitoringManager>,
    pub console_manager: Arc<ConsoleManager>,
    pub cluster_manager: Arc<ClusterManager>,
    pub alert_manager: Arc<AlertManager>,
    pub otel_manager: Arc<OtelManager>,
    pub tls_manager: Arc<TlsManager>,
    pub vault_manager: Arc<VaultManager>,
    pub audit_logger: Arc<AuditLogger>,
    pub database: Arc<db::Database>,
    pub _rate_limiter: Arc<middleware::rate_limit::RateLimiter>, // Reserved for future rate limiting middleware
    pub storage_manager: Arc<storage::StorageManager>,
    #[cfg(feature = "cluster")]
    pub ha_manager: Arc<ha::HaManager>,
    pub migration_manager: Arc<migration::MigrationManager>,
    pub gpu_manager: Arc<gpu::GpuManager>,
    pub prometheus_manager: Arc<prometheus::PrometheusManager>,
    pub webhook_manager: Arc<webhooks::WebhookManager>,
    pub cni_manager: Arc<tokio::sync::RwLock<sdn::cni::CniManager>>,
    pub network_policy_manager: Arc<tokio::sync::RwLock<sdn::policy::NetworkPolicyManager>>,
    pub snapshot_manager: Arc<tokio::sync::RwLock<vm::snapshot::VmSnapshotManager>>,
    pub snapshot_scheduler: Arc<vm::snapshot_scheduler::SnapshotScheduler>,
    pub snapshot_quota_manager: Arc<vm::snapshot_quota::SnapshotQuotaManager>,
    pub clone_manager: Arc<vm::clone::VmCloneManager>,
    pub clone_job_manager: Arc<vm::clone_progress::CloneJobManager>,
    pub replication_manager: Arc<vm::replication::ReplicationManager>,
    pub ws_state: Arc<websocket::WsState>,
    #[cfg(any(feature = "docker", feature = "podman"))]
    pub image_manager: Arc<container::image::ImageManager>,
    #[cfg(any(feature = "docker", feature = "podman"))]
    pub volume_manager: Arc<container::volume::VolumeManager>,
    #[cfg(any(feature = "docker", feature = "podman"))]
    pub container_network_manager: Arc<container::cnetwork::ContainerNetworkManager>,
    #[cfg(any(feature = "docker", feature = "podman"))]
    pub stack_manager: Arc<container::stack::StackManager>,
    #[cfg(any(
        feature = "docker",
        feature = "podman",
        feature = "lxc",
        feature = "lxd",
        feature = "incus"
    ))]
    pub container_ws_manager: Arc<container::ws::ContainerWsManager>,
    #[cfg(feature = "kubernetes")]
    pub kubernetes_manager: Arc<kubernetes::KubernetesManager>,
}

impl middleware::AuthState for AppState {
    fn database(&self) -> &Arc<db::Database> {
        &self.database
    }
}

impl websocket::WsStateSource for AppState {
    fn ws_state(&self) -> &Arc<websocket::WsState> {
        &self.ws_state
    }
}
