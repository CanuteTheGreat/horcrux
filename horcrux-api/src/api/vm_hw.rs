//! VM hardware + power actions (Proxmox `qm set`, `qm shutdown|reboot|reset`)
//! and the ISO library (Proxmox ISO storage: list, download from URL, delete).
//!
//! - `GET  /api/vms/host-options`      bridges (+ VLAN awareness) and ISOs a VM
//!   form can offer
//! - `PUT  /api/vms/:id/config`        name / CPUs / memory / NICs / CD-ROM /
//!   boot order (CD-ROM also while running)
//! - `POST /api/vms/:id/shutdown`      ACPI shutdown, optional forced stop
//! - `POST /api/vms/:id/reboot`        shutdown + start
//! - `POST /api/vms/:id/reset`         hard reset
//! - `GET|POST /api/vms/:id/firewall/rules`, `DELETE .../:rule_id`: the
//!   VM's own firewall (bridge-family nftables chains jumped to from the
//!   VM's tap devices, so a VM rule only ever touches that VM's traffic)
//! - `GET  /api/storage/isos`, `POST /api/storage/isos/download`,
//!   `DELETE /api/storage/isos/:name`
//!
//! ISOs live in the image directory (`HORCRUX_IMAGE_DIR`, default
//! `<data_dir>/images`, shared with disk-image import) and in storage pool
//! directories (top level and Proxmox's `template/iso`). A VM's CD-ROM may
//! only point at a file there, so the API can't be used to attach arbitrary
//! host files to a guest.
use crate::api::vm_import::{allowed_local_source, image_dir};
use crate::error::ApiError;
use crate::vm::hardware::VmConfigUpdate;
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post, put},
    Json, Router,
};
use horcrux_common::{VmConfig, VmHardware};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/vms/host-options", get(host_options))
        .route("/api/vms/:id/config", put(update_config))
        .route("/api/vms/:id/shutdown", post(shutdown))
        .route("/api/vms/:id/reboot", post(reboot))
        .route("/api/vms/:id/reset", post(reset))
        .route(
            "/api/vms/:id/firewall/rules",
            get(list_vm_firewall).post(add_vm_firewall),
        )
        .route(
            "/api/vms/:id/firewall/rules/:rule_id",
            delete(delete_vm_firewall),
        )
        .route("/api/storage/isos", get(list_isos))
        .route("/api/storage/isos/download", post(download_iso))
        .route("/api/storage/isos/:name", delete(delete_iso))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HostBridge {
    pub name: String,
    pub vlan_aware: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IsoFile {
    pub name: String,
    /// Absolute path: what a VM's `hardware.cdrom` is set to.
    pub path: String,
    pub size: u64,
    /// Still being downloaded (`size` grows); can't be attached yet.
    #[serde(default)]
    pub downloading: bool,
    /// Last download error for this file name, if the download failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostOptions {
    pub bridges: Vec<HostBridge>,
    pub isos: Vec<IsoFile>,
    /// Where downloaded ISOs go.
    pub iso_dir: String,
}

/// Linux bridges on this host.
pub fn host_bridges() -> Vec<HostBridge> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir("/sys/class/net") {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Some(vlan_aware) = crate::vm::qemu::host_bridge(&name) {
                out.push(HostBridge { name, vlan_aware });
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Directories ISOs are listed from and may be attached from.
async fn iso_roots(state: &AppState) -> Vec<PathBuf> {
    let mut roots = vec![image_dir(&state.config.paths.data_dir)];
    for p in state.storage_manager.list_pools().await {
        if p.path.starts_with('/') {
            let base = PathBuf::from(&p.path);
            roots.push(base.join("template/iso"));
            roots.push(base);
        }
    }
    roots
}

fn is_iso_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".iso") || lower.ends_with(".img")
}

/// In-flight and failed downloads, keyed by target path.
fn downloads() -> &'static Mutex<HashMap<PathBuf, Option<String>>> {
    static D: OnceLock<Mutex<HashMap<PathBuf, Option<String>>>> = OnceLock::new();
    D.get_or_init(Default::default)
}

fn partial_path(target: &FsPath) -> PathBuf {
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    target.with_file_name(format!(".{name}.part"))
}

async fn scan_isos(state: &AppState) -> Vec<IsoFile> {
    let mut out: Vec<IsoFile> = Vec::new();
    for root in iso_roots(state).await {
        let Ok(rd) = std::fs::read_dir(&root) else {
            continue;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let path = e.path();
            if name.starts_with('.') || !is_iso_name(&name) || !path.is_file() {
                continue;
            }
            let path_s = path.to_string_lossy().to_string();
            if out.iter().any(|i| i.path == path_s) {
                continue;
            }
            out.push(IsoFile {
                name,
                path: path_s,
                size: e.metadata().map(|m| m.len()).unwrap_or(0),
                downloading: false,
                error: None,
            });
        }
    }
    // Downloads in progress (written to a hidden .part file) and failures.
    let state_now = downloads().lock().unwrap().clone();
    for (target, err) in state_now {
        let path_s = target.to_string_lossy().to_string();
        if out.iter().any(|i| i.path == path_s) {
            continue;
        }
        out.push(IsoFile {
            name: target
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            path: path_s,
            size: std::fs::metadata(partial_path(&target))
                .map(|m| m.len())
                .unwrap_or(0),
            downloading: err.is_none(),
            error: err,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// A host-side check of a VM's hardware: bridges exist (and are VLAN-aware
/// when a tag is set) and the CD-ROM ISO is a file in the ISO library.
pub async fn check_host_hardware(state: &AppState, hw: &VmHardware) -> Result<(), ApiError> {
    for (i, nic) in hw.nets.iter().enumerate() {
        match crate::vm::qemu::host_bridge(&nic.bridge) {
            None => {
                return Err(ApiError::BadRequest(format!(
                    "net{i}: bridge {} does not exist on this host (bridges: {})",
                    nic.bridge,
                    host_bridges()
                        .iter()
                        .map(|b| b.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )))
            }
            Some(false) if nic.vlan.is_some() => {
                return Err(ApiError::BadRequest(format!(
                    "net{i}: bridge {} is not VLAN-aware, so a VLAN tag can't be used on it",
                    nic.bridge
                )))
            }
            _ => {}
        }
    }
    if let Some(iso) = &hw.cdrom {
        let p = std::fs::canonicalize(iso)
            .map_err(|e| ApiError::BadRequest(format!("cdrom {iso}: {e}")))?;
        if !p.is_file() || !allowed_local_source(&p, &iso_roots(state).await) {
            return Err(ApiError::Forbidden(format!(
                "cdrom: ISOs must be files in the ISO library ({} or a storage pool directory)",
                image_dir(&state.config.paths.data_dir).display()
            )));
        }
    }
    Ok(())
}

async fn host_options(State(state): State<Arc<AppState>>) -> Json<HostOptions> {
    Json(HostOptions {
        bridges: host_bridges(),
        isos: scan_isos(&state).await,
        iso_dir: image_dir(&state.config.paths.data_dir)
            .to_string_lossy()
            .to_string(),
    })
}

async fn update_config(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(upd): Json<VmConfigUpdate>,
) -> Result<Json<VmConfig>, ApiError> {
    if let Some(hw) = &upd.hardware {
        check_host_hardware(&state, hw).await?;
    }
    Ok(Json(state.vm_manager.update_config(&id, upd).await?))
}

#[derive(Debug, Default, Deserialize)]
pub struct ShutdownRequest {
    /// Seconds to wait for the guest to power off (default 60, max 3600).
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Stop the VM if it hasn't powered off by then (default false).
    #[serde(default)]
    pub force_stop: bool,
}

fn timeout_of(t: Option<u64>) -> Duration {
    Duration::from_secs(t.unwrap_or(60).clamp(1, 3600))
}

async fn shutdown(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Option<Json<ShutdownRequest>>,
) -> Result<Json<VmConfig>, ApiError> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    let vm = state
        .vm_manager
        .shutdown_vm(&id, timeout_of(req.timeout), req.force_stop)
        .await?;
    state
        .ws_state
        .broadcast_vm_status(id, "Running".into(), format!("{:?}", vm.status));
    Ok(Json(vm))
}

async fn reboot(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Option<Json<ShutdownRequest>>,
) -> Result<Json<VmConfig>, ApiError> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    Ok(Json(
        state
            .vm_manager
            .reboot_vm(&id, timeout_of(req.timeout))
            .await?,
    ))
}

async fn reset(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<VmConfig>, ApiError> {
    Ok(Json(state.vm_manager.reset_vm(&id).await?))
}

async fn list_vm_firewall(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Vec<crate::firewall::FirewallRule>>, ApiError> {
    state.vm_manager.get_vm(&id).await?;
    Ok(Json(
        state
            .firewall_manager
            .list_rules(crate::firewall::FirewallScope::Vm(id))
            .await,
    ))
}

/// Add a rule to a VM's firewall; it is installed at once. `direction`
/// "in" = traffic to the VM, "out" = traffic from it. A missing id is
/// generated.
async fn add_vm_firewall(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(mut rule): Json<crate::firewall::FirewallRule>,
) -> Result<(StatusCode, Json<crate::firewall::FirewallRule>), ApiError> {
    state.vm_manager.get_vm(&id).await?;
    if rule.id.trim().is_empty() {
        rule.id = format!("fw-{}", uuid::Uuid::new_v4().simple());
    }
    if !rule
        .id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(ApiError::BadRequest(
            "rule id: letters, digits, - _ . only".into(),
        ));
    }
    state
        .firewall_manager
        .add_rule(crate::firewall::FirewallScope::Vm(id), rule.clone())
        .await?;
    Ok((StatusCode::CREATED, Json(rule)))
}

async fn delete_vm_firewall(
    State(state): State<Arc<AppState>>,
    Path((id, rule_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let scope = crate::firewall::FirewallScope::Vm(id);
    if !state
        .firewall_manager
        .list_rules(scope.clone())
        .await
        .iter()
        .any(|r| r.id == rule_id)
    {
        return Err(ApiError::NotFound(format!(
            "firewall rule {rule_id} not found"
        )));
    }
    state.firewall_manager.remove_rule(scope, &rule_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_isos(State(state): State<Arc<AppState>>) -> Json<Vec<IsoFile>> {
    Json(scan_isos(&state).await)
}

#[derive(Debug, Deserialize)]
pub struct DownloadRequest {
    pub url: String,
    /// File name in the ISO directory (default: the URL's last path part).
    #[serde(default)]
    pub filename: Option<String>,
}

/// A safe file name for the ISO directory, or why not.
pub fn iso_file_name(url: &str, filename: Option<&str>) -> Result<String, String> {
    let name = match filename.map(str::trim).filter(|f| !f.is_empty()) {
        Some(f) => f.to_string(),
        None => url
            .split(['?', '#'])
            .next()
            .unwrap_or_default()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string(),
    };
    if name.is_empty()
        || name.starts_with('.')
        || name.len() > 200
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'))
    {
        return Err(format!(
            "{name:?} is not a usable file name (letters, digits, - _ . + only)"
        ));
    }
    if !is_iso_name(&name) {
        return Err(format!("{name}: the file name must end in .iso or .img"));
    }
    Ok(name)
}

/// Start downloading an ISO into the ISO directory (`202 Accepted`); the
/// list shows it as downloading until it completes. Written to a hidden
/// `.part` file and renamed when complete, so a failed or interrupted
/// download never shows up as a usable ISO.
async fn download_iso(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DownloadRequest>,
) -> Result<(StatusCode, Json<IsoFile>), ApiError> {
    if !(req.url.starts_with("http://") || req.url.starts_with("https://")) {
        return Err(ApiError::BadRequest("url must be http(s)".into()));
    }
    let name = iso_file_name(&req.url, req.filename.as_deref()).map_err(ApiError::BadRequest)?;
    let dir = image_dir(&state.config.paths.data_dir);
    tokio::fs::create_dir_all(&dir).await?;
    let target = dir.join(&name);
    if target.exists() {
        return Err(ApiError::Conflict(format!("{name} already exists")));
    }
    {
        let mut d = downloads().lock().unwrap();
        if matches!(d.get(&target), Some(None)) {
            return Err(ApiError::Conflict(format!("{name} is already downloading")));
        }
        d.insert(target.clone(), None);
    }
    let url = req.url.clone();
    let t = target.clone();
    tokio::spawn(async move {
        let result = fetch(&url, &t).await;
        let mut d = downloads().lock().unwrap();
        match result {
            Ok(()) => {
                tracing::info!("ISO {} downloaded from {url}", t.display());
                d.remove(&t);
            }
            Err(e) => {
                tracing::warn!("ISO download {url} failed: {e}");
                d.insert(t, Some(e));
            }
        }
    });
    Ok((
        StatusCode::ACCEPTED,
        Json(IsoFile {
            name,
            path: target.to_string_lossy().to_string(),
            size: 0,
            downloading: true,
            error: None,
        }),
    ))
}

async fn fetch(url: &str, target: &FsPath) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let part = partial_path(target);
    let res: Result<(), String> = async {
        let mut resp = reqwest::Client::new()
            .get(url)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }
        let mut f = tokio::fs::File::create(&part)
            .await
            .map_err(|e| e.to_string())?;
        while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
            f.write_all(&chunk).await.map_err(|e| e.to_string())?;
        }
        f.flush().await.map_err(|e| e.to_string())?;
        f.sync_all().await.map_err(|e| e.to_string())?;
        tokio::fs::rename(&part, target)
            .await
            .map_err(|e| e.to_string())
    }
    .await;
    if res.is_err() {
        let _ = tokio::fs::remove_file(&part).await;
    }
    res
}

/// Delete an ISO from the ISO directory. Refused while a VM's CD-ROM holds
/// it. A failed download's entry is cleared the same way.
async fn delete_iso(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let dir = image_dir(&state.config.paths.data_dir);
    let target = dir.join(&name);
    if name.contains('/') || name.starts_with('.') || !is_iso_name(&name) {
        return Err(ApiError::BadRequest(format!("invalid ISO name {name:?}")));
    }
    {
        let mut d = downloads().lock().unwrap();
        match d.get(&target) {
            Some(None) => return Err(ApiError::Conflict(format!("{name} is still downloading"))),
            Some(Some(_)) => {
                d.remove(&target);
                return Ok(StatusCode::NO_CONTENT);
            }
            None => {}
        }
    }
    if !target.is_file() {
        return Err(ApiError::NotFound(format!("ISO {name} not found")));
    }
    let path_s = target.to_string_lossy().to_string();
    let users: Vec<String> = state
        .vm_manager
        .list_vms()
        .await
        .into_iter()
        .filter(|v| v.hardware.cdrom.as_deref() == Some(path_s.as_str()))
        .map(|v| format!("{} ({})", v.name, v.id))
        .collect();
    if !users.is_empty() {
        return Err(ApiError::Conflict(format!(
            "{name} is in the CD-ROM drive of {}; eject it first",
            users.join(", ")
        )));
    }
    tokio::fs::remove_file(&target).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_file_names_are_safe() {
        assert_eq!(
            iso_file_name("https://x.test/a/alpine-virt-3.20.iso?dl=1", None).unwrap(),
            "alpine-virt-3.20.iso"
        );
        assert_eq!(
            iso_file_name("https://x.test/download", Some("debian+12.iso")).unwrap(),
            "debian+12.iso"
        );
        assert!(iso_file_name("https://x.test/a/", None).is_err());
        assert!(iso_file_name("https://x.test/a.iso", Some("../x.iso")).is_err());
        assert!(iso_file_name("https://x.test/a.iso", Some(".hidden.iso")).is_err());
        assert!(iso_file_name("https://x.test/a.qcow2", None).is_err());
        assert!(iso_file_name("https://x.test/a b.iso", None).is_err());
    }

    #[test]
    fn partial_downloads_are_hidden_files() {
        assert_eq!(
            partial_path(FsPath::new("/d/x.iso")),
            PathBuf::from("/d/.x.iso.part")
        );
    }
}
