//! Hardware GPU slicing: per-GPU capabilities, declarative SR-IOV provisioning
//! and the save method of a VM's host devices.
//!
//! * `GET  /api/gpu/capabilities`       every GPU: passthrough / SR-IOV VFs
//!   (driver, totalvfs, current VFs, VRAM per VF, VFIO migration per VF) /
//!   vendor vGPU (mdev types and devices) and a reason for each mode that is
//!   not available
//! * `GET  /api/gpu/sriov/config`       the desired SR-IOV config
//! * `PUT  /api/gpu/sriov/config`       store it (does not touch hardware)
//! * `POST /api/gpu/sriov/plan`         `{config?}` steps or refusal reasons
//! * `POST /api/gpu/sriov/apply`        `{config?, dry_run?}` carry them out
//! * `GET  /api/gpu/sriov/generations`  applied configs
//! * `POST /api/gpu/sriov/rollback`     `{generation?}` re-apply an earlier one
//! * `GET  /api/vms/:id/host-device-save` how the VM's host devices affect a
//!   host-side save (`host save (VFIO migration supported)` or
//!   `not saveable: <reason>`)
//!
//! The same config is applied when the API starts (`apply_on_boot`), so VFs
//! exist before VMs autostart on both init systems.
use crate::error::ApiError;
use crate::vm::gpucap::{self, GpuCapability, GpuCaps, HostPaths, IoctlProbe, VmDeviceSave};
use crate::vm::sriov::{self, ApplyReport, Generation, Plan, SriovConfig, SriovStore, Usage};
use crate::AppState;
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use horcrux_common::{Error, HostPciKind, VmStatus};
use serde::Deserialize;
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/gpu/capabilities", get(capabilities))
        .route("/api/gpu/sriov/config", get(get_config).put(put_config))
        .route("/api/gpu/sriov/plan", post(plan))
        .route("/api/gpu/sriov/apply", post(apply))
        .route("/api/gpu/sriov/generations", get(generations))
        .route("/api/gpu/sriov/rollback", post(rollback))
        .route("/api/vms/:id/host-device-save", get(vm_host_device_save))
}

fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> impl std::future::Future<Output = Result<T, ApiError>> {
    async move {
        tokio::task::spawn_blocking(f)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
            .map_err(ApiError::from)
    }
}

/// VF address / mdev uuid -> (vm id, running) for every VM hostpci entry.
async fn assignments(state: &AppState) -> Vec<(String, String, bool)> {
    let mut out = Vec::new();
    for v in state.vm_manager.list_vms().await {
        let running = v.status == VmStatus::Running;
        for d in &v.hardware.hostpci {
            let key = match d.kind() {
                Ok(HostPciKind::Pci(a)) => a,
                Ok(HostPciKind::Mdev(u)) => u,
                Err(_) => continue,
            };
            out.push((key, v.id.clone(), running));
        }
    }
    out
}

pub async fn usage(state: &AppState) -> Usage {
    let mut u = Usage::default();
    for (k, vm, running) in assignments(state).await {
        if running {
            u.running.insert(k.clone());
        }
        u.assigned.insert(k, vm);
    }
    u
}

async fn capabilities(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<GpuCapability>>, ApiError> {
    let mut caps =
        blocking(|| Ok(GpuCaps::new(HostPaths::from_env(), &IoctlProbe).collect())).await?;
    let asg = assignments(&state).await;
    let vm_of = |k: &str| {
        asg.iter()
            .find(|(a, _, _)| a == k)
            .map(|(_, v, _)| v.clone())
    };
    for g in &mut caps {
        for vf in g.sriov.iter_mut().flat_map(|s| s.vfs.iter_mut()) {
            vf.vm = vm_of(&vf.address);
        }
        for m in g.vendor_vgpu.iter_mut().flat_map(|v| v.devices.iter_mut()) {
            m.vm = vm_of(&m.uuid);
        }
    }
    Ok(Json(caps))
}

async fn get_config() -> Json<SriovConfig> {
    Json(SriovStore::from_env().config())
}

async fn put_config(Json(c): Json<SriovConfig>) -> Result<Json<SriovConfig>, ApiError> {
    blocking(move || {
        SriovStore::from_env().set_config(&c)?;
        Ok(c)
    })
    .await
    .map(Json)
}

#[derive(Deserialize, Default)]
struct ApplyReq {
    /// The config to plan/apply; the stored one when absent.
    #[serde(default)]
    config: Option<SriovConfig>,
    #[serde(default)]
    dry_run: bool,
}

async fn plan(
    State(state): State<Arc<AppState>>,
    body: Option<Json<ApplyReq>>,
) -> Result<Json<Plan>, ApiError> {
    let req = body.map(|b| b.0).unwrap_or_default();
    let u = usage(&state).await;
    let r = blocking(move || {
        let store = SriovStore::from_env();
        let cfg = req.config.unwrap_or_else(|| store.config());
        sriov::reconcile(&HostPaths::from_env(), &store, &cfg, &u, true, "apply")
    })
    .await?;
    Ok(Json(r.plan))
}

async fn apply(
    State(state): State<Arc<AppState>>,
    body: Option<Json<ApplyReq>>,
) -> Result<Json<ApplyReport>, ApiError> {
    let req = body.map(|b| b.0).unwrap_or_default();
    let u = usage(&state).await;
    blocking(move || {
        let store = SriovStore::from_env();
        let cfg = req.config.unwrap_or_else(|| store.config());
        sriov::reconcile(
            &HostPaths::from_env(),
            &store,
            &cfg,
            &u,
            req.dry_run,
            "apply",
        )
    })
    .await
    .map(Json)
}

async fn generations() -> Json<Vec<Generation>> {
    Json(SriovStore::from_env().generations())
}

#[derive(Deserialize, Default)]
struct RollbackReq {
    /// Generation to return to; the one before the newest when absent.
    #[serde(default)]
    generation: Option<u64>,
    #[serde(default)]
    dry_run: bool,
}

async fn rollback(
    State(state): State<Arc<AppState>>,
    body: Option<Json<RollbackReq>>,
) -> Result<Json<ApplyReport>, ApiError> {
    let req = body.map(|b| b.0).unwrap_or_default();
    let u = usage(&state).await;
    blocking(move || {
        let store = SriovStore::from_env();
        let gens = store.generations();
        let target = match req.generation {
            Some(id) => gens.iter().find(|g| g.id == id),
            None if gens.len() >= 2 => gens.get(gens.len() - 2),
            None => None,
        }
        .ok_or_else(|| {
            Error::NotFound(match req.generation {
                Some(id) => format!("generation {id} does not exist"),
                None => "there is no earlier generation to return to".into(),
            })
        })?;
        sriov::reconcile(
            &HostPaths::from_env(),
            &store,
            &target.config.clone(),
            &u,
            req.dry_run,
            "rollback",
        )
    })
    .await
    .map(Json)
}

async fn vm_host_device_save(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<VmDeviceSave>, ApiError> {
    let vm = state.vm_manager.get_vm(&id).await?;
    let devices: Vec<String> = vm
        .hardware
        .hostpci
        .iter()
        .map(|d| d.device.clone())
        .collect();
    blocking(move || {
        Ok(gpucap::vm_device_save(
            &HostPaths::from_env(),
            &IoctlProbe,
            &devices,
        ))
    })
    .await
    .map(Json)
}

/// Apply the stored config at startup (before VMs autostart). Failures are
/// logged, never fatal: the node still starts.
pub async fn apply_on_boot() {
    let r = tokio::task::spawn_blocking(|| {
        let store = SriovStore::from_env();
        let cfg = store.config();
        if !cfg.apply_on_boot || cfg.pfs.is_empty() {
            return None;
        }
        Some(sriov::reconcile(
            &HostPaths::from_env(),
            &store,
            &cfg,
            &Usage::default(),
            false,
            "boot",
        ))
    })
    .await;
    match r {
        Ok(Some(Ok(rep))) if rep.ok => {
            tracing::info!("SR-IOV: applied {} step(s) at boot", rep.results.len())
        }
        Ok(Some(Ok(rep))) => {
            for e in &rep.plan.errors {
                tracing::warn!("SR-IOV boot apply: {e}");
            }
            for s in rep.results.iter().filter(|s| s.status != "applied") {
                tracing::warn!(
                    "SR-IOV boot apply: {} {} {}: {}",
                    s.step.pf,
                    s.step.action,
                    s.status,
                    s.message
                );
            }
        }
        Ok(Some(Err(e))) => tracing::warn!("SR-IOV boot apply: {e}"),
        _ => {}
    }
}
