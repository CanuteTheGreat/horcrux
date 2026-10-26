//! OVF / OVA import client (mirrors horcrux-api `api::ovf`; checked by
//! tests/api_contract.rs).
use super::{post_json, ApiError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OvfDisk {
    pub file: String,
    pub capacity_bytes: Option<u64>,
    pub bus: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OvfNic {
    pub model: String,
    pub network: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OvfPlan {
    pub name: String,
    pub os: Option<String>,
    pub cpus: u32,
    pub memory_mb: u64,
    pub firmware: String,
    pub disks: Vec<OvfDisk>,
    pub nics: Vec<OvfNic>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct OvfPreview {
    pub plan: OvfPlan,
    pub id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OvfImported {
    pub vm: horcrux_common::VmConfig,
    pub plan: OvfPlan,
    pub warnings: Vec<String>,
}

pub async fn preview(source: &str) -> Result<OvfPreview, ApiError> {
    post_json(
        "/vms/import/ovf/preview",
        &serde_json::json!({ "source": source }),
    )
    .await
}

pub async fn import(
    source: &str,
    id: &str,
    name: &str,
    bridge: &str,
    storage: &str,
) -> Result<OvfImported, ApiError> {
    let opt = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_string());
    post_json(
        "/vms/import/ovf",
        &serde_json::json!({
            "source": source, "id": opt(id), "name": opt(name),
            "bridge": opt(bridge), "storage": opt(storage),
        }),
    )
    .await
}
