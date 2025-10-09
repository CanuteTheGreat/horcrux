//! API client for communicating with Horcrux backend

use horcrux_common::VmConfig;
use serde::{Deserialize, Serialize};

const API_BASE: &str = "http://localhost:8006/api";

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
