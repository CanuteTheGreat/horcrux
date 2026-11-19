//! Hyper-V-style "Automatic stop action" / "Automatic start action" for VMs
//! and containers, with real save-to-disk of running guests.
//!
//! - **Policy** (`policy.rs`): a node default plus per-guest overrides: stop
//!   action Save state | Shut down | Power off, start action Resume if it was
//!   running | Always start | Nothing, start delay and order, and the
//!   storage saved states go to.
//! - **Save / resume** (`engine.rs`): QEMU VMs are saved with a QMP
//!   `migrate` to a file (vm/savestate.rs), containers with CRIU (LXC, Incus,
//!   LXD, Docker, Podman: container/savestate.rs). A saved state is recorded
//!   in an index file; it is deleted after a successful resume and kept (with
//!   the error) when a resume fails, so it is never resumed twice.
//! - **Host shutdown / boot** (`engine.rs`, `agent.rs`): `horcrux-api guests
//!   stop` (the systemd/OpenRC unit's stop path) saves every running guest
//!   in parallel (bounded, per-guest timeout, falling back to shut down /
//!   power off); `horcrux-api guests boot` (or the API itself at startup)
//!   resumes saved guests in start order with their delays.
//! - HTTP routes (below) for the UI, the CLI and automation.
pub mod agent;
pub mod crypt;
pub mod engine;
pub mod policy;

use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
pub use engine::{GuestKind, SavedGuest};
pub use policy::{
    Effective, GuestOverride, NodePolicy, SaveEncryption, SaveFallback, StartAction, StopAction,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Protected routes (merged with the other protected routes in app/mod.rs).
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/nodes/guest-power", get(overview).put(put_node_policy))
        .route(
            "/api/nodes/guest-power/boot-guests",
            post(admin_boot_guests),
        )
        .route(
            "/api/vms/:id/power-policy",
            get(vm_get_policy).put(vm_put_policy),
        )
        .route("/api/vms/:id/save-state", post(vm_save))
        .route("/api/vms/:id/resume-state", post(vm_resume))
        .route("/api/vms/:id/saved-state", get(vm_saved).delete(vm_discard))
        .route(
            "/api/containers/:id/power-policy",
            get(ct_get_policy).put(ct_put_policy),
        )
        .route("/api/containers/:id/save-state", post(ct_save))
        .route("/api/containers/:id/resume-state", post(ct_resume))
        .route(
            "/api/containers/:id/saved-state",
            get(ct_saved).delete(ct_discard),
        )
}

/// Everything the UI's power page needs in one call.
#[derive(Serialize)]
pub struct Overview {
    pub node: NodePolicy,
    pub capabilities: engine::Capabilities,
    pub guests: Vec<engine::GuestView>,
    /// What the last host shutdown did to each guest.
    pub last_stop: Option<engine::Manifest>,
    /// What the node's default save directory is stored on (saved memory
    /// may only go to encrypted storage unless the node allows otherwise).
    pub save_target: crypt::Probe,
    /// Worst-case seconds the host stop pass takes with the guests running
    /// now, from the configured timeouts.
    pub stop_budget_secs: u64,
    /// Set when an init system's own shutdown limit is shorter than the
    /// budget (the stop pass could be cut short).
    pub stop_budget_warning: Option<String>,
}

/// systemd ends a shutdown/reboot job after 30 minutes by default
/// (`JobTimeoutSec=30min` in poweroff/reboot/halt.target); OpenRC has none.
pub const SYSTEMD_SHUTDOWN_LIMIT_SECS: u64 = 1800;

pub fn budget_warning(budget: u64, systemd: bool) -> Option<String> {
    (systemd && budget > SYSTEMD_SHUTDOWN_LIMIT_SECS).then(|| {
        format!(
            "The worst-case time to save/stop all guests ({budget} s) is longer than systemd's \
             shutdown limit ({SYSTEMD_SHUTDOWN_LIMIT_SECS} s, JobTimeoutSec of the shutdown \
             targets): lower the save/shutdown timeouts or raise the parallelism, or raise that \
             limit with a systemd drop-in"
        )
    })
}

async fn overview(State(state): State<Arc<AppState>>) -> Result<Json<Overview>, ApiError> {
    let node = policy::load_node(&state).await;
    let dir = node
        .save_dir
        .as_deref()
        .filter(|d| !d.trim().is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| policy::default_save_dir(&state));
    let budget = engine::stop_budget(&state).await;
    Ok(Json(Overview {
        save_target: crypt::probe(&dir).await,
        stop_budget_secs: budget,
        stop_budget_warning: budget_warning(
            budget,
            std::path::Path::new("/run/systemd/system").is_dir(),
        ),
        node,
        capabilities: engine::capabilities().await,
        guests: engine::guest_views(&state).await,
        last_stop: engine::read_manifest(&state),
    }))
}

async fn put_node_policy(
    State(state): State<Arc<AppState>>,
    Json(p): Json<NodePolicy>,
) -> Result<Json<NodePolicy>, ApiError> {
    p.validate().map_err(ApiError::ValidationError)?;
    policy::save_node(&state, &p).await?;
    Ok(Json(p))
}

async fn admin_boot_guests(
    State(state): State<Arc<AppState>>,
) -> Result<Json<engine::Report>, ApiError> {
    Ok(Json(engine::boot_all(&state, true).await))
}

#[derive(Serialize)]
pub struct PolicyView {
    #[serde(rename = "override")]
    pub overrides: GuestOverride,
    pub effective: Effective,
    /// What the host shutdown will really do (save | shutdown | poweroff |
    /// none) and why it differs from the configured action.
    pub host_stop_action: String,
    pub host_stop_reason: Option<String>,
}

async fn get_policy(state: &Arc<AppState>, k: GuestKind, id: &str) -> Result<PolicyView, ApiError> {
    engine::require_guest(state, k, id).await?;
    let v = engine::guest_view(state, k, id).await?;
    Ok(PolicyView {
        overrides: policy::load_override(state, k, id).await,
        effective: policy::effective(state, k, id).await,
        host_stop_action: v.host_stop_action,
        host_stop_reason: v.host_stop_reason,
    })
}

async fn put_policy(
    state: &Arc<AppState>,
    k: GuestKind,
    id: &str,
    o: GuestOverride,
) -> Result<PolicyView, ApiError> {
    engine::require_guest(state, k, id).await?;
    o.validate().map_err(ApiError::ValidationError)?;
    policy::save_override(state, k, id, &o).await?;
    // Incus/LXD instances need migration.stateful before a stateful stop.
    if o.stop_action == Some(StopAction::Save) || o.stop_action.is_none() {
        engine::prepare_stateful(state, k, id).await;
    }
    get_policy(state, k, id).await
}

#[derive(Debug, Default, Deserialize)]
pub struct SaveRequest {
    /// Storage directory for this save only (default: the guest's policy).
    #[serde(default)]
    pub save_dir: Option<String>,
}

macro_rules! guest_handlers {
    ($kind:expr, $get:ident, $put:ident, $save:ident, $resume:ident, $saved:ident, $discard:ident) => {
        async fn $get(
            State(s): State<Arc<AppState>>,
            Path(id): Path<String>,
        ) -> Result<Json<PolicyView>, ApiError> {
            Ok(Json(get_policy(&s, $kind, &id).await?))
        }
        async fn $put(
            State(s): State<Arc<AppState>>,
            Path(id): Path<String>,
            Json(o): Json<GuestOverride>,
        ) -> Result<Json<PolicyView>, ApiError> {
            Ok(Json(put_policy(&s, $kind, &id, o).await?))
        }
        async fn $save(
            State(s): State<Arc<AppState>>,
            Path(id): Path<String>,
            body: Option<Json<SaveRequest>>,
        ) -> Result<Json<SavedGuest>, ApiError> {
            let req = body.map(|Json(b)| b).unwrap_or_default();
            Ok(Json(engine::save_guest(&s, $kind, &id, req.save_dir).await?))
        }
        async fn $resume(
            State(s): State<Arc<AppState>>,
            Path(id): Path<String>,
        ) -> Result<Json<engine::GuestView>, ApiError> {
            engine::resume_guest(&s, $kind, &id).await?;
            Ok(Json(engine::guest_view(&s, $kind, &id).await?))
        }
        async fn $saved(
            State(s): State<Arc<AppState>>,
            Path(id): Path<String>,
        ) -> Result<Json<SavedGuest>, ApiError> {
            engine::require_guest(&s, $kind, &id).await?;
            engine::load_saved(&s, $kind, &id)
                .map(Json)
                .ok_or_else(|| ApiError::NotFound(format!("{} has no saved state", id)))
        }
        async fn $discard(
            State(s): State<Arc<AppState>>,
            Path(id): Path<String>,
        ) -> Result<Json<serde_json::Value>, ApiError> {
            engine::discard_guest(&s, $kind, &id).await?;
            Ok(Json(serde_json::json!({"discarded": id})))
        }
    };
}

guest_handlers!(
    GuestKind::Vm,
    vm_get_policy,
    vm_put_policy,
    vm_save,
    vm_resume,
    vm_saved,
    vm_discard
);
guest_handlers!(
    GuestKind::Container,
    ct_get_policy,
    ct_put_policy,
    ct_save,
    ct_resume,
    ct_saved,
    ct_discard
);
