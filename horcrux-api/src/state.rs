//! Application State
//!
//! Shared state for the Horcrux API server

use std::sync::Arc;

use crate::vm::VmManager;
use crate::container::ContainerManager;
use crate::backup::BackupManager;
use crate::cloudinit::CloudInitManager;
use crate::template::TemplateManager;
use crate::auth::AuthManager;
use crate::firewall::FirewallManager;
use crate::monitoring::MonitoringManager;
use crate::console::ConsoleManager;
use crate::cluster::ClusterManager;
use crate::alerts::AlertManager;
use crate::observability::OtelManager;
use crate::tls::TlsManager;
use crate::secrets::VaultManager;
use crate::audit::AuditLogger;
use crate::db::Database;
use crate::middleware::rate_limit::RateLimiter;
use crate::storage::StorageManager;
use crate::ha::HaManager;
use crate::migration::MigrationManager;
use crate::gpu::GpuManager;
use crate::prometheus::PrometheusManager;
use crate::webhooks::WebhookManager;
use crate::sdn::cni::CniManager;
use crate::sdn::policy::NetworkPolicyManager;
use crate::vm::snapshot::VmSnapshotManager;
use crate::vm::snapshot_scheduler::SnapshotScheduler;
use crate::vm::snapshot_quota::SnapshotQuotaManager;
use crate::vm::clone::VmCloneManager;
use crate::vm::clone_progress::CloneJobManager;
use crate::vm::replication::ReplicationManager;
use crate::websocket::WsState;
#[cfg(feature = "kubernetes")]
use crate::kubernetes::KubernetesManager;

/// Shared application state
#[derive(Clone)]
pub struct AppState {
    pub vm_manager: Arc<VmManager>,
    pub container_manager: Arc<ContainerManager>,
    pub backup_manager: Arc<BackupManager>,
    pub cloudinit_manager: Arc<CloudInitManager>,
    pub template_manager: Arc<TemplateManager>,
    pub auth_manager: Arc<AuthManager>,
    pub firewall_manager: Arc<FirewallManager>,
    pub monitoring_manager: Arc<MonitoringManager>,
    pub console_manager: Arc<ConsoleManager>,
    pub cluster_manager: Arc<ClusterManager>,
    pub alert_manager: Arc<AlertManager>,
    pub otel_manager: Arc<OtelManager>,
    pub tls_manager: Arc<TlsManager>,
    pub vault_manager: Arc<VaultManager>,
    pub audit_logger: Arc<AuditLogger>,
    pub database: Arc<Database>,
    pub _rate_limiter: Arc<RateLimiter>,
    pub storage_manager: Arc<StorageManager>,
    pub ha_manager: Arc<HaManager>,
    pub migration_manager: Arc<MigrationManager>,
    pub gpu_manager: Arc<GpuManager>,
    pub prometheus_manager: Arc<PrometheusManager>,
    pub webhook_manager: Arc<WebhookManager>,
    pub cni_manager: Arc<tokio::sync::RwLock<CniManager>>,
    pub network_policy_manager: Arc<tokio::sync::RwLock<NetworkPolicyManager>>,
    pub snapshot_manager: Arc<tokio::sync::RwLock<VmSnapshotManager>>,
    pub snapshot_scheduler: Arc<SnapshotScheduler>,
    pub snapshot_quota_manager: Arc<SnapshotQuotaManager>,
    pub clone_manager: Arc<VmCloneManager>,
    pub clone_job_manager: Arc<CloneJobManager>,
    pub replication_manager: Arc<ReplicationManager>,
    pub ws_state: Arc<WsState>,
    #[cfg(feature = "kubernetes")]
    pub kubernetes_manager: Arc<KubernetesManager>,
}
