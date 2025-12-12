//! Horcrux API Client for Terraform Provider

use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;

/// Client errors
#[derive(Error, Debug)]
pub enum ClientError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("API error: {status} - {message}")]
    Api { status: u16, message: String },
    #[error("Authentication failed")]
    AuthFailed,
    #[error("Resource not found: {0}")]
    NotFound(String),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, ClientError>;

/// Horcrux API Client
#[derive(Clone)]
pub struct HorcruxClient {
    client: reqwest::Client,
    base_url: String,
    token: Option<String>,
}

impl HorcruxClient {
    /// Create a new client
    pub fn new(base_url: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            token: None,
        }
    }

    /// Set authentication token
    pub fn with_token(mut self, token: &str) -> Self {
        self.token = Some(token.to_string());
        self
    }

    /// Authenticate with username and password
    pub async fn authenticate(&mut self, username: &str, password: &str) -> Result<String> {
        #[derive(Serialize)]
        struct LoginRequest<'a> {
            username: &'a str,
            password: &'a str,
        }

        #[derive(Deserialize)]
        struct LoginResponse {
            token: String,
        }

        let response: LoginResponse = self
            .post(
                "/api/auth/login",
                &LoginRequest { username, password },
            )
            .await?;

        self.token = Some(response.token.clone());
        Ok(response.token)
    }

    /// Build headers for requests
    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        if let Some(token) = &self.token {
            if let Ok(value) = HeaderValue::from_str(&format!("Bearer {}", token)) {
                headers.insert(AUTHORIZATION, value);
            }
        }

        headers
    }

    /// GET request
    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let response = self
            .client
            .get(&url)
            .headers(self.headers())
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// POST request
    pub async fn post<T: DeserializeOwned, B: Serialize>(&self, path: &str, body: &B) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let response = self
            .client
            .post(&url)
            .headers(self.headers())
            .json(body)
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// PUT request
    pub async fn put<T: DeserializeOwned, B: Serialize>(&self, path: &str, body: &B) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let response = self
            .client
            .put(&url)
            .headers(self.headers())
            .json(body)
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// DELETE request
    pub async fn delete(&self, path: &str) -> Result<()> {
        let url = format!("{}{}", self.base_url, path);
        let response = self
            .client
            .delete(&url)
            .headers(self.headers())
            .send()
            .await?;

        if response.status().is_success() {
            Ok(())
        } else {
            let status = response.status().as_u16();
            let message = response.text().await.unwrap_or_default();
            Err(ClientError::Api { status, message })
        }
    }

    /// Handle API response
    async fn handle_response<T: DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<T> {
        let status = response.status();

        if status.is_success() {
            let body = response.text().await?;
            Ok(serde_json::from_str(&body)?)
        } else if status.as_u16() == 401 {
            Err(ClientError::AuthFailed)
        } else if status.as_u16() == 404 {
            Err(ClientError::NotFound("Resource not found".to_string()))
        } else {
            let message = response.text().await.unwrap_or_default();
            Err(ClientError::Api {
                status: status.as_u16(),
                message,
            })
        }
    }
}

// ============================================================================
// API Data Types
// ============================================================================

/// VM data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vm {
    pub id: String,
    pub name: String,
    pub hypervisor: String,
    pub architecture: String,
    pub cpus: u32,
    pub memory: u64,
    pub disk_size: u64,
    pub status: String,
    #[serde(default)]
    pub node: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Create VM request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateVmRequest {
    pub id: String,
    pub name: String,
    #[serde(default = "default_hypervisor")]
    pub hypervisor: String,
    #[serde(default = "default_architecture")]
    pub architecture: String,
    pub cpus: u32,
    pub memory: u64,
    pub disk_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tags: Vec<String>,
}

fn default_hypervisor() -> String {
    "Qemu".to_string()
}

fn default_architecture() -> String {
    "X86_64".to_string()
}

/// Update VM request
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateVmRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpus: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

/// Container data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub runtime: String,
    pub image: String,
    pub status: String,
    #[serde(default)]
    pub cpus: Option<f64>,
    #[serde(default)]
    pub memory: Option<u64>,
    #[serde(default)]
    pub ports: Vec<PortMapping>,
    #[serde(default)]
    pub environment: std::collections::HashMap<String, String>,
}

/// Port mapping
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortMapping {
    pub host_port: u16,
    pub container_port: u16,
    #[serde(default = "default_protocol")]
    pub protocol: String,
}

fn default_protocol() -> String {
    "tcp".to_string()
}

/// Create container request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateContainerRequest {
    pub id: String,
    pub name: String,
    pub runtime: String,
    pub image: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpus: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub ports: Vec<PortMapping>,
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty", default)]
    pub environment: std::collections::HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
}

/// Storage pool data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoragePool {
    pub id: String,
    pub name: String,
    pub pool_type: String,
    pub path: Option<String>,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
}

/// Create storage pool request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateStoragePoolRequest {
    pub id: String,
    pub name: String,
    pub pool_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ceph_config: Option<CephConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nfs_config: Option<NfsConfig>,
}

/// Ceph configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CephConfig {
    pub monitors: Vec<String>,
    pub pool_name: String,
    pub user: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyring: Option<String>,
}

/// NFS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NfsConfig {
    pub server: String,
    pub export_path: String,
    #[serde(default = "default_nfs_version")]
    pub version: String,
}

fn default_nfs_version() -> String {
    "4.2".to_string()
}

/// Network data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Network {
    pub id: String,
    pub name: String,
    pub network_type: String,
    pub bridge: Option<String>,
    pub vlan_id: Option<u16>,
    pub subnet: Option<String>,
    pub gateway: Option<String>,
}

/// Create network request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateNetworkRequest {
    pub id: String,
    pub name: String,
    pub network_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bridge: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vlan_id: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subnet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dhcp_range: Option<DhcpRange>,
}

/// DHCP range
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DhcpRange {
    pub start: String,
    pub end: String,
}

/// Firewall rule data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirewallRule {
    pub id: String,
    pub name: String,
    pub action: String,
    pub direction: String,
    pub protocol: Option<String>,
    pub port: Option<u16>,
    pub source: Option<String>,
    pub destination: Option<String>,
    pub enabled: bool,
    pub priority: i32,
}

/// Create firewall rule request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFirewallRuleRequest {
    pub name: String,
    pub action: String,
    #[serde(default = "default_direction")]
    pub direction: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub priority: i32,
}

fn default_direction() -> String {
    "in".to_string()
}

fn default_enabled() -> bool {
    true
}

// ============================================================================
// API Methods
// ============================================================================

impl HorcruxClient {
    // VM operations
    pub async fn list_vms(&self) -> Result<Vec<Vm>> {
        self.get("/api/vms").await
    }

    pub async fn get_vm(&self, id: &str) -> Result<Vm> {
        self.get(&format!("/api/vms/{}", id)).await
    }

    pub async fn create_vm(&self, request: &CreateVmRequest) -> Result<Vm> {
        self.post("/api/vms", request).await
    }

    pub async fn update_vm(&self, id: &str, request: &UpdateVmRequest) -> Result<Vm> {
        self.put(&format!("/api/vms/{}", id), request).await
    }

    pub async fn delete_vm(&self, id: &str) -> Result<()> {
        self.delete(&format!("/api/vms/{}", id)).await
    }

    pub async fn start_vm(&self, id: &str) -> Result<Vm> {
        self.post(&format!("/api/vms/{}/start", id), &()).await
    }

    pub async fn stop_vm(&self, id: &str) -> Result<Vm> {
        self.post(&format!("/api/vms/{}/stop", id), &()).await
    }

    // Container operations
    pub async fn list_containers(&self) -> Result<Vec<Container>> {
        self.get("/api/containers").await
    }

    pub async fn get_container(&self, id: &str) -> Result<Container> {
        self.get(&format!("/api/containers/{}", id)).await
    }

    pub async fn create_container(&self, request: &CreateContainerRequest) -> Result<Container> {
        self.post("/api/containers", request).await
    }

    pub async fn delete_container(&self, id: &str) -> Result<()> {
        self.delete(&format!("/api/containers/{}", id)).await
    }

    pub async fn start_container(&self, id: &str) -> Result<Container> {
        self.post(&format!("/api/containers/{}/start", id), &())
            .await
    }

    pub async fn stop_container(&self, id: &str) -> Result<Container> {
        self.post(&format!("/api/containers/{}/stop", id), &())
            .await
    }

    // Storage operations
    pub async fn list_storage_pools(&self) -> Result<Vec<StoragePool>> {
        self.get("/api/storage").await
    }

    pub async fn get_storage_pool(&self, id: &str) -> Result<StoragePool> {
        self.get(&format!("/api/storage/{}", id)).await
    }

    pub async fn create_storage_pool(&self, request: &CreateStoragePoolRequest) -> Result<StoragePool> {
        self.post("/api/storage", request).await
    }

    pub async fn delete_storage_pool(&self, id: &str) -> Result<()> {
        self.delete(&format!("/api/storage/{}", id)).await
    }

    // Network operations
    pub async fn list_networks(&self) -> Result<Vec<Network>> {
        self.get("/api/networks").await
    }

    pub async fn get_network(&self, id: &str) -> Result<Network> {
        self.get(&format!("/api/networks/{}", id)).await
    }

    pub async fn create_network(&self, request: &CreateNetworkRequest) -> Result<Network> {
        self.post("/api/networks", request).await
    }

    pub async fn delete_network(&self, id: &str) -> Result<()> {
        self.delete(&format!("/api/networks/{}", id)).await
    }

    // Firewall operations
    pub async fn list_firewall_rules(&self) -> Result<Vec<FirewallRule>> {
        self.get("/api/firewall/rules").await
    }

    pub async fn get_firewall_rule(&self, id: &str) -> Result<FirewallRule> {
        self.get(&format!("/api/firewall/rules/{}", id)).await
    }

    pub async fn create_firewall_rule(&self, request: &CreateFirewallRuleRequest) -> Result<FirewallRule> {
        self.post("/api/firewall/rules", request).await
    }

    pub async fn delete_firewall_rule(&self, id: &str) -> Result<()> {
        self.delete(&format!("/api/firewall/rules/{}", id)).await
    }

    pub async fn apply_firewall(&self) -> Result<()> {
        self.post::<serde_json::Value, _>("/api/firewall/apply", &())
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let client = HorcruxClient::new("http://localhost:8006");
        assert!(client.token.is_none());
    }

    #[test]
    fn test_client_with_token() {
        let client = HorcruxClient::new("http://localhost:8006").with_token("test-token");
        assert_eq!(client.token, Some("test-token".to_string()));
    }
}
