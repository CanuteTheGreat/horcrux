//! Node operations
//!
//! List nodes, cordon, uncordon, and drain.

use crate::kubernetes::client::K8sClient;
#[cfg(not(feature = "kubernetes"))]
use crate::kubernetes::error::K8sError;
use crate::kubernetes::error::K8sResult;
use crate::kubernetes::types::{NodeCondition, NodeInfo, NodeStatus};

/// List all nodes
#[cfg(feature = "kubernetes")]
pub async fn list_nodes(client: &K8sClient) -> K8sResult<Vec<NodeInfo>> {
    use k8s_openapi::api::core::v1::Node;
    use kube::api::{Api, ListParams};

    let nodes: Api<Node> = Api::all(client.inner().clone());
    let list = nodes.list(&ListParams::default()).await?;

    Ok(list.items.into_iter().map(node_to_info).collect())
}

/// Get a node
#[cfg(feature = "kubernetes")]
pub async fn get_node(client: &K8sClient, name: &str) -> K8sResult<NodeInfo> {
    use k8s_openapi::api::core::v1::Node;
    use kube::api::Api;

    let nodes: Api<Node> = Api::all(client.inner().clone());
    let node = nodes.get(name).await?;

    Ok(node_to_info(node))
}

/// Cordon a node (mark as unschedulable)
#[cfg(feature = "kubernetes")]
pub async fn cordon_node(client: &K8sClient, name: &str) -> K8sResult<()> {
    use k8s_openapi::api::core::v1::Node;
    use kube::api::{Api, Patch, PatchParams};

    let nodes: Api<Node> = Api::all(client.inner().clone());

    let patch = serde_json::json!({
        "spec": {
            "unschedulable": true
        }
    });

    nodes
        .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
        .await?;

    Ok(())
}

/// Uncordon a node (mark as schedulable)
#[cfg(feature = "kubernetes")]
pub async fn uncordon_node(client: &K8sClient, name: &str) -> K8sResult<()> {
    use k8s_openapi::api::core::v1::Node;
    use kube::api::{Api, Patch, PatchParams};

    let nodes: Api<Node> = Api::all(client.inner().clone());

    let patch = serde_json::json!({
        "spec": {
            "unschedulable": false
        }
    });

    nodes
        .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
        .await?;

    Ok(())
}

/// Drain a node (evict all pods)
#[cfg(feature = "kubernetes")]
pub async fn drain_node(
    client: &K8sClient,
    name: &str,
    ignore_daemonsets: bool,
    delete_emptydir_data: bool,
    grace_period_seconds: Option<i64>,
) -> K8sResult<()> {
    use k8s_openapi::api::core::v1::Pod;
    use kube::api::{Api, DeleteParams, EvictParams, ListParams};

    // First cordon the node
    cordon_node(client, name).await?;

    // List pods on this node
    let pods: Api<Pod> = Api::all(client.inner().clone());
    let field_selector = format!("spec.nodeName={}", name);
    let pod_list = pods
        .list(&ListParams::default().fields(&field_selector))
        .await?;

    // Evict each pod
    for pod in pod_list.items {
        let pod_name = pod.metadata.name.clone().unwrap_or_default();
        let pod_namespace = pod.metadata.namespace.clone().unwrap_or_default();

        // Skip mirror pods (managed by kubelet)
        if let Some(annotations) = &pod.metadata.annotations {
            if annotations.contains_key("kubernetes.io/config.mirror") {
                continue;
            }
        }

        // Skip DaemonSet pods if requested
        if ignore_daemonsets {
            if let Some(refs) = &pod.metadata.owner_references {
                if refs.iter().any(|r| r.kind == "DaemonSet") {
                    continue;
                }
            }
        }

        // Check for local storage
        if !delete_emptydir_data {
            if let Some(spec) = &pod.spec {
                if let Some(volumes) = &spec.volumes {
                    let has_emptydir = volumes.iter().any(|v| v.empty_dir.is_some());
                    if has_emptydir {
                        tracing::warn!(
                            "Skipping pod {} with emptyDir volume",
                            pod_name
                        );
                        continue;
                    }
                }
            }
        }

        // Create eviction params with optional grace period
        let delete_params = if let Some(grace) = grace_period_seconds {
            DeleteParams {
                grace_period_seconds: Some(grace as u32),
                ..Default::default()
            }
        } else {
            DeleteParams::default()
        };
        let evict_params = EvictParams {
            delete_options: Some(delete_params),
            ..Default::default()
        };

        let pod_api: Api<Pod> = Api::namespaced(client.inner().clone(), &pod_namespace);

        if let Err(e) = pod_api.evict(&pod_name, &evict_params).await {
            tracing::warn!("Failed to evict pod {}: {}", pod_name, e);
        }
    }

    Ok(())
}

#[cfg(feature = "kubernetes")]
fn node_to_info(node: k8s_openapi::api::core::v1::Node) -> NodeInfo {
    let metadata = node.metadata;
    let _spec = node.spec.unwrap_or_default();
    let status = node.status.unwrap_or_default();

    // Determine node status from conditions
    let conditions = status.conditions.unwrap_or_default();
    let is_ready = conditions
        .iter()
        .any(|c| c.type_ == "Ready" && c.status == "True");

    let node_status = if is_ready {
        NodeStatus::Ready
    } else {
        NodeStatus::NotReady
    };

    // Get roles from labels
    let labels = metadata.labels.clone().unwrap_or_default();
    let roles: Vec<String> = labels
        .keys()
        .filter(|k| k.starts_with("node-role.kubernetes.io/"))
        .map(|k| k.trim_start_matches("node-role.kubernetes.io/").to_string())
        .collect();

    // Get IPs
    let addresses = status.addresses.unwrap_or_default();
    let internal_ip = addresses
        .iter()
        .find(|a| a.type_ == "InternalIP")
        .map(|a| a.address.clone());
    let external_ip = addresses
        .iter()
        .find(|a| a.type_ == "ExternalIP")
        .map(|a| a.address.clone());

    // Get node info
    let node_info = status.node_info.unwrap_or_default();

    // Get allocatable resources
    let allocatable = status.allocatable.unwrap_or_default();
    let allocatable_cpu = allocatable
        .get("cpu")
        .map(|q| q.0.clone())
        .unwrap_or_default();
    let allocatable_memory = allocatable
        .get("memory")
        .map(|q| q.0.clone())
        .unwrap_or_default();

    let node_conditions: Vec<NodeCondition> = conditions
        .into_iter()
        .map(|c| NodeCondition {
            condition_type: c.type_,
            status: c.status,
            reason: c.reason,
            message: c.message,
        })
        .collect();

    NodeInfo {
        name: metadata.name.unwrap_or_default(),
        status: node_status,
        roles,
        internal_ip,
        external_ip,
        os_image: node_info.os_image,
        kernel_version: node_info.kernel_version,
        container_runtime: node_info.container_runtime_version,
        kubelet_version: node_info.kubelet_version,
        allocatable_cpu,
        allocatable_memory,
        conditions: node_conditions,
        created_at: metadata.creation_timestamp.map(|t| t.0.to_rfc3339()),
    }
}

// Stubs
#[cfg(not(feature = "kubernetes"))]
pub async fn list_nodes(_client: &K8sClient) -> K8sResult<Vec<NodeInfo>> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_node(_client: &K8sClient, _name: &str) -> K8sResult<NodeInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn cordon_node(_client: &K8sClient, _name: &str) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn uncordon_node(_client: &K8sClient, _name: &str) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn drain_node(
    _client: &K8sClient,
    _name: &str,
    _ignore_daemonsets: bool,
    _delete_emptydir_data: bool,
    _grace_period_seconds: Option<i64>,
) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}
