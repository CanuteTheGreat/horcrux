//! DaemonSet operations
//!
//! CRUD operations for Kubernetes DaemonSets.

use crate::kubernetes::client::K8sClient;
#[cfg(not(feature = "kubernetes"))]
use crate::kubernetes::error::K8sError;
use crate::kubernetes::error::K8sResult;
use crate::kubernetes::types::DaemonSetInfo;

/// List DaemonSets in a namespace
#[cfg(feature = "kubernetes")]
pub async fn list_daemonsets(
    client: &K8sClient,
    namespace: &str,
) -> K8sResult<Vec<DaemonSetInfo>> {
    use k8s_openapi::api::apps::v1::DaemonSet;
    use kube::api::{Api, ListParams};

    let ds: Api<DaemonSet> = Api::namespaced(client.inner().clone(), namespace);
    let list = ds.list(&ListParams::default()).await?;

    Ok(list.items.into_iter().map(daemonset_to_info).collect())
}

/// Get a specific DaemonSet
#[cfg(feature = "kubernetes")]
pub async fn get_daemonset(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<DaemonSetInfo> {
    use k8s_openapi::api::apps::v1::DaemonSet;
    use kube::api::Api;

    let ds: Api<DaemonSet> = Api::namespaced(client.inner().clone(), namespace);
    let daemonset = ds.get(name).await?;

    Ok(daemonset_to_info(daemonset))
}

/// Delete a DaemonSet
#[cfg(feature = "kubernetes")]
pub async fn delete_daemonset(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<()> {
    use k8s_openapi::api::apps::v1::DaemonSet;
    use kube::api::{Api, DeleteParams};

    let ds: Api<DaemonSet> = Api::namespaced(client.inner().clone(), namespace);
    ds.delete(name, &DeleteParams::default()).await?;

    Ok(())
}

/// Restart a DaemonSet by triggering a rollout
#[cfg(feature = "kubernetes")]
pub async fn restart_daemonset(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<DaemonSetInfo> {
    use k8s_openapi::api::apps::v1::DaemonSet;
    use kube::api::{Api, Patch, PatchParams};

    let ds: Api<DaemonSet> = Api::namespaced(client.inner().clone(), namespace);

    // Add/update restart annotation to trigger rollout
    let now = chrono::Utc::now().to_rfc3339();
    let patch = serde_json::json!({
        "spec": {
            "template": {
                "metadata": {
                    "annotations": {
                        "kubectl.kubernetes.io/restartedAt": now
                    }
                }
            }
        }
    });

    let patched = ds
        .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
        .await?;

    Ok(daemonset_to_info(patched))
}

#[cfg(feature = "kubernetes")]
fn daemonset_to_info(ds: k8s_openapi::api::apps::v1::DaemonSet) -> DaemonSetInfo {
    let metadata = ds.metadata;
    let spec = ds.spec.unwrap_or_default();
    let status = ds.status.unwrap_or_default();

    // Get selector labels
    let selector = spec
        .selector
        .match_labels
        .unwrap_or_default();

    // Get update strategy
    let update_strategy = spec
        .update_strategy
        .and_then(|s| s.type_)
        .unwrap_or_else(|| "RollingUpdate".to_string());

    DaemonSetInfo {
        name: metadata.name.unwrap_or_default(),
        namespace: metadata.namespace.unwrap_or_default(),
        desired_number_scheduled: status.desired_number_scheduled,
        current_number_scheduled: status.current_number_scheduled,
        number_ready: status.number_ready,
        number_available: status.number_available.unwrap_or(0),
        number_misscheduled: status.number_misscheduled,
        labels: metadata.labels.unwrap_or_default(),
        selector,
        update_strategy,
        created_at: metadata.creation_timestamp.map(|t| t.0.to_rfc3339()),
    }
}

// Stubs for when kubernetes feature is disabled
#[cfg(not(feature = "kubernetes"))]
pub async fn list_daemonsets(
    _client: &K8sClient,
    _namespace: &str,
) -> K8sResult<Vec<DaemonSetInfo>> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_daemonset(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<DaemonSetInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn delete_daemonset(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn restart_daemonset(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<DaemonSetInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}
