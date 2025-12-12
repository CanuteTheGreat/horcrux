//! StorageClass operations
//!
//! CRUD operations for Kubernetes StorageClasses.
//! Bridges to existing Horcrux storage backends.

use crate::kubernetes::client::K8sClient;
#[cfg(not(feature = "kubernetes"))]
use crate::kubernetes::error::K8sError;
use crate::kubernetes::error::K8sResult;
use crate::kubernetes::types::{CreateStorageClassRequest, StorageClassInfo};

/// List all StorageClasses
#[cfg(feature = "kubernetes")]
pub async fn list_storage_classes(client: &K8sClient) -> K8sResult<Vec<StorageClassInfo>> {
    use k8s_openapi::api::storage::v1::StorageClass;
    use kube::api::{Api, ListParams};

    let scs: Api<StorageClass> = Api::all(client.inner().clone());
    let list = scs.list(&ListParams::default()).await?;

    Ok(list.items.into_iter().map(storage_class_to_info).collect())
}

/// Get a specific StorageClass
#[cfg(feature = "kubernetes")]
pub async fn get_storage_class(client: &K8sClient, name: &str) -> K8sResult<StorageClassInfo> {
    use k8s_openapi::api::storage::v1::StorageClass;
    use kube::api::Api;

    let scs: Api<StorageClass> = Api::all(client.inner().clone());
    let sc = scs.get(name).await?;

    Ok(storage_class_to_info(sc))
}

/// Create a new StorageClass
#[cfg(feature = "kubernetes")]
pub async fn create_storage_class(
    client: &K8sClient,
    request: &CreateStorageClassRequest,
) -> K8sResult<StorageClassInfo> {
    use k8s_openapi::api::storage::v1::StorageClass;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use kube::api::{Api, PostParams};

    let scs: Api<StorageClass> = Api::all(client.inner().clone());

    // Build annotations for default storage class
    let mut annotations = std::collections::BTreeMap::new();
    if request.is_default {
        annotations.insert(
            "storageclass.kubernetes.io/is-default-class".to_string(),
            "true".to_string(),
        );
    }

    let sc = StorageClass {
        metadata: ObjectMeta {
            name: Some(request.name.clone()),
            labels: if request.labels.is_empty() {
                None
            } else {
                Some(request.labels.clone())
            },
            annotations: if annotations.is_empty() {
                None
            } else {
                Some(annotations)
            },
            ..Default::default()
        },
        provisioner: request.provisioner.clone(),
        reclaim_policy: request.reclaim_policy.clone(),
        volume_binding_mode: request.volume_binding_mode.clone(),
        allow_volume_expansion: Some(request.allow_volume_expansion),
        parameters: if request.parameters.is_empty() {
            None
        } else {
            Some(request.parameters.clone())
        },
        ..Default::default()
    };

    let created = scs.create(&PostParams::default(), &sc).await?;
    Ok(storage_class_to_info(created))
}

/// Delete a StorageClass
#[cfg(feature = "kubernetes")]
pub async fn delete_storage_class(client: &K8sClient, name: &str) -> K8sResult<()> {
    use k8s_openapi::api::storage::v1::StorageClass;
    use kube::api::{Api, DeleteParams};

    let scs: Api<StorageClass> = Api::all(client.inner().clone());
    scs.delete(name, &DeleteParams::default()).await?;

    Ok(())
}

/// Set a StorageClass as default
#[cfg(feature = "kubernetes")]
pub async fn set_default_storage_class(client: &K8sClient, name: &str) -> K8sResult<()> {
    use k8s_openapi::api::storage::v1::StorageClass;
    use kube::api::{Api, ListParams, Patch, PatchParams};

    let scs: Api<StorageClass> = Api::all(client.inner().clone());

    // First, remove default annotation from all other storage classes
    let all_scs = scs.list(&ListParams::default()).await?;
    for sc in all_scs.items {
        let sc_name = sc.metadata.name.unwrap_or_default();
        if sc_name != name {
            if let Some(annotations) = sc.metadata.annotations {
                if annotations
                    .get("storageclass.kubernetes.io/is-default-class")
                    .map(|v| v == "true")
                    .unwrap_or(false)
                {
                    let patch = serde_json::json!({
                        "metadata": {
                            "annotations": {
                                "storageclass.kubernetes.io/is-default-class": "false"
                            }
                        }
                    });
                    scs.patch(&sc_name, &PatchParams::default(), &Patch::Merge(&patch))
                        .await?;
                }
            }
        }
    }

    // Set the new default
    let patch = serde_json::json!({
        "metadata": {
            "annotations": {
                "storageclass.kubernetes.io/is-default-class": "true"
            }
        }
    });
    scs.patch(name, &PatchParams::default(), &Patch::Merge(&patch))
        .await?;

    Ok(())
}

#[cfg(feature = "kubernetes")]
fn storage_class_to_info(sc: k8s_openapi::api::storage::v1::StorageClass) -> StorageClassInfo {
    let metadata = sc.metadata;

    // Check if this is the default storage class
    let is_default = metadata
        .annotations
        .as_ref()
        .and_then(|a| a.get("storageclass.kubernetes.io/is-default-class"))
        .map(|v| v == "true")
        .unwrap_or(false);

    StorageClassInfo {
        name: metadata.name.unwrap_or_default(),
        provisioner: sc.provisioner,
        reclaim_policy: sc.reclaim_policy,
        volume_binding_mode: sc.volume_binding_mode,
        allow_volume_expansion: sc.allow_volume_expansion.unwrap_or(false),
        parameters: sc.parameters.unwrap_or_default(),
        labels: metadata.labels.unwrap_or_default(),
        is_default,
        created_at: metadata.creation_timestamp.map(|t| t.0.to_rfc3339()),
    }
}

// Stubs for when kubernetes feature is disabled
#[cfg(not(feature = "kubernetes"))]
pub async fn list_storage_classes(_client: &K8sClient) -> K8sResult<Vec<StorageClassInfo>> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_storage_class(_client: &K8sClient, _name: &str) -> K8sResult<StorageClassInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn create_storage_class(
    _client: &K8sClient,
    _request: &CreateStorageClassRequest,
) -> K8sResult<StorageClassInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn delete_storage_class(_client: &K8sClient, _name: &str) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn set_default_storage_class(_client: &K8sClient, _name: &str) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}
