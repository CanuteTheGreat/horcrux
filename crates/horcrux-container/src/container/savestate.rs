//! Stateful stop for containers ("save state", the container half of
//! Hyper-V-style save/resume): the running processes are checkpointed with
//! CRIU, the container stops, and a later resume restores them exactly where
//! they were (same PIDs, memory, open sockets).
//!
//! How each runtime does it:
//!
//! | runtime | save | resume | state lives |
//! |---|---|---|---|
//! | LXC | `lxc-checkpoint -s` | `lxc-checkpoint -r` | the state directory |
//! | Incus | `incus stop --stateful` | `incus start` | in the instance |
//! | LXD | `lxc stop --stateful` | `lxc start` | in the instance |
//! | Docker | `docker checkpoint create` | `docker start --checkpoint` | the state directory |
//! | Podman | `podman container checkpoint` | `podman container restore` | in Podman's storage |
//!
//! All of them need CRIU (and, for Docker, its experimental checkpoint
//! support) on the node. [`criu_status`] and [`support`] probe that, so the
//! UI can say per guest whether state can be saved and, when it can't, why;
//! the caller then falls back to a clean shutdown.
use super::ContainerManager;
use horcrux_common::{ContainerRuntime, ContainerStatus, Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::process::Command;

/// Name of the Docker checkpoint Horcrux creates.
pub const DOCKER_CHECKPOINT: &str = "horcrux-save";

/// CRIU on this node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CriuStatus {
    /// `criu check` passed: the binary exists and the kernel supports it.
    pub available: bool,
    pub version: Option<String>,
    /// Why it is not available (shown to the user).
    pub error: Option<String>,
}

/// Whether a runtime can save a container's state right now.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Support {
    pub supported: bool,
    /// What does the work, e.g. `incus stop --stateful`.
    pub mechanism: String,
    /// Why not (shown next to the guest), when unsupported.
    pub reason: Option<String>,
}

/// What a successful save reports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerSaveOutcome {
    /// Size of the saved state, when it is a file/directory Horcrux can see.
    pub size_bytes: Option<u64>,
    pub mechanism: String,
}

static CRIU_CACHE: Mutex<Option<(Instant, CriuStatus)>> = Mutex::new(None);

/// Probe CRIU (`criu --version`, `criu check`). Cached for 30 s: `criu
/// check` forks test processes and every guest row asks.
pub async fn criu_status() -> CriuStatus {
    if let Some((at, s)) = CRIU_CACHE.lock().unwrap().as_ref() {
        if at.elapsed() < Duration::from_secs(30) {
            return s.clone();
        }
    }
    let s = probe_criu().await;
    *CRIU_CACHE.lock().unwrap() = Some((Instant::now(), s.clone()));
    s
}

/// Forget the cached CRIU probe (after installing CRIU, and in tests).
pub fn forget_criu_probe() {
    *CRIU_CACHE.lock().unwrap() = None;
}

async fn probe_criu() -> CriuStatus {
    let bin = std::env::var("HORCRUX_CRIU_BIN").unwrap_or_else(|_| "criu".into());
    let ver = Command::new(&bin).arg("--version").output().await;
    let ver = match ver {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .find_map(|l| l.strip_prefix("Version:"))
            .map(|v| v.trim().to_string()),
        _ => {
            return CriuStatus {
                available: false,
                version: None,
                error: Some(format!(
                    "CRIU ({bin}) is not installed on this node; install it (Gentoo: \
                     `emerge sys-process/criu`) to save container state"
                )),
            }
        }
    };
    match Command::new(&bin).arg("check").output().await {
        Ok(o) if o.status.success() => CriuStatus {
            available: true,
            version: ver,
            error: None,
        },
        Ok(o) => {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&o.stderr),
                String::from_utf8_lossy(&o.stdout)
            );
            let detail: Vec<&str> = text
                .lines()
                .filter(|l| l.contains("Error") || l.contains("error") || l.contains("Warn"))
                .take(3)
                .collect();
            CriuStatus {
                available: false,
                version: ver,
                error: Some(format!(
                    "CRIU is installed but `criu check` fails on this host (the kernel or \
                     container environment lacks checkpoint/restore support): {}",
                    if detail.is_empty() {
                        text.trim()
                            .lines()
                            .last()
                            .unwrap_or("no output")
                            .to_string()
                    } else {
                        detail.join("; ")
                    }
                )),
            }
        }
        Err(e) => CriuStatus {
            available: false,
            version: ver,
            error: Some(format!("cannot run `criu check`: {e}")),
        },
    }
}

pub(super) async fn have(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Is the CLI of `runtime` installed on this node?
pub async fn runtime_present(runtime: &ContainerRuntime) -> bool {
    match runtime {
        // lxc-checkpoint has no --version in every release; any exit means
        // the binary ran.
        ContainerRuntime::Lxc => Command::new("lxc-checkpoint")
            .arg("--help")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .is_ok(),
        ContainerRuntime::Incus => have("incus").await,
        ContainerRuntime::Lxd => have("lxc").await,
        ContainerRuntime::Docker => have("docker").await,
        ContainerRuntime::Podman => have("podman").await,
    }
}

/// Can a container of `runtime` be saved on this node right now?
pub async fn support(runtime: &ContainerRuntime) -> Support {
    let (mechanism, cli) = match runtime {
        ContainerRuntime::Lxc => ("lxc-checkpoint", "lxc-checkpoint"),
        ContainerRuntime::Incus => ("incus stop --stateful", "incus"),
        ContainerRuntime::Lxd => ("lxc stop --stateful", "lxc"),
        ContainerRuntime::Docker => ("docker checkpoint", "docker"),
        ContainerRuntime::Podman => ("podman container checkpoint", "podman"),
    };
    let no = |reason: String| Support {
        supported: false,
        mechanism: mechanism.to_string(),
        reason: Some(reason),
    };
    let present = runtime_present(runtime).await;
    if !present {
        return no(format!("`{cli}` is not installed on this node"));
    }
    let criu = criu_status().await;
    if !criu.available {
        return no(criu.error.unwrap_or_else(|| "CRIU is unavailable".into()));
    }
    if matches!(runtime, ContainerRuntime::Docker) {
        // Docker's checkpoint support is experimental and off by default.
        let out = Command::new("docker")
            .args(["checkpoint", "ls", "horcrux-probe-nonexistent"])
            .output()
            .await;
        if let Ok(o) = out {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&o.stderr),
                String::from_utf8_lossy(&o.stdout)
            );
            let low = text.to_lowercase();
            if !o.status.success()
                && !low.contains("no such container")
                && (low.contains("experimental") || low.contains("unknown command"))
            {
                return no(
                    "Docker's checkpoint/restore is an experimental feature and is \
                     switched off on this node: set \"experimental\": true in \
                     /etc/docker/daemon.json and restart Docker"
                        .into(),
                );
            }
        }
    }
    Support {
        supported: true,
        mechanism: mechanism.to_string(),
        reason: None,
    }
}

/// Incus/LXD instances can only be checkpointed when they are privileged
/// (CRIU cannot restore a user-namespaced container: "Can't open file root on
/// restore: Permission denied") and have no console (`lxc.console.path =
/// none`; otherwise the restore refuses: "lxc.console.path configured on
/// source host but not target"). Returns what to change when they are not.
pub async fn instance_blocker(runtime: &ContainerRuntime, name: &str) -> Option<String> {
    let cli = match runtime {
        ContainerRuntime::Incus => "incus",
        ContainerRuntime::Lxd => "lxc",
        ContainerRuntime::Lxc => return lxc_blocker(name),
        _ => return None,
    };
    let get = |key: &'static str| async move {
        Command::new(cli)
            .args(["config", "get", name, key, "--expanded"])
            .output()
            .await
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    };
    let privileged = get("security.privileged").await == "true";
    let raw = get("raw.lxc").await;
    let no_console = raw
        .lines()
        .any(|l| l.replace(' ', "") == "lxc.console.path=none");
    if privileged && no_console {
        return None;
    }
    Some(format!(
        "{cli} can only save state of privileged instances without a console (CRIU cannot \
         restore user-namespaced containers or ones with a console): run `{cli} config set \
         {name} security.privileged=true raw.lxc=\"lxc.console.path = none\"` and restart the \
         instance"
    ))
}

/// LXC containers can only be checkpointed when privileged, with no console
/// and no ttys (see `LxcFeatures::checkpoint`). Reads the container's config.
fn lxc_blocker(name: &str) -> Option<String> {
    let path = std::env::var("HORCRUX_LXC_PATH").unwrap_or_else(|_| "/var/lib/lxc".into());
    let text = std::fs::read_to_string(Path::new(&path).join(name).join("config")).ok()?;
    let has = |l: &str| text.lines().any(|x| x.replace(' ', "") == l);
    let userns = text
        .lines()
        .any(|x| x.trim_start().starts_with("lxc.idmap"));
    if userns {
        return Some(format!(
            "CRIU cannot checkpoint a user-namespaced (unprivileged) LXC container: create \
             {name} with `unprivileged: false` and the `checkpoint` feature"
        ));
    }
    if !(has("lxc.console.path=none") && has("lxc.tty.max=0")) {
        return Some(format!(
            "LXC container {name} has a console/ttys, which CRIU cannot checkpoint: turn on the \
             container's `checkpoint` feature (Configure -> checkpointable) and restart it"
        ));
    }
    None
}

/// Run `cmd` for at most `timeout` with stdout/stderr going to files, not
/// pipes: what it starts (a restored container) keeps those descriptors, and
/// a pipe would never reach EOF while the container runs. `watch` is a log
/// file and a line that means the run has failed: `lxc-checkpoint --restore`
/// never exits after a CRIU failure, so the log is polled instead of waiting
/// out the timeout.
async fn run_detached(
    cmd: &mut Command,
    what: &str,
    timeout: Duration,
    watch: Option<(PathBuf, &str)>,
) -> Result<String> {
    let dir = std::env::temp_dir();
    let tag = format!(
        "hx-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let (out_p, err_p) = (
        dir.join(format!("{tag}.out")),
        dir.join(format!("{tag}.err")),
    );
    let open = |p: &Path| {
        std::fs::File::create(p).map_err(|e| Error::System(format!("{}: {e}", p.display())))
    };
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(open(&out_p)?)
        .stderr(open(&err_p)?)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| Error::System(format!("failed to run {what}: {e}")))?;
    let deadline = Instant::now() + timeout;
    let mut failure: Option<String> = None;
    let status = loop {
        match tokio::time::timeout(Duration::from_secs(1), child.wait()).await {
            Ok(r) => break r.ok(),
            Err(_) => {
                if let Some((log, marker)) = &watch {
                    if let Ok(t) = std::fs::read_to_string(log) {
                        if t.contains(marker) {
                            failure = Some("CRIU reported a failed restore".to_string());
                        }
                    }
                }
                if failure.is_some() || Instant::now() > deadline {
                    let _ = child.kill().await;
                    break None;
                }
            }
        }
    };
    let out = std::fs::read_to_string(&out_p).unwrap_or_default();
    let err = std::fs::read_to_string(&err_p).unwrap_or_default();
    let _ = std::fs::remove_file(&out_p);
    let _ = std::fs::remove_file(&err_p);
    match status {
        None => {
            let msg =
                failure.unwrap_or_else(|| format!("did not finish within {} s", timeout.as_secs()));
            Err(Error::System(format!(
                "{what} failed: {msg} {}",
                err.trim()
            )))
        }
        Some(st) if !st.success() => {
            let msg = if err.trim().is_empty() {
                out.trim()
            } else {
                err.trim()
            };
            Err(Error::System(format!("{what} failed: {msg}")))
        }
        Some(_) => Ok(out),
    }
}

/// Total size of the files below `p`.
pub fn dir_size(p: &Path) -> u64 {
    let Ok(md) = std::fs::symlink_metadata(p) else {
        return 0;
    };
    if md.is_file() {
        return md.len();
    }
    if !md.is_dir() {
        return 0;
    }
    std::fs::read_dir(p)
        .map(|rd| rd.flatten().map(|e| dir_size(&e.path())).sum())
        .unwrap_or(0)
}

/// Run `cmd` for at most `timeout`; Ok(stdout) or an error with its output.
pub(super) async fn run(cmd: &mut Command, what: &str, timeout: Duration) -> Result<String> {
    let child = cmd.stdin(Stdio::null()).kill_on_drop(true).output();
    let out = tokio::time::timeout(timeout, child)
        .await
        .map_err(|_| {
            Error::System(format!(
                "{what} did not finish within {} s",
                timeout.as_secs()
            ))
        })?
        .map_err(|e| Error::System(format!("failed to run {what}: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let msg = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        return Err(Error::System(format!("{what} failed: {msg}")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// CRIU's own dump/restore log in `dir` (LXC and Docker leave it there),
/// reduced to its error lines.
fn criu_log_errors(dir: &Path, names: &[&str]) -> String {
    for n in names {
        if let Ok(t) = std::fs::read_to_string(dir.join(n)) {
            let errs: Vec<&str> = t.lines().filter(|l| l.contains("Error")).take(4).collect();
            if !errs.is_empty() {
                return format!(" (CRIU: {})", errs.join("; "));
            }
        }
    }
    String::new()
}

impl ContainerManager {
    pub(super) async fn stored(&self, id: &str) -> Result<super::Container> {
        self.containers
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| Error::ContainerNotFound(id.to_string()))
    }

    /// Whether container `id` can be saved right now and why not.
    pub async fn state_support(&self, id: &str) -> Result<Support> {
        let c = self.stored(id).await?;
        let mut sup = support(&c.runtime).await;
        if sup.supported {
            if let Some(why) = instance_blocker(&c.runtime, &c.name).await {
                sup.supported = false;
                sup.reason = Some(why);
            }
        }
        Ok(sup)
    }

    #[cfg(feature = "lxc")]
    pub(super) fn lxc_name_and_path(&self, c: &super::Container) -> (String, PathBuf) {
        (
            self.lxc_manager.resolve_name(&c.id),
            self.lxc_manager.lxc_path().to_path_buf(),
        )
    }

    #[cfg(not(feature = "lxc"))]
    pub(super) fn lxc_name_and_path(&self, c: &super::Container) -> (String, PathBuf) {
        (c.name.clone(), PathBuf::from("/var/lib/lxc"))
    }

    /// Checkpoint the running container `id` and stop it. `dir` is where
    /// runtimes that keep the state in a directory (LXC, Docker) put it;
    /// Incus/LXD/Podman keep it themselves. On failure the container is left
    /// as it was as far as the runtime allows (CRIU resumes the processes of
    /// a failed dump); the caller decides what to do next.
    pub async fn save_container_state(
        &self,
        id: &str,
        dir: &Path,
        timeout: Duration,
    ) -> Result<ContainerSaveOutcome> {
        let c = self.stored(id).await?;
        let mut sup = support(&c.runtime).await;
        if sup.supported {
            if let Some(why) = instance_blocker(&c.runtime, &c.name).await {
                sup.supported = false;
                sup.reason = Some(why);
            }
        }
        if !sup.supported {
            return Err(Error::Validation(sup.reason.unwrap_or_default()));
        }
        let mechanism = sup.mechanism;
        let size_bytes = match c.runtime {
            ContainerRuntime::Lxc => {
                let (name, lxcpath) = self.lxc_name_and_path(&c);
                prepare_dir(dir).await?;
                keep_apparmor_profile(&lxcpath, &name, dir);
                let r = run(
                    Command::new("lxc-checkpoint")
                        .arg("--lxcpath")
                        .arg(&lxcpath)
                        .arg("-n")
                        .arg(&name)
                        .arg("-D")
                        .arg(dir)
                        .args(["-s", "-v"]),
                    "lxc-checkpoint",
                    timeout,
                )
                .await;
                // A container that was itself restored by lxc-checkpoint has
                // no cgroup information in LXC's command handler, and
                // lxc-checkpoint cannot dump it again ("Failed getting
                // freezer path"): run the same CRIU dump directly.
                let r = match r {
                    Err(e) if e.to_string().contains("Failed getting freezer path") => {
                        tracing::info!("lxc-checkpoint cannot dump the restored container {name} (freezer path); running criu dump directly");
                        direct_lxc_dump(&lxcpath, &name, dir, timeout).await
                    }
                    other => other,
                };
                if let Err(e) = r {
                    let extra = criu_log_errors(dir, &["dump.log"]);
                    return Err(Error::System(format!("{e}{extra}")));
                }
                Some(dir_size(dir))
            }
            ContainerRuntime::Docker => {
                prepare_dir(dir).await?;
                let r = run(
                    Command::new("docker")
                        .args(["checkpoint", "create", "--checkpoint-dir"])
                        .arg(dir)
                        .arg(&c.name)
                        .arg(DOCKER_CHECKPOINT),
                    "docker checkpoint create",
                    timeout,
                )
                .await;
                if let Err(e) = r {
                    let extra = criu_log_errors(&dir.join(DOCKER_CHECKPOINT), &["dump.log"]);
                    return Err(Error::System(format!("{e}{extra}")));
                }
                Some(dir_size(dir))
            }
            ContainerRuntime::Incus | ContainerRuntime::Lxd => {
                let cli = if c.runtime == ContainerRuntime::Incus {
                    "incus"
                } else {
                    "lxc"
                };
                // Stateful stop needs this key on the instance.
                let _ = run(
                    Command::new(cli).args(["config", "set", &c.name, "migration.stateful=true"]),
                    "set migration.stateful",
                    Duration::from_secs(30),
                )
                .await;
                run(
                    Command::new(cli).args(["stop", &c.name, "--stateful"]),
                    &format!("{cli} stop --stateful"),
                    timeout,
                )
                .await?;
                None
            }
            ContainerRuntime::Podman => {
                run(
                    Command::new("podman").args(["container", "checkpoint", &c.name]),
                    "podman container checkpoint",
                    timeout,
                )
                .await?;
                None
            }
        };
        if let Some(stored) = self.containers.write().await.get_mut(id) {
            stored.status = ContainerStatus::Stopped;
        }
        Ok(ContainerSaveOutcome {
            size_bytes,
            mechanism,
        })
    }

    /// Restore container `id` from the state saved by
    /// [`Self::save_container_state`]; it continues where it was. The saved
    /// state is not deleted here (see [`Self::discard_container_state`]).
    pub async fn resume_container_state(
        &self,
        id: &str,
        dir: &Path,
        timeout: Duration,
    ) -> Result<()> {
        let c = self.stored(id).await?;
        match c.runtime {
            ContainerRuntime::Lxc => {
                let (name, lxcpath) = self.lxc_name_and_path(&c);
                // LXC 6.0 hands CRIU the literal profile name "generated" and
                // the restore dies in the kernel ("can't write lsm profile").
                let fix = name_generated_apparmor_profile(&lxcpath, &name, dir).await;
                tracing::info!(
                    "lxc restore of {name}: AppArmor profile named explicitly: {}",
                    fix.is_some()
                );
                let r = run_detached(
                    Command::new("lxc-checkpoint")
                        .arg("--lxcpath")
                        .arg(&lxcpath)
                        .arg("-n")
                        .arg(&name)
                        .arg("-D")
                        .arg(dir)
                        .args(["-r", "-v"]),
                    "lxc-checkpoint --restore",
                    timeout,
                    Some((
                        dir.join("restore.log"),
                        "Killing processes because of failure",
                    )),
                )
                .await;
                if let Some((cfg, original)) = fix {
                    let _ = tokio::fs::write(cfg, original).await;
                }
                if let Err(e) = r {
                    let extra = criu_log_errors(dir, &["restore.log"]);
                    return Err(Error::System(format!("{e}{extra}")));
                }
            }
            ContainerRuntime::Docker => {
                // `docker start --checkpoint-dir` is refused by current
                // daemons ("custom checkpointdir is not supported"): the
                // checkpoint is put where the daemon looks for it first.
                install_docker_checkpoint(&c.name, dir).await?;
                let r = run(
                    Command::new("docker")
                        .args(["start", "--checkpoint", DOCKER_CHECKPOINT])
                        .arg(&c.name),
                    "docker start --checkpoint",
                    timeout,
                )
                .await;
                if let Err(e) = r {
                    let extra = criu_log_errors(&dir.join(DOCKER_CHECKPOINT), &["restore.log"]);
                    return Err(Error::System(format!("{e}{extra}")));
                }
            }
            ContainerRuntime::Incus | ContainerRuntime::Lxd => {
                let cli = if c.runtime == ContainerRuntime::Incus {
                    "incus"
                } else {
                    "lxc"
                };
                run(
                    Command::new(cli).args(["start", &c.name]),
                    &format!("{cli} start (restore)"),
                    timeout,
                )
                .await?;
            }
            ContainerRuntime::Podman => {
                if let Ok(img) = run(
                    Command::new("podman").args(["inspect", "--format", "{{.Image}}", &c.name]),
                    "podman inspect",
                    Duration::from_secs(30),
                )
                .await
                {
                    ensure_podman_apparmor_profile(img.trim()).await;
                }
                run(
                    Command::new("podman").args(["container", "restore", &c.name]),
                    "podman container restore",
                    timeout,
                )
                .await?;
            }
        }
        if let Err(e) = self.name_host_veths(id).await {
            tracing::warn!("container {id}: host-side interfaces not renamed: {e}");
        }
        if let Some(stored) = self.containers.write().await.get_mut(id) {
            stored.status = ContainerStatus::Running;
        }
        Ok(())
    }

    /// Throw away runtime-side saved state of `id` (Docker's checkpoint
    /// entry); the state directory itself is the caller's to delete.
    pub async fn discard_container_state(&self, id: &str, _dir: &Path) {
        if let Ok(c) = self.stored(id).await {
            if c.runtime == ContainerRuntime::Docker {
                let _ = Command::new("docker")
                    .args(["checkpoint", "rm"])
                    .arg(&c.name)
                    .arg(DOCKER_CHECKPOINT)
                    .output()
                    .await;
            }
        }
    }
}

/// `criu dump` of LXC container `name` with the arguments `lxc-checkpoint`
/// uses, stopping it (the dump kills the tasks). For containers whose LXC
/// command handler cannot say where their cgroup is (restored ones).
async fn direct_lxc_dump(
    lxcpath: &Path,
    name: &str,
    dir: &Path,
    timeout: Duration,
) -> Result<String> {
    let pid = run(
        Command::new("lxc-info")
            .arg("-P")
            .arg(lxcpath)
            .args(["-n", name, "-p", "-H"]),
        "lxc-info",
        Duration::from_secs(30),
    )
    .await?
    .trim()
    .to_string();
    if pid.parse::<u32>().is_err() {
        return Err(Error::System(format!(
            "container {name} has no init pid ({pid})"
        )));
    }
    // The payload cgroup: the `lxc.payload.*` component of init's cgroup.
    let cg = std::fs::read_to_string(format!("/proc/{pid}/cgroup"))
        .map_err(|e| Error::System(format!("cannot read the cgroup of {pid}: {e}")))?;
    let rel = cg
        .lines()
        .find_map(|l| l.strip_prefix("0::"))
        .and_then(|p| {
            let mut out = String::new();
            for comp in p.split('/').filter(|c| !c.is_empty()) {
                out.push('/');
                out.push_str(comp);
                if comp.starts_with("lxc.payload.") {
                    return Some(out);
                }
            }
            None
        })
        .ok_or_else(|| Error::System(format!("no lxc.payload cgroup for {name} in {cg}")))?;
    let controllers = std::fs::read_to_string("/sys/fs/cgroup/cgroup.controllers")
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(",");
    let rel_trim = rel.trim_start_matches('/');
    let criu = std::env::var("HORCRUX_CRIU_BIN").unwrap_or_else(|_| "criu".into());
    let mut cmd = Command::new(criu);
    cmd.args([
        "dump",
        "--tcp-established",
        "--file-locks",
        "--link-remap",
        "--manage-cgroups=full",
        "--ext-mount-map",
        "auto",
        "--enable-external-sharing",
        "--enable-external-masters",
        "--enable-fs",
        "hugetlbfs",
        "--enable-fs",
        "tracefs",
        "-v4",
    ])
    .arg("-D")
    .arg(dir)
    .arg("-o")
    .arg(dir.join("dump.log"))
    .arg("--cgroup-root")
    .arg(format!("{controllers}:{rel_trim}"))
    .args([
        "--ext-mount-map",
        "/sys/fs/fuse/connections:sys/fs/fuse/connections",
        "-t",
        &pid,
        "--skip-in-flight",
    ])
    .arg("--freeze-cgroup")
    .arg(format!("/sys/fs/cgroup/{rel_trim}"))
    .arg("--force-irmap");
    let out = run(&mut cmd, "criu dump", timeout).await?;
    // The dump killed the tasks; let LXC notice (and clean its cgroup up).
    for _ in 0..30 {
        let st = Command::new("lxc-info")
            .arg("-P")
            .arg(lxcpath)
            .args(["-n", name, "-s", "-H"])
            .output()
            .await
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        if st == "STOPPED" {
            return Ok(out);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let _ = Command::new("lxc-stop")
        .arg("-P")
        .arg(lxcpath)
        .args(["-n", name, "-k"])
        .output()
        .await;
    Ok(out)
}

/// Where a checkpoint keeps the AppArmor profile LXC generated for the
/// container (LXC deletes its own copy when the container stops, and the
/// kernel forgets it at reboot).
const APPARMOR_COPY: &str = "hx-apparmor-profile";

/// Before an LXC dump: keep the container's generated AppArmor profile next
/// to the checkpoint (see [`APPARMOR_COPY`]).
fn keep_apparmor_profile(lxcpath: &Path, name: &str, dir: &Path) {
    let lp = lxcpath.to_string_lossy().to_string();
    let own = lxcpath
        .join(name)
        .join("apparmor")
        .join(format!("lxc-{name}_<{}>", lp.replace('/', "-")));
    // LXC's file is gone once the container was restored (and not started
    // by LXC itself): then the copy kept in the container directory.
    let kept = lxcpath.join(name).join(APPARMOR_COPY);
    if let Some(f) = [own, kept].into_iter().find(|f| f.exists()) {
        let _ = std::fs::copy(&f, dir.join(APPARMOR_COPY));
    }
}

/// For a container whose config says `lxc.apparmor.profile = generated`:
/// load the profile kept with the checkpoint (or LXC's own copy) and name it
/// explicitly in the config for the restore; LXC 6.0 hands CRIU the literal
/// "generated" otherwise. Returns the config path and its original text, to
/// put back afterwards.
async fn name_generated_apparmor_profile(
    lxcpath: &Path,
    name: &str,
    dir: &Path,
) -> Option<(PathBuf, String)> {
    let cfg = lxcpath.join(name).join("config");
    let text = tokio::fs::read_to_string(&cfg).await.ok()?;
    if !text
        .lines()
        .any(|l| l.trim() == "lxc.apparmor.profile = generated")
    {
        return None;
    }
    let lp = lxcpath.to_string_lossy().to_string();
    let own = lxcpath
        .join(name)
        .join("apparmor")
        .join(format!("lxc-{name}_<{}>", lp.replace('/', "-")));
    let file = [dir.join(APPARMOR_COPY), own]
        .into_iter()
        .find(|f| f.exists())?;
    let body = tokio::fs::read_to_string(&file).await.ok()?;
    // Keep it with the container for its next checkpoint.
    let _ = tokio::fs::write(lxcpath.join(name).join(APPARMOR_COPY), &body).await;
    // `profile "lxc-NAME_</var/lib/lxc>" flags=(...) {`
    let profile = body
        .lines()
        .find_map(|l| l.trim().strip_prefix("profile \""))
        .and_then(|r| r.split('"').next())
        .map(str::to_string)?;
    let apparmor_parser = [
        "apparmor_parser",
        "/usr/sbin/apparmor_parser",
        "/sbin/apparmor_parser",
    ];
    let mut loaded = false;
    for bin in apparmor_parser {
        if let Ok(o) = Command::new(bin)
            .args(["-r", "-W"])
            .arg(&file)
            .output()
            .await
        {
            loaded = o.status.success();
            break;
        }
    }
    if !loaded {
        return None;
    }
    let edited: String = text
        .lines()
        .map(|l| {
            if l.trim() == "lxc.apparmor.profile = generated" {
                format!("lxc.apparmor.profile = {profile}")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    tokio::fs::write(&cfg, edited).await.ok()?;
    Some((cfg, text))
}

/// Podman installs its default AppArmor profile (`containers-default-<ver>`)
/// when a container is *started*, never when one is restored, and the kernel
/// forgets it at every reboot: a restore after a reboot, or on a node that
/// never ran a container, dies in the kernel ("can't write lsm profile").
/// Running a throw-away container from `image` loads it (the profile is
/// installed while the spec is generated, before the command even executes,
/// so it does not matter that the entrypoint may not exist in the image).
pub(super) async fn ensure_podman_apparmor_profile(image: &str) {
    let probe = format!("hx-aa-probe-{}", std::process::id());
    let _ = tokio::time::timeout(
        Duration::from_secs(60),
        Command::new("podman")
            .args([
                "run",
                "--rm",
                "--pull=never",
                "--network=none",
                "--name",
                &probe,
                "--entrypoint",
                "/bin/true",
                image,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .status(),
    )
    .await;
    let _ = Command::new("podman")
        .args(["rm", "-f", &probe])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
}

/// Copy the checkpoint `dir`/horcrux-save into the daemon's own checkpoint
/// store of container `name`, replacing an older one.
async fn install_docker_checkpoint(name: &str, dir: &Path) -> Result<()> {
    let root = run(
        Command::new("docker").args(["info", "--format", "{{.DockerRootDir}}"]),
        "docker info",
        Duration::from_secs(30),
    )
    .await?;
    let id = run(
        Command::new("docker").args(["inspect", "--format", "{{.Id}}", name]),
        "docker inspect",
        Duration::from_secs(30),
    )
    .await?;
    let dest = PathBuf::from(root.trim())
        .join("containers")
        .join(id.trim())
        .join("checkpoints");
    let target = dest.join(DOCKER_CHECKPOINT);
    let _ = tokio::fs::remove_dir_all(&target).await;
    tokio::fs::create_dir_all(&dest)
        .await
        .map_err(|e| Error::System(format!("cannot create {}: {e}", dest.display())))?;
    run(
        Command::new("cp")
            .arg("-a")
            .arg(dir.join(DOCKER_CHECKPOINT))
            .arg(&target),
        "copy the checkpoint into Docker's store",
        Duration::from_secs(300),
    )
    .await?;
    Ok(())
}

async fn prepare_dir(dir: &Path) -> Result<()> {
    // CRIU needs a clean, empty directory of its own.
    let _ = tokio::fs::remove_dir_all(dir).await;
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| Error::System(format!("cannot create {}: {e}", dir.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_size_sums_nested_files() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("a")).unwrap();
        std::fs::write(d.path().join("a/x"), vec![0u8; 100]).unwrap();
        std::fs::write(d.path().join("y"), vec![0u8; 50]).unwrap();
        assert_eq!(dir_size(d.path()), 150);
        assert_eq!(dir_size(&d.path().join("none")), 0);
    }

    #[test]
    fn criu_errors_are_pulled_from_the_log() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("dump.log"),
            "(00.1) ok\n(00.2) Error (criu/sk-unix.c:1): bad socket\n(00.3) fine\n",
        )
        .unwrap();
        let s = criu_log_errors(d.path(), &["dump.log"]);
        assert!(s.contains("bad socket"), "{s}");
        assert_eq!(criu_log_errors(d.path(), &["restore.log"]), "");
    }

    #[tokio::test]
    async fn missing_criu_is_reported_with_a_reason() {
        // Serialized with the cache: a bogus binary name.
        std::env::set_var("HORCRUX_CRIU_BIN", "/nonexistent/criu-for-test");
        forget_criu_probe();
        let s = criu_status().await;
        assert!(!s.available);
        assert!(s.error.unwrap().contains("not installed"));
        let sup = support(&ContainerRuntime::Incus).await;
        assert!(!sup.supported);
        assert!(sup.reason.is_some());
        std::env::remove_var("HORCRUX_CRIU_BIN");
        forget_criu_probe();
    }
}
