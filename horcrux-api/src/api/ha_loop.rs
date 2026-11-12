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
//!    leader needs a majority and waits out the self-fence time, and then
//!    only recovers a node whose kernel is silent: a node that still answers
//!    TCP has a hung or dead manager that cannot self-fence, its guests may
//!    still run, so they are left alone (split-brain guard) UNLESS that node
//!    runs the manager-independent dead-man agent (`ha_fence_agent`, advertised
//!    in the placement sync): the agent kills the node's HA guests once the
//!    manager's heartbeat is older than its deadline, so after that deadline a
//!    silent manager proves the guests are gone (a rebooted node whose manager
//!    did not start no longer blocks recovery for minutes). Each
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
    env_secs("HORCRUX_HA_TICK_SECS", 3)
}
/// How long a node without a majority waits before it kills its HA VMs,
/// counted from the LAST moment it still saw a majority (see [`FenceClock`]).
fn self_fence_secs() -> i64 {
    env_secs("HORCRUX_HA_SELF_FENCE_SECS", 10) as i64
}
/// Period of the self-fence watchdog (its own task, independent of the HA
/// tick, which can be stuck in a peer call for a long time).
const WATCHDOG_SECS: i64 = 1;
/// Upper bound for killing the guests of a node that lost its majority, whatever
/// state QEMU is in (SIGKILL, then a node reset if one survives; see
/// `ha_fence_agent::fence_kill`). Not "kill_qemu's 10 s": a QEMU in D-state on
/// NFS never dies by signal, the reset is what bounds it.
const KILL_BUDGET_SECS: i64 = super::ha_fence_agent::FENCE_KILL_BUDGET_SECS;
/// Clock-rate drift allowance between two nodes over one fencing window.
const DRIFT_SECS: i64 = 2;

/// Smallest recovery margin that keeps the isolated node's self-fence strictly
/// before the survivors' recovery for the given monitor timings (proof in
/// `fencing_timing_tests`): the survivors may notice the loss up to
/// `2*poll + fetch` earlier than the isolated node loses its LAST peer, plus
/// the watchdog granularity (twice) and the time to kill the guests.
pub(crate) fn recovery_margin_floor(poll: i64, fetch: i64, watchdog: i64, kill: i64) -> i64 {
    2 * poll + fetch + 2 * watchdog + kill + DRIFT_SECS
}

/// Extra time the leader waits, beyond the self-fence time, before it
/// recovers the VMs of an unfenced node. Default = the proven floor for the
/// monitor timings in effect; an explicit `HORCRUX_HA_RECOVERY_MARGIN_SECS`
/// overrides it (lab/CI shortcuts: a value below the floor is logged).
fn recovery_margin_secs() -> i64 {
    let floor = recovery_margin_floor(
        crate::cluster::usage::poll_interval().as_secs() as i64,
        crate::cluster::usage::FETCH_TIMEOUT.as_secs() as i64,
        WATCHDOG_SECS,
        KILL_BUDGET_SECS,
    );
    match std::env::var("HORCRUX_HA_RECOVERY_MARGIN_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|v| *v >= 1)
    {
        Some(v) => {
            if v < floor {
                static WARNED: OnceLock<()> = OnceLock::new();
                WARNED.get_or_init(|| {
                    tracing::warn!(
                        "HA: HORCRUX_HA_RECOVERY_MARGIN_SECS={v} is below the safe floor {floor} s \
                         for the monitor timings in effect: a partitioned node may still run its \
                         guest when the survivors recover it (split brain)"
                    )
                });
            }
            v
        }
        None => floor,
    }
}

/// The self-fence clock of one node. It starts at the LAST observation that
/// still had a majority, not when the loss is first noticed: a node notices a
/// lost majority one heartbeat timeout after its peers noticed it, and a
/// clock started at the notice let the survivors recover a guest ~6 s before
/// the isolated node had killed its copy (two writers on one disk; lab HA
/// suite 2026-11-06, partition of 1 node).
#[derive(Debug, Default)]
pub(crate) struct FenceClock {
    last_quorum: Option<i64>,
    no_quorum_since: Option<i64>,
}

impl FenceClock {
    /// One observation at `now`; true when the node must kill its HA guests.
    pub(crate) fn observe(&mut self, now: i64, has_majority: bool, fence_after: i64) -> bool {
        if has_majority {
            self.last_quorum = Some(now);
            self.no_quorum_since = None;
            return false;
        }
        let since = *self
            .no_quorum_since
            .get_or_insert(self.last_quorum.unwrap_or(now));
        now - since >= fence_after
    }
}

/// Wait before fencing a node that has gone quiet (it may only be slow).
fn fence_after_secs() -> i64 {
    env_secs("HORCRUX_HA_FENCE_AFTER_SECS", 10) as i64
}

/// What a TCP probe of a silent node's API port says about the node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Liveness {
    /// Its kernel answered (connection accepted, or actively refused): the
    /// machine is up, so QEMU processes there may well still run their
    /// guests. A SIGSTOPped or dead manager looks like this too.
    KernelAlive,
    /// Nothing answered (timeout, unreachable, no route): crashed, powered
    /// off or partitioned; a node in that state self-fences or is dead.
    Silent,
}

/// Classify the result of a TCP connect to a node's API port. Only a
/// completed handshake or a RST from the node itself proves its kernel is
/// alive; a timeout or an ICMP unreachable (from a router, or ARP failure)
/// proves nothing and is treated as silent (crash/partition).
pub(crate) fn classify_probe(r: &std::io::Result<()>) -> Liveness {
    match r {
        Ok(()) => Liveness::KernelAlive,
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => Liveness::KernelAlive,
        Err(_) => Liveness::Silent,
    }
}

/// What the leader does with a node that has been offline long enough.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryGate {
    /// Safe to move its guests.
    Recover,
    /// Silent node, but the self-fence window is not over yet.
    Wait,
    /// Kernel alive and nothing proves its guests are gone: leave them.
    HoldAlive,
}

/// The split-brain gate for a node with no fencing device.
///
/// * silent kernel (crash, power-off, partition): it self-fenced or is dead
///   once `silent_wait` has passed;
/// * kernel alive and its manager still answers HTTP: the node is up and
///   managing its guests, only its heartbeat is lost: never recover;
/// * kernel alive, manager silent (hung, dead, or a rebooted node whose manager
///   did not start): recover only if the node runs the dead-man agent and the
///   manager has been silent for `agent_wait` (agent deadline + settle time),
///   i.e. its QEMUs are provably gone. Without an agent they may still run: hold.
/// * the same node, but its agent answers a `safe` attestation (heartbeat stale
///   and guests gone, or no manager at boot and nothing running): recover at once
///   (after [`ATTEST_MIN_SILENT_SECS`] of silence), no advert needed.
pub(crate) fn recovery_gate(
    live: Liveness,
    manager_answers: bool,
    agent_wait: Option<i64>,
    mgr_silent_for: i64,
    offline_for: i64,
    silent_wait: i64,
    attest: Attest,
) -> RecoveryGate {
    match live {
        Liveness::Silent => {
            if offline_for >= silent_wait {
                RecoveryGate::Recover
            } else {
                RecoveryGate::Wait
            }
        }
        Liveness::KernelAlive if manager_answers => RecoveryGate::HoldAlive,
        Liveness::KernelAlive => match (attest, agent_wait) {
            // first-hand proof from the node's own agent (works with no manager at all)
            (Attest::Safe, _) if mgr_silent_for >= ATTEST_MIN_SILENT_SECS => RecoveryGate::Recover,
            (Attest::Safe, _) => RecoveryGate::Wait,
            // the agent answers but the node is not provably dead yet (fresh heartbeat,
            // guests still being killed, manager still allowed to start): wait, but
            // not forever
            (Attest::NotYet, _) if mgr_silent_for < ATTEST_NOT_YET_MAX_SECS => RecoveryGate::Wait,
            (_, Some(a)) => {
                if mgr_silent_for >= a && offline_for >= a {
                    RecoveryGate::Recover
                } else {
                    RecoveryGate::Wait
                }
            }
            _ => RecoveryGate::HoldAlive,
        },
    }
}

/// What the node's own fence agent said when asked (`ha_fence_agent::fetch_attestation`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Attest {
    /// Its HA guests are provably not running and nothing can start them.
    Safe,
    /// The agent answered; the node is not provably dead (yet).
    NotYet,
    /// No answer (older agent, port closed, filtered).
    Unavailable,
}

/// The manager must have been silent this long before an attestation is acted
/// on: one lost probe is not a dead manager.
pub(crate) const ATTEST_MIN_SILENT_SECS: i64 = 3;
/// A node whose agent keeps saying "not yet" this long is a manager that is not
/// beating but answers nothing either: held like any unproven node.
pub(crate) const ATTEST_NOT_YET_MAX_SECS: i64 = 180;

/// Does a manager answer HTTP on the node (any status)? A SIGSTOPped manager
/// still completes TCP handshakes in the kernel backlog but never replies.
async fn manager_answers_http(client: &reqwest::Client, host: &str, port: u16) -> bool {
    let url = if host.contains(':') && !host.starts_with('[') {
        format!("https://[{host}]:{port}/api/health/live")
    } else {
        format!("https://{host}:{port}/api/health/live")
    };
    client
        .get(url)
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .is_ok()
}

/// Probe `host:port` with a short TCP connect.
pub(crate) async fn probe_kernel(host: &str, port: u16) -> Liveness {
    let attempt = async {
        let mut addrs = tokio::net::lookup_host((host, port)).await?;
        let addr = addrs
            .next()
            .ok_or_else(|| std::io::Error::other("no address"))?;
        tokio::net::TcpStream::connect(addr).await.map(|_| ())
    };
    match tokio::time::timeout(Duration::from_secs(3), attempt).await {
        Ok(r) => classify_probe(&r),
        Err(_) => Liveness::Silent,
    }
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
        .route("/api/ha/peer/group", post(peer_group))
        .route("/api/ha/peer/members", post(peer_members))
        .route(
            "/api/ha/peer/members/:name/removed",
            post(peer_member_removed),
        )
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
    /// Every HA group the sender knows, so groups without a resource (or
    /// created while HA was off) still reach all nodes.
    #[serde(default)]
    groups: Vec<HaGroup>,
    /// Set when the sender runs the manager-independent fence agent
    /// (`ha_fence_agent`): its deadline in seconds. The leader only recovers
    /// the guests of a silent-manager node on a live kernel when it was set.
    #[serde(default)]
    fence_agent_secs: Option<i64>,
}

/// A cluster-wide HA group change pushed by the node that took the request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum GroupChange {
    Put { group: HaGroup },
    Delete { name: String },
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
    super::ha_fence_agent::note_peer_agent(
        &req.from,
        req.fence_agent_secs,
        chrono::Utc::now().timestamp(),
    );
    let st = store(&state);
    // HA is a cluster-wide mode: the first resource a peer publishes
    // switches it on here too (the supervisor idles while it is off).
    state.ha_manager.merge_peer_groups(req.groups).await;
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

/// A peer created, edited or deleted an HA group: mirror it (local only,
/// never fans out again).
async fn peer_group(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Json(change): Json<GroupChange>,
) -> Result<StatusCode, ApiError> {
    require_peer(&user)?;
    match change {
        GroupChange::Put { group } => state.ha_manager.put_peer_group(group).await,
        GroupChange::Delete { name } => {
            state.ha_manager.drop_peer_group(&name).await;
        }
    }
    Ok(StatusCode::OK)
}

/// Push an HA group change to every online peer (best effort: a peer that
/// misses it still learns created groups from the periodic sync).
pub async fn broadcast_group(state: &Arc<AppState>, change: GroupChange) {
    let Ok(local) = state.cluster_manager.get_local_node_name().await else {
        return;
    };
    let Ok(client) = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(10))
        .danger_accept_invalid_certs(true)
        .build()
    else {
        return;
    };
    let ls = LoopState {
        client,
        port: state.config.server.port,
        offline_since: HashMap::new(),
        last_fence: HashMap::new(),
        hung_reported: Default::default(),
        mgr_silent_since: HashMap::new(),
        synced_with_peer: true,
    };
    for n in state.cluster_manager.list_nodes().await {
        if n.name == local || n.is_local || n.status != NodeStatus::Online {
            continue;
        }
        let url = n.api_url(ls.port);
        if let Err(e) = peer_post::<_, ()>(&ls, &local, &url, "/api/ha/peer/group", &change).await {
            tracing::warn!("HA: could not replicate group change to {}: {e}", n.name);
        }
    }
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
        .accept_rejoined_member(&m.name, &m.address)
        .await?;
    Ok(StatusCode::OK)
}

/// A member tells us a node was removed from the cluster. Local only: the
/// member that took the `DELETE` informs every other member itself, so this
/// never fans out again.
async fn peer_member_removed(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    require_peer(&user)?;
    state.cluster_manager.accept_member_removed(&name).await?;
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
pub(crate) struct RecoveryGuard(String);

impl RecoveryGuard {
    pub(crate) fn new(vm_id: &str) -> Self {
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
    /// node -> last fencing attempt (retry pacing)
    last_fence: HashMap<String, i64>,
    /// nodes already reported as "kernel alive, manager silent" (log/event once)
    hung_reported: std::collections::HashSet<String>,
    /// node -> unix time the leader first saw it with a live kernel and a
    /// manager that does not answer, continuously since (see [`recovery_gate`]).
    mgr_silent_since: HashMap<String, i64>,
    /// This manager process has completed a placement sync with at least one
    /// peer (see [`may_restart_local_guests`]).
    synced_with_peer: bool,
}

/// May this node (re)start HA guests that its own records say belong here?
///
/// A node that has just booted (hard crash, power loss, manager restart) holds a
/// STALE placement: while it was down the cluster may have recovered the guest on
/// another node. Its records are only trustworthy once it has exchanged placements
/// with a peer (`publish` adopts every newer placement, the stale-copy sweep then
/// kills a leftover), so no guest is started before one sync round succeeded.
/// Without it a failed or not-yet-possible sync (peers still `Offline` in the
/// first seconds, a dropped request) lets the node start the guest a second time
/// on the same disks (lab cluster-hasuite j01296, 'hard crash, node stays down').
/// Clusters below 3 nodes, or a lone node with no online peer, have nobody to ask
/// (and nobody who may recover for them without a fencing device).
pub(crate) fn may_restart_local_guests(
    total: usize,
    quorum: bool,
    online_peers: usize,
    synced_with_peer: bool,
) -> bool {
    if total < 3 {
        return online_peers == 0 || synced_with_peer;
    }
    quorum && synced_with_peer
}

/// Start the supervisor (once per process). Needs the usage monitor, which
/// supplies the node online/offline view.
/// Self-fence watchdog: a node of a 3+ node cluster that has had no majority
/// for [`self_fence_secs`] (from its last majority) kills its HA guests. Own
/// task on a 1 s period: the HA tick can sit for a long time in a peer call
/// (`publish`, 60 s client timeout) exactly when the network is partitioned.
async fn self_fence_watchdog(state: Arc<AppState>) {
    let mut clock = FenceClock::default();
    let mut had_majority = true;
    let mut ticker = tokio::time::interval(Duration::from_secs(WATCHDOG_SECS as u64));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        let Ok(local) = state.cluster_manager.get_local_node_name().await else {
            continue;
        };
        let nodes = state.cluster_manager.list_nodes().await;
        let now = chrono::Utc::now().timestamp();
        let online = nodes
            .iter()
            .filter(|n| n.is_local || n.name == local || n.status == NodeStatus::Online)
            .count();
        let total = nodes.len().max(online);
        let majority = total < 3 || online * 2 > total;
        if majority != had_majority {
            // The partition record the lab suite reads: when this node lost / regained
            // its majority and what it saw (the evidence for the self-fence timing).
            tracing::warn!(
                "HA: majority {}: {online}/{total} nodes online ({}), self-fence after {} s",
                if majority { "regained" } else { "LOST" },
                nodes
                    .iter()
                    .map(|n| format!(
                        "{}={:?} seen {}s ago",
                        n.name,
                        n.status,
                        n.last_seen.map(|s| now - s).unwrap_or(-1)
                    ))
                    .collect::<Vec<_>>()
                    .join(", "),
                self_fence_secs()
            );
            had_majority = majority;
        }
        if !clock.observe(now, majority, self_fence_secs()) {
            continue;
        }
        let running: Vec<_> = state
            .ha_manager
            .resources_on(&local)
            .await
            .into_iter()
            .filter(|r| QemuManager::process_running(&r.vm_id))
            .collect();
        let ids: Vec<String> = running.iter().map(|r| r.vm_id.clone()).collect();
        for id in &ids {
            tracing::error!("HA: no majority: self-fencing, killing VM {id}");
        }
        // SIGKILL all, reset the node if one survives (QEMU stuck in storage I/O)
        let reset = super::ha_fence_agent::fence_kill(&ids, "no majority").await;
        if !ids.is_empty() {
            tracing::error!(
                "HA: no majority: self-fence of {} guest(s) done{}",
                ids.len(),
                if reset {
                    " (a QEMU was stuck: node reset requested)"
                } else {
                    ""
                }
            );
        }
        let kills = running.iter().map(|r| {
            let state = state.clone();
            let local = local.clone();
            let vm_id = r.vm_id.clone();
            async move {
                let _ = state.vm_manager.stop_vm(&vm_id).await;
                state
                    .ha_manager
                    .note_failure(
                        &vm_id,
                        &local,
                        "node lost the cluster majority: VM stopped (self-fence)".into(),
                    )
                    .await;
            }
        });
        futures::future::join_all(kills).await;
    }
}

/// Heartbeat for the dead-man agent (`ha_fence_agent`): its own task, one write
/// per second while this manager is able to answer a question about its HA
/// resources. A hung or dead manager stops writing and the agent kills the HA
/// guests after `HORCRUX_HA_AGENT_FENCE_SECS`. Also keeps the agent alive.
async fn fence_heartbeat(state: Arc<AppState>) {
    use super::ha_fence_agent as agent;
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut local: Option<String> = None;
    // The beat proves the manager ANSWERS HTTP, not that some task still runs: the
    // leader's "manager silent" probe and this check are the same question, so a
    // node the leader sees silent has stopped beating (a deadlocked HTTP stack with
    // a live heartbeat task would otherwise be recovered while its guests run).
    let Ok(selfcheck) = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(2))
        .danger_accept_invalid_certs(true)
        .build()
    else {
        return;
    };
    let port = state.config.server.port;
    agent::ensure_service();
    let mut until_agent_check: u8 = 0;
    loop {
        ticker.tick().await;
        if until_agent_check == 0 {
            agent::ensure_agent();
            until_agent_check = 5;
        }
        until_agent_check -= 1;
        if local.is_none() {
            local = state.cluster_manager.get_local_node_name().await.ok();
        }
        let Some(local) = local.as_deref() else {
            continue;
        };
        if !manager_answers_http(&selfcheck, "127.0.0.1", port).await {
            continue;
        }
        let vms = tokio::time::timeout(Duration::from_secs(2), async {
            state
                .ha_manager
                .resources_on(local)
                .await
                .into_iter()
                .map(|r| r.vm_id)
                .collect::<Vec<_>>()
        })
        .await;
        if let Ok(vms) = vms {
            if let Err(e) = agent::beat(&vms) {
                tracing::debug!("HA: fence heartbeat: {e}");
            }
        }
    }
}

pub fn spawn(state: Arc<AppState>) {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    tokio::spawn(self_fence_watchdog(state.clone()));
    tokio::spawn(fence_heartbeat(state.clone()));
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
            last_fence: HashMap::new(),
            hung_reported: Default::default(),
            mgr_silent_since: HashMap::new(),
            synced_with_peer: false,
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
    peer_post_for(ls, local, base, path, body, None).await
}

/// How long one placement sync to one peer may take. The supervisor client's 60 s
/// timeout is for `recover` (it starts a guest); a sync is a small best-effort
/// message, and a peer whose manager is SIGSTOPped/deadlocked still completes
/// the TCP handshake in the kernel backlog and then never answers: with the 60 s
/// timeout that peer froze the WHOLE tick (its own recovery included) for a minute
/// right after the injection (j02106 real laptops, 'manager hung': tick stuck
/// +5 s..+65 s, recovery 98 s instead of ~30 s).
fn sync_timeout() -> Duration {
    Duration::from_secs(env_secs("HORCRUX_HA_SYNC_TIMEOUT_SECS", SYNC_TIMEOUT_SECS))
}
const SYNC_TIMEOUT_SECS: u64 = 5;

async fn peer_post_for<T: Serialize, R: serde::de::DeserializeOwned>(
    ls: &LoopState,
    local: &str,
    base: &str,
    path: &str,
    body: &T,
    timeout: Option<Duration>,
) -> Result<R, String> {
    let token = crate::middleware::auth::generate_cluster_ha_token(local)?;
    let mut rb = ls
        .client
        .post(format!("{base}{path}"))
        .bearer_auth(token)
        .json(body);
    if let Some(t) = timeout {
        rb = rb.timeout(t);
    }
    let resp = rb.send().await.map_err(|e| e.to_string())?;
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
    // Required node labels come from each guest's placement rules.
    for r in &all {
        let rules = super::drain::load_rules(state, &r.vm_id).await;
        let labels = rules.required_labels;
        state.ha_manager.set_required_labels(&r.vm_id, labels).await;
    }
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
                    cpu_level: n.cpu_level,
                    caps: n.caps.clone(),
                })
                .await;
        } else {
            state.ha_manager.remove_node_resources(&n.name).await;
        }
    }

    // 1. publish to peers, adopt newer placements.
    let synced = publish(state, ls, &local, &nodes, &online).await;
    if synced > 0 {
        ls.synced_with_peer = true;
    }

    // 2. stale copies of VMs the cluster placed elsewhere.
    for r in state.ha_manager.list_resources().await {
        if let Some(node) = r.current_node.as_deref() {
            // A VM whose live migration to this node is in flight has its QEMU here (paused,
            // `-incoming`) while the placement still names the source until the commit: that is
            // the migration target, not a stale copy (lab HA suite 2026-11-06: every live migration
            // of an HA guest was killed by this check, 'Connection reset by peer' on the source).
            if node != local
                && !is_recovering(&r.vm_id)
                && !super::migration_peer::has_incoming(&r.vm_id).await
                && QemuManager::process_running(&r.vm_id)
            {
                tracing::warn!(
                    "HA: VM {} runs here but is placed on {node}; stopping the stale copy",
                    r.vm_id
                );
                crate::vm::migrate::kill_qemu(&r.vm_id).await;
                let _ = state.vm_manager.stop_vm(&r.vm_id).await;
            }
        }
    }

    // 3. (self-fence without a majority runs in its own watchdog task,
    // `self_fence_watchdog`: it must not wait for this tick.)
    // 4. restart HA VMs that should run here.
    let online_peers = online.iter().filter(|n| **n != local).count();
    if (total < 3 || quorum)
        && may_restart_local_guests(total, quorum, online_peers, ls.synced_with_peer)
    {
        local_restarts(state, ls, &local, &nodes, &online).await;
    }

    // 5. leader: recover resources of dead nodes.
    for n in &nodes {
        if is_local(n) || n.status == NodeStatus::Online {
            ls.offline_since.remove(&n.name);
            ls.hung_reported.remove(&n.name);
            ls.mgr_silent_since.remove(&n.name);
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
            // The dead-man agent (ha_fence_agent) bounds how long a node with a
            // dead or hung manager can keep its guests: when the node advertised
            // one, a silent manager on a live kernel is safe to recover after
            // the agent's deadline (counted from when WE first saw the manager
            // silent), not after the (longer) network self-fence margin.
            let silent_wait = self_fence_secs() + recovery_margin_secs();
            let agent_wait = super::ha_fence_agent::peer_agent_secs(&node, now)
                .map(|s| s + super::ha_fence_agent::AGENT_SETTLE_SECS);
            // A node whose manager is hung (SIGSTOP, deadlock) or dead cannot
            // self-fence, yet its QEMUs keep running the guests: the timer
            // above only proves safety for a node that lost the network.
            // With no fencing device the only proof of death is silence, so
            // a node whose kernel still answers is NOT recovered (a second
            // copy would write the same disks) unless its agent has had the
            // time to kill them; it is recovered after it is fenced (add a
            // device), or when it falls silent / its manager is back.
            let ip = nodes
                .iter()
                .find(|x| x.name == node)
                .map(|x| x.ip.clone())
                .unwrap_or_default();
            let live = probe_kernel(&ip, ls.port).await;
            let manager_answers = live == Liveness::KernelAlive
                && manager_answers_http(&ls.client, &ip, ls.port).await;
            // `now` is the tick start (a tick can sit in a 60 s peer call): the
            // silence clock must start when the probe actually returned.
            let observed = chrono::Utc::now().timestamp();
            let mgr_silent_for = if live == Liveness::KernelAlive && !manager_answers {
                observed - *ls.mgr_silent_since.entry(node.clone()).or_insert(observed)
            } else {
                ls.mgr_silent_since.remove(&node);
                0
            };
            let attest = if live == Liveness::KernelAlive && !manager_answers {
                match super::ha_fence_agent::fetch_attestation(&ip).await {
                    Some(a) => {
                        tracing::info!(
                            "HA: {node} agent attests safe={} ({}); heartbeat age {:?}, running {:?}, boot {}",
                            a.safe, a.reason, a.hb_age, a.running, a.boot_id
                        );
                        if a.safe {
                            Attest::Safe
                        } else {
                            Attest::NotYet
                        }
                    }
                    None => Attest::Unavailable,
                }
            } else {
                Attest::Unavailable
            };
            let gate = recovery_gate(
                live,
                manager_answers,
                agent_wait,
                mgr_silent_for,
                offline_for,
                silent_wait,
                attest,
            );
            if gate == RecoveryGate::Wait {
                continue;
            }
            if gate == RecoveryGate::HoldAlive {
                if ls.hung_reported.insert(node.clone()) {
                    tracing::error!(
                        "HA: node {node} is offline for the cluster but its kernel still \
                         answers: its manager is hung or dead and cannot self-fence, its \
                         guests may still be running. NOT recovering them (split brain); \
                         add a fencing device or restart the manager there"
                    );
                    state
                        .ha_manager
                        .note_failure(
                            &affected[0].vm_id,
                            &node,
                            format!(
                                "manager on {node} unresponsive but the node is alive: guests \
                                 are not recovered (no fencing device)"
                            ),
                        )
                        .await;
                    notify(
                        state,
                        "node_manager_hung",
                        serde_json::json!({"node": node, "detected_by": local}),
                    )
                    .await;
                }
                continue;
            }
            ls.hung_reported.remove(&node);
            if live == Liveness::KernelAlive {
                // WARN (the default log level drops INFO): which proof released the guests of a node whose kernel still answers
                tracing::warn!(
                    "HA: {node} manager silent for {mgr_silent_for} s, kernel alive: recovering via {} (offline {offline_for} s)",
                    match (attest, agent_wait) {
                        (Attest::Safe, _) => "the node's own safe attestation",
                        (_, Some(_)) => "the advertised agent deadline",
                        _ => "unknown",
                    }
                );
            }
        }
        // The node may have come back while the probe above ran (a returning node starts
        // its guests itself): look again right before moving anything.
        if state
            .cluster_manager
            .list_nodes()
            .await
            .iter()
            .any(|x| x.name == node && x.status == NodeStatus::Online)
        {
            ls.offline_since.remove(&node);
            ls.mgr_silent_since.remove(&node);
            continue;
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
) -> usize {
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
        groups: state.ha_manager.list_groups().await,
        fence_agent_secs: super::ha_fence_agent::agent_running()
            .then(super::ha_fence_agent::fence_secs),
    };
    let mut synced = 0;
    // All peers at once, each bounded by `sync_timeout`: one silent peer must not delay the others
    // (nor this tick's recovery step) by its timeout.
    let timeout = sync_timeout();
    let calls = nodes
        .iter()
        .filter(|n| !(n.name == local || n.is_local || !online.contains(&n.name)))
        .map(|n| {
            let url = n.api_url(ls.port);
            let req = &req;
            async move {
                (
                    n,
                    peer_post_for::<_, SyncReply>(
                        ls,
                        local,
                        &url,
                        "/api/ha/peer/sync",
                        req,
                        Some(timeout),
                    )
                    .await,
                )
            }
        });
    for (n, res) in futures::future::join_all(calls).await {
        match res {
            Ok(reply) => {
                synced += 1;
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
    synced
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
mod restart_barrier_tests {
    use super::*;

    #[test]
    fn a_freshly_booted_node_starts_nothing_before_it_synced_with_a_peer() {
        // 3 nodes, majority visible, but no sync yet (stale placement): never start
        assert!(!may_restart_local_guests(3, true, 2, false));
        assert!(!may_restart_local_guests(5, true, 4, false));
        // after one successful sync round it may
        assert!(may_restart_local_guests(3, true, 2, true));
    }

    #[test]
    fn no_majority_never_starts() {
        assert!(!may_restart_local_guests(3, false, 0, true));
        assert!(!may_restart_local_guests(3, false, 1, true));
    }

    #[test]
    fn small_clusters_have_nobody_to_ask() {
        assert!(may_restart_local_guests(1, false, 0, false));
        assert!(may_restart_local_guests(2, false, 0, false));
        // a 2-node cluster with its peer online waits for the sync
        assert!(!may_restart_local_guests(2, false, 1, false));
        assert!(may_restart_local_guests(2, false, 1, true));
    }
}

#[cfg(test)]
mod group_wire_tests {
    use super::*;

    #[test]
    fn sync_request_without_groups_still_parses() {
        let r: SyncRequest = serde_json::from_str(r#"{"from":"a","items":[]}"#).unwrap();
        assert!(r.groups.is_empty());
    }

    #[test]
    fn group_change_round_trips() {
        let g = HaGroup {
            name: "hxg".into(),
            nodes: vec!["n1".into()],
            restricted: true,
            no_failback: false,
        };
        let j = serde_json::to_string(&GroupChange::Put { group: g }).unwrap();
        assert!(j.contains(r#""op":"put""#));
        match serde_json::from_str::<GroupChange>(&j).unwrap() {
            GroupChange::Put { group } => assert_eq!(group.name, "hxg"),
            _ => panic!("wrong variant"),
        }
        let d: GroupChange = serde_json::from_str(r#"{"op":"delete","name":"x"}"#).unwrap();
        assert!(matches!(d, GroupChange::Delete { name } if name == "x"));
    }
}

#[cfg(test)]
mod hung_manager_tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    #[test]
    fn accepted_and_refused_connections_prove_the_kernel_is_alive() {
        assert_eq!(classify_probe(&Ok(())), Liveness::KernelAlive);
        assert_eq!(
            classify_probe(&Err(Error::from(ErrorKind::ConnectionRefused))),
            Liveness::KernelAlive
        );
    }

    #[test]
    fn silence_and_unreachable_prove_nothing() {
        for k in [
            ErrorKind::TimedOut,
            ErrorKind::HostUnreachable,
            ErrorKind::NetworkUnreachable,
            ErrorKind::ConnectionReset,
            ErrorKind::Other,
        ] {
            assert_eq!(
                classify_probe(&Err(Error::from(k))),
                Liveness::Silent,
                "{k:?}"
            );
        }
    }

    /// A stopped manager keeps its listening socket: the kernel completes
    /// the handshake (backlog) although nothing ever serves the request.
    #[tokio::test]
    async fn listener_that_never_accepts_is_alive() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert_eq!(probe_kernel("127.0.0.1", port).await, Liveness::KernelAlive);
        drop(l);
        // nothing listens any more: RST from the live kernel (manager dead)
        assert_eq!(probe_kernel("127.0.0.1", port).await, Liveness::KernelAlive);
    }

    #[tokio::test]
    async fn unresolvable_node_is_silent() {
        assert_eq!(probe_kernel("", 8006).await, Liveness::Silent);
        assert_eq!(
            probe_kernel("no-such-host.invalid", 8006).await,
            Liveness::Silent
        );
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

/// Proof that the isolated node's self-fence precedes the survivors' recovery.
///
/// Model (integer seconds, real time; the partition starts at t = 0, no contact
/// after). Every node polls its peers every `poll` s at its own phase; a peer
/// turns Offline at the first poll that is more than `stale` s after the last
/// successful one (the status is written up to `fetch` s after the poll
/// starts). The survivors' leader starts its recovery clock when it notices the
/// node Offline (earliest: the poll itself) and recovers after
/// `fence + margin`. The isolated node feeds the REAL [`FenceClock`] once per
/// watchdog period (its majority is gone when its last peer turned Offline) and
/// needs up to `kill` s to stop the guests. A node only compares against its own
/// clock, so skew between nodes cancels; rate drift is the `DRIFT_SECS` part of
/// the floor. Exhaustive over every poll phase of both sides and the watchdog.
#[cfg(test)]
mod fencing_timing_tests {
    use super::*;

    #[derive(Clone, Copy)]
    struct Timing {
        poll: i64,
        fetch: i64,
        stale: i64,
        watchdog: i64,
        fence: i64,
        kill: i64,
    }

    /// start of the last poll at or before the cut (t = 0), phase in 0..poll
    fn last_seen(phase: i64, poll: i64) -> i64 {
        if phase == 0 {
            0
        } else {
            phase - poll
        }
    }

    /// first poll start with poll - last_seen > stale
    fn flip_poll(last_seen: i64, phase: i64, t: &Timing) -> i64 {
        let mut p = phase - t.poll * 100;
        while p <= last_seen + t.stale {
            p += t.poll;
        }
        p
    }

    /// (guests stopped at, earliest recovery start) for one phase combination;
    /// `old` = the pre-fix clock that starts when the loss is first noticed.
    fn run(t: &Timing, margin: i64, sp: i64, ip: i64, wp: i64, old: bool) -> (i64, i64) {
        let recover_at = flip_poll(last_seen(sp, t.poll), sp, t) + t.fence + margin;
        let majority_lost_at = flip_poll(last_seen(ip, t.poll), ip, t) + t.fetch;
        let mut clock = FenceClock::default();
        let mut first_without: Option<i64> = None;
        let mut tick = wp - t.watchdog * 1000;
        loop {
            let majority = tick < majority_lost_at;
            let fire = if old {
                if majority {
                    false
                } else {
                    let since = *first_without.get_or_insert(tick);
                    tick - since >= t.fence
                }
            } else {
                clock.observe(tick, majority, t.fence)
            };
            if fire {
                return (tick + t.kill, recover_at);
            }
            tick += t.watchdog;
            assert!(tick < majority_lost_at + t.fence + 1000, "never fired");
        }
    }

    /// (smallest recover_at - fenced_at, number of violating phase combinations)
    fn sweep(t: &Timing, margin: i64, old: bool) -> (i64, u32) {
        let (mut worst, mut bad) = (i64::MAX, 0);
        for sp in 0..t.poll {
            for ip in 0..t.poll {
                for wp in 0..t.watchdog {
                    let (fenced, recover) = run(t, margin, sp, ip, wp, old);
                    worst = worst.min(recover - fenced);
                    if fenced >= recover {
                        bad += 1;
                    }
                }
            }
        }
        (worst, bad)
    }

    fn defaults() -> Timing {
        Timing {
            poll: 2,
            fetch: 3,
            stale: 6,
            watchdog: WATCHDOG_SECS,
            fence: 10,
            kill: KILL_BUDGET_SECS,
        }
    }

    #[test]
    fn fence_precedes_recovery_for_every_phase_with_default_timings() {
        let t = defaults();
        let margin = recovery_margin_floor(t.poll, t.fetch, t.watchdog, t.kill);
        let (slack, bad) = sweep(&t, margin, false);
        assert_eq!(bad, 0, "split brain possible");
        assert!(
            slack >= DRIFT_SECS,
            "slack {slack} s leaves no room for clock drift"
        );
    }

    #[test]
    fn fence_precedes_recovery_over_a_grid_of_timings() {
        for poll in [1, 2, 3, 5, 10, 15, 20] {
            for fetch in [1, 2, 3, 4, 8] {
                for stale_polls in [2, 3, 5] {
                    for fence in [1, 8, 30, 60, 300] {
                        for watchdog in [1, 2] {
                            let t = Timing {
                                poll,
                                fetch,
                                stale: poll * stale_polls,
                                watchdog,
                                fence,
                                kill: KILL_BUDGET_SECS,
                            };
                            let margin = recovery_margin_floor(poll, fetch, watchdog, t.kill);
                            let (slack, bad) = sweep(&t, margin, false);
                            assert_eq!(bad, 0, "split brain: poll {poll} fetch {fetch} stale {} fence {fence} wd {watchdog}", t.stale);
                            assert!(slack >= DRIFT_SECS, "slack {slack}");
                        }
                    }
                }
            }
        }
    }

    /// the model has teeth: the old clock (started at the notice, margin 15)
    /// violates it, as the lab saw (survivors recovered ~6 s before the kill)
    #[test]
    fn old_clock_with_old_margin_is_caught_by_the_model() {
        let mut t = defaults();
        t.fence = 60;
        // the old clock (started at the notice) with a margin of the size the lab
        // run used (15 s, sized for a 10 s kill) violates the property
        let (_, bad) = sweep(&t, 5, true);
        assert!(bad > 0, "the model must flag the pre-fix behaviour");
        // and a clock from the last majority with a margin below the floor is still
        // unsafe (the margin is part of the proof)
        let (_, bad) = sweep(&t, 5, false);
        assert!(bad > 0, "margin 5 is below the floor");
    }

    /// below the proven floor the property breaks; the floor is tight to within the allowances
    #[test]
    fn margin_below_the_floor_breaks_the_property() {
        let t = defaults();
        let floor = recovery_margin_floor(t.poll, t.fetch, t.watchdog, t.kill);
        // recover - fenced is linear in the margin, so the model's own slack at the
        // floor is exactly how far the margin can drop before it breaks
        let (slack, bad) = sweep(&t, floor, false);
        assert_eq!(bad, 0);
        assert!(
            slack <= DRIFT_SECS + 2 * t.watchdog + 2 * t.poll,
            "floor is {slack} s above the model's limit: too loose"
        );
        let (worst, bad) = sweep(&t, floor - slack - 1, false);
        assert!(bad > 0, "{} s below the floor must violate", slack + 1);
        assert!(worst < 0);
    }

    /// The stuck-QEMU case is inside the proof: the model charges the full kill
    /// budget (SIGKILL wait + node reset) to every phase combination, and the
    /// budget is the bound `fence_kill` enforces (a QEMU in D-state is reset, not waited for).
    #[test]
    fn the_kill_budget_is_the_enforced_fence_bound() {
        use crate::api::ha_fence_agent::{FENCE_KILL_WAIT_SECS, FENCE_RESET_SECS};
        assert_eq!(KILL_BUDGET_SECS, FENCE_KILL_WAIT_SECS + FENCE_RESET_SECS);
        let t = defaults();
        assert_eq!(t.kill, FENCE_KILL_WAIT_SECS + FENCE_RESET_SECS);
        // the budget is load-bearing: a fence that takes `slack + 1` s longer than the
        // budget the margin was sized for breaks the property
        let margin = recovery_margin_floor(t.poll, t.fetch, t.watchdog, t.kill);
        let (slack, bad) = sweep(&t, margin, false);
        assert_eq!(bad, 0);
        let slower = Timing {
            kill: t.kill + slack + 1,
            ..t
        };
        let (_, bad) = sweep(&slower, margin, false);
        assert!(bad > 0, "model must notice a fence slower than its budget");
    }

    /// G1: a silent node's guest is recovered inside 90 s with the shipped
    /// defaults: stale detection + a supervisor tick + the proven wait + the
    /// peer recover call + guest start (lab: 7 s start, 5 s tick, 10 s slack).
    #[test]
    fn default_silent_node_failover_fits_the_90_s_goal() {
        let t = defaults();
        let wait = self_fence_secs() + recovery_margin_secs();
        let detect = t.stale + t.poll; // last poll + stale threshold + one poll to flip
        let worst = detect + tick_secs() as i64 + wait + 7 + 10;
        assert!(worst < 90, "worst case {worst} s (wait {wait} s)");
        // and the wait is exactly the proven floor, never below it
        assert_eq!(
            wait,
            self_fence_secs() + recovery_margin_floor(t.poll, t.fetch, t.watchdog, t.kill)
        );
    }

    #[test]
    fn clock_starts_at_the_last_majority_not_at_the_notice() {
        let mut c = FenceClock::default();
        assert!(!c.observe(100, true, 30));
        assert!(!c.observe(120, false, 30)); // noticed 20 s later: clock runs since 100
        assert!(!c.observe(129, false, 30));
        assert!(c.observe(130, false, 30));
        assert!(c.observe(131, false, 30));
        // majority back: the clock resets
        assert!(!c.observe(140, true, 30));
        assert!(!c.observe(150, false, 30));
        assert!(c.observe(170, false, 30));
    }

    #[test]
    fn a_node_that_never_had_a_majority_fences_from_its_first_observation() {
        let mut c = FenceClock::default();
        assert!(!c.observe(10, false, 30));
        assert!(c.observe(40, false, 30));
    }
}

/// The leader's split-brain gate with the dead-man agent (lab j01330: a rebooted
/// node whose manager stayed down kept the leader from recovering for 5 minutes).
#[cfg(test)]
mod fence_agent_gate_tests {
    use super::*;
    use crate::api::ha_fence_agent::{fence_secs, AGENT_SETTLE_SECS};

    const SILENT: i64 = 58;

    fn agent() -> Option<i64> {
        Some(fence_secs() + AGENT_SETTLE_SECS)
    }

    #[test]
    fn rebooted_node_without_manager_is_recovered_once_the_agent_deadline_passed() {
        let a = agent().unwrap();
        // refused (manager not running), agent advertised, silent long enough
        assert_eq!(
            recovery_gate(
                Liveness::KernelAlive,
                false,
                agent(),
                a,
                a,
                SILENT,
                Attest::Unavailable
            ),
            RecoveryGate::Recover
        );
        // the guest may still be running before that
        assert_eq!(
            recovery_gate(
                Liveness::KernelAlive,
                false,
                agent(),
                a - 1,
                a + 30,
                SILENT,
                Attest::Unavailable
            ),
            RecoveryGate::Wait
        );
        // seen silent for long, but only just marked offline: both clocks count
        assert_eq!(
            recovery_gate(
                Liveness::KernelAlive,
                false,
                agent(),
                a + 50,
                a - 1,
                SILENT,
                Attest::Unavailable
            ),
            RecoveryGate::Wait
        );
    }

    #[test]
    fn no_agent_means_no_recovery_on_a_live_kernel() {
        // the split-brain guard stays for nodes that cannot be proven fenced
        for t in [0, 33, 58, 600, 100_000] {
            assert_eq!(
                recovery_gate(
                    Liveness::KernelAlive,
                    false,
                    None,
                    t,
                    t,
                    SILENT,
                    Attest::Unavailable
                ),
                RecoveryGate::HoldAlive
            );
        }
    }

    #[test]
    fn an_answering_manager_is_never_recovered() {
        for t in [0, 33, 58, 600, 100_000] {
            assert_eq!(
                recovery_gate(
                    Liveness::KernelAlive,
                    true,
                    agent(),
                    t,
                    t,
                    SILENT,
                    Attest::Unavailable
                ),
                RecoveryGate::HoldAlive
            );
        }
    }

    #[test]
    fn silent_node_still_waits_for_the_network_self_fence_margin() {
        assert_eq!(
            recovery_gate(
                Liveness::Silent,
                false,
                agent(),
                0,
                SILENT - 1,
                SILENT,
                Attest::Unavailable
            ),
            RecoveryGate::Wait
        );
        assert_eq!(
            recovery_gate(
                Liveness::Silent,
                false,
                None,
                0,
                SILENT,
                SILENT,
                Attest::Unavailable
            ),
            RecoveryGate::Recover
        );
    }

    /// Exhaustive over the phase between the node's last heartbeat, its manager
    /// stopping to answer, and the leader's first silent probe: the agent has
    /// killed the guests before the leader's earliest recovery.
    /// t=0: the manager answers for the last time. The heartbeat is at most one
    /// period (1 s) older than that; the agent polls every second and needs up
    /// to `kill` seconds; the leader starts counting at its first silent probe,
    /// which is never before t=0.
    #[test]
    fn agent_kill_always_precedes_the_leaders_recovery() {
        let fence = fence_secs();
        let wait = agent().unwrap();
        for hb_age_at_last_answer in 0..=1i64 {
            for first_probe in 0..=40i64 {
                for drift in -2..=2i64 {
                    let last_beat = -hb_age_at_last_answer;
                    let killed_by =
                        last_beat + fence + 1 + crate::api::ha_fence_agent::FENCE_KILL_BUDGET_SECS;
                    let recovery_at = first_probe + wait;
                    assert!(
                        killed_by + drift.abs() <= recovery_at,
                        "kill at {killed_by} (+drift {drift}) after recovery at {recovery_at}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_safe_attestation_recovers_a_live_kernel_without_waiting_for_the_advert() {
        let k = Liveness::KernelAlive;
        // manager-less reboot (j01743): no agent advert at all, agent attests safe
        assert_eq!(
            recovery_gate(k, false, None, 3, 3, SILENT, Attest::Safe),
            RecoveryGate::Recover
        );
        assert_eq!(
            recovery_gate(k, false, None, 2, 40, SILENT, Attest::Safe),
            RecoveryGate::Wait
        );
        // an answering manager always wins: never recovered, whatever the agent says
        assert_eq!(
            recovery_gate(k, true, None, 99, 99, SILENT, Attest::Safe),
            RecoveryGate::HoldAlive
        );
        // not provably dead yet (guests still running / fresh heartbeat): wait, bounded
        assert_eq!(
            recovery_gate(k, false, None, 100, 100, SILENT, Attest::NotYet),
            RecoveryGate::Wait
        );
        assert_eq!(
            recovery_gate(
                k,
                false,
                None,
                ATTEST_NOT_YET_MAX_SECS,
                999,
                SILENT,
                Attest::NotYet
            ),
            RecoveryGate::HoldAlive
        );
        // a safe attestation never applies to a silent kernel (that path is unchanged)
        assert_eq!(
            recovery_gate(Liveness::Silent, false, None, 0, 1, SILENT, Attest::Safe),
            RecoveryGate::Wait
        );
    }

    /// The point of the change: a rebooted node (kernel back within ~30 s,
    /// manager off) is recovered well inside the 90 s goal, from the first probe
    /// that sees its kernel alive.
    #[test]
    fn recovery_fits_the_goal_for_a_rebooted_node() {
        let detect = 15; // lab j01330: offline for the cluster after 14 s
        let kernel_back = 30;
        let recovered_at = detect.max(kernel_back) + agent().unwrap() + 5 /* tick */;
        assert!(recovered_at + 15 /* guest start */ < 90, "{recovered_at}");
        // and faster than the network self-fence margin it replaces
        assert!(agent().unwrap() < SILENT);
    }
}

#[cfg(test)]
mod sync_stall_tests {
    use super::*;

    /// A peer that accepts the TCP connection and never answers (a SIGSTOPped manager) must not hold a
    /// placement sync for the supervisor client's 60 s: the tick has to reach its recovery step in seconds.
    #[tokio::test]
    async fn a_frozen_peer_cannot_stall_a_sync_for_the_client_timeout() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            loop {
                if let Ok((s, _)) = l.accept().await {
                    held.push(s); // accepted, never read, never answered
                }
            }
        });
        let ls = LoopState {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .unwrap(),
            port,
            offline_since: HashMap::new(),
            last_fence: HashMap::new(),
            hung_reported: Default::default(),
            mgr_silent_since: HashMap::new(),
            synced_with_peer: true,
        };
        let t = std::time::Instant::now();
        let r: Result<(), String> = peer_post_for(
            &ls,
            "n1",
            &format!("http://127.0.0.1:{port}"),
            "/api/ha/peer/sync",
            &serde_json::json!({}),
            Some(Duration::from_millis(800)),
        )
        .await;
        assert!(r.is_err());
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    }

    /// The default sync bound plus the probes of one tick must stay well under the
    /// attestation window (agent deadline 15 s + settle): detection is not delayed by a sync.
    #[test]
    fn the_default_sync_bound_is_short() {
        const { assert!(SYNC_TIMEOUT_SECS <= 6) };
        const { assert!(SYNC_TIMEOUT_SECS >= 3) }; // a healthy peer under load must still sync
    }
}
