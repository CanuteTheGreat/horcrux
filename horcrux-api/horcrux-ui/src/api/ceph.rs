//! Ceph management client (mirrors horcrux-api `api::ceph_admin`).
use super::{delete_json, fetch_json, post_json, ApiError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthCheck {
    pub name: String,
    pub severity: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CephStatus {
    pub available: bool,
    pub reachable: bool,
    pub health: String,
    pub health_checks: Vec<HealthCheck>,
    pub fsid: Option<String>,
    pub mons_total: u64,
    pub mons_in_quorum: Vec<String>,
    pub mgr_available: bool,
    pub mgr_standbys: u64,
    pub osds_total: u64,
    pub osds_up: u64,
    pub osds_in: u64,
    pub pgs_total: u64,
    pub pools: u64,
    pub pg_states: Vec<(String, u64)>,
    pub bytes_total: u64,
    pub bytes_used: u64,
    pub bytes_avail: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Monitor {
    pub name: String,
    pub rank: u64,
    pub address: String,
    pub in_quorum: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Osd {
    pub id: u64,
    pub name: String,
    pub host: String,
    pub status: String,
    pub reweight: f64,
    pub crush_weight: f64,
    pub device_class: Option<String>,
    pub is_in: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pool {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub min_size: u64,
    pub pg_num: u64,
    pub crush_rule: u64,
    pub applications: Vec<String>,
    pub stored_bytes: u64,
    pub objects: u64,
    pub max_avail: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrushRule {
    pub id: u64,
    pub name: String,
    pub failure_domain: Option<String>,
    pub root: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CephFlag {
    pub name: String,
    pub set: bool,
}

pub async fn ceph_status() -> Result<CephStatus, ApiError> {
    fetch_json("/ceph/status").await
}
pub async fn ceph_monitors() -> Result<Vec<Monitor>, ApiError> {
    fetch_json("/ceph/monitors").await
}
pub async fn ceph_osds() -> Result<Vec<Osd>, ApiError> {
    fetch_json("/ceph/osds").await
}
pub async fn ceph_osd_action(id: u64, action: &str) -> Result<(), ApiError> {
    post_json::<serde_json::Value, _>(&format!("/ceph/osds/{id}/{action}"), &()).await?;
    Ok(())
}
pub async fn ceph_pools() -> Result<Vec<Pool>, ApiError> {
    fetch_json("/ceph/pools").await
}
pub async fn ceph_create_pool(
    name: &str,
    pg_num: u32,
    size: u32,
    application: &str,
) -> Result<(), ApiError> {
    post_json::<serde_json::Value, _>(
        "/ceph/pools",
        &serde_json::json!({"name": name, "pg_num": pg_num, "size": size,
                            "application": if application.is_empty() { None } else { Some(application) }}),
    )
    .await?;
    Ok(())
}
pub async fn ceph_delete_pool(name: &str) -> Result<(), ApiError> {
    let n = urlencoding::encode(name);
    delete_json(&format!("/ceph/pools/{n}?confirm={n}")).await
}
pub async fn ceph_crush_rules() -> Result<Vec<CrushRule>, ApiError> {
    fetch_json("/ceph/crush-rules").await
}
pub async fn ceph_flags() -> Result<Vec<CephFlag>, ApiError> {
    fetch_json("/ceph/flags").await
}
pub async fn ceph_set_flag(flag: &str, set: bool) -> Result<Vec<CephFlag>, ApiError> {
    post_json(
        &format!("/ceph/flags/{flag}"),
        &serde_json::json!({"set": set}),
    )
    .await
}
