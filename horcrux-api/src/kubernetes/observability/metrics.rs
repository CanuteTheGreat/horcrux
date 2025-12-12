//! Metrics from metrics-server
//!
//! Fetches resource metrics for nodes and pods from the Kubernetes metrics-server.

use crate::kubernetes::client::K8sClient;
#[cfg(not(feature = "kubernetes"))]
use crate::kubernetes::error::K8sError;
use crate::kubernetes::error::K8sResult;
use crate::kubernetes::types::{ContainerMetrics, NodeMetrics, PodMetrics};

/// Get metrics for all nodes
#[cfg(feature = "kubernetes")]
pub async fn get_node_metrics(client: &K8sClient) -> K8sResult<Vec<NodeMetrics>> {
    // Build the request for metrics API using http crate
    let request = http::Request::builder()
        .method(http::Method::GET)
        .uri("/apis/metrics.k8s.io/v1beta1/nodes")
        .body(vec![])
        .map_err(|e| crate::kubernetes::error::K8sError::Internal(e.to_string()))?;

    let response: serde_json::Value = client
        .inner()
        .request(request)
        .await
        .map_err(|e| crate::kubernetes::error::K8sError::KubeError(e))?;

    // Parse the response
    let items = response["items"].as_array().cloned().unwrap_or_default();

    let metrics: Vec<NodeMetrics> = items
        .into_iter()
        .filter_map(|item| {
            let name = item["metadata"]["name"].as_str()?.to_string();
            let usage = &item["usage"];
            let cpu = usage["cpu"].as_str().unwrap_or("0").to_string();
            let memory = usage["memory"].as_str().unwrap_or("0").to_string();
            let timestamp = item["timestamp"].as_str().unwrap_or("").to_string();

            Some(NodeMetrics {
                name,
                cpu_usage: cpu,
                memory_usage: memory,
                timestamp,
            })
        })
        .collect();

    Ok(metrics)
}

/// Get metrics for a specific node
#[cfg(feature = "kubernetes")]
pub async fn get_node_metric(client: &K8sClient, name: &str) -> K8sResult<NodeMetrics> {
    let request = http::Request::builder()
        .method(http::Method::GET)
        .uri(format!("/apis/metrics.k8s.io/v1beta1/nodes/{}", name))
        .body(vec![])
        .map_err(|e| crate::kubernetes::error::K8sError::Internal(e.to_string()))?;

    let response: serde_json::Value = client
        .inner()
        .request(request)
        .await
        .map_err(|e| crate::kubernetes::error::K8sError::KubeError(e))?;

    let usage = &response["usage"];
    let cpu = usage["cpu"].as_str().unwrap_or("0").to_string();
    let memory = usage["memory"].as_str().unwrap_or("0").to_string();
    let timestamp = response["timestamp"].as_str().unwrap_or("").to_string();

    Ok(NodeMetrics {
        name: name.to_string(),
        cpu_usage: cpu,
        memory_usage: memory,
        timestamp,
    })
}

/// Get metrics for all pods in a namespace
#[cfg(feature = "kubernetes")]
pub async fn get_pod_metrics(client: &K8sClient, namespace: &str) -> K8sResult<Vec<PodMetrics>> {
    let request = http::Request::builder()
        .method(http::Method::GET)
        .uri(format!(
            "/apis/metrics.k8s.io/v1beta1/namespaces/{}/pods",
            namespace
        ))
        .body(vec![])
        .map_err(|e| crate::kubernetes::error::K8sError::Internal(e.to_string()))?;

    let response: serde_json::Value = client
        .inner()
        .request(request)
        .await
        .map_err(|e| crate::kubernetes::error::K8sError::KubeError(e))?;

    let items = response["items"].as_array().cloned().unwrap_or_default();

    let metrics: Vec<PodMetrics> = items
        .into_iter()
        .filter_map(|item| {
            let name = item["metadata"]["name"].as_str()?.to_string();
            let namespace = item["metadata"]["namespace"].as_str()?.to_string();
            let timestamp = item["timestamp"].as_str().unwrap_or("").to_string();

            let containers: Vec<ContainerMetrics> = item["containers"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|c| {
                    let name = c["name"].as_str()?.to_string();
                    let usage = &c["usage"];
                    let cpu = usage["cpu"].as_str().unwrap_or("0").to_string();
                    let memory = usage["memory"].as_str().unwrap_or("0").to_string();

                    Some(ContainerMetrics {
                        name,
                        cpu_usage: cpu,
                        memory_usage: memory,
                    })
                })
                .collect();

            Some(PodMetrics {
                name,
                namespace,
                containers,
                timestamp,
            })
        })
        .collect();

    Ok(metrics)
}

/// Get metrics for a specific pod
#[cfg(feature = "kubernetes")]
pub async fn get_pod_metric(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<PodMetrics> {
    let request = http::Request::builder()
        .method(http::Method::GET)
        .uri(format!(
            "/apis/metrics.k8s.io/v1beta1/namespaces/{}/pods/{}",
            namespace, name
        ))
        .body(vec![])
        .map_err(|e| crate::kubernetes::error::K8sError::Internal(e.to_string()))?;

    let response: serde_json::Value = client
        .inner()
        .request(request)
        .await
        .map_err(|e| crate::kubernetes::error::K8sError::KubeError(e))?;

    let timestamp = response["timestamp"].as_str().unwrap_or("").to_string();

    let containers: Vec<ContainerMetrics> = response["containers"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|c| {
            let name = c["name"].as_str()?.to_string();
            let usage = &c["usage"];
            let cpu = usage["cpu"].as_str().unwrap_or("0").to_string();
            let memory = usage["memory"].as_str().unwrap_or("0").to_string();

            Some(ContainerMetrics {
                name,
                cpu_usage: cpu,
                memory_usage: memory,
            })
        })
        .collect();

    Ok(PodMetrics {
        name: name.to_string(),
        namespace: namespace.to_string(),
        containers,
        timestamp,
    })
}

/// Get all pod metrics across all namespaces
#[cfg(feature = "kubernetes")]
pub async fn get_all_pod_metrics(client: &K8sClient) -> K8sResult<Vec<PodMetrics>> {
    let request = http::Request::builder()
        .method(http::Method::GET)
        .uri("/apis/metrics.k8s.io/v1beta1/pods")
        .body(vec![])
        .map_err(|e| crate::kubernetes::error::K8sError::Internal(e.to_string()))?;

    let response: serde_json::Value = client
        .inner()
        .request(request)
        .await
        .map_err(|e| crate::kubernetes::error::K8sError::KubeError(e))?;

    let items = response["items"].as_array().cloned().unwrap_or_default();

    let metrics: Vec<PodMetrics> = items
        .into_iter()
        .filter_map(|item| {
            let name = item["metadata"]["name"].as_str()?.to_string();
            let namespace = item["metadata"]["namespace"].as_str()?.to_string();
            let timestamp = item["timestamp"].as_str().unwrap_or("").to_string();

            let containers: Vec<ContainerMetrics> = item["containers"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|c| {
                    let name = c["name"].as_str()?.to_string();
                    let usage = &c["usage"];
                    let cpu = usage["cpu"].as_str().unwrap_or("0").to_string();
                    let memory = usage["memory"].as_str().unwrap_or("0").to_string();

                    Some(ContainerMetrics {
                        name,
                        cpu_usage: cpu,
                        memory_usage: memory,
                    })
                })
                .collect();

            Some(PodMetrics {
                name,
                namespace,
                containers,
                timestamp,
            })
        })
        .collect();

    Ok(metrics)
}

// Stubs for when kubernetes feature is disabled
#[cfg(not(feature = "kubernetes"))]
pub async fn get_node_metrics(_client: &K8sClient) -> K8sResult<Vec<NodeMetrics>> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_node_metric(_client: &K8sClient, _name: &str) -> K8sResult<NodeMetrics> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_pod_metrics(_client: &K8sClient, _namespace: &str) -> K8sResult<Vec<PodMetrics>> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_pod_metric(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<PodMetrics> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_all_pod_metrics(_client: &K8sClient) -> K8sResult<Vec<PodMetrics>> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}
