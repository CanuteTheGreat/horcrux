//! External metric servers client (InfluxDB / Graphite); mirrors horcrux-api
//! `api::metric_servers` and is checked by tests/api_contract.rs.
use super::{delete_json, fetch_json, post_json, put_json, ApiError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SendStatus {
    pub last_success: Option<i64>,
    pub last_error: Option<String>,
    pub last_error_at: Option<i64>,
    pub points_sent: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricServer {
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// `influxdb` or `graphite`
    #[serde(rename = "type")]
    pub kind: String,
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub protocol: String,
    pub api_version: String,
    pub organization: String,
    pub bucket: String,
    pub token: String,
    pub database: String,
    pub username: String,
    pub password: String,
    pub verify_tls: bool,
    pub prefix: String,
    pub interval_secs: u64,
    #[serde(default, skip_serializing)]
    pub status: Option<SendStatus>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TestResult {
    pub points: usize,
}

const BASE: &str = "/observability/metric-servers";

pub async fn list() -> Result<Vec<MetricServer>, ApiError> {
    fetch_json(BASE).await
}

pub async fn create(s: &MetricServer) -> Result<MetricServer, ApiError> {
    post_json(BASE, s).await
}

pub async fn update(s: &MetricServer) -> Result<MetricServer, ApiError> {
    put_json(&format!("{BASE}/{}", urlencoding::encode(&s.id)), s).await
}

pub async fn delete(id: &str) -> Result<(), ApiError> {
    delete_json(&format!("{BASE}/{}", urlencoding::encode(id))).await
}

pub async fn test(id: &str) -> Result<TestResult, ApiError> {
    post_json(&format!("{BASE}/{}/test", urlencoding::encode(id)), &()).await
}
