//! Mediated devices (vGPU) and the VMs that use passthrough devices.
//!
//! * `GET    /api/gpu/mdev/types`        what the host can create (NVIDIA vGPU
//!   profiles, Intel GVT-g types, ... from `/sys/class/mdev_bus`)
//! * `GET    /api/gpu/mdev/devices`      the mediated devices that exist, with
//!   the VM (if any) that has each one in its `hardware.hostpci`
//! * `POST   /api/gpu/mdev/devices`      `{parent, type, uuid?}` create one
//! * `DELETE /api/gpu/mdev/devices/:uuid` remove one (409 while a VM lists it)
//!
//! A VM gets a device through `PUT /api/vms/:id/config`
//! (`hardware.hostpci: [{device: "0000:01:00.0"|"<mdev uuid>", primary_gpu}]`);
//! plain PCI devices are prepared with `/api/gpu/devices/:addr/bind-vfio`.
use crate::error::ApiError;
use crate::vm::mdev::{MdevDevice, MdevManager, MdevType};
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get},
    Json, Router,
};
use horcrux_common::{Error, HostPciKind};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/gpu/mdev/types", get(list_types))
        .route(
            "/api/gpu/mdev/devices",
            get(list_devices).post(create_device),
        )
        .route("/api/gpu/mdev/devices/:uuid", delete(delete_device))
}

#[derive(Serialize)]
struct MdevView {
    #[serde(flatten)]
    device: MdevDevice,
    /// Id of the VM that lists the device, if any.
    vm: Option<String>,
}

#[derive(Deserialize)]
struct CreateMdev {
    parent: String,
    #[serde(rename = "type")]
    mdev_type: String,
    #[serde(default)]
    uuid: Option<String>,
}

async fn list_types() -> Json<Vec<MdevType>> {
    Json(MdevManager::new().list_types())
}

/// The VM (by id) whose `hostpci` names the mediated device `uuid`.
async fn user_of(state: &AppState, uuid: &str) -> Option<String> {
    state.vm_manager.list_vms().await.into_iter().find_map(|v| {
        v.hardware
            .hostpci
            .iter()
            .any(|d| d.kind() == Ok(HostPciKind::Mdev(uuid.to_string())))
            .then_some(v.id)
    })
}

async fn list_devices(State(state): State<Arc<AppState>>) -> Json<Vec<MdevView>> {
    let mut out = Vec::new();
    for device in MdevManager::new().list_devices() {
        let vm = user_of(&state, &device.uuid).await;
        out.push(MdevView { device, vm });
    }
    Json(out)
}

async fn create_device(
    Json(r): Json<CreateMdev>,
) -> Result<(StatusCode, Json<MdevDevice>), ApiError> {
    let d = tokio::task::spawn_blocking(move || {
        MdevManager::new().create(&r.parent, &r.mdev_type, r.uuid.as_deref())
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))??;
    Ok((StatusCode::CREATED, Json(d)))
}

async fn delete_device(
    State(state): State<Arc<AppState>>,
    Path(uuid): Path<String>,
) -> Result<StatusCode, ApiError> {
    let uuid = uuid.to_ascii_lowercase();
    if let Some(vm) = user_of(&state, &uuid).await {
        return Err(Error::Conflict(format!(
            "mediated device {uuid} is used by VM {vm}; remove it from the VM's hardware first"
        ))
        .into());
    }
    tokio::task::spawn_blocking(move || MdevManager::new().remove(&uuid))
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))??;
    Ok(StatusCode::NO_CONTENT)
}
