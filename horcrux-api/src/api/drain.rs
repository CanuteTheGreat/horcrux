//! Node maintenance ("drain"), rebalance and guest placement rules (USE=cluster).
//!
//! - **Drain**: a node in maintenance takes no new guests, and its running VMs
//!   are live-migrated to other nodes (the QEMU-native engine in
//!   `vm_migration.rs`: memory/device state over TCP, local disks mirrored,
//!   shared storage not copied), placed by free RAM, architecture, pin,
//!   affinity and anti-affinity (`cluster/drain.rs`). Whatever cannot be moved
//!   (no node has room, a rule forbids it, the migration failed or timed out,
//!   containers - they have no cross-node transport yet) stays where it is and
//!   is handled by the local stop action (Save state by default), so a host
//!   shutdown never loses a guest. The host shutdown hook runs this drain
//!   first when `drain_on_shutdown` is on (node default, on).
//! - **Ownership**: a VM is owned by exactly one node. The migration protocol
//!   resumes the target only after the source has frozen and finished sending
//!   (a failed or cancelled move leaves the source running and the target
//!   without a trace), and for HA VMs the target takes the placement under a
//!   higher placement number in the same step (`migration_peer::commit`); the
//!   HA supervisors stop any copy on a node the cluster has not placed the VM
//!   on, and fencing/HA decides if the source dies mid-move.
//! - **Rebalance**: evens out memory load with live migrations only, at most
//!   `max_moves` at once, honouring the same rules; on demand ("Rebalance
//!   now", with a dry-run plan) or automatically when a node returns from
//!   maintenance or a new node joins (`on_return`), or whenever the spread
//!   exceeds the threshold (`auto`). The leader (lowest-named online node
//!   that is not in maintenance) runs the automatic passes.
//! - Every move is recorded (`guest_move`): when, from where to where, why,
//!   result - the guests' location history.
use crate::cluster::drain::{plan_drain, plan_rebalance, Move, Plan, PlanGuest, PlanNode, Rules};
use crate::cluster::node::{Node, NodeStatus};
use crate::db::config_store::ConfigStore;
use crate::error::ApiError;
use crate::middleware::auth::{AuthUser, CLUSTER_HA_ROLE};
use crate::migration::{MigrationConfig, MigrationState, MigrationType};
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Extension, Json, Router,
};
use horcrux_common::{VmHypervisor, VmStatus};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

const PLACEMENT: &str = "guest_placement";
const MAINT: &str = "node_maintenance";
const SETTINGS: &str = "rebalance_settings";
const MOVES: &str = "guest_move";
const MAX_HISTORY: usize = 500;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/nodes/maintenance",
            get(get_maintenance).post(set_maintenance),
        )
        .route("/api/nodes/maintenance/plan", get(drain_plan))
        .route(
            "/api/cluster/rebalance",
            get(get_rebalance).put(put_rebalance),
        )
        .route("/api/cluster/rebalance/run", post(run_rebalance))
        .route("/api/cluster/moves", get(list_moves))
        .route(
            "/api/vms/:id/placement",
            get(get_placement).put(put_placement),
        )
        .route("/api/ha/peer/placement", get(peer_placement))
        .route("/api/ha/peer/move", post(peer_move))
        .route("/api/ha/peer/rebalance-settings", post(peer_settings))
}

fn store(state: &AppState) -> ConfigStore {
    ConfigStore::new(&state.database)
}

fn require_peer(user: &Option<Extension<AuthUser>>) -> Result<(), ApiError> {
    match user {
        Some(Extension(u)) if u.role == CLUSTER_HA_ROLE => Ok(()),
        _ => Err(ApiError::Forbidden(
            "this route is only for other cluster nodes".into(),
        )),
    }
}

// ---------------------------------------------------------------- types

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// `off`, `on_return` (a node returns from maintenance or joins) or `auto`.
    pub mode: String,
    pub max_moves: usize,
    /// Memory-load spread (percentage points) above which rebalance acts.
    pub threshold_pct: f64,
    /// Least seconds between automatic passes.
    pub min_interval_secs: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: "on_return".into(),
            max_moves: 2,
            threshold_pct: 20.0,
            min_interval_secs: 300,
        }
    }
}

impl Settings {
    fn validate(&self) -> Result<(), String> {
        if !["off", "on_return", "auto"].contains(&self.mode.as_str()) {
            return Err("mode must be off, on_return or auto".into());
        }
        if self.max_moves == 0 || self.max_moves > 32 {
            return Err("max_moves must be 1-32".into());
        }
        if !(0.1..=100.0).contains(&self.threshold_pct) {
            return Err("threshold_pct must be 0.1-100".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MoveResult {
    pub guest_id: String,
    pub name: String,
    pub from: String,
    pub to: String,
    pub reason: String,
    /// pending, running, done, failed
    pub status: String,
    pub job_id: Option<String>,
    pub progress: u32,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Run {
    pub kind: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    /// planning, running, done
    pub status: String,
    pub plan: Plan,
    pub results: Vec<MoveResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Maintenance {
    pub enabled: bool,
    pub since: Option<String>,
    pub reason: Option<String>,
    pub drain: Option<Run>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveRecord {
    pub at: String,
    pub guest_id: String,
    pub name: String,
    pub from: String,
    pub to: String,
    pub reason: String,
    pub ok: bool,
    pub job_id: Option<String>,
    pub error: Option<String>,
}

/// What a node tells the others about itself (peer route).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub node: PlanNode,
    pub guests: Vec<PlanGuest>,
}

fn maint_cell() -> &'static Mutex<Maintenance> {
    static M: OnceLock<Mutex<Maintenance>> = OnceLock::new();
    M.get_or_init(Default::default)
}

fn rebalance_cell() -> &'static Mutex<Option<Run>> {
    static M: OnceLock<Mutex<Option<Run>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

// ------------------------------------------------------------- stored data

pub async fn load_rules(state: &AppState, vm: &str) -> Rules {
    store(state)
        .load::<Rules>(PLACEMENT)
        .await
        .ok()
        .and_then(|v| v.into_iter().find(|(i, _)| i == vm))
        .map(|(_, r)| r)
        .unwrap_or_default()
}

async fn load_settings(state: &AppState) -> Settings {
    store(state)
        .load::<Settings>(SETTINGS)
        .await
        .ok()
        .and_then(|v| v.into_iter().next())
        .map(|(_, s)| s)
        .unwrap_or_default()
}

async fn load_maintenance(state: &AppState) -> Maintenance {
    let mem = maint_cell().lock().unwrap().clone();
    if mem.enabled || mem.since.is_some() {
        return mem;
    }
    store(state)
        .load::<Maintenance>(MAINT)
        .await
        .ok()
        .and_then(|v| v.into_iter().next())
        .map(|(_, m)| m)
        .unwrap_or_default()
}

async fn save_maintenance(state: &AppState, m: &Maintenance) {
    *maint_cell().lock().unwrap() = m.clone();
    let _ = store(state).put(MAINT, "local", m).await;
}

async fn record_move(state: &AppState, r: &MoveResult) {
    let rec = MoveRecord {
        at: now(),
        guest_id: r.guest_id.clone(),
        name: r.name.clone(),
        from: r.from.clone(),
        to: r.to.clone(),
        reason: r.reason.clone(),
        ok: r.status == "done",
        job_id: r.job_id.clone(),
        error: r.error.clone(),
    };
    tracing::info!(
        "guest move {} {}: {} -> {} ({}): {}",
        r.guest_id,
        r.name,
        r.from,
        r.to,
        r.reason,
        if rec.ok {
            "done"
        } else {
            rec.error.as_deref().unwrap_or("failed")
        }
    );
    let st = store(state);
    let id = format!("{}-{}", chrono::Utc::now().timestamp_millis(), r.guest_id);
    let _ = st.put(MOVES, &id, &rec).await;
    // Keep the history bounded (oldest first by insertion order).
    if let Ok(all) = st.load::<MoveRecord>(MOVES).await {
        if all.len() > MAX_HISTORY {
            for (old, _) in all.iter().take(all.len() - MAX_HISTORY) {
                let _ = st.delete(MOVES, old).await;
            }
        }
    }
}

// ----------------------------------------------------------- the snapshot

fn arch_name() -> String {
    match std::env::consts::ARCH {
        "aarch64" => "Aarch64",
        "riscv64" => "Riscv64",
        "powerpc64" => "Ppc64le",
        _ => "X86_64",
    }
    .into()
}

fn mem_total_mb() -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.strip_prefix("MemTotal:"))
                .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
        })
        .map(|kb| kb / 1024)
        .unwrap_or(0)
}

/// This node and its guests, as the planner wants them.
pub async fn local_info(state: &Arc<AppState>) -> PeerInfo {
    let name = state
        .cluster_manager
        .get_local_node_name()
        .await
        .unwrap_or_else(|_| "localhost".into());
    let maintenance = load_maintenance(state).await.enabled;
    let mut guests = Vec::new();
    for v in state.vm_manager.list_vms().await {
        let qemu = v.hypervisor == VmHypervisor::Qemu;
        guests.push(PlanGuest {
            id: v.id.clone(),
            name: v.name.clone(),
            node: name.clone(),
            running: matches!(v.status, VmStatus::Running | VmStatus::Paused),
            memory_mb: v.memory,
            cpus: v.cpus,
            arch: format!("{:?}", v.architecture),
            immovable: (!qemu)
                .then(|| "LXD/Incus VMs are moved by LXD/Incus itself, not by Horcrux".to_string()),
            rules: load_rules(state, &v.id).await,
        });
    }
    for c in state.container_manager.list_containers().await {
        guests.push(PlanGuest {
            id: c.id.clone(),
            name: c.name.clone(),
            node: name.clone(),
            running: matches!(c.status, horcrux_common::ContainerStatus::Running),
            memory_mb: c.memory,
            cpus: c.cpus,
            arch: arch_name(),
            immovable: Some(
                "containers cannot be moved between Horcrux nodes yet; their state is saved locally"
                    .into(),
            ),
            rules: Rules::default(),
        });
    }
    PeerInfo {
        node: PlanNode {
            name,
            online: true,
            maintenance,
            arch: arch_name(),
            mem_total_mb: mem_total_mb(),
            cpu_cores: std::thread::available_parallelism()
                .map(|n| n.get() as u32)
                .unwrap_or(1),
        },
        guests,
    }
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(3600))
        .build()
        .map_err(|e| e.to_string())
}

fn peer_nodes(nodes: &[Node], local: &str) -> Vec<Node> {
    nodes
        .iter()
        .filter(|n| n.name != local && !n.is_local && n.status == NodeStatus::Online)
        .cloned()
        .collect()
}

async fn peer_get<T: serde::de::DeserializeOwned>(
    state: &AppState,
    local: &str,
    node: &Node,
    path: &str,
) -> Result<T, String> {
    let token = crate::middleware::auth::generate_cluster_ha_token(local)?;
    let r = client()
        .map_err(|e| e.to_string())?
        .get(format!("{}{path}", node.api_url(state.config.server.port)))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!(
            "{}: {}",
            r.status(),
            r.text().await.unwrap_or_default()
        ));
    }
    r.json::<T>().await.map_err(|e| e.to_string())
}

async fn peer_post<B: Serialize, T: serde::de::DeserializeOwned>(
    state: &AppState,
    local: &str,
    node: &Node,
    path: &str,
    body: &B,
) -> Result<T, String> {
    let token = crate::middleware::auth::generate_cluster_ha_token(local)?;
    let r = client()
        .map_err(|e| e.to_string())?
        .post(format!("{}{path}", node.api_url(state.config.server.port)))
        .bearer_auth(token)
        .json(body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = r.status();
    let text = r.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("{status}: {text}"));
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Every online node and every guest in the cluster.
pub async fn snapshot(state: &Arc<AppState>) -> (String, Vec<PlanNode>, Vec<PlanGuest>) {
    let me = local_info(state).await;
    let local = me.node.name.clone();
    let mut nodes = vec![me.node];
    let mut guests = me.guests;
    let all = state.cluster_manager.list_nodes().await;
    for n in peer_nodes(&all, &local) {
        match peer_get::<PeerInfo>(state, &local, &n, "/api/ha/peer/placement").await {
            Ok(info) => {
                nodes.push(info.node);
                guests.extend(info.guests);
            }
            Err(e) => tracing::warn!("placement info of {} unavailable: {e}", n.name),
        }
    }
    (local, nodes, guests)
}

// --------------------------------------------------------------- execution

/// Live-migrate one VM from this node and wait for the result.
async fn move_here(state: &Arc<AppState>, m: &Move, timeout_secs: u64) -> MoveResult {
    let mut r = MoveResult {
        guest_id: m.guest_id.clone(),
        name: m.name.clone(),
        from: m.from.clone(),
        to: m.to.clone(),
        reason: m.reason.clone(),
        status: "running".into(),
        ..Default::default()
    };
    let cfg = MigrationConfig {
        vm_id: m.guest_id.clone(),
        target_node: m.to.clone(),
        migration_type: MigrationType::Live,
        // MB/s; unset = unlimited (the test suite slows moves down to catch one).
        bandwidth_limit: std::env::var("HORCRUX_DRAIN_BANDWIDTH_MBPS")
            .ok()
            .and_then(|v| v.parse().ok()),
        force: false,
        // Local disks are mirrored when the target cannot see them; shared
        // storage is detected by the engine and not copied.
        with_local_disks: true,
        timeout_secs: Some(timeout_secs),
    };
    // The migration manager runs one migration at a time unless told
    // otherwise: wait for our turn instead of failing the move.
    let wait_until = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    let job_id = loop {
        match super::vm_migration::start(state, cfg.clone()).await {
            Ok(j) => break j,
            Err(e) => {
                let text = format!("{e:?}");
                if text.contains("Maximum concurrent migrations")
                    && tokio::time::Instant::now() < wait_until
                {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
                r.status = "failed".into();
                r.error = Some(text);
                record_move(state, &r).await;
                return r;
            }
        }
    };
    r.job_id = Some(job_id.clone());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs + 120);
    loop {
        match state.migration_manager.get_job(&job_id).await {
            Some(j) => match j.state {
                MigrationState::Completed => {
                    r.status = "done".into();
                    r.progress = 100;
                    break;
                }
                MigrationState::Failed | MigrationState::Cancelled => {
                    r.status = "failed".into();
                    r.error = Some(
                        j.error
                            .clone()
                            .unwrap_or_else(|| "migration did not complete".into()),
                    );
                    break;
                }
                _ => r.progress = j.progress as u32,
            },
            None => {
                r.status = "failed".into();
                r.error = Some("the migration job vanished".into());
                break;
            }
        }
        if tokio::time::Instant::now() > deadline {
            let _ = state.migration_manager.cancel_migration(&job_id).await;
            r.status = "failed".into();
            r.error = Some("the migration did not finish in time and was cancelled".into());
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    record_move(state, &r).await;
    r
}

/// Execute `moves` (from this node or any other: a move from another node is
/// started there, over the peer API), at most `parallel` at a time.
async fn execute(
    state: &Arc<AppState>,
    local: &str,
    moves: &[Move],
    parallel: usize,
    timeout_secs: u64,
    progress: impl Fn(&[MoveResult]) + Send + Sync + 'static,
) -> Vec<MoveResult> {
    let results = Arc::new(Mutex::new(
        moves
            .iter()
            .map(|m| MoveResult {
                guest_id: m.guest_id.clone(),
                name: m.name.clone(),
                from: m.from.clone(),
                to: m.to.clone(),
                reason: m.reason.clone(),
                status: "pending".into(),
                ..Default::default()
            })
            .collect::<Vec<_>>(),
    ));
    let progress = Arc::new(progress);
    let sem = Arc::new(tokio::sync::Semaphore::new(parallel.max(1)));
    let mut tasks = Vec::new();
    for (i, m) in moves.iter().cloned().enumerate() {
        let (st, sem, res, prog, local) = (
            state.clone(),
            sem.clone(),
            results.clone(),
            progress.clone(),
            local.to_string(),
        );
        tasks.push(tokio::spawn(async move {
            let _p = sem.acquire_owned().await;
            res.lock().unwrap()[i].status = "running".into();
            prog(&res.lock().unwrap());
            let out = if m.from == local {
                move_here(&st, &m, timeout_secs).await
            } else {
                let all = st.cluster_manager.list_nodes().await;
                match all.iter().find(|n| n.name == m.from) {
                    Some(n) => match peer_post::<_, MoveResult>(
                        &st,
                        &local,
                        n,
                        "/api/ha/peer/move",
                        &PeerMove {
                            guest_id: m.guest_id.clone(),
                            to: m.to.clone(),
                            reason: m.reason.clone(),
                            timeout_secs,
                        },
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(e) => MoveResult {
                            guest_id: m.guest_id.clone(),
                            name: m.name.clone(),
                            from: m.from.clone(),
                            to: m.to.clone(),
                            reason: m.reason.clone(),
                            status: "failed".into(),
                            error: Some(e),
                            ..Default::default()
                        },
                    },
                    None => MoveResult {
                        guest_id: m.guest_id.clone(),
                        name: m.name.clone(),
                        from: m.from.clone(),
                        to: m.to.clone(),
                        reason: m.reason.clone(),
                        status: "failed".into(),
                        error: Some(format!("{} is not a node of this cluster", m.from)),
                        ..Default::default()
                    },
                }
            };
            // The source node recorded the move it ran; the node that
            // orchestrated it keeps a record too, so the history shows every
            // move it asked for.
            if m.from != local {
                record_move(&st, &out).await;
            }
            res.lock().unwrap()[i] = out;
            prog(&res.lock().unwrap());
        }));
    }
    for t in tasks {
        let _ = t.await;
    }
    let out = results.lock().unwrap().clone();
    out
}

#[derive(Serialize, Deserialize)]
struct PeerMove {
    guest_id: String,
    to: String,
    reason: String,
    timeout_secs: u64,
}

// ------------------------------------------------------------------- drain

/// Whether this node has peers (`list_nodes` holds the other members; the
/// local node may or may not be listed too).
pub fn is_clustered(nodes: &[Node]) -> bool {
    nodes.iter().any(|n| !n.is_local)
}

/// Put this node in maintenance and move its running VMs away. Returns the
/// run (also kept for the UI); guests that could not be moved are in
/// `plan.unplaced` or have a failed result: they are still running here.
pub async fn drain_node(state: &Arc<AppState>, reason: &str, timeout_secs: u64) -> Run {
    let started = now();
    let mut m = Maintenance {
        enabled: true,
        since: Some(started.clone()),
        reason: Some(reason.to_string()),
        drain: Some(Run {
            kind: "drain".into(),
            started_at: started.clone(),
            status: "planning".into(),
            ..Default::default()
        }),
    };
    save_maintenance(state, &m).await;
    let (local, nodes, guests) = snapshot(state).await;
    let plan = plan_drain(&local, &nodes, &guests);
    let mut run = Run {
        kind: "drain".into(),
        started_at: started,
        status: "running".into(),
        plan: plan.clone(),
        results: vec![],
        finished_at: None,
    };
    m.drain = Some(run.clone());
    save_maintenance(state, &m).await;
    let base = m.clone();
    let plan_c = plan.clone();
    let results = execute(state, &local, &plan.moves, 2, timeout_secs, move |res| {
        let mut mm = base.clone();
        mm.drain = Some(Run {
            kind: "drain".into(),
            started_at: base.since.clone().unwrap_or_default(),
            status: "running".into(),
            plan: plan_c.clone(),
            results: res.to_vec(),
            finished_at: None,
        });
        *maint_cell().lock().unwrap() = mm;
    })
    .await;
    run.results = results;
    run.status = "done".into();
    run.finished_at = Some(now());
    m.drain = Some(run.clone());
    save_maintenance(state, &m).await;
    run
}

// -------------------------------------------------------------- rebalance

fn leader(nodes: &[PlanNode]) -> Option<String> {
    nodes
        .iter()
        .filter(|n| n.online && !n.maintenance)
        .map(|n| n.name.clone())
        .min()
}

static REBALANCE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn rebalance_run(
    state: &Arc<AppState>,
    st: &Settings,
    dry: bool,
    max: Option<usize>,
    kind: &str,
) -> Run {
    // One rebalance executes at a time (the on-return trigger, the leader's
    // loop and "Rebalance now" must not move the same guest twice).
    let _guard = if dry {
        None
    } else {
        match REBALANCE_LOCK.try_lock() {
            Ok(g) => Some(g),
            Err(_) => {
                return Run {
                    kind: kind.into(),
                    started_at: now(),
                    finished_at: Some(now()),
                    status: "skipped: a rebalance is already running".into(),
                    ..Default::default()
                }
            }
        }
    };
    let (local, nodes, guests) = snapshot(state).await;
    let plan = plan_rebalance(
        &nodes,
        &guests,
        max.unwrap_or(st.max_moves),
        st.threshold_pct,
    );
    let mut run = Run {
        kind: kind.into(),
        started_at: now(),
        status: if dry || plan.moves.is_empty() {
            "done".into()
        } else {
            "running".into()
        },
        plan: plan.clone(),
        results: vec![],
        finished_at: None,
    };
    if dry || plan.moves.is_empty() {
        run.finished_at = Some(now());
        return run;
    }
    *rebalance_cell().lock().unwrap() = Some(run.clone());
    let base = run.clone();
    let results = execute(state, &local, &plan.moves, st.max_moves, 900, move |res| {
        let mut r = base.clone();
        r.results = res.to_vec();
        *rebalance_cell().lock().unwrap() = Some(r);
    })
    .await;
    run.results = results;
    run.status = "done".into();
    run.finished_at = Some(now());
    *rebalance_cell().lock().unwrap() = Some(run.clone());
    run
}

/// Background: the leader notices a node that joined or came back from
/// maintenance (`on_return`/`auto`) and rebalances; `auto` also acts on spread.
pub fn spawn(state: Arc<AppState>) {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    tokio::spawn(async move {
        let mut known: Option<HashMap<String, bool>> = None; // node -> in maintenance
        let mut last_run = std::time::Instant::now() - Duration::from_secs(86400);
        loop {
            tokio::time::sleep(Duration::from_secs(
                std::env::var("HORCRUX_REBALANCE_TICK_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(15),
            ))
            .await;
            let st = load_settings(&state).await;
            if st.mode == "off" {
                known = None;
                continue;
            }
            let all = state.cluster_manager.list_nodes().await;
            if !is_clustered(&all) {
                continue;
            }
            let (local, nodes, _) = snapshot(&state).await;
            if leader(&nodes).as_deref() != Some(local.as_str()) {
                known = None;
                continue;
            }
            let now_map: HashMap<String, bool> = nodes
                .iter()
                .filter(|n| n.online)
                .map(|n| (n.name.clone(), n.maintenance))
                .collect();
            let arrived = match &known {
                None => false, // first look: nothing to compare with
                Some(prev) => now_map.iter().any(|(n, maint)| {
                    !maint
                        && match prev.get(n) {
                            None => true,       // joined / came back online
                            Some(true) => true, // left maintenance
                            Some(false) => false,
                        }
                }),
            };
            known = Some(now_map);
            let due = last_run.elapsed() >= Duration::from_secs(st.min_interval_secs);
            let busy = !state.migration_manager.list_active().await.is_empty();
            if busy || !(arrived || (st.mode == "auto" && due)) {
                continue;
            }
            let r = rebalance_run(
                &state,
                &st,
                false,
                None,
                if arrived { "on-return" } else { "auto" },
            )
            .await;
            last_run = std::time::Instant::now();
            if !r.plan.moves.is_empty() {
                tracing::info!("rebalance ({}): {} move(s)", r.kind, r.plan.moves.len());
            }
        }
    });
}

// ------------------------------------------------------------ HTTP handlers

#[derive(Serialize)]
struct MaintenanceView {
    #[serde(flatten)]
    m: Maintenance,
    /// This node would take part in the cluster's placement.
    clustered: bool,
}

async fn get_maintenance(
    State(state): State<Arc<AppState>>,
) -> Result<Json<MaintenanceView>, ApiError> {
    let clustered = is_clustered(&state.cluster_manager.list_nodes().await);
    Ok(Json(MaintenanceView {
        m: load_maintenance(&state).await,
        clustered,
    }))
}

#[derive(Deserialize)]
struct SetMaintenance {
    enabled: bool,
    /// Move the running VMs away when entering maintenance (default true).
    #[serde(default = "yes")]
    drain: bool,
}

fn yes() -> bool {
    true
}

async fn set_maintenance(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetMaintenance>,
) -> Result<Json<MaintenanceView>, ApiError> {
    let clustered = is_clustered(&state.cluster_manager.list_nodes().await);
    if req.enabled {
        if !clustered {
            return Err(ApiError::Conflict(
                "this node is not in a cluster: there is nowhere to move guests to".into(),
            ));
        }
        if req.drain {
            let st = state.clone();
            let timeout = crate::api::guest_state::policy::load_node(&st)
                .await
                .drain_timeout_secs;
            // Runs in the background; the UI polls GET /nodes/maintenance.
            tokio::spawn(async move {
                drain_node(&st, "maintenance", timeout).await;
            });
            // Make the flag visible at once.
            let mut m = load_maintenance(&state).await;
            m.enabled = true;
            m.since = Some(now());
            m.drain = Some(Run {
                kind: "drain".into(),
                started_at: now(),
                status: "planning".into(),
                ..Default::default()
            });
            *maint_cell().lock().unwrap() = m;
        } else {
            let m = Maintenance {
                enabled: true,
                since: Some(now()),
                reason: Some("maintenance".into()),
                drain: None,
            };
            save_maintenance(&state, &m).await;
        }
    } else {
        let m = Maintenance {
            enabled: false,
            since: None,
            reason: None,
            drain: load_maintenance(&state).await.drain,
        };
        save_maintenance(&state, &m).await;
        // The node is back: the cluster may want to even out (on_return/auto).
        let st = load_settings(&state).await;
        if st.mode != "off" && clustered {
            let s2 = state.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(3)).await;
                rebalance_run(&s2, &st, false, None, "on-return").await;
            });
        }
    }
    get_maintenance(State(state)).await
}

/// Dry run: what a drain of this node would do.
async fn drain_plan(State(state): State<Arc<AppState>>) -> Result<Json<Plan>, ApiError> {
    let (local, nodes, guests) = snapshot(&state).await;
    Ok(Json(plan_drain(&local, &nodes, &guests)))
}

#[derive(Serialize)]
struct RebalanceView {
    settings: Settings,
    /// What a rebalance would do right now.
    plan: Plan,
    nodes: Vec<PlanNode>,
    last: Option<Run>,
}

async fn get_rebalance(
    State(state): State<Arc<AppState>>,
) -> Result<Json<RebalanceView>, ApiError> {
    let settings = load_settings(&state).await;
    let (_, nodes, guests) = snapshot(&state).await;
    let plan = plan_rebalance(&nodes, &guests, settings.max_moves, settings.threshold_pct);
    Ok(Json(RebalanceView {
        settings,
        plan,
        nodes,
        last: rebalance_cell().lock().unwrap().clone(),
    }))
}

async fn put_rebalance(
    State(state): State<Arc<AppState>>,
    Json(s): Json<Settings>,
) -> Result<Json<Settings>, ApiError> {
    s.validate().map_err(ApiError::ValidationError)?;
    store(&state).put(SETTINGS, "cluster", &s).await?;
    // The leader may be any node: tell the others (best effort).
    if let Ok(local) = state.cluster_manager.get_local_node_name().await {
        let all = state.cluster_manager.list_nodes().await;
        for n in peer_nodes(&all, &local) {
            if let Err(e) = peer_post::<_, serde_json::Value>(
                &state,
                &local,
                &n,
                "/api/ha/peer/rebalance-settings",
                &s,
            )
            .await
            {
                tracing::warn!("rebalance settings not sent to {}: {e}", n.name);
            }
        }
    }
    Ok(Json(s))
}

#[derive(Deserialize, Default)]
struct RunRequest {
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    max_moves: Option<usize>,
}

async fn run_rebalance(
    State(state): State<Arc<AppState>>,
    body: Option<Json<RunRequest>>,
) -> Result<Json<Run>, ApiError> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    let st = load_settings(&state).await;
    if req.max_moves == Some(0) || req.max_moves.is_some_and(|m| m > 32) {
        return Err(ApiError::ValidationError("max_moves must be 1-32".into()));
    }
    if !req.dry_run && state.migration_manager.list_active().await.len() >= st.max_moves.max(1) * 4
    {
        return Err(ApiError::Conflict("migrations are already running".into()));
    }
    let r = if req.dry_run {
        rebalance_run(&state, &st, true, req.max_moves, "manual").await
    } else {
        // Return at once; progress is in GET /cluster/rebalance (`last`).
        let s2 = state.clone();
        let (st2, mm) = (st.clone(), req.max_moves);
        let run = rebalance_run(&state, &st, true, req.max_moves, "manual").await;
        if !run.plan.moves.is_empty() {
            tokio::spawn(async move {
                rebalance_run(&s2, &st2, false, mm, "manual").await;
            });
        }
        Run {
            status: if run.plan.moves.is_empty() {
                "done".into()
            } else {
                "running".into()
            },
            ..run
        }
    };
    Ok(Json(r))
}

#[derive(Deserialize)]
struct MovesQuery {
    guest: Option<String>,
}

async fn list_moves(
    State(state): State<Arc<AppState>>,
    Query(q): Query<MovesQuery>,
) -> Result<Json<Vec<MoveRecord>>, ApiError> {
    let mut v: Vec<MoveRecord> = store(&state)
        .load_values::<MoveRecord>(MOVES)
        .await?
        .into_iter()
        .filter(|m| q.guest.as_ref().is_none_or(|g| *g == m.guest_id))
        .collect();
    v.reverse();
    Ok(Json(v))
}

async fn get_placement(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Rules>, ApiError> {
    state.vm_manager.get_vm(&id).await?;
    Ok(Json(load_rules(&state, &id).await))
}

async fn put_placement(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(r): Json<Rules>,
) -> Result<Json<Rules>, ApiError> {
    state.vm_manager.get_vm(&id).await?;
    for (what, g) in [
        ("affinity_group", &r.affinity_group),
        ("anti_affinity_group", &r.anti_affinity_group),
    ] {
        if g.as_deref()
            .is_some_and(|g| g.len() > 64 || g.contains(char::is_whitespace))
        {
            return Err(ApiError::ValidationError(format!(
                "{what}: up to 64 characters, no spaces"
            )));
        }
    }
    if r.affinity_group.is_some() && r.affinity_group == r.anti_affinity_group {
        return Err(ApiError::ValidationError(
            "a guest cannot be in the same affinity and anti-affinity group".into(),
        ));
    }
    if let Some(p) = &r.pin {
        let nodes = state.cluster_manager.list_nodes().await;
        let local = state
            .cluster_manager
            .get_local_node_name()
            .await
            .unwrap_or_default();
        if *p != local && !nodes.iter().any(|n| &n.name == p) {
            return Err(ApiError::ValidationError(format!(
                "{p} is not a node of this cluster"
            )));
        }
    }
    if r == Rules::default() {
        store(&state).delete(PLACEMENT, &id).await?;
    } else {
        store(&state).put(PLACEMENT, &id, &r).await?;
    }
    Ok(Json(r))
}

// --- peer handlers

async fn peer_placement(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
) -> Result<Json<PeerInfo>, ApiError> {
    require_peer(&user)?;
    Ok(Json(local_info(&state).await))
}

async fn peer_move(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Json(req): Json<PeerMove>,
) -> Result<Json<MoveResult>, ApiError> {
    require_peer(&user)?;
    let local = state.cluster_manager.get_local_node_name().await?;
    let name = state
        .vm_manager
        .get_vm(&req.guest_id)
        .await
        .map(|v| v.name)
        .unwrap_or_default();
    let m = Move {
        guest_id: req.guest_id,
        name,
        from: local,
        to: req.to,
        reason: req.reason,
    };
    Ok(Json(move_here(&state, &m, req.timeout_secs).await))
}

async fn peer_settings(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Json(s): Json<Settings>,
) -> Result<Json<Settings>, ApiError> {
    require_peer(&user)?;
    s.validate().map_err(ApiError::ValidationError)?;
    store(&state).put(SETTINGS, "cluster", &s).await?;
    Ok(Json(s))
}

/// Host shutdown: drain first when the node is clustered and the node policy
/// says so. Returns the guests that moved (id -> target) and why the others
/// did not.
pub async fn drain_for_shutdown(
    state: &Arc<AppState>,
    timeout_secs: u64,
) -> Option<(HashMap<String, String>, HashMap<String, String>)> {
    let all = state.cluster_manager.list_nodes().await;
    if !is_clustered(&all) {
        return None;
    }
    let run = drain_node(state, "host shutdown", timeout_secs).await;
    let mut moved = HashMap::new();
    let mut stayed: HashMap<String, String> = run
        .plan
        .unplaced
        .iter()
        .map(|u| (u.guest_id.clone(), u.reason.clone()))
        .collect();
    for r in &run.results {
        if r.status == "done" {
            moved.insert(r.guest_id.clone(), r.to.clone());
        } else {
            stayed.insert(
                r.guest_id.clone(),
                r.error.clone().unwrap_or_else(|| "move failed".into()),
            );
        }
    }
    Some((moved, stayed))
}

/// After a host boot: a node that went into maintenance for its own shutdown
/// is back; leave maintenance (explicit administrator maintenance stays) and
/// let the cluster rebalance if its settings say so.
pub async fn leave_boot_maintenance(state: &Arc<AppState>) {
    let m = load_maintenance(state).await;
    if !(m.enabled && m.reason.as_deref() == Some("host shutdown")) {
        return;
    }
    save_maintenance(
        state,
        &Maintenance {
            enabled: false,
            since: None,
            reason: None,
            drain: m.drain,
        },
    )
    .await;
    tracing::info!("node back from its shutdown drain: maintenance off");
    let st = load_settings(state).await;
    if st.mode != "off" {
        let s2 = state.clone();
        tokio::spawn(async move {
            // Let the other nodes see this one online first.
            tokio::time::sleep(Duration::from_secs(10)).await;
            rebalance_run(&s2, &st, false, None, "on-return").await;
        });
    }
}
