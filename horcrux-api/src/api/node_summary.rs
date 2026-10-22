//! `GET /api/nodes/summary`: one cheap call for monitoring systems
//! (version, node identity, cluster quorum, HA mode, guest counts by state)
//! instead of five. Needs only Sys.Audit, so a read-only API key can poll it.
use crate::AppState;
use axum::{extract::State, routing::get, Json, Router};
use horcrux_common::{ContainerStatus, VmStatus};
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/api/nodes/summary", get(summary))
}

#[derive(Serialize, Default)]
pub struct GuestCounts {
    pub total: usize,
    pub running: usize,
    pub stopped: usize,
    pub paused: usize,
    pub unknown: usize,
}

#[derive(Serialize)]
pub struct ClusterSummary {
    pub name: Option<String>,
    pub nodes_total: usize,
    pub nodes_online: usize,
    /// More than half of the nodes answer the heartbeat (`None` = no cluster).
    pub quorum: Option<bool>,
}

#[derive(Serialize)]
pub struct HaSummary {
    pub enabled: bool,
    pub resources: usize,
    /// HA resources by state ("Started", "Error", ...).
    pub by_state: BTreeMap<String, usize>,
}

#[derive(Serialize)]
pub struct NodeSummary {
    pub version: &'static str,
    pub node: String,
    pub node_id: String,
    pub uptime_secs: Option<u64>,
    pub vms: GuestCounts,
    pub containers: GuestCounts,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster: Option<ClusterSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ha: Option<HaSummary>,
}

async fn summary(State(state): State<Arc<AppState>>) -> Json<NodeSummary> {
    let mut vms = GuestCounts::default();
    for vm in state.vm_manager.list_vms().await {
        vms.total += 1;
        match vm.status {
            VmStatus::Running => vms.running += 1,
            VmStatus::Stopped => vms.stopped += 1,
            VmStatus::Paused => vms.paused += 1,
            VmStatus::Unknown => vms.unknown += 1,
        }
    }
    let mut containers = GuestCounts::default();
    for c in state.container_manager.list_containers().await {
        containers.total += 1;
        match c.status {
            ContainerStatus::Running => containers.running += 1,
            ContainerStatus::Stopped => containers.stopped += 1,
            ContainerStatus::Paused => containers.paused += 1,
            ContainerStatus::Unknown => containers.unknown += 1,
        }
    }
    let node = state
        .cluster_manager
        .get_local_node_name()
        .await
        .unwrap_or_default();
    #[cfg(feature = "cluster")]
    let cluster = {
        let report = state.cluster_manager.status_report(vms.total).await;
        report.name.clone().map(|name| ClusterSummary {
            name: Some(name),
            nodes_total: report.total_nodes,
            nodes_online: report.online_nodes,
            quorum: report.has_quorum,
        })
    };
    #[cfg(not(feature = "cluster"))]
    let cluster = None;
    #[cfg(feature = "cluster")]
    let ha = {
        let resources = state.ha_manager.list_resources().await;
        let mut by_state = BTreeMap::new();
        for r in &resources {
            *by_state.entry(format!("{:?}", r.state)).or_insert(0) += 1;
        }
        Some(HaSummary {
            enabled: state.ha_manager.is_enabled().await,
            resources: resources.len(),
            by_state,
        })
    };
    #[cfg(not(feature = "cluster"))]
    let ha = None;
    let uptime_secs = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(str::to_string))
        .and_then(|s| s.parse::<f64>().ok())
        .map(|v| v as u64);
    Json(NodeSummary {
        version: env!("CARGO_PKG_VERSION"),
        node,
        node_id: state.cluster_manager.local_node_id().to_string(),
        uptime_secs,
        vms,
        containers,
        cluster,
        ha,
    })
}
