//! Prometheus `/metrics`: the exporter's gauges are filled from the live
//! sources at scrape time (node usage from the cluster sampler - a real CPU
//! delta and cgroup-aware memory -, per-VM CPU/RAM from the metrics
//! collector, storage pools, VM counts, cluster nodes), and every HTTP
//! request is counted by method, route pattern and status.
use crate::AppState;
use axum::{
    extract::{MatchedPath, Request, State},
    middleware::Next,
    response::Response,
};
use std::sync::Arc;

/// Re-fill the scrape-time gauges from the current state.
pub async fn refresh(state: &AppState) {
    let p = &state.prometheus_manager;

    // This node.
    let node = state
        .cluster_manager
        .get_local_node_name()
        .await
        .unwrap_or_else(|_| "localhost".to_string());
    if let Some(u) = state.cluster_manager.local_usage().await {
        let uptime = std::fs::read_to_string("/proc/uptime")
            .ok()
            .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok())
            .unwrap_or(0.0) as u64;
        p.update_node_metrics(&node, u.cpu_usage, u.memory_used, uptime)
            .await;
    }

    // Cluster nodes.
    let nodes = state.cluster_manager.list_nodes().await;
    let online = nodes.iter().filter(|n| n.is_online()).count();
    p.update_cluster_metrics(nodes.len(), online).await;

    // VMs: counts, and CPU/RAM of the ones the collector has figures for
    // (series of deleted/stopped VMs are dropped).
    let vms = state.vm_manager.list_vms().await;
    let running = vms
        .iter()
        .filter(|v| v.status == horcrux_common::VmStatus::Running)
        .count();
    p.update_vm_counts(vms.len(), running).await;
    let reg = p.registry();
    reg.reset("horcrux_vm_cpu_usage_percent").await;
    reg.reset("horcrux_vm_memory_usage_bytes").await;
    for m in state.monitoring_manager.list_vm_metrics().await {
        if vms.iter().any(|v| v.id == m.id) {
            p.update_vm_metrics(
                &m.id,
                &m.name,
                m.cpu.usage_percent,
                m.memory.used_bytes,
                "running",
            )
            .await;
        }
    }

    // Storage pools.
    reg.reset("horcrux_storage_total_bytes").await;
    reg.reset("horcrux_storage_used_bytes").await;
    for s in state.monitoring_manager.list_storage_metrics().await {
        p.update_storage_metrics(&s.name, &s.name, s.total_bytes, s.used_bytes)
            .await;
    }
}

/// Count every request in `horcrux_http_requests_total`, labelled with the
/// route pattern (`/api/vms/:id`, not the concrete path, so the series
/// count stays bounded); requests that matched no route are `unmatched`.
pub async fn count_http_requests(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method().to_string();
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "unmatched".to_string());
    let response = next.run(request).await;
    state
        .prometheus_manager
        .inc_http_requests(&method, &path, response.status().as_u16())
        .await;
    response
}
