//! Deployment operations
//!
//! CRUD operations for Kubernetes Deployments, including scaling and rollouts.

use crate::kubernetes::client::K8sClient;
use crate::kubernetes::error::{K8sError, K8sResult};
use crate::kubernetes::types::DeploymentInfo;

/// List deployments in a namespace
#[cfg(feature = "kubernetes")]
pub async fn list_deployments(
    client: &K8sClient,
    namespace: &str,
) -> K8sResult<Vec<DeploymentInfo>> {
    use k8s_openapi::api::apps::v1::Deployment;
    use kube::api::{Api, ListParams};

    let deployments: Api<Deployment> = Api::namespaced(client.inner().clone(), namespace);
    let list = deployments.list(&ListParams::default()).await?;

    Ok(list.items.into_iter().map(deployment_to_info).collect())
}

/// Get a single deployment
#[cfg(feature = "kubernetes")]
pub async fn get_deployment(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<DeploymentInfo> {
    use k8s_openapi::api::apps::v1::Deployment;
    use kube::api::Api;

    let deployments: Api<Deployment> = Api::namespaced(client.inner().clone(), namespace);
    let deployment = deployments.get(name).await?;

    Ok(deployment_to_info(deployment))
}

/// Scale a deployment
#[cfg(feature = "kubernetes")]
pub async fn scale_deployment(
    client: &K8sClient,
    namespace: &str,
    name: &str,
    replicas: i32,
) -> K8sResult<DeploymentInfo> {
    use k8s_openapi::api::apps::v1::Deployment;
    use kube::api::{Api, Patch, PatchParams};

    let deployments: Api<Deployment> = Api::namespaced(client.inner().clone(), namespace);

    let patch = serde_json::json!({
        "spec": {
            "replicas": replicas
        }
    });

    let deployment = deployments
        .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
        .await?;

    Ok(deployment_to_info(deployment))
}

/// Restart a deployment by updating its annotation
#[cfg(feature = "kubernetes")]
pub async fn restart_deployment(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<()> {
    use k8s_openapi::api::apps::v1::Deployment;
    use kube::api::{Api, Patch, PatchParams};

    let deployments: Api<Deployment> = Api::namespaced(client.inner().clone(), namespace);

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

    deployments
        .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
        .await?;

    Ok(())
}

/// Delete a deployment
#[cfg(feature = "kubernetes")]
pub async fn delete_deployment(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<()> {
    use k8s_openapi::api::apps::v1::Deployment;
    use kube::api::{Api, DeleteParams};

    let deployments: Api<Deployment> = Api::namespaced(client.inner().clone(), namespace);
    deployments.delete(name, &DeleteParams::default()).await?;

    Ok(())
}

/// Rollback a deployment to a previous revision
#[cfg(feature = "kubernetes")]
pub async fn rollback_deployment(
    client: &K8sClient,
    namespace: &str,
    name: &str,
    revision: Option<i64>,
) -> K8sResult<()> {
    use k8s_openapi::api::apps::v1::{Deployment, ReplicaSet};
    use kube::api::{Api, ListParams, Patch, PatchParams};

    let deployments: Api<Deployment> = Api::namespaced(client.inner().clone(), namespace);
    let replicasets: Api<ReplicaSet> = Api::namespaced(client.inner().clone(), namespace);

    // Get the deployment
    let deployment = deployments.get(name).await?;
    let labels = deployment
        .spec
        .as_ref()
        .and_then(|s| s.selector.match_labels.clone())
        .unwrap_or_default();

    // Build label selector
    let selector = labels
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join(",");

    // List replicasets for this deployment
    let rs_list = replicasets
        .list(&ListParams::default().labels(&selector))
        .await?;

    // Sort by revision annotation
    let mut rs_with_revision: Vec<_> = rs_list
        .items
        .into_iter()
        .filter_map(|rs| {
            let rev = rs
                .metadata
                .annotations
                .as_ref()
                .and_then(|a| a.get("deployment.kubernetes.io/revision"))
                .and_then(|r| r.parse::<i64>().ok())?;
            Some((rev, rs))
        })
        .collect();

    rs_with_revision.sort_by(|a, b| b.0.cmp(&a.0));

    // Find target revision
    let target_rs = if let Some(rev) = revision {
        rs_with_revision.iter().find(|(r, _)| *r == rev)
    } else {
        // Get previous revision (second most recent)
        rs_with_revision.get(1)
    };

    let (_, target_rs) = target_rs.ok_or_else(|| {
        K8sError::Internal("No previous revision found for rollback".to_string())
    })?;

    // Get the template from the target replicaset
    let template = target_rs
        .spec
        .as_ref()
        .and_then(|s| s.template.clone())
        .ok_or_else(|| K8sError::Internal("No template in replicaset".to_string()))?;

    // Patch the deployment with the old template
    let patch = serde_json::json!({
        "spec": {
            "template": template
        }
    });

    deployments
        .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
        .await?;

    Ok(())
}

/// Convert k8s Deployment to DeploymentInfo
#[cfg(feature = "kubernetes")]
fn deployment_to_info(deployment: k8s_openapi::api::apps::v1::Deployment) -> DeploymentInfo {
    let metadata = deployment.metadata;
    let spec = deployment.spec.unwrap_or_default();
    let status = deployment.status.unwrap_or_default();

    let selector = spec
        .selector
        .match_labels
        .unwrap_or_default();

    let strategy = spec
        .strategy
        .and_then(|s| s.type_)
        .unwrap_or_else(|| "RollingUpdate".to_string());

    DeploymentInfo {
        name: metadata.name.unwrap_or_default(),
        namespace: metadata.namespace.unwrap_or_default(),
        replicas: spec.replicas.unwrap_or(0),
        ready_replicas: status.ready_replicas.unwrap_or(0),
        available_replicas: status.available_replicas.unwrap_or(0),
        updated_replicas: status.updated_replicas.unwrap_or(0),
        labels: metadata.labels.unwrap_or_default(),
        selector,
        strategy,
        created_at: metadata
            .creation_timestamp
            .map(|t| t.0.to_rfc3339()),
    }
}

// Stub implementations when kubernetes feature is disabled
#[cfg(not(feature = "kubernetes"))]
pub async fn list_deployments(
    _client: &K8sClient,
    _namespace: &str,
) -> K8sResult<Vec<DeploymentInfo>> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_deployment(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<DeploymentInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn scale_deployment(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
    _replicas: i32,
) -> K8sResult<DeploymentInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn restart_deployment(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn delete_deployment(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn rollback_deployment(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
    _revision: Option<i64>,
) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}
