//! Idle power-down of lab nodes (readiness area 7): the live planner view, the
//! guarded "power one node off" action, and an opt-in background loop.
//!
//! The loop is OFF unless `HORCRUX_IDLE_POWER=1`. Only the leader (lowest-named
//! online node) acts, at most one node per sample, and only a node that the
//! quorum-safe planner has listed as a candidate for `HORCRUX_IDLE_POWER_SAMPLES`
//! (default 3) consecutive samples taken `HORCRUX_IDLE_POWER_INTERVAL_SECS`
//! (default 300) apart. Only nodes carrying the `power-managed` label qualify.

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
    if !power::plan_power_down(&nodes, &policy).iter().any(|n| n == node) {
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
    tracing::info!("idle power-down enabled: {samples} idle samples, {interval}s apart");
    tokio::spawn(async move {
        let mut tracker = IdleTracker::default();
        loop {
            tokio::time::sleep(Duration::from_secs(interval)).await;
            let (local, snap, _) = crate::api::drain::snapshot(&state).await;
            if crate::api::drain::leader(&snap).as_deref() != Some(local.as_str()) {
                tracker = IdleTracker::default();
                continue;
            }
            let (_, nodes, policy) = power_view(&state).await;
            let cands = power::plan_power_down(&nodes, &policy);
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
