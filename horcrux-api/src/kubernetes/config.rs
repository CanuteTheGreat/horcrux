//! Kubeconfig storage and management
//!
//! Handles secure storage of kubeconfig credentials using Vault (when enabled)
//! or encrypted database storage as fallback.

use super::error::{K8sError, K8sResult};
use super::types::K8sCluster;
use crate::db::Database;
use crate::encryption::EncryptionManager;
use crate::secrets::VaultManager;
use std::sync::Arc;

/// Kubeconfig storage manager
pub struct KubeconfigStore {
    db: Option<Arc<Database>>,
    vault: Option<Arc<VaultManager>>,
    encryption: Option<Arc<EncryptionManager>>,
}

impl KubeconfigStore {
    /// Create a new kubeconfig store
    pub fn new(db: Option<Arc<Database>>, vault: Option<Arc<VaultManager>>) -> Self {
        Self {
            db,
            vault,
            encryption: None,
        }
    }

    /// Create a new kubeconfig store with encryption
    pub fn with_encryption(
        db: Option<Arc<Database>>,
        vault: Option<Arc<VaultManager>>,
        encryption: Option<Arc<EncryptionManager>>,
    ) -> Self {
        Self { db, vault, encryption }
    }

    /// Store kubeconfig for a cluster
    pub async fn store(&self, cluster_id: &str, kubeconfig: &str) -> K8sResult<()> {
        // Try Vault first if available
        if let Some(vault) = &self.vault {
            let config = vault.get_config().await;
            if config.enabled {
                let mut data = std::collections::HashMap::new();
                data.insert("kubeconfig".to_string(), kubeconfig.to_string());

                vault
                    .write_secret(&format!("kubernetes/clusters/{}/kubeconfig", cluster_id), data)
                    .await
                    .map_err(|e| K8sError::Internal(format!("Failed to store in Vault: {}", e)))?;

                tracing::info!(
                    "Stored kubeconfig for cluster {} in Vault",
                    cluster_id
                );
                return Ok(());
            }
        }

        // Fall back to encrypted database storage
        if let Some(db) = &self.db {
            // Encrypt the kubeconfig if encryption is available
            let encrypted_kubeconfig = if let Some(encryption) = &self.encryption {
                if encryption.is_available().await {
                    encryption
                        .encrypt_string(kubeconfig)
                        .await
                        .map_err(|e| K8sError::Internal(format!("Encryption failed: {}", e)))?
                } else {
                    tracing::warn!(
                        "Encryption manager not initialized, storing kubeconfig unencrypted for cluster {}",
                        cluster_id
                    );
                    kubeconfig.to_string()
                }
            } else {
                tracing::warn!(
                    "No encryption manager, storing kubeconfig unencrypted for cluster {}",
                    cluster_id
                );
                kubeconfig.to_string()
            };

            sqlx::query(
                "UPDATE k8s_clusters SET kubeconfig_encrypted = ?, updated_at = ? WHERE id = ?",
            )
            .bind(&encrypted_kubeconfig)
            .bind(chrono::Utc::now().timestamp())
            .bind(cluster_id)
            .execute(db.pool())
            .await
            .map_err(|e| K8sError::Internal(format!("Failed to store kubeconfig: {}", e)))?;

            tracing::info!(
                "Stored encrypted kubeconfig for cluster {} in database",
                cluster_id
            );
            return Ok(());
        }

        Err(K8sError::Internal(
            "No storage backend available for kubeconfig".to_string(),
        ))
    }

    /// Retrieve kubeconfig for a cluster
    pub async fn get(&self, cluster_id: &str) -> K8sResult<String> {
        // Try Vault first if available
        if let Some(vault) = &self.vault {
            let config = vault.get_config().await;
            if config.enabled {
                match vault
                    .read_secret(&format!("kubernetes/clusters/{}/kubeconfig", cluster_id))
                    .await
                {
                    Ok(secret) => {
                        if let Some(kubeconfig) = secret.data.get("kubeconfig") {
                            return Ok(kubeconfig.clone());
                        }
                    }
                    Err(e) => {
                        tracing::debug!(
                            "Kubeconfig not found in Vault for {}: {}",
                            cluster_id,
                            e
                        );
                    }
                }
            }
        }

        // Fall back to database
        if let Some(db) = &self.db {
            let row: Option<(Option<String>,)> = sqlx::query_as(
                "SELECT kubeconfig_encrypted FROM k8s_clusters WHERE id = ?",
            )
            .bind(cluster_id)
            .fetch_optional(db.pool())
            .await
            .map_err(|e| K8sError::Internal(format!("Database query failed: {}", e)))?;

            if let Some((Some(encrypted_kubeconfig),)) = row {
                // Decrypt the kubeconfig if encryption is available
                let kubeconfig = if let Some(encryption) = &self.encryption {
                    if encryption.is_available().await {
                        // Try to decrypt - if it fails, it might be stored unencrypted
                        match encryption.decrypt_string(&encrypted_kubeconfig).await {
                            Ok(decrypted) => decrypted,
                            Err(e) => {
                                tracing::debug!(
                                    "Failed to decrypt kubeconfig for {}, assuming unencrypted: {}",
                                    cluster_id,
                                    e
                                );
                                encrypted_kubeconfig
                            }
                        }
                    } else {
                        encrypted_kubeconfig
                    }
                } else {
                    encrypted_kubeconfig
                };

                return Ok(kubeconfig);
            }
        }

        Err(K8sError::ClusterNotFound(cluster_id.to_string()))
    }

    /// Delete kubeconfig for a cluster
    pub async fn delete(&self, cluster_id: &str) -> K8sResult<()> {
        // Try to delete from Vault
        if let Some(vault) = &self.vault {
            let config = vault.get_config().await;
            if config.enabled {
                let _ = vault
                    .delete_secret(&format!("kubernetes/clusters/{}/kubeconfig", cluster_id))
                    .await;
            }
        }

        // Delete from database (clear the encrypted field)
        if let Some(db) = &self.db {
            sqlx::query(
                "UPDATE k8s_clusters SET kubeconfig_encrypted = NULL, updated_at = ? WHERE id = ?",
            )
            .bind(chrono::Utc::now().timestamp())
            .bind(cluster_id)
            .execute(db.pool())
            .await
            .map_err(|e| K8sError::Internal(format!("Failed to delete kubeconfig: {}", e)))?;
        }

        Ok(())
    }
}

/// Database operations for K8s clusters
pub mod db {
    use super::*;
    use sqlx::SqlitePool;

    /// Create a new cluster record
    pub async fn create_cluster(pool: &SqlitePool, cluster: &K8sCluster) -> K8sResult<()> {
        sqlx::query(
            r#"
            INSERT INTO k8s_clusters (id, name, context, api_server, version, status, node_count, provider, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&cluster.id)
        .bind(&cluster.name)
        .bind(&cluster.context)
        .bind(&cluster.api_server)
        .bind(&cluster.version)
        .bind(cluster.status.to_string())
        .bind(cluster.node_count as i64)
        .bind(cluster.provider.to_string())
        .bind(cluster.created_at)
        .bind(cluster.updated_at)
        .execute(pool)
        .await
        .map_err(|e| K8sError::Internal(format!("Failed to create cluster: {}", e)))?;

        Ok(())
    }

    /// Get a cluster by ID
    pub async fn get_cluster(pool: &SqlitePool, id: &str) -> K8sResult<K8sCluster> {
        let row: (
            String,
            String,
            String,
            String,
            Option<String>,
            String,
            i64,
            String,
            i64,
            i64,
        ) = sqlx::query_as(
            r#"
            SELECT id, name, context, api_server, version, status, node_count, provider, created_at, updated_at
            FROM k8s_clusters WHERE id = ?
            "#,
        )
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| K8sError::Internal(format!("Database query failed: {}", e)))?
        .ok_or_else(|| K8sError::ClusterNotFound(id.to_string()))?;

        Ok(K8sCluster {
            id: row.0,
            name: row.1,
            context: row.2,
            api_server: row.3,
            version: row.4,
            status: parse_cluster_status(&row.5),
            node_count: row.6 as u32,
            provider: parse_cluster_provider(&row.7),
            created_at: row.8,
            updated_at: row.9,
        })
    }

    /// List all clusters
    pub async fn list_clusters(pool: &SqlitePool) -> K8sResult<Vec<K8sCluster>> {
        let rows: Vec<(
            String,
            String,
            String,
            String,
            Option<String>,
            String,
            i64,
            String,
            i64,
            i64,
        )> = sqlx::query_as(
            r#"
            SELECT id, name, context, api_server, version, status, node_count, provider, created_at, updated_at
            FROM k8s_clusters ORDER BY created_at DESC
            "#,
        )
        .fetch_all(pool)
        .await
        .map_err(|e| K8sError::Internal(format!("Database query failed: {}", e)))?;

        Ok(rows
            .into_iter()
            .map(|row| K8sCluster {
                id: row.0,
                name: row.1,
                context: row.2,
                api_server: row.3,
                version: row.4,
                status: parse_cluster_status(&row.5),
                node_count: row.6 as u32,
                provider: parse_cluster_provider(&row.7),
                created_at: row.8,
                updated_at: row.9,
            })
            .collect())
    }

    /// Update cluster status
    pub async fn update_cluster_status(
        pool: &SqlitePool,
        id: &str,
        status: &super::super::types::ClusterStatus,
        version: Option<&str>,
        node_count: Option<u32>,
    ) -> K8sResult<()> {
        let mut query = String::from("UPDATE k8s_clusters SET status = ?, updated_at = ?");
        let now = chrono::Utc::now().timestamp();

        if version.is_some() {
            query.push_str(", version = ?");
        }
        if node_count.is_some() {
            query.push_str(", node_count = ?");
        }
        query.push_str(" WHERE id = ?");

        let mut q = sqlx::query(&query)
            .bind(status.to_string())
            .bind(now);

        if let Some(v) = version {
            q = q.bind(v);
        }
        if let Some(n) = node_count {
            q = q.bind(n as i64);
        }
        q = q.bind(id);

        q.execute(pool)
            .await
            .map_err(|e| K8sError::Internal(format!("Failed to update cluster: {}", e)))?;

        Ok(())
    }

    /// Delete a cluster
    pub async fn delete_cluster(pool: &SqlitePool, id: &str) -> K8sResult<()> {
        sqlx::query("DELETE FROM k8s_clusters WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await
            .map_err(|e| K8sError::Internal(format!("Failed to delete cluster: {}", e)))?;

        Ok(())
    }

    /// Check if cluster name exists
    pub async fn cluster_name_exists(pool: &SqlitePool, name: &str) -> K8sResult<bool> {
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM k8s_clusters WHERE name = ?")
            .bind(name)
            .fetch_one(pool)
            .await
            .map_err(|e| K8sError::Internal(format!("Database query failed: {}", e)))?;

        Ok(count.0 > 0)
    }

    fn parse_cluster_status(s: &str) -> super::super::types::ClusterStatus {
        use super::super::types::ClusterStatus;
        match s {
            "connected" => ClusterStatus::Connected,
            "disconnected" => ClusterStatus::Disconnected,
            "provisioning" => ClusterStatus::Provisioning,
            "error" => ClusterStatus::Error,
            _ => ClusterStatus::Unknown,
        }
    }

    fn parse_cluster_provider(s: &str) -> super::super::types::ClusterProvider {
        use super::super::types::ClusterProvider;
        match s {
            "external" => ClusterProvider::External,
            "k3s" => ClusterProvider::K3s,
            "kubeadm" => ClusterProvider::Kubeadm,
            "managed" => ClusterProvider::Managed,
            _ => ClusterProvider::External,
        }
    }
}
