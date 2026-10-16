//! API client for Horcrux backend

use gloo_net::http::Request;
use gloo_storage::{LocalStorage, Storage};
use serde::{Deserialize, Serialize};

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
            return Err(format!(
                "HTTP {}: {}",
                response.status(),
                response.status_text()
            ));
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
            return Err(format!(
                "HTTP {}: {}",
                response.status(),
                response.status_text()
            ));
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

    /// List cluster nodes
    pub async fn list_cluster_nodes() -> Result<Vec<NodeInfo>, String> {
        Self::get("/cluster/nodes").await
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
        #[derive(Serialize, Deserialize)]
        struct Empty {}

        let _: Empty = Self::post(&format!("/vms/{}/start", id), &Empty {}).await?;
        Ok(())
    }

    /// Stop VM
    pub async fn stop_vm(id: &str) -> Result<(), String> {
        #[derive(Serialize, Deserialize)]
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

/// `GET /api/cluster/status` (horcrux-api `cluster::ClusterStatusReport`).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ClusterStatus {
    /// Corosync cluster name; `None` when no Corosync cluster is set up.
    #[serde(default)]
    pub name: Option<String>,
    pub total_nodes: usize,
    pub online_nodes: usize,
    /// `None` when quorum can't be determined.
    #[serde(default)]
    pub has_quorum: Option<bool>,
    #[serde(default)]
    pub total_vms: usize,
}

impl ClusterStatus {
    pub fn display_name(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| "(no Corosync cluster)".to_string())
    }

    /// (text, css class) for the quorum field.
    pub fn quorum_display(&self) -> (&'static str, &'static str) {
        match self.has_quorum {
            Some(true) => ("Yes", "value success"),
            Some(false) => ("No", "value error"),
            None => ("Unknown", "value"),
        }
    }
}

/// One entry of `GET /api/cluster/nodes` (horcrux-api `cluster::node::Node`).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct NodeInfo {
    #[serde(default)]
    pub id: u32,
    pub name: String,
    pub ip: String,
    /// "online" | "offline" | "unknown"
    pub status: String,
    #[serde(default)]
    pub is_local: bool,
    pub architecture: String,
    #[serde(default)]
    pub cpu_cores: u32,
    /// Bytes.
    #[serde(default)]
    pub memory_total: u64,
    /// Live CPU %, `None` = unknown.
    #[serde(default)]
    pub cpu_usage: Option<f64>,
    /// Live RAM in use (bytes), `None` = unknown.
    #[serde(default)]
    pub memory_used: Option<u64>,
}

impl NodeInfo {
    pub fn is_online(&self) -> bool {
        self.status == "online"
    }

    /// Live usage text; unknown figures say so instead of showing 0.
    pub fn usage_label(&self) -> String {
        const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
        let cpu = self
            .cpu_usage
            .map(|c| format!("{:.1}% CPU", c))
            .unwrap_or_else(|| "CPU unknown".to_string());
        let mem = self
            .memory_used
            .map(|m| format!("{:.1} GB RAM used", m as f64 / GIB))
            .unwrap_or_else(|| "RAM unknown".to_string());
        format!("{}, {}", cpu, mem)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cluster_status_deserializes_from_real_api_shape() {
        let status: ClusterStatus = serde_json::from_str(
            r#"{"name":null,"total_nodes":2,"online_nodes":1,"has_quorum":null,
                "total_vms":3,"total_cpus":12,"total_memory":25769803776,
                "memory_used":null,"nodes_reporting_usage":0}"#,
        )
        .expect("must deserialize");
        assert_eq!(status.total_nodes, 2);
        assert_eq!(status.online_nodes, 1);
        assert_eq!(status.quorum_display().0, "Unknown");
        assert_eq!(status.display_name(), "(no Corosync cluster)");
    }

    #[test]
    fn node_info_deserializes_from_real_api_shape() {
        let nodes: Vec<NodeInfo> = serde_json::from_str(
            r#"[{"id":1,"name":"n1","ip":"10.0.0.1","status":"online","priority":100,
                 "is_local":true,"architecture":"x86_64","cpu_cores":8,
                 "memory_total":17179869184,"cpu_usage":12.5,"memory_used":4294967296,
                 "last_seen":1760000000},
                {"id":2,"name":"n2","ip":"10.0.0.2","status":"offline","priority":100,
                 "is_local":false,"architecture":"aarch64","cpu_cores":4,
                 "memory_total":4294967296,"cpu_usage":null,"memory_used":null,
                 "last_seen":null}]"#,
        )
        .expect("must deserialize");
        assert!(nodes[0].is_online());
        assert_eq!(nodes[0].usage_label(), "12.5% CPU, 4.0 GB RAM used");
        assert!(!nodes[1].is_online());
        assert_eq!(nodes[1].usage_label(), "CPU unknown, RAM unknown");
    }
}
