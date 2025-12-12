//! Kubernetes port forwarding
//!
//! Forward local ports to pods in a Kubernetes cluster.

use crate::kubernetes::client::K8sClient;
#[cfg(not(feature = "kubernetes"))]
use crate::kubernetes::error::K8sError;
use crate::kubernetes::error::K8sResult;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Port forward request
#[derive(Debug, Clone, Deserialize)]
pub struct PortForwardRequest {
    /// Namespace of the pod
    pub namespace: String,
    /// Pod name
    pub pod_name: String,
    /// Port mappings: local_port -> pod_port
    pub ports: Vec<PortMapping>,
}

/// Port mapping for forwarding
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PortMapping {
    /// Local port to listen on (0 for auto-assign)
    pub local_port: u16,
    /// Pod port to forward to
    pub pod_port: u16,
}

/// Active port forward session
#[derive(Debug, Clone, Serialize)]
pub struct PortForwardSession {
    /// Unique session ID
    pub id: String,
    /// Namespace
    pub namespace: String,
    /// Pod name
    pub pod_name: String,
    /// Active port mappings (actual local ports)
    pub ports: Vec<PortMapping>,
    /// Session status
    pub status: PortForwardStatus,
    /// Created timestamp
    pub created_at: i64,
}

/// Port forward status
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PortForwardStatus {
    Active,
    Stopped,
    Error,
}

/// Manager for port forward sessions
pub struct PortForwardManager {
    sessions: Arc<RwLock<HashMap<String, PortForwardSession>>>,
    #[cfg(feature = "kubernetes")]
    handles: Arc<RwLock<HashMap<String, tokio::task::JoinHandle<()>>>>,
}

impl PortForwardManager {
    /// Create a new port forward manager
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            #[cfg(feature = "kubernetes")]
            handles: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Start a port forward session
    #[cfg(feature = "kubernetes")]
    pub async fn start_port_forward(
        &self,
        client: &K8sClient,
        request: PortForwardRequest,
    ) -> K8sResult<PortForwardSession> {
        use k8s_openapi::api::core::v1::Pod;
        use kube::api::Api;
        use tokio::net::TcpListener;

        let pods: Api<Pod> = Api::namespaced(client.inner().clone(), &request.namespace);

        // Verify pod exists
        pods.get(&request.pod_name).await?;

        let session_id = uuid::Uuid::new_v4().to_string();
        let mut actual_ports = Vec::new();

        // For each port mapping, create a listener and spawn a forwarder
        for port_map in &request.ports {
            let local_addr = format!("127.0.0.1:{}", port_map.local_port);
            let listener = TcpListener::bind(&local_addr)
                .await
                .map_err(|e| crate::kubernetes::error::K8sError::PortForwardError(
                    format!("Failed to bind to {}: {}", local_addr, e)
                ))?;

            let actual_local_port = listener.local_addr()
                .map_err(|e| crate::kubernetes::error::K8sError::PortForwardError(e.to_string()))?
                .port();

            actual_ports.push(PortMapping {
                local_port: actual_local_port,
                pod_port: port_map.pod_port,
            });

            // Spawn the port forwarder task
            let pods_clone = pods.clone();
            let pod_name = request.pod_name.clone();
            let pod_port = port_map.pod_port;
            let session_id_clone = session_id.clone();
            let sessions = self.sessions.clone();

            let handle = tokio::spawn(async move {
                loop {
                    match listener.accept().await {
                        Ok((mut tcp_stream, _)) => {
                            let pods = pods_clone.clone();
                            let pod_name = pod_name.clone();

                            tokio::spawn(async move {
                                if let Err(e) = forward_connection(
                                    &pods,
                                    &pod_name,
                                    pod_port,
                                    &mut tcp_stream,
                                ).await {
                                    tracing::error!("Port forward error: {}", e);
                                }
                            });
                        }
                        Err(e) => {
                            tracing::error!("Accept error: {}", e);
                            // Update session status to error
                            let mut sessions = sessions.write().await;
                            if let Some(session) = sessions.get_mut(&session_id_clone) {
                                session.status = PortForwardStatus::Error;
                            }
                            break;
                        }
                    }
                }
            });

            // Store the handle
            let mut handles = self.handles.write().await;
            handles.insert(format!("{}:{}", session_id, port_map.pod_port), handle);
        }

        let session = PortForwardSession {
            id: session_id.clone(),
            namespace: request.namespace,
            pod_name: request.pod_name,
            ports: actual_ports,
            status: PortForwardStatus::Active,
            created_at: chrono::Utc::now().timestamp(),
        };

        // Store the session
        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(session_id, session.clone());
        }

        Ok(session)
    }

    /// Stop a port forward session
    #[cfg(feature = "kubernetes")]
    pub async fn stop_port_forward(&self, session_id: &str) -> K8sResult<()> {
        // Get and update the session
        let ports = {
            let mut sessions = self.sessions.write().await;
            if let Some(session) = sessions.get_mut(session_id) {
                session.status = PortForwardStatus::Stopped;
                session.ports.clone()
            } else {
                return Err(crate::kubernetes::error::K8sError::Internal(
                    format!("Session not found: {}", session_id)
                ));
            }
        };

        // Cancel all handles for this session
        let mut handles = self.handles.write().await;
        for port in &ports {
            let handle_key = format!("{}:{}", session_id, port.pod_port);
            if let Some(handle) = handles.remove(&handle_key) {
                handle.abort();
            }
        }

        Ok(())
    }

    /// List active port forward sessions
    pub async fn list_sessions(&self) -> Vec<PortForwardSession> {
        let sessions = self.sessions.read().await;
        sessions.values().cloned().collect()
    }

    /// Get a specific session
    pub async fn get_session(&self, session_id: &str) -> Option<PortForwardSession> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).cloned()
    }

    /// Clean up stopped sessions
    pub async fn cleanup_stopped(&self) {
        let mut sessions = self.sessions.write().await;
        sessions.retain(|_, session| session.status == PortForwardStatus::Active);
    }

    // Stub for non-kubernetes feature
    #[cfg(not(feature = "kubernetes"))]
    pub async fn start_port_forward(
        &self,
        _client: &K8sClient,
        _request: PortForwardRequest,
    ) -> K8sResult<PortForwardSession> {
        Err(K8sError::Internal(
            "Kubernetes feature not enabled".to_string(),
        ))
    }

    #[cfg(not(feature = "kubernetes"))]
    pub async fn stop_port_forward(&self, _session_id: &str) -> K8sResult<()> {
        Err(K8sError::Internal(
            "Kubernetes feature not enabled".to_string(),
        ))
    }
}

impl Default for PortForwardManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Forward a single TCP connection to a pod port
#[cfg(feature = "kubernetes")]
async fn forward_connection(
    pods: &kube::api::Api<k8s_openapi::api::core::v1::Pod>,
    pod_name: &str,
    port: u16,
    tcp_stream: &mut tokio::net::TcpStream,
) -> K8sResult<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // Create port forwarder
    let mut forwarder = pods.portforward(pod_name, &[port]).await?;

    // Get the port stream
    let port_stream = forwarder
        .take_stream(port)
        .ok_or_else(|| crate::kubernetes::error::K8sError::PortForwardError(
            "Failed to get port stream".to_string()
        ))?;

    // Split both streams
    let (mut tcp_read, mut tcp_write) = tcp_stream.split();
    let (mut pod_read, mut pod_write) = tokio::io::split(port_stream);

    // Forward data in both directions
    let client_to_pod = async {
        let mut buf = [0u8; 8192];
        loop {
            match tcp_read.read(&mut buf).await {
                Ok(0) => break, // EOF
                Ok(n) => {
                    if pod_write.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    };

    let pod_to_client = async {
        let mut buf = [0u8; 8192];
        loop {
            match pod_read.read(&mut buf).await {
                Ok(0) => break, // EOF
                Ok(n) => {
                    if tcp_write.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    };

    // Run both directions concurrently
    tokio::select! {
        _ = client_to_pod => {}
        _ = pod_to_client => {}
    }

    Ok(())
}

/// Forward to a service instead of a pod
#[cfg(feature = "kubernetes")]
pub async fn forward_to_service(
    client: &K8sClient,
    namespace: &str,
    service_name: &str,
    service_port: u16,
    local_port: u16,
) -> K8sResult<PortForwardSession> {
    use k8s_openapi::api::core::v1::{Pod, Service};
    use kube::api::{Api, ListParams};

    // Get the service to find selector
    let services: Api<Service> = Api::namespaced(client.inner().clone(), namespace);
    let service = services.get(service_name).await?;

    // Get the selector from the service
    let selector = service
        .spec
        .as_ref()
        .and_then(|s| s.selector.as_ref())
        .ok_or_else(|| crate::kubernetes::error::K8sError::Internal(
            "Service has no selector".to_string()
        ))?;

    // Build label selector string
    let label_selector: String = selector
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join(",");

    // Find a pod matching the selector
    let pods: Api<Pod> = Api::namespaced(client.inner().clone(), namespace);
    let pod_list = pods
        .list(&ListParams::default().labels(&label_selector))
        .await?;

    let pod = pod_list
        .items
        .into_iter()
        .find(|p| {
            p.status
                .as_ref()
                .and_then(|s| s.phase.as_ref())
                .map(|phase| phase == "Running")
                .unwrap_or(false)
        })
        .ok_or_else(|| crate::kubernetes::error::K8sError::Internal(
            format!("No running pod found for service {}", service_name)
        ))?;

    let pod_name = pod.metadata.name.unwrap_or_default();

    // Find the target port (could be a named port)
    let target_port = service
        .spec
        .as_ref()
        .and_then(|s| s.ports.as_ref())
        .and_then(|ports| {
            ports.iter().find(|p| {
                p.port == service_port as i32
            })
        })
        .and_then(|p| p.target_port.as_ref())
        .map(|tp| match tp {
            k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::Int(i) => *i as u16,
            k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::String(_) => service_port,
        })
        .unwrap_or(service_port);

    // Create a port forward manager and start forwarding
    let manager = PortForwardManager::new();
    let request = PortForwardRequest {
        namespace: namespace.to_string(),
        pod_name,
        ports: vec![PortMapping {
            local_port,
            pod_port: target_port,
        }],
    };

    manager.start_port_forward(client, request).await
}

#[cfg(not(feature = "kubernetes"))]
pub async fn forward_to_service(
    _client: &K8sClient,
    _namespace: &str,
    _service_name: &str,
    _service_port: u16,
    _local_port: u16,
) -> K8sResult<PortForwardSession> {
    Err(K8sError::Internal(
        "Kubernetes feature not enabled".to_string(),
    ))
}
