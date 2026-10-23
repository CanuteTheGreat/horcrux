//! Cross-node container moves for drain / rebalance (USE=cluster): the
//! peer protocol and the source-side driver. The per-runtime mechanics
//! (Incus/LXD `incus move`; LXC/Docker/Podman checkpoint -> copy -> restore,
//! or stop -> copy -> start) are in `container/transfer.rs`.
//!
//! Single-owner rule, same as VMs: nothing is copied before the source
//! container is stopped (checkpointed), the target starts it only after the
//! whole bundle arrived, the source's stopped copy is deleted only after the
//! target answered "running", and when the target fails the source first has
//! the target remove whatever it has (`abort`) and only then runs its own copy
//! again. If the target cannot be asked, the source stays stopped.
use super::drain::{MoveResult, PeerCtx};
use crate::container::savestate;
use crate::container::transfer::{self, MoveManifest};
use crate::error::ApiError;
use crate::middleware::auth::{AuthUser, CLUSTER_HA_ROLE};
use crate::AppState;
use axum::{
    body::Body,
    extract::{Path, State},
    routing::post,
    Extension, Json, Router,
};
use futures::StreamExt;
use horcrux_common::{ContainerConfig, ContainerRuntime, ContainerStatus};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/ha/peer/ct/prepare", post(peer_prepare))
        .route(
            "/api/ha/peer/ct/receive/:id",
            post(peer_receive).layer(axum::extract::DefaultBodyLimit::disable()),
        )
        .route("/api/ha/peer/ct/adopt", post(peer_adopt))
        .route("/api/ha/peer/ct/abort/:id", post(peer_abort))
}

fn require_peer(user: &Option<Extension<AuthUser>>) -> Result<(), ApiError> {
    match user {
        Some(Extension(u)) if u.role == CLUSTER_HA_ROLE => Ok(()),
        _ => Err(ApiError::Forbidden(
            "this route is only for other cluster nodes".into(),
        )),
    }
}

#[derive(Serialize, Deserialize)]
struct PrepareReq {
    id: String,
    name: String,
    runtime: ContainerRuntime,
    from: String,
}

#[derive(Serialize, Deserialize)]
struct PrepareResp {
    ok: bool,
    live: bool,
    reason: Option<String>,
    /// Incus/LXD: one-time trust token for the source's `remote add`.
    token: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct AdoptReq {
    config: ContainerConfig,
    start: bool,
}

#[derive(Serialize, Deserialize)]
struct Received {
    status: String,
}

/// This node's address as the other cluster nodes know it.
async fn local_ip(state: &AppState) -> Option<String> {
    let me = state.cluster_manager.get_local_node_name().await.ok()?;
    state
        .cluster_manager
        .list_nodes()
        .await
        .into_iter()
        .find(|n| n.is_local || n.name == me)
        .map(|n| n.ip)
        .filter(|ip| !ip.is_empty())
}

/// Where on this node a bundle for container `id` is unpacked.
fn stage_dir(state: &AppState, id: &str) -> std::path::PathBuf {
    state.container_manager.move_stage_dir(id)
}

async fn peer_prepare(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Json(req): Json<PrepareReq>,
) -> Result<Json<PrepareResp>, ApiError> {
    require_peer(&user)?;
    let sup = transfer::move_support(&req.runtime, &req.name).await;
    if !sup.ok {
        return Ok(Json(PrepareResp {
            ok: false,
            live: false,
            reason: sup.reason,
            token: None,
        }));
    }
    if state.container_manager.get_container(&req.id).await.is_ok() {
        return Ok(Json(PrepareResp {
            ok: false,
            live: false,
            reason: Some(format!("container {} already exists on this node", req.id)),
            token: None,
        }));
    }
    let mut token = None;
    if matches!(req.runtime, ContainerRuntime::Incus | ContainerRuntime::Lxd) {
        match state
            .container_manager
            .incus_prepare(&req.runtime, &req.from, local_ip(&state).await.as_deref())
            .await
        {
            Ok(t) => token = Some(t),
            Err(e) => {
                return Ok(Json(PrepareResp {
                    ok: false,
                    live: false,
                    reason: Some(format!("cannot accept an Incus/LXD move: {e}")),
                    token: None,
                }))
            }
        }
    }
    Ok(Json(PrepareResp {
        ok: true,
        live: sup.live,
        reason: sup.reason,
        token,
    }))
}

async fn peer_receive(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Path(id): Path<String>,
    body: Body,
) -> Result<Json<Received>, ApiError> {
    require_peer(&user)?;
    let stage = stage_dir(&state, &id);
    let _ = tokio::fs::remove_dir_all(&stage).await;
    tokio::fs::create_dir_all(&stage)
        .await
        .map_err(|e| ApiError::Internal(format!("cannot create {}: {e}", stage.display())))?;
    let mut tar = Command::new("tar")
        .args(["--numeric-owner", "--xattrs", "-xf", "-", "-C"])
        .arg(&stage)
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| ApiError::Internal(format!("cannot run tar: {e}")))?;
    let mut stdin = tar.stdin.take().expect("piped");
    let mut stream = body.into_data_stream();
    let mut copy_err = None;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(c) => {
                if let Err(e) = stdin.write_all(&c).await {
                    copy_err = Some(e.to_string());
                    break;
                }
            }
            Err(e) => {
                copy_err = Some(format!("transfer interrupted: {e}"));
                break;
            }
        }
    }
    drop(stdin);
    let out = tar
        .wait_with_output()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if copy_err.is_some() || !out.status.success() {
        let _ = tokio::fs::remove_dir_all(&stage).await;
        return Err(ApiError::Internal(format!(
            "bundle not received: {} {}",
            copy_err.unwrap_or_default(),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let m: MoveManifest = serde_json::from_slice(
        &tokio::fs::read(stage.join("manifest.json"))
            .await
            .map_err(|e| ApiError::Internal(format!("bundle has no manifest: {e}")))?,
    )
    .map_err(|e| ApiError::Internal(format!("bad manifest: {e}")))?;
    if m.config.id != id {
        let _ = tokio::fs::remove_dir_all(&stage).await;
        return Err(ApiError::ValidationError(
            "bundle is for another container".into(),
        ));
    }
    let r = state
        .container_manager
        .move_import(&stage, &m, Duration::from_secs(1800))
        .await;
    let _ = tokio::fs::remove_dir_all(&stage).await;
    match r {
        Ok(c) => {
            tracing::info!("container {id} received and {:?}", c.status);
            Ok(Json(Received {
                status: format!("{:?}", c.status).to_lowercase(),
            }))
        }
        Err(e) => Err(ApiError::Internal(format!("import failed: {e}"))),
    }
}

async fn peer_adopt(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Json(req): Json<AdoptReq>,
) -> Result<Json<Received>, ApiError> {
    require_peer(&user)?;
    let c = state
        .container_manager
        .incus_adopt(req.config, req.start)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(Received {
        status: format!("{:?}", c.status).to_lowercase(),
    }))
}

/// Remove whatever this node has of container `id` (a half-imported or
/// restored copy), so the source may run its own again. Idempotent.
async fn peer_abort(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Path(id): Path<String>,
) -> Result<Json<Received>, ApiError> {
    require_peer(&user)?;
    state.container_manager.move_abort_import(&id).await;
    let _ = tokio::fs::remove_dir_all(stage_dir(&state, &id)).await;
    if state.container_manager.get_container(&id).await.is_ok() {
        return Err(ApiError::Internal(format!(
            "container {id} is still present on this node"
        )));
    }
    Ok(Json(Received {
        status: "absent".into(),
    }))
}

// ------------------------------------------------------------- the driver

/// Stream `stage` (a tar of its contents) to `url`.
async fn upload_stage(
    stage: &std::path::Path,
    url: &str,
    token: &str,
    timeout: Duration,
) -> Result<(), String> {
    // The stage holds regular files plus symlinks to big directories (an LXC
    // container's directory): follow only those top-level links - `tar -h`
    // would also follow the guest's own absolute symlinks (busybox's
    // /sbin/reboot -> /bin/busybox ...) into the host.
    let mut cmd = Command::new("tar");
    cmd.args(["--numeric-owner", "--xattrs", "-cSf", "-"]);
    let entries =
        std::fs::read_dir(stage).map_err(|e| format!("cannot read {}: {e}", stage.display()))?;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let path = e.path();
        match std::fs::read_link(&path) {
            Ok(target) => {
                let parent = target.parent().unwrap_or(std::path::Path::new("/"));
                let base = target
                    .file_name()
                    .map(|b| b.to_string_lossy().to_string())
                    .unwrap_or_default();
                cmd.arg("-C").arg(parent);
                cmd.arg(format!(
                    "--transform=s,^{},{},",
                    base.replace('.', "\\."),
                    name
                ));
                cmd.arg(&base);
            }
            Err(_) => {
                cmd.arg("-C").arg(stage).arg(&name);
            }
        }
    }
    let mut tar = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("cannot run tar: {e}"))?;
    let stdout = tar.stdout.take().expect("piped");
    let stream = futures::stream::unfold(stdout, |mut out| async move {
        let mut buf = vec![0u8; 256 * 1024];
        match out.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok::<_, std::io::Error>(buf), out))
            }
            Err(e) => Some((Err(e), out)),
        }
    });
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .connect_timeout(Duration::from_secs(5))
        .timeout(timeout)
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .post(url)
        .bearer_auth(token)
        .header("content-type", "application/x-tar")
        .body(reqwest::Body::wrap_stream(stream))
        .send()
        .await
        .map_err(|e| format!("transfer failed: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let tar_out = tar.wait_with_output().await;
    if !status.is_success() {
        return Err(format!("{status}: {text}"));
    }
    if let Ok(o) = tar_out {
        // Exit 1 is tar's "a file changed as it was read" (a log line in the
        // stopped container's directory): the data went through.
        if !o.status.success() && o.status.code() != Some(1) {
            return Err(format!(
                "tar of the bundle failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ));
        }
    }
    Ok(())
}

fn host_of(api_url: &str) -> String {
    let h = api_url
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    // `[v6]:port` or `host:port`
    if let Some(rest) = h.strip_prefix('[') {
        return format!("[{}]", rest.split(']').next().unwrap_or(""));
    }
    h.rsplit_once(':').map(|(a, _)| a).unwrap_or(h).to_string()
}

/// Move container `r.guest_id` from this node to `r.to`. Fills in `r`
/// (status done/failed, error) and returns it.
pub async fn move_container_here(
    state: &Arc<AppState>,
    ctx: &PeerCtx,
    mut r: MoveResult,
    timeout_secs: u64,
) -> MoveResult {
    let timeout = Duration::from_secs(timeout_secs.max(60));
    match run_move(state, ctx, &r, timeout).await {
        Ok(()) => {
            r.status = "done".into();
            r.progress = 100;
        }
        Err(e) => {
            r.status = "failed".into();
            r.error = Some(e);
        }
    }
    r
}

async fn run_move(
    state: &Arc<AppState>,
    ctx: &PeerCtx,
    r: &MoveResult,
    timeout: Duration,
) -> Result<(), String> {
    let id = &r.guest_id;
    let cm = &state.container_manager;
    let cfg = cm.get_container(id).await.map_err(|e| e.to_string())?;
    let nodes = state.cluster_manager.list_nodes().await;
    let node = nodes
        .iter()
        .find(|n| n.name == r.to)
        .ok_or_else(|| format!("{} is not a node of this cluster", r.to))?;
    let base = node.api_url(state.config.server.port);
    let token = crate::middleware::auth::generate_cluster_ha_token(&ctx.local)?;

    // 1. Ask the target before anything is stopped.
    let prep: PrepareResp = ctx
        .post(
            node,
            "/api/ha/peer/ct/prepare",
            &PrepareReq {
                id: id.clone(),
                name: cfg.name.clone(),
                runtime: cfg.runtime.clone(),
                from: ctx.local.clone(),
            },
        )
        .await?;
    if !prep.ok {
        return Err(format!(
            "{} cannot take container {}: {}",
            r.to,
            cfg.name,
            prep.reason.unwrap_or_default()
        ));
    }
    let local_live = savestate::support(&cfg.runtime).await.supported;
    let live = prep.live && local_live;
    let running = matches!(cfg.status, ContainerStatus::Running);

    // 2a. Incus / LXD: `incus move`.
    if matches!(cfg.runtime, ContainerRuntime::Incus | ContainerRuntime::Lxd) {
        let token_inc = prep
            .token
            .ok_or_else(|| "the target gave no trust token".to_string())?;
        let address = format!("{}:8443", host_of(&base));
        let restart = cm
            .incus_move(id, &r.to, &address, &token_inc, live && running, timeout)
            .await
            .map_err(|e| format!("{}: {e}", cfg.name))?;
        // The instance now lives on the target (Incus removed it here).
        let mut last = String::new();
        for _ in 0..3 {
            match ctx
                .post::<_, Received>(
                    node,
                    "/api/ha/peer/ct/adopt",
                    &AdoptReq {
                        config: cfg.clone(),
                        start: restart,
                    },
                )
                .await
            {
                Ok(_) => return Ok(()),
                Err(e) => last = e,
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        return Err(format!(
            "{} was moved to {} but could not be started or registered there: {last}",
            cfg.name, r.to
        ));
    }

    // 2b. LXC / Docker / Podman: checkpoint or stop, copy, restore or start.
    let stage = cm.move_stage_dir(id);
    let manifest = match cm.move_export(id, &stage, live && running, timeout).await {
        Ok(m) => m,
        Err(e) if live && running => {
            // The checkpoint failed (CRIU could not dump it); the container
            // still runs here. Fall back to stop -> copy -> start.
            tracing::warn!("live move of {id}: checkpoint failed ({e}); moving it cold");
            cm.move_export(id, &stage, false, timeout)
                .await
                .map_err(|e2| format!("cannot prepare {}: {e2} (checkpoint: {e})", cfg.name))?
        }
        Err(e) => {
            let _ = tokio::fs::remove_dir_all(&stage).await;
            return Err(format!("cannot prepare {}: {e}", cfg.name));
        }
    };
    if !manifest.bind_mounts.is_empty() {
        tracing::warn!(
            "container {id}: host bind mounts are not copied: {}",
            manifest.bind_mounts.join(", ")
        );
    }
    let url = format!("{base}/api/ha/peer/ct/receive/{id}");
    let sent = upload_stage(&stage, &url, &token, timeout + Duration::from_secs(600)).await;
    match sent {
        Ok(()) => {
            // Running on the target: ours is the stopped leftover.
            if let Err(e) = cm.move_finish_source(id, &stage).await {
                tracing::warn!("container {id}: source copy not removed after the move: {e}");
            }
            Ok(())
        }
        Err(e) => {
            // The target may or may not have started it: have it remove its
            // copy, and only when it confirms take the guest back here.
            let mut confirmed = false;
            for _ in 0..3 {
                if ctx
                    .post::<_, Received>(
                        node,
                        &format!("/api/ha/peer/ct/abort/{id}"),
                        &serde_json::json!({}),
                    )
                    .await
                    .is_ok()
                {
                    confirmed = true;
                    break;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            if confirmed {
                let back = cm.move_rollback(id, &stage, &manifest, timeout).await;
                let _ = tokio::fs::remove_dir_all(&stage).await;
                match back {
                    Ok(()) => Err(format!("{e} (the container runs on this node again)")),
                    Err(e2) => Err(format!("{e}; and it could not be started here again: {e2}")),
                }
            } else {
                Err(format!(
                    "{e}; {} could not be asked whether it holds a copy, so the container \
                     stays stopped here (state kept in {}) rather than risk two running copies",
                    r.to,
                    stage.display()
                ))
            }
        }
    }
}
