//! ResourceQuota and LimitRange operations
//!
//! CRUD operations for namespace resource limits.

use crate::kubernetes::client::K8sClient;
#[cfg(not(feature = "kubernetes"))]
use crate::kubernetes::error::K8sError;
use crate::kubernetes::error::K8sResult;
use crate::kubernetes::types::{
    CreateLimitRangeRequest, CreateResourceQuotaRequest, LimitRangeInfo, LimitRangeItem,
    ResourceQuotaInfo,
};

// ============================================================================
// ResourceQuota Operations
// ============================================================================

/// List ResourceQuotas in a namespace
#[cfg(feature = "kubernetes")]
pub async fn list_resource_quotas(
    client: &K8sClient,
    namespace: &str,
) -> K8sResult<Vec<ResourceQuotaInfo>> {
    use k8s_openapi::api::core::v1::ResourceQuota;
    use kube::api::{Api, ListParams};

    let quotas: Api<ResourceQuota> = Api::namespaced(client.inner().clone(), namespace);
    let list = quotas.list(&ListParams::default()).await?;

    Ok(list.items.into_iter().map(quota_to_info).collect())
}

/// Get a specific ResourceQuota
#[cfg(feature = "kubernetes")]
pub async fn get_resource_quota(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<ResourceQuotaInfo> {
    use k8s_openapi::api::core::v1::ResourceQuota;
    use kube::api::Api;

    let quotas: Api<ResourceQuota> = Api::namespaced(client.inner().clone(), namespace);
    let quota = quotas.get(name).await?;

    Ok(quota_to_info(quota))
}

/// Create a new ResourceQuota
#[cfg(feature = "kubernetes")]
pub async fn create_resource_quota(
    client: &K8sClient,
    request: &CreateResourceQuotaRequest,
) -> K8sResult<ResourceQuotaInfo> {
    use k8s_openapi::api::core::v1::{ResourceQuota, ResourceQuotaSpec};
    use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use kube::api::{Api, PostParams};

    let quotas: Api<ResourceQuota> = Api::namespaced(client.inner().clone(), &request.namespace);

    // Convert hard limits to Quantity
    let hard: std::collections::BTreeMap<String, Quantity> = request
        .hard
        .iter()
        .map(|(k, v)| (k.clone(), Quantity(v.clone())))
        .collect();

    let quota = ResourceQuota {
        metadata: ObjectMeta {
            name: Some(request.name.clone()),
            namespace: Some(request.namespace.clone()),
            labels: if request.labels.is_empty() {
                None
            } else {
                Some(request.labels.clone())
            },
            ..Default::default()
        },
        spec: Some(ResourceQuotaSpec {
            hard: Some(hard),
            ..Default::default()
        }),
        ..Default::default()
    };

    let created = quotas.create(&PostParams::default(), &quota).await?;
    Ok(quota_to_info(created))
}

/// Update a ResourceQuota
#[cfg(feature = "kubernetes")]
pub async fn update_resource_quota(
    client: &K8sClient,
    namespace: &str,
    name: &str,
    hard: &std::collections::BTreeMap<String, String>,
) -> K8sResult<ResourceQuotaInfo> {
    use k8s_openapi::api::core::v1::ResourceQuota;
    use kube::api::{Api, Patch, PatchParams};

    let quotas: Api<ResourceQuota> = Api::namespaced(client.inner().clone(), namespace);

    let patch = serde_json::json!({
        "spec": {
            "hard": hard
        }
    });

    let patched = quotas
        .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
        .await?;

    Ok(quota_to_info(patched))
}

/// Delete a ResourceQuota
#[cfg(feature = "kubernetes")]
pub async fn delete_resource_quota(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<()> {
    use k8s_openapi::api::core::v1::ResourceQuota;
    use kube::api::{Api, DeleteParams};

    let quotas: Api<ResourceQuota> = Api::namespaced(client.inner().clone(), namespace);
    quotas.delete(name, &DeleteParams::default()).await?;

    Ok(())
}

#[cfg(feature = "kubernetes")]
fn quota_to_info(quota: k8s_openapi::api::core::v1::ResourceQuota) -> ResourceQuotaInfo {
    let metadata = quota.metadata;
    let spec = quota.spec.unwrap_or_default();
    let status = quota.status.unwrap_or_default();

    // Convert hard limits
    let hard: std::collections::BTreeMap<String, String> = spec
        .hard
        .unwrap_or_default()
        .into_iter()
        .map(|(k, v)| (k, v.0))
        .collect();

    // Convert used
    let used: std::collections::BTreeMap<String, String> = status
        .used
        .unwrap_or_default()
        .into_iter()
        .map(|(k, v)| (k, v.0))
        .collect();

    ResourceQuotaInfo {
        name: metadata.name.unwrap_or_default(),
        namespace: metadata.namespace.unwrap_or_default(),
        hard,
        used,
        labels: metadata.labels.unwrap_or_default(),
        created_at: metadata.creation_timestamp.map(|t| t.0.to_rfc3339()),
    }
}

// ============================================================================
// LimitRange Operations
// ============================================================================

/// List LimitRanges in a namespace
#[cfg(feature = "kubernetes")]
pub async fn list_limit_ranges(
    client: &K8sClient,
    namespace: &str,
) -> K8sResult<Vec<LimitRangeInfo>> {
    use k8s_openapi::api::core::v1::LimitRange;
    use kube::api::{Api, ListParams};

    let ranges: Api<LimitRange> = Api::namespaced(client.inner().clone(), namespace);
    let list = ranges.list(&ListParams::default()).await?;

    Ok(list.items.into_iter().map(limit_range_to_info).collect())
}

/// Get a specific LimitRange
#[cfg(feature = "kubernetes")]
pub async fn get_limit_range(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<LimitRangeInfo> {
    use k8s_openapi::api::core::v1::LimitRange;
    use kube::api::Api;

    let ranges: Api<LimitRange> = Api::namespaced(client.inner().clone(), namespace);
    let range = ranges.get(name).await?;

    Ok(limit_range_to_info(range))
}

/// Create a new LimitRange
#[cfg(feature = "kubernetes")]
pub async fn create_limit_range(
    client: &K8sClient,
    request: &CreateLimitRangeRequest,
) -> K8sResult<LimitRangeInfo> {
    use k8s_openapi::api::core::v1::{LimitRange, LimitRangeItem as K8sLimitRangeItem, LimitRangeSpec};
    use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use kube::api::{Api, PostParams};

    let ranges: Api<LimitRange> = Api::namespaced(client.inner().clone(), &request.namespace);

    // Convert limits
    let limits: Vec<K8sLimitRangeItem> = request
        .limits
        .iter()
        .map(|l| {
            K8sLimitRangeItem {
                type_: l.limit_type.clone(),
                default: l.default.as_ref().map(|d| {
                    d.iter().map(|(k, v)| (k.clone(), Quantity(v.clone()))).collect()
                }),
                default_request: l.default_request.as_ref().map(|d| {
                    d.iter().map(|(k, v)| (k.clone(), Quantity(v.clone()))).collect()
                }),
                max: l.max.as_ref().map(|d| {
                    d.iter().map(|(k, v)| (k.clone(), Quantity(v.clone()))).collect()
                }),
                min: l.min.as_ref().map(|d| {
                    d.iter().map(|(k, v)| (k.clone(), Quantity(v.clone()))).collect()
                }),
                ..Default::default()
            }
        })
        .collect();

    let range = LimitRange {
        metadata: ObjectMeta {
            name: Some(request.name.clone()),
            namespace: Some(request.namespace.clone()),
            labels: if request.labels.is_empty() {
                None
            } else {
                Some(request.labels.clone())
            },
            ..Default::default()
        },
        spec: Some(LimitRangeSpec { limits }),
    };

    let created = ranges.create(&PostParams::default(), &range).await?;
    Ok(limit_range_to_info(created))
}

/// Delete a LimitRange
#[cfg(feature = "kubernetes")]
pub async fn delete_limit_range(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<()> {
    use k8s_openapi::api::core::v1::LimitRange;
    use kube::api::{Api, DeleteParams};

    let ranges: Api<LimitRange> = Api::namespaced(client.inner().clone(), namespace);
    ranges.delete(name, &DeleteParams::default()).await?;

    Ok(())
}

#[cfg(feature = "kubernetes")]
fn limit_range_to_info(range: k8s_openapi::api::core::v1::LimitRange) -> LimitRangeInfo {
    let metadata = range.metadata;
    let spec = range.spec.unwrap_or_default();

    let limits: Vec<LimitRangeItem> = spec
        .limits
        .into_iter()
        .map(|l| LimitRangeItem {
            limit_type: l.type_,
            default: l.default.map(|d| {
                d.into_iter().map(|(k, v)| (k, v.0)).collect()
            }),
            default_request: l.default_request.map(|d| {
                d.into_iter().map(|(k, v)| (k, v.0)).collect()
            }),
            max: l.max.map(|d| {
                d.into_iter().map(|(k, v)| (k, v.0)).collect()
            }),
            min: l.min.map(|d| {
                d.into_iter().map(|(k, v)| (k, v.0)).collect()
            }),
        })
        .collect();

    LimitRangeInfo {
        name: metadata.name.unwrap_or_default(),
        namespace: metadata.namespace.unwrap_or_default(),
        limits,
        labels: metadata.labels.unwrap_or_default(),
        created_at: metadata.creation_timestamp.map(|t| t.0.to_rfc3339()),
    }
}

// ============================================================================
// Stubs for when kubernetes feature is disabled
// ============================================================================

#[cfg(not(feature = "kubernetes"))]
pub async fn list_resource_quotas(
    _client: &K8sClient,
    _namespace: &str,
) -> K8sResult<Vec<ResourceQuotaInfo>> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_resource_quota(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<ResourceQuotaInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn create_resource_quota(
    _client: &K8sClient,
    _request: &CreateResourceQuotaRequest,
) -> K8sResult<ResourceQuotaInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn update_resource_quota(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
    _hard: &std::collections::BTreeMap<String, String>,
) -> K8sResult<ResourceQuotaInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn delete_resource_quota(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn list_limit_ranges(
    _client: &K8sClient,
    _namespace: &str,
) -> K8sResult<Vec<LimitRangeInfo>> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_limit_range(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<LimitRangeInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn create_limit_range(
    _client: &K8sClient,
    _request: &CreateLimitRangeRequest,
) -> K8sResult<LimitRangeInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn delete_limit_range(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}
