//! Storage pool volumes (Proxmox `pvesm alloc|free|list`, `qm disk resize`
//! on an unattached volume).
//!
//! - `GET    /api/storage/pools/:pool_id/volumes`              list
//! - `POST   /api/storage/pools/:pool_id/volumes`              create `{name, size}` (GB)
//! - `DELETE /api/storage/pools/:pool_id/volumes/:name`        delete (409 while a VM uses it)
//! - `POST   /api/storage/pools/:pool_id/volumes/:name/resize` grow `{size}` (GB)
//! - `GET    /api/storage/pools/:pool_id/volumes/:name/snapshots`  snapshot chain (oldest first)
//! - `POST   /api/storage/pools/:pool_id/volumes/:name/snapshots`  take `{name}`
//! - `DELETE /api/storage/pools/:pool_id/volumes/:name/snapshots/:snap`  delete one (merged into its child)
//! - `POST   /api/storage/pools/:pool_id/volumes/:name/snapshots/:snap/rollback`  back to the newest
//!
//!   The snapshot routes work on LVM pools with `snapshot_as_volume_chain`
//!   (qcow2 volume chain on thick LVs, e.g. a shared VG on a SAN LUN); a
//!   volume a VM uses is refused (409), like delete and resize.
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
        .route(
            "/api/storage/pools/:pool_id/volumes/:name/snapshots",
            get(list_snapshots).post(create_snapshot),
        )
        .route(
            "/api/storage/pools/:pool_id/volumes/:name/snapshots/:snap",
            axum::routing::delete(delete_snapshot),
        )
        .route(
            "/api/storage/pools/:pool_id/volumes/:name/snapshots/:snap/rollback",
            post(rollback_snapshot),
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

/// Block pools that hold VM disks as volumes (Proxmox: a disk on LVM is
/// a logical volume `vm-<id>-disk-<n>`): enabled LVM pools, including a
/// shared VG on a SAN LUN and pools with `snapshot_as_volume_chain` (the
/// volume then holds a qcow2 image). Only Create VM and Add disk place
/// disks there; importers copy into files and keep file pools.
fn holds_vm_block_volumes(p: &crate::storage::StoragePool) -> bool {
    p.enabled && p.storage_type == crate::storage::StorageType::Lvm
}

/// Image format of a VM disk on block pool `p`: qcow2 in a volume-chain
/// pool, raw otherwise.
fn block_volume_format(p: &crate::storage::StoragePool) -> &'static str {
    if p.lvm.as_ref().is_some_and(|o| o.snapshot_as_volume_chain) {
        "qcow2"
    } else {
        "raw"
    }
}

/// Pools offered by the Create VM / Add disk pool selector.
pub async fn vm_disk_pools(state: &AppState) -> Vec<VmDiskPool> {
    let mut out: Vec<VmDiskPool> = state
        .storage_manager
        .list_pools()
        .await
        .into_iter()
        .filter(|p| holds_vm_disks(p) || holds_vm_block_volumes(p))
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

/// The block pool `pool_id` if it holds VM disks as volumes (`None` for a
/// file pool or an unknown id: the file path then decides).
async fn block_pool(state: &AppState, pool_id: &str) -> Option<crate::storage::StoragePool> {
    state
        .storage_manager
        .get_pool(pool_id)
        .await
        .ok()
        .filter(holds_vm_block_volumes)
}

/// Create a VM's disk `name` of `size_gb` GB in block pool `pool`:
/// (device path, image format).
async fn provision_block_disk(
    state: &AppState,
    pool: &crate::storage::StoragePool,
    name: &str,
    size_gb: u64,
) -> Result<(String, String), ApiError> {
    if size_gb == 0 {
        return Err(ApiError::BadRequest(
            "disk size must be at least 1 GB".into(),
        ));
    }
    let existing = state
        .storage_manager
        .list_volumes(&pool.id)
        .await
        .map_err(err)?;
    if existing.iter().any(|v| v.name == name) {
        return Err(ApiError::Conflict(format!(
            "storage pool {} already holds a volume {name}",
            pool.name
        )));
    }
    state
        .storage_manager
        .create_volume(&pool.id, name, size_gb)
        .await
        .map_err(err)?;
    let dev = crate::storage::StorageManager::volume_location(pool, name).map_err(err)?;
    Ok((dev, block_volume_format(pool).to_string()))
}

/// Delete the block-pool volumes among `disks` (VM deleted, create failed,
/// disk removed with its data): through the pool, so its wipe/discard
/// settings apply. Failures are logged and returned as text.
pub async fn release_block_disks(
    state: &AppState,
    disks: &[horcrux_common::VmDisk],
) -> Vec<String> {
    let mut failed = Vec::new();
    for d in disks.iter().filter(|d| d.is_block_volume()) {
        let (Some(pool), Some(name)) = (
            d.storage.as_deref(),
            std::path::Path::new(&d.path)
                .file_name()
                .and_then(|n| n.to_str()),
        ) else {
            continue;
        };
        if let Err(e) = state.storage_manager.delete_volume(pool, name).await {
            tracing::warn!("volume {name} of storage pool {pool} stayed: {e}");
            failed.push(format!("{pool}/{name}: {e}"));
        }
    }
    failed
}

/// Create VM (and the declarative config): like `place_vm_disks`, and a
/// disk on a block pool (LVM, shared LVM on a SAN LUN) gets its own
/// volume `vm-<id>-disk-<n>` created now. On error every volume made
/// here is deleted again; the caller deletes them if creating the VM fails
/// later (`release_block_disks`).
pub async fn place_vm_disks_with_volumes(
    state: &AppState,
    config: &mut horcrux_common::VmConfig,
) -> Result<(), ApiError> {
    let mut made: Vec<horcrux_common::VmDisk> = Vec::new();
    for i in 0..config.disks.len() {
        let size = if i == 0 && config.disks[i].size_gb == 0 {
            config.disk_size
        } else {
            config.disks[i].size_gb
        };
        let d = &mut config.disks[i];
        d.storage = d.storage.take().filter(|s| !s.trim().is_empty());
        d.format = None;
        let placed: Result<String, ApiError> = match d.storage.clone() {
            None => Ok(String::new()),
            Some(pool_id) => match block_pool(state, &pool_id).await {
                Some(pool) => {
                    let name = format!("vm-{}-disk-{i}", config.id);
                    let r = match crate::validation::validate_vm_id(&config.id) {
                        Err(e) => Err(ApiError::from(e)),
                        Ok(()) => provision_block_disk(state, &pool, &name, size).await,
                    };
                    match r {
                        Ok((dev, fmt)) => {
                            let d = &mut config.disks[i];
                            d.size_gb = size;
                            d.format = Some(fmt);
                            d.path = dev.clone();
                            made.push(d.clone());
                            Ok(dev)
                        }
                        Err(e) => Err(e),
                    }
                }
                None => vm_disk_dir(state, &pool_id, size)
                    .await
                    .map(|dir| dir.join("pending.qcow2").to_string_lossy().to_string()),
            },
        };
        match placed {
            Ok(path) => config.disks[i].path = path,
            Err(e) => {
                release_block_disks(state, &made).await;
                return Err(e);
            }
        }
    }
    Ok(())
}

/// Add disk on the VM's Hardware page: like `place_new_disk`, and a disk
/// on a block pool gets its own volume `vm-<id>-disk-<n>` (the first free
/// `n`) created now; the caller deletes it if attaching fails.
pub async fn place_new_disk_with_volume(
    state: &AppState,
    vm_id: &str,
    spec: &mut crate::vm::hardware::DiskSpec,
) -> Result<(), ApiError> {
    spec.storage = spec.storage.take().filter(|s| !s.trim().is_empty());
    spec.volume = None;
    let Some(pool) = (match &spec.storage {
        Some(p) => block_pool(state, p).await,
        None => None,
    }) else {
        return place_new_disk(state, spec).await;
    };
    crate::validation::validate_vm_id(vm_id)?;
    let taken: Vec<String> = state
        .storage_manager
        .list_volumes(&pool.id)
        .await
        .map_err(err)?
        .into_iter()
        .map(|v| v.name)
        .collect();
    let used: Vec<String> = state
        .vm_manager
        .get_vm(vm_id)
        .await
        .map(|vm| vm.disks.into_iter().map(|d| d.path).collect())
        .unwrap_or_default();
    let name = (1..256)
        .map(|n| format!("vm-{vm_id}-disk-{n}"))
        .find(|n| !taken.contains(n) && !used.iter().any(|p| p.ends_with(&format!("/{n}"))))
        .ok_or_else(|| ApiError::Conflict(format!("no free volume name for VM {vm_id}")))?;
    let (dev, fmt) = provision_block_disk(state, &pool, &name, spec.size_gb).await?;
    spec.storage = Some(pool.id.clone());
    spec.volume = Some((dev, fmt));
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

/// The pool and the location of its volume `name`, 404 for an unknown pool.
async fn pool_location(state: &AppState, pool_id: &str, name: &str) -> Result<String, ApiError> {
    let pool = state
        .storage_manager
        .get_pool(pool_id)
        .await
        .map_err(|_| ApiError::NotFound(format!("Storage pool {pool_id} not found")))?;
    crate::storage::StorageManager::volume_location(&pool, name).map_err(err)
}

async fn list_snapshots(
    State(state): State<Arc<AppState>>,
    Path((pool_id, name)): Path<(String, String)>,
) -> Result<Json<crate::storage::lvm_chain::ChainView>, ApiError> {
    check_name(&name)?;
    pool_location(&state, &pool_id, &name).await?;
    Ok(Json(
        state
            .storage_manager
            .volume_snapshots(&pool_id, &name)
            .await
            .map_err(err)?,
    ))
}

#[derive(Debug, Deserialize)]
pub struct CreateSnapshotRequest {
    pub name: String,
}

async fn create_snapshot(
    State(state): State<Arc<AppState>>,
    Path((pool_id, name)): Path<(String, String)>,
    Json(req): Json<CreateSnapshotRequest>,
) -> Result<
    (
        axum::http::StatusCode,
        Json<crate::storage::lvm_chain::ChainView>,
    ),
    ApiError,
> {
    check_name(&name)?;
    check_name(&req.name)?;
    let location = pool_location(&state, &pool_id, &name).await?;
    refuse_in_use(&state, &name, &location, "snapshot the VM instead").await?;
    state
        .storage_manager
        .create_snapshot(&pool_id, &name, &req.name)
        .await
        .map_err(err)?;
    Ok((
        axum::http::StatusCode::CREATED,
        Json(
            state
                .storage_manager
                .volume_snapshots(&pool_id, &name)
                .await
                .map_err(err)?,
        ),
    ))
}

async fn delete_snapshot(
    State(state): State<Arc<AppState>>,
    Path((pool_id, name, snap)): Path<(String, String, String)>,
) -> Result<Json<crate::storage::lvm_chain::ChainView>, ApiError> {
    check_name(&name)?;
    check_name(&snap)?;
    let location = pool_location(&state, &pool_id, &name).await?;
    refuse_in_use(&state, &name, &location, "stop the VM first").await?;
    state
        .storage_manager
        .delete_volume_snapshot(&pool_id, &name, &snap)
        .await
        .map_err(err)?;
    Ok(Json(
        state
            .storage_manager
            .volume_snapshots(&pool_id, &name)
            .await
            .map_err(err)?,
    ))
}

async fn rollback_snapshot(
    State(state): State<Arc<AppState>>,
    Path((pool_id, name, snap)): Path<(String, String, String)>,
) -> Result<Json<crate::storage::lvm_chain::ChainView>, ApiError> {
    check_name(&name)?;
    check_name(&snap)?;
    let location = pool_location(&state, &pool_id, &name).await?;
    refuse_in_use(&state, &name, &location, "stop the VM first").await?;
    state
        .storage_manager
        .restore_snapshot(&pool_id, &name, &snap)
        .await
        .map_err(err)?;
    Ok(Json(
        state
            .storage_manager
            .volume_snapshots(&pool_id, &name)
            .await
            .map_err(err)?,
    ))
}
