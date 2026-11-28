//! Import guests from Proxmox VE: VMs (qemu-server) become Horcrux VMs,
//! containers (pct) become Horcrux LXC containers.
//!
//! * `POST /api/import/proxmox/plan {source, guests?, bridge_map?, io_devices?}`:
//!   per guest the resulting Horcrux config, a carried / changed / not
//!   carried table of every Proxmox key, the volumes whose data moves, what
//!   blocks the import, and for VMs the source's QEMU command line
//!   (`qm showcmd`, when known) next to the one Horcrux will run. Read only.
//! * `POST /api/import/proxmox {source, guests, bridge_map?, io_devices?,
//!   storage?, ids?}` - 202 `{job}`; poll `POST /api/import/proxmox/status
//!   {job}`. `POST /api/import/proxmox/jobs {}` lists recent jobs.
//!
//! Sources (`source::Source`): a directory with the guest configs and the
//! received volumes (`{"type":"dir","path":..,"storage_map":{"local-zfs":
//! {"zfs":"tank/pve/data"}}}`), a Proxmox node over SSH (`{"type":"ssh",
//! "host":"root@pve"}`, read only), or a container vzdump archive.
//!
//! Data always MOVES into Horcrux's own storage: VM disks are converted to
//! qcow2 in the VM storage (or `storage` pool) and checked with `qemu-img
//! compare`; the UEFI variable store, the swtpm state and the cloud-init
//! seed are copied next to the first disk; a container's files are copied
//! into the LXC store with their owners as the container sees them. Nothing
//! of the source is changed; for a local source its volumes are offered for
//! removal afterwards (`/api/vms/import/cleanup`, opt-in, with sizes).
pub mod cmdline;
pub mod conf;
pub mod source;

use crate::error::ApiError;
use crate::AppState;
use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use conf::{GuestPlan, HostCtx, PveConf, Resolved, VolKind, VolRole};
use serde::{Deserialize, Serialize};
use source::{Kind, Source};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::process::Command;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/import/proxmox/plan", post(plan))
        .route("/api/import/proxmox", post(import))
        .route("/api/import/proxmox/status", post(status))
        .route("/api/import/proxmox/jobs", post(jobs_list))
}

// ------------------------------------------------------------------ plan

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PlanRequest {
    pub source: Source,
    /// Proxmox ids to plan (empty = every guest of the source).
    #[serde(default)]
    pub guests: Vec<u32>,
    /// Proxmox bridge -> Horcrux bridge (unmapped bridges keep their name).
    #[serde(default)]
    pub bridge_map: BTreeMap<String, String>,
    /// Source block device numbers (`8:16`) -> device path on this host, to
    /// carry raw `lxc.cgroup2.io.max` limits (SSH sources fill it in).
    #[serde(default)]
    pub io_devices: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlannedGuest {
    #[serde(flatten)]
    pub plan: GuestPlan,
    /// The Proxmox QEMU command line vs the Horcrux one (VMs with a known
    /// `qm showcmd`), fact by fact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qemu_diff: Option<Vec<cmdline::DiffRow>>,
    /// The Horcrux QEMU command line the import will run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub horcrux_cmdline: Option<String>,
    /// A guest with this id already exists here.
    pub exists: bool,
    /// The Horcrux guest this source guest was already imported as.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imported_as: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanResponse {
    pub source: String,
    pub guests: Vec<PlannedGuest>,
    pub bridges: Vec<String>,
}

fn source_label(s: &Source) -> String {
    match s {
        Source::Dir { path, .. } => format!("dir:{path}"),
        Source::Ssh { host, .. } => format!("ssh:{host}"),
        Source::Vzdump { path } => format!("vzdump:{path}"),
    }
}

/// PCI devices of this host with their IOMMU group.
fn host_pci() -> BTreeMap<String, Option<u32>> {
    let root = std::env::var("HORCRUX_SYSFS_PCI").unwrap_or_else(|_| "/sys/bus/pci/devices".into());
    let mut m = BTreeMap::new();
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let addr = e.file_name().to_string_lossy().to_string();
            let g = std::fs::read_link(e.path().join("iommu_group"))
                .ok()
                .and_then(|l| l.file_name().map(|n| n.to_string_lossy().to_string()))
                .and_then(|n| n.parse().ok());
            m.insert(addr, g);
        }
    }
    m
}

fn storage_cfg_of(src: &Source) -> BTreeMap<String, (String, String)> {
    if let Source::Dir { path, .. } = src {
        for f in ["storage.cfg", "storage/storage.cfg", "etc/pve/storage.cfg"] {
            if let Ok(t) = std::fs::read_to_string(Path::new(path).join(f)) {
                return source::parse_storage_cfg(&t);
            }
        }
    }
    BTreeMap::new()
}

/// Source device numbers used by containers' io.max rules -> a stable device
/// path on the SSH node (`/dev/disk/by-id/...` when there is one).
async fn ssh_io_devices(src: &Source, confs: &[PveConf]) -> BTreeMap<String, String> {
    let mut nums: Vec<String> = Vec::new();
    for c in confs {
        for (k, v) in &c.keys {
            if k == "lxc.cgroup2.io.max" {
                if let Some(d) = v.split_whitespace().next() {
                    if d.split(':').all(|x| x.parse::<u32>().is_ok())
                        && !nums.contains(&d.to_string())
                    {
                        nums.push(d.to_string());
                    }
                }
            }
        }
    }
    let mut out = BTreeMap::new();
    if nums.is_empty() {
        return out;
    }
    let mut script = String::new();
    for n in &nums {
        script.push_str(&format!(
            "d=$(readlink -f /sys/dev/block/{n}); d=/dev/${{d##*/}}; id=''; for l in /dev/disk/by-id/*; do case \"$l\" in *-part*|*/wwn-*|*/nvme-eui.*) continue;; esac; [ \"$(readlink -f \"$l\")\" = \"$d\" ] && {{ id=$l; break; }}; done; echo \"HXDEV {n}|${{id:-$d}}\"\n"
        ));
    }
    if let Ok(o) = source::ssh_run(src, &script).await {
        for l in String::from_utf8_lossy(&o).lines() {
            if let Some((n, p)) = l.strip_prefix("HXDEV ").and_then(|r| r.split_once('|')) {
                if p.starts_with("/dev/") && p != "/dev/" {
                    out.insert(n.to_string(), p.to_string());
                }
            }
        }
    }
    out
}

/// Host facts + the source's volumes, then every guest planned.
pub async fn plan_guests(
    state: &AppState,
    req: &PlanRequest,
) -> Result<(Vec<(source::Guest, GuestPlan)>, HostCtx), ApiError> {
    req.source.validate()?;
    let mut guests = source::list(&req.source).await?;
    if !req.guests.is_empty() {
        for id in &req.guests {
            if !guests.iter().any(|g| g.vmid == *id) {
                return Err(ApiError::NotFound(format!("the source has no guest {id}")));
            }
        }
        guests.retain(|g| req.guests.contains(&g.vmid));
    }
    let confs: Vec<PveConf> = guests.iter().map(|g| conf::parse_conf(&g.conf)).collect();
    let mut io_devices = req.io_devices.clone();
    let ssh_vols = if req.source.ssh_argv().is_some() {
        for (k, v) in ssh_io_devices(&req.source, &confs).await {
            io_devices.entry(k).or_insert(v);
        }
        let specs: Vec<String> = confs.iter().flat_map(source::volume_specs).collect();
        Some(source::resolve_ssh(&req.source, &specs).await)
    } else {
        None
    };
    let host = HostCtx {
        bridges: crate::api::vm_hw::host_bridges()
            .into_iter()
            .map(|b| b.name)
            .collect(),
        bridge_map: req.bridge_map.clone(),
        pci: host_pci(),
        io_devices,
        ovmf_secboot: crate::vm::firmware::find_ovmf_for(true).is_some(),
    };
    let src = req.source.clone();
    let cfg = storage_cfg_of(&src);
    let host2 = host.clone();
    let planned = tokio::task::spawn_blocking(move || {
        let resolver = |v: &conf::PveVol| -> Result<Resolved, String> {
            match &src {
                Source::Ssh { .. } => {
                    if v.storage.is_none() && v.volume.starts_with('/') {
                        return Err(format!(
                            "{} is a host path on the Proxmox node: not imported over SSH",
                            v.volume
                        ));
                    }
                    ssh_vols
                        .as_ref()
                        .and_then(|s| s.map.get(&v.spec()).cloned())
                        .unwrap_or_else(|| Err("not looked up".into()))
                }
                Source::Vzdump { path } => match v.storage {
                    // the rootfs and backed-up mount points are in the archive
                    Some(_) => Ok(Resolved {
                        path: path.clone(),
                        kind: VolKind::File,
                        format: "vzdump-tar".into(),
                        size: None,
                    }),
                    None => Err("a host path".into()),
                },
                Source::Dir { path, storage_map } => {
                    source::resolve_dir(Path::new(path), storage_map, &cfg, v)
                }
            }
        };
        guests
            .into_iter()
            .zip(confs)
            .map(|(g, c)| {
                let mut p = match g.kind {
                    Kind::Vm => conf::plan_vm(g.vmid, &c, &resolver, &host2),
                    Kind::Ct => conf::plan_ct(g.vmid, &c, &resolver, &host2),
                };
                if let (Source::Vzdump { .. }, Some(ct)) = (&src, p.ct.as_mut()) {
                    // backed-up mount point data is inside the rootfs archive
                    if let Some(o) = ct.lxc.as_mut() {
                        o.mounts.retain(|m| m.source.is_some());
                    }
                    p.volumes.retain(|v| v.role != VolRole::Mount);
                    p.rows.push(conf::Row {
                        key: "mount points".into(),
                        value: String::new(),
                        carry: conf::Carry::Changed,
                        horcrux: "files inside the rootfs".into(),
                        note: "a vzdump archive holds backed-up mount points inside the container's file tree: their files land in the rootfs at the same paths".into(),
                    });
                }
                (g, p)
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| ApiError::Internal(format!("plan task: {e}")))?;
    let _ = state;
    Ok((planned, host))
}

/// Known, deliberate differences between the two command lines.
pub(crate) fn expected_differences() -> BTreeMap<String, String> {
    [
        ("serial-ports", "every Horcrux VM has a serial console (serial0, logged); Proxmox adds one only with serial0: socket"),
        ("cdroms", "Horcrux always has a CD-ROM drive"),
        ("tablet", "Horcrux adds the absolute pointer through its own USB controller"),
    ]
    .into_iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect()
}

/// The QEMU argv Horcrux runs for `vm` (disks at their future place).
pub fn horcrux_argv(state: &AppState, vm: &horcrux_common::VmConfig) -> Vec<String> {
    let dir = &state.config.paths.vm_storage;
    horcrux_argv_in(
        dir,
        vm,
        crate::vm::firmware::find_ovmf_for(vm.hardware.secure_boot).as_ref(),
    )
}

pub fn horcrux_argv_in(
    dir: &Path,
    vm: &horcrux_common::VmConfig,
    ovmf: Option<&crate::vm::firmware::Ovmf>,
) -> Vec<String> {
    let mut disks = vm.disks.clone();
    for (i, d) in disks.iter_mut().enumerate() {
        d.path = dir
            .join(if i == 0 {
                format!("{}.qcow2", vm.id)
            } else {
                format!("{}-disk{i}.qcow2", vm.id)
            })
            .to_string_lossy()
            .to_string();
    }
    let disk_path = PathBuf::from(&disks[0].path);
    let q = crate::vm::qemu::QemuVm {
        id: vm.id.clone(),
        name: vm.name.clone(),
        memory: vm.memory,
        cpus: vm.cpus,
        disk_path: disk_path.clone(),
        disk_size: vm.disk_size,
        status: horcrux_common::VmStatus::Stopped,
        _pid: None,
        hypervisor: vm.hypervisor.clone(),
        architecture: vm.architecture.clone(),
        display: vm.display,
        hardware: vm.hardware.clone(),
        disks,
    };
    let rt = match crate::vm::display::DisplayRuntime::plan(&disk_path, &vm.display) {
        Ok(rt) => rt,
        Err(_) => return Vec::new(),
    };
    crate::vm::qemu::qemu_command(&q, &rt, true, ovmf).1
}

fn with_diff(
    state: &AppState,
    g: &source::Guest,
    p: &GuestPlan,
) -> (Option<Vec<cmdline::DiffRow>>, Option<String>) {
    let Some(vm) = &p.vm else { return (None, None) };
    if vm.disks.is_empty() {
        return (None, None);
    }
    let argv = horcrux_argv(state, vm);
    let line = argv.join(" ");
    let diff = g.showcmd.as_ref().map(|s| {
        let pve = cmdline::facts(&cmdline::shell_words(s));
        let hx = cmdline::facts(&argv);
        cmdline::diff(&pve, &hx, &expected_differences())
    });
    (diff, Some(line))
}

async fn existing_ids(state: &AppState) -> (Vec<String>, Vec<String>) {
    let vms = state
        .vm_manager
        .list_vms()
        .await
        .into_iter()
        .map(|v| v.id)
        .collect();
    let cts = state
        .container_manager
        .list_containers()
        .await
        .into_iter()
        .map(|c| c.id)
        .collect();
    (vms, cts)
}

/// `<data_dir>/proxmox-import/<id>.json`: which source guest became `id`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImportRecord {
    pub id: String,
    pub kind: Kind,
    pub source: String,
    pub vmid: u32,
}

fn record_dir(state: &AppState) -> PathBuf {
    state.config.paths.data_dir.join("proxmox-import")
}

fn records(state: &AppState) -> Vec<ImportRecord> {
    let Ok(rd) = std::fs::read_dir(record_dir(state)) else {
        return vec![];
    };
    rd.flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read(e.path()).ok())
        .filter_map(|b| serde_json::from_slice(&b).ok())
        .collect()
}

async fn plan(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PlanRequest>,
) -> Result<Json<PlanResponse>, ApiError> {
    let (planned, host) = plan_guests(&state, &req).await?;
    let (vms, cts) = existing_ids(&state).await;
    let recs = records(&state);
    let label = source_label(&req.source);
    let mut out = Vec::new();
    for (g, p) in planned {
        let (qemu_diff, horcrux_cmdline) = with_diff(&state, &g, &p);
        let exists = vms.contains(&p.target_id) || cts.contains(&p.target_id);
        let imported_as = recs
            .iter()
            .find(|r| {
                r.source == label
                    && r.vmid == g.vmid
                    && (vms.contains(&r.id) || cts.contains(&r.id))
            })
            .map(|r| r.id.clone());
        out.push(PlannedGuest {
            plan: p,
            qemu_diff,
            horcrux_cmdline,
            exists,
            imported_as,
        });
    }
    Ok(Json(PlanResponse {
        source: label,
        guests: out,
        bridges: host.bridges,
    }))
}

// ------------------------------------------------------------------ jobs

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ImportRequest {
    #[serde(flatten)]
    pub plan: PlanRequest,
    /// Storage pool for VM disks (default: the VM storage directory).
    #[serde(default)]
    pub storage: Option<String>,
    /// Proxmox id -> Horcrux id (default: from the guest's name).
    #[serde(default)]
    pub ids: BTreeMap<u32, String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct GuestResult {
    pub vmid: u32,
    pub kind: String,
    pub name: String,
    /// The Horcrux id (when created).
    pub id: Option<String>,
    /// `waiting`, `running`, `done`, `failed`.
    pub state: String,
    pub message: String,
    pub error: Option<String>,
    pub warnings: Vec<String>,
    /// The new guest's page.
    pub link: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Job {
    pub id: String,
    /// `running`, `done` (every guest imported), `failed` (some failed).
    pub state: String,
    pub percent: f32,
    pub message: String,
    pub source: String,
    pub guests: Vec<GuestResult>,
    pub started_at: String,
}

fn jobs() -> &'static std::sync::Mutex<Vec<Job>> {
    static J: std::sync::OnceLock<std::sync::Mutex<Vec<Job>>> = std::sync::OnceLock::new();
    J.get_or_init(Default::default)
}

fn job_update(id: &str, f: impl FnOnce(&mut Job)) {
    if let Ok(mut j) = jobs().lock() {
        if let Some(x) = j.iter_mut().find(|x| x.id == id) {
            f(x);
        }
    }
}

#[derive(Deserialize)]
struct StatusRequest {
    job: String,
}

async fn status(Json(req): Json<StatusRequest>) -> Result<Json<Job>, ApiError> {
    jobs()
        .lock()
        .ok()
        .and_then(|j| j.iter().find(|x| x.id == req.job).cloned())
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("no Proxmox import job {}", req.job)))
}

async fn jobs_list() -> Json<Vec<Job>> {
    let mut v = jobs().lock().map(|j| j.clone()).unwrap_or_default();
    v.reverse();
    Json(v)
}

async fn import(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ImportRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    if req.plan.guests.is_empty() {
        return Err(ApiError::BadRequest(
            "pick the guests to import (guests: [proxmox ids])".into(),
        ));
    }
    for (vmid, id) in &req.ids {
        crate::validation::validate_vm_id(id)
            .map_err(|e| ApiError::BadRequest(format!("id for guest {vmid}: {e}")))?;
    }
    // Plan first: a source that can't be read fails here, synchronously.
    let (planned, _) = plan_guests(&state, &req.plan).await?;
    let id = format!("pve-{:016x}", rand::random::<u64>());
    {
        let mut j = jobs()
            .lock()
            .map_err(|_| ApiError::Internal("jobs".into()))?;
        if j.iter().any(|x| x.state == "running") {
            return Err(ApiError::Conflict(
                "another Proxmox import is running: wait for it (disks are copied at low priority)"
                    .into(),
            ));
        }
        if j.len() > 20 {
            j.remove(0);
        }
        j.push(Job {
            id: id.clone(),
            state: "running".into(),
            message: "starting".into(),
            source: source_label(&req.plan.source),
            started_at: chrono::Utc::now().to_rfc3339(),
            guests: planned
                .iter()
                .map(|(g, p)| GuestResult {
                    vmid: g.vmid,
                    kind: if g.kind == Kind::Vm { "vm" } else { "ct" }.into(),
                    name: p.name.clone(),
                    state: "waiting".into(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        });
    }
    let jid = id.clone();
    tokio::spawn(async move {
        let total = planned.len().max(1) as f32;
        let mut failed = 0;
        for (n, (g, p)) in planned.into_iter().enumerate() {
            let base = 100.0 * n as f32 / total;
            let vmid = g.vmid;
            let set = |f: &dyn Fn(&mut GuestResult)| {
                job_update(&jid, |j| {
                    if let Some(r) = j.guests.iter_mut().find(|r| r.vmid == vmid) {
                        f(r);
                    }
                })
            };
            set(&|r| r.state = "running".into());
            let progress = |pct: f32, msg: String| {
                job_update(&jid, |j| {
                    j.percent = base + pct / total;
                    j.message = format!("guest {vmid}: {msg}");
                    if let Some(r) = j.guests.iter_mut().find(|r| r.vmid == vmid) {
                        r.message = msg.clone();
                    }
                })
            };
            let res = import_guest(&state, &req, &g, p, &progress).await;
            match res {
                Ok((hid, warnings)) => {
                    let link = if g.kind == Kind::Vm {
                        format!("/vms/{hid}")
                    } else {
                        format!("/containers/{hid}/lxc")
                    };
                    set(&|r| {
                        r.state = "done".into();
                        r.id = Some(hid.clone());
                        r.link = Some(link.clone());
                        r.message = "imported".into();
                        r.warnings = warnings.clone();
                    });
                }
                Err(e) => {
                    failed += 1;
                    let msg = api_msg(e);
                    tracing::warn!("Proxmox import of guest {vmid} failed: {msg}");
                    set(&|r| {
                        r.state = "failed".into();
                        r.error = Some(msg.clone());
                    });
                }
            }
        }
        job_update(&jid, |j| {
            j.percent = 100.0;
            j.state = if failed == 0 { "done" } else { "failed" }.into();
            j.message = if failed == 0 {
                "imported".into()
            } else {
                format!("{failed} guest(s) failed: nothing of them was kept")
            };
        });
    });
    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({ "job": id }))))
}

fn api_msg(e: ApiError) -> String {
    match e {
        ApiError::BadRequest(m)
        | ApiError::Conflict(m)
        | ApiError::Internal(m)
        | ApiError::NotFound(m) => m,
        ApiError::NotAvailable { message, hint, .. } => format!("{message}. {hint}"),
        other => format!("{other:?}"),
    }
}

/// What a failed guest import removes again (the source is never touched).
#[derive(Default)]
struct Undo {
    vm: Option<String>,
    ct: Option<String>,
    files: Vec<PathBuf>,
}

impl Undo {
    async fn run(&mut self, state: &AppState) {
        if let Some(v) = self.vm.take() {
            let _ = state.vm_manager.delete_vm(&v).await;
        }
        if let Some(c) = self.ct.take() {
            let _ = state.container_manager.delete_container(&c).await;
        }
        for f in self.files.drain(..) {
            let _ = std::fs::remove_file(&f);
        }
    }
}

async fn import_guest(
    state: &Arc<AppState>,
    req: &ImportRequest,
    g: &source::Guest,
    mut p: GuestPlan,
    progress: &(dyn Fn(f32, String) + Sync),
) -> Result<(String, Vec<String>), ApiError> {
    if !p.blockers.is_empty() {
        return Err(ApiError::Conflict(format!(
            "not imported: {}. Nothing was changed",
            p.blockers.join("; ")
        )));
    }
    let label = source_label(&req.plan.source);
    let (vms, cts) = existing_ids(state).await;
    if let Some(r) = records(state).iter().find(|r| {
        r.source == label && r.vmid == g.vmid && (vms.contains(&r.id) || cts.contains(&r.id))
    }) {
        return Err(ApiError::Conflict(format!(
            "guest {} of this source was already imported as {}",
            g.vmid, r.id
        )));
    }
    let id = req
        .ids
        .get(&g.vmid)
        .cloned()
        .unwrap_or_else(|| p.target_id.clone());
    if vms.contains(&id) || cts.contains(&id) {
        return Err(ApiError::Conflict(format!(
            "a guest {id} already exists: pick another id (ids: {{\"{}\": \"<new id>\"}})",
            g.vmid
        )));
    }
    p.target_id = id.clone();
    let mut undo = Undo::default();
    let r = match g.kind {
        Kind::Vm => import_vm(state, req, g, &p, &mut undo, progress).await,
        Kind::Ct => import_ct(state, req, g, &p, &mut undo, progress).await,
    };
    match r {
        Ok(w) => {
            let rec = ImportRecord {
                id: id.clone(),
                kind: g.kind,
                source: label,
                vmid: g.vmid,
            };
            let dir = record_dir(state);
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join(format!("{id}.conf")), &g.conf);
            let _ = std::fs::write(
                dir.join(format!("{id}.json")),
                serde_json::to_vec_pretty(&rec).unwrap_or_default(),
            );
            if !p.tags.is_empty() {
                let key = if g.kind == Kind::Vm {
                    format!("vm:{id}")
                } else {
                    format!("ct:{id}")
                };
                let _ = crate::api::inventory::store_tags(state, key, p.tags.clone()).await;
            }
            Ok((id, w))
        }
        Err(e) => {
            undo.run(state).await;
            Err(e)
        }
    }
}

fn free_bytes(dir: &Path) -> u64 {
    nix::sys::statvfs::statvfs(dir)
        .map(|s| s.blocks_available() * s.fragment_size())
        .unwrap_or(u64::MAX)
}

fn low_priority(program: &str) -> Command {
    let mut c = Command::new("nice");
    c.args(["-n", "19"]);
    if std::process::Command::new("ionice")
        .arg("-h")
        .output()
        .is_ok()
    {
        c.args(["ionice", "-c", "3"]);
    }
    c.arg(program);
    c
}

/// Bytes of a local volume (block device, file).
fn local_size(path: &str) -> u64 {
    use std::io::{Seek, SeekFrom};
    std::fs::File::open(path)
        .and_then(|mut f| f.seek(SeekFrom::End(0)))
        .unwrap_or(0)
}

/// Copy the SSH source's `remote` volume into the local file `to` (raw
/// bytes), reporting the fraction done.
async fn fetch_ssh(
    src: &Source,
    remote: &str,
    to: &Path,
    size: u64,
    report: &(dyn Fn(f32) + Sync),
) -> Result<(), ApiError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let argv = src
        .ssh_argv()
        .ok_or_else(|| ApiError::Internal("not ssh".into()))?;
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .arg(format!("dd if={} bs=4M status=none", source::sq(remote)))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| ApiError::Internal(format!("ssh: {e}")))?;
    let mut out = child.stdout.take().expect("piped");
    let mut f = tokio::fs::File::create(to).await?;
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    loop {
        let n = out.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n]).await?;
        done += n as u64;
        if size > 0 {
            report((done as f32 / size as f32).min(1.0));
        }
    }
    f.sync_all().await?;
    let o = child.wait_with_output().await?;
    if !o.status.success() {
        return Err(ApiError::Internal(format!(
            "reading {remote} on the Proxmox node: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )));
    }
    if size > 0 && done != size {
        return Err(ApiError::Internal(format!(
            "{remote}: got {done} bytes, the volume has {size}"
        )));
    }
    Ok(())
}

/// `src` (raw/qcow2/vmdk) -> qcow2 `dst`, verified with `qemu-img compare`.
async fn convert_verified(
    src: &str,
    fmt: &str,
    dst: &Path,
    report: &(dyn Fn(f32) + Sync),
) -> Result<(), ApiError> {
    use tokio::io::AsyncReadExt;
    let d = dst.to_string_lossy().to_string();
    let mut child = low_priority("qemu-img")
        .args(["convert", "-p", "-f", fmt, "-O", "qcow2", src, &d])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| ApiError::Internal(format!("qemu-img convert: {e}")))?;
    let mut out = child.stdout.take().expect("piped");
    let mut buf = [0u8; 256];
    loop {
        let n = out.read(&mut buf).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        if let Some(p) =
            crate::api::libvirt_import::parse_progress(&String::from_utf8_lossy(&buf[..n]))
        {
            report(p / 100.0);
        }
    }
    let o = child
        .wait_with_output()
        .await
        .map_err(|e| ApiError::Internal(format!("qemu-img convert: {e}")))?;
    if !o.status.success() {
        return Err(ApiError::Internal(crate::api::vm_import::qemu_img_error(
            &format!("convert {src}"),
            &String::from_utf8_lossy(&o.stderr),
        )));
    }
    let c = low_priority("qemu-img")
        .args(["compare", "-f", fmt, "-F", "qcow2", src, &d])
        .output()
        .await
        .map_err(|e| ApiError::Internal(format!("qemu-img compare: {e}")))?;
    if !c.status.success() {
        return Err(ApiError::Internal(format!(
            "the copy of {src} differs from the source: {}{}",
            String::from_utf8_lossy(&c.stdout).trim(),
            String::from_utf8_lossy(&c.stderr).trim()
        )));
    }
    tokio::fs::File::open(dst).await?.sync_all().await?;
    Ok(())
}

/// Read a (small) volume: local, or over SSH.
async fn read_small(src: &Source, r: &Resolved, max: usize) -> Result<Vec<u8>, ApiError> {
    if src.ssh_argv().is_some() {
        let out = source::ssh_run(src, &format!("head -c {max} {}", source::sq(&r.path))).await?;
        return Ok(out);
    }
    use std::io::Read;
    let mut f = std::fs::File::open(&r.path)
        .map_err(|e| ApiError::BadRequest(format!("{}: {e}", r.path)))?;
    let mut buf = Vec::new();
    f.by_ref().take(max as u64).read_to_end(&mut buf)?;
    Ok(buf)
}

/// The length of the UEFI variable store at the start of an efidisk: the
/// firmware volume header (`_FVH` at 0x28) carries it at 0x20.
pub fn ovmf_vars_len(head: &[u8]) -> Option<u64> {
    if head.len() < 0x30 || &head[0x28..0x2c] != b"_FVH" {
        return None;
    }
    Some(u64::from_le_bytes(head[0x20..0x28].try_into().ok()?))
}

fn zfs_dataset_of_zvol(path: &str) -> Option<String> {
    path.strip_prefix("/dev/zvol/").map(String::from)
}

async fn import_vm(
    state: &Arc<AppState>,
    req: &ImportRequest,
    _g: &source::Guest,
    p: &GuestPlan,
    undo: &mut Undo,
    progress: &(dyn Fn(f32, String) + Sync),
) -> Result<Vec<String>, ApiError> {
    let src = &req.plan.source;
    let ssh = src.ssh_argv().is_some();
    let mut warnings: Vec<String> = p
        .rows
        .iter()
        .filter(|r| r.carry == conf::Carry::NotCarried)
        .map(|r| format!("{}: {}", r.key, r.note))
        .collect();
    let mut cfg =
        p.vm.clone()
            .ok_or_else(|| ApiError::Internal("no VM in the plan".into()))?;
    cfg.id = p.target_id.clone();
    let pool = req
        .storage
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);
    for d in cfg.disks.iter_mut() {
        d.storage = pool.clone();
    }
    let disk_vols: Vec<&conf::PlanVol> = {
        let mut v: Vec<&conf::PlanVol> = p
            .volumes
            .iter()
            .filter(|v| v.role == VolRole::Disk)
            .collect();
        v.sort_by_key(|v| v.index);
        v
    };
    if disk_vols.len() != cfg.disks.len() {
        return Err(ApiError::Internal(
            "the plan's disks do not match the VM".into(),
        ));
    }
    let resolved = |v: &conf::PlanVol| -> Result<Resolved, ApiError> {
        v.resolved.clone().ok_or_else(|| {
            ApiError::BadRequest(format!("{}: volume {} not found", v.key, v.source))
        })
    };

    // 1. firmware and swtpm must exist here
    if cfg.hardware.firmware == horcrux_common::Firmware::Ovmf
        && crate::vm::firmware::find_ovmf_for(cfg.hardware.secure_boot).is_none()
    {
        return Err(ApiError::BadRequest(if cfg.hardware.secure_boot {
            crate::vm::firmware::ovmf_secboot_missing().to_string()
        } else {
            crate::vm::firmware::ovmf_missing().to_string()
        }));
    }
    if cfg.hardware.tpm && !crate::api::vm_hw::in_path("swtpm") {
        return Err(ApiError::BadRequest(
            "the VM has a vTPM but swtpm is not installed on this host (app-crypt/swtpm)".into(),
        ));
    }

    // 2. sizes and free space
    progress(1.0, "checking the disks".into());
    let mut sizes: Vec<u64> = Vec::new();
    for v in &disk_vols {
        let r = resolved(v)?;
        let bytes = if ssh {
            r.size.or(v.size_bytes).unwrap_or(0)
        } else if r.kind == VolKind::File && r.format != "raw" {
            let info: serde_json::Value = serde_json::from_str(
                &crate::api::vm_import::qemu_img(&["info", "--output=json", &r.path]).await?,
            )
            .map_err(|e| ApiError::Internal(format!("qemu-img info: {e}")))?;
            crate::api::vm_import::checked_format(&info)
                .map_err(|e| ApiError::BadRequest(format!("{}: {e}", r.path)))?;
            info["virtual-size"].as_u64().unwrap_or(0)
        } else {
            local_size(&r.path)
        };
        if bytes == 0 {
            return Err(ApiError::BadRequest(format!(
                "{}: cannot tell the size of {}",
                v.key, r.path
            )));
        }
        sizes.push(bytes);
    }
    for (d, s) in cfg.disks.iter_mut().zip(&sizes) {
        d.size_gb = s.div_ceil(1 << 30).max(1);
    }
    cfg.disk_size = cfg.disks[0].size_gb;
    let target_dir = match &pool {
        Some(pl) => {
            crate::api::storage_volumes::vm_disk_dir(
                state,
                pl,
                cfg.disks.iter().map(|d| d.size_gb).sum(),
            )
            .await?
        }
        None => state.config.paths.vm_storage.clone(),
    };
    std::fs::create_dir_all(&target_dir)?;
    // qcow2 copies hold at most the data; SSH copies also need a raw
    // staging file of the biggest disk.
    let need: u64 = sizes.iter().sum::<u64>()
        + if ssh {
            sizes.iter().copied().max().unwrap_or(0)
        } else {
            0
        };
    crate::api::libvirt_import::check_free_space(need, free_bytes(&target_dir), &target_dir)
        .map_err(ApiError::BadRequest)?;

    // 3. the VM with every disk (bus, cache, discard, I/O limits as
    //    planned): Horcrux places them where any new VM's go
    cfg.hardware
        .validate_for(&cfg.architecture, cfg.cpus)
        .map_err(ApiError::BadRequest)?;
    crate::api::storage_volumes::place_vm_disks(state, &mut cfg).await?;
    crate::api::vm_hw::check_host_hardware(state, &cfg.hardware).await?;
    let vm = state.vm_manager.create_vm(cfg).await?;
    undo.vm = Some(vm.id.clone());
    let new_paths: Vec<PathBuf> = vm.disks.iter().map(|d| PathBuf::from(&d.path)).collect();
    if new_paths.len() != disk_vols.len() {
        return Err(ApiError::Internal("the VM has not got every disk".into()));
    }
    let mut roots = vec![state.config.paths.vm_storage.clone(), target_dir.clone()];
    roots.dedup();
    let sources: Vec<PathBuf> = disk_vols
        .iter()
        .filter_map(|v| v.resolved.as_ref().map(|r| PathBuf::from(&r.path)))
        .collect();
    crate::api::libvirt_import::verify_placement(&roots, &new_paths, &sources)
        .map_err(ApiError::Internal)?;

    // 4. copy + verify each disk (low priority, with progress)
    let total = disk_vols.len() as f32;
    for (n, (v, to)) in disk_vols.iter().zip(&new_paths).enumerate() {
        let r = resolved(v)?;
        let tmp = to.with_extension("import.tmp");
        undo.files.push(tmp.clone());
        let label = format!(
            "copying disk {} of {} ({})",
            n + 1,
            disk_vols.len(),
            v.source
        );
        progress(2.0 + 90.0 * n as f32 / total, label.clone());
        let base = n as f32;
        let report = |f: f32| progress(2.0 + 90.0 * (base + f) / total, label.clone());
        if ssh {
            let stage = to.with_extension("fetch.raw");
            undo.files.push(stage.clone());
            let half = |f: f32| report(f * 0.5);
            fetch_ssh(src, &r.path, &stage, sizes[n], &half).await?;
            let fmt = if r.kind == VolKind::File {
                r.format.clone()
            } else {
                "raw".into()
            };
            convert_verified(&stage.to_string_lossy(), &fmt, &tmp, &|f| {
                report(0.5 + f * 0.5)
            })
            .await?;
            let _ = std::fs::remove_file(&stage);
        } else {
            convert_verified(&r.path, &r.format, &tmp, &report).await?;
        }
        tokio::fs::rename(&tmp, to).await?;
    }

    // 5. UEFI variables, TPM state, cloud-init seed next to the first disk
    let disk0 = new_paths[0].clone();
    progress(93.0, "firmware variables, TPM state, cloud-init".into());
    let secure = vm.hardware.secure_boot;
    if let Some(v) = p.volumes.iter().find(|v| v.role == VolRole::Efivars) {
        let r = resolved(v)?;
        let head = read_small(src, &r, 8 << 20).await?;
        let tpl_len = crate::vm::firmware::find_ovmf_for(secure)
            .and_then(|o| std::fs::metadata(o.vars_template).ok())
            .map(|m| m.len());
        match (ovmf_vars_len(&head), tpl_len) {
            (Some(n), Some(t)) if n == t && head.len() as u64 >= n => {
                let raw = disk0.with_extension("efivars.import.raw");
                undo.files.push(raw.clone());
                std::fs::write(&raw, &head[..n as usize])?;
                let vars = crate::vm::firmware::efivars_path(&disk0);
                crate::api::vm_import::qemu_img(&[
                    "convert", "-f", "raw", "-O", "qcow2",
                    &raw.to_string_lossy(), &vars.to_string_lossy(),
                ])
                .await?;
                let _ = std::fs::remove_file(&raw);
            }
            (n, t) => warnings.push(format!(
                "the UEFI variable store ({} bytes) does not match this host's OVMF layout ({} bytes): a fresh store is used; the firmware finds the boot disk itself (custom boot entries and enrolled keys must be added again)",
                n.map(|x| x.to_string()).unwrap_or_else(|| "unreadable".into()),
                t.map(|x| x.to_string()).unwrap_or_else(|| "?".into())
            )),
        }
    }
    if let Some(v) = p.volumes.iter().find(|v| v.role == VolRole::Tpm) {
        let r = resolved(v)?;
        let raw = read_small(src, &r, 64 << 20).await?;
        let to = crate::vm::firmware::tpm_state_dir(&disk0);
        let staged = disk0.with_extension("tpm.import.raw");
        undo.files.push(staged.clone());
        std::fs::write(&staged, &raw)?;
        let (s2, t2) = (staged.clone(), to.clone());
        tokio::task::spawn_blocking(move || {
            crate::vm::firmware::import_linear_tpm_volume(&s2, &t2)
        })
        .await
        .map_err(|e| ApiError::Internal(format!("TPM task: {e}")))?
        .map_err(|e| ApiError::Internal(format!("TPM state not carried over: {e}")))?;
        let _ = std::fs::remove_file(&staged);
    }
    if let Some(v) = p.volumes.iter().find(|v| v.role == VolRole::Cloudinit) {
        match &v.resolved {
            Some(r) => {
                let data = read_small(src, r, 64 << 20).await?;
                let seed = crate::vm::qemu::cloudinit_seed(&disk0);
                std::fs::write(&seed, &data)?;
            }
            None => warnings.push(format!(
                "the cloud-init drive {} was not found: the guest boots without its seed (create one with Horcrux cloud-init if the guest needs it again)",
                v.source
            )),
        }
    }

    // 6. verify the stored VM
    let got = state.vm_manager.get_vm(&vm.id).await?;
    let got_paths: Vec<PathBuf> = got.disks.iter().map(|d| PathBuf::from(&d.path)).collect();
    crate::api::libvirt_import::verify_placement(&roots, &got_paths, &sources)
        .map_err(ApiError::Internal)?;
    if got_paths.iter().any(|p| !p.is_file())
        || got.hardware.nets.len() != p.vm.as_ref().map(|v| v.hardware.nets.len()).unwrap_or(0)
    {
        return Err(ApiError::Internal(
            "the stored VM does not match the source".into(),
        ));
    }

    // 7. start at boot / order (the guest power policy)
    let ov = crate::api::guest_state::GuestOverride {
        start_action: Some(if p.autostart.onboot {
            crate::api::guest_state::StartAction::Always
        } else {
            crate::api::guest_state::StartAction::Nothing
        }),
        start_order: p.autostart.order,
        start_delay_secs: p.autostart.up_delay_secs,
        ..Default::default()
    };
    crate::api::guest_state::policy::save_override(
        state,
        crate::api::guest_state::GuestKind::Vm,
        &vm.id,
        &ov,
    )
    .await
    .map_err(|e| ApiError::Internal(format!("start policy: {e}")))?;

    // 8. the old source: offered for removal (local sources only)
    if !ssh {
        use crate::api::import_cleanup::Item;
        let mut items = Vec::new();
        for (v, new) in disk_vols.iter().zip(&new_paths) {
            if let Some(r) = &v.resolved {
                items.push(match zfs_dataset_of_zvol(&r.path) {
                    Some(ds) => Item {
                        kind: "zfs-dataset".into(),
                        path: ds,
                        requires: Some(new.to_string_lossy().to_string()),
                    },
                    None => Item {
                        kind: "file".into(),
                        path: r.path.clone(),
                        requires: Some(new.to_string_lossy().to_string()),
                    },
                });
            }
        }
        for role in [VolRole::Efivars, VolRole::Tpm, VolRole::Cloudinit] {
            if let Some(r) = p
                .volumes
                .iter()
                .find(|v| v.role == role)
                .and_then(|v| v.resolved.as_ref())
            {
                let requires = match role {
                    VolRole::Efivars => crate::vm::firmware::efivars_path(&disk0),
                    VolRole::Tpm => crate::vm::firmware::tpm_state_dir(&disk0),
                    _ => crate::vm::qemu::cloudinit_seed(&disk0),
                };
                if role == VolRole::Efivars && !requires.exists() {
                    continue; // a fresh store: keep the old one
                }
                items.push(match zfs_dataset_of_zvol(&r.path) {
                    Some(ds) => Item {
                        kind: "zfs-dataset".into(),
                        path: ds,
                        requires: Some(requires.to_string_lossy().to_string()),
                    },
                    None => Item {
                        kind: "file".into(),
                        path: r.path.clone(),
                        requires: Some(requires.to_string_lossy().to_string()),
                    },
                });
            }
        }
        let _ = crate::api::import_cleanup::write_record(
            state,
            &crate::api::import_cleanup::Record {
                vm_id: vm.id.clone(),
                source: "proxmox".into(),
                items,
                guest: None,
            },
        );
        warnings.push("the Proxmox volumes remain: do not start the guest on Proxmox as well (both would use one MAC and OS). Remove them with the cleanup offer once the VM runs here".into());
    } else {
        warnings.push("the guest still exists on the Proxmox node: keep it stopped there (same MAC addresses), and remove it there once the VM runs here".into());
    }
    undo.vm = None;
    undo.files.clear();
    tracing::info!(
        "VM {}: imported from Proxmox guest {} ({})",
        vm.id,
        p.vmid,
        source_label(src)
    );
    Ok(warnings)
}

#[cfg(feature = "lxc")]
async fn import_ct(
    state: &Arc<AppState>,
    req: &ImportRequest,
    _g: &source::Guest,
    p: &GuestPlan,
    undo: &mut Undo,
    progress: &(dyn Fn(f32, String) + Sync),
) -> Result<Vec<String>, ApiError> {
    use crate::container::lxc::RootfsSource;
    let src = &req.plan.source;
    let ssh = src.ssh_argv().is_some();
    let mut warnings: Vec<String> = p
        .rows
        .iter()
        .filter(|r| r.carry == conf::Carry::NotCarried)
        .map(|r| format!("{}: {}", r.key, r.note))
        .collect();
    let mut cfg =
        p.ct.clone()
            .ok_or_else(|| ApiError::Internal("no container in the plan".into()))?;
    cfg.id = p.target_id.clone();
    cfg.name = p.target_id.clone();
    let o = cfg.lxc.clone().unwrap_or_default();
    let lxc = state.container_manager.lxc();
    let lxc_dir = lxc.lxc_path().to_path_buf();
    std::fs::create_dir_all(&lxc_dir)?;
    let rootfs = p
        .volumes
        .iter()
        .find(|v| v.role == VolRole::Rootfs)
        .and_then(|v| v.resolved.clone())
        .ok_or_else(|| ApiError::BadRequest("the container's rootfs was not found".into()))?;
    let need = p.volumes.iter().filter_map(|v| v.size_bytes).sum::<u64>();
    crate::api::libvirt_import::check_free_space(need, free_bytes(&lxc_dir), &lxc_dir)
        .map_err(ApiError::BadRequest)?;
    let idmap = o.unprivileged.then_some((
        horcrux_common::lxc::LXC_DEFAULT_IDMAP_BASE,
        horcrux_common::lxc::LXC_DEFAULT_IDMAP_SIZE,
    ));

    // 1. the rootfs source
    progress(
        2.0,
        format!("copying the root file system ({})", rootfs.path),
    );
    let mut mounted: Option<PathBuf> = None;
    let rsrc = match (&rootfs.kind, ssh, src) {
        (_, _, Source::Vzdump { path }) => RootfsSource::Tar(PathBuf::from(path)),
        (VolKind::Dir, false, _) => RootfsSource::Dir {
            path: PathBuf::from(&rootfs.path),
            src_idmap: idmap,
        },
        (VolKind::Dir, true, _) => {
            let tar = lxc_dir.join(format!(".import-{}.tar", cfg.id));
            undo.files.push(tar.clone());
            let inner = match idmap {
                Some((b, n)) => format!("lxc-usernsexec -m b:0:{b}:{n} -- tar --numeric-owner --xattrs -cpf - '--exclude=./dev/*' -C {} .", source::sq(&rootfs.path)),
                None => format!("tar --numeric-owner --xattrs -cpf - '--exclude=./dev/*' -C {} .", source::sq(&rootfs.path)),
            };
            ssh_to_file(src, &inner, &tar).await?;
            check_tar(&tar)?;
            RootfsSource::Tar(tar)
        }
        (VolKind::Block | VolKind::File, false, _) => {
            // a raw rootfs image (LVM-thin, dir storage): mount it read-only
            let mnt = lxc_dir.join(format!(".import-{}-mnt", cfg.id));
            std::fs::create_dir_all(&mnt)?;
            let o2 = Command::new("mount")
                .args(["-o", "ro,noload"])
                .arg(&rootfs.path)
                .arg(&mnt)
                .output()
                .await?;
            if !o2.status.success() {
                let o3 = Command::new("mount")
                    .args(["-o", "ro"])
                    .arg(&rootfs.path)
                    .arg(&mnt)
                    .output()
                    .await?;
                if !o3.status.success() {
                    let _ = std::fs::remove_dir(&mnt);
                    return Err(ApiError::BadRequest(format!(
                        "cannot mount the rootfs image {}: {}",
                        rootfs.path,
                        String::from_utf8_lossy(&o3.stderr).trim()
                    )));
                }
            }
            mounted = Some(mnt.clone());
            RootfsSource::Dir {
                path: mnt,
                src_idmap: idmap,
            }
        }
        (VolKind::Block | VolKind::File, true, _) => {
            return Err(ApiError::BadRequest(
                "a container rootfs that is a disk image cannot be read over SSH: copy the image (or the configs and a received dataset) to this host and import the directory".into(),
            ));
        }
    };
    let created = lxc.create_container_from(&cfg, rsrc).await;
    if let Some(mnt) = mounted {
        let _ = Command::new("umount").arg(&mnt).output().await;
        let _ = std::fs::remove_dir(&mnt);
    }
    let created =
        created.map_err(|e| ApiError::BadRequest(format!("creating the container: {e}")))?;
    let registered = state
        .container_manager
        .register_existing(created.to_config())
        .await
        .map_err(|e| ApiError::Internal(format!("registering the container: {e}")))?;
    undo.ct = Some(registered.id.clone());
    for f in undo.files.drain(..) {
        let _ = std::fs::remove_file(f);
    }

    // 2. managed mount points: copy their data (owners already in the
    //    container's id map: same base on both sides)
    let ct_dir = lxc_dir.join(&cfg.name);
    let mounts = o.mounts.clone();
    let mvols: Vec<&conf::PlanVol> = p
        .volumes
        .iter()
        .filter(|v| v.role == VolRole::Mount)
        .collect();
    for (k, v) in mvols.iter().enumerate() {
        let Some(i) = v.index else { continue };
        let Some(m) = mounts.get(i) else { continue };
        let dst = crate::container::lxc_config::mount_source(&ct_dir, i, m);
        std::fs::create_dir_all(&dst)?;
        let r = v.resolved.clone().ok_or_else(|| {
            ApiError::BadRequest(format!("{}: volume {} not found", v.key, v.source))
        })?;
        progress(
            60.0 + 35.0 * k as f32 / mvols.len().max(1) as f32,
            format!("copying {} ({})", v.key, v.source),
        );
        if r.kind != VolKind::Dir {
            return Err(ApiError::BadRequest(format!(
                "{}: {} is not a directory volume",
                v.key, r.path
            )));
        }
        if ssh {
            let tar = lxc_dir.join(format!(".import-{}-{}.tar", cfg.id, v.key));
            undo.files.push(tar.clone());
            ssh_to_file(
                src,
                &format!(
                    "tar --numeric-owner --xattrs -cpf - -C {} .",
                    source::sq(&r.path)
                ),
                &tar,
            )
            .await?;
            let o2 = low_priority("tar")
                .args(["--numeric-owner", "--xattrs", "-xpf"])
                .arg(&tar)
                .arg("-C")
                .arg(&dst)
                .output()
                .await?;
            let _ = std::fs::remove_file(&tar);
            if !o2.status.success() {
                return Err(ApiError::Internal(format!(
                    "extracting {}: {}",
                    v.key,
                    String::from_utf8_lossy(&o2.stderr).trim()
                )));
            }
        } else {
            let o2 = low_priority("cp")
                .args(["-a", "--reflink=auto", "--"])
                .arg(format!("{}/.", r.path.trim_end_matches('/')))
                .arg(&dst)
                .output()
                .await?;
            if !o2.status.success() {
                return Err(ApiError::Internal(format!(
                    "copying {}: {}",
                    v.key,
                    String::from_utf8_lossy(&o2.stderr).trim()
                )));
            }
            let (a, b) = (tree_count(Path::new(&r.path)), tree_count(&dst));
            if a != b {
                return Err(ApiError::Internal(format!(
                    "{}: the copy has {} entries / {} bytes, the source {} / {}",
                    v.key, b.0, b.1, a.0, a.1
                )));
            }
        }
    }
    for m in &mounts {
        if let Some(s) = &m.source {
            if !Path::new(s).is_dir() {
                warnings.push(format!("bind mount source {s} does not exist on this host yet: create or restore it before starting the container"));
            }
        }
    }

    // 3. the old source: offered for removal (local sources only)
    if !ssh {
        use crate::api::import_cleanup::Item;
        let mut items = Vec::new();
        for v in p
            .volumes
            .iter()
            .filter(|v| matches!(v.role, VolRole::Rootfs | VolRole::Mount))
        {
            if let (Some(r), Some(ds)) = (&v.resolved, zfs_dataset_for_dir(v)) {
                let _ = r;
                items.push(Item {
                    kind: "zfs-dataset".into(),
                    path: ds,
                    requires: Some(ct_dir.to_string_lossy().to_string()),
                });
            } else if let (Some(r), Source::Vzdump { .. }) = (&v.resolved, src) {
                items.push(Item {
                    kind: "file".into(),
                    path: r.path.clone(),
                    requires: Some(ct_dir.to_string_lossy().to_string()),
                });
            }
        }
        items.dedup();
        let _ = crate::api::import_cleanup::write_record(
            state,
            &crate::api::import_cleanup::Record {
                vm_id: registered.id.clone(),
                source: "proxmox".into(),
                items,
                guest: Some("ct".into()),
            },
        );
    }
    undo.ct = None;
    undo.files.clear();
    tracing::info!(
        "container {}: imported from Proxmox guest {}",
        registered.id,
        p.vmid
    );
    Ok(warnings)
}

#[cfg(not(feature = "lxc"))]
async fn import_ct(
    _state: &Arc<AppState>,
    _req: &ImportRequest,
    _g: &source::Guest,
    _p: &GuestPlan,
    _undo: &mut Undo,
    _progress: &(dyn Fn(f32, String) + Sync),
) -> Result<Vec<String>, ApiError> {
    Err(ApiError::NotAvailable {
        reason: "lxc-not-built",
        message: "this Horcrux build has no LXC support".into(),
        hint: "Containers are imported as LXC containers: build Horcrux with USE=lxc".into(),
    })
}

/// The ZFS dataset behind a directory volume of a local source (from the
/// storage map: `<prefix>/<subvol>`), for the cleanup offer.
fn zfs_dataset_for_dir(v: &conf::PlanVol) -> Option<String> {
    let r = v.resolved.as_ref()?;
    if r.kind != VolKind::Dir {
        return None;
    }
    let out = std::process::Command::new("zfs")
        .args(["list", "-H", "-o", "name,mountpoint", "-t", "filesystem"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).lines().find_map(|l| {
        let (n, mp) = l.split_once('\t')?;
        (mp == r.path).then(|| n.to_string())
    })
}

/// (entries, bytes of regular files) of a tree, not crossing mounts.
fn tree_count(p: &Path) -> (u64, u64) {
    fn walk(p: &Path, dev: u64, acc: &mut (u64, u64)) {
        use std::os::unix::fs::MetadataExt;
        let Ok(rd) = std::fs::read_dir(p) else { return };
        for e in rd.flatten() {
            let Ok(m) = e.path().symlink_metadata() else {
                continue;
            };
            acc.0 += 1;
            if m.is_file() {
                acc.1 += m.len();
            }
            if m.is_dir() && m.dev() == dev {
                walk(&e.path(), dev, acc);
            }
        }
    }
    use std::os::unix::fs::MetadataExt;
    let dev = std::fs::metadata(p).map(|m| m.dev()).unwrap_or(0);
    let mut acc = (0, 0);
    walk(p, dev, &mut acc);
    acc
}

/// `path` starts with a tar header (`ustar` at 257): a remote shell that
/// prints anything to stdout would otherwise hand tar a broken archive.
fn check_tar(path: &Path) -> Result<(), ApiError> {
    use std::io::Read;
    let mut head = vec![0u8; 512];
    let n = std::fs::File::open(path)?.read(&mut head)?;
    if n < 263 || &head[257..262] != b"ustar" {
        return Err(ApiError::Internal(format!(
            "the Proxmox node did not send a tar archive (it starts with {:?}): check that root's shell start files print nothing",
            String::from_utf8_lossy(&head[..n.min(80)])
        )));
    }
    Ok(())
}

/// Run `cmd` on the SSH source and save its stdout in `to`.
async fn ssh_to_file(src: &Source, cmd: &str, to: &Path) -> Result<(), ApiError> {
    use tokio::io::AsyncReadExt;
    let argv = src
        .ssh_argv()
        .ok_or_else(|| ApiError::Internal("not ssh".into()))?;
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .arg(cmd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| ApiError::Internal(format!("ssh: {e}")))?;
    let mut out = child.stdout.take().expect("piped");
    let mut err = child.stderr.take().expect("piped");
    let errt = tokio::spawn(async move {
        let mut v = Vec::new();
        let _ = err.read_to_end(&mut v).await;
        v
    });
    let mut f = tokio::fs::File::create(to).await?;
    let n = tokio::io::copy(&mut out, &mut f).await?;
    f.sync_all().await?;
    let st = child.wait().await?;
    let stderr = errt.await.unwrap_or_default();
    if !st.success() {
        return Err(source::ssh_error(src, &String::from_utf8_lossy(&stderr)));
    }
    if n == 0 {
        return Err(ApiError::Internal(format!(
            "the Proxmox node sent nothing: {}",
            String::from_utf8_lossy(&stderr).trim()
        )));
    }
    Ok(())
}
