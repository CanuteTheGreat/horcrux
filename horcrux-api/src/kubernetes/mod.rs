//! Kubernetes integration for Horcrux
//!
//! Provides comprehensive Kubernetes cluster management including:
//! - Cluster connection via kubeconfig
//! - Cluster provisioning (k3s, kubeadm)
//! - Workload management (Pods, Deployments, StatefulSets, etc.)
//! - Networking (Services, Ingress, NetworkPolicies)
//! - Configuration (ConfigMaps, Secrets)
//! - Storage (PVCs, PVs, StorageClasses)
//! - Observability (Metrics, Events, Logs)
//! - Helm chart management

// Allow dead code for library functions that are exposed for API consumers
#![allow(dead_code)]

pub mod client;
pub mod config;
pub mod error;
pub mod types;

// Sub-modules for different resource types
pub mod cluster;
pub mod cluster_resources;
pub mod config_storage;
pub mod exec;
pub mod helm;
pub mod networking;
pub mod observability;
pub mod portforward;
pub mod watch;
pub mod workloads;

use crate::db::Database;
use crate::secrets::VaultManager;
use client::K8sClient;
use config::KubeconfigStore;
use error::{K8sError, K8sResult};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use types::{ClusterConnectRequest, ClusterHealth, ClusterStatus, K8sCluster, K8sVersion};

/// Main Kubernetes manager
///
/// Handles cluster connections, stores clients, and delegates operations
/// to specialized sub-managers.
pub struct KubernetesManager {
    /// Connected clusters indexed by cluster_id
    clusters: Arc<RwLock<HashMap<String, K8sCluster>>>,
    /// Kube clients per cluster
    clients: Arc<RwLock<HashMap<String, K8sClient>>>,
    /// Kubeconfig storage
    kubeconfig_store: KubeconfigStore,
    /// Database connection
    db: Option<Arc<Database>>,
    /// Vault manager for secrets
    vault: Option<Arc<VaultManager>>,
}

impl KubernetesManager {
    /// Create a new KubernetesManager
    pub fn new() -> Self {
        Self {
            clusters: Arc::new(RwLock::new(HashMap::new())),
            clients: Arc::new(RwLock::new(HashMap::new())),
            kubeconfig_store: KubeconfigStore::new(None, None),
            db: None,
            vault: None,
        }
    }

    /// Create with database support
    pub fn with_database(db: Arc<Database>) -> Self {
        Self {
            clusters: Arc::new(RwLock::new(HashMap::new())),
            clients: Arc::new(RwLock::new(HashMap::new())),
            kubeconfig_store: KubeconfigStore::new(Some(db.clone()), None),
            db: Some(db),
            vault: None,
        }
    }

    /// Create with database and vault support
    pub fn with_database_and_vault(db: Arc<Database>, vault: Arc<VaultManager>) -> Self {
        Self {
            clusters: Arc::new(RwLock::new(HashMap::new())),
            clients: Arc::new(RwLock::new(HashMap::new())),
            kubeconfig_store: KubeconfigStore::new(Some(db.clone()), Some(vault.clone())),
            db: Some(db),
            vault: Some(vault),
        }
    }

    /// Initialize manager and load clusters from database
    pub async fn initialize(&self) -> K8sResult<()> {
        if let Some(db) = &self.db {
            let clusters = config::db::list_clusters(db.pool()).await?;
            let mut clusters_map = self.clusters.write().await;

            for cluster in clusters {
                tracing::info!("Loaded cluster '{}' from database", cluster.name);
                clusters_map.insert(cluster.id.clone(), cluster);
            }

            tracing::info!(
                "Kubernetes manager initialized with {} clusters",
                clusters_map.len()
            );
        }

        Ok(())
    }

    /// Connect to a Kubernetes cluster via kubeconfig
    pub async fn connect_cluster(&self, request: ClusterConnectRequest) -> K8sResult<K8sCluster> {
        // Validate cluster name uniqueness
        if let Some(db) = &self.db {
            if config::db::cluster_name_exists(db.pool(), &request.name).await? {
                return Err(K8sError::Conflict(format!(
                    "Cluster with name '{}' already exists",
                    request.name
                )));
            }
        }

        let cluster_id = uuid::Uuid::new_v4().to_string();
        let context = request.context.clone();

        // Create the K8s client
        let client = K8sClient::from_kubeconfig(
            &request.kubeconfig,
            context.as_deref(),
            cluster_id.clone(),
            request.name.clone(),
        )
        .await?;

        // Verify connection
        let status = client.health_check().await?;
        if status != ClusterStatus::Connected {
            return Err(K8sError::ClusterNotConnected(
                "Failed to connect to cluster".to_string(),
            ));
        }

        // Get cluster info
        let version = client.get_version().await.ok();
        let node_count = client.get_node_count().await.unwrap_or(0);

        let now = chrono::Utc::now().timestamp();
        let cluster = K8sCluster {
            id: cluster_id.clone(),
            name: request.name.clone(),
            context: context.unwrap_or_else(|| "default".to_string()),
            api_server: client.api_server().to_string(),
            version: version.as_ref().map(|v| v.git_version.clone()),
            status: ClusterStatus::Connected,
            node_count,
            provider: types::ClusterProvider::External,
            created_at: now,
            updated_at: now,
        };

        // Store in database
        if let Some(db) = &self.db {
            config::db::create_cluster(db.pool(), &cluster).await?;
        }

        // Store kubeconfig securely
        self.kubeconfig_store
            .store(&cluster_id, &request.kubeconfig)
            .await?;

        // Add to in-memory maps
        {
            let mut clusters = self.clusters.write().await;
            clusters.insert(cluster_id.clone(), cluster.clone());
        }
        {
            let mut clients = self.clients.write().await;
            clients.insert(cluster_id, client);
        }

        tracing::info!("Connected to cluster '{}' ({})", cluster.name, cluster.id);

        Ok(cluster)
    }

    /// Disconnect from a cluster (keeps record in database)
    pub async fn disconnect_cluster(&self, cluster_id: &str) -> K8sResult<()> {
        // Remove client
        {
            let mut clients = self.clients.write().await;
            clients.remove(cluster_id);
        }

        // Update status in memory
        {
            let mut clusters = self.clusters.write().await;
            if let Some(cluster) = clusters.get_mut(cluster_id) {
                cluster.status = ClusterStatus::Disconnected;
                cluster.updated_at = chrono::Utc::now().timestamp();
            }
        }

        // Update status in database
        if let Some(db) = &self.db {
            config::db::update_cluster_status(
                db.pool(),
                cluster_id,
                &ClusterStatus::Disconnected,
                None,
                None,
            )
            .await?;
        }

        tracing::info!("Disconnected from cluster {}", cluster_id);

        Ok(())
    }

    /// Delete a cluster (removes from database)
    pub async fn delete_cluster(&self, cluster_id: &str) -> K8sResult<()> {
        // Disconnect first
        self.disconnect_cluster(cluster_id).await?;

        // Remove kubeconfig
        self.kubeconfig_store.delete(cluster_id).await?;

        // Remove from database
        if let Some(db) = &self.db {
            config::db::delete_cluster(db.pool(), cluster_id).await?;
        }

        // Remove from memory
        {
            let mut clusters = self.clusters.write().await;
            clusters.remove(cluster_id);
        }

        tracing::info!("Deleted cluster {}", cluster_id);

        Ok(())
    }

    /// Reconnect to a previously connected cluster
    pub async fn reconnect_cluster(&self, cluster_id: &str) -> K8sResult<K8sCluster> {
        // Get kubeconfig from storage
        let kubeconfig = self.kubeconfig_store.get(cluster_id).await?;

        // Get cluster info from database
        let cluster = if let Some(db) = &self.db {
            config::db::get_cluster(db.pool(), cluster_id).await?
        } else {
            let clusters = self.clusters.read().await;
            clusters
                .get(cluster_id)
                .cloned()
                .ok_or_else(|| K8sError::ClusterNotFound(cluster_id.to_string()))?
        };

        // Create client
        let client = K8sClient::from_kubeconfig(
            &kubeconfig,
            Some(&cluster.context),
            cluster_id.to_string(),
            cluster.name.clone(),
        )
        .await?;

        // Verify connection
        let status = client.health_check().await?;
        let version = client.get_version().await.ok();
        let node_count = client.get_node_count().await.unwrap_or(0);

        // Update cluster info
        let mut updated_cluster = cluster.clone();
        updated_cluster.status = status;
        updated_cluster.version = version.as_ref().map(|v| v.git_version.clone());
        updated_cluster.node_count = node_count;
        updated_cluster.updated_at = chrono::Utc::now().timestamp();

        // Update database
        if let Some(db) = &self.db {
            config::db::update_cluster_status(
                db.pool(),
                cluster_id,
                &updated_cluster.status,
                updated_cluster.version.as_deref(),
                Some(node_count),
            )
            .await?;
        }

        // Update in-memory state
        {
            let mut clusters = self.clusters.write().await;
            clusters.insert(cluster_id.to_string(), updated_cluster.clone());
        }
        {
            let mut clients = self.clients.write().await;
            clients.insert(cluster_id.to_string(), client);
        }

        tracing::info!(
            "Reconnected to cluster '{}' ({})",
            updated_cluster.name,
            cluster_id
        );

        Ok(updated_cluster)
    }

    /// Get cluster by ID
    pub async fn get_cluster(&self, cluster_id: &str) -> K8sResult<K8sCluster> {
        let clusters = self.clusters.read().await;
        clusters
            .get(cluster_id)
            .cloned()
            .ok_or_else(|| K8sError::ClusterNotFound(cluster_id.to_string()))
    }

    /// List all clusters
    pub async fn list_clusters(&self) -> Vec<K8sCluster> {
        let clusters = self.clusters.read().await;
        clusters.values().cloned().collect()
    }

    /// Get the kube client for a cluster
    pub async fn get_client(&self, cluster_id: &str) -> K8sResult<K8sClient> {
        let clients = self.clients.read().await;
        clients
            .get(cluster_id)
            .cloned()
            .ok_or_else(|| K8sError::ClusterNotConnected(cluster_id.to_string()))
    }

    /// Check cluster health
    pub async fn check_cluster_health(&self, cluster_id: &str) -> K8sResult<ClusterHealth> {
        let client = self.get_client(cluster_id).await?;
        cluster::health::check_health(&client).await
    }

    /// Get cluster version info
    pub async fn get_cluster_version(&self, cluster_id: &str) -> K8sResult<K8sVersion> {
        let client = self.get_client(cluster_id).await?;
        client.get_version().await
    }

    /// Refresh cluster status
    pub async fn refresh_cluster_status(&self, cluster_id: &str) -> K8sResult<ClusterStatus> {
        let client = self.get_client(cluster_id).await?;
        let status = client.health_check().await?;

        // Update in memory
        {
            let mut clusters = self.clusters.write().await;
            if let Some(cluster) = clusters.get_mut(cluster_id) {
                cluster.status = status;
                cluster.updated_at = chrono::Utc::now().timestamp();
            }
        }

        // Update in database
        if let Some(db) = &self.db {
            config::db::update_cluster_status(db.pool(), cluster_id, &status, None, None).await?;
        }

        Ok(status)
    }
}

impl Default for KubernetesManager {
    fn default() -> Self {
        Self::new()
    }
}
