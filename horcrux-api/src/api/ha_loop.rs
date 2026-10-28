//! HA supervisor: failure detection, fencing and recovery (USE=cluster).
//!
//! Every node runs one supervisor tick loop (`spawn`). Each tick it
//!
//! 1. **publishes** the HA resources placed on this node (with the VM
//!    definition and HA group) to every online peer
//!    (`POST /api/ha/peer/sync`), the pmxcfs-lite that lets any node
//!    restart a VM whose owner died, and adopts newer placements the peers
//!    report back;
//! 2. **drops stale copies**: a VM that runs here although the cluster has
//!    placed it elsewhere (this node was replaced while it was away) is
//!    killed, so a returning node can never run a second copy;
//! 3. **self-fences**: in a cluster of 3+ nodes, a node that cannot see a
//!    majority for `HORCRUX_HA_SELF_FENCE_SECS` kills the QEMU of its HA
//!    VMs (the software watchdog; the survivors wait longer than this
//!    before recovering them);
//! 4. **restarts** HA VMs that should run here but do not (guest crash,
//!    QEMU killed), within the resource's restart budget;
//! 5. as the **leader** (lowest-named online node), recovers the resources
//!    of nodes that are offline: if the node has fencing devices it is
//!    fenced first (IPMI/iLO/DRAC/SSH/libvirt/SNMP PDU); without devices the
//!    leader needs a majority and waits out the self-fence time. Each
//!    resource is restarted on the best online node of its HA group
//!    (`POST /api/ha/peer/recover/:vm`), whose disks must be visible there
//!    (shared storage).
//!
//! Service-to-service calls carry a short-lived `cluster-ha` token that is
//! valid on `/api/ha/peer/` only.
use crate::cluster::balancer::NodeResources;
use crate::cluster::node::NodeStatus;
use crate::db::config_store::{kinds, ConfigStore};
use crate::error::ApiError;
use crate::ha::fencing::{FencingAgentType, FencingDevice, FencingManager, FencingResult};
use crate::ha::{HaAction, HaGroup, HaResource, HaState};
use crate::middleware::auth::{AuthUser, CLUSTER_HA_ROLE};
use crate::vm::qemu::QemuManager;
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post},
    Extension, Json, Router,
};
use horcrux_common::VmConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

fn env_secs(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v >= 1)
        .unwrap_or(default)
}

fn tick_secs() -> u64 {
    env_secs("HORCRUX_HA_TICK_SECS", 5)
}
/// How long a node without a majority waits before it kills its HA VMs.
fn self_fence_secs() -> i64 {
    env_secs("HORCRUX_HA_SELF_FENCE_SECS", 60) as i64
}
/// Extra time the leader waits, beyond the self-fence time, before it
/// recovers the VMs of an unfenced node (clock/poll skew).
fn recovery_margin_secs() -> i64 {
    env_secs("HORCRUX_HA_RECOVERY_MARGIN_SECS", 15) as i64
}
/// Wait before fencing a node that has gone quiet (it may only be slow).
fn fence_after_secs() -> i64 {
    env_secs("HORCRUX_HA_FENCE_AFTER_SECS", 10) as i64
}

static FENCING: OnceLock<Arc<FencingManager>> = OnceLock::new();

/// The node's fencing manager (devices persisted under
/// `kinds::FENCING_DEVICE`).
pub fn fencing() -> Arc<FencingManager> {
    FENCING
        .get_or_init(|| Arc::new(FencingManager::new()))
        .clone()
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/ha/events", get(list_events))
        .route("/api/ha/enable", post(enable_ha))
        .route("/api/ha/disable", post(disable_ha))
        .route(
            "/api/ha/fencing/devices",
            get(list_devices).post(add_device),
        )
        .route("/api/ha/fencing/devices/:id", delete(remove_device))
        .route("/api/ha/fencing/events", get(fencing_events))
        .route("/api/ha/peer/sync", post(peer_sync))
        .route("/api/ha/peer/members", post(peer_members))
        .route("/api/ha/peer/recover/:vm_id", post(peer_recover))
}

// ---------------------------------------------------------------- wire types

/// What the owner of an HA resource publishes to its peers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HaReplica {
    pub vm: VmConfig,
    pub resource: HaResource,
    pub group: Option<HaGroup>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SyncRequest {
    from: String,
    items: Vec<HaReplica>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SyncReply {
    /// Resources this peer knows with a newer placement than the sender's.
    newer: Vec<HaResource>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RecoverRequest {
    from: String,
    seq: u64,
}

fn require_peer(user: &Option<Extension<AuthUser>>) -> Result<(), ApiError> {
    match user {
        Some(Extension(u)) if u.role == CLUSTER_HA_ROLE => Ok(()),
        _ => Err(ApiError::Forbidden(
            "the HA peer API is only for other cluster nodes".into(),
        )),
    }
}

fn store(state: &AppState) -> ConfigStore {
    ConfigStore::new(&state.database)
}

// ------------------------------------------------------------ peer handlers

async fn peer_sync(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Json(req): Json<SyncRequest>,
) -> Result<Json<SyncReply>, ApiError> {
    require_peer(&user)?;
    let st = store(&state);
    // HA is a cluster-wide mode: the first resource a peer publishes
    // switches it on here too (the supervisor idles while it is off).
    if !req.items.is_empty() && !state.ha_manager.is_enabled().await {
        state.ha_manager.enable().await;
    }
    let mut reply = SyncReply::default();
    let mut keep = Vec::new();
    for item in req.items {
        keep.push(item.resource.vm_id.clone());
        let id = item.resource.vm_id.clone();
        let applied = state
            .ha_manager
            .apply_peer_resource(item.resource, item.group)
            .await;
        if applied {
            st.put(kinds::HA_VM_REPLICA, &id, &item.vm).await?;
        } else if let Some(local) = state.ha_manager.get_resource(&id).await {
            reply.newer.push(local);
        }
    }
    for gone in state.ha_manager.prune_resources_of(&req.from, &keep).await {
        let _ = st.delete(kinds::HA_VM_REPLICA, &gone).await;
    }
    Ok(Json(reply))
}

#[derive(Deserialize)]
struct MemberInfo {
    name: String,
    address: String,
}

/// A member tells us a node joined the cluster.
async fn peer_members(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Json(m): Json<MemberInfo>,
) -> Result<StatusCode, ApiError> {
    require_peer(&user)?;
    state
        .cluster_manager
        .accept_member(&m.name, &m.address)
        .await?;
    Ok(StatusCode::OK)
}

async fn peer_recover(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Path(vm_id): Path<String>,
    Json(req): Json<RecoverRequest>,
) -> Result<StatusCode, ApiError> {
    require_peer(&user)?;
    recover_here(&state, &vm_id, req.seq, &req.from).await?;
    Ok(StatusCode::OK)
}

/// VM ids being recovered on this node right now. Their QEMU process exists
/// before the new placement is recorded, so the stale-copy sweep must not
/// mistake the freshly started guest for a leftover and kill it.
static RECOVERING: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn is_recovering(vm_id: &str) -> bool {
    RECOVERING
        .lock()
        .map(|g| g.iter().any(|v| v == vm_id))
        .unwrap_or(false)
}

/// Marks `vm_id` as recovering until dropped.
struct RecoveryGuard(String);

impl RecoveryGuard {
    fn new(vm_id: &str) -> Self {
        if let Ok(mut g) = RECOVERING.lock() {
            g.push(vm_id.to_string());
        }
        RecoveryGuard(vm_id.to_string())
    }
}

impl Drop for RecoveryGuard {
    fn drop(&mut self) {
        if let Ok(mut g) = RECOVERING.lock() {
            if let Some(i) = g.iter().position(|v| v == &self.0) {
                g.remove(i);
            }
        }
    }
}

/// Bring an HA VM up on this node from the replicated definition.
async fn recover_here(
    state: &Arc<AppState>,
    vm_id: &str,
    seq: u64,
    from: &str,
) -> Result<(), ApiError> {
    let local = state.cluster_manager.get_local_node_name().await?;
    let Some(res) = state.ha_manager.get_resource(vm_id).await else {
        return Err(ApiError::NotFound(format!(
            "VM {vm_id} is not an HA resource here"
        )));
    };
    if res.current_node.as_deref() == Some(local.as_str())
        && res.placement_seq >= seq
        && QemuManager::process_running(vm_id)
    {
        return Ok(()); // already recovered here (repeat call)
    }
    if res.placement_seq >= seq {
        return Err(ApiError::Conflict(format!(
            "VM {vm_id} was already moved (placement {} >= {seq})",
            res.placement_seq
        )));
    }
    let replica: VmConfig = store(state)
        .load::<VmConfig>(kinds::HA_VM_REPLICA)
        .await?
        .into_iter()
        .find(|(id, _)| id == vm_id)
        .map(|(_, v)| v)
        .ok_or_else(|| ApiError::NotFound(format!("no replicated definition of VM {vm_id}")))?;
    // Shared storage is a precondition: the disks must be here.
    for d in &replica.disks {
        if !std::path::Path::new(&d.path).exists() {
            return Err(ApiError::Conflict(format!(
                "disk {} of VM {vm_id} is not visible on {local}: HA recovery needs storage \
                 shared between the nodes",
                d.path
            )));
        }
    }
    // Passthrough devices belong to one host: recovery needs the same
    // device (bound to vfio-pci) here, else say so instead of a half-start.
    for d in &replica.hardware.hostpci {
        crate::vm::mdev::check_device(std::path::Path::new("/sys"), d).map_err(|e| {
            ApiError::Conflict(format!(
                "VM {vm_id} uses passthrough device {} that {local} cannot give it ({e}); HA \
                 recovery of such a VM only works on a node with the same device bound to vfio-pci",
                d.device
            ))
        })?;
    }
    if state.vm_manager.get_vm(vm_id).await.is_err() {
        let mut cfg = replica.clone();
        cfg.status = horcrux_common::VmStatus::Stopped;
        state.vm_manager.import_vm(cfg).await?;
    }
    let _recovering = RecoveryGuard::new(vm_id);
    if !QemuManager::process_running(vm_id) {
        state.vm_manager.start_vm(vm_id).await?;
    }
    state
        .ha_manager
        .set_placement(vm_id, &local, seq, true)
        .await;
    tracing::warn!("HA: recovered VM {vm_id} on {local} (was on {from})");
    notify(
        state,
        "ha_failover",
        serde_json::json!({"vm_id": vm_id, "from": from, "to": local}),
    )
    .await;
    Ok(())
}

async fn notify(state: &Arc<AppState>, event: &str, data: serde_json::Value) {
    let kind = match event {
        "ha_failover" => crate::webhooks::WebhookEventType::HaFailover,
        "node_failed" => crate::webhooks::WebhookEventType::NodeFailed,
        other => crate::webhooks::WebhookEventType::Custom(other.to_string()),
    };
    let _ = state.webhook_manager.trigger_event(kind, data).await;
}

// ------------------------------------------------------------ user handlers

#[derive(Deserialize)]
struct EventsQuery {
    vm_id: Option<String>,
    limit: Option<usize>,
}

async fn list_events(
    State(state): State<Arc<AppState>>,
    Query(q): Query<EventsQuery>,
) -> Json<Vec<crate::ha::HaEvent>> {
    Json(
        state
            .ha_manager
            .get_events(q.vm_id.as_deref(), q.limit)
            .await,
    )
}

async fn enable_ha(State(state): State<Arc<AppState>>) -> StatusCode {
    state.ha_manager.enable().await;
    StatusCode::OK
}

async fn disable_ha(State(state): State<Arc<AppState>>) -> StatusCode {
    state.ha_manager.disable().await;
    StatusCode::OK
}

/// A fencing device as shown to operators (the password stays server-side).
#[derive(Serialize)]
struct DeviceView {
    id: String,
    node: String,
    agent_type: FencingAgentType,
    address: String,
    username: Option<String>,
    has_password: bool,
    port: Option<u16>,
    options: HashMap<String, String>,
    priority: u32,
}

impl From<FencingDevice> for DeviceView {
    fn from(d: FencingDevice) -> Self {
        DeviceView {
            id: d.id,
            node: d.node,
            agent_type: d.agent_type,
            address: d.address,
            username: d.username,
            has_password: d.password.is_some(),
            port: d.port,
            options: d.options,
            priority: d.priority,
        }
    }
}

async fn list_devices() -> Json<Vec<DeviceView>> {
    let mut v: Vec<DeviceView> = fencing()
        .list_devices()
        .await
        .into_iter()
        .map(Into::into)
        .collect();
    v.sort_by(|a, b| a.id.cmp(&b.id));
    Json(v)
}

async fn add_device(
    State(state): State<Arc<AppState>>,
    Json(dev): Json<FencingDevice>,
) -> Result<(StatusCode, Json<DeviceView>), ApiError> {
    if dev.id.is_empty() || dev.node.is_empty() {
        return Err(ApiError::ValidationError(
            "a fencing device needs an id and the node it fences".into(),
        ));
    }
    if dev.address.is_empty() && dev.agent_type != FencingAgentType::Manual {
        return Err(ApiError::ValidationError(
            "a fencing device needs an address".into(),
        ));
    }
    if state
        .cluster_manager
        .list_nodes()
        .await
        .iter()
        .all(|n| n.name != dev.node)
    {
        return Err(ApiError::NotFound(format!(
            "node {} is not in the cluster",
            dev.node
        )));
    }
    fencing().add_device(dev.clone()).await?;
    store(&state)
        .put(kinds::FENCING_DEVICE, &dev.id, &dev)
        .await?;
    Ok((StatusCode::CREATED, Json(dev.into())))
}

async fn remove_device(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    fencing()
        .remove_device(&id)
        .await
        .map_err(|e| ApiError::NotFound(e.to_string()))?;
    store(&state).delete(kinds::FENCING_DEVICE, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn fencing_events() -> Json<Vec<crate::ha::fencing::FencingEvent>> {
    Json(fencing().get_events(None, 200).await)
}

// --------------------------------------------------------------- supervisor

struct LoopState {
    client: reqwest::Client,
    port: u16,
    /// node -> unix time the supervisor first saw it offline
    offline_since: HashMap<String, i64>,
    /// since when this node has not seen a majority
    no_quorum_since: Option<i64>,
    /// node -> last fencing attempt (retry pacing)
    last_fence: HashMap<String, i64>,
}

/// Start the supervisor (once per process). Needs the usage monitor, which
/// supplies the node online/offline view.
pub fn spawn(state: Arc<AppState>) {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    tokio::spawn(async move {
        // Saved fencing devices.
        match store(&state)
            .load_values::<FencingDevice>(kinds::FENCING_DEVICE)
            .await
        {
            Ok(devs) => {
                for d in devs {
                    let _ = fencing().add_device(d).await;
                }
            }
            Err(e) => tracing::error!("HA: could not load fencing devices: {e}"),
        }
        let client = match reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(60))
            .danger_accept_invalid_certs(true)
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("HA supervisor disabled: HTTP client: {e}");
                return;
            }
        };
        let mut ls = LoopState {
            client,
            port: state.config.server.port,
            offline_since: HashMap::new(),
            no_quorum_since: None,
            last_fence: HashMap::new(),
        };
        let mut ticker = tokio::time::interval(Duration::from_secs(tick_secs()));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            if !state.ha_manager.is_enabled().await {
                continue;
            }
            tick(&state, &mut ls).await;
        }
    });
}

async fn peer_post<T: Serialize, R: serde::de::DeserializeOwned>(
    ls: &LoopState,
    local: &str,
    base: &str,
    path: &str,
    body: &T,
) -> Result<R, String> {
    let token = crate::middleware::auth::generate_cluster_ha_token(local)?;
    let resp = ls
        .client
        .post(format!("{base}{path}"))
        .bearer_auth(token)
        .json(body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("{status}: {text}"));
    }
    serde_json::from_str(&text).or_else(|_| serde_json::from_str("null").map_err(|e| e.to_string()))
}

async fn tick(state: &Arc<AppState>, ls: &mut LoopState) {
    let Ok(local) = state.cluster_manager.get_local_node_name().await else {
        return;
    };
    let nodes = state.cluster_manager.list_nodes().await;
    let now = chrono::Utc::now().timestamp();
    let is_local = |n: &crate::cluster::node::Node| n.name == local || n.is_local;
    let online: Vec<String> = nodes
        .iter()
        .filter(|n| is_local(n) || n.status == NodeStatus::Online)
        .map(|n| n.name.clone())
        .collect();
    // The local node may not be registered as a node of its own.
    let total = nodes.len().max(online.len());
    let quorum = online.len() * 2 > total;

    // Nodes in maintenance are online but take no guests: HA never picks
    // them as a recovery or relocation target.
    let maintenance = if online.len() > 1 {
        super::drain::maintenance_nodes(state, &nodes, &local).await
    } else {
        Default::default()
    };
    let targets = crate::cluster::drain::recovery_targets(&online, &maintenance);

    // Feed live node figures to placement.
    let all = state.ha_manager.list_resources().await;
    for n in &nodes {
        if targets.contains(&n.name) {
            state
                .ha_manager
                .update_node_resources(NodeResources {
                    node_name: n.name.clone(),
                    cpu_usage: n.cpu_usage.unwrap_or(0.0) as f32,
                    memory_usage: n
                        .memory_used
                        .filter(|_| n.memory_total > 0)
                        .map(|u| (u as f64 / n.memory_total as f64 * 100.0) as f32)
                        .unwrap_or(0.0),
                    disk_usage: 0.0,
                    network_usage: 0.0,
                    vm_count: all
                        .iter()
                        .filter(|r| r.current_node.as_deref() == Some(n.name.as_str()))
                        .count(),
                    total_cpu_cores: n.cpu_cores as usize,
                    total_memory_gb: (n.memory_total / (1024 * 1024 * 1024)) as usize,
                    total_disk_gb: 1_000_000,
                    architecture: n.architecture.clone(),
                    available_passthrough_devices: Vec::new(),
                    placement_weight: n.priority,
                    cpu_vendor: n.cpu_vendor.clone(),
                    caps: n.caps.clone(),
                })
                .await;
        } else {
            state.ha_manager.remove_node_resources(&n.name).await;
        }
    }

    // 1. publish to peers, adopt newer placements.
    publish(state, ls, &local, &nodes, &online).await;

    // 2. stale copies of VMs the cluster placed elsewhere.
    for r in state.ha_manager.list_resources().await {
        if let Some(node) = r.current_node.as_deref() {
            if node != local && !is_recovering(&r.vm_id) && QemuManager::process_running(&r.vm_id) {
                tracing::warn!(
                    "HA: VM {} runs here but is placed on {node}; stopping the stale copy",
                    r.vm_id
                );
                crate::vm::migrate::kill_qemu(&r.vm_id).await;
                let _ = state.vm_manager.stop_vm(&r.vm_id).await;
            }
        }
    }

    // 3. self-fence without a majority (3+ nodes).
    if total >= 3 && !quorum {
        let since = *ls.no_quorum_since.get_or_insert(now);
        if now - since >= self_fence_secs() {
            for r in state.ha_manager.resources_on(&local).await {
                if QemuManager::process_running(&r.vm_id) {
                    tracing::error!(
                        "HA: no majority for {}s: self-fencing, killing VM {}",
                        now - since,
                        r.vm_id
                    );
                    crate::vm::migrate::kill_qemu(&r.vm_id).await;
                    let _ = state.vm_manager.stop_vm(&r.vm_id).await;
                    state
                        .ha_manager
                        .note_failure(
                            &r.vm_id,
                            &local,
                            "node lost the cluster majority: VM stopped (self-fence)".into(),
                        )
                        .await;
                }
            }
        }
    } else {
        ls.no_quorum_since = None;
        // 4. restart HA VMs that should run here.
        if total < 3 || quorum {
            local_restarts(state, ls, &local, &nodes, &online).await;
        }
    }

    // 5. leader: recover resources of dead nodes.
    for n in &nodes {
        if is_local(n) || n.status == NodeStatus::Online {
            ls.offline_since.remove(&n.name);
        } else if n.status == NodeStatus::Offline && !ls.offline_since.contains_key(&n.name) {
            ls.offline_since.insert(n.name.clone(), now);
            notify(
                state,
                "node_failed",
                serde_json::json!({"node": n.name, "detected_by": local}),
            )
            .await;
        }
    }
    let leader = online.iter().min().cloned();
    if leader.as_deref() != Some(local.as_str()) {
        return;
    }
    let dead: Vec<(String, i64)> = ls
        .offline_since
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    for (node, since) in dead {
        let affected = state.ha_manager.resources_on(&node).await;
        if affected
            .iter()
            .all(|r| !matches!(r.state, HaState::Started | HaState::Migrating))
        {
            continue;
        }
        let offline_for = now - since;
        let devices = fencing().get_node_devices(&node).await;
        if !devices.is_empty() {
            if offline_for < fence_after_secs() {
                continue;
            }
            if now - ls.last_fence.get(&node).copied().unwrap_or(0) < 30 {
                continue;
            }
            ls.last_fence.insert(node.clone(), now);
            match fencing()
                .fence_node(&node, "node stopped answering the cluster heartbeat")
                .await
            {
                FencingResult::Success => {}
                other => {
                    tracing::error!(
                        "HA: fencing {node} did not succeed ({other:?}); not recovering"
                    );
                    state
                        .ha_manager
                        .note_failure(
                            &affected[0].vm_id,
                            &node,
                            format!("fencing {node} failed ({other:?}); VMs stay frozen"),
                        )
                        .await;
                    continue;
                }
            }
        } else {
            if total < 3 || !quorum {
                tracing::warn!(
                    "HA: node {node} is down but it has no fencing device and the cluster has \
                     no 3-node majority: its VMs are NOT recovered (add a fencing device)"
                );
                continue;
            }
            if offline_for < self_fence_secs() + recovery_margin_secs() {
                continue;
            }
        }
        if targets.is_empty() {
            tracing::warn!(
                "HA: node {node} is down but every online node is in maintenance: \
                 its guests wait until one takes guests again"
            );
            continue;
        }
        recover_node(state, ls, &local, &nodes, &targets, &node).await;
    }
}

async fn publish(
    state: &Arc<AppState>,
    ls: &LoopState,
    local: &str,
    nodes: &[crate::cluster::node::Node],
    online: &[String],
) {
    let mut items = Vec::new();
    for r in state.ha_manager.resources_on(local).await {
        let Ok(vm) = state.vm_manager.get_vm(&r.vm_id).await else {
            continue;
        };
        let group = state.ha_manager.get_group(&r.group).await;
        items.push(HaReplica {
            vm,
            resource: r,
            group,
        });
    }
    let req = SyncRequest {
        from: local.to_string(),
        items,
    };
    for n in nodes {
        if n.name == local || n.is_local || !online.contains(&n.name) {
            continue;
        }
        let url = n.api_url(ls.port);
        match peer_post::<_, SyncReply>(ls, local, &url, "/api/ha/peer/sync", &req).await {
            Ok(reply) => {
                for newer in reply.newer {
                    let id = newer.vm_id.clone();
                    if state.ha_manager.apply_peer_resource(newer, None).await {
                        tracing::warn!("HA: adopted newer placement of VM {id} from {}", n.name);
                    }
                }
            }
            Err(e) => tracing::debug!("HA: sync to {} failed: {e}", n.name),
        }
    }
}

async fn local_restarts(
    state: &Arc<AppState>,
    ls: &LoopState,
    local: &str,
    nodes: &[crate::cluster::node::Node],
    online: &[String],
) {
    for r in state.ha_manager.resources_on(local).await {
        if r.state != HaState::Started {
            continue;
        }
        if QemuManager::process_running(&r.vm_id) {
            if r.restart_count > 0 {
                let _ = state
                    .ha_manager
                    .update_resource_state(&r.vm_id, HaState::Started, None)
                    .await;
            }
            continue;
        }
        match state.ha_manager.handle_vm_failure(&r.vm_id, local).await {
            Ok(HaAction::Restart) => match state.vm_manager.start_vm(&r.vm_id).await {
                Ok(_) => tracing::warn!("HA: restarted failed VM {}", r.vm_id),
                Err(e) => {
                    tracing::error!("HA: restart of VM {} failed: {e}", r.vm_id);
                    state
                        .ha_manager
                        .note_failure(&r.vm_id, local, format!("restart failed: {e}"))
                        .await;
                }
            },
            Ok(HaAction::Migrate(target)) => {
                let seq = r.placement_seq + 1;
                if move_vm(
                    state, ls, local, nodes, online, &r.vm_id, &target, seq, local,
                )
                .await
                {
                    tracing::warn!(
                        "HA: VM {} relocated to {target} after restart failures",
                        r.vm_id
                    );
                }
            }
            Ok(HaAction::None) => {}
            Err(e) => tracing::error!("HA: VM {} failure handling: {e}", r.vm_id),
        }
    }
}

async fn recover_node(
    state: &Arc<AppState>,
    ls: &LoopState,
    local: &str,
    nodes: &[crate::cluster::node::Node],
    online: &[String],
    failed: &str,
) {
    let plan = state.ha_manager.plan_relocations(failed, online).await;
    for p in plan {
        tracing::warn!(
            "HA: recovering VM {} of failed node {failed} on {}",
            p.vm_id,
            p.target
        );
        if !move_vm(
            state, ls, local, nodes, online, &p.vm_id, &p.target, p.seq, failed,
        )
        .await
        {
            state.ha_manager.count_failed_relocation(&p.vm_id).await;
        }
    }
}

/// Ask `target` (possibly this node) to start `vm_id`; on success record
/// the new placement here as well. Returns success.
#[allow(clippy::too_many_arguments)]
async fn move_vm(
    state: &Arc<AppState>,
    ls: &LoopState,
    local: &str,
    nodes: &[crate::cluster::node::Node],
    _online: &[String],
    vm_id: &str,
    target: &str,
    seq: u64,
    from: &str,
) -> bool {
    let res: Result<(), String> = if target == local {
        recover_here(state, vm_id, seq, from)
            .await
            .map_err(|e| format!("{e:?}"))
    } else if let Some(n) = nodes.iter().find(|n| n.name == target) {
        peer_post::<_, serde_json::Value>(
            ls,
            local,
            &n.api_url(ls.port),
            &format!("/api/ha/peer/recover/{vm_id}"),
            &RecoverRequest {
                from: from.to_string(),
                seq,
            },
        )
        .await
        .map(|_| ())
    } else {
        Err(format!("unknown node {target}"))
    };
    match res {
        Ok(()) => {
            state
                .ha_manager
                .set_placement(vm_id, target, seq, target != local)
                .await;
            true
        }
        Err(e) => {
            tracing::error!("HA: recovering VM {vm_id} on {target} failed: {e}");
            state
                .ha_manager
                .note_failure(vm_id, target, format!("recovery on {target} failed: {e}"))
                .await;
            false
        }
    }
}

#[cfg(test)]
mod recovery_guard_tests {
    use super::*;

    #[test]
    fn guard_marks_vm_until_dropped() {
        assert!(!is_recovering("guard-vm"));
        {
            let _g = RecoveryGuard::new("guard-vm");
            assert!(is_recovering("guard-vm"));
            assert!(!is_recovering("other-vm"));
        }
        assert!(!is_recovering("guard-vm"));
    }
}
