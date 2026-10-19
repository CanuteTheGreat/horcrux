//! Monitoring > Dashboards API client (mirrors horcrux-api `api::dashboards`;
//! checked against the live API by tests/api_contract.rs).
use super::{delete_json, fetch_json, post_json, put_json, ApiError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DashboardPanel {
    #[serde(default)]
    pub id: String,
    pub title: String,
    /// Metrics Explorer query
    pub query: String,
    /// line | area | bar | stat
    pub chart: String,
    /// 1 = half width, 2 = full width
    pub width: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dashboard {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// 15m | 1h | 6h | 24h
    pub time_range: String,
    pub refresh_secs: u32,
    #[serde(default)]
    pub panels: Vec<DashboardPanel>,
    #[serde(default)]
    pub created_by: String,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

/// Body of create / update.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DashboardRequest {
    pub name: String,
    pub description: String,
    pub time_range: String,
    pub refresh_secs: u32,
    pub panels: Vec<DashboardPanel>,
}

pub const RANGES: &[(&str, &str, i64)] = &[
    ("15m", "Last 15 minutes", 900),
    ("1h", "Last hour", 3600),
    ("6h", "Last 6 hours", 6 * 3600),
    ("24h", "Last 24 hours", 24 * 3600),
];

/// Seconds covered by a time range id.
pub fn range_secs(id: &str) -> i64 {
    RANGES
        .iter()
        .find(|(r, _, _)| *r == id)
        .map(|(_, _, s)| *s)
        .unwrap_or(3600)
}

/// Query step for a range: about 120 points per chart.
pub fn range_step(id: &str) -> u64 {
    ((range_secs(id) / 120).max(15)) as u64
}

pub async fn list() -> Result<Vec<Dashboard>, ApiError> {
    fetch_json("/monitoring/dashboards").await
}
pub async fn get(id: &str) -> Result<Dashboard, ApiError> {
    fetch_json(&format!("/monitoring/dashboards/{}", id)).await
}
pub async fn create(req: &DashboardRequest) -> Result<Dashboard, ApiError> {
    post_json("/monitoring/dashboards", req).await
}
pub async fn update(id: &str, req: &DashboardRequest) -> Result<Dashboard, ApiError> {
    put_json(&format!("/monitoring/dashboards/{}", id), req).await
}
pub async fn delete(id: &str) -> Result<(), ApiError> {
    delete_json(&format!("/monitoring/dashboards/{}", id)).await
}
