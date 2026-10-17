//! VM display settings and SPICE console endpoints.
//!
//! * `PUT  /api/vms/:id/display`        protocol / video card / sound card
//!   of a stopped QEMU VM (applies at the next start).
//! * `POST /api/console/:vm_id/spice`   SPICE endpoint + this run's password.
//! * `GET  /api/console/:vm_id/spice.vv` connection file for remote-viewer
//!   (display, audio playback/record, clipboard agent, USB redirection).
//!   QEMU VMs: QEMU's own SPICE server; LXD/Incus VMs: Horcrux's
//!   password-protected relay to the daemon's SPICE display.
//!
//! The VNC (noVNC) and serial console endpoints live with the other console
//! routes in main.rs; all of them read what QEMU was actually started with
//! (`vm::display::DisplayRuntime`).
use crate::console::{self, ConsoleInfo, ConsoleType};
use crate::error::ApiError;
use crate::vm::display::{virt_viewer_file, DisplayRuntime};
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use horcrux_common::{VmConfig, VmDisplay, VmHypervisor};
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/vms/:id/display", put(set_vm_display))
        .route("/api/console/:vm_id/spice", post(create_spice_console))
        .route("/api/console/:vm_id/spice.vv", get(get_spice_vv))
}

async fn set_vm_display(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
    Json(display): Json<VmDisplay>,
) -> Result<Json<VmConfig>, ApiError> {
    Ok(Json(state.vm_manager.set_display(&vm_id, display).await?))
}

async fn create_spice_console(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
) -> Result<Json<ConsoleInfo>, ApiError> {
    let vm = state.vm_manager.get_vm(&vm_id).await?;
    Ok(Json(
        state
            .console_manager
            .create_console(&vm, ConsoleType::Spice)
            .await?,
    ))
}

/// When SPICE listens on all interfaces the viewer should dial the host
/// this API was reached at; otherwise the listen address itself.
fn viewer_host(listen: &str, headers: &HeaderMap) -> String {
    match listen {
        "0.0.0.0" | "::" | "[::]" => headers
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .map(|h| h.rsplit_once(':').map(|(h, _)| h).unwrap_or(h).to_string())
            .unwrap_or_else(|| listen.to_string()),
        _ => listen.to_string(),
    }
}

/// `.vv` for an LXD/Incus VM: points at its SPICE relay.
#[cfg(any(feature = "lxd", feature = "incus"))]
fn instance_vv(vm: &VmConfig, info: &ConsoleInfo, headers: &HeaderMap) -> Result<String, ApiError> {
    let relay = console::spice_relay::RelayInfo {
        listen: info.host.clone(),
        port: info.port,
        password: info.password.clone().unwrap_or_default(),
    };
    Ok(console::spice_relay::virt_viewer_file(
        &vm.name,
        &viewer_host(&info.host, headers),
        &relay,
    ))
}

#[cfg(not(any(feature = "lxd", feature = "incus")))]
fn instance_vv(vm: &VmConfig, _: &ConsoleInfo, _: &HeaderMap) -> Result<String, ApiError> {
    Err(crate::features::hypervisor_not_compiled(&vm.hypervisor).into())
}

async fn get_spice_vv(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let vm = state.vm_manager.get_vm(&vm_id).await?;
    // Validates that SPICE is configured and the VM is running (clear 422
    // otherwise, same as the console endpoint).
    let info = state
        .console_manager
        .create_console(&vm, ConsoleType::Spice)
        .await?;
    let body = if vm.hypervisor == VmHypervisor::Qemu {
        let rt = DisplayRuntime::load(&console::runtime_disk(&vm)?).await?;
        let listen = rt.spice_listen.as_deref().unwrap_or("127.0.0.1");
        virt_viewer_file(&vm.name, &viewer_host(listen, &headers), &rt)
            .ok_or_else(|| ApiError::ValidationError("SPICE is not configured".into()))?
    } else {
        instance_vv(&vm, &info, &headers)?
    };
    Ok((
        [
            (
                header::CONTENT_TYPE,
                "application/x-virt-viewer".to_string(),
            ),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}.vv\"", vm.id),
            ),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        body,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewer_host_uses_request_host_only_for_wildcard_listen() {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, "pve.lan:8006".parse().unwrap());
        assert_eq!(viewer_host("0.0.0.0", &h), "pve.lan");
        assert_eq!(viewer_host("127.0.0.1", &h), "127.0.0.1");
        assert_eq!(viewer_host("10.0.0.5", &h), "10.0.0.5");
        assert_eq!(viewer_host("::", &HeaderMap::new()), "::");
    }
}
