//! K8s Secrets operations
//!
//! CRUD operations for Kubernetes Secrets.
//! Note: Secret values are not exposed in responses for security.

use crate::kubernetes::client::K8sClient;
#[cfg(not(feature = "kubernetes"))]
use crate::kubernetes::error::K8sError;
use crate::kubernetes::error::K8sResult;
use crate::kubernetes::types::{CreateSecretRequest, SecretInfo};

/// List Secrets in a namespace
#[cfg(feature = "kubernetes")]
pub async fn list_secrets(
    client: &K8sClient,
    namespace: &str,
) -> K8sResult<Vec<SecretInfo>> {
    use k8s_openapi::api::core::v1::Secret;
    use kube::api::{Api, ListParams};

    let secrets: Api<Secret> = Api::namespaced(client.inner().clone(), namespace);
    let list = secrets.list(&ListParams::default()).await?;

    Ok(list.items.into_iter().map(secret_to_info).collect())
}

/// Get a specific Secret (metadata only, not values)
#[cfg(feature = "kubernetes")]
pub async fn get_secret(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<SecretInfo> {
    use k8s_openapi::api::core::v1::Secret;
    use kube::api::Api;

    let secrets: Api<Secret> = Api::namespaced(client.inner().clone(), namespace);
    let secret = secrets.get(name).await?;

    Ok(secret_to_info(secret))
}

/// Create a new Secret
#[cfg(feature = "kubernetes")]
pub async fn create_secret(
    client: &K8sClient,
    request: &CreateSecretRequest,
) -> K8sResult<SecretInfo> {
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use k8s_openapi::ByteString;
    use kube::api::{Api, PostParams};

    let secrets: Api<Secret> = Api::namespaced(client.inner().clone(), &request.namespace);

    // Convert data to ByteString (base64 encoded values)
    let data: Option<std::collections::BTreeMap<String, ByteString>> = if request.data.is_empty() {
        None
    } else {
        Some(
            request
                .data
                .iter()
                .map(|(k, v)| {
                    // Data values should already be base64 encoded
                    (k.clone(), ByteString(v.as_bytes().to_vec()))
                })
                .collect(),
        )
    };

    // string_data is for plaintext values (K8s will encode them)
    let string_data: Option<std::collections::BTreeMap<String, String>> =
        if request.string_data.is_empty() {
            None
        } else {
            Some(request.string_data.clone())
        };

    let secret = Secret {
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
        type_: request.secret_type.clone(),
        data,
        string_data,
        immutable: None,
    };

    let created = secrets.create(&PostParams::default(), &secret).await?;
    Ok(secret_to_info(created))
}

/// Delete a Secret
#[cfg(feature = "kubernetes")]
pub async fn delete_secret(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<()> {
    use k8s_openapi::api::core::v1::Secret;
    use kube::api::{Api, DeleteParams};

    let secrets: Api<Secret> = Api::namespaced(client.inner().clone(), namespace);
    secrets.delete(name, &DeleteParams::default()).await?;

    Ok(())
}

#[cfg(feature = "kubernetes")]
fn secret_to_info(secret: k8s_openapi::api::core::v1::Secret) -> SecretInfo {
    let metadata = secret.metadata;

    // Get data keys (not values for security)
    let data_keys: Vec<String> = secret
        .data
        .map(|d| d.keys().cloned().collect())
        .unwrap_or_default();

    // Get secret type
    let secret_type = secret.type_.unwrap_or_else(|| "Opaque".to_string());

    SecretInfo {
        name: metadata.name.unwrap_or_default(),
        namespace: metadata.namespace.unwrap_or_default(),
        secret_type,
        data_keys,
        labels: metadata.labels.unwrap_or_default(),
        annotations: metadata.annotations.unwrap_or_default(),
        created_at: metadata.creation_timestamp.map(|t| t.0.to_rfc3339()),
    }
}

// Stubs for when kubernetes feature is disabled
#[cfg(not(feature = "kubernetes"))]
pub async fn list_secrets(
    _client: &K8sClient,
    _namespace: &str,
) -> K8sResult<Vec<SecretInfo>> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_secret(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<SecretInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn create_secret(
    _client: &K8sClient,
    _request: &CreateSecretRequest,
) -> K8sResult<SecretInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn delete_secret(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}
