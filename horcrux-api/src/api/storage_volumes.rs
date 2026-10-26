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

/// 409 if the volume is a VM's disk, or the base image (anywhere in the
/// backing chain) of a linked clone's disk: deleting or rewriting it
/// would leave those VMs unbootable ("Could not open backing file").
/// Same check as deleting a VM or a template with linked clones.
async fn refuse_in_use(
    state: &AppState,
    name: &str,
    location: &str,
    what: &str,
) -> Result<(), ApiError> {
    if let Some(vm) = used_by(state, location).await {
        return Err(ApiError::Conflict(format!(
            "Volume {name} is a disk of VM {vm}; {what}"
        )));
    }
    let dependents = state
        .vm_manager
        .vms_backed_by(std::path::Path::new(location))
        .await;
    if !dependents.is_empty() {
        return Err(ApiError::Conflict(format!(
            "Volume {name} is the base image of the linked clone(s) {}; delete those VMs \
             first",
            dependents.join(", ")
        )));
    }
    Ok(())
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
    refuse_in_use(&state, &name, &location, "detach it from the VM first").await?;
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
    let pool = state
        .storage_manager
        .get_pool(&pool_id)
        .await
        .map_err(|_| ApiError::NotFound(format!("Storage pool {pool_id} not found")))?;
    let location = crate::storage::StorageManager::volume_location(&pool, &name).map_err(err)?;
    refuse_in_use(
        &state,
        &name,
        &location,
        "grow it on the VM's Hardware page",
    )
    .await?;
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

// ---- VM disks on storage pools ------------------------------------------

/// A pool a VM disk can be created in (`GET /api/vms/host-options`).
#[derive(Debug, Clone, serde::Serialize, Deserialize)]
pub struct VmDiskPool {
    pub id: String,
    pub name: String,
    pub storage_type: crate::storage::StorageType,
    pub path: String,
    /// GB free.
    pub available: u64,
}

/// File-based pools hold VM disks as `<pool dir>/<vm>[-diskN].qcow2`;
/// block pools (ZFS/LVM/Ceph/iSCSI), btrfs (subvolume per volume) and S3
/// are not offered for VM disks.
fn holds_vm_disks(p: &crate::storage::StoragePool) -> bool {
    use crate::storage::StorageType as T;
    p.enabled
        && matches!(
            p.storage_type,
            T::Directory | T::Nfs | T::Cifs | T::GlusterFs | T::CephFs
        )
        && std::path::Path::new(&p.path).is_absolute()
        && std::path::Path::new(&p.path).is_dir()
}

/// Pools offered by the Create VM / Add disk pool selector.
pub async fn vm_disk_pools(state: &AppState) -> Vec<VmDiskPool> {
    let mut out: Vec<VmDiskPool> = state
        .storage_manager
        .list_pools()
        .await
        .into_iter()
        .filter(holds_vm_disks)
        .map(|p| VmDiskPool {
            id: p.id,
            name: p.name,
            storage_type: p.storage_type,
            path: p.path,
            available: p.available,
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Directory to create a `size_gb` VM disk in for pool `pool_id` (400 if
/// the pool can't hold VM disks or lacks the space, 404 if unknown).
pub async fn vm_disk_dir(
    state: &AppState,
    pool_id: &str,
    size_gb: u64,
) -> Result<std::path::PathBuf, ApiError> {
    let pool = state
        .storage_manager
        .get_pool(pool_id)
        .await
        .map_err(|_| ApiError::NotFound(format!("Storage pool {pool_id} not found")))?;
    if !holds_vm_disks(&pool) {
        return Err(ApiError::BadRequest(format!(
            "Storage pool {} ({:?}) can't hold VM disks: use an enabled directory, NFS, \
             CIFS or GlusterFS pool",
            pool.name, pool.storage_type
        )));
    }
    if size_gb > pool.available {
        return Err(ApiError::BadRequest(format!(
            "Not enough space in pool {}. Requested: {}GB, Available: {}GB",
            pool.name, size_gb, pool.available
        )));
    }
    Ok(std::path::PathBuf::from(&pool.path))
}

/// Create VM: resolve each disk's `storage` pool to its directory (the VM
/// manager keeps that directory and picks the file name). Request paths
/// are never trusted; disks without a pool go to the VM storage directory.
pub async fn place_vm_disks(
    state: &AppState,
    config: &mut horcrux_common::VmConfig,
) -> Result<(), ApiError> {
    for (i, d) in config.disks.iter_mut().enumerate() {
        d.storage = d.storage.take().filter(|s| !s.trim().is_empty());
        d.path = match &d.storage {
            None => String::new(),
            Some(pool) => {
                let size = if i == 0 && d.size_gb == 0 {
                    config.disk_size
                } else {
                    d.size_gb
                };
                vm_disk_dir(state, pool, size)
                    .await?
                    .join("pending.qcow2")
                    .to_string_lossy()
                    .to_string()
            }
        };
    }
    Ok(())
}

/// Add disk: resolve the spec's `storage` pool to its directory.
pub async fn place_new_disk(
    state: &AppState,
    spec: &mut crate::vm::hardware::DiskSpec,
) -> Result<(), ApiError> {
    spec.storage = spec.storage.take().filter(|s| !s.trim().is_empty());
    spec.dir = match &spec.storage {
        None => None,
        Some(pool) => Some(vm_disk_dir(state, pool, spec.size_gb).await?),
    };
    Ok(())
}
