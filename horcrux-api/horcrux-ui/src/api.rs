//! API client for communicating with Horcrux backend

#![allow(dead_code)]

use horcrux_common::VmConfig;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

const API_BASE: &str = "http://localhost:8006/api";

/// Generic JSON fetch helper
pub async fn fetch_json<T: DeserializeOwned>(path: &str) -> Result<T, ApiError> {
    let url = if path.starts_with("http") {
        path.to_string()
    } else if path.starts_with("/api") {
        format!("http://localhost:8006{}", path)
    } else {
        format!("{}{}", API_BASE, path)
    };

    let response = reqwasm::http::Request::get(&url)
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        response.json().await.map_err(|e| ApiError { message: e.to_string() })
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

/// POST request helper
pub async fn post_json<T: DeserializeOwned, B: Serialize>(path: &str, body: &B) -> Result<T, ApiError> {
    let url = if path.starts_with("http") {
        path.to_string()
    } else if path.starts_with("/api") {
        format!("http://localhost:8006{}", path)
    } else {
        format!("{}{}", API_BASE, path)
    };

    let response = reqwasm::http::Request::post(&url)
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(body).unwrap())
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        response.json().await.map_err(|e| ApiError { message: e.to_string() })
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

/// PUT request helper
pub async fn put_json<T: DeserializeOwned, B: Serialize>(path: &str, body: &B) -> Result<T, ApiError> {
    let url = if path.starts_with("http") {
        path.to_string()
    } else if path.starts_with("/api") {
        format!("http://localhost:8006{}", path)
    } else {
        format!("{}{}", API_BASE, path)
    };

    let response = reqwasm::http::Request::put(&url)
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(body).unwrap())
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        response.json().await.map_err(|e| ApiError { message: e.to_string() })
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

/// DELETE request helper
pub async fn delete_json(path: &str) -> Result<(), ApiError> {
    let url = if path.starts_with("http") {
        path.to_string()
    } else if path.starts_with("/api") {
        format!("http://localhost:8006{}", path)
    } else {
        format!("{}{}", API_BASE, path)
    };

    let response = reqwasm::http::Request::delete(&url)
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        Ok(())
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

/// POST without body helper
pub async fn post_empty(path: &str) -> Result<(), ApiError> {
    let url = if path.starts_with("http") {
        path.to_string()
    } else if path.starts_with("/api") {
        format!("http://localhost:8006{}", path)
    } else {
        format!("{}{}", API_BASE, path)
    };

    let response = reqwasm::http::Request::post(&url)
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        Ok(())
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

/// Generic text fetch helper
pub async fn fetch_text(path: &str) -> Result<String, ApiError> {
    let url = if path.starts_with("http") {
        path.to_string()
    } else if path.starts_with("/api") {
        format!("http://localhost:8006{}", path)
    } else {
        format!("{}{}", API_BASE, path)
    };

    let response = reqwasm::http::Request::get(&url)
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        response.text().await.map_err(|e| ApiError { message: e.to_string() })
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiError {
    pub message: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ApiError {}

/// Get all VMs
pub async fn get_vms() -> Result<Vec<VmConfig>, ApiError> {
    let response = reqwasm::http::Request::get(&format!("{}/vms", API_BASE))
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        response.json().await.map_err(|e| ApiError { message: e.to_string() })
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

/// Create a new VM
pub async fn create_vm(config: VmConfig) -> Result<VmConfig, ApiError> {
    let response = reqwasm::http::Request::post(&format!("{}/vms", API_BASE))
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(&config).unwrap())
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        response.json().await.map_err(|e| ApiError { message: e.to_string() })
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

/// Start a VM
pub async fn start_vm(vm_id: &str) -> Result<(), ApiError> {
    let response = reqwasm::http::Request::post(&format!("{}/vms/{}/start", API_BASE, vm_id))
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        Ok(())
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

/// Stop a VM
pub async fn stop_vm(vm_id: &str) -> Result<(), ApiError> {
    let response = reqwasm::http::Request::post(&format!("{}/vms/{}/stop", API_BASE, vm_id))
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        Ok(())
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

/// Delete a VM
pub async fn delete_vm(vm_id: &str) -> Result<(), ApiError> {
    let response = reqwasm::http::Request::delete(&format!("{}/vms/{}", API_BASE, vm_id))
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        Ok(())
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeMetrics {
    pub hostname: String,
    pub timestamp: i64,
    pub cpu_usage: f64,
    pub memory_used: u64,
    pub memory_total: u64,
    pub uptime_seconds: u64,
}

/// Get node metrics
pub async fn get_node_metrics() -> Result<NodeMetrics, ApiError> {
    let response = reqwasm::http::Request::get(&format!("{}/monitoring/node", API_BASE))
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        response.json().await.map_err(|e| ApiError { message: e.to_string() })
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Alert {
    pub id: String,
    pub rule_name: String,
    pub severity: String,
    pub status: String,
    pub message: String,
    pub target: String,
    pub fired_at: i64,
}

/// Get active alerts
pub async fn get_active_alerts() -> Result<Vec<ActiveAlert>, ApiError> {
    fetch_json("/alerts/active").await
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClusterNode {
    pub name: String,
    pub ip: String,
    #[serde(default)]
    pub address: String,
    pub status: String,
    pub architecture: String,
    pub cpu_cores: u32,
    pub memory_total: u64,
    #[serde(default)]
    pub memory_used: u64,
    #[serde(default)]
    pub cpu_usage: f64,
    #[serde(default)]
    pub memory_usage: f64,
    #[serde(default)]
    pub disk_usage: f64,
    #[serde(default)]
    pub vm_count: u32,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub last_seen: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub resources: ClusterNodeResources,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ClusterNodeResources {
    #[serde(default)]
    pub cpu_count: u32,
    #[serde(default)]
    pub cpu_cores: u32,
    #[serde(default)]
    pub cpu_allocated: u32,
    #[serde(default)]
    pub memory_total: u64,
    #[serde(default)]
    pub memory_mb: u64,
    #[serde(default)]
    pub memory_allocated: u64,
    #[serde(default)]
    pub storage_total: u64,
    #[serde(default)]
    pub disk_gb: u64,
    #[serde(default)]
    pub storage_used: u64,
}

/// Get cluster nodes
pub async fn get_cluster_nodes() -> Result<Vec<ClusterNode>, ApiError> {
    let response = reqwasm::http::Request::get(&format!("{}/cluster/nodes", API_BASE))
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        response.json().await.map_err(|e| ApiError { message: e.to_string() })
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

// Container Management APIs

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Container {
    pub id: String,
    #[serde(default)]
    pub vmid: String,
    pub name: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub node: String,
    pub runtime: String,
    pub image: String,
    pub status: String,
    #[serde(default)]
    pub cpus: u32,
    #[serde(default)]
    pub maxmem: u64,
    pub created_at: Option<String>,
}

/// Get all containers
pub async fn get_containers() -> Result<Vec<Container>, ApiError> {
    let response = reqwasm::http::Request::get(&format!("{}/containers", API_BASE))
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        response.json().await.map_err(|e| ApiError { message: e.to_string() })
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

/// Start a container
pub async fn start_container(container_id: &str) -> Result<(), ApiError> {
    let response = reqwasm::http::Request::post(&format!("{}/containers/{}/start", API_BASE, container_id))
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        Ok(())
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

/// Stop a container
pub async fn stop_container(container_id: &str) -> Result<(), ApiError> {
    let response = reqwasm::http::Request::post(&format!("{}/containers/{}/stop", API_BASE, container_id))
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        Ok(())
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

/// Delete a container
pub async fn delete_container(container_id: &str) -> Result<(), ApiError> {
    let response = reqwasm::http::Request::delete(&format!("{}/containers/{}", API_BASE, container_id))
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        Ok(())
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

// Snapshot Management APIs

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub vm_id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: String,
    pub size_bytes: u64,
    pub include_memory: bool,
}

/// Get snapshots for a VM
pub async fn get_vm_snapshots(vm_id: &str) -> Result<Vec<VmSnapshot>, ApiError> {
    let response = reqwasm::http::Request::get(&format!("{}/vms/{}/snapshots", API_BASE, vm_id))
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        response.json().await.map_err(|e| ApiError { message: e.to_string() })
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

/// Create a snapshot
pub async fn create_snapshot(vm_id: &str, name: &str, description: Option<String>, include_memory: bool) -> Result<Snapshot, ApiError> {
    #[derive(Serialize)]
    struct CreateRequest {
        name: String,
        description: Option<String>,
        include_memory: bool,
    }

    let request = CreateRequest {
        name: name.to_string(),
        description,
        include_memory,
    };

    let response = reqwasm::http::Request::post(&format!("{}/vms/{}/snapshots", API_BASE, vm_id))
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(&request).unwrap())
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        response.json().await.map_err(|e| ApiError { message: e.to_string() })
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

/// Restore a snapshot
pub async fn restore_snapshot(vm_id: &str, snapshot_id: &str) -> Result<(), ApiError> {
    let response = reqwasm::http::Request::post(&format!("{}/vms/{}/snapshots/{}/restore", API_BASE, vm_id, snapshot_id))
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        Ok(())
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

/// Delete a snapshot
pub async fn delete_snapshot(vm_id: &str, snapshot_id: &str) -> Result<(), ApiError> {
    let response = reqwasm::http::Request::delete(&format!("{}/vms/{}/snapshots/{}", API_BASE, vm_id, snapshot_id))
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        Ok(())
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

// Clone Management APIs

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CloneJob {
    pub job_id: String,
    pub source_vm_id: String,
    pub target_vm_name: String,
    pub clone_type: String,
    pub status: String,
    pub progress: f64,
    pub created_at: String,
}

/// Get all clone jobs
pub async fn get_clone_jobs() -> Result<Vec<CloneJob>, ApiError> {
    let response = reqwasm::http::Request::get(&format!("{}/vms/clone/jobs", API_BASE))
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        response.json().await.map_err(|e| ApiError { message: e.to_string() })
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

// Replication Management APIs

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReplicationJob {
    pub id: String,
    pub vm_id: String,
    pub source_node: String,
    pub target_node: String,
    pub schedule: String,
    pub enabled: bool,
    pub last_sync: Option<String>,
    pub status: String,
}

/// Get all replication jobs
pub async fn get_replication_jobs() -> Result<Vec<ReplicationJob>, ApiError> {
    let response = reqwasm::http::Request::get(&format!("{}/replication/jobs", API_BASE))
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        response.json().await.map_err(|e| ApiError { message: e.to_string() })
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

/// Execute a replication job
pub async fn execute_replication(job_id: &str) -> Result<(), ApiError> {
    let response = reqwasm::http::Request::post(&format!("{}/replication/jobs/{}/execute", API_BASE, job_id))
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        Ok(())
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

/// Delete a replication job
pub async fn delete_replication(job_id: &str) -> Result<(), ApiError> {
    let response = reqwasm::http::Request::delete(&format!("{}/replication/jobs/{}", API_BASE, job_id))
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        Ok(())
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

// =============================================================================
// Console APIs
// =============================================================================

/// VM representation for console page
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Vm {
    pub id: String,
    pub name: String,
    pub status: String,
    pub cpus: u32,
    pub memory: u64,
}

/// Extended VM information for backup and template operations
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VmInfo {
    pub id: String,
    #[serde(default)]
    pub vmid: String,
    pub name: String,
    pub state: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub node: String,
    #[serde(default)]
    pub cpus: u32,
    #[serde(default)]
    pub maxcpu: u32,
    #[serde(default)]
    pub memory: u64,
    #[serde(default)]
    pub maxmem: u64,
    #[serde(default)]
    pub disk_size: u64,
}

/// Virtual machine representation for HA operations
pub type VirtualMachine = VmInfo;

/// Get virtual machines
pub async fn get_virtual_machines() -> Result<Vec<VirtualMachine>, ApiError> {
    fetch_json("/vms").await
}

// =============================================================================
// Metrics Types
// =============================================================================

/// Query for metrics
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricQuery {
    pub metric: String,
    #[serde(default)]
    pub labels: std::collections::HashMap<String, String>,
    pub start_time: Option<chrono::DateTime<chrono::Utc>>,
    pub end_time: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default = "default_step")]
    pub step: u64,
}

fn default_step() -> u64 { 60 }

/// A single data point in a metric result
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricDataPoint {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub value: f64,
    #[serde(default)]
    pub labels: std::collections::HashMap<String, String>,
}

/// Result from a metrics query
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricResult {
    pub metric: String,
    #[serde(default)]
    pub labels: std::collections::HashMap<String, String>,
    pub values: Vec<MetricDataPoint>,
}

/// Component for building PromQL queries
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum QueryComponent {
    Metric {
        name: String,
        labels: std::collections::HashMap<String, String>,
    },
    Function {
        name: String,
        args: Vec<String>,
        modifiers: Vec<String>,
    },
    Operator {
        operator: String,
    },
}

/// PromQL function definition
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PromQLFunction {
    pub name: String,
    pub description: String,
    pub syntax: String,
    pub category: String,
    pub example: String,
}

/// Query validation result
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueryValidationResult {
    pub valid: bool,
    #[serde(default)]
    pub message: String,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    #[serde(default)]
    pub suggestions: Vec<String>,
}

/// Get a single VM
pub async fn get_vm(vm_id: &str) -> Result<Vm, ApiError> {
    let response = reqwasm::http::Request::get(&format!("{}/vms/{}", API_BASE, vm_id))
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        response.json().await.map_err(|e| ApiError { message: e.to_string() })
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

/// Get console URL for a VM
pub async fn get_console_url(vm_id: &str, console_type: &str) -> Result<String, ApiError> {
    let response = reqwasm::http::Request::get(&format!(
        "{}/vms/{}/console?type={}",
        API_BASE, vm_id, console_type
    ))
    .send()
    .await
    .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        #[derive(Deserialize)]
        struct ConsoleResponse {
            url: String,
        }
        let resp: ConsoleResponse = response.json().await
            .map_err(|e| ApiError { message: e.to_string() })?;
        Ok(resp.url)
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

/// Send keys to console (e.g., Ctrl+Alt+Del)
pub async fn send_console_keys(vm_id: &str, keys: &[&str]) -> Result<(), ApiError> {
    #[derive(Serialize)]
    struct KeysRequest<'a> {
        keys: &'a [&'a str],
    }

    let response = reqwasm::http::Request::post(&format!("{}/vms/{}/console/keys", API_BASE, vm_id))
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(&KeysRequest { keys }).unwrap())
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        Ok(())
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

/// Send input to serial console
pub async fn send_serial_input(vm_id: &str, input: &str) -> Result<String, ApiError> {
    #[derive(Serialize)]
    struct SerialInput<'a> {
        input: &'a str,
    }

    let response = reqwasm::http::Request::post(&format!("{}/vms/{}/console/serial", API_BASE, vm_id))
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(&SerialInput { input }).unwrap())
        .send()
        .await
        .map_err(|e| ApiError { message: e.to_string() })?;

    if response.ok() {
        #[derive(Deserialize)]
        struct SerialOutput {
            output: String,
        }
        let resp: SerialOutput = response.json().await
            .map_err(|e| ApiError { message: e.to_string() })?;
        Ok(resp.output)
    } else {
        Err(ApiError { message: format!("HTTP {}", response.status()) })
    }
}

// =============================================================================
// Authentication & User Management APIs
// =============================================================================

/// User information from API
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub email: String,
    pub role: String,
    pub realm: String,
    pub enabled: bool,
    pub roles: Vec<String>,
    pub comment: Option<String>,
    pub last_login: Option<String>,
}

/// Role definition
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Role {
    pub name: String,
    pub description: String,
    pub permissions: Vec<Permission>,
}

/// Permission for a resource path
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Permission {
    pub path: String,
    pub privileges: Vec<String>,
}

/// API Token information
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiToken {
    pub id: String,
    pub user: String,
    pub enabled: bool,
    pub expire: Option<i64>,
    pub comment: Option<String>,
    pub created_at: Option<String>,
    pub last_used: Option<String>,
}

/// User session information
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserSession {
    pub session_id: String,
    pub username: String,
    pub realm: String,
    pub created: i64,
    pub expires: i64,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
}

/// Login request
#[derive(Clone, Debug, Serialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    pub realm: Option<String>,
}

/// Login response
#[derive(Clone, Debug, Deserialize)]
pub struct LoginResponse {
    pub ticket: String,
    pub csrf_token: String,
    pub username: String,
    pub roles: Vec<String>,
}

/// Create user request
#[derive(Clone, Debug, Serialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
    pub email: String,
    pub role: String,
    pub realm: String,
    pub enabled: bool,
    pub comment: Option<String>,
}

/// Update user request
#[derive(Clone, Debug, Serialize)]
pub struct UpdateUserRequest {
    pub email: Option<String>,
    pub role: Option<String>,
    pub enabled: Option<bool>,
    pub comment: Option<String>,
}

/// Change password request
#[derive(Clone, Debug, Serialize)]
pub struct ChangePasswordRequest {
    pub old_password: String,
    pub new_password: String,
}

/// Create API token request
#[derive(Clone, Debug, Serialize)]
pub struct CreateApiTokenRequest {
    pub comment: Option<String>,
    pub expire: Option<i64>,
    pub permissions: Option<Vec<String>>,
}

// ============================================================================
// Kubernetes Workload Data Structures
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KubernetesPod {
    pub name: String,
    pub namespace: String,
    pub status: PodStatus,
    pub ready: String,
    pub restarts: u32,
    pub age: String,
    pub node: Option<String>,
    pub containers: Vec<Container>,
    pub labels: std::collections::HashMap<String, String>,
    pub annotations: std::collections::HashMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PodStatus {
    pub phase: String,
    pub reason: Option<String>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContainerStatus {
    pub name: String,
    pub ready: bool,
    pub restart_count: u32,
    pub state: ContainerState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContainerState {
    pub running: Option<ContainerStateRunning>,
    pub waiting: Option<ContainerStateWaiting>,
    pub terminated: Option<ContainerStateTerminated>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContainerStateRunning {
    pub started_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContainerStateWaiting {
    pub reason: Option<String>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContainerStateTerminated {
    pub exit_code: i32,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KubernetesDeployment {
    pub name: String,
    pub namespace: String,
    pub replicas: DeploymentReplicas,
    pub strategy: String,
    pub age: String,
    pub labels: std::collections::HashMap<String, String>,
    pub annotations: std::collections::HashMap<String, String>,
    pub conditions: Vec<DeploymentCondition>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeploymentReplicas {
    pub desired: u32,
    pub current: u32,
    pub ready: u32,
    pub available: u32,
    pub unavailable: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeploymentCondition {
    pub condition_type: String,
    pub status: String,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub last_update_time: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KubernetesService {
    pub name: String,
    pub namespace: String,
    pub service_type: String,
    pub cluster_ip: String,
    pub external_ips: Vec<String>,
    pub ports: Vec<ServicePort>,
    pub age: String,
    pub selector: std::collections::HashMap<String, String>,
    pub labels: std::collections::HashMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServicePort {
    pub name: Option<String>,
    pub protocol: String,
    pub port: u16,
    pub target_port: Option<String>,
    pub node_port: Option<u16>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateServiceRequest {
    pub name: String,
    pub service_type: String,
    pub ports: Vec<ServicePort>,
    pub selector: std::collections::HashMap<String, String>,
    pub labels: Option<std::collections::HashMap<String, String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KubernetesIngress {
    pub name: String,
    pub namespace: String,
    pub class: Option<String>,
    pub hosts: Vec<String>,
    pub age: String,
    pub rules: Vec<IngressRule>,
    pub tls: Vec<IngressTLS>,
    pub labels: std::collections::HashMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IngressRule {
    pub host: Option<String>,
    pub paths: Vec<IngressPath>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IngressPath {
    pub path: String,
    pub path_type: String,
    pub service_name: String,
    pub service_port: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IngressTLS {
    pub hosts: Vec<String>,
    pub secret_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateIngressRequest {
    pub name: String,
    pub ingress_class: Option<String>,
    pub rules: Vec<IngressRule>,
    pub tls: Option<Vec<IngressTLS>>,
    pub labels: Option<std::collections::HashMap<String, String>>,
    pub annotations: Option<std::collections::HashMap<String, String>>,
}

/// Get all users
pub async fn get_users() -> Result<Vec<User>, ApiError> {
    fetch_json("/users").await
}

/// Get a specific user by ID
pub async fn get_user(user_id: &str) -> Result<User, ApiError> {
    fetch_json(&format!("/users/{}", user_id)).await
}

/// Create a new user
pub async fn create_user(request: CreateUserRequest) -> Result<User, ApiError> {
    post_json("/users", &request).await
}

/// Update a user
pub async fn update_user(user_id: &str, request: UpdateUserRequest) -> Result<User, ApiError> {
    post_json(&format!("/users/{}", user_id), &request).await
}

/// Delete a user
pub async fn delete_user(user_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/users/{}", user_id)).await
}

/// Enable/disable a user
pub async fn toggle_user(user_id: &str, enabled: bool) -> Result<(), ApiError> {
    post_json(&format!("/users/{}/toggle", user_id), &serde_json::json!({"enabled": enabled})).await
}

/// Get all roles
pub async fn get_roles() -> Result<Vec<Role>, ApiError> {
    fetch_json("/roles").await
}

/// Get a specific role
pub async fn get_role(role_name: &str) -> Result<Role, ApiError> {
    fetch_json(&format!("/roles/{}", role_name)).await
}

/// Create a new role
pub async fn create_role(role: Role) -> Result<Role, ApiError> {
    post_json("/roles", &role).await
}

/// Update a role
pub async fn update_role(role_name: &str, role: Role) -> Result<Role, ApiError> {
    post_json(&format!("/roles/{}", role_name), &role).await
}

/// Delete a role
pub async fn delete_role(role_name: &str) -> Result<(), ApiError> {
    delete_json(&format!("/roles/{}", role_name)).await
}

/// Get permissions for a user
pub async fn get_user_permissions(user_id: &str) -> Result<Vec<Permission>, ApiError> {
    fetch_json(&format!("/users/{}/permissions", user_id)).await
}

/// Add permission to a user
pub async fn add_user_permission(user_id: &str, permission: Permission) -> Result<(), ApiError> {
    post_json(&format!("/users/{}/permissions", user_id), &permission).await
}

/// Remove permission from a user
pub async fn remove_user_permission(user_id: &str, path: &str) -> Result<(), ApiError> {
    delete_json(&format!("/users/{}/permissions?path={}", user_id, path)).await
}

/// Get all active sessions
pub async fn get_active_sessions() -> Result<Vec<UserSession>, ApiError> {
    fetch_json("/auth/sessions").await
}

/// Terminate a user session
pub async fn terminate_session(session_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/auth/sessions/{}", session_id)).await
}

/// Terminate all sessions for a user
pub async fn terminate_user_sessions(user_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/users/{}/sessions", user_id)).await
}

/// Get API tokens for a user
pub async fn get_user_api_tokens(user_id: &str) -> Result<Vec<ApiToken>, ApiError> {
    fetch_json(&format!("/users/{}/api-keys", user_id)).await
}

/// Create a new API token for a user
pub async fn create_api_token(user_id: &str, request: CreateApiTokenRequest) -> Result<ApiToken, ApiError> {
    post_json(&format!("/users/{}/api-keys", user_id), &request).await
}

/// Update an API token
pub async fn update_api_token(user_id: &str, token_id: &str, request: CreateApiTokenRequest) -> Result<ApiToken, ApiError> {
    post_json(&format!("/users/{}/api-keys/{}", user_id, token_id), &request).await
}

/// Delete an API token
pub async fn delete_api_token(user_id: &str, token_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/users/{}/api-keys/{}", user_id, token_id)).await
}

/// Toggle an API token (enable/disable)
pub async fn toggle_api_token(user_id: &str, token_id: &str, enabled: bool) -> Result<(), ApiError> {
    post_json(&format!("/users/{}/api-keys/{}/toggle", user_id, token_id), &serde_json::json!({"enabled": enabled})).await
}

/// Change user password
pub async fn change_password(user_id: &str, request: ChangePasswordRequest) -> Result<(), ApiError> {
    post_json(&format!("/users/{}/password", user_id), &request).await
}

/// Login user
pub async fn login(request: LoginRequest) -> Result<LoginResponse, ApiError> {
    post_json("/auth/login", &request).await
}

/// Logout current user
pub async fn logout() -> Result<(), ApiError> {
    post_empty("/auth/logout").await
}

/// Verify current session
pub async fn verify_session() -> Result<bool, ApiError> {
    match fetch_json::<serde_json::Value>("/auth/verify").await {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

// ============================================================================
// Kubernetes Workload Management APIs
// ============================================================================

/// Get pods in a namespace
pub async fn get_pods(cluster_id: &str, namespace: &str) -> Result<Vec<KubernetesPod>, ApiError> {
    fetch_json(&format!("/k8s/clusters/{}/namespaces/{}/pods", cluster_id, namespace)).await
}

/// Get pod details
pub async fn get_pod(cluster_id: &str, namespace: &str, pod_name: &str) -> Result<KubernetesPod, ApiError> {
    fetch_json(&format!("/k8s/clusters/{}/namespaces/{}/pods/{}", cluster_id, namespace, pod_name)).await
}

/// Delete a pod
pub async fn delete_pod(cluster_id: &str, namespace: &str, pod_name: &str) -> Result<(), ApiError> {
    delete_json(&format!("/k8s/clusters/{}/namespaces/{}/pods/{}", cluster_id, namespace, pod_name)).await
}

/// Get pod logs
pub async fn get_pod_logs(cluster_id: &str, namespace: &str, pod_name: &str, container: Option<&str>) -> Result<String, ApiError> {
    let mut url = format!("/k8s/clusters/{}/namespaces/{}/pods/{}/logs", cluster_id, namespace, pod_name);
    if let Some(container) = container {
        url = format!("{}?container={}", url, container);
    }
    fetch_text(&url).await
}

/// Get deployments in a namespace
pub async fn get_deployments(cluster_id: &str, namespace: &str) -> Result<Vec<KubernetesDeployment>, ApiError> {
    fetch_json(&format!("/k8s/clusters/{}/namespaces/{}/deployments", cluster_id, namespace)).await
}

/// Get deployment details
pub async fn get_deployment(cluster_id: &str, namespace: &str, deployment_name: &str) -> Result<KubernetesDeployment, ApiError> {
    fetch_json(&format!("/k8s/clusters/{}/namespaces/{}/deployments/{}", cluster_id, namespace, deployment_name)).await
}

/// Scale deployment
pub async fn scale_deployment(cluster_id: &str, namespace: &str, deployment_name: &str, replicas: u32) -> Result<(), ApiError> {
    post_json(&format!("/k8s/clusters/{}/namespaces/{}/deployments/{}/scale", cluster_id, namespace, deployment_name),
              &serde_json::json!({"replicas": replicas})).await
}

/// Restart deployment
pub async fn restart_deployment(cluster_id: &str, namespace: &str, deployment_name: &str) -> Result<(), ApiError> {
    post_empty(&format!("/k8s/clusters/{}/namespaces/{}/deployments/{}/restart", cluster_id, namespace, deployment_name)).await
}

/// Delete deployment
pub async fn delete_deployment(cluster_id: &str, namespace: &str, deployment_name: &str) -> Result<(), ApiError> {
    delete_json(&format!("/k8s/clusters/{}/namespaces/{}/deployments/{}", cluster_id, namespace, deployment_name)).await
}

/// Get services in a namespace
pub async fn get_services(cluster_id: &str, namespace: &str) -> Result<Vec<KubernetesService>, ApiError> {
    fetch_json(&format!("/k8s/clusters/{}/namespaces/{}/services", cluster_id, namespace)).await
}

/// Get service details
pub async fn get_service(cluster_id: &str, namespace: &str, service_name: &str) -> Result<KubernetesService, ApiError> {
    fetch_json(&format!("/k8s/clusters/{}/namespaces/{}/services/{}", cluster_id, namespace, service_name)).await
}

/// Create service
pub async fn create_service(cluster_id: &str, namespace: &str, service: CreateServiceRequest) -> Result<KubernetesService, ApiError> {
    post_json(&format!("/k8s/clusters/{}/namespaces/{}/services", cluster_id, namespace), &service).await
}

/// Delete service
pub async fn delete_service(cluster_id: &str, namespace: &str, service_name: &str) -> Result<(), ApiError> {
    delete_json(&format!("/k8s/clusters/{}/namespaces/{}/services/{}", cluster_id, namespace, service_name)).await
}

/// Get ingresses in a namespace
pub async fn get_ingresses(cluster_id: &str, namespace: &str) -> Result<Vec<KubernetesIngress>, ApiError> {
    fetch_json(&format!("/k8s/clusters/{}/namespaces/{}/ingresses", cluster_id, namespace)).await
}

/// Get ingress details
pub async fn get_ingress(cluster_id: &str, namespace: &str, ingress_name: &str) -> Result<KubernetesIngress, ApiError> {
    fetch_json(&format!("/k8s/clusters/{}/namespaces/{}/ingresses/{}", cluster_id, namespace, ingress_name)).await
}

/// Create ingress
pub async fn create_ingress(cluster_id: &str, namespace: &str, ingress: CreateIngressRequest) -> Result<KubernetesIngress, ApiError> {
    post_json(&format!("/k8s/clusters/{}/namespaces/{}/ingresses", cluster_id, namespace), &ingress).await
}

/// Delete ingress
pub async fn delete_ingress(cluster_id: &str, namespace: &str, ingress_name: &str) -> Result<(), ApiError> {
    delete_json(&format!("/k8s/clusters/{}/namespaces/{}/ingresses/{}", cluster_id, namespace, ingress_name)).await
}

// ============================================================================
// HA Management Data Structures
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HaResource {
    pub resource_id: String,
    pub resource_type: String, // vm, container
    pub priority: u32,
    pub state: String, // started, stopped, ignored
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HaGroup {
    pub id: String,
    pub name: String,
    pub priority: u32,
    pub max_restart: u32,
    pub max_relocate: u32,
    pub enabled: bool,
    pub comment: Option<String>,
    pub resources: Vec<HaResource>,
    #[serde(default)]
    pub vm_ids: Vec<String>,
    pub nodes: Vec<String>,
    pub state: String, // active, inactive, maintenance
    pub restricted: bool,
    pub nofailback: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HaResourceAssignment {
    pub group_id: String,
    pub resource_id: String,
    pub resource_type: String,
    pub priority: u32,
    pub state: String,
}

// ============================================================================
// Migration Management Data Structures
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MigrationJob {
    pub job_id: String,
    pub resource_type: String, // vm, container
    pub resource_id: String,
    pub source_node: String,
    pub target_node: String,
    pub migration_type: String, // online, offline
    pub status: String, // pending, running, completed, failed, cancelled
    pub progress: u32, // 0-100
    pub start_time: chrono::DateTime<chrono::Utc>,
    pub end_time: Option<chrono::DateTime<chrono::Utc>>,
    pub error: Option<String>,
    pub bandwidth_limit: Option<u64>,
    pub timeout: u32,
    pub force: bool,
    pub with_local_disks: bool,
    pub estimated_duration: Option<u32>, // minutes
    pub transferred_bytes: u64,
    pub remaining_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BulkMigrationJob {
    pub job_id: String,
    pub target_node: String,
    pub resources: Vec<(String, String)>, // (type, id) pairs
    pub max_workers: u32,
    pub migration_type: String,
    pub status: String,
    pub start_time: chrono::DateTime<chrono::Utc>,
    pub end_time: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_count: usize,
    pub failed_count: usize,
    pub total_count: usize,
}

// ============================================================================
// Helm Management Data Structures
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HelmRepository {
    pub name: String,
    pub url: String,
    pub description: Option<String>,
    pub added_at: String,
    pub status: String,
    pub last_update: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AddHelmRepoRequest {
    pub name: String,
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub force_update: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HelmChart {
    pub name: String,
    pub version: String,
    pub app_version: Option<String>,
    pub description: String,
    pub repository: String,
    pub icon: Option<String>,
    pub keywords: Vec<String>,
    pub maintainers: Vec<HelmMaintainer>,
    pub created: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HelmMaintainer {
    pub name: String,
    pub email: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HelmRelease {
    pub name: String,
    pub namespace: String,
    pub revision: u32,
    pub status: String,
    pub chart: String,
    pub app_version: Option<String>,
    pub updated: String,
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HelmInstallRequest {
    pub name: String,
    pub chart: String,
    pub version: Option<String>,
    pub values: Option<serde_json::Value>,
    pub create_namespace: Option<bool>,
    pub wait: Option<bool>,
    pub timeout: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HelmValues {
    pub values: serde_json::Value,
    pub computed_values: serde_json::Value,
    pub user_supplied_values: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KubernetesConfigMap {
    pub name: String,
    pub namespace: String,
    pub data: std::collections::HashMap<String, String>,
    pub binary_data: Option<std::collections::HashMap<String, String>>,
    pub labels: std::collections::HashMap<String, String>,
    pub annotations: std::collections::HashMap<String, String>,
    pub age: String,
    #[serde(default)]
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateConfigMapRequest {
    pub name: String,
    pub data: std::collections::HashMap<String, String>,
    pub binary_data: Option<std::collections::HashMap<String, String>>,
    pub labels: Option<std::collections::HashMap<String, String>>,
    pub annotations: Option<std::collections::HashMap<String, String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KubernetesSecret {
    pub name: String,
    pub namespace: String,
    #[serde(default)]
    pub secret_type: Option<String>,
    #[serde(default)]
    pub data: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub labels: Option<std::collections::HashMap<String, String>>,
    #[serde(default)]
    pub annotations: Option<std::collections::HashMap<String, String>>,
    #[serde(default)]
    pub age: String,
    #[serde(default)]
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateSecretRequest {
    pub name: String,
    pub secret_type: String,
    pub data: std::collections::HashMap<String, String>,
    pub string_data: Option<std::collections::HashMap<String, String>>,
    pub labels: Option<std::collections::HashMap<String, String>>,
    pub annotations: Option<std::collections::HashMap<String, String>>,
}

// ============================================================================
// Helm Management APIs
// ============================================================================

/// Get Helm repositories
pub async fn get_helm_repositories() -> Result<Vec<HelmRepository>, ApiError> {
    fetch_json("/k8s/helm/repos").await
}

/// Add Helm repository
pub async fn add_helm_repository(request: AddHelmRepoRequest) -> Result<HelmRepository, ApiError> {
    post_json("/k8s/helm/repos", &request).await
}

/// Update Helm repository
pub async fn update_helm_repository(repo_name: &str) -> Result<(), ApiError> {
    post_empty(&format!("/k8s/helm/repos/{}/update", repo_name)).await
}

/// Remove Helm repository
pub async fn remove_helm_repository(repo_name: &str) -> Result<(), ApiError> {
    delete_json(&format!("/k8s/helm/repos/{}", repo_name)).await
}

/// Search Helm charts
pub async fn search_helm_charts(query: &str, repo: Option<&str>) -> Result<Vec<HelmChart>, ApiError> {
    let mut url = format!("/k8s/helm/charts?query={}", query);
    if let Some(repo) = repo {
        url = format!("{}&repo={}", url, repo);
    }
    fetch_json(&url).await
}

/// Get chart details
pub async fn get_helm_chart(repo: &str, chart: &str) -> Result<HelmChart, ApiError> {
    fetch_json(&format!("/k8s/helm/charts/{}/{}", repo, chart)).await
}

/// Get chart versions
pub async fn get_helm_chart_versions(repo: &str, chart: &str) -> Result<Vec<String>, ApiError> {
    fetch_json(&format!("/k8s/helm/charts/{}/{}/versions", repo, chart)).await
}

/// Get chart values
pub async fn get_helm_chart_values(repo: &str, chart: &str, version: Option<&str>) -> Result<serde_json::Value, ApiError> {
    let mut url = format!("/k8s/helm/charts/{}/{}/values", repo, chart);
    if let Some(version) = version {
        url = format!("{}?version={}", url, version);
    }
    fetch_json(&url).await
}

/// Get Helm releases
pub async fn get_helm_releases(cluster_id: &str, namespace: Option<&str>) -> Result<Vec<HelmRelease>, ApiError> {
    let mut url = format!("/k8s/clusters/{}/helm/releases", cluster_id);
    if let Some(namespace) = namespace {
        url = format!("{}?namespace={}", url, namespace);
    }
    fetch_json(&url).await
}

/// Get Helm release details
pub async fn get_helm_release(cluster_id: &str, namespace: &str, release_name: &str) -> Result<HelmRelease, ApiError> {
    fetch_json(&format!("/k8s/clusters/{}/helm/releases/{}/{}", cluster_id, namespace, release_name)).await
}

/// Install Helm chart
pub async fn install_helm_chart(cluster_id: &str, namespace: &str, request: HelmInstallRequest) -> Result<HelmRelease, ApiError> {
    post_json(&format!("/k8s/clusters/{}/namespaces/{}/helm/install", cluster_id, namespace), &request).await
}

/// Upgrade Helm release
pub async fn upgrade_helm_release(cluster_id: &str, namespace: &str, release_name: &str, request: HelmInstallRequest) -> Result<HelmRelease, ApiError> {
    post_json(&format!("/k8s/clusters/{}/namespaces/{}/helm/releases/{}/upgrade", cluster_id, namespace, release_name), &request).await
}

/// Uninstall Helm release
pub async fn uninstall_helm_release(cluster_id: &str, namespace: &str, release_name: &str) -> Result<(), ApiError> {
    delete_json(&format!("/k8s/clusters/{}/namespaces/{}/helm/releases/{}", cluster_id, namespace, release_name)).await
}

/// Get Helm release values
pub async fn get_helm_release_values(cluster_id: &str, namespace: &str, release_name: &str) -> Result<HelmValues, ApiError> {
    fetch_json(&format!("/k8s/clusters/{}/namespaces/{}/helm/releases/{}/values", cluster_id, namespace, release_name)).await
}

/// Get Helm release history
pub async fn get_helm_release_history(cluster_id: &str, namespace: &str, release_name: &str) -> Result<Vec<HelmRelease>, ApiError> {
    fetch_json(&format!("/k8s/clusters/{}/namespaces/{}/helm/releases/{}/history", cluster_id, namespace, release_name)).await
}

/// Rollback Helm release
pub async fn rollback_helm_release(cluster_id: &str, namespace: &str, release_name: &str, revision: u32) -> Result<HelmRelease, ApiError> {
    post_json(&format!("/k8s/clusters/{}/namespaces/{}/helm/releases/{}/rollback", cluster_id, namespace, release_name),
              &serde_json::json!({"revision": revision})).await
}

// ============================================================================
// Configuration Management APIs
// ============================================================================

/// Get ConfigMaps in a namespace
pub async fn get_configmaps(cluster_id: &str, namespace: &str) -> Result<Vec<KubernetesConfigMap>, ApiError> {
    fetch_json(&format!("/k8s/clusters/{}/namespaces/{}/configmaps", cluster_id, namespace)).await
}

/// Get ConfigMap details
pub async fn get_configmap(cluster_id: &str, namespace: &str, configmap_name: &str) -> Result<KubernetesConfigMap, ApiError> {
    fetch_json(&format!("/k8s/clusters/{}/namespaces/{}/configmaps/{}", cluster_id, namespace, configmap_name)).await
}

/// Create ConfigMap
pub async fn create_configmap(cluster_id: &str, namespace: &str, request: CreateConfigMapRequest) -> Result<KubernetesConfigMap, ApiError> {
    post_json(&format!("/k8s/clusters/{}/namespaces/{}/configmaps", cluster_id, namespace), &request).await
}

/// Update ConfigMap
pub async fn update_configmap(cluster_id: &str, namespace: &str, configmap_name: &str, request: CreateConfigMapRequest) -> Result<KubernetesConfigMap, ApiError> {
    post_json(&format!("/k8s/clusters/{}/namespaces/{}/configmaps/{}", cluster_id, namespace, configmap_name), &request).await
}

/// Delete ConfigMap
pub async fn delete_configmap(cluster_id: &str, namespace: &str, configmap_name: &str) -> Result<(), ApiError> {
    delete_json(&format!("/k8s/clusters/{}/namespaces/{}/configmaps/{}", cluster_id, namespace, configmap_name)).await
}

/// Get Secrets in a namespace
pub async fn get_secrets(cluster_id: &str, namespace: &str) -> Result<Vec<KubernetesSecret>, ApiError> {
    fetch_json(&format!("/k8s/clusters/{}/namespaces/{}/secrets", cluster_id, namespace)).await
}

/// Get Secret details
pub async fn get_secret(cluster_id: &str, namespace: &str, secret_name: &str) -> Result<KubernetesSecret, ApiError> {
    fetch_json(&format!("/k8s/clusters/{}/namespaces/{}/secrets/{}", cluster_id, namespace, secret_name)).await
}

/// Create Secret
pub async fn create_secret(cluster_id: &str, namespace: &str, request: CreateSecretRequest) -> Result<KubernetesSecret, ApiError> {
    post_json(&format!("/k8s/clusters/{}/namespaces/{}/secrets", cluster_id, namespace), &request).await
}

/// Update Secret
pub async fn update_secret(cluster_id: &str, namespace: &str, secret_name: &str, request: CreateSecretRequest) -> Result<KubernetesSecret, ApiError> {
    post_json(&format!("/k8s/clusters/{}/namespaces/{}/secrets/{}", cluster_id, namespace, secret_name), &request).await
}

/// Delete Secret
pub async fn delete_secret(cluster_id: &str, namespace: &str, secret_name: &str) -> Result<(), ApiError> {
    delete_json(&format!("/k8s/clusters/{}/namespaces/{}/secrets/{}", cluster_id, namespace, secret_name)).await
}

// ============================================================================
// Kubernetes API Aliases (for consistency with page imports)
// ============================================================================

/// Get ConfigMaps in a namespace (alias)
pub async fn get_kubernetes_configmaps(cluster_id: &str, namespace: Option<&str>) -> Result<Vec<KubernetesConfigMap>, ApiError> {
    get_configmaps(cluster_id, namespace.unwrap_or("default")).await
}

/// Create ConfigMap (alias)
pub async fn create_kubernetes_configmap(cluster_id: &str, namespace: &str, request: CreateConfigMapRequest) -> Result<KubernetesConfigMap, ApiError> {
    create_configmap(cluster_id, namespace, request).await
}

/// Update ConfigMap (alias)
pub async fn update_kubernetes_configmap(cluster_id: &str, namespace: &str, configmap_name: &str, request: CreateConfigMapRequest) -> Result<KubernetesConfigMap, ApiError> {
    update_configmap(cluster_id, namespace, configmap_name, request).await
}

/// Delete ConfigMap (alias)
pub async fn delete_kubernetes_configmap(cluster_id: &str, namespace: &str, configmap_name: &str) -> Result<(), ApiError> {
    delete_configmap(cluster_id, namespace, configmap_name).await
}

/// Get Secrets in a namespace (alias)
pub async fn get_kubernetes_secrets(cluster_id: &str, namespace: Option<&str>) -> Result<Vec<KubernetesSecret>, ApiError> {
    get_secrets(cluster_id, namespace.unwrap_or("default")).await
}

/// Create Secret (alias)
pub async fn create_kubernetes_secret(cluster_id: &str, namespace: &str, request: CreateSecretRequest) -> Result<KubernetesSecret, ApiError> {
    create_secret(cluster_id, namespace, request).await
}

/// Update Secret (alias)
pub async fn update_kubernetes_secret(cluster_id: &str, namespace: &str, secret_name: &str, request: CreateSecretRequest) -> Result<KubernetesSecret, ApiError> {
    update_secret(cluster_id, namespace, secret_name, request).await
}

/// Delete Secret (alias)
pub async fn delete_kubernetes_secret(cluster_id: &str, namespace: &str, secret_name: &str) -> Result<(), ApiError> {
    delete_secret(cluster_id, namespace, secret_name).await
}

// =============================================================================
// Backup & Data Protection API
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmBackup {
    pub id: String,
    pub vm_id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub target: BackupTarget,
    pub status: BackupStatus,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub size: Option<u64>,
    pub compressed_size: Option<u64>,
    pub error: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupTarget {
    pub storage_id: String,
    pub path: String,
    pub encryption: Option<BackupEncryption>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupEncryption {
    pub method: String,
    pub key_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BackupStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateBackupRequest {
    pub vm_id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub target: BackupTarget,
    pub include_memory: bool,
    pub compress: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreBackupRequest {
    pub vm_id: String,
    pub restore_memory: Option<bool>,
    pub overwrite_existing: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupJob {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub vm_ids: Vec<String>,
    pub schedule: String, // Cron expression
    pub retention: RetentionPolicy,
    pub target: BackupTarget,
    pub enabled: bool,
    pub created_at: String,
    pub last_run: Option<String>,
    pub next_run: Option<String>,
    pub last_status: Option<BackupStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateBackupJobRequest {
    pub name: String,
    pub description: Option<String>,
    pub vm_ids: Vec<String>,
    pub schedule: String,
    pub retention: RetentionPolicy,
    pub target: BackupTarget,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionPolicy {
    pub keep_hourly: Option<u32>,
    pub keep_daily: Option<u32>,
    pub keep_weekly: Option<u32>,
    pub keep_monthly: Option<u32>,
    pub keep_yearly: Option<u32>,
    pub max_age_days: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmSnapshot {
    pub id: String,
    pub vm_id: String,
    pub name: String,
    pub description: Option<String>,
    pub parent_id: Option<String>,
    pub created_at: String,
    pub memory_included: bool,
    pub size_mb: u64,
    pub children: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSnapshotRequest {
    pub name: String,
    pub description: Option<String>,
    pub include_memory: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreSnapshotRequest {
    pub restore_memory: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotTreeNode {
    pub snapshot: VmSnapshot,
    pub children: Vec<SnapshotTreeNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotSchedule {
    pub id: String,
    pub vm_id: String,
    pub name: String,
    pub description: Option<String>,
    pub schedule: String, // Cron expression
    pub retention_policy: RetentionPolicy,
    pub include_memory: bool,
    pub enabled: bool,
    pub created_at: String,
    pub last_run: Option<String>,
    pub next_run: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSnapshotScheduleRequest {
    pub vm_id: String,
    pub name: String,
    pub description: Option<String>,
    pub schedule: String,
    pub retention_policy: RetentionPolicy,
    pub include_memory: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotQuota {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub quota_type: QuotaType,
    pub limit_value: u64,
    pub storage_path: String,
    pub cleanup_policy: CleanupPolicy,
    pub enabled: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QuotaType {
    MaxCount,
    MaxSize,
    MaxAge,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CleanupPolicy {
    OldestFirst,
    LargestFirst,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSnapshotQuotaRequest {
    pub name: String,
    pub description: Option<String>,
    pub quota_type: QuotaType,
    pub limit_value: u64,
    pub storage_path: String,
    pub cleanup_policy: CleanupPolicy,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaUsage {
    pub quota_id: String,
    pub current_count: u64,
    pub current_size: u64,
    pub oldest_snapshot: Option<String>,
    pub newest_snapshot: Option<String>,
    pub exceeds_quota: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaSummary {
    pub total_quotas: u64,
    pub active_quotas: u64,
    pub total_snapshots: u64,
    pub total_size: u64,
    pub quotas_exceeded: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnforceQuotaRequest {
    pub snapshot_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmTemplate {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub source_vm_id: String,
    pub created_at: String,
    pub config: serde_json::Value,
    pub size_mb: u64,
    pub storage_location: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTemplateRequest {
    pub name: String,
    pub description: Option<String>,
    pub source_vm_id: String,
}

// Backup API Functions
/// List all backups
pub async fn get_backups() -> Result<Vec<VmBackup>, ApiError> {
    fetch_json("/backups").await
}

/// Get backup details
pub async fn get_backup(id: &str) -> Result<VmBackup, ApiError> {
    fetch_json(&format!("/backups/{}", id)).await
}

/// Create a new backup
pub async fn create_backup(request: CreateBackupRequest) -> Result<VmBackup, ApiError> {
    post_json("/backups", &request).await
}

/// Delete a backup
pub async fn delete_backup(id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/backups/{}", id)).await
}

/// Restore a backup
pub async fn restore_backup(id: &str, request: RestoreBackupRequest) -> Result<(), ApiError> {
    post_json(&format!("/backups/{}/restore", id), &request).await
}

// Backup Jobs API Functions
/// List all backup jobs
pub async fn get_backup_jobs() -> Result<Vec<BackupJob>, ApiError> {
    fetch_json("/backup-jobs").await
}

/// Create a new backup job
pub async fn create_backup_job(request: CreateBackupJobRequest) -> Result<BackupJob, ApiError> {
    post_json("/backup-jobs", &request).await
}

/// Run backup job immediately
pub async fn run_backup_job_now(id: &str) -> Result<(), ApiError> {
    post_json(&format!("/backup-jobs/{}/run", id), &()).await
}

/// Apply retention policy
pub async fn apply_retention_policy(target_id: &str) -> Result<(), ApiError> {
    post_json(&format!("/backups/retention/{}", target_id), &()).await
}

// Duplicate VM Snapshot function removed - using first definition

/// Get snapshot details
pub async fn get_vm_snapshot(vm_id: &str, snapshot_id: &str) -> Result<VmSnapshot, ApiError> {
    fetch_json(&format!("/vms/{}/snapshots/{}", vm_id, snapshot_id)).await
}

/// Create a new VM snapshot
pub async fn create_vm_snapshot(vm_id: &str, request: CreateSnapshotRequest) -> Result<VmSnapshot, ApiError> {
    post_json(&format!("/vms/{}/snapshots", vm_id), &request).await
}

/// Delete a VM snapshot
pub async fn delete_vm_snapshot(vm_id: &str, snapshot_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/vms/{}/snapshots/{}", vm_id, snapshot_id)).await
}

/// Restore a VM snapshot
pub async fn restore_vm_snapshot(vm_id: &str, snapshot_id: &str, request: RestoreSnapshotRequest) -> Result<(), ApiError> {
    post_json(&format!("/vms/{}/snapshots/{}/restore", vm_id, snapshot_id), &request).await
}

/// Get snapshot tree for a VM
pub async fn get_vm_snapshot_tree(vm_id: &str) -> Result<Vec<SnapshotTreeNode>, ApiError> {
    fetch_json(&format!("/vms/{}/snapshots/tree", vm_id)).await
}

// Snapshot Schedule API Functions
/// List all snapshot schedules
pub async fn get_snapshot_schedules() -> Result<Vec<SnapshotSchedule>, ApiError> {
    fetch_json("/snapshot-schedules").await
}

/// Get snapshot schedule details
pub async fn get_snapshot_schedule(id: &str) -> Result<SnapshotSchedule, ApiError> {
    fetch_json(&format!("/snapshot-schedules/{}", id)).await
}

/// Create a new snapshot schedule
pub async fn create_snapshot_schedule(request: CreateSnapshotScheduleRequest) -> Result<SnapshotSchedule, ApiError> {
    post_json("/snapshot-schedules", &request).await
}

/// Update a snapshot schedule
pub async fn update_snapshot_schedule(id: &str, request: CreateSnapshotScheduleRequest) -> Result<SnapshotSchedule, ApiError> {
    put_json(&format!("/snapshot-schedules/{}", id), &request).await
}

/// Delete a snapshot schedule
pub async fn delete_snapshot_schedule(id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/snapshot-schedules/{}", id)).await
}

// Snapshot Quota API Functions
/// List all snapshot quotas
pub async fn get_snapshot_quotas() -> Result<Vec<SnapshotQuota>, ApiError> {
    fetch_json("/snapshot-quotas").await
}

/// Get snapshot quota details
pub async fn get_snapshot_quota(id: &str) -> Result<SnapshotQuota, ApiError> {
    fetch_json(&format!("/snapshot-quotas/{}", id)).await
}

/// Create a new snapshot quota
pub async fn create_snapshot_quota(request: CreateSnapshotQuotaRequest) -> Result<SnapshotQuota, ApiError> {
    post_json("/snapshot-quotas", &request).await
}

/// Update a snapshot quota
pub async fn update_snapshot_quota(id: &str, request: CreateSnapshotQuotaRequest) -> Result<SnapshotQuota, ApiError> {
    put_json(&format!("/snapshot-quotas/{}", id), &request).await
}

/// Delete a snapshot quota
pub async fn delete_snapshot_quota(id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/snapshot-quotas/{}", id)).await
}

/// Get snapshot quota usage
pub async fn get_snapshot_quota_usage(id: &str) -> Result<QuotaUsage, ApiError> {
    fetch_json(&format!("/snapshot-quotas/{}/usage", id)).await
}

/// Get snapshot quota summary
pub async fn get_snapshot_quota_summary() -> Result<QuotaSummary, ApiError> {
    fetch_json("/snapshot-quotas/summary").await
}

/// Enforce snapshot quota
pub async fn enforce_snapshot_quota(id: &str, request: EnforceQuotaRequest) -> Result<(), ApiError> {
    post_json(&format!("/snapshot-quotas/{}/enforce", id), &request).await
}

// Template API Functions
/// List all templates
pub async fn get_templates() -> Result<Vec<VmTemplate>, ApiError> {
    fetch_json("/templates").await
}

/// Get template details
pub async fn get_template(id: &str) -> Result<VmTemplate, ApiError> {
    fetch_json(&format!("/templates/{}", id)).await
}

/// Create a new template
pub async fn create_template(request: CreateTemplateRequest) -> Result<VmTemplate, ApiError> {
    post_json("/templates", &request).await
}

/// Delete a template
pub async fn delete_template(id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/templates/{}", id)).await
}

/// Clone a template
pub async fn clone_template(id: &str, name: String) -> Result<(), ApiError> {
    post_json(&format!("/templates/{}/clone", id), &serde_json::json!({"name": name})).await
}

// =============================================================================
// High Availability & Clustering API
// =============================================================================

// Duplicate removed - using the first ClusterNode definition

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NodeStatus {
    Online,
    Offline,
    Maintenance,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeResources {
    pub cpu_cores: u32,
    pub memory_mb: u64,
    pub disk_gb: u64,
    pub network_interfaces: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddNodeRequest {
    pub name: String,
    pub address: String,
    pub ssh_key: Option<String>,
    pub architecture: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterArchitecture {
    pub total_nodes: u32,
    pub online_nodes: u32,
    pub total_vms: u32,
    pub architectures: Vec<ArchitectureInfo>,
    pub load_balance_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchitectureInfo {
    pub name: String,
    pub node_count: u32,
    pub vm_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindNodeRequest {
    pub vm_config: serde_json::Value,
    pub preferred_architecture: Option<String>,
    pub exclude_nodes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRecommendation {
    pub node_name: String,
    pub score: f64,
    pub reason: String,
    pub resources_available: NodeResources,
}

// Removed duplicate HA/Migration structs - using first definitions

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateHaResourceRequest {
    pub vm_id: String,
    pub priority: u32,
    pub group_id: Option<String>,
    pub failover_domain: Vec<String>,
    pub auto_failover: bool,
    pub max_restart_attempts: u32,
    pub restart_policy_delay: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HaStatus {
    pub cluster_status: HaClusterStatus,
    pub resources: Vec<HaResourceStatus>,
    pub last_failover: Option<String>,
    pub total_failovers: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HaClusterStatus {
    Active,
    Degraded,
    Failed,
    Maintenance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HaResourceStatus {
    pub vm_id: String,
    pub current_node: String,
    pub desired_node: Option<String>,
    pub status: HaResourceState,
    pub health_check: HealthCheckStatus,
    pub last_migration: Option<String>,
    pub restart_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HaResourceState {
    Running,
    Stopped,
    Migrating,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HealthCheckStatus {
    Healthy,
    Warning,
    Critical,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PlacementPolicy {
    Balanced,
    Consolidated,
    AntiAffinity,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateHaGroupRequest {
    pub name: String,
    pub description: Option<String>,
    pub priority: u32,
    pub failover_domain: Vec<String>,
    pub placement_policy: PlacementPolicy,
    pub vm_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MigrationType {
    Live,
    Offline,
    Online,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MigrationStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationRequest {
    pub target_node: String,
    pub migration_type: Option<String>,
    pub bandwidth_limit: Option<u64>,
    pub force: Option<bool>,
}

// Cluster Management API Functions (duplicate get_cluster_nodes removed)

/// Add a new node to the cluster
pub async fn add_cluster_node(name: String, request: AddNodeRequest) -> Result<ClusterNode, ApiError> {
    post_json(&format!("/cluster/nodes/{}", name), &request).await
}

/// Get cluster architecture information
pub async fn get_cluster_architecture() -> Result<ClusterArchitecture, ApiError> {
    fetch_json("/cluster/architecture").await
}

/// Find the best node for a VM
pub async fn find_best_node_for_vm(request: FindNodeRequest) -> Result<NodeRecommendation, ApiError> {
    post_json("/cluster/find-node", &request).await
}

// HA Management API Functions
/// List all HA resources
pub async fn get_ha_resources() -> Result<Vec<HaResource>, ApiError> {
    fetch_json("/ha/resources").await
}

/// Add a VM to HA management
pub async fn add_ha_resource(request: CreateHaResourceRequest) -> Result<HaResource, ApiError> {
    post_json("/ha/resources", &request).await
}

/// Remove a VM from HA management
pub async fn remove_ha_resource(vm_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/ha/resources/{}", vm_id)).await
}

/// Get HA cluster status
pub async fn get_ha_status() -> Result<HaStatus, ApiError> {
    fetch_json("/ha/status").await
}

/// List all HA groups
pub async fn get_ha_groups() -> Result<Vec<HaGroup>, ApiError> {
    fetch_json("/ha/groups").await
}

/// Create a new HA group
pub async fn create_ha_group(group: HaGroup) -> Result<HaGroup, ApiError> {
    post_json("/ha/groups", &group).await
}

// Migration API Functions
/// Start VM migration
pub async fn migrate_vm(vm_id: &str, request: MigrationRequest) -> Result<MigrationJob, ApiError> {
    post_json(&format!("/migrate/{}", vm_id), &request).await
}

/// Get migration status
pub async fn get_migration_status(vm_id: &str) -> Result<MigrationJob, ApiError> {
    fetch_json(&format!("/migrate/{}/status", vm_id)).await
}

// Additional HA Management API Functions for the UI components
/// Update an existing HA group
pub async fn update_ha_group(group: HaGroup) -> Result<HaGroup, ApiError> {
    put_json(&format!("/ha/groups/{}", group.id), &group).await
}

/// Delete an HA group
pub async fn delete_ha_group(group_id: String) -> Result<(), ApiError> {
    delete_json(&format!("/ha/groups/{}", group_id)).await
}

/// Assign resource to HA group
pub async fn assign_resource_to_ha_group(assignment: HaResourceAssignment) -> Result<(), ApiError> {
    post_json(&format!("/ha/groups/{}/resources", assignment.group_id), &assignment).await
}

// Migration Management API Functions for the UI components
/// Get all migration jobs
pub async fn get_migration_jobs() -> Result<Vec<MigrationJob>, ApiError> {
    fetch_json("/migration/jobs").await
}

/// Create migration job
pub async fn create_migration_job(job: MigrationJob) -> Result<MigrationJob, ApiError> {
    post_json("/migration/jobs", &job).await
}

/// Cancel migration job
pub async fn cancel_migration_job(job_id: String) -> Result<(), ApiError> {
    delete_json(&format!("/migration/jobs/{}", job_id)).await
}

/// Create bulk migration job
pub async fn create_bulk_migration_job(job: BulkMigrationJob) -> Result<BulkMigrationJob, ApiError> {
    post_json("/migration/bulk", &job).await
}

// ============================================================================
// Monitoring & Alerting Data Structures
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AlertRule {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub metric: String,
    pub condition: String, // greater_than, less_than, equal_to, not_equal_to
    pub threshold: f64,
    pub duration_seconds: u32,
    pub severity: String, // info, warning, critical
    pub enabled: bool,
    pub labels: std::collections::HashMap<String, String>,
    pub annotations: std::collections::HashMap<String, String>,
    pub notification_channels: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_triggered: Option<chrono::DateTime<chrono::Utc>>,
    pub trigger_count: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActiveAlert {
    pub id: String,
    pub rule_id: String,
    pub rule_name: String,
    pub metric: String,
    pub condition: String,
    pub threshold: f64,
    pub current_value: f64,
    pub severity: String,
    pub status: String, // firing, pending, resolved, silenced
    pub message: String,
    pub labels: std::collections::HashMap<String, String>,
    pub annotations: std::collections::HashMap<String, String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    pub acknowledged_at: Option<chrono::DateTime<chrono::Utc>>,
    pub silenced_until: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NotificationChannel {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub channel_type: String, // email, slack, teams, discord, pagerduty, webhook
    pub enabled: bool,
    pub config: NotificationChannelConfig,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_used: Option<chrono::DateTime<chrono::Utc>>,
    pub success_count: u32,
    pub failure_count: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum NotificationChannelConfig {
    Email {
        address: String,
        smtp_server: String,
        smtp_port: u16,
        username: String,
        password: String,
        use_tls: bool,
    },
    Slack {
        webhook_url: String,
        channel: Option<String>,
        username: Option<String>,
        icon_emoji: Option<String>,
    },
    Teams {
        webhook_url: String,
    },
    Discord {
        webhook_url: String,
        username: Option<String>,
        avatar_url: Option<String>,
    },
    PagerDuty {
        integration_key: String,
        #[serde(default)]
        routing_key: String,
        severity: String,
    },
    Webhook {
        url: String,
        method: String,
        headers: std::collections::HashMap<String, String>,
        auth: Option<WebhookAuth>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WebhookAuth {
    BasicAuth { username: String, password: String },
    BearerToken { token: String },
    ApiKey { key: String, header: String },
}

// ============================================================================
// Metrics & Query Management Data Structures
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MetricDefinition {
    pub name: String,
    pub description: String,
    pub metric_type: String, // counter, gauge, histogram, summary
    pub labels: Vec<String>,
    pub unit: Option<String>,
    pub help: String,
    pub scrape_interval: String,
    pub retention: String,
    pub cardinality: u64,
    pub last_scraped: String,
    pub source: String, // prometheus, node_exporter, custom, etc.
    pub category: String, // system, application, network, storage
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricSample {
    pub timestamp: String,
    pub value: f64,
    pub labels: std::collections::HashMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricSeries {
    pub metric: String,
    pub samples: Vec<MetricSample>,
    pub min_value: f64,
    pub max_value: f64,
    pub avg_value: f64,
    pub sample_count: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueryHistoryEntry {
    pub id: String,
    pub query: String,
    #[serde(default)]
    pub time_range: String,
    pub query_type: String, // promql, logql, sql
    pub timestamp: String,
    #[serde(default)]
    pub executed_at: chrono::DateTime<chrono::Utc>,
    pub execution_time_ms: f64,
    pub result_count: u64,
    pub status: String, // success, error, timeout
    pub error_message: Option<String>,
    pub user: String,
    pub source: String, // metrics_explorer, dashboard, alert_rule, api
    pub tags: Vec<String>,
    pub favorite: bool,
    pub shared: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueryTemplate {
    pub id: String,
    pub name: String,
    pub description: String,
    pub query: String,
    pub category: String,
    pub variables: Vec<QueryVariable>,
    pub created_by: String,
    pub created_at: String,
    pub usage_count: u64,
    pub public: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueryVariable {
    pub name: String,
    pub description: String,
    pub var_type: String, // string, number, metric, label_value
    pub default_value: Option<String>,
    pub options: Vec<String>,
    pub required: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateQueryTemplateRequest {
    pub name: String,
    pub description: String,
    pub query: String,
    pub category: String,
    pub public: bool,
    pub variables: Vec<QueryVariable>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AlertPreview {
    pub query_valid: bool,
    pub current_value: Option<f64>,
    pub would_trigger: bool,
    pub sample_data: Vec<MetricSample>,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateAlertRuleRequest {
    pub name: String,
    pub description: String,
    pub query: String,
    pub condition: String,
    pub threshold: f64,
    pub duration: String,
    pub severity: String,
    pub labels: std::collections::HashMap<String, String>,
    pub annotations: std::collections::HashMap<String, String>,
    pub notification_channels: Vec<String>,
}

// Metrics API Functions
pub async fn get_metrics_catalog() -> Result<Vec<MetricDefinition>, ApiError> {
    fetch_json("/metrics/catalog").await
}

pub async fn get_metric_samples(metric_name: String, time_range: String) -> Result<MetricSeries, ApiError> {
    fetch_json(&format!("/metrics/{}/samples?range={}", metric_name, time_range)).await
}

pub async fn get_query_history(time_range: String) -> Result<Vec<QueryHistoryEntry>, ApiError> {
    fetch_json(&format!("/metrics/query-history?range={}", time_range)).await
}

pub async fn toggle_query_favorite(entry_id: String) -> Result<(), ApiError> {
    post_json(&format!("/metrics/query-history/{}/favorite", entry_id), &()).await
}

pub async fn get_query_templates() -> Result<Vec<QueryTemplate>, ApiError> {
    fetch_json("/metrics/query-templates").await
}

pub async fn create_query_template(request: CreateQueryTemplateRequest) -> Result<QueryTemplate, ApiError> {
    post_json("/metrics/query-templates", &request).await
}

pub async fn delete_query_history_entry(entry_id: String) -> Result<(), ApiError> {
    delete_json(&format!("/metrics/query-history/{}", entry_id)).await
}

pub async fn get_alert_rules() -> Result<Vec<AlertRule>, ApiError> {
    fetch_json("/alerts/rules").await
}

pub async fn create_alert_rule(rule: AlertRule) -> Result<AlertRule, ApiError> {
    post_json("/alerts/rules", &rule).await
}

pub async fn update_alert_rule(rule: AlertRule) -> Result<AlertRule, ApiError> {
    put_json(&format!("/alerts/rules/{}", rule.id), &rule).await
}

pub async fn delete_alert_rule(rule_id: String) -> Result<(), ApiError> {
    delete_json(&format!("/alerts/rules/{}", rule_id)).await
}

pub async fn toggle_alert_rule(rule_id: String, enabled: bool) -> Result<(), ApiError> {
    put_json(&format!("/alerts/rules/{}/toggle", rule_id), &serde_json::json!({"enabled": enabled})).await
}

pub async fn preview_alert_rule(query: String, condition: String, threshold: f64) -> Result<AlertPreview, ApiError> {
    let request = serde_json::json!({
        "query": query,
        "condition": condition,
        "threshold": threshold
    });
    post_json("/alerts/preview", &request).await
}

pub async fn get_notification_channels() -> Result<Vec<NotificationChannel>, ApiError> {
    fetch_json("/notifications/channels").await
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WebhookConfig {
    pub id: String,
    pub name: String,
    pub url: String,
    pub method: String,
    pub headers: std::collections::HashMap<String, String>,
    pub events: Vec<String>,
    pub enabled: bool,
    pub secret: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_triggered: Option<chrono::DateTime<chrono::Utc>>,
    pub trigger_count: u32,
    pub success_count: u32,
    pub failure_count: u32,
}

// (Duplicate alert/notification functions removed - already defined above)
pub async fn delete_notification_channel(channel_id: String) -> Result<(), ApiError> {
    delete_json(&format!("/notifications/channels/{}", channel_id)).await
}

/// Test notification channel
pub async fn test_notification_channel(channel_id: String) -> Result<(), ApiError> {
    post_json(&format!("/notifications/channels/{}/test", channel_id), &()).await
}

/// Test alert rule
pub async fn test_alert_rule(rule_id: String) -> Result<(), ApiError> {
    post_json(&format!("/alerts/rules/{}/test", rule_id), &()).await
}

/// Acknowledge alert
pub async fn acknowledge_alert(alert_id: String) -> Result<(), ApiError> {
    post_json(&format!("/alerts/{}/acknowledge", alert_id), &()).await
}

/// Silence alert for a duration
pub async fn silence_alert(alert_id: String, duration_seconds: u32) -> Result<(), ApiError> {
    let body = serde_json::json!({ "duration_seconds": duration_seconds });
    post_json(&format!("/alerts/{}/silence", alert_id), &body).await
}

/// Create notification channel
pub async fn create_notification_channel(channel: NotificationChannel) -> Result<NotificationChannel, ApiError> {
    post_json("/notifications/channels", &channel).await
}

/// Update notification channel
pub async fn update_notification_channel(channel_id: &str, channel: NotificationChannel) -> Result<NotificationChannel, ApiError> {
    put_json(&format!("/notifications/channels/{}", channel_id), &channel).await
}

/// Get webhook configurations
pub async fn get_webhook_configs() -> Result<Vec<WebhookConfig>, ApiError> {
    fetch_json("/webhooks").await
}

/// Query metrics
pub async fn query_metrics(query: MetricQuery) -> Result<MetricResult, ApiError> {
    post_json("/metrics/query", &query).await
}

/// Get available metrics
pub async fn get_available_metrics() -> Result<Vec<MetricDefinition>, ApiError> {
    fetch_json("/metrics/definitions").await
}

// ============================================================================
// System Configuration Data Structures
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SystemConfiguration {
    pub hostname: String,
    pub domain: Option<String>,
    pub timezone: String,
    pub locale: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetworkInterface {
    pub name: String,
    pub method: String, // dhcp, static, manual
    pub address: Option<String>,
    pub netmask: Option<String>,
    pub gateway: Option<String>,
    pub mtu: Option<u16>,
    pub auto: bool,
    pub bridge: Option<String>,
    pub bridge_ports: Option<Vec<String>>,
    pub vlan_id: Option<u16>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DnsConfiguration {
    pub servers: Vec<String>,
    pub search_domains: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NtpConfiguration {
    pub servers: Vec<String>,
    pub timezone: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SystemHealth {
    pub hostname: String,
    pub uptime: u64,
    pub load_average: [f64; 3],
    pub memory_usage: MemoryUsage,
    pub disk_usage: Vec<DiskUsage>,
    pub network_stats: Vec<NetworkStats>,
    pub system_info: SystemInfo,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryUsage {
    pub total: u64,
    pub used: u64,
    pub free: u64,
    pub available: u64,
    pub buffers: u64,
    pub cached: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiskUsage {
    pub device: String,
    pub mount_point: String,
    pub total: u64,
    pub used: u64,
    pub available: u64,
    pub filesystem: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetworkStats {
    pub interface: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub rx_errors: u64,
    pub tx_errors: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SystemInfo {
    pub kernel_version: String,
    pub arch: String,
    pub cpu_model: String,
    pub cpu_cores: u32,
    pub total_memory: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub name: String,
    pub status: String, // active, inactive, failed, unknown
    pub enabled: bool,
    pub description: String,
    pub pid: Option<u32>,
    pub memory_usage: Option<u64>,
    pub cpu_usage: Option<f64>,
}

// Removing duplicates - using extended versions below

// System Configuration API Functions
/// Get system configuration
pub async fn get_system_configuration() -> Result<SystemConfiguration, ApiError> {
    fetch_json("/system/config").await
}

/// Update system configuration
pub async fn update_system_configuration(config: SystemConfiguration) -> Result<(), ApiError> {
    post_json("/system/config", &config).await
}

/// Get network interfaces
pub async fn get_network_interfaces() -> Result<Vec<NetworkInterface>, ApiError> {
    fetch_json("/system/network/interfaces").await
}

/// Update network interface
pub async fn update_network_interface(interface: NetworkInterface) -> Result<(), ApiError> {
    post_json(&format!("/system/network/interfaces/{}", interface.name), &interface).await
}

/// Get DNS configuration
pub async fn get_dns_configuration() -> Result<DnsConfiguration, ApiError> {
    fetch_json("/system/network/dns").await
}

/// Update DNS configuration
pub async fn update_dns_configuration(config: DnsConfiguration) -> Result<(), ApiError> {
    post_json("/system/network/dns", &config).await
}

/// Get NTP configuration
pub async fn get_ntp_configuration() -> Result<NtpConfiguration, ApiError> {
    fetch_json("/system/time/ntp").await
}

/// Update NTP configuration
pub async fn update_ntp_configuration(config: NtpConfiguration) -> Result<(), ApiError> {
    post_json("/system/time/ntp", &config).await
}

/// Get system health
pub async fn get_system_health() -> Result<SystemHealth, ApiError> {
    fetch_json("/system/health").await
}

/// Get services status
pub async fn get_services_status() -> Result<Vec<ServiceStatus>, ApiError> {
    fetch_json("/system/services").await
}

/// Control service (start, stop, restart, enable, disable)
pub async fn control_service(service_name: &str, action: &str) -> Result<(), ApiError> {
    post_json(&format!("/system/services/{}/{}", service_name, action), &()).await
}

// Removing old get_system_logs - using the extended version with filters below

/// Search packages
pub async fn search_packages(query: &str) -> Result<Vec<PackageInfo>, ApiError> {
    fetch_json(&format!("/system/packages/search?q={}", query)).await
}

/// Install package
pub async fn install_package(package_name: &str) -> Result<(), ApiError> {
    post_json(&format!("/system/packages/{}/install", package_name), &()).await
}

/// Remove package
pub async fn remove_package(package_name: &str) -> Result<(), ApiError> {
    post_json(&format!("/system/packages/{}/remove", package_name), &()).await
}

/// Update package database
pub async fn update_package_database() -> Result<(), ApiError> {
    post_json("/system/packages/update", &()).await
}

// ============================================================================
// System Management Extensions for Logs, Package Manager, and Diagnostics
// ============================================================================

// Extended LogEntry structure to support the logs page
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogFilters {
    pub level: Option<String>,
    pub service: Option<String>,
    pub search: Option<String>,
    pub limit: Option<u32>,
}

/// Get system logs with extended filtering
pub async fn get_system_logs(filters: LogFilters) -> Result<Vec<LogEntry>, ApiError> {
    let mut url = "/system/logs".to_string();
    let mut params = Vec::new();

    if let Some(level) = filters.level {
        params.push(format!("level={}", level));
    }
    if let Some(service) = filters.service {
        params.push(format!("service={}", service));
    }
    if let Some(search) = filters.search {
        params.push(format!("search={}", search));
    }
    if let Some(limit) = filters.limit {
        params.push(format!("limit={}", limit));
    }

    if !params.is_empty() {
        url.push('?');
        url.push_str(&params.join("&"));
    }

    fetch_json(&url).await
}

/// Clear system logs
pub async fn clear_system_logs() -> Result<(), ApiError> {
    delete_json("/system/logs").await
}

/// Export system logs
pub async fn export_system_logs(filters: LogFilters, format: String) -> Result<String, ApiError> {
    let mut url = format!("/system/logs/export?format={}", format);
    let mut params = Vec::new();

    if let Some(level) = filters.level {
        params.push(format!("level={}", level));
    }
    if let Some(service) = filters.service {
        params.push(format!("service={}", service));
    }
    if let Some(search) = filters.search {
        params.push(format!("search={}", search));
    }

    if !params.is_empty() {
        url.push('&');
        url.push_str(&params.join("&"));
    }

    fetch_text(&url).await
}

// Package Manager Extensions
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PackageInfo {
    pub name: String,
    pub description: String,
    pub status: String, // installed, upgradeable, not-installed
    pub installed_version: Option<String>,
    pub available_version: Option<String>,
    pub size: Option<u64>,
}

/// Get installed packages
pub async fn get_installed_packages() -> Result<Vec<PackageInfo>, ApiError> {
    fetch_json("/system/packages").await
}

/// Upgrade package
pub async fn upgrade_package(package_name: &str) -> Result<(), ApiError> {
    post_json(&format!("/system/packages/{}/upgrade", package_name), &()).await
}

/// Upgrade all packages
pub async fn upgrade_all_packages() -> Result<u32, ApiError> {
    post_json("/system/packages/upgrade-all", &()).await
}

/// Bulk install packages
pub async fn bulk_install_packages(package_names: Vec<String>) -> Result<(), ApiError> {
    post_json("/system/packages/bulk-install", &package_names).await
}

// Diagnostics System
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SystemDiagnostics {
    pub overall_health_score: f64,
    pub critical_issues_count: u32,
    pub warning_count: u32,
    pub last_check_time: String,
    pub performance_metrics: PerformanceMetrics,
    pub security_status: SecurityStatus,
    pub available_tests: Vec<DiagnosticTest>,
    pub system_issues: Vec<SystemIssue>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PerformanceMetrics {
    pub cpu_score: f64,
    pub memory_score: f64,
    pub disk_score: f64,
    pub network_score: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SecurityStatus {
    pub score: f64,
    pub vulnerability_count: u32,
    pub compliance_level: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagnosticTest {
    pub name: String,
    pub category: String,
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagnosticTestResult {
    pub passed: bool,
    pub message: String,
    pub duration_ms: u64,
    pub error_details: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SystemIssue {
    pub severity: String,
    pub category: String,
    pub description: String,
    pub recommendation: Option<String>,
}

/// Get system diagnostics
pub async fn get_system_diagnostics() -> Result<SystemDiagnostics, ApiError> {
    fetch_json("/system/diagnostics").await
}

/// Run diagnostic test
pub async fn run_diagnostic_test(test_name: &str) -> Result<DiagnosticTestResult, ApiError> {
    post_json(&format!("/system/diagnostics/tests/{}", test_name), &()).await
}

/// Run all diagnostic tests
pub async fn run_all_diagnostic_tests() -> Result<std::collections::HashMap<String, DiagnosticTestResult>, ApiError> {
    post_json("/system/diagnostics/tests/run-all", &()).await
}

/// Generate diagnostic report
pub async fn generate_diagnostic_report(format: &str) -> Result<String, ApiError> {
    fetch_text(&format!("/system/diagnostics/report?format={}", format)).await
}

// Extended LogEntry to support service field and details
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: String,
    pub service: Option<String>,
    pub message: String,
    pub details: Option<String>,
}

// ============================================================================
// Custom Dashboard System Data Structures
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CustomDashboard {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub category: String,
    pub layout: String,
    pub refresh_interval: u32,
    pub public: bool,
    pub tags: Vec<String>,
    pub widget_count: u32,
    pub usage_count: u32,
    pub rating: Option<f64>,
    pub created_by: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateDashboardRequest {
    pub name: String,
    pub description: Option<String>,
    pub category: String,
    pub layout: String,
    pub refresh_interval: u32,
    pub public: bool,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateDashboardRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub layout: Option<String>,
    pub refresh_interval: Option<u32>,
    pub public: Option<bool>,
    pub tags: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CloneDashboardRequest {
    pub name: String,
    pub copy_widgets: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportDashboardRequest {
    pub source_dashboard_id: String,
    pub name: String,
    pub import_widgets: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DashboardWidget {
    pub id: String,
    pub dashboard_id: String,
    pub dashboard_name: String,
    pub title: String,
    pub widget_type: String,
    pub metric: String,
    pub width: u32,
    pub height: u32,
    pub position_x: u32,
    pub position_y: u32,
    pub config: WidgetConfig,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct WidgetConfig {
    pub colors: Option<Vec<String>>,
    pub theme: Option<String>,
    pub show_legend: Option<bool>,
    pub show_grid: Option<bool>,
    pub time_range: Option<String>,
    pub aggregation: Option<String>,
    pub threshold: Option<f64>,
    pub unit: Option<String>,
    pub decimal_places: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateWidgetRequest {
    pub title: String,
    pub widget_type: String,
    pub metric: String,
    pub width: u32,
    pub height: u32,
    pub position_x: u32,
    pub position_y: u32,
    pub config: WidgetConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChartTemplate {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub category: String,
    pub chart_type: String,
    pub config: serde_json::Value,
    pub usage_count: u32,
    pub custom: bool,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateChartTemplateRequest {
    pub name: String,
    pub description: Option<String>,
    pub category: String,
    pub chart_type: String,
    pub config: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DashboardCategory {
    pub name: String,
    pub description: Option<String>,
    pub dashboard_count: u32,
}

// ============================================================================
// Custom Dashboard System API Functions
// ============================================================================

/// Get all custom dashboards
pub async fn get_custom_dashboards() -> Result<Vec<CustomDashboard>, ApiError> {
    fetch_json("/dashboards/custom").await
}

/// Get specific custom dashboard
pub async fn get_custom_dashboard(id: &str) -> Result<CustomDashboard, ApiError> {
    fetch_json(&format!("/dashboards/custom/{}", id)).await
}

/// Create custom dashboard
pub async fn create_custom_dashboard(request: CreateDashboardRequest) -> Result<CustomDashboard, ApiError> {
    post_json("/dashboards/custom", &request).await
}

/// Update custom dashboard
pub async fn update_custom_dashboard(id: &str, request: UpdateDashboardRequest) -> Result<CustomDashboard, ApiError> {
    put_json(&format!("/dashboards/custom/{}", id), &request).await
}

/// Delete custom dashboard
pub async fn delete_custom_dashboard(id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/dashboards/custom/{}", id)).await
}

/// Clone custom dashboard
pub async fn clone_custom_dashboard(id: &str, request: CloneDashboardRequest) -> Result<CustomDashboard, ApiError> {
    post_json(&format!("/dashboards/custom/{}/clone", id), &request).await
}

/// Export dashboard
pub async fn export_dashboard(id: &str) -> Result<String, ApiError> {
    fetch_text(&format!("/dashboards/custom/{}/export", id)).await
}

/// Import dashboard
pub async fn import_dashboard(request: ImportDashboardRequest) -> Result<CustomDashboard, ApiError> {
    post_json("/dashboards/custom/import", &request).await
}

/// Get dashboard widgets
pub async fn get_dashboard_widgets(dashboard_id: &str) -> Result<Vec<DashboardWidget>, ApiError> {
    fetch_json(&format!("/dashboards/custom/{}/widgets", dashboard_id)).await
}

/// Add widget to dashboard
pub async fn add_dashboard_widget(dashboard_id: &str, request: CreateWidgetRequest) -> Result<DashboardWidget, ApiError> {
    post_json(&format!("/dashboards/custom/{}/widgets", dashboard_id), &request).await
}

/// Update widget position
pub async fn update_widget_position(dashboard_id: &str, widget_id: &str, x: u32, y: u32) -> Result<(), ApiError> {
    put_json(&format!("/dashboards/custom/{}/widgets/{}/position", dashboard_id, widget_id),
             &serde_json::json!({"x": x, "y": y})).await
}

/// Remove widget from dashboard
pub async fn remove_dashboard_widget(dashboard_id: &str, widget_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/dashboards/custom/{}/widgets/{}", dashboard_id, widget_id)).await
}

/// Save dashboard layout
pub async fn save_dashboard_layout(dashboard_id: &str, widgets: Vec<DashboardWidget>) -> Result<(), ApiError> {
    put_json(&format!("/dashboards/custom/{}/layout", dashboard_id), &widgets).await
}

/// Get all widgets across dashboards
pub async fn get_all_widgets() -> Result<Vec<DashboardWidget>, ApiError> {
    fetch_json("/dashboards/widgets").await
}

/// Bulk delete widgets
pub async fn bulk_delete_widgets(widget_ids: Vec<String>) -> Result<(), ApiError> {
    post_json("/dashboards/widgets/bulk-delete", &widget_ids).await
}

/// Bulk move widgets
pub async fn bulk_move_widgets(widget_ids: Vec<String>, target_dashboard_id: String) -> Result<(), ApiError> {
    post_json("/dashboards/widgets/bulk-move",
              &serde_json::json!({"widget_ids": widget_ids, "target_dashboard_id": target_dashboard_id})).await
}

/// Get chart templates
pub async fn get_chart_templates() -> Result<Vec<ChartTemplate>, ApiError> {
    fetch_json("/dashboards/chart-templates").await
}

/// Create chart template
pub async fn create_chart_template(request: CreateChartTemplateRequest) -> Result<ChartTemplate, ApiError> {
    post_json("/dashboards/chart-templates", &request).await
}

/// Delete chart template
pub async fn delete_chart_template(id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/dashboards/chart-templates/{}", id)).await
}

/// Export chart template
pub async fn export_chart_template(id: &str) -> Result<String, ApiError> {
    fetch_text(&format!("/dashboards/chart-templates/{}/export", id)).await
}

/// Get featured dashboards
pub async fn get_featured_dashboards() -> Result<Vec<CustomDashboard>, ApiError> {
    fetch_json("/dashboards/gallery/featured").await
}

/// Get public dashboards
pub async fn get_public_dashboards() -> Result<Vec<CustomDashboard>, ApiError> {
    fetch_json("/dashboards/gallery/public").await
}

/// Get dashboard categories
pub async fn get_dashboard_categories() -> Result<Vec<DashboardCategory>, ApiError> {
    fetch_json("/dashboards/categories").await
}

// ============================================================================
// Audit Log & Compliance Data Structures
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: String,
    pub timestamp: String,
    pub event_type: String,
    pub severity: String,
    pub user: Option<String>,
    pub source_ip: Option<String>,
    pub user_agent: Option<String>,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub action: String,
    pub details: serde_json::Value,
    pub success: bool,
    pub error_message: Option<String>,
    pub session_id: Option<String>,
    pub correlation_id: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditFilter {
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub event_types: Vec<String>,
    pub users: Vec<String>,
    pub resource_types: Vec<String>,
    pub actions: Vec<String>,
    pub severity_levels: Vec<String>,
    pub success_filter: Option<bool>,
    pub source_ips: Vec<String>,
    pub search_term: Option<String>,
    pub correlation_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditFilterOptions {
    pub event_types: Vec<String>,
    pub users: Vec<String>,
    pub resource_types: Vec<String>,
    pub actions: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditExportRequest {
    pub format: String,
    pub filter: AuditFilter,
    pub fields: Vec<String>,
    pub include_details: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SecurityEvent {
    pub id: String,
    pub timestamp: String,
    pub event_type: String,
    pub severity: String,
    pub source_ip: String,
    pub target_user: Option<String>,
    pub target_resource: Option<String>,
    pub description: String,
    pub details: serde_json::Value,
    pub status: String,
    pub assigned_to: Option<String>,
    pub resolution: Option<String>,
    pub related_events: Vec<String>,
    pub indicators: Vec<SecurityIndicator>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SecurityIndicator {
    pub indicator_type: String,
    pub value: String,
    pub confidence: f64,
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SecurityThreat {
    pub id: String,
    pub name: String,
    pub description: String,
    pub severity: String,
    pub affected_events: Vec<String>,
    pub detection_time: String,
    pub status: String,
    pub mitigations: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SecurityStats {
    pub total_events_24h: u64,
    pub critical_events_24h: u64,
    pub blocked_ips_24h: u64,
    pub failed_logins_24h: u64,
    pub active_threats: u64,
    pub events_by_type: std::collections::HashMap<String, u64>,
    pub events_by_severity: std::collections::HashMap<String, u64>,
    pub top_source_ips: Vec<(String, u64)>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ComplianceFramework {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub enabled: bool,
    pub total_controls: u32,
    pub passing_controls: u32,
    pub failing_controls: u32,
    pub not_applicable_controls: u32,
    pub last_assessment: String,
    pub score: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ComplianceControl {
    pub id: String,
    pub framework_id: String,
    pub control_id: String,
    pub title: String,
    pub description: String,
    pub category: String,
    pub severity: String,
    pub status: String,
    pub evidence: Vec<ComplianceEvidence>,
    pub recommendations: Vec<String>,
    pub last_checked: String,
    pub manual_override: Option<ManualOverride>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ComplianceEvidence {
    pub evidence_type: String,
    pub source: String,
    pub timestamp: String,
    pub data: serde_json::Value,
    pub verified: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ManualOverride {
    pub status: String,
    pub reason: String,
    pub user: String,
    pub timestamp: String,
    pub expires: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ComplianceReport {
    pub id: String,
    pub framework_id: String,
    pub framework_name: String,
    pub report_type: String,
    pub generated_at: String,
    pub generated_by: String,
    pub period_start: String,
    pub period_end: String,
    pub overall_score: f64,
    pub status: String,
    pub download_url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GenerateReportRequest {
    pub framework_id: String,
    pub report_type: String,
    pub period_start: String,
    pub period_end: String,
    pub include_evidence: bool,
    pub format: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Investigation {
    pub id: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub severity: String,
    pub created_at: String,
    pub updated_at: String,
    pub created_by: String,
    pub assigned_to: Option<String>,
    pub related_events: Vec<String>,
    pub timeline: Vec<TimelineEntry>,
    pub findings: Vec<Finding>,
    pub artifacts: Vec<Artifact>,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TimelineEntry {
    pub id: String,
    pub timestamp: String,
    pub event_type: String,
    pub description: String,
    pub source: String,
    pub details: serde_json::Value,
    pub important: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Finding {
    pub id: String,
    pub title: String,
    pub description: String,
    pub finding_type: String,
    pub severity: String,
    pub evidence: Vec<String>,
    pub created_at: String,
    pub created_by: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Artifact {
    pub id: String,
    pub name: String,
    pub artifact_type: String,
    pub description: String,
    pub size_bytes: u64,
    pub hash: String,
    pub collected_at: String,
    pub collected_by: String,
    pub download_url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CreateInvestigationRequest {
    pub title: String,
    pub description: String,
    pub severity: String,
    pub related_events: Vec<String>,
    pub tags: Vec<String>,
}

// Audit Log API Functions
pub async fn get_audit_events(filter: AuditFilter, page: u64, page_size: u64) -> Result<(Vec<AuditEvent>, u64), ApiError> {
    let request = serde_json::json!({
        "filter": filter,
        "page": page,
        "page_size": page_size
    });
    post_json("/audit/events/search", &request).await
}

pub async fn get_audit_filter_options() -> Result<AuditFilterOptions, ApiError> {
    fetch_json("/audit/filter-options").await
}

pub async fn export_audit_events(request: AuditExportRequest) -> Result<String, ApiError> {
    post_json("/audit/export", &request).await
}

// Security Events API Functions
pub async fn get_security_events(time_range: String) -> Result<Vec<SecurityEvent>, ApiError> {
    fetch_json(&format!("/security/events?range={}", time_range)).await
}

pub async fn update_security_event_status(event_id: String, status: String) -> Result<(), ApiError> {
    put_json(&format!("/security/events/{}/status", event_id), &serde_json::json!({"status": status})).await
}

pub async fn get_active_threats() -> Result<Vec<SecurityThreat>, ApiError> {
    fetch_json("/security/threats/active").await
}

pub async fn get_security_stats() -> Result<SecurityStats, ApiError> {
    fetch_json("/security/stats").await
}

pub async fn block_ip_address(ip: String) -> Result<(), ApiError> {
    post_json("/security/block-ip", &serde_json::json!({"ip": ip})).await
}

// Compliance API Functions
pub async fn get_compliance_frameworks() -> Result<Vec<ComplianceFramework>, ApiError> {
    fetch_json("/compliance/frameworks").await
}

pub async fn get_compliance_controls(framework_id: String) -> Result<Vec<ComplianceControl>, ApiError> {
    fetch_json(&format!("/compliance/frameworks/{}/controls", framework_id)).await
}

pub async fn run_compliance_assessment(framework_id: String) -> Result<(), ApiError> {
    post_json(&format!("/compliance/frameworks/{}/assess", framework_id), &()).await
}

pub async fn set_control_override(control_id: String, override_data: ManualOverride) -> Result<(), ApiError> {
    put_json(&format!("/compliance/controls/{}/override", control_id), &override_data).await
}

pub async fn get_compliance_reports() -> Result<Vec<ComplianceReport>, ApiError> {
    fetch_json("/compliance/reports").await
}

pub async fn generate_compliance_report(request: GenerateReportRequest) -> Result<ComplianceReport, ApiError> {
    post_json("/compliance/reports/generate", &request).await
}

// Forensics API Functions
pub async fn get_investigations() -> Result<Vec<Investigation>, ApiError> {
    fetch_json("/forensics/investigations").await
}

pub async fn get_investigation(id: String) -> Result<Investigation, ApiError> {
    fetch_json(&format!("/forensics/investigations/{}", id)).await
}

pub async fn create_investigation(request: CreateInvestigationRequest) -> Result<Investigation, ApiError> {
    post_json("/forensics/investigations", &request).await
}

pub async fn update_investigation_status(id: String, status: String) -> Result<(), ApiError> {
    put_json(&format!("/forensics/investigations/{}/status", id), &serde_json::json!({"status": status})).await
}

pub async fn add_investigation_finding(investigation_id: String, finding: Finding) -> Result<(), ApiError> {
    post_json(&format!("/forensics/investigations/{}/findings", investigation_id), &finding).await
}

pub async fn collect_investigation_artifact(investigation_id: String, request: serde_json::Value) -> Result<Artifact, ApiError> {
    post_json(&format!("/forensics/investigations/{}/artifacts/collect", investigation_id), &request).await
}

// ============================================================================
// MFA Management Data Structures
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MfaStatus {
    pub enabled: bool,
    pub methods: Vec<MfaMethod>,
    pub backup_codes_remaining: u32,
    pub last_verified: Option<String>,
    pub trusted_devices: Vec<TrustedDevice>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MfaMethod {
    pub id: String,
    pub method_type: String,
    pub name: String,
    pub enabled: bool,
    pub registered_at: String,
    pub last_used: Option<String>,
    pub is_primary: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TotpSetup {
    pub secret: String,
    pub qr_code_url: String,
    pub backup_codes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrustedDevice {
    pub id: String,
    pub name: String,
    pub device_type: String,
    pub browser: String,
    pub os: String,
    pub ip_address: String,
    pub location: Option<String>,
    pub trusted_at: String,
    pub expires_at: String,
    pub last_used: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MfaEnforcementPolicy {
    pub global_enforcement: bool,
    pub grace_period_days: u32,
    pub required_for_roles: Vec<String>,
    pub allowed_methods: Vec<String>,
    pub require_backup_codes: bool,
    pub max_trusted_devices: u32,
    pub trusted_device_expiry_days: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct UserMfaStatus {
    pub user_id: String,
    pub username: String,
    pub email: String,
    pub mfa_enabled: bool,
    pub methods_count: u32,
    pub last_mfa_activity: Option<String>,
    pub enforcement_status: String,
}

// MFA API Functions
pub async fn get_mfa_status() -> Result<MfaStatus, ApiError> {
    fetch_json("/auth/mfa/status").await
}

pub async fn initiate_totp_setup() -> Result<TotpSetup, ApiError> {
    post_json("/auth/mfa/totp/setup", &()).await
}

pub async fn verify_totp_setup(code: String) -> Result<Vec<String>, ApiError> {
    post_json("/auth/mfa/totp/verify", &serde_json::json!({"code": code})).await
}

pub async fn register_webauthn_credential(name: String) -> Result<(), ApiError> {
    post_json("/auth/mfa/webauthn/register", &serde_json::json!({"name": name})).await
}

pub async fn regenerate_mfa_backup_codes() -> Result<Vec<String>, ApiError> {
    post_json("/auth/mfa/backup-codes/regenerate", &()).await
}

pub async fn revoke_trusted_device(device_id: String) -> Result<(), ApiError> {
    delete_json(&format!("/auth/mfa/trusted-devices/{}", device_id)).await
}

pub async fn disable_mfa(method_id: String, verification_code: String) -> Result<(), ApiError> {
    post_json(&format!("/auth/mfa/methods/{}/disable", method_id), &serde_json::json!({"code": verification_code})).await
}

pub async fn set_primary_mfa_method(method_id: String) -> Result<(), ApiError> {
    put_json(&format!("/auth/mfa/methods/{}/primary", method_id), &()).await
}

pub async fn get_mfa_enforcement_policy() -> Result<MfaEnforcementPolicy, ApiError> {
    fetch_json("/auth/mfa/policy").await
}

pub async fn update_mfa_enforcement_policy(policy: MfaEnforcementPolicy) -> Result<(), ApiError> {
    put_json("/auth/mfa/policy", &policy).await
}

pub async fn get_users_mfa_status() -> Result<Vec<UserMfaStatus>, ApiError> {
    fetch_json("/auth/mfa/users").await
}

pub async fn reset_user_mfa(user_id: String) -> Result<(), ApiError> {
    post_json(&format!("/auth/mfa/users/{}/reset", user_id), &()).await
}

pub async fn enforce_user_mfa(user_id: String) -> Result<(), ApiError> {
    post_json(&format!("/auth/mfa/users/{}/enforce", user_id), &()).await
}

// =============================================================================
// Storage Management API
// =============================================================================

// Storage Pool Info (for volume management)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoragePoolInfo {
    pub id: String,
    pub name: String,
    pub pool_type: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub status: String,
    pub path: Option<String>,
}

// Disk Management Types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub used_bytes: u64,
    pub model: String,
    pub serial: String,
    pub disk_type: String,
    pub interface: String,
    pub partitions: Vec<PartitionInfo>,
    pub smart_status: String,
    pub temperature: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartitionInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub used_bytes: u64,
    pub filesystem: String,
    pub mount_point: Option<String>,
    pub label: Option<String>,
}

// Volume Management Types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeInfo {
    pub id: String,
    pub name: String,
    pub pool_id: String,
    pub pool_name: String,
    pub size_bytes: u64,
    pub used_bytes: u64,
    pub volume_type: String,
    pub format: String,
    pub attached_to: Option<String>,
    pub attached_name: Option<String>,
    pub snapshots: u32,
    pub created_at: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeSnapshot {
    pub id: String,
    pub name: String,
    pub volume_id: String,
    pub size_bytes: u64,
    pub created_at: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VolumeFilters {
    pub search: String,
    pub pool_id: Option<String>,
    pub volume_type: Option<String>,
    pub status: Option<String>,
    pub attached_only: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateVolumeForm {
    pub name: String,
    pub pool_id: String,
    pub size_gb: u64,
    pub volume_type: String,
    pub format: String,
    pub thin_provisioned: bool,
    pub description: String,
}

// SMART Monitoring Types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartDiskInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub model: String,
    pub serial: String,
    pub firmware: String,
    pub capacity_bytes: u64,
    pub disk_type: String,
    pub interface: String,
    pub smart_enabled: bool,
    pub smart_status: SmartStatus,
    pub temperature: Option<u32>,
    pub power_on_hours: Option<u64>,
    pub power_cycle_count: Option<u64>,
    pub attributes: Vec<SmartAttribute>,
    pub last_test: Option<SmartTestResult>,
    pub health_score: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SmartStatus {
    Healthy,
    Warning,
    Critical,
    Unknown,
}

impl SmartStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SmartStatus::Healthy => "healthy",
            SmartStatus::Warning => "warning",
            SmartStatus::Critical => "critical",
            SmartStatus::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartAttribute {
    pub id: u8,
    pub name: String,
    pub value: u64,
    pub worst: u64,
    pub threshold: u64,
    pub raw_value: String,
    pub status: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartTestResult {
    pub test_type: String,
    pub status: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub lifetime_hours: u64,
    pub error_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartAlert {
    pub id: String,
    pub disk_id: String,
    pub disk_name: String,
    pub alert_type: String,
    pub severity: String,
    pub message: String,
    pub attribute_id: Option<u8>,
    pub attribute_name: Option<String>,
    pub created_at: String,
    pub acknowledged: bool,
}

// Disk Management API
pub async fn get_disk_list() -> Result<Vec<DiskInfo>, ApiError> {
    fetch_json("/storage/disks").await
}

pub async fn get_disk_details(disk_id: &str) -> Result<DiskInfo, ApiError> {
    fetch_json(&format!("/storage/disks/{}", disk_id)).await
}

pub async fn get_disk_partitions(disk_id: &str) -> Result<Vec<PartitionInfo>, ApiError> {
    fetch_json(&format!("/storage/disks/{}/partitions", disk_id)).await
}

pub async fn create_partition(disk_id: &str, size_bytes: u64, filesystem: &str) -> Result<PartitionInfo, ApiError> {
    post_json(
        &format!("/storage/disks/{}/partitions", disk_id),
        &serde_json::json!({
            "size_bytes": size_bytes,
            "filesystem": filesystem
        })
    ).await
}

pub async fn delete_partition(disk_id: &str, partition_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/storage/disks/{}/partitions/{}", disk_id, partition_id)).await
}

pub async fn format_partition(disk_id: &str, partition_id: &str, filesystem: &str) -> Result<(), ApiError> {
    post_json(
        &format!("/storage/disks/{}/partitions/{}/format", disk_id, partition_id),
        &serde_json::json!({"filesystem": filesystem})
    ).await
}

pub async fn mount_partition(disk_id: &str, partition_id: &str, mount_point: &str) -> Result<(), ApiError> {
    post_json(
        &format!("/storage/disks/{}/partitions/{}/mount", disk_id, partition_id),
        &serde_json::json!({"mount_point": mount_point})
    ).await
}

pub async fn unmount_partition(disk_id: &str, partition_id: &str) -> Result<(), ApiError> {
    post_json(
        &format!("/storage/disks/{}/partitions/{}/unmount", disk_id, partition_id),
        &()
    ).await
}

// Volume Management API
pub async fn get_volumes(filters: VolumeFilters) -> Result<Vec<VolumeInfo>, ApiError> {
    let mut query_params = Vec::new();
    if !filters.search.is_empty() {
        query_params.push(format!("search={}", filters.search));
    }
    if let Some(ref pool_id) = filters.pool_id {
        if !pool_id.is_empty() {
            query_params.push(format!("pool_id={}", pool_id));
        }
    }
    if let Some(ref volume_type) = filters.volume_type {
        if !volume_type.is_empty() {
            query_params.push(format!("type={}", volume_type));
        }
    }
    if let Some(ref status) = filters.status {
        if !status.is_empty() {
            query_params.push(format!("status={}", status));
        }
    }
    if filters.attached_only {
        query_params.push("attached=true".to_string());
    }

    let query_string = if query_params.is_empty() {
        String::new()
    } else {
        format!("?{}", query_params.join("&"))
    };

    fetch_json(&format!("/storage/volumes{}", query_string)).await
}

pub async fn get_volume(volume_id: &str) -> Result<VolumeInfo, ApiError> {
    fetch_json(&format!("/storage/volumes/{}", volume_id)).await
}

pub async fn create_new_volume(form: CreateVolumeForm) -> Result<VolumeInfo, ApiError> {
    post_json("/storage/volumes", &serde_json::json!({
        "name": form.name,
        "pool_id": form.pool_id,
        "size_gb": form.size_gb,
        "volume_type": form.volume_type,
        "format": form.format,
        "thin_provisioned": form.thin_provisioned,
        "description": form.description
    })).await
}

pub async fn resize_volume_api(volume_id: &str, new_size_gb: u64) -> Result<(), ApiError> {
    put_json(
        &format!("/storage/volumes/{}/resize", volume_id),
        &serde_json::json!({"size_gb": new_size_gb})
    ).await
}

pub async fn delete_volume_api(volume_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/storage/volumes/{}", volume_id)).await
}

pub async fn attach_volume(volume_id: &str, vm_id: &str, device: Option<&str>) -> Result<(), ApiError> {
    post_json(
        &format!("/storage/volumes/{}/attach", volume_id),
        &serde_json::json!({
            "vm_id": vm_id,
            "device": device
        })
    ).await
}

pub async fn detach_volume(volume_id: &str) -> Result<(), ApiError> {
    post_json(&format!("/storage/volumes/{}/detach", volume_id), &()).await
}

pub async fn get_volume_snapshots(volume_id: &str) -> Result<Vec<VolumeSnapshot>, ApiError> {
    fetch_json(&format!("/storage/volumes/{}/snapshots", volume_id)).await
}

pub async fn create_volume_snapshot(volume_id: &str, name: &str, description: Option<&str>) -> Result<VolumeSnapshot, ApiError> {
    post_json(
        &format!("/storage/volumes/{}/snapshots", volume_id),
        &serde_json::json!({
            "name": name,
            "description": description
        })
    ).await
}

pub async fn delete_volume_snapshot(volume_id: &str, snapshot_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/storage/volumes/{}/snapshots/{}", volume_id, snapshot_id)).await
}

pub async fn restore_volume_snapshot(volume_id: &str, snapshot_id: &str) -> Result<(), ApiError> {
    post_json(
        &format!("/storage/volumes/{}/snapshots/{}/restore", volume_id, snapshot_id),
        &()
    ).await
}

// SMART Monitoring API
pub async fn get_smart_disk_info() -> Result<Vec<SmartDiskInfo>, ApiError> {
    fetch_json("/storage/smart/disks").await
}

pub async fn get_smart_disk_details(disk_id: &str) -> Result<SmartDiskInfo, ApiError> {
    fetch_json(&format!("/storage/smart/disks/{}", disk_id)).await
}

pub async fn get_smart_alerts() -> Result<Vec<SmartAlert>, ApiError> {
    fetch_json("/storage/smart/alerts").await
}

pub async fn acknowledge_smart_alert(alert_id: &str) -> Result<(), ApiError> {
    put_json(&format!("/storage/smart/alerts/{}/acknowledge", alert_id), &()).await
}

pub async fn start_smart_test(disk_id: &str, test_type: &str) -> Result<(), ApiError> {
    post_json(
        &format!("/storage/smart/disks/{}/test", disk_id),
        &serde_json::json!({"test_type": test_type})
    ).await
}

pub async fn get_smart_test_status(disk_id: &str) -> Result<serde_json::Value, ApiError> {
    fetch_json(&format!("/storage/smart/disks/{}/test/status", disk_id)).await
}

pub async fn enable_smart_monitoring(disk_id: &str) -> Result<(), ApiError> {
    post_json(&format!("/storage/smart/disks/{}/enable", disk_id), &()).await
}

pub async fn disable_smart_monitoring(disk_id: &str) -> Result<(), ApiError> {
    post_json(&format!("/storage/smart/disks/{}/disable", disk_id), &()).await
}

// Storage Migration API
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MigrationStoragePool {
    pub id: String,
    pub name: String,
    pub pool_type: String,
    pub node: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MigratableResource {
    pub id: String,
    pub name: String,
    pub resource_type: String, // vm, container, volume
    pub size_bytes: u64,
    pub current_pool: String,
    pub status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateMigrationRequest {
    pub source_pool_id: String,
    pub target_pool_id: String,
    pub resource_ids: Vec<String>,
    pub options: MigrationOptions,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MigrationOptions {
    pub live_migrate: bool,
    pub verify_data: bool,
    pub compress_transfer: bool,
    pub bandwidth_limit_mbps: Option<u32>,
    pub schedule_time: Option<String>,
    pub delete_source_after: bool,
    pub priority: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StorageMigrationJob {
    pub id: String,
    pub source_pool: String,
    pub target_pool: String,
    pub status: String, // pending, running, paused, completed, failed, cancelled
    pub progress: f32,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub resources: Vec<MigrationResourceStatus>,
    pub options: MigrationOptions,
    pub error_message: Option<String>,
    pub bytes_transferred: u64,
    pub bytes_total: u64,
    pub transfer_rate_mbps: Option<f32>,
    pub estimated_completion: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MigrationResourceStatus {
    pub resource_id: String,
    pub resource_name: String,
    pub status: String, // pending, migrating, verifying, completed, failed
    pub progress: f32,
    pub error_message: Option<String>,
}

pub async fn get_migration_storage_pools() -> Result<Vec<MigrationStoragePool>, ApiError> {
    fetch_json("/storage/pools").await
}

pub async fn get_migratable_resources(pool_id: &str) -> Result<Vec<MigratableResource>, ApiError> {
    fetch_json(&format!("/storage/pools/{}/resources", pool_id)).await
}

pub async fn create_storage_migration(request: CreateMigrationRequest) -> Result<StorageMigrationJob, ApiError> {
    post_json("/storage/migrations", &request).await
}

pub async fn get_storage_migrations() -> Result<Vec<StorageMigrationJob>, ApiError> {
    fetch_json("/storage/migrations").await
}

pub async fn get_storage_migration(migration_id: &str) -> Result<StorageMigrationJob, ApiError> {
    fetch_json(&format!("/storage/migrations/{}", migration_id)).await
}

pub async fn pause_storage_migration(migration_id: &str) -> Result<(), ApiError> {
    post_json(&format!("/storage/migrations/{}/pause", migration_id), &()).await
}

pub async fn resume_storage_migration(migration_id: &str) -> Result<(), ApiError> {
    post_json(&format!("/storage/migrations/{}/resume", migration_id), &()).await
}

pub async fn cancel_storage_migration(migration_id: &str) -> Result<(), ApiError> {
    post_json(&format!("/storage/migrations/{}/cancel", migration_id), &()).await
}

pub async fn retry_storage_migration(migration_id: &str) -> Result<(), ApiError> {
    post_json(&format!("/storage/migrations/{}/retry", migration_id), &()).await
}

pub async fn delete_storage_migration(migration_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/storage/migrations/{}", migration_id)).await
}

// Storage Pools API (generic)
pub async fn get_storage_pools() -> Result<Vec<StoragePoolInfo>, ApiError> {
    fetch_json("/storage/pools").await
}

pub async fn get_storage_pool(pool_id: &str) -> Result<StoragePoolInfo, ApiError> {
    fetch_json(&format!("/storage/pools/{}", pool_id)).await
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateStoragePoolRequest {
    pub name: String,
    pub pool_type: String,
    pub path: Option<String>,
    pub config: serde_json::Value,
}

pub async fn create_storage_pool(request: CreateStoragePoolRequest) -> Result<StoragePoolInfo, ApiError> {
    post_json("/storage/pools", &request).await
}

pub async fn delete_storage_pool(pool_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/storage/pools/{}", pool_id)).await
}

pub async fn get_storage_pool_health(pool_id: &str) -> Result<serde_json::Value, ApiError> {
    fetch_json(&format!("/storage/pools/{}/health", pool_id)).await
}

pub async fn scan_storage_pool(pool_id: &str) -> Result<(), ApiError> {
    post_json(&format!("/storage/pools/{}/scan", pool_id), &()).await
}

// =============================================================================
// Backup Validation & Restore Testing API
// =============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackupValidation {
    pub id: String,
    pub backup_id: String,
    pub backup_name: String,
    pub vm_name: String,
    pub validation_type: String, // checksum, integrity, restore_test
    pub status: String, // pending, running, passed, failed, skipped
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub duration_seconds: Option<u64>,
    pub results: ValidationResults,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ValidationResults {
    pub checksum_valid: Option<bool>,
    pub integrity_valid: Option<bool>,
    pub files_checked: u64,
    pub files_passed: u64,
    pub files_failed: u64,
    pub total_size_bytes: u64,
    pub issues: Vec<ValidationIssue>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValidationIssue {
    pub severity: String, // info, warning, error
    pub category: String, // checksum, permissions, missing, corrupted
    pub path: String,
    pub message: String,
    pub details: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RestoreTest {
    pub id: String,
    pub backup_id: String,
    pub backup_name: String,
    pub test_type: String, // full_restore, partial_restore, boot_test
    pub target_environment: String, // isolated_vm, sandbox, staging
    pub status: String, // pending, provisioning, restoring, testing, cleaning, passed, failed
    pub progress: f64,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub test_results: RestoreTestResults,
    pub cleanup_status: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RestoreTestResults {
    pub restore_successful: bool,
    pub boot_successful: Option<bool>,
    pub service_checks: Vec<ServiceCheck>,
    pub data_integrity_score: Option<f64>,
    pub restore_time_seconds: u64,
    pub issues: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServiceCheck {
    pub service_name: String,
    pub expected_status: String,
    pub actual_status: String,
    pub passed: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ValidationSchedule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub schedule: String, // cron expression
    pub backup_selection: String, // all, recent, specific
    pub validation_types: Vec<String>,
    pub notify_on_failure: bool,
    pub notification_channels: Vec<String>,
    pub last_run: Option<String>,
    pub next_run: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateValidationScheduleRequest {
    pub name: String,
    pub schedule: String,
    pub backup_selection: String,
    pub validation_types: Vec<String>,
    pub notify_on_failure: bool,
    pub notification_channels: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AvailableBackup {
    pub id: String,
    pub name: String,
    pub vm_id: String,
    pub vm_name: String,
    pub backup_type: String,
    pub size_bytes: u64,
    pub created_at: String,
    pub storage_pool: String,
}

// Backup Validation API Functions
pub async fn get_backup_validations() -> Result<Vec<BackupValidation>, ApiError> {
    fetch_json("/backups/validations").await
}

pub async fn get_backup_validation(validation_id: &str) -> Result<BackupValidation, ApiError> {
    fetch_json(&format!("/backups/validations/{}", validation_id)).await
}

pub async fn start_backup_validation(backup_id: &str, validation_type: &str) -> Result<BackupValidation, ApiError> {
    post_json(
        "/backups/validations",
        &serde_json::json!({
            "backup_id": backup_id,
            "validation_type": validation_type
        })
    ).await
}

pub async fn cancel_backup_validation(validation_id: &str) -> Result<(), ApiError> {
    post_json(&format!("/backups/validations/{}/cancel", validation_id), &()).await
}

pub async fn delete_backup_validation(validation_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/backups/validations/{}", validation_id)).await
}

pub async fn retry_backup_validation(validation_id: &str) -> Result<BackupValidation, ApiError> {
    post_json(&format!("/backups/validations/{}/retry", validation_id), &()).await
}

// Restore Test API Functions
pub async fn get_restore_tests() -> Result<Vec<RestoreTest>, ApiError> {
    fetch_json("/backups/restore-tests").await
}

pub async fn get_restore_test(test_id: &str) -> Result<RestoreTest, ApiError> {
    fetch_json(&format!("/backups/restore-tests/{}", test_id)).await
}

pub async fn start_restore_test(backup_id: &str, test_type: &str, target_environment: &str) -> Result<RestoreTest, ApiError> {
    post_json(
        "/backups/restore-tests",
        &serde_json::json!({
            "backup_id": backup_id,
            "test_type": test_type,
            "target_environment": target_environment
        })
    ).await
}

pub async fn cancel_restore_test(test_id: &str) -> Result<(), ApiError> {
    post_json(&format!("/backups/restore-tests/{}/cancel", test_id), &()).await
}

pub async fn cleanup_restore_test(test_id: &str) -> Result<(), ApiError> {
    post_json(&format!("/backups/restore-tests/{}/cleanup", test_id), &()).await
}

pub async fn delete_restore_test(test_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/backups/restore-tests/{}", test_id)).await
}

// Validation Schedule API Functions
pub async fn get_validation_schedules() -> Result<Vec<ValidationSchedule>, ApiError> {
    fetch_json("/backups/validation-schedules").await
}

pub async fn get_validation_schedule(schedule_id: &str) -> Result<ValidationSchedule, ApiError> {
    fetch_json(&format!("/backups/validation-schedules/{}", schedule_id)).await
}

pub async fn create_validation_schedule(request: CreateValidationScheduleRequest) -> Result<ValidationSchedule, ApiError> {
    post_json("/backups/validation-schedules", &request).await
}

pub async fn update_validation_schedule(schedule_id: &str, request: CreateValidationScheduleRequest) -> Result<ValidationSchedule, ApiError> {
    put_json(&format!("/backups/validation-schedules/{}", schedule_id), &request).await
}

pub async fn delete_validation_schedule(schedule_id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/backups/validation-schedules/{}", schedule_id)).await
}

pub async fn enable_validation_schedule(schedule_id: &str) -> Result<(), ApiError> {
    post_json(&format!("/backups/validation-schedules/{}/enable", schedule_id), &()).await
}

pub async fn disable_validation_schedule(schedule_id: &str) -> Result<(), ApiError> {
    post_json(&format!("/backups/validation-schedules/{}/disable", schedule_id), &()).await
}

pub async fn run_validation_schedule_now(schedule_id: &str) -> Result<(), ApiError> {
    post_json(&format!("/backups/validation-schedules/{}/run", schedule_id), &()).await
}

// Available Backups API
pub async fn get_available_backups() -> Result<Vec<AvailableBackup>, ApiError> {
    fetch_json("/backups/available").await
}

pub async fn get_available_backups_for_vm(vm_id: &str) -> Result<Vec<AvailableBackup>, ApiError> {
    fetch_json(&format!("/backups/available?vm_id={}", vm_id)).await
}
