//! PersistentVolumeClaim and PersistentVolume operations
//!
//! CRUD operations for PVCs and PVs.

use crate::kubernetes::client::K8sClient;
#[cfg(not(feature = "kubernetes"))]
use crate::kubernetes::error::K8sError;
use crate::kubernetes::error::K8sResult;
use crate::kubernetes::types::{CreatePvcRequest, CreatePvRequest, PvClaimRef, PvcInfo, PvInfo};

// ============================================================================
// PersistentVolumeClaim Operations
// ============================================================================

/// List PVCs in a namespace
#[cfg(feature = "kubernetes")]
pub async fn list_pvcs(client: &K8sClient, namespace: &str) -> K8sResult<Vec<PvcInfo>> {
    use k8s_openapi::api::core::v1::PersistentVolumeClaim;
    use kube::api::{Api, ListParams};

    let pvcs: Api<PersistentVolumeClaim> = Api::namespaced(client.inner().clone(), namespace);
    let list = pvcs.list(&ListParams::default()).await?;

    Ok(list.items.into_iter().map(pvc_to_info).collect())
}

/// Get a specific PVC
#[cfg(feature = "kubernetes")]
pub async fn get_pvc(client: &K8sClient, namespace: &str, name: &str) -> K8sResult<PvcInfo> {
    use k8s_openapi::api::core::v1::PersistentVolumeClaim;
    use kube::api::Api;

    let pvcs: Api<PersistentVolumeClaim> = Api::namespaced(client.inner().clone(), namespace);
    let pvc = pvcs.get(name).await?;

    Ok(pvc_to_info(pvc))
}

/// Create a new PVC
#[cfg(feature = "kubernetes")]
pub async fn create_pvc(client: &K8sClient, request: &CreatePvcRequest) -> K8sResult<PvcInfo> {
    use k8s_openapi::api::core::v1::{
        PersistentVolumeClaim, PersistentVolumeClaimSpec, VolumeResourceRequirements,
    };
    use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
    use kube::api::{Api, PostParams};
    use std::collections::BTreeMap;

    let pvcs: Api<PersistentVolumeClaim> =
        Api::namespaced(client.inner().clone(), &request.namespace);

    // Build resources
    let mut requests = BTreeMap::new();
    requests.insert("storage".to_string(), Quantity(request.storage.clone()));

    let pvc = PersistentVolumeClaim {
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
        spec: Some(PersistentVolumeClaimSpec {
            access_modes: Some(request.access_modes.clone()),
            storage_class_name: request.storage_class.clone(),
            resources: Some(VolumeResourceRequirements {
                requests: Some(requests),
                limits: None,
            }),
            volume_mode: request.volume_mode.clone(),
            selector: request.selector.as_ref().map(|sel| LabelSelector {
                match_labels: if sel.is_empty() {
                    None
                } else {
                    Some(sel.clone())
                },
                match_expressions: None,
            }),
            ..Default::default()
        }),
        ..Default::default()
    };

    let created = pvcs.create(&PostParams::default(), &pvc).await?;
    Ok(pvc_to_info(created))
}

/// Delete a PVC
#[cfg(feature = "kubernetes")]
pub async fn delete_pvc(client: &K8sClient, namespace: &str, name: &str) -> K8sResult<()> {
    use k8s_openapi::api::core::v1::PersistentVolumeClaim;
    use kube::api::{Api, DeleteParams};

    let pvcs: Api<PersistentVolumeClaim> = Api::namespaced(client.inner().clone(), namespace);
    pvcs.delete(name, &DeleteParams::default()).await?;

    Ok(())
}

#[cfg(feature = "kubernetes")]
fn pvc_to_info(pvc: k8s_openapi::api::core::v1::PersistentVolumeClaim) -> PvcInfo {
    let metadata = pvc.metadata;
    let spec = pvc.spec.unwrap_or_default();
    let status = pvc.status.unwrap_or_default();

    // Get capacity from status or spec
    let capacity = status
        .capacity
        .and_then(|c| c.get("storage").map(|q| q.0.clone()));

    let requested_capacity = spec
        .resources
        .and_then(|r| r.requests)
        .and_then(|r| r.get("storage").map(|q| q.0.clone()));

    PvcInfo {
        name: metadata.name.unwrap_or_default(),
        namespace: metadata.namespace.unwrap_or_default(),
        status: status.phase.unwrap_or_else(|| "Unknown".to_string()),
        volume_name: spec.volume_name,
        storage_class: spec.storage_class_name,
        access_modes: status.access_modes.or(spec.access_modes).unwrap_or_default(),
        capacity,
        requested_capacity,
        labels: metadata.labels.unwrap_or_default(),
        created_at: metadata.creation_timestamp.map(|t| t.0.to_rfc3339()),
    }
}

// ============================================================================
// PersistentVolume Operations
// ============================================================================

/// List all PVs (cluster-scoped)
#[cfg(feature = "kubernetes")]
pub async fn list_pvs(client: &K8sClient) -> K8sResult<Vec<PvInfo>> {
    use k8s_openapi::api::core::v1::PersistentVolume;
    use kube::api::{Api, ListParams};

    let pvs: Api<PersistentVolume> = Api::all(client.inner().clone());
    let list = pvs.list(&ListParams::default()).await?;

    Ok(list.items.into_iter().map(pv_to_info).collect())
}

/// Get a specific PV
#[cfg(feature = "kubernetes")]
pub async fn get_pv(client: &K8sClient, name: &str) -> K8sResult<PvInfo> {
    use k8s_openapi::api::core::v1::PersistentVolume;
    use kube::api::Api;

    let pvs: Api<PersistentVolume> = Api::all(client.inner().clone());
    let pv = pvs.get(name).await?;

    Ok(pv_to_info(pv))
}

/// Create a new PV
#[cfg(feature = "kubernetes")]
pub async fn create_pv(client: &K8sClient, request: &CreatePvRequest) -> K8sResult<PvInfo> {
    use k8s_openapi::api::core::v1::{
        HostPathVolumeSource, NFSVolumeSource, PersistentVolume, PersistentVolumeSpec,
    };
    use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use kube::api::{Api, PostParams};
    use std::collections::BTreeMap;

    let pvs: Api<PersistentVolume> = Api::all(client.inner().clone());

    // Build capacity
    let mut capacity = BTreeMap::new();
    capacity.insert("storage".to_string(), Quantity(request.capacity.clone()));

    let pv = PersistentVolume {
        metadata: ObjectMeta {
            name: Some(request.name.clone()),
            labels: if request.labels.is_empty() {
                None
            } else {
                Some(request.labels.clone())
            },
            ..Default::default()
        },
        spec: Some(PersistentVolumeSpec {
            capacity: Some(capacity),
            access_modes: Some(request.access_modes.clone()),
            persistent_volume_reclaim_policy: request.reclaim_policy.clone(),
            storage_class_name: request.storage_class.clone(),
            volume_mode: request.volume_mode.clone(),
            host_path: request.host_path.as_ref().map(|path| HostPathVolumeSource {
                path: path.clone(),
                type_: Some("DirectoryOrCreate".to_string()),
            }),
            nfs: request.nfs.as_ref().map(|nfs| NFSVolumeSource {
                server: nfs.server.clone(),
                path: nfs.path.clone(),
                read_only: Some(nfs.read_only),
            }),
            ..Default::default()
        }),
        ..Default::default()
    };

    let created = pvs.create(&PostParams::default(), &pv).await?;
    Ok(pv_to_info(created))
}

/// Delete a PV
#[cfg(feature = "kubernetes")]
pub async fn delete_pv(client: &K8sClient, name: &str) -> K8sResult<()> {
    use k8s_openapi::api::core::v1::PersistentVolume;
    use kube::api::{Api, DeleteParams};

    let pvs: Api<PersistentVolume> = Api::all(client.inner().clone());
    pvs.delete(name, &DeleteParams::default()).await?;

    Ok(())
}

#[cfg(feature = "kubernetes")]
fn pv_to_info(pv: k8s_openapi::api::core::v1::PersistentVolume) -> PvInfo {
    let metadata = pv.metadata;
    let spec = pv.spec.unwrap_or_default();
    let status = pv.status.unwrap_or_default();

    // Get capacity
    let capacity = spec
        .capacity
        .and_then(|c| c.get("storage").map(|q| q.0.clone()))
        .unwrap_or_default();

    // Get claim reference
    let claim_ref = spec.claim_ref.map(|cr| PvClaimRef {
        name: cr.name.unwrap_or_default(),
        namespace: cr.namespace.unwrap_or_default(),
    });

    PvInfo {
        name: metadata.name.unwrap_or_default(),
        status: status.phase.unwrap_or_else(|| "Unknown".to_string()),
        capacity,
        access_modes: spec.access_modes.unwrap_or_default(),
        reclaim_policy: spec
            .persistent_volume_reclaim_policy
            .unwrap_or_else(|| "Retain".to_string()),
        storage_class: spec.storage_class_name,
        volume_mode: spec.volume_mode,
        claim_ref,
        labels: metadata.labels.unwrap_or_default(),
        created_at: metadata.creation_timestamp.map(|t| t.0.to_rfc3339()),
    }
}

// ============================================================================
// Stubs for when kubernetes feature is disabled
// ============================================================================

#[cfg(not(feature = "kubernetes"))]
pub async fn list_pvcs(_client: &K8sClient, _namespace: &str) -> K8sResult<Vec<PvcInfo>> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_pvc(_client: &K8sClient, _namespace: &str, _name: &str) -> K8sResult<PvcInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn create_pvc(_client: &K8sClient, _request: &CreatePvcRequest) -> K8sResult<PvcInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn delete_pvc(_client: &K8sClient, _namespace: &str, _name: &str) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn list_pvs(_client: &K8sClient) -> K8sResult<Vec<PvInfo>> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_pv(_client: &K8sClient, _name: &str) -> K8sResult<PvInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn create_pv(_client: &K8sClient, _request: &CreatePvRequest) -> K8sResult<PvInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn delete_pv(_client: &K8sClient, _name: &str) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}
