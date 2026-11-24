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
use crate::api::vm_import::{
    allowed_local_source, download_client, error_chain, image_dir, refused_url,
};
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
        .route(
            "/api/vms/defaults",
            get(get_vm_defaults).put(put_vm_defaults),
        )
        .route("/api/vms/:id/config", put(update_config))
        .route("/api/vms/:id/tpm", get(tpm_status))
        .route("/api/vms/:id/tpm/reset", post(tpm_reset))
        .route("/api/vms/:id/tpm/import-state", post(tpm_import_state))
        .route("/api/vms/:id/shutdown", post(shutdown))
        .route("/api/vms/:id/reboot", post(reboot))
        .route("/api/vms/:id/reset", post(reset))
        .route("/api/vms/:id/pause", post(pause))
        .route("/api/vms/:id/resume", post(resume))
        .route("/api/vms/:id/disks", post(add_disk))
        .route(
            "/api/vms/:id/disks/:index",
            put(update_disk).delete(remove_disk),
        )
        .route(
            "/api/vms/:id/firewall/rules",
            get(list_vm_firewall).post(add_vm_firewall),
        )
        .route(
            "/api/vms/:id/firewall/rules/:rule_id",
            delete(delete_vm_firewall),
        )
        .route(
            "/api/containers/:id/firewall/rules",
            get(list_ct_firewall).post(add_ct_firewall),
        )
        .route(
            "/api/containers/:id/firewall/rules/:rule_id",
            delete(delete_ct_firewall),
        )
        .route("/api/storage/isos", get(list_isos))
        .route("/api/storage/isos/download", post(download_iso))
        .route(
            "/api/storage/isos/upload",
            post(upload_iso).layer(axum::extract::DefaultBodyLimit::disable()),
        )
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
    /// Storage pool holding the file (`""`: the node's image library).
    #[serde(default)]
    pub storage: String,
    #[serde(default)]
    pub storage_name: String,
}

/// A place an ISO can be picked from, uploaded to or downloaded into.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IsoStorage {
    /// Storage pool id; `""` is the node's image library (`iso_dir`).
    pub id: String,
    pub name: String,
    /// Directory ISOs go to.
    pub dir: String,
    /// GB free on its file system.
    pub available: u64,
}

/// What this host can run: bounds for the Create VM sliders.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HostCapabilities {
    /// Logical CPUs.
    pub cpus: u32,
    /// Installed RAM (MB).
    pub memory_mb: u64,
    /// RAM kept for the host itself (MB).
    pub memory_reserve_mb: u64,
    /// Most RAM one VM may be given (`memory_mb - memory_reserve_mb`).
    pub max_vm_memory_mb: u64,
    /// GB free where VM disks go when no storage pool is chosen.
    pub default_disk_available_gb: u64,
}

/// Networks a new VM's first NIC attaches to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct DefaultNetworks {
    /// The QEMU hypervisor's network: a Linux bridge.
    pub qemu: Option<String>,
    pub incus: Option<String>,
    pub lxd: Option<String>,
    /// Global override (Settings): used instead of the hypervisor's own.
    #[serde(rename = "override")]
    pub override_network: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostOptions {
    pub bridges: Vec<HostBridge>,
    pub isos: Vec<IsoFile>,
    /// Where downloaded ISOs go.
    pub iso_dir: String,
    /// UEFI firmware (OVMF) is installed: VMs can boot with UEFI.
    #[serde(default)]
    pub ovmf: bool,
    /// swtpm is installed: VMs can have a TPM 2.0.
    #[serde(default)]
    pub swtpm: bool,
    /// KVM acceleration is available (CPU type "host" needs it).
    #[serde(default)]
    pub kvm: bool,
    /// Storage pools a VM disk can be created in (Create VM / Add disk).
    #[serde(default)]
    pub storage_pools: Vec<crate::api::storage_volumes::VmDiskPool>,
    /// Storages ISOs can be picked from / added to (always at least the
    /// image library).
    #[serde(default)]
    pub iso_storages: Vec<IsoStorage>,
    /// Storage ISOs go to unless another is chosen.
    #[serde(default)]
    pub default_iso_storage: String,
    pub host: HostCapabilities,
    #[serde(default)]
    pub default_networks: DefaultNetworks,
    /// Wired host interfaces a NIC can use directly as macvtap (no bridge).
    #[serde(default)]
    pub macvtap_uplinks: Vec<String>,
}

/// Global VM defaults (Settings): `GET|PUT /api/vms/defaults`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct VmDefaults {
    /// Network (bridge) every new VM's first NIC attaches to instead of the
    /// hypervisor's own default; empty/absent = per hypervisor.
    #[serde(default)]
    pub default_network: Option<String>,
}

const VM_DEFAULTS_KIND: &str = "vm_defaults";

/// The network a new QEMU VM gets by default: the global default network if
/// set (and present), else the hypervisor's own (vmbr0, else the bridge the
/// host reaches the network through). Same rule as the create form.
pub async fn default_vm_network(state: &AppState) -> Option<String> {
    let bridges = vm_bridges();
    load_vm_defaults(state)
        .await
        .default_network
        .filter(|n| bridges.iter().any(|b| &b.name == n))
        .or_else(|| default_qemu_bridge(&bridges, default_route_dev().as_deref()))
}

pub async fn load_vm_defaults(state: &AppState) -> VmDefaults {
    crate::db::config_store::ConfigStore::new(&state.database)
        .load_values::<VmDefaults>(VM_DEFAULTS_KIND)
        .await
        .ok()
        .and_then(|v| v.into_iter().next())
        .unwrap_or_default()
}

async fn get_vm_defaults(State(state): State<Arc<AppState>>) -> Json<VmDefaults> {
    Json(load_vm_defaults(&state).await)
}

async fn put_vm_defaults(
    State(state): State<Arc<AppState>>,
    Json(mut req): Json<VmDefaults>,
) -> Result<Json<VmDefaults>, ApiError> {
    req.default_network = req
        .default_network
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty());
    if let Some(n) = &req.default_network {
        if crate::vm::qemu::HostNet::system()
            .bridge_vlan_aware(n)
            .is_none()
        {
            return Err(ApiError::BadRequest(format!(
                "default network {n}: no such bridge on this host (bridges: {})",
                host_bridges()
                    .iter()
                    .map(|b| b.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    }
    crate::db::config_store::ConfigStore::new(&state.database)
        .put(VM_DEFAULTS_KIND, "default", &req)
        .await?;
    Ok(Json(req))
}

/// Logical CPUs, RAM and reserve of this host.
pub fn host_capabilities(default_disk_available_gb: u64) -> HostCapabilities {
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1);
    let memory_mb = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|m| {
            m.lines()
                .find_map(|l| l.strip_prefix("MemTotal:"))
                .and_then(|r| {
                    r.split_whitespace()
                        .next()
                        .and_then(|k| k.parse::<u64>().ok())
                })
        })
        .map(|kb| kb / 1024)
        .unwrap_or(0);
    let memory_reserve_mb = memory_reserve_mb(memory_mb);
    HostCapabilities {
        cpus,
        memory_mb,
        memory_reserve_mb,
        max_vm_memory_mb: memory_mb.saturating_sub(memory_reserve_mb),
        default_disk_available_gb,
    }
}

/// RAM left to the host: 1 GB, or 10% on big hosts.
pub fn memory_reserve_mb(total_mb: u64) -> u64 {
    (total_mb / 10).max(1024).min(total_mb / 2)
}

/// The Linux bridge new QEMU VMs attach to: `vmbr0`, else the bridge the
/// default route leaves through, else a general-purpose bridge (not one
/// that belongs to a container runtime).
pub fn default_qemu_bridge(bridges: &[HostBridge], route_dev: Option<&str>) -> Option<String> {
    if let Some(b) = bridges.iter().find(|b| b.name == "vmbr0") {
        return Some(b.name.clone());
    }
    if let Some(dev) = route_dev {
        if let Some(b) = bridges.iter().find(|b| b.name == dev) {
            return Some(b.name.clone());
        }
        // route leaves through a bridge port
        if let Ok(master) = std::fs::read_link(format!("/sys/class/net/{dev}/master")) {
            let m = master
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if bridges.iter().any(|b| b.name == m) {
                return Some(m);
            }
        }
    }
    let runtime = |n: &str| {
        crate::container::probe::is_container_runtime_bridge(n)
            || ["incusbr", "lxdbr", "lxcbr"]
                .iter()
                .any(|p| n.starts_with(p))
    };
    bridges
        .iter()
        .find(|b| !runtime(&b.name))
        .map(|b| b.name.clone())
}

/// Interface of the IPv4 default route.
fn default_route_dev() -> Option<String> {
    let t = std::fs::read_to_string("/proc/net/route").ok()?;
    t.lines().skip(1).find_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        (f.len() > 2 && f[1] == "00000000" && f[2] != "00000000").then(|| f[0].to_string())
    })
}

/// Whether `program` is an executable in PATH.
pub(crate) fn in_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|d| {
                use std::os::unix::fs::PermissionsExt;
                std::fs::metadata(d.join(program))
                    .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Firmware/TPM settings this host can't run: refused when saved rather
/// than failing at the next start.
pub fn check_host_firmware(hw: &VmHardware) -> Result<(), ApiError> {
    if hw.firmware == horcrux_common::Firmware::Ovmf && crate::vm::firmware::find_ovmf().is_none() {
        return Err(ApiError::BadRequest(
            crate::vm::firmware::ovmf_missing().to_string(),
        ));
    }
    if hw.secure_boot && crate::vm::firmware::find_ovmf_for(true).is_none() {
        return Err(ApiError::BadRequest(
            crate::vm::firmware::ovmf_secboot_missing().to_string(),
        ));
    }
    if hw.tpm && !in_path("swtpm") {
        return Err(ApiError::BadRequest(
            "A TPM needs swtpm, which is not installed on this host (app-crypt/swtpm; \
             Debian/Ubuntu: swtpm)"
                .into(),
        ));
    }
    Ok(())
}

/// Directories a hookscript may live in (storage pools' `snippets`, and the
/// node's own) and directories a virtiofs share may come from (pool paths).
async fn advanced_roots(state: &AppState) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut snippets = vec![PathBuf::from(&state.config.paths.data_dir).join("snippets")];
    let mut pools = Vec::new();
    for p in state.storage_manager.list_pools().await {
        if p.path.starts_with('/') {
            snippets.push(PathBuf::from(&p.path).join("snippets"));
            pools.push(PathBuf::from(&p.path));
        }
    }
    (snippets, pools)
}

/// Host-side checks of the advanced hardware: tty exists, virtiofs shares
/// come from a storage pool directory and virtiofsd is installed, the
/// hookscript is an executable snippet (so VM.Config can't run arbitrary
/// host programs).
pub async fn check_host_advanced(state: &AppState, hw: &VmHardware) -> Result<(), ApiError> {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    let adv = &hw.adv;
    for p in &adv.serial {
        if let Some(d) = &p.device {
            let ok = std::fs::metadata(d)
                .map(|m| m.file_type().is_char_device())
                .unwrap_or(false);
            if !ok {
                return Err(ApiError::BadRequest(format!(
                    "serial device {d} does not exist on this host"
                )));
            }
        }
    }
    if adv.virtiofs.is_empty() && adv.hookscript.is_none() {
        return Ok(());
    }
    let (snippets, pools) = advanced_roots(state).await;
    for v in &adv.virtiofs {
        let real = std::fs::canonicalize(&v.path).map_err(|_| {
            ApiError::BadRequest(format!(
                "virtiofs share {}: {} is not a directory on this host",
                v.tag, v.path
            ))
        })?;
        let inside = pools
            .iter()
            .filter_map(|p| std::fs::canonicalize(p).ok())
            .any(|p| real.starts_with(&p));
        if !real.is_dir() || !inside {
            return Err(ApiError::BadRequest(format!(
                "virtiofs share {}: {} must be a directory inside a storage pool",
                v.tag, v.path
            )));
        }
    }
    if !adv.virtiofs.is_empty() && crate::vm::advanced::virtiofsd_binary().is_none() {
        return Err(ApiError::BadRequest(
            "virtiofs needs virtiofsd, which is not installed on this host (Debian/Ubuntu: \
             virtiofsd; Gentoo: app-emulation/virtiofsd)"
                .into(),
        ));
    }
    if let Some(h) = &adv.hookscript {
        let real = std::fs::canonicalize(h)
            .map_err(|_| ApiError::BadRequest(format!("hookscript {h} does not exist")))?;
        let inside = snippets
            .iter()
            .filter_map(|p| std::fs::canonicalize(p).ok())
            .any(|p| real.starts_with(&p));
        if !inside {
            return Err(ApiError::BadRequest(format!(
                "hookscript {h} must be a snippet (upload it to a storage pool's snippets \
                 content type)"
            )));
        }
        let mode = std::fs::metadata(&real)
            .map(|m| m.permissions().mode())
            .unwrap_or(0);
        if !real.is_file() || mode & 0o100 == 0 {
            return Err(ApiError::BadRequest(format!(
                "hookscript {h} must be an executable file"
            )));
        }
    }
    Ok(())
}

/// Bridges a VM NIC may be put on: every host bridge except the ones a
/// container runtime made for itself (docker0, podman0, cni0, br-<hash>).
pub fn vm_bridges() -> Vec<HostBridge> {
    vm_bridges_of(host_bridges())
}

pub fn vm_bridges_of(bridges: Vec<HostBridge>) -> Vec<HostBridge> {
    bridges
        .into_iter()
        .filter(|b| !crate::container::probe::is_container_runtime_bridge(&b.name))
        .collect()
}

/// Host interfaces a VM NIC can sit on directly as macvtap (wired, not a
/// bridge or a bridge port), e.g. the uplink of a desktop or laptop host
/// whose own network must not be moved onto a bridge.
pub fn macvtap_uplinks() -> Vec<String> {
    let host = crate::vm::qemu::HostNet::system();
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir("/sys/class/net") {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            // physical (or VLAN/bond on physical) Ethernet only: virtual
            // devices such as veth/docker/wg have no `device` link
            let p = e.path();
            let physical = p.join("device").exists()
                || p.join("bonding").is_dir()
                || std::fs::read_to_string("/proc/net/vlan/config")
                    .map(|c| {
                        c.lines()
                            .any(|l| l.split('|').next().map(str::trim) == Some(&name))
                    })
                    .unwrap_or(false);
            if physical && host.macvtap_problem(&name).is_none() {
                out.push(name);
            }
        }
    }
    out.sort();
    out
}

/// Linux bridges on this host.
pub fn host_bridges() -> Vec<HostBridge> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir("/sys/class/net") {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Some(vlan_aware) = crate::vm::qemu::HostNet::system().bridge_vlan_aware(&name) {
                out.push(HostBridge { name, vlan_aware });
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Directories ISOs are listed from and may be attached from.
async fn iso_roots(state: &AppState) -> Vec<PathBuf> {
    iso_sources(state).await.into_iter().map(|r| r.0).collect()
}

/// (directory, storage id, storage name) ISOs are listed from.
async fn iso_sources(state: &AppState) -> Vec<(PathBuf, String, String)> {
    let mut roots = vec![(
        image_dir(&state.config.paths.data_dir),
        String::new(),
        "Image library".to_string(),
    )];
    for p in state.storage_manager.list_pools().await {
        if p.enabled && p.path.starts_with('/') {
            let base = PathBuf::from(&p.path);
            roots.push((base.join("template/iso"), p.id.clone(), p.name.clone()));
            roots.push((base, p.id, p.name));
        }
    }
    roots
}

/// Storages an ISO can be added to: the image library and every enabled
/// directory-backed pool.
pub async fn iso_storages(state: &AppState) -> Vec<IsoStorage> {
    let lib = image_dir(&state.config.paths.data_dir);
    let mut out = Vec::new();
    let mut pools = state.storage_manager.list_pools().await;
    pools.sort_by(|a, b| a.name.cmp(&b.name));
    for p in pools {
        if p.enabled && p.path.starts_with('/') && std::path::Path::new(&p.path).is_dir() {
            out.push(IsoStorage {
                id: p.id,
                name: p.name,
                dir: PathBuf::from(&p.path)
                    .join("template/iso")
                    .to_string_lossy()
                    .to_string(),
                available: p.available,
            });
        }
    }
    out.push(IsoStorage {
        id: String::new(),
        name: "Image library".into(),
        available: crate::storage::fs_capacity_gb(&nearest_existing(&lib).to_string_lossy())
            .map(|c| c.1)
            .unwrap_or(0),
        dir: lib.to_string_lossy().to_string(),
    });
    out
}

pub(crate) fn nearest_existing(p: &FsPath) -> PathBuf {
    let mut p = p.to_path_buf();
    while !p.exists() {
        if !p.pop() {
            return PathBuf::from("/");
        }
    }
    p
}

/// Directory of ISO storage `id` (`""` = image library), or why not.
pub async fn iso_dir_of(state: &AppState, id: &str) -> Result<(PathBuf, String), ApiError> {
    iso_storages(state)
        .await
        .into_iter()
        .find(|s| s.id == id)
        .map(|s| (PathBuf::from(s.dir), s.name))
        .ok_or_else(|| ApiError::NotFound(format!("ISO storage {id:?} not found")))
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
    for (root, storage, storage_name) in iso_sources(state).await {
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
                storage: storage.clone(),
                storage_name: storage_name.clone(),
            });
        }
    }
    // Downloads in progress (written to a hidden .part file) and failures.
    let state_now = downloads().lock().unwrap().clone();
    let stores = iso_storages(state).await;
    for (target, err) in state_now {
        let st = stores
            .iter()
            .find(|s| target.parent() == Some(FsPath::new(&s.dir)))
            .cloned();
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
            storage: st.as_ref().map(|s| s.id.clone()).unwrap_or_default(),
            storage_name: st.map(|s| s.name).unwrap_or_default(),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// A host-side check of a VM's hardware: bridges exist (and are VLAN-aware
/// when a tag is set) and the CD-ROM ISO is a file in the ISO library.
pub async fn check_host_hardware(state: &AppState, hw: &VmHardware) -> Result<(), ApiError> {
    check_host_firmware(hw)?;
    check_host_advanced(state, hw).await?;
    for d in &hw.hostpci {
        crate::vm::mdev::check_device(std::path::Path::new("/sys"), d)?;
    }
    for (i, nic) in hw.nets.iter().enumerate() {
        if let Some(r) = nic.rate_mbps {
            if r == 0 || r > horcrux_common::MAX_NIC_RATE_MBPS {
                return Err(ApiError::BadRequest(format!(
                    "net{i}: rate limit must be 1-{} Mbit/s (leave it empty for no limit)",
                    horcrux_common::MAX_NIC_RATE_MBPS
                )));
            }
        }
        if nic.attach == horcrux_common::NicAttach::Macvtap {
            if let Some(p) = crate::vm::qemu::HostNet::system().macvtap_problem(&nic.bridge) {
                return Err(ApiError::BadRequest(format!(
                    "net{i}: {p} (interfaces that can carry one: {})",
                    macvtap_uplinks().join(", ")
                )));
            }
            continue;
        }
        match crate::vm::qemu::HostNet::system().bridge_vlan_aware(&nic.bridge) {
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
        check_library_iso(state, "cdrom", iso).await?;
    }
    for (i, iso) in hw.extra_cdroms.iter().enumerate() {
        check_library_iso(state, &format!("extra_cdroms[{i}]"), iso).await?;
    }
    Ok(())
}

/// An ISO a VM's CD-ROM drive may use: a file in the ISO library.
async fn check_library_iso(state: &AppState, what: &str, iso: &str) -> Result<(), ApiError> {
    let p = std::fs::canonicalize(iso)
        .map_err(|e| ApiError::BadRequest(format!("{what} {iso}: {e}")))?;
    if !p.is_file() || !allowed_local_source(&p, &iso_roots(state).await) {
        return Err(ApiError::Forbidden(format!(
            "{what}: ISOs must be files in the ISO library ({} or a storage pool directory)",
            image_dir(&state.config.paths.data_dir).display()
        )));
    }
    Ok(())
}

async fn host_options(State(state): State<Arc<AppState>>) -> Json<HostOptions> {
    Json(HostOptions {
        bridges: vm_bridges(),
        macvtap_uplinks: macvtap_uplinks(),
        isos: scan_isos(&state).await,
        iso_dir: image_dir(&state.config.paths.data_dir)
            .to_string_lossy()
            .to_string(),
        ovmf: crate::vm::firmware::find_ovmf().is_some(),
        swtpm: in_path("swtpm"),
        kvm: std::path::Path::new("/dev/kvm").exists(),
        storage_pools: crate::api::storage_volumes::vm_disk_pools(&state).await,
        iso_storages: iso_storages(&state).await,
        default_iso_storage: default_iso_storage(&state).await,
        host: host_capabilities(
            crate::storage::fs_capacity_gb(
                &nearest_existing(&state.config.paths.vm_storage).to_string_lossy(),
            )
            .map(|c| c.1)
            .unwrap_or(0),
        ),
        default_networks: {
            let bridges = vm_bridges();
            DefaultNetworks {
                qemu: default_qemu_bridge(&bridges, default_route_dev().as_deref()),
                incus: bridges
                    .iter()
                    .find(|b| b.name == "incusbr0")
                    .map(|b| b.name.clone()),
                lxd: bridges
                    .iter()
                    .find(|b| b.name == "lxdbr0")
                    .map(|b| b.name.clone()),
                override_network: load_vm_defaults(&state)
                    .await
                    .default_network
                    .filter(|n| bridges.iter().any(|b| &b.name == n)),
            }
        },
    })
}

/// The pool named `local` (created on install) if present, else the first
/// pool, else the image library.
async fn default_iso_storage(state: &AppState) -> String {
    let st = iso_storages(state).await;
    st.iter()
        .find(|s| s.id == DEFAULT_POOL_ID)
        .or_else(|| st.first())
        .map(|s| s.id.clone())
        .unwrap_or_default()
}

/// Id of the storage pool created on first start.
pub const DEFAULT_POOL_ID: &str = "local";

/// Add a new empty disk to a stopped VM.
async fn add_disk(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(mut spec): Json<crate::vm::hardware::DiskSpec>,
) -> Result<(StatusCode, Json<VmConfig>), ApiError> {
    crate::api::storage_volumes::place_new_disk(&state, &mut spec).await?;
    Ok((
        StatusCode::CREATED,
        Json(state.vm_manager.add_disk(&id, spec).await?),
    ))
}

/// Change a disk's bus/cache/options (stopped VM) or grow it (any time).
async fn update_disk(
    State(state): State<Arc<AppState>>,
    Path((id, index)): Path<(String, usize)>,
    Json(upd): Json<crate::vm::hardware::DiskUpdate>,
) -> Result<Json<VmConfig>, ApiError> {
    Ok(Json(state.vm_manager.update_disk(&id, index, upd).await?))
}

#[derive(Debug, Default, Deserialize)]
pub struct RemoveDiskQuery {
    /// Also delete the image file (default: keep it).
    #[serde(default)]
    pub delete_file: bool,
}

/// Detach a disk (not disk0) from a stopped VM.
async fn remove_disk(
    State(state): State<Arc<AppState>>,
    Path((id, index)): Path<(String, usize)>,
    axum::extract::Query(q): axum::extract::Query<RemoveDiskQuery>,
) -> Result<Json<VmConfig>, ApiError> {
    crate::api::protection::ensure_vm_config_unprotected(&state, &id, "removing a disk").await?;
    Ok(Json(
        state
            .vm_manager
            .remove_disk(&id, index, q.delete_file)
            .await?,
    ))
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

/// The VM's TPM: model and whether its state is there.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TpmStatus {
    pub enabled: bool,
    pub model: horcrux_common::TpmModel,
    /// The state file exists on this node.
    pub state_present: bool,
    /// The VM has used its TPM: a start without the state is refused.
    pub state_expected: bool,
    pub state_bytes: u64,
    /// Expected but missing: the VM cannot start until the state is restored
    /// or a new TPM is created explicitly (`POST .../tpm/reset`).
    pub blocked: bool,
}

pub fn tpm_status_of(vm: &VmConfig) -> TpmStatus {
    let disk0 = vm.disks.first().map(|d| PathBuf::from(&d.path));
    let (present, bytes) = match &disk0 {
        Some(d) => (
            crate::vm::firmware::tpm_state_present(d),
            std::fs::metadata(
                crate::vm::firmware::tpm_state_dir(d).join(crate::vm::firmware::TPM_PERMALL),
            )
            .map(|m| m.len())
            .unwrap_or(0),
        ),
        None => (false, 0),
    };
    TpmStatus {
        enabled: vm.hardware.tpm,
        model: vm.hardware.tpm_model(),
        state_present: present,
        state_expected: vm.hardware.tpm_state_expected,
        state_bytes: bytes,
        blocked: vm.hardware.tpm && vm.hardware.tpm_state_expected && !present,
    }
}

async fn tpm_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<TpmStatus>, ApiError> {
    let vm = state.vm_manager.get_vm(&id).await?;
    Ok(Json(tpm_status_of(&vm)))
}

#[derive(Debug, Default, Deserialize)]
pub struct TpmResetRequest {
    /// Must be true: the guest loses its BitLocker keys, Windows Hello and
    /// Entra/Intune device identity (keep the BitLocker recovery key).
    #[serde(default)]
    pub confirm_data_loss: bool,
}

/// Delete the TPM state and start over with an empty TPM at the next start.
async fn tpm_reset(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Option<Json<TpmResetRequest>>,
) -> Result<Json<TpmStatus>, ApiError> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    if !req.confirm_data_loss {
        return Err(ApiError::BadRequest(
            "Creating a new TPM destroys what the guest keeps in the old one (BitLocker keys, \
             Windows Hello, Microsoft Entra/Intune device identity). Have the BitLocker recovery \
             key at hand and send {\"confirm_data_loss\": true}"
                .into(),
        ));
    }
    let vm = state.vm_manager.reset_tpm(&id).await?;
    Ok(Json(tpm_status_of(&vm)))
}

#[derive(Debug, Deserialize)]
pub struct TpmImportRequest {
    /// A swtpm state directory (libvirt `.../swtpm/<uuid>/tpm2`, virt-manager's
    /// per-user copy, a plain `swtpm --tpmstate dir=` directory) or a swtpm
    /// linear state file (a Proxmox `tpmstate0` volume exported to a file).
    /// Under the image library or a storage pool directory.
    pub source: String,
    /// Replace the TPM state the VM already has (the old one is kept as
    /// `<disk0>.tpm.prev`).
    #[serde(default)]
    pub replace: bool,
}

/// Give a stopped VM the TPM state of the machine it was moved from, so the
/// guest keeps its keys (BitLocker, Windows Hello, Entra/Intune identity).
async fn tpm_import_state(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<TpmImportRequest>,
) -> Result<Json<TpmStatus>, ApiError> {
    let vm = state.vm_manager.get_vm(&id).await?;
    if vm.hypervisor != horcrux_common::VmHypervisor::Qemu || !vm.hardware.tpm {
        return Err(ApiError::BadRequest(format!(
            "VM {id} has no emulated TPM: enable it under Hardware first"
        )));
    }
    if crate::vm::QemuManager::process_running(&id) {
        return Err(ApiError::Conflict(format!(
            "VM {id} is running: stop it first"
        )));
    }
    let disk0 = vm
        .disks
        .first()
        .map(|d| PathBuf::from(&d.path))
        .ok_or_else(|| ApiError::BadRequest("the VM has no disk".into()))?;
    let src = std::fs::canonicalize(&req.source)
        .map_err(|e| ApiError::BadRequest(format!("source {}: {e}", req.source)))?;
    let mut roots = vec![image_dir(&state.config.paths.data_dir)];
    roots.extend(
        state
            .storage_manager
            .list_pools()
            .await
            .into_iter()
            .filter(|p| p.path.starts_with('/'))
            .map(|p| PathBuf::from(p.path)),
    );
    if !allowed_local_source(&src, &roots) {
        return Err(ApiError::Forbidden(format!(
            "the TPM state must lie under {} or a storage pool directory",
            image_dir(&state.config.paths.data_dir).display()
        )));
    }
    let to = crate::vm::firmware::tpm_state_dir(&disk0);
    if crate::vm::firmware::tpm_state_present(&disk0) && !req.replace {
        return Err(ApiError::Conflict(format!(
            "VM {id} already has TPM state: send replace=true to swap it (the old state is kept as {}.prev)",
            to.display()
        )));
    }
    let tmp = to.with_extension("tpm.import");
    let prev = to.with_extension("tpm.prev");
    let (s2, t2) = (src.clone(), tmp.clone());
    let staged = tokio::task::spawn_blocking(move || -> horcrux_common::Result<()> {
        let _ = std::fs::remove_dir_all(&t2);
        if s2.is_dir() {
            crate::vm::firmware::copy_tpm_state_verified(&s2, &t2).map(|_| ())
        } else {
            crate::vm::firmware::import_linear_tpm_volume(&s2, &t2)
        }
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;
    if let Err(e) = staged {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e.into());
    }
    if to.exists() {
        let _ = std::fs::remove_dir_all(&prev);
        std::fs::rename(&to, &prev).map_err(|e| ApiError::Internal(format!("{e}")))?;
    }
    std::fs::rename(&tmp, &to).map_err(|e| ApiError::Internal(format!("{e}")))?;
    let vm = state.vm_manager.set_tpm_state_expected(&id, true).await?;
    Ok(Json(tpm_status_of(&vm)))
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

/// Freeze the VM's vCPUs (status "paused"); `POST .../resume` continues it.
async fn pause(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<VmConfig>, ApiError> {
    let before = state.vm_manager.get_vm(&id).await?;
    let vm = state.vm_manager.pause_vm(&id).await?;
    let _ = state
        .webhook_manager
        .trigger_event(
            crate::webhooks::WebhookEventType::VmStateChanged,
            serde_json::json!({"vm_id": vm.id, "name": vm.name,
                "old_state": format!("{:?}", before.status), "new_state": format!("{:?}", vm.status)}),
        )
        .await;
    state.ws_state.broadcast_vm_status(
        id,
        format!("{:?}", before.status),
        format!("{:?}", vm.status),
    );
    Ok(Json(vm))
}

async fn resume(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<VmConfig>, ApiError> {
    let before = state.vm_manager.get_vm(&id).await?;
    let vm = state.vm_manager.resume_vm(&id).await?;
    let _ = state
        .webhook_manager
        .trigger_event(
            crate::webhooks::WebhookEventType::VmStateChanged,
            serde_json::json!({"vm_id": vm.id, "name": vm.name,
                "old_state": format!("{:?}", before.status), "new_state": format!("{:?}", vm.status)}),
        )
        .await;
    state.ws_state.broadcast_vm_status(
        id,
        format!("{:?}", before.status),
        format!("{:?}", vm.status),
    );
    Ok(Json(vm))
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

/// A container firewall rule list plus the host-side interfaces it
/// matches (`vc<hash>i<n>`), so the page can show what the rules apply to.
#[derive(Debug, Serialize, Deserialize)]
pub struct ContainerFirewall {
    pub rules: Vec<crate::firewall::FirewallRule>,
    /// Interface name prefix the rules match (`vc<hash>i`).
    pub interface_prefix: String,
    /// Host-side interfaces of the container that exist right now.
    pub interfaces: Vec<String>,
}

fn ct_interfaces(id: &str) -> Vec<String> {
    let prefix = horcrux_common::ct_veth_prefix(id);
    let mut out: Vec<String> = std::fs::read_dir("/sys/class/net")
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.starts_with(&prefix))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

async fn list_ct_firewall(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ContainerFirewall>, ApiError> {
    state.container_manager.get_container(&id).await?;
    Ok(Json(ContainerFirewall {
        rules: state
            .firewall_manager
            .list_rules(crate::firewall::FirewallScope::Container(id.clone()))
            .await,
        interface_prefix: horcrux_common::ct_veth_prefix(&id),
        interfaces: ct_interfaces(&id),
    }))
}

/// Add a rule to one container's firewall; installed at once on that
/// container's host-side interfaces only (bridge-family nftables).
async fn add_ct_firewall(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(mut rule): Json<crate::firewall::FirewallRule>,
) -> Result<(StatusCode, Json<crate::firewall::FirewallRule>), ApiError> {
    state.container_manager.get_container(&id).await?;
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
    // A running Docker/Podman container's veths get their stable names
    // now (otherwise at its next start).
    if let Err(e) = state.container_manager.name_host_veths(&id).await {
        tracing::warn!("container {id}: {e}");
    }
    state
        .firewall_manager
        .add_rule(crate::firewall::FirewallScope::Container(id), rule.clone())
        .await?;
    Ok((StatusCode::CREATED, Json(rule)))
}

async fn delete_ct_firewall(
    State(state): State<Arc<AppState>>,
    Path((id, rule_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let scope = crate::firewall::FirewallScope::Container(id);
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
    /// Storage pool to put it in (`""`/absent: the image library).
    #[serde(default)]
    pub storage: Option<String>,
    /// Expected SHA-256 (64 hex digits); a mismatch discards the file.
    #[serde(default)]
    pub sha256: Option<String>,
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
    // Same guard as disk import: never the node's own services or cloud
    // metadata. A literal refused host is a 403 here; host names are
    // checked when resolved, and redirects hop by hop (in `fetch`).
    let parsed = reqwest::Url::parse(&req.url)
        .map_err(|e| ApiError::BadRequest(format!("download {}: {e}", req.url)))?;
    if let Some(why) = refused_url(&parsed) {
        return Err(ApiError::Forbidden(why));
    }
    if let Some(s) = &req.sha256 {
        if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ApiError::BadRequest("sha256 must be 64 hex digits".into()));
        }
    }
    let (dir, _) = iso_dir_of(&state, req.storage.as_deref().unwrap_or("")).await?;
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
    let want = req.sha256.clone();
    tokio::spawn(async move {
        let result = fetch(&url, &t, want.as_deref()).await;
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
            storage: req.storage.clone().unwrap_or_default(),
            storage_name: String::new(),
        }),
    ))
}

async fn fetch(url: &str, target: &FsPath, sha256: Option<&str>) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncWriteExt;
    let mut hasher = Sha256::new();
    let part = partial_path(target);
    let res: Result<(), String> = async {
        let parsed = reqwest::Url::parse(url).map_err(|e| e.to_string())?;
        if let Some(why) = refused_url(&parsed) {
            return Err(why);
        }
        let mut resp = download_client()
            .map_err(|e| format!("http client: {e}"))?
            .get(parsed)
            .send()
            .await
            .map_err(|e| error_chain(&e))?;
        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }
        let mut f = tokio::fs::File::create(&part)
            .await
            .map_err(|e| e.to_string())?;
        while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
            hasher.update(&chunk);
            f.write_all(&chunk).await.map_err(|e| e.to_string())?;
        }
        f.flush().await.map_err(|e| e.to_string())?;
        f.sync_all().await.map_err(|e| e.to_string())?;
        if let Some(want) = sha256 {
            let got = hex::encode(hasher.finalize());
            if !want.eq_ignore_ascii_case(&got) {
                return Err(format!("sha256 mismatch: downloaded data is {got}"));
            }
        }
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
#[derive(Debug, Deserialize)]
pub struct UploadIsoQuery {
    pub filename: String,
    /// Storage pool id (absent: the image library).
    #[serde(default)]
    pub storage: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
}

/// `POST /api/storage/isos/upload?filename=x.iso[&storage=<pool>][&sha256=..]`:
/// the raw request body is streamed into the ISO directory of that storage.
async fn upload_iso(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<UploadIsoQuery>,
    body: axum::body::Body,
) -> Result<(StatusCode, Json<IsoFile>), ApiError> {
    let storage = q.storage.clone().unwrap_or_default();
    let (dir, storage_name) = iso_dir_of(&state, &storage).await?;
    let ct = crate::api::storage_content::content_type("iso")?;
    let (_, Json(item)) = crate::api::storage_content::store_upload(
        ct,
        dir,
        q.filename,
        q.sha256,
        body,
        &format!("ISO storage {storage_name}"),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(IsoFile {
            name: item.name,
            path: item.path,
            size: item.size,
            downloading: false,
            error: None,
            storage,
            storage_name,
        }),
    ))
}

#[derive(Debug, Deserialize)]
pub struct DeleteIsoQuery {
    /// Storage pool the ISO is in (absent: the image library).
    #[serde(default)]
    pub storage: Option<String>,
}

async fn delete_iso(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    axum::extract::Query(q): axum::extract::Query<DeleteIsoQuery>,
) -> Result<StatusCode, ApiError> {
    let (dir, _) = iso_dir_of(&state, q.storage.as_deref().unwrap_or("")).await?;
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

    #[tokio::test]
    async fn iso_downloads_never_connect_to_the_node_itself() {
        // A listener that records whether anything connected.
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let hit = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let h = hit.clone();
        let srv = tokio::spawn(async move {
            while l.accept().await.is_ok() {
                h.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        });
        let d = std::env::temp_dir().join(format!("iso-dl-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let to = d.join("x.iso");
        for url in [
            format!("http://127.0.0.1:{port}/x.iso"),
            format!("http://[::ffff:127.0.0.1]:{port}/x.iso"),
            format!("http://localhost:{port}/x.iso"),
            "http://169.254.169.254/latest/meta-data/x.iso".to_string(),
        ] {
            let err = fetch(&url, &to, None).await.unwrap_err();
            assert!(err.contains("refusing"), "{url}: {err}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !hit.load(std::sync::atomic::Ordering::SeqCst),
            "nothing may connect to the node's own services"
        );
        assert!(!to.exists() && !partial_path(&to).exists());
        std::fs::remove_dir_all(&d).unwrap();
        srv.abort();
    }

    #[test]
    fn memory_reserve_leaves_room_for_the_host() {
        assert_eq!(memory_reserve_mb(63_739), 6_373);
        assert_eq!(memory_reserve_mb(8_192), 1_024);
        // tiny hosts keep half
        assert_eq!(memory_reserve_mb(1_024), 512);
        assert_eq!(memory_reserve_mb(0), 0);
    }

    #[test]
    fn default_bridge_prefers_vmbr0_then_a_general_purpose_bridge() {
        let b = |n: &str| HostBridge {
            name: n.into(),
            vlan_aware: false,
        };
        assert_eq!(
            default_qemu_bridge(&[b("docker0"), b("virbr0"), b("vmbr0")], None).as_deref(),
            Some("vmbr0")
        );
        // container runtimes' own bridges are skipped
        assert_eq!(
            default_qemu_bridge(&[b("docker0"), b("podman0"), b("virbr0")], None).as_deref(),
            Some("virbr0")
        );
        assert_eq!(
            default_qemu_bridge(&[b("br0"), b("virbr0")], Some("br0")).as_deref(),
            Some("br0")
        );
        assert_eq!(default_qemu_bridge(&[b("docker0")], None), None);
        assert_eq!(default_qemu_bridge(&[], None), None);
        // a user bridge named br-* is a real bridge; a Docker network's is not
        assert_eq!(
            default_qemu_bridge(&[b("br-0123456789ab"), b("br-lan")], None).as_deref(),
            Some("br-lan")
        );
    }

    #[test]
    fn vm_nic_choices_leave_out_container_runtime_bridges() {
        let b = |n: &str| HostBridge {
            name: n.into(),
            vlan_aware: false,
        };
        let names: Vec<String> = vm_bridges_of(
            [
                "br-0123456789ab",
                "br0",
                "cni0",
                "docker0",
                "incusbr0",
                "podman0",
                "virbr0",
                "vmbr0",
            ]
            .map(b)
            .to_vec(),
        )
        .into_iter()
        .map(|b| b.name)
        .collect();
        assert_eq!(names, ["br0", "incusbr0", "virbr0", "vmbr0"]);
    }

    #[test]
    fn partial_downloads_are_hidden_files() {
        assert_eq!(
            partial_path(FsPath::new("/d/x.iso")),
            PathBuf::from("/d/.x.iso.part")
        );
    }
}
