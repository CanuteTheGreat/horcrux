//! ConfigMap operations
//!
//! CRUD operations for Kubernetes ConfigMaps.

use crate::kubernetes::client::K8sClient;
#[cfg(not(feature = "kubernetes"))]
use crate::kubernetes::error::K8sError;
use crate::kubernetes::error::K8sResult;
use crate::kubernetes::types::{ConfigMapInfo, CreateConfigMapRequest, UpdateConfigMapRequest};

/// List ConfigMaps in a namespace
#[cfg(feature = "kubernetes")]
pub async fn list_configmaps(client: &K8sClient, namespace: &str) -> K8sResult<Vec<ConfigMapInfo>> {
    use k8s_openapi::api::core::v1::ConfigMap;
    use kube::api::{Api, ListParams};

    let configmaps: Api<ConfigMap> = Api::namespaced(client.inner().clone(), namespace);
    let list = configmaps.list(&ListParams::default()).await?;

    Ok(list.items.into_iter().map(configmap_to_info).collect())
}

/// Get a specific ConfigMap
#[cfg(feature = "kubernetes")]
pub async fn get_configmap(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<ConfigMapInfo> {
    use k8s_openapi::api::core::v1::ConfigMap;
    use kube::api::Api;

    let configmaps: Api<ConfigMap> = Api::namespaced(client.inner().clone(), namespace);
    let configmap = configmaps.get(name).await?;

    Ok(configmap_to_info(configmap))
}

/// Create a new ConfigMap
#[cfg(feature = "kubernetes")]
pub async fn create_configmap(
    client: &K8sClient,
    request: &CreateConfigMapRequest,
) -> K8sResult<ConfigMapInfo> {
    use k8s_openapi::api::core::v1::ConfigMap;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use kube::api::{Api, PostParams};

    let configmaps: Api<ConfigMap> = Api::namespaced(client.inner().clone(), &request.namespace);

    let configmap = ConfigMap {
        metadata: ObjectMeta {
            name: Some(request.name.clone()),
            namespace: Some(request.namespace.clone()),
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
        data: if request.data.is_empty() {
            None
        } else {
            Some(request.data.clone())
        },
        binary_data: None,
        immutable: None,
    };

    let created = configmaps
        .create(&PostParams::default(), &configmap)
        .await?;
    Ok(configmap_to_info(created))
}

/// Update a ConfigMap
#[cfg(feature = "kubernetes")]
pub async fn update_configmap(
    client: &K8sClient,
    namespace: &str,
    name: &str,
    request: &UpdateConfigMapRequest,
) -> K8sResult<ConfigMapInfo> {
    use k8s_openapi::api::core::v1::ConfigMap;
    use kube::api::{Api, Patch, PatchParams};

    let configmaps: Api<ConfigMap> = Api::namespaced(client.inner().clone(), namespace);

    let mut patch = serde_json::json!({});

    if !request.data.is_empty() {
        patch["data"] = serde_json::json!(request.data);
    }

    if !request.labels.is_empty() || !request.annotations.is_empty() {
        let mut metadata = serde_json::json!({});
        if !request.labels.is_empty() {
            metadata["labels"] = serde_json::json!(request.labels);
        }
        if !request.annotations.is_empty() {
            metadata["annotations"] = serde_json::json!(request.annotations);
        }
        patch["metadata"] = metadata;
    }

    let patched = configmaps
        .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
        .await?;

    Ok(configmap_to_info(patched))
}

/// Replace a ConfigMap's data/labels/annotations with exactly the given
/// maps (what the web UI's edit dialog shows): keys that were removed in the
/// dialog are removed from the ConfigMap, unlike [`update_configmap`]'s
/// additive merge.
#[cfg(feature = "kubernetes")]
pub async fn replace_configmap(
    client: &K8sClient,
    namespace: &str,
    name: &str,
    request: &UpdateConfigMapRequest,
) -> K8sResult<ConfigMapInfo> {
    use k8s_openapi::api::core::v1::ConfigMap;
    use kube::api::{Api, Patch, PatchParams};

    let configmaps: Api<ConfigMap> = Api::namespaced(client.inner().clone(), namespace);
    let current = configmaps.get(name).await?;
    let patch = serde_json::json!({
        "data": replace_map(current.data.as_ref(), &request.data),
        "metadata": {
            "labels": replace_map(current.metadata.labels.as_ref(), &request.labels),
            "annotations": replace_map(current.metadata.annotations.as_ref(), &request.annotations),
        },
    });
    let patched = configmaps
        .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
        .await?;
    Ok(configmap_to_info(patched))
}

/// JSON merge patch turning `current` into `wanted` (removed keys -> null).
pub fn replace_map(
    current: Option<&std::collections::BTreeMap<String, String>>,
    wanted: &std::collections::BTreeMap<String, String>,
) -> serde_json::Value {
    let mut patch = serde_json::Map::new();
    if let Some(cur) = current {
        for k in cur.keys() {
            if !wanted.contains_key(k) {
                patch.insert(k.clone(), serde_json::Value::Null);
            }
        }
    }
    for (k, v) in wanted {
        patch.insert(k.clone(), serde_json::Value::String(v.clone()));
    }
    serde_json::Value::Object(patch)
}

/// Delete a ConfigMap
#[cfg(feature = "kubernetes")]
pub async fn delete_configmap(client: &K8sClient, namespace: &str, name: &str) -> K8sResult<()> {
    use k8s_openapi::api::core::v1::ConfigMap;
    use kube::api::{Api, DeleteParams};

    let configmaps: Api<ConfigMap> = Api::namespaced(client.inner().clone(), namespace);
    configmaps.delete(name, &DeleteParams::default()).await?;

    Ok(())
}

#[cfg(feature = "kubernetes")]
fn configmap_to_info(configmap: k8s_openapi::api::core::v1::ConfigMap) -> ConfigMapInfo {
    let metadata = configmap.metadata;

    // Get data keys
    let data = configmap.data.unwrap_or_default();

    // Get binary data keys (we only expose the keys, not the values)
    let binary_data_keys: Vec<String> = configmap
        .binary_data
        .map(|bd| bd.keys().cloned().collect())
        .unwrap_or_default();

    ConfigMapInfo {
        name: metadata.name.unwrap_or_default(),
        namespace: metadata.namespace.unwrap_or_default(),
        data,
        binary_data_keys,
        labels: metadata.labels.unwrap_or_default(),
        annotations: metadata.annotations.unwrap_or_default(),
        created_at: metadata.creation_timestamp.map(|t| t.0.to_rfc3339()),
    }
}

// Stubs for when kubernetes feature is disabled
#[cfg(not(feature = "kubernetes"))]
pub async fn list_configmaps(
    _client: &K8sClient,
    _namespace: &str,
) -> K8sResult<Vec<ConfigMapInfo>> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_configmap(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<ConfigMapInfo> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn create_configmap(
    _client: &K8sClient,
    _request: &CreateConfigMapRequest,
) -> K8sResult<ConfigMapInfo> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn update_configmap(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
    _request: &UpdateConfigMapRequest,
) -> K8sResult<ConfigMapInfo> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn delete_configmap(_client: &K8sClient, _namespace: &str, _name: &str) -> K8sResult<()> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}
