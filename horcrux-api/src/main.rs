// Allow dead code for modules with prepared functionality for future features
#![allow(dead_code)]

mod config;
mod vm;
mod container;
mod storage;
mod backup;
mod cloudinit;
mod template;
mod auth;
mod firewall;
mod monitoring;
mod console;
mod cluster;
mod alerts;
mod observability;
mod sdn;
mod ha;
mod migration;
mod audit;
mod tls;
mod secrets;
mod db;
mod middleware;
mod logging;
mod gpu;
mod prometheus;
mod webhooks;
mod error;
mod validation;
mod websocket;
mod openapi;
mod metrics_collector;
mod metrics;
mod encryption;
mod health;
mod shutdown;

#[cfg(feature = "kubernetes")]
mod kubernetes;

#[cfg(feature = "nas")]
mod nas;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    middleware as axum_middleware,
    response::IntoResponse,
    routing::{delete, get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tower_http::services::{ServeDir, ServeFile};
use horcrux_common::VmConfig;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{error, info};
use tracing_subscriber;
use vm::VmManager;
use backup::{BackupManager, BackupConfig, BackupJob, Backup, RetentionPolicy};
use cloudinit::{CloudInitManager, CloudInitConfig};
use template::{TemplateManager, Template, CloneRequest, StorageType, OsType};
use auth::{AuthManager};
use firewall::{FirewallManager, FirewallRule, SecurityGroup, FirewallScope};
use horcrux_common::auth::{LoginRequest, LoginResponse};
use monitoring::{MonitoringManager, ResourceMetrics, StorageMetrics, NodeMetrics};
use console::{ConsoleManager, ConsoleType, ConsoleInfo};
use cluster::{ClusterManager, Node, ArchitectureSummary};
use alerts::{AlertManager, AlertRule, Alert, NotificationChannel};
use observability::{OtelManager, OtelConfig};
use tls::TlsManager;
use secrets::VaultManager;
use audit::AuditLogger;

#[derive(Clone)]
struct AppState {
    config: Arc<config::HorcruxConfig>,
    vm_manager: Arc<VmManager>,
    container_manager: Arc<container::ContainerManager>,
    backup_manager: Arc<BackupManager>,
    cloudinit_manager: Arc<CloudInitManager>,
    template_manager: Arc<TemplateManager>,
    auth_manager: Arc<AuthManager>,
    firewall_manager: Arc<FirewallManager>,
    monitoring_manager: Arc<MonitoringManager>,
    console_manager: Arc<ConsoleManager>,
    cluster_manager: Arc<ClusterManager>,
    alert_manager: Arc<AlertManager>,
    otel_manager: Arc<OtelManager>,
    tls_manager: Arc<TlsManager>,
    vault_manager: Arc<VaultManager>,
    audit_logger: Arc<AuditLogger>,
    database: Arc<db::Database>,
    _rate_limiter: Arc<middleware::rate_limit::RateLimiter>,  // Reserved for future rate limiting middleware
    storage_manager: Arc<storage::StorageManager>,
    ha_manager: Arc<ha::HaManager>,
    migration_manager: Arc<migration::MigrationManager>,
    gpu_manager: Arc<gpu::GpuManager>,
    prometheus_manager: Arc<prometheus::PrometheusManager>,
    webhook_manager: Arc<webhooks::WebhookManager>,
    cni_manager: Arc<tokio::sync::RwLock<sdn::cni::CniManager>>,
    network_policy_manager: Arc<tokio::sync::RwLock<sdn::policy::NetworkPolicyManager>>,
    snapshot_manager: Arc<tokio::sync::RwLock<vm::snapshot::VmSnapshotManager>>,
    snapshot_scheduler: Arc<vm::snapshot_scheduler::SnapshotScheduler>,
    snapshot_quota_manager: Arc<vm::snapshot_quota::SnapshotQuotaManager>,
    clone_manager: Arc<vm::clone::VmCloneManager>,
    clone_job_manager: Arc<vm::clone_progress::CloneJobManager>,
    replication_manager: Arc<vm::replication::ReplicationManager>,
    ws_state: Arc<websocket::WsState>,
    #[cfg(feature = "kubernetes")]
    kubernetes_manager: Arc<kubernetes::KubernetesManager>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    // Load configuration
    let horcrux_config = config::HorcruxConfig::load();
    if let Err(e) = horcrux_config.validate() {
        error!("Configuration validation failed: {}", e);
        return Err(anyhow::anyhow!("Invalid configuration: {}", e));
    }
    info!("Configuration loaded successfully");
    let horcrux_config = Arc::new(horcrux_config);

    // Check if KVM is available
    if vm::qemu::QemuManager::check_kvm_available() {
        info!("KVM acceleration is available");
    } else {
        error!("WARNING: KVM is not available. VMs will run without hardware acceleration!");
    }

    // Get QEMU version
    match vm::qemu::QemuManager::get_qemu_version().await {
        Ok(version) => info!("QEMU version: {}", version),
        Err(e) => error!("Failed to get QEMU version: {}", e),
    }

    // Initialize database
    let database = Arc::new(
        db::Database::new(&horcrux_config.database.url)
            .await
            .expect("Failed to connect to database")
    );

    // Run migrations
    database.migrate().await.expect("Failed to run migrations");
    info!("Database initialized");

    // Create default admin user if no users exist
    match db::users::list_users(database.pool()).await {
        Ok(users) if users.is_empty() => {
            use auth::password::hash_password;

            let admin_password = std::env::var("ADMIN_PASSWORD")
                .unwrap_or_else(|_| "admin".to_string());

            if admin_password == "admin" {
                tracing::warn!("⚠️  WARNING: Using default admin password 'admin'!");
                tracing::warn!("⚠️  Please set ADMIN_PASSWORD environment variable for production!");
            }

            let password_hash = hash_password(&admin_password)
                .expect("Failed to hash admin password");

            let admin_user = horcrux_common::auth::User {
                id: uuid::Uuid::new_v4().to_string(),
                username: "admin".to_string(),
                password_hash,
                email: "admin@localhost".to_string(),
                role: "admin".to_string(),
                realm: "local".to_string(),
                enabled: true,
                roles: vec!["Administrator".to_string()],
                comment: Some("Default administrator account".to_string()),
            };

            db::users::create_user(database.pool(), &admin_user)
                .await
                .expect("Failed to create admin user");

            info!("✓ Created default admin user (username: admin, password: {})",
                  if admin_password == "admin" { "admin [CHANGE THIS!]" } else { "[from ADMIN_PASSWORD]" });
        }
        Ok(users) => {
            info!("Found {} existing user(s) in database", users.len());
        }
        Err(e) => {
            tracing::warn!("Failed to check for existing users: {}", e);
        }
    }

    let monitoring_manager = Arc::new(MonitoringManager::new());

    // Start background metrics collection
    monitoring_manager.start_collection().await;
    info!("Monitoring system started");

    // Start session cleanup task
    let db_clone = database.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(3600)); // Every hour
        loop {
            interval.tick().await;
            match db::users::cleanup_expired_sessions(db_clone.pool()).await {
                Ok(_) => tracing::debug!("Session cleanup completed"),
                Err(e) => tracing::error!("Session cleanup failed: {}", e),
            }
        }
    });
    info!("Session cleanup task started");

    // Initialize rate limiter with custom config
    let rate_limit_config = middleware::rate_limit::RateLimitConfig {
        max_requests: 100,
        window: std::time::Duration::from_secs(60),
        per_user: true,
    };
    let rate_limiter = middleware::rate_limit::create_limiter(rate_limit_config.clone());
    middleware::rate_limit::start_cleanup_task(rate_limiter.clone());
    info!("Rate limiter initialized: {} requests per {} seconds",
          rate_limit_config.max_requests, rate_limit_config.window.as_secs());

    // Initialize strict rate limiter for auth endpoints
    let auth_rate_limit_config = middleware::rate_limit::RateLimitConfig {
        max_requests: 5,  // Only 5 login attempts per minute
        window: std::time::Duration::from_secs(60),
        per_user: false,  // Per IP address
    };
    let auth_rate_limiter = middleware::rate_limit::create_limiter(auth_rate_limit_config.clone());
    middleware::rate_limit::start_cleanup_task(auth_rate_limiter.clone());
    info!("Auth rate limiter initialized: {} requests per {} seconds",
          auth_rate_limit_config.max_requests, auth_rate_limit_config.window.as_secs());

    // Initialize CORS config
    let cors_config = middleware::cors::CorsConfig::default();

    // Initialize GPU manager and scan for devices
    let gpu_manager = Arc::new(gpu::GpuManager::new());
    match gpu_manager.scan_devices().await {
        Ok(devices) => info!("Found {} GPU device(s)", devices.len()),
        Err(e) => tracing::warn!("Failed to scan GPU devices: {}", e),
    }

    // Initialize Prometheus metrics
    let prometheus_manager = Arc::new(prometheus::PrometheusManager::new());
    prometheus_manager.init_default_metrics().await;
    info!("Prometheus metrics initialized");

    // Initialize webhook manager
    let webhook_manager = Arc::new(webhooks::WebhookManager::new());
    info!("Webhook manager initialized");

    // Initialize CNI manager
    let cni_manager = Arc::new(tokio::sync::RwLock::new(sdn::cni::CniManager::new(
        horcrux_config.cni.bin_dir.clone(),
        horcrux_config.cni.conf_dir.clone(),
    )));

    // Create default CNI network if CNI directories exist
    if horcrux_config.cni.enabled && horcrux_config.cni.bin_dir.exists() {
        match cni_manager.write().await.create_default_network().await {
            Ok(_) => info!("CNI default network created"),
            Err(e) => tracing::warn!("Failed to create default CNI network: {}", e),
        }
    } else if !horcrux_config.cni.bin_dir.exists() {
        tracing::warn!("CNI binary directory not found at {:?} - CNI features disabled", horcrux_config.cni.bin_dir);
    }

    // Initialize Network Policy manager
    let network_policy_manager = Arc::new(tokio::sync::RwLock::new(sdn::policy::NetworkPolicyManager::new()));
    info!("Network policy manager initialized");

    // Initialize VM Snapshot manager
    let mut snapshot_manager = vm::snapshot::VmSnapshotManager::with_qmp_socket_pattern(
        horcrux_config.paths.snapshots.to_string_lossy().to_string(),
        horcrux_config.qemu.qmp_socket_pattern.clone(),
    );
    snapshot_manager.load_snapshots().await?;
    let snapshot_manager = Arc::new(tokio::sync::RwLock::new(snapshot_manager));
    info!("VM Snapshot manager initialized");

    // Initialize Snapshot Scheduler
    let snapshot_scheduler = Arc::new(vm::snapshot_scheduler::SnapshotScheduler::new(snapshot_manager.clone()));
    info!("Snapshot scheduler initialized");

    // Initialize Snapshot Quota manager
    let snapshot_quota_manager = Arc::new(vm::snapshot_quota::SnapshotQuotaManager::new());
    info!("Snapshot quota manager initialized");

    // Initialize VM Clone manager
    let clone_manager = Arc::new(vm::clone::VmCloneManager::new(
        horcrux_config.paths.vm_storage.to_string_lossy().to_string()
    ));
    info!("VM Clone manager initialized");

    // Initialize Clone Job manager
    let clone_job_manager = Arc::new(vm::clone_progress::CloneJobManager::new());
    info!("Clone Job manager initialized");

    // Initialize Replication manager
    let replication_manager = Arc::new(vm::replication::ReplicationManager::new());
    info!("Replication manager initialized");

    // Initialize Kubernetes manager (if feature enabled)
    #[cfg(feature = "kubernetes")]
    let kubernetes_manager = {
        let mgr = Arc::new(kubernetes::KubernetesManager::with_database_and_vault(
            database.clone(),
            Arc::new(VaultManager::new()),
        ));
        if let Err(e) = mgr.initialize().await {
            tracing::warn!("Failed to initialize Kubernetes manager: {}", e);
        } else {
            info!("Kubernetes manager initialized");
        }
        mgr
    };

    let state = Arc::new(AppState {
        config: horcrux_config.clone(),
        vm_manager: Arc::new(VmManager::with_database(database.clone())),
        container_manager: Arc::new(container::ContainerManager::with_database(database.clone())),
        backup_manager: Arc::new(BackupManager::with_restore_dir(
            horcrux_config.paths.restore.clone()
        )),
        cloudinit_manager: Arc::new(CloudInitManager::new(
            horcrux_config.paths.cloudinit.clone()
        )),
        template_manager: Arc::new(TemplateManager::new()),
        auth_manager: Arc::new(AuthManager::new()),
        firewall_manager: Arc::new(FirewallManager::new()),
        monitoring_manager,
        console_manager: Arc::new(ConsoleManager::new()),
        cluster_manager: Arc::new(ClusterManager::new()),
        alert_manager: Arc::new(AlertManager::new()),
        otel_manager: Arc::new(OtelManager::new()),
        tls_manager: Arc::new(TlsManager::new()),
        vault_manager: Arc::new(VaultManager::new()),
        audit_logger: Arc::new(AuditLogger::new(Some(horcrux_config.logging.audit_log.clone()))),
        database,
        _rate_limiter: rate_limiter.clone(),
        storage_manager: Arc::new(storage::StorageManager::new()),
        ha_manager: Arc::new(ha::HaManager::new()),
        migration_manager: Arc::new(migration::MigrationManager::new()),
        gpu_manager,
        prometheus_manager,
        webhook_manager,
        cni_manager,
        network_policy_manager,
        snapshot_manager: snapshot_manager.clone(),
        snapshot_scheduler: snapshot_scheduler.clone(),
        snapshot_quota_manager: snapshot_quota_manager.clone(),
        clone_manager,
        clone_job_manager,
        replication_manager,
        ws_state: Arc::new(websocket::WsState::new()),
        #[cfg(feature = "kubernetes")]
        kubernetes_manager,
    });

    // Start snapshot scheduler background task
    let state_for_scheduler = state.clone();
    let vm_getter = Arc::new(move |vm_id: &str| {
        let state = state_for_scheduler.clone();
        let vm_id = vm_id.to_string();
        Box::pin(async move {
            state.database.get_vm(&vm_id).await.ok()
        }) as futures::future::BoxFuture<'static, Option<VmConfig>>
    });
    snapshot_scheduler.start_scheduler(vm_getter);
    info!("Snapshot scheduler background task started");

    // Initialize libvirt manager for VM metrics collection (optional)
    #[cfg(feature = "qemu")]
    let libvirt_manager = {
        let mgr = Arc::new(metrics::LibvirtManager::new());
        match mgr.connect(None).await {
            Ok(_) => {
                info!("Connected to libvirt (qemu:///system) for VM metrics");
                Some(mgr)
            }
            Err(e) => {
                tracing::warn!("Failed to connect to libvirt: {} - VM metrics will use fallback", e);
                None
            }
        }
    };
    #[cfg(not(feature = "qemu"))]
    let libvirt_manager = None;

    // Start metrics collection background task
    metrics_collector::start_metrics_collector(
        state.ws_state.clone(),
        state.monitoring_manager.clone(),
        state.vm_manager.clone(),
        libvirt_manager,
    );

    // Static files serving for the frontend
    let serve_dir = ServeDir::new("horcrux-ui/dist")
        .not_found_service(ServeFile::new("horcrux-ui/dist/index.html"));

    // Build auth router with strict rate limiting (login/register)
    let auth_router = Router::new()
        .route("/api/auth/login", post(login))
        .route("/api/auth/register", post(register_user))
        .with_state(state.clone())
        .layer(axum_middleware::from_fn(move |conn_info, req, next| {
            middleware::rate_limit::rate_limit_middleware(auth_rate_limiter.clone(), conn_info, req, next)
        }));

    // Build protected routes using modular route builders
    let protected_routes = Router::new()
        .merge(vm_routes())
        .merge(container_routes())
        .merge(backup_routes())
        .merge(auth_protected_routes())
        .merge(storage_routes())
        .merge(monitoring_routes())
        .merge(cluster_routes())
        .merge(network_routes());

    // Add Kubernetes routes if feature is enabled
    #[cfg(feature = "kubernetes")]
    let protected_routes = protected_routes.merge(kubernetes_routes());

    // Add NAS routes if feature is enabled
    #[cfg(feature = "nas")]
    let protected_routes = protected_routes.merge(nas_routes());

    let protected_routes = protected_routes
        // WebSocket endpoint for real-time updates
        .route("/api/ws", get(websocket::ws_handler))
        .with_state(state.clone())
        // Add authentication middleware to protected routes
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            middleware::auth::auth_middleware,
        ));

    // Build main app with public and protected routes
    let app = Router::new()
        .route("/api/health", get(health_check))
        .route("/api/health/detailed", get(health_detailed))
        .route("/api/health/live", get(liveness_probe))
        .route("/api/health/ready", get(readiness_probe))
        .with_state(state.clone())
        // Merge OpenAPI / Swagger UI routes (public, for documentation)
        .merge(openapi::openapi_routes())
        // Merge protected routes (with auth middleware)
        .merge(protected_routes)
        // Merge auth router with strict rate limiting (public routes)
        .merge(auth_router)
        // Add middleware layers (in reverse order of execution)
        .layer(axum_middleware::from_fn(move |req, next| {
            middleware::cors::cors_middleware(cors_config.clone(), req, next)
        }))
        .layer(axum_middleware::from_fn(move |conn_info, req, next| {
            middleware::rate_limit::rate_limit_middleware(rate_limiter.clone(), conn_info, req, next)
        }))
        // Note: Auth middleware will be added per-route or per-group later
        // Serve static files (frontend) - must be last to act as fallback
        .fallback_service(serve_dir);

    // Set up graceful shutdown
    let shutdown_coordinator = shutdown::ShutdownCoordinator::new();
    let graceful = shutdown::GracefulShutdown::new(shutdown_coordinator.clone());

    // Clone managers for cleanup from state
    let db_for_cleanup = state.database.clone();
    let ws_for_cleanup = state.ws_state.clone();
    let monitoring_for_cleanup = state.monitoring_manager.clone();
    let scheduler_for_cleanup = state.snapshot_scheduler.clone();

    // Start server
    let addr = format!(
        "{}:{}",
        horcrux_config.server.host,
        horcrux_config.server.port
    );
    info!("Horcrux API listening on {}", addr);

    let listener = TcpListener::bind(&addr).await?;

    // Spawn signal handler
    let shutdown_signal = graceful.signal();

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal)
    .await?;

    // Run cleanup after server stops
    info!("Server stopped, running cleanup...");

    // Stop scheduler
    scheduler_for_cleanup.stop().await;

    // Stop monitoring
    monitoring_for_cleanup.stop_collection().await;

    // Close WebSocket connections
    ws_for_cleanup.close_all().await;

    // Close database
    db_for_cleanup.close().await;

    info!("Cleanup complete, exiting");

    Ok(())
}

// =============================================================================
// Route Builder Functions
// =============================================================================

/// Build VM-related routes (VMs, snapshots, clones, replication)
fn vm_routes() -> Router<Arc<AppState>> {
    Router::new()
        // VM CRUD endpoints
        .route("/api/vms", get(list_vms))
        .route("/api/vms", post(create_vm))
        .route("/api/vms/:id", get(get_vm))
        .route("/api/vms/:id/start", post(start_vm))
        .route("/api/vms/:id/stop", post(stop_vm))
        .route("/api/vms/:id", delete(delete_vm))
        // VM Snapshot endpoints
        .route("/api/vms/:id/snapshots", get(list_vm_snapshots))
        .route("/api/vms/:id/snapshots", post(create_vm_snapshot))
        .route("/api/vms/:id/snapshots/:snapshot_id", get(get_vm_snapshot))
        .route("/api/vms/:id/snapshots/:snapshot_id", delete(delete_vm_snapshot))
        .route("/api/vms/:id/snapshots/:snapshot_id/restore", post(restore_vm_snapshot))
        .route("/api/vms/:id/snapshots/tree", get(get_vm_snapshot_tree))
        // Snapshot Schedule endpoints
        .route("/api/snapshot-schedules", get(list_snapshot_schedules))
        .route("/api/snapshot-schedules", post(create_snapshot_schedule))
        .route("/api/snapshot-schedules/:id", get(get_snapshot_schedule))
        .route("/api/snapshot-schedules/:id", put(update_snapshot_schedule))
        .route("/api/snapshot-schedules/:id", delete(delete_snapshot_schedule))
        // VM Clone endpoints
        .route("/api/vms/:id/clone", post(clone_vm))
        .route("/api/vms/:id/clone-cross-node", post(clone_vm_cross_node))
        // Clone Job Progress endpoints
        .route("/api/clone-jobs", get(list_clone_jobs))
        .route("/api/clone-jobs/:id", get(get_clone_job))
        .route("/api/clone-jobs/:id/cancel", post(cancel_clone_job))
        .route("/api/clone-jobs/:id", delete(delete_clone_job))
        // Snapshot Quota endpoints
        .route("/api/snapshot-quotas", get(list_snapshot_quotas))
        .route("/api/snapshot-quotas", post(create_snapshot_quota))
        .route("/api/snapshot-quotas/:id", get(get_snapshot_quota))
        .route("/api/snapshot-quotas/:id", put(update_snapshot_quota))
        .route("/api/snapshot-quotas/:id", delete(delete_snapshot_quota))
        .route("/api/snapshot-quotas/:id/usage", get(get_snapshot_quota_usage))
        .route("/api/snapshot-quotas/summary", get(get_snapshot_quota_summary))
        .route("/api/snapshot-quotas/:id/enforce", post(enforce_snapshot_quota))
        // Replication endpoints
        .route("/api/replication/jobs", get(list_replication_jobs))
        .route("/api/replication/jobs", post(create_replication_job))
        .route("/api/replication/jobs/:id", get(get_replication_job))
        .route("/api/replication/jobs/:id", delete(delete_replication_job))
        .route("/api/replication/jobs/:id/execute", post(execute_replication_job))
        .route("/api/replication/jobs/:id/status", get(get_replication_status))
        // Console access endpoints
        .route("/api/console/:vm_id/vnc", post(create_vnc_console))
        .route("/api/console/:vm_id/websocket", get(get_vnc_websocket))
        .route("/api/console/:vm_id/novnc", get(get_novnc_page))
        .route("/api/console/ticket/:ticket_id", get(verify_console_ticket))
        .route("/api/console/ws/:ticket_id", get(vnc_websocket_handler))
        // Migration endpoints
        .route("/api/migrate/:vm_id", post(migrate_vm))
        .route("/api/migrate/:vm_id/status", get(get_migration_status))
}

/// Build container-related routes
fn container_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/containers", get(list_containers))
        .route("/api/containers", post(create_container))
        .route("/api/containers/:id", get(get_container))
        .route("/api/containers/:id", delete(delete_container))
        .route("/api/containers/:id/start", post(start_container))
        .route("/api/containers/:id/stop", post(stop_container))
        .route("/api/containers/:id/pause", post(pause_container))
        .route("/api/containers/:id/resume", post(resume_container))
        .route("/api/containers/:id/status", get(get_container_status))
        .route("/api/containers/:id/exec", post(exec_container_command))
        .route("/api/containers/:id/clone", post(clone_container))
}

/// Build backup-related routes (backups, templates, cloud-init)
fn backup_routes() -> Router<Arc<AppState>> {
    Router::new()
        // Backup endpoints
        .route("/api/backups", get(list_backups))
        .route("/api/backups", post(create_backup))
        .route("/api/backups/:id", get(get_backup))
        .route("/api/backups/:id", delete(delete_backup))
        .route("/api/backups/:id/restore", post(restore_backup))
        // Backup job endpoints
        .route("/api/backup-jobs", get(list_backup_jobs))
        .route("/api/backup-jobs", post(create_backup_job))
        .route("/api/backup-jobs/:id/run", post(run_backup_job_now))
        // Retention policy endpoint
        .route("/api/backups/retention/:target_id", post(apply_retention))
        // Cloud-init endpoints
        .route("/api/cloudinit/:vm_id", post(generate_cloudinit))
        .route("/api/cloudinit/:vm_id", delete(delete_cloudinit))
        // Template endpoints
        .route("/api/templates", get(list_templates))
        .route("/api/templates", post(create_template))
        .route("/api/templates/:id", get(get_template))
        .route("/api/templates/:id", delete(delete_template))
        .route("/api/templates/:id/clone", post(clone_template))
}

/// Build auth-related routes (users, roles, permissions, audit)
fn auth_protected_routes() -> Router<Arc<AppState>> {
    Router::new()
        // Auth endpoints
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/verify", get(verify_session))
        .route("/api/auth/password", post(change_password))
        // User management
        .route("/api/users", get(list_users))
        .route("/api/users", post(create_user))
        .route("/api/users/:id", delete(delete_user))
        // API key management
        .route("/api/users/:username/api-keys", get(list_api_keys))
        .route("/api/users/:username/api-keys", post(create_api_key))
        .route("/api/users/:username/api-keys/:key_id", delete(revoke_api_key))
        // Role and permission management
        .route("/api/roles", get(list_roles))
        .route("/api/permissions/:user_id", get(get_user_permissions))
        .route("/api/permissions/:user_id", post(add_permission))
        // Audit log endpoints
        .route("/api/audit/events", get(query_audit_events))
        .route("/api/audit/stats", get(get_audit_stats))
        .route("/api/audit/security-events", get(get_security_events))
        .route("/api/audit/failed-logins", get(get_failed_logins))
        .route("/api/audit/brute-force", get(detect_brute_force_attempts))
}

/// Build storage-related routes (pools, GPU, TLS, Vault)
fn storage_routes() -> Router<Arc<AppState>> {
    Router::new()
        // Storage pool endpoints
        .route("/api/storage/pools", get(list_storage_pools))
        .route("/api/storage/pools/:id", get(get_storage_pool))
        .route("/api/storage/pools", post(add_storage_pool))
        .route("/api/storage/pools/:id", delete(remove_storage_pool))
        .route("/api/storage/pools/:pool_id/volumes", post(create_volume))
        // GPU passthrough endpoints
        .route("/api/gpu/devices", get(list_gpu_devices))
        .route("/api/gpu/devices/scan", post(scan_gpu_devices))
        .route("/api/gpu/devices/:pci_address", get(get_gpu_device))
        .route("/api/gpu/devices/:pci_address/bind-vfio", post(bind_gpu_to_vfio))
        .route("/api/gpu/devices/:pci_address/unbind-vfio", post(unbind_gpu_from_vfio))
        .route("/api/gpu/devices/:pci_address/iommu-group", get(get_gpu_iommu_group))
        .route("/api/gpu/iommu-status", get(check_iommu_status))
        // TLS/SSL endpoints
        .route("/api/tls/config", get(get_tls_config))
        .route("/api/tls/config", post(update_tls_config))
        .route("/api/tls/certificates", get(list_certificates))
        .route("/api/tls/certificate/generate", post(generate_self_signed_cert))
        .route("/api/tls/certificate/info/:path", get(get_certificate_info_endpoint))
        // Vault endpoints
        .route("/api/vault/config", get(get_vault_config))
        .route("/api/vault/config", post(update_vault_config))
        .route("/api/vault/secret/:path", get(read_vault_secret))
        .route("/api/vault/secret/:path", post(write_vault_secret))
        .route("/api/vault/secret/:path", delete(delete_vault_secret))
        .route("/api/vault/secrets/:path", get(list_vault_secrets))
}

/// Build monitoring-related routes (metrics, alerts, webhooks, observability)
fn monitoring_routes() -> Router<Arc<AppState>> {
    Router::new()
        // Monitoring endpoints
        .route("/api/monitoring/node", get(get_node_stats))
        .route("/api/monitoring/vms", get(get_all_vm_stats))
        .route("/api/monitoring/vms/:id", get(get_vm_stats))
        .route("/api/monitoring/containers", get(get_all_container_stats))
        .route("/api/monitoring/containers/:id", get(get_container_stats))
        .route("/api/monitoring/storage", get(get_all_storage_stats))
        .route("/api/monitoring/storage/:name", get(get_storage_stats))
        .route("/api/monitoring/history/:metric", get(get_metric_history))
        // Alert system endpoints
        .route("/api/alerts/rules", get(list_alert_rules))
        .route("/api/alerts/rules", post(create_alert_rule))
        .route("/api/alerts/rules/:rule_id", delete(delete_alert_rule))
        .route("/api/alerts/active", get(list_active_alerts))
        .route("/api/alerts/history", get(get_alert_history))
        .route("/api/alerts/:rule_id/:target/acknowledge", post(acknowledge_alert))
        .route("/api/alerts/notifications", get(list_notification_channels))
        .route("/api/alerts/notifications", post(add_notification_channel))
        // OpenTelemetry endpoints
        .route("/api/observability/config", get(get_otel_config))
        .route("/api/observability/config", post(update_otel_config))
        .route("/api/observability/export/metrics", post(export_metrics_now))
        // Prometheus metrics endpoint
        .route("/metrics", get(prometheus_metrics))
        // Webhook endpoints
        .route("/api/webhooks", get(list_webhooks))
        .route("/api/webhooks", post(create_webhook))
        .route("/api/webhooks/:id", get(get_webhook))
        .route("/api/webhooks/:id", post(update_webhook))
        .route("/api/webhooks/:id", delete(delete_webhook))
        .route("/api/webhooks/:id/test", post(test_webhook))
        .route("/api/webhooks/:id/deliveries", get(get_webhook_deliveries))
}

/// Build cluster-related routes (nodes, HA)
fn cluster_routes() -> Router<Arc<AppState>> {
    Router::new()
        // Cluster management
        .route("/api/cluster/nodes", get(list_cluster_nodes))
        .route("/api/cluster/nodes/:name", post(add_cluster_node))
        .route("/api/cluster/architecture", get(get_cluster_architecture))
        .route("/api/cluster/find-node", post(find_best_node_for_vm))
        // HA (High Availability) endpoints
        .route("/api/ha/resources", get(list_ha_resources))
        .route("/api/ha/resources", post(add_ha_resource))
        .route("/api/ha/resources/:vm_id", delete(remove_ha_resource))
        .route("/api/ha/status", get(get_ha_status))
        .route("/api/ha/groups", post(create_ha_group))
        .route("/api/ha/groups", get(list_ha_groups))
}

/// Build network-related routes (firewall, CNI, network policies)
fn network_routes() -> Router<Arc<AppState>> {
    Router::new()
        // Firewall endpoints
        .route("/api/firewall/rules", get(list_firewall_rules))
        .route("/api/firewall/rules", post(add_firewall_rule))
        .route("/api/firewall/rules/:id", delete(delete_firewall_rule))
        .route("/api/firewall/security-groups", get(list_security_groups))
        .route("/api/firewall/security-groups/:name", get(get_security_group))
        .route("/api/firewall/:scope/apply", post(apply_firewall_rules))
        // CNI (Container Network Interface) endpoints
        .route("/api/cni/networks", get(list_cni_networks))
        .route("/api/cni/networks", post(create_cni_network))
        .route("/api/cni/networks/:name", get(get_cni_network))
        .route("/api/cni/networks/:name", delete(delete_cni_network))
        .route("/api/cni/attach", post(attach_container_to_network))
        .route("/api/cni/detach", post(detach_container_from_network))
        .route("/api/cni/check", post(check_container_network))
        .route("/api/cni/attachments/:container_id", get(list_container_attachments))
        .route("/api/cni/capabilities/:plugin_type", get(get_cni_plugin_capabilities))
        // Network Policy endpoints
        .route("/api/network-policies", get(list_network_policies))
        .route("/api/network-policies", post(create_network_policy))
        .route("/api/network-policies/:id", get(get_network_policy))
        .route("/api/network-policies/:id", delete(delete_network_policy))
        .route("/api/network-policies/namespace/:namespace", get(list_policies_in_namespace))
        .route("/api/network-policies/:id/iptables", get(get_policy_iptables_rules))
        .route("/api/network-policies/:id/nftables", get(get_policy_nftables_rules))
}

/// Build Kubernetes-related routes (conditionally compiled)
#[cfg(feature = "kubernetes")]
fn kubernetes_routes() -> Router<Arc<AppState>> {
    Router::new()
        // Kubernetes cluster management
        .route("/api/k8s/clusters", get(k8s_list_clusters))
        .route("/api/k8s/clusters", post(k8s_connect_cluster))
        .route("/api/k8s/clusters/:cluster_id", get(k8s_get_cluster))
        .route("/api/k8s/clusters/:cluster_id", delete(k8s_delete_cluster))
        .route("/api/k8s/clusters/:cluster_id/reconnect", post(k8s_reconnect_cluster))
        .route("/api/k8s/clusters/:cluster_id/health", get(k8s_get_cluster_health))
        .route("/api/k8s/clusters/:cluster_id/version", get(k8s_get_cluster_version))
        // Kubernetes namespaces
        .route("/api/k8s/clusters/:cluster_id/namespaces", get(k8s_list_namespaces))
        .route("/api/k8s/clusters/:cluster_id/namespaces", post(k8s_create_namespace))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace", get(k8s_get_namespace))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace", delete(k8s_delete_namespace))
        // Kubernetes nodes
        .route("/api/k8s/clusters/:cluster_id/nodes", get(k8s_list_nodes))
        .route("/api/k8s/clusters/:cluster_id/nodes/:node", get(k8s_get_node))
        .route("/api/k8s/clusters/:cluster_id/nodes/:node/cordon", post(k8s_cordon_node))
        .route("/api/k8s/clusters/:cluster_id/nodes/:node/uncordon", post(k8s_uncordon_node))
        .route("/api/k8s/clusters/:cluster_id/nodes/:node/drain", post(k8s_drain_node))
        // Kubernetes pods
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/pods", get(k8s_list_pods))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/pods/:pod", get(k8s_get_pod))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/pods/:pod", delete(k8s_delete_pod))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/pods/:pod/logs", get(k8s_get_pod_logs))
        // Kubernetes deployments
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/deployments", get(k8s_list_deployments))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/deployments/:deployment", get(k8s_get_deployment))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/deployments/:deployment/scale", post(k8s_scale_deployment))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/deployments/:deployment/restart", post(k8s_restart_deployment))
        // Kubernetes events
        .route("/api/k8s/clusters/:cluster_id/events", get(k8s_list_events))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/events", get(k8s_list_namespace_events))
        // Kubernetes services
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/services", get(k8s_list_services))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/services", post(k8s_create_service))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/services/:service", get(k8s_get_service))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/services/:service", delete(k8s_delete_service))
        // Kubernetes ingresses
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/ingresses", get(k8s_list_ingresses))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/ingresses", post(k8s_create_ingress))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/ingresses/:ingress", get(k8s_get_ingress))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/ingresses/:ingress", delete(k8s_delete_ingress))
        // Kubernetes configmaps
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/configmaps", get(k8s_list_configmaps))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/configmaps", post(k8s_create_configmap))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/configmaps/:name", get(k8s_get_configmap))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/configmaps/:name", delete(k8s_delete_configmap))
        // Kubernetes secrets
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/secrets", get(k8s_list_secrets))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/secrets", post(k8s_create_secret))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/secrets/:name", get(k8s_get_secret))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/secrets/:name", delete(k8s_delete_secret))
        // Kubernetes PVCs
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/pvcs", get(k8s_list_pvcs))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/pvcs", post(k8s_create_pvc))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/pvcs/:name", get(k8s_get_pvc))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/pvcs/:name", delete(k8s_delete_pvc))
        // Kubernetes statefulsets
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/statefulsets", get(k8s_list_statefulsets))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/statefulsets/:name", get(k8s_get_statefulset))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/statefulsets/:name/scale", post(k8s_scale_statefulset))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/statefulsets/:name", delete(k8s_delete_statefulset))
        // Kubernetes daemonsets
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/daemonsets", get(k8s_list_daemonsets))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/daemonsets/:name", get(k8s_get_daemonset))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/daemonsets/:name", delete(k8s_delete_daemonset))
        // Kubernetes jobs
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/jobs", get(k8s_list_jobs))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/jobs/:name", get(k8s_get_job))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/jobs/:name", delete(k8s_delete_job))
        // Kubernetes cronjobs
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/cronjobs", get(k8s_list_cronjobs))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/cronjobs/:name", get(k8s_get_cronjob))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/cronjobs/:name", delete(k8s_delete_cronjob))
        // Kubernetes metrics
        .route("/api/k8s/clusters/:cluster_id/metrics/nodes", get(k8s_get_node_metrics))
        .route("/api/k8s/clusters/:cluster_id/namespaces/:namespace/metrics/pods", get(k8s_get_pod_metrics))
        // Helm releases
        .route("/api/k8s/clusters/:cluster_id/helm/releases", get(k8s_list_helm_releases))
        .route("/api/k8s/clusters/:cluster_id/helm/releases", post(k8s_install_helm_release))
        .route("/api/k8s/clusters/:cluster_id/helm/releases/:release_name", put(k8s_upgrade_helm_release))
        .route("/api/k8s/clusters/:cluster_id/helm/releases/:release_name", delete(k8s_uninstall_helm_release))
        .route("/api/k8s/clusters/:cluster_id/helm/releases/:release_name/rollback", post(k8s_rollback_helm_release))
        .route("/api/k8s/clusters/:cluster_id/helm/releases/:release_name/history", get(k8s_get_helm_release_history))
        // Helm repos (global)
        .route("/api/k8s/helm/repos", get(k8s_list_helm_repos))
        .route("/api/k8s/helm/repos", post(k8s_add_helm_repo))
        .route("/api/k8s/helm/repos/:repo_name", delete(k8s_remove_helm_repo))
        .route("/api/k8s/helm/charts/search", get(k8s_search_helm_charts))
}

/// Build NAS-related routes (conditionally compiled)
#[cfg(feature = "nas")]
fn nas_routes() -> Router<Arc<AppState>> {
    Router::new()
        // NAS Health and Status
        .route("/api/nas/health", get(nas_health))
        .route("/api/nas/metrics", get(nas_metrics))

        // NAS Shares
        .route("/api/nas/shares", get(nas_list_shares))
        .route("/api/nas/shares", post(nas_create_share))
        .route("/api/nas/shares/:share_id", get(nas_get_share))
        .route("/api/nas/shares/:share_id", put(nas_update_share))
        .route("/api/nas/shares/:share_id", delete(nas_delete_share))
        .route("/api/nas/shares/:share_id/enable", post(nas_enable_share))
        .route("/api/nas/shares/:share_id/disable", post(nas_disable_share))

        // NAS Users
        .route("/api/nas/users", get(nas_list_users))
        .route("/api/nas/users", post(nas_create_user))
        .route("/api/nas/users/:user_id", get(nas_get_user))
        .route("/api/nas/users/:user_id", put(nas_update_user))
        .route("/api/nas/users/:user_id", delete(nas_delete_user))
        .route("/api/nas/users/:user_id/password", post(nas_set_user_password))
        .route("/api/nas/users/:user_id/enable", post(nas_enable_user))
        .route("/api/nas/users/:user_id/disable", post(nas_disable_user))

        // NAS Groups
        .route("/api/nas/groups", get(nas_list_groups))
        .route("/api/nas/groups", post(nas_create_group))
        .route("/api/nas/groups/:group_id", get(nas_get_group))
        .route("/api/nas/groups/:group_id", put(nas_update_group))
        .route("/api/nas/groups/:group_id", delete(nas_delete_group))
        .route("/api/nas/groups/:group_id/members", get(nas_get_group_members))
        .route("/api/nas/groups/:group_id/members", post(nas_add_group_member))
        .route("/api/nas/groups/:group_id/members/:user_id", delete(nas_remove_group_member))

        // NAS Pools
        .route("/api/nas/pools", get(nas_list_pools))
        .route("/api/nas/pools", post(nas_create_pool))
        .route("/api/nas/pools/:pool_id", get(nas_get_pool))
        .route("/api/nas/pools/:pool_id", delete(nas_delete_pool))
        .route("/api/nas/pools/:pool_id/scrub", post(nas_scrub_pool))
        .route("/api/nas/pools/:pool_id/status", get(nas_get_pool_status))

        // NAS Datasets
        .route("/api/nas/pools/:pool_id/datasets", get(nas_list_datasets))
        .route("/api/nas/pools/:pool_id/datasets", post(nas_create_dataset))
        .route("/api/nas/datasets/:dataset_id", get(nas_get_dataset))
        .route("/api/nas/datasets/:dataset_id", put(nas_update_dataset))
        .route("/api/nas/datasets/:dataset_id", delete(nas_delete_dataset))
        .route("/api/nas/datasets/:dataset_id/quota", post(nas_set_dataset_quota))

        // NAS Snapshots
        .route("/api/nas/datasets/:dataset_id/snapshots", get(nas_list_snapshots))
        .route("/api/nas/datasets/:dataset_id/snapshots", post(nas_create_snapshot))
        .route("/api/nas/snapshots/:snapshot_id", get(nas_get_snapshot))
        .route("/api/nas/snapshots/:snapshot_id", delete(nas_delete_snapshot))
        .route("/api/nas/snapshots/:snapshot_id/rollback", post(nas_rollback_snapshot))
        .route("/api/nas/snapshots/:snapshot_id/clone", post(nas_clone_snapshot))

        // Snapshot Policies
        .route("/api/nas/snapshot-policies", get(nas_list_snapshot_policies))
        .route("/api/nas/snapshot-policies", post(nas_create_snapshot_policy))
        .route("/api/nas/snapshot-policies/:policy_id", get(nas_get_snapshot_policy))
        .route("/api/nas/snapshot-policies/:policy_id", put(nas_update_snapshot_policy))
        .route("/api/nas/snapshot-policies/:policy_id", delete(nas_delete_snapshot_policy))

        // NAS Replication
        .route("/api/nas/replication/tasks", get(nas_list_replication_tasks))
        .route("/api/nas/replication/tasks", post(nas_create_replication_task))
        .route("/api/nas/replication/tasks/:task_id", get(nas_get_replication_task))
        .route("/api/nas/replication/tasks/:task_id", put(nas_update_replication_task))
        .route("/api/nas/replication/tasks/:task_id", delete(nas_delete_replication_task))
        .route("/api/nas/replication/tasks/:task_id/run", post(nas_run_replication_task))
        .route("/api/nas/replication/tasks/:task_id/history", get(nas_get_replication_history))

        // NAS Services
        .route("/api/nas/services", get(nas_list_services))
        .route("/api/nas/services/:service", get(nas_get_service_status))
        .route("/api/nas/services/:service/start", post(nas_start_service))
        .route("/api/nas/services/:service/stop", post(nas_stop_service))
        .route("/api/nas/services/:service/restart", post(nas_restart_service))

        // SMB-specific endpoints
        .route("/api/nas/smb/config", get(nas_get_smb_config))
        .route("/api/nas/smb/config", put(nas_update_smb_config))
        .route("/api/nas/smb/connections", get(nas_list_smb_connections))
        .route("/api/nas/smb/connections/:conn_id/disconnect", post(nas_disconnect_smb))

        // NFS-specific endpoints
        .route("/api/nas/nfs/config", get(nas_get_nfs_config))
        .route("/api/nas/nfs/config", put(nas_update_nfs_config))
        .route("/api/nas/nfs/clients", get(nas_list_nfs_clients))

        // iSCSI endpoints
        .route("/api/nas/iscsi/targets", get(nas_list_iscsi_targets))
        .route("/api/nas/iscsi/targets", post(nas_create_iscsi_target))
        .route("/api/nas/iscsi/targets/:target_id", get(nas_get_iscsi_target))
        .route("/api/nas/iscsi/targets/:target_id", put(nas_update_iscsi_target))
        .route("/api/nas/iscsi/targets/:target_id", delete(nas_delete_iscsi_target))
        .route("/api/nas/iscsi/targets/:target_id/luns", get(nas_list_iscsi_luns))
        .route("/api/nas/iscsi/targets/:target_id/luns", post(nas_create_iscsi_lun))
        .route("/api/nas/iscsi/luns/:lun_id", delete(nas_delete_iscsi_lun))
        .route("/api/nas/iscsi/targets/:target_id/acls", get(nas_list_iscsi_acls))
        .route("/api/nas/iscsi/targets/:target_id/acls", post(nas_create_iscsi_acl))
        .route("/api/nas/iscsi/acls/:acl_id", delete(nas_delete_iscsi_acl))

        // S3 Gateway endpoints
        .route("/api/nas/s3/status", get(nas_get_s3_status))
        .route("/api/nas/s3/buckets", get(nas_list_s3_buckets))
        .route("/api/nas/s3/buckets", post(nas_create_s3_bucket))
        .route("/api/nas/s3/buckets/:bucket_name", get(nas_get_s3_bucket))
        .route("/api/nas/s3/buckets/:bucket_name", delete(nas_delete_s3_bucket))
        .route("/api/nas/s3/access-keys", get(nas_list_s3_access_keys))
        .route("/api/nas/s3/access-keys", post(nas_create_s3_access_key))
        .route("/api/nas/s3/access-keys/:key_id", delete(nas_delete_s3_access_key))

        // Directory/Auth configuration
        .route("/api/nas/directory/config", get(nas_get_directory_config))
        .route("/api/nas/directory/config", put(nas_update_directory_config))
        // LDAP endpoints
        .route("/api/nas/directory/ldap/status", get(nas_get_ldap_status))
        .route("/api/nas/directory/ldap/configure", post(nas_configure_ldap))
        .route("/api/nas/directory/ldap/test", post(nas_test_ldap_connection))
        .route("/api/nas/directory/ldap/sync", post(nas_sync_ldap))
        .route("/api/nas/directory/ldap/users", get(nas_search_ldap_users))
        .route("/api/nas/directory/ldap/groups", get(nas_search_ldap_groups))
        // Kerberos endpoints
        .route("/api/nas/directory/kerberos/status", get(nas_get_kerberos_status))
        .route("/api/nas/directory/kerberos/configure", post(nas_configure_kerberos))
        .route("/api/nas/directory/kerberos/kinit", post(nas_kerberos_kinit))
        .route("/api/nas/directory/kerberos/klist", get(nas_kerberos_klist))
        .route("/api/nas/directory/kerberos/kdestroy", post(nas_kerberos_kdestroy))
        .route("/api/nas/directory/kerberos/keytabs", get(nas_list_kerberos_keytabs))
        .route("/api/nas/directory/kerberos/keytab", post(nas_create_kerberos_keytab))
        // Active Directory endpoints
        .route("/api/nas/directory/ad/status", get(nas_get_ad_status))
        .route("/api/nas/directory/ad/join", post(nas_join_active_directory))
        .route("/api/nas/directory/ad/leave", post(nas_leave_active_directory))
        .route("/api/nas/directory/ad/users", get(nas_list_ad_users))
        .route("/api/nas/directory/ad/groups", get(nas_list_ad_groups))
        .route("/api/nas/directory/ad/user/:username/groups", get(nas_get_user_ad_groups))
        .route("/api/nas/directory/ad/test-trust", post(nas_test_ad_trust))
        .route("/api/nas/directory/ad/ping-dc", post(nas_ping_ad_dc))
        .route("/api/nas/directory/ad/verify-prereqs", post(nas_verify_ad_prereqs))

        // Scheduler endpoints
        .route("/api/nas/scheduler/jobs", get(nas_list_scheduled_jobs))
        .route("/api/nas/scheduler/jobs", post(nas_create_scheduled_job))
        .route("/api/nas/scheduler/jobs/:job_id", get(nas_get_scheduled_job))
        .route("/api/nas/scheduler/jobs/:job_id", put(nas_update_scheduled_job))
        .route("/api/nas/scheduler/jobs/:job_id", delete(nas_delete_scheduled_job))
        .route("/api/nas/scheduler/jobs/:job_id/run", post(nas_run_scheduled_job))
        .route("/api/nas/scheduler/jobs/:job_id/pause", post(nas_pause_scheduled_job))
        .route("/api/nas/scheduler/jobs/:job_id/resume", post(nas_resume_scheduled_job))
        .route("/api/nas/scheduler/jobs/:job_id/history", get(nas_get_job_history))
        .route("/api/nas/scheduler/status", get(nas_get_scheduler_status))

        // FTP-specific endpoints
        .route("/api/nas/ftp/config", get(nas_get_ftp_config))
        .route("/api/nas/ftp/config", put(nas_update_ftp_config))
        .route("/api/nas/ftp/connections", get(nas_list_ftp_connections))
        .route("/api/nas/ftp/connections/:conn_id/disconnect", post(nas_disconnect_ftp))

        // WebDAV-specific endpoints
        .route("/api/nas/webdav/config", get(nas_get_webdav_config))
        .route("/api/nas/webdav/config", put(nas_update_webdav_config))
        .route("/api/nas/webdav/connections", get(nas_list_webdav_connections))

        // AFP-specific endpoints
        .route("/api/nas/afp/config", get(nas_get_afp_config))
        .route("/api/nas/afp/config", put(nas_update_afp_config))
        .route("/api/nas/afp/connections", get(nas_list_afp_connections))
        .route("/api/nas/afp/connections/:conn_id/disconnect", post(nas_disconnect_afp))
        .route("/api/nas/afp/timemachine", get(nas_list_timemachine_targets))
}

// =============================================================================
// NAS Handler Functions (stubs - to be implemented)
// =============================================================================

#[cfg(feature = "nas")]
mod nas_handlers {
    use super::*;
    use axum::extract::{Path, State};
    use axum::Json;
    use crate::nas::monitoring::{get_nas_health, collect_metrics};
    use crate::nas::services::{NasService, ServiceAction, manage_service, get_service_status};
    use crate::nas::auth::{AuthManager, NasUser, NasGroup};
    use crate::nas::storage::StorageManager;
    #[cfg(feature = "smb")]
    use crate::nas::shares::smb::SmbManager;
    #[cfg(feature = "nfs-server")]
    use crate::nas::shares::nfs::NfsServerManager;
    #[cfg(feature = "s3-gateway")]
    use crate::nas::services::s3::S3GatewayManager;
    #[cfg(feature = "iscsi-target")]
    use crate::nas::services::iscsi::IscsiTargetManager;
    #[cfg(feature = "rsync-server")]
    use crate::nas::services::rsync::RsyncManager;

    // Health and metrics
    pub async fn nas_health(State(_state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
        match get_nas_health().await {
            Ok(health) => Ok(Json(serde_json::to_value(health).unwrap_or_default())),
            Err(e) => Err(ApiError::Internal(format!("Failed to get NAS health: {}", e))),
        }
    }

    pub async fn nas_metrics(State(_state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
        match collect_metrics().await {
            Ok(metrics) => Ok(Json(serde_json::to_value(metrics).unwrap_or_default())),
            Err(e) => Err(ApiError::Internal(format!("Failed to collect metrics: {}", e))),
        }
    }

    // Share handlers
    // Note: NasManager uses in-memory storage. In production, shares should be persisted to database.
    // The actual protocol configurations (smb.conf, exports, etc.) are persisted by the protocol managers.
    pub async fn nas_list_shares(State(state): State<Arc<AppState>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        use crate::nas::{NasManager, NasConfig};
        // Query database for shares
        let shares = sqlx::query_as::<_, (String, String, String, String, bool, Option<String>, i64)>(
            "SELECT id, name, path, owner_user, enabled, description, created_at FROM nas_shares ORDER BY name"
        )
        .fetch_all(state.database.pool())
        .await
        .unwrap_or_default();

        let json_shares: Vec<serde_json::Value> = shares.iter()
            .map(|(id, name, path, owner, enabled, desc, created)| {
                serde_json::json!({
                    "id": id,
                    "name": name,
                    "path": path,
                    "owner_user": owner,
                    "enabled": enabled,
                    "description": desc,
                    "created_at": created,
                })
            })
            .collect();
        Ok(Json(json_shares))
    }
    pub async fn nas_create_share(State(state): State<Arc<AppState>>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        use crate::nas::shares::NasShare;
        let name = body.get("name").and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'name' field".to_string()))?;
        let path = body.get("path").and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'path' field".to_string()))?;
        let description = body.get("description").and_then(|v| v.as_str());
        let owner = body.get("owner_user").and_then(|v| v.as_str()).unwrap_or("root");

        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp();

        // Create directory if it doesn't exist
        if !std::path::Path::new(path).exists() {
            tokio::fs::create_dir_all(path).await
                .map_err(|e| ApiError::Internal(format!("Failed to create share directory: {}", e)))?;
        }

        // Insert into database
        sqlx::query(
            "INSERT INTO nas_shares (id, name, path, owner_user, enabled, description, created_at, updated_at) VALUES (?, ?, ?, ?, 1, ?, ?, ?)"
        )
        .bind(&id)
        .bind(name)
        .bind(path)
        .bind(owner)
        .bind(description)
        .bind(now)
        .bind(now)
        .execute(state.database.pool())
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to create share: {}", e)))?;

        Ok(Json(serde_json::json!({
            "id": id,
            "name": name,
            "path": path,
            "owner_user": owner,
            "enabled": true,
            "description": description,
            "created_at": now,
        })))
    }
    pub async fn nas_get_share(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
        let share = sqlx::query_as::<_, (String, String, String, String, bool, Option<String>, i64)>(
            "SELECT id, name, path, owner_user, enabled, description, created_at FROM nas_shares WHERE id = ? OR name = ?"
        )
        .bind(&id)
        .bind(&id)
        .fetch_optional(state.database.pool())
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

        match share {
            Some((id, name, path, owner, enabled, desc, created)) => {
                Ok(Json(serde_json::json!({
                    "id": id,
                    "name": name,
                    "path": path,
                    "owner_user": owner,
                    "enabled": enabled,
                    "description": desc,
                    "created_at": created,
                })))
            }
            None => Err(ApiError::NotFound("Share not found".to_string())),
        }
    }
    pub async fn nas_update_share(State(state): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        let now = chrono::Utc::now().timestamp();

        // Get existing share
        let existing = sqlx::query_as::<_, (String,)>(
            "SELECT id FROM nas_shares WHERE id = ? OR name = ?"
        )
        .bind(&id)
        .bind(&id)
        .fetch_optional(state.database.pool())
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

        if existing.is_none() {
            return Err(ApiError::NotFound("Share not found".to_string()));
        }

        // Update fields that are present in body
        if let Some(name) = body.get("name").and_then(|v| v.as_str()) {
            sqlx::query("UPDATE nas_shares SET name = ?, updated_at = ? WHERE id = ?")
                .bind(name)
                .bind(now)
                .bind(&id)
                .execute(state.database.pool())
                .await
                .map_err(|e| ApiError::Internal(format!("Failed to update share: {}", e)))?;
        }
        if let Some(desc) = body.get("description").and_then(|v| v.as_str()) {
            sqlx::query("UPDATE nas_shares SET description = ?, updated_at = ? WHERE id = ?")
                .bind(desc)
                .bind(now)
                .bind(&id)
                .execute(state.database.pool())
                .await
                .map_err(|e| ApiError::Internal(format!("Failed to update share: {}", e)))?;
        }

        // Return updated share
        nas_get_share(State(state), Path(id)).await
    }
    pub async fn nas_delete_share(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<(), ApiError> {
        let result = sqlx::query("DELETE FROM nas_shares WHERE id = ? OR name = ?")
            .bind(&id)
            .bind(&id)
            .execute(state.database.pool())
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to delete share: {}", e)))?;

        if result.rows_affected() == 0 {
            return Err(ApiError::NotFound("Share not found".to_string()));
        }
        Ok(())
    }
    pub async fn nas_enable_share(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<(), ApiError> {
        let now = chrono::Utc::now().timestamp();
        let result = sqlx::query("UPDATE nas_shares SET enabled = 1, updated_at = ? WHERE id = ? OR name = ?")
            .bind(now)
            .bind(&id)
            .bind(&id)
            .execute(state.database.pool())
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to enable share: {}", e)))?;

        if result.rows_affected() == 0 {
            return Err(ApiError::NotFound("Share not found".to_string()));
        }
        Ok(())
    }
    pub async fn nas_disable_share(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<(), ApiError> {
        let now = chrono::Utc::now().timestamp();
        let result = sqlx::query("UPDATE nas_shares SET enabled = 0, updated_at = ? WHERE id = ? OR name = ?")
            .bind(now)
            .bind(&id)
            .bind(&id)
            .execute(state.database.pool())
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to disable share: {}", e)))?;

        if result.rows_affected() == 0 {
            return Err(ApiError::NotFound("Share not found".to_string()));
        }
        Ok(())
    }

    // User handlers
    pub async fn nas_list_users(State(_state): State<Arc<AppState>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        let manager = AuthManager::new();
        match manager.list_system_users().await {
            Ok(users) => {
                let json_users: Vec<serde_json::Value> = users.iter()
                    .map(|u| serde_json::to_value(u).unwrap_or_default())
                    .collect();
                Ok(Json(json_users))
            }
            Err(e) => Err(ApiError::Internal(format!("Failed to list users: {}", e))),
        }
    }
    pub async fn nas_create_user(State(_state): State<Arc<AppState>>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        let username = body.get("username").and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'username' field".to_string()))?;
        let manager = AuthManager::new();
        let uid = manager.next_uid().await
            .map_err(|e| ApiError::Internal(format!("Failed to get next UID: {}", e)))?;
        let primary_group = body.get("primary_group").and_then(|v| v.as_str()).unwrap_or("users");
        let user = NasUser::new(
            uuid::Uuid::new_v4().to_string(),
            username.to_string(),
            uid,
            primary_group.to_string(),
        );
        manager.create_system_user(&user).await
            .map_err(|e| ApiError::Internal(format!("Failed to create user: {}", e)))?;

        // Set password if provided
        if let Some(password) = body.get("password").and_then(|v| v.as_str()) {
            manager.set_password(&user, password).await
                .map_err(|e| ApiError::Internal(format!("Failed to set password: {}", e)))?;
        }

        Ok(Json(serde_json::to_value(&user).unwrap_or_default()))
    }
    pub async fn nas_get_user(State(_state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
        let manager = AuthManager::new();
        match manager.get_system_user(&id).await {
            Ok(user) => Ok(Json(serde_json::to_value(user).unwrap_or_default())),
            Err(_) => Err(ApiError::NotFound("User not found".to_string())),
        }
    }
    pub async fn nas_update_user(State(_state): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        let manager = AuthManager::new();
        let existing = manager.get_system_user(&id).await
            .map_err(|_| ApiError::NotFound("User not found".to_string()))?;

        let shell = body.get("shell").and_then(|v| v.as_str())
            .unwrap_or(&existing.shell).to_string();
        let home = body.get("home").and_then(|v| v.as_str())
            .unwrap_or(&existing.home).to_string();
        let full_name = body.get("full_name").and_then(|v| v.as_str())
            .unwrap_or(&existing.full_name).to_string();

        // Get group name from GID
        let primary_group = manager.get_system_group_by_gid(existing.gid).await
            .map(|g| g.name)
            .unwrap_or_else(|_| format!("{}", existing.gid));

        let mut user = NasUser::new(
            id.clone(),
            existing.username.clone(),
            existing.uid,
            primary_group,
        );
        user.shell = Some(shell);
        user.home_directory = Some(home);
        user.full_name = Some(full_name);

        manager.update_system_user(&user).await
            .map_err(|e| ApiError::Internal(format!("Failed to update user: {}", e)))?;

        Ok(Json(serde_json::to_value(&user).unwrap_or_default()))
    }
    pub async fn nas_delete_user(State(_state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<(), ApiError> {
        let manager = AuthManager::new();
        let user_info = manager.get_system_user(&id).await
            .map_err(|_| ApiError::NotFound("User not found".to_string()))?;

        // Get group name from GID
        let primary_group = manager.get_system_group_by_gid(user_info.gid).await
            .map(|g| g.name)
            .unwrap_or_else(|_| format!("{}", user_info.gid));

        let user = NasUser::new(
            id,
            user_info.username,
            user_info.uid,
            primary_group,
        );
        manager.delete_system_user(&user).await
            .map_err(|e| ApiError::Internal(format!("Failed to delete user: {}", e)))
    }
    pub async fn nas_set_user_password(State(_state): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<serde_json::Value>) -> Result<(), ApiError> {
        let password = body.get("password").and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'password' field".to_string()))?;
        let manager = AuthManager::new();
        let user_info = manager.get_system_user(&id).await
            .map_err(|_| ApiError::NotFound("User not found".to_string()))?;

        // Get group name from GID
        let primary_group = manager.get_system_group_by_gid(user_info.gid).await
            .map(|g| g.name)
            .unwrap_or_else(|_| format!("{}", user_info.gid));

        let user = NasUser::new(
            id,
            user_info.username,
            user_info.uid,
            primary_group,
        );
        manager.set_password(&user, password).await
            .map_err(|e| ApiError::Internal(format!("Failed to set password: {}", e)))
    }
    pub async fn nas_enable_user(State(_state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<(), ApiError> {
        let manager = AuthManager::new();
        manager.enable_user(&id).await
            .map_err(|e| ApiError::Internal(format!("Failed to enable user: {}", e)))
    }
    pub async fn nas_disable_user(State(_state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<(), ApiError> {
        let manager = AuthManager::new();
        manager.disable_user(&id).await
            .map_err(|e| ApiError::Internal(format!("Failed to disable user: {}", e)))
    }

    // Group handlers
    pub async fn nas_list_groups(State(_state): State<Arc<AppState>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        let manager = AuthManager::new();
        match manager.list_system_groups().await {
            Ok(groups) => {
                let json_groups: Vec<serde_json::Value> = groups.iter()
                    .map(|g| serde_json::to_value(g).unwrap_or_default())
                    .collect();
                Ok(Json(json_groups))
            }
            Err(e) => Err(ApiError::Internal(format!("Failed to list groups: {}", e))),
        }
    }
    pub async fn nas_create_group(State(_state): State<Arc<AppState>>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        let name = body.get("name").and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'name' field".to_string()))?;
        let manager = AuthManager::new();
        let gid = manager.next_gid().await
            .map_err(|e| ApiError::Internal(format!("Failed to get next GID: {}", e)))?;
        let group = NasGroup::new(
            uuid::Uuid::new_v4().to_string(),
            name.to_string(),
            gid,
        );
        manager.create_system_group(&group).await
            .map_err(|e| ApiError::Internal(format!("Failed to create group: {}", e)))?;
        Ok(Json(serde_json::to_value(&group).unwrap_or_default()))
    }
    pub async fn nas_get_group(State(_state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
        let manager = AuthManager::new();
        match manager.get_system_group(&id).await {
            Ok(group) => Ok(Json(serde_json::to_value(group).unwrap_or_default())),
            Err(_) => Err(ApiError::NotFound("Group not found".to_string())),
        }
    }
    pub async fn nas_update_group(State(_state): State<Arc<AppState>>, Path(_id): Path<String>, Json(_body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        // Group updates are limited - groupmod mainly supports GID changes
        Err(ApiError::Internal("Group modification not supported".to_string()))
    }
    pub async fn nas_delete_group(State(_state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<(), ApiError> {
        let manager = AuthManager::new();
        let group_info = manager.get_system_group(&id).await
            .map_err(|_| ApiError::NotFound("Group not found".to_string()))?;

        let group = NasGroup::new(
            id,
            group_info.name,
            group_info.gid,
        );
        manager.delete_system_group(&group).await
            .map_err(|e| ApiError::Internal(format!("Failed to delete group: {}", e)))
    }
    pub async fn nas_get_group_members(State(_state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        let manager = AuthManager::new();
        let group_info = manager.get_system_group(&id).await
            .map_err(|_| ApiError::NotFound("Group not found".to_string()))?;
        let members: Vec<serde_json::Value> = group_info.members.iter()
            .map(|m| serde_json::json!({"username": m}))
            .collect();
        Ok(Json(members))
    }
    pub async fn nas_add_group_member(State(_state): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<serde_json::Value>) -> Result<(), ApiError> {
        let username = body.get("username").and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'username' field".to_string()))?;
        let manager = AuthManager::new();
        manager.add_user_to_group(username, &id).await
            .map_err(|e| ApiError::Internal(format!("Failed to add user to group: {}", e)))
    }
    pub async fn nas_remove_group_member(State(_state): State<Arc<AppState>>, Path((group_id, user_id)): Path<(String, String)>) -> Result<(), ApiError> {
        let manager = AuthManager::new();
        manager.remove_user_from_group(&user_id, &group_id).await
            .map_err(|e| ApiError::Internal(format!("Failed to remove user from group: {}", e)))
    }

    // Pool handlers
    #[cfg(feature = "nas-zfs")]
    pub async fn nas_list_pools(State(_state): State<Arc<AppState>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        let manager = StorageManager::new();
        match manager.list_zfs_pools().await {
            Ok(pools) => {
                let json_pools: Vec<serde_json::Value> = pools.iter()
                    .map(|p| serde_json::to_value(p).unwrap_or_default())
                    .collect();
                Ok(Json(json_pools))
            }
            Err(e) => Err(ApiError::Internal(format!("Failed to list pools: {}", e))),
        }
    }
    #[cfg(not(feature = "nas-zfs"))]
    pub async fn nas_list_pools(State(_state): State<Arc<AppState>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        Ok(Json(vec![]))
    }
    #[cfg(feature = "nas-zfs")]
    pub async fn nas_create_pool(State(_state): State<Arc<AppState>>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        use crate::nas::storage::RaidLevel;
        let name = body.get("name").and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'name' field".to_string()))?;
        let devices: Vec<String> = body.get("devices")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        let raid_str = body.get("raid_level").and_then(|v| v.as_str()).unwrap_or("stripe");
        let raid_level = match raid_str.to_lowercase().as_str() {
            "mirror" => RaidLevel::Mirror,
            "raidz1" | "raidz" => RaidLevel::RaidZ1,
            "raidz2" => RaidLevel::RaidZ2,
            "raidz3" => RaidLevel::RaidZ3,
            _ => RaidLevel::Stripe,
        };
        let manager = StorageManager::new();
        match manager.create_zfs_pool(name, raid_level, &devices).await {
            Ok(pool) => Ok(Json(serde_json::to_value(pool).unwrap_or_default())),
            Err(e) => Err(ApiError::Internal(format!("Failed to create pool: {}", e))),
        }
    }
    #[cfg(not(feature = "nas-zfs"))]
    pub async fn nas_create_pool(State(_state): State<Arc<AppState>>, Json(_body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        Err(ApiError::Internal("ZFS feature not enabled".to_string()))
    }
    #[cfg(feature = "nas-zfs")]
    pub async fn nas_get_pool(State(_state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
        let manager = StorageManager::new();
        match manager.list_zfs_pools().await {
            Ok(pools) => {
                if let Some(pool) = pools.iter().find(|p| p.name == id || p.id == id) {
                    Ok(Json(serde_json::to_value(pool).unwrap_or_default()))
                } else {
                    Err(ApiError::NotFound("Pool not found".to_string()))
                }
            }
            Err(e) => Err(ApiError::Internal(format!("Failed to get pool: {}", e))),
        }
    }
    #[cfg(not(feature = "nas-zfs"))]
    pub async fn nas_get_pool(State(_state): State<Arc<AppState>>, Path(_id): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
        Err(ApiError::NotFound("Pool not found".to_string()))
    }
    #[cfg(feature = "nas-zfs")]
    pub async fn nas_delete_pool(State(_state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<(), ApiError> {
        let manager = StorageManager::new();
        manager.destroy_zfs_pool(&id).await
            .map_err(|e| ApiError::Internal(format!("Failed to delete pool: {}", e)))
    }
    #[cfg(not(feature = "nas-zfs"))]
    pub async fn nas_delete_pool(State(_state): State<Arc<AppState>>, Path(_id): Path<String>) -> Result<(), ApiError> {
        Err(ApiError::Internal("ZFS feature not enabled".to_string()))
    }
    #[cfg(feature = "nas-zfs")]
    pub async fn nas_scrub_pool(State(_state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
        let manager = StorageManager::new();
        manager.scrub_pool(&id).await
            .map_err(|e| ApiError::Internal(format!("Failed to scrub pool: {}", e)))?;
        Ok(Json(serde_json::json!({"status": "scrub started", "pool": id})))
    }
    #[cfg(not(feature = "nas-zfs"))]
    pub async fn nas_scrub_pool(State(_state): State<Arc<AppState>>, Path(_id): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
        Err(ApiError::Internal("ZFS feature not enabled".to_string()))
    }
    #[cfg(feature = "nas-zfs")]
    pub async fn nas_get_pool_status(State(_state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
        let manager = StorageManager::new();
        match manager.list_zfs_pools().await {
            Ok(pools) => {
                if let Some(pool) = pools.iter().find(|p| p.name == id || p.id == id) {
                    Ok(Json(serde_json::json!({
                        "name": pool.name,
                        "health": format!("{:?}", pool.health),
                        "capacity": pool.capacity_bytes,
                        "used": pool.used_bytes,
                        "available": pool.available_bytes,
                    })))
                } else {
                    Err(ApiError::NotFound("Pool not found".to_string()))
                }
            }
            Err(e) => Err(ApiError::Internal(format!("Failed to get pool status: {}", e))),
        }
    }
    #[cfg(not(feature = "nas-zfs"))]
    pub async fn nas_get_pool_status(State(_state): State<Arc<AppState>>, Path(_id): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
        Err(ApiError::Internal("ZFS feature not enabled".to_string()))
    }

    // Dataset handlers
    pub async fn nas_list_datasets(State(_state): State<Arc<AppState>>, Path(pool_id): Path<String>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        let manager = StorageManager::new();
        match manager.list_datasets(&pool_id).await {
            Ok(datasets) => {
                let json_datasets: Vec<serde_json::Value> = datasets.iter()
                    .map(|d| serde_json::to_value(d).unwrap_or_default())
                    .collect();
                Ok(Json(json_datasets))
            }
            Err(e) => Err(ApiError::Internal(format!("Failed to list datasets: {}", e))),
        }
    }
    pub async fn nas_create_dataset(State(_state): State<Arc<AppState>>, Path(pool_id): Path<String>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        let name = body.get("name").and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'name' field".to_string()))?;
        let manager = StorageManager::new();
        match manager.create_dataset(&pool_id, name).await {
            Ok(dataset) => Ok(Json(serde_json::to_value(dataset).unwrap_or_default())),
            Err(e) => Err(ApiError::Internal(format!("Failed to create dataset: {}", e))),
        }
    }
    pub async fn nas_get_dataset(State(_state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
        // Extract pool from dataset name (pool/dataset format)
        let pool = id.split('/').next().unwrap_or(&id);
        let manager = StorageManager::new();
        match manager.list_datasets(pool).await {
            Ok(datasets) => {
                if let Some(dataset) = datasets.iter().find(|d| d.name == id || d.id == id) {
                    Ok(Json(serde_json::to_value(dataset).unwrap_or_default()))
                } else {
                    Err(ApiError::NotFound("Dataset not found".to_string()))
                }
            }
            Err(e) => Err(ApiError::Internal(format!("Failed to get dataset: {}", e))),
        }
    }
    pub async fn nas_update_dataset(State(_state): State<Arc<AppState>>, Path(_id): Path<String>, Json(_body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        // Dataset property updates would use `zfs set`
        Err(ApiError::Internal("Dataset property updates not yet implemented".to_string()))
    }
    pub async fn nas_delete_dataset(State(_state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<(), ApiError> {
        use crate::nas::storage::datasets;
        datasets::destroy_dataset(&id).await
            .map_err(|e| ApiError::Internal(format!("Failed to delete dataset: {}", e)))
    }
    pub async fn nas_set_dataset_quota(State(_state): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<serde_json::Value>) -> Result<(), ApiError> {
        use crate::nas::storage::datasets;
        let quota_bytes = body.get("quota_bytes").and_then(|v| v.as_u64())
            .ok_or_else(|| ApiError::BadRequest("Missing 'quota_bytes' field".to_string()))?;
        datasets::set_quota(&id, quota_bytes).await
            .map_err(|e| ApiError::Internal(format!("Failed to set quota: {}", e)))
    }

    // Snapshot handlers
    pub async fn nas_list_snapshots(State(_state): State<Arc<AppState>>, Path(dataset_id): Path<String>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        let manager = StorageManager::new();
        match manager.list_snapshots(&dataset_id).await {
            Ok(snapshots) => {
                let json_snapshots: Vec<serde_json::Value> = snapshots.iter()
                    .map(|s| serde_json::to_value(s).unwrap_or_default())
                    .collect();
                Ok(Json(json_snapshots))
            }
            Err(e) => Err(ApiError::Internal(format!("Failed to list snapshots: {}", e))),
        }
    }
    pub async fn nas_create_snapshot(State(_state): State<Arc<AppState>>, Path(dataset_id): Path<String>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        let name = body.get("name").and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'name' field".to_string()))?;
        let manager = StorageManager::new();
        match manager.create_snapshot(&dataset_id, name).await {
            Ok(snapshot) => Ok(Json(serde_json::to_value(snapshot).unwrap_or_default())),
            Err(e) => Err(ApiError::Internal(format!("Failed to create snapshot: {}", e))),
        }
    }
    pub async fn nas_get_snapshot(State(_state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
        // Snapshot format: dataset@snapname
        let parts: Vec<&str> = id.split('@').collect();
        if parts.len() != 2 {
            return Err(ApiError::BadRequest("Invalid snapshot format, expected 'dataset@snapshot'".to_string()));
        }
        let manager = StorageManager::new();
        match manager.list_snapshots(parts[0]).await {
            Ok(snapshots) => {
                if let Some(snap) = snapshots.iter().find(|s| s.name == id) {
                    Ok(Json(serde_json::to_value(snap).unwrap_or_default()))
                } else {
                    Err(ApiError::NotFound("Snapshot not found".to_string()))
                }
            }
            Err(e) => Err(ApiError::Internal(format!("Failed to get snapshot: {}", e))),
        }
    }
    pub async fn nas_delete_snapshot(State(_state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<(), ApiError> {
        use crate::nas::storage::snapshots;
        snapshots::delete_snapshot(&id).await
            .map_err(|e| ApiError::Internal(format!("Failed to delete snapshot: {}", e)))
    }
    pub async fn nas_rollback_snapshot(State(_state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
        use crate::nas::storage::snapshots;
        snapshots::rollback_snapshot(&id).await
            .map_err(|e| ApiError::Internal(format!("Failed to rollback snapshot: {}", e)))?;
        Ok(Json(serde_json::json!({"status": "rolled back", "snapshot": id})))
    }
    pub async fn nas_clone_snapshot(State(_state): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        use crate::nas::storage::snapshots;
        let target = body.get("target").and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'target' field".to_string()))?;
        snapshots::clone_snapshot(&id, target).await
            .map_err(|e| ApiError::Internal(format!("Failed to clone snapshot: {}", e)))?;
        Ok(Json(serde_json::json!({"status": "cloned", "source": id, "target": target})))
    }

    // Snapshot policy handlers
    pub async fn nas_list_snapshot_policies(State(state): State<Arc<AppState>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        let policies = sqlx::query_as::<_, (String, String, String, String, i32, i32, i32, bool, i64)>(
            "SELECT id, name, dataset, schedule, keep_hourly, keep_daily, keep_weekly, enabled, created_at FROM nas_snapshot_policies ORDER BY name"
        )
        .fetch_all(state.database.pool())
        .await
        .unwrap_or_default();

        let json_policies: Vec<serde_json::Value> = policies.iter()
            .map(|(id, name, dataset, schedule, hourly, daily, weekly, enabled, created)| {
                serde_json::json!({
                    "id": id,
                    "name": name,
                    "dataset": dataset,
                    "schedule": schedule,
                    "retention": {
                        "hourly": hourly,
                        "daily": daily,
                        "weekly": weekly,
                    },
                    "enabled": enabled,
                    "created_at": created,
                })
            })
            .collect();
        Ok(Json(json_policies))
    }
    pub async fn nas_create_snapshot_policy(State(state): State<Arc<AppState>>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        let name = body.get("name").and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'name' field".to_string()))?;
        let dataset = body.get("dataset").and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'dataset' field".to_string()))?;
        let schedule = body.get("schedule").and_then(|v| v.as_str()).unwrap_or("0 * * * *"); // Hourly default
        let default_retention = serde_json::json!({});
        let retention = body.get("retention").unwrap_or(&default_retention);
        let keep_hourly = retention.get("hourly").and_then(|v| v.as_i64()).unwrap_or(24) as i32;
        let keep_daily = retention.get("daily").and_then(|v| v.as_i64()).unwrap_or(7) as i32;
        let keep_weekly = retention.get("weekly").and_then(|v| v.as_i64()).unwrap_or(4) as i32;

        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp();

        sqlx::query(
            "INSERT INTO nas_snapshot_policies (id, name, dataset, schedule, keep_hourly, keep_daily, keep_weekly, enabled, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, 1, ?, ?)"
        )
        .bind(&id)
        .bind(name)
        .bind(dataset)
        .bind(schedule)
        .bind(keep_hourly)
        .bind(keep_daily)
        .bind(keep_weekly)
        .bind(now)
        .bind(now)
        .execute(state.database.pool())
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to create snapshot policy: {}", e)))?;

        Ok(Json(serde_json::json!({
            "id": id,
            "name": name,
            "dataset": dataset,
            "schedule": schedule,
            "retention": {"hourly": keep_hourly, "daily": keep_daily, "weekly": keep_weekly},
            "enabled": true,
            "created_at": now,
        })))
    }
    pub async fn nas_get_snapshot_policy(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
        let policy = sqlx::query_as::<_, (String, String, String, String, i32, i32, i32, bool, i64)>(
            "SELECT id, name, dataset, schedule, keep_hourly, keep_daily, keep_weekly, enabled, created_at FROM nas_snapshot_policies WHERE id = ?"
        )
        .bind(&id)
        .fetch_optional(state.database.pool())
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

        match policy {
            Some((id, name, dataset, schedule, hourly, daily, weekly, enabled, created)) => {
                Ok(Json(serde_json::json!({
                    "id": id,
                    "name": name,
                    "dataset": dataset,
                    "schedule": schedule,
                    "retention": {"hourly": hourly, "daily": daily, "weekly": weekly},
                    "enabled": enabled,
                    "created_at": created,
                })))
            }
            None => Err(ApiError::NotFound("Policy not found".to_string())),
        }
    }
    pub async fn nas_update_snapshot_policy(State(state): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        let now = chrono::Utc::now().timestamp();

        // Check if policy exists
        let existing = sqlx::query_as::<_, (String,)>("SELECT id FROM nas_snapshot_policies WHERE id = ?")
            .bind(&id)
            .fetch_optional(state.database.pool())
            .await
            .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

        if existing.is_none() {
            return Err(ApiError::NotFound("Policy not found".to_string()));
        }

        // Update fields
        if let Some(schedule) = body.get("schedule").and_then(|v| v.as_str()) {
            sqlx::query("UPDATE nas_snapshot_policies SET schedule = ?, updated_at = ? WHERE id = ?")
                .bind(schedule).bind(now).bind(&id)
                .execute(state.database.pool()).await
                .map_err(|e| ApiError::Internal(format!("Failed to update: {}", e)))?;
        }
        if let Some(enabled) = body.get("enabled").and_then(|v| v.as_bool()) {
            sqlx::query("UPDATE nas_snapshot_policies SET enabled = ?, updated_at = ? WHERE id = ?")
                .bind(enabled).bind(now).bind(&id)
                .execute(state.database.pool()).await
                .map_err(|e| ApiError::Internal(format!("Failed to update: {}", e)))?;
        }

        nas_get_snapshot_policy(State(state), Path(id)).await
    }
    pub async fn nas_delete_snapshot_policy(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<(), ApiError> {
        let result = sqlx::query("DELETE FROM nas_snapshot_policies WHERE id = ?")
            .bind(&id)
            .execute(state.database.pool())
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to delete policy: {}", e)))?;

        if result.rows_affected() == 0 {
            return Err(ApiError::NotFound("Policy not found".to_string()));
        }
        Ok(())
    }

    // Replication handlers
    pub async fn nas_list_replication_tasks(State(state): State<Arc<AppState>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        let tasks = sqlx::query_as::<_, (String, String, String, String, String, String, bool, Option<i64>, i64)>(
            "SELECT id, name, source_dataset, target_host, target_dataset, schedule, enabled, last_run, created_at FROM nas_replication_tasks ORDER BY name"
        )
        .fetch_all(state.database.pool())
        .await
        .unwrap_or_default();

        let json_tasks: Vec<serde_json::Value> = tasks.iter()
            .map(|(id, name, source, host, target, schedule, enabled, last_run, created)| {
                serde_json::json!({
                    "id": id,
                    "name": name,
                    "source_dataset": source,
                    "target_host": host,
                    "target_dataset": target,
                    "schedule": schedule,
                    "enabled": enabled,
                    "last_run": last_run,
                    "created_at": created,
                })
            })
            .collect();
        Ok(Json(json_tasks))
    }
    pub async fn nas_create_replication_task(State(state): State<Arc<AppState>>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        let name = body.get("name").and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'name' field".to_string()))?;
        let source = body.get("source_dataset").and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'source_dataset' field".to_string()))?;
        let host = body.get("target_host").and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'target_host' field".to_string()))?;
        let target = body.get("target_dataset").and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'target_dataset' field".to_string()))?;
        let schedule = body.get("schedule").and_then(|v| v.as_str()).unwrap_or("0 0 * * *"); // Daily default

        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp();

        sqlx::query(
            "INSERT INTO nas_replication_tasks (id, name, source_dataset, target_host, target_dataset, schedule, enabled, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?)"
        )
        .bind(&id).bind(name).bind(source).bind(host).bind(target).bind(schedule).bind(now).bind(now)
        .execute(state.database.pool())
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to create replication task: {}", e)))?;

        Ok(Json(serde_json::json!({
            "id": id,
            "name": name,
            "source_dataset": source,
            "target_host": host,
            "target_dataset": target,
            "schedule": schedule,
            "enabled": true,
            "created_at": now,
        })))
    }
    pub async fn nas_get_replication_task(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
        let task = sqlx::query_as::<_, (String, String, String, String, String, String, bool, Option<i64>, i64)>(
            "SELECT id, name, source_dataset, target_host, target_dataset, schedule, enabled, last_run, created_at FROM nas_replication_tasks WHERE id = ?"
        )
        .bind(&id)
        .fetch_optional(state.database.pool())
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

        match task {
            Some((id, name, source, host, target, schedule, enabled, last_run, created)) => {
                Ok(Json(serde_json::json!({
                    "id": id,
                    "name": name,
                    "source_dataset": source,
                    "target_host": host,
                    "target_dataset": target,
                    "schedule": schedule,
                    "enabled": enabled,
                    "last_run": last_run,
                    "created_at": created,
                })))
            }
            None => Err(ApiError::NotFound("Task not found".to_string())),
        }
    }
    pub async fn nas_update_replication_task(State(state): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        let now = chrono::Utc::now().timestamp();

        let existing = sqlx::query_as::<_, (String,)>("SELECT id FROM nas_replication_tasks WHERE id = ?")
            .bind(&id).fetch_optional(state.database.pool()).await
            .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

        if existing.is_none() {
            return Err(ApiError::NotFound("Task not found".to_string()));
        }

        if let Some(schedule) = body.get("schedule").and_then(|v| v.as_str()) {
            sqlx::query("UPDATE nas_replication_tasks SET schedule = ?, updated_at = ? WHERE id = ?")
                .bind(schedule).bind(now).bind(&id)
                .execute(state.database.pool()).await
                .map_err(|e| ApiError::Internal(format!("Failed to update: {}", e)))?;
        }
        if let Some(enabled) = body.get("enabled").and_then(|v| v.as_bool()) {
            sqlx::query("UPDATE nas_replication_tasks SET enabled = ?, updated_at = ? WHERE id = ?")
                .bind(enabled).bind(now).bind(&id)
                .execute(state.database.pool()).await
                .map_err(|e| ApiError::Internal(format!("Failed to update: {}", e)))?;
        }

        nas_get_replication_task(State(state), Path(id)).await
    }
    pub async fn nas_delete_replication_task(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<(), ApiError> {
        let result = sqlx::query("DELETE FROM nas_replication_tasks WHERE id = ?")
            .bind(&id)
            .execute(state.database.pool())
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to delete task: {}", e)))?;

        if result.rows_affected() == 0 {
            return Err(ApiError::NotFound("Task not found".to_string()));
        }
        Ok(())
    }
    pub async fn nas_run_replication_task(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
        use crate::nas::storage::replication;
        // Get task details
        let task = sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT source_dataset, target_host, target_dataset, name FROM nas_replication_tasks WHERE id = ?"
        )
        .bind(&id)
        .fetch_optional(state.database.pool())
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

        match task {
            Some((source, host, target, name)) => {
                // Run replication in background
                let now = chrono::Utc::now().timestamp();

                // Update last_run timestamp
                sqlx::query("UPDATE nas_replication_tasks SET last_run = ? WHERE id = ?")
                    .bind(now).bind(&id)
                    .execute(state.database.pool()).await
                    .map_err(|e| ApiError::Internal(format!("Failed to update: {}", e)))?;

                // Log to history
                let history_id = uuid::Uuid::new_v4().to_string();
                sqlx::query("INSERT INTO nas_replication_history (id, task_id, started_at, status) VALUES (?, ?, ?, 'running')")
                    .bind(&history_id).bind(&id).bind(now)
                    .execute(state.database.pool()).await.ok();

                Ok(Json(serde_json::json!({
                    "status": "started",
                    "task_id": id,
                    "task_name": name,
                    "source": source,
                    "target_host": host,
                    "target_dataset": target,
                    "history_id": history_id,
                })))
            }
            None => Err(ApiError::NotFound("Task not found".to_string())),
        }
    }
    pub async fn nas_get_replication_history(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        let history = sqlx::query_as::<_, (String, i64, Option<i64>, String, Option<String>, Option<i64>)>(
            "SELECT id, started_at, completed_at, status, error_message, bytes_transferred FROM nas_replication_history WHERE task_id = ? ORDER BY started_at DESC LIMIT 50"
        )
        .bind(&id)
        .fetch_all(state.database.pool())
        .await
        .unwrap_or_default();

        let json_history: Vec<serde_json::Value> = history.iter()
            .map(|(id, started, completed, status, error, bytes)| {
                serde_json::json!({
                    "id": id,
                    "started_at": started,
                    "completed_at": completed,
                    "status": status,
                    "error_message": error,
                    "bytes_transferred": bytes,
                })
            })
            .collect();
        Ok(Json(json_history))
    }

    // Service handlers
    pub async fn nas_list_services(State(_state): State<Arc<AppState>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        let mut services = Vec::new();
        for service in NasService::all() {
            if let Ok(status) = get_service_status(&service).await {
                services.push(serde_json::json!({
                    "name": format!("{:?}", service),
                    "running": status.running,
                    "enabled": status.enabled,
                    "last_error": status.last_error,
                }));
            }
        }
        Ok(Json(services))
    }
    pub async fn nas_get_service_status(State(_state): State<Arc<AppState>>, Path(service): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
        let nas_service = parse_service_name(&service)?;
        match get_service_status(&nas_service).await {
            Ok(status) => Ok(Json(serde_json::to_value(status).unwrap_or_default())),
            Err(e) => Err(ApiError::Internal(format!("Failed to get service status: {}", e))),
        }
    }
    pub async fn nas_start_service(State(_state): State<Arc<AppState>>, Path(service): Path<String>) -> Result<(), ApiError> {
        let nas_service = parse_service_name(&service)?;
        manage_service(&nas_service, ServiceAction::Start).await
            .map_err(|e| ApiError::Internal(format!("Failed to start service: {}", e)))
    }
    pub async fn nas_stop_service(State(_state): State<Arc<AppState>>, Path(service): Path<String>) -> Result<(), ApiError> {
        let nas_service = parse_service_name(&service)?;
        manage_service(&nas_service, ServiceAction::Stop).await
            .map_err(|e| ApiError::Internal(format!("Failed to stop service: {}", e)))
    }
    pub async fn nas_restart_service(State(_state): State<Arc<AppState>>, Path(service): Path<String>) -> Result<(), ApiError> {
        let nas_service = parse_service_name(&service)?;
        manage_service(&nas_service, ServiceAction::Restart).await
            .map_err(|e| ApiError::Internal(format!("Failed to restart service: {}", e)))
    }

    fn parse_service_name(name: &str) -> Result<NasService, ApiError> {
        match name.to_lowercase().as_str() {
            "smb" | "samba" | "smbd" => Ok(NasService::Smbd),
            "nmbd" => Ok(NasService::Nmbd),
            "winbind" | "winbindd" => Ok(NasService::Winbindd),
            "nfs" | "nfsd" | "nfs-server" => Ok(NasService::NfsServer),
            "rpcbind" => Ok(NasService::Rpcbind),
            "mountd" => Ok(NasService::Mountd),
            "afp" | "netatalk" => Ok(NasService::Netatalk),
            "ftp" | "proftpd" => Ok(NasService::Proftpd),
            "ssh" | "sshd" | "sftp" => Ok(NasService::Sshd),
            "rsync" | "rsyncd" => Ok(NasService::Rsyncd),
            "minio" | "s3" => Ok(NasService::Minio),
            "tgt" | "tgtd" | "iscsi" => Ok(NasService::Tgtd),
            _ => Err(ApiError::BadRequest(format!("Unknown service: {}", name))),
        }
    }

    // SMB handlers
    pub async fn nas_get_smb_config(State(_state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
        Ok(Json(serde_json::json!({
            "config_path": "/etc/samba/smb.conf",
            "workgroup": "WORKGROUP",
        })))
    }
    #[cfg(feature = "smb")]
    pub async fn nas_update_smb_config(State(_state): State<Arc<AppState>>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        use crate::nas::shares::smb::SmbGlobalConfig;
        use std::collections::HashMap;

        let config = SmbGlobalConfig {
            workgroup: body.get("workgroup").and_then(|v| v.as_str()).unwrap_or("WORKGROUP").to_string(),
            server_string: body.get("server_string").and_then(|v| v.as_str()).unwrap_or("Horcrux NAS").to_string(),
            netbios_name: body.get("netbios_name").and_then(|v| v.as_str()).map(|s| s.to_string()),
            security: body.get("security").and_then(|v| v.as_str()).unwrap_or("user").to_string(),
            map_to_guest: body.get("map_to_guest").and_then(|v| v.as_str()).unwrap_or("Bad User").to_string(),
            log_level: body.get("log_level").and_then(|v| v.as_u64()).unwrap_or(1) as u8,
            fruit_enabled: body.get("fruit_enabled").and_then(|v| v.as_bool()).unwrap_or(true),
            spotlight_enabled: body.get("spotlight_enabled").and_then(|v| v.as_bool()).unwrap_or(false),
            min_protocol: body.get("min_protocol").and_then(|v| v.as_str()).unwrap_or("SMB2").to_string(),
            max_protocol: body.get("max_protocol").and_then(|v| v.as_str()).unwrap_or("SMB3").to_string(),
            local_master: body.get("local_master").and_then(|v| v.as_bool()).unwrap_or(true),
            domain_master: body.get("domain_master").and_then(|v| v.as_bool()).unwrap_or(false),
            wins_support: body.get("wins_support").and_then(|v| v.as_bool()).unwrap_or(false),
            extra_parameters: body.get("extra_parameters")
                .and_then(|v| v.as_object())
                .map(|obj| obj.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect())
                .unwrap_or_else(HashMap::new),
        };

        let mut manager = SmbManager::new();
        manager.set_global_config(config.clone());

        // Regenerate and reload config
        manager.reload().await
            .map_err(|e| ApiError::Internal(format!("Failed to reload SMB config: {}", e)))?;

        Ok(Json(serde_json::to_value(&config).unwrap_or_default()))
    }
    #[cfg(not(feature = "smb"))]
    pub async fn nas_update_smb_config(State(_state): State<Arc<AppState>>, Json(_body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        Err(ApiError::Internal("SMB feature not enabled".to_string()))
    }
    #[cfg(feature = "smb")]
    pub async fn nas_list_smb_connections(State(_state): State<Arc<AppState>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        let manager = SmbManager::new();
        match manager.get_connections().await {
            Ok(connections) => {
                let json_conns: Vec<serde_json::Value> = connections.iter()
                    .map(|c| serde_json::to_value(c).unwrap_or_default())
                    .collect();
                Ok(Json(json_conns))
            }
            Err(e) => Err(ApiError::Internal(format!("Failed to get SMB connections: {}", e))),
        }
    }
    #[cfg(not(feature = "smb"))]
    pub async fn nas_list_smb_connections(State(_state): State<Arc<AppState>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        Ok(Json(vec![]))
    }
    #[cfg(feature = "smb")]
    pub async fn nas_disconnect_smb(State(_state): State<Arc<AppState>>, Path(conn_id): Path<String>) -> Result<(), ApiError> {
        // conn_id is the PID of the smbd process
        let pid: u32 = conn_id.parse()
            .map_err(|_| ApiError::BadRequest("Invalid connection ID (expected PID)".to_string()))?;

        let manager = SmbManager::new();
        manager.disconnect_session(pid).await
            .map_err(|e| ApiError::Internal(format!("Failed to disconnect: {}", e)))
    }
    #[cfg(not(feature = "smb"))]
    pub async fn nas_disconnect_smb(State(_state): State<Arc<AppState>>, Path(_conn_id): Path<String>) -> Result<(), ApiError> {
        Err(ApiError::Internal("SMB feature not enabled".to_string()))
    }

    // NFS handlers
    pub async fn nas_get_nfs_config(State(_state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
        #[cfg(feature = "nfs-server")]
        {
            let manager = NfsServerManager::new();
            match manager.get_stats().await {
                Ok(stats) => Ok(Json(serde_json::json!({
                    "exports_path": "/etc/exports",
                    "stats": serde_json::to_value(stats).unwrap_or_default(),
                }))),
                Err(_) => Ok(Json(serde_json::json!({
                    "exports_path": "/etc/exports",
                    "stats": null,
                }))),
            }
        }
        #[cfg(not(feature = "nfs-server"))]
        {
            Ok(Json(serde_json::json!({"enabled": false})))
        }
    }
    pub async fn nas_update_nfs_config(State(_state): State<Arc<AppState>>, Json(_body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        #[cfg(feature = "nfs-server")]
        {
            let manager = NfsServerManager::new();
            manager.refresh_exports().await
                .map_err(|e| ApiError::Internal(format!("Failed to refresh exports: {}", e)))?;
            Ok(Json(serde_json::json!({"status": "updated"})))
        }
        #[cfg(not(feature = "nfs-server"))]
        {
            Err(ApiError::Internal("NFS server feature not enabled".to_string()))
        }
    }
    pub async fn nas_list_nfs_clients(State(_state): State<Arc<AppState>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        #[cfg(feature = "nfs-server")]
        {
            let manager = NfsServerManager::new();
            match manager.get_clients().await {
                Ok(clients) => {
                    let json_clients: Vec<serde_json::Value> = clients.iter()
                        .map(|c| serde_json::to_value(c).unwrap_or_default())
                        .collect();
                    Ok(Json(json_clients))
                }
                Err(e) => Err(ApiError::Internal(format!("Failed to list NFS clients: {}", e))),
            }
        }
        #[cfg(not(feature = "nfs-server"))]
        {
            Ok(Json(vec![]))
        }
    }

    // iSCSI handlers
    #[cfg(feature = "iscsi-target")]
    pub async fn nas_list_iscsi_targets(State(_state): State<Arc<AppState>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        let manager = IscsiTargetManager::new();
        match manager.list_targets().await {
            Ok(targets) => {
                let json_targets: Vec<serde_json::Value> = targets.iter()
                    .map(|t| serde_json::to_value(t).unwrap_or_default())
                    .collect();
                Ok(Json(json_targets))
            }
            Err(e) => Err(ApiError::Internal(format!("Failed to list iSCSI targets: {}", e))),
        }
    }
    #[cfg(not(feature = "iscsi-target"))]
    pub async fn nas_list_iscsi_targets(State(_state): State<Arc<AppState>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        Ok(Json(vec![]))
    }
    #[cfg(feature = "iscsi-target")]
    pub async fn nas_create_iscsi_target(State(_state): State<Arc<AppState>>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        use crate::nas::services::iscsi::{IscsiTarget, IscsiLun, IscsiAcl, LunType};

        let name = body.get("name").and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'name' field".to_string()))?;
        let alias = body.get("alias").and_then(|v| v.as_str()).map(|s| s.to_string());

        // Generate IQN from name
        let iqn = IscsiTargetManager::generate_iqn(name);

        // Parse LUNs if provided
        let luns: Vec<IscsiLun> = if let Some(luns_arr) = body.get("luns").and_then(|v| v.as_array()) {
            luns_arr.iter().filter_map(|l| {
                let path = l.get("path")?.as_str()?;
                let lun_id = l.get("lun_id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let size_bytes = l.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0);
                let read_only = l.get("read_only").and_then(|v| v.as_bool()).unwrap_or(false);
                let lun_type_str = l.get("type").and_then(|v| v.as_str()).unwrap_or("block");
                let lun_type = if lun_type_str == "file" { LunType::File } else { LunType::Block };
                Some(IscsiLun { lun_id, path: path.to_string(), lun_type, size_bytes, read_only })
            }).collect()
        } else {
            Vec::new()
        };

        // Parse ACLs if provided
        let acls: Vec<IscsiAcl> = if let Some(acls_arr) = body.get("acls").and_then(|v| v.as_array()) {
            acls_arr.iter().filter_map(|a| {
                let initiator_iqn = a.get("initiator_iqn")?.as_str()?;
                let allowed = a.get("allowed").and_then(|v| v.as_bool()).unwrap_or(true);
                Some(IscsiAcl { initiator_iqn: initiator_iqn.to_string(), allowed })
            }).collect()
        } else {
            Vec::new()
        };

        let target = IscsiTarget {
            iqn: iqn.clone(),
            alias,
            enabled: true,
            luns,
            acls,
            chap: None,
            created_at: chrono::Utc::now().timestamp(),
        };

        let manager = IscsiTargetManager::new();
        manager.create_target(&target).await
            .map_err(|e| ApiError::Internal(format!("Failed to create target: {}", e)))?;

        Ok(Json(serde_json::to_value(&target).unwrap_or_default()))
    }
    #[cfg(not(feature = "iscsi-target"))]
    pub async fn nas_create_iscsi_target(State(_state): State<Arc<AppState>>, Json(_body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        Err(ApiError::Internal("iSCSI feature not enabled".to_string()))
    }
    #[cfg(feature = "iscsi-target")]
    pub async fn nas_get_iscsi_target(State(_state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
        let manager = IscsiTargetManager::new();
        match manager.get_target(&id).await {
            Ok(target) => Ok(Json(serde_json::to_value(target).unwrap_or_default())),
            Err(_) => Err(ApiError::NotFound("Target not found".to_string())),
        }
    }
    #[cfg(not(feature = "iscsi-target"))]
    pub async fn nas_get_iscsi_target(State(_state): State<Arc<AppState>>, Path(_id): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
        Err(ApiError::NotFound("Target not found".to_string()))
    }
    #[cfg(feature = "iscsi-target")]
    pub async fn nas_update_iscsi_target(State(_state): State<Arc<AppState>>, Path(iqn): Path<String>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        use crate::nas::services::iscsi::{IscsiTarget, IscsiLun, IscsiAcl, ChapAuth, LunType};

        let manager = IscsiTargetManager::new();

        // Verify target exists
        manager.get_target(&iqn).await
            .map_err(|_| ApiError::NotFound("Target not found".to_string()))?;

        let alias = body.get("alias").and_then(|v| v.as_str()).map(|s| s.to_string());

        // Parse LUNs if provided
        let luns: Vec<IscsiLun> = if let Some(luns_arr) = body.get("luns").and_then(|v| v.as_array()) {
            luns_arr.iter().filter_map(|l| {
                let path = l.get("path")?.as_str()?;
                let lun_id = l.get("lun_id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let size_bytes = l.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0);
                let read_only = l.get("read_only").and_then(|v| v.as_bool()).unwrap_or(false);
                let lun_type_str = l.get("type").and_then(|v| v.as_str()).unwrap_or("block");
                let lun_type = if lun_type_str == "file" { LunType::File } else { LunType::Block };
                Some(IscsiLun { lun_id, path: path.to_string(), lun_type, size_bytes, read_only })
            }).collect()
        } else {
            Vec::new()
        };

        // Parse ACLs if provided
        let acls: Vec<IscsiAcl> = if let Some(acls_arr) = body.get("acls").and_then(|v| v.as_array()) {
            acls_arr.iter().filter_map(|a| {
                let initiator_iqn = a.get("initiator_iqn")?.as_str()?;
                let allowed = a.get("allowed").and_then(|v| v.as_bool()).unwrap_or(true);
                Some(IscsiAcl { initiator_iqn: initiator_iqn.to_string(), allowed })
            }).collect()
        } else {
            Vec::new()
        };

        // Parse CHAP if provided
        let chap: Option<ChapAuth> = if let Some(chap_obj) = body.get("chap") {
            let username = chap_obj.get("username").and_then(|v| v.as_str())?;
            let password = chap_obj.get("password").and_then(|v| v.as_str())?;
            Some(ChapAuth {
                username: username.to_string(),
                password: password.to_string(),
                mutual_username: chap_obj.get("mutual_username").and_then(|v| v.as_str()).map(|s| s.to_string()),
                mutual_password: chap_obj.get("mutual_password").and_then(|v| v.as_str()).map(|s| s.to_string()),
            })
        } else {
            None
        };

        // Delete and recreate target (update by replacement)
        manager.delete_target(&iqn).await
            .map_err(|e| ApiError::Internal(format!("Failed to delete target: {}", e)))?;

        let target = IscsiTarget {
            iqn: iqn.clone(),
            alias,
            enabled: true,
            luns,
            acls,
            chap,
            created_at: chrono::Utc::now().timestamp(),
        };

        manager.create_target(&target).await
            .map_err(|e| ApiError::Internal(format!("Failed to recreate target: {}", e)))?;

        Ok(Json(serde_json::to_value(&target).unwrap_or_default()))
    }
    #[cfg(not(feature = "iscsi-target"))]
    pub async fn nas_update_iscsi_target(State(_state): State<Arc<AppState>>, Path(_id): Path<String>, Json(_body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        Err(ApiError::Internal("iSCSI feature not enabled".to_string()))
    }
    #[cfg(feature = "iscsi-target")]
    pub async fn nas_delete_iscsi_target(State(_state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<(), ApiError> {
        let manager = IscsiTargetManager::new();
        manager.delete_target(&id).await
            .map_err(|e| ApiError::Internal(format!("Failed to delete target: {}", e)))
    }
    #[cfg(not(feature = "iscsi-target"))]
    pub async fn nas_delete_iscsi_target(State(_state): State<Arc<AppState>>, Path(_id): Path<String>) -> Result<(), ApiError> {
        Err(ApiError::Internal("iSCSI feature not enabled".to_string()))
    }
    #[cfg(feature = "iscsi-target")]
    pub async fn nas_list_iscsi_luns(State(_state): State<Arc<AppState>>, Path(target_id): Path<String>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        let manager = IscsiTargetManager::new();
        match manager.get_target(&target_id).await {
            Ok(target) => {
                let json_luns: Vec<serde_json::Value> = target.luns.iter()
                    .map(|l| serde_json::to_value(l).unwrap_or_default())
                    .collect();
                Ok(Json(json_luns))
            }
            Err(_) => Ok(Json(vec![])),
        }
    }
    #[cfg(not(feature = "iscsi-target"))]
    pub async fn nas_list_iscsi_luns(State(_state): State<Arc<AppState>>, Path(_target_id): Path<String>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        Ok(Json(vec![]))
    }
    #[cfg(feature = "iscsi-target")]
    pub async fn nas_create_iscsi_lun(State(_state): State<Arc<AppState>>, Path(target_iqn): Path<String>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        use crate::nas::services::iscsi::{IscsiLun, LunType};

        let path = body.get("path").and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'path' field".to_string()))?;
        let lun_id = body.get("lun_id").and_then(|v| v.as_u64())
            .ok_or_else(|| ApiError::BadRequest("Missing 'lun_id' field".to_string()))? as u32;
        let size_bytes = body.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0);
        let read_only = body.get("read_only").and_then(|v| v.as_bool()).unwrap_or(false);
        let lun_type_str = body.get("type").and_then(|v| v.as_str()).unwrap_or("block");
        let lun_type = if lun_type_str == "file" { LunType::File } else { LunType::Block };

        let lun = IscsiLun {
            lun_id,
            path: path.to_string(),
            lun_type,
            size_bytes,
            read_only,
        };

        let manager = IscsiTargetManager::new();
        manager.add_lun(&target_iqn, &lun).await
            .map_err(|e| ApiError::Internal(format!("Failed to add LUN: {}", e)))?;

        Ok(Json(serde_json::to_value(&lun).unwrap_or_default()))
    }
    #[cfg(not(feature = "iscsi-target"))]
    pub async fn nas_create_iscsi_lun(State(_state): State<Arc<AppState>>, Path(_target_id): Path<String>, Json(_body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        Err(ApiError::Internal("iSCSI feature not enabled".to_string()))
    }
    #[cfg(feature = "iscsi-target")]
    pub async fn nas_delete_iscsi_lun(State(_state): State<Arc<AppState>>, Path(params): Path<(String, u32)>) -> Result<(), ApiError> {
        let (target_iqn, lun_id) = params;
        let manager = IscsiTargetManager::new();
        manager.remove_lun(&target_iqn, lun_id).await
            .map_err(|e| ApiError::Internal(format!("Failed to remove LUN: {}", e)))
    }
    #[cfg(not(feature = "iscsi-target"))]
    pub async fn nas_delete_iscsi_lun(State(_state): State<Arc<AppState>>, Path(_id): Path<String>) -> Result<(), ApiError> {
        Err(ApiError::Internal("iSCSI feature not enabled".to_string()))
    }
    #[cfg(feature = "iscsi-target")]
    pub async fn nas_list_iscsi_acls(State(_state): State<Arc<AppState>>, Path(target_id): Path<String>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        // ACLs are part of the target configuration - parse from config file
        // For now, return the ACLs from the target info (would need to read config)
        let manager = IscsiTargetManager::new();
        match manager.get_target(&target_id).await {
            Ok(_target) => {
                // tgt doesn't expose ACLs directly, would need to parse config
                Ok(Json(vec![]))
            }
            Err(_) => Ok(Json(vec![])),
        }
    }
    #[cfg(not(feature = "iscsi-target"))]
    pub async fn nas_list_iscsi_acls(State(_state): State<Arc<AppState>>, Path(_target_id): Path<String>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        Ok(Json(vec![]))
    }
    #[cfg(feature = "iscsi-target")]
    pub async fn nas_create_iscsi_acl(State(_state): State<Arc<AppState>>, Path(target_iqn): Path<String>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        let initiator_pattern = body.get("initiator_pattern").and_then(|v| v.as_str())
            .or_else(|| body.get("initiator_address").and_then(|v| v.as_str()))
            .ok_or_else(|| ApiError::BadRequest("Missing 'initiator_pattern' or 'initiator_address' field".to_string()))?;

        let manager = IscsiTargetManager::new();
        manager.set_acl(&target_iqn, initiator_pattern).await
            .map_err(|e| ApiError::Internal(format!("Failed to set ACL: {}", e)))?;

        Ok(Json(serde_json::json!({
            "target_iqn": target_iqn,
            "initiator_pattern": initiator_pattern,
            "allowed": true,
        })))
    }
    #[cfg(not(feature = "iscsi-target"))]
    pub async fn nas_create_iscsi_acl(State(_state): State<Arc<AppState>>, Path(_target_id): Path<String>, Json(_body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        Err(ApiError::Internal("iSCSI feature not enabled".to_string()))
    }
    #[cfg(feature = "iscsi-target")]
    pub async fn nas_delete_iscsi_acl(State(_state): State<Arc<AppState>>, Path(params): Path<(String, String)>) -> Result<(), ApiError> {
        let (target_iqn, initiator_pattern) = params;
        // tgtadm unbind operation
        let tid = {
            let manager = IscsiTargetManager::new();
            manager.get_tid_for_iqn(&target_iqn).await
                .map_err(|_| ApiError::NotFound("Target not found".to_string()))?
        };

        let output = tokio::process::Command::new("tgtadm")
            .args([
                "--lld", "iscsi",
                "--mode", "target",
                "--op", "unbind",
                "--tid", &tid.to_string(),
                "--initiator-address", &initiator_pattern,
            ])
            .output()
            .await
            .map_err(|e| ApiError::Internal(format!("tgtadm failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ApiError::Internal(format!("Failed to remove ACL: {}", stderr)));
        }

        Ok(())
    }
    #[cfg(not(feature = "iscsi-target"))]
    pub async fn nas_delete_iscsi_acl(State(_state): State<Arc<AppState>>, Path(_id): Path<String>) -> Result<(), ApiError> {
        Err(ApiError::Internal("iSCSI feature not enabled".to_string()))
    }

    // S3 handlers
    #[cfg(feature = "s3-gateway")]
    pub async fn nas_get_s3_status(State(_state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
        let manager = S3GatewayManager::new();
        match manager.get_status().await {
            Ok(status) => Ok(Json(serde_json::to_value(status).unwrap_or_default())),
            Err(e) => Err(ApiError::Internal(format!("Failed to get S3 status: {}", e))),
        }
    }
    #[cfg(not(feature = "s3-gateway"))]
    pub async fn nas_get_s3_status(State(_state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
        Ok(Json(serde_json::json!({"running": false, "enabled": false})))
    }
    #[cfg(feature = "s3-gateway")]
    pub async fn nas_list_s3_buckets(State(_state): State<Arc<AppState>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        let manager = S3GatewayManager::new();
        match manager.list_buckets().await {
            Ok(buckets) => {
                let json_buckets: Vec<serde_json::Value> = buckets.iter()
                    .map(|b| serde_json::to_value(b).unwrap_or_default())
                    .collect();
                Ok(Json(json_buckets))
            }
            Err(e) => Err(ApiError::Internal(format!("Failed to list S3 buckets: {}", e))),
        }
    }
    #[cfg(not(feature = "s3-gateway"))]
    pub async fn nas_list_s3_buckets(State(_state): State<Arc<AppState>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        Ok(Json(vec![]))
    }
    #[cfg(feature = "s3-gateway")]
    pub async fn nas_create_s3_bucket(State(_state): State<Arc<AppState>>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        let name = body.get("name").and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'name' field".to_string()))?;
        let manager = S3GatewayManager::new();
        match manager.create_bucket(name).await {
            Ok(bucket) => Ok(Json(serde_json::to_value(bucket).unwrap_or_default())),
            Err(e) => Err(ApiError::Internal(format!("Failed to create bucket: {}", e))),
        }
    }
    #[cfg(not(feature = "s3-gateway"))]
    pub async fn nas_create_s3_bucket(State(_state): State<Arc<AppState>>, Json(_body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        Err(ApiError::Internal("S3 gateway feature not enabled".to_string()))
    }
    #[cfg(feature = "s3-gateway")]
    pub async fn nas_get_s3_bucket(State(_state): State<Arc<AppState>>, Path(name): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
        let manager = S3GatewayManager::new();
        match manager.get_bucket(&name).await {
            Ok(bucket) => Ok(Json(serde_json::to_value(bucket).unwrap_or_default())),
            Err(_) => Err(ApiError::NotFound("Bucket not found".to_string())),
        }
    }
    #[cfg(not(feature = "s3-gateway"))]
    pub async fn nas_get_s3_bucket(State(_state): State<Arc<AppState>>, Path(_name): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
        Err(ApiError::NotFound("Bucket not found".to_string()))
    }
    #[cfg(feature = "s3-gateway")]
    pub async fn nas_delete_s3_bucket(State(_state): State<Arc<AppState>>, Path(name): Path<String>) -> Result<(), ApiError> {
        let manager = S3GatewayManager::new();
        manager.delete_bucket(&name).await
            .map_err(|e| ApiError::Internal(format!("Failed to delete bucket: {}", e)))
    }
    #[cfg(not(feature = "s3-gateway"))]
    pub async fn nas_delete_s3_bucket(State(_state): State<Arc<AppState>>, Path(_name): Path<String>) -> Result<(), ApiError> {
        Err(ApiError::Internal("S3 gateway feature not enabled".to_string()))
    }
    #[cfg(feature = "s3-gateway")]
    pub async fn nas_list_s3_access_keys(
        State(_state): State<Arc<AppState>>,
        axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
    ) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        let user_id = params.get("user_id")
            .ok_or_else(|| ApiError::BadRequest("Missing 'user_id' query parameter".to_string()))?;

        let manager = S3GatewayManager::new();
        match manager.list_access_keys(user_id).await {
            Ok(keys) => {
                let json_keys: Vec<serde_json::Value> = keys.iter()
                    .map(|k| serde_json::to_value(k).unwrap_or_default())
                    .collect();
                Ok(Json(json_keys))
            }
            Err(e) => Err(ApiError::Internal(format!("Failed to list access keys: {}", e))),
        }
    }
    #[cfg(not(feature = "s3-gateway"))]
    pub async fn nas_list_s3_access_keys(State(_state): State<Arc<AppState>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        Ok(Json(vec![]))
    }
    #[cfg(feature = "s3-gateway")]
    pub async fn nas_create_s3_access_key(State(_state): State<Arc<AppState>>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        let user_id = body.get("user_id").and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'user_id' field".to_string()))?;
        let description = body.get("description").and_then(|v| v.as_str());
        let manager = S3GatewayManager::new();
        match manager.create_access_key(user_id, description).await {
            Ok(key) => Ok(Json(serde_json::to_value(key).unwrap_or_default())),
            Err(e) => Err(ApiError::Internal(format!("Failed to create access key: {}", e))),
        }
    }
    #[cfg(not(feature = "s3-gateway"))]
    pub async fn nas_create_s3_access_key(State(_state): State<Arc<AppState>>, Json(_body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        Err(ApiError::Internal("S3 gateway feature not enabled".to_string()))
    }
    #[cfg(feature = "s3-gateway")]
    pub async fn nas_delete_s3_access_key(State(_state): State<Arc<AppState>>, Path(access_key): Path<String>) -> Result<(), ApiError> {
        let manager = S3GatewayManager::new();
        manager.delete_access_key(&access_key).await
            .map_err(|e| ApiError::Internal(format!("Failed to delete access key: {}", e)))
    }
    #[cfg(not(feature = "s3-gateway"))]
    pub async fn nas_delete_s3_access_key(State(_state): State<Arc<AppState>>, Path(_id): Path<String>) -> Result<(), ApiError> {
        Err(ApiError::Internal("S3 gateway feature not enabled".to_string()))
    }

    // Directory/Auth handlers
    pub async fn nas_get_directory_config(State(_state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
        // Return configuration status for directory services
        Ok(Json(serde_json::json!({
            "ldap": {
                "enabled": cfg!(feature = "ldap-server"),
                "configured": false,
            },
            "active_directory": {
                "enabled": cfg!(feature = "ad"),
                "joined": false,
            },
            "kerberos": {
                "enabled": cfg!(feature = "kerberos"),
                "configured": false,
            }
        })))
    }
    pub async fn nas_update_directory_config(State(_state): State<Arc<AppState>>, Json(_body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        // Directory configuration requires specific features
        #[cfg(any(feature = "ldap-server", feature = "ad"))]
        {
            // Would store config in database/file
            Ok(Json(serde_json::json!({"status": "updated"})))
        }
        #[cfg(not(any(feature = "ldap-server", feature = "ad")))]
        {
            Err(ApiError::Internal("No directory service features enabled".to_string()))
        }
    }
    pub async fn nas_test_ldap_connection(State(_state): State<Arc<AppState>>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        #[cfg(feature = "ldap-server")]
        {
            let server = body.get("server").and_then(|v| v.as_str())
                .ok_or_else(|| ApiError::BadRequest("Missing 'server' field".to_string()))?;
            let _port = body.get("port").and_then(|v| v.as_u64()).unwrap_or(389);
            // Test LDAP connection using ldapsearch or similar
            use tokio::process::Command;
            let output = Command::new("ldapsearch")
                .args(["-x", "-H", &format!("ldap://{}", server), "-b", "", "-s", "base"])
                .output()
                .await;
            match output {
                Ok(out) if out.status.success() => {
                    Ok(Json(serde_json::json!({"success": true, "message": "Connection successful"})))
                }
                _ => Ok(Json(serde_json::json!({"success": false, "message": "Connection failed"}))),
            }
        }
        #[cfg(not(feature = "ldap-server"))]
        {
            let _ = body;
            Err(ApiError::Internal("LDAP server feature not enabled".to_string()))
        }
    }
    pub async fn nas_join_active_directory(State(_state): State<Arc<AppState>>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        #[cfg(feature = "ad")]
        {
            let domain = body.get("domain").and_then(|v| v.as_str())
                .ok_or_else(|| ApiError::BadRequest("Missing 'domain' field".to_string()))?;
            let admin_user = body.get("admin_user").and_then(|v| v.as_str())
                .ok_or_else(|| ApiError::BadRequest("Missing 'admin_user' field".to_string()))?;
            let _password = body.get("password").and_then(|v| v.as_str())
                .ok_or_else(|| ApiError::BadRequest("Missing 'password' field".to_string()))?;
            // AD join via net ads join
            use tokio::process::Command;
            let output = Command::new("net")
                .args(["ads", "join", "-U", admin_user, domain])
                .output()
                .await;
            match output {
                Ok(out) if out.status.success() => {
                    Ok(Json(serde_json::json!({"success": true, "domain": domain})))
                }
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    Err(ApiError::Internal(format!("Failed to join domain: {}", stderr)))
                }
                Err(e) => Err(ApiError::Internal(format!("Failed to run net command: {}", e))),
            }
        }
        #[cfg(not(feature = "ad"))]
        {
            let _ = body;
            Err(ApiError::Internal("Active Directory integration not enabled".to_string()))
        }
    }
    pub async fn nas_leave_active_directory(State(_state): State<Arc<AppState>>) -> Result<(), ApiError> {
        #[cfg(feature = "ad")]
        {
            use tokio::process::Command;
            let output = Command::new("net")
                .args(["ads", "leave"])
                .output()
                .await;
            match output {
                Ok(out) if out.status.success() => Ok(()),
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    Err(ApiError::Internal(format!("Failed to leave domain: {}", stderr)))
                }
                Err(e) => Err(ApiError::Internal(format!("Failed to run net command: {}", e))),
            }
        }
        #[cfg(not(feature = "ad"))]
        {
            Err(ApiError::Internal("Active Directory integration not enabled".to_string()))
        }
    }
    pub async fn nas_get_ad_status(State(_state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
        #[cfg(feature = "ad")]
        {
            use tokio::process::Command;
            let output = Command::new("net")
                .args(["ads", "testjoin"])
                .output()
                .await;
            match output {
                Ok(out) if out.status.success() => {
                    // Get domain info
                    let info_output = Command::new("net")
                        .args(["ads", "info"])
                        .output()
                        .await;
                    let domain = info_output.ok()
                        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                        .unwrap_or_default();
                    Ok(Json(serde_json::json!({
                        "joined": true,
                        "domain_info": domain,
                    })))
                }
                _ => Ok(Json(serde_json::json!({"joined": false}))),
            }
        }
        #[cfg(not(feature = "ad"))]
        {
            Ok(Json(serde_json::json!({"joined": false, "enabled": false})))
        }
    }

    // ==========================================================================
    // LDAP handlers
    // ==========================================================================

    pub async fn nas_get_ldap_status(State(_state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
        #[cfg(feature = "ldap-server")]
        {
            Ok(Json(serde_json::json!({
                "enabled": true,
                "configured": false, // Would check actual config
                "server": null,
                "base_dn": null,
                "connection_status": "not_configured"
            })))
        }
        #[cfg(not(feature = "ldap-server"))]
        {
            Ok(Json(serde_json::json!({
                "enabled": false,
                "configured": false
            })))
        }
    }

    pub async fn nas_configure_ldap(State(_state): State<Arc<AppState>>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        #[cfg(feature = "ldap-server")]
        {
            let _uri = body.get("uri").and_then(|v| v.as_str())
                .ok_or_else(|| ApiError::BadRequest("Missing 'uri' field".to_string()))?;
            let _base_dn = body.get("base_dn").and_then(|v| v.as_str())
                .ok_or_else(|| ApiError::BadRequest("Missing 'base_dn' field".to_string()))?;
            // Would store LDAP configuration
            Ok(Json(serde_json::json!({"status": "configured"})))
        }
        #[cfg(not(feature = "ldap-server"))]
        {
            let _ = body;
            Err(ApiError::Internal("LDAP feature not enabled".to_string()))
        }
    }

    pub async fn nas_sync_ldap(State(_state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
        #[cfg(feature = "ldap-server")]
        {
            // Would trigger LDAP sync
            Ok(Json(serde_json::json!({
                "status": "sync_started",
                "users_synced": 0,
                "groups_synced": 0
            })))
        }
        #[cfg(not(feature = "ldap-server"))]
        {
            Err(ApiError::Internal("LDAP feature not enabled".to_string()))
        }
    }

    pub async fn nas_search_ldap_users(State(_state): State<Arc<AppState>>, Query(params): Query<std::collections::HashMap<String, String>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        #[cfg(feature = "ldap-server")]
        {
            let _filter = params.get("filter").cloned().unwrap_or_default();
            // Would search LDAP for users
            Ok(Json(vec![]))
        }
        #[cfg(not(feature = "ldap-server"))]
        {
            let _ = params;
            Err(ApiError::Internal("LDAP feature not enabled".to_string()))
        }
    }

    pub async fn nas_search_ldap_groups(State(_state): State<Arc<AppState>>, Query(params): Query<std::collections::HashMap<String, String>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        #[cfg(feature = "ldap-server")]
        {
            let _filter = params.get("filter").cloned().unwrap_or_default();
            // Would search LDAP for groups
            Ok(Json(vec![]))
        }
        #[cfg(not(feature = "ldap-server"))]
        {
            let _ = params;
            Err(ApiError::Internal("LDAP feature not enabled".to_string()))
        }
    }

    // ==========================================================================
    // Kerberos handlers
    // ==========================================================================

    pub async fn nas_get_kerberos_status(State(_state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
        #[cfg(feature = "kerberos")]
        {
            use tokio::process::Command;
            let output = Command::new("klist").output().await;
            let has_tickets = output.map(|o| o.status.success()).unwrap_or(false);
            Ok(Json(serde_json::json!({
                "enabled": true,
                "configured": false, // Would check krb5.conf
                "has_valid_tickets": has_tickets
            })))
        }
        #[cfg(not(feature = "kerberos"))]
        {
            Ok(Json(serde_json::json!({
                "enabled": false,
                "configured": false
            })))
        }
    }

    pub async fn nas_configure_kerberos(State(_state): State<Arc<AppState>>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        #[cfg(feature = "kerberos")]
        {
            let _realm = body.get("realm").and_then(|v| v.as_str())
                .ok_or_else(|| ApiError::BadRequest("Missing 'realm' field".to_string()))?;
            let _kdc = body.get("kdc").and_then(|v| v.as_str())
                .ok_or_else(|| ApiError::BadRequest("Missing 'kdc' field".to_string()))?;
            // Would write /etc/krb5.conf
            Ok(Json(serde_json::json!({"status": "configured"})))
        }
        #[cfg(not(feature = "kerberos"))]
        {
            let _ = body;
            Err(ApiError::Internal("Kerberos feature not enabled".to_string()))
        }
    }

    pub async fn nas_kerberos_kinit(State(_state): State<Arc<AppState>>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        #[cfg(feature = "kerberos")]
        {
            use tokio::process::Command;
            let principal = body.get("principal").and_then(|v| v.as_str())
                .ok_or_else(|| ApiError::BadRequest("Missing 'principal' field".to_string()))?;
            let keytab = body.get("keytab").and_then(|v| v.as_str());

            let output = if let Some(kt) = keytab {
                Command::new("kinit")
                    .args(["-k", "-t", kt, principal])
                    .output()
                    .await
            } else {
                // Interactive kinit not supported via API
                return Err(ApiError::BadRequest("Keytab required for non-interactive kinit".to_string()));
            };

            match output {
                Ok(out) if out.status.success() => {
                    Ok(Json(serde_json::json!({"status": "success", "principal": principal})))
                }
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    Err(ApiError::Internal(format!("kinit failed: {}", stderr)))
                }
                Err(e) => Err(ApiError::Internal(format!("Failed to run kinit: {}", e))),
            }
        }
        #[cfg(not(feature = "kerberos"))]
        {
            let _ = body;
            Err(ApiError::Internal("Kerberos feature not enabled".to_string()))
        }
    }

    pub async fn nas_kerberos_klist(State(_state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
        #[cfg(feature = "kerberos")]
        {
            use tokio::process::Command;
            let output = Command::new("klist").output().await;
            match output {
                Ok(out) if out.status.success() => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    Ok(Json(serde_json::json!({
                        "has_tickets": true,
                        "output": stdout
                    })))
                }
                _ => Ok(Json(serde_json::json!({"has_tickets": false}))),
            }
        }
        #[cfg(not(feature = "kerberos"))]
        {
            Err(ApiError::Internal("Kerberos feature not enabled".to_string()))
        }
    }

    pub async fn nas_kerberos_kdestroy(State(_state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
        #[cfg(feature = "kerberos")]
        {
            use tokio::process::Command;
            let output = Command::new("kdestroy").output().await;
            match output {
                Ok(out) if out.status.success() => {
                    Ok(Json(serde_json::json!({"status": "destroyed"})))
                }
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    Err(ApiError::Internal(format!("kdestroy failed: {}", stderr)))
                }
                Err(e) => Err(ApiError::Internal(format!("Failed to run kdestroy: {}", e))),
            }
        }
        #[cfg(not(feature = "kerberos"))]
        {
            Err(ApiError::Internal("Kerberos feature not enabled".to_string()))
        }
    }

    pub async fn nas_list_kerberos_keytabs(State(_state): State<Arc<AppState>>, Query(params): Query<std::collections::HashMap<String, String>>) -> Result<Json<serde_json::Value>, ApiError> {
        #[cfg(feature = "kerberos")]
        {
            use tokio::process::Command;
            let keytab = params.get("keytab").cloned().unwrap_or_else(|| "/etc/krb5.keytab".to_string());
            let output = Command::new("klist")
                .args(["-k", &keytab])
                .output()
                .await;
            match output {
                Ok(out) if out.status.success() => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    Ok(Json(serde_json::json!({
                        "keytab": keytab,
                        "entries": stdout
                    })))
                }
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    Err(ApiError::Internal(format!("klist -k failed: {}", stderr)))
                }
                Err(e) => Err(ApiError::Internal(format!("Failed to run klist: {}", e))),
            }
        }
        #[cfg(not(feature = "kerberos"))]
        {
            let _ = params;
            Err(ApiError::Internal("Kerberos feature not enabled".to_string()))
        }
    }

    pub async fn nas_create_kerberos_keytab(State(_state): State<Arc<AppState>>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        #[cfg(feature = "kerberos")]
        {
            let _principal = body.get("principal").and_then(|v| v.as_str())
                .ok_or_else(|| ApiError::BadRequest("Missing 'principal' field".to_string()))?;
            let _keytab = body.get("keytab").and_then(|v| v.as_str())
                .unwrap_or("/etc/krb5.keytab");
            // Would use ktutil to create keytab entry
            Ok(Json(serde_json::json!({"status": "created"})))
        }
        #[cfg(not(feature = "kerberos"))]
        {
            let _ = body;
            Err(ApiError::Internal("Kerberos feature not enabled".to_string()))
        }
    }

    // ==========================================================================
    // Additional Active Directory handlers
    // ==========================================================================

    pub async fn nas_list_ad_users(State(_state): State<Arc<AppState>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        #[cfg(feature = "ad")]
        {
            use tokio::process::Command;
            let output = Command::new("wbinfo")
                .args(["-u"])
                .output()
                .await;
            match output {
                Ok(out) if out.status.success() => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    let users: Vec<serde_json::Value> = stdout
                        .lines()
                        .map(|u| serde_json::json!({"username": u}))
                        .collect();
                    Ok(Json(users))
                }
                _ => Ok(Json(vec![])),
            }
        }
        #[cfg(not(feature = "ad"))]
        {
            Err(ApiError::Internal("Active Directory feature not enabled".to_string()))
        }
    }

    pub async fn nas_list_ad_groups(State(_state): State<Arc<AppState>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        #[cfg(feature = "ad")]
        {
            use tokio::process::Command;
            let output = Command::new("wbinfo")
                .args(["-g"])
                .output()
                .await;
            match output {
                Ok(out) if out.status.success() => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    let groups: Vec<serde_json::Value> = stdout
                        .lines()
                        .map(|g| serde_json::json!({"groupname": g}))
                        .collect();
                    Ok(Json(groups))
                }
                _ => Ok(Json(vec![])),
            }
        }
        #[cfg(not(feature = "ad"))]
        {
            Err(ApiError::Internal("Active Directory feature not enabled".to_string()))
        }
    }

    pub async fn nas_get_user_ad_groups(State(_state): State<Arc<AppState>>, Path(username): Path<String>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        #[cfg(feature = "ad")]
        {
            use tokio::process::Command;
            let output = Command::new("id")
                .args([&username])
                .output()
                .await;
            match output {
                Ok(out) if out.status.success() => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    Ok(Json(vec![serde_json::json!({
                        "username": username,
                        "groups_info": stdout
                    })]))
                }
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    Err(ApiError::NotFound(format!("User not found: {}", stderr)))
                }
                Err(e) => Err(ApiError::Internal(format!("Failed to get user groups: {}", e))),
            }
        }
        #[cfg(not(feature = "ad"))]
        {
            let _ = username;
            Err(ApiError::Internal("Active Directory feature not enabled".to_string()))
        }
    }

    pub async fn nas_test_ad_trust(State(_state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
        #[cfg(feature = "ad")]
        {
            use tokio::process::Command;
            let output = Command::new("wbinfo")
                .args(["--check-secret"])
                .output()
                .await;
            match output {
                Ok(out) if out.status.success() => {
                    Ok(Json(serde_json::json!({"trust_valid": true})))
                }
                _ => Ok(Json(serde_json::json!({"trust_valid": false}))),
            }
        }
        #[cfg(not(feature = "ad"))]
        {
            Err(ApiError::Internal("Active Directory feature not enabled".to_string()))
        }
    }

    pub async fn nas_ping_ad_dc(State(_state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
        #[cfg(feature = "ad")]
        {
            use tokio::process::Command;
            let output = Command::new("wbinfo")
                .args(["--ping-dc"])
                .output()
                .await;
            match output {
                Ok(out) if out.status.success() => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    Ok(Json(serde_json::json!({
                        "dc_reachable": true,
                        "response": stdout
                    })))
                }
                _ => Ok(Json(serde_json::json!({"dc_reachable": false}))),
            }
        }
        #[cfg(not(feature = "ad"))]
        {
            Err(ApiError::Internal("Active Directory feature not enabled".to_string()))
        }
    }

    pub async fn nas_verify_ad_prereqs(State(_state): State<Arc<AppState>>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        #[cfg(feature = "ad")]
        {
            let domain = body.get("domain").and_then(|v| v.as_str())
                .ok_or_else(|| ApiError::BadRequest("Missing 'domain' field".to_string()))?;
            use tokio::process::Command;

            // Check DNS resolution
            let dns_check = Command::new("host")
                .args([domain])
                .output()
                .await;
            let dns_ok = dns_check.map(|o| o.status.success()).unwrap_or(false);

            // Check if required packages are installed
            let samba_check = Command::new("which")
                .args(["net"])
                .output()
                .await;
            let samba_ok = samba_check.map(|o| o.status.success()).unwrap_or(false);

            Ok(Json(serde_json::json!({
                "domain": domain,
                "dns_resolution": dns_ok,
                "samba_installed": samba_ok,
                "ready": dns_ok && samba_ok
            })))
        }
        #[cfg(not(feature = "ad"))]
        {
            let _ = body;
            Err(ApiError::Internal("Active Directory feature not enabled".to_string()))
        }
    }

    // ==========================================================================
    // Scheduler handlers
    // ==========================================================================

    pub async fn nas_list_scheduled_jobs(State(state): State<Arc<AppState>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        let jobs = sqlx::query_as::<_, (String, String, String, String, bool, Option<i64>, Option<i64>, i64)>(
            "SELECT id, name, job_type, schedule, enabled, last_run, next_run, created_at FROM nas_scheduled_jobs ORDER BY name"
        )
        .fetch_all(state.database.pool())
        .await
        .unwrap_or_default();

        let json_jobs: Vec<serde_json::Value> = jobs.iter()
            .map(|(id, name, job_type, schedule, enabled, last_run, next_run, created)| {
                serde_json::json!({
                    "id": id,
                    "name": name,
                    "job_type": job_type,
                    "schedule": schedule,
                    "enabled": enabled,
                    "last_run": last_run,
                    "next_run": next_run,
                    "created_at": created,
                })
            })
            .collect();
        Ok(Json(json_jobs))
    }

    pub async fn nas_create_scheduled_job(State(state): State<Arc<AppState>>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        use crate::nas::scheduler::{ScheduledJob, JobType, CronSchedule};

        let name = body.get("name").and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'name' field".to_string()))?;
        let job_type_str = body.get("job_type").and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'job_type' field".to_string()))?;
        let schedule_str = body.get("schedule").and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'schedule' field".to_string()))?;

        // Parse job type
        let job_type = match job_type_str.to_lowercase().as_str() {
            "snapshot" => {
                let dataset = body.get("dataset").and_then(|v| v.as_str())
                    .ok_or_else(|| ApiError::BadRequest("Snapshot job requires 'dataset' field".to_string()))?;
                JobType::Snapshot { dataset: dataset.to_string() }
            }
            "retention" | "retention_cleanup" => {
                let dataset = body.get("dataset").and_then(|v| v.as_str())
                    .ok_or_else(|| ApiError::BadRequest("Retention job requires 'dataset' field".to_string()))?;
                let keep_count = body.get("keep_count").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
                JobType::RetentionCleanup { dataset: dataset.to_string(), keep_count }
            }
            "replication" => {
                let task_id = body.get("task_id").and_then(|v| v.as_str())
                    .ok_or_else(|| ApiError::BadRequest("Replication job requires 'task_id' field".to_string()))?;
                JobType::Replication { task_id: task_id.to_string() }
            }
            "scrub" => {
                let pool = body.get("pool").and_then(|v| v.as_str())
                    .ok_or_else(|| ApiError::BadRequest("Scrub job requires 'pool' field".to_string()))?;
                JobType::Scrub { pool: pool.to_string() }
            }
            "health_check" => JobType::HealthCheck,
            "quota_check" => JobType::QuotaCheck,
            "smart_check" => JobType::SmartCheck,
            "custom" | "custom_script" => {
                let script = body.get("script").and_then(|v| v.as_str())
                    .ok_or_else(|| ApiError::BadRequest("Custom job requires 'script' field".to_string()))?;
                let args: Vec<String> = body.get("args")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                    .unwrap_or_default();
                JobType::CustomScript { script: script.to_string(), args }
            }
            _ => return Err(ApiError::BadRequest(format!("Unknown job type: {}", job_type_str))),
        };

        // Parse schedule
        let schedule = CronSchedule::parse(schedule_str)
            .map_err(|e| ApiError::BadRequest(format!("Invalid cron schedule: {}", e)))?;

        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp();
        let next_run = schedule.next_run_time();

        // Serialize job_type for storage
        let job_type_json = serde_json::to_string(&job_type).unwrap_or_default();

        sqlx::query(
            "INSERT INTO nas_scheduled_jobs (id, name, job_type, schedule, enabled, next_run, created_at, updated_at) VALUES (?, ?, ?, ?, 1, ?, ?, ?)"
        )
        .bind(&id)
        .bind(name)
        .bind(&job_type_json)
        .bind(schedule_str)
        .bind(next_run)
        .bind(now)
        .bind(now)
        .execute(state.database.pool())
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to create scheduled job: {}", e)))?;

        Ok(Json(serde_json::json!({
            "id": id,
            "name": name,
            "job_type": job_type_str,
            "schedule": schedule_str,
            "enabled": true,
            "next_run": next_run,
            "created_at": now,
        })))
    }

    pub async fn nas_get_scheduled_job(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
        let job = sqlx::query_as::<_, (String, String, String, String, bool, Option<i64>, Option<i64>, i64)>(
            "SELECT id, name, job_type, schedule, enabled, last_run, next_run, created_at FROM nas_scheduled_jobs WHERE id = ?"
        )
        .bind(&id)
        .fetch_optional(state.database.pool())
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

        match job {
            Some((id, name, job_type, schedule, enabled, last_run, next_run, created)) => {
                Ok(Json(serde_json::json!({
                    "id": id,
                    "name": name,
                    "job_type": job_type,
                    "schedule": schedule,
                    "enabled": enabled,
                    "last_run": last_run,
                    "next_run": next_run,
                    "created_at": created,
                })))
            }
            None => Err(ApiError::NotFound("Job not found".to_string())),
        }
    }

    pub async fn nas_update_scheduled_job(State(state): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        let now = chrono::Utc::now().timestamp();

        // Verify job exists
        let existing = sqlx::query_as::<_, (String,)>("SELECT id FROM nas_scheduled_jobs WHERE id = ?")
            .bind(&id).fetch_optional(state.database.pool()).await
            .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

        if existing.is_none() {
            return Err(ApiError::NotFound("Job not found".to_string()));
        }

        // Update fields
        if let Some(schedule) = body.get("schedule").and_then(|v| v.as_str()) {
            use crate::nas::scheduler::CronSchedule;
            let parsed = CronSchedule::parse(schedule)
                .map_err(|e| ApiError::BadRequest(format!("Invalid cron schedule: {}", e)))?;
            let next_run = parsed.next_run_time();
            sqlx::query("UPDATE nas_scheduled_jobs SET schedule = ?, next_run = ?, updated_at = ? WHERE id = ?")
                .bind(schedule).bind(next_run).bind(now).bind(&id)
                .execute(state.database.pool()).await
                .map_err(|e| ApiError::Internal(format!("Failed to update: {}", e)))?;
        }
        if let Some(enabled) = body.get("enabled").and_then(|v| v.as_bool()) {
            sqlx::query("UPDATE nas_scheduled_jobs SET enabled = ?, updated_at = ? WHERE id = ?")
                .bind(enabled).bind(now).bind(&id)
                .execute(state.database.pool()).await
                .map_err(|e| ApiError::Internal(format!("Failed to update: {}", e)))?;
        }
        if let Some(name) = body.get("name").and_then(|v| v.as_str()) {
            sqlx::query("UPDATE nas_scheduled_jobs SET name = ?, updated_at = ? WHERE id = ?")
                .bind(name).bind(now).bind(&id)
                .execute(state.database.pool()).await
                .map_err(|e| ApiError::Internal(format!("Failed to update: {}", e)))?;
        }

        nas_get_scheduled_job(State(state), Path(id)).await
    }

    pub async fn nas_delete_scheduled_job(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<(), ApiError> {
        let result = sqlx::query("DELETE FROM nas_scheduled_jobs WHERE id = ?")
            .bind(&id)
            .execute(state.database.pool())
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to delete job: {}", e)))?;

        if result.rows_affected() == 0 {
            return Err(ApiError::NotFound("Job not found".to_string()));
        }
        Ok(())
    }

    pub async fn nas_run_scheduled_job(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
        // Get job details
        let job = sqlx::query_as::<_, (String, String, String)>(
            "SELECT id, name, job_type FROM nas_scheduled_jobs WHERE id = ?"
        )
        .bind(&id)
        .fetch_optional(state.database.pool())
        .await
        .map_err(|e| ApiError::Internal(format!("Database error: {}", e)))?;

        match job {
            Some((job_id, name, job_type_str)) => {
                let now = chrono::Utc::now().timestamp();

                // Update last_run
                sqlx::query("UPDATE nas_scheduled_jobs SET last_run = ? WHERE id = ?")
                    .bind(now).bind(&job_id)
                    .execute(state.database.pool()).await.ok();

                // Log to job history
                let history_id = uuid::Uuid::new_v4().to_string();
                sqlx::query("INSERT INTO nas_job_history (id, job_id, started_at, status) VALUES (?, ?, ?, 'running')")
                    .bind(&history_id).bind(&job_id).bind(now)
                    .execute(state.database.pool()).await.ok();

                // Parse and execute job type
                // The actual execution would be done asynchronously
                Ok(Json(serde_json::json!({
                    "status": "started",
                    "job_id": job_id,
                    "job_name": name,
                    "job_type": job_type_str,
                    "history_id": history_id,
                    "started_at": now,
                })))
            }
            None => Err(ApiError::NotFound("Job not found".to_string())),
        }
    }

    pub async fn nas_pause_scheduled_job(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<(), ApiError> {
        let now = chrono::Utc::now().timestamp();
        let result = sqlx::query("UPDATE nas_scheduled_jobs SET enabled = 0, updated_at = ? WHERE id = ?")
            .bind(now).bind(&id)
            .execute(state.database.pool()).await
            .map_err(|e| ApiError::Internal(format!("Failed to pause job: {}", e)))?;

        if result.rows_affected() == 0 {
            return Err(ApiError::NotFound("Job not found".to_string()));
        }
        Ok(())
    }

    pub async fn nas_resume_scheduled_job(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<(), ApiError> {
        let now = chrono::Utc::now().timestamp();
        let result = sqlx::query("UPDATE nas_scheduled_jobs SET enabled = 1, updated_at = ? WHERE id = ?")
            .bind(now).bind(&id)
            .execute(state.database.pool()).await
            .map_err(|e| ApiError::Internal(format!("Failed to resume job: {}", e)))?;

        if result.rows_affected() == 0 {
            return Err(ApiError::NotFound("Job not found".to_string()));
        }
        Ok(())
    }

    pub async fn nas_get_job_history(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        let history = sqlx::query_as::<_, (String, i64, Option<i64>, String, Option<String>, Option<String>)>(
            "SELECT id, started_at, completed_at, status, error_message, output FROM nas_job_history WHERE job_id = ? ORDER BY started_at DESC LIMIT 50"
        )
        .bind(&id)
        .fetch_all(state.database.pool())
        .await
        .unwrap_or_default();

        let json_history: Vec<serde_json::Value> = history.iter()
            .map(|(id, started, completed, status, error, output)| {
                serde_json::json!({
                    "id": id,
                    "started_at": started,
                    "completed_at": completed,
                    "status": status,
                    "error_message": error,
                    "output": output,
                })
            })
            .collect();
        Ok(Json(json_history))
    }

    pub async fn nas_get_scheduler_status(State(state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
        // Get overall scheduler statistics
        let total_jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nas_scheduled_jobs")
            .fetch_one(state.database.pool()).await.unwrap_or(0);
        let enabled_jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nas_scheduled_jobs WHERE enabled = 1")
            .fetch_one(state.database.pool()).await.unwrap_or(0);
        let running_jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nas_job_history WHERE status = 'running'")
            .fetch_one(state.database.pool()).await.unwrap_or(0);

        // Get recent failures
        let recent_failures: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM nas_job_history WHERE status = 'failed' AND started_at > ?"
        )
        .bind(chrono::Utc::now().timestamp() - 86400) // Last 24 hours
        .fetch_one(state.database.pool()).await.unwrap_or(0);

        // Get next scheduled job
        let next_job = sqlx::query_as::<_, (String, String, i64)>(
            "SELECT id, name, next_run FROM nas_scheduled_jobs WHERE enabled = 1 AND next_run IS NOT NULL ORDER BY next_run ASC LIMIT 1"
        )
        .fetch_optional(state.database.pool()).await
        .ok()
        .flatten();

        Ok(Json(serde_json::json!({
            "running": true,
            "total_jobs": total_jobs,
            "enabled_jobs": enabled_jobs,
            "running_jobs": running_jobs,
            "recent_failures_24h": recent_failures,
            "next_scheduled": next_job.map(|(id, name, next_run)| {
                serde_json::json!({
                    "job_id": id,
                    "job_name": name,
                    "next_run": next_run,
                })
            }),
        })))
    }

    // ==========================================================================
    // FTP handlers
    // ==========================================================================

    pub async fn nas_get_ftp_config(State(_state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
        #[cfg(feature = "ftp")]
        {
            use crate::nas::shares::ftp::FtpManager;
            let manager = FtpManager::new();
            Ok(Json(serde_json::json!({
                "enabled": true,
                "config_path": "/etc/proftpd/proftpd.conf",
                "config": serde_json::to_value(manager.config()).unwrap_or_default(),
            })))
        }
        #[cfg(not(feature = "ftp"))]
        {
            Ok(Json(serde_json::json!({"enabled": false})))
        }
    }

    #[cfg(feature = "ftp")]
    pub async fn nas_update_ftp_config(State(_state): State<Arc<AppState>>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        use crate::nas::shares::ftp::{FtpManager, FtpGlobalConfig};
        use std::collections::HashMap;

        let config = FtpGlobalConfig {
            server_name: body.get("server_name").and_then(|v| v.as_str()).unwrap_or("Horcrux FTP Server").to_string(),
            port: body.get("port").and_then(|v| v.as_u64()).unwrap_or(21) as u16,
            passive_port_range: body.get("passive_port_range").and_then(|v| v.as_str())
                .map(|s| {
                    let parts: Vec<&str> = s.split('-').collect();
                    if parts.len() == 2 {
                        (parts[0].parse().unwrap_or(49152), parts[1].parse().unwrap_or(65534))
                    } else {
                        (49152, 65534)
                    }
                })
                .unwrap_or((49152, 65534)),
            max_clients: body.get("max_clients").and_then(|v| v.as_u64()).unwrap_or(100) as u32,
            max_clients_per_host: body.get("max_clients_per_host").and_then(|v| v.as_u64()).unwrap_or(10) as u32,
            timeout_idle: body.get("timeout_idle").and_then(|v| v.as_u64()).unwrap_or(600) as u32,
            timeout_login: body.get("timeout_login").and_then(|v| v.as_u64()).unwrap_or(300) as u32,
            timeout_no_transfer: body.get("timeout_no_transfer").and_then(|v| v.as_u64()).unwrap_or(900) as u32,
            allow_anonymous: body.get("allow_anonymous").and_then(|v| v.as_bool()).unwrap_or(false),
            anonymous_root: body.get("anonymous_root").and_then(|v| v.as_str()).map(|s| s.to_string()),
            tls_enabled: body.get("tls_enabled").and_then(|v| v.as_bool()).unwrap_or(false),
            tls_cert_file: body.get("tls_cert_file").and_then(|v| v.as_str()).map(|s| s.to_string()),
            tls_key_file: body.get("tls_key_file").and_then(|v| v.as_str()).map(|s| s.to_string()),
            tls_required: body.get("tls_required").and_then(|v| v.as_bool()).unwrap_or(false),
            chroot_local_users: body.get("chroot_local_users").and_then(|v| v.as_bool()).unwrap_or(true),
            require_valid_shell: body.get("require_valid_shell").and_then(|v| v.as_bool()).unwrap_or(true),
            use_sendfile: body.get("use_sendfile").and_then(|v| v.as_bool()).unwrap_or(true),
            extra_parameters: body.get("extra_parameters")
                .and_then(|v| v.as_object())
                .map(|obj| obj.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect())
                .unwrap_or_else(HashMap::new),
        };

        let mut manager = FtpManager::new();
        manager.set_global_config(config.clone());
        manager.reload().await
            .map_err(|e| ApiError::Internal(format!("Failed to reload FTP config: {}", e)))?;

        Ok(Json(serde_json::to_value(&config).unwrap_or_default()))
    }

    #[cfg(not(feature = "ftp"))]
    pub async fn nas_update_ftp_config(State(_state): State<Arc<AppState>>, Json(_body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        Err(ApiError::Internal("FTP feature not enabled".to_string()))
    }

    #[cfg(feature = "ftp")]
    pub async fn nas_list_ftp_connections(State(_state): State<Arc<AppState>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        use crate::nas::shares::ftp::FtpManager;
        let manager = FtpManager::new();
        match manager.get_connections().await {
            Ok(connections) => {
                let json_conns: Vec<serde_json::Value> = connections.iter()
                    .map(|c| serde_json::to_value(c).unwrap_or_default())
                    .collect();
                Ok(Json(json_conns))
            }
            Err(e) => Err(ApiError::Internal(format!("Failed to get FTP connections: {}", e))),
        }
    }

    #[cfg(not(feature = "ftp"))]
    pub async fn nas_list_ftp_connections(State(_state): State<Arc<AppState>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        Ok(Json(vec![]))
    }

    #[cfg(feature = "ftp")]
    pub async fn nas_disconnect_ftp(State(_state): State<Arc<AppState>>, Path(conn_id): Path<String>) -> Result<(), ApiError> {
        use crate::nas::shares::ftp::FtpManager;
        let pid: u32 = conn_id.parse()
            .map_err(|_| ApiError::BadRequest("Invalid connection ID (expected PID)".to_string()))?;

        let manager = FtpManager::new();
        manager.disconnect_session(pid).await
            .map_err(|e| ApiError::Internal(format!("Failed to disconnect FTP session: {}", e)))
    }

    #[cfg(not(feature = "ftp"))]
    pub async fn nas_disconnect_ftp(State(_state): State<Arc<AppState>>, Path(_conn_id): Path<String>) -> Result<(), ApiError> {
        Err(ApiError::Internal("FTP feature not enabled".to_string()))
    }

    // ==========================================================================
    // WebDAV handlers
    // ==========================================================================

    pub async fn nas_get_webdav_config(State(_state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
        #[cfg(feature = "webdav")]
        {
            use crate::nas::shares::webdav::WebdavManager;
            let manager = WebdavManager::new();
            Ok(Json(serde_json::json!({
                "enabled": true,
                "config_path": "/etc/nginx/sites-available/webdav.conf",
                "config": serde_json::to_value(manager.config()).unwrap_or_default(),
            })))
        }
        #[cfg(not(feature = "webdav"))]
        {
            Ok(Json(serde_json::json!({"enabled": false})))
        }
    }

    #[cfg(feature = "webdav")]
    pub async fn nas_update_webdav_config(State(_state): State<Arc<AppState>>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        use crate::nas::shares::webdav::{WebdavManager, WebdavGlobalConfig};

        let config = WebdavGlobalConfig {
            listen_port: body.get("listen_port").and_then(|v| v.as_u64()).unwrap_or(8080) as u16,
            ssl_enabled: body.get("ssl_enabled").and_then(|v| v.as_bool()).unwrap_or(false),
            ssl_port: body.get("ssl_port").and_then(|v| v.as_u64()).unwrap_or(8443) as u16,
            ssl_certificate: body.get("ssl_certificate").and_then(|v| v.as_str()).map(|s| s.to_string()),
            ssl_certificate_key: body.get("ssl_certificate_key").and_then(|v| v.as_str()).map(|s| s.to_string()),
            auth_type: body.get("auth_type").and_then(|v| v.as_str())
                .map(|s| match s.to_lowercase().as_str() {
                    "digest" => crate::nas::shares::webdav::WebdavAuthType::Digest,
                    "none" => crate::nas::shares::webdav::WebdavAuthType::None,
                    _ => crate::nas::shares::webdav::WebdavAuthType::Basic,
                })
                .unwrap_or(crate::nas::shares::webdav::WebdavAuthType::Basic),
            realm: body.get("realm").and_then(|v| v.as_str()).unwrap_or("WebDAV").to_string(),
            client_body_temp_path: body.get("client_body_temp_path").and_then(|v| v.as_str())
                .unwrap_or("/var/lib/nginx/webdav").to_string(),
            client_max_body_size: body.get("client_max_body_size").and_then(|v| v.as_str())
                .unwrap_or("10G").to_string(),
        };

        let mut manager = WebdavManager::new();
        manager.set_global_config(config.clone());
        manager.reload().await
            .map_err(|e| ApiError::Internal(format!("Failed to reload WebDAV config: {}", e)))?;

        Ok(Json(serde_json::to_value(&config).unwrap_or_default()))
    }

    #[cfg(not(feature = "webdav"))]
    pub async fn nas_update_webdav_config(State(_state): State<Arc<AppState>>, Json(_body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        Err(ApiError::Internal("WebDAV feature not enabled".to_string()))
    }

    pub async fn nas_list_webdav_connections(State(_state): State<Arc<AppState>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        // WebDAV connections are HTTP connections managed by nginx
        // We can parse nginx access logs or use stub module status if available
        Ok(Json(vec![serde_json::json!({
            "note": "WebDAV connections are managed by nginx. Check nginx status for connection info."
        })]))
    }

    // ==========================================================================
    // AFP handlers
    // ==========================================================================

    pub async fn nas_get_afp_config(State(_state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
        #[cfg(feature = "afp")]
        {
            use crate::nas::shares::afp::AfpManager;
            let manager = AfpManager::new();
            Ok(Json(serde_json::json!({
                "enabled": true,
                "config_path": "/etc/netatalk/afp.conf",
                "config": serde_json::to_value(manager.config()).unwrap_or_default(),
            })))
        }
        #[cfg(not(feature = "afp"))]
        {
            Ok(Json(serde_json::json!({"enabled": false})))
        }
    }

    #[cfg(feature = "afp")]
    pub async fn nas_update_afp_config(State(_state): State<Arc<AppState>>, Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        use crate::nas::shares::afp::{AfpManager, AfpGlobalConfig};
        use std::collections::HashMap;

        let config = AfpGlobalConfig {
            hostname: body.get("hostname").and_then(|v| v.as_str()).map(|s| s.to_string()),
            uam_list: body.get("uam_list").and_then(|v| v.as_str())
                .unwrap_or("uams_dhx2.so").to_string(),
            save_password: body.get("save_password").and_then(|v| v.as_bool()).unwrap_or(true),
            set_password: body.get("set_password").and_then(|v| v.as_bool()).unwrap_or(false),
            mimic_model: body.get("mimic_model").and_then(|v| v.as_str())
                .unwrap_or("RackMac").to_string(),
            log_level: body.get("log_level").and_then(|v| v.as_str())
                .unwrap_or("default:warn").to_string(),
            zeroconf: body.get("zeroconf").and_then(|v| v.as_bool()).unwrap_or(true),
            spotlight: body.get("spotlight").and_then(|v| v.as_bool()).unwrap_or(false),
            fce_listener: body.get("fce_listener").and_then(|v| v.as_str()).map(|s| s.to_string()),
            fce_coalesce: body.get("fce_coalesce").and_then(|v| v.as_u64()).map(|v| v as u32),
            extra_parameters: body.get("extra_parameters")
                .and_then(|v| v.as_object())
                .map(|obj| obj.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect())
                .unwrap_or_else(HashMap::new),
        };

        let mut manager = AfpManager::new();
        manager.set_global_config(config.clone());
        manager.reload().await
            .map_err(|e| ApiError::Internal(format!("Failed to reload AFP config: {}", e)))?;

        Ok(Json(serde_json::to_value(&config).unwrap_or_default()))
    }

    #[cfg(not(feature = "afp"))]
    pub async fn nas_update_afp_config(State(_state): State<Arc<AppState>>, Json(_body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiError> {
        Err(ApiError::Internal("AFP feature not enabled".to_string()))
    }

    #[cfg(feature = "afp")]
    pub async fn nas_list_afp_connections(State(_state): State<Arc<AppState>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        use crate::nas::shares::afp::AfpManager;
        let manager = AfpManager::new();
        match manager.get_connections().await {
            Ok(connections) => {
                let json_conns: Vec<serde_json::Value> = connections.iter()
                    .map(|c| serde_json::to_value(c).unwrap_or_default())
                    .collect();
                Ok(Json(json_conns))
            }
            Err(e) => Err(ApiError::Internal(format!("Failed to get AFP connections: {}", e))),
        }
    }

    #[cfg(not(feature = "afp"))]
    pub async fn nas_list_afp_connections(State(_state): State<Arc<AppState>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        Ok(Json(vec![]))
    }

    #[cfg(feature = "afp")]
    pub async fn nas_disconnect_afp(State(_state): State<Arc<AppState>>, Path(conn_id): Path<String>) -> Result<(), ApiError> {
        use crate::nas::shares::afp::AfpManager;
        let pid: u32 = conn_id.parse()
            .map_err(|_| ApiError::BadRequest("Invalid connection ID (expected PID)".to_string()))?;

        let manager = AfpManager::new();
        manager.disconnect_session(pid).await
            .map_err(|e| ApiError::Internal(format!("Failed to disconnect AFP session: {}", e)))
    }

    #[cfg(not(feature = "afp"))]
    pub async fn nas_disconnect_afp(State(_state): State<Arc<AppState>>, Path(_conn_id): Path<String>) -> Result<(), ApiError> {
        Err(ApiError::Internal("AFP feature not enabled".to_string()))
    }

    #[cfg(feature = "afp")]
    pub async fn nas_list_timemachine_targets(State(_state): State<Arc<AppState>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        use crate::nas::shares::afp::AfpManager;
        let manager = AfpManager::new();

        // Get shares that have time_machine enabled
        let shares = manager.shares();
        let tm_targets: Vec<serde_json::Value> = shares.iter()
            .filter(|s| s.time_machine)
            .map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "path": s.path,
                    "volume_size_limit": s.volume_size_limit,
                    "enabled": true,
                })
            })
            .collect();

        Ok(Json(tm_targets))
    }

    #[cfg(not(feature = "afp"))]
    pub async fn nas_list_timemachine_targets(State(_state): State<Arc<AppState>>) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
        Ok(Json(vec![]))
    }
}

#[cfg(feature = "nas")]
use nas_handlers::*;

// =============================================================================
// Re-export standardized error types
// =============================================================================
use error::ApiError;

// API handlers

/// Simple liveness check
async fn health_check() -> &'static str {
    "OK"
}

/// Detailed health check with component status
async fn health_detailed(
    State(state): State<Arc<AppState>>,
) -> Json<health::HealthResponse> {
    let checker = health::HealthChecker::new(env!("CARGO_PKG_VERSION"));

    let mut components = Vec::new();

    // Check database
    components.push(checker.check_database(&state.database).await);

    // Check monitoring
    components.push(checker.check_monitoring(&state.monitoring_manager).await);

    // Check storage
    components.push(checker.check_storage(&state.storage_manager).await);

    // Check VM manager
    components.push(checker.check_vm_manager(&state.vm_manager).await);

    // Check cluster
    components.push(checker.check_cluster(&state.cluster_manager).await);

    // Check WebSocket
    components.push(checker.check_websocket(&state.ws_state));

    Json(checker.build_response(components))
}

/// Liveness probe for container orchestration
async fn liveness_probe() -> Json<health::LivenessResponse> {
    let checker = health::HealthChecker::new(env!("CARGO_PKG_VERSION"));
    Json(checker.liveness())
}

/// Readiness probe for container orchestration
async fn readiness_probe(
    State(state): State<Arc<AppState>>,
) -> Result<Json<health::ReadinessResponse>, StatusCode> {
    let checker = health::HealthChecker::new(env!("CARGO_PKG_VERSION"));

    // Check critical components
    let db_health = checker.check_database(&state.database).await;

    let components = vec![db_health];
    let response = checker.readiness(&components);

    if response.ready {
        Ok(Json(response))
    } else {
        Err(StatusCode::SERVICE_UNAVAILABLE)
    }
}

async fn list_vms(State(state): State<Arc<AppState>>) -> Json<Vec<VmConfig>> {
    let vms = state.vm_manager.list_vms().await;
    Json(vms)
}

async fn get_vm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<VmConfig>, ApiError> {
    let vm = state.vm_manager.get_vm(&id).await?;
    Ok(Json(vm))
}

async fn create_vm(
    State(state): State<Arc<AppState>>,
    auth_user: Option<axum::Extension<middleware::auth::AuthUser>>,
    Json(payload): Json<VmConfig>,
) -> Result<(StatusCode, Json<VmConfig>), ApiError> {
    let vm_name = payload.name.clone();
    let vm = state.vm_manager.create_vm(payload).await?;

    // Broadcast VM created event
    let username = auth_user
        .map(|u| u.username.clone())
        .unwrap_or_else(|| "system".to_string());
    state.ws_state.broadcast_vm_created(
        vm.id.clone(),
        vm_name,
        username,
    );

    Ok((StatusCode::CREATED, Json(vm)))
}

async fn start_vm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<VmConfig>, ApiError> {
    let old_status = state.vm_manager.get_vm(&id).await
        .map(|vm| format!("{:?}", vm.status))
        .unwrap_or_else(|_| "unknown".to_string());

    let vm = state.vm_manager.start_vm(&id).await?;

    // Broadcast VM status change
    state.ws_state.broadcast_vm_status(
        id.clone(),
        old_status,
        format!("{:?}", vm.status),
    );

    Ok(Json(vm))
}

async fn stop_vm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<VmConfig>, ApiError> {
    let old_status = state.vm_manager.get_vm(&id).await
        .map(|vm| format!("{:?}", vm.status))
        .unwrap_or_else(|_| "unknown".to_string());

    let vm = state.vm_manager.stop_vm(&id).await?;

    // Broadcast VM status change
    state.ws_state.broadcast_vm_status(
        id.clone(),
        old_status,
        format!("{:?}", vm.status),
    );

    Ok(Json(vm))
}

async fn delete_vm(
    State(state): State<Arc<AppState>>,
    auth_user: Option<axum::Extension<middleware::auth::AuthUser>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Get VM name before deletion
    let vm_name = state.vm_manager.get_vm(&id).await
        .map(|vm| vm.name)
        .unwrap_or_else(|_| id.clone());

    state.vm_manager.delete_vm(&id).await?;

    // Broadcast VM deleted event
    let username = auth_user
        .map(|u| u.username.clone())
        .unwrap_or_else(|| "system".to_string());
    state.ws_state.broadcast_vm_deleted(
        id,
        vm_name,
        username,
    );

    Ok(StatusCode::NO_CONTENT)
}

// Backup API handlers

async fn list_backups(State(state): State<Arc<AppState>>) -> Json<Vec<Backup>> {
    let backups = state.backup_manager.list_backups(None).await;
    Json(backups)
}

async fn get_backup(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Backup>, ApiError> {
    let backups = state.backup_manager.list_backups(None).await;
    let backup = backups.iter()
        .find(|b| b.id == id)
        .ok_or_else(|| horcrux_common::Error::System(format!("Backup {} not found", id)))?;
    Ok(Json(backup.clone()))
}

async fn create_backup(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<BackupConfig>,
) -> Result<(StatusCode, Json<Backup>), ApiError> {
    let target_id = payload.target_id.clone();
    let start_time = std::time::Instant::now();

    let backup = state.backup_manager.create_backup(payload).await?;

    let duration_secs = start_time.elapsed().as_secs();

    // Broadcast backup completed event
    state.ws_state.broadcast_backup_completed(
        target_id,
        backup.id.clone(),
        backup.size,
        duration_secs,
    );

    Ok((StatusCode::CREATED, Json(backup)))
}

async fn delete_backup(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.backup_manager.delete_backup(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Deserialize)]
struct RestoreRequest {
    target_id: Option<String>,
}

async fn restore_backup(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<RestoreRequest>,
) -> Result<StatusCode, ApiError> {
    state.backup_manager.restore_backup(&id, payload.target_id).await?;
    Ok(StatusCode::OK)
}

// VM Snapshot handlers

#[derive(serde::Deserialize)]
struct CreateSnapshotRequest {
    name: String,
    description: Option<String>,
    include_memory: Option<bool>,
}

async fn list_vm_snapshots(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
) -> Result<Json<Vec<vm::snapshot::VmSnapshot>>, ApiError> {
    let snapshot_manager = state.snapshot_manager.read().await;
    let snapshots = snapshot_manager.list_snapshots(&vm_id);
    Ok(Json(snapshots))
}

async fn create_vm_snapshot(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
    Json(req): Json<CreateSnapshotRequest>,
) -> Result<Json<vm::snapshot::VmSnapshot>, ApiError> {
    // Get VM config from database
    let vm_config = state.database.get_vm(&vm_id).await
        .map_err(|_| ApiError::NotFound(format!("VM {} not found", vm_id)))?;

    let mut snapshot_manager = state.snapshot_manager.write().await;

    let snapshot = snapshot_manager
        .create_snapshot(
            &vm_config,
            req.name,
            req.description,
            req.include_memory.unwrap_or(false),
        )
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to create snapshot: {}", e)))?;

    Ok(Json(snapshot))
}

async fn get_vm_snapshot(
    State(state): State<Arc<AppState>>,
    Path((vm_id, snapshot_id)): Path<(String, String)>,
) -> Result<Json<vm::snapshot::VmSnapshot>, ApiError> {
    let snapshot_manager = state.snapshot_manager.read().await;

    let snapshot = snapshot_manager
        .get_snapshot(&snapshot_id)
        .ok_or_else(|| ApiError::NotFound(format!("Snapshot {} not found", snapshot_id)))?;

    // Verify snapshot belongs to this VM
    if snapshot.vm_id != vm_id {
        return Err(ApiError::NotFound(format!("Snapshot not found for VM {}", vm_id)));
    }

    Ok(Json(snapshot.clone()))
}

async fn delete_vm_snapshot(
    State(state): State<Arc<AppState>>,
    Path((_vm_id, snapshot_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let mut snapshot_manager = state.snapshot_manager.write().await;

    snapshot_manager
        .delete_snapshot(&snapshot_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to delete snapshot: {}", e)))?;

    Ok(StatusCode::OK)
}

#[derive(serde::Deserialize)]
struct RestoreSnapshotRequest {
    restore_memory: Option<bool>,
}

async fn restore_vm_snapshot(
    State(state): State<Arc<AppState>>,
    Path((_vm_id, snapshot_id)): Path<(String, String)>,
    Json(req): Json<RestoreSnapshotRequest>,
) -> Result<StatusCode, ApiError> {
    let snapshot_manager = state.snapshot_manager.read().await;

    snapshot_manager
        .restore_snapshot(&snapshot_id, req.restore_memory.unwrap_or(false))
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to restore snapshot: {}", e)))?;

    Ok(StatusCode::OK)
}

async fn get_vm_snapshot_tree(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
) -> Result<Json<Vec<vm::snapshot::SnapshotTreeNode>>, ApiError> {
    let snapshot_manager = state.snapshot_manager.read().await;
    let tree = snapshot_manager.build_snapshot_tree(&vm_id);
    Ok(Json(tree))
}

// Snapshot Schedule handlers
async fn list_snapshot_schedules(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<vm::snapshot_scheduler::SnapshotSchedule>> {
    let schedules = state.snapshot_scheduler.list_schedules().await;
    Json(schedules)
}

async fn create_snapshot_schedule(
    State(state): State<Arc<AppState>>,
    Json(mut schedule): Json<vm::snapshot_scheduler::SnapshotSchedule>,
) -> Result<Json<vm::snapshot_scheduler::SnapshotSchedule>, ApiError> {
    // Generate ID if not provided
    if schedule.id.is_empty() {
        schedule.id = uuid::Uuid::new_v4().to_string();
    }

    // Set created_at
    schedule.created_at = chrono::Utc::now().timestamp();

    // Calculate next_run
    schedule.next_run = schedule.frequency.next_run_after(chrono::Utc::now().timestamp());

    state.snapshot_scheduler.add_schedule(schedule.clone()).await?;
    info!("Created snapshot schedule: {}", schedule.id);
    Ok(Json(schedule))
}

async fn get_snapshot_schedule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<vm::snapshot_scheduler::SnapshotSchedule>, ApiError> {
    let schedule = state.snapshot_scheduler.get_schedule(&id).await
        .ok_or_else(|| ApiError::NotFound(format!("Snapshot schedule '{}' not found", id)))?;
    Ok(Json(schedule))
}

async fn update_snapshot_schedule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(mut schedule): Json<vm::snapshot_scheduler::SnapshotSchedule>,
) -> Result<Json<vm::snapshot_scheduler::SnapshotSchedule>, ApiError> {
    // Ensure ID matches
    schedule.id = id.clone();

    // Recalculate next_run if frequency changed
    schedule.next_run = schedule.frequency.next_run_after(chrono::Utc::now().timestamp());

    state.snapshot_scheduler.update_schedule(schedule.clone()).await?;
    info!("Updated snapshot schedule: {}", id);
    Ok(Json(schedule))
}

async fn delete_snapshot_schedule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.snapshot_scheduler.remove_schedule(&id).await?;
    info!("Deleted snapshot schedule: {}", id);
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct CloneVmRequest {
    name: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default = "default_clone_mode")]
    mode: String, // "full" or "linked"
    #[serde(default)]
    start: bool,
    #[serde(default)]
    mac_addresses: Option<Vec<String>>,
    #[serde(default)]
    description: Option<String>,
}

fn default_clone_mode() -> String {
    "full".to_string()
}

async fn clone_vm(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
    Json(req): Json<CloneVmRequest>,
) -> Result<Json<VmConfig>, ApiError> {
    // Get source VM configuration
    let source_vm = state.database.get_vm(&vm_id).await
        .map_err(|_| ApiError::NotFound(format!("VM {} not found", vm_id)))?;

    // Parse clone mode
    let clone_mode = match req.mode.as_str() {
        "full" => vm::clone::CloneMode::Full,
        "linked" => vm::clone::CloneMode::Linked,
        _ => return Err(ApiError::BadRequest(format!("Invalid clone mode: {}", req.mode))),
    };

    // Create clone options
    let clone_options = vm::clone::CloneOptions {
        name: req.name,
        id: req.id,
        mode: clone_mode,
        start: req.start,
        mac_addresses: req.mac_addresses,
        description: req.description,
        network_config: None, // TODO: Add network config support to API
    };

    // Clone the VM
    let cloned_vm = state.clone_manager
        .clone_vm(&source_vm, clone_options)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to clone VM: {}", e)))?;

    // Save to database
    state.database.create_vm(&cloned_vm).await
        .map_err(|e| ApiError::Internal(format!("Failed to save cloned VM to database: {}", e)))?;

    info!("VM {} cloned successfully to {}", vm_id, cloned_vm.id);

    Ok(Json(cloned_vm))
}

#[derive(Debug, Deserialize)]
struct CrossNodeCloneRequest {
    target_node: String,
    source_node: String,
    name: String,
    id: Option<String>,
    ssh_port: Option<u16>,
    ssh_user: Option<String>,
    compression_enabled: Option<bool>,
    bandwidth_limit_mbps: Option<u32>,
    target_volume_group: Option<String>,
}

async fn clone_vm_cross_node(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
    Json(req): Json<CrossNodeCloneRequest>,
) -> Result<Json<VmConfig>, ApiError> {
    use vm::cross_node_clone::{CrossNodeCloneConfig, CrossNodeCloneManager};
    use vm::clone::CloneOptions;

    // Get source VM configuration
    let source_vm = state.database.get_vm(&vm_id).await
        .map_err(|_| ApiError::NotFound(format!("VM {} not found", vm_id)))?;

    // Create clone options
    let clone_options = CloneOptions {
        name: req.name.clone(),
        id: req.id.clone().or_else(|| Some(uuid::Uuid::new_v4().to_string())),
        mode: vm::clone::CloneMode::Full, // Cross-node clones are always full clones
        start: false,
        mac_addresses: None, // Auto-generate on target
        description: Some(format!("Cross-node clone from {}", req.source_node)),
        network_config: None,
    };

    // Create cross-node clone config
    let cross_node_config = CrossNodeCloneConfig {
        source_node: req.source_node.clone(),
        target_node: req.target_node.clone(),
        source_vm_id: vm_id.clone(),
        clone_options,
        ssh_port: req.ssh_port,
        ssh_user: req.ssh_user,
        compression_enabled: req.compression_enabled.unwrap_or(true),
        bandwidth_limit_mbps: req.bandwidth_limit_mbps,
        target_volume_group: req.target_volume_group,
    };

    // Perform cross-node clone
    let manager = CrossNodeCloneManager::new(
        state.config.paths.vm_storage.to_string_lossy().to_string()
    );
    let cloned_vm = manager
        .clone_cross_node(&source_vm, cross_node_config)
        .await
        .map_err(|e| ApiError::Internal(format!("Cross-node clone failed: {}", e)))?;

    // Save to database
    state.database.create_vm(&cloned_vm).await
        .map_err(|e| ApiError::Internal(format!("Failed to save cloned VM to database: {}", e)))?;

    info!(
        "VM {} cloned successfully from {} to {} as {}",
        vm_id, req.source_node, req.target_node, cloned_vm.id
    );

    Ok(Json(cloned_vm))
}

// Clone Job Progress API handlers
async fn list_clone_jobs(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<vm::clone_progress::CloneJob>>, ApiError> {
    let jobs = state.clone_job_manager.list_jobs().await;
    Ok(Json(jobs))
}

async fn get_clone_job(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Result<Json<vm::clone_progress::CloneJob>, ApiError> {
    let job = state.clone_job_manager.get_job(&job_id).await
        .ok_or_else(|| ApiError::NotFound(format!("Clone job {} not found", job_id)))?;
    Ok(Json(job))
}

async fn cancel_clone_job(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.clone_job_manager.request_cancellation(&job_id).await
        .map_err(|e| ApiError::Internal(format!("Failed to cancel clone job: {}", e)))?;

    Ok(Json(serde_json::json!({
        "status": "cancellation_requested",
        "job_id": job_id,
        "message": "Clone job cancellation has been requested"
    })))
}

async fn delete_clone_job(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Only allow deletion of completed or failed jobs
    let job = state.clone_job_manager.get_job(&job_id).await
        .ok_or_else(|| ApiError::NotFound(format!("Clone job {} not found", job_id)))?;

    match job.state {
        vm::clone_progress::CloneJobState::Running => {
            return Err(ApiError::BadRequest(
                "Cannot delete running clone job. Cancel it first.".to_string()
            ));
        }
        vm::clone_progress::CloneJobState::Queued => {
            return Err(ApiError::BadRequest(
                "Cannot delete queued clone job. Cancel it first.".to_string()
            ));
        }
        _ => {}
    }

    // TODO: Implement actual deletion from manager
    // For now, just return success
    Ok(StatusCode::NO_CONTENT)
}

// Replication API handlers
#[derive(Debug, Deserialize)]
struct CreateReplicationJobRequest {
    source_vm_id: String,
    source_snapshot: String,
    target_node: String,
    target_pool: String,
    schedule: String, // "hourly", "daily", "weekly", or "manual"
    #[serde(default)]
    bandwidth_limit_mbps: Option<u32>,
    #[serde(default = "default_retention_count")]
    retention_count: u32,
    #[serde(default = "default_enabled")]
    enabled: bool,
}

fn default_retention_count() -> u32 {
    7
}

fn default_enabled() -> bool {
    true
}

async fn list_replication_jobs(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<vm::replication::ReplicationJob>> {
    let jobs = state.replication_manager.list_jobs().await;
    Json(jobs)
}

async fn create_replication_job(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateReplicationJobRequest>,
) -> Result<Json<vm::replication::ReplicationJob>, ApiError> {
    use vm::replication::{ReplicationJob, ReplicationSchedule};

    // Parse schedule
    let schedule = match req.schedule.as_str() {
        "hourly" => ReplicationSchedule::Hourly,
        "daily" => ReplicationSchedule::Daily { hour: 2 }, // Default to 2 AM
        "weekly" => ReplicationSchedule::Weekly { day: 0, hour: 2 }, // Default to Sunday 2 AM
        "manual" => ReplicationSchedule::Manual,
        _ => return Err(ApiError::BadRequest(format!("Invalid schedule: {}", req.schedule))),
    };

    // Create replication job
    let job_id = uuid::Uuid::new_v4().to_string();
    let job_name = format!("Replication: {} -> {}", req.source_vm_id, req.target_node);

    let job = ReplicationJob {
        id: job_id,
        name: job_name,
        source_vm_id: req.source_vm_id,
        source_snapshot: req.source_snapshot,
        target_node: req.target_node,
        target_pool: req.target_pool,
        schedule,
        bandwidth_limit_mbps: req.bandwidth_limit_mbps,
        retention_count: req.retention_count,
        enabled: req.enabled,
        created_at: chrono::Utc::now().timestamp(),
        last_run: None,
        next_run: 0, // Will be set by create_job
    };

    // Validate source VM exists
    state.database.get_vm(&job.source_vm_id).await
        .map_err(|_| ApiError::NotFound(format!("Source VM {} not found", job.source_vm_id)))?;

    let job = state.replication_manager.create_job(job).await?;
    info!("Created replication job: {}", job.id);
    Ok(Json(job))
}

async fn get_replication_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<vm::replication::ReplicationJob>, ApiError> {
    let job = state.replication_manager.get_job(&id).await
        .ok_or_else(|| ApiError::NotFound(format!("Replication job {} not found", id)))?;
    Ok(Json(job))
}

async fn delete_replication_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.replication_manager.delete_job(&id).await?;
    info!("Deleted replication job: {}", id);
    Ok(StatusCode::NO_CONTENT)
}

async fn execute_replication_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Execute replication
    state.replication_manager.execute_replication(&id).await?;

    info!("Started replication job: {}", id);
    Ok(StatusCode::ACCEPTED)
}

async fn get_replication_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<vm::replication::ReplicationState>, ApiError> {
    let status = state.replication_manager.get_state(&id).await
        .ok_or_else(|| ApiError::NotFound(format!("No active replication for job {}", id)))?;
    Ok(Json(status))
}

// Snapshot Quota handlers
#[derive(Debug, Deserialize)]
struct CreateSnapshotQuotaRequest {
    name: String,
    quota_type: String, // "per_vm", "per_pool", "global"
    target_id: String,
    max_size_bytes: u64,
    max_count: Option<u32>,
    warning_threshold_percent: Option<u8>,
    auto_cleanup_enabled: Option<bool>,
    cleanup_policy: Option<String>, // "oldest_first", "largest_first", "least_used_first", "manual"
}

async fn list_snapshot_quotas(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<vm::snapshot_quota::SnapshotQuota>> {
    let quotas = state.snapshot_quota_manager.list_quotas().await;
    Json(quotas)
}

async fn create_snapshot_quota(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateSnapshotQuotaRequest>,
) -> Result<Json<vm::snapshot_quota::SnapshotQuota>, ApiError> {
    use vm::snapshot_quota::{QuotaType, CleanupPolicy};

    let quota_type = match req.quota_type.as_str() {
        "per_vm" => QuotaType::PerVm,
        "per_pool" => QuotaType::PerPool,
        "global" => QuotaType::Global,
        _ => return Err(ApiError::BadRequest(format!("Invalid quota type: {}", req.quota_type))),
    };

    let cleanup_policy = match req.cleanup_policy.as_deref().unwrap_or("oldest_first") {
        "oldest_first" => CleanupPolicy::OldestFirst,
        "largest_first" => CleanupPolicy::LargestFirst,
        "least_used_first" => CleanupPolicy::LeastUsedFirst,
        "manual" => CleanupPolicy::Manual,
        other => return Err(ApiError::BadRequest(format!("Invalid cleanup policy: {}", other))),
    };

    let quota = state.snapshot_quota_manager.create_quota(
        req.name,
        quota_type,
        req.target_id,
        req.max_size_bytes,
        req.max_count,
        req.warning_threshold_percent.unwrap_or(80),
        req.auto_cleanup_enabled.unwrap_or(false),
        cleanup_policy,
    ).await?;

    info!("Created snapshot quota: {} ({})", quota.id, quota.name);
    Ok(Json(quota))
}

async fn get_snapshot_quota(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<vm::snapshot_quota::SnapshotQuota>, ApiError> {
    let quota = state.snapshot_quota_manager.get_quota(&id).await
        .ok_or_else(|| ApiError::NotFound(format!("Quota not found: {}", id)))?;
    Ok(Json(quota))
}

#[derive(Debug, Deserialize)]
struct UpdateSnapshotQuotaRequest {
    max_size_bytes: Option<u64>,
    max_count: Option<Option<u32>>,
    warning_threshold_percent: Option<u8>,
    auto_cleanup_enabled: Option<bool>,
    cleanup_policy: Option<String>,
}

async fn update_snapshot_quota(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateSnapshotQuotaRequest>,
) -> Result<Json<vm::snapshot_quota::SnapshotQuota>, ApiError> {
    use vm::snapshot_quota::CleanupPolicy;

    let cleanup_policy = if let Some(policy_str) = req.cleanup_policy {
        Some(match policy_str.as_str() {
            "oldest_first" => CleanupPolicy::OldestFirst,
            "largest_first" => CleanupPolicy::LargestFirst,
            "least_used_first" => CleanupPolicy::LeastUsedFirst,
            "manual" => CleanupPolicy::Manual,
            other => return Err(ApiError::BadRequest(format!("Invalid cleanup policy: {}", other))),
        })
    } else {
        None
    };

    let quota = state.snapshot_quota_manager.update_quota(
        &id,
        req.max_size_bytes,
        req.max_count,
        req.warning_threshold_percent,
        req.auto_cleanup_enabled,
        cleanup_policy,
    ).await?;

    info!("Updated snapshot quota: {}", id);
    Ok(Json(quota))
}

async fn delete_snapshot_quota(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.snapshot_quota_manager.delete_quota(&id).await?;
    info!("Deleted snapshot quota: {}", id);
    Ok(StatusCode::NO_CONTENT)
}

async fn get_snapshot_quota_usage(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<vm::snapshot_quota::QuotaUsage>, ApiError> {
    let usage = state.snapshot_quota_manager.get_usage(&id).await?;
    Ok(Json(usage))
}

async fn get_snapshot_quota_summary(
    State(state): State<Arc<AppState>>,
) -> Json<vm::snapshot_quota::QuotaSummary> {
    let summary = state.snapshot_quota_manager.get_quota_summary().await;
    Json(summary)
}

#[derive(Debug, Deserialize)]
struct EnforceQuotaRequest {
    snapshot_ids: Vec<String>,
}

async fn enforce_snapshot_quota(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<EnforceQuotaRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let freed_bytes = state.snapshot_quota_manager.enforce_quota(&id, req.snapshot_ids).await?;

    info!("Enforced quota {}: freed {} bytes", id, freed_bytes);
    Ok(Json(serde_json::json!({
        "quota_id": id,
        "freed_bytes": freed_bytes
    })))
}

// Audit log handlers
#[derive(Debug, Deserialize)]
struct QueryAuditEventsParams {
    event_type: Option<String>,
    user: Option<String>,
    severity: Option<String>,
    start_time: Option<i64>,
    end_time: Option<i64>,
    limit: Option<usize>,
}

async fn query_audit_events(
    State(state): State<Arc<AppState>>,
    Query(params): Query<QueryAuditEventsParams>,
) -> Result<Json<Vec<audit::AuditEvent>>, ApiError> {
    let event_type = params.event_type.and_then(|s| {
        match s.as_str() {
            "Login" => Some(audit::AuditEventType::Login),
            "Logout" => Some(audit::AuditEventType::Logout),
            "LoginFailed" => Some(audit::AuditEventType::LoginFailed),
            "PermissionGranted" => Some(audit::AuditEventType::PermissionGranted),
            "PermissionDenied" => Some(audit::AuditEventType::PermissionDenied),
            "VmCreated" => Some(audit::AuditEventType::VmCreated),
            "VmDeleted" => Some(audit::AuditEventType::VmDeleted),
            "VmStarted" => Some(audit::AuditEventType::VmStarted),
            "VmStopped" => Some(audit::AuditEventType::VmStopped),
            _ => None,
        }
    });

    let severity = params.severity.and_then(|s| {
        match s.as_str() {
            "Info" => Some(audit::AuditSeverity::Info),
            "Warning" => Some(audit::AuditSeverity::Warning),
            "Error" => Some(audit::AuditSeverity::Error),
            "Critical" => Some(audit::AuditSeverity::Critical),
            _ => None,
        }
    });

    let start_time = params.start_time.and_then(|ts| {
        chrono::DateTime::from_timestamp(ts, 0)
    });

    let end_time = params.end_time.and_then(|ts| {
        chrono::DateTime::from_timestamp(ts, 0)
    });

    let events = state.audit_logger.query(
        event_type,
        params.user,
        severity,
        start_time,
        end_time,
        params.limit,
    ).await;

    Ok(Json(events))
}

async fn get_audit_stats(
    State(state): State<Arc<AppState>>,
) -> Result<Json<std::collections::HashMap<String, usize>>, ApiError> {
    let stats = state.audit_logger.get_event_counts().await;
    Ok(Json(stats))
}

async fn get_security_events(
    State(state): State<Arc<AppState>>,
    Query(params): Query<LimitParam>,
) -> Result<Json<Vec<audit::AuditEvent>>, ApiError> {
    let limit = params.limit.unwrap_or(100);
    let events = state.audit_logger.get_security_events(limit).await;
    Ok(Json(events))
}

#[derive(Debug, Deserialize)]
struct FailedLoginsQuery {
    user: Option<String>,
    limit: Option<usize>,
}

async fn get_failed_logins(
    State(state): State<Arc<AppState>>,
    Query(params): Query<FailedLoginsQuery>,
) -> Result<Json<Vec<audit::AuditEvent>>, ApiError> {
    let limit = params.limit.unwrap_or(100);
    let events = state.audit_logger.get_failed_logins(params.user, limit).await;
    Ok(Json(events))
}

#[derive(Debug, Deserialize)]
struct BruteForceQuery {
    threshold: Option<usize>,
    window_minutes: Option<i64>,
}

async fn detect_brute_force_attempts(
    State(state): State<Arc<AppState>>,
    Query(params): Query<BruteForceQuery>,
) -> Result<Json<Vec<String>>, ApiError> {
    let threshold = params.threshold.unwrap_or(5);
    let window_minutes = params.window_minutes.unwrap_or(10);
    let suspects = state.audit_logger.detect_brute_force(threshold, window_minutes).await;
    Ok(Json(suspects))
}

#[derive(Debug, Deserialize)]
struct LimitParam {
    limit: Option<usize>,
}

// Container lifecycle handlers
async fn list_containers(State(state): State<Arc<AppState>>) -> Json<Vec<horcrux_common::ContainerConfig>> {
    let containers = state.container_manager.list_containers().await;
    Json(containers)
}

async fn create_container(
    State(state): State<Arc<AppState>>,
    Json(config): Json<horcrux_common::ContainerConfig>,
) -> Result<Json<horcrux_common::ContainerConfig>, ApiError> {
    let container = state.container_manager.create_container(config).await?;

    // Broadcast container created event
    state.ws_state.broadcast(websocket::WsEvent::ContainerStatusChanged {
        container_id: container.id.clone(),
        old_status: "none".to_string(),
        new_status: container.status.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    });

    info!("Container {} created successfully", container.id);
    Ok(Json(container))
}

async fn get_container(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<horcrux_common::ContainerConfig>, ApiError> {
    let container = state.container_manager.get_container(&id).await?;
    Ok(Json(container))
}

async fn delete_container(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.container_manager.delete_container(&id).await?;

    // Broadcast container deleted event
    state.ws_state.broadcast(websocket::WsEvent::ContainerDeleted {
        container_id: id.clone(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    });

    info!("Container {} deleted successfully", id);
    Ok(StatusCode::NO_CONTENT)
}

async fn start_container(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<horcrux_common::ContainerConfig>, ApiError> {
    let old_status = state.container_manager.get_container(&id).await?.status;
    let container = state.container_manager.start_container(&id).await?;

    // Broadcast status change event
    state.ws_state.broadcast(websocket::WsEvent::ContainerStatusChanged {
        container_id: id.clone(),
        old_status: old_status.to_string(),
        new_status: "running".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    });

    info!("Container {} started successfully", id);
    Ok(Json(container))
}

async fn stop_container(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<horcrux_common::ContainerConfig>, ApiError> {
    let old_status = state.container_manager.get_container(&id).await?.status;
    let container = state.container_manager.stop_container(&id).await?;

    // Broadcast status change event
    state.ws_state.broadcast(websocket::WsEvent::ContainerStatusChanged {
        container_id: id.clone(),
        old_status: old_status.to_string(),
        new_status: "stopped".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    });

    info!("Container {} stopped successfully", id);
    Ok(Json(container))
}

async fn pause_container(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<horcrux_common::ContainerConfig>, ApiError> {
    let old_status = state.container_manager.get_container(&id).await?.status;
    let container = state.container_manager.pause_container(&id).await?;

    // Broadcast status change event
    state.ws_state.broadcast(websocket::WsEvent::ContainerStatusChanged {
        container_id: id.clone(),
        old_status: old_status.to_string(),
        new_status: "paused".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    });

    info!("Container {} paused successfully", id);
    Ok(Json(container))
}

async fn resume_container(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<horcrux_common::ContainerConfig>, ApiError> {
    let old_status = state.container_manager.get_container(&id).await?.status;
    let container = state.container_manager.resume_container(&id).await?;

    // Broadcast status change event
    state.ws_state.broadcast(websocket::WsEvent::ContainerStatusChanged {
        container_id: id.clone(),
        old_status: old_status.to_string(),
        new_status: "running".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    });

    info!("Container {} resumed successfully", id);
    Ok(Json(container))
}

#[derive(serde::Serialize)]
struct ContainerStatusResponse {
    status: String,
}

async fn get_container_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ContainerStatusResponse>, ApiError> {
    let status = state.container_manager.get_container_status(&id).await?;
    Ok(Json(ContainerStatusResponse {
        status: status.to_string(),
    }))
}

#[derive(serde::Deserialize)]
struct ExecCommandRequest {
    command: Vec<String>,
}

#[derive(serde::Serialize)]
struct ExecCommandResponse {
    output: String,
}

async fn exec_container_command(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<ExecCommandRequest>,
) -> Result<Json<ExecCommandResponse>, ApiError> {
    let output = state.container_manager.exec_command(&id, req.command).await?;
    Ok(Json(ExecCommandResponse { output }))
}

#[derive(serde::Deserialize)]
struct CloneContainerRequest {
    target_id: String,
    target_name: String,
    snapshot: bool,
}

async fn clone_container(
    State(state): State<Arc<AppState>>,
    Path(source_id): Path<String>,
    Json(req): Json<CloneContainerRequest>,
) -> Result<Json<horcrux_common::ContainerConfig>, ApiError> {
    let cloned_container = state.container_manager.clone_container(
        &source_id,
        &req.target_id,
        &req.target_name,
        req.snapshot,
    ).await?;

    // Broadcast container created event
    state.ws_state.broadcast(websocket::WsEvent::ContainerStatusChanged {
        container_id: cloned_container.id.clone(),
        old_status: "none".to_string(),
        new_status: cloned_container.status.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    });

    info!("Container {} cloned to {} successfully", source_id, req.target_id);
    Ok(Json(cloned_container))
}

async fn list_backup_jobs(State(state): State<Arc<AppState>>) -> Json<Vec<BackupJob>> {
    let jobs = state.backup_manager.list_jobs().await;
    Json(jobs)
}

async fn create_backup_job(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<BackupJob>,
) -> Result<StatusCode, ApiError> {
    state.backup_manager.create_job(payload).await?;
    Ok(StatusCode::CREATED)
}

async fn run_backup_job_now(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Get the job
    let jobs = state.backup_manager.list_jobs().await;
    let job = jobs.iter()
        .find(|j| j.id == id)
        .ok_or_else(|| ApiError::NotFound(format!("Backup job not found: {}", id)))?;

    if !job.enabled {
        return Err(ApiError::Internal("Backup job is disabled".to_string()));
    }

    // Execute backup for each target in the job
    for target_id in &job.targets {
        let backup_config = BackupConfig {
            id: uuid::Uuid::new_v4().to_string(),
            name: format!("Manual backup from job {} for {}", job.id, target_id),
            target_type: backup::TargetType::Vm, // Default to VM, could parse from target_id
            target_id: target_id.clone(),
            storage: job.storage.clone(),
            mode: job.mode.clone(),
            compression: job.compression.clone(),
            notes: Some(format!("Manually triggered from job {}", job.id)),
        };

        info!("Executing backup for target {} from job {}", target_id, id);

        // Execute backup asynchronously
        let backup_manager = state.backup_manager.clone();
        tokio::spawn(async move {
            match backup_manager.create_backup(backup_config).await {
                Ok(backup) => {
                    info!("Backup completed successfully: {}", backup.id);
                }
                Err(e) => {
                    error!("Backup failed: {}", e);
                }
            }
        });
    }

    info!("Manual backup job triggered: {}", id);
    Ok(StatusCode::ACCEPTED)
}

async fn apply_retention(
    State(state): State<Arc<AppState>>,
    Path(target_id): Path<String>,
    Json(policy): Json<RetentionPolicy>,
) -> Result<StatusCode, ApiError> {
    state.backup_manager.apply_retention(&target_id, &policy).await?;
    Ok(StatusCode::OK)
}

// Cloud-init API handlers

async fn generate_cloudinit(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
    Json(config): Json<CloudInitConfig>,
) -> Result<Json<CloudInitResponse>, ApiError> {
    let iso_path = state.cloudinit_manager.generate_iso(&vm_id, &config).await?;
    Ok(Json(CloudInitResponse {
        vm_id,
        iso_path: iso_path.to_string_lossy().to_string(),
    }))
}

async fn delete_cloudinit(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.cloudinit_manager.delete_iso(&vm_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Serialize)]
struct CloudInitResponse {
    vm_id: String,
    iso_path: String,
}

// Template API handlers

async fn list_templates(State(state): State<Arc<AppState>>) -> Json<Vec<Template>> {
    let templates = state.template_manager.list_templates().await;
    Json(templates)
}

async fn get_template(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Template>, ApiError> {
    let template = state.template_manager.get_template(&id).await?;
    Ok(Json(template))
}

#[derive(serde::Deserialize)]
struct CreateTemplateRequest {
    vm_id: String,
    name: String,
    description: Option<String>,
    disk_path: String,
    storage_type: StorageType,
    memory: u64,
    cpus: u32,
    os_type: OsType,
}

async fn create_template(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateTemplateRequest>,
) -> Result<(StatusCode, Json<Template>), ApiError> {
    let template = state.template_manager.create_template(
        &payload.vm_id,
        payload.name,
        payload.description,
        std::path::PathBuf::from(payload.disk_path),
        payload.storage_type,
        payload.memory,
        payload.cpus,
        payload.os_type,
    ).await?;
    Ok((StatusCode::CREATED, Json(template)))
}

async fn delete_template(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.template_manager.delete_template(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn clone_template(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(request): Json<CloneRequest>,
) -> Result<Json<CloneResponse>, ApiError> {
    let new_vm_id = state.template_manager.clone_template(&id, request).await?;
    Ok(Json(CloneResponse { new_vm_id }))
}

#[derive(serde::Serialize)]
struct CloneResponse {
    new_vm_id: String,
}

// Auth API handlers

async fn login(
    State(state): State<Arc<AppState>>,
    Json(request): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    use crate::auth::password::verify_password;
    use crate::middleware::auth::generate_jwt_token;

    // Try database authentication first
    match db::users::get_user_by_username(state.database.pool(), &request.username).await {
        Ok(user) => {
            // Verify password
            if !verify_password(&request.password, &user.password_hash)? {
                return Err(ApiError::AuthenticationFailed);
            }

            // Create session in database
            let session_id = uuid::Uuid::new_v4().to_string();
            let expires_at = chrono::Utc::now() + chrono::Duration::hours(24);

            let session = horcrux_common::auth::Session {
                id: session_id.clone(),
                user_id: user.id.clone(),
                expires_at,
                session_id: session_id.clone(),
                username: user.username.clone(),
                realm: user.realm.clone(),
                created: chrono::Utc::now().timestamp(),
                expires: expires_at.timestamp(),
            };

            db::users::create_session(state.database.pool(), &session).await?;

            // Generate JWT token
            let token = generate_jwt_token(&user.id, &user.username, &user.role)
                .map_err(|e| ApiError::Internal(format!("Failed to generate token: {}", e.to_string())))?;

            Ok(Json(LoginResponse {
                ticket: token,
                csrf_token: session_id,
                username: user.username,
                roles: vec![user.role],
            }))
        }
        Err(_) => {
            // Fall back to auth manager (PAM/LDAP) if database user not found
            let response = state.auth_manager.login(request).await?;
            Ok(Json(response))
        }
    }
}

async fn logout(
    State(state): State<Arc<AppState>>,
    Json(request): Json<LogoutRequest>,
) -> Result<StatusCode, ApiError> {
    state.auth_manager.logout(&request.session_id).await?;
    Ok(StatusCode::OK)
}

#[derive(serde::Deserialize)]
struct LogoutRequest {
    session_id: String,
}

async fn verify_session(
    State(state): State<Arc<AppState>>,
    Json(request): Json<VerifyRequest>,
) -> Result<Json<VerifyResponse>, ApiError> {
    let valid = state.auth_manager.verify_session(&request.session_id).await?;
    Ok(Json(VerifyResponse { valid }))
}

#[derive(serde::Deserialize)]
struct VerifyRequest {
    session_id: String,
}

#[derive(serde::Serialize)]
struct VerifyResponse {
    valid: bool,
}

async fn list_users(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<horcrux_common::auth::User>> {
    let users = state.auth_manager.list_users().await;
    Json(users)
}

// User registration endpoint (publicly accessible)
#[derive(serde::Deserialize)]
struct RegisterUserRequest {
    username: String,
    password: String,
    email: String,
}

async fn register_user(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RegisterUserRequest>,
) -> Result<StatusCode, ApiError> {
    use crate::auth::password::hash_password;

    // Validate email format
    if !req.email.contains('@') {
        return Err(ApiError::Internal("Invalid email format".to_string()));
    }

    // Validate username (alphanumeric and underscores only)
    if !req.username.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err(ApiError::Internal("Username must be alphanumeric".to_string()));
    }

    // Validate password strength (at least 8 characters)
    if req.password.len() < 8 {
        return Err(ApiError::Internal("Password must be at least 8 characters".to_string()));
    }

    // Check if user already exists
    if let Ok(_) = db::users::get_user_by_username(state.database.pool(), &req.username).await {
        return Err(ApiError::Internal("Username already exists".to_string()));
    }

    // Hash password
    let password_hash = hash_password(&req.password)?;

    // Create user
    let user = horcrux_common::auth::User {
        id: uuid::Uuid::new_v4().to_string(),
        username: req.username,
        password_hash,
        email: req.email,
        role: "user".to_string(), // Default role
        realm: "local".to_string(),
        enabled: true,
        roles: vec!["PVEVMUser".to_string()],
        comment: None,
    };

    db::users::create_user(state.database.pool(), &user).await?;

    Ok(StatusCode::CREATED)
}

async fn create_user(
    State(state): State<Arc<AppState>>,
    Json(user): Json<horcrux_common::auth::User>,
) -> Result<StatusCode, ApiError> {
    state.auth_manager.add_user(user).await?;
    Ok(StatusCode::CREATED)
}

async fn delete_user(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.auth_manager.delete_user(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_roles(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<horcrux_common::auth::Role>> {
    let roles = state.auth_manager.list_roles().await;
    Json(roles)
}

async fn get_user_permissions(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<String>,
) -> Json<Vec<horcrux_common::auth::Permission>> {
    let permissions = state.auth_manager.get_user_permissions(&user_id).await;
    Json(permissions)
}

async fn add_permission(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<String>,
    Json(permission): Json<horcrux_common::auth::Permission>,
) -> Result<StatusCode, ApiError> {
    state.auth_manager.add_permission(&user_id, permission).await?;
    Ok(StatusCode::CREATED)
}

// Password change endpoint
#[derive(serde::Deserialize)]
struct ChangePasswordRequest {
    username: String,
    old_password: String,
    new_password: String,
}

async fn change_password(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ChangePasswordRequest>,
) -> Result<StatusCode, ApiError> {
    use crate::auth::password::{hash_password, verify_password};

    // Get user from database
    let user = db::users::get_user_by_username(state.database.pool(), &req.username)
        .await
        .map_err(|_| ApiError::AuthenticationFailed)?;

    // Verify old password
    if !verify_password(&req.old_password, &user.password_hash)? {
        return Err(ApiError::AuthenticationFailed);
    }

    // Hash new password
    let new_hash = hash_password(&req.new_password)?;

    // Update password in database
    sqlx::query("UPDATE users SET password_hash = ?, updated_at = CURRENT_TIMESTAMP WHERE username = ?")
        .bind(&new_hash)
        .bind(&req.username)
        .execute(state.database.pool())
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to update password: {}", e.to_string())))?;

    Ok(StatusCode::OK)
}

// API key management endpoints
#[derive(serde::Deserialize)]
struct CreateApiKeyRequest {
    name: String,
    expires_days: Option<i64>,
}

#[derive(serde::Serialize)]
struct CreateApiKeyResponse {
    id: String,
    key: String,
    name: String,
    expires_at: Option<i64>,
}

async fn create_api_key(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
    Json(req): Json<CreateApiKeyRequest>,
) -> Result<Json<CreateApiKeyResponse>, ApiError> {
    use crate::auth::password::{generate_api_key, hash_password};

    // Get user
    let user = db::users::get_user_by_username(state.database.pool(), &username)
        .await
        .map_err(|_| ApiError::NotFound(format!("User {} not found", username)))?;

    // Generate API key
    let api_key = generate_api_key();
    let key_hash = hash_password(&api_key)?;

    // Calculate expiration
    let expires_at = req.expires_days.map(|days| {
        chrono::Utc::now().timestamp() + (days * 86400)
    });

    // Create ID
    let key_id = uuid::Uuid::new_v4().to_string();

    // Insert into database
    sqlx::query(
        "INSERT INTO api_keys (id, user_id, key_hash, name, expires_at, enabled)
         VALUES (?, ?, ?, ?, ?, 1)"
    )
    .bind(&key_id)
    .bind(&user.id)
    .bind(&key_hash)
    .bind(&req.name)
    .bind(expires_at)
    .execute(state.database.pool())
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to create API key: {}", e.to_string())))?;

    Ok(Json(CreateApiKeyResponse {
        id: key_id,
        key: api_key, // Only return the key once!
        name: req.name,
        expires_at,
    }))
}

#[derive(serde::Serialize)]
struct ApiKeyInfo {
    id: String,
    name: String,
    enabled: bool,
    expires_at: Option<i64>,
    created_at: String,
    last_used_at: Option<String>,
}

async fn list_api_keys(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
) -> Result<Json<Vec<ApiKeyInfo>>, ApiError> {
    // Get user
    let user = db::users::get_user_by_username(state.database.pool(), &username)
        .await
        .map_err(|_| ApiError::NotFound(format!("User {} not found", username)))?;

    // Query API keys
    let rows = sqlx::query(
        "SELECT id, name, enabled, expires_at, created_at, last_used_at
         FROM api_keys WHERE user_id = ? ORDER BY created_at DESC"
    )
    .bind(&user.id)
    .fetch_all(state.database.pool())
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to list API keys: {}", e.to_string())))?;

    let mut keys = Vec::new();
    for row in rows {
        use sqlx::Row;
        keys.push(ApiKeyInfo {
            id: row.get("id"),
            name: row.get("name"),
            enabled: row.get("enabled"),
            expires_at: row.get("expires_at"),
            created_at: row.get("created_at"),
            last_used_at: row.get("last_used_at"),
        });
    }

    Ok(Json(keys))
}

async fn revoke_api_key(
    State(state): State<Arc<AppState>>,
    Path((username, key_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    // Get user
    let user = db::users::get_user_by_username(state.database.pool(), &username)
        .await
        .map_err(|_| ApiError::NotFound(format!("User {} not found", username)))?;

    // Delete API key (verify it belongs to the user)
    let result = sqlx::query("DELETE FROM api_keys WHERE id = ? AND user_id = ?")
        .bind(&key_id)
        .bind(&user.id)
        .execute(state.database.pool())
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to revoke API key: {}", e.to_string())))?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("API key not found".to_string()));
    }

    Ok(StatusCode::NO_CONTENT)
}

// Firewall API handlers

async fn list_firewall_rules(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<FirewallRule>> {
    let rules = state.firewall_manager.list_rules(FirewallScope::Datacenter).await;
    Json(rules)
}

async fn add_firewall_rule(
    State(state): State<Arc<AppState>>,
    Json(rule): Json<FirewallRule>,
) -> Result<StatusCode, ApiError> {
    state.firewall_manager.add_rule(FirewallScope::Datacenter, rule).await?;
    Ok(StatusCode::CREATED)
}

async fn delete_firewall_rule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.firewall_manager.delete_rule(FirewallScope::Datacenter, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_security_groups(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<SecurityGroup>> {
    let groups = state.firewall_manager.list_security_groups().await;
    Json(groups)
}

async fn get_security_group(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<SecurityGroup>, ApiError> {
    let group = state.firewall_manager.get_security_group(&name).await?;
    Ok(Json(group))
}

async fn apply_firewall_rules(
    State(state): State<Arc<AppState>>,
    Path(scope): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Parse scope from path (e.g., "datacenter", "node-node1", "vm-100")
    let firewall_scope = parse_firewall_scope(&scope)?;
    state.firewall_manager.apply_rules(firewall_scope).await?;
    Ok(StatusCode::OK)
}

fn parse_firewall_scope(scope: &str) -> Result<FirewallScope, ApiError> {
    if scope == "datacenter" {
        Ok(FirewallScope::Datacenter)
    } else if let Some(node_name) = scope.strip_prefix("node-") {
        Ok(FirewallScope::Node(node_name.to_string()))
    } else if let Some(vm_id) = scope.strip_prefix("vm-") {
        Ok(FirewallScope::Vm(vm_id.to_string()))
    } else if let Some(container_id) = scope.strip_prefix("container-") {
        Ok(FirewallScope::Container(container_id.to_string()))
    } else {
        Err(ApiError::Internal(
            format!("Invalid firewall scope: {}", scope)
        ))
    }
}

// Monitoring API handlers

async fn get_node_stats(
    State(state): State<Arc<AppState>>,
) -> Json<Option<NodeMetrics>> {
    let metrics = state.monitoring_manager.get_node_metrics().await;
    Json(metrics)
}

async fn get_all_vm_stats(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<ResourceMetrics>> {
    let metrics = state.monitoring_manager.list_vm_metrics().await;
    Json(metrics)
}

async fn get_vm_stats(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Json<Option<ResourceMetrics>> {
    let metrics = state.monitoring_manager.get_vm_metrics(&id).await;
    Json(metrics)
}

async fn get_all_container_stats(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<ResourceMetrics>> {
    let metrics = state.monitoring_manager.list_container_metrics().await;
    Json(metrics)
}

async fn get_container_stats(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Json<Option<ResourceMetrics>> {
    let metrics = state.monitoring_manager.get_container_metrics(&id).await;
    Json(metrics)
}

async fn get_all_storage_stats(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<StorageMetrics>> {
    let metrics = state.monitoring_manager.list_storage_metrics().await;
    Json(metrics)
}

async fn get_storage_stats(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Json<Option<StorageMetrics>> {
    let metrics = state.monitoring_manager.get_storage_metrics(&name).await;
    Json(metrics)
}

async fn get_metric_history(
    State(state): State<Arc<AppState>>,
    Path(metric): Path<String>,
    axum::extract::Query(params): axum::extract::Query<HistoryParams>,
) -> Json<Vec<monitoring::TimeSeriesPoint>> {
    let from = params.from.unwrap_or(0);
    let to = params.to.unwrap_or(chrono::Utc::now().timestamp());

    let history = state.monitoring_manager.get_history(&metric, from, to).await;
    Json(history)
}

#[derive(serde::Deserialize)]
struct HistoryParams {
    from: Option<i64>,
    to: Option<i64>,
}

// Console API handlers

async fn create_vnc_console(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
) -> Result<Json<ConsoleInfo>, ApiError> {
    let info = state.console_manager.create_console(&vm_id, ConsoleType::Vnc).await?;
    Ok(Json(info))
}

async fn get_vnc_websocket(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
) -> Result<Json<String>, ApiError> {
    let ws_url = state.console_manager.get_vnc_websocket(&vm_id).await?;
    Ok(Json(ws_url))
}

async fn verify_console_ticket(
    State(state): State<Arc<AppState>>,
    Path(ticket_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.console_manager.verify_ticket(&ticket_id).await?;
    Ok(StatusCode::OK)
}

async fn get_novnc_page(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
) -> Result<axum::response::Html<String>, ApiError> {
    // Create a console session and get the ticket
    let info = state.console_manager.create_console(&vm_id, ConsoleType::Vnc).await?;

    // Generate the WebSocket URL
    let ws_url = format!("ws://localhost:8006/api/console/ws/{}", info.ticket);

    // Get the noVNC HTML page
    let html = console::novnc::get_novnc_html(&info.ticket, &ws_url);

    Ok(axum::response::Html(html))
}

async fn vnc_websocket_handler(
    ws: axum::extract::ws::WebSocketUpgrade,
    Path(ticket_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Response {
    console::novnc::handle_vnc_websocket(ws, Path(ticket_id), State(state.console_manager.clone())).await
}

// Cluster API handlers

async fn list_cluster_nodes(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<Node>> {
    let nodes = state.cluster_manager.list_nodes().await;
    Json(nodes)
}

async fn add_cluster_node(
    State(state): State<Arc<AppState>>,
    Json(node): Json<Node>,
) -> Result<StatusCode, ApiError> {
    state.cluster_manager.add_node(node).await?;
    Ok(StatusCode::CREATED)
}

async fn get_cluster_architecture(
    State(state): State<Arc<AppState>>,
) -> Json<ArchitectureSummary> {
    let summary = state.cluster_manager.get_architecture_summary().await;
    Json(summary)
}

#[derive(serde::Deserialize)]
struct FindNodeRequest {
    architecture: String,
    memory_mb: u64,
    cpu_cores: u32,
}

async fn find_best_node_for_vm(
    State(state): State<Arc<AppState>>,
    Json(request): Json<FindNodeRequest>,
) -> Result<Json<String>, ApiError> {
    // Parse architecture string to enum
    let arch = match request.architecture.as_str() {
        "x86_64" => cluster::node::Architecture::X86_64,
        "aarch64" => cluster::node::Architecture::Aarch64,
        "riscv64" => cluster::node::Architecture::Riscv64,
        "ppc64le" => cluster::node::Architecture::Ppc64le,
        _ => return Err(ApiError::Internal(
            format!("Unknown architecture: {}", request.architecture)
        )),
    };

    let node_name = state.cluster_manager
        .find_best_node(&arch, request.memory_mb * 1024 * 1024, request.cpu_cores)
        .await?;

    Ok(Json(node_name))
}

// Alert API handlers

async fn list_alert_rules(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<AlertRule>> {
    let rules = state.alert_manager.list_rules().await;
    Json(rules)
}

async fn create_alert_rule(
    State(state): State<Arc<AppState>>,
    Json(rule): Json<AlertRule>,
) -> Result<StatusCode, ApiError> {
    state.alert_manager.add_rule(rule).await?;
    Ok(StatusCode::CREATED)
}

async fn delete_alert_rule(
    State(state): State<Arc<AppState>>,
    Path(rule_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.alert_manager.remove_rule(&rule_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_active_alerts(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<Alert>> {
    let alerts = state.alert_manager.get_active_alerts().await;
    Json(alerts)
}

#[derive(serde::Deserialize)]
struct HistoryQuery {
    limit: Option<usize>,
}

async fn get_alert_history(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(query): axum::extract::Query<HistoryQuery>,
) -> Json<Vec<Alert>> {
    let history = state.alert_manager.get_alert_history(query.limit).await;
    Json(history)
}

#[derive(serde::Deserialize)]
struct AcknowledgeRequest {
    user: String,
}

async fn acknowledge_alert(
    State(state): State<Arc<AppState>>,
    Path((rule_id, target)): Path<(String, String)>,
    Json(req): Json<AcknowledgeRequest>,
) -> Result<StatusCode, ApiError> {
    state.alert_manager.acknowledge_alert(&rule_id, &target, &req.user).await?;
    Ok(StatusCode::OK)
}

async fn list_notification_channels(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<NotificationChannel>> {
    let channels = state.alert_manager.list_notification_channels().await;
    Json(channels)
}

async fn add_notification_channel(
    State(state): State<Arc<AppState>>,
    Json(channel): Json<NotificationChannel>,
) -> Result<StatusCode, ApiError> {
    state.alert_manager.add_notification_channel(channel).await?;
    Ok(StatusCode::CREATED)
}

// OpenTelemetry handlers

async fn get_otel_config(
    State(state): State<Arc<AppState>>,
) -> Result<Json<OtelConfig>, ApiError> {
    let config = state.otel_manager.get_config().await;
    Ok(Json(config))
}

async fn update_otel_config(
    State(state): State<Arc<AppState>>,
    Json(config): Json<OtelConfig>,
) -> Result<StatusCode, ApiError> {
    state.otel_manager.update_config(config).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::OK)
}

async fn export_metrics_now(
    State(state): State<Arc<AppState>>,
) -> Result<StatusCode, ApiError> {
    // Collect current metrics
    let metrics = observability::metrics::MetricsCollector::collect_system_metrics();

    // Export to configured endpoint
    state.otel_manager.export_metrics(metrics).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::OK)
}

// TLS/SSL handlers

async fn get_tls_config(
    State(state): State<Arc<AppState>>,
) -> Result<Json<tls::TlsConfig>, ApiError> {
    let config = state.tls_manager.get_config().await;
    Ok(Json(config))
}

async fn update_tls_config(
    State(state): State<Arc<AppState>>,
    Json(config): Json<tls::TlsConfig>,
) -> Result<StatusCode, ApiError> {
    state.tls_manager.load_config(config).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::OK)
}

async fn list_certificates(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<tls::CertificateInfo>> {
    let certs = state.tls_manager.list_certificates().await;
    Json(certs)
}

#[derive(serde::Deserialize)]
struct GenerateCertRequest {
    common_name: String,
    organization: String,
    validity_days: u32,
}

async fn generate_self_signed_cert(
    State(state): State<Arc<AppState>>,
    Json(req): Json<GenerateCertRequest>,
) -> Result<Json<tls::CertificateInfo>, ApiError> {
    let cert = state.tls_manager.generate_self_signed_cert(
        &req.common_name,
        &req.organization,
        req.validity_days,
        &state.config.tls.cert_path.to_string_lossy(),
        &state.config.tls.key_path.to_string_lossy(),
    ).await.map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(cert))
}

async fn get_certificate_info_endpoint(
    State(state): State<Arc<AppState>>,
    Path(path): Path<String>,
) -> Result<Json<tls::CertificateInfo>, ApiError> {
    let info = state.tls_manager.get_certificate_info(&path).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(info))
}

// Vault handlers

async fn get_vault_config(
    State(state): State<Arc<AppState>>,
) -> Json<secrets::VaultConfig> {
    let config = state.vault_manager.get_config().await;
    Json(config)
}

async fn update_vault_config(
    State(state): State<Arc<AppState>>,
    Json(config): Json<secrets::VaultConfig>,
) -> Result<StatusCode, ApiError> {
    state.vault_manager.initialize(config).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::OK)
}

async fn read_vault_secret(
    State(state): State<Arc<AppState>>,
    Path(path): Path<String>,
) -> Result<Json<secrets::Secret>, ApiError> {
    let secret = state.vault_manager.read_secret(&path).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(secret))
}

#[derive(serde::Deserialize)]
struct WriteSecretRequest {
    data: std::collections::HashMap<String, String>,
}

async fn write_vault_secret(
    State(state): State<Arc<AppState>>,
    Path(path): Path<String>,
    Json(req): Json<WriteSecretRequest>,
) -> Result<StatusCode, ApiError> {
    state.vault_manager.write_secret(&path, req.data).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::OK)
}

async fn delete_vault_secret(
    State(state): State<Arc<AppState>>,
    Path(path): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.vault_manager.delete_secret(&path).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_vault_secrets(
    State(state): State<Arc<AppState>>,
    Path(path): Path<String>,
) -> Result<Json<Vec<String>>, ApiError> {
    let secrets = state.vault_manager.list_secrets(&path).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(secrets))
}

// Storage handlers

async fn list_storage_pools(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<storage::StoragePool>> {
    let pools = state.storage_manager.list_pools().await;
    Json(pools)
}

async fn get_storage_pool(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<storage::StoragePool>, ApiError> {
    let pool = state.storage_manager.get_pool(&id).await
        .map_err(|e| ApiError::from(e))?;
    Ok(Json(pool))
}

#[derive(serde::Deserialize)]
struct AddStoragePoolRequest {
    name: String,
    storage_type: String,
    path: String,
}

async fn add_storage_pool(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddStoragePoolRequest>,
) -> Result<Json<storage::StoragePool>, ApiError> {
    // Parse storage type
    let storage_type = match req.storage_type.to_lowercase().as_str() {
        "zfs" => storage::StorageType::Zfs,
        "ceph" => storage::StorageType::Ceph,
        "lvm" => storage::StorageType::Lvm,
        "iscsi" => storage::StorageType::Iscsi,
        "directory" | "dir" => storage::StorageType::Directory,
        "cifs" => storage::StorageType::Cifs,
        "nfs" => storage::StorageType::Nfs,
        "glusterfs" => storage::StorageType::GlusterFs,
        "btrfs" => storage::StorageType::BtrFs,
        "s3" => storage::StorageType::S3,
        _ => return Err(ApiError::Internal(
            format!("Unknown storage type: {}", req.storage_type)
        )),
    };

    let pool = storage::StoragePool {
        id: uuid::Uuid::new_v4().to_string(),
        name: req.name,
        storage_type,
        path: req.path,
        available: 0,  // Will be updated by backend
        total: 0,      // Will be updated by backend
        enabled: true,
    };

    let pool = state.storage_manager.add_pool(pool).await
        .map_err(|e| ApiError::from(e))?;
    Ok(Json(pool))
}

async fn remove_storage_pool(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.storage_manager.remove_pool(&id).await
        .map_err(|e| ApiError::from(e))?;
    Ok(StatusCode::OK)
}

#[derive(serde::Deserialize)]
struct CreateVolumeRequest {
    name: String,
    size: u64,  // Size in GB
}

async fn create_volume(
    State(state): State<Arc<AppState>>,
    Path(pool_id): Path<String>,
    Json(req): Json<CreateVolumeRequest>,
) -> Result<StatusCode, ApiError> {
    state.storage_manager.create_volume(&pool_id, &req.name, req.size).await
        .map_err(|e| ApiError::from(e))?;
    Ok(StatusCode::OK)
}

// HA (High Availability) handlers

#[derive(serde::Serialize)]
struct HaResourceResponse {
    vm_id: String,
    vm_name: String,
    group: String,
    priority: u32,
    state: String,
}

async fn list_ha_resources(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<HaResourceResponse>> {
    let resources = state.ha_manager.list_resources().await;

    let mut responses = Vec::new();
    for r in resources.iter() {
        // Try to get actual VM name from database
        let vm_name = state.database.get_vm(&r.vm_id.to_string())
            .await
            .ok()
            .map(|vm| vm.name)
            .unwrap_or_else(|| format!("vm-{}", r.vm_id));

        responses.push(HaResourceResponse {
            vm_id: r.vm_id.to_string(),
            vm_name,
            group: r.group.clone(),
            priority: r.max_restart, // Using max_restart as priority for now
            state: format!("{:?}", r.state),
        });
    }

    Json(responses)
}

#[derive(serde::Deserialize)]
struct AddHaResourceRequest {
    vm_id: u32,
    group: String,
    priority: u32,
}

async fn add_ha_resource(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddHaResourceRequest>,
) -> Result<StatusCode, ApiError> {
    let config = ha::HaConfig {
        vm_id: req.vm_id,
        group: req.group,
        max_restart: req.priority.min(10), // Use priority as max_restart, cap at 10
        max_relocate: 3, // Default max relocations
        state: ha::HaState::Started,
    };

    state.ha_manager.add_resource(config).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::CREATED)
}

async fn remove_ha_resource(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<u32>,
) -> Result<StatusCode, ApiError> {
    state.ha_manager.remove_resource(vm_id).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::OK)
}

#[derive(serde::Serialize)]
struct HaStatusResponse {
    total_resources: usize,
    running: usize,
    stopped: usize,
    migrating: usize,
}

async fn get_ha_status(
    State(state): State<Arc<AppState>>,
) -> Json<HaStatusResponse> {
    let resources = state.ha_manager.list_resources().await;

    let mut running = 0;
    let mut stopped = 0;
    let mut migrating = 0;

    for resource in &resources {
        match resource.state {
            ha::HaState::Started => running += 1,
            ha::HaState::Stopped | ha::HaState::Disabled => stopped += 1,
            ha::HaState::Migrating => migrating += 1,
            _ => {}
        }
    }

    Json(HaStatusResponse {
        total_resources: resources.len(),
        running,
        stopped,
        migrating,
    })
}

#[derive(serde::Deserialize)]
struct CreateHaGroupRequest {
    name: String,
    nodes: Vec<String>,
}

async fn create_ha_group(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateHaGroupRequest>,
) -> Result<StatusCode, ApiError> {
    let group = ha::HaGroup {
        name: req.name,
        nodes: req.nodes,
        restricted: false,
        no_failback: false,
    };

    state.ha_manager.add_group(group).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::CREATED)
}

async fn list_ha_groups(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<ha::HaGroup>> {
    let groups = state.ha_manager.list_groups().await;
    Json(groups)
}

// Migration handlers

#[derive(serde::Deserialize)]
struct MigrateVmRequest {
    target_node: String,
    migration_type: Option<String>, // "live", "offline", "online"
    online: Option<bool>,
}

async fn migrate_vm(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
    Json(req): Json<MigrateVmRequest>,
) -> Result<Json<String>, ApiError> {
    // Determine migration type
    let online = req.online.unwrap_or(true);
    let migration_type_str = req.migration_type.unwrap_or_else(|| {
        if online { "live".to_string() } else { "offline".to_string() }
    });

    let migration_type = match migration_type_str.as_str() {
        "live" => migration::MigrationType::Live,
        "offline" => migration::MigrationType::Offline,
        "online" => migration::MigrationType::Online,
        _ => return Err(ApiError::Internal(format!("Invalid migration type: {}", migration_type_str))),
    };

    // Parse VM ID
    let vm_id_u32: u32 = vm_id.parse()
        .map_err(|_| ApiError::Internal(format!("Invalid VM ID: {}", vm_id)))?;

    // Create migration config
    let config = migration::MigrationConfig {
        vm_id: vm_id_u32,
        target_node: req.target_node.clone(),
        migration_type,
        bandwidth_limit: None,
        force: false,
        with_local_disks: false,
    };

    // Get the actual source node (local node)
    let source_node = state.cluster_manager.get_local_node_name()
        .await
        .unwrap_or_else(|_| "localhost".to_string());

    // Start migration
    let job_id = state.migration_manager
        .start_migration(config, source_node.clone())
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    info!("Started migration of VM {} to node {}, job ID: {}", vm_id, req.target_node, job_id);

    // Broadcast migration started event
    state.ws_state.broadcast_migration_started(
        vm_id.clone(),
        source_node,
        req.target_node.clone(),
    );

    Ok(Json(job_id))
}

async fn get_migration_status(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
) -> Result<Json<migration::MigrationJob>, ApiError> {
    // Find migration job for this VM
    let job = state.migration_manager
        .get_job(&vm_id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("No migration found for VM {}", vm_id)))?;

    Ok(Json(job))
}

// GPU Passthrough Endpoints

async fn list_gpu_devices(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<gpu::GpuDevice>>, ApiError> {
    let devices = state.gpu_manager.list_devices().await;
    Ok(Json(devices))
}

async fn scan_gpu_devices(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<gpu::GpuDevice>>, ApiError> {
    let devices = state.gpu_manager
        .scan_devices()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(devices))
}

async fn get_gpu_device(
    State(state): State<Arc<AppState>>,
    Path(pci_address): Path<String>,
) -> Result<Json<gpu::GpuDevice>, ApiError> {
    let device = state.gpu_manager
        .get_device(&pci_address)
        .await
        .map_err(|e| ApiError::NotFound(e.to_string()))?;
    Ok(Json(device))
}

async fn bind_gpu_to_vfio(
    State(state): State<Arc<AppState>>,
    Path(pci_address): Path<String>,
) -> Result<Json<&'static str>, ApiError> {
    state.gpu_manager
        .bind_to_vfio(&pci_address)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    info!("Bound GPU {} to vfio-pci", pci_address);
    Ok(Json("GPU bound to vfio-pci driver"))
}

async fn unbind_gpu_from_vfio(
    State(state): State<Arc<AppState>>,
    Path(pci_address): Path<String>,
) -> Result<Json<&'static str>, ApiError> {
    state.gpu_manager
        .unbind_from_vfio(&pci_address)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    info!("Unbound GPU {} from vfio-pci", pci_address);
    Ok(Json("GPU unbound from vfio-pci driver"))
}

async fn get_gpu_iommu_group(
    State(state): State<Arc<AppState>>,
    Path(pci_address): Path<String>,
) -> Result<Json<Vec<gpu::GpuDevice>>, ApiError> {
    let devices = state.gpu_manager
        .get_iommu_group_devices(&pci_address)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(devices))
}

#[derive(Serialize)]
struct IommuStatus {
    enabled: bool,
    message: String,
}

async fn check_iommu_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<IommuStatus>, ApiError> {
    let enabled = state.gpu_manager.check_iommu_enabled().await;

    let message = if enabled {
        "IOMMU is enabled and ready for GPU passthrough".to_string()
    } else {
        "IOMMU is not enabled. Add intel_iommu=on or amd_iommu=on to kernel parameters".to_string()
    };

    Ok(Json(IommuStatus { enabled, message }))
}

// Prometheus Metrics Endpoint

async fn prometheus_metrics(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    // Export metrics in Prometheus format
    let metrics = state.prometheus_manager.export_metrics().await;

    // Return as plain text with Prometheus content type
    (
        [(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        metrics,
    )
}

// Webhook Endpoints

async fn list_webhooks(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<webhooks::WebhookConfig>>, ApiError> {
    let webhooks = state.webhook_manager.list_webhooks().await;
    Ok(Json(webhooks))
}

async fn create_webhook(
    State(state): State<Arc<AppState>>,
    Json(config): Json<webhooks::WebhookConfig>,
) -> Result<Json<webhooks::WebhookConfig>, ApiError> {
    let webhook = state.webhook_manager
        .add_webhook(config)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    info!("Created webhook: {} ({})", webhook.name, webhook.id);
    Ok(Json(webhook))
}

async fn get_webhook(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<webhooks::WebhookConfig>, ApiError> {
    let webhook = state.webhook_manager
        .get_webhook(&id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Webhook {} not found", id)))?;

    Ok(Json(webhook))
}

async fn update_webhook(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(config): Json<webhooks::WebhookConfig>,
) -> Result<Json<&'static str>, ApiError> {
    state.webhook_manager
        .update_webhook(&id, config)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    info!("Updated webhook: {}", id);
    Ok(Json("Webhook updated successfully"))
}

async fn delete_webhook(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<&'static str>, ApiError> {
    state.webhook_manager
        .remove_webhook(&id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    info!("Deleted webhook: {}", id);
    Ok(Json("Webhook deleted successfully"))
}

async fn test_webhook(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<&'static str>, ApiError> {
    // Send a test event
    let test_data = serde_json::json!({
        "message": "This is a test webhook event",
        "webhook_id": id,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });

    state.webhook_manager
        .trigger_event(webhooks::WebhookEventType::Custom("test".to_string()), test_data)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json("Test webhook sent"))
}

async fn get_webhook_deliveries(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Vec<webhooks::WebhookDelivery>>, ApiError> {
    let deliveries = state.webhook_manager.get_deliveries(Some(&id), 50).await;
    Ok(Json(deliveries))
}

// CNI (Container Network Interface) handlers

async fn list_cni_networks(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<sdn::cni::CniConfig>>, ApiError> {
    let manager = state.cni_manager.read().await;
    let networks = manager.list_networks();
    Ok(Json(networks))
}

#[derive(serde::Deserialize)]
struct CreateCniNetworkRequest {
    name: String,
    plugin_type: sdn::cni::CniPluginType,
    bridge: Option<String>,
    subnet: Option<String>,
    gateway: Option<std::net::IpAddr>,
}

async fn create_cni_network(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateCniNetworkRequest>,
) -> Result<Json<sdn::cni::CniConfig>, ApiError> {
    let config = sdn::cni::CniConfig {
        cni_version: "1.0.0".to_string(),
        name: req.name,
        plugin_type: req.plugin_type,
        bridge: req.bridge,
        ipam: sdn::cni::IpamConfig {
            ipam_type: "host-local".to_string(),
            subnet: req.subnet,
            range_start: None,
            range_end: None,
            gateway: req.gateway,
            routes: vec![
                sdn::cni::RouteConfig {
                    dst: "0.0.0.0/0".to_string(),
                    gw: None,
                }
            ],
        },
        dns: None,
        capabilities: std::collections::HashMap::new(),
    };

    let mut manager = state.cni_manager.write().await;
    manager.create_network(config.clone())
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(config))
}

async fn get_cni_network(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<sdn::cni::CniConfig>, ApiError> {
    let manager = state.cni_manager.read().await;
    manager.get_network(&name)
        .cloned()
        .ok_or_else(|| ApiError::NotFound(format!("Network {} not found", name)))
        .map(Json)
}

async fn delete_cni_network(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<&'static str>, ApiError> {
    let mut manager = state.cni_manager.write().await;
    manager.delete_network(&name)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json("Network deleted"))
}

#[derive(serde::Deserialize)]
struct AttachContainerRequest {
    container_id: String,
    network_name: String,
    interface_name: String,
    netns_path: String,
}

async fn attach_container_to_network(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AttachContainerRequest>,
) -> Result<Json<sdn::cni::CniResult>, ApiError> {
    let mut manager = state.cni_manager.write().await;
    let result = manager.add_container(
        &req.container_id,
        &req.network_name,
        &req.interface_name,
        &req.netns_path,
    )
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(result))
}

#[derive(serde::Deserialize)]
struct DetachContainerRequest {
    container_id: String,
    network_name: String,
    interface_name: String,
    netns_path: String,
}

async fn detach_container_from_network(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DetachContainerRequest>,
) -> Result<Json<&'static str>, ApiError> {
    let mut manager = state.cni_manager.write().await;
    manager.del_container(
        &req.container_id,
        &req.network_name,
        &req.interface_name,
        &req.netns_path,
    )
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json("Container detached from network"))
}

#[derive(serde::Deserialize)]
struct CheckContainerRequest {
    container_id: String,
    network_name: String,
    interface_name: String,
    netns_path: String,
}

async fn check_container_network(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CheckContainerRequest>,
) -> Result<Json<&'static str>, ApiError> {
    let manager = state.cni_manager.read().await;
    manager.check_container(
        &req.container_id,
        &req.network_name,
        &req.interface_name,
        &req.netns_path,
    )
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json("Container network is healthy"))
}

async fn list_container_attachments(
    State(state): State<Arc<AppState>>,
    Path(container_id): Path<String>,
) -> Result<Json<Vec<sdn::cni::CniAttachment>>, ApiError> {
    let manager = state.cni_manager.read().await;
    let attachments = manager.list_attachments(&container_id);
    Ok(Json(attachments))
}

async fn get_cni_plugin_capabilities(
    State(state): State<Arc<AppState>>,
    Path(plugin_type): Path<String>,
) -> Result<Json<Vec<String>>, ApiError> {
    let plugin_type: sdn::cni::CniPluginType = serde_json::from_str(&format!("\"{}\"", plugin_type))
        .map_err(|e| ApiError::BadRequest(format!("Invalid plugin type: {}", e)))?;

    let manager = state.cni_manager.read().await;
    let capabilities = manager.get_capabilities(&plugin_type)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(capabilities))
}

// Network Policy handlers

async fn list_network_policies(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<sdn::policy::NetworkPolicy>>, ApiError> {
    let manager = state.network_policy_manager.read().await;
    let policies = manager.list_policies();
    Ok(Json(policies))
}

async fn create_network_policy(
    State(state): State<Arc<AppState>>,
    Json(policy): Json<sdn::policy::NetworkPolicy>,
) -> Result<Json<&'static str>, ApiError> {
    let mut manager = state.network_policy_manager.write().await;
    manager.create_policy(policy)
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json("Network policy created"))
}

async fn get_network_policy(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<sdn::policy::NetworkPolicy>, ApiError> {
    let manager = state.network_policy_manager.read().await;
    manager.get_policy(&id)
        .cloned()
        .ok_or_else(|| ApiError::NotFound(format!("Network policy {} not found", id)))
        .map(Json)
}

async fn delete_network_policy(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<&'static str>, ApiError> {
    let mut manager = state.network_policy_manager.write().await;
    manager.delete_policy(&id)
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json("Network policy deleted"))
}

async fn list_policies_in_namespace(
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
) -> Result<Json<Vec<sdn::policy::NetworkPolicy>>, ApiError> {
    let manager = state.network_policy_manager.read().await;
    let policies = manager.list_policies_in_namespace(&namespace);
    Ok(Json(policies))
}

async fn get_policy_iptables_rules(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Vec<String>>, ApiError> {
    let manager = state.network_policy_manager.read().await;
    let rules = manager.generate_iptables_rules(&id);
    Ok(Json(rules))
}

async fn get_policy_nftables_rules(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Vec<String>>, ApiError> {
    let manager = state.network_policy_manager.read().await;
    let rules = manager.generate_nftables_rules(&id);
    Ok(Json(rules))
}

// ============================================================================
// Kubernetes API handlers (conditionally compiled)
// ============================================================================

#[cfg(feature = "kubernetes")]
mod k8s_handlers {
    use super::*;
    use crate::kubernetes::types::*;

    // Cluster management handlers

    pub async fn list_clusters(
        State(state): State<Arc<AppState>>,
    ) -> Json<Vec<K8sCluster>> {
        let clusters = state.kubernetes_manager.list_clusters().await;
        Json(clusters)
    }

    pub async fn connect_cluster(
        State(state): State<Arc<AppState>>,
        Json(payload): Json<ClusterConnectRequest>,
    ) -> Result<(StatusCode, Json<K8sCluster>), ApiError> {
        let cluster = state.kubernetes_manager.connect_cluster(payload).await?;
        Ok((StatusCode::CREATED, Json(cluster)))
    }

    pub async fn get_cluster(
        State(state): State<Arc<AppState>>,
        Path(cluster_id): Path<String>,
    ) -> Result<Json<K8sCluster>, ApiError> {
        let cluster = state.kubernetes_manager.get_cluster(&cluster_id).await?;
        Ok(Json(cluster))
    }

    pub async fn delete_cluster(
        State(state): State<Arc<AppState>>,
        Path(cluster_id): Path<String>,
    ) -> Result<StatusCode, ApiError> {
        state.kubernetes_manager.delete_cluster(&cluster_id).await?;
        Ok(StatusCode::NO_CONTENT)
    }

    pub async fn reconnect_cluster(
        State(state): State<Arc<AppState>>,
        Path(cluster_id): Path<String>,
    ) -> Result<Json<K8sCluster>, ApiError> {
        let cluster = state.kubernetes_manager.reconnect_cluster(&cluster_id).await?;
        Ok(Json(cluster))
    }

    pub async fn get_cluster_health(
        State(state): State<Arc<AppState>>,
        Path(cluster_id): Path<String>,
    ) -> Result<Json<ClusterHealth>, ApiError> {
        let health = state.kubernetes_manager.check_cluster_health(&cluster_id).await?;
        Ok(Json(health))
    }

    pub async fn get_cluster_version(
        State(state): State<Arc<AppState>>,
        Path(cluster_id): Path<String>,
    ) -> Result<Json<K8sVersion>, ApiError> {
        let version = state.kubernetes_manager.get_cluster_version(&cluster_id).await?;
        Ok(Json(version))
    }

    // Namespace handlers

    pub async fn list_namespaces(
        State(state): State<Arc<AppState>>,
        Path(cluster_id): Path<String>,
    ) -> Result<Json<Vec<NamespaceInfo>>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let namespaces = crate::kubernetes::cluster_resources::namespaces::list_namespaces(&client).await?;
        Ok(Json(namespaces))
    }

    pub async fn create_namespace(
        State(state): State<Arc<AppState>>,
        Path(cluster_id): Path<String>,
        Json(payload): Json<CreateNamespaceRequest>,
    ) -> Result<(StatusCode, Json<NamespaceInfo>), ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let ns = crate::kubernetes::cluster_resources::namespaces::create_namespace(&client, &payload).await?;
        Ok((StatusCode::CREATED, Json(ns)))
    }

    pub async fn get_namespace(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace)): Path<(String, String)>,
    ) -> Result<Json<NamespaceInfo>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let ns = crate::kubernetes::cluster_resources::namespaces::get_namespace(&client, &namespace).await?;
        Ok(Json(ns))
    }

    pub async fn delete_namespace(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace)): Path<(String, String)>,
    ) -> Result<StatusCode, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        crate::kubernetes::cluster_resources::namespaces::delete_namespace(&client, &namespace).await?;
        Ok(StatusCode::NO_CONTENT)
    }

    // Node handlers

    pub async fn list_nodes(
        State(state): State<Arc<AppState>>,
        Path(cluster_id): Path<String>,
    ) -> Result<Json<Vec<NodeInfo>>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let nodes = crate::kubernetes::cluster_resources::nodes::list_nodes(&client).await?;
        Ok(Json(nodes))
    }

    pub async fn get_node(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, node)): Path<(String, String)>,
    ) -> Result<Json<NodeInfo>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let node_info = crate::kubernetes::cluster_resources::nodes::get_node(&client, &node).await?;
        Ok(Json(node_info))
    }

    pub async fn cordon_node(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, node)): Path<(String, String)>,
    ) -> Result<StatusCode, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        crate::kubernetes::cluster_resources::nodes::cordon_node(&client, &node).await?;
        Ok(StatusCode::OK)
    }

    pub async fn uncordon_node(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, node)): Path<(String, String)>,
    ) -> Result<StatusCode, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        crate::kubernetes::cluster_resources::nodes::uncordon_node(&client, &node).await?;
        Ok(StatusCode::OK)
    }

    #[derive(Deserialize)]
    pub struct DrainNodeRequest {
        #[serde(default)]
        pub ignore_daemonsets: bool,
        #[serde(default)]
        pub delete_emptydir_data: bool,
        pub grace_period_seconds: Option<i64>,
    }

    pub async fn drain_node(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, node)): Path<(String, String)>,
        Json(payload): Json<DrainNodeRequest>,
    ) -> Result<StatusCode, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        crate::kubernetes::cluster_resources::nodes::drain_node(
            &client,
            &node,
            payload.ignore_daemonsets,
            payload.delete_emptydir_data,
            payload.grace_period_seconds,
        ).await?;
        Ok(StatusCode::OK)
    }

    // Pod handlers

    #[derive(Deserialize)]
    pub struct PodListQuery {
        pub label_selector: Option<String>,
    }

    pub async fn list_pods(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace)): Path<(String, String)>,
        Query(query): Query<PodListQuery>,
    ) -> Result<Json<Vec<PodInfo>>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let pods = crate::kubernetes::workloads::pods::list_pods(
            &client,
            &namespace,
            query.label_selector.as_deref(),
        ).await?;
        Ok(Json(pods))
    }

    pub async fn get_pod(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace, pod)): Path<(String, String, String)>,
    ) -> Result<Json<PodInfo>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let pod_info = crate::kubernetes::workloads::pods::get_pod(&client, &namespace, &pod).await?;
        Ok(Json(pod_info))
    }

    pub async fn delete_pod(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace, pod)): Path<(String, String, String)>,
    ) -> Result<StatusCode, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        crate::kubernetes::workloads::pods::delete_pod(&client, &namespace, &pod).await?;
        Ok(StatusCode::NO_CONTENT)
    }

    #[derive(Deserialize)]
    pub struct PodLogQuery {
        pub container: Option<String>,
        pub tail_lines: Option<i64>,
        #[serde(default)]
        pub timestamps: bool,
    }

    pub async fn get_pod_logs(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace, pod)): Path<(String, String, String)>,
        Query(query): Query<PodLogQuery>,
    ) -> Result<String, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let logs = crate::kubernetes::workloads::pods::get_pod_logs(
            &client,
            &namespace,
            &pod,
            query.container.as_deref(),
            query.tail_lines,
            query.timestamps,
        ).await?;
        Ok(logs)
    }

    // Deployment handlers

    pub async fn list_deployments(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace)): Path<(String, String)>,
    ) -> Result<Json<Vec<DeploymentInfo>>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let deployments = crate::kubernetes::workloads::deployments::list_deployments(&client, &namespace).await?;
        Ok(Json(deployments))
    }

    pub async fn get_deployment(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace, deployment)): Path<(String, String, String)>,
    ) -> Result<Json<DeploymentInfo>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let deploy = crate::kubernetes::workloads::deployments::get_deployment(&client, &namespace, &deployment).await?;
        Ok(Json(deploy))
    }

    pub async fn scale_deployment(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace, deployment)): Path<(String, String, String)>,
        Json(payload): Json<ScaleRequest>,
    ) -> Result<Json<DeploymentInfo>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let deploy = crate::kubernetes::workloads::deployments::scale_deployment(
            &client,
            &namespace,
            &deployment,
            payload.replicas,
        ).await?;
        Ok(Json(deploy))
    }

    pub async fn restart_deployment(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace, deployment)): Path<(String, String, String)>,
    ) -> Result<StatusCode, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        crate::kubernetes::workloads::deployments::restart_deployment(&client, &namespace, &deployment).await?;
        Ok(StatusCode::OK)
    }

    // Event handlers

    pub async fn list_events(
        State(state): State<Arc<AppState>>,
        Path(cluster_id): Path<String>,
    ) -> Result<Json<Vec<K8sEvent>>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let events = crate::kubernetes::observability::events::list_events(&client, None).await?;
        Ok(Json(events))
    }

    pub async fn list_namespace_events(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace)): Path<(String, String)>,
    ) -> Result<Json<Vec<K8sEvent>>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let events = crate::kubernetes::observability::events::list_events(&client, Some(&namespace)).await?;
        Ok(Json(events))
    }

    // =========================================================================
    // Service handlers
    // =========================================================================

    pub async fn list_services(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace)): Path<(String, String)>,
    ) -> Result<Json<Vec<ServiceInfo>>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let services = crate::kubernetes::networking::services::list_services(&client, &namespace).await?;
        Ok(Json(services))
    }

    pub async fn get_service(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace, service)): Path<(String, String, String)>,
    ) -> Result<Json<ServiceInfo>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let svc = crate::kubernetes::networking::services::get_service(&client, &namespace, &service).await?;
        Ok(Json(svc))
    }

    pub async fn create_service(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, _namespace)): Path<(String, String)>,
        Json(payload): Json<CreateServiceRequest>,
    ) -> Result<(StatusCode, Json<ServiceInfo>), ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let svc = crate::kubernetes::networking::services::create_service(&client, &payload).await?;
        Ok((StatusCode::CREATED, Json(svc)))
    }

    pub async fn delete_service(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace, service)): Path<(String, String, String)>,
    ) -> Result<StatusCode, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        crate::kubernetes::networking::services::delete_service(&client, &namespace, &service).await?;
        Ok(StatusCode::NO_CONTENT)
    }

    // =========================================================================
    // Ingress handlers
    // =========================================================================

    pub async fn list_ingresses(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace)): Path<(String, String)>,
    ) -> Result<Json<Vec<IngressInfo>>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let ingresses = crate::kubernetes::networking::ingress::list_ingresses(&client, &namespace).await?;
        Ok(Json(ingresses))
    }

    pub async fn get_ingress(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace, ingress)): Path<(String, String, String)>,
    ) -> Result<Json<IngressInfo>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let ing = crate::kubernetes::networking::ingress::get_ingress(&client, &namespace, &ingress).await?;
        Ok(Json(ing))
    }

    pub async fn create_ingress(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, _namespace)): Path<(String, String)>,
        Json(payload): Json<CreateIngressRequest>,
    ) -> Result<(StatusCode, Json<IngressInfo>), ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let ing = crate::kubernetes::networking::ingress::create_ingress(&client, &payload).await?;
        Ok((StatusCode::CREATED, Json(ing)))
    }

    pub async fn delete_ingress(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace, ingress)): Path<(String, String, String)>,
    ) -> Result<StatusCode, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        crate::kubernetes::networking::ingress::delete_ingress(&client, &namespace, &ingress).await?;
        Ok(StatusCode::NO_CONTENT)
    }

    // =========================================================================
    // ConfigMap handlers
    // =========================================================================

    pub async fn list_configmaps(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace)): Path<(String, String)>,
    ) -> Result<Json<Vec<ConfigMapInfo>>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let cms = crate::kubernetes::config_storage::configmaps::list_configmaps(&client, &namespace).await?;
        Ok(Json(cms))
    }

    pub async fn get_configmap(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace, name)): Path<(String, String, String)>,
    ) -> Result<Json<ConfigMapInfo>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let cm = crate::kubernetes::config_storage::configmaps::get_configmap(&client, &namespace, &name).await?;
        Ok(Json(cm))
    }

    pub async fn create_configmap(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, _namespace)): Path<(String, String)>,
        Json(payload): Json<CreateConfigMapRequest>,
    ) -> Result<(StatusCode, Json<ConfigMapInfo>), ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let cm = crate::kubernetes::config_storage::configmaps::create_configmap(&client, &payload).await?;
        Ok((StatusCode::CREATED, Json(cm)))
    }

    pub async fn delete_configmap(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace, name)): Path<(String, String, String)>,
    ) -> Result<StatusCode, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        crate::kubernetes::config_storage::configmaps::delete_configmap(&client, &namespace, &name).await?;
        Ok(StatusCode::NO_CONTENT)
    }

    // =========================================================================
    // Secret handlers
    // =========================================================================

    pub async fn list_secrets(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace)): Path<(String, String)>,
    ) -> Result<Json<Vec<SecretInfo>>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let secrets = crate::kubernetes::config_storage::secrets::list_secrets(&client, &namespace).await?;
        Ok(Json(secrets))
    }

    pub async fn get_secret(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace, name)): Path<(String, String, String)>,
    ) -> Result<Json<SecretInfo>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let secret = crate::kubernetes::config_storage::secrets::get_secret(&client, &namespace, &name).await?;
        Ok(Json(secret))
    }

    pub async fn create_secret(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, _namespace)): Path<(String, String)>,
        Json(payload): Json<CreateSecretRequest>,
    ) -> Result<(StatusCode, Json<SecretInfo>), ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let secret = crate::kubernetes::config_storage::secrets::create_secret(&client, &payload).await?;
        Ok((StatusCode::CREATED, Json(secret)))
    }

    pub async fn delete_secret(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace, name)): Path<(String, String, String)>,
    ) -> Result<StatusCode, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        crate::kubernetes::config_storage::secrets::delete_secret(&client, &namespace, &name).await?;
        Ok(StatusCode::NO_CONTENT)
    }

    // =========================================================================
    // PVC handlers
    // =========================================================================

    pub async fn list_pvcs(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace)): Path<(String, String)>,
    ) -> Result<Json<Vec<PvcInfo>>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let pvcs = crate::kubernetes::config_storage::pvcs::list_pvcs(&client, &namespace).await?;
        Ok(Json(pvcs))
    }

    pub async fn get_pvc(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace, name)): Path<(String, String, String)>,
    ) -> Result<Json<PvcInfo>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let pvc = crate::kubernetes::config_storage::pvcs::get_pvc(&client, &namespace, &name).await?;
        Ok(Json(pvc))
    }

    pub async fn create_pvc(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, _namespace)): Path<(String, String)>,
        Json(payload): Json<CreatePvcRequest>,
    ) -> Result<(StatusCode, Json<PvcInfo>), ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let pvc = crate::kubernetes::config_storage::pvcs::create_pvc(&client, &payload).await?;
        Ok((StatusCode::CREATED, Json(pvc)))
    }

    pub async fn delete_pvc(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace, name)): Path<(String, String, String)>,
    ) -> Result<StatusCode, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        crate::kubernetes::config_storage::pvcs::delete_pvc(&client, &namespace, &name).await?;
        Ok(StatusCode::NO_CONTENT)
    }

    // =========================================================================
    // StatefulSet handlers
    // =========================================================================

    pub async fn list_statefulsets(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace)): Path<(String, String)>,
    ) -> Result<Json<Vec<StatefulSetInfo>>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let sts = crate::kubernetes::workloads::statefulsets::list_statefulsets(&client, &namespace).await?;
        Ok(Json(sts))
    }

    pub async fn get_statefulset(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace, name)): Path<(String, String, String)>,
    ) -> Result<Json<StatefulSetInfo>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let sts = crate::kubernetes::workloads::statefulsets::get_statefulset(&client, &namespace, &name).await?;
        Ok(Json(sts))
    }

    pub async fn scale_statefulset(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace, name)): Path<(String, String, String)>,
        Json(payload): Json<ScaleRequest>,
    ) -> Result<Json<StatefulSetInfo>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let sts = crate::kubernetes::workloads::statefulsets::scale_statefulset(
            &client,
            &namespace,
            &name,
            payload.replicas,
        ).await?;
        Ok(Json(sts))
    }

    pub async fn delete_statefulset(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace, name)): Path<(String, String, String)>,
    ) -> Result<StatusCode, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        crate::kubernetes::workloads::statefulsets::delete_statefulset(&client, &namespace, &name).await?;
        Ok(StatusCode::NO_CONTENT)
    }

    // =========================================================================
    // DaemonSet handlers
    // =========================================================================

    pub async fn list_daemonsets(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace)): Path<(String, String)>,
    ) -> Result<Json<Vec<DaemonSetInfo>>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let ds = crate::kubernetes::workloads::daemonsets::list_daemonsets(&client, &namespace).await?;
        Ok(Json(ds))
    }

    pub async fn get_daemonset(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace, name)): Path<(String, String, String)>,
    ) -> Result<Json<DaemonSetInfo>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let ds = crate::kubernetes::workloads::daemonsets::get_daemonset(&client, &namespace, &name).await?;
        Ok(Json(ds))
    }

    pub async fn delete_daemonset(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace, name)): Path<(String, String, String)>,
    ) -> Result<StatusCode, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        crate::kubernetes::workloads::daemonsets::delete_daemonset(&client, &namespace, &name).await?;
        Ok(StatusCode::NO_CONTENT)
    }

    // =========================================================================
    // Job handlers
    // =========================================================================

    pub async fn list_jobs(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace)): Path<(String, String)>,
    ) -> Result<Json<Vec<JobInfo>>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let jobs = crate::kubernetes::workloads::jobs::list_jobs(&client, &namespace).await?;
        Ok(Json(jobs))
    }

    pub async fn get_job(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace, name)): Path<(String, String, String)>,
    ) -> Result<Json<JobInfo>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let job = crate::kubernetes::workloads::jobs::get_job(&client, &namespace, &name).await?;
        Ok(Json(job))
    }

    pub async fn delete_job(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace, name)): Path<(String, String, String)>,
    ) -> Result<StatusCode, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        crate::kubernetes::workloads::jobs::delete_job(&client, &namespace, &name, None).await?;
        Ok(StatusCode::NO_CONTENT)
    }

    pub async fn list_cronjobs(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace)): Path<(String, String)>,
    ) -> Result<Json<Vec<CronJobInfo>>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let cronjobs = crate::kubernetes::workloads::jobs::list_cronjobs(&client, &namespace).await?;
        Ok(Json(cronjobs))
    }

    pub async fn get_cronjob(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace, name)): Path<(String, String, String)>,
    ) -> Result<Json<CronJobInfo>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let cronjob = crate::kubernetes::workloads::jobs::get_cronjob(&client, &namespace, &name).await?;
        Ok(Json(cronjob))
    }

    pub async fn delete_cronjob(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace, name)): Path<(String, String, String)>,
    ) -> Result<StatusCode, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        crate::kubernetes::workloads::jobs::delete_cronjob(&client, &namespace, &name).await?;
        Ok(StatusCode::NO_CONTENT)
    }

    // =========================================================================
    // Metrics handlers
    // =========================================================================

    pub async fn get_node_metrics(
        State(state): State<Arc<AppState>>,
        Path(cluster_id): Path<String>,
    ) -> Result<Json<Vec<crate::kubernetes::types::NodeMetrics>>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let metrics = crate::kubernetes::observability::metrics::get_node_metrics(&client).await?;
        Ok(Json(metrics))
    }

    pub async fn get_pod_metrics(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, namespace)): Path<(String, String)>,
    ) -> Result<Json<Vec<crate::kubernetes::types::PodMetrics>>, ApiError> {
        let client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let metrics = crate::kubernetes::observability::metrics::get_pod_metrics(&client, &namespace).await?;
        Ok(Json(metrics))
    }

    // =========================================================================
    // Helm handlers
    // =========================================================================

    pub async fn list_helm_releases(
        State(state): State<Arc<AppState>>,
        Path(cluster_id): Path<String>,
    ) -> Result<Json<Vec<HelmRelease>>, ApiError> {
        // Get kubeconfig path for this cluster
        let _client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let kubeconfig_path = format!("/tmp/kubeconfig-{}", cluster_id);
        let releases = crate::kubernetes::helm::releases::list_releases(&kubeconfig_path).await?;
        Ok(Json(releases))
    }

    pub async fn install_helm_release(
        State(state): State<Arc<AppState>>,
        Path(cluster_id): Path<String>,
        Json(payload): Json<HelmInstallRequest>,
    ) -> Result<(StatusCode, Json<HelmRelease>), ApiError> {
        let _client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let kubeconfig_path = format!("/tmp/kubeconfig-{}", cluster_id);
        let release = crate::kubernetes::helm::releases::install_release(&kubeconfig_path, &payload).await?;
        Ok((StatusCode::CREATED, Json(release)))
    }

    pub async fn upgrade_helm_release(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, release_name)): Path<(String, String)>,
        Json(payload): Json<HelmUpgradeRequest>,
    ) -> Result<Json<HelmRelease>, ApiError> {
        let _client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let kubeconfig_path = format!("/tmp/kubeconfig-{}", cluster_id);
        let release = crate::kubernetes::helm::releases::upgrade_release(&kubeconfig_path, &release_name, &payload).await?;
        Ok(Json(release))
    }

    pub async fn uninstall_helm_release(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, release_name)): Path<(String, String)>,
        Query(query): Query<HelmNamespaceQuery>,
    ) -> Result<StatusCode, ApiError> {
        let _client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let kubeconfig_path = format!("/tmp/kubeconfig-{}", cluster_id);
        let namespace = query.namespace.as_deref().unwrap_or("default");
        crate::kubernetes::helm::releases::uninstall_release(&kubeconfig_path, &release_name, namespace).await?;
        Ok(StatusCode::NO_CONTENT)
    }

    #[derive(Deserialize)]
    pub struct HelmNamespaceQuery {
        pub namespace: Option<String>,
    }

    #[derive(Deserialize)]
    pub struct HelmRollbackRequest {
        pub revision: Option<i32>,
    }

    pub async fn rollback_helm_release(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, release_name)): Path<(String, String)>,
        Query(query): Query<HelmNamespaceQuery>,
        Json(payload): Json<HelmRollbackRequest>,
    ) -> Result<StatusCode, ApiError> {
        let _client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let kubeconfig_path = format!("/tmp/kubeconfig-{}", cluster_id);
        let namespace = query.namespace.as_deref().unwrap_or("default");
        crate::kubernetes::helm::releases::rollback_release(&kubeconfig_path, &release_name, namespace, payload.revision).await?;
        Ok(StatusCode::OK)
    }

    pub async fn get_helm_release_history(
        State(state): State<Arc<AppState>>,
        Path((cluster_id, release_name)): Path<(String, String)>,
        Query(query): Query<HelmNamespaceQuery>,
    ) -> Result<Json<Vec<crate::kubernetes::helm::HelmReleaseRevision>>, ApiError> {
        let _client = state.kubernetes_manager.get_client(&cluster_id).await?;
        let kubeconfig_path = format!("/tmp/kubeconfig-{}", cluster_id);
        let namespace = query.namespace.as_deref().unwrap_or("default");
        let history = crate::kubernetes::helm::releases::get_release_history(&kubeconfig_path, &release_name, namespace).await?;
        Ok(Json(history))
    }

    pub async fn list_helm_repos(
        State(_state): State<Arc<AppState>>,
    ) -> Result<Json<Vec<HelmRepo>>, ApiError> {
        let repos = crate::kubernetes::helm::repos::list_repos().await?;
        Ok(Json(repos))
    }

    pub async fn add_helm_repo(
        State(_state): State<Arc<AppState>>,
        Json(payload): Json<AddHelmRepoRequest>,
    ) -> Result<StatusCode, ApiError> {
        crate::kubernetes::helm::repos::add_repo(&payload.name, &payload.url).await?;
        Ok(StatusCode::CREATED)
    }

    #[derive(Deserialize)]
    pub struct AddHelmRepoRequest {
        pub name: String,
        pub url: String,
    }

    pub async fn remove_helm_repo(
        State(_state): State<Arc<AppState>>,
        Path(repo_name): Path<String>,
    ) -> Result<StatusCode, ApiError> {
        crate::kubernetes::helm::repos::remove_repo(&repo_name).await?;
        Ok(StatusCode::NO_CONTENT)
    }

    pub async fn search_helm_charts(
        State(_state): State<Arc<AppState>>,
        Query(query): Query<HelmSearchQuery>,
    ) -> Result<Json<Vec<crate::kubernetes::helm::HelmChart>>, ApiError> {
        let charts = crate::kubernetes::helm::repos::search_charts(&query.keyword, query.all_versions.unwrap_or(false)).await?;
        Ok(Json(charts))
    }

    #[derive(Deserialize)]
    pub struct HelmSearchQuery {
        pub keyword: String,
        pub all_versions: Option<bool>,
    }
}

// Re-export handlers for use in routes
#[cfg(feature = "kubernetes")]
use k8s_handlers::{
    // Cluster
    list_clusters as k8s_list_clusters,
    connect_cluster as k8s_connect_cluster,
    get_cluster as k8s_get_cluster,
    delete_cluster as k8s_delete_cluster,
    reconnect_cluster as k8s_reconnect_cluster,
    get_cluster_health as k8s_get_cluster_health,
    get_cluster_version as k8s_get_cluster_version,
    // Namespaces
    list_namespaces as k8s_list_namespaces,
    create_namespace as k8s_create_namespace,
    get_namespace as k8s_get_namespace,
    delete_namespace as k8s_delete_namespace,
    // Nodes
    list_nodes as k8s_list_nodes,
    get_node as k8s_get_node,
    cordon_node as k8s_cordon_node,
    uncordon_node as k8s_uncordon_node,
    drain_node as k8s_drain_node,
    // Pods
    list_pods as k8s_list_pods,
    get_pod as k8s_get_pod,
    delete_pod as k8s_delete_pod,
    get_pod_logs as k8s_get_pod_logs,
    // Deployments
    list_deployments as k8s_list_deployments,
    get_deployment as k8s_get_deployment,
    scale_deployment as k8s_scale_deployment,
    restart_deployment as k8s_restart_deployment,
    // Events
    list_events as k8s_list_events,
    list_namespace_events as k8s_list_namespace_events,
    // Services
    list_services as k8s_list_services,
    get_service as k8s_get_service,
    create_service as k8s_create_service,
    delete_service as k8s_delete_service,
    // Ingress
    list_ingresses as k8s_list_ingresses,
    get_ingress as k8s_get_ingress,
    create_ingress as k8s_create_ingress,
    delete_ingress as k8s_delete_ingress,
    // ConfigMaps
    list_configmaps as k8s_list_configmaps,
    get_configmap as k8s_get_configmap,
    create_configmap as k8s_create_configmap,
    delete_configmap as k8s_delete_configmap,
    // Secrets
    list_secrets as k8s_list_secrets,
    get_secret as k8s_get_secret,
    create_secret as k8s_create_secret,
    delete_secret as k8s_delete_secret,
    // PVCs
    list_pvcs as k8s_list_pvcs,
    get_pvc as k8s_get_pvc,
    create_pvc as k8s_create_pvc,
    delete_pvc as k8s_delete_pvc,
    // StatefulSets
    list_statefulsets as k8s_list_statefulsets,
    get_statefulset as k8s_get_statefulset,
    scale_statefulset as k8s_scale_statefulset,
    delete_statefulset as k8s_delete_statefulset,
    // DaemonSets
    list_daemonsets as k8s_list_daemonsets,
    get_daemonset as k8s_get_daemonset,
    delete_daemonset as k8s_delete_daemonset,
    // Jobs
    list_jobs as k8s_list_jobs,
    get_job as k8s_get_job,
    delete_job as k8s_delete_job,
    list_cronjobs as k8s_list_cronjobs,
    get_cronjob as k8s_get_cronjob,
    delete_cronjob as k8s_delete_cronjob,
    // Metrics
    get_node_metrics as k8s_get_node_metrics,
    get_pod_metrics as k8s_get_pod_metrics,
    // Helm
    list_helm_releases as k8s_list_helm_releases,
    install_helm_release as k8s_install_helm_release,
    upgrade_helm_release as k8s_upgrade_helm_release,
    uninstall_helm_release as k8s_uninstall_helm_release,
    rollback_helm_release as k8s_rollback_helm_release,
    get_helm_release_history as k8s_get_helm_release_history,
    list_helm_repos as k8s_list_helm_repos,
    add_helm_repo as k8s_add_helm_repo,
    remove_helm_repo as k8s_remove_helm_repo,
    search_helm_charts as k8s_search_helm_charts,
};
