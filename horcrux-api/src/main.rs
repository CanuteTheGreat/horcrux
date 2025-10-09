mod vm;
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

use axum::{
    extract::{ConnectInfo, Path, State},
    http::StatusCode,
    middleware as axum_middleware,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use serde::Serialize;
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
use cluster::{ClusterManager, Node, ClusterStatus, ArchitectureSummary};
use alerts::{AlertManager, AlertRule, Alert, NotificationChannel};
use observability::{OtelManager, OtelConfig};
use tls::TlsManager;
use secrets::VaultManager;
use audit::AuditLogger;

#[derive(Clone)]
struct AppState {
    vm_manager: Arc<VmManager>,
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
    rate_limiter: Arc<middleware::rate_limit::RateLimiter>,
    storage_manager: Arc<storage::StorageManager>,
    ha_manager: Arc<ha::HaManager>,
    migration_manager: Arc<migration::MigrationManager>,
    gpu_manager: Arc<gpu::GpuManager>,
    prometheus_manager: Arc<prometheus::PrometheusManager>,
    webhook_manager: Arc<webhooks::WebhookManager>,
    cni_manager: Arc<tokio::sync::RwLock<sdn::cni::CniManager>>,
    network_policy_manager: Arc<tokio::sync::RwLock<sdn::policy::NetworkPolicyManager>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

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
        db::Database::new("sqlite:///var/lib/horcrux/horcrux.db")
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
    let cni_bin_dir = std::path::PathBuf::from("/opt/cni/bin");
    let cni_conf_dir = std::path::PathBuf::from("/etc/cni/net.d");
    let cni_manager = Arc::new(tokio::sync::RwLock::new(sdn::cni::CniManager::new(cni_bin_dir, cni_conf_dir)));

    // Create default CNI network if CNI directories exist
    if std::path::Path::new("/opt/cni/bin").exists() {
        match cni_manager.write().await.create_default_network().await {
            Ok(_) => info!("CNI default network created"),
            Err(e) => tracing::warn!("Failed to create default CNI network: {}", e),
        }
    } else {
        tracing::warn!("CNI binary directory not found at /opt/cni/bin - CNI features disabled");
    }

    // Initialize Network Policy manager
    let network_policy_manager = Arc::new(tokio::sync::RwLock::new(sdn::policy::NetworkPolicyManager::new()));
    info!("Network policy manager initialized");

    let state = Arc::new(AppState {
        vm_manager: Arc::new(VmManager::with_database(database.clone())),
        backup_manager: Arc::new(BackupManager::new()),
        cloudinit_manager: Arc::new(CloudInitManager::new(
            std::path::PathBuf::from("/var/lib/horcrux/cloudinit")
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
        audit_logger: Arc::new(AuditLogger::new(Some(std::path::PathBuf::from("/var/log/horcrux/audit.log")))),
        database,
        rate_limiter: rate_limiter.clone(),
        storage_manager: Arc::new(storage::StorageManager::new()),
        ha_manager: Arc::new(ha::HaManager::new()),
        migration_manager: Arc::new(migration::MigrationManager::new()),
        gpu_manager,
        prometheus_manager,
        webhook_manager,
        cni_manager,
        network_policy_manager,
    });

    // Static files serving for the frontend
    let serve_dir = ServeDir::new("horcrux-ui/dist")
        .not_found_service(ServeFile::new("horcrux-ui/dist/index.html"));

    // Build auth router with strict rate limiting
    let auth_router = Router::new()
        .route("/api/auth/login", post(login))
        .route("/api/auth/register", post(register_user))
        .with_state(state.clone())
        .layer(axum_middleware::from_fn(move |conn_info, req, next| {
            middleware::rate_limit::rate_limit_middleware(auth_rate_limiter.clone(), conn_info, req, next)
        }));

    // Build protected routes (require authentication)
    let protected_routes = Router::new()
        // VM endpoints
        .route("/api/vms", get(list_vms))
        .route("/api/vms", post(create_vm))
        .route("/api/vms/:id", get(get_vm))
        .route("/api/vms/:id/start", post(start_vm))
        .route("/api/vms/:id/stop", post(stop_vm))
        .route("/api/vms/:id", delete(delete_vm))
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
        // Auth endpoints (login and register are in auth_router with stricter rate limiting)
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/verify", get(verify_session))
        .route("/api/auth/password", post(change_password))
        .route("/api/users", get(list_users))
        .route("/api/users", post(create_user))
        .route("/api/users/:id", delete(delete_user))
        .route("/api/users/:username/api-keys", get(list_api_keys))
        .route("/api/users/:username/api-keys", post(create_api_key))
        .route("/api/users/:username/api-keys/:key_id", delete(revoke_api_key))
        .route("/api/roles", get(list_roles))
        .route("/api/permissions/:user_id", get(get_user_permissions))
        .route("/api/permissions/:user_id", post(add_permission))
        // Storage endpoints
        .route("/api/storage/pools", get(list_storage_pools))
        .route("/api/storage/pools/:id", get(get_storage_pool))
        .route("/api/storage/pools", post(add_storage_pool))
        .route("/api/storage/pools/:id", delete(remove_storage_pool))
        .route("/api/storage/pools/:pool_id/volumes", post(create_volume))
        // Firewall endpoints
        .route("/api/firewall/rules", get(list_firewall_rules))
        .route("/api/firewall/rules", post(add_firewall_rule))
        .route("/api/firewall/rules/:id", delete(delete_firewall_rule))
        .route("/api/firewall/security-groups", get(list_security_groups))
        .route("/api/firewall/security-groups/:name", get(get_security_group))
        .route("/api/firewall/:scope/apply", post(apply_firewall_rules))
        // Monitoring endpoints
        .route("/api/monitoring/node", get(get_node_stats))
        .route("/api/monitoring/vms", get(get_all_vm_stats))
        .route("/api/monitoring/vms/:id", get(get_vm_stats))
        .route("/api/monitoring/containers", get(get_all_container_stats))
        .route("/api/monitoring/containers/:id", get(get_container_stats))
        .route("/api/monitoring/storage", get(get_all_storage_stats))
        .route("/api/monitoring/storage/:name", get(get_storage_stats))
        .route("/api/monitoring/history/:metric", get(get_metric_history))
        // Console access
        .route("/api/console/:vm_id/vnc", post(create_vnc_console))
        .route("/api/console/:vm_id/websocket", get(get_vnc_websocket))
        .route("/api/console/ticket/:ticket_id", get(verify_console_ticket))
        // Cluster management
        .route("/api/cluster/nodes", get(list_cluster_nodes))
        .route("/api/cluster/nodes/:name", post(add_cluster_node))
        .route("/api/cluster/architecture", get(get_cluster_architecture))
        .route("/api/cluster/find-node", post(find_best_node_for_vm))
        // Alert system
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
        // GPU passthrough endpoints
        .route("/api/gpu/devices", get(list_gpu_devices))
        .route("/api/gpu/devices/scan", post(scan_gpu_devices))
        .route("/api/gpu/devices/:pci_address", get(get_gpu_device))
        .route("/api/gpu/devices/:pci_address/bind-vfio", post(bind_gpu_to_vfio))
        .route("/api/gpu/devices/:pci_address/unbind-vfio", post(unbind_gpu_from_vfio))
        .route("/api/gpu/devices/:pci_address/iommu-group", get(get_gpu_iommu_group))
        .route("/api/gpu/iommu-status", get(check_iommu_status))
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
        .route("/api/vault/secrets/:path", get(list_vault_secrets))
        // Audit log endpoints
        .route("/api/audit/events", get(query_audit_events))
        .route("/api/audit/failed-logins", get(get_failed_logins_endpoint))
        .route("/api/audit/security-events", get(get_security_events_endpoint))
        .route("/api/audit/export", post(export_audit_logs))
        // HA (High Availability) endpoints
        .route("/api/ha/resources", get(list_ha_resources))
        .route("/api/ha/resources", post(add_ha_resource))
        .route("/api/ha/resources/:vm_id", delete(remove_ha_resource))
        .route("/api/ha/status", get(get_ha_status))
        .route("/api/ha/groups", post(create_ha_group))
        .route("/api/ha/groups", get(list_ha_groups))
        // Migration endpoints
        .route("/api/migrate/:vm_id", post(migrate_vm))
        .route("/api/migrate/:vm_id/status", get(get_migration_status))
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
        .with_state(state.clone())
        // Add authentication middleware to protected routes
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            middleware::auth::auth_middleware,
        ));

    // Build main app with public and protected routes
    let app = Router::new()
        .route("/api/health", get(health_check))
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

    // Start server
    let addr = "0.0.0.0:8006"; // Using Proxmox's default port
    info!("Horcrux API listening on {}", addr);

    let listener = TcpListener::bind(addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}

// API error handling
enum ApiError {
    Internal(String),
    NotFound(String),
    AuthenticationFailed,
    BadRequest(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::Internal(msg) => {
                error!("API error: {}", msg);
                (StatusCode::INTERNAL_SERVER_ERROR, msg)
            }
            ApiError::NotFound(msg) => {
                (StatusCode::NOT_FOUND, msg)
            }
            ApiError::AuthenticationFailed => {
                (StatusCode::UNAUTHORIZED, "Authentication failed".to_string())
            }
            ApiError::BadRequest(msg) => {
                (StatusCode::BAD_REQUEST, msg)
            }
        };

        (status, message).into_response()
    }
}

impl From<horcrux_common::Error> for ApiError {
    fn from(err: horcrux_common::Error) -> Self {
        match err {
            horcrux_common::Error::VmNotFound(id) => {
                ApiError::NotFound(format!("VM not found: {}", id))
            }
            horcrux_common::Error::ContainerNotFound(id) => {
                ApiError::NotFound(format!("Container not found: {}", id))
            }
            horcrux_common::Error::AuthenticationFailed => {
                ApiError::AuthenticationFailed
            }
            _ => ApiError::Internal(err.to_string()),
        }
    }
}

// API handlers

async fn health_check() -> &'static str {
    "OK"
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
    Json(payload): Json<VmConfig>,
) -> Result<(StatusCode, Json<VmConfig>), ApiError> {
    let vm = state.vm_manager.create_vm(payload).await?;
    Ok((StatusCode::CREATED, Json(vm)))
}

async fn start_vm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<VmConfig>, ApiError> {
    let vm = state.vm_manager.start_vm(&id).await?;
    Ok(Json(vm))
}

async fn stop_vm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<VmConfig>, ApiError> {
    let vm = state.vm_manager.stop_vm(&id).await?;
    Ok(Json(vm))
}

async fn delete_vm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.vm_manager.delete_vm(&id).await?;
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
    let backup = state.backup_manager.create_backup(payload).await?;
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
        "/etc/horcrux/ssl/cert.pem",
        "/etc/horcrux/ssl/key.pem",
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

// Audit log handlers

#[derive(serde::Deserialize)]
struct AuditQueryParams {
    event_type: Option<String>,
    user: Option<String>,
    severity: Option<String>,
    limit: Option<usize>,
}

async fn query_audit_events(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<AuditQueryParams>,
) -> Json<Vec<audit::AuditEvent>> {
    // Parse event type
    let event_type = params.event_type.as_ref().and_then(|s| {
        match s.as_str() {
            "Login" => Some(audit::AuditEventType::Login),
            "Logout" => Some(audit::AuditEventType::Logout),
            "LoginFailed" => Some(audit::AuditEventType::LoginFailed),
            _ => None,
        }
    });

    // Parse severity
    let severity = params.severity.as_ref().and_then(|s| {
        match s.as_str() {
            "Info" => Some(audit::AuditSeverity::Info),
            "Warning" => Some(audit::AuditSeverity::Warning),
            "Error" => Some(audit::AuditSeverity::Error),
            "Critical" => Some(audit::AuditSeverity::Critical),
            _ => None,
        }
    });

    let events = state.audit_logger.query(
        event_type,
        params.user,
        severity,
        None,
        None,
        params.limit,
    ).await;

    Json(events)
}

async fn get_failed_logins_endpoint(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<FailedLoginsParams>,
) -> Json<Vec<audit::AuditEvent>> {
    let events = state.audit_logger.get_failed_logins(
        params.user,
        params.limit.unwrap_or(20),
    ).await;
    Json(events)
}

#[derive(serde::Deserialize)]
struct FailedLoginsParams {
    user: Option<String>,
    limit: Option<usize>,
}

async fn get_security_events_endpoint(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<SecurityEventsParams>,
) -> Json<Vec<audit::AuditEvent>> {
    let events = state.audit_logger.get_security_events(
        params.limit.unwrap_or(20),
    ).await;
    Json(events)
}

#[derive(serde::Deserialize)]
struct SecurityEventsParams {
    limit: Option<usize>,
}

#[derive(serde::Deserialize)]
struct ExportRequest {
    path: String,
}

async fn export_audit_logs(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ExportRequest>,
) -> Result<StatusCode, ApiError> {
    state.audit_logger.export(&req.path).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::OK)
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
        .start_migration(config, source_node)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    info!("Started migration of VM {} to node {}, job ID: {}", vm_id, req.target_node, job_id);

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
