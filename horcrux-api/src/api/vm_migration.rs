//! Source side of moving a QEMU VM to another cluster node (the target
//! side is `api/migration_peer.rs`; job tracking and cancel are the
//! `MigrationManager`'s).
//!
//! - Offline (VM stopped, or "offline" chosen for a running VM, which is
//!   shut down first and started again on the target): every disk not on
//!   storage both nodes share is copied over HTTPS in chunks (all-zero
//!   chunks skipped, so sparse images stay sparse; a linked clone's disk
//!   is flattened on the way), verified by SHA-256, together with the EFI
//!   variable store, the cloud-init seed and the TPM state.
//! - Live ("live"/"online", VM running): the target starts QEMU waiting
//!   for the state, the source mirrors each non-shared disk into the
//!   target's NBD export (`drive-mirror`), then QMP `migrate` moves RAM and
//!   device state; when it completes, the mirrors are completed and the
//!   guest runs on the target. Disks on shared storage aren't copied.
//!
//! Cancel, failure or timeout rolls back: migration and mirrors are
//! cancelled (connections to a target that stopped answering are yanked,
//! so the mirror really goes), the target discards what it received, and
//! the VM keeps (or resumes) running on the source. While it runs the
//! source sends the target keepalives: a target that discarded the job or
//! stayed unreachable for `peer_ttl` fails it; the target likewise
//! discards a receive whose source went quiet (`migration_peer`).
use crate::api::migration_peer::{
    self, CommitRequest, FileDigest, FilePlan, PrepareRequest, PrepareResponse, ProbeFile,
    ProbeRequest, StartResponse, CHUNK,
};
use crate::error::ApiError;
use crate::migration::{
    JobReporter, MigrationConfig, MigrationExecutor, MigrationState, MigrationType,
};
use crate::vm::display::{qmp_command, DisplayRuntime};
use crate::AppState;
use horcrux_common::{Error, Result, VmConfig, VmHypervisor};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// HTTP client for one node: the target's API with a fresh
/// `cluster-migration` token on every request.
#[derive(Clone)]
struct Peer {
    base: String,
    client: reqwest::Client,
    local: String,
}

impl Peer {
    fn token(&self) -> Result<String> {
        crate::middleware::auth::generate_cluster_migration_token(&self.local)
            .map_err(Error::System)
    }

    async fn call<T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T> {
        let mut req = self
            .client
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(self.token()?);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| Error::System(format!("target node {}: {e}", self.base)))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            let msg = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| {
                    v["message"]
                        .as_str()
                        .or(v["error"].as_str())
                        .map(str::to_string)
                })
                .unwrap_or(text);
            return Err(if status == reqwest::StatusCode::CONFLICT {
                Error::Conflict(format!("target node: {msg}"))
            } else {
                Error::System(format!("target node answered {status}: {msg}"))
            });
        }
        let text = if text.trim().is_empty() {
            "null".into()
        } else {
            text
        };
        serde_json::from_str(&text).map_err(|e| Error::System(format!("target node reply: {e}")))
    }

    /// Keepalive for `job`: Ok(false) if the target no longer has it.
    async fn keepalive(&self, job: &str) -> Result<bool> {
        let resp = self
            .client
            .post(format!("{}/api/migration/peer/{job}/keepalive", self.base))
            .bearer_auth(self.token()?)
            .timeout(migration_peer::KEEPALIVE_EVERY * 2)
            .send()
            .await
            .map_err(|e| Error::System(format!("target node {}: {e}", self.base)))?;
        match resp.status() {
            s if s.is_success() => Ok(true),
            reqwest::StatusCode::NOT_FOUND => Ok(false),
            s => Err(Error::System(format!("target node answered {s}"))),
        }
    }

    /// Ask the target to discard `job` (idempotent), bounded in time.
    async fn abort(&self, job: &str) -> Result<()> {
        match tokio::time::timeout(
            Duration::from_secs(20),
            self.call::<serde_json::Value>(
                reqwest::Method::POST,
                &format!("/api/migration/peer/{job}/abort"),
                None,
            ),
        )
        .await
        {
            Ok(r) => r.map(|_| ()),
            Err(_) => Err(Error::System(format!(
                "target node {}: no answer within 20 s",
                self.base
            ))),
        }
    }

    async fn put_chunk(
        &self,
        job: &str,
        kind: &str,
        offset: u64,
        total: u64,
        data: Vec<u8>,
    ) -> Result<()> {
        let resp = self
            .client
            .put(format!(
                "{}/api/migration/peer/{job}/file/{kind}?offset={offset}&total={total}",
                self.base
            ))
            .bearer_auth(self.token()?)
            .body(data)
            .send()
            .await
            .map_err(|e| Error::System(format!("copying {kind} to the target: {e}")))?;
        if !resp.status().is_success() {
            let s = resp.status();
            return Err(Error::System(format!(
                "copying {kind} to the target: {s} {}",
                resp.text().await.unwrap_or_default()
            )));
        }
        Ok(())
    }
}

/// What has been done so far (for the rollback).
#[derive(Default)]
struct Progress {
    prepared: bool,
    /// Live: the target's QEMU is waiting / mirrors started / migrate sent.
    mirrors: Vec<String>,
    migrating: bool,
    /// Offline move of a running VM: we shut it down.
    stopped_here: bool,
    /// The commit request was sent (keepalives stop: the target no
    /// longer lists the job while it commits).
    committing: bool,
    /// The target owns the VM now: nothing may be rolled back any more.
    committed: bool,
    /// Live on shared storage: the source's runtime files were renamed
    /// aside (`*.mig-src`) for the target's QEMU.
    runtime_aside: bool,
    probe_files: Vec<PathBuf>,
}

/// Everything about one migration the executor needs.
struct Plan {
    state: Arc<AppState>,
    job_vm: VmConfig,
    target: String,
    peer: Peer,
    live: bool,
    was_running: bool,
    bandwidth: Option<u64>,
    progress: Mutex<Progress>,
    job_id: Mutex<String>,
}

/// Start migrating `config.vm_id` to `config.target_node`; returns the job id.
pub async fn start(
    state: &Arc<AppState>,
    config: MigrationConfig,
) -> std::result::Result<String, ApiError> {
    let vm = state.vm_manager.get_vm(&config.vm_id).await?;
    if vm.hypervisor != VmHypervisor::Qemu {
        return Err(ApiError::BadRequest(
            "LXD/Incus VMs are moved by LXD/Incus itself (lxc/incus move inside an LXD/Incus \
             cluster); Horcrux migrates its QEMU VMs"
                .into(),
        ));
    }
    let local = state
        .cluster_manager
        .get_local_node_name()
        .await
        .unwrap_or_else(|_| "localhost".to_string());
    if config.target_node == local {
        return Err(ApiError::BadRequest(format!(
            "VM {} is already on node {}",
            vm.id, local
        )));
    }
    let node = state
        .cluster_manager
        .list_nodes()
        .await
        .into_iter()
        .find(|n| n.name == config.target_node)
        .ok_or_else(|| {
            ApiError::BadRequest(format!(
                "{} is not a node of this cluster (add it under HA > Cluster first)",
                config.target_node
            ))
        })?;
    if crate::cluster::usage::is_this_host(&node, &local) {
        return Err(ApiError::BadRequest(format!(
            "node {} is this node",
            config.target_node
        )));
    }
    let client = reqwest::Client::builder()
        // Node certificates come from each node's own local CA; the
        // short-lived scoped token is what authenticates the request.
        .danger_accept_invalid_certs(true)
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let peer = Peer {
        base: node.api_url(state.config.server.port),
        client,
        local: local.clone(),
    };
    let was_running = crate::vm::QemuManager::process_running(&vm.id);
    if was_running {
        let disk0 = PathBuf::from(&vm.disks.first().map(|d| d.path.clone()).unwrap_or_default());
        let left = migration_block_jobs(&DisplayRuntime::qmp_socket(&disk0)).await;
        if !left.is_empty() {
            return Err(ApiError::Conflict(format!(
                "VM {} still has the disk mirror {} of an earlier migration to a node that \
                 stopped answering attached; it goes once that node answers again (or stop and \
                 start the VM), then migrate",
                vm.id,
                left.join(", ")
            )));
        }
    }
    let live = was_running && config.migration_type != MigrationType::Offline;
    let bandwidth = state.migration_manager.effective_bandwidth(&config).await;
    let plan = Arc::new(Plan {
        state: state.clone(),
        job_vm: vm,
        target: config.target_node.clone(),
        peer,
        live,
        was_running,
        bandwidth,
        progress: Mutex::new(Progress::default()),
        job_id: Mutex::new(String::new()),
    });
    let (p_run, p_abort) = (plan.clone(), plan.clone());
    let mut config = config;
    if !was_running {
        // A stopped VM can only move offline.
        config.migration_type = MigrationType::Offline;
    }
    let executor = MigrationExecutor {
        run: Box::new(move |r: JobReporter| Box::pin(async move { p_run.run(r).await })),
        abort: Box::new(move |reason: String| {
            Box::pin(async move { p_abort.rollback(reason).await })
        }),
    };
    let job_id = state
        .migration_manager
        .start_migration(config, local, executor)
        .await
        .map_err(|e| match e {
            Error::Conflict(m) => ApiError::Conflict(m),
            other => {
                let m = other.to_string();
                if m.contains("Maximum concurrent migrations") {
                    ApiError::Conflict(m)
                } else {
                    ApiError::Internal(m)
                }
            }
        })?;
    Ok(job_id)
}

impl Plan {
    fn disk0(&self) -> PathBuf {
        PathBuf::from(&self.job_vm.disks[0].path)
    }

    fn qmp(&self) -> PathBuf {
        DisplayRuntime::qmp_socket(&self.disk0())
    }

    /// The source QEMU's QMP socket: renamed aside during a live
    /// migration on shared storage.
    async fn source_qmp(&self) -> PathBuf {
        if self.progress.lock().await.runtime_aside {
            self.disk0()
                .with_extension(format!("qmp.sock.{}", crate::vm::migrate::MIG_SRC_SUFFIX))
        } else {
            self.qmp()
        }
    }

    async fn run(self: Arc<Self>, r: JobReporter) -> Result<()> {
        *self.job_id.lock().await = r.job_id().to_string();
        let job = r.job_id().to_string();
        r.update(MigrationState::Preparing, 2.0).await;

        // Offline move of a running VM: shut it down first (ACPI, then
        // stop after 60 s), start it again on the target.
        if self.was_running && !self.live {
            self.state
                .vm_manager
                .shutdown_vm(&self.job_vm.id, Duration::from_secs(60), true)
                .await?;
            self.progress.lock().await.stopped_here = true;
        }

        // What to move: disks, EFI vars, cloud-init seed, TPM state.
        let files = self.collect_files().await?;
        let shared = self.probe_shared(&job, &files).await?;
        let plans: Vec<FilePlan> = files
            .iter()
            .zip(&shared)
            .map(|((kind, path, size), sh)| FilePlan {
                kind: kind.clone(),
                source_path: path.to_string_lossy().to_string(),
                shared: *sh,
                virtual_size: *size,
            })
            .collect();
        let firewall = self
            .state
            .firewall_manager
            .list_rules(crate::firewall::FirewallScope::Vm(self.job_vm.id.clone()))
            .await;
        let prep: PrepareResponse = self
            .peer
            .call(
                reqwest::Method::POST,
                "/api/migration/peer/prepare",
                Some(
                    serde_json::to_value(PrepareRequest {
                        job_id: job.clone(),
                        vm: self.job_vm.clone(),
                        live: self.live,
                        files: plans.clone(),
                        firewall,
                        keepalive: true,
                    })
                    .map_err(|e| Error::System(e.to_string()))?,
                ),
            )
            .await?;
        self.progress.lock().await.prepared = true;
        r.update(MigrationState::Transferring, 5.0).await;

        let work = async {
            if self.live {
                self.run_live(&r, &job, &plans).await
            } else {
                self.run_offline(&r, &job, &plans).await
            }
        };
        if prep.keepalive {
            tokio::select! {
                res = work => res?,
                e = self.keep_alive(&job) => return Err(e),
            }
        } else {
            work.await?;
        }

        // The VM is the target's now; remove it here.
        r.update(MigrationState::Finalizing, 97.0).await;
        let delete: Vec<PathBuf> = plans
            .iter()
            .filter(|p| !p.shared && p.kind.starts_with("disk"))
            .map(|p| PathBuf::from(&p.source_path))
            .collect();
        let shared_disk0 = plans.iter().any(|p| p.shared && p.kind == "disk0");
        self.state
            .vm_manager
            .forget_migrated(&self.job_vm.id, &delete, shared_disk0)
            .await?;
        // Its firewall moved with it (installed on the target at commit).
        let _ = self
            .state
            .firewall_manager
            .remove_guest_rules(crate::firewall::FirewallScope::Vm(self.job_vm.id.clone()))
            .await;
        tracing::info!(
            "Migration {job}: VM {} moved to {}",
            self.job_vm.id,
            self.target
        );
        Ok(())
    }

    /// Keepalives to the target until the commit; returns why the
    /// migration must stop: the target discarded it, or did not answer
    /// for `peer_ttl` (partitioned, down).
    async fn keep_alive(&self, job: &str) -> Error {
        let ttl = migration_peer::peer_ttl();
        let mut last_ok = std::time::Instant::now();
        let committing = || async {
            let p = self.progress.lock().await;
            p.committing || p.committed
        };
        loop {
            tokio::time::sleep(migration_peer::KEEPALIVE_EVERY).await;
            if committing().await {
                return std::future::pending().await;
            }
            match self.peer.keepalive(job).await {
                Ok(true) => last_ok = std::time::Instant::now(),
                // (A keepalive that crossed the commit request.)
                Ok(false) if committing().await => return std::future::pending().await,
                Ok(false) => {
                    return Error::System(format!(
                        "target node {} no longer has this migration (it discarded its copy)",
                        self.target
                    ))
                }
                Err(e) => {
                    if last_ok.elapsed() > ttl {
                        return Error::System(format!(
                            "lost contact with target node {} for {} s ({e})",
                            self.target,
                            last_ok.elapsed().as_secs()
                        ));
                    }
                }
            }
        }
    }

    /// (kind, path, virtual size) of every file that belongs to the VM.
    async fn collect_files(&self) -> Result<Vec<(String, PathBuf, u64)>> {
        let mut out = Vec::new();
        for (i, d) in self.job_vm.disks.iter().enumerate() {
            let p = PathBuf::from(&d.path);
            if !p.is_file() {
                return Err(Error::Validation(format!(
                    "disk{i} ({}) is not a file on this node; only file-backed disks can be migrated",
                    d.path
                )));
            }
            let size = crate::vm::QemuManager::virtual_size(&p).await?;
            out.push((format!("disk{i}"), p, size));
        }
        let disk0 = self.disk0();
        let efivars = crate::vm::firmware::efivars_path(&disk0);
        if efivars.is_file() {
            let size = crate::vm::QemuManager::virtual_size(&efivars).await?;
            out.push(("efivars".into(), efivars, size));
        }
        let seed = crate::vm::qemu::cloudinit_seed(&disk0);
        if seed.is_file() {
            let size = tokio::fs::metadata(&seed).await?.len();
            out.push(("cidata".into(), seed, size));
        }
        // TPM state: copied offline; a live migration carries it in the
        // migration stream.
        let tpm_dir = crate::vm::firmware::tpm_state_dir(&disk0);
        if !self.live && tpm_dir.is_dir() {
            let mut rd = tokio::fs::read_dir(&tpm_dir).await?;
            while let Some(e) = rd.next_entry().await? {
                let name = e.file_name().to_string_lossy().to_string();
                if e.file_type().await?.is_file() && !name.starts_with('.') {
                    let size = e.metadata().await?.len();
                    out.push((format!("tpm-{name}"), e.path(), size));
                }
            }
        }
        Ok(out)
    }

    /// Which files the target sees at the same path (shared storage).
    async fn probe_shared(&self, job: &str, files: &[(String, PathBuf, u64)]) -> Result<Vec<bool>> {
        let token = migration_peer::hex(&rand::random::<[u8; 16]>());
        let mut probes = Vec::new();
        for (kind, path, _) in files {
            // TPM files and the seed follow disk0; probe disks and EFI vars.
            if kind.starts_with("disk") || kind == "efivars" {
                let pp = migration_peer::probe_path(&path.to_string_lossy());
                if tokio::fs::write(&pp, &token).await.is_ok() {
                    self.progress.lock().await.probe_files.push(pp);
                }
            }
            probes.push(ProbeFile {
                path: path.to_string_lossy().to_string(),
                token: token.clone(),
            });
        }
        let result: Result<Vec<bool>> = self
            .peer
            .call(
                reqwest::Method::POST,
                "/api/migration/peer/probe",
                Some(serde_json::to_value(ProbeRequest { files: probes }).unwrap_or_default()),
            )
            .await;
        for pp in std::mem::take(&mut self.progress.lock().await.probe_files) {
            let _ = tokio::fs::remove_file(pp).await;
        }
        let mut shared = result?;
        // The seed and TPM state live next to disk0 and share its fate.
        let disk0_shared = shared.first().copied().unwrap_or(false);
        for (i, (kind, _, _)) in files.iter().enumerate() {
            if kind == "cidata" || kind.starts_with("tpm-") {
                shared[i] = disk0_shared;
            }
        }
        tracing::info!(
            "Migration {job}: {} of {} file(s) on storage shared with the target",
            shared.iter().filter(|s| **s).count(),
            shared.len()
        );
        Ok(shared)
    }

    async fn run_offline(&self, r: &JobReporter, job: &str, plans: &[FilePlan]) -> Result<()> {
        let to_copy: Vec<&FilePlan> = plans.iter().filter(|p| !p.shared).collect();
        let mut sources = Vec::new();
        let mut total = 0u64;
        for p in &to_copy {
            let (src, temp) = self.copy_source(job, p).await?;
            total += tokio::fs::metadata(&src).await?.len();
            sources.push((p.kind.clone(), src, temp));
        }
        let mut done = 0u64;
        let result = async {
            for (kind, src, _) in &sources {
                self.send_file(r, job, kind, src, &mut done, total).await?;
            }
            Ok::<(), Error>(())
        }
        .await;
        for (_, src, temp) in &sources {
            if *temp {
                let _ = tokio::fs::remove_file(src).await;
            }
        }
        result?;
        r.update(MigrationState::Syncing, 92.0).await;
        self.progress.lock().await.committing = true;
        let _: VmConfig = self
            .peer
            .call(
                reqwest::Method::POST,
                &format!("/api/migration/peer/{job}/commit"),
                Some(
                    serde_json::to_value(CommitRequest {
                        start: self.was_running,
                    })
                    .unwrap_or_default(),
                ),
            )
            .await?;
        self.progress.lock().await.committed = true;
        Ok(())
    }

    /// The file to send for a plan: the disk itself, or for a linked
    /// clone (a qcow2 overlay on a backing file the target doesn't have)
    /// a flattened temporary copy.
    async fn copy_source(&self, job: &str, p: &FilePlan) -> Result<(PathBuf, bool)> {
        let src = PathBuf::from(&p.source_path);
        if p.kind.starts_with("disk")
            && !crate::vm::QemuManager::backing_files(&src).await.is_empty()
        {
            let tmp = src.with_extension(format!("{job}.flat.qcow2"));
            let out = tokio::process::Command::new("qemu-img")
                .args(["convert", "-U", "-O", "qcow2"])
                .arg(&src)
                .arg(&tmp)
                .kill_on_drop(true)
                .output()
                .await
                .map_err(|e| Error::System(format!("qemu-img: {e}")))?;
            if !out.status.success() {
                let _ = tokio::fs::remove_file(&tmp).await;
                return Err(Error::System(format!(
                    "flattening {} for the copy: {}",
                    src.display(),
                    String::from_utf8_lossy(&out.stderr).trim()
                )));
            }
            return Ok((tmp, true));
        }
        Ok((src, false))
    }

    /// Copy one file in chunks (zero chunks skipped) and verify it.
    async fn send_file(
        &self,
        r: &JobReporter,
        job: &str,
        kind: &str,
        src: &Path,
        done: &mut u64,
        total_all: u64,
    ) -> Result<()> {
        use sha2::Digest;
        use tokio::io::AsyncReadExt;
        let mut f = tokio::fs::File::open(src).await?;
        let size = f.metadata().await?.len();
        let mut hasher = sha2::Sha256::new();
        let mut offset = 0u64;
        let limiter = self.bandwidth.map(|mb| mb * 1024 * 1024);
        let started = std::time::Instant::now();
        loop {
            let mut buf = vec![0u8; CHUNK];
            let mut n = 0;
            while n < CHUNK {
                let k = f.read(&mut buf[n..]).await?;
                if k == 0 {
                    break;
                }
                n += k;
            }
            if n == 0 && offset > 0 {
                break;
            }
            buf.truncate(n);
            hasher.update(&buf);
            let zero = buf.iter().all(|b| *b == 0);
            if offset == 0 || !zero {
                self.peer.put_chunk(job, kind, offset, size, buf).await?;
            }
            offset += n as u64;
            *done += n as u64;
            r.bytes(*done, total_all).await;
            let pct = 5.0 + 85.0 * (*done as f32 / total_all.max(1) as f32);
            r.update(MigrationState::Transferring, pct.min(90.0)).await;
            if let Some(bps) = limiter {
                let should = Duration::from_secs_f64(*done as f64 / bps as f64);
                if let Some(wait) = should.checked_sub(started.elapsed()) {
                    tokio::time::sleep(wait).await;
                }
            }
            if n < CHUNK {
                break;
            }
        }
        let want = migration_peer::hex(&hasher.finalize());
        let got: FileDigest = self
            .peer
            .call(
                reqwest::Method::GET,
                &format!("/api/migration/peer/{job}/file/{kind}/sha256"),
                None,
            )
            .await?;
        if got.sha256 != want || got.size != size {
            return Err(Error::System(format!(
                "{kind} arrived damaged on the target (sha256 {} / {} bytes, expected {want} / {size})",
                got.sha256, got.size
            )));
        }
        tracing::info!("Migration {job}: {kind} copied ({size} bytes, sha256 verified)");
        Ok(())
    }

    async fn run_live(&self, r: &JobReporter, job: &str, plans: &[FilePlan]) -> Result<()> {
        // The cloud-init seed is a small read-only file: copied first.
        let mut done = 0;
        for p in plans.iter().filter(|p| !p.shared && p.kind == "cidata") {
            let size = tokio::fs::metadata(&p.source_path).await?.len();
            self.send_file(r, job, "cidata", Path::new(&p.source_path), &mut done, size)
                .await?;
        }
        // Shared storage: the target's QEMU creates its console/QMP sockets
        // at the same paths next to the shared disk; move ours aside first.
        if plans.iter().any(|p| p.shared && p.kind == "disk0") {
            crate::vm::migrate::set_runtime_files_aside(&self.disk0(), false).await;
            self.progress.lock().await.runtime_aside = true;
        }
        let start: StartResponse = self
            .peer
            .call(
                reqwest::Method::POST,
                &format!("/api/migration/peer/{job}/start"),
                None,
            )
            .await?;
        let sock = self.source_qmp().await;
        let speed = self.bandwidth.map(|mb| mb * 1024 * 1024).unwrap_or(0);

        // 1. Mirror each non-shared disk into the target's NBD export.
        let (nbd_host, nbd_port) = start
            .nbd
            .rsplit_once(':')
            .map(|(h, p)| {
                (
                    h.trim_start_matches('[').trim_end_matches(']').to_string(),
                    p.to_string(),
                )
            })
            .ok_or_else(|| Error::System(format!("bad NBD address {}", start.nbd)))?;
        for (drive, export) in &start.exports {
            let job_name = format!("mig-{drive}");
            let mut args = json!({
                "job-id": job_name,
                "device": drive,
                "target": format!("nbd:{nbd_host}:{nbd_port}:exportname={export}"),
                "format": "raw",
                "sync": "full",
                "mode": "existing",
            });
            if speed > 0 {
                args["speed"] = json!(speed);
            }
            qmp_command(&sock, json!({"execute": "drive-mirror", "arguments": args})).await?;
            self.progress.lock().await.mirrors.push(job_name);
        }
        let mirrors = self.progress.lock().await.mirrors.clone();
        // 2. Wait until every mirror has caught up (it then keeps the
        //    target in sync with every new guest write).
        loop {
            let jobs = qmp_command(&sock, json!({"execute": "query-block-jobs"})).await?;
            let list = jobs.as_array().cloned().unwrap_or_default();
            let (mut off, mut len, mut ready) = (0u64, 0u64, 0usize);
            for name in &mirrors {
                let Some(j) = list
                    .iter()
                    .find(|j| j["device"].as_str() == Some(name.as_str()))
                else {
                    return Err(Error::System(format!(
                        "disk mirror {name} stopped before it caught up"
                    )));
                };
                off += j["offset"].as_u64().unwrap_or(0);
                len += j["len"].as_u64().unwrap_or(0);
                if j["ready"].as_bool() == Some(true) {
                    ready += 1;
                }
            }
            r.bytes(off, len).await;
            r.update(
                MigrationState::Transferring,
                5.0 + 55.0 * (off as f32 / len.max(1) as f32),
            )
            .await;
            if ready == mirrors.len() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        // 3. RAM + device state.
        if speed > 0 {
            qmp_command(
                &sock,
                json!({"execute": "migrate-set-parameters", "arguments": {"max-bandwidth": speed}}),
            )
            .await?;
        }
        qmp_command(
            &sock,
            json!({"execute": "migrate", "arguments": {"uri": start.migrate_uri}}),
        )
        .await?;
        self.progress.lock().await.migrating = true;
        loop {
            let m = qmp_command(&sock, json!({"execute": "query-migrate"})).await?;
            match m["status"].as_str().unwrap_or("") {
                "completed" => break,
                "failed" | "cancelled" => {
                    return Err(Error::System(format!(
                        "QEMU migration {}: {}",
                        m["status"].as_str().unwrap_or(""),
                        m["error-desc"].as_str().unwrap_or("no reason given")
                    )))
                }
                _ => {
                    let t = m["ram"]["transferred"].as_u64().unwrap_or(0);
                    let total = m["ram"]["total"].as_u64().unwrap_or(0);
                    r.bytes(t, total).await;
                    let pct = 60.0 + 30.0 * (t as f32 / total.max(1) as f32);
                    r.update(MigrationState::Syncing, pct.min(90.0)).await;
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        // 4. The source is paused (post-migrate): complete the mirrors
        //    without switching the source to them, then hand over.
        for name in &mirrors {
            let _ = qmp_command(
                &sock,
                json!({"execute": "block-job-cancel", "arguments": {"device": name}}),
            )
            .await;
        }
        for _ in 0..100 {
            let jobs = qmp_command(&sock, json!({"execute": "query-block-jobs"})).await?;
            if jobs.as_array().map(|a| a.is_empty()).unwrap_or(true) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        r.update(MigrationState::Finalizing, 94.0).await;
        self.progress.lock().await.committing = true;
        let _: VmConfig = self
            .peer
            .call(
                reqwest::Method::POST,
                &format!("/api/migration/peer/{job}/commit"),
                Some(json!({})),
            )
            .await?;
        let shared_runtime = {
            let mut p = self.progress.lock().await;
            p.committed = true;
            p.runtime_aside
        };
        // The guest runs on the target: end the paused source QEMU. On
        // shared storage not with `quit`: a clean exit unlinks the socket
        // paths QEMU bound, which now hold the target's QMP/serial sockets;
        // forget_migrated SIGKILLs it instead.
        if !shared_runtime {
            let _ = qmp_command(&sock, json!({"execute": "quit"})).await;
        }
        Ok(())
    }

    /// Undo after a failure, a cancel or a timeout: the VM keeps running
    /// here. Err says what could not be undone.
    async fn rollback(self: Arc<Self>, reason: String) -> std::result::Result<(), String> {
        let job = self.job_id.lock().await.clone();
        let p = {
            let mut g = self.progress.lock().await;
            std::mem::take(&mut *g)
        };
        for pp in &p.probe_files {
            let _ = tokio::fs::remove_file(pp).await;
        }
        if p.committed {
            tracing::warn!(
                "Migration {job}: VM {} already belongs to {}; only the source cleanup failed ({reason})",
                self.job_vm.id,
                self.target
            );
            return Ok(());
        }
        let sock = if p.runtime_aside {
            self.disk0()
                .with_extension(format!("qmp.sock.{}", crate::vm::migrate::MIG_SRC_SUFFIX))
        } else {
            self.qmp()
        };
        let mut incomplete = None;
        if self.live && crate::vm::QemuManager::process_running(&self.job_vm.id) {
            if p.migrating {
                let _ = qmp_command(&sock, json!({"execute": "migrate_cancel"})).await;
            }
            for m in &p.mirrors {
                let _ = qmp_command(
                    &sock,
                    json!({"execute": "block-job-cancel", "arguments": {"device": m, "force": true}}),
                )
                .await;
            }
            if !p.mirrors.is_empty() || p.migrating {
                // A cancelled mirror only goes once its in-flight writes
                // to the target finish - never, if the target stopped
                // answering: yank (cut) the connections to it.
                let mut left = wait_jobs_gone(&sock, &p.mirrors, Duration::from_secs(5)).await;
                let stuck_migration = p.migrating && migration_active(&sock).await;
                if !left.is_empty() || stuck_migration {
                    tracing::warn!(
                        "Migration {job}: target not answering, cutting the connections to it"
                    );
                    yank_target_connections(&sock).await;
                    left = wait_jobs_gone(&sock, &p.mirrors, Duration::from_secs(30)).await;
                }
                if !left.is_empty() {
                    incomplete = Some(format!(
                        "disk mirror {} to the target is still attached on this node",
                        left.join(", ")
                    ));
                }
            }
        }
        if p.prepared {
            if let Err(e) = self.peer.abort(&job).await {
                tracing::warn!(
                    "Migration {job}: target could not discard its copy now ({e}); retrying in the \
                     background (it also discards it by itself after {} s without word from us)",
                    migration_peer::peer_ttl().as_secs()
                );
                let (peer, job) = (self.peer.clone(), job.clone());
                tokio::spawn(async move {
                    for _ in 0..120 {
                        tokio::time::sleep(Duration::from_secs(15)).await;
                        if peer.abort(&job).await.is_ok() {
                            tracing::info!("Migration {job}: target discarded its copy");
                            return;
                        }
                    }
                });
            }
        }
        if p.runtime_aside {
            // The target's QEMU is gone (abort): our sockets go back.
            crate::vm::migrate::set_runtime_files_aside(&self.disk0(), true).await;
        }
        let sock = self.qmp();
        if self.live && crate::vm::QemuManager::process_running(&self.job_vm.id) {
            // A migration that completed (guest paused here) but couldn't
            // be committed: the target's copy is gone, resume here.
            if let Ok(s) = qmp_command(&sock, json!({"execute": "query-status"})).await {
                if s["running"].as_bool() == Some(false) {
                    let _ = qmp_command(&sock, json!({"execute": "cont"})).await;
                }
            }
        }
        if p.stopped_here {
            if let Err(e) = self.state.vm_manager.start_vm(&self.job_vm.id).await {
                tracing::error!(
                    "Migration {job}: VM {} could not be started again here: {e}",
                    self.job_vm.id
                );
                incomplete = Some(format!("the VM could not be started again here: {e}"));
            }
        }
        match incomplete {
            None => {
                tracing::info!(
                    "Migration {job}: rolled back ({reason}); VM {} stays on this node",
                    self.job_vm.id
                );
                Ok(())
            }
            Some(left) => {
                tracing::error!(
                    "Migration {job}: rolled back incompletely ({reason}): {left}; VM {} stays on this node",
                    self.job_vm.id
                );
                Err(left)
            }
        }
    }
}

/// Block jobs of an earlier migration (`mig-*`) still on the QEMU behind
/// `sock` (none if it doesn't answer).
async fn migration_block_jobs(sock: &Path) -> Vec<String> {
    match qmp_command(sock, json!({"execute": "query-block-jobs"})).await {
        Ok(jobs) => jobs
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|j| j["device"].as_str())
            .filter(|d| d.starts_with("mig-"))
            .map(str::to_string)
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Wait up to `within` for the block jobs `names` to be gone; returns
/// those still there.
async fn wait_jobs_gone(sock: &Path, names: &[String], within: Duration) -> Vec<String> {
    let end = std::time::Instant::now() + within;
    loop {
        let jobs = match qmp_command(sock, json!({"execute": "query-block-jobs"})).await {
            Ok(j) => j,
            Err(_) => return Vec::new(),
        };
        let left: Vec<String> = names
            .iter()
            .filter(|n| {
                jobs.as_array()
                    .into_iter()
                    .flatten()
                    .any(|j| j["device"].as_str() == Some(n.as_str()))
            })
            .cloned()
            .collect();
        if left.is_empty() || std::time::Instant::now() >= end {
            return left;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Whether the outgoing migration is still active/cancelling.
async fn migration_active(sock: &Path) -> bool {
    tokio::time::sleep(Duration::from_secs(1)).await;
    match qmp_command(sock, json!({"execute": "query-migrate"})).await {
        Ok(m) => matches!(
            m["status"].as_str(),
            Some("active" | "cancelling" | "setup" | "pre-switchover" | "device")
        ),
        Err(_) => false,
    }
}

/// QMP `yank` of the source QEMU's connections to the target: the NBD
/// clients of the mirror targets and the migration stream (never the
/// chardevs: those are the VM's consoles). In-flight requests then fail
/// at once instead of waiting for a TCP timeout.
async fn yank_target_connections(sock: &Path) {
    let Ok(list) = qmp_command(sock, json!({"execute": "query-yank"})).await else {
        return;
    };
    let instances = yank_instances(&list);
    if instances.is_empty() {
        return;
    }
    if let Err(e) = qmp_command(
        sock,
        json!({"execute": "yank", "arguments": {"instances": instances}}),
    )
    .await
    {
        tracing::warn!("yank of the migration connections failed: {e}");
    }
}

/// The yank instances of a `query-yank` reply that belong to the target:
/// block nodes (the source's own disks are files, which register none;
/// the NBD mirror targets do) and the migration.
fn yank_instances(list: &serde_json::Value) -> Vec<serde_json::Value> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter(|i| matches!(i["type"].as_str(), Some("block-node" | "migration")))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only the connections to the target are yanked, never the VM's
    /// consoles (chardevs).
    #[test]
    fn yank_cuts_nbd_and_migration_but_not_consoles() {
        let list = json!([
            {"type": "block-node", "node-name": "#block512"},
            {"type": "chardev", "id": "serial0"},
            {"type": "migration"},
            {"type": "chardev", "id": "qmp"}
        ]);
        let got = yank_instances(&list);
        assert_eq!(
            got,
            vec![
                json!({"type": "block-node", "node-name": "#block512"}),
                json!({"type": "migration"})
            ]
        );
        assert!(yank_instances(&json!([])).is_empty());
        assert!(yank_instances(&json!({"error": "x"})).is_empty());
    }

    /// A leftover mirror of an earlier migration is found through QMP
    /// (a real Unix socket answering like QEMU).
    #[tokio::test]
    async fn leftover_migration_mirror_is_detected() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("vm.qmp.sock");
        let listener = tokio::net::UnixListener::bind(&sock).unwrap();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let (r, mut w) = stream.into_split();
                    let _ = w
                        .write_all(b"{\"QMP\": {\"version\": {}, \"capabilities\": []}}\n")
                        .await;
                    let mut lines = BufReader::new(r).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        let reply = if line.contains("query-block-jobs") {
                            "{\"return\": [{\"device\": \"mig-drive-disk0\", \"type\": \"mirror\"}, {\"device\": \"backup-1\", \"type\": \"backup\"}]}\n"
                        } else {
                            "{\"return\": {}}\n"
                        };
                        let _ = w.write_all(reply.as_bytes()).await;
                    }
                });
            }
        });
        assert_eq!(
            migration_block_jobs(&sock).await,
            vec!["mig-drive-disk0".to_string()]
        );
        let left = wait_jobs_gone(
            &sock,
            &["mig-drive-disk0".to_string()],
            Duration::from_millis(300),
        )
        .await;
        assert_eq!(left, vec!["mig-drive-disk0".to_string()]);
        assert!(wait_jobs_gone(
            &sock,
            &["mig-drive-disk1".to_string()],
            Duration::from_secs(1)
        )
        .await
        .is_empty());
        // No QEMU: nothing to report.
        assert!(migration_block_jobs(&dir.path().join("none.sock"))
            .await
            .is_empty());
    }
}
