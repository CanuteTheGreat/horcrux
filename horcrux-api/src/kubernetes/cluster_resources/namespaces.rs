//! Namespace operations

use crate::kubernetes::client::K8sClient;
#[cfg(not(feature = "kubernetes"))]
use crate::kubernetes::error::K8sError;
use crate::kubernetes::error::K8sResult;
use crate::kubernetes::types::{CreateNamespaceRequest, NamespaceInfo};

/// List all namespaces
#[cfg(feature = "kubernetes")]
pub async fn list_namespaces(client: &K8sClient) -> K8sResult<Vec<NamespaceInfo>> {
    use k8s_openapi::api::core::v1::Namespace;
    use kube::api::{Api, ListParams};

    let namespaces: Api<Namespace> = Api::all(client.inner().clone());
    let list = namespaces.list(&ListParams::default()).await?;

    Ok(list.items.into_iter().map(namespace_to_info).collect())
}

/// Get a namespace
#[cfg(feature = "kubernetes")]
pub async fn get_namespace(client: &K8sClient, name: &str) -> K8sResult<NamespaceInfo> {
    use k8s_openapi::api::core::v1::Namespace;
    use kube::api::Api;

    let namespaces: Api<Namespace> = Api::all(client.inner().clone());
    let ns = namespaces.get(name).await?;

    Ok(namespace_to_info(ns))
}

/// Create a namespace
#[cfg(feature = "kubernetes")]
pub async fn create_namespace(
    client: &K8sClient,
    request: &CreateNamespaceRequest,
) -> K8sResult<NamespaceInfo> {
    use k8s_openapi::api::core::v1::Namespace;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use kube::api::{Api, PostParams};

    let namespaces: Api<Namespace> = Api::all(client.inner().clone());

    let ns = Namespace {
        metadata: ObjectMeta {
            name: Some(request.name.clone()),
            labels: if request.labels.is_empty() {
                None
            } else {
                Some(request.labels.clone())
            },
            annotations: if request.annotations.is_empty() {
                None
            } else {
                Some(request.annotations.clone())
            },
            ..Default::default()
        },
        ..Default::default()
    };

    let created = namespaces.create(&PostParams::default(), &ns).await?;

    Ok(namespace_to_info(created))
}

/// Delete a namespace
#[cfg(feature = "kubernetes")]
pub async fn delete_namespace(client: &K8sClient, name: &str) -> K8sResult<()> {
    use k8s_openapi::api::core::v1::Namespace;
    use kube::api::{Api, DeleteParams};

    let namespaces: Api<Namespace> = Api::all(client.inner().clone());
    namespaces.delete(name, &DeleteParams::default()).await?;

    Ok(())
}

#[cfg(feature = "kubernetes")]
fn namespace_to_info(ns: k8s_openapi::api::core::v1::Namespace) -> NamespaceInfo {
    let metadata = ns.metadata;
    let status = ns.status.and_then(|s| s.phase).unwrap_or_default();

    NamespaceInfo {
        name: metadata.name.unwrap_or_default(),
        status,
        labels: metadata.labels.unwrap_or_default(),
        annotations: metadata.annotations.unwrap_or_default(),
        created_at: metadata.creation_timestamp.map(|t| t.0.to_rfc3339()),
    }
}

// Stubs
#[cfg(not(feature = "kubernetes"))]
pub async fn list_namespaces(_client: &K8sClient) -> K8sResult<Vec<NamespaceInfo>> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_namespace(_client: &K8sClient, _name: &str) -> K8sResult<NamespaceInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn create_namespace(
    _client: &K8sClient,
    _request: &CreateNamespaceRequest,
) -> K8sResult<NamespaceInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn delete_namespace(_client: &K8sClient, _name: &str) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}
