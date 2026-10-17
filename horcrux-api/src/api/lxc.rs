//! LXC parity API (Proxmox `pct`/`pveam` equivalents): template catalog and
//! cache, clean shutdown/reboot, settings (`pct set`), disk resize and
//! snapshots for LXC containers. Create/start/stop/delete/clone/exec use the
//! generic /api/containers routes with `runtime: "lxc"`.
use crate::container::lxc::{LxcConfigView, LxcHostInfo, DEFAULT_SHUTDOWN_TIMEOUT};
use crate::error::ApiError;
use crate::websocket::WsEvent;
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use horcrux_common::{ContainerConfig, LxcCatalogEntry, LxcOptions, LxcSnapshot, LxcTemplate};
use serde::Deserialize;
use std::sync::Arc;

pub fn lxc_routes() -> Router<Arc<AppState>> {
    let r = Router::new();
    // Web terminal + console log for LXC. With docker/podman compiled in
    // these are already served by the OCI route group.
    #[cfg(not(any(feature = "docker", feature = "podman")))]
    let r = r
        .merge(crate::container_ws_ticket_routes())
        .route(
            "/api/containers/ws/logs/:ticket_id",
            get(crate::container_logs_websocket_handler),
        )
        .route(
            "/api/containers/ws/exec/:ticket_id",
            get(crate::container_exec_websocket_handler),
        );
    r.route("/api/lxc/host", get(host_info))
        .route("/api/lxc/templates/catalog", get(template_catalog))
        .route(
            "/api/lxc/templates",
            get(list_templates).post(download_template),
        )
        .route(
            "/api/lxc/templates/*id",
            axum::routing::delete(delete_template),
        )
        .route("/api/containers/:id/shutdown", post(shutdown))
        .route("/api/containers/:id/reboot", post(reboot))
        .route(
            "/api/containers/:id/lxc/config",
            get(get_config).put(update_config),
        )
        .route("/api/containers/:id/resize", post(resize))
        .route(
            "/api/containers/:id/snapshots",
            get(list_snapshots).post(create_snapshot),
        )
        .route(
            "/api/containers/:id/snapshots/:snap/rollback",
            post(rollback_snapshot),
        )
        .route(
            "/api/containers/:id/snapshots/:snap",
            axum::routing::delete(delete_snapshot),
        )
}

fn status_event(state: &AppState, id: &str, old: &str, new: &str) {
    state.ws_state.broadcast(WsEvent::ContainerStatusChanged {
        container_id: id.to_string(),
        old_status: old.to_string(),
        new_status: new.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    });
}

async fn host_info(State(state): State<Arc<AppState>>) -> Json<LxcHostInfo> {
    Json(state.container_manager.lxc().host_info().await)
}

#[derive(Debug, Deserialize)]
struct CatalogQuery {
    #[serde(default)]
    refresh: bool,
    arch: Option<String>,
    distro: Option<String>,
}

/// `pveam available`: images on the template server (default: this
/// host's architecture).
async fn template_catalog(
    State(state): State<Arc<AppState>>,
    Query(q): Query<CatalogQuery>,
) -> Result<Json<Vec<LxcCatalogEntry>>, ApiError> {
    let arch = q
        .arch
        .unwrap_or_else(|| crate::container::lxc_config::host_arch().to_string());
    let list = state
        .container_manager
        .lxc()
        .templates()
        .catalog(q.refresh)
        .await
        .map_err(|e| ApiError::ServiceUnavailable(format!("template catalog: {e}")))?;
    Ok(Json(
        list.into_iter()
            .filter(|e| arch == "all" || e.arch == arch)
            .filter(|e| q.distro.as_deref().map(|d| e.distro == d).unwrap_or(true))
            .collect(),
    ))
}

/// `pveam list`: downloaded templates.
async fn list_templates(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<LxcTemplate>>, ApiError> {
    Ok(Json(
        state.container_manager.lxc().templates().list().await?,
    ))
}

#[derive(Debug, Deserialize)]
struct DownloadRequest {
    distro: String,
    release: String,
    arch: Option<String>,
    variant: Option<String>,
}

/// `pveam download`.
async fn download_template(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DownloadRequest>,
) -> Result<Json<LxcTemplate>, ApiError> {
    let arch = req
        .arch
        .unwrap_or_else(|| crate::container::lxc_config::host_arch().to_string());
    let variant = req.variant.unwrap_or_else(|| "default".into());
    // Own task: a client that gives up mid-download must not leave a
    // half-written template behind.
    let mgr = state.container_manager.clone();
    let t = tokio::spawn(async move {
        mgr.lxc()
            .templates()
            .download(&req.distro, &req.release, &arch, &variant)
            .await
    })
    .await
    .map_err(|e| ApiError::Internal(format!("template download task failed: {e}")))??;
    Ok(Json(t))
}

async fn delete_template(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state
        .container_manager
        .lxc()
        .templates()
        .delete(id.trim_start_matches('/'))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Default, Deserialize)]
struct ShutdownRequest {
    timeout: Option<u32>,
    #[serde(default)]
    force: bool,
}

/// `pct shutdown`: clean power-off; `force` kills after the timeout.
async fn shutdown(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Option<Json<ShutdownRequest>>,
) -> Result<Json<ContainerConfig>, ApiError> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    let timeout = req.timeout.unwrap_or(DEFAULT_SHUTDOWN_TIMEOUT).min(3600);
    let mgr = state.container_manager.clone();
    let cid = id.clone();
    let c = tokio::spawn(async move { mgr.shutdown_container(&cid, timeout, req.force).await })
        .await
        .map_err(|e| ApiError::Internal(format!("shutdown task failed: {e}")))??;
    status_event(&state, &id, "running", "stopped");
    Ok(Json(c))
}

#[derive(Debug, Default, Deserialize)]
struct RebootRequest {
    timeout: Option<u32>,
}

/// `pct reboot`: shutdown + start, applying pending changes.
async fn reboot(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Option<Json<RebootRequest>>,
) -> Result<Json<ContainerConfig>, ApiError> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    let timeout = req.timeout.unwrap_or(DEFAULT_SHUTDOWN_TIMEOUT).min(3600);
    let mgr = state.container_manager.clone();
    let cid = id.clone();
    let c = tokio::spawn(async move { mgr.reboot_container(&cid, timeout).await })
        .await
        .map_err(|e| ApiError::Internal(format!("reboot task failed: {e}")))??;
    status_event(&state, &id, "running", "running");
    Ok(Json(c))
}

async fn get_config(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<LxcConfigView>, ApiError> {
    Ok(Json(state.container_manager.lxc_config(&id).await?))
}

#[derive(Debug, Deserialize)]
struct UpdateConfigRequest {
    memory: Option<u64>,
    cpus: Option<u32>,
    lxc: LxcOptions,
}

/// `pct set`.
async fn update_config(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateConfigRequest>,
) -> Result<Json<LxcConfigView>, ApiError> {
    Ok(Json(
        state
            .container_manager
            .update_lxc_config(&id, req.memory, req.cpus, req.lxc)
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
struct ResizeRequest {
    disk_gb: u64,
}

/// `pct resize rootfs <size>` (grow only, works while running).
async fn resize(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<ResizeRequest>,
) -> Result<Json<LxcConfigView>, ApiError> {
    Ok(Json(
        state
            .container_manager
            .resize_container(&id, req.disk_gb)
            .await?,
    ))
}

async fn list_snapshots(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Vec<LxcSnapshot>>, ApiError> {
    Ok(Json(state.container_manager.list_snapshots(&id).await?))
}

#[derive(Debug, Deserialize)]
struct SnapshotRequest {
    name: String,
    comment: Option<String>,
}

async fn create_snapshot(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<SnapshotRequest>,
) -> Result<Json<LxcSnapshot>, ApiError> {
    let mgr = state.container_manager.clone();
    let s = tokio::spawn(async move { mgr.create_snapshot(&id, &req.name, req.comment).await })
        .await
        .map_err(|e| ApiError::Internal(format!("snapshot task failed: {e}")))??;
    Ok(Json(s))
}

async fn rollback_snapshot(
    State(state): State<Arc<AppState>>,
    Path((id, snap)): Path<(String, String)>,
) -> Result<Json<ContainerConfig>, ApiError> {
    let mgr = state.container_manager.clone();
    let cid = id.clone();
    let c = tokio::spawn(async move { mgr.rollback_snapshot(&cid, &snap).await })
        .await
        .map_err(|e| ApiError::Internal(format!("rollback task failed: {e}")))??;
    status_event(&state, &id, "unknown", &c.status.to_string());
    Ok(Json(c))
}

async fn delete_snapshot(
    State(state): State<Arc<AppState>>,
    Path((id, snap)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    state.container_manager.delete_snapshot(&id, &snap).await?;
    Ok(StatusCode::NO_CONTENT)
}
