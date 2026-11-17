//! Idle power-down of lab nodes (readiness area 7): the live planner view, the
//! guarded "power one node off" action, and an opt-in background loop.
//!
//! The loop is OFF unless `HORCRUX_IDLE_POWER=1`. Only the leader (lowest-named
//! online node) acts, at most one node per sample, and only a node that the
//! quorum-safe planner has listed as a candidate for `HORCRUX_IDLE_POWER_SAMPLES`
//! (default 3) consecutive samples taken `HORCRUX_IDLE_POWER_INTERVAL_SECS`
//! (default 300) apart.
//! The same loop also wakes one powered-off `power-managed` node through its BMC when
//! fewer than `min_online` nodes run or every running node is at/above
//! `HORCRUX_IDLE_POWER_WAKE_CPU` (80 %); `HORCRUX_IDLE_POWER_WAKE=0` turns that off. A woken node is
//! not powered down again for `HORCRUX_IDLE_POWER_WAKE_GRACE_SECS` (1800). Only nodes carrying the `power-managed` label qualify.

use crate::cluster::power::{self, IdleTracker, PowerNode, PowerPolicy};
use crate::AppState;
use axum::http::StatusCode;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// Live planner input for the idle power-down views: (local node name, nodes, policy).
pub async fn power_view(state: &Arc<AppState>) -> (String, Vec<PowerNode>, PowerPolicy) {
    let (local, snap_nodes, guests) = crate::api::drain::snapshot(state).await;
    let cluster_nodes = state.cluster_manager.list_nodes_with_local().await;
    let bmc: Vec<String> = crate::api::ha_loop::fencing()
        .list_devices()
        .await
        .into_iter()
        .map(|d| d.node)
        .collect();
    let reachable: Vec<String> = snap_nodes.iter().map(|n| n.name.clone()).collect();
    let nodes = power::power_nodes_from_state(&cluster_nodes, &reachable, &guests, &bmc, &local);
    (local, nodes, PowerPolicy::default())
}

/// Gracefully power one node off through its BMC. Refused (409) unless the
/// planner currently lists it as a quorum-safe candidate and it is not this node.
pub async fn power_down(state: &Arc<AppState>, node: &str) -> Result<(), (StatusCode, String)> {
    use crate::ha::fencing::FencingResult;
    let (local, nodes, policy) = power_view(state).await;
    if node == local {
        return Err((
            StatusCode::CONFLICT,
            "refusing to power off the node serving this request".into(),
        ));
    }
    if !nodes.iter().any(|n| n.name == node) {
        return Err((StatusCode::NOT_FOUND, format!("unknown node {node}")));
    }
    if !power::plan_power_down(&nodes, &policy)
        .iter()
        .any(|n| n == node)
    {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "{node} is not a safe power-down candidate right now (see GET /api/cluster/power/status)"
            ),
        ));
    }
    match crate::api::ha_loop::fencing()
        .power_off_node_graceful(node, "idle power-down")
        .await
    {
        FencingResult::Success => Ok(()),
        FencingResult::Failed(e) => Err((StatusCode::BAD_GATEWAY, e)),
        other => Err((
            StatusCode::BAD_GATEWAY,
            format!("BMC shutdown not confirmed: {other:?}"),
        )),
    }
}

fn env_num<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// True when the operator enabled automatic idle power-down.
pub fn enabled() -> bool {
    matches!(
        std::env::var("HORCRUX_IDLE_POWER").as_deref(),
        Ok("1") | Ok("true") | Ok("yes") | Ok("on")
    )
}

pub fn spawn(state: Arc<AppState>) {
    static STARTED: OnceLock<()> = OnceLock::new();
    if !enabled() || STARTED.set(()).is_err() {
        return;
    }
    let interval = env_num("HORCRUX_IDLE_POWER_INTERVAL_SECS", 300u64).max(10);
    let samples = env_num("HORCRUX_IDLE_POWER_SAMPLES", 3u32).max(1);
    let wake_on_demand = !matches!(
        std::env::var("HORCRUX_IDLE_POWER_WAKE").as_deref(),
        Ok("0") | Ok("false") | Ok("no") | Ok("off")
    );
    let busy_cpu = env_num("HORCRUX_IDLE_POWER_WAKE_CPU", 80.0f32);
    let grace = Duration::from_secs(env_num("HORCRUX_IDLE_POWER_WAKE_GRACE_SECS", 1800u64));
    tracing::info!("idle power-down enabled: {samples} idle samples, {interval}s apart");
    tokio::spawn(async move {
        let mut tracker = IdleTracker::default();
        let mut woken: std::collections::HashMap<String, std::time::Instant> =
            std::collections::HashMap::new();
        loop {
            tokio::time::sleep(Duration::from_secs(interval)).await;
            let (local, snap, _) = crate::api::drain::snapshot(&state).await;
            if crate::api::drain::leader(&snap).as_deref() != Some(local.as_str()) {
                tracker = IdleTracker::default();
                continue;
            }
            let (_, nodes, policy) = power_view(&state).await;
            if wake_on_demand {
                if let Some(node) = power::plan_auto_wake(&nodes, policy.min_online, busy_cpu) {
                    woken.insert(node.clone(), std::time::Instant::now());
                    match crate::api::ha_loop::fencing().power_on_node(&node).await {
                        crate::ha::fencing::FencingResult::Success => {
                            tracing::info!("idle power: wake-on-demand powered {node} on")
                        }
                        other => tracing::warn!("idle power: wake-on-demand of {node}: {other:?}"),
                    }
                    tracker = IdleTracker::default();
                    continue;
                }
            }
            woken.retain(|_, t| t.elapsed() < grace);
            let mut cands = power::plan_power_down(&nodes, &policy);
            // A node just woken for demand is idle until guests land: leave it be.
            cands.retain(|n| !woken.contains_key(n));
            let Some(node) = tracker.observe(&cands, samples).into_iter().next() else {
                continue;
            };
            match power_down(&state, &node).await {
                Ok(()) => tracing::info!("idle power-down: {node} shut down through its BMC"),
                Err((code, e)) => tracing::warn!("idle power-down of {node} refused ({code}): {e}"),
            }
            tracker.reset(&node);
        }
    });
}

/// Nodes powered on for HA capacity, with when (so a node that never rejoins is
/// not woken in a loop, and the next parked node gets its turn).
static CAPACITY_WAKES: OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>,
> = OnceLock::new();

const CAPACITY_WAKE_RETRY: Duration = Duration::from_secs(900);

/// Pick the parked node to wake: first of `plan_wake` that was not already woken
/// for capacity within [`CAPACITY_WAKE_RETRY`].
pub fn pick_capacity_wake(
    nodes: &[power::PowerNode],
    recent: &std::collections::HashMap<String, std::time::Instant>,
) -> Option<String> {
    let open: Vec<power::PowerNode> = nodes
        .iter()
        .filter(|n| matches!(n.wake, Some(power::WakeMethod::Bmc)))
        .filter(|n| {
            recent
                .get(&n.name)
                .is_none_or(|t| t.elapsed() >= CAPACITY_WAKE_RETRY)
        })
        .cloned()
        .collect();
    power::plan_wake(&open, 1).into_iter().next()
}

/// The parked node HA would wake for capacity right now, if any.
pub async fn next_capacity_wake(state: &Arc<AppState>) -> Option<String> {
    let (_, nodes, _) = power_view(state).await;
    let map = CAPACITY_WAKES.get_or_init(Default::default).lock().ok()?;
    pick_capacity_wake(&nodes, &map)
}

/// True when HA may still gain capacity: a parked node can be woken, or one woken
/// for capacity in the last [`CAPACITY_WAKE_RETRY`] has not joined (`online`) yet,
/// so its guests wait for it instead of being failed while it boots.
pub async fn capacity_may_arrive(state: &Arc<AppState>, online: &[String]) -> bool {
    if next_capacity_wake(state).await.is_some() {
        return true;
    }
    CAPACITY_WAKES
        .get_or_init(Default::default)
        .lock()
        .map(|m| {
            m.iter()
                .any(|(n, t)| t.elapsed() < CAPACITY_WAKE_RETRY && !online.contains(n))
        })
        .unwrap_or(false)
}

/// HA found guests (`vm_ids`) that fit on no awake node: power on one parked node
/// through its BMC so the next HA pass can place them.
pub async fn wake_for_capacity(state: &Arc<AppState>, vm_ids: &[String]) {
    let Some(node) = next_capacity_wake(state).await else {
        return;
    };
    if let Ok(mut m) = CAPACITY_WAKES.get_or_init(Default::default).lock() {
        m.insert(node.clone(), std::time::Instant::now());
    }
    match crate::api::ha_loop::fencing().power_on_node(&node).await {
        crate::ha::fencing::FencingResult::Success => tracing::warn!(
            "HA: no awake node can take {} guest(s) ({}): powered {node} on for capacity",
            vm_ids.len(),
            vm_ids.join(", ")
        ),
        other => tracing::warn!("HA: wake of {node} for capacity failed: {other:?}"),
    }
}

#[cfg(test)]
mod capacity_wake_tests {
    use super::*;
    use crate::cluster::power::WakeMethod;

    fn node(name: &str, on: bool, wake: Option<WakeMethod>) -> PowerNode {
        PowerNode {
            name: name.into(),
            labels: vec![power::POWER_LABEL.into()],
            powered_on: on,
            running_guests: 0,
            cpu_percent: 0.0,
            hosts_osds: false,
            pinned: false,
            votes: 1,
            wake,
        }
    }

    #[test]
    fn capacity_wake_takes_parked_bmc_nodes_one_turn_at_a_time() {
        let nodes = vec![
            node("n1", true, Some(WakeMethod::Bmc)),
            node("n2", false, Some(WakeMethod::Wol([1, 2, 3, 4, 5, 6]))),
            node("n3", false, Some(WakeMethod::Bmc)),
            node("n4", false, Some(WakeMethod::Bmc)),
        ];
        let mut recent = std::collections::HashMap::new();
        // WoL-only nodes cannot be woken through the BMC; running nodes are not parked.
        assert_eq!(pick_capacity_wake(&nodes, &recent).as_deref(), Some("n3"));
        // n3 was just woken and has not rejoined: n4 is next, then nothing.
        recent.insert("n3".to_string(), std::time::Instant::now());
        assert_eq!(pick_capacity_wake(&nodes, &recent).as_deref(), Some("n4"));
        recent.insert("n4".to_string(), std::time::Instant::now());
        assert_eq!(pick_capacity_wake(&nodes, &recent), None);
    }
}
