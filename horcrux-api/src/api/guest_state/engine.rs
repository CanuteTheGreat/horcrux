//! Saving, resuming and the host shutdown / boot passes (see mod.rs).
use super::crypt;
use super::hibernate;
use super::policy::{self, Effective, SaveEncryption, SaveFallback, StartAction, StopAction};
use crate::container::savestate as ctsave;
use crate::vm::savestate::VmSaveOutcome;
use crate::AppState;
use horcrux_common::{ContainerRuntime, Error, Result, VmHypervisor, VmStatus};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestKind {
    Vm,
    Container,
}

impl GuestKind {
    pub fn prefix(self) -> &'static str {
        match self {
            GuestKind::Vm => "vm",
            GuestKind::Container => "ct",
        }
    }
}

/// A guest as the power engine sees it.
#[derive(Debug, Clone)]
pub struct GuestInfo {
    pub kind: GuestKind,
    pub id: String,
    pub name: String,
    pub runtime: String,
    pub status: String,
    pub memory_mb: u64,
    pub qemu: bool,
    pub ct_runtime: Option<ContainerRuntime>,
    /// `incus` / `lxc` CLI for LXD/Incus VMs and containers.
    pub instance_cli: Option<&'static str>,
    /// A VM with host PCI / mediated devices: QEMU cannot save its state.
    pub passthrough: bool,
}

impl GuestInfo {
    pub fn is_up(&self) -> bool {
        self.status == "running" || self.status == "paused"
    }
}

/// A guest's saved state (the index file).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedGuest {
    pub kind: GuestKind,
    pub id: String,
    pub name: String,
    pub runtime: String,
    pub saved_at: String,
    pub mechanism: String,
    pub size_bytes: Option<u64>,
    /// Directory holding the state (`<storage>/<vm|ct>-<id>`).
    pub state_dir: String,
    pub state_file: Option<String>,
    pub vm: Option<VmSaveOutcome>,
    /// Set when the state was saved as part of a host shutdown that had to
    /// compromise.
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuestResult {
    pub at: String,
    /// `host-stop`, `host-boot`, `manual-save`, `manual-resume`, `discard`.
    pub phase: String,
    /// What was done: saved, shutdown, poweroff, resumed, started, kept, skipped, failed.
    pub action: String,
    pub ok: bool,
    /// Why state could not be saved / what went wrong (shown in the UI).
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuestOutcome {
    pub kind: GuestKind,
    pub id: String,
    pub name: String,
    pub action: String,
    pub ok: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub boot_id: String,
    pub written_at: String,
    /// true: written by a host shutdown; false: the periodic snapshot of
    /// what is running (all that is left after a crash).
    pub clean: bool,
    pub guests: Vec<ManifestGuest>,
    #[serde(default)]
    pub outcomes: Vec<GuestOutcome>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestGuest {
    pub kind: GuestKind,
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Report {
    pub outcomes: Vec<GuestOutcome>,
    /// The pass did not run (already done for this boot).
    pub skipped: bool,
    pub message: String,
}

// ---------------------------------------------------------------- paths

fn root(state: &AppState) -> PathBuf {
    state.config.paths.data_dir.join("saved-state")
}

pub fn safe(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn index_path(state: &AppState, k: GuestKind, id: &str) -> PathBuf {
    root(state)
        .join("index")
        .join(format!("{}-{}.json", k.prefix(), safe(id)))
}

pub fn token_path(data_dir: &Path) -> PathBuf {
    data_dir.join("saved-state").join("host-agent.token")
}

fn running_path(state: &AppState) -> PathBuf {
    root(state).join("running.json")
}

/// What the last host shutdown did to each guest. `running.json` is
/// refreshed every 20 s once the host is up again (the crash fallback), so
/// the outcomes - which guests were saved, which fell back and why - live in
/// their own file until the next host shutdown replaces them.
fn last_stop_path(state: &AppState) -> PathBuf {
    root(state).join("last-stop.json")
}

pub fn read_last_stop(state: &AppState) -> Option<Manifest> {
    serde_json::from_str(&std::fs::read_to_string(last_stop_path(state)).ok()?).ok()
}

fn boot_marker_path(state: &AppState) -> PathBuf {
    root(state).join("boot-pass.json")
}

pub fn boot_id() -> String {
    let f = std::env::var("HORCRUX_BOOT_ID_FILE")
        .unwrap_or_else(|_| "/proc/sys/kernel/random/boot_id".into());
    std::fs::read_to_string(f)
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    if let Ok(f) = std::fs::File::open(&tmp) {
        let _ = f.sync_all();
    }
    std::fs::rename(&tmp, path)
}

pub fn load_saved(state: &AppState, k: GuestKind, id: &str) -> Option<SavedGuest> {
    let t = std::fs::read_to_string(index_path(state, k, id)).ok()?;
    serde_json::from_str(&t).ok()
}

fn write_saved(state: &AppState, s: &SavedGuest) -> Result<()> {
    write_atomic(
        &index_path(state, s.kind, &s.id),
        serde_json::to_string_pretty(s).unwrap().as_bytes(),
    )
    .map_err(|e| Error::System(format!("cannot record the saved state: {e}")))
}

pub fn has_saved(state: &AppState, k: GuestKind, id: &str) -> bool {
    index_path(state, k, id).exists()
}

pub fn read_manifest(state: &AppState) -> Option<Manifest> {
    serde_json::from_str(&std::fs::read_to_string(running_path(state)).ok()?).ok()
}

fn write_manifest(state: &AppState, m: &Manifest) {
    if let Err(e) = write_atomic(
        &running_path(state),
        serde_json::to_string_pretty(m).unwrap().as_bytes(),
    ) {
        warn!("cannot write {}: {e}", running_path(state).display());
    }
}

/// Remove the index first (so the state can never be resumed again), then
/// the state files.
fn delete_saved(state: &AppState, s: &SavedGuest) {
    let _ = std::fs::remove_file(index_path(state, s.kind, &s.id));
    let dir = Path::new(&s.state_dir);
    let ours = dir
        .file_name()
        .map(|n| {
            n.to_string_lossy()
                .starts_with(&format!("{}-", s.kind.prefix()))
        })
        .unwrap_or(false);
    if ours && !s.state_dir.is_empty() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

// ----------------------------------------------------------- guest lookup

pub async fn list_guests(state: &Arc<AppState>) -> Vec<GuestInfo> {
    let mut out = Vec::new();
    for v in state.vm_manager.list_vms().await {
        out.push(vm_info(&v));
    }
    for c in state.container_manager.list_containers().await {
        out.push(ct_info(&c));
    }
    out
}

fn cli_of(rt: &ContainerRuntime) -> Option<&'static str> {
    match rt {
        ContainerRuntime::Incus => Some("incus"),
        ContainerRuntime::Lxd => Some("lxc"),
        _ => None,
    }
}

fn vm_info(v: &horcrux_common::VmConfig) -> GuestInfo {
    GuestInfo {
        kind: GuestKind::Vm,
        id: v.id.clone(),
        name: v.name.clone(),
        runtime: format!("{:?}", v.hypervisor).to_lowercase(),
        status: v.status.to_string(),
        memory_mb: v.memory,
        qemu: v.hypervisor == VmHypervisor::Qemu,
        ct_runtime: None,
        instance_cli: match v.hypervisor {
            VmHypervisor::Incus => Some("incus"),
            VmHypervisor::Lxd => Some("lxc"),
            _ => None,
        },
        passthrough: !v.hardware.hostpci.is_empty(),
    }
}

fn ct_info(c: &horcrux_common::ContainerConfig) -> GuestInfo {
    GuestInfo {
        kind: GuestKind::Container,
        id: c.id.clone(),
        name: c.name.clone(),
        runtime: format!("{:?}", c.runtime).to_lowercase(),
        status: c.status.to_string(),
        memory_mb: c.memory,
        qemu: false,
        ct_runtime: Some(c.runtime.clone()),
        instance_cli: cli_of(&c.runtime),
        passthrough: false,
    }
}

pub async fn find_guest(state: &Arc<AppState>, k: GuestKind, id: &str) -> Result<GuestInfo> {
    match k {
        GuestKind::Vm => Ok(vm_info(&state.vm_manager.get_vm(id).await?)),
        GuestKind::Container => Ok(ct_info(&state.container_manager.get_container(id).await?)),
    }
}

pub use find_guest as require_guest;

// ------------------------------------------------------------ guest locks

static BUSY: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

/// One save/resume/discard per guest at a time.
pub struct BusyGuard(String);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        BUSY.get().unwrap().lock().unwrap().remove(&self.0);
    }
}

fn lock_guest(k: GuestKind, id: &str) -> Result<BusyGuard> {
    let key = format!("{}:{}", k.prefix(), id);
    let set = BUSY.get_or_init(Default::default);
    if !set.lock().unwrap().insert(key.clone()) {
        return Err(Error::Conflict(format!(
            "another save/resume of {id} is already in progress"
        )));
    }
    Ok(BusyGuard(key))
}

// ----------------------------------------------------------------- results

async fn record(
    state: &Arc<AppState>,
    k: GuestKind,
    id: &str,
    phase: &str,
    action: &str,
    ok: bool,
    msg: Option<String>,
) {
    let r = GuestResult {
        at: chrono::Utc::now().to_rfc3339(),
        phase: phase.into(),
        action: action.into(),
        ok,
        message: msg,
    };
    let store = crate::db::config_store::ConfigStore::new(&state.database);
    if let Err(e) = store
        .put("guest_power_result", &format!("{}:{}", k.prefix(), id), &r)
        .await
    {
        warn!("cannot store the power result of {id}: {e}");
    }
}

async fn load_results(state: &Arc<AppState>) -> BTreeMap<String, GuestResult> {
    crate::db::config_store::ConfigStore::new(&state.database)
        .load::<GuestResult>("guest_power_result")
        .await
        .map(|v| v.into_iter().collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------- the CLI

async fn cli(bin: &str, args: &[&str], timeout: Duration) -> Result<String> {
    let out = tokio::time::timeout(
        timeout,
        tokio::process::Command::new(bin)
            .args(args)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| {
        Error::System(format!(
            "`{bin} {}` timed out after {} s",
            args.join(" "),
            timeout.as_secs()
        ))
    })?
    .map_err(|e| Error::System(format!("cannot run {bin}: {e}")))?;
    if !out.status.success() {
        return Err(Error::System(format!(
            "`{bin} {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// LXD/Incus need `migration.stateful=true` on an instance for a stateful stop.
pub async fn prepare_stateful(state: &Arc<AppState>, k: GuestKind, id: &str) {
    if let Ok(g) = find_guest(state, k, id).await {
        if let Some(c) = g.instance_cli {
            let _ = cli(
                c,
                &["config", "set", &g.name, "migration.stateful=true"],
                Duration::from_secs(30),
            )
            .await;
        }
    }
}

// ------------------------------------------------------------------- save

/// Where a guest's saved state physically ends up: our directory for
/// QEMU VMs, LXC and Docker; the runtime's own storage for the runtimes
/// that keep the state themselves.
pub fn save_target_dir(g: &GuestInfo, base: &Path) -> PathBuf {
    match (&g.ct_runtime, g.qemu) {
        (_, true) => base.to_path_buf(),
        (Some(ContainerRuntime::Lxc), _) | (Some(ContainerRuntime::Docker), _) => {
            base.to_path_buf()
        }
        (Some(ContainerRuntime::Incus), _) => PathBuf::from("/var/lib/incus"),
        (Some(ContainerRuntime::Lxd), _) => PathBuf::from("/var/lib/lxd"),
        (Some(ContainerRuntime::Podman), _) => PathBuf::from("/var/lib/containers"),
        (None, false) => match g.instance_cli {
            Some("lxc") => PathBuf::from("/var/lib/lxd"),
            _ => PathBuf::from("/var/lib/incus"),
        },
    }
}

/// Free-space check for a saved state in `dir`; the directory is private to
/// root (a RAM image is as sensitive as the guest's secrets).
fn space_ok(dir: &Path, mem_mb: u64) -> Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|e| Error::System(format!("cannot create {}: {e}", dir.display())))?;
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    crate::vm::savestate::check_free_space(dir, mem_mb)
}

/// Save a running guest to disk and stop it. On error the guest is left
/// running (as far as the runtime allows) and nothing is recorded.
pub async fn save_guest(
    state: &Arc<AppState>,
    k: GuestKind,
    id: &str,
    dir_override: Option<String>,
) -> Result<SavedGuest> {
    let _g = lock_guest(k, id)?;
    let r = save_locked(state, k, id, dir_override, None).await;
    match &r {
        Ok(_) => record(state, k, id, "manual-save", "saved", true, None).await,
        Err(e) => {
            record(
                state,
                k,
                id,
                "manual-save",
                "failed",
                false,
                Some(e.to_string()),
            )
            .await
        }
    }
    r
}

/// Why a VM with host devices cannot be saved (the kernel's answer per
/// device is in `reason`).
fn device_blocker(reason: Option<&str>) -> String {
    format!(
        "the VM owns passthrough host devices whose state cannot be saved ({}): remove them or shut the VM down",
        reason.unwrap_or("no reason reported")
    )
}

async fn save_locked(
    state: &Arc<AppState>,
    k: GuestKind,
    id: &str,
    dir_override: Option<String>,
    note: Option<String>,
) -> Result<SavedGuest> {
    let g = find_guest(state, k, id).await?;
    if has_saved(state, k, id) {
        return Err(Error::Conflict(format!(
            "{} already has a saved state; resume it or discard it first",
            g.name
        )));
    }
    if !g.is_up() {
        return Err(Error::Conflict(format!("{} is not running", g.name)));
    }
    let eff = policy::effective(state, k, id).await;
    if g.passthrough && g.qemu {
        let rep = state
            .vm_manager
            .device_save_report(id, eff.device_save)
            .await?;
        if !rep.may_save() {
            return Err(Error::Conflict(device_blocker(rep.reason.as_deref())));
        }
    }
    let base = match dir_override.as_deref().filter(|d| !d.trim().is_empty()) {
        Some(d) => {
            if !d.starts_with('/')
                || d.contains(['\'', '\n', '\0'])
                || d.split('/').any(|p| p == "..")
            {
                return Err(Error::Validation(
                    "the saved-state directory must be an absolute path".into(),
                ));
            }
            PathBuf::from(d)
        }
        None => eff.save_dir.clone(),
    };
    // Saved memory holds secrets: only onto encrypted storage (unless the
    // administrator opted out).
    let target = save_target_dir(&g, &base);
    crypt::verdict(&crypt::probe(&target).await, eff.save_encryption).map_err(Error::Conflict)?;
    let dir = base.join(format!("{}-{}", k.prefix(), safe(id)));
    let timeout = Duration::from_secs(eff.save_timeout_secs);
    let mut saved = SavedGuest {
        kind: k,
        id: id.into(),
        name: g.name.clone(),
        runtime: g.runtime.clone(),
        saved_at: chrono::Utc::now().to_rfc3339(),
        mechanism: String::new(),
        size_bytes: None,
        state_dir: dir.to_string_lossy().into_owned(),
        state_file: None,
        vm: None,
        note,
    };
    match k {
        GuestKind::Vm if g.qemu => {
            let file = dir.join("vmstate.bin");
            let _ = std::fs::remove_dir_all(&dir);
            if let Err(e) = space_ok(&dir, g.memory_mb) {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(e);
            }
            let out = match state.vm_manager.save_state_to(id, &file, timeout).await {
                Ok(o) => o,
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&dir);
                    return Err(e);
                }
            };
            saved.mechanism = format!("QEMU migrate-to-file ({})", out.accel);
            saved.size_bytes = Some(out.size_bytes);
            saved.state_file = Some(file.to_string_lossy().into_owned());
            saved.vm = Some(out);
        }
        GuestKind::Vm => {
            let c = g.instance_cli.ok_or_else(|| {
                Error::Validation(format!("{} VMs cannot be saved to a file", g.runtime))
            })?;
            let _ = cli(
                c,
                &["config", "set", &g.name, "migration.stateful=true"],
                Duration::from_secs(30),
            )
            .await;
            cli(c, &["stop", &g.name, "--stateful"], timeout).await.map_err(|e| {
                Error::System(format!(
                    "{e}. A {c} VM can only be saved if it was started with migration.stateful=true \
                     (Horcrux has just set it: restart the VM once and try again)"
                ))
            })?;
            let _ = state.vm_manager.record_status(id, VmStatus::Stopped).await;
            saved.mechanism = format!("{c} stop --stateful");
        }
        GuestKind::Container => {
            let rt = g.ct_runtime.clone().unwrap();
            // Runtimes that keep the state themselves need no space here.
            if matches!(rt, ContainerRuntime::Lxc | ContainerRuntime::Docker) {
                if let Err(e) = space_ok(&dir, g.memory_mb) {
                    let _ = std::fs::remove_dir_all(&dir);
                    return Err(e);
                }
            }
            let ck = dir.join("ckpt");
            match state
                .container_manager
                .save_container_state(id, &ck, timeout)
                .await
            {
                Ok(o) => {
                    saved.mechanism = o.mechanism;
                    saved.size_bytes = o.size_bytes;
                }
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&dir);
                    return Err(e);
                }
            }
        }
    }
    write_saved(state, &saved)?;
    info!("saved state of {} {} ({})", k.prefix(), id, saved.mechanism);
    Ok(saved)
}

// -------------------------------------------------------- guest hibernate

/// Hibernate a running QEMU VM through its guest agent now (the same step the
/// host shutdown uses for passthrough VMs); the VM is stopped when the guest
/// has written its image and powered off. Start it again to resume.
pub async fn hibernate_guest(state: &Arc<AppState>, k: GuestKind, id: &str) -> Result<String> {
    if k != GuestKind::Vm {
        return Err(Error::Validation("only VMs can hibernate".into()));
    }
    let _g = lock_guest(k, id)?;
    let g = find_guest(state, k, id).await?;
    if !g.qemu {
        return Err(Error::Validation(format!(
            "{} VMs cannot hibernate through the guest agent",
            g.runtime
        )));
    }
    if g.status != "running" {
        return Err(Error::Conflict(format!("{} is not running", g.name)));
    }
    let eff = policy::effective(state, k, id).await;
    let r = hibernate::hibernate(state, &g, &eff).await;
    match &r {
        Ok(m) => {
            record(
                state,
                k,
                id,
                "manual-hibernate",
                "hibernated",
                true,
                Some(m.clone()),
            )
            .await
        }
        Err(e) => {
            record(
                state,
                k,
                id,
                "manual-hibernate",
                "failed",
                false,
                Some(e.to_string()),
            )
            .await
        }
    }
    r
}

// ----------------------------------------------------------------- resume

/// Resume a guest from its saved state. The state is deleted after the
/// guest runs; on failure it is kept and the error recorded.
pub async fn resume_guest(state: &Arc<AppState>, k: GuestKind, id: &str) -> Result<()> {
    let _g = lock_guest(k, id)?;
    let r = resume_locked(state, k, id).await;
    match &r {
        Ok(_) => record(state, k, id, "manual-resume", "resumed", true, None).await,
        Err(e) => {
            record(
                state,
                k,
                id,
                "manual-resume",
                "failed",
                false,
                Some(format!("{e} (the saved state was kept)")),
            )
            .await
        }
    }
    r
}

async fn resume_locked(state: &Arc<AppState>, k: GuestKind, id: &str) -> Result<()> {
    let saved = load_saved(state, k, id)
        .ok_or_else(|| Error::NotFound(format!("{id} has no saved state")))?;
    let g = find_guest(state, k, id).await?;
    if g.is_up() {
        return Err(Error::Conflict(format!(
            "{} is already running; its saved state is stale and can only be discarded",
            g.name
        )));
    }
    let eff = policy::effective(state, k, id).await;
    let timeout = Duration::from_secs(eff.save_timeout_secs.max(60));
    let dir = PathBuf::from(&saved.state_dir);
    match k {
        GuestKind::Vm if g.qemu => {
            let (file, out) = match (&saved.state_file, &saved.vm) {
                (Some(f), Some(o)) => (PathBuf::from(f), o),
                _ => return Err(Error::System("the saved-state record is incomplete".into())),
            };
            state
                .vm_manager
                .resume_from_file(id, &file, out, timeout)
                .await?;
        }
        GuestKind::Vm => {
            let c = g.instance_cli.unwrap_or("incus");
            cli(c, &["start", &g.name], timeout).await?;
            let _ = state.vm_manager.record_status(id, VmStatus::Running).await;
        }
        GuestKind::Container => {
            state
                .container_manager
                .resume_container_state(id, &dir.join("ckpt"), timeout)
                .await?;
            state
                .container_manager
                .discard_container_state(id, &dir.join("ckpt"))
                .await;
        }
    }
    delete_saved(state, &saved);
    info!("resumed {} {} from its saved state", k.prefix(), id);
    Ok(())
}

/// Throw the saved state away: the guest then starts normally. LXD/Incus
/// keep the state inside the instance, which only `start --stateless` drops
/// (that boots the guest, which is stopped again right away).
pub async fn discard_guest(state: &Arc<AppState>, k: GuestKind, id: &str) -> Result<()> {
    let _g = lock_guest(k, id)?;
    let saved = load_saved(state, k, id)
        .ok_or_else(|| Error::NotFound(format!("{id} has no saved state")))?;
    let g = find_guest(state, k, id).await?;
    if !g.is_up() {
        if let Some(c) = g.instance_cli {
            cli(
                c,
                &["start", "--stateless", &g.name],
                Duration::from_secs(120),
            )
            .await?;
            cli(c, &["stop", "--force", &g.name], Duration::from_secs(120)).await?;
            if k == GuestKind::Vm {
                let _ = state.vm_manager.record_status(id, VmStatus::Stopped).await;
            }
        }
    }
    if k == GuestKind::Container {
        state
            .container_manager
            .discard_container_state(id, &Path::new(&saved.state_dir).join("ckpt"))
            .await;
    }
    delete_saved(state, &saved);
    record(state, k, id, "discard", "discarded", true, None).await;
    Ok(())
}

// ----------------------------------------------------------- start/stop

async fn start_plain(state: &Arc<AppState>, g: &GuestInfo) -> Result<()> {
    match g.kind {
        GuestKind::Vm => state.vm_manager.start_vm(&g.id).await.map(|_| ()),
        GuestKind::Container => state
            .container_manager
            .start_container(&g.id)
            .await
            .map(|_| ()),
    }
}

async fn clean_shutdown(state: &Arc<AppState>, g: &GuestInfo, secs: u64) -> Result<()> {
    match g.kind {
        GuestKind::Vm if g.qemu => {
            if g.status == "paused" {
                let _ = state.vm_manager.resume_vm(&g.id).await;
            }
            state
                .vm_manager
                .shutdown_vm(&g.id, Duration::from_secs(secs), true)
                .await
                .map(|_| ())
        }
        GuestKind::Vm => state.vm_manager.stop_vm(&g.id).await.map(|_| ()),
        GuestKind::Container if g.ct_runtime == Some(ContainerRuntime::Lxc) => {
            #[cfg(feature = "lxc")]
            {
                state
                    .container_manager
                    .shutdown_container(&g.id, secs as u32, true)
                    .await
                    .map(|_| ())
            }
            #[cfg(not(feature = "lxc"))]
            {
                state
                    .container_manager
                    .stop_container(&g.id)
                    .await
                    .map(|_| ())
            }
        }
        GuestKind::Container => state
            .container_manager
            .stop_container(&g.id)
            .await
            .map(|_| ()),
    }
}

async fn poweroff(state: &Arc<AppState>, g: &GuestInfo) -> Result<()> {
    match g.kind {
        GuestKind::Vm => state.vm_manager.stop_vm(&g.id).await.map(|_| ()),
        GuestKind::Container => state
            .container_manager
            .stop_container(&g.id)
            .await
            .map(|_| ()),
    }
}

/// Apply the stop action to one running guest: Save falls back to Shutdown
/// (with the reason, which the UI shows), Shutdown to Power off.
async fn stop_one(state: &Arc<AppState>, g: GuestInfo, eff: Effective) -> GuestOutcome {
    let out = |action: &str, ok: bool, message: Option<String>| GuestOutcome {
        kind: g.kind,
        id: g.id.clone(),
        name: g.name.clone(),
        action: action.into(),
        ok,
        message,
    };
    let mut warning = None;
    if eff.stop_action == StopAction::None {
        return out(
            "left",
            true,
            Some(
                "The stop action is None: the guest was left alone and ends with the host.".into(),
            ),
        );
    }
    let guard = lock_guest(g.kind, &g.id);
    let mut fell_back = false;
    // A VM that owns hardware whose state cannot be saved (a whole GPU; host
    // devices with kernel-reported migration ARE saved below): the guest
    // hibernates itself (own timeout), else the save fallback below.
    let device_blocker_now = if eff.stop_action == StopAction::Save && g.passthrough && g.qemu {
        match state
            .vm_manager
            .device_save_report(&g.id, eff.device_save)
            .await
        {
            Ok(r) if r.may_save() => None,
            Ok(r) => Some(device_blocker(r.reason.as_deref())),
            Err(_) => Some(device_blocker(Some("the devices could not be read"))),
        }
    } else {
        None
    };
    if let (Some(no_save), true) = (&device_blocker_now, guard.is_ok()) {
        if eff.guest_hibernate {
            match hibernate::hibernate(state, &g, &eff).await {
                Ok(method) => {
                    return out(
                        "hibernated",
                        true,
                        Some(format!(
                            "The guest hibernated itself ({method}); it resumes when the VM starts at boot."
                        )),
                    )
                }
                Err(e) => {
                    warn!("vm {}: guest hibernate failed: {e}", g.id);
                    fell_back = true;
                    warning = Some(format!(
                        "Guest hibernate was not possible ({e}). State could not be saved: {no_save}. The guest was shut down instead and will be started again."
                    ));
                }
            }
        } else {
            fell_back = true;
            warning = Some(format!(
                "State could not be saved: {no_save}. Guest hibernate is off for this VM. The guest was shut down instead and will be started again."
            ));
        }
    }
    if eff.stop_action == StopAction::Save && !fell_back {
        match &guard {
            Ok(_) => match save_locked(state, g.kind, &g.id, None, None).await {
                Ok(_) => return out("saved", true, None),
                Err(e) => {
                    warn!(
                        "{} {}: cannot save state: {e}; shutting down instead",
                        g.kind.prefix(),
                        g.id
                    );
                    fell_back = true;
                    warning = Some(format!("State could not be saved: {e}. The guest was shut down instead and will be started again."));
                }
            },
            Err(e) => {
                fell_back = true;
                warning = Some(format!("State could not be saved: {e}"))
            }
        }
    }
    let straight_to_poweroff = eff.stop_action == StopAction::Poweroff
        || (fell_back && eff.save_fallback == SaveFallback::Poweroff);
    if !straight_to_poweroff {
        match clean_shutdown(state, &g, eff.shutdown_timeout_secs).await {
            Ok(_) => return out("shutdown", true, warning),
            Err(e) => {
                warning = Some(format!(
                    "{}Clean shutdown failed ({e}); powered off.",
                    warning.map(|w| format!("{w} ")).unwrap_or_default()
                ));
            }
        }
    }
    match poweroff(state, &g).await {
        Ok(_) => out("poweroff", true, warning),
        Err(e) => out(
            "failed",
            false,
            Some(format!(
                "{}{e}",
                warning.map(|w| format!("{w} ")).unwrap_or_default()
            )),
        ),
    }
}

/// Seconds the host stop pass may take, from the configured timeouts and the
/// number of guests that could be running. The stop client waits this long
/// (plus nothing hard-coded) and the UI warns when an init system's own
/// shutdown limit is shorter.
pub async fn stop_budget(state: &Arc<AppState>) -> u64 {
    let node = policy::load_node(state).await;
    let n = list_guests(state)
        .await
        .iter()
        .filter(|g| g.is_up())
        .count();
    node.stop_budget_secs(n)
}

static STOPPING: AtomicBool = AtomicBool::new(false);
static BOOT_DONE: AtomicBool = AtomicBool::new(false);
static BOOT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Host shutdown: apply every running guest's stop action, in parallel
/// (bounded by the node's `max_parallel`), and record what was running so the
/// next boot can bring it back.
pub async fn stop_all(state: &Arc<AppState>) -> Report {
    STOPPING.store(true, Ordering::SeqCst);
    let node = policy::load_node(state).await;
    // Clustered: move running VMs to other nodes first (live migration);
    // what stays is handled by the stop action below.
    #[allow(unused_mut)]
    let mut moved: Vec<GuestOutcome> = Vec::new();
    #[allow(unused_mut)]
    let mut stayed: std::collections::HashMap<String, String> = Default::default();
    #[cfg(feature = "cluster")]
    if node.drain_on_shutdown {
        if let Some((m, s)) =
            crate::api::drain::drain_for_shutdown(state, node.drain_timeout_secs).await
        {
            for (id, to) in m {
                moved.push(GuestOutcome {
                    kind: GuestKind::Vm,
                    name: id.clone(),
                    id,
                    action: "migrated".into(),
                    ok: true,
                    message: Some(format!("moved to {to} without a restart")),
                });
            }
            stayed = s;
        }
    }
    let running: Vec<GuestInfo> = list_guests(state)
        .await
        .into_iter()
        .filter(|g| g.is_up())
        .collect();
    info!("host shutdown: {} running guest(s)", running.len());
    let manifest_guests: Vec<ManifestGuest> = running
        .iter()
        .map(|g| ManifestGuest {
            kind: g.kind,
            id: g.id.clone(),
            name: g.name.clone(),
        })
        .collect();
    // Written first: if this pass is cut short, the boot still knows.
    write_manifest(
        state,
        &Manifest {
            boot_id: boot_id(),
            written_at: chrono::Utc::now().to_rfc3339(),
            clean: false,
            guests: manifest_guests.clone(),
            outcomes: vec![],
        },
    );
    let sem = Arc::new(tokio::sync::Semaphore::new(node.max_parallel.max(1)));
    // Reverse of the start order: what started last is handled first (the
    // semaphore queue keeps this order).
    let mut ordered = Vec::new();
    for g in running {
        let eff = policy::effective(state, g.kind, &g.id).await;
        ordered.push((eff, g));
    }
    ordered.sort_by(|a, b| {
        (std::cmp::Reverse(a.0.start_order), &a.1.name)
            .cmp(&(std::cmp::Reverse(b.0.start_order), &b.1.name))
    });
    let mut tasks = Vec::new();
    for (eff, g) in ordered {
        let st = state.clone();
        let sem = sem.clone();
        tasks.push(tokio::spawn(async move {
            let _permit = sem.acquire_owned().await;
            let o = stop_one(&st, g, eff).await;
            record(
                &st,
                o.kind,
                &o.id,
                "host-stop",
                &o.action,
                o.ok,
                o.message.clone(),
            )
            .await;
            o
        }));
    }
    let mut outcomes = Vec::new();
    for t in tasks {
        if let Ok(mut o) = t.await {
            if let Some(why) = stayed.get(&o.id) {
                o.message = Some(format!(
                    "Could not be moved to another node ({why}). {}",
                    o.message.unwrap_or_default()
                ));
            }
            outcomes.push(o);
        }
    }
    let local_outcomes = outcomes.len();
    outcomes.extend(moved);
    let done = Manifest {
        boot_id: boot_id(),
        written_at: chrono::Utc::now().to_rfc3339(),
        clean: true,
        guests: manifest_guests,
        outcomes: outcomes.clone(),
    };
    write_manifest(state, &done);
    if let Err(e) = write_atomic(
        &last_stop_path(state),
        serde_json::to_string_pretty(&done).unwrap().as_bytes(),
    ) {
        warn!("cannot write {}: {e}", last_stop_path(state).display());
    }
    let saved = outcomes.iter().filter(|o| o.action == "saved").count();
    Report {
        message: format!(
            "{} guest(s) handled here, {} saved, {} moved to other nodes",
            local_outcomes,
            saved,
            outcomes.len() - local_outcomes
        ),
        outcomes,
        skipped: false,
    }
}

// ------------------------------------------------------------------- boot

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootAction {
    Resume,
    Start,
    /// Stopped on purpose; saved state (if any) stays.
    Skip,
    /// An HA resource: the HA loop owns its start (see [`ha_boot_override`]).
    LeaveToHa,
}

/// A guest the HA manager owns is never started or resumed by the boot pass. After a crash the
/// cluster may have restarted it on another node; starting it here as well runs it twice on the
/// same disks (lab cluster-hasuite j01296, 'hard crash, node stays down': split brain ~10 s after
/// the node came back). The HA loop restarts it here once it has synced the placement.
pub fn ha_boot_override(act: BootAction, ha_managed: bool) -> BootAction {
    match act {
        BootAction::Resume | BootAction::Start if ha_managed => BootAction::LeaveToHa,
        a => a,
    }
}

/// Is this guest an HA resource (VMs only)?
async fn ha_managed(state: &Arc<AppState>, kind: GuestKind, id: &str) -> bool {
    #[cfg(feature = "cluster")]
    if kind == GuestKind::Vm {
        return state.ha_manager.get_resource(id).await.is_some();
    }
    let _ = (state, kind, id);
    false
}

/// What to do with a guest at boot (pure; unit-tested).
pub fn boot_decision(start: StartAction, has_saved: bool, was_running: bool) -> BootAction {
    match start {
        StartAction::Nothing => BootAction::Skip,
        StartAction::Always if has_saved => BootAction::Resume,
        StartAction::Always => BootAction::Start,
        StartAction::ResumeIfRunning if has_saved => BootAction::Resume,
        StartAction::ResumeIfRunning if was_running => BootAction::Start,
        StartAction::ResumeIfRunning => BootAction::Skip,
    }
}

/// Boot pass: resume / start guests per policy, in start order, each after
/// its start delay (counted from the beginning of the pass). Runs once per
/// host boot unless `force`.
pub async fn boot_all(state: &Arc<AppState>, force: bool) -> Report {
    let _l = BOOT_LOCK.lock().await;
    let bid = boot_id();
    if !force {
        let done = std::fs::read_to_string(boot_marker_path(state))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v["boot_id"].as_str().map(str::to_string));
        if done.as_deref() == Some(bid.as_str()) {
            BOOT_DONE.store(true, Ordering::SeqCst);
            return Report {
                skipped: true,
                message: "the boot pass already ran for this boot".into(),
                ..Default::default()
            };
        }
    }
    let manifest = read_manifest(state);
    let was: Vec<ManifestGuest> = manifest
        .as_ref()
        .map(|m| m.guests.clone())
        .unwrap_or_default();
    // guests that put themselves to sleep at the last host shutdown
    let hibernated: HashSet<String> = manifest
        .as_ref()
        .map(|m| {
            m.outcomes
                .iter()
                .filter(|o| o.action == "hibernated")
                .map(|o| format!("{}:{}", o.kind.prefix(), o.id))
                .collect()
        })
        .unwrap_or_default();
    let guests = list_guests(state).await;
    let mut plan = Vec::new();
    for g in guests {
        let eff = policy::effective(state, g.kind, &g.id).await;
        let was_running = was.iter().any(|w| w.kind == g.kind && w.id == g.id);
        let saved = has_saved(state, g.kind, &g.id);
        let act = ha_boot_override(
            boot_decision(eff.start_action, saved, was_running),
            ha_managed(state, g.kind, &g.id).await,
        );
        plan.push((eff.start_order, eff.start_delay_secs, g, act, saved));
    }
    plan.sort_by(|a, b| (a.0, a.1, &a.2.name).cmp(&(b.0, b.1, &b.2.name)));
    let t0 = tokio::time::Instant::now();
    let stagger = Duration::from_secs(policy::load_node(state).await.resume_stagger_secs as u64);
    let mut started_any = false;
    let mut outcomes = Vec::new();
    for (_, delay, g, act, saved) in plan {
        let mk = |action: &str, ok: bool, message: Option<String>| GuestOutcome {
            kind: g.kind,
            id: g.id.clone(),
            name: g.name.clone(),
            action: action.into(),
            ok,
            message,
        };
        let o = match act {
            BootAction::LeaveToHa => {
                // Not started here: the cluster may have restarted it on another node while
                // this one was down. The HA loop starts it after it has adopted the newest
                // placement (and only with a quorum); a stale copy is never started.
                mk(
                    "left-to-ha",
                    true,
                    Some("HA-managed guest: the HA manager decides where it runs".into()),
                )
            }
            BootAction::Skip => {
                if saved {
                    mk(
                        "kept",
                        true,
                        Some("start action is Nothing: the saved state was kept".into()),
                    )
                } else {
                    continue;
                }
            }
            _ if g.is_up() => mk("already-running", true, None),
            _ => {
                tokio::time::sleep_until(t0 + Duration::from_secs(delay as u64)).await;
                if started_any && !stagger.is_zero() {
                    tokio::time::sleep(stagger).await;
                }
                started_any = true;
                if act == BootAction::Resume {
                    let _guard = lock_guest(g.kind, &g.id);
                    match resume_locked(state, g.kind, &g.id).await {
                        Ok(_) => mk("resumed", true, None),
                        Err(e) => mk(
                            "failed",
                            false,
                            Some(format!("Resuming the saved state failed: {e}. The state was kept; fix the problem and press Resume, or Discard it to start the guest normally.")),
                        ),
                    }
                } else {
                    match start_plain(state, &g).await {
                        Ok(_) if hibernated.contains(&format!("{}:{}", g.kind.prefix(), g.id)) => {
                            mk(
                                "started",
                                true,
                                Some("Started: the guest resumes from its own hibernation.".into()),
                            )
                        }
                        Ok(_) => mk("started", true, None),
                        Err(e) => mk("failed", false, Some(e.to_string())),
                    }
                }
            }
        };
        record(
            state,
            o.kind,
            &o.id,
            "host-boot",
            &o.action,
            o.ok,
            o.message.clone(),
        )
        .await;
        outcomes.push(o);
    }
    #[cfg(feature = "cluster")]
    crate::api::drain::leave_boot_maintenance(state).await;
    let _ = write_atomic(
        &boot_marker_path(state),
        serde_json::json!({"boot_id": bid, "at": chrono::Utc::now().to_rfc3339()})
            .to_string()
            .as_bytes(),
    );
    BOOT_DONE.store(true, Ordering::SeqCst);
    STOPPING.store(false, Ordering::SeqCst);
    let failed = outcomes.iter().filter(|o| !o.ok).count();
    Report {
        message: format!("{} guest(s) handled, {} failed", outcomes.len(), failed),
        outcomes,
        skipped: false,
    }
}

/// The host-shutdown half of guest power only works when the
/// `horcrux-guests` boot service is enabled AND started (init systems stop
/// only started services at shutdown); without it every running VM is
/// killed with QEMU at power-off. The packages install the service but a
/// desktop edition never enabled it, so the manager does: once per node
/// (a marker file records it, so an administrator who later disables the
/// service is not overridden), and only where the manager itself is a boot
/// service. Opt out with `HORCRUX_GUESTS_NO_SERVICE=1`. Never fails the
/// manager.
fn ensure_guests_service(data_dir: &Path) {
    if std::env::var_os("HORCRUX_GUESTS_NO_SERVICE").is_some() {
        return;
    }
    let marker = data_dir.join("saved-state/.guests-service-enabled");
    if marker.exists() {
        return;
    }
    let p = Path::new;
    let enabled = if p("/sbin/openrc-run").exists() && p("/etc/init.d/horcrux-guests").exists() {
        let rl = p("/etc/runlevels/default");
        if !rl.join("horcrux-api").exists() && !rl.join("horcrux").exists() {
            return; // the manager isn't started at boot either
        }
        let link = rl.join("horcrux-guests");
        std::fs::symlink_metadata(&link).is_ok()
            || std::os::unix::fs::symlink("/etc/init.d/horcrux-guests", &link).is_ok()
    } else if p("/run/systemd/system").is_dir() {
        let wants = p("/etc/systemd/system/multi-user.target.wants");
        if !wants.join("horcrux-api.service").exists() && !wants.join("horcrux.service").exists() {
            return;
        }
        let unit = [
            "/usr/lib/systemd/system",
            "/lib/systemd/system",
            "/etc/systemd/system",
        ]
        .iter()
        .map(|d| p(d).join("horcrux-guests.service"))
        .find(|u| u.exists());
        let Some(unit) = unit else { return };
        let link = wants.join("horcrux-guests.service");
        std::fs::symlink_metadata(&link).is_ok() || std::os::unix::fs::symlink(&unit, &link).is_ok()
    } else {
        return;
    };
    if !enabled {
        warn!("guest power: could not enable the horcrux-guests boot service");
        return;
    }
    let _ = std::fs::create_dir_all(marker.parent().unwrap_or(Path::new("/")));
    let _ = std::fs::write(&marker, chrono::Utc::now().to_rfc3339());
    info!("guest power: horcrux-guests enabled at boot (guests are saved/shut down cleanly when the host goes down)");
    // Start it for this boot too, once the manager is up: its start runs the
    // boot pass, which is a no-op when it already ran for this boot.
    tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(60)).await;
        let (prog, args): (&str, &[&str]) = if Path::new("/sbin/openrc-run").exists() {
            ("rc-service", &["horcrux-guests", "start"])
        } else {
            (
                "systemctl",
                &["start", "--no-block", "horcrux-guests.service"],
            )
        };
        match tokio::process::Command::new(prog)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
        {
            Ok(s) if s.success() => info!("guest power: horcrux-guests started"),
            other => warn!("guest power: starting horcrux-guests: {other:?}"),
        }
    });
}

/// Background work: the host-agent token, the boot pass at API start (unless
/// `HORCRUX_GUEST_AUTOSTART=0`, when the unit's `guests boot` does it) and the
/// periodic snapshot of what is running (the crash fallback).
pub fn spawn(state: Arc<AppState>) {
    if let Err(e) = super::agent::ensure_token(&state.config.paths.data_dir) {
        warn!("host-agent token: {e}");
    }
    ensure_guests_service(&state.config.paths.data_dir);
    let st = state.clone();
    tokio::spawn(async move {
        if std::env::var("HORCRUX_GUEST_AUTOSTART").as_deref() != Ok("0") {
            // Let host bridges and container runtimes come up first.
            tokio::time::sleep(Duration::from_secs(
                std::env::var("HORCRUX_GUEST_BOOT_DELAY_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(8),
            ))
            .await;
            let r = boot_all(&st, false).await;
            info!("guest boot pass: {}", r.message);
        }
    });
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(20)).await;
            if BOOT_DONE.load(Ordering::SeqCst) && !STOPPING.load(Ordering::SeqCst) {
                let guests: Vec<ManifestGuest> = list_guests(&state)
                    .await
                    .into_iter()
                    .filter(|g| g.is_up())
                    .map(|g| ManifestGuest {
                        kind: g.kind,
                        id: g.id,
                        name: g.name,
                    })
                    .collect();
                write_manifest(
                    &state,
                    &Manifest {
                        boot_id: boot_id(),
                        written_at: chrono::Utc::now().to_rfc3339(),
                        clean: false,
                        guests,
                        outcomes: vec![],
                    },
                );
            }
        }
    });
}

// -------------------------------------------------------------- UI views

#[derive(Serialize)]
pub struct GuestView {
    pub kind: GuestKind,
    pub id: String,
    pub name: String,
    pub runtime: String,
    pub status: String,
    pub policy: Effective,
    #[serde(rename = "override")]
    pub overrides: policy::GuestOverride,
    pub saved: Option<SavedGuest>,
    pub can_save: bool,
    /// Why state cannot be saved (CRIU missing, ...): shown as a warning.
    pub save_blocker: Option<String>,
    /// What the host shutdown will really do to this guest right now:
    /// save | shutdown | poweroff | none (after the fallback is applied).
    pub host_stop_action: String,
    /// Why it is not the configured action (passthrough, unencrypted
    /// target, no CRIU, ...).
    pub host_stop_reason: Option<String>,
    /// Can the guest hibernate itself (the method for passthrough VMs)?
    /// `None` = not asked (not a running passthrough VM).
    pub can_hibernate: Option<bool>,
    /// Why not (guest tools missing, no swap, ...), or how it will.
    pub hibernate_detail: Option<String>,
    /// VMs with host devices only: what the kernel says about saving their
    /// state (per device, with the reason when it cannot).
    pub device_save: Option<horcrux_vm::vm::gpucap::DeviceSaveReport>,
    pub last_result: Option<GuestResult>,
}

#[derive(Serialize)]
pub struct Capabilities {
    pub kvm: bool,
    pub criu: ctsave::CriuStatus,
    pub runtimes: BTreeMap<String, ctsave::Support>,
}

pub async fn capabilities() -> Capabilities {
    let mut runtimes = BTreeMap::new();
    for (n, rt) in [
        ("lxc", ContainerRuntime::Lxc),
        ("incus", ContainerRuntime::Incus),
        ("lxd", ContainerRuntime::Lxd),
        ("docker", ContainerRuntime::Docker),
        ("podman", ContainerRuntime::Podman),
    ] {
        runtimes.insert(n.to_string(), ctsave::support(&rt).await);
    }
    Capabilities {
        kvm: std::fs::metadata("/dev/kvm").is_ok(),
        criu: ctsave::criu_status().await,
        runtimes,
    }
}

/// What the host shutdown will do to a guest, given the configured action
/// and whether its state can be saved (pure, unit-tested).
/// What the guest-hibernate step can do for a VM whose state cannot be saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HibernatePlan {
    /// Not a passthrough VM (or hibernate is off): the plain save plan.
    NotApplicable,
    /// The guest can hibernate itself.
    Ready(String),
    /// It cannot (reason).
    Blocked(String),
}

pub fn plan_host_stop(
    action: StopAction,
    fallback: SaveFallback,
    can_save: bool,
    blocker: Option<&str>,
    hib: &HibernatePlan,
) -> (String, Option<String>) {
    if action == StopAction::Save && !can_save {
        match hib {
            HibernatePlan::Ready(how) => {
                return (
                    "hibernate".into(),
                    Some(format!(
                        "{}. The guest will hibernate itself ({how}) and resume at boot.",
                        blocker
                            .unwrap_or("its state cannot be saved")
                            .trim_end_matches('.')
                    )),
                )
            }
            HibernatePlan::Blocked(why) => {
                let (a, r) = plan_host_stop(
                    action,
                    fallback,
                    can_save,
                    blocker,
                    &HibernatePlan::NotApplicable,
                );
                return (
                    a,
                    Some(format!(
                        "{}. Guest hibernate unavailable: {why}",
                        r.unwrap_or_default().trim_end_matches('.')
                    )),
                );
            }
            HibernatePlan::NotApplicable => {}
        }
    }
    match action {
        StopAction::None => ("none".into(), None),
        StopAction::Poweroff => ("poweroff".into(), None),
        StopAction::Shutdown => ("shutdown".into(), None),
        StopAction::Save if can_save => ("save".into(), None),
        StopAction::Save => (
            match fallback {
                SaveFallback::Shutdown => "shutdown",
                SaveFallback::Poweroff => "poweroff",
            }
            .into(),
            Some(blocker.unwrap_or("its state cannot be saved").to_string()),
        ),
    }
}

async fn view_of(
    state: &Arc<AppState>,
    g: GuestInfo,
    sup: &mut BTreeMap<String, ctsave::Support>,
    results: &BTreeMap<String, GuestResult>,
    probes: &mut BTreeMap<PathBuf, crypt::Probe>,
) -> GuestView {
    let eff = policy::effective(state, g.kind, &g.id).await;
    let mut device_save = None;
    if g.qemu && g.passthrough {
        device_save = state
            .vm_manager
            .device_save_report(&g.id, eff.device_save)
            .await
            .ok();
    }
    let (mut can_save, mut save_blocker) = match (&g.ct_runtime, g.qemu) {
        (_, true) if g.passthrough => match &device_save {
            Some(r) if !r.may_save() => (false, Some(device_blocker(r.reason.as_deref()))),
            Some(_) => (true, None),
            None => (
                false,
                Some(device_blocker(Some("the devices could not be read"))),
            ),
        },
        (_, true) => (true, None),
        (Some(rt), _) => {
            let s = match sup.get(&g.runtime) {
                Some(s) => s.clone(),
                None => {
                    let s = ctsave::support(rt).await;
                    sup.insert(g.runtime.clone(), s.clone());
                    s
                }
            };
            let (mut ok, mut reason) = (s.supported, s.reason);
            if ok {
                if let Some(why) = ctsave::instance_blocker(rt, &g.name).await {
                    ok = false;
                    reason = Some(why);
                }
            }
            (ok, reason)
        }
        (None, false) => (
            g.instance_cli.is_some(),
            if g.instance_cli.is_some() {
                None
            } else {
                Some(format!("{} VMs cannot be saved", g.runtime))
            },
        ),
    };
    if can_save && eff.save_encryption == SaveEncryption::Require {
        let target = save_target_dir(&g, &eff.save_dir);
        let p = match probes.get(&target) {
            Some(p) => p.clone(),
            None => {
                let p = crypt::probe(&target).await;
                probes.insert(target.clone(), p.clone());
                p
            }
        };
        if let Err(why) = crypt::verdict(&p, eff.save_encryption) {
            can_save = false;
            save_blocker = Some(why);
        }
    }
    let mut can_hibernate = None;
    let mut hibernate_detail = None;
    let mut hib = HibernatePlan::NotApplicable;
    // only a VM whose host devices cannot save their state hibernates itself
    let devices_save = device_save.as_ref().is_some_and(|r| r.may_save());
    if g.passthrough && g.qemu && eff.stop_action == StopAction::Save && !devices_save {
        if !eff.guest_hibernate {
            hibernate_detail = Some("guest hibernate is off for this VM".to_string());
        } else if g.status == "running" {
            let p = hibernate::probe(state, &g, &eff, false).await;
            can_hibernate = Some(p.ready);
            hib = if p.ready {
                let how = p.method.clone().unwrap_or_else(|| "guest tools".into());
                let note = if p.verified {
                    how
                } else {
                    format!("{how}; the guest's setup cannot be checked without command execution")
                };
                hibernate_detail = Some(note.clone());
                HibernatePlan::Ready(note)
            } else {
                let why = p.reason.clone().unwrap_or_default();
                hibernate_detail = Some(why.clone());
                HibernatePlan::Blocked(why)
            };
        } else {
            hibernate_detail = Some("checked while the VM runs".to_string());
        }
    }
    let (host_stop_action, host_stop_reason) = plan_host_stop(
        eff.stop_action,
        eff.save_fallback,
        can_save,
        save_blocker.as_deref(),
        &hib,
    );
    GuestView {
        host_stop_action,
        host_stop_reason,
        can_hibernate,
        hibernate_detail,
        policy: eff,
        overrides: policy::load_override(state, g.kind, &g.id).await,
        saved: load_saved(state, g.kind, &g.id),
        last_result: results
            .get(&format!("{}:{}", g.kind.prefix(), g.id))
            .cloned(),
        kind: g.kind,
        id: g.id,
        name: g.name,
        runtime: g.runtime,
        status: g.status,
        can_save,
        save_blocker,
        device_save,
    }
}

pub async fn guest_views(state: &Arc<AppState>) -> Vec<GuestView> {
    let results = load_results(state).await;
    let mut sup = BTreeMap::new();
    let mut probes = BTreeMap::new();
    let mut out = Vec::new();
    for g in list_guests(state).await {
        out.push(view_of(state, g, &mut sup, &results, &mut probes).await);
    }
    out
}

pub async fn guest_view(state: &Arc<AppState>, k: GuestKind, id: &str) -> Result<GuestView> {
    let g = find_guest(state, k, id).await?;
    let results = load_results(state).await;
    Ok(view_of(
        state,
        g,
        &mut BTreeMap::new(),
        &results,
        &mut BTreeMap::new(),
    )
    .await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_decisions() {
        use BootAction::*;
        use StartAction::*;
        assert_eq!(boot_decision(ResumeIfRunning, true, true), Resume);
        assert_eq!(boot_decision(ResumeIfRunning, false, true), Start);
        assert_eq!(boot_decision(ResumeIfRunning, false, false), Skip);
        assert_eq!(boot_decision(Always, false, false), Start);
        assert_eq!(boot_decision(Always, true, false), Resume);
        assert_eq!(boot_decision(Nothing, true, true), Skip);
    }

    #[test]
    fn ha_guests_are_left_to_the_ha_loop_at_boot() {
        use BootAction::*;
        assert_eq!(ha_boot_override(Start, true), LeaveToHa);
        assert_eq!(ha_boot_override(Resume, true), LeaveToHa);
        assert_eq!(ha_boot_override(Skip, true), Skip);
        assert_eq!(ha_boot_override(Start, false), Start);
        assert_eq!(ha_boot_override(Resume, false), Resume);
    }

    #[test]
    fn host_stop_plan_applies_the_fallback_and_names_the_reason() {
        use SaveFallback::*;
        let (a, r) = plan_host_stop(
            StopAction::Save,
            Shutdown,
            true,
            None,
            &HibernatePlan::NotApplicable,
        );
        assert_eq!((a.as_str(), r), ("save", None));
        let (a, r) = plan_host_stop(
            StopAction::Save,
            Shutdown,
            false,
            Some("passthrough"),
            &HibernatePlan::NotApplicable,
        );
        assert_eq!(a, "shutdown");
        assert_eq!(r.as_deref(), Some("passthrough"));
        let (a, _) = plan_host_stop(
            StopAction::Save,
            Poweroff,
            false,
            None,
            &HibernatePlan::NotApplicable,
        );
        assert_eq!(a, "poweroff");
        assert_eq!(
            plan_host_stop(
                StopAction::None,
                Shutdown,
                false,
                None,
                &HibernatePlan::NotApplicable
            )
            .0,
            "none"
        );
        assert_eq!(
            plan_host_stop(
                StopAction::Shutdown,
                Shutdown,
                false,
                Some("x"),
                &HibernatePlan::NotApplicable
            )
            .1,
            None
        );
    }

    #[test]
    fn a_passthrough_vm_hibernates_itself_before_any_fallback() {
        use SaveFallback::*;
        let why = "owns passthrough hardware.";
        let ready = HibernatePlan::Ready("systemctl hibernate".into());
        let (a, r) = plan_host_stop(StopAction::Save, Shutdown, false, Some(why), &ready);
        assert_eq!(a, "hibernate");
        let r = r.unwrap();
        assert!(
            r.contains("passthrough") && r.contains("systemctl hibernate"),
            "{r}"
        );
        // blocked: the fallback applies and both reasons are shown
        let blocked = HibernatePlan::Blocked("swap is 0 MB".into());
        let (a, r) = plan_host_stop(StopAction::Save, Poweroff, false, Some(why), &blocked);
        assert_eq!(a, "poweroff");
        let r = r.unwrap();
        assert!(
            r.contains("passthrough") && r.contains("swap is 0 MB"),
            "{r}"
        );
        // a VM whose state CAN be saved is saved; other actions ignore hibernate
        assert_eq!(
            plan_host_stop(StopAction::Save, Shutdown, true, None, &ready).0,
            "save"
        );
        assert_eq!(
            plan_host_stop(StopAction::Shutdown, Shutdown, false, Some(why), &ready).0,
            "shutdown"
        );
        assert_eq!(
            plan_host_stop(StopAction::Poweroff, Shutdown, false, None, &ready).0,
            "poweroff"
        );
    }

    #[test]
    fn save_target_follows_the_runtime() {
        let mut g = GuestInfo {
            kind: GuestKind::Vm,
            id: "1".into(),
            name: "n".into(),
            runtime: "qemu".into(),
            status: "running".into(),
            memory_mb: 1,
            qemu: true,
            ct_runtime: None,
            instance_cli: None,
            passthrough: false,
        };
        let base = Path::new("/enc/saved");
        assert_eq!(save_target_dir(&g, base), PathBuf::from("/enc/saved"));
        g.qemu = false;
        g.instance_cli = Some("incus");
        assert_eq!(save_target_dir(&g, base), PathBuf::from("/var/lib/incus"));
        g.kind = GuestKind::Container;
        g.ct_runtime = Some(ContainerRuntime::Lxc);
        assert_eq!(save_target_dir(&g, base), PathBuf::from("/enc/saved"));
    }

    #[test]
    fn ids_become_safe_file_names() {
        assert_eq!(safe("a/b c"), "a_b_c");
        assert_eq!(safe("vm-1.x_y"), "vm-1.x_y");
    }

    #[test]
    fn manifest_round_trips() {
        let m = Manifest {
            boot_id: "b".into(),
            written_at: "t".into(),
            clean: true,
            guests: vec![ManifestGuest {
                kind: GuestKind::Vm,
                id: "1".into(),
                name: "n".into(),
            }],
            outcomes: vec![],
        };
        let back: Manifest = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back.guests, m.guests);
        assert!(back.clean);
    }
}
