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

// ---- hardware GPU slicing (mirrors horcrux-api `api::gpu_slice`, horcrux-vm
// `vm::gpucap` / `vm::sriov`; checked by tests/api_contract.rs) ----

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MigrationInfo {
    /// `supported`, `unsupported` or `unknown`.
    pub state: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub stop_copy: bool,
    #[serde(default)]
    pub p2p: bool,
    #[serde(default)]
    pub pre_copy: bool,
}

impl MigrationInfo {
    /// Text for the UI: what a VM using the device gets on host shutdown.
    pub fn label(&self) -> String {
        match self.state.as_str() {
            "supported" => "host save (VFIO migration supported)".to_string(),
            "unsupported" => format!("not saveable: {}", self.reason),
            _ => format!("unknown: {}", self.reason),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModeStatus {
    pub available: bool,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VfInfo {
    pub vf: u32,
    pub address: String,
    pub driver: Option<String>,
    pub iommu_group: Option<String>,
    pub vram_mib: Option<u64>,
    pub exec_quantum_ms: Option<u64>,
    pub preempt_timeout_us: Option<u64>,
    pub migration: MigrationInfo,
    #[serde(default)]
    pub vm: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SriovCap {
    pub driver: String,
    pub total_vfs: u32,
    pub current_vfs: u32,
    pub autoprobe: Option<bool>,
    pub per_vf_resources: bool,
    pub vf_drivers: Vec<String>,
    pub vfs: Vec<VfInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MdevTypeInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub available_instances: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MdevInstanceInfo {
    pub uuid: String,
    pub mdev_type: String,
    pub migration: MigrationInfo,
    #[serde(default)]
    pub vm: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VgpuCap {
    pub types: Vec<MdevTypeInfo>,
    pub devices: Vec<MdevInstanceInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpuCapability {
    pub address: String,
    pub vendor_id: String,
    pub vendor_name: String,
    pub device_id: String,
    pub driver: Option<String>,
    pub iommu_group: Option<String>,
    pub passthrough: ModeStatus,
    pub sriov: Option<SriovCap>,
    pub vendor_vgpu: Option<VgpuCap>,
    pub why_not: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VfSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vram_quota_mib: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_quantum_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preempt_timeout_us: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PfSpec {
    pub pf: String,
    pub num_vfs: u32,
    /// `auto`, `vfio-pci` or `xe-vfio-pci`.
    #[serde(default = "auto")]
    pub vf_driver: String,
    #[serde(default)]
    pub autoprobe: bool,
    #[serde(flatten)]
    pub all: VfSpec,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub vfs: std::collections::BTreeMap<u32, VfSpec>,
}

fn auto() -> String {
    "auto".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SriovConfig {
    pub apply_on_boot: bool,
    pub settle_timeout_s: u64,
    pub generations_keep: usize,
    pub pfs: Vec<PfSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SriovStep {
    pub pf: String,
    pub action: String,
    pub target: String,
    pub detail: String,
    pub value: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SriovPlan {
    pub steps: Vec<SriovStep>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SriovStepResult {
    #[serde(flatten)]
    pub step: SriovStep,
    pub status: String,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SriovReport {
    pub ok: bool,
    pub dry_run: bool,
    pub plan: SriovPlan,
    pub results: Vec<SriovStepResult>,
    pub generation: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SriovGeneration {
    pub id: u64,
    pub at: u64,
    pub origin: String,
    pub config: SriovConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceSaveInfo {
    pub device: String,
    pub kind: String,
    pub migration: MigrationInfo,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VmDeviceSave {
    pub devices: Vec<DeviceSaveInfo>,
    pub host_save: bool,
    pub summary: String,
}

pub async fn gpu_capabilities() -> Result<Vec<GpuCapability>, ApiError> {
    fetch_json("/gpu/capabilities").await
}

pub async fn sriov_config() -> Result<SriovConfig, ApiError> {
    fetch_json("/gpu/sriov/config").await
}

pub async fn sriov_plan(cfg: &SriovConfig) -> Result<SriovPlan, ApiError> {
    post_json("/gpu/sriov/plan", &serde_json::json!({ "config": cfg })).await
}

pub async fn sriov_apply(cfg: &SriovConfig) -> Result<SriovReport, ApiError> {
    post_json("/gpu/sriov/apply", &serde_json::json!({ "config": cfg })).await
}

pub async fn sriov_generations() -> Result<Vec<SriovGeneration>, ApiError> {
    fetch_json("/gpu/sriov/generations").await
}

pub async fn sriov_rollback(generation: Option<u64>) -> Result<SriovReport, ApiError> {
    post_json(
        "/gpu/sriov/rollback",
        &serde_json::json!({ "generation": generation }),
    )
    .await
}

pub async fn vm_device_save(vm: &str) -> Result<VmDeviceSave, ApiError> {
    fetch_json(&format!(
        "/vms/{}/host-device-save",
        urlencoding::encode(vm)
    ))
    .await
}
