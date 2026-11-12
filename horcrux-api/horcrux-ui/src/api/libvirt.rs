//! libvirt domain import client (mirrors horcrux-api `api::libvirt_import`).
use super::{post_json, ApiError};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LvListed {
    pub domain: String,
    pub uuid: String,
    pub state: String,
    pub cpus: u32,
    pub memory_mb: u64,
    pub disks: Vec<String>,
    /// Horcrux VM that already uses the domain's disk.
    pub imported_as: Option<String>,
    pub problem: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LvList {
    pub available: bool,
    pub domains: Vec<LvListed>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LvDisk {
    pub path: String,
    pub format: String,
    pub bus: String,
    pub discard: bool,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LvNic {
    pub mac: Option<String>,
    pub model: String,
    pub network: String,
    pub bridge: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LvPlan {
    pub domain: String,
    pub name: String,
    pub cpus: u32,
    pub memory_mb: u64,
    pub machine: String,
    pub firmware: String,
    pub secure_boot: bool,
    pub tpm: bool,
    pub nvram: Option<String>,
    pub tpm_state: Option<String>,
    pub disks: Vec<LvDisk>,
    pub nics: Vec<LvNic>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LvPreview {
    pub plan: LvPlan,
    pub id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LvImported {
    pub vm: horcrux_common::VmConfig,
    pub warnings: Vec<String>,
    pub cleanup_offered: bool,
    pub xml_backup: String,
}

pub async fn list() -> Result<LvList, ApiError> {
    post_json("/vms/import/libvirt/list", &serde_json::json!({})).await
}

pub async fn preview(domain: &str) -> Result<LvPreview, ApiError> {
    post_json(
        "/vms/import/libvirt/preview",
        &serde_json::json!({ "domain": domain }),
    )
    .await
}

/// Import progress (a big disk takes minutes): poll `import_status`.
#[derive(Debug, Clone, Deserialize)]
pub struct LvJob {
    pub state: String,
    pub percent: f32,
    pub message: String,
    pub result: Option<LvImported>,
    pub error: Option<String>,
}

#[derive(Deserialize)]
struct Started {
    job: String,
}

/// Starts the import (the disks are copied into Horcrux storage); returns the job id.
pub async fn import(
    domain: &str,
    id: &str,
    name: &str,
    bridge: &str,
    storage: &str,
) -> Result<String, ApiError> {
    let opt = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_string());
    let s: Started = post_json(
        "/vms/import/libvirt",
        &serde_json::json!({
            "domain": domain, "id": opt(id), "name": opt(name), "bridge": opt(bridge),
            "storage": opt(storage),
        }),
    )
    .await?;
    Ok(s.job)
}

pub async fn import_status(job: &str) -> Result<LvJob, ApiError> {
    post_json(
        "/vms/import/libvirt/status",
        &serde_json::json!({ "job": job }),
    )
    .await
}

// ---- opt-in cleanup of the old source (mirrors `api::import_cleanup`)

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CleanupItem {
    pub kind: String,
    pub path: String,
    pub bytes: u64,
    pub remove: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CleanupOffer {
    pub vm_id: String,
    pub source: String,
    pub items: Vec<CleanupItem>,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CleanupDone {
    pub removed: Vec<CleanupItem>,
    pub kept: Vec<CleanupItem>,
    pub xml_backup: Option<String>,
}

pub async fn cleanup_pending() -> Result<Vec<CleanupOffer>, ApiError> {
    post_json("/vms/import/cleanup/pending", &serde_json::json!({})).await
}

pub async fn cleanup_preview(vm_id: &str) -> Result<CleanupOffer, ApiError> {
    post_json(
        "/vms/import/cleanup/preview",
        &serde_json::json!({ "vm_id": vm_id }),
    )
    .await
}

pub async fn cleanup(vm_id: &str) -> Result<CleanupDone, ApiError> {
    post_json(
        "/vms/import/cleanup",
        &serde_json::json!({ "vm_id": vm_id, "confirm": true }),
    )
    .await
}

/// "1.5 GiB" for the cleanup list.
pub fn human_bytes(b: u64) -> String {
    const U: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let (mut v, mut i) = (b as f64, 0);
    while v >= 1024.0 && i < 3 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_human() {
        assert_eq!(human_bytes(10), "10 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(3 << 30), "3.0 GiB");
    }
}
