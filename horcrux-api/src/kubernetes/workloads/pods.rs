//! Pod operations
//!
//! CRUD operations for Kubernetes Pods, plus logs and exec.

use crate::kubernetes::client::K8sClient;
#[cfg(not(feature = "kubernetes"))]
use crate::kubernetes::error::K8sError;
use crate::kubernetes::error::K8sResult;
use crate::kubernetes::types::{ContainerInfo, ContainerState, PodInfo, PodStatus};

/// List pods in a namespace
#[cfg(feature = "kubernetes")]
pub async fn list_pods(
    client: &K8sClient,
    namespace: &str,
    label_selector: Option<&str>,
) -> K8sResult<Vec<PodInfo>> {
    use k8s_openapi::api::core::v1::Pod;
    use kube::api::{Api, ListParams};

    let pods: Api<Pod> = Api::namespaced(client.inner().clone(), namespace);

    let mut lp = ListParams::default();
    if let Some(selector) = label_selector {
        lp = lp.labels(selector);
    }

    let pod_list = pods.list(&lp).await?;

    Ok(pod_list.items.into_iter().map(pod_to_info).collect())
}

/// Get a single pod
#[cfg(feature = "kubernetes")]
pub async fn get_pod(client: &K8sClient, namespace: &str, name: &str) -> K8sResult<PodInfo> {
    use k8s_openapi::api::core::v1::Pod;
    use kube::api::Api;

    let pods: Api<Pod> = Api::namespaced(client.inner().clone(), namespace);
    let pod = pods.get(name).await?;

    Ok(pod_to_info(pod))
}

/// Delete a pod
#[cfg(feature = "kubernetes")]
pub async fn delete_pod(client: &K8sClient, namespace: &str, name: &str) -> K8sResult<()> {
    use k8s_openapi::api::core::v1::Pod;
    use kube::api::{Api, DeleteParams};

    let pods: Api<Pod> = Api::namespaced(client.inner().clone(), namespace);
    pods.delete(name, &DeleteParams::default()).await?;

    Ok(())
}

/// Get pod logs
#[cfg(feature = "kubernetes")]
pub async fn get_pod_logs(
    client: &K8sClient,
    namespace: &str,
    name: &str,
    container: Option<&str>,
    tail_lines: Option<i64>,
    timestamps: bool,
) -> K8sResult<String> {
    use k8s_openapi::api::core::v1::Pod;
    use kube::api::{Api, LogParams};

    let pods: Api<Pod> = Api::namespaced(client.inner().clone(), namespace);

    let mut lp = LogParams::default();
    if let Some(c) = container {
        lp.container = Some(c.to_string());
    }
    if let Some(tail) = tail_lines {
        lp.tail_lines = Some(tail);
    }
    lp.timestamps = timestamps;

    let logs = pods.logs(name, &lp).await?;

    Ok(logs)
}

/// Convert k8s Pod to PodInfo
#[cfg(feature = "kubernetes")]
fn pod_to_info(pod: k8s_openapi::api::core::v1::Pod) -> PodInfo {
    let metadata = pod.metadata;
    let spec = pod.spec.unwrap_or_default();
    let status = pod.status.unwrap_or_default();

    let pod_status = match status.phase.as_deref() {
        Some("Pending") => PodStatus::Pending,
        Some("Running") => PodStatus::Running,
        Some("Succeeded") => PodStatus::Succeeded,
        Some("Failed") => PodStatus::Failed,
        _ => PodStatus::Unknown,
    };

    let container_statuses = status.container_statuses.unwrap_or_default();
    let containers: Vec<ContainerInfo> = spec
        .containers
        .iter()
        .map(|c| {
            let cs = container_statuses
                .iter()
                .find(|cs| cs.name == c.name);

            let (ready, restart_count, state) = if let Some(cs) = cs {
                let state = if let Some(ref s) = cs.state {
                    if let Some(ref waiting) = s.waiting {
                        ContainerState::Waiting {
                            reason: waiting.reason.clone(),
                        }
                    } else if let Some(ref running) = s.running {
                        ContainerState::Running {
                            started_at: running.started_at.as_ref().map(|t| t.0.to_rfc3339()),
                        }
                    } else if let Some(ref terminated) = s.terminated {
                        ContainerState::Terminated {
                            exit_code: terminated.exit_code,
                            reason: terminated.reason.clone(),
                        }
                    } else {
                        ContainerState::Unknown
                    }
                } else {
                    ContainerState::Unknown
                };

                (cs.ready, cs.restart_count, state)
            } else {
                (false, 0, ContainerState::Unknown)
            };

            ContainerInfo {
                name: c.name.clone(),
                image: c.image.clone().unwrap_or_default(),
                ready,
                restart_count,
                state,
            }
        })
        .collect();

    let restart_count: i32 = containers.iter().map(|c| c.restart_count).sum();

    PodInfo {
        name: metadata.name.unwrap_or_default(),
        namespace: metadata.namespace.unwrap_or_default(),
        status: pod_status,
        node_name: spec.node_name,
        pod_ip: status.pod_ip,
        host_ip: status.host_ip,
        containers,
        labels: metadata.labels.unwrap_or_default(),
        annotations: metadata.annotations.unwrap_or_default(),
        created_at: metadata
            .creation_timestamp
            .map(|t| t.0.to_rfc3339()),
        restart_count,
    }
}

// Stub implementations when kubernetes feature is disabled
#[cfg(not(feature = "kubernetes"))]
pub async fn list_pods(
    _client: &K8sClient,
    _namespace: &str,
    _label_selector: Option<&str>,
) -> K8sResult<Vec<PodInfo>> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_pod(_client: &K8sClient, _namespace: &str, _name: &str) -> K8sResult<PodInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn delete_pod(_client: &K8sClient, _namespace: &str, _name: &str) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_pod_logs(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
    _container: Option<&str>,
    _tail_lines: Option<i64>,
    _timestamps: bool,
) -> K8sResult<String> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}
