//! Host PCI inventory for passthrough (any device, not only GPUs).
//!
//! * `GET  /api/gpu/pci/status`                      IOMMU / vfio / mdev support of this host
//! * `GET  /api/gpu/pci/devices`                     every PCI device with class, driver,
//!   IOMMU group (and which group members block passthrough) and the VMs that list it
//! * `POST /api/gpu/pci/devices/:addr/bind-vfio`     move a device to `vfio-pci`
//! * `POST /api/gpu/pci/devices/:addr/unbind-vfio`   give it back to its host driver
//!   (409 while a VM lists it)
//!
//! A VM gets the device through `hardware.hostpci` (see `api/mdev.rs`).
//! Such a VM is pinned to this host: live migration is refused (device state
//! cannot be moved), drain/rebalance report it as not movable, and HA
//! recovery only starts it on a node that has the same device bound to
//! `vfio-pci` (otherwise the recovery fails with that reason and is retried
//! elsewhere).
use crate::error::ApiError;
use crate::vm::pci::{PciDevice, PciManager};
use crate::AppState;
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use horcrux_common::{Error, HostPciKind};
use serde::Serialize;
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/gpu/pci/status", get(status))
        .route("/api/gpu/pci/devices", get(list))
        .route("/api/gpu/pci/devices/:addr/bind-vfio", post(bind))
        .route("/api/gpu/pci/devices/:addr/unbind-vfio", post(unbind))
}

#[derive(Serialize)]
struct PciView {
    #[serde(flatten)]
    device: PciDevice,
    /// Ids of the VMs whose `hostpci` lists this device.
    vms: Vec<String>,
}

#[derive(Serialize)]
struct Status {
    iommu_enabled: bool,
    iommu_groups: usize,
    vfio_pci_loaded: bool,
    mdev_supported: bool,
    message: String,
}

/// (vm id, device) pairs of every PCI device a VM lists.
async fn users(state: &AppState) -> Vec<(String, String)> {
    state
        .vm_manager
        .list_vms()
        .await
        .into_iter()
        .flat_map(|v| {
            v.hardware
                .hostpci
                .iter()
                .filter_map(|d| match d.kind() {
                    Ok(HostPciKind::Pci(a)) => Some((v.id.clone(), a)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

async fn status() -> Json<Status> {
    let m = PciManager::new();
    let groups = m.iommu_groups();
    let vfio = std::path::Path::new("/sys/bus/pci/drivers/vfio-pci").exists();
    let mdev = std::path::Path::new("/sys/class/mdev_bus").exists();
    let message = if groups == 0 {
        "No IOMMU groups: enable the IOMMU (intel_iommu=on / amd_iommu=on, AMD EPYC: iommu=pt) in the \
         BIOS and the kernel command line, then reboot"
            .to_string()
    } else if !vfio {
        format!(
            "IOMMU active ({groups} groups); the vfio-pci driver is not loaded (modprobe vfio-pci)"
        )
    } else {
        format!("IOMMU active ({groups} groups), vfio-pci loaded")
    };
    Json(Status {
        iommu_enabled: groups > 0,
        iommu_groups: groups,
        vfio_pci_loaded: vfio,
        mdev_supported: mdev,
        message,
    })
}

async fn list(State(state): State<Arc<AppState>>) -> Result<Json<Vec<PciView>>, ApiError> {
    let devices = tokio::task::spawn_blocking(|| PciManager::new().list())
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let used = users(&state).await;
    Ok(Json(
        devices
            .into_iter()
            .map(|d| {
                let vms = used
                    .iter()
                    .filter(|(_, a)| *a == d.address)
                    .map(|(v, _)| v.clone())
                    .collect();
                PciView { device: d, vms }
            })
            .collect(),
    ))
}

async fn bind(Path(addr): Path<String>) -> Result<Json<PciDevice>, ApiError> {
    let d = tokio::task::spawn_blocking(move || PciManager::new().bind_vfio(&addr))
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))??;
    Ok(Json(d))
}

async fn unbind(
    State(state): State<Arc<AppState>>,
    Path(addr): Path<String>,
) -> Result<Json<PciDevice>, ApiError> {
    let a = match (horcrux_common::VmHostPci {
        device: addr.clone(),
        primary_gpu: false,
    })
    .kind()
    {
        Ok(HostPciKind::Pci(a)) => a,
        _ => {
            return Err(Error::Validation(format!("{addr:?} is not a PCI address")).into());
        }
    };
    if let Some((vm, _)) = users(&state).await.into_iter().find(|(_, d)| *d == a) {
        return Err(Error::Conflict(format!(
            "PCI device {a} is used by VM {vm}; remove it from the VM's hardware first"
        ))
        .into());
    }
    let d = tokio::task::spawn_blocking(move || PciManager::new().unbind_vfio(&addr))
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))??;
    Ok(Json(d))
}
