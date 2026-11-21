//! Gaming VM wizard (docs/design/gaming-vm.md, phase 2).
//!
//! * `POST /api/vms/gaming`            compose a Windows gaming VM from the host check
//!   and the wizard's answers, create it as an ordinary VM, write the answer-file disc
//!   and (with `start`) start the unattended install. `dry_run` returns the plan only.
//! * `GET  /api/vms/gaming/compat`     the per-game anti-cheat table
//!   (`passthrough.gaming_compat_files`; `?q=title` matches one title)
//! * `GET  /api/vms/gaming/settings`   wizard defaults (memory, disk, SMBIOS strings,
//!   Windows locale, download pages, ...); `PUT` changes them (admins)
//!
//! The answer-file disc holds the Windows account password: it is 0600, lives
//! next to the VM's first disk and is deleted as soon as the installed
//! guest's agent answers (checked after create/start, every
//! `unattend_poll_secs`).
use crate::error::ApiError;
use crate::vm::gaming::{
    self, CompatEntry, GamingPlan, GamingSettings, GamingVmRequest, HostFacts,
};
use crate::AppState;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use horcrux_common::{Error, VmConfig};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/vms/gaming", post(create))
        .route("/api/vms/gaming/compat", get(compat))
        .route(
            "/api/vms/gaming/settings",
            get(get_settings).put(put_settings),
        )
}

/// Config-store kind of the wizard settings.
const SETTINGS_KIND: &str = "gaming_settings";

pub async fn load_settings(state: &AppState) -> GamingSettings {
    crate::db::config_store::ConfigStore::new(&state.database)
        .load_values::<GamingSettings>(SETTINGS_KIND)
        .await
        .ok()
        .and_then(|v| v.into_iter().next())
        .unwrap_or_default()
}

async fn get_settings(State(state): State<Arc<AppState>>) -> Json<GamingSettings> {
    Json(load_settings(&state).await)
}

async fn put_settings(
    State(state): State<Arc<AppState>>,
    Json(s): Json<GamingSettings>,
) -> Result<Json<GamingSettings>, ApiError> {
    s.validate().map_err(ApiError::BadRequest)?;
    crate::db::config_store::ConfigStore::new(&state.database)
        .put(SETTINGS_KIND, "default", &s)
        .await?;
    Ok(Json(s))
}

fn compat_rows() -> Vec<CompatEntry> {
    gaming::load_compat(&crate::vm::tuning::config().gaming_compat_files)
}

#[derive(Deserialize)]
struct CompatQuery {
    q: Option<String>,
}

#[derive(Serialize)]
struct CompatView {
    /// Files read, in order (later rows replace earlier ones).
    files: Vec<String>,
    games: Vec<CompatEntry>,
}

async fn compat(Query(q): Query<CompatQuery>) -> Result<Json<CompatView>, ApiError> {
    let rows = tokio::task::spawn_blocking(compat_rows)
        .await
        .map_err(|e| Error::System(format!("compat table: {e}")))?;
    let games = match q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(t) => gaming::find_game(&rows, t).cloned().into_iter().collect(),
        None => rows,
    };
    Ok(Json(CompatView {
        files: crate::vm::tuning::config()
            .gaming_compat_files
            .iter()
            .filter(|p| p.exists())
            .map(|p| p.display().to_string())
            .collect(),
        games,
    }))
}

fn host_memory_mb() -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse::<u64>().ok())
        })
        .map(|kb| kb / 1024)
        .unwrap_or(0)
}

#[derive(Serialize)]
struct Created {
    #[serde(flatten)]
    plan: GamingPlan,
    /// Created and (with `start`) started; false for a dry run.
    created: bool,
    /// The answer-file disc attached to the VM.
    unattend_iso: Option<String>,
}

async fn create(
    State(state): State<Arc<AppState>>,
    Json(req): Json<GamingVmRequest>,
) -> Result<(StatusCode, Json<Created>), ApiError> {
    let settings = load_settings(&state).await;
    let (report, compat) = tokio::task::spawn_blocking(|| {
        let quirks = crate::vm::handoff::load_quirks(&crate::vm::tuning::config().quirks_files);
        (
            crate::vm::handoff::host_report(&crate::vm::handoff::Host::default(), &quirks),
            compat_rows(),
        )
    })
    .await
    .map_err(|e| Error::System(format!("gaming check: {e}")))?;
    let default_bridge = crate::api::vm_hw::default_vm_network(&state).await;
    let sys = std::path::Path::new("/sys/class/net");
    let lan = |b: &str| !crate::vm::remote_play::bridge_uplinks(sys, b).is_empty();
    let lan_bridge = default_bridge.clone().filter(|b| lan(b)).or_else(|| {
        crate::api::vm_hw::host_bridges()
            .into_iter()
            .map(|b| b.name)
            .find(|b| lan(b))
    });
    let host = HostFacts {
        memory_mb: host_memory_mb(),
        default_bridge,
        lan_bridge,
        macvtap_uplink: crate::api::vm_hw::macvtap_uplinks().into_iter().next(),
    };
    let mut plan = gaming::compose(&req, &report, &host, &settings, &compat)?;
    // ISOs from the library, bridges that exist, firmware/TPM present
    crate::api::vm_hw::check_host_hardware(&state, &plan.config.hardware).await?;
    if req.dry_run {
        return Ok((
            StatusCode::OK,
            Json(Created {
                plan,
                created: false,
                unattend_iso: None,
            }),
        ));
    }
    crate::api::storage_volumes::place_vm_disks(&state, &mut plan.config).await?;
    let vm: VmConfig = state
        .vm_manager
        .create_vm_from_image(plan.config.clone(), None)
        .await?;
    let disk0 = vm
        .disks
        .first()
        .map(|d| PathBuf::from(&d.path))
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| Error::System("the new VM has no disk path".into()))?;
    let iso = crate::vm::qemu::unattend_iso(&disk0);
    if let Err(e) = gaming::build_unattend_iso(&iso, &gaming::unattend_xml(&plan.unattend)).await {
        // no half-made gaming VM
        let _ = state.vm_manager.delete_vm(&vm.id).await;
        return Err(e.into());
    }
    tracing::info!("gaming VM {} created ({})", vm.id, vm.name);
    if vm.hardware.adv.remote_play.is_some() {
        crate::api::remote_play::kick();
    }
    plan.config = vm.clone();
    if req.start {
        state.vm_manager.start_vm(&vm.id).await?;
        let boot_key = settings.boot_key_secs;
        let d = disk0.clone();
        tokio::spawn(async move { gaming::press_boot_key(&d, boot_key).await });
    }
    spawn_unattend_cleanup(
        state.clone(),
        vm.id.clone(),
        disk0,
        settings.unattend_poll_secs,
    );
    Ok((
        StatusCode::CREATED,
        Json(Created {
            plan,
            created: true,
            unattend_iso: Some(iso.display().to_string()),
        }),
    ))
}

/// Delete the answer-file disc once the installed Windows' guest agent
/// answers (it only exists while the install runs). Also called when such a
/// VM is started again after a restart of the server.
pub fn spawn_unattend_cleanup(state: Arc<AppState>, vm_id: String, disk0: PathBuf, poll: u64) {
    tokio::spawn(async move {
        let poll = std::time::Duration::from_secs(poll.max(5));
        loop {
            tokio::time::sleep(poll).await;
            let iso = crate::vm::qemu::unattend_iso(&disk0);
            if !iso.exists() {
                return;
            }
            match state.vm_manager.get_vm(&vm_id).await {
                Ok(vm) if vm.status == horcrux_common::VmStatus::Running => {}
                // stopped or gone: picked up again at the next start
                _ => return,
            }
            if gaming::guest_agent_answers(&disk0).await {
                remove_unattend(&iso);
                tracing::info!("gaming VM {vm_id}: Windows is installed, answer disc removed");
                return;
            }
        }
    });
}

fn remove_unattend(iso: &Path) {
    if let Err(e) = std::fs::remove_file(iso) {
        tracing::warn!("removing {}: {e}", iso.display());
    }
}
