//! Ceph install / release choice, kernel line and platform versions
//! (mirrors horcrux-api `api::ceph_install`, `system_admin::kernel_line`,
//! `system_admin::versions`).
use super::{fetch_json, post_json, put_json, ApiError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CephRelease {
    pub major: u64,
    pub name: String,
    pub newest: String,
    pub versions: Vec<String>,
    /// stable | testing
    pub keyword: String,
    pub repo: String,
    pub available: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CephReleases {
    pub arch: String,
    pub package_manager: bool,
    pub installed: Option<String>,
    pub installed_major: Option<u64>,
    pub pinned: Option<u64>,
    pub releases: Vec<CephRelease>,
    pub recommended: Option<u64>,
    pub configured: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonVersions {
    pub by_type: Vec<(String, Vec<(u64, u64)>)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CephUpgradeStatus {
    pub installed: Option<String>,
    pub installed_major: Option<u64>,
    pub pinned: Option<u64>,
    pub daemons: Option<DaemonVersions>,
    pub targets: Vec<u64>,
    pub next_step: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskStarted {
    pub upid: String,
}

/// Upstream release name, or `Ceph N` (same table as the server).
pub fn release_label(major: u64, name: &str) -> String {
    if name.is_empty() {
        format!("Ceph {major}")
    } else {
        format!("{name} ({major})")
    }
}

pub async fn ceph_releases() -> Result<CephReleases, ApiError> {
    fetch_json("/ceph/releases").await
}
pub async fn ceph_install(
    release: u64,
    bootstrap: serde_json::Value,
) -> Result<TaskStarted, ApiError> {
    post_json(
        "/ceph/install",
        &serde_json::json!({"release": release.to_string(), "bootstrap": bootstrap}),
    )
    .await
}
pub async fn ceph_upgrade_status() -> Result<CephUpgradeStatus, ApiError> {
    fetch_json("/ceph/upgrade").await
}
pub async fn ceph_upgrade(release: u64, steps: Vec<String>) -> Result<TaskStarted, ApiError> {
    post_json(
        "/ceph/upgrade",
        &serde_json::json!({"release": release.to_string(), "steps": steps}),
    )
    .await
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KernelCap {
    pub max: String,
    pub by: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KernelLineStatus {
    /// default | newest
    pub line: String,
    pub arch: String,
    pub running: String,
    pub installed: Vec<String>,
    pub provider: String,
    pub provider_installed: Option<String>,
    pub default_version: Option<String>,
    pub newest_version: Option<String>,
    pub newest_available: Option<String>,
    pub cap: Option<KernelCap>,
    pub held_back: Option<String>,
    pub update_pending: Option<String>,
    pub pins_in_sync: bool,
    pub reboot_required: bool,
}

pub async fn kernel_line() -> Result<KernelLineStatus, ApiError> {
    fetch_json("/system/kernel-line").await
}
pub async fn set_kernel_line(line: &str) -> Result<KernelLineStatus, ApiError> {
    put_json("/system/kernel-line", &serde_json::json!({"line": line})).await
}
pub async fn apply_kernel_line() -> Result<TaskStarted, ApiError> {
    post_json("/system/kernel-line/apply", &serde_json::json!({})).await
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Component {
    pub id: String,
    pub name: String,
    pub package: String,
    pub version: Option<String>,
    pub running: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Versions {
    pub horcrux: String,
    pub package_db: String,
    pub components: Vec<Component>,
}

pub async fn versions() -> Result<Versions, ApiError> {
    fetch_json("/system/versions").await
}
