//! Ingress operations
//!
//! CRUD operations for Kubernetes Ingress resources.

use crate::kubernetes::client::K8sClient;
#[cfg(not(feature = "kubernetes"))]
use crate::kubernetes::error::K8sError;
use crate::kubernetes::error::K8sResult;
use crate::kubernetes::types::{
    CreateIngressRequest, IngressInfo, IngressPath, IngressRule, IngressTls,
};

/// List Ingresses in a namespace
#[cfg(feature = "kubernetes")]
pub async fn list_ingresses(
    client: &K8sClient,
    namespace: &str,
) -> K8sResult<Vec<IngressInfo>> {
    use k8s_openapi::api::networking::v1::Ingress;
    use kube::api::{Api, ListParams};

    let ingresses: Api<Ingress> = Api::namespaced(client.inner().clone(), namespace);
    let list = ingresses.list(&ListParams::default()).await?;

    Ok(list.items.into_iter().map(ingress_to_info).collect())
}

/// Get a specific Ingress
#[cfg(feature = "kubernetes")]
pub async fn get_ingress(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<IngressInfo> {
    use k8s_openapi::api::networking::v1::Ingress;
    use kube::api::Api;

    let ingresses: Api<Ingress> = Api::namespaced(client.inner().clone(), namespace);
    let ingress = ingresses.get(name).await?;

    Ok(ingress_to_info(ingress))
}

/// Create a new Ingress
#[cfg(feature = "kubernetes")]
pub async fn create_ingress(
    client: &K8sClient,
    request: &CreateIngressRequest,
) -> K8sResult<IngressInfo> {
    use k8s_openapi::api::networking::v1::{
        HTTPIngressPath, HTTPIngressRuleValue, Ingress, IngressBackend, IngressRule as K8sIngressRule,
        IngressServiceBackend, IngressSpec, IngressTLS, ServiceBackendPort,
    };
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use kube::api::{Api, PostParams};

    let ingresses: Api<Ingress> = Api::namespaced(client.inner().clone(), &request.namespace);

    // Convert rules
    let rules: Vec<K8sIngressRule> = request
        .rules
        .iter()
        .map(|r| {
            let paths: Vec<HTTPIngressPath> = r
                .paths
                .iter()
                .map(|p| HTTPIngressPath {
                    path: Some(p.path.clone()),
                    path_type: p.path_type.clone().unwrap_or_else(|| "Prefix".to_string()),
                    backend: IngressBackend {
                        service: Some(IngressServiceBackend {
                            name: p.service_name.clone(),
                            port: Some(ServiceBackendPort {
                                number: Some(p.service_port),
                                name: None,
                            }),
                        }),
                        resource: None,
                    },
                })
                .collect();

            K8sIngressRule {
                host: r.host.clone(),
                http: Some(HTTPIngressRuleValue { paths }),
            }
        })
        .collect();

    // Convert TLS
    let tls: Option<Vec<IngressTLS>> = request.tls.as_ref().map(|tls_list| {
        tls_list
            .iter()
            .map(|t| IngressTLS {
                hosts: Some(t.hosts.clone()),
                secret_name: t.secret_name.clone(),
            })
            .collect()
    });

    let ingress = Ingress {
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
        spec: Some(IngressSpec {
            ingress_class_name: request.ingress_class.clone(),
            rules: Some(rules),
            tls,
            default_backend: None,
        }),
        ..Default::default()
    };

    let created = ingresses.create(&PostParams::default(), &ingress).await?;
    Ok(ingress_to_info(created))
}

/// Update an Ingress
#[cfg(feature = "kubernetes")]
pub async fn update_ingress(
    client: &K8sClient,
    namespace: &str,
    name: &str,
    request: &CreateIngressRequest,
) -> K8sResult<IngressInfo> {
    use k8s_openapi::api::networking::v1::Ingress;
    use kube::api::{Api, Patch, PatchParams};

    let ingresses: Api<Ingress> = Api::namespaced(client.inner().clone(), namespace);

    // Build rules JSON
    let rules: Vec<serde_json::Value> = request
        .rules
        .iter()
        .map(|r| {
            let paths: Vec<serde_json::Value> = r
                .paths
                .iter()
                .map(|p| {
                    serde_json::json!({
                        "path": p.path,
                        "pathType": p.path_type.clone().unwrap_or_else(|| "Prefix".to_string()),
                        "backend": {
                            "service": {
                                "name": p.service_name,
                                "port": {
                                    "number": p.service_port
                                }
                            }
                        }
                    })
                })
                .collect();

            let mut rule = serde_json::json!({
                "http": {
                    "paths": paths
                }
            });
            if let Some(ref host) = r.host {
                rule["host"] = serde_json::json!(host);
            }
            rule
        })
        .collect();

    // Build TLS JSON
    let tls: Option<Vec<serde_json::Value>> = request.tls.as_ref().map(|tls_list| {
        tls_list
            .iter()
            .map(|t| {
                let mut tls_json = serde_json::json!({
                    "hosts": t.hosts
                });
                if let Some(ref secret) = t.secret_name {
                    tls_json["secretName"] = serde_json::json!(secret);
                }
                tls_json
            })
            .collect()
    });

    let mut patch = serde_json::json!({
        "spec": {
            "rules": rules
        }
    });

    if let Some(tls_config) = tls {
        patch["spec"]["tls"] = serde_json::json!(tls_config);
    }

    if let Some(ref ingress_class) = request.ingress_class {
        patch["spec"]["ingressClassName"] = serde_json::json!(ingress_class);
    }

    let patched = ingresses
        .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
        .await?;

    Ok(ingress_to_info(patched))
}

/// Delete an Ingress
#[cfg(feature = "kubernetes")]
pub async fn delete_ingress(
    client: &K8sClient,
    namespace: &str,
    name: &str,
) -> K8sResult<()> {
    use k8s_openapi::api::networking::v1::Ingress;
    use kube::api::{Api, DeleteParams};

    let ingresses: Api<Ingress> = Api::namespaced(client.inner().clone(), namespace);
    ingresses.delete(name, &DeleteParams::default()).await?;

    Ok(())
}

#[cfg(feature = "kubernetes")]
fn ingress_to_info(ingress: k8s_openapi::api::networking::v1::Ingress) -> IngressInfo {
    let metadata = ingress.metadata;
    let spec = ingress.spec.unwrap_or_default();

    // Get ingress class
    let ingress_class = spec.ingress_class_name;

    // Convert rules
    let rules: Vec<IngressRule> = spec
        .rules
        .unwrap_or_default()
        .into_iter()
        .map(|r| {
            let paths: Vec<IngressPath> = r
                .http
                .map(|http| {
                    http.paths
                        .into_iter()
                        .map(|p| {
                            let (service_name, service_port) = p
                                .backend
                                .service
                                .map(|svc| {
                                    let port = svc
                                        .port
                                        .map(|port| {
                                            port.number
                                                .map(|n| n.to_string())
                                                .or(port.name)
                                                .unwrap_or_default()
                                        })
                                        .unwrap_or_default();
                                    (svc.name, port)
                                })
                                .unwrap_or_default();

                            IngressPath {
                                path: p.path.unwrap_or_else(|| "/".to_string()),
                                path_type: p.path_type,
                                backend_service: service_name,
                                backend_port: service_port,
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

            IngressRule {
                host: r.host,
                paths,
            }
        })
        .collect();

    // Convert TLS
    let tls: Vec<IngressTls> = spec
        .tls
        .unwrap_or_default()
        .into_iter()
        .map(|t| IngressTls {
            hosts: t.hosts.unwrap_or_default(),
            secret_name: t.secret_name,
        })
        .collect();

    // Get load balancer IPs from status
    let load_balancer_ips: Vec<String> = ingress
        .status
        .and_then(|s| s.load_balancer)
        .and_then(|lb| lb.ingress)
        .map(|ingress_list| {
            ingress_list
                .into_iter()
                .filter_map(|ing| ing.ip.or(ing.hostname))
                .collect()
        })
        .unwrap_or_default();

    IngressInfo {
        name: metadata.name.unwrap_or_default(),
        namespace: metadata.namespace.unwrap_or_default(),
        ingress_class,
        rules,
        tls,
        labels: metadata.labels.unwrap_or_default(),
        annotations: metadata.annotations.unwrap_or_default(),
        load_balancer_ips,
        created_at: metadata.creation_timestamp.map(|t| t.0.to_rfc3339()),
    }
}

// Stubs for when kubernetes feature is disabled
#[cfg(not(feature = "kubernetes"))]
pub async fn list_ingresses(
    _client: &K8sClient,
    _namespace: &str,
) -> K8sResult<Vec<IngressInfo>> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn get_ingress(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<IngressInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn create_ingress(
    _client: &K8sClient,
    _request: &CreateIngressRequest,
) -> K8sResult<IngressInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn update_ingress(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
    _request: &CreateIngressRequest,
) -> K8sResult<IngressInfo> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}

#[cfg(not(feature = "kubernetes"))]
pub async fn delete_ingress(
    _client: &K8sClient,
    _namespace: &str,
    _name: &str,
) -> K8sResult<()> {
    Err(K8sError::Internal("Kubernetes feature not enabled".to_string()))
}
