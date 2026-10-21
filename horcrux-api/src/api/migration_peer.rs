//! Migration peer API: the TARGET node's half of moving a QEMU VM here
//! from another node of the cluster (the source half is
//! `api/vm_migration.rs`). Only the source node calls these, with a
//! short-lived `cluster-migration` service token (nodes of one cluster
//! share JWT_SECRET); user logins are refused even with VM.Migrate.
//!
//! 1. `POST /api/migration/peer/probe`    which of the source's disk paths
//!    this node sees too (shared storage: a probe file the source wrote
//!    next to each one is visible here with the same token)
//! 2. `POST /api/migration/peer/prepare`  check the VM can run here (id
//!    free, bridges, ISO, firmware), allocate its non-shared disks
//! 3. `PUT  /api/migration/peer/:job/file/:kind?offset=&total=` copy a
//!    file (disk, EFI vars, cloud-init seed, TPM state) in chunks;
//!    `GET .../file/:kind/sha256` verifies it
//! 4. live only: `POST /api/migration/peer/:job/start` starts QEMU with
//!    `-incoming defer`, exports the empty disks over NBD (the source
//!    mirrors into them) and listens for the migration stream
//! 5. `POST /api/migration/peer/:job/commit` records the VM here (starts
//!    it after an offline move of a running VM; after a live move checks
//!    the guest runs here)
//! 6. `POST /api/migration/peer/:job/abort` undoes 2-4 at any point.
//!
//! While a migration runs the source calls
//! `POST /api/migration/peer/:job/keepalive` every few seconds. A received
//! VM the source stops talking about (target partitioned away, source
//! crashed or gave up and could not reach us to abort) is discarded by
//! [`spawn_reaper`] after [`peer_ttl`]: its receiving QEMU is stopped and
//! the half-received copy deleted, so the VM can be moved here again. Each
//! prepared receive is also recorded under `<data dir>/migration-incoming/`
//! so an API restart picks the leftovers up again (the receiving QEMU
//! outlives the API).
//!
//! Migration traffic (NBD + QEMU migration stream) is plain TCP on the
//! cluster network, bound to the address the source reached this node on,
//! open only for the duration of one migration; NBD exports are named by a
//! random token.
use crate::error::ApiError;
use crate::middleware::auth::{AuthUser, CLUSTER_MIGRATION_ROLE};
use crate::vm::display::{qmp_command, DisplayRuntime};
use crate::AppState;
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::HeaderMap,
    routing::{get, post, put},
    Extension, Json, Router,
};
use horcrux_common::VmConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, OnceLock};
use tokio::sync::Mutex;

/// Largest chunk the source sends per file request.
pub const CHUNK: usize = 16 * 1024 * 1024;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/migration/peer/probe", post(probe))
        .route("/api/migration/peer/prepare", post(prepare))
        .route(
            "/api/migration/peer/:job/file/:kind",
            put(put_file).layer(DefaultBodyLimit::max(CHUNK + 1024)),
        )
        .route(
            "/api/migration/peer/:job/file/:kind/sha256",
            get(file_sha256),
        )
        .route("/api/migration/peer/:job/start", post(start_incoming))
        .route("/api/migration/peer/:job/commit", post(commit))
        .route("/api/migration/peer/:job/abort", post(abort))
        .route("/api/migration/peer/:job/keepalive", post(keepalive))
}

fn require_peer(user: &Option<Extension<AuthUser>>) -> Result<(), ApiError> {
    match user {
        Some(Extension(u)) if u.role == CLUSTER_MIGRATION_ROLE => Ok(()),
        _ => Err(ApiError::Forbidden(
            "the migration peer API is only for other cluster nodes".into(),
        )),
    }
}

/// One VM being received (also persisted, see [`persist`]).
#[derive(Clone, Serialize, Deserialize)]
struct Incoming {
    vm: VmConfig,
    live: bool,
    /// kind -> path on this node of every file the source sends or mirrors.
    files: HashMap<String, PathBuf>,
    /// Files/dirs this node created (removed on abort).
    created: Vec<PathBuf>,
    /// Kinds seen on shared storage (neither copied nor mirrored).
    shared: Vec<String>,
    qemu_started: bool,
    firewall: Vec<crate::firewall::FirewallRule>,
    /// The source sends keepalives (sources from before keepalives get
    /// [`LEGACY_TTL`]).
    #[serde(default)]
    keepalive: bool,
}

fn incoming() -> &'static Mutex<HashMap<String, Incoming>> {
    static M: OnceLock<Mutex<HashMap<String, Incoming>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

fn valid_job(job: &str) -> Result<(), ApiError> {
    if job.is_empty()
        || job.len() > 128
        || !job
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(ApiError::BadRequest(format!("invalid job id {job:?}")));
    }
    Ok(())
}

/// How long a received VM may go without word from its source before it
/// is discarded (`HORCRUX_MIGRATION_PEER_TTL_SECS`, default 90 s, at
/// least 5). The source sends a keepalive every [`KEEPALIVE_EVERY`] and
/// gives up on the target after the same time without an answer.
pub fn peer_ttl() -> std::time::Duration {
    let secs = std::env::var("HORCRUX_MIGRATION_PEER_TTL_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|s| *s >= 5)
        .unwrap_or(90);
    std::time::Duration::from_secs(secs)
}

/// Keepalive interval of the source.
pub const KEEPALIVE_EVERY: std::time::Duration = std::time::Duration::from_secs(5);

/// Expiry for a receive whose source doesn't send keepalives (older
/// version): only its own requests show it is alive.
const LEGACY_TTL: std::time::Duration = std::time::Duration::from_secs(3600);

type ActivityMap = HashMap<String, (std::time::Instant, u32)>;

/// Last time each job heard from its source, and how many of the
/// source's requests for it are in flight (a long sha256 or commit
/// counts as contact for as long as it runs).
fn activity() -> &'static std::sync::Mutex<ActivityMap> {
    static A: OnceLock<std::sync::Mutex<ActivityMap>> = OnceLock::new();
    A.get_or_init(Default::default)
}

fn touch(job: &str) {
    if let Ok(mut a) = activity().lock() {
        a.entry(job.to_string())
            .or_insert((std::time::Instant::now(), 0))
            .0 = std::time::Instant::now();
    }
}

fn forget_activity(job: &str) {
    if let Ok(mut a) = activity().lock() {
        a.remove(job);
    }
}

/// Marks a request of the source for a job in flight (until dropped).
struct Busy(String);

impl Busy {
    fn new(job: &str) -> Self {
        if let Ok(mut a) = activity().lock() {
            let e = a
                .entry(job.to_string())
                .or_insert((std::time::Instant::now(), 0));
            e.0 = std::time::Instant::now();
            e.1 += 1;
        }
        Busy(job.to_string())
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        if let Ok(mut a) = activity().lock() {
            if let Some(e) = a.get_mut(&self.0) {
                e.0 = std::time::Instant::now();
                e.1 = e.1.saturating_sub(1);
            }
        }
    }
}

/// Jobs among `jobs` (job, keepalive) that heard nothing from their
/// source for longer than their TTL and have no request in flight.
fn expired(jobs: &[(String, bool)], ttl: std::time::Duration) -> Vec<String> {
    let Ok(a) = activity().lock() else {
        return Vec::new();
    };
    jobs.iter()
        .filter(|(job, keepalive)| {
            let limit = if *keepalive { ttl } else { LEGACY_TTL };
            match a.get(job) {
                Some((last, busy)) => *busy == 0 && last.elapsed() > limit,
                None => true,
            }
        })
        .map(|(job, _)| job.clone())
        .collect()
}

/// `<data dir>/migration-incoming`: one JSON record per prepared receive.
fn records_dir(state: &AppState) -> PathBuf {
    state.config.paths.data_dir.join("migration-incoming")
}

async fn persist(state: &AppState, job: &str, inc: &Incoming) {
    let dir = records_dir(state);
    let write = async {
        tokio::fs::create_dir_all(&dir).await?;
        let tmp = dir.join(format!(".{job}.json.tmp"));
        let body = serde_json::to_vec(inc).map_err(std::io::Error::other)?;
        tokio::fs::write(&tmp, body).await?;
        tokio::fs::rename(&tmp, dir.join(format!("{job}.json"))).await
    };
    if let Err(e) = write.await {
        tracing::warn!("Migration {job}: receive not recorded on disk: {e}");
    }
}

async fn unpersist(state: &AppState, job: &str) {
    let _ = tokio::fs::remove_file(records_dir(state).join(format!("{job}.json"))).await;
}

/// Discard everything a receive left on this node: the receiving QEMU,
/// the files it created, the record.
async fn discard(state: &AppState, job: &str, inc: &Incoming) {
    forget_activity(job);
    // The id is free here (prepare checked), so a QEMU with it is the
    // receiving one - also if the API died before it noted the start.
    if inc.qemu_started || crate::vm::QemuManager::process_running(&inc.vm.id) {
        state.vm_manager.stop_incoming(&inc.vm).await;
    }
    remove_all(&inc.created).await;
    if !inc.shared.iter().any(|k| k == "disk0") {
        // Files the incoming QEMU/swtpm created next to the (removed)
        // disk: serial log, console state, TPM state.
        let disk0 = PathBuf::from(&inc.vm.disks[0].path);
        for ext in ["serial.log", "display.json", "hold.json"] {
            let _ = tokio::fs::remove_file(disk0.with_extension(ext)).await;
        }
        crate::vm::firmware::remove_state(&disk0).await;
    }
    unpersist(state, job).await;
}

/// Load the receives recorded before an API restart: one whose VM is
/// recorded here was committed (only the record is left); the others are
/// tracked again and expire like any other unless their source still
/// talks to us.
async fn recover(state: &AppState) {
    let dir = records_dir(state);
    let Ok(mut rd) = tokio::fs::read_dir(&dir).await else {
        return;
    };
    while let Ok(Some(e)) = rd.next_entry().await {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(job) = name.strip_suffix(".json").filter(|j| !j.starts_with('.')) else {
            continue;
        };
        let job = job.to_string();
        let inc: Incoming = match tokio::fs::read(e.path())
            .await
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
        {
            Some(i) => i,
            None => {
                tracing::warn!(
                    "Migration {job}: unreadable receive record {}",
                    e.path().display()
                );
                continue;
            }
        };
        if state.vm_manager.vm_exists(&inc.vm.id).await {
            unpersist(state, &job).await;
            continue;
        }
        tracing::info!(
            "Migration {job}: VM {} was being received before the restart; \
             discarded unless its source resumes",
            inc.vm.id
        );
        touch(&job);
        incoming().lock().await.insert(job, inc);
    }
}

/// Once per process (at API start): pick up receives recorded before a
/// restart, then discard receives whose source went quiet (see the module
/// docs).
pub fn spawn_reaper(state: Arc<AppState>) {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    tokio::spawn(async move {
        recover(&state).await;
        let ttl = peer_ttl();
        let every = (ttl / 6).clamp(
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(10),
        );
        loop {
            tokio::time::sleep(every).await;
            reap(&state, ttl).await;
        }
    });
}

async fn reap(state: &AppState, ttl: std::time::Duration) {
    let gone: Vec<(String, Incoming)> = {
        let mut map = incoming().lock().await;
        let jobs: Vec<(String, bool)> = map.iter().map(|(j, i)| (j.clone(), i.keepalive)).collect();
        expired(&jobs, ttl)
            .into_iter()
            .filter_map(|j| map.remove(&j).map(|i| (j, i)))
            .collect()
    };
    for (job, inc) in gone {
        tracing::warn!(
            "Migration {job}: no word from the source for {} s: discarding the half-received VM {}",
            ttl.as_secs(),
            inc.vm.id
        );
        discard(state, &job, &inc).await;
    }
}

/// The source is still at it (`404`: this node no longer has the job, so
/// the source must give up).
async fn keepalive(
    user: Option<Extension<AuthUser>>,
    Path(job): Path<String>,
) -> Result<axum::http::StatusCode, ApiError> {
    require_peer(&user)?;
    if !incoming().lock().await.contains_key(&job) {
        return Err(ApiError::NotFound(format!(
            "migration {job} is not prepared here (any more)"
        )));
    }
    touch(&job);
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProbeFile {
    pub path: String,
    pub token: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProbeRequest {
    pub files: Vec<ProbeFile>,
}

/// `<path>.hxprobe`: the file the source writes next to each disk.
pub fn probe_path(path: &str) -> PathBuf {
    PathBuf::from(format!("{path}.hxprobe"))
}

/// Which of the given paths are shared with the source: for each, whether
/// its probe file exists here with the source's token.
async fn probe(
    State(_state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Json(req): Json<ProbeRequest>,
) -> Result<Json<Vec<bool>>, ApiError> {
    require_peer(&user)?;
    let mut out = Vec::new();
    for f in req.files {
        let seen = tokio::fs::read_to_string(probe_path(&f.path))
            .await
            .map(|t| t.trim() == f.token && f.token.len() >= 16)
            .unwrap_or(false);
        out.push(seen);
    }
    Ok(Json(out))
}

/// One file of the VM the source will send (or mirror, for live disks).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilePlan {
    /// "disk<N>", "efivars", "cidata" or "tpm-<file name>".
    pub kind: String,
    /// Path on the source.
    pub source_path: String,
    /// Seen at the same path on this node (shared storage): not copied.
    pub shared: bool,
    /// Virtual size in bytes (live: the empty image the mirror fills).
    pub virtual_size: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PrepareRequest {
    pub job_id: String,
    pub vm: VmConfig,
    pub live: bool,
    pub files: Vec<FilePlan>,
    #[serde(default)]
    pub firewall: Vec<crate::firewall::FirewallRule>,
    /// The source sends keepalives while the migration runs.
    #[serde(default)]
    pub keepalive: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PrepareResponse {
    /// The VM's disks' paths on this node, in order.
    pub disk_paths: Vec<String>,
    /// This node expects keepalives and discards a receive whose source
    /// went quiet (older nodes: false).
    #[serde(default)]
    pub keepalive: bool,
}

fn kind_ok(kind: &str) -> bool {
    let ok_name = |s: &str| {
        !s.is_empty()
            && s.len() <= 64
            && !s.starts_with('.')
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    kind == "efivars"
        || kind == "cidata"
        || kind
            .strip_prefix("disk")
            .map(|n| n.parse::<usize>().is_ok())
            .unwrap_or(false)
        || kind.strip_prefix("tpm-").map(ok_name).unwrap_or(false)
}

/// Where a non-shared file of the VM goes on this node: disks into the VM
/// storage directory under their own file name, the rest next to disk0.
fn target_path(storage: &FsPath, disk0: &FsPath, plan: &FilePlan) -> Result<PathBuf, ApiError> {
    if plan.shared {
        return Ok(PathBuf::from(&plan.source_path));
    }
    if plan.kind.starts_with("disk") {
        let name = FsPath::new(&plan.source_path)
            .file_name()
            .ok_or_else(|| ApiError::BadRequest(format!("{}: no file name", plan.source_path)))?;
        return Ok(storage.join(name));
    }
    Ok(match plan.kind.as_str() {
        "efivars" => crate::vm::firmware::efivars_path(disk0),
        "cidata" => crate::vm::qemu::cloudinit_seed(disk0),
        tpm => {
            crate::vm::firmware::tpm_state_dir(disk0).join(tpm.strip_prefix("tpm-").unwrap_or(tpm))
        }
    })
}

async fn prepare(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Json(req): Json<PrepareRequest>,
) -> Result<Json<PrepareResponse>, ApiError> {
    require_peer(&user)?;
    valid_job(&req.job_id)?;
    let mut vm = req.vm;
    // The ID names the disk files, sockets and pidfile created here.
    crate::validation::validate_vm_id(&vm.id)?;
    if vm.hypervisor != horcrux_common::VmHypervisor::Qemu {
        return Err(ApiError::BadRequest("only QEMU VMs can be migrated".into()));
    }
    if state.vm_manager.vm_exists(&vm.id).await {
        return Err(ApiError::Conflict(format!(
            "a VM with id {} already exists on this node",
            vm.id
        )));
    }
    if incoming().lock().await.values().any(|i| i.vm.id == vm.id) {
        return Err(ApiError::Conflict(format!(
            "VM {} is already being received",
            vm.id
        )));
    }
    for f in &req.files {
        if !kind_ok(&f.kind) {
            return Err(ApiError::BadRequest(format!(
                "invalid file kind {:?}",
                f.kind
            )));
        }
    }
    // Hardware must work here too: bridges, firmware, and an ISO in the
    // CD-ROM must be in this node's ISO library at the same path.
    if let Some(iso) = &vm.hardware.cdrom {
        if !FsPath::new(iso).is_file() {
            return Err(ApiError::BadRequest(format!(
                "the ISO in the VM's CD-ROM ({iso}) is not on this node: eject it, or put the \
                 same ISO in this node's ISO library, then migrate"
            )));
        }
    }
    super::vm_hw::check_host_hardware(&state, &vm.hardware)
        .await
        .map_err(|e| match e {
            ApiError::BadRequest(m) | ApiError::Forbidden(m) => {
                ApiError::BadRequest(format!("this node can't run the VM: {m}"))
            }
            other => other,
        })?;

    let storage = state.vm_manager.storage_dir();
    tokio::fs::create_dir_all(&storage).await?;
    let disk0_plan = req
        .files
        .iter()
        .find(|f| f.kind == "disk0")
        .ok_or_else(|| ApiError::BadRequest("no disk0 in the plan".into()))?;
    let disk0 = target_path(&storage, &storage, disk0_plan)?;

    let mut files = HashMap::new();
    let mut created: Vec<PathBuf> = Vec::new();
    let shared: Vec<String> = req
        .files
        .iter()
        .filter(|f| f.shared)
        .map(|f| f.kind.clone())
        .collect();
    let result: Result<(), ApiError> = async {
        for f in &req.files {
            let path = target_path(&storage, &disk0, f)?;
            if !f.shared {
                if path.exists() {
                    return Err(ApiError::Conflict(format!(
                        "{} already exists on this node",
                        path.display()
                    )));
                }
                if f.kind.starts_with("tpm-") {
                    let dir = crate::vm::firmware::tpm_state_dir(&disk0);
                    if !dir.exists() {
                        tokio::fs::create_dir_all(&dir).await?;
                        created.push(dir);
                    }
                }
                // Live: the empty images the source's mirror jobs write
                // into through NBD (same virtual size). Offline: the copy
                // creates them.
                if req.live && (f.kind.starts_with("disk") || f.kind == "efivars") {
                    crate::vm::QemuManager::create_disk_image_bytes(&path, f.virtual_size).await?;
                    created.push(path.clone());
                }
            } else if !path.exists() {
                return Err(ApiError::BadRequest(format!(
                    "{} was reported shared but is not visible here",
                    path.display()
                )));
            }
            files.insert(f.kind.clone(), path);
        }
        Ok(())
    }
    .await;
    if let Err(e) = result {
        remove_all(&created).await;
        return Err(e);
    }
    for (i, d) in vm.disks.iter_mut().enumerate() {
        if let Some(p) = files.get(&format!("disk{i}")) {
            d.path = p.to_string_lossy().to_string();
        }
    }
    // A VM always records its disks (the source sent them all).
    let disk_paths = vm.disks.iter().map(|d| d.path.clone()).collect();
    vm.status = horcrux_common::VmStatus::Stopped;
    tracing::info!(
        "Migration {}: receiving VM {} ({} mode, {} file(s))",
        req.job_id,
        vm.id,
        if req.live { "live" } else { "offline" },
        files.len()
    );
    let inc = Incoming {
        vm,
        live: req.live,
        files,
        created,
        shared,
        qemu_started: false,
        firewall: req.firewall,
        keepalive: req.keepalive,
    };
    persist(&state, &req.job_id, &inc).await;
    touch(&req.job_id);
    incoming().lock().await.insert(req.job_id, inc);
    spawn_reaper(state.clone());
    Ok(Json(PrepareResponse {
        disk_paths,
        keepalive: true,
    }))
}

async fn remove_all(paths: &[PathBuf]) {
    for p in paths.iter().rev() {
        if p.is_dir() {
            let _ = tokio::fs::remove_dir_all(p).await;
        } else {
            let _ = tokio::fs::remove_file(p).await;
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct FileQuery {
    pub offset: u64,
    pub total: u64,
}

/// Write one chunk of a file at `offset` (the file is created and sized
/// to `total` by the chunk at offset 0; all-zero chunks are skipped by the
/// source, so sparse images stay sparse).
async fn put_file(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Path((job, kind)): Path<(String, String)>,
    Query(q): Query<FileQuery>,
    body: Bytes,
) -> Result<axum::http::StatusCode, ApiError> {
    require_peer(&user)?;
    let _busy = Busy::new(&job);
    use tokio::io::{AsyncSeekExt, AsyncWriteExt};
    let mut record = None;
    let path = {
        let mut map = incoming().lock().await;
        let inc = map
            .get_mut(&job)
            .ok_or_else(|| ApiError::NotFound(format!("migration {job} is not prepared here")))?;
        let path = inc.files.get(&kind).cloned().ok_or_else(|| {
            ApiError::BadRequest(format!("{kind} is not part of migration {job}"))
        })?;
        if inc.live && (kind.starts_with("disk") || kind == "efivars") {
            return Err(ApiError::BadRequest(format!(
                "{kind} is mirrored live, not copied"
            )));
        }
        if q.offset == 0 && !inc.created.contains(&path) {
            inc.created.push(path.clone());
            record = Some(inc.clone());
        }
        path
    };
    if let Some(inc) = record {
        persist(&state, &job, &inc).await;
    }
    if q.offset + body.len() as u64 > q.total {
        return Err(ApiError::BadRequest(
            "chunk past the end of the file".into(),
        ));
    }
    let mut f = if q.offset == 0 {
        let f = tokio::fs::File::create(&path).await?;
        f.set_len(q.total).await?;
        f
    } else {
        tokio::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .await?
    };
    f.seek(std::io::SeekFrom::Start(q.offset)).await?;
    f.write_all(&body).await?;
    f.flush().await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FileDigest {
    pub sha256: String,
    pub size: u64,
}

/// SHA-256 of a received file, so the source can check the copy.
async fn file_sha256(
    user: Option<Extension<AuthUser>>,
    Path((job, kind)): Path<(String, String)>,
) -> Result<Json<FileDigest>, ApiError> {
    require_peer(&user)?;
    let _busy = Busy::new(&job);
    let path = incoming()
        .lock()
        .await
        .get(&job)
        .and_then(|i| i.files.get(&kind).cloned())
        .ok_or_else(|| ApiError::NotFound(format!("{kind} of migration {job}")))?;
    let (sha256, size) = sha256_file(&path).await?;
    Ok(Json(FileDigest { sha256, size }))
}

/// SHA-256 (hex) and length of a file.
pub async fn sha256_file(path: &FsPath) -> std::io::Result<(String, u64)> {
    use sha2::Digest;
    use tokio::io::AsyncReadExt;
    let mut f = tokio::fs::File::open(path).await?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    let mut n_total = 0u64;
    loop {
        let n = f.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        n_total += n as u64;
    }
    Ok((hex(&h.finalize()), n_total))
}

pub fn hex(b: &[u8]) -> String {
    hex::encode(b)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StartResponse {
    /// `host:port` of the NBD server holding the mirror targets.
    pub nbd: String,
    /// drive id -> NBD export name, for each non-shared disk / EFI vars.
    pub exports: HashMap<String, String>,
    /// QEMU migration URI to send the VM state to.
    pub migrate_uri: String,
}

/// The address the source reached this node on (`Host` header), which
/// the NBD server and the migration listener bind to.
fn bind_host(headers: &HeaderMap) -> String {
    let host = headers
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("127.0.0.1");
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or("::1").to_string();
    }
    host.rsplit_once(':')
        .map(|(h, _)| h)
        .unwrap_or(host)
        .to_string()
}

fn free_port(host: &str) -> Result<u16, ApiError> {
    let l = std::net::TcpListener::bind((host, 0))
        .map_err(|e| ApiError::Internal(format!("no free port on {host}: {e}")))?;
    Ok(l.local_addr()
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .port())
}

fn random_token() -> String {
    use rand::Rng;
    let b: [u8; 16] = rand::thread_rng().gen();
    hex(&b)
}

/// Live: start the receiving QEMU (paused, `-incoming defer`), export its
/// empty disks over NBD and listen for the migration stream.
async fn start_incoming(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    headers: HeaderMap,
    Path(job): Path<String>,
) -> Result<Json<StartResponse>, ApiError> {
    require_peer(&user)?;
    let _busy = Busy::new(&job);
    let (vm, files, shared) = {
        let map = incoming().lock().await;
        let inc = map
            .get(&job)
            .ok_or_else(|| ApiError::NotFound(format!("migration {job} is not prepared here")))?;
        if !inc.live {
            return Err(ApiError::BadRequest(format!("migration {job} is offline")));
        }
        (inc.vm.clone(), inc.files.clone(), inc.shared.clone())
    };
    let host = bind_host(&headers);
    state.vm_manager.start_incoming(&vm).await?;
    let record = incoming().lock().await.get_mut(&job).map(|i| {
        i.qemu_started = true;
        i.clone()
    });
    if let Some(inc) = record {
        persist(&state, &job, &inc).await;
    }
    let disk0 = PathBuf::from(&vm.disks[0].path);
    let sock = DisplayRuntime::qmp_socket(&disk0);
    let nbd_port = free_port(&host)?;
    qmp_command(
        &sock,
        serde_json::json!({"execute": "nbd-server-start", "arguments": {
            "addr": {"type": "inet", "data": {"host": host, "port": nbd_port.to_string()}}
        }}),
    )
    .await?;
    // Block node of each drive the source mirrors into (the non-shared
    // disks and EFI vars), exported writable under a random name.
    let blocks = qmp_command(&sock, serde_json::json!({"execute": "query-block"})).await?;
    let node_of = |drive: &str| -> Option<String> {
        blocks.as_array()?.iter().find_map(|b| {
            (b["device"].as_str() == Some(drive))
                .then(|| b["inserted"]["node-name"].as_str().map(str::to_string))
                .flatten()
        })
    };
    let mut exports = HashMap::new();
    for kind in files
        .keys()
        .filter(|k| (k.starts_with("disk") || *k == "efivars") && !shared.contains(k))
    {
        let drive = if kind == "efivars" {
            crate::vm::firmware::EFIVARS_DRIVE_ID.to_string()
        } else {
            crate::vm::qemu::disk_drive_id(kind.trim_start_matches("disk").parse().unwrap_or(0))
        };
        let node = node_of(&drive)
            .ok_or_else(|| ApiError::Internal(format!("no block node for {drive}")))?;
        let name = random_token();
        qmp_command(
            &sock,
            serde_json::json!({"execute": "block-export-add", "arguments": {
                "type": "nbd", "id": format!("mig-{drive}"), "node-name": node,
                "name": name, "writable": true
            }}),
        )
        .await?;
        exports.insert(drive, name);
    }
    let mig_port = free_port(&host)?;
    let uri = format!(
        "tcp:{}:{mig_port}",
        if host.contains(':') {
            format!("[{host}]")
        } else {
            host.clone()
        }
    );
    qmp_command(
        &sock,
        serde_json::json!({"execute": "migrate-incoming", "arguments": {"uri": uri}}),
    )
    .await?;
    let nbd = if host.contains(':') {
        format!("[{host}]:{nbd_port}")
    } else {
        format!("{host}:{nbd_port}")
    };
    tracing::info!(
        "Migration {job}: VM {} waiting for its state on {uri}",
        vm.id
    );
    Ok(Json(StartResponse {
        nbd,
        exports,
        migrate_uri: uri,
    }))
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CommitRequest {
    /// Offline: start the VM here (it was running on the source).
    #[serde(default)]
    pub start: bool,
}

/// Record the received VM on this node.
async fn commit(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Path(job): Path<String>,
    body: Option<Json<CommitRequest>>,
) -> Result<Json<VmConfig>, ApiError> {
    require_peer(&user)?;
    let req = body.map(|Json(b)| b).unwrap_or_default();
    let inc = incoming()
        .lock()
        .await
        .remove(&job)
        .ok_or_else(|| ApiError::NotFound(format!("migration {job} is not prepared here")))?;
    let _busy = Busy::new(&job);
    let restore = |inc: Incoming| {
        let j = job.clone();
        async move {
            touch(&j);
            incoming().lock().await.insert(j, inc);
        }
    };
    let disk0 = PathBuf::from(&inc.vm.disks[0].path);
    if inc.live {
        let sock = DisplayRuntime::qmp_socket(&disk0);
        // The state has arrived once QEMU leaves "inmigrate"; it waits
        // paused (-S) until the source completed its disk mirrors - which
        // it has when it commits - and then resumes here.
        let mut running = false;
        for _ in 0..240 {
            match qmp_command(&sock, serde_json::json!({"execute": "query-status"})).await {
                Ok(s) if s["status"].as_str() == Some("running") => {
                    running = true;
                    break;
                }
                Ok(s)
                    if matches!(
                        s["status"].as_str(),
                        Some("paused" | "prelaunch" | "postmigrate")
                    ) =>
                {
                    let _ = qmp_command(&sock, serde_json::json!({"execute": "cont"})).await;
                }
                _ => {}
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        if !running {
            let msg = "the VM did not start running on the target after the migration".to_string();
            restore(inc).await;
            return Err(ApiError::Conflict(msg));
        }
        // The mirror targets are complete: no more NBD access. (The
        // console ports/password of this run were set when QEMU started.)
        let _ = qmp_command(&sock, serde_json::json!({"execute": "nbd-server-stop"})).await;
    }
    let vm = match state.vm_manager.import_vm(inc.vm.clone()).await {
        Ok(v) => v,
        Err(e) => {
            restore(inc).await;
            return Err(e.into());
        }
    };
    unpersist(&state, &job).await;
    forget_activity(&job);
    for rule in &inc.firewall {
        if let Err(e) = state
            .firewall_manager
            .add_rule(
                crate::firewall::FirewallScope::Vm(vm.id.clone()),
                rule.clone(),
            )
            .await
        {
            tracing::warn!(
                "VM {}: firewall rule {} not carried over: {e}",
                vm.id,
                rule.id
            );
        }
    }
    let vm = if !inc.live && req.start {
        state.vm_manager.start_vm(&vm.id).await?
    } else {
        vm
    };
    tracing::info!("Migration {job}: VM {} now lives on this node", vm.id);
    Ok(Json(vm))
}

/// Undo a prepared/started incoming migration (idempotent).
async fn abort(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Path(job): Path<String>,
) -> Result<axum::http::StatusCode, ApiError> {
    require_peer(&user)?;
    let Some(inc) = incoming().lock().await.remove(&job) else {
        return Ok(axum::http::StatusCode::NO_CONTENT);
    };
    discard(&state, &job, &inc).await;
    tracing::info!("Migration {job}: aborted, VM {} not kept here", inc.vm.id);
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_kinds_are_restricted() {
        for ok in [
            "disk0",
            "disk12",
            "efivars",
            "cidata",
            "tpm-tpm2-00.permall",
        ] {
            assert!(kind_ok(ok), "{ok}");
        }
        for bad in [
            "disk",
            "diskx",
            "tpm-",
            "tpm-../x",
            "tpm-.hidden",
            "../etc",
            "seed",
        ] {
            assert!(!kind_ok(bad), "{bad}");
        }
    }

    #[test]
    fn non_shared_files_land_in_storage_or_next_to_disk0() {
        let storage = FsPath::new("/t/vms");
        let disk0 = FsPath::new("/t/vms/vm-1.qcow2");
        let plan = |kind: &str, path: &str, shared: bool| FilePlan {
            kind: kind.into(),
            source_path: path.into(),
            shared,
            virtual_size: 0,
        };
        assert_eq!(
            target_path(
                storage,
                disk0,
                &plan("disk1", "/s/pool/vm-1-disk1.qcow2", false)
            )
            .unwrap(),
            PathBuf::from("/t/vms/vm-1-disk1.qcow2")
        );
        assert_eq!(
            target_path(storage, disk0, &plan("disk0", "/nfs/vm-1.qcow2", true)).unwrap(),
            PathBuf::from("/nfs/vm-1.qcow2")
        );
        assert_eq!(
            target_path(
                storage,
                disk0,
                &plan("efivars", "/s/vm-1.efivars.qcow2", false)
            )
            .unwrap(),
            PathBuf::from("/t/vms/vm-1.efivars.qcow2")
        );
        assert_eq!(
            target_path(
                storage,
                disk0,
                &plan("tpm-tpm2-00.permall", "/s/vm-1.tpm/tpm2-00.permall", false)
            )
            .unwrap(),
            PathBuf::from("/t/vms/vm-1.tpm/tpm2-00.permall")
        );
    }

    /// A receive whose source went quiet expires after the TTL; one with a
    /// request in flight (a long sha256/commit) or recent contact does
    /// not; a source without keepalives gets the long legacy TTL.
    #[test]
    fn quiet_receives_expire_after_the_ttl() {
        let ttl = std::time::Duration::from_millis(200);
        let jobs = |names: &[&str], ka: bool| -> Vec<(String, bool)> {
            names.iter().map(|n| (n.to_string(), ka)).collect()
        };
        touch("t-quiet");
        touch("t-talking");
        let busy = Busy::new("t-busy");
        touch("t-legacy");
        std::thread::sleep(std::time::Duration::from_millis(300));
        touch("t-talking");
        let mut all = jobs(&["t-quiet", "t-talking", "t-busy"], true);
        all.extend(jobs(&["t-legacy"], false));
        assert_eq!(expired(&all, ttl), vec!["t-quiet".to_string()]);
        drop(busy);
        assert_eq!(
            expired(&all, ttl),
            vec!["t-quiet".to_string()],
            "a request that just finished is contact"
        );
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(
            expired(&all, ttl),
            ["t-quiet", "t-talking", "t-busy"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
        // Untracked (never heard of): expired at once.
        assert_eq!(
            expired(&jobs(&["t-never"], true), ttl),
            vec!["t-never".to_string()]
        );
        for j in ["t-quiet", "t-talking", "t-busy", "t-legacy"] {
            forget_activity(j);
        }
    }

    #[test]
    fn peer_ttl_defaults_and_ignores_silly_values() {
        // One test: the variable is process-wide.
        std::env::remove_var("HORCRUX_MIGRATION_PEER_TTL_SECS");
        assert_eq!(peer_ttl().as_secs(), 90);
        std::env::set_var("HORCRUX_MIGRATION_PEER_TTL_SECS", "20");
        assert_eq!(peer_ttl().as_secs(), 20);
        std::env::set_var("HORCRUX_MIGRATION_PEER_TTL_SECS", "1");
        assert_eq!(peer_ttl().as_secs(), 90);
        std::env::remove_var("HORCRUX_MIGRATION_PEER_TTL_SECS");
    }

    #[test]
    fn bind_host_comes_from_the_host_header() {
        let mut h = HeaderMap::new();
        h.insert("host", "10.0.0.2:8006".parse().unwrap());
        assert_eq!(bind_host(&h), "10.0.0.2");
        h.insert("host", "[fd00::2]:8006".parse().unwrap());
        assert_eq!(bind_host(&h), "fd00::2");
        h.insert("host", "node2".parse().unwrap());
        assert_eq!(bind_host(&h), "node2");
    }
}
