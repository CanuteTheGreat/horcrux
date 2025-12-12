//! Secrets management with HashiCorp Vault integration
//!
//! Securely stores and retrieves sensitive data like passwords, API keys, certificates

#![allow(dead_code)]

use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Vault configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultConfig {
    pub enabled: bool,
    pub address: String,
    pub token: Option<String>,
    pub namespace: Option<String>,
    pub mount_path: String,
    pub tls_skip_verify: bool,
    pub auth_method: VaultAuthMethod,
}

/// Vault authentication method
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum VaultAuthMethod {
    #[serde(rename = "token")]
    Token { token: String },
    #[serde(rename = "approle")]
    AppRole { role_id: String, secret_id: String },
    #[serde(rename = "kubernetes")]
    Kubernetes { role: String, jwt_path: String },
    #[serde(rename = "ldap")]
    Ldap { username: String, password: String },
}

impl Default for VaultConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            address: "http://127.0.0.1:8200".to_string(),
            token: None,
            namespace: None,
            mount_path: "secret".to_string(),
            tls_skip_verify: false,
            auth_method: VaultAuthMethod::Token {
                token: String::new(),
            },
        }
    }
}

/// Secret metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretMetadata {
    pub created_time: String,
    pub deletion_time: Option<String>,
    pub destroyed: bool,
    pub version: u32,
}

/// Secret with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Secret {
    pub data: HashMap<String, String>,
    pub metadata: Option<SecretMetadata>,
}

/// Vault manager
pub struct VaultManager {
    config: Arc<RwLock<VaultConfig>>,
    client: reqwest::Client,
    token_cache: Arc<RwLock<Option<String>>>,
}

impl VaultManager {
    pub fn new() -> Self {
        Self {
            config: Arc::new(RwLock::new(VaultConfig::default())),
            client: reqwest::Client::new(),
            token_cache: Arc::new(RwLock::new(None)),
        }
    }

    /// Get current configuration
    pub async fn get_config(&self) -> VaultConfig {
        self.config.read().await.clone()
    }

    /// Initialize Vault connection
    pub async fn initialize(&self, config: VaultConfig) -> Result<()> {
        // Test connection if enabled
        if config.enabled {
            // Authenticate and get token
            let token = self.authenticate(&config).await?;
            *self.token_cache.write().await = Some(token);

            tracing::info!("Vault connection initialized: {}", config.address);
        }

        *self.config.write().await = config;
        Ok(())
    }

    /// Authenticate with Vault
    async fn authenticate(&self, config: &VaultConfig) -> Result<String> {
        match &config.auth_method {
            VaultAuthMethod::Token { token } => {
                // Verify token is valid
                self.verify_token(config, token).await?;
                Ok(token.clone())
            }
            VaultAuthMethod::AppRole { role_id, secret_id } => {
                self.login_approle(config, role_id, secret_id).await
            }
            VaultAuthMethod::Kubernetes { role, jwt_path } => {
                self.login_kubernetes(config, role, jwt_path).await
            }
            VaultAuthMethod::Ldap { username, password } => {
                self.login_ldap(config, username, password).await
            }
        }
    }

    /// Verify token is valid
    async fn verify_token(&self, config: &VaultConfig, token: &str) -> Result<()> {
        let url = format!("{}/v1/auth/token/lookup-self", config.address);

        let response = self
            .client
            .get(&url)
            .header("X-Vault-Token", token)
            .send()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Vault request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(horcrux_common::Error::System(
                "Invalid Vault token".to_string(),
            ));
        }

        Ok(())
    }

    /// Login with AppRole
    async fn login_approle(
        &self,
        config: &VaultConfig,
        role_id: &str,
        secret_id: &str,
    ) -> Result<String> {
        let url = format!("{}/v1/auth/approle/login", config.address);

        let payload = serde_json::json!({
            "role_id": role_id,
            "secret_id": secret_id,
        });

        let response = self
            .client
            .post(&url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("AppRole login failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(horcrux_common::Error::System(
                "AppRole authentication failed".to_string(),
            ));
        }

        let body: serde_json::Value = response.json().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to parse response: {}", e))
        })?;

        let token = body["auth"]["client_token"]
            .as_str()
            .ok_or_else(|| horcrux_common::Error::System("No token in response".to_string()))?
            .to_string();

        Ok(token)
    }

    /// Login with Kubernetes
    async fn login_kubernetes(
        &self,
        config: &VaultConfig,
        role: &str,
        jwt_path: &str,
    ) -> Result<String> {
        // Read JWT token from file
        let jwt = tokio::fs::read_to_string(jwt_path).await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to read JWT: {}", e))
        })?;

        let url = format!("{}/v1/auth/kubernetes/login", config.address);

        let payload = serde_json::json!({
            "role": role,
            "jwt": jwt.trim(),
        });

        let response = self
            .client
            .post(&url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("K8s login failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(horcrux_common::Error::System(
                "Kubernetes authentication failed".to_string(),
            ));
        }

        let body: serde_json::Value = response.json().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to parse response: {}", e))
        })?;

        let token = body["auth"]["client_token"]
            .as_str()
            .ok_or_else(|| horcrux_common::Error::System("No token in response".to_string()))?
            .to_string();

        Ok(token)
    }

    /// Login with LDAP
    async fn login_ldap(
        &self,
        config: &VaultConfig,
        username: &str,
        password: &str,
    ) -> Result<String> {
        let url = format!("{}/v1/auth/ldap/login/{}", config.address, username);

        let payload = serde_json::json!({
            "password": password,
        });

        let response = self
            .client
            .post(&url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("LDAP login failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(horcrux_common::Error::System(
                "LDAP authentication failed".to_string(),
            ));
        }

        let body: serde_json::Value = response.json().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to parse response: {}", e))
        })?;

        let token = body["auth"]["client_token"]
            .as_str()
            .ok_or_else(|| horcrux_common::Error::System("No token in response".to_string()))?
            .to_string();

        Ok(token)
    }

    /// Get token from cache
    async fn get_token(&self) -> Result<String> {
        let token = self.token_cache.read().await;
        token
            .as_ref()
            .ok_or_else(|| horcrux_common::Error::System("Not authenticated to Vault".to_string()))
            .map(|t| t.clone())
    }

    /// Read secret from Vault
    pub async fn read_secret(&self, path: &str) -> Result<Secret> {
        let config = self.config.read().await;

        if !config.enabled {
            return Err(horcrux_common::Error::InvalidConfig(
                "Vault is not enabled".to_string(),
            ));
        }

        let token = self.get_token().await?;
        let url = format!(
            "{}/v1/{}/data/{}",
            config.address, config.mount_path, path
        );

        let mut request = self.client.get(&url).header("X-Vault-Token", token);

        if let Some(ref ns) = config.namespace {
            request = request.header("X-Vault-Namespace", ns);
        }

        let response = request.send().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to read secret: {}", e))
        })?;

        if !response.status().is_success() {
            let status = response.status();
            return Err(horcrux_common::Error::System(format!(
                "Failed to read secret: HTTP {}",
                status
            )));
        }

        let body: serde_json::Value = response.json().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to parse response: {}", e))
        })?;

        // Parse Vault KV v2 response
        let data: HashMap<String, String> = body["data"]["data"]
            .as_object()
            .ok_or_else(|| horcrux_common::Error::System("Invalid secret format".to_string()))?
            .iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
            .collect();

        let metadata = if let Some(meta) = body["data"]["metadata"].as_object() {
            Some(SecretMetadata {
                created_time: meta
                    .get("created_time")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                deletion_time: meta
                    .get("deletion_time")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                destroyed: meta
                    .get("destroyed")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                version: meta
                    .get("version")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32,
            })
        } else {
            None
        };

        Ok(Secret { data, metadata })
    }

    /// Write secret to Vault
    pub async fn write_secret(&self, path: &str, data: HashMap<String, String>) -> Result<()> {
        let config = self.config.read().await;

        if !config.enabled {
            return Err(horcrux_common::Error::InvalidConfig(
                "Vault is not enabled".to_string(),
            ));
        }

        let token = self.get_token().await?;
        let url = format!(
            "{}/v1/{}/data/{}",
            config.address, config.mount_path, path
        );

        let payload = serde_json::json!({
            "data": data,
        });

        let mut request = self
            .client
            .post(&url)
            .header("X-Vault-Token", token)
            .json(&payload);

        if let Some(ref ns) = config.namespace {
            request = request.header("X-Vault-Namespace", ns);
        }

        let response = request.send().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to write secret: {}", e))
        })?;

        if !response.status().is_success() {
            let status = response.status();
            return Err(horcrux_common::Error::System(format!(
                "Failed to write secret: HTTP {}",
                status
            )));
        }

        tracing::info!("Secret written to Vault: {}", path);

        Ok(())
    }

    /// Delete secret from Vault
    pub async fn delete_secret(&self, path: &str) -> Result<()> {
        let config = self.config.read().await;

        if !config.enabled {
            return Err(horcrux_common::Error::InvalidConfig(
                "Vault is not enabled".to_string(),
            ));
        }

        let token = self.get_token().await?;
        let url = format!(
            "{}/v1/{}/metadata/{}",
            config.address, config.mount_path, path
        );

        let mut request = self.client.delete(&url).header("X-Vault-Token", token);

        if let Some(ref ns) = config.namespace {
            request = request.header("X-Vault-Namespace", ns);
        }

        let response = request.send().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to delete secret: {}", e))
        })?;

        if !response.status().is_success() {
            let status = response.status();
            return Err(horcrux_common::Error::System(format!(
                "Failed to delete secret: HTTP {}",
                status
            )));
        }

        tracing::info!("Secret deleted from Vault: {}", path);

        Ok(())
    }

    /// List secrets at path
    pub async fn list_secrets(&self, path: &str) -> Result<Vec<String>> {
        let config = self.config.read().await;

        if !config.enabled {
            return Err(horcrux_common::Error::InvalidConfig(
                "Vault is not enabled".to_string(),
            ));
        }

        let token = self.get_token().await?;
        let url = format!(
            "{}/v1/{}/metadata/{}",
            config.address, config.mount_path, path
        );

        let mut request = self
            .client
            .request(reqwest::Method::from_bytes(b"LIST").unwrap(), &url)
            .header("X-Vault-Token", token);

        if let Some(ref ns) = config.namespace {
            request = request.header("X-Vault-Namespace", ns);
        }

        let response = request.send().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to list secrets: {}", e))
        })?;

        if !response.status().is_success() {
            let status = response.status();
            return Err(horcrux_common::Error::System(format!(
                "Failed to list secrets: HTTP {}",
                status
            )));
        }

        let body: serde_json::Value = response.json().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to parse response: {}", e))
        })?;

        let keys = body["data"]["keys"]
            .as_array()
            .ok_or_else(|| horcrux_common::Error::System("Invalid list response".to_string()))?
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();

        Ok(keys)
    }

    /// Renew authentication token
    pub async fn renew_token(&self) -> Result<()> {
        let config = self.config.read().await;

        if !config.enabled {
            return Ok(());
        }

        let token = self.get_token().await?;
        let url = format!("{}/v1/auth/token/renew-self", config.address);

        let response = self
            .client
            .post(&url)
            .header("X-Vault-Token", &token)
            .send()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Token renewal failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(horcrux_common::Error::System(
                "Failed to renew token".to_string(),
            ));
        }

        tracing::info!("Vault token renewed successfully");

        Ok(())
    }

    /// Check if Vault is sealed
    pub async fn is_sealed(&self) -> Result<bool> {
        let config = self.config.read().await;

        if !config.enabled {
            return Ok(false);
        }

        let url = format!("{}/v1/sys/seal-status", config.address);

        let response = self.client.get(&url).send().await.map_err(|e| {
            horcrux_common::Error::System(format!("Seal status check failed: {}", e))
        })?;

        if !response.status().is_success() {
            return Err(horcrux_common::Error::System(
                "Failed to check seal status".to_string(),
            ));
        }

        let body: serde_json::Value = response.json().await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to parse response: {}", e))
        })?;

        let sealed = body["sealed"].as_bool().unwrap_or(true);

        Ok(sealed)
    }
}

/// Helper functions for common secret operations

impl VaultManager {
    /// Store VM password
    pub async fn store_vm_password(&self, vm_id: &str, password: &str) -> Result<()> {
        let mut data = HashMap::new();
        data.insert("password".to_string(), password.to_string());
        self.write_secret(&format!("vms/{}/password", vm_id), data)
            .await
    }

    /// Retrieve VM password
    pub async fn get_vm_password(&self, vm_id: &str) -> Result<String> {
        let secret = self.read_secret(&format!("vms/{}/password", vm_id)).await?;
        secret
            .data
            .get("password")
            .cloned()
            .ok_or_else(|| horcrux_common::Error::System("Password not found".to_string()))
    }

    /// Store API token
    pub async fn store_api_token(&self, service: &str, token: &str) -> Result<()> {
        let mut data = HashMap::new();
        data.insert("token".to_string(), token.to_string());
        self.write_secret(&format!("tokens/{}", service), data)
            .await
    }

    /// Retrieve API token
    pub async fn get_api_token(&self, service: &str) -> Result<String> {
        let secret = self.read_secret(&format!("tokens/{}", service)).await?;
        secret
            .data
            .get("token")
            .cloned()
            .ok_or_else(|| horcrux_common::Error::System("Token not found".to_string()))
    }

    /// Store database credentials
    pub async fn store_db_credentials(
        &self,
        db_name: &str,
        username: &str,
        password: &str,
    ) -> Result<()> {
        let mut data = HashMap::new();
        data.insert("username".to_string(), username.to_string());
        data.insert("password".to_string(), password.to_string());
        self.write_secret(&format!("databases/{}", db_name), data)
            .await
    }

    /// Retrieve database credentials
    pub async fn get_db_credentials(&self, db_name: &str) -> Result<(String, String)> {
        let secret = self.read_secret(&format!("databases/{}", db_name)).await?;
        let username = secret
            .data
            .get("username")
            .cloned()
            .ok_or_else(|| horcrux_common::Error::System("Username not found".to_string()))?;
        let password = secret
            .data
            .get("password")
            .cloned()
            .ok_or_else(|| horcrux_common::Error::System("Password not found".to_string()))?;
        Ok((username, password))
    }

    /// Store kubeconfig for a Kubernetes cluster
    pub async fn store_kubeconfig(&self, cluster_id: &str, kubeconfig: &str) -> Result<()> {
        let mut data = HashMap::new();
        data.insert("kubeconfig".to_string(), kubeconfig.to_string());
        self.write_secret(&format!("kubernetes/clusters/{}/kubeconfig", cluster_id), data)
            .await
    }

    /// Retrieve kubeconfig for a Kubernetes cluster
    pub async fn get_kubeconfig(&self, cluster_id: &str) -> Result<String> {
        let secret = self
            .read_secret(&format!("kubernetes/clusters/{}/kubeconfig", cluster_id))
            .await?;
        secret
            .data
            .get("kubeconfig")
            .cloned()
            .ok_or_else(|| horcrux_common::Error::System("Kubeconfig not found".to_string()))
    }

    /// Delete kubeconfig for a Kubernetes cluster
    pub async fn delete_kubeconfig(&self, cluster_id: &str) -> Result<()> {
        self.delete_secret(&format!("kubernetes/clusters/{}/kubeconfig", cluster_id))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vault_config_default() {
        let config = VaultConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.address, "http://127.0.0.1:8200");
        assert_eq!(config.mount_path, "secret");
    }

    #[tokio::test]
    async fn test_vault_manager_creation() {
        let manager = VaultManager::new();
        let config = manager.config.read().await;
        assert!(!config.enabled);
    }

    #[tokio::test]
    async fn test_vault_disabled_operations() {
        let manager = VaultManager::new();

        // Reading should fail when vault is disabled
        let result = manager.read_secret("test/path").await;
        assert!(result.is_err());
    }
}
