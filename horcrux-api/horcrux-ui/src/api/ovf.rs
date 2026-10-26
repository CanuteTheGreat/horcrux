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

// ---- ESXi host importer (mirrors horcrux-api `api::esxi`)

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EsxiVm {
    pub folder: String,
    pub vmx: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EsxiDatastore {
    pub name: String,
    pub vms: Vec<EsxiVm>,
}

/// `host` or `host:port` (a single colon followed by digits).
pub fn split_host_port(h: &str) -> (String, Option<u16>) {
    let h = h.trim();
    match h.split_once(':') {
        Some((host, port)) if !port.contains(':') && !host.is_empty() => {
            match port.parse::<u16>() {
                Ok(p) => (host.to_string(), Some(p)),
                Err(_) => (h.to_string(), None),
            }
        }
        _ => (h.to_string(), None),
    }
}

fn esxi_conn(host: &str, user: &str, pass: &str, verify: bool) -> serde_json::Value {
    let (host, port) = split_host_port(host);
    serde_json::json!({
        "host": host, "port": port, "username": user.trim(), "password": pass, "verify_tls": verify,
    })
}

pub async fn esxi_list(
    host: &str,
    user: &str,
    pass: &str,
    verify: bool,
) -> Result<Vec<EsxiDatastore>, ApiError> {
    post_json(
        "/vms/import/esxi/list",
        &esxi_conn(host, user, pass, verify),
    )
    .await
}

fn esxi_target(
    (host, user, pass, verify): (&str, &str, &str, bool),
    datastore: &str,
    folder: &str,
) -> serde_json::Value {
    let mut v = esxi_conn(host, user, pass, verify);
    v["datastore"] = datastore.into();
    v["folder"] = folder.into();
    v
}

pub async fn esxi_preview(
    conn: (&str, &str, &str, bool),
    datastore: &str,
    folder: &str,
) -> Result<OvfPreview, ApiError> {
    post_json(
        "/vms/import/esxi/preview",
        &esxi_target(conn, datastore, folder),
    )
    .await
}

pub async fn esxi_import(
    conn: (&str, &str, &str, bool),
    datastore: &str,
    folder: &str,
    id: &str,
    name: &str,
    bridge: &str,
    storage: &str,
) -> Result<OvfImported, ApiError> {
    let opt = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_string());
    let mut v = esxi_target(conn, datastore, folder);
    v["id"] = serde_json::json!(opt(id));
    v["name"] = serde_json::json!(opt(name));
    v["bridge"] = serde_json::json!(opt(bridge));
    v["storage"] = serde_json::json!(opt(storage));
    post_json("/vms/import/esxi", &v).await
}
