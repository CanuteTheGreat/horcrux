//! Kubernetes client wrapper
//!
//! Wraps the kube-rs Client with cluster context and helper methods.

#[cfg(feature = "kubernetes")]
use kube::{Client, Config};

use super::error::{K8sError, K8sResult};
use super::types::{ClusterStatus, K8sVersion};

/// Wrapper around kube-rs Client with cluster context
#[derive(Clone)]
pub struct K8sClient {
    #[cfg(feature = "kubernetes")]
    inner: Client,
    cluster_id: String,
    cluster_name: String,
    api_server: String,
}

impl K8sClient {
    /// Create client from kubeconfig YAML with optional context
    #[cfg(feature = "kubernetes")]
    pub async fn from_kubeconfig(
        kubeconfig_yaml: &str,
        context: Option<&str>,
        cluster_id: String,
        cluster_name: String,
    ) -> K8sResult<Self> {
        use kube::config::{KubeConfigOptions, Kubeconfig};

        let kubeconfig = Kubeconfig::from_yaml(kubeconfig_yaml).map_err(|e| {
            K8sError::InvalidKubeconfig(format!("Failed to parse kubeconfig: {}", e))
        })?;

        // Get the API server URL from kubeconfig
        let api_server = Self::extract_api_server(&kubeconfig, context)?;

        let config = Config::from_custom_kubeconfig(
            kubeconfig,
            &KubeConfigOptions {
                context: context.map(String::from),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| K8sError::InvalidKubeconfig(format!("Failed to create config: {}", e)))?;

        let client = Client::try_from(config)
            .map_err(|e| K8sError::InvalidKubeconfig(format!("Failed to create client: {}", e)))?;

        Ok(Self {
            inner: client,
            cluster_id,
            cluster_name,
            api_server,
        })
    }

    /// Create client from in-cluster configuration (for running inside K8s)
    #[cfg(feature = "kubernetes")]
    pub async fn from_incluster(cluster_id: String, cluster_name: String) -> K8sResult<Self> {
        let config = Config::incluster().map_err(|e| {
            K8sError::InvalidKubeconfig(format!("Failed to get in-cluster config: {}", e))
        })?;

        let api_server = config.cluster_url.to_string();

        let client = Client::try_from(config)
            .map_err(|e| K8sError::InvalidKubeconfig(format!("Failed to create client: {}", e)))?;

        Ok(Self {
            inner: client,
            cluster_id,
            cluster_name,
            api_server,
        })
    }

    /// Extract API server URL from kubeconfig
    #[cfg(feature = "kubernetes")]
    fn extract_api_server(
        kubeconfig: &kube::config::Kubeconfig,
        context_name: Option<&str>,
    ) -> K8sResult<String> {
        // Find the context to use
        let context_name = context_name
            .map(String::from)
            .or_else(|| kubeconfig.current_context.clone())
            .ok_or_else(|| {
                K8sError::InvalidKubeconfig("No context specified and no current-context".into())
            })?;

        let context = kubeconfig
            .contexts
            .iter()
            .find(|c| c.name == context_name)
            .ok_or_else(|| {
                K8sError::InvalidKubeconfig(format!("Context '{}' not found", context_name))
            })?;

        let cluster_name = context.context.as_ref().map(|c| c.cluster.as_str());

        let cluster_name = cluster_name.ok_or_else(|| {
            K8sError::InvalidKubeconfig("Context has no cluster reference".into())
        })?;

        let cluster = kubeconfig
            .clusters
            .iter()
            .find(|c| c.name == cluster_name)
            .ok_or_else(|| {
                K8sError::InvalidKubeconfig(format!("Cluster '{}' not found", cluster_name))
            })?;

        cluster
            .cluster
            .as_ref()
            .and_then(|c| c.server.clone())
            .ok_or_else(|| K8sError::InvalidKubeconfig("Cluster has no server URL".into()))
    }

    /// Get the inner kube-rs Client
    #[cfg(feature = "kubernetes")]
    pub fn inner(&self) -> &Client {
        &self.inner
    }

    /// Get cluster ID
    pub fn cluster_id(&self) -> &str {
        &self.cluster_id
    }

    /// Get cluster name
    pub fn cluster_name(&self) -> &str {
        &self.cluster_name
    }

    /// Get API server URL
    pub fn api_server(&self) -> &str {
        &self.api_server
    }

    /// Check if the cluster is reachable
    #[cfg(feature = "kubernetes")]
    pub async fn health_check(&self) -> K8sResult<ClusterStatus> {
        use kube::api::Api;
        use k8s_openapi::api::core::v1::Namespace;

        let namespaces: Api<Namespace> = Api::all(self.inner.clone());

        match namespaces.list(&Default::default()).await {
            Ok(_) => Ok(ClusterStatus::Connected),
            Err(e) => {
                tracing::warn!("Cluster health check failed: {}", e);
                Ok(ClusterStatus::Error)
            }
        }
    }

    /// Get Kubernetes version information
    #[cfg(feature = "kubernetes")]
    pub async fn get_version(&self) -> K8sResult<K8sVersion> {
        let version = self.inner.apiserver_version().await?;

        Ok(K8sVersion {
            server: format!("{}.{}", version.major, version.minor),
            git_version: version.git_version,
            git_commit: version.git_commit,
            build_date: version.build_date,
            platform: version.platform,
        })
    }

    /// Get number of nodes in the cluster
    #[cfg(feature = "kubernetes")]
    pub async fn get_node_count(&self) -> K8sResult<u32> {
        use kube::api::Api;
        use k8s_openapi::api::core::v1::Node;

        let nodes: Api<Node> = Api::all(self.inner.clone());
        let node_list = nodes.list(&Default::default()).await?;

        Ok(node_list.items.len() as u32)
    }

    // Stub implementations when kubernetes feature is not enabled
    #[cfg(not(feature = "kubernetes"))]
    pub async fn from_kubeconfig(
        _kubeconfig_yaml: &str,
        _context: Option<&str>,
        _cluster_id: String,
        _cluster_name: String,
    ) -> K8sResult<Self> {
        Err(K8sError::Internal(
            "Kubernetes feature not enabled".to_string(),
        ))
    }

    #[cfg(not(feature = "kubernetes"))]
    pub async fn from_incluster(_cluster_id: String, _cluster_name: String) -> K8sResult<Self> {
        Err(K8sError::Internal(
            "Kubernetes feature not enabled".to_string(),
        ))
    }

    #[cfg(not(feature = "kubernetes"))]
    pub async fn health_check(&self) -> K8sResult<ClusterStatus> {
        Err(K8sError::Internal(
            "Kubernetes feature not enabled".to_string(),
        ))
    }

    #[cfg(not(feature = "kubernetes"))]
    pub async fn get_version(&self) -> K8sResult<K8sVersion> {
        Err(K8sError::Internal(
            "Kubernetes feature not enabled".to_string(),
        ))
    }

    #[cfg(not(feature = "kubernetes"))]
    pub async fn get_node_count(&self) -> K8sResult<u32> {
        Err(K8sError::Internal(
            "Kubernetes feature not enabled".to_string(),
        ))
    }
}

impl std::fmt::Debug for K8sClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("K8sClient")
            .field("cluster_id", &self.cluster_id)
            .field("cluster_name", &self.cluster_name)
            .field("api_server", &self.api_server)
            .finish()
    }
}
