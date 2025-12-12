//! RBAC operations
//!
//! Roles, ClusterRoles, RoleBindings, ClusterRoleBindings, ServiceAccounts.

use crate::kubernetes::client::K8sClient;
#[cfg(not(feature = "kubernetes"))]
use crate::kubernetes::error::K8sError;
use crate::kubernetes::error::K8sResult;
use crate::kubernetes::types::{
    ClusterRoleBindingInfo, ClusterRoleInfo, CreateClusterRoleBindingRequest,
    CreateClusterRoleRequest, CreateRoleBindingRequest, CreateRoleRequest,
    CreateServiceAccountRequest, PolicyRule, RoleBindingInfo, RoleInfo, RoleRef,
    ServiceAccountInfo, Subject,
};

// ============================================================================
// ServiceAccount Operations
// ============================================================================

/// List ServiceAccounts in a namespace
#[cfg(feature = "kubernetes")]
pub async fn list_service_accounts(
    client: &K8sClient,
    namespace: &str,
) -> K8sResult<Vec<ServiceAccountInfo>> {
    use k8s_openapi::api::core::v1::ServiceAccount;
    use kube::api::{Api, ListParams};

    let sas: Api<ServiceAccount> = Api::namespaced(client.inner().clone(), namespace);
    let list = sas.list(&ListParams::default()).await?;

    Ok(list.items.into_iter().map(sa_to_info).collect())
}

/// Get a specific ServiceAccount
#[cfg(feature = "kubernetes")]
pub async fn get_service_account(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<ServiceAccountInfo> {
    use k8s_openapi::api::core::v1::ServiceAccount;
    use kube::api::Api;

    let sas: Api<ServiceAccount> = Api::namespaced(client.inner().clone(), namespace);
    let sa = sas.get(name).await?;

    Ok(sa_to_info(sa))
}

/// Create a new ServiceAccount
#[cfg(feature = "kubernetes")]
pub async fn create_service_account(
    client: &K8sClient,
    request: &CreateServiceAccountRequest,
) -> K8sResult<ServiceAccountInfo> {
    use k8s_openapi::api::core::v1::ServiceAccount;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use kube::api::{Api, PostParams};

    let sas: Api<ServiceAccount> = Api::namespaced(client.inner().clone(), &request.namespace);

    let sa = ServiceAccount {
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
        ..Default::default()
    };

    let created = sas.create(&PostParams::default(), &sa).await?;
    Ok(sa_to_info(created))
}

/// Delete a ServiceAccount
#[cfg(feature = "kubernetes")]
pub async fn delete_service_account(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<()> {
    use k8s_openapi::api::core::v1::ServiceAccount;
    use kube::api::{Api, DeleteParams};

    let sas: Api<ServiceAccount> = Api::namespaced(client.inner().clone(), namespace);
    sas.delete(name, &DeleteParams::default()).await?;

    Ok(())
}

#[cfg(feature = "kubernetes")]
fn sa_to_info(sa: k8s_openapi::api::core::v1::ServiceAccount) -> ServiceAccountInfo {
    let metadata = sa.metadata;

    let secrets: Vec<String> = sa
        .secrets
        .unwrap_or_default()
        .into_iter()
        .filter_map(|s| s.name)
        .collect();

    let image_pull_secrets: Vec<String> = sa
        .image_pull_secrets
        .unwrap_or_default()
        .into_iter()
        .map(|s| s.name)
        .collect();

    ServiceAccountInfo {
        name: metadata.name.unwrap_or_default(),
        namespace: metadata.namespace.unwrap_or_default(),
        secrets,
        image_pull_secrets,
        labels: metadata.labels.unwrap_or_default(),
        created_at: metadata.creation_timestamp.map(|t| t.0.to_rfc3339()),
    }
}

// ============================================================================
// Role Operations
// ============================================================================

/// List Roles in a namespace
#[cfg(feature = "kubernetes")]
pub async fn list_roles(client: &K8sClient, namespace: &str) -> K8sResult<Vec<RoleInfo>> {
    use k8s_openapi::api::rbac::v1::Role;
    use kube::api::{Api, ListParams};

    let roles: Api<Role> = Api::namespaced(client.inner().clone(), namespace);
    let list = roles.list(&ListParams::default()).await?;

    Ok(list.items.into_iter().map(role_to_info).collect())
}

/// Get a specific Role
#[cfg(feature = "kubernetes")]
pub async fn get_role(client: &K8sClient, namespace: &str, name: &str) -> K8sResult<RoleInfo> {
    use k8s_openapi::api::rbac::v1::Role;
    use kube::api::Api;

    let roles: Api<Role> = Api::namespaced(client.inner().clone(), namespace);
    let role = roles.get(name).await?;

    Ok(role_to_info(role))
}

/// Create a new Role
#[cfg(feature = "kubernetes")]
pub async fn create_role(client: &K8sClient, request: &CreateRoleRequest) -> K8sResult<RoleInfo> {
    use k8s_openapi::api::rbac::v1::{PolicyRule as K8sPolicyRule, Role};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use kube::api::{Api, PostParams};

    let roles: Api<Role> = Api::namespaced(client.inner().clone(), &request.namespace);

    let rules: Vec<K8sPolicyRule> = request.rules.iter().map(convert_policy_rule).collect();

    let role = Role {
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
        rules: Some(rules),
    };

    let created = roles.create(&PostParams::default(), &role).await?;
    Ok(role_to_info(created))
}

/// Delete a Role
#[cfg(feature = "kubernetes")]
pub async fn delete_role(client: &K8sClient, namespace: &str, name: &str) -> K8sResult<()> {
    use k8s_openapi::api::rbac::v1::Role;
    use kube::api::{Api, DeleteParams};

    let roles: Api<Role> = Api::namespaced(client.inner().clone(), namespace);
    roles.delete(name, &DeleteParams::default()).await?;

    Ok(())
}

#[cfg(feature = "kubernetes")]
fn role_to_info(role: k8s_openapi::api::rbac::v1::Role) -> RoleInfo {
    let metadata = role.metadata;

    let rules: Vec<PolicyRule> = role
        .rules
        .unwrap_or_default()
        .into_iter()
        .map(|r| PolicyRule {
            api_groups: r.api_groups.unwrap_or_default(),
            resources: r.resources.unwrap_or_default(),
            verbs: r.verbs,
            resource_names: r.resource_names,
        })
        .collect();

    RoleInfo {
        name: metadata.name.unwrap_or_default(),
        namespace: metadata.namespace.unwrap_or_default(),
        rules,
        labels: metadata.labels.unwrap_or_default(),
        created_at: metadata.creation_timestamp.map(|t| t.0.to_rfc3339()),
    }
}

// ============================================================================
// ClusterRole Operations
// ============================================================================

/// List all ClusterRoles
#[cfg(feature = "kubernetes")]
pub async fn list_cluster_roles(client: &K8sClient) -> K8sResult<Vec<ClusterRoleInfo>> {
    use k8s_openapi::api::rbac::v1::ClusterRole;
    use kube::api::{Api, ListParams};

    let roles: Api<ClusterRole> = Api::all(client.inner().clone());
    let list = roles.list(&ListParams::default()).await?;

    Ok(list.items.into_iter().map(cluster_role_to_info).collect())
}

/// Get a specific ClusterRole
#[cfg(feature = "kubernetes")]
pub async fn get_cluster_role(client: &K8sClient, name: &str) -> K8sResult<ClusterRoleInfo> {
    use k8s_openapi::api::rbac::v1::ClusterRole;
    use kube::api::Api;

    let roles: Api<ClusterRole> = Api::all(client.inner().clone());
    let role = roles.get(name).await?;

    Ok(cluster_role_to_info(role))
}

/// Create a new ClusterRole
#[cfg(feature = "kubernetes")]
pub async fn create_cluster_role(
    client: &K8sClient,
    request: &CreateClusterRoleRequest,
) -> K8sResult<ClusterRoleInfo> {
    use k8s_openapi::api::rbac::v1::{ClusterRole, PolicyRule as K8sPolicyRule};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use kube::api::{Api, PostParams};

    let roles: Api<ClusterRole> = Api::all(client.inner().clone());

    let rules: Vec<K8sPolicyRule> = request.rules.iter().map(convert_policy_rule).collect();

    let role = ClusterRole {
        metadata: ObjectMeta {
            name: Some(request.name.clone()),
            labels: if request.labels.is_empty() {
                None
            } else {
                Some(request.labels.clone())
            },
            ..Default::default()
        },
        rules: Some(rules),
        ..Default::default()
    };

    let created = roles.create(&PostParams::default(), &role).await?;
    Ok(cluster_role_to_info(created))
}

/// Delete a ClusterRole
#[cfg(feature = "kubernetes")]
pub async fn delete_cluster_role(client: &K8sClient, name: &str) -> K8sResult<()> {
    use k8s_openapi::api::rbac::v1::ClusterRole;
    use kube::api::{Api, DeleteParams};

    let roles: Api<ClusterRole> = Api::all(client.inner().clone());
    roles.delete(name, &DeleteParams::default()).await?;

    Ok(())
}

#[cfg(feature = "kubernetes")]
fn cluster_role_to_info(role: k8s_openapi::api::rbac::v1::ClusterRole) -> ClusterRoleInfo {
    let metadata = role.metadata;

    let rules: Vec<PolicyRule> = role
        .rules
        .unwrap_or_default()
        .into_iter()
        .map(|r| PolicyRule {
            api_groups: r.api_groups.unwrap_or_default(),
            resources: r.resources.unwrap_or_default(),
            verbs: r.verbs,
            resource_names: r.resource_names,
        })
        .collect();

    ClusterRoleInfo {
        name: metadata.name.unwrap_or_default(),
        rules,
        labels: metadata.labels.unwrap_or_default(),
        created_at: metadata.creation_timestamp.map(|t| t.0.to_rfc3339()),
    }
}

// ============================================================================
// RoleBinding Operations
// ============================================================================

/// List RoleBindings in a namespace
#[cfg(feature = "kubernetes")]
pub async fn list_role_bindings(
    client: &K8sClient,
    namespace: &str,
) -> K8sResult<Vec<RoleBindingInfo>> {
    use k8s_openapi::api::rbac::v1::RoleBinding;
    use kube::api::{Api, ListParams};

    let bindings: Api<RoleBinding> = Api::namespaced(client.inner().clone(), namespace);
    let list = bindings.list(&ListParams::default()).await?;

    Ok(list.items.into_iter().map(role_binding_to_info).collect())
}

/// Get a specific RoleBinding
#[cfg(feature = "kubernetes")]
pub async fn get_role_binding(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<RoleBindingInfo> {
    use k8s_openapi::api::rbac::v1::RoleBinding;
    use kube::api::Api;

    let bindings: Api<RoleBinding> = Api::namespaced(client.inner().clone(), namespace);
    let binding = bindings.get(name).await?;

    Ok(role_binding_to_info(binding))
}

/// Create a new RoleBinding
#[cfg(feature = "kubernetes")]
pub async fn create_role_binding(
    client: &K8sClient,
    request: &CreateRoleBindingRequest,
) -> K8sResult<RoleBindingInfo> {
    use k8s_openapi::api::rbac::v1::{RoleBinding, RoleRef as K8sRoleRef, Subject as K8sSubject};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use kube::api::{Api, PostParams};

    let bindings: Api<RoleBinding> = Api::namespaced(client.inner().clone(), &request.namespace);

    let subjects: Vec<K8sSubject> = request.subjects.iter().map(convert_subject).collect();

    let binding = RoleBinding {
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
        role_ref: K8sRoleRef {
            api_group: request.role_ref.api_group.clone(),
            kind: request.role_ref.kind.clone(),
            name: request.role_ref.name.clone(),
        },
        subjects: Some(subjects),
    };

    let created = bindings.create(&PostParams::default(), &binding).await?;
    Ok(role_binding_to_info(created))
}

/// Delete a RoleBinding
#[cfg(feature = "kubernetes")]
pub async fn delete_role_binding(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<()> {
    use k8s_openapi::api::rbac::v1::RoleBinding;
    use kube::api::{Api, DeleteParams};

    let bindings: Api<RoleBinding> = Api::namespaced(client.inner().clone(), namespace);
    bindings.delete(name, &DeleteParams::default()).await?;

    Ok(())
}

#[cfg(feature = "kubernetes")]
fn role_binding_to_info(binding: k8s_openapi::api::rbac::v1::RoleBinding) -> RoleBindingInfo {
    let metadata = binding.metadata;

    let role_ref = RoleRef {
        api_group: binding.role_ref.api_group,
        kind: binding.role_ref.kind,
        name: binding.role_ref.name,
    };

    let subjects: Vec<Subject> = binding
        .subjects
        .unwrap_or_default()
        .into_iter()
        .map(|s| Subject {
            kind: s.kind,
            name: s.name,
            namespace: s.namespace,
            api_group: s.api_group,
        })
        .collect();

    RoleBindingInfo {
        name: metadata.name.unwrap_or_default(),
        namespace: metadata.namespace.unwrap_or_default(),
        role_ref,
        subjects,
        labels: metadata.labels.unwrap_or_default(),
        created_at: metadata.creation_timestamp.map(|t| t.0.to_rfc3339()),
    }
}

// ============================================================================
// ClusterRoleBinding Operations
// ============================================================================

/// List all ClusterRoleBindings
#[cfg(feature = "kubernetes")]
pub async fn list_cluster_role_bindings(
    client: &K8sClient,
) -> K8sResult<Vec<ClusterRoleBindingInfo>> {
    use k8s_openapi::api::rbac::v1::ClusterRoleBinding;
    use kube::api::{Api, ListParams};

    let bindings: Api<ClusterRoleBinding> = Api::all(client.inner().clone());
    let list = bindings.list(&ListParams::default()).await?;

    Ok(list
        .items
        .into_iter()
        .map(cluster_role_binding_to_info)
        .collect())
}

/// Get a specific ClusterRoleBinding
#[cfg(feature = "kubernetes")]
pub async fn get_cluster_role_binding(
    client: &K8sClient,
    name: &str,
) -> K8sResult<ClusterRoleBindingInfo> {
    use k8s_openapi::api::rbac::v1::ClusterRoleBinding;
    use kube::api::Api;

    let bindings: Api<ClusterRoleBinding> = Api::all(client.inner().clone());
    let binding = bindings.get(name).await?;

    Ok(cluster_role_binding_to_info(binding))
}

/// Create a new ClusterRoleBinding
#[cfg(feature = "kubernetes")]
pub async fn create_cluster_role_binding(
    client: &K8sClient,
    request: &CreateClusterRoleBindingRequest,
) -> K8sResult<ClusterRoleBindingInfo> {
    use k8s_openapi::api::rbac::v1::{
        ClusterRoleBinding, RoleRef as K8sRoleRef, Subject as K8sSubject,
    };
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use kube::api::{Api, PostParams};

    let bindings: Api<ClusterRoleBinding> = Api::all(client.inner().clone());

    let subjects: Vec<K8sSubject> = request.subjects.iter().map(convert_subject).collect();

    let binding = ClusterRoleBinding {
        metadata: ObjectMeta {
            name: Some(request.name.clone()),
            labels: if request.labels.is_empty() {
                None
            } else {
                Some(request.labels.clone())
            },
            ..Default::default()
        },
        role_ref: K8sRoleRef {
            api_group: request.role_ref.api_group.clone(),
            kind: request.role_ref.kind.clone(),
            name: request.role_ref.name.clone(),
        },
        subjects: Some(subjects),
    };

    let created = bindings.create(&PostParams::default(), &binding).await?;
    Ok(cluster_role_binding_to_info(created))
}

/// Delete a ClusterRoleBinding
#[cfg(feature = "kubernetes")]
pub async fn delete_cluster_role_binding(client: &K8sClient, name: &str) -> K8sResult<()> {
    use k8s_openapi::api::rbac::v1::ClusterRoleBinding;
    use kube::api::{Api, DeleteParams};

    let bindings: Api<ClusterRoleBinding> = Api::all(client.inner().clone());
    bindings.delete(name, &DeleteParams::default()).await?;

    Ok(())
}

#[cfg(feature = "kubernetes")]
fn cluster_role_binding_to_info(
    binding: k8s_openapi::api::rbac::v1::ClusterRoleBinding,
) -> ClusterRoleBindingInfo {
    let metadata = binding.metadata;

    let role_ref = RoleRef {
        api_group: binding.role_ref.api_group,
        kind: binding.role_ref.kind,
        name: binding.role_ref.name,
    };

    let subjects: Vec<Subject> = binding
        .subjects
        .unwrap_or_default()
        .into_iter()
        .map(|s| Subject {
            kind: s.kind,
            name: s.name,
            namespace: s.namespace,
            api_group: s.api_group,
        })
        .collect();

    ClusterRoleBindingInfo {
        name: metadata.name.unwrap_or_default(),
        role_ref,
        subjects,
        labels: metadata.labels.unwrap_or_default(),
        created_at: metadata.creation_timestamp.map(|t| t.0.to_rfc3339()),
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

#[cfg(feature = "kubernetes")]
fn convert_policy_rule(rule: &PolicyRule) -> k8s_openapi::api::rbac::v1::PolicyRule {
    k8s_openapi::api::rbac::v1::PolicyRule {
        api_groups: Some(rule.api_groups.clone()),
        resources: Some(rule.resources.clone()),
        verbs: rule.verbs.clone(),
        resource_names: rule.resource_names.clone(),
        non_resource_urls: None,
    }
}

#[cfg(feature = "kubernetes")]
fn convert_subject(subject: &Subject) -> k8s_openapi::api::rbac::v1::Subject {
    k8s_openapi::api::rbac::v1::Subject {
        kind: subject.kind.clone(),
        name: subject.name.clone(),
        namespace: subject.namespace.clone(),
        api_group: subject.api_group.clone(),
    }
}

// ============================================================================
// Stubs for when kubernetes feature is disabled
// ============================================================================

#[cfg(not(feature = "kubernetes"))]
pub async fn list_service_accounts(
    _client: &K8sClient,
    _namespace: &str,
) -> K8sResult<Vec<ServiceAccountInfo>> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_service_account(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<ServiceAccountInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn create_service_account(
    _client: &K8sClient,
    _request: &CreateServiceAccountRequest,
) -> K8sResult<ServiceAccountInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn delete_service_account(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn list_roles(_client: &K8sClient, _namespace: &str) -> K8sResult<Vec<RoleInfo>> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_role(_client: &K8sClient, _namespace: &str, _name: &str) -> K8sResult<RoleInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn create_role(_client: &K8sClient, _request: &CreateRoleRequest) -> K8sResult<RoleInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn delete_role(_client: &K8sClient, _namespace: &str, _name: &str) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn list_cluster_roles(_client: &K8sClient) -> K8sResult<Vec<ClusterRoleInfo>> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_cluster_role(_client: &K8sClient, _name: &str) -> K8sResult<ClusterRoleInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn create_cluster_role(
    _client: &K8sClient,
    _request: &CreateClusterRoleRequest,
) -> K8sResult<ClusterRoleInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn delete_cluster_role(_client: &K8sClient, _name: &str) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn list_role_bindings(
    _client: &K8sClient,
    _namespace: &str,
) -> K8sResult<Vec<RoleBindingInfo>> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_role_binding(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<RoleBindingInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn create_role_binding(
    _client: &K8sClient,
    _request: &CreateRoleBindingRequest,
) -> K8sResult<RoleBindingInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn delete_role_binding(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn list_cluster_role_bindings(
    _client: &K8sClient,
) -> K8sResult<Vec<ClusterRoleBindingInfo>> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_cluster_role_binding(
    _client: &K8sClient,
    _name: &str,
) -> K8sResult<ClusterRoleBindingInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn create_cluster_role_binding(
    _client: &K8sClient,
    _request: &CreateClusterRoleBindingRequest,
) -> K8sResult<ClusterRoleBindingInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn delete_cluster_role_binding(_client: &K8sClient, _name: &str) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}
