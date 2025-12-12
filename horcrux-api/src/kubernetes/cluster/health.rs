//! Cluster health checks
//!
//! Provides detailed health status for Kubernetes clusters.

use crate::kubernetes::client::K8sClient;
#[cfg(not(feature = "kubernetes"))]
use crate::kubernetes::error::K8sError;
use crate::kubernetes::error::K8sResult;
use crate::kubernetes::types::{
    ClusterHealth, ComponentHealth, HealthStatus, NodeHealthSummary,
};

/// Perform a comprehensive health check on a cluster
#[cfg(feature = "kubernetes")]
pub async fn check_health(client: &K8sClient) -> K8sResult<ClusterHealth> {
    use k8s_openapi::api::core::v1::{ComponentStatus, Node};
    use kube::api::Api;

    let kube_client = client.inner();

    // Check API server (implicitly tested by other calls)
    let api_server_healthy = true;

    // Get component statuses
    let components_api: Api<ComponentStatus> = Api::all(kube_client.clone());
    let component_list = components_api.list(&Default::default()).await;

    let (controller_manager_healthy, scheduler_healthy, etcd_healthy, components) =
        match component_list {
            Ok(list) => {
                let mut cm_healthy = false;
                let mut sched_healthy = false;
                let mut etcd_healthy = false;
                let mut components = Vec::new();

                for cs in list.items {
                    let name = cs.metadata.name.unwrap_or_default();
                    let conditions = cs.conditions.unwrap_or_default();
                    let healthy = conditions
                        .iter()
                        .any(|c| c.type_ == "Healthy" && c.status == "True");

                    if name.contains("controller-manager") {
                        cm_healthy = healthy;
                    } else if name.contains("scheduler") {
                        sched_healthy = healthy;
                    } else if name.contains("etcd") {
                        etcd_healthy = healthy || etcd_healthy; // Any etcd healthy is good
                    }

                    let message = conditions.first().and_then(|c| c.message.clone());

                    components.push(ComponentHealth {
                        name,
                        status: if healthy {
                            HealthStatus::Healthy
                        } else {
                            HealthStatus::Unhealthy
                        },
                        message,
                    });
                }

                (cm_healthy, sched_healthy, etcd_healthy, components)
            }
            Err(e) => {
                tracing::warn!("Failed to get component statuses: {}", e);
                // Component status API may not be available in all clusters
                (true, true, true, Vec::new())
            }
        };

    // Get node health
    let nodes_api: Api<Node> = Api::all(kube_client.clone());
    let node_list = nodes_api.list(&Default::default()).await?;

    let mut total_nodes = 0;
    let mut ready_nodes = 0;
    let mut not_ready_nodes = 0;

    for node in node_list.items {
        total_nodes += 1;

        let conditions = node
            .status
            .and_then(|s| s.conditions)
            .unwrap_or_default();

        let is_ready = conditions
            .iter()
            .any(|c| c.type_ == "Ready" && c.status == "True");

        if is_ready {
            ready_nodes += 1;
        } else {
            not_ready_nodes += 1;
        }
    }

    // Determine overall status
    let status = if ready_nodes == total_nodes
        && controller_manager_healthy
        && scheduler_healthy
        && etcd_healthy
    {
        HealthStatus::Healthy
    } else if ready_nodes > 0 {
        HealthStatus::Degraded
    } else {
        HealthStatus::Unhealthy
    };

    Ok(ClusterHealth {
        status,
        api_server_healthy,
        controller_manager_healthy,
        scheduler_healthy,
        etcd_healthy,
        nodes: NodeHealthSummary {
            total: total_nodes,
            ready: ready_nodes,
            not_ready: not_ready_nodes,
        },
        components,
    })
}

/// Stub implementation when kubernetes feature is disabled
#[cfg(not(feature = "kubernetes"))]
pub async fn check_health(_client: &K8sClient) -> K8sResult<ClusterHealth> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}
