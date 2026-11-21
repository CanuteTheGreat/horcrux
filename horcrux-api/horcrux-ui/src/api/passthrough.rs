//! Host PCI passthrough + mediated devices client (mirrors horcrux-api
//! `api::passthrough` and `api::mdev`; checked by tests/api_contract.rs).
use super::{delete_json, fetch_json, post_json, put_json, ApiError};
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
    /// Nested virtualisation on in the host's KVM (VBS / Memory Integrity).
    #[serde(default)]
    pub nested_virt: bool,
    #[serde(default)]
    pub nested_virt_problem: String,
    /// The host is itself a VM (VBS would be a third nesting level).
    #[serde(default)]
    pub host_is_vm: bool,
    /// Secure Boot OVMF (Microsoft keys) installed.
    #[serde(default)]
    pub secure_boot_firmware: bool,
    #[serde(default)]
    pub swtpm: bool,
    /// ISO builder for the answer-file disc ("" = none).
    #[serde(default)]
    pub iso_tool: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostCpus {
    pub online: Vec<u32>,
    pub performance: Vec<u32>,
    pub efficiency: Vec<u32>,
    pub smt_groups: Vec<Vec<u32>>,
    /// CPUs per L3 cache (AMD CCD/CCX).
    #[serde(default)]
    pub l3_groups: Vec<Vec<u32>>,
    #[serde(default)]
    pub l3_sizes_kb: Vec<u64>,
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
    /// Every display connector of the GPU.
    #[serde(default)]
    pub connectors: Vec<String>,
    /// `Some(false)`: no display outputs (compute card, muxless laptop GPU).
    #[serde(default)]
    pub has_outputs: Option<bool>,
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

/// Gaming VM wizard settings (`GET/PUT /api/vms/gaming/settings`; horcrux-vm
/// `gaming::GamingSettings`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct GamingSettings {
    pub windows_download_page: String,
    pub windows_eval_page: String,
    pub windows_eval_iso_url: String,
    pub virtio_win_url: String,
    pub default_memory_cap_mb: u64,
    pub min_memory_mb: u64,
    pub default_disk_gb: u64,
    pub min_disk_gb: u64,
    pub hugepages_kb: u64,
    pub vbs: bool,
    #[serde(default)]
    pub vbs_on_virtual_host: bool,
    pub smbios_manufacturer: String,
    pub smbios_product: String,
    pub smbios_family: String,
    pub smbios_version: String,
    pub ui_language: String,
    pub input_locale: String,
    pub timezone: String,
    pub image_index: u32,
    pub driver_search_letters: String,
    pub boot_key_secs: u32,
    pub unattend_poll_secs: u64,
}

pub async fn gaming_settings() -> Result<GamingSettings, ApiError> {
    fetch_json("/vms/gaming/settings").await
}

pub async fn save_gaming_settings(s: &GamingSettings) -> Result<GamingSettings, ApiError> {
    put_json("/vms/gaming/settings", s).await
}

/// One row of the per-game anti-cheat table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompatEntry {
    pub title: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub anti_cheat: String,
    /// works | works-if | blocked-by-publisher | unknown
    pub vm_status: String,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub host_status: String,
    #[serde(default)]
    pub note: String,
    pub source: String,
    #[serde(default)]
    pub source_url: String,
    pub checked_on: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompatView {
    pub files: Vec<String>,
    pub games: Vec<CompatEntry>,
}

pub async fn gaming_compat() -> Result<CompatView, ApiError> {
    fetch_json("/vms/gaming/compat").await
}

/// `POST /api/vms/gaming` body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct GamingVmRequest {
    pub name: String,
    pub gpu: Option<String>,
    pub allow_display_loss: bool,
    pub vcpus: Option<u32>,
    pub memory_mb: Option<u64>,
    pub disk_gb: Option<u64>,
    pub storage: Option<String>,
    pub bridge: Option<String>,
    pub windows_iso: String,
    pub virtio_iso: String,
    pub username: String,
    pub password: String,
    pub product_key: Option<String>,
    pub image_name: Option<String>,
    pub ui_language: Option<String>,
    pub input_locale: Option<String>,
    pub timezone: Option<String>,
    pub evdev: Vec<String>,
    pub hugepages: Option<bool>,
    pub vbs: Option<bool>,
    pub games: Vec<String>,
    /// Steam Remote Play / Moonlight readiness (None = off).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_play: Option<horcrux_common::remote_play::VmRemotePlay>,
    pub start: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompatMatch {
    pub query: String,
    pub entry: Option<CompatEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnattendView {
    pub computer_name: String,
    pub username: String,
    pub image_name: Option<String>,
    pub image_index: u32,
    pub ui_language: String,
    pub input_locale: String,
    pub timezone: String,
    pub vbs: bool,
}

/// The wizard's answer: the composed VM (created unless dry run).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GamingPlan {
    pub config: horcrux_common::VmConfig,
    pub unattend: UnattendView,
    pub warnings: Vec<String>,
    pub notes: Vec<String>,
    pub games: Vec<CompatMatch>,
    pub created: bool,
    pub unattend_iso: Option<String>,
}

pub async fn create_gaming_vm(req: &GamingVmRequest) -> Result<GamingPlan, ApiError> {
    post_json("/vms/gaming", req).await
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
