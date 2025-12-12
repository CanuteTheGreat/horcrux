//! NetworkPolicy operations
//!
//! CRUD operations for Kubernetes NetworkPolicies.
//! Bridges to existing Horcrux SDN policy system.

use crate::kubernetes::client::K8sClient;
#[cfg(not(feature = "kubernetes"))]
use crate::kubernetes::error::K8sError;
use crate::kubernetes::error::K8sResult;
use crate::kubernetes::types::{CreateNetworkPolicyRequest, NetworkPolicyInfo};

/// List NetworkPolicies in a namespace
#[cfg(feature = "kubernetes")]
pub async fn list_network_policies(
    client: &K8sClient,
    namespace: &str,
) -> K8sResult<Vec<NetworkPolicyInfo>> {
    use k8s_openapi::api::networking::v1::NetworkPolicy;
    use kube::api::{Api, ListParams};

    let policies: Api<NetworkPolicy> = Api::namespaced(client.inner().clone(), namespace);
    let list = policies.list(&ListParams::default()).await?;

    Ok(list.items.into_iter().map(network_policy_to_info).collect())
}

/// Get a specific NetworkPolicy
#[cfg(feature = "kubernetes")]
pub async fn get_network_policy(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<NetworkPolicyInfo> {
    use k8s_openapi::api::networking::v1::NetworkPolicy;
    use kube::api::Api;

    let policies: Api<NetworkPolicy> = Api::namespaced(client.inner().clone(), namespace);
    let policy = policies.get(name).await?;

    Ok(network_policy_to_info(policy))
}

/// Create a new NetworkPolicy
#[cfg(feature = "kubernetes")]
pub async fn create_network_policy(
    client: &K8sClient,
    request: &CreateNetworkPolicyRequest,
) -> K8sResult<NetworkPolicyInfo> {
    use k8s_openapi::api::networking::v1::{
        NetworkPolicy, NetworkPolicyEgressRule as K8sEgressRule,
        NetworkPolicyIngressRule as K8sIngressRule, NetworkPolicySpec,
    };
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
    use kube::api::{Api, PostParams};

    let policies: Api<NetworkPolicy> = Api::namespaced(client.inner().clone(), &request.namespace);

    // Convert ingress rules
    let ingress: Option<Vec<K8sIngressRule>> = request.ingress.as_ref().map(|rules| {
        rules
            .iter()
            .map(|r| K8sIngressRule {
                from: r.from.as_ref().map(|peers| {
                    peers
                        .iter()
                        .map(|p| convert_peer(p))
                        .collect()
                }),
                ports: r.ports.as_ref().map(|ports| {
                    ports
                        .iter()
                        .map(|p| convert_port(p))
                        .collect()
                }),
            })
            .collect()
    });

    // Convert egress rules
    let egress: Option<Vec<K8sEgressRule>> = request.egress.as_ref().map(|rules| {
        rules
            .iter()
            .map(|r| K8sEgressRule {
                to: r.to.as_ref().map(|peers| {
                    peers
                        .iter()
                        .map(|p| convert_peer(p))
                        .collect()
                }),
                ports: r.ports.as_ref().map(|ports| {
                    ports
                        .iter()
                        .map(|p| convert_port(p))
                        .collect()
                }),
            })
            .collect()
    });

    let policy = NetworkPolicy {
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
        spec: Some(NetworkPolicySpec {
            pod_selector: LabelSelector {
                match_labels: if request.pod_selector.is_empty() {
                    None
                } else {
                    Some(request.pod_selector.clone())
                },
                match_expressions: None,
            },
            policy_types: request.policy_types.clone(),
            ingress,
            egress,
        }),
        ..Default::default()
    };

    let created = policies.create(&PostParams::default(), &policy).await?;
    Ok(network_policy_to_info(created))
}

/// Delete a NetworkPolicy
#[cfg(feature = "kubernetes")]
pub async fn delete_network_policy(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<()> {
    use k8s_openapi::api::networking::v1::NetworkPolicy;
    use kube::api::{Api, DeleteParams};

    let policies: Api<NetworkPolicy> = Api::namespaced(client.inner().clone(), namespace);
    policies.delete(name, &DeleteParams::default()).await?;

    Ok(())
}

#[cfg(feature = "kubernetes")]
fn convert_peer(
    peer: &crate::kubernetes::types::NetworkPolicyPeer,
) -> k8s_openapi::api::networking::v1::NetworkPolicyPeer {
    use k8s_openapi::api::networking::v1::{IPBlock, NetworkPolicyPeer as K8sPeer};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;

    K8sPeer {
        pod_selector: peer.pod_selector.as_ref().map(|sel| LabelSelector {
            match_labels: if sel.is_empty() {
                None
            } else {
                Some(sel.clone())
            },
            match_expressions: None,
        }),
        namespace_selector: peer.namespace_selector.as_ref().map(|sel| LabelSelector {
            match_labels: if sel.is_empty() {
                None
            } else {
                Some(sel.clone())
            },
            match_expressions: None,
        }),
        ip_block: peer.ip_block.as_ref().map(|ib| IPBlock {
            cidr: ib.cidr.clone(),
            except: ib.except.clone(),
        }),
    }
}

#[cfg(feature = "kubernetes")]
fn convert_port(
    port: &crate::kubernetes::types::NetworkPolicyPort,
) -> k8s_openapi::api::networking::v1::NetworkPolicyPort {
    use k8s_openapi::api::networking::v1::NetworkPolicyPort as K8sPort;
    use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;

    K8sPort {
        protocol: port.protocol.clone(),
        port: port.port.map(IntOrString::Int),
        end_port: port.end_port,
    }
}

#[cfg(feature = "kubernetes")]
fn network_policy_to_info(
    policy: k8s_openapi::api::networking::v1::NetworkPolicy,
) -> NetworkPolicyInfo {
    let metadata = policy.metadata;
    let spec = policy.spec.unwrap_or_default();

    // Get pod selector
    let pod_selector = spec
        .pod_selector
        .match_labels
        .unwrap_or_default();

    // Get policy types
    let policy_types = spec.policy_types.unwrap_or_default();

    // Count rules
    let ingress_rules_count = spec.ingress.map(|i| i.len()).unwrap_or(0);
    let egress_rules_count = spec.egress.map(|e| e.len()).unwrap_or(0);

    NetworkPolicyInfo {
        name: metadata.name.unwrap_or_default(),
        namespace: metadata.namespace.unwrap_or_default(),
        pod_selector,
        policy_types,
        ingress_rules_count,
        egress_rules_count,
        labels: metadata.labels.unwrap_or_default(),
        created_at: metadata.creation_timestamp.map(|t| t.0.to_rfc3339()),
    }
}

// Stubs for when kubernetes feature is disabled
#[cfg(not(feature = "kubernetes"))]
pub async fn list_network_policies(
    _client: &K8sClient,
    _namespace: &str,
) -> K8sResult<Vec<NetworkPolicyInfo>> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_network_policy(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<NetworkPolicyInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn create_network_policy(
    _client: &K8sClient,
    _request: &CreateNetworkPolicyRequest,
) -> K8sResult<NetworkPolicyInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn delete_network_policy(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}
