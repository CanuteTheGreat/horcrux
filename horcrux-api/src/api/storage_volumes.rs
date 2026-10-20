//! Storage pool volumes (Proxmox `pvesm alloc|free|list`, `qm disk resize`
//! on an unattached volume).
//!
//! - `GET    /api/storage/pools/:pool_id/volumes`              list
//! - `POST   /api/storage/pools/:pool_id/volumes`              create `{name, size}` (GB)
//! - `DELETE /api/storage/pools/:pool_id/volumes/:name`        delete (409 while a VM uses it)
//! - `POST   /api/storage/pools/:pool_id/volumes/:name/resize` grow `{size}` (GB)
//!
//! Every volume name goes through `validation::validate_volume_name` here
//! (400 for `../x`, `a/b`, `/abs`, `.`, ...) and again inside each storage
//! backend before it is joined onto a pool path.
use crate::error::ApiError;
use crate::storage::VolumeInfo;
use crate::AppState;
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/storage/pools/:pool_id/volumes",
            get(list_volumes).post(create_volume),
        )
        .route(
            "/api/storage/pools/:pool_id/volumes/:name",
            axum::routing::delete(delete_volume),
        )
        .route(
            "/api/storage/pools/:pool_id/volumes/:name/resize",
            post(resize_volume),
        )
}

/// Bad input (invalid name, shrink, not enough space, unsupported on this
/// pool type) is a 400; everything else keeps its usual status.
fn err(e: horcrux_common::Error) -> ApiError {
    match e {
        horcrux_common::Error::Validation(m) | horcrux_common::Error::InvalidConfig(m) => {
            ApiError::BadRequest(m)
        }
        other => ApiError::from(other),
    }
}

fn check_name(name: &str) -> Result<(), ApiError> {
    crate::validation::validate_volume_name(name).map_err(err)
}

async fn list_volumes(
    State(state): State<Arc<AppState>>,
    Path(pool_id): Path<String>,
) -> Result<Json<Vec<VolumeInfo>>, ApiError> {
    Ok(Json(
        state
            .storage_manager
            .list_volumes(&pool_id)
            .await
            .map_err(err)?,
    ))
}

#[derive(Debug, Deserialize)]
pub struct CreateVolumeRequest {
    pub name: String,
    /// Size in GB.
    pub size: u64,
}

async fn create_volume(
    State(state): State<Arc<AppState>>,
    Path(pool_id): Path<String>,
    Json(req): Json<CreateVolumeRequest>,
) -> Result<Json<VolumeInfo>, ApiError> {
    check_name(&req.name)?;
    let path = state
        .storage_manager
        .create_volume(&pool_id, &req.name, req.size)
        .await
        .map_err(err)?;
    let listed = state
        .storage_manager
        .list_volumes(&pool_id)
        .await
        .unwrap_or_default();
    Ok(Json(
        listed
            .into_iter()
            .find(|v| v.name == req.name)
            .unwrap_or(VolumeInfo {
                name: req.name,
                path,
                format: String::new(),
                size_bytes: req.size * 1024 * 1024 * 1024,
                allocated_bytes: 0,
            }),
    ))
}

/// Name of a VM whose disk is `location`, if any.
async fn used_by(state: &AppState, location: &str) -> Option<String> {
    state
        .vm_manager
        .list_vms()
        .await
        .into_iter()
        .find(|vm| vm.disks.iter().any(|d| d.path == location))
        .map(|vm| format!("{} ({})", vm.name, vm.id))
}

async fn delete_volume(
    State(state): State<Arc<AppState>>,
    Path((pool_id, name)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    check_name(&name)?;
    let pool = state
        .storage_manager
        .get_pool(&pool_id)
        .await
        .map_err(|_| ApiError::NotFound(format!("Storage pool {pool_id} not found")))?;
    let location = crate::storage::StorageManager::volume_location(&pool, &name).map_err(err)?;
    if let Some(vm) = used_by(&state, &location).await {
        return Err(ApiError::Conflict(format!(
            "Volume {name} is a disk of VM {vm}; detach it from the VM first"
        )));
    }
    state
        .storage_manager
        .delete_volume(&pool_id, &name)
        .await
        .map_err(err)?;
    Ok(Json(serde_json::json!({ "deleted": name })))
}

#[derive(Debug, Deserialize)]
pub struct ResizeVolumeRequest {
    /// New size in GB (grow only).
    pub size: u64,
}

async fn resize_volume(
    State(state): State<Arc<AppState>>,
    Path((pool_id, name)): Path<(String, String)>,
    Json(req): Json<ResizeVolumeRequest>,
) -> Result<Json<VolumeInfo>, ApiError> {
    check_name(&name)?;
    state
        .storage_manager
        .resize_volume(&pool_id, &name, req.size)
        .await
        .map_err(err)?;
    state
        .storage_manager
        .list_volumes(&pool_id)
        .await
        .map_err(err)?
        .into_iter()
        .find(|v| v.name == name)
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("Volume {name} not found")))
}
