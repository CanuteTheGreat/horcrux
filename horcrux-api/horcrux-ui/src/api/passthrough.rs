//! Host PCI passthrough + mediated devices client (mirrors horcrux-api
//! `api::passthrough` and `api::mdev`; checked by tests/api_contract.rs).
use super::{delete_json, fetch_json, post_json, ApiError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PciStatus {
    pub iommu_enabled: bool,
    pub iommu_groups: usize,
    pub vfio_pci_loaded: bool,
    pub mdev_supported: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PciDevice {
    pub address: String,
    pub vendor_id: String,
    pub device_id: String,
    pub vendor_name: String,
    pub class: String,
    pub class_name: String,
    pub driver: Option<String>,
    pub iommu_group: Option<String>,
    pub group_members: Vec<String>,
    pub group_blockers: Vec<String>,
    pub vfio: bool,
    pub sriov_vfs: u32,
    pub sriov_max_vfs: u32,
    pub physfn: Option<String>,
    pub numa_node: Option<i32>,
    /// VMs that list the device in `hardware.hostpci`.
    pub vms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MdevType {
    pub parent: String,
    pub id: String,
    pub name: String,
    pub description: String,
    pub available_instances: u32,
    pub device_api: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MdevDevice {
    pub uuid: String,
    pub parent: String,
    pub mdev_type: String,
    pub vm: Option<String>,
}

pub async fn status() -> Result<PciStatus, ApiError> {
    fetch_json("/gpu/pci/status").await
}

pub async fn list_pci() -> Result<Vec<PciDevice>, ApiError> {
    fetch_json("/gpu/pci/devices").await
}

pub async fn bind_vfio(addr: &str) -> Result<serde_json::Value, ApiError> {
    post_json(
        &format!("/gpu/pci/devices/{}/bind-vfio", urlencoding::encode(addr)),
        &(),
    )
    .await
}

pub async fn unbind_vfio(addr: &str) -> Result<serde_json::Value, ApiError> {
    post_json(
        &format!("/gpu/pci/devices/{}/unbind-vfio", urlencoding::encode(addr)),
        &(),
    )
    .await
}

pub async fn mdev_types() -> Result<Vec<MdevType>, ApiError> {
    fetch_json("/gpu/mdev/types").await
}

pub async fn mdev_devices() -> Result<Vec<MdevDevice>, ApiError> {
    fetch_json("/gpu/mdev/devices").await
}

pub async fn create_mdev(parent: &str, mdev_type: &str) -> Result<serde_json::Value, ApiError> {
    post_json(
        "/gpu/mdev/devices",
        &serde_json::json!({"parent": parent, "type": mdev_type}),
    )
    .await
}

pub async fn delete_mdev(uuid: &str) -> Result<(), ApiError> {
    delete_json(&format!("/gpu/mdev/devices/{}", urlencoding::encode(uuid))).await
}
