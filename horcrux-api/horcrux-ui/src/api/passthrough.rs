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

/// `GET /api/gpu/pci/gaming-check` (horcrux-vm `handoff::GamingReport`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GamingReport {
    pub kvm: bool,
    pub iommu_groups: usize,
    pub vfio_pci_loaded: bool,
    pub cpus: HostCpus,
    pub suggested_vcpus: u32,
    pub suggested_pinning: String,
    /// (page KiB, total, free)
    pub hugepages: Vec<(u64, u64, u64)>,
    pub gpus: Vec<GpuCheck>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostCpus {
    pub online: Vec<u32>,
    pub performance: Vec<u32>,
    pub efficiency: Vec<u32>,
    pub smt_groups: Vec<Vec<u32>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpuHolder {
    pub pid: u32,
    pub command: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpuQuirk {
    pub vendor: String,
    pub severity: String,
    pub title: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub links: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpuCheck {
    pub address: String,
    pub vendor_id: String,
    pub device_id: String,
    pub vendor_name: String,
    pub driver: Option<String>,
    pub iommu_group: Option<String>,
    pub functions: Vec<String>,
    pub displays: Vec<String>,
    pub host_keeps_display: bool,
    pub reset_methods: Vec<String>,
    pub holders: Vec<GpuHolder>,
    pub quirks: Vec<GpuQuirk>,
    pub verdict: String,
    pub reasons: Vec<String>,
}

pub async fn gaming_check() -> Result<GamingReport, ApiError> {
    fetch_json("/gpu/pci/gaming-check").await
}

/// `GET /api/gpu/pci/protected-check` (horcrux-vm `protected::ProtectedReport`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProtectedReport {
    pub hardware: ConfidentialHw,
    pub layers: ProtectedLayers,
    pub tier: String,
    pub confidential_enabled: bool,
    pub protected_enabled: bool,
    pub reasons: Vec<String>,
    pub confidential_gpu_passthrough: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfidentialHw {
    pub vendor: String,
    pub sev: bool,
    pub sev_es: bool,
    pub sev_snp: bool,
    pub tdx: bool,
    pub memory_encryption: bool,
    pub memory_encryption_active: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProtectedLayers {
    pub lockdown: String,
    pub lockdown_ok: bool,
    pub ptrace_scope: i32,
    pub ima: bool,
    pub evm: bool,
    pub secure_boot: bool,
    pub guest_memfd: bool,
    pub sw_protected_vm: bool,
    pub host_read_refused: Option<bool>,
}

pub async fn protected_check() -> Result<ProtectedReport, ApiError> {
    fetch_json("/gpu/pci/protected-check").await
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
