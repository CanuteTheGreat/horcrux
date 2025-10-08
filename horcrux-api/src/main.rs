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

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use horcrux_common::VmConfig;
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

    let monitoring_manager = Arc::new(MonitoringManager::new());

    // Start background metrics collection
    monitoring_manager.start_collection().await;
    info!("Monitoring system started");

    let state = Arc::new(AppState {
        vm_manager: Arc::new(VmManager::new()),
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
    });

    // Build router
    let app = Router::new()
        .route("/api/health", get(health_check))
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
        // Auth endpoints
        .route("/api/auth/login", post(login))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/verify", get(verify_session))
        .route("/api/users", get(list_users))
        .route("/api/users", post(create_user))
        .route("/api/users/:id", delete(delete_user))
        .route("/api/roles", get(list_roles))
        .route("/api/permissions/:user_id", get(get_user_permissions))
        .route("/api/permissions/:user_id", post(add_permission))
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
        .with_state(state);

    // Start server
    let addr = "0.0.0.0:8006"; // Using Proxmox's default port
    info!("Horcrux API listening on {}", addr);

    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

// API error handling
enum ApiError {
    Internal(horcrux_common::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::Internal(e) => {
                error!("API error: {}", e);
                match e {
                    horcrux_common::Error::VmNotFound(id) => {
                        (StatusCode::NOT_FOUND, format!("VM not found: {}", id))
                    }
                    horcrux_common::Error::InvalidConfig(msg) => {
                        (StatusCode::BAD_REQUEST, msg)
                    }
                    _ => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
                }
            }
        };

        (status, message).into_response()
    }
}

impl From<horcrux_common::Error> for ApiError {
    fn from(err: horcrux_common::Error) -> Self {
        ApiError::Internal(err)
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
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // TODO: Trigger job execution immediately
    info!("Manual backup job trigger: {}", id);
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
    let response = state.auth_manager.login(request).await?;
    Ok(Json(response))
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
        Err(ApiError::Internal(horcrux_common::Error::InvalidConfig(
            format!("Invalid firewall scope: {}", scope)
        )))
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
        _ => return Err(ApiError::Internal(horcrux_common::Error::InvalidConfig(
            format!("Unknown architecture: {}", request.architecture)
        ))),
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
        .map_err(|e| ApiError::Internal(horcrux_common::Error::System(e)))?;
    Ok(StatusCode::OK)
}

async fn export_metrics_now(
    State(state): State<Arc<AppState>>,
) -> Result<StatusCode, ApiError> {
    // Collect current metrics
    let metrics = observability::metrics::MetricsCollector::collect_system_metrics();

    // Export to configured endpoint
    state.otel_manager.export_metrics(metrics).await
        .map_err(|e| ApiError::Internal(horcrux_common::Error::System(e)))?;

    Ok(StatusCode::OK)
}
