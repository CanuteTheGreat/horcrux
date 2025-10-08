//! API client for Horcrux backend

use gloo_net::http::Request;
use gloo_storage::{LocalStorage, Storage};
use serde::{Deserialize, Serialize};
use wasm_bindgen_futures::spawn_local;

const API_BASE: &str = "/api";
const TOKEN_KEY: &str = "horcrux_token";

/// API client for backend communication
pub struct ApiClient;

impl ApiClient {
    /// Get authentication token from local storage
    fn get_token() -> Option<String> {
        LocalStorage::get(TOKEN_KEY).ok()
    }

    /// Set authentication token in local storage
    pub fn set_token(token: String) {
        let _ = LocalStorage::set(TOKEN_KEY, token);
    }

    /// Clear authentication token
    pub fn clear_token() {
        LocalStorage::delete(TOKEN_KEY);
    }

    /// Make authenticated GET request
    pub async fn get<T: for<'de> Deserialize<'de>>(path: &str) -> Result<T, String> {
        let url = format!("{}{}", API_BASE, path);

        let mut request = Request::get(&url);

        if let Some(token) = Self::get_token() {
            request = request.header("Authorization", &format!("Bearer {}", token));
        }

        let response = request
            .send()
            .await
            .map_err(|e| format!("Request failed: {}", e))?;

        if !response.ok() {
            return Err(format!("HTTP {}: {}", response.status(), response.status_text()));
        }

        response
            .json()
            .await
            .map_err(|e| format!("JSON parse error: {}", e))
    }

    /// Make authenticated POST request
    pub async fn post<B: Serialize, T: for<'de> Deserialize<'de>>(
        path: &str,
        body: &B,
    ) -> Result<T, String> {
        let url = format!("{}{}", API_BASE, path);

        let mut request = Request::post(&url);

        if let Some(token) = Self::get_token() {
            request = request.header("Authorization", &format!("Bearer {}", token));
        }

        let response = request
            .json(body)
            .map_err(|e| format!("JSON serialize error: {}", e))?
            .send()
            .await
            .map_err(|e| format!("Request failed: {}", e))?;

        if !response.ok() {
            return Err(format!("HTTP {}: {}", response.status(), response.status_text()));
        }

        response
            .json()
            .await
            .map_err(|e| format!("JSON parse error: {}", e))
    }

    /// Login
    pub async fn login(username: &str, password: &str) -> Result<LoginResponse, String> {
        #[derive(Serialize)]
        struct LoginRequest<'a> {
            username: &'a str,
            password: &'a str,
        }

        let body = LoginRequest { username, password };
        Self::post("/auth/login", &body).await
    }

    /// Get cluster status
    pub async fn get_cluster_status() -> Result<ClusterStatus, String> {
        Self::get("/cluster/status").await
    }

    /// List VMs
    pub async fn list_vms() -> Result<Vec<VmInfo>, String> {
        Self::get("/vms").await
    }

    /// Get VM details
    pub async fn get_vm(id: &str) -> Result<VmInfo, String> {
        Self::get(&format!("/vms/{}", id)).await
    }

    /// Start VM
    pub async fn start_vm(id: &str) -> Result<(), String> {
        #[derive(Serialize)]
        struct Empty {}

        let _: Empty = Self::post(&format!("/vms/{}/start", id), &Empty {}).await?;
        Ok(())
    }

    /// Stop VM
    pub async fn stop_vm(id: &str) -> Result<(), String> {
        #[derive(Serialize)]
        struct Empty {}

        let _: Empty = Self::post(&format!("/vms/{}/stop", id), &Empty {}).await?;
        Ok(())
    }

    /// Get node stats
    pub async fn get_node_stats() -> Result<NodeStats, String> {
        Self::get("/monitoring/node").await
    }
}

// API response types

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub username: String,
    pub expires: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ClusterStatus {
    pub name: String,
    pub nodes: Vec<NodeInfo>,
    pub quorum: bool,
    pub online: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NodeInfo {
    pub id: String,
    pub name: String,
    pub online: bool,
    pub local: bool,
    pub architecture: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VmInfo {
    pub id: String,
    pub name: String,
    pub status: String,
    pub cpu_cores: u32,
    pub memory_mb: u64,
    pub node: Option<String>,
    pub architecture: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NodeStats {
    pub cpu_usage: f64,
    pub memory_total: u64,
    pub memory_used: u64,
    pub uptime: u64,
    pub load_average: (f64, f64, f64),
}
