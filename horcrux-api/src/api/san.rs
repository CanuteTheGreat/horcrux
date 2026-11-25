//! SAN storage for the cluster: iSCSI targets logged in on every path,
//! dm-multipath, and shared LVM (lvmlockd + sanlock) on the LUNs.
//!
//! * `GET/PUT  /api/san/settings`            multipath + reconcile settings (cluster-wide)
//! * `GET/POST /api/san/targets`, `GET/PUT/DELETE /api/san/targets/:id`
//! * `POST     /api/san/discover`            SendTargets on portals, every advertised portal
//! * `GET      /api/san/status`              this node: tools, services, sanlock host id
//! * `GET      /api/san/paths`               this node: LUN -> map -> paths (address, state)
//! * `GET      /api/san/nodes/:node/paths`   the same, asked of another member
//! * `POST     /api/san/apply`               reconcile this node now, returns the report
//! * `GET/POST /api/san/lvm`, `GET/DELETE /api/san/lvm/:id`  shared VGs used as storage
//!
//! Cluster: every row (settings, targets, shared VGs, sanlock host ids) is
//! replicated to all members (pushed on change, pulled every reconcile
//! interval, newest `rev` wins, deletes are tombstones), and each node applies
//! it itself: logins on every path, the multipath drop-in, lockstart of the
//! shared VGs and the matching local storage pool. A node that joins later
//! pulls everything on its first tick. CHAP secrets travel only in peer
//! sync bodies (authenticated peer channel) and are stored per node in
//! `<data_dir>/secrets/san/<target>.chap` (0600), never in the rows or in
//! API answers.
use crate::db::config_store::ConfigStore;
use crate::error::ApiError;
use crate::middleware::auth::{AuthUser, CLUSTER_HA_ROLE};
use crate::storage::lvm::LvmPoolOptions;
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Extension, Json, Router,
};
use horcrux_storage::san::{
    self, iscsi, kinds, lvm_shared, multipath, paths, tools, HostId, IscsiTargetSpec,
    MultipathSettings, SharedLvmSpec, TargetWrite, SETTINGS_ID,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::Duration;

/// Deadline for one host tool call (iscsiadm, multipathd, lvm).
const TOOL_TIMEOUT: Duration = Duration::from_secs(60);

pub fn routes(state: &Arc<AppState>) -> Router<Arc<AppState>> {
    spawn_reconciler(state.clone());
    Router::new()
        .route("/api/san/settings", get(get_settings).put(put_settings))
        .route("/api/san/targets", get(list_targets).post(create_target))
        .route(
            "/api/san/targets/:id",
            get(get_target).put(update_target).delete(delete_target),
        )
        .route("/api/san/discover", post(discover))
        .route("/api/san/status", get(status))
        .route("/api/san/paths", get(local_paths))
        .route("/api/san/nodes/:node/paths", get(node_paths))
        .route("/api/san/apply", post(apply_now))
        .route("/api/san/lvm", get(list_lvm).post(create_lvm))
        .route("/api/san/lvm/:id", get(get_lvm).delete(delete_lvm))
        .route("/api/ha/peer/san/sync", post(peer_sync))
        .route("/api/ha/peer/san/list", post(peer_list))
        .route("/api/ha/peer/san/paths", post(peer_paths))
}

// ---------------------------------------------------------------- store

fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis() as u64
}

async fn local_name(state: &AppState) -> String {
    state
        .cluster_manager
        .get_local_node_name()
        .await
        .unwrap_or_else(|_| "localhost".into())
}

async fn rows<T: serde::de::DeserializeOwned>(state: &AppState, kind: &str) -> Vec<T> {
    ConfigStore::new(&state.database)
        .load_values::<T>(kind)
        .await
        .unwrap_or_default()
}

async fn put_row<T: Serialize>(
    state: &AppState,
    kind: &str,
    id: &str,
    v: &T,
) -> Result<(), ApiError> {
    ConfigStore::new(&state.database)
        .put(kind, id, v)
        .await
        .map_err(ApiError::from)
}

async fn settings(state: &AppState) -> MultipathSettings {
    rows::<MultipathSettings>(state, kinds::SETTINGS)
        .await
        .into_iter()
        .next()
        .unwrap_or_default()
}

fn secrets_dir(state: &AppState) -> PathBuf {
    state.config.paths.data_dir.join("secrets").join("san")
}

fn secret_path(state: &AppState, id: &str) -> PathBuf {
    secrets_dir(state).join(format!("{id}.chap"))
}

fn read_secret(state: &AppState, id: &str) -> Option<String> {
    std::fs::read_to_string(secret_path(state, id))
        .ok()
        .map(|s| s.trim_end_matches('\n').to_string())
        .filter(|s| !s.is_empty())
}

fn write_secret(state: &AppState, id: &str, secret: Option<&str>) -> Result<(), ApiError> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let p = secret_path(state, id);
    match secret {
        None | Some("") => {
            let _ = std::fs::remove_file(&p);
            Ok(())
        }
        Some(s) => {
            let dir = secrets_dir(state);
            std::fs::create_dir_all(&dir)?;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
            let tmp = p.with_extension("tmp");
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(s.as_bytes())?;
            f.sync_all()?;
            std::fs::rename(&tmp, &p)?;
            Ok(())
        }
    }
}

/// A target as the API shows it: never the secret, only whether one is set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetView {
    #[serde(flatten)]
    pub target: IscsiTargetSpec,
    pub chap_secret_set: bool,
}

fn view(state: &AppState, t: IscsiTargetSpec) -> TargetView {
    TargetView {
        chap_secret_set: read_secret(state, &t.id).is_some(),
        target: t,
    }
}

/// A target with its secret, for peer sync only.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TargetWire {
    target: IscsiTargetSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chap_secret: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PeerList {
    #[serde(default)]
    targets: Vec<TargetWire>,
    #[serde(default)]
    settings: Option<MultipathSettings>,
    #[serde(default)]
    lvm: Vec<SharedLvmSpec>,
    #[serde(default)]
    host_ids: Vec<HostId>,
}

async fn everything(state: &AppState) -> PeerList {
    let targets = rows::<IscsiTargetSpec>(state, kinds::TARGET)
        .await
        .into_iter()
        .map(|t| TargetWire {
            chap_secret: if t.deleted {
                None
            } else {
                read_secret(state, &t.id)
            },
            target: t,
        })
        .collect();
    let s = rows::<MultipathSettings>(state, kinds::SETTINGS).await;
    PeerList {
        targets,
        settings: s.into_iter().next(),
        lvm: rows(state, kinds::SHARED_LVM).await,
        host_ids: rows(state, kinds::HOST_ID).await,
    }
}

/// Store what is newer than ours; true if anything changed.
async fn merge_in(state: &AppState, inc: PeerList) -> bool {
    let mut changed = false;
    let have: HashMap<String, u64> = rows::<IscsiTargetSpec>(state, kinds::TARGET)
        .await
        .into_iter()
        .map(|t| (t.id, t.rev))
        .collect();
    for w in inc.targets {
        let t = w.target;
        if have.get(&t.id).is_none_or(|r| t.rev > *r)
            && put_row(state, kinds::TARGET, &t.id, &t).await.is_ok()
        {
            let _ = write_secret(
                state,
                &t.id,
                if t.deleted {
                    None
                } else {
                    w.chap_secret.as_deref()
                },
            );
            changed = true;
        }
    }
    if let Some(s) = inc.settings {
        let mine = rows::<MultipathSettings>(state, kinds::SETTINGS).await;
        if mine.first().is_none_or(|m| s.rev > m.rev)
            && s.validate().is_ok()
            && put_row(state, kinds::SETTINGS, SETTINGS_ID, &s)
                .await
                .is_ok()
        {
            changed = true;
        }
    }
    let have: HashMap<String, u64> = rows::<SharedLvmSpec>(state, kinds::SHARED_LVM)
        .await
        .into_iter()
        .map(|l| (l.id, l.rev))
        .collect();
    for l in inc.lvm {
        if have.get(&l.id).is_none_or(|r| l.rev > *r)
            && put_row(state, kinds::SHARED_LVM, &l.id, &l).await.is_ok()
        {
            changed = true;
        }
    }
    let have: HashMap<String, u64> = rows::<HostId>(state, kinds::HOST_ID)
        .await
        .into_iter()
        .map(|h| (h.node, h.rev))
        .collect();
    for h in inc.host_ids {
        if have.get(&h.node).is_none_or(|r| h.rev > *r)
            && put_row(state, kinds::HOST_ID, &h.node, &h).await.is_ok()
        {
            changed = true;
        }
    }
    changed
}

// --------------------------------------------------------------- cluster

#[cfg(feature = "cluster")]
async fn call<B: Serialize, T: serde::de::DeserializeOwned>(
    state: &Arc<AppState>,
    node: &crate::cluster::node::Node,
    path: &str,
    body: &B,
) -> Result<T, String> {
    super::drain::call_peer(state, node, path, body).await
}

#[cfg(not(feature = "cluster"))]
async fn call<B: Serialize, T: serde::de::DeserializeOwned>(
    _state: &Arc<AppState>,
    _node: &crate::cluster::node::Node,
    _path: &str,
    _body: &B,
) -> Result<T, String> {
    Err("clustering is not compiled into this build".into())
}

fn peers(nodes: Vec<crate::cluster::node::Node>, local: &str) -> Vec<crate::cluster::node::Node> {
    nodes
        .into_iter()
        .filter(|n| {
            n.name != local && !n.is_local && n.status == crate::cluster::node::NodeStatus::Online
        })
        .collect()
}

async fn push_to_peers(state: &Arc<AppState>, body: PeerList) {
    let local = local_name(state).await;
    for node in peers(state.cluster_manager.list_nodes().await, &local) {
        if let Err(e) =
            call::<_, serde_json::Value>(state, &node, "/api/ha/peer/san/sync", &body).await
        {
            tracing::warn!(
                "SAN config not pushed to {}: {e} (it pulls it later)",
                node.name
            );
        }
    }
}

async fn pull_from_peers(state: &Arc<AppState>) {
    let local = local_name(state).await;
    for node in peers(state.cluster_manager.list_nodes().await, &local) {
        if let Ok(list) = call::<_, PeerList>(
            state,
            &node,
            "/api/ha/peer/san/list",
            &serde_json::json!({}),
        )
        .await
        {
            merge_in(state, list).await;
        }
    }
}

fn require_peer(user: &Option<Extension<AuthUser>>) -> Result<(), ApiError> {
    match user {
        Some(Extension(u)) if u.role == CLUSTER_HA_ROLE => Ok(()),
        _ => Err(ApiError::Forbidden(
            "this route is only for other cluster nodes".into(),
        )),
    }
}

async fn peer_sync(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Json(req): Json<PeerList>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_peer(&user)?;
    if merge_in(&state, req).await {
        let s = state.clone();
        tokio::spawn(async move {
            let _ = reconcile(&s).await;
        });
    }
    Ok(Json(serde_json::json!({"ok": true})))
}

async fn peer_list(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
) -> Result<Json<PeerList>, ApiError> {
    require_peer(&user)?;
    Ok(Json(everything(&state).await))
}

async fn peer_paths(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
) -> Result<Json<NodePaths>, ApiError> {
    require_peer(&user)?;
    Ok(Json(collect_paths(&state).await))
}

// ------------------------------------------------------------- reconcile

/// What one reconcile pass did on this node.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApplyReport {
    pub node: String,
    pub actions: Vec<String>,
    pub errors: Vec<String>,
    /// Reason codes of missing facilities (as in a 424 answer).
    pub missing: Vec<String>,
}

fn reconcile_lock() -> &'static tokio::sync::Mutex<()> {
    static L: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    L.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// WWIDs already handed to multipathd by this process.
fn known_wwids() -> &'static StdMutex<HashSet<String>> {
    static W: OnceLock<StdMutex<HashSet<String>>> = OnceLock::new();
    W.get_or_init(Default::default)
}

/// The last report, for `GET /api/san/status`.
fn last_report() -> &'static StdMutex<Option<ApplyReport>> {
    static R: OnceLock<StdMutex<Option<ApplyReport>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

fn missing(rep: &mut ApplyReport, e: horcrux_common::Error) {
    match e {
        horcrux_common::Error::NodeLacks {
            reason, message, ..
        } => {
            rep.missing.push(reason.to_string());
            rep.errors.push(message);
        }
        other => rep.errors.push(other.to_string()),
    }
}

/// Start (and enable at boot) a host service through the node's init
/// system; a container/chroot without one is left alone.
async fn ensure_service(name: &str, rep: &mut ApplyReport) {
    let Some(b) = crate::init::backend() else {
        return;
    };
    if !b.exists(name) {
        rep.errors
            .push(format!("service {name} is not installed on this node"));
        return;
    }
    if !matches!(b.is_enabled(name).await, Ok(true)) {
        match b.enable(name).await {
            Ok(()) => rep.actions.push(format!("enabled {name} at boot")),
            Err(e) => rep.errors.push(format!("enable {name}: {e}")),
        }
    }
    if !matches!(b.state(name).await, Ok(crate::init::ServiceState::Active)) {
        match b.start(name).await {
            Ok(()) => rep.actions.push(format!("started {name}")),
            Err(e) => rep.errors.push(format!("start {name}: {e}")),
        }
    }
}

async fn restart_service(name: &str, rep: &mut ApplyReport) {
    if let Some(b) = crate::init::backend() {
        if b.exists(name) && matches!(b.state(name).await, Ok(crate::init::ServiceState::Active)) {
            match b.restart(name).await {
                Ok(()) => rep.actions.push(format!("restarted {name}")),
                Err(e) => rep.errors.push(format!("restart {name}: {e}")),
            }
        }
    }
}

/// Apply the replicated SAN config on this node.
pub async fn reconcile(state: &Arc<AppState>) -> ApplyReport {
    let _g = reconcile_lock().lock().await;
    let local = local_name(state).await;
    let mut rep = ApplyReport {
        node: local.clone(),
        ..Default::default()
    };
    let cfg = settings(state).await;
    let all_targets = rows::<IscsiTargetSpec>(state, kinds::TARGET).await;
    let mine: Vec<&IscsiTargetSpec> = all_targets
        .iter()
        .filter(|t| t.applies_to(&local))
        .collect();
    let lvms: Vec<SharedLvmSpec> = rows::<SharedLvmSpec>(state, kinds::SHARED_LVM)
        .await
        .into_iter()
        .filter(|l| l.applies_to(&local))
        .collect();
    let tools = tools::ToolStatus::probe();

    // Targets that were deleted or moved off this node: log out.
    let gone: Vec<&IscsiTargetSpec> = all_targets
        .iter()
        .filter(|t| !t.applies_to(&local))
        .collect();
    if !gone.is_empty() && tools.iscsiadm {
        if let Ok(sessions) = iscsi::sessions(TOOL_TIMEOUT).await {
            for t in gone {
                if mine.iter().any(|m| m.iqn == t.iqn) {
                    continue;
                }
                for s in sessions.iter().filter(|s| s.iqn == t.iqn) {
                    match iscsi::remove_path(&t.iqn, &s.portal, TOOL_TIMEOUT).await {
                        Ok(()) => rep
                            .actions
                            .push(format!("logged out of {} on {}", t.iqn, s.portal)),
                        Err(e) => rep.errors.push(e),
                    }
                }
            }
        }
    }

    if !mine.is_empty() {
        if let Err(e) = tools::Need::Iscsi.check(&tools) {
            missing(&mut rep, e);
        } else {
            ensure_service("iscsid", &mut rep).await;
            // Multipath first, so the LUNs that appear on login get mapped.
            if cfg.enabled {
                apply_multipath(&cfg, &tools, &mut rep).await;
            }
            for t in &mine {
                login_target(state, t, &mut rep).await;
            }
            if cfg.enabled && tools.multipathd {
                register_wwids(&mine, &mut rep).await;
            }
        }
    } else if !cfg.enabled {
        if let Ok(true) = multipath::remove_dropin(std::path::Path::new(&cfg.config_path)) {
            rep.actions.push(format!("removed {}", cfg.config_path));
            let _ = multipath::reconfigure(TOOL_TIMEOUT).await;
        }
    }

    // Shared VGs deleted or moved off this node: drop the pool, leave the
    // lockspace (the VG and its data stay on the LUN).
    for l in rows::<SharedLvmSpec>(state, kinds::SHARED_LVM).await {
        if l.applies_to(&local) {
            continue;
        }
        let pools = state.storage_manager.list_pools().await;
        if pools.iter().any(|p| p.name == l.id && p.path == l.vg_name) {
            remove_pool_for(state, &l).await;
            let _ = lvm_shared::lockstop(&l.vg_name, TOOL_TIMEOUT).await;
            rep.actions.push(format!("removed storage pool {}", l.id));
        }
    }

    if !lvms.is_empty() {
        apply_shared_lvm(state, &local, &cfg, &lvms, &tools, &mut rep).await;
    }
    for e in &rep.errors {
        tracing::warn!("SAN on {local}: {e}");
    }
    *last_report().lock().unwrap() = Some(rep.clone());
    rep
}

async fn apply_multipath(
    cfg: &MultipathSettings,
    tools: &tools::ToolStatus,
    rep: &mut ApplyReport,
) {
    if let Err(e) = tools::Need::Multipath.check(tools) {
        missing(rep, e);
        return;
    }
    let path = std::path::Path::new(&cfg.config_path);
    match multipath::write_dropin(path, &multipath::render(cfg)) {
        Ok(multipath::WriteOutcome::Written) => {
            rep.actions.push(format!("wrote {}", cfg.config_path));
            ensure_service("multipathd", rep).await;
            if let Err(e) = multipath::reconfigure(TOOL_TIMEOUT).await {
                rep.errors.push(e);
            }
        }
        Ok(multipath::WriteOutcome::Unchanged) => ensure_service("multipathd", rep).await,
        Err(e) => rep.errors.push(e),
    }
}

async fn login_target(state: &AppState, t: &IscsiTargetSpec, rep: &mut ApplyReport) {
    let (found, errs) = if t.use_advertised_portals {
        iscsi::discover(&t.portals, TOOL_TIMEOUT).await
    } else {
        (Vec::new(), Vec::new())
    };
    rep.errors
        .extend(errs.into_iter().map(|e| format!("{} discovery: {e}", t.id)));
    let want = iscsi::login_portals(t, &found);
    let have: HashSet<String> = iscsi::sessions(TOOL_TIMEOUT)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|s| s.iqn == t.iqn && s.logged_in())
        .map(|s| s.portal)
        .collect();
    let secret = read_secret(state, &t.id);
    for p in want {
        if have.contains(&p) {
            continue;
        }
        match iscsi::ensure_path(t, &p, secret.as_deref(), TOOL_TIMEOUT).await {
            Ok(()) => rep.actions.push(format!("logged in to {} on {p}", t.iqn)),
            Err(e) => rep.errors.push(format!("{}: {e}", t.id)),
        }
    }
}

/// `find_multipaths strict` only maps known WWIDs: register the LUNs of
/// our own targets (and nothing else, so local disks stay untouched).
async fn register_wwids(mine: &[&IscsiTargetSpec], rep: &mut ApplyReport) {
    let iqns: HashSet<&str> = mine.iter().map(|t| t.iqn.as_str()).collect();
    let sessions = iscsi::sessions(TOOL_TIMEOUT).await.unwrap_or_default();
    let mut added = false;
    for s in sessions.iter().filter(|s| iqns.contains(s.iqn.as_str())) {
        for d in &s.disks {
            let Some(w) = multipath::wwid_of(&d.dev, TOOL_TIMEOUT).await else {
                continue;
            };
            if known_wwids().lock().unwrap().contains(&w) {
                continue;
            }
            match multipath::add_wwid(&w, TOOL_TIMEOUT).await {
                Ok(()) => {
                    known_wwids().lock().unwrap().insert(w.clone());
                    rep.actions.push(format!("multipath: added {w}"));
                    added = true;
                }
                Err(e) => rep.errors.push(e),
            }
        }
    }
    if added {
        let _ = multipath::reconfigure(TOOL_TIMEOUT).await;
    }
}

async fn my_host_id(state: &Arc<AppState>, local: &str, rep: &mut ApplyReport) -> u16 {
    let claims = rows::<HostId>(state, kinds::HOST_ID).await;
    let id = san::pick_host_id(local, &claims);
    let current = claims.iter().find(|c| c.node == local);
    if current.map(|c| c.host_id) != Some(id) {
        let row = HostId {
            node: local.to_string(),
            host_id: id,
            rev: now_ms().max(current.map(|c| c.rev + 1).unwrap_or(0)),
        };
        if put_row(state, kinds::HOST_ID, local, &row).await.is_ok() {
            rep.actions.push(format!("sanlock host id {id}"));
            push_to_peers(
                state,
                PeerList {
                    host_ids: vec![row],
                    ..Default::default()
                },
            )
            .await;
        }
    }
    id
}

async fn apply_shared_lvm(
    state: &Arc<AppState>,
    local: &str,
    cfg: &MultipathSettings,
    lvms: &[SharedLvmSpec],
    tools: &tools::ToolStatus,
    rep: &mut ApplyReport,
) {
    if let Err(e) = tools::Need::SharedLvm.check(tools) {
        missing(rep, e);
        return;
    }
    let host_id = my_host_id(state, local, rep).await;
    let lvmlocal = std::path::Path::new(lvm_shared::LVMLOCAL_PATH);
    let mut restart_lvmlockd = false;
    match lvm_shared::write_owned(lvmlocal, &lvm_shared::render_lvmlocal(host_id)) {
        Ok(true) => {
            rep.actions
                .push(format!("wrote {}", lvm_shared::LVMLOCAL_PATH));
            restart_lvmlockd = true;
        }
        Ok(false) => {}
        Err(e) => {
            rep.errors.push(e);
            return;
        }
    }
    let sanlock_conf = std::path::Path::new(lvm_shared::SANLOCK_CONF_PATH);
    let sl = match lvm_shared::render_sanlock_conf(cfg.sanlock_watchdog) {
        Some(c) => lvm_shared::write_owned(sanlock_conf, &c),
        None => lvm_shared::remove_owned(sanlock_conf),
    };
    match sl {
        Ok(true) => {
            rep.actions
                .push(format!("updated {}", lvm_shared::SANLOCK_CONF_PATH));
            restart_service("sanlock", rep).await;
        }
        Ok(false) => {}
        Err(e) => rep.errors.push(e),
    }
    if cfg.sanlock_watchdog {
        ensure_service("wdmd", rep).await;
    }
    ensure_service("sanlock", rep).await;
    if restart_lvmlockd {
        restart_service("lvmlockd", rep).await;
    }
    ensure_service("lvmlockd", rep).await;

    for l in lvms {
        match lvm_shared::vg_lock_type(&l.vg_name, TOOL_TIMEOUT).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                rep.errors.push(format!(
                    "{}: volume group {} is not visible on this node yet (LUN {} missing?)",
                    l.id, l.vg_name, l.device
                ));
                continue;
            }
            Err(e) => {
                rep.errors.push(e);
                continue;
            }
        }
        if let Err(e) = lvm_shared::lockstart(&l.vg_name, TOOL_TIMEOUT).await {
            rep.errors.push(format!("{}: {e}", l.id));
            continue;
        }
        ensure_pool(state, l, rep).await;
    }
}

fn pool_options(l: &SharedLvmSpec) -> LvmPoolOptions {
    LvmPoolOptions {
        shared: true,
        wipe_removed: l.wipe_removed,
        discard_removed: l.discard_removed,
        ..Default::default()
    }
}

/// The local storage pool for a shared VG (same name on every node).
async fn ensure_pool(state: &AppState, l: &SharedLvmSpec, rep: &mut ApplyReport) {
    let pools = state.storage_manager.list_pools().await;
    if let Some(p) = pools.iter().find(|p| p.name == l.id) {
        let want = Some(pool_options(l));
        if p.lvm != want || p.path != l.vg_name {
            if let Err(e) = state.storage_manager.remove_pool(&p.id).await {
                rep.errors.push(format!("{}: {e}", l.id));
                return;
            }
        } else {
            return;
        }
    }
    let pool = crate::storage::StoragePool {
        id: uuid::Uuid::new_v4().to_string(),
        name: l.id.clone(),
        storage_type: crate::storage::StorageType::Lvm,
        path: l.vg_name.clone(),
        available: 0,
        total: 0,
        enabled: true,
        source: None,
        lvm: Some(pool_options(l)),
    };
    match state.storage_manager.add_pool(pool).await {
        Ok(_) => rep
            .actions
            .push(format!("storage pool {} on {}", l.id, l.vg_name)),
        Err(e) => rep.errors.push(format!("{}: {e}", l.id)),
    }
}

async fn remove_pool_for(state: &AppState, l: &SharedLvmSpec) {
    let pools = state.storage_manager.list_pools().await;
    if let Some(p) = pools.iter().find(|p| p.name == l.id && p.path == l.vg_name) {
        let _ = state.storage_manager.remove_pool(&p.id).await;
    }
}

fn spawn_reconciler(state: Arc<AppState>) {
    tokio::spawn(async move {
        // Let the node finish starting (cluster membership, DB) first.
        tokio::time::sleep(Duration::from_secs(5)).await;
        loop {
            pull_from_peers(&state).await;
            let configured = !rows::<IscsiTargetSpec>(&state, kinds::TARGET)
                .await
                .is_empty()
                || !rows::<SharedLvmSpec>(&state, kinds::SHARED_LVM)
                    .await
                    .is_empty();
            if configured {
                let _ = reconcile(&state).await;
            }
            let secs = settings(&state)
                .await
                .reconcile_interval_secs
                .clamp(5, 3600);
            tokio::time::sleep(Duration::from_secs(secs.into())).await;
        }
    });
}

// ------------------------------------------------------------- handlers

async fn get_settings(State(state): State<Arc<AppState>>) -> Json<MultipathSettings> {
    Json(settings(&state).await)
}

async fn put_settings(
    State(state): State<Arc<AppState>>,
    Json(mut s): Json<MultipathSettings>,
) -> Result<Json<MultipathSettings>, ApiError> {
    s.validate().map_err(ApiError::BadRequest)?;
    let old = settings(&state).await;
    s.rev = now_ms().max(old.rev + 1);
    put_row(&state, kinds::SETTINGS, SETTINGS_ID, &s).await?;
    push_to_peers(
        &state,
        PeerList {
            settings: Some(s.clone()),
            ..Default::default()
        },
    )
    .await;
    kick(&state);
    Ok(Json(s))
}

/// Re-apply in the background (callers that want the result use /apply).
fn kick(state: &Arc<AppState>) {
    let s = state.clone();
    tokio::spawn(async move {
        let _ = reconcile(&s).await;
    });
}

async fn live_targets(state: &AppState) -> Vec<IscsiTargetSpec> {
    rows::<IscsiTargetSpec>(state, kinds::TARGET)
        .await
        .into_iter()
        .filter(|t| !t.deleted)
        .collect()
}

async fn list_targets(State(state): State<Arc<AppState>>) -> Json<Vec<TargetView>> {
    Json(
        live_targets(&state)
            .await
            .into_iter()
            .map(|t| view(&state, t))
            .collect(),
    )
}

async fn find_target(state: &AppState, id: &str) -> Result<IscsiTargetSpec, ApiError> {
    live_targets(state)
        .await
        .into_iter()
        .find(|t| t.id == id)
        .ok_or_else(|| ApiError::NotFound(format!("SAN target {id} not found")))
}

async fn get_target(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<TargetView>, ApiError> {
    Ok(Json(view(&state, find_target(&state, &id).await?)))
}

async fn commit_target(
    state: &Arc<AppState>,
    mut t: IscsiTargetSpec,
    secret: Option<String>,
    base_rev: u64,
) -> Result<TargetView, ApiError> {
    t.rev = now_ms().max(base_rev + 1);
    if let Some(s) = &secret {
        if !s.is_empty() {
            san::validate_chap_secret(s).map_err(ApiError::BadRequest)?;
            if t.chap_user.is_none() {
                return Err(ApiError::BadRequest("chap_secret needs chap_user".into()));
            }
        }
    }
    put_row(state, kinds::TARGET, &t.id, &t).await?;
    if secret.is_some() {
        write_secret(state, &t.id, secret.as_deref())?;
    }
    if t.chap_user.is_none() {
        write_secret(state, &t.id, None)?;
    }
    push_to_peers(
        state,
        PeerList {
            targets: vec![TargetWire {
                chap_secret: read_secret(state, &t.id),
                target: t.clone(),
            }],
            ..Default::default()
        },
    )
    .await;
    kick(state);
    Ok(view(state, t))
}

async fn create_target(
    State(state): State<Arc<AppState>>,
    Json(w): Json<TargetWrite>,
) -> Result<(StatusCode, Json<TargetView>), ApiError> {
    let mut t = w.target;
    t.validate().map_err(ApiError::BadRequest)?;
    t.deleted = false;
    let live = live_targets(&state).await;
    if live.iter().any(|o| o.id == t.id) {
        return Err(ApiError::Conflict(format!(
            "SAN target {} already exists",
            t.id
        )));
    }
    if let Some(o) = live.iter().find(|o| o.iqn == t.iqn) {
        return Err(ApiError::Conflict(format!(
            "target {} is already configured as {}",
            t.iqn, o.id
        )));
    }
    // A tombstone of the same id is overwritten by the newer rev.
    let base = rows::<IscsiTargetSpec>(&state, kinds::TARGET)
        .await
        .into_iter()
        .find(|o| o.id == t.id)
        .map(|o| o.rev)
        .unwrap_or(0);
    let v = commit_target(&state, t, w.chap_secret, base).await?;
    Ok((StatusCode::CREATED, Json(v)))
}

async fn update_target(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(w): Json<TargetWrite>,
) -> Result<Json<TargetView>, ApiError> {
    let old = find_target(&state, &id).await?;
    let mut t = w.target;
    t.id = id;
    t.validate().map_err(ApiError::BadRequest)?;
    t.deleted = false;
    if live_targets(&state)
        .await
        .iter()
        .any(|o| o.id != t.id && o.iqn == t.iqn)
    {
        return Err(ApiError::Conflict(format!(
            "target {} is configured twice",
            t.iqn
        )));
    }
    Ok(Json(
        commit_target(&state, t, w.chap_secret, old.rev).await?,
    ))
}

async fn delete_target(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let mut t = find_target(&state, &id).await?;
    t.deleted = true;
    t.rev = now_ms().max(t.rev + 1);
    put_row(&state, kinds::TARGET, &t.id, &t).await?;
    write_secret(&state, &t.id, None)?;
    push_to_peers(
        &state,
        PeerList {
            targets: vec![TargetWire {
                target: t,
                chap_secret: None,
            }],
            ..Default::default()
        },
    )
    .await;
    kick(&state);
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct DiscoverReq {
    portals: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DiscoverAnswer {
    pub targets: Vec<iscsi::DiscoveredTarget>,
    /// Portals that did not answer (the others still count).
    pub errors: Vec<String>,
}

async fn discover(Json(req): Json<DiscoverReq>) -> Result<Json<DiscoverAnswer>, ApiError> {
    if req.portals.is_empty() {
        return Err(ApiError::BadRequest("portals: at least one".into()));
    }
    let mut ps = Vec::new();
    for p in &req.portals {
        ps.push(
            iscsi::Portal::parse(p)
                .map_err(ApiError::BadRequest)?
                .to_string(),
        );
    }
    tools::Need::Iscsi
        .check(&tools::ToolStatus::probe())
        .map_err(ApiError::from)?;
    let (targets, errors) = iscsi::discover(&ps, TOOL_TIMEOUT).await;
    if targets.is_empty() && !errors.is_empty() {
        return Err(ApiError::BadRequest(format!(
            "no portal answered: {}",
            errors.join("; ")
        )));
    }
    Ok(Json(DiscoverAnswer { targets, errors }))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StatusView {
    pub node: String,
    pub tools: tools::ToolStatus,
    pub host_id: Option<u16>,
    pub multipath_config: String,
    pub last_apply: Option<ApplyReport>,
}

async fn status(State(state): State<Arc<AppState>>) -> Json<StatusView> {
    let node = local_name(&state).await;
    let host_id = rows::<HostId>(&state, kinds::HOST_ID)
        .await
        .into_iter()
        .find(|h| h.node == node)
        .map(|h| h.host_id);
    Json(StatusView {
        node,
        tools: tools::ToolStatus::probe(),
        host_id,
        multipath_config: settings(&state).await.config_path,
        last_apply: last_report().lock().unwrap().clone(),
    })
}

/// One node's SAN paths.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NodePaths {
    pub node: String,
    pub multipathd_running: bool,
    pub luns: Vec<paths::LunView>,
    pub errors: Vec<String>,
}

async fn collect_paths(state: &AppState) -> NodePaths {
    let node = local_name(state).await;
    let t = tools::ToolStatus::probe();
    let mut out = NodePaths {
        node,
        ..Default::default()
    };
    if !t.iscsiadm && !t.multipathd {
        return out;
    }
    let sessions = if t.iscsiadm {
        match iscsi::sessions(TOOL_TIMEOUT).await {
            Ok(s) => s,
            Err(e) => {
                out.errors.push(e);
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    let maps = if t.multipathd {
        match multipath::maps(TOOL_TIMEOUT).await {
            Ok(m) => {
                out.multipathd_running = true;
                m
            }
            Err(e) => {
                out.errors.push(e);
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    // Only SAN maps: those with a path from an iSCSI session (or, later,
    // NVMe-oF/FC); local multipathed disks are not ours to show.
    let session_devs: HashSet<&str> = sessions
        .iter()
        .flat_map(|s| s.disks.iter().map(|d| d.dev.as_str()))
        .collect();
    let maps: Vec<multipath::Map> = maps
        .into_iter()
        .filter(|m| {
            m.paths
                .iter()
                .any(|p| session_devs.contains(p.dev.as_str()))
        })
        .collect();
    let mut wwids = HashMap::new();
    let mapped: HashSet<&str> = maps
        .iter()
        .flat_map(|m| m.paths.iter().map(|p| p.dev.as_str()))
        .collect();
    for d in session_devs.iter().filter(|d| !mapped.contains(*d)) {
        if let Some(w) = multipath::wwid_of(d, TOOL_TIMEOUT).await {
            wwids.insert(d.to_string(), w);
        }
    }
    out.luns = paths::assemble(&sessions, &maps, &wwids, paths::sysfs_size);
    out
}

async fn local_paths(State(state): State<Arc<AppState>>) -> Json<NodePaths> {
    Json(collect_paths(&state).await)
}

async fn node_paths(
    State(state): State<Arc<AppState>>,
    Path(node): Path<String>,
) -> Result<Json<NodePaths>, ApiError> {
    let local = local_name(&state).await;
    if node == local {
        return Ok(Json(collect_paths(&state).await));
    }
    let n = state
        .cluster_manager
        .list_nodes()
        .await
        .into_iter()
        .find(|c| c.name == node)
        .ok_or_else(|| ApiError::NotFound(format!("node {node} is not a cluster member")))?;
    let p = call::<_, NodePaths>(&state, &n, "/api/ha/peer/san/paths", &serde_json::json!({}))
        .await
        .map_err(|e| ApiError::Internal(format!("node {node} did not answer: {e}")))?;
    Ok(Json(p))
}

async fn apply_now(State(state): State<Arc<AppState>>) -> Json<ApplyReport> {
    pull_from_peers(&state).await;
    Json(reconcile(&state).await)
}

async fn live_lvm(state: &AppState) -> Vec<SharedLvmSpec> {
    rows::<SharedLvmSpec>(state, kinds::SHARED_LVM)
        .await
        .into_iter()
        .filter(|l| !l.deleted)
        .collect()
}

async fn list_lvm(State(state): State<Arc<AppState>>) -> Json<Vec<SharedLvmSpec>> {
    Json(live_lvm(&state).await)
}

async fn get_lvm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<SharedLvmSpec>, ApiError> {
    live_lvm(&state)
        .await
        .into_iter()
        .find(|l| l.id == id)
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("shared LVM storage {id} not found")))
}

#[derive(Debug, Deserialize)]
struct CreateLvm {
    #[serde(flatten)]
    spec: SharedLvmSpec,
    /// Create the VG on the LUN now (from this node). False = the VG
    /// already exists (made by another tool or an earlier install).
    #[serde(default)]
    create_vg: bool,
}

async fn create_lvm(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateLvm>,
) -> Result<(StatusCode, Json<ApplyReport>), ApiError> {
    let mut l = req.spec;
    l.validate().map_err(ApiError::BadRequest)?;
    l.deleted = false;
    let live = live_lvm(&state).await;
    if live.iter().any(|o| o.id == l.id || o.vg_name == l.vg_name) {
        return Err(ApiError::Conflict(format!(
            "shared LVM storage {} / VG {} exists",
            l.id, l.vg_name
        )));
    }
    if state
        .storage_manager
        .list_pools()
        .await
        .iter()
        .any(|p| p.name == l.id)
    {
        return Err(ApiError::Conflict(format!(
            "a storage pool named {} exists",
            l.id
        )));
    }
    let t = tools::ToolStatus::probe();
    tools::Need::SharedLvm.check(&t).map_err(ApiError::from)?;
    if l.wipe_removed || l.discard_removed {
        tools::Need::Discard.check(&t).map_err(ApiError::from)?;
    }
    let dev = l.device_path().map_err(ApiError::BadRequest)?;
    let base = rows::<SharedLvmSpec>(&state, kinds::SHARED_LVM)
        .await
        .into_iter()
        .find(|o| o.id == l.id)
        .map(|o| o.rev)
        .unwrap_or(0);
    l.rev = now_ms().max(base + 1);
    // Store first: the reconcile below writes lvmlocal.conf and starts
    // lvmlockd/sanlock, which vgcreate --shared needs.
    put_row(&state, kinds::SHARED_LVM, &l.id, &l).await?;
    let mut rep = reconcile(&state).await;
    if req.create_vg {
        match lvm_shared::vg_lock_type(&l.vg_name, TOOL_TIMEOUT).await {
            Ok(Some(_)) => rep.errors.push(format!(
                "volume group {} exists already; not created",
                l.vg_name
            )),
            _ => {
                if !std::path::Path::new(&dev).exists() {
                    rep.errors
                        .push(format!("{dev} does not exist on this node"));
                } else {
                    match lvm_shared::create_shared_vg(&l.vg_name, &dev, TOOL_TIMEOUT).await {
                        Ok(()) => {
                            rep.actions
                                .push(format!("created shared VG {} on {dev}", l.vg_name));
                            rep = merge_reports(rep, reconcile(&state).await);
                        }
                        Err(e) => rep.errors.push(e),
                    }
                }
            }
        }
    }
    push_to_peers(
        &state,
        PeerList {
            lvm: vec![l.clone()],
            ..Default::default()
        },
    )
    .await;
    Ok((StatusCode::CREATED, Json(rep)))
}

fn merge_reports(mut a: ApplyReport, b: ApplyReport) -> ApplyReport {
    a.actions.extend(b.actions);
    a.errors = b.errors;
    a.missing = b.missing;
    a
}

async fn delete_lvm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let mut l = live_lvm(&state)
        .await
        .into_iter()
        .find(|l| l.id == id)
        .ok_or_else(|| ApiError::NotFound(format!("shared LVM storage {id} not found")))?;
    l.deleted = true;
    l.rev = now_ms().max(l.rev + 1);
    put_row(&state, kinds::SHARED_LVM, &l.id, &l).await?;
    // The pool goes away on every node; the VG and its data stay on the LUN.
    remove_pool_for(&state, &l).await;
    let _ = lvm_shared::lockstop(&l.vg_name, TOOL_TIMEOUT).await;
    push_to_peers(
        &state,
        PeerList {
            lvm: vec![l],
            ..Default::default()
        },
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_view_hides_the_secret() {
        let w: TargetWrite = serde_json::from_value(serde_json::json!({
            "id": "a", "iqn": "iqn.2003-01.org.x:t", "portals": ["10.0.0.1"],
            "chap_user": "u", "chap_secret": "0123456789abcdef"
        }))
        .unwrap();
        assert_eq!(w.chap_secret.as_deref(), Some("0123456789abcdef"));
        let v = TargetView {
            target: w.target,
            chap_secret_set: true,
        };
        let j = serde_json::to_string(&v).unwrap();
        assert!(!j.contains("0123456789abcdef"));
        assert!(j.contains("\"chap_secret_set\":true"));
        assert!(j.contains("\"chap_user\":\"u\""));
    }

    #[test]
    fn peer_list_defaults() {
        let p: PeerList = serde_json::from_str("{}").unwrap();
        assert!(
            p.targets.is_empty()
                && p.settings.is_none()
                && p.lvm.is_empty()
                && p.host_ids.is_empty()
        );
    }
}
