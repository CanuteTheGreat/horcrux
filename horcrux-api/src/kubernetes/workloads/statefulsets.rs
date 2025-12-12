//! StatefulSet operations
//!
//! CRUD operations for Kubernetes StatefulSets.

use crate::kubernetes::client::K8sClient;
#[cfg(not(feature = "kubernetes"))]
use crate::kubernetes::error::K8sError;
use crate::kubernetes::error::K8sResult;
use crate::kubernetes::types::StatefulSetInfo;

/// List StatefulSets in a namespace
#[cfg(feature = "kubernetes")]
pub async fn list_statefulsets(
    client: &K8sClient,
    namespace: &str,
) -> K8sResult<Vec<StatefulSetInfo>> {
    use k8s_openapi::api::apps::v1::StatefulSet;
    use kube::api::{Api, ListParams};

    let sts: Api<StatefulSet> = Api::namespaced(client.inner().clone(), namespace);
    let list = sts.list(&ListParams::default()).await?;

    Ok(list.items.into_iter().map(statefulset_to_info).collect())
}

/// Get a specific StatefulSet
#[cfg(feature = "kubernetes")]
pub async fn get_statefulset(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<StatefulSetInfo> {
    use k8s_openapi::api::apps::v1::StatefulSet;
    use kube::api::Api;

    let sts: Api<StatefulSet> = Api::namespaced(client.inner().clone(), namespace);
    let statefulset = sts.get(name).await?;

    Ok(statefulset_to_info(statefulset))
}

/// Scale a StatefulSet
#[cfg(feature = "kubernetes")]
pub async fn scale_statefulset(
    client: &K8sClient,
    namespace: &str,
    name: &str,
    replicas: i32,
) -> K8sResult<StatefulSetInfo> {
    use k8s_openapi::api::apps::v1::StatefulSet;
    use kube::api::{Api, Patch, PatchParams};

    let sts: Api<StatefulSet> = Api::namespaced(client.inner().clone(), namespace);

    let patch = serde_json::json!({
        "spec": {
            "replicas": replicas
        }
    });

    let patched = sts
        .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
        .await?;

    Ok(statefulset_to_info(patched))
}

/// Delete a StatefulSet
#[cfg(feature = "kubernetes")]
pub async fn delete_statefulset(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<()> {
    use k8s_openapi::api::apps::v1::StatefulSet;
    use kube::api::{Api, DeleteParams};

    let sts: Api<StatefulSet> = Api::namespaced(client.inner().clone(), namespace);
    sts.delete(name, &DeleteParams::default()).await?;

    Ok(())
}

/// Restart a StatefulSet by triggering a rollout
#[cfg(feature = "kubernetes")]
pub async fn restart_statefulset(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<StatefulSetInfo> {
    use k8s_openapi::api::apps::v1::StatefulSet;
    use kube::api::{Api, Patch, PatchParams};

    let sts: Api<StatefulSet> = Api::namespaced(client.inner().clone(), namespace);

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

    let patched = sts
        .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
        .await?;

    Ok(statefulset_to_info(patched))
}

#[cfg(feature = "kubernetes")]
fn statefulset_to_info(sts: k8s_openapi::api::apps::v1::StatefulSet) -> StatefulSetInfo {
    let metadata = sts.metadata;
    let spec = sts.spec.unwrap_or_default();
    let status = sts.status.unwrap_or_default();

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

    // Get pod management policy
    let pod_management_policy = spec
        .pod_management_policy
        .unwrap_or_else(|| "OrderedReady".to_string());

    StatefulSetInfo {
        name: metadata.name.unwrap_or_default(),
        namespace: metadata.namespace.unwrap_or_default(),
        replicas: spec.replicas.unwrap_or(1),
        ready_replicas: status.ready_replicas.unwrap_or(0),
        current_replicas: status.current_replicas.unwrap_or(0),
        updated_replicas: status.updated_replicas.unwrap_or(0),
        labels: metadata.labels.unwrap_or_default(),
        selector,
        service_name: spec.service_name,
        pod_management_policy,
        update_strategy,
        created_at: metadata.creation_timestamp.map(|t| t.0.to_rfc3339()),
    }
}

// Stubs for when kubernetes feature is disabled
#[cfg(not(feature = "kubernetes"))]
pub async fn list_statefulsets(
    _client: &K8sClient,
    _namespace: &str,
) -> K8sResult<Vec<StatefulSetInfo>> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_statefulset(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<StatefulSetInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn scale_statefulset(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
    _replicas: i32,
) -> K8sResult<StatefulSetInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn delete_statefulset(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn restart_statefulset(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<StatefulSetInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}
