//! Container log streaming
//!
//! Fetch and stream container logs from Kubernetes pods.

use crate::kubernetes::client::K8sClient;
#[cfg(not(feature = "kubernetes"))]
use crate::kubernetes::error::K8sError;
use crate::kubernetes::error::K8sResult;
use crate::kubernetes::types::PodLogParams;

/// Get logs from a container in a pod
#[cfg(feature = "kubernetes")]
pub async fn get_pod_logs(
    client: &K8sClient,
    namespace: &str,
    pod_name: &str,
    params: &PodLogParams,
) -> K8sResult<String> {
    use k8s_openapi::api::core::v1::Pod;
    use kube::api::{Api, LogParams};

    let pods: Api<Pod> = Api::namespaced(client.inner().clone(), namespace);

    let mut log_params = LogParams::default();

    if let Some(container) = &params.container {
        log_params.container = Some(container.clone());
    }

    log_params.follow = params.follow;

    if let Some(tail) = params.tail_lines {
        log_params.tail_lines = Some(tail);
    }

    log_params.timestamps = params.timestamps;

    if let Some(since) = params.since_seconds {
        log_params.since_seconds = Some(since);
    }

    if let Some(limit) = params.limit_bytes {
        log_params.limit_bytes = Some(limit);
    }

    let logs = pods.logs(pod_name, &log_params).await?;

    Ok(logs)
}

/// Get logs from a specific container
#[cfg(feature = "kubernetes")]
pub async fn get_container_logs(
    client: &K8sClient,
    namespace: &str,
    pod_name: &str,
    container_name: &str,
    tail_lines: Option<i64>,
    timestamps: bool,
) -> K8sResult<String> {
    let params = PodLogParams {
        container: Some(container_name.to_string()),
        follow: false,
        tail_lines,
        timestamps,
        since_time: None,
        since_seconds: None,
        limit_bytes: None,
    };

    get_pod_logs(client, namespace, pod_name, &params).await
}

/// Get previous container logs (from crashed/restarted container)
#[cfg(feature = "kubernetes")]
pub async fn get_previous_logs(
    client: &K8sClient,
    namespace: &str,
    pod_name: &str,
    container_name: Option<&str>,
    tail_lines: Option<i64>,
) -> K8sResult<String> {
    use k8s_openapi::api::core::v1::Pod;
    use kube::api::{Api, LogParams};

    let pods: Api<Pod> = Api::namespaced(client.inner().clone(), namespace);

    let mut log_params = LogParams {
        previous: true,
        ..Default::default()
    };

    if let Some(container) = container_name {
        log_params.container = Some(container.to_string());
    }

    if let Some(tail) = tail_lines {
        log_params.tail_lines = Some(tail);
    }

    let logs = pods.logs(pod_name, &log_params).await?;

    Ok(logs)
}

/// Get a log stream reader for a container (returns AsyncBufRead for streaming)
/// Use this for WebSocket-based log streaming
#[cfg(feature = "kubernetes")]
pub async fn get_log_stream(
    client: &K8sClient,
    namespace: &str,
    pod_name: &str,
    params: &PodLogParams,
) -> K8sResult<impl futures::io::AsyncBufRead> {
    use k8s_openapi::api::core::v1::Pod;
    use kube::api::{Api, LogParams};

    let pods: Api<Pod> = Api::namespaced(client.inner().clone(), namespace);

    let mut log_params = LogParams {
        follow: true, // Always follow for streaming
        ..Default::default()
    };

    if let Some(container) = &params.container {
        log_params.container = Some(container.clone());
    }

    if let Some(tail) = params.tail_lines {
        log_params.tail_lines = Some(tail);
    }

    log_params.timestamps = params.timestamps;

    if let Some(since) = params.since_seconds {
        log_params.since_seconds = Some(since);
    }

    let stream = pods.log_stream(pod_name, &log_params).await?;

    Ok(stream)
}

/// Get logs from all containers in a pod
#[cfg(feature = "kubernetes")]
pub async fn get_all_container_logs(
    client: &K8sClient,
    namespace: &str,
    pod_name: &str,
    tail_lines: Option<i64>,
    timestamps: bool,
) -> K8sResult<std::collections::HashMap<String, String>> {
    use k8s_openapi::api::core::v1::Pod;
    use kube::api::Api;
    use std::collections::HashMap;

    let pods: Api<Pod> = Api::namespaced(client.inner().clone(), namespace);
    let pod = pods.get(pod_name).await?;

    let mut all_logs = HashMap::new();

    // Get container names from spec
    if let Some(spec) = pod.spec {
        // Regular containers
        for container in spec.containers {
            let container_name = container.name;
            match get_container_logs(
                client,
                namespace,
                pod_name,
                &container_name,
                tail_lines,
                timestamps,
            )
            .await
            {
                Ok(logs) => {
                    all_logs.insert(container_name, logs);
                }
                Err(e) => {
                    all_logs.insert(container_name, format!("Error fetching logs: {}", e));
                }
            }
        }

        // Init containers
        if let Some(init_containers) = spec.init_containers {
            for container in init_containers {
                let container_name = format!("init:{}", container.name);
                match get_container_logs(
                    client,
                    namespace,
                    pod_name,
                    &container.name,
                    tail_lines,
                    timestamps,
                )
                .await
                {
                    Ok(logs) => {
                        all_logs.insert(container_name, logs);
                    }
                    Err(e) => {
                        all_logs.insert(container_name, format!("Error fetching logs: {}", e));
                    }
                }
            }
        }
    }

    Ok(all_logs)
}

// ============================================================================
// Stubs for when kubernetes feature is disabled
// ============================================================================

#[cfg(not(feature = "kubernetes"))]
pub async fn get_pod_logs(
    _client: &K8sClient,
    _namespace: &str,
    _pod_name: &str,
    _params: &PodLogParams,
) -> K8sResult<String> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_container_logs(
    _client: &K8sClient,
    _namespace: &str,
    _pod_name: &str,
    _container_name: &str,
    _tail_lines: Option<i64>,
    _timestamps: bool,
) -> K8sResult<String> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_previous_logs(
    _client: &K8sClient,
    _namespace: &str,
    _pod_name: &str,
    _container_name: Option<&str>,
    _tail_lines: Option<i64>,
) -> K8sResult<String> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_all_container_logs(
    _client: &K8sClient,
    _namespace: &str,
    _pod_name: &str,
    _tail_lines: Option<i64>,
    _timestamps: bool,
) -> K8sResult<std::collections::HashMap<String, String>> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}
