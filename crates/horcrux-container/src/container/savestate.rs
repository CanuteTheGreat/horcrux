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

async fn have(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
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
    // lxc-checkpoint has no --version in every release; any exit means the
    // binary ran.
    let present = if matches!(runtime, ContainerRuntime::Lxc) {
        Command::new(cli)
            .arg("--help")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .is_ok()
    } else {
        have(cli).await
    };
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
async fn run(cmd: &mut Command, what: &str, timeout: Duration) -> Result<String> {
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
    async fn stored(&self, id: &str) -> Result<super::Container> {
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
        Ok(support(&c.runtime).await)
    }

    #[cfg(feature = "lxc")]
    fn lxc_name_and_path(&self, c: &super::Container) -> (String, PathBuf) {
        (
            self.lxc_manager.resolve_name(&c.id),
            self.lxc_manager.lxc_path().to_path_buf(),
        )
    }

    #[cfg(not(feature = "lxc"))]
    fn lxc_name_and_path(&self, c: &super::Container) -> (String, PathBuf) {
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
        let sup = support(&c.runtime).await;
        if !sup.supported {
            return Err(Error::Validation(sup.reason.unwrap_or_default()));
        }
        let mechanism = sup.mechanism;
        let size_bytes = match c.runtime {
            ContainerRuntime::Lxc => {
                let (name, lxcpath) = self.lxc_name_and_path(&c);
                prepare_dir(dir).await?;
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
                let r = run(
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
                )
                .await;
                if let Err(e) = r {
                    let extra = criu_log_errors(dir, &["restore.log"]);
                    return Err(Error::System(format!("{e}{extra}")));
                }
            }
            ContainerRuntime::Docker => {
                let r = run(
                    Command::new("docker")
                        .args(["start", "--checkpoint-dir"])
                        .arg(dir)
                        .args(["--checkpoint", DOCKER_CHECKPOINT])
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
    pub async fn discard_container_state(&self, id: &str, dir: &Path) {
        if let Ok(c) = self.stored(id).await {
            if c.runtime == ContainerRuntime::Docker {
                let _ = Command::new("docker")
                    .args(["checkpoint", "rm", "--checkpoint-dir"])
                    .arg(dir)
                    .arg(&c.name)
                    .arg(DOCKER_CHECKPOINT)
                    .output()
                    .await;
            }
        }
    }
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
