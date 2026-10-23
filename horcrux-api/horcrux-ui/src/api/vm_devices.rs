//! VM devices client: memory balloon, guest agent, USB passthrough (mirrors
//! horcrux-api `api::vm_devices`; checked by tests/api_contract.rs).
use super::{delete_json, fetch_json, post_json, ApiError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BalloonInfo {
    pub enabled: bool,
    pub running: bool,
    pub memory_mb: u64,
    pub actual_mb: Option<u64>,
    pub target_mb: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentInterface {
    pub name: String,
    pub mac: Option<String>,
    pub addresses: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentInfo {
    pub enabled: bool,
    pub running: bool,
    pub available: bool,
    pub version: Option<String>,
    pub os_name: Option<String>,
    pub os_version: Option<String>,
    pub kernel: Option<String>,
    pub hostname: Option<String>,
    pub interfaces: Vec<AgentInterface>,
    pub commands: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostUsbDevice {
    pub vendor_id: String,
    pub product_id: String,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub bus: Option<u32>,
    pub device: Option<u32>,
    pub speed_mbps: Option<u32>,
    pub is_hub: bool,
    pub port: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentResult {
    pub action: String,
    pub result: serde_json::Value,
}

pub async fn get_balloon(vm: &str) -> Result<BalloonInfo, ApiError> {
    fetch_json(&format!("/vms/{}/balloon", urlencoding::encode(vm))).await
}

pub async fn set_balloon(vm: &str, target_mb: u64) -> Result<BalloonInfo, ApiError> {
    post_json(
        &format!("/vms/{}/balloon", urlencoding::encode(vm)),
        &serde_json::json!({ "target_mb": target_mb }),
    )
    .await
}

pub async fn get_agent(vm: &str) -> Result<AgentInfo, ApiError> {
    fetch_json(&format!("/vms/{}/agent", urlencoding::encode(vm))).await
}

pub async fn agent_action(vm: &str, action: &str) -> Result<AgentResult, ApiError> {
    post_json(
        &format!("/vms/{}/agent/{}", urlencoding::encode(vm), action),
        &(),
    )
    .await
}

pub async fn list_host_usb() -> Result<Vec<HostUsbDevice>, ApiError> {
    fetch_json("/nodes/usb").await
}

pub async fn attach_usb(
    vm: &str,
    vendor: &str,
    product: &str,
) -> Result<horcrux_common::VmConfig, ApiError> {
    post_json(
        &format!("/vms/{}/usb", urlencoding::encode(vm)),
        &horcrux_common::UsbDevice {
            vendor_id: vendor.into(),
            product_id: product.into(),
        },
    )
    .await
}

pub async fn detach_usb(vm: &str, vendor: &str, product: &str) -> Result<(), ApiError> {
    delete_json(&format!(
        "/vms/{}/usb/{}/{}",
        urlencoding::encode(vm),
        vendor,
        product
    ))
    .await
}

/// Add CPUs to a running VM (`POST /api/vms/:id/hotplug/cpu`).
pub async fn hotplug_cpu(vm: &str, cpus: u32) -> Result<horcrux_common::VmConfig, ApiError> {
    post_json(
        &format!("/vms/{}/hotplug/cpu", urlencoding::encode(vm)),
        &serde_json::json!({ "cpus": cpus }),
    )
    .await
}

/// Add memory to a running VM (`POST /api/vms/:id/hotplug/memory`).
pub async fn hotplug_memory(vm: &str, add_mb: u64) -> Result<horcrux_common::VmConfig, ApiError> {
    post_json(
        &format!("/vms/{}/hotplug/memory", urlencoding::encode(vm)),
        &serde_json::json!({ "add_mb": add_mb }),
    )
    .await
}
