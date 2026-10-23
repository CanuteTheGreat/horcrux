//! Live VM devices: memory balloon, QEMU guest agent, USB passthrough.
//!
//! * `GET|POST /api/vms/:id/balloon`        actual memory / ask for a target (MB)
//! * `GET  /api/vms/:id/agent`              what the agent in the guest reports
//! * `POST /api/vms/:id/agent/:action`      ping | fsfreeze | fsthaw | fsfreeze-status
//!   | fstrim | shutdown | reboot | time
//! * `GET  /api/nodes/usb`                  host USB devices that can be passed through
//! * `POST /api/vms/:id/hotplug/cpu {cpus}` / `.../hotplug/memory {add_mb}`  grow a running VM
//! * `POST /api/vms/:id/usb`                add a host device (hot-plug when running)
//! * `DELETE /api/vms/:id/usb/:vendor/:product`
//!
//! The balloon device, agent channel and USB controller are VM hardware
//! (`PUT /api/vms/:id/config`, `balloon` / `guest_agent` / `usb_controller` /
//! `usb`): they exist from the VM's next start.
use crate::error::ApiError;
use crate::vm::devices::{AgentInfo, BalloonInfo, HostUsbDevice};
use crate::AppState;
use axum::{
    extract::{Path, State},
    routing::{delete, get, post},
    Json, Router,
};
use horcrux_common::{Error, UsbDevice, VmConfig};
use serde::Deserialize;
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/vms/:id/balloon", get(get_balloon).post(set_balloon))
        .route("/api/vms/:id/agent", get(get_agent))
        .route("/api/vms/:id/agent/:action", post(agent_action))
        .route("/api/nodes/usb", get(host_usb))
        .route("/api/vms/:id/usb", post(attach_usb))
        .route("/api/vms/:id/usb/:vendor/:product", delete(detach_usb))
        .route("/api/vms/:id/hotplug/cpu", post(hotplug_cpu))
        .route("/api/vms/:id/hotplug/memory", post(hotplug_memory))
}

#[derive(Deserialize)]
struct HotplugCpu {
    /// Total CPUs wanted (above the current count, up to max CPUs).
    cpus: u32,
}

#[derive(Deserialize)]
struct HotplugMemory {
    /// MB to add, in multiples of 128.
    add_mb: u64,
}

/// Add CPUs to a running VM (the new count is stored).
async fn hotplug_cpu(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(r): Json<HotplugCpu>,
) -> Result<Json<VmConfig>, ApiError> {
    Ok(Json(state.vm_manager.hotplug_cpus(&id, r.cpus).await?))
}

/// Add a memory DIMM to a running VM (the new size is stored).
async fn hotplug_memory(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(r): Json<HotplugMemory>,
) -> Result<Json<VmConfig>, ApiError> {
    Ok(Json(state.vm_manager.hotplug_memory(&id, r.add_mb).await?))
}

/// The guest agent is a separate program in the guest: when it is not
/// there, that is "service unavailable", not a server fault.
fn agent_err(e: Error) -> ApiError {
    match e {
        Error::System(m) => ApiError::ServiceUnavailable(m),
        o => o.into(),
    }
}

async fn get_balloon(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<BalloonInfo>, ApiError> {
    Ok(Json(state.vm_manager.balloon_info(&id).await?))
}

#[derive(Deserialize)]
struct BalloonRequest {
    target_mb: u64,
}

async fn set_balloon(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(r): Json<BalloonRequest>,
) -> Result<Json<BalloonInfo>, ApiError> {
    Ok(Json(state.vm_manager.set_balloon(&id, r.target_mb).await?))
}

async fn get_agent(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<AgentInfo>, ApiError> {
    Ok(Json(state.vm_manager.agent_info(&id).await?))
}

async fn agent_action(
    State(state): State<Arc<AppState>>,
    Path((id, action)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .vm_manager
        .agent_action(&id, &action)
        .await
        .map(Json)
        .map_err(agent_err)
}

async fn host_usb() -> Json<Vec<HostUsbDevice>> {
    Json(crate::vm::devices::host_usb_devices())
}

async fn attach_usb(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(d): Json<UsbDevice>,
) -> Result<Json<VmConfig>, ApiError> {
    Ok(Json(state.vm_manager.attach_usb(&id, d).await?))
}

async fn detach_usb(
    State(state): State<Arc<AppState>>,
    Path((id, vendor, product)): Path<(String, String, String)>,
) -> Result<Json<VmConfig>, ApiError> {
    Ok(Json(
        state
            .vm_manager
            .detach_usb(
                &id,
                UsbDevice {
                    vendor_id: vendor,
                    product_id: product,
                },
            )
            .await?,
    ))
}
