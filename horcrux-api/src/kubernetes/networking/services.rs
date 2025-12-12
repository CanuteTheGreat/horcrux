//! Service operations
//!
//! CRUD operations for Kubernetes Services.

use crate::kubernetes::client::K8sClient;
#[cfg(not(feature = "kubernetes"))]
use crate::kubernetes::error::K8sError;
use crate::kubernetes::error::K8sResult;
use crate::kubernetes::types::{CreateServiceRequest, ServiceInfo, ServicePort, ServiceType};

/// List Services in a namespace
#[cfg(feature = "kubernetes")]
pub async fn list_services(
    client: &K8sClient,
    namespace: &str,
) -> K8sResult<Vec<ServiceInfo>> {
    use k8s_openapi::api::core::v1::Service;
    use kube::api::{Api, ListParams};

    let services: Api<Service> = Api::namespaced(client.inner().clone(), namespace);
    let list = services.list(&ListParams::default()).await?;

    Ok(list.items.into_iter().map(service_to_info).collect())
}

/// Get a specific Service
#[cfg(feature = "kubernetes")]
pub async fn get_service(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<ServiceInfo> {
    use k8s_openapi::api::core::v1::Service;
    use kube::api::Api;

    let services: Api<Service> = Api::namespaced(client.inner().clone(), namespace);
    let service = services.get(name).await?;

    Ok(service_to_info(service))
}

/// Create a new Service
#[cfg(feature = "kubernetes")]
pub async fn create_service(
    client: &K8sClient,
    request: &CreateServiceRequest,
) -> K8sResult<ServiceInfo> {
    use k8s_openapi::api::core::v1::{Service, ServicePort as K8sServicePort, ServiceSpec};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
    use kube::api::{Api, PostParams};

    let services: Api<Service> = Api::namespaced(client.inner().clone(), &request.namespace);

    // Convert ports
    let ports: Vec<K8sServicePort> = request
        .ports
        .iter()
        .map(|p| K8sServicePort {
            name: p.name.clone(),
            protocol: p.protocol.clone(),
            port: p.port,
            target_port: p.target_port.as_ref().map(|tp| {
                if let Ok(port_num) = tp.parse::<i32>() {
                    IntOrString::Int(port_num)
                } else {
                    IntOrString::String(tp.clone())
                }
            }),
            node_port: p.node_port,
            ..Default::default()
        })
        .collect();

    let service = Service {
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
        spec: Some(ServiceSpec {
            type_: request.service_type.clone(),
            ports: Some(ports),
            selector: if request.selector.is_empty() {
                None
            } else {
                Some(request.selector.clone())
            },
            cluster_ip: request.cluster_ip.clone(),
            external_ips: request.external_ips.clone(),
            load_balancer_ip: request.load_balancer_ip.clone(),
            session_affinity: request.session_affinity.clone(),
            ..Default::default()
        }),
        ..Default::default()
    };

    let created = services.create(&PostParams::default(), &service).await?;
    Ok(service_to_info(created))
}

/// Update a Service
#[cfg(feature = "kubernetes")]
pub async fn update_service(
    client: &K8sClient,
    namespace: &str,
    name: &str,
    request: &CreateServiceRequest,
) -> K8sResult<ServiceInfo> {
    use k8s_openapi::api::core::v1::Service;
    use kube::api::{Api, Patch, PatchParams};

    let services: Api<Service> = Api::namespaced(client.inner().clone(), namespace);

    // Convert ports
    let ports: Vec<serde_json::Value> = request
        .ports
        .iter()
        .map(|p| {
            let mut port_json = serde_json::json!({
                "port": p.port,
            });
            if let Some(ref name) = p.name {
                port_json["name"] = serde_json::json!(name);
            }
            if let Some(ref protocol) = p.protocol {
                port_json["protocol"] = serde_json::json!(protocol);
            }
            if let Some(ref tp) = p.target_port {
                if let Ok(port_num) = tp.parse::<i32>() {
                    port_json["targetPort"] = serde_json::json!(port_num);
                } else {
                    port_json["targetPort"] = serde_json::json!(tp);
                }
            }
            if let Some(np) = p.node_port {
                port_json["nodePort"] = serde_json::json!(np);
            }
            port_json
        })
        .collect();

    let patch = serde_json::json!({
        "spec": {
            "ports": ports,
            "selector": if request.selector.is_empty() { serde_json::Value::Null } else { serde_json::json!(request.selector) },
        }
    });

    let patched = services
        .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
        .await?;

    Ok(service_to_info(patched))
}

/// Delete a Service
#[cfg(feature = "kubernetes")]
pub async fn delete_service(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<()> {
    use k8s_openapi::api::core::v1::Service;
    use kube::api::{Api, DeleteParams};

    let services: Api<Service> = Api::namespaced(client.inner().clone(), namespace);
    services.delete(name, &DeleteParams::default()).await?;

    Ok(())
}

#[cfg(feature = "kubernetes")]
fn service_to_info(service: k8s_openapi::api::core::v1::Service) -> ServiceInfo {
    let metadata = service.metadata;
    let spec = service.spec.unwrap_or_default();

    // Parse service type
    let service_type = match spec.type_.as_deref() {
        Some("ClusterIP") | None => ServiceType::ClusterIP,
        Some("NodePort") => ServiceType::NodePort,
        Some("LoadBalancer") => ServiceType::LoadBalancer,
        Some("ExternalName") => ServiceType::ExternalName,
        _ => ServiceType::ClusterIP,
    };

    // Get external IP from status or spec
    let external_ip = service
        .status
        .and_then(|s| s.load_balancer)
        .and_then(|lb| lb.ingress)
        .and_then(|ingress| ingress.first().cloned())
        .and_then(|ing| ing.ip.or(ing.hostname));

    // Convert ports
    let ports: Vec<ServicePort> = spec
        .ports
        .unwrap_or_default()
        .into_iter()
        .map(|p| ServicePort {
            name: p.name,
            protocol: p.protocol.unwrap_or_else(|| "TCP".to_string()),
            port: p.port,
            target_port: p
                .target_port
                .map(|tp| match tp {
                    k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::Int(i) => {
                        i.to_string()
                    }
                    k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::String(s) => s,
                })
                .unwrap_or_default(),
            node_port: p.node_port,
        })
        .collect();

    ServiceInfo {
        name: metadata.name.unwrap_or_default(),
        namespace: metadata.namespace.unwrap_or_default(),
        service_type,
        cluster_ip: spec.cluster_ip,
        external_ip,
        ports,
        selector: spec.selector.unwrap_or_default(),
        labels: metadata.labels.unwrap_or_default(),
        created_at: metadata.creation_timestamp.map(|t| t.0.to_rfc3339()),
    }
}

// Stubs for when kubernetes feature is disabled
#[cfg(not(feature = "kubernetes"))]
pub async fn list_services(_client: &K8sClient, _namespace: &str) -> K8sResult<Vec<ServiceInfo>> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_service(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<ServiceInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn create_service(
    _client: &K8sClient,
    _request: &CreateServiceRequest,
) -> K8sResult<ServiceInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn update_service(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
    _request: &CreateServiceRequest,
) -> K8sResult<ServiceInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn delete_service(_client: &K8sClient, _namespace: &str, _name: &str) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}
