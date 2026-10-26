//! Cross-node container moves (the container half of drain / rebalance).
//!
//! | runtime | live (CRIU on both nodes) | otherwise |
//! |---|---|---|
//! | Incus / LXD | `incus move` of the running instance | stateful stop (or plain stop) -> `incus move` -> start on the target |
//! | LXC | `lxc-checkpoint` -> copy container dir + checkpoint -> `lxc-checkpoint -r` | stop -> copy dir -> start |
//! | Docker | `docker checkpoint` -> copy image (committed) + volumes + checkpoint -> create + `start --checkpoint` | stop -> copy -> create + start |
//! | Podman | `podman container checkpoint --export` -> copy image + export -> `podman container restore --import` | stop -> copy -> create + start |
//!
//! The bundle for LXC/Docker/Podman is a staging directory
//! (`manifest.json`, `ct/`, `ckpt/`, `image.tar`, `vol-N.tar`) that the API
//! layer streams to the target as a tar. Ownership rule (same as VMs): the
//! container is stopped on the source *before* anything is copied, the target
//! starts it only after the whole bundle arrived, and the source's stopped copy
//! is removed only after the target confirmed it is running. If the target
//! fails, the source restores/starts its own copy (the target having removed
//! its half-import first), so there are never two running copies.
use super::savestate::{self, have, run};
use super::{Container, ContainerManager};
use horcrux_common::{ContainerConfig, ContainerRuntime, ContainerStatus, Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

/// What the target can do for a runtime (answer to `prepare`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MoveSupport {
    /// The runtime CLI exists on this node (and a container of that name does not).
    pub ok: bool,
    /// CRIU checkpoint/restore works here for this runtime.
    pub live: bool,
    pub reason: Option<String>,
}

/// A data mount (named/anonymous volume) copied with the container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveMount {
    pub index: usize,
    pub destination: String,
    pub name: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveManifest {
    pub config: ContainerConfig,
    /// A CRIU checkpoint is part of the bundle (the processes continue).
    pub live: bool,
    /// Docker/Podman image in `image.tar`.
    pub image: Option<String>,
    pub mounts: Vec<MoveMount>,
    /// Host paths bind-mounted into the container: not copied (host state).
    pub bind_mounts: Vec<String>,
    /// The runtime's own id of the container on the source (Docker), which a
    /// CRIU checkpoint refers to (cgroup scope, mount sources).
    #[serde(default)]
    pub source_id: Option<String>,
}

fn sys(m: impl Into<String>) -> Error {
    Error::System(m.into())
}

fn cli_of(rt: &ContainerRuntime) -> &'static str {
    match rt {
        ContainerRuntime::Podman => "podman",
        ContainerRuntime::Lxd => "lxc",
        ContainerRuntime::Incus => "incus",
        _ => "docker",
    }
}

/// Is `id`/`name` valid as a path component here (the target builds paths
/// from names it was sent).
fn safe_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() < 128
        && !n.starts_with('-')
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && n != "."
        && n != ".."
}

/// Can this node take a container of `runtime` called `name`? (Called on the
/// target before anything is stopped.)
pub async fn move_support(runtime: &ContainerRuntime, name: &str) -> MoveSupport {
    if !safe_name(name) {
        return MoveSupport {
            ok: false,
            live: false,
            reason: Some(format!("invalid container name '{name}'")),
        };
    }
    if !savestate::runtime_present(runtime).await {
        return MoveSupport {
            ok: false,
            live: false,
            reason: Some(format!(
                "`{}` is not installed on this node",
                cli_of(runtime)
            )),
        };
    }
    let sup = savestate::support(runtime).await;
    MoveSupport {
        ok: true,
        live: sup.supported,
        reason: sup.reason,
    }
}

/// Fill `cfg` (memory, cpus, command, env, ports, volumes) from `inspect`
/// where the API's record is empty (an adopted container).
async fn fill_from_inspect(cli: &str, cfg: &mut ContainerConfig) -> Result<()> {
    let text = out_of(
        Command::new(cli).args(["inspect", &cfg.name]),
        "inspect container",
    )
    .await?;
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| Error::Internal(e.to_string()))?;
    let i = &v[0];
    let strings = |x: &serde_json::Value| -> Vec<String> {
        x.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    if cfg.memory == 0 {
        cfg.memory = i["HostConfig"]["Memory"].as_u64().unwrap_or(0) / (1024 * 1024);
    }
    if cfg.cpus == 0 {
        cfg.cpus = (i["HostConfig"]["NanoCpus"].as_u64().unwrap_or(0) / 1_000_000_000) as u32;
    }
    if cfg.options.is_empty() {
        cfg.options.command = strings(&i["Config"]["Cmd"]);
        cfg.options.env = strings(&i["Config"]["Env"]);
        if let Some(pb) = i["HostConfig"]["PortBindings"].as_object() {
            for (cport, binds) in pb {
                for b in binds.as_array().into_iter().flatten() {
                    if let Some(hp) = b["HostPort"].as_str().filter(|p| !p.is_empty()) {
                        cfg.options.ports.push(format!("{hp}:{cport}"));
                    }
                }
            }
        }
        for mt in i["Mounts"].as_array().into_iter().flatten() {
            let (src, dst) = (
                mt["Name"]
                    .as_str()
                    .filter(|_| mt["Type"] == "volume")
                    .or_else(|| mt["Source"].as_str()),
                mt["Destination"].as_str(),
            );
            if let (Some(s), Some(d)) = (src, dst) {
                if mt["Type"] == "volume" && s.len() == 64 {
                    continue; // an anonymous volume: the create makes its own
                }
                cfg.options.volumes.push(format!("{s}:{d}"));
            }
        }
    }
    Ok(())
}

/// Replace the 64-hex container id `old` by `new` in every CRIU image below
/// `dir` (same length, so the protobuf framing stays valid). Returns how many
/// files changed.
fn rewrite_container_id(dir: &Path, old: &str, new: &str) -> Result<usize> {
    if old.len() != new.len() || old == new {
        return Ok(0);
    }
    let mut changed = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)? {
            let p = e?.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "img") {
                let data = std::fs::read(&p)?;
                let needle = old.as_bytes();
                if data.windows(needle.len()).any(|w| w == needle) {
                    let mut out = Vec::with_capacity(data.len());
                    let mut i = 0;
                    while i < data.len() {
                        if data[i..].starts_with(needle) {
                            out.extend_from_slice(new.as_bytes());
                            i += needle.len();
                        } else {
                            out.push(data[i]);
                            i += 1;
                        }
                    }
                    std::fs::write(&p, out)?;
                    changed += 1;
                }
            }
        }
    }
    Ok(changed)
}

/// Make the Incus/LXD server listen on the network (`core.https_address`),
/// which a move between two servers needs on both ends. Left alone when it
/// already does.
async fn ensure_listening(cli: &str, cluster_ip: Option<&str>) -> Result<()> {
    let addr = out_of(
        Command::new(cli).args(["config", "get", "core.https_address"]),
        "read core.https_address",
    )
    .await
    .unwrap_or_default();
    if addr.trim().is_empty() {
        // The cluster address, so the other server is told an address that
        // reaches this one (a server listening on every address advertises
        // all of them, including ones that mean something else elsewhere).
        let listen = match cluster_ip {
            Some(ip) if ip.contains(':') && !ip.starts_with('[') => format!("[{ip}]:8443"),
            Some(ip) => format!("{ip}:8443"),
            None => "[::]:8443".to_string(),
        };
        run(
            Command::new(cli).args(["config", "set", "core.https_address", &listen]),
            "set core.https_address",
            Duration::from_secs(30),
        )
        .await?;
    }
    Ok(())
}

async fn out_of(cmd: &mut Command, what: &str) -> Result<String> {
    run(cmd, what, Duration::from_secs(300)).await
}

impl ContainerManager {
    fn lxc_dir_of(&self, c: &Container) -> (String, PathBuf) {
        let (name, path) = self.lxc_name_and_path(c);
        let dir = path.join(&name);
        (name, dir)
    }

    /// Where a bundle for `id` is staged on this node (next to LXC's
    /// containers so an LXC import can rename instead of copy).
    pub fn move_stage_dir(&self, id: &str) -> PathBuf {
        let base = std::env::var("HORCRUX_MOVE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let lxc =
                    std::env::var("HORCRUX_LXC_PATH").unwrap_or_else(|_| "/var/lib/lxc".into());
                PathBuf::from(lxc).join(".horcrux-move")
            });
        base.join(id)
    }

    /// Source side: stop (live: checkpoint) container `id` and lay its bundle
    /// out in `stage`. On return the container is stopped here and everything
    /// needed to run it elsewhere - or here again ([`Self::move_rollback`]) -
    /// is in `stage`. Incus/LXD are not handled here (see [`Self::incus_move`]).
    pub async fn move_export(
        &self,
        id: &str,
        stage: &Path,
        live: bool,
        timeout: Duration,
    ) -> Result<MoveManifest> {
        let c = self.stored(id).await?;
        if !safe_name(&c.name) || !safe_name(&c.id) {
            return Err(Error::Validation(format!(
                "container '{}' has a name that cannot be moved",
                c.name
            )));
        }
        let _ = tokio::fs::remove_dir_all(stage).await;
        tokio::fs::create_dir_all(stage)
            .await
            .map_err(|e| sys(format!("cannot create {}: {e}", stage.display())))?;
        let was_running = matches!(
            self.get_container_status(id).await,
            Ok(ContainerStatus::Running)
        );
        let mut m = MoveManifest {
            config: c.to_config(),
            live: live && was_running,
            image: None,
            mounts: vec![],
            bind_mounts: vec![],
            source_id: None,
        };
        m.config.status = ContainerStatus::Stopped;
        if matches!(
            c.runtime,
            ContainerRuntime::Docker | ContainerRuntime::Podman
        ) {
            // A container the API adopted after a restart knows nothing of
            // how it was created: read it back from the runtime.
            if let Err(e) = fill_from_inspect(cli_of(&c.runtime), &mut m.config).await {
                tracing::warn!("container {id}: settings not read back from the runtime: {e}");
            }
        }
        match c.runtime {
            ContainerRuntime::Lxc => {
                if m.live {
                    self.save_container_state(id, &stage.join("ckpt"), timeout)
                        .await?;
                } else if was_running {
                    self.stop_container(id).await?;
                }
                let (_, dir) = self.lxc_dir_of(&c);
                std::os::unix::fs::symlink(&dir, stage.join("ct"))
                    .map_err(|e| sys(format!("cannot stage {}: {e}", dir.display())))?;
            }
            ContainerRuntime::Docker | ContainerRuntime::Podman => {
                let cli = cli_of(&c.runtime);
                let podman_live = m.live && c.runtime == ContainerRuntime::Podman;
                m.source_id = out_of(
                    Command::new(cli).args(["inspect", "--format", "{{.Id}}", &c.name]),
                    "inspect id",
                )
                .await
                .ok()
                .map(|t| t.trim().to_string());
                if m.live && c.runtime == ContainerRuntime::Docker {
                    self.save_container_state(id, &stage.join("ckpt"), timeout)
                        .await?;
                } else if podman_live {
                    tokio::fs::create_dir_all(stage.join("ckpt"))
                        .await
                        .map_err(|e| sys(e.to_string()))?;
                    run(
                        Command::new("podman")
                            .args(["container", "checkpoint", "--ignore-volumes", "--export"])
                            .arg(stage.join("ckpt").join("podman.tar.gz"))
                            .arg(&c.name),
                        "podman container checkpoint --export",
                        timeout,
                    )
                    .await?;
                    if let Some(s) = self.containers.write().await.get_mut(id) {
                        s.status = ContainerStatus::Stopped;
                    }
                } else if was_running {
                    self.stop_container(id).await?;
                }
                // Mounts: volumes travel, host bind mounts are host state.
                let fmt = out_of(
                    Command::new(cli).args(["inspect", "--format", "{{json .Mounts}}", &c.name]),
                    "inspect mounts",
                )
                .await?;
                let mounts: Vec<serde_json::Value> =
                    serde_json::from_str(fmt.trim()).unwrap_or_default();
                for mt in mounts {
                    let ty = mt["Type"].as_str().unwrap_or("");
                    let src = mt["Source"].as_str().unwrap_or("").to_string();
                    let dst = mt["Destination"].as_str().unwrap_or("").to_string();
                    if ty == "bind" {
                        m.bind_mounts.push(format!("{src}:{dst}"));
                    } else if ty == "volume" && !src.is_empty() {
                        let index = m.mounts.len();
                        let tar = stage.join(format!("vol-{index}.tar"));
                        run(
                            Command::new("tar")
                                .args(["--numeric-owner", "--xattrs", "-cSf"])
                                .arg(&tar)
                                .arg("-C")
                                .arg(&src)
                                .arg("."),
                            "archive volume",
                            timeout,
                        )
                        .await?;
                        m.mounts.push(MoveMount {
                            index,
                            destination: dst,
                            name: mt["Name"].as_str().map(str::to_string),
                            source: src,
                        });
                    }
                }
                // The image: the container's own filesystem changes are
                // committed into it (a live Podman export carries them itself,
                // so the plain base image is enough there).
                let tag = if podman_live {
                    c.rootfs.clone()
                } else {
                    let t = format!("horcrux-move/{}:latest", c.name.to_lowercase());
                    out_of(
                        Command::new(cli).args(["commit", &c.name, &t]),
                        "commit container filesystem",
                    )
                    .await?;
                    t
                };
                run(
                    Command::new(cli)
                        .arg("save")
                        .arg("-o")
                        .arg(stage.join("image.tar"))
                        .arg(&tag),
                    "save image",
                    timeout,
                )
                .await?;
                m.image = Some(tag);
            }
            ContainerRuntime::Incus | ContainerRuntime::Lxd => {
                return Err(Error::Validation(
                    "Incus/LXD instances move with `incus move`".into(),
                ))
            }
        }
        std::fs::write(
            stage.join("manifest.json"),
            serde_json::to_vec_pretty(&m).map_err(|e| Error::Internal(e.to_string()))?,
        )?;
        Ok(m)
    }

    /// Source side, after a failed transfer that the target has confirmed
    /// left nothing running: run the container here again, from its own
    /// checkpoint when there is one.
    pub async fn move_rollback(
        &self,
        id: &str,
        stage: &Path,
        m: &MoveManifest,
        timeout: Duration,
    ) -> Result<()> {
        // Podman keeps the checkpoint of the container it exported (the
        // export is a copy), so every runtime resumes its own.
        self.stored(id).await?;
        if m.live {
            self.resume_container_state(id, &stage.join("ckpt"), timeout)
                .await
        } else {
            self.start_container(id).await.map(|_| ())
        }
    }

    /// Source side, once the target runs the container: forget the stopped
    /// copy here (its runtime object, and a Docker checkpoint) and the staging.
    pub async fn move_finish_source(&self, id: &str, stage: &Path) -> Result<()> {
        let image = std::fs::read_to_string(stage.join("manifest.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<MoveManifest>(&t).ok())
            .and_then(|m| m.image)
            .filter(|t| t.starts_with("horcrux-move/"));
        self.discard_container_state(id, &stage.join("ckpt")).await;
        // The export stopped (checkpointed) the source, but the tracked status
        // may still say Running until the next reconcile; runtimes refuse to
        // delete a container the record calls running (podman did, racing the
        // reconcile loop).
        if let Some(c) = self.containers.write().await.get_mut(id) {
            c.status = ContainerStatus::Stopped;
        }
        let r = self.delete_container(id).await;
        let _ = tokio::fs::remove_dir_all(stage).await;
        if let Some(tag) = image {
            let _ = Command::new("docker").args(["rmi", &tag]).output().await;
            let _ = Command::new("podman").args(["rmi", &tag]).output().await;
        }
        r
    }

    /// Target side: the bundle was extracted into `stage`. Create the
    /// container here and run it (restoring its processes when `m.live`).
    pub async fn move_import(
        &self,
        stage: &Path,
        m: &MoveManifest,
        timeout: Duration,
    ) -> Result<ContainerConfig> {
        let cfg = m.config.clone();
        if !safe_name(&cfg.name) || !safe_name(&cfg.id) {
            return Err(Error::Validation("invalid container name".into()));
        }
        if self.containers.read().await.contains_key(&cfg.id) {
            return Err(Error::Conflict(format!(
                "container {} already exists on this node",
                cfg.id
            )));
        }
        match cfg.runtime {
            ContainerRuntime::Lxc => self.import_lxc(stage, m, timeout).await,
            ContainerRuntime::Docker | ContainerRuntime::Podman => {
                self.import_oci(stage, m, timeout).await
            }
            _ => Err(Error::Validation(
                "Incus/LXD instances move with `incus move`".into(),
            )),
        }
    }

    async fn import_lxc(
        &self,
        stage: &Path,
        m: &MoveManifest,
        timeout: Duration,
    ) -> Result<ContainerConfig> {
        let cfg = &m.config;
        let probe = Container {
            id: cfg.id.clone(),
            name: cfg.name.clone(),
            runtime: ContainerRuntime::Lxc,
            memory: cfg.memory,
            cpus: cfg.cpus,
            rootfs: cfg.rootfs.clone(),
            status: ContainerStatus::Stopped,
            options: cfg.options.clone(),
            created_at: None,
            lxc: cfg.lxc.clone(),
        };
        let (name, dir) = self.lxc_dir_of(&probe);
        if dir.exists() {
            return Err(Error::Conflict(format!(
                "{} already exists on this node",
                dir.display()
            )));
        }
        tokio::fs::rename(stage.join("ct"), &dir)
            .await
            .map_err(|e| sys(format!("cannot place {}: {e}", dir.display())))?;
        #[cfg(feature = "lxc")]
        self.lxc_manager.adopt_restored(&name, &cfg.id)?;
        #[cfg(not(feature = "lxc"))]
        let _ = name;
        self.register_existing(ContainerConfig {
            status: ContainerStatus::Stopped,
            ..cfg.clone()
        })
        .await?;
        let r = if m.live {
            self.resume_container_state(&cfg.id, &stage.join("ckpt"), timeout)
                .await
        } else {
            self.start_container(&cfg.id).await.map(|_| ())
        };
        if let Err(e) = r {
            self.move_abort_import(&cfg.id).await;
            return Err(e);
        }
        self.get_container(&cfg.id).await
    }

    async fn import_oci(
        &self,
        stage: &Path,
        m: &MoveManifest,
        timeout: Duration,
    ) -> Result<ContainerConfig> {
        let cfg = &m.config;
        let cli = cli_of(&cfg.runtime);
        let image = m.image.clone().ok_or_else(|| sys("bundle has no image"))?;
        run(
            Command::new(cli)
                .arg("load")
                .arg("-i")
                .arg(stage.join("image.tar")),
            "load image",
            timeout,
        )
        .await?;
        let podman_live = m.live && cfg.runtime == ContainerRuntime::Podman;
        let r: Result<()> = async {
            if podman_live {
                savestate::ensure_podman_apparmor_profile(&image).await;
                // The export carries no volumes (restoring them would fail on
                // a node that already has a volume of that name): make each
                // volume here and fill it from the bundle.
                for mt in &m.mounts {
                    let Some(vol) = &mt.name else { continue };
                    let have = Command::new("podman")
                        .args(["volume", "inspect", vol])
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status()
                        .await
                        .map(|s| s.success())
                        .unwrap_or(false);
                    if !have {
                        out_of(
                            Command::new("podman").args(["volume", "create", vol]),
                            "create volume",
                        )
                        .await?;
                    }
                    let mp = out_of(
                        Command::new("podman").args([
                            "volume",
                            "inspect",
                            "--format",
                            "{{.Mountpoint}}",
                            vol,
                        ]),
                        "inspect volume",
                    )
                    .await?;
                    run(
                        Command::new("tar")
                            .args(["--numeric-owner", "--xattrs", "-xf"])
                            .arg(stage.join(format!("vol-{}.tar", mt.index)))
                            .arg("-C")
                            .arg(mp.trim()),
                        "restore volume",
                        timeout,
                    )
                    .await?;
                }
                run(
                    Command::new("podman")
                        .args(["container", "restore", "--ignore-volumes", "--import"])
                        .arg(stage.join("ckpt").join("podman.tar.gz"))
                        .arg("--name")
                        .arg(&cfg.name),
                    "podman container restore --import",
                    timeout,
                )
                .await?;
                self.register_existing(ContainerConfig {
                    status: ContainerStatus::Running,
                    rootfs: image.clone(),
                    ..cfg.clone()
                })
                .await?;
                return Ok(());
            }
            // Created from the committed image, like the source's (options,
            // env, ports, volumes, network).
            self.create_container(ContainerConfig {
                rootfs: image.clone(),
                status: ContainerStatus::Stopped,
                ..cfg.clone()
            })
            .await?;
            // Volume data goes into the volumes the create made.
            if !m.mounts.is_empty() {
                let fmt = out_of(
                    Command::new(cli).args(["inspect", "--format", "{{json .Mounts}}", &cfg.name]),
                    "inspect mounts",
                )
                .await?;
                let here: Vec<serde_json::Value> =
                    serde_json::from_str(fmt.trim()).unwrap_or_default();
                for mt in &m.mounts {
                    let Some(dst) = here
                        .iter()
                        .find(|h| h["Destination"].as_str() == Some(mt.destination.as_str()))
                        .and_then(|h| h["Source"].as_str())
                    else {
                        continue;
                    };
                    tokio::fs::create_dir_all(dst).await?;
                    run(
                        Command::new("tar")
                            .args(["--numeric-owner", "--xattrs", "-xf"])
                            .arg(stage.join(format!("vol-{}.tar", mt.index)))
                            .arg("-C")
                            .arg(dst),
                        "restore volume",
                        timeout,
                    )
                    .await?;
                }
            }
            if m.live {
                if cfg.runtime == ContainerRuntime::Docker {
                    // The checkpoint names the source container's cgroup scope
                    // and mount sources by its id: point them at this one, or
                    // the restored processes land in a scope that is not the
                    // container's and it can never be checkpointed again.
                    let new_id = out_of(
                        Command::new(cli).args(["inspect", "--format", "{{.Id}}", &cfg.name]),
                        "inspect id",
                    )
                    .await?;
                    if let Some(old_id) = &m.source_id {
                        let n = rewrite_container_id(&stage.join("ckpt"), old_id, new_id.trim())?;
                        tracing::info!(
                            "container {}: id {} -> {} in {n} checkpoint image(s)",
                            cfg.id,
                            &old_id[..12.min(old_id.len())],
                            &new_id.trim()[..12.min(new_id.trim().len())]
                        );
                    }
                }
                self.resume_container_state(&cfg.id, &stage.join("ckpt"), timeout)
                    .await
            } else {
                self.start_container(&cfg.id).await.map(|_| ())
            }
        }
        .await;
        if let Err(e) = r {
            self.move_abort_import(&cfg.id).await;
            return Err(e);
        }
        self.get_container(&cfg.id).await
    }

    /// Target side: remove whatever a failed/aborted import left (a running
    /// half-restored copy included), so the source can take the guest back.
    pub async fn move_abort_import(&self, id: &str) {
        if let Ok(c) = self.stored(id).await {
            let _ = self.stop_container(id).await;
            if c.runtime == ContainerRuntime::Lxc {
                let (_, dir) = self.lxc_dir_of(&c);
                let _ = Command::new("lxc-stop")
                    .arg("-P")
                    .arg(dir.parent().unwrap_or(Path::new("/var/lib/lxc")))
                    .arg("-n")
                    .arg(&c.name)
                    .arg("-k")
                    .output()
                    .await;
            }
            let _ = self.delete_container(id).await;
        }
    }

    // ------------------------------------------------------------ Incus/LXD

    /// Source side for Incus/LXD: move instance `id` to the Incus/LXD server
    /// at `address` (its trust `token` was made by the target). Live
    /// (`incus move` of the running instance, CRIU on both) when `live`,
    /// else a stateful or plain stop first. Returns whether it ended up
    /// needing to be started on the target (it was running and was stopped
    /// for the move). The instance is removed from this node by `incus move`
    /// itself, after the copy.
    pub async fn incus_move(
        &self,
        id: &str,
        target_name: &str,
        address: &str,
        token: &str,
        live: bool,
        timeout: Duration,
    ) -> Result<bool> {
        let c = self.stored(id).await?;
        let cli = cli_of(&c.runtime);
        if !matches!(c.runtime, ContainerRuntime::Incus | ContainerRuntime::Lxd) {
            return Err(Error::Validation("not an Incus/LXD instance".into()));
        }
        let remote = format!("hx-{}", target_name.to_lowercase());
        if !safe_name(&remote) {
            return Err(Error::Validation(format!(
                "invalid node name {target_name}"
            )));
        }
        // `--mode=push` below: this (source) server connects to the target's
        // address, the one `remote add` was given. The default pull mode has
        // the target dial back to every address this server advertises (all of
        // them when it listens on a wildcard), and an address such as a
        // user-mode NIC's 10.0.2.15 reaches the wrong host or none
        // ("websocket: bad handshake").
        let _ = Command::new(cli)
            .args(["remote", "remove", &remote])
            .output()
            .await;
        run(
            Command::new(cli)
                .args(["remote", "add", &remote, address, "--accept-certificate"])
                .args(["--token", token]),
            &format!("{cli} remote add"),
            Duration::from_secs(60),
        )
        .await?;
        let was_running = matches!(
            self.get_container_status(id).await,
            Ok(ContainerStatus::Running)
        );
        let mut restart_on_target = false;
        let dest = format!("{remote}:");
        let result: Result<()> = async {
            if was_running && live {
                let _ = Command::new(cli)
                    .args(["config", "set", &c.name, "migration.stateful=true"])
                    .output()
                    .await;
                match run(
                    Command::new(cli).args(["move", &c.name, &dest, "--mode=push"]),
                    &format!("{cli} move (live)"),
                    timeout,
                )
                .await
                {
                    Ok(_) => return Ok(()),
                    Err(e) => {
                        tracing::warn!(
                            "live {cli} move of {} failed ({e}); stopping it and moving it cold",
                            c.name
                        );
                        // A failed live move leaves the instance where it was
                        // (never on both): only go on cold when it still is here.
                        let st = self.get_container_status(id).await;
                        if !matches!(
                            st,
                            Ok(ContainerStatus::Running) | Ok(ContainerStatus::Stopped)
                        ) {
                            return Err(e);
                        }
                    }
                }
            }
            if matches!(
                self.get_container_status(id).await,
                Ok(ContainerStatus::Running)
            ) {
                let stateful = live
                    && run(
                        Command::new(cli).args(["stop", &c.name, "--stateful"]),
                        &format!("{cli} stop --stateful"),
                        timeout,
                    )
                    .await
                    .is_ok();
                if !stateful {
                    run(
                        Command::new(cli).args(["stop", &c.name]),
                        &format!("{cli} stop"),
                        timeout,
                    )
                    .await?;
                }
                restart_on_target = was_running;
            }
            run(
                Command::new(cli).args(["move", &c.name, &dest, "--mode=push"]),
                &format!("{cli} move"),
                timeout,
            )
            .await?;
            Ok(())
        }
        .await;
        let _ = Command::new(cli)
            .args(["remote", "remove", &remote])
            .output()
            .await;
        match result {
            Ok(()) => {
                // Gone from this node (incus removed it): drop our record.
                self.containers.write().await.remove(id);
                Ok(restart_on_target)
            }
            Err(e) => {
                // Still here (a stopped copy at worst): bring it back if it ran.
                if restart_on_target {
                    let _ = self.start_container(id).await;
                }
                Err(e)
            }
        }
    }

    /// Target side for Incus/LXD: make this node's server reachable and
    /// return a one-time trust token for the source.
    pub async fn incus_prepare(
        &self,
        runtime: &ContainerRuntime,
        for_node: &str,
        cluster_ip: Option<&str>,
    ) -> Result<String> {
        let cli = cli_of(runtime);
        ensure_listening(cli, cluster_ip).await?;
        let name = format!("hx-{}-{}", for_node.to_lowercase(), std::process::id());
        let text = out_of(
            Command::new(cli).args(["config", "trust", "add", &name]),
            "create trust token",
        )
        .await?;
        // Output: a hint line followed by the token (newer releases print
        // only the token); the token is the longest base64 word.
        text.split_whitespace()
            .filter(|w| {
                w.len() > 40
                    && w.chars().all(|c| {
                        c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-' | '_')
                    })
            })
            .max_by_key(|w| w.len())
            .map(str::to_string)
            .ok_or_else(|| {
                sys(format!(
                    "no trust token in `{cli} config trust add` output: {text}"
                ))
            })
    }

    /// Target side, after `incus move` delivered the instance: track it here
    /// and start it when the source had to stop it for the move.
    pub async fn incus_adopt(
        &self,
        config: ContainerConfig,
        start: bool,
    ) -> Result<ContainerConfig> {
        let cli = cli_of(&config.runtime);
        // The instance must really be here.
        out_of(
            Command::new(cli).args(["info", &config.name]),
            "instance not on this node",
        )
        .await?;
        let id = config.id.clone();
        if !self.containers.read().await.contains_key(&id) {
            self.register_existing(ContainerConfig {
                status: ContainerStatus::Stopped,
                ..config
            })
            .await?;
        }
        if start
            && !matches!(
                self.get_container_status(&id).await,
                Ok(ContainerStatus::Running)
            )
        {
            // A stateful-stopped instance restores its processes on start.
            self.start_container(&id).await?;
        }
        self.get_container(&id).await
    }

    /// Whether `have(cli)`: kept public for the API's capability report.
    pub async fn runtime_installed(&self, runtime: &ContainerRuntime) -> bool {
        have(cli_of(runtime)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_path_safe() {
        assert!(safe_name("web-1"));
        assert!(safe_name("a.b_c"));
        assert!(!safe_name("../x"));
        assert!(!safe_name("-rf"));
        assert!(!safe_name("a/b"));
        assert!(!safe_name(""));
        assert!(!safe_name(".."));
    }

    #[tokio::test]
    async fn an_unsafe_name_is_refused_before_anything_happens() {
        let s = move_support(&ContainerRuntime::Docker, "../etc").await;
        assert!(!s.ok);
        assert!(s.reason.unwrap().contains("invalid container name"));
    }

    #[test]
    fn container_ids_are_rewritten_in_images_only_when_present() {
        let d = tempfile::tempdir().unwrap();
        let old = "a".repeat(64);
        let new = "b".repeat(64);
        std::fs::write(
            d.path().join("cgroup.img"),
            format!("xx{old}yy{old}").as_bytes(),
        )
        .unwrap();
        std::fs::write(d.path().join("core-1.img"), b"nothing here").unwrap();
        std::fs::write(d.path().join("notes.txt"), old.as_bytes()).unwrap();
        assert_eq!(rewrite_container_id(d.path(), &old, &new).unwrap(), 1);
        assert_eq!(
            std::fs::read_to_string(d.path().join("cgroup.img")).unwrap(),
            format!("xx{new}yy{new}")
        );
        assert_eq!(
            std::fs::read_to_string(d.path().join("notes.txt")).unwrap(),
            old
        );
        assert_eq!(rewrite_container_id(d.path(), &old, "short").unwrap(), 0);
    }

    #[test]
    fn manifest_round_trips() {
        let m = MoveManifest {
            config: ContainerConfig {
                id: "a".into(),
                name: "a".into(),
                runtime: ContainerRuntime::Podman,
                ..Default::default()
            },
            live: true,
            source_id: None,
            image: Some("x:y".into()),
            mounts: vec![MoveMount {
                index: 0,
                destination: "/d".into(),
                name: Some("v".into()),
                source: "/s".into(),
            }],
            bind_mounts: vec!["/h:/c".into()],
        };
        let t = serde_json::to_string(&m).unwrap();
        let b: MoveManifest = serde_json::from_str(&t).unwrap();
        assert!(
            b.live
                && b.mounts[0].destination == "/d"
                && b.config.runtime == ContainerRuntime::Podman
        );
    }
}
