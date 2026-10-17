//! Backup and restore (vzdump equivalent).
//!
//! Real backups of VMs (every qcow2/raw disk, flattened with
//! `qemu-img convert`) and containers (Docker/Podman `export`, the LXC
//! container directory, LXD/Incus `export`) plus the guest config, packed
//! into one tar archive compressed with zstd/gzip/lzo, written to a backup
//! storage (a directory, or a directory-type storage pool), checksummed
//! (SHA-256) and recorded in the database catalog. Restores recreate the
//! guest (same or new id), verify re-reads and checks archives, retention
//! prunes per guest and storage with Proxmox semantics, and scheduled jobs
//! are fired by a real cron loop (`start_scheduler`).
//!
//! Modes: a stopped guest is always copied offline. For a running guest,
//! `stop` stops it, backs it up and starts it again; `suspend` and
//! `snapshot` pause it for the copy (VMs over their QMP socket, which makes
//! QEMU flush every disk; containers are frozen) and resume it afterwards.
//! A copy-before-write block job (true no-pause snapshot) is not
//! implemented yet.
pub mod archive;
pub mod providers;
pub mod retention;
pub mod scheduler;
mod store;

use crate::container::ContainerManager;
use crate::vm::VmManager;
use archive::{ArchiveFile, Manifest, FORMAT_VERSION, MANIFEST_NAME};
use futures::future::BoxFuture;
use horcrux_common::{ContainerConfig, ContainerRuntime, ContainerStatus, Error, Result, VmConfig};
use retention::{PruneDecision, PruneMark};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use store::Store;

/// Backup mode
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BackupMode {
    /// Consistent copy without stopping the guest (offline copy when the
    /// guest is stopped; containers are frozen during the copy).
    #[default]
    Snapshot,
    /// Suspend/freeze the guest during the copy.
    Suspend,
    /// Stop the guest, copy, start it again.
    Stop,
}

/// Backup compression
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Compression {
    None,
    Gzip,
    Lzo,
    #[default]
    Zstd,
}

/// Target type for backup
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetType {
    Vm,
    Container,
}

impl TargetType {
    fn as_str(&self) -> &'static str {
        match self {
            TargetType::Vm => "qemu",
            TargetType::Container => "lxc",
        }
    }
}

/// Request to back up one guest (`POST /api/backups`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BackupConfig {
    /// Ignored (the backup id is generated); accepted for compatibility.
    #[serde(default)]
    pub id: String,
    /// Display name of the guest (defaults to the guest's own name).
    #[serde(default)]
    pub name: String,
    /// "vm" or "container"; detected from the id when omitted.
    #[serde(default)]
    pub target_type: Option<TargetType>,
    pub target_id: String,
    /// Backup storage: an absolute directory, a storage pool id/name, or
    /// empty / "local" for the default backup directory.
    #[serde(default)]
    pub storage: String,
    #[serde(default)]
    pub mode: BackupMode,
    #[serde(default)]
    pub compression: Compression,
    #[serde(default)]
    pub notes: Option<String>,
    /// Protected backups are never pruned or deleted.
    #[serde(default)]
    pub protected: bool,
}

/// Scheduled backup job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupJob {
    /// Generated when empty.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Cron expression (`0 2 * * *`) or alias (`daily`, `weekly`, ...).
    pub schedule: String,
    /// VM and/or container ids.
    pub targets: Vec<String>,
    #[serde(default)]
    pub storage: String,
    #[serde(default)]
    pub mode: BackupMode,
    #[serde(default)]
    pub compression: Compression,
    #[serde(default)]
    pub retention: RetentionPolicy,
    /// When to send a notification to the configured notification
    /// channels: "always", "failure" or "never" (default).
    #[serde(default)]
    pub notify: Option<String>,
    /// Unix time of the last run (set by the server).
    #[serde(default)]
    pub last_run: Option<i64>,
    /// "ok" or "failed" (set by the server).
    #[serde(default)]
    pub last_status: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    /// Unix time of the next scheduled run (set by the server).
    #[serde(default)]
    pub next_run: Option<i64>,
}

fn default_true() -> bool {
    true
}

/// Retention policy (Proxmox `prune-backups` semantics).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct RetentionPolicy {
    #[serde(default)]
    pub keep_last: Option<u32>,
    #[serde(default)]
    pub keep_hourly: Option<u32>,
    #[serde(default)]
    pub keep_daily: Option<u32>,
    #[serde(default)]
    pub keep_weekly: Option<u32>,
    #[serde(default)]
    pub keep_monthly: Option<u32>,
    #[serde(default)]
    pub keep_yearly: Option<u32>,
}

/// A backup in the catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Backup {
    pub id: String,
    pub target_type: TargetType,
    pub target_id: String,
    pub target_name: String,
    /// Unix time the backup was taken.
    pub timestamp: i64,
    /// Archive size in bytes.
    pub size: u64,
    pub mode: BackupMode,
    pub compression: Compression,
    /// Archive file.
    pub path: PathBuf,
    /// Storage label the backup was written to ("local", a pool id, or a
    /// directory).
    #[serde(default)]
    pub storage: String,
    pub config_included: bool,
    pub notes: Option<String>,
    #[serde(default)]
    pub protected: bool,
    /// SHA-256 of the archive file.
    #[serde(default)]
    pub sha256: String,
    /// Job that created the backup, if any.
    #[serde(default)]
    pub job_id: Option<String>,
    /// "ok" / "failed" after a verify.
    #[serde(default)]
    pub verify_state: Option<String>,
    #[serde(default)]
    pub verified_at: Option<i64>,
    #[serde(default)]
    pub verify_error: Option<String>,
}

/// `PUT /api/backups/:id`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BackupUpdate {
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub protected: Option<bool>,
}

/// `POST /api/backups/:id/restore`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RestoreRequest {
    /// Guest id to restore to (default: the original guest).
    #[serde(default)]
    pub target_id: Option<String>,
    /// Name for the restored guest.
    #[serde(default)]
    pub name: Option<String>,
    /// Overwrite an existing guest other than the original.
    #[serde(default)]
    pub force: bool,
    /// Start the guest after restoring it.
    #[serde(default)]
    pub start: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreResult {
    pub backup_id: String,
    pub target_type: TargetType,
    pub target_id: String,
    pub name: String,
    /// True when an existing guest was replaced.
    pub replaced: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyResult {
    pub backup_id: String,
    pub ok: bool,
    pub checked_at: i64,
    /// Payload files checked against the manifest.
    pub files_checked: usize,
    pub error: Option<String>,
}

/// `POST /api/backups/prune`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PruneRequest {
    pub target_id: String,
    /// Only prune backups on this storage (default: every storage, each
    /// one pruned separately).
    #[serde(default)]
    pub storage: Option<String>,
    #[serde(flatten)]
    pub policy: RetentionPolicy,
    /// Only report what would be removed.
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobTargetResult {
    pub target_id: String,
    pub backup_id: Option<String>,
    pub error: Option<String>,
    /// Backups removed by the job's retention policy.
    pub pruned: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRunReport {
    pub job_id: String,
    pub started_at: i64,
    pub finished_at: i64,
    pub ok: bool,
    pub results: Vec<JobTargetResult>,
}

pub type JobHook = Arc<dyn Fn(BackupJob, JobRunReport) -> BoxFuture<'static, ()> + Send + Sync>;

/// The guest managers backups operate on.
pub struct BackupRuntime {
    pub vm_manager: Arc<VmManager>,
    pub container_manager: Arc<ContainerManager>,
    pub storage_manager: Option<Arc<crate::storage::StorageManager>>,
    /// Directory new VM disks are restored into.
    pub vm_storage: PathBuf,
    /// LXC container directory (`lxc.lxcpath`).
    pub lxc_path: PathBuf,
    /// Called after every job run (notifications, events).
    pub on_job_finished: Option<JobHook>,
}

/// Releases a busy-guest claim on drop.
struct BusyGuard<'a> {
    set: &'a Mutex<HashSet<String>>,
    key: String,
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.set
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.key);
    }
}

fn claim<'a>(set: &'a Mutex<HashSet<String>>, key: &str, what: &str) -> Result<BusyGuard<'a>> {
    let mut g = set.lock().unwrap_or_else(|e| e.into_inner());
    if !g.insert(key.to_string()) {
        return Err(Error::Conflict(format!(
            "a backup or restore of {what} is already running"
        )));
    }
    Ok(BusyGuard {
        set,
        key: key.to_string(),
    })
}

/// Removes a staging directory on drop.
struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn valid_guest_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !id.starts_with('.')
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Backup manager
pub struct BackupManager {
    store: Store,
    runtime: OnceLock<BackupRuntime>,
    default_storage: PathBuf,
    work_dir: PathBuf,
    busy: Mutex<HashSet<String>>,
    running_jobs: Mutex<HashSet<String>>,
    /// Serializes read-modify-write of job records (the scheduler, job
    /// runs and API edits all update them).
    job_lock: tokio::sync::Mutex<()>,
}

impl Default for BackupManager {
    fn default() -> Self {
        Self::new()
    }
}

impl BackupManager {
    /// In-memory catalog, default directories.
    pub fn new() -> Self {
        Self::with_restore_dir(PathBuf::from("/var/lib/horcrux/restore"))
    }

    /// In-memory catalog with a custom restore work directory.
    pub fn with_restore_dir(restore_dir: PathBuf) -> Self {
        Self {
            store: Store::memory(),
            runtime: OnceLock::new(),
            default_storage: PathBuf::from("/var/lib/horcrux/backups"),
            work_dir: restore_dir,
            busy: Mutex::new(HashSet::new()),
            running_jobs: Mutex::new(HashSet::new()),
            job_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Catalog and jobs persisted in the database (loaded now, written
    /// through on every change). `backup_dir` is the default ("local")
    /// backup storage, `restore_dir` the work directory for container
    /// restores.
    pub async fn with_database(
        db: Arc<crate::db::Database>,
        backup_dir: PathBuf,
        restore_dir: PathBuf,
    ) -> Result<Self> {
        let store = Store::load(crate::db::config_store::ConfigStore::new(&db)).await?;
        Ok(Self {
            store,
            runtime: OnceLock::new(),
            default_storage: backup_dir,
            work_dir: restore_dir,
            busy: Mutex::new(HashSet::new()),
            running_jobs: Mutex::new(HashSet::new()),
            job_lock: tokio::sync::Mutex::new(()),
        })
    }

    /// Connect the manager to the VM/container managers. Must be called
    /// once before backups, restores or jobs can run.
    pub fn attach_runtime(&self, rt: BackupRuntime) -> Result<()> {
        self.runtime
            .set(rt)
            .map_err(|_| Error::System("backup runtime already attached".to_string()))
    }

    fn rt(&self) -> Result<&BackupRuntime> {
        self.runtime.get().ok_or_else(|| {
            Error::System("backup manager is not connected to the VM/container managers".into())
        })
    }

    pub fn default_storage(&self) -> &Path {
        &self.default_storage
    }

    /// Resolve a storage spec to (label, directory).
    pub async fn resolve_storage(&self, spec: &str) -> Result<(String, PathBuf)> {
        let spec = spec.trim();
        if spec.starts_with('/') {
            if spec.split('/').any(|c| c == "..") {
                return Err(Error::Validation(format!(
                    "backup storage path '{spec}' must not contain '..'"
                )));
            }
            return Ok((spec.to_string(), PathBuf::from(spec)));
        }
        if let Some(sm) = self.runtime.get().and_then(|r| r.storage_manager.as_ref()) {
            if !spec.is_empty() {
                if let Some(pool) = sm
                    .list_pools()
                    .await
                    .into_iter()
                    .find(|p| p.id == spec || p.name == spec)
                {
                    use crate::storage::StorageType::*;
                    let fs_backed = matches!(
                        pool.storage_type,
                        Directory | Nfs | Cifs | BtrFs | GlusterFs | Zfs
                    ) && pool.path.starts_with('/');
                    if !fs_backed {
                        return Err(Error::Validation(format!(
                            "storage pool '{spec}' ({:?}) cannot hold backup archives; use a directory, NFS, CIFS, BtrFS, GlusterFS or mounted ZFS pool",
                            pool.storage_type
                        )));
                    }
                    if !pool.enabled {
                        return Err(Error::Validation(format!(
                            "storage pool '{spec}' is disabled"
                        )));
                    }
                    return Ok((pool.id.clone(), PathBuf::from(&pool.path).join("dump")));
                }
            }
        }
        if spec.is_empty() || spec == "local" {
            return Ok(("local".to_string(), self.default_storage.clone()));
        }
        Err(Error::Validation(format!(
            "unknown backup storage '{spec}': use 'local', a storage pool id, or an absolute directory"
        )))
    }

    async fn detect_target(&self, id: &str) -> Result<TargetType> {
        let rt = self.rt()?;
        if rt.vm_manager.get_vm(id).await.is_ok() {
            return Ok(TargetType::Vm);
        }
        if rt.container_manager.get_container(id).await.is_ok() {
            return Ok(TargetType::Container);
        }
        Err(Error::NotFound(format!(
            "no VM or container with id '{id}'"
        )))
    }

    // ------------------------------------------------------------------
    // Catalog
    // ------------------------------------------------------------------

    /// List backups, newest first, optionally for one guest.
    pub async fn list_backups(&self, target_id: Option<String>) -> Result<Vec<Backup>> {
        let all = self.store.list_backups().await?;
        Ok(match target_id {
            Some(t) => all.into_iter().filter(|b| b.target_id == t).collect(),
            None => all,
        })
    }

    pub async fn get_backup(&self, id: &str) -> Result<Backup> {
        self.store
            .get_backup(id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("Backup {id} not found")))
    }

    pub async fn update_backup(&self, id: &str, update: BackupUpdate) -> Result<Backup> {
        let mut b = self.get_backup(id).await?;
        if let Some(n) = update.notes {
            b.notes = if n.trim().is_empty() { None } else { Some(n) };
        }
        if let Some(p) = update.protected {
            b.protected = p;
        }
        self.store.put_backup(&b).await?;
        Ok(b)
    }

    /// Delete a backup and its archive. Protected backups are refused.
    pub async fn delete_backup(&self, backup_id: &str) -> Result<()> {
        let b = self.get_backup(backup_id).await?;
        if b.protected {
            return Err(Error::Conflict(format!(
                "backup {backup_id} is protected; unprotect it first"
            )));
        }
        self.remove_backup(&b).await
    }

    async fn remove_backup(&self, b: &Backup) -> Result<()> {
        match tokio::fs::remove_file(&b.path).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(Error::System(format!(
                    "cannot delete archive {}: {e}",
                    b.path.display()
                )))
            }
        }
        self.store.delete_backup(&b.id).await?;
        tracing::info!("Backup {} deleted", b.id);
        Ok(())
    }

    // ------------------------------------------------------------------
    // Backup
    // ------------------------------------------------------------------

    /// Back up one guest. Runs to completion (the archive is on disk and
    /// in the catalog when this returns).
    pub async fn create_backup(&self, config: BackupConfig) -> Result<Backup> {
        self.create_backup_inner(config, None).await
    }

    async fn create_backup_inner(
        &self,
        config: BackupConfig,
        job_id: Option<String>,
    ) -> Result<Backup> {
        let rt = self.rt()?;
        let target_id = config.target_id.trim().to_string();
        if !valid_guest_id(&target_id) {
            return Err(Error::Validation(format!(
                "target_id '{target_id}' is missing or invalid"
            )));
        }
        let target_type = match config.target_type.clone() {
            Some(t) => t,
            None => self.detect_target(&target_id).await?,
        };
        let (storage_label, storage_dir) = self.resolve_storage(&config.storage).await?;
        let _busy = claim(&self.busy, &target_id, &target_id)?;

        tokio::fs::create_dir_all(&storage_dir).await.map_err(|e| {
            Error::System(format!(
                "cannot create backup storage directory {}: {e}",
                storage_dir.display()
            ))
        })?;

        let backup_id = uuid::Uuid::new_v4().to_string();
        let started = chrono::Local::now();
        let filename = format!(
            "vzdump-{}-{}-{}.{}",
            target_type.as_str(),
            target_id,
            started.format("%Y_%m_%d-%H_%M_%S"),
            archive::file_extension(&config.compression)
        );
        let mut archive_path = storage_dir.join(&filename);
        if archive_path.exists() {
            // Two backups of one guest within the same second (e.g. a job
            // run right after a manual backup): never overwrite.
            archive_path = storage_dir.join(format!(
                "vzdump-{}-{}-{}-{}.{}",
                target_type.as_str(),
                target_id,
                started.format("%Y_%m_%d-%H_%M_%S"),
                &backup_id[..8],
                archive::file_extension(&config.compression)
            ));
        }
        let staging = TempDir(storage_dir.join(format!(".tmp-{backup_id}")));
        tokio::fs::create_dir_all(&staging.0).await?;

        tracing::info!(
            "Backing up {} {} to {} ({:?}, {:?})",
            target_type.as_str(),
            target_id,
            archive_path.display(),
            config.mode,
            config.compression
        );

        let (target_name, guest_config, runtime, files) = match target_type {
            TargetType::Vm => {
                let vm = rt.vm_manager.get_vm(&target_id).await?;
                let files = self.stage_vm(rt, &vm, &config.mode, &staging.0).await?;
                (
                    vm.name.clone(),
                    serde_json::to_value(&vm).unwrap_or_default(),
                    None,
                    files,
                )
            }
            TargetType::Container => {
                let c = rt.container_manager.get_container(&target_id).await?;
                let files = self
                    .stage_container(rt, &c, &config.mode, &staging.0)
                    .await?;
                (
                    c.name.clone(),
                    serde_json::to_value(&c).unwrap_or_default(),
                    Some(runtime_name(&c.runtime).to_string()),
                    files,
                )
            }
        };

        let manifest = Manifest {
            format_version: FORMAT_VERSION,
            backup_id: backup_id.clone(),
            target_type: target_type.clone(),
            target_id: target_id.clone(),
            target_name: target_name.clone(),
            created_at: started.timestamp(),
            config: guest_config,
            runtime,
            files,
        };
        let manifest_json = serde_json::to_vec_pretty(&manifest)
            .map_err(|e| Error::System(format!("serialize manifest: {e}")))?;
        tokio::fs::write(staging.0.join(MANIFEST_NAME), manifest_json).await?;

        let mut names = vec![MANIFEST_NAME.to_string()];
        names.extend(manifest.files.iter().map(|f| f.name.clone()));
        {
            let dir = staging.0.clone();
            let out = archive_path.clone();
            let comp = config.compression.clone();
            tokio::task::spawn_blocking(move || archive::write_archive(&dir, &names, &out, &comp))
                .await
                .map_err(|e| Error::System(format!("archive task failed: {e}")))??;
        }
        drop(staging);

        let (sha256, size) = archive::sha256_file_async(archive_path.clone()).await?;
        let backup = Backup {
            id: backup_id,
            target_type,
            target_id,
            target_name: if config.name.trim().is_empty() {
                target_name
            } else {
                config.name.clone()
            },
            timestamp: started.timestamp(),
            size,
            mode: config.mode,
            compression: config.compression,
            path: archive_path,
            storage: storage_label,
            config_included: true,
            notes: config.notes.filter(|n| !n.trim().is_empty()),
            protected: config.protected,
            sha256,
            job_id,
            verify_state: None,
            verified_at: None,
            verify_error: None,
        };
        if let Err(e) = self.store.put_backup(&backup).await {
            let _ = tokio::fs::remove_file(&backup.path).await;
            return Err(e);
        }
        tracing::info!(
            "Backup {} of {} complete: {} bytes",
            backup.id,
            backup.target_id,
            backup.size
        );
        Ok(backup)
    }

    /// Copy every disk of a VM into `staging` (stopping it first in stop
    /// mode, starting it again afterwards).
    async fn stage_vm(
        &self,
        rt: &BackupRuntime,
        vm: &VmConfig,
        mode: &BackupMode,
        staging: &Path,
    ) -> Result<Vec<ArchiveFile>> {
        let mut disks: Vec<PathBuf> = vm.disks.iter().map(|d| PathBuf::from(&d.path)).collect();
        if disks.is_empty() {
            disks.push(rt.vm_storage.join(format!("{}.qcow2", vm.id)));
        }

        let running = crate::vm::QemuManager::process_running(&vm.id);
        let mut restart = false;
        let mut paused: Option<crate::migration::qemu_monitor::QemuMonitor> = None;
        if running {
            match mode {
                BackupMode::Stop => {
                    stop_vm_and_wait(rt, &vm.id).await?;
                    restart = true;
                }
                BackupMode::Suspend | BackupMode::Snapshot => {
                    // Pause the guest over its QMP socket: QEMU drains and
                    // flushes every block device on `stop`, so the image
                    // on disk is consistent while it stays paused. (No
                    // copy-before-write block job yet, so snapshot mode
                    // also pauses for the duration of the copy.)
                    let qmp = crate::vm::display::DisplayRuntime::qmp_socket(&disks[0]);
                    let mon = crate::migration::qemu_monitor::QemuMonitor::new(qmp.clone());
                    mon.stop().await.map_err(|e| {
                        Error::Conflict(format!(
                            "VM {} is running but could not be paused over QMP ({}): {e}; use mode \"stop\"",
                            vm.id,
                            qmp.display()
                        ))
                    })?;
                    tracing::info!("VM {} paused for backup", vm.id);
                    paused = Some(mon);
                }
            }
        }
        // A paused QEMU still holds its image lock: read with force-share.
        let force_share = paused.is_some();

        let result = async {
            let mut files = Vec::new();
            for (i, src) in disks.iter().enumerate() {
                let name = format!("disk-{i}.qcow2");
                let dst = staging.join(&name);
                let (s, d) = (src.clone(), dst.clone());
                tokio::task::spawn_blocking(move || archive::export_disk(&s, &d, force_share))
                    .await
                    .map_err(|e| Error::System(format!("disk export task failed: {e}")))??;
                let (sha256, size) = archive::sha256_file_async(dst).await?;
                files.push(ArchiveFile {
                    name,
                    kind: "disk".into(),
                    size,
                    sha256,
                    format: Some("qcow2".into()),
                    source: Some(src.to_string_lossy().into()),
                });
            }
            Ok(files)
        }
        .await;

        if let Some(mon) = paused {
            if let Err(e) = mon.cont().await {
                tracing::error!("VM {} could not be resumed after backup: {e}", vm.id);
                if result.is_ok() {
                    return Err(Error::System(format!(
                        "backup copied but VM {} failed to resume: {e}",
                        vm.id
                    )));
                }
            } else {
                tracing::info!("VM {} resumed after backup", vm.id);
            }
        }
        if restart {
            if let Err(e) = rt.vm_manager.start_vm(&vm.id).await {
                tracing::error!("VM {} could not be restarted after backup: {e}", vm.id);
                if result.is_ok() {
                    return Err(Error::System(format!(
                        "backup copied but VM {} failed to restart: {e}",
                        vm.id
                    )));
                }
            }
        }
        result
    }

    async fn stage_container(
        &self,
        rt: &BackupRuntime,
        c: &ContainerConfig,
        mode: &BackupMode,
        staging: &Path,
    ) -> Result<Vec<ArchiveFile>> {
        let cm = &rt.container_manager;
        let status = cm
            .get_container_status(&c.id)
            .await
            .unwrap_or(c.status.clone());
        let running = status == ContainerStatus::Running;
        enum Undo {
            None,
            Start,
            Resume,
        }
        let undo = if !running {
            Undo::None
        } else if *mode == BackupMode::Stop {
            cm.stop_container(&c.id).await?;
            Undo::Start
        } else {
            cm.pause_container(&c.id).await?;
            Undo::Resume
        };

        let result = self.copy_container(rt, c, staging).await;

        let restored = match undo {
            Undo::None => Ok(()),
            Undo::Start => cm.start_container(&c.id).await.map(|_| ()),
            Undo::Resume => cm.resume_container(&c.id).await.map(|_| ()),
        };
        if let Err(e) = restored {
            tracing::error!(
                "container {} not returned to running after backup: {e}",
                c.id
            );
            if result.is_ok() {
                return Err(Error::System(format!(
                    "backup copied but container {} failed to resume: {e}",
                    c.id
                )));
            }
        }
        result
    }

    async fn copy_container(
        &self,
        rt: &BackupRuntime,
        c: &ContainerConfig,
        staging: &Path,
    ) -> Result<Vec<ArchiveFile>> {
        let mut files = Vec::new();
        match c.runtime {
            ContainerRuntime::Docker | ContainerRuntime::Podman => {
                let prog = runtime_name(&c.runtime);
                let rootfs = staging.join("rootfs.tar");
                archive::run_async(
                    prog,
                    vec![
                        "export".into(),
                        "-o".into(),
                        rootfs.to_string_lossy().into(),
                        c.name.clone(),
                    ],
                )
                .await?;
                files.push(payload(&rootfs, "rootfs").await?);
                let out = archive::run_async(prog, vec!["inspect".into(), c.name.clone()]).await?;
                let v: serde_json::Value = serde_json::from_slice(&out.stdout)
                    .map_err(|e| Error::System(format!("{prog} inspect output: {e}")))?;
                let first = v.get(0).cloned().unwrap_or(v);
                let inspect = staging.join("inspect.json");
                tokio::fs::write(
                    &inspect,
                    serde_json::to_vec_pretty(&first).unwrap_or_default(),
                )
                .await?;
                files.push(payload(&inspect, "inspect").await?);
            }
            ContainerRuntime::Lxc => {
                let dir = rt.lxc_path.join(&c.name);
                if !dir.is_dir() {
                    return Err(Error::System(format!(
                        "LXC container directory {} does not exist",
                        dir.display()
                    )));
                }
                let rootfs = staging.join("rootfs.tar");
                archive::run_async(
                    "tar",
                    vec![
                        "--numeric-owner".into(),
                        "--xattrs".into(),
                        // rootfs.raw (sized LXC disks) is a sparse image.
                        "--sparse".into(),
                        // Snapshots aren't part of a backup (as vzdump).
                        "--exclude=./horcrux-snaps".into(),
                        "--exclude=./.setup-mnt".into(),
                        "-cpf".into(),
                        rootfs.to_string_lossy().into(),
                        "-C".into(),
                        dir.to_string_lossy().into(),
                        ".".into(),
                    ],
                )
                .await?;
                files.push(payload(&rootfs, "rootfs").await?);
            }
            ContainerRuntime::Lxd | ContainerRuntime::Incus => {
                let prog = runtime_name(&c.runtime);
                let inst = staging.join("instance.tar.gz");
                archive::run_async(
                    prog,
                    vec![
                        "export".into(),
                        c.name.clone(),
                        inst.to_string_lossy().into(),
                        "--instance-only".into(),
                    ],
                )
                .await?;
                files.push(payload(&inst, "instance").await?);
            }
        }
        Ok(files)
    }

    // ------------------------------------------------------------------
    // Verify
    // ------------------------------------------------------------------

    /// Re-read the archive: its SHA-256 must match the catalog, it must
    /// decompress and unpack, every payload must match the manifest
    /// checksums and every disk must pass `qemu-img check`. The result is
    /// recorded on the backup.
    pub async fn verify_backup(&self, backup_id: &str) -> Result<VerifyResult> {
        let mut b = self.get_backup(backup_id).await?;
        let outcome = self.verify_archive(&b).await;
        let checked_at = now();
        let (ok, files_checked, error) = match outcome {
            Ok(n) => (true, n, None),
            Err(e) => (false, 0, Some(e.to_string())),
        };
        b.verify_state = Some(if ok { "ok" } else { "failed" }.to_string());
        b.verified_at = Some(checked_at);
        b.verify_error = error.clone();
        self.store.put_backup(&b).await?;
        Ok(VerifyResult {
            backup_id: b.id,
            ok,
            checked_at,
            files_checked,
            error,
        })
    }

    async fn verify_archive(&self, b: &Backup) -> Result<usize> {
        let dir = self.unpack_checked(b, None).await?;
        let m = archive::read_manifest(&dir.0)?;
        Ok(m.files.len())
    }

    /// Check the archive checksum, unpack it into a fresh directory (under
    /// `parent`, default next to the archive) and verify every payload.
    async fn unpack_checked(&self, b: &Backup, parent: Option<&Path>) -> Result<TempDir> {
        if !b.path.exists() {
            return Err(Error::System(format!(
                "archive {} is missing",
                b.path.display()
            )));
        }
        if !b.sha256.is_empty() {
            let (sum, _) = archive::sha256_file_async(b.path.clone()).await?;
            if sum != b.sha256 {
                return Err(Error::System(format!(
                    "archive checksum mismatch (expected {}, got {sum}): the archive is corrupted",
                    b.sha256
                )));
            }
        }
        let parent = match parent {
            Some(p) => p.to_path_buf(),
            None => b
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.work_dir.clone()),
        };
        let dir = TempDir(parent.join(format!(".unpack-{}", uuid::Uuid::new_v4())));
        tokio::fs::create_dir_all(&dir.0).await?;
        let (archive_path, d, comp) = (b.path.clone(), dir.0.clone(), b.compression.clone());
        tokio::task::spawn_blocking(move || -> Result<()> {
            archive::extract_archive(&archive_path, &d, &comp)?;
            let m = archive::read_manifest(&d)?;
            archive::verify_extracted(&d, &m)
        })
        .await
        .map_err(|e| Error::System(format!("unpack task failed: {e}")))??;
        Ok(dir)
    }

    // ------------------------------------------------------------------
    // Restore
    // ------------------------------------------------------------------

    /// Restore a backup to its original guest (replacing it) or to a new
    /// guest id. The archive is fully verified before anything existing is
    /// touched.
    pub async fn restore_backup(
        &self,
        backup_id: &str,
        req: RestoreRequest,
    ) -> Result<RestoreResult> {
        let rt = self.rt()?;
        let b = self.get_backup(backup_id).await?;
        let target = req
            .target_id
            .clone()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| b.target_id.clone());
        if !valid_guest_id(&target) {
            return Err(Error::Validation(format!(
                "invalid target id '{target}' (letters, digits, '-', '_', '.')"
            )));
        }
        let _busy = claim(&self.busy, &target, &target)?;
        tracing::info!("Restoring backup {} to {}", b.id, target);

        match b.target_type {
            TargetType::Vm => {
                tokio::fs::create_dir_all(&rt.vm_storage).await?;
                let dir = self.unpack_checked(&b, Some(&rt.vm_storage)).await?;
                let m = archive::read_manifest(&dir.0)?;
                self.restore_vm(rt, &b, &m, &dir.0, &target, &req).await
            }
            TargetType::Container => {
                tokio::fs::create_dir_all(&self.work_dir).await?;
                let dir = self.unpack_checked(&b, Some(&self.work_dir)).await?;
                let m = archive::read_manifest(&dir.0)?;
                self.restore_container(rt, &b, &m, &dir.0, &target, &req)
                    .await
            }
        }
    }

    async fn restore_vm(
        &self,
        rt: &BackupRuntime,
        b: &Backup,
        m: &Manifest,
        dir: &Path,
        target: &str,
        req: &RestoreRequest,
    ) -> Result<RestoreResult> {
        let mut config: VmConfig = serde_json::from_value(m.config.clone())
            .map_err(|e| Error::System(format!("backup has an unreadable VM config: {e}")))?;
        let same = target == m.target_id;
        let existing = rt.vm_manager.get_vm(target).await.ok();
        let replaced = existing.is_some();
        if existing.is_some() {
            if !same && !req.force {
                return Err(Error::Conflict(format!(
                    "VM {target} already exists; restore with force to overwrite it"
                )));
            }
            if crate::vm::QemuManager::process_running(target) {
                return Err(Error::Conflict(format!(
                    "VM {target} is running; stop it before restoring over it"
                )));
            }
        }

        let disks: Vec<&ArchiveFile> = m.files.iter().filter(|f| f.kind == "disk").collect();
        if disks.is_empty() {
            return Err(Error::System("backup contains no VM disks".into()));
        }
        let dests: Vec<PathBuf> = (0..disks.len())
            .map(|i| {
                rt.vm_storage.join(if i == 0 {
                    format!("{target}.qcow2")
                } else {
                    format!("{target}-disk{i}.qcow2")
                })
            })
            .collect();
        if existing.is_none() && !req.force {
            if let Some(d) = dests.iter().find(|d| d.exists()) {
                return Err(Error::Conflict(format!(
                    "disk image {} already exists (not owned by a known VM); restore with force to overwrite it",
                    d.display()
                )));
            }
        }

        // Point of no return: replace the existing VM.
        if existing.is_some() {
            rt.vm_manager.delete_vm(target).await?;
        }
        for (f, dest) in disks.iter().zip(&dests) {
            tokio::fs::rename(dir.join(&f.name), dest)
                .await
                .map_err(|e| {
                    Error::System(format!(
                        "cannot move restored disk to {}: {e}",
                        dest.display()
                    ))
                })?;
        }

        config.id = target.to_string();
        config.name = match &req.name {
            Some(n) if !n.trim().is_empty() => n.trim().to_string(),
            _ if same => config.name,
            _ => format!("{}-restored", config.name),
        };
        config.status = horcrux_common::VmStatus::Stopped;
        let template = config.disks.first().cloned();
        config.disks = dests
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mut d = config
                    .disks
                    .get(i)
                    .cloned()
                    .or_else(|| template.clone())
                    .unwrap_or(horcrux_common::VmDisk {
                        path: String::new(),
                        size_gb: config.disk_size,
                        disk_type: "virtio".into(),
                        cache: "writethrough".into(),
                    });
                d.path = p.to_string_lossy().into();
                d
            })
            .collect();
        let restored = rt.vm_manager.import_vm(config).await?;
        if req.start {
            rt.vm_manager.start_vm(target).await?;
        }
        Ok(RestoreResult {
            backup_id: b.id.clone(),
            target_type: TargetType::Vm,
            target_id: restored.id,
            name: restored.name,
            replaced,
        })
    }

    async fn restore_container(
        &self,
        rt: &BackupRuntime,
        b: &Backup,
        m: &Manifest,
        dir: &Path,
        target: &str,
        req: &RestoreRequest,
    ) -> Result<RestoreResult> {
        let cm = &rt.container_manager;
        let mut config: ContainerConfig =
            serde_json::from_value(m.config.clone()).map_err(|e| {
                Error::System(format!("backup has an unreadable container config: {e}"))
            })?;
        let same = target == m.target_id;
        let name = match &req.name {
            Some(n) if !n.trim().is_empty() => n.trim().to_string(),
            _ if same => config.name.clone(),
            _ => target.to_string(),
        };
        if !valid_guest_id(&name) {
            return Err(Error::Validation(format!(
                "invalid container name '{name}'"
            )));
        }

        let existing = cm.get_container(target).await.ok();
        let replaced = existing.is_some();
        if let Some(ex) = &existing {
            if !same && !req.force {
                return Err(Error::Conflict(format!(
                    "container {target} already exists; restore with force to overwrite it"
                )));
            }
            let st = cm
                .get_container_status(target)
                .await
                .unwrap_or(ex.status.clone());
            if st == ContainerStatus::Running || st == ContainerStatus::Paused {
                return Err(Error::Conflict(format!(
                    "container {target} is {st}; stop it before restoring over it"
                )));
            }
        }

        config.id = target.to_string();
        config.name = name.clone();
        config.status = ContainerStatus::Stopped;
        config.created_at = Some(chrono::Utc::now().to_rfc3339());

        match config.runtime {
            ContainerRuntime::Docker | ContainerRuntime::Podman => {
                let prog = runtime_name(&config.runtime);
                let inspect: serde_json::Value =
                    match tokio::fs::read(dir.join("inspect.json")).await {
                        Ok(raw) => serde_json::from_slice(&raw).unwrap_or_default(),
                        Err(_) => serde_json::Value::Null,
                    };
                let short: String = b.id.chars().take(8).collect();
                let tag = format!("horcrux-restore/{}:{}", target.to_ascii_lowercase(), short);
                let mut args = vec!["import".to_string()];
                for change in image_changes(&inspect) {
                    args.push("--change".into());
                    args.push(change);
                }
                args.push(dir.join("rootfs.tar").to_string_lossy().into());
                args.push(tag.clone());
                archive::run_async(prog, args).await?;
                if config.options.is_empty() {
                    config.options = options_from_inspect(&inspect);
                }
                config.rootfs = tag;
                if existing.is_some() {
                    cm.delete_container(target).await?;
                }
                let created = cm.create_container(config).await?;
                if req.start {
                    cm.start_container(target).await?;
                }
                Ok(RestoreResult {
                    backup_id: b.id.clone(),
                    target_type: TargetType::Container,
                    target_id: created.id,
                    name: created.name,
                    replaced,
                })
            }
            ContainerRuntime::Lxc => {
                let dest = rt.lxc_path.join(&name);
                if existing.is_some() {
                    cm.delete_container(target).await?;
                }
                if dest.exists() {
                    if !req.force && !same {
                        return Err(Error::Conflict(format!(
                            "LXC directory {} already exists; restore with force to overwrite it",
                            dest.display()
                        )));
                    }
                    tokio::fs::remove_dir_all(&dest).await?;
                }
                tokio::fs::create_dir_all(&dest).await?;
                archive::run_async(
                    "tar",
                    vec![
                        "--numeric-owner".into(),
                        "--xattrs".into(),
                        "-xpf".into(),
                        dir.join("rootfs.tar").to_string_lossy().into(),
                        "-C".into(),
                        dest.to_string_lossy().into(),
                    ],
                )
                .await?;
                let old_name = m.config["name"]
                    .as_str()
                    .unwrap_or(&m.target_id)
                    .to_string();
                rewrite_lxc_config(&dest.join("config"), &rt.lxc_path, &old_name, &name).await?;
                // Horcrux-managed LXC containers carry their settings in
                // horcrux.json: record the new id and re-render the config.
                #[cfg(feature = "lxc")]
                cm.lxc().adopt_restored(&name, &config.id)?;
                let registered = cm.register_existing(config).await?;
                if req.start {
                    cm.start_container(target).await?;
                }
                Ok(RestoreResult {
                    backup_id: b.id.clone(),
                    target_type: TargetType::Container,
                    target_id: registered.id,
                    name: registered.name,
                    replaced,
                })
            }
            ContainerRuntime::Lxd | ContainerRuntime::Incus => {
                let prog = runtime_name(&config.runtime);
                if existing.is_some() {
                    cm.delete_container(target).await?;
                }
                archive::run_async(
                    prog,
                    vec![
                        "import".into(),
                        dir.join("instance.tar.gz").to_string_lossy().into(),
                        name.clone(),
                    ],
                )
                .await?;
                let registered = cm.register_existing(config).await?;
                if req.start {
                    cm.start_container(target).await?;
                }
                Ok(RestoreResult {
                    backup_id: b.id.clone(),
                    target_type: TargetType::Container,
                    target_id: registered.id,
                    name: registered.name,
                    replaced,
                })
            }
        }
    }

    // ------------------------------------------------------------------
    // Retention
    // ------------------------------------------------------------------

    /// Apply a retention policy to one guest's backups (each storage
    /// separately). Returns every decision; removes backups unless dry_run.
    pub async fn prune(&self, req: &PruneRequest) -> Result<Vec<PruneDecision>> {
        let all = self.list_backups(Some(req.target_id.clone())).await?;
        let mut storages: Vec<String> = all.iter().map(|b| b.storage.clone()).collect();
        storages.sort();
        storages.dedup();
        let mut decisions = Vec::new();
        for s in storages {
            if let Some(only) = &req.storage {
                if *only != s {
                    continue;
                }
            }
            let group: Vec<&Backup> = all.iter().filter(|b| b.storage == s).collect();
            decisions.extend(retention::mark(&group, &req.policy));
        }
        if !req.dry_run {
            for d in decisions.iter().filter(|d| d.mark == PruneMark::Remove) {
                if let Some(b) = all.iter().find(|b| b.id == d.backup_id) {
                    self.remove_backup(b).await?;
                }
            }
        }
        Ok(decisions)
    }

    /// Prune a guest's backups on every storage (kept for the
    /// `/api/backups/retention/:target_id` route).
    pub async fn apply_retention(
        &self,
        target_id: &str,
        policy: &RetentionPolicy,
    ) -> Result<Vec<PruneDecision>> {
        self.prune(&PruneRequest {
            target_id: target_id.to_string(),
            storage: None,
            policy: policy.clone(),
            dry_run: false,
        })
        .await
    }

    // ------------------------------------------------------------------
    // Jobs
    // ------------------------------------------------------------------

    fn validate_job(job: &mut BackupJob) -> Result<scheduler::Schedule> {
        if job.id.trim().is_empty() {
            job.id = format!("backup-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        }
        if !valid_guest_id(&job.id) {
            return Err(Error::Validation(format!("invalid job id '{}'", job.id)));
        }
        job.targets = job
            .targets
            .iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        job.targets.dedup();
        if job.targets.is_empty() {
            return Err(Error::Validation(
                "a backup job needs at least one VM or container".into(),
            ));
        }
        match job.notify.as_deref().map(str::trim) {
            None | Some("") => job.notify = None,
            Some("always") | Some("failure") | Some("never") => {}
            Some(other) => {
                return Err(Error::Validation(format!(
                    "notify must be \"always\", \"failure\" or \"never\", not \"{other}\""
                )))
            }
        }
        scheduler::Schedule::parse(&job.schedule).map_err(Error::Validation)
    }

    fn next_run(schedule: &scheduler::Schedule) -> Option<i64> {
        schedule
            .next_after(chrono::Local::now())
            .map(|t| t.timestamp())
    }

    pub async fn create_job(&self, mut job: BackupJob) -> Result<BackupJob> {
        let schedule = Self::validate_job(&mut job)?;
        self.resolve_storage(&job.storage).await?;
        let _lock = self.job_lock.lock().await;
        if self.store.get_job(&job.id).await?.is_some() {
            return Err(Error::Conflict(format!(
                "Backup job {} already exists",
                job.id
            )));
        }
        job.last_run = None;
        job.last_status = None;
        job.last_error = None;
        job.next_run = if job.enabled {
            Self::next_run(&schedule)
        } else {
            None
        };
        self.store.put_job(&job).await?;
        Ok(job)
    }

    /// Replace a job's settings (keeps its run history).
    pub async fn update_job(&self, id: &str, mut job: BackupJob) -> Result<BackupJob> {
        job.id = id.to_string();
        let schedule = Self::validate_job(&mut job)?;
        self.resolve_storage(&job.storage).await?;
        let _lock = self.job_lock.lock().await;
        let old = self.get_job(id).await?;
        job.last_run = old.last_run;
        job.last_status = old.last_status;
        job.last_error = old.last_error;
        job.next_run = if job.enabled {
            Self::next_run(&schedule)
        } else {
            None
        };
        self.store.put_job(&job).await?;
        Ok(job)
    }

    pub async fn get_job(&self, id: &str) -> Result<BackupJob> {
        self.store
            .get_job(id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("Backup job not found: {id}")))
    }

    /// Delete a backup job. Backups it produced are kept.
    pub async fn delete_job(&self, job_id: &str) -> Result<()> {
        if self.store.delete_job(job_id).await? {
            Ok(())
        } else {
            Err(Error::NotFound(format!("Backup job not found: {job_id}")))
        }
    }

    pub async fn list_jobs(&self) -> Result<Vec<BackupJob>> {
        self.store.list_jobs().await
    }

    /// Run a job now: back up every target, apply the job's retention to
    /// each, record the outcome on the job and fire the job hook.
    pub async fn run_job(&self, job_id: &str) -> Result<JobRunReport> {
        let job = self.get_job(job_id).await?;
        let _running = claim(&self.running_jobs, &job.id, &format!("job {}", job.id))?;
        let started_at = now();
        tracing::info!(
            "Running backup job {} ({} targets)",
            job.id,
            job.targets.len()
        );
        let mut results = Vec::new();
        for target in &job.targets {
            let cfg = BackupConfig {
                target_id: target.clone(),
                storage: job.storage.clone(),
                mode: job.mode.clone(),
                compression: job.compression.clone(),
                notes: Some(format!("backup job {}", job.id)),
                ..Default::default()
            };
            let mut r = JobTargetResult {
                target_id: target.clone(),
                backup_id: None,
                error: None,
                pruned: Vec::new(),
            };
            match self.create_backup_inner(cfg, Some(job.id.clone())).await {
                Ok(b) => {
                    r.backup_id = Some(b.id.clone());
                    if !job.retention.keeps_all() {
                        match self
                            .prune(&PruneRequest {
                                target_id: target.clone(),
                                storage: Some(b.storage.clone()),
                                policy: job.retention.clone(),
                                dry_run: false,
                            })
                            .await
                        {
                            Ok(d) => {
                                r.pruned = d
                                    .into_iter()
                                    .filter(|d| d.mark == PruneMark::Remove)
                                    .map(|d| d.backup_id)
                                    .collect()
                            }
                            Err(e) => r.error = Some(format!("backup ok, prune failed: {e}")),
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("backup job {}: {} failed: {e}", job.id, target);
                    r.error = Some(e.to_string());
                }
            }
            results.push(r);
        }
        let ok = results.iter().all(|r| r.error.is_none());
        let report = JobRunReport {
            job_id: job.id.clone(),
            started_at,
            finished_at: now(),
            ok,
            results,
        };

        // Re-read so a concurrent edit (enable/disable) isn't overwritten.
        let lock = self.job_lock.lock().await;
        let mut latest = self.get_job(&job.id).await.unwrap_or(job);
        latest.last_run = Some(started_at);
        latest.last_status = Some(if ok { "ok" } else { "failed" }.to_string());
        latest.last_error = if ok {
            None
        } else {
            Some(
                report
                    .results
                    .iter()
                    .filter_map(|r| r.error.as_ref().map(|e| format!("{}: {e}", r.target_id)))
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        };
        self.store.put_job(&latest).await?;
        drop(lock);
        if let Some(hook) = self.runtime.get().and_then(|r| r.on_job_finished.clone()) {
            hook(latest, report.clone()).await;
        }
        Ok(report)
    }

    /// One scheduler pass: run every enabled job whose next run is due
    /// (in the background) and schedule its following run. Returns the ids
    /// of the jobs started.
    pub async fn run_due_jobs(
        self: &Arc<Self>,
        at: chrono::DateTime<chrono::Local>,
    ) -> Vec<String> {
        let jobs = match self.list_jobs().await {
            Ok(j) => j,
            Err(e) => {
                tracing::error!("backup scheduler: cannot list jobs: {e}");
                return Vec::new();
            }
        };
        let mut started = Vec::new();
        for job in jobs {
            if !job.enabled {
                continue;
            }
            let schedule = match scheduler::Schedule::parse(&job.schedule) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("backup job {} has an invalid schedule: {e}", job.id);
                    continue;
                }
            };
            let due = match job.next_run {
                Some(t) => t <= at.timestamp(),
                None => false,
            };
            let next = schedule.next_after(at).map(|t| t.timestamp());
            if due || job.next_run.is_none() {
                // Re-read under the lock: a run may have just recorded its
                // result on the job.
                let _lock = self.job_lock.lock().await;
                let mut fresh = match self.store.get_job(&job.id).await {
                    Ok(Some(j)) => j,
                    _ => continue,
                };
                fresh.next_run = next;
                if let Err(e) = self.store.put_job(&fresh).await {
                    tracing::error!("backup scheduler: cannot save job {}: {e}", job.id);
                    continue;
                }
            }
            if due {
                started.push(job.id.clone());
                let me = self.clone();
                let id = job.id.clone();
                tokio::spawn(async move {
                    match me.run_job(&id).await {
                        Ok(r) => tracing::info!(
                            "scheduled backup job {id} finished: {}",
                            if r.ok { "ok" } else { "failed" }
                        ),
                        Err(e) => tracing::error!("scheduled backup job {id}: {e}"),
                    }
                });
            }
        }
        started
    }

    /// Start the scheduler loop (every `tick`). Missed runs while the
    /// server was down are not caught up: each job's next run is
    /// recomputed from now at start-up.
    pub fn start_scheduler(self: Arc<Self>, tick: std::time::Duration) {
        tokio::spawn(async move {
            let lock = self.job_lock.lock().await;
            if let Ok(jobs) = self.list_jobs().await {
                for mut j in jobs {
                    let next = if j.enabled {
                        scheduler::Schedule::parse(&j.schedule)
                            .ok()
                            .and_then(|s| Self::next_run(&s))
                    } else {
                        None
                    };
                    if next != j.next_run {
                        j.next_run = next;
                        let _ = self.store.put_job(&j).await;
                    }
                }
            }
            drop(lock);
            tracing::info!("Backup scheduler started (tick {:?})", tick);
            let mut interval = tokio::time::interval(tick);
            loop {
                interval.tick().await;
                self.run_due_jobs(chrono::Local::now()).await;
            }
        });
    }
}

fn runtime_name(r: &ContainerRuntime) -> &'static str {
    match r {
        ContainerRuntime::Docker => "docker",
        ContainerRuntime::Podman => "podman",
        ContainerRuntime::Lxc => "lxc",
        ContainerRuntime::Lxd => "lxc",
        ContainerRuntime::Incus => "incus",
    }
}

async fn payload(path: &Path, kind: &str) -> Result<ArchiveFile> {
    let (sha256, size) = archive::sha256_file_async(path.to_path_buf()).await?;
    Ok(ArchiveFile {
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        kind: kind.to_string(),
        size,
        sha256,
        format: None,
        source: None,
    })
}

/// Whether `pid` is a live process. A zombie (exited, not yet reaped -
/// e.g. a daemonized QEMU whose parent is a container's non-reaping PID 1)
/// has stopped writing its disks and counts as exited.
fn pid_alive(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        // Format: "pid (comm) S ...": the state follows the last ')'.
        Ok(stat) => stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .map(|state| state != "Z" && state != "X")
            .unwrap_or(true),
        Err(_) => false,
    }
}

/// Stop a running QEMU VM and wait for its process to exit, so its disk
/// is no longer being written.
async fn stop_vm_and_wait(rt: &BackupRuntime, id: &str) -> Result<()> {
    let pid: Option<u32> = tokio::fs::read_to_string(crate::vm::qemu::pidfile(id))
        .await
        .ok()
        .and_then(|s| s.trim().parse().ok());
    rt.vm_manager.stop_vm(id).await?;
    if let Some(pid) = pid {
        for _ in 0..600 {
            if !pid_alive(pid) {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        return Err(Error::System(format!(
            "VM {id} (pid {pid}) did not exit within 60s of being stopped; backup aborted"
        )));
    }

    Ok(())
}

/// `docker import --change` instructions that restore the image config
/// (`docker export` only carries the filesystem).
fn image_changes(inspect: &serde_json::Value) -> Vec<String> {
    let cfg = &inspect["Config"];
    let mut out = Vec::new();
    let json_array = |v: &serde_json::Value| -> Option<String> {
        v.as_array()
            .filter(|a| !a.is_empty())
            .map(|_| serde_json::to_string(v).unwrap_or_default())
    };
    if let Some(e) = json_array(&cfg["Entrypoint"]) {
        out.push(format!("ENTRYPOINT {e}"));
    }
    if let Some(c) = json_array(&cfg["Cmd"]) {
        out.push(format!("CMD {c}"));
    }
    if let Some(env) = cfg["Env"].as_array() {
        for e in env.iter().filter_map(|e| e.as_str()) {
            if let Some((k, v)) = e.split_once('=') {
                out.push(format!(
                    "ENV {k}={}",
                    serde_json::to_string(v).unwrap_or_default()
                ));
            }
        }
    }
    if let Some(w) = cfg["WorkingDir"].as_str().filter(|w| !w.is_empty()) {
        out.push(format!("WORKDIR {w}"));
    }
    if let Some(u) = cfg["User"].as_str().filter(|u| !u.is_empty()) {
        out.push(format!("USER {u}"));
    }
    out
}

/// Ports/volumes/network of a container from `docker inspect`, for
/// containers Horcrux adopted from the runtime (whose creation options it
/// never saw).
fn options_from_inspect(inspect: &serde_json::Value) -> horcrux_common::ContainerOptions {
    let mut o = horcrux_common::ContainerOptions::default();
    let host = &inspect["HostConfig"];
    if let Some(pb) = host["PortBindings"].as_object() {
        for (cport, binds) in pb {
            for b in binds.as_array().into_iter().flatten() {
                let hp = b["HostPort"].as_str().unwrap_or("");
                let hip = b["HostIp"].as_str().unwrap_or("");
                if hp.is_empty() {
                    continue;
                }
                o.ports.push(if hip.is_empty() {
                    format!("{hp}:{cport}")
                } else {
                    format!("{hip}:{hp}:{cport}")
                });
            }
        }
    }
    if let Some(binds) = host["Binds"].as_array() {
        o.volumes
            .extend(binds.iter().filter_map(|b| b.as_str().map(str::to_string)));
    }
    if let Some(n) = host["NetworkMode"].as_str() {
        if !matches!(n, "" | "default" | "bridge") {
            o.network = Some(n.to_string());
        }
    }
    o
}

/// Point a restored LXC container's config at its new name/directory.
async fn rewrite_lxc_config(cfg: &Path, lxc_path: &Path, old: &str, new: &str) -> Result<()> {
    let raw = match tokio::fs::read_to_string(cfg).await {
        Ok(r) => r,
        Err(_) => return Ok(()),
    };
    if old == new {
        return Ok(());
    }
    let old_dir = lxc_path.join(old).to_string_lossy().to_string();
    let new_dir = lxc_path.join(new).to_string_lossy().to_string();
    let out: Vec<String> = raw
        .lines()
        .map(|l| {
            let key = l.split('=').next().unwrap_or("").trim();
            if key == "lxc.uts.name" || key == "lxc.utsname" {
                format!("{key} = {new}")
            } else {
                l.replace(&format!("{old_dir}/"), &format!("{new_dir}/"))
            }
        })
        .collect();
    tokio::fs::write(cfg, out.join("\n") + "\n").await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_liveness() {
        assert!(pid_alive(std::process::id()));
        // A reaped child is gone.
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let cpid = child.id();
        child.wait().unwrap();
        assert!(!pid_alive(cpid));
        // An exited but unreaped child is a zombie: treated as exited.
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let zpid = child.id();
        for _ in 0..100 {
            if !pid_alive(zpid) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!pid_alive(zpid), "zombie counts as exited");
        child.wait().unwrap();
    }

    #[test]
    fn guest_ids() {
        assert!(valid_guest_id("vm-100"));
        assert!(valid_guest_id("web_1.a"));
        assert!(!valid_guest_id("../etc"));
        assert!(!valid_guest_id(".hidden"));
        assert!(!valid_guest_id("a/b"));
        assert!(!valid_guest_id(""));
    }

    #[test]
    fn docker_image_changes_from_inspect() {
        let v = serde_json::json!({"Config": {
            "Cmd": ["sh", "-c", "sleep 1"], "Entrypoint": null,
            "Env": ["PATH=/bin", "A=b c"], "WorkingDir": "/srv", "User": ""
        }});
        let c = image_changes(&v);
        assert!(c.contains(&r#"CMD ["sh","-c","sleep 1"]"#.to_string()));
        assert!(c.contains(&r#"ENV A="b c""#.to_string()));
        assert!(c.contains(&"WORKDIR /srv".to_string()));
        assert!(!c.iter().any(|x| x.starts_with("USER")));
    }

    #[test]
    fn options_from_docker_inspect() {
        let v = serde_json::json!({"HostConfig": {
            "PortBindings": {"80/tcp": [{"HostIp": "", "HostPort": "8080"}]},
            "Binds": ["/data:/data"], "NetworkMode": "mynet"
        }});
        let o = options_from_inspect(&v);
        assert_eq!(o.ports, vec!["8080:80/tcp"]);
        assert_eq!(o.volumes, vec!["/data:/data"]);
        assert_eq!(o.network.as_deref(), Some("mynet"));
    }

    #[tokio::test]
    async fn lxc_config_is_renamed() {
        let d = tempfile::tempdir().unwrap();
        let cfg = d.path().join("config");
        tokio::fs::write(
            &cfg,
            "lxc.uts.name = old\nlxc.rootfs.path = dir:/var/lib/lxc/old/rootfs\nlxc.net.0.type = veth\n",
        )
        .await
        .unwrap();
        rewrite_lxc_config(&cfg, Path::new("/var/lib/lxc"), "old", "new")
            .await
            .unwrap();
        let s = tokio::fs::read_to_string(&cfg).await.unwrap();
        assert!(s.contains("lxc.uts.name = new"));
        assert!(s.contains("dir:/var/lib/lxc/new/rootfs"));
        assert!(s.contains("lxc.net.0.type = veth"));
    }

    #[test]
    fn job_validation() {
        let mut j = BackupJob {
            id: String::new(),
            comment: None,
            enabled: true,
            schedule: "0 2 * * *".into(),
            targets: vec![" vm1 ".into(), "".into()],
            storage: String::new(),
            mode: BackupMode::Stop,
            compression: Compression::Zstd,
            retention: RetentionPolicy::default(),
            notify: Some("failure".into()),
            last_run: None,
            last_status: None,
            last_error: None,
            next_run: None,
        };
        BackupManager::validate_job(&mut j).unwrap();
        assert!(j.id.starts_with("backup-"));
        assert_eq!(j.targets, vec!["vm1"]);
        j.schedule = "bogus".into();
        assert!(BackupManager::validate_job(&mut j).is_err());
        j.schedule = "daily".into();
        j.notify = Some("someone@example.com".into());
        assert!(BackupManager::validate_job(&mut j).is_err());
    }
}
