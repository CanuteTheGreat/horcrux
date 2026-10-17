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

    /// Database + Vault, with kubeconfigs that fall back to the database
    /// encrypted (AES-256-GCM) under a host key kept in `key_file` (created
    /// with mode 0600 on first start). Kubeconfigs carry cluster-admin
    /// credentials; they used to be written to the database in plaintext
    /// because no encryption manager was ever passed to the store.
    pub async fn with_encrypted_store(
        db: Arc<Database>,
        vault: Arc<VaultManager>,
        key_file: &std::path::Path,
    ) -> K8sResult<Self> {
        use crate::encryption::{EncryptionConfig, EncryptionManager};
        if !key_file.exists() {
            if let Some(dir) = key_file.parent() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| K8sError::Internal(format!("{}: {}", dir.display(), e)))?;
            }
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(key_file)
                .map_err(|e| K8sError::Internal(format!("{}: {}", key_file.display(), e)))?;
            f.write_all(EncryptionManager::generate_key_hex().as_bytes())
                .map_err(|e| K8sError::Internal(format!("{}: {}", key_file.display(), e)))?;
            tracing::info!("Created kubeconfig encryption key {}", key_file.display());
        }
        let enc = Arc::new(EncryptionManager::new());
        enc.initialize(EncryptionConfig {
            master_key: None,
            key_file: Some(key_file.to_string_lossy().to_string()),
            auto_generate: false,
        })
        .await
        .map_err(|e| K8sError::Internal(format!("kubeconfig encryption key: {}", e)))?;
        Ok(Self {
            clusters: Arc::new(RwLock::new(HashMap::new())),
            clients: Arc::new(RwLock::new(HashMap::new())),
            kubeconfig_store: KubeconfigStore::with_encryption(
                Some(db.clone()),
                Some(vault.clone()),
                Some(enc),
            ),
            db: Some(db),
            vault: Some(vault),
        })
    }

    /// Initialize manager and load clusters from database
    pub async fn initialize(&self) -> K8sResult<()> {
        if let Some(db) = &self.db {
            let clusters = config::db::list_clusters(db.pool()).await?;
            // Re-store kubeconfigs that an older version wrote in plaintext,
            // so they end up encrypted at rest.
            for cluster in &clusters {
                let raw: Option<(Option<String>,)> =
                    sqlx::query_as("SELECT kubeconfig_encrypted FROM k8s_clusters WHERE id = ?")
                        .bind(&cluster.id)
                        .fetch_optional(db.pool())
                        .await
                        .map_err(|e| K8sError::Internal(e.to_string()))?;
                if let Some((Some(text),)) = raw {
                    if kube::config::Kubeconfig::from_yaml(&text).is_ok() {
                        if let Err(e) = self.kubeconfig_store.store(&cluster.id, &text).await {
                            tracing::warn!("re-encrypting kubeconfig of {}: {}", cluster.id, e);
                        }
                    }
                }
            }
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
        // Record the context actually used. It used to be stored as the
        // literal "default" whenever the request named none, so reconnecting
        // (and anything else that re-reads the kubeconfig) asked for a
        // context most kubeconfigs don't have.
        let context = context
            .or_else(|| current_context(&request.kubeconfig))
            .unwrap_or_else(|| "default".to_string());
        let cluster = K8sCluster {
            id: cluster_id.clone(),
            name: request.name.clone(),
            context,
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
        let context = usable_context(&kubeconfig, &cluster.context);
        let client = K8sClient::from_kubeconfig(
            &kubeconfig,
            context.as_deref(),
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

    /// Re-create the API clients of every registered cluster after a
    /// restart, in the background. Without this the clusters loaded from the
    /// database showed their last status ("connected") while every workload,
    /// node and config page failed with "cluster not connected" until someone
    /// pressed Reconnect.
    pub fn reconnect_all_in_background(self: &Arc<Self>) {
        let mgr = self.clone();
        tokio::spawn(async move {
            let ids: Vec<String> = mgr.clusters.read().await.keys().cloned().collect();
            for id in ids {
                match mgr.reconnect_cluster(&id).await {
                    Ok(c) => tracing::info!("Kubernetes cluster '{}' reconnected", c.name),
                    Err(e) => {
                        tracing::warn!("Kubernetes cluster {} not reachable at startup: {}", id, e);
                        if let Some(c) = mgr.clusters.write().await.get_mut(&id) {
                            c.status = ClusterStatus::Disconnected;
                        }
                    }
                }
            }
        });
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

    /// The cluster's stored kubeconfig as a private temp file for CLI tools
    /// (helm). Works whether or not the cluster is currently connected.
    pub async fn helm_target(&self, cluster_id: &str) -> K8sResult<helm::KubeTarget> {
        let cluster = self.get_cluster(cluster_id).await?;
        let kubeconfig = self.kubeconfig_store.get(cluster_id).await?;
        let context = usable_context(&kubeconfig, &cluster.context);
        helm::KubeTarget::new(&kubeconfig, context.as_deref())
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

/// `current-context` of a kubeconfig, if it names one.
fn current_context(kubeconfig_yaml: &str) -> Option<String> {
    kube::config::Kubeconfig::from_yaml(kubeconfig_yaml)
        .ok()
        .and_then(|k| k.current_context)
        .filter(|c| !c.is_empty())
}

/// The stored context if the kubeconfig has it; otherwise `None` (= use the
/// kubeconfig's current-context). Clusters registered before the context was
/// recorded correctly carry a placeholder "default".
fn usable_context(kubeconfig_yaml: &str, stored: &str) -> Option<String> {
    let cfg = kube::config::Kubeconfig::from_yaml(kubeconfig_yaml).ok()?;
    if cfg.contexts.iter().any(|c| c.name == stored) {
        Some(stored.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod context_tests {
    use super::*;

    const KUBECONFIG: &str = r#"
apiVersion: v1
kind: Config
clusters:
- name: k3s
  cluster:
    server: https://127.0.0.1:6443
contexts:
- name: k3s-ctx
  context:
    cluster: k3s
    user: admin
current-context: k3s-ctx
users:
- name: admin
  user:
    token: abc
"#;

    #[test]
    fn context_resolution() {
        assert_eq!(current_context(KUBECONFIG).as_deref(), Some("k3s-ctx"));
        assert_eq!(
            usable_context(KUBECONFIG, "k3s-ctx").as_deref(),
            Some("k3s-ctx")
        );
        assert_eq!(usable_context(KUBECONFIG, "default"), None);
    }
}

#[cfg(test)]
mod kubeconfig_encryption_tests {
    use super::*;

    const KC: &str = "apiVersion: v1\nkind: Config\nclusters:\n- name: c\n  cluster:\n    server: https://127.0.0.1:6443\ncontexts:\n- name: x\n  context:\n    cluster: c\n    user: u\ncurrent-context: x\nusers:\n- name: u\n  user:\n    token: s3cret-admin-token\n";

    #[tokio::test]
    async fn kubeconfig_is_encrypted_at_rest_and_plaintext_rows_get_reencrypted() {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}/h.db?mode=rwc", dir.path().display());
        let db = Arc::new(Database::new(&url).await.unwrap());
        db.migrate().await.unwrap();
        let key = dir.path().join("secrets/kubeconfig.key");
        let mgr = KubernetesManager::with_encrypted_store(
            db.clone(),
            Arc::new(VaultManager::new()),
            &key,
        )
        .await
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&key).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let now = chrono::Utc::now().timestamp();
        let cluster = K8sCluster {
            id: "c1".into(),
            name: "c1".into(),
            context: "x".into(),
            api_server: "https://127.0.0.1:6443".into(),
            version: None,
            status: ClusterStatus::Connected,
            node_count: 1,
            provider: types::ClusterProvider::External,
            created_at: now,
            updated_at: now,
        };
        config::db::create_cluster(db.pool(), &cluster)
            .await
            .unwrap();
        // An older version stored it in plaintext.
        sqlx::query("UPDATE k8s_clusters SET kubeconfig_encrypted = ? WHERE id = 'c1'")
            .bind(KC)
            .execute(db.pool())
            .await
            .unwrap();
        mgr.initialize().await.unwrap();
        let (raw,): (String,) =
            sqlx::query_as("SELECT kubeconfig_encrypted FROM k8s_clusters WHERE id = 'c1'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert!(
            !raw.contains("s3cret-admin-token"),
            "kubeconfig still plaintext"
        );
        assert_eq!(mgr.kubeconfig_store.get("c1").await.unwrap(), KC);
        // A second manager (restart) with the same key file reads it back.
        let again =
            KubernetesManager::with_encrypted_store(db, Arc::new(VaultManager::new()), &key)
                .await
                .unwrap();
        assert_eq!(again.kubeconfig_store.get("c1").await.unwrap(), KC);
        let target = {
            again.initialize().await.unwrap();
            again.helm_target("c1").await.unwrap()
        };
        assert_eq!(std::fs::read_to_string(target.path()).unwrap(), KC);
    }
}
