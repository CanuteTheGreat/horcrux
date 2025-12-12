//! API client for communicating with Horcrux backend

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiError {
    pub message: String,
}

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
pub async fn get_active_alerts() -> Result<Vec<Alert>, ApiError> {
    let response = reqwasm::http::Request::get(&format!("{}/alerts/active", API_BASE))
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
pub struct ClusterNode {
    pub name: String,
    pub ip: String,
    pub status: String,
    pub architecture: String,
    pub cpu_cores: u32,
    pub memory_total: u64,
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
    pub name: String,
    pub runtime: String,
    pub image: String,
    pub status: String,
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
pub async fn get_vm_snapshots(vm_id: &str) -> Result<Vec<Snapshot>, ApiError> {
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
