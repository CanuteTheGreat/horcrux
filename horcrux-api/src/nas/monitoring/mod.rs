//! NAS Monitoring module
//!
//! Provides health checks, metrics, and alerting for NAS services.

pub mod health;
pub mod metrics;

use horcrux_common::{Error, Result};
use crate::nas::services::NasService;
use crate::nas::storage::{NasPool, PoolHealth};
use serde::{Deserialize, Serialize};

/// NAS health status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NasHealth {
    /// Overall health status
    pub status: HealthStatus,
    /// Service statuses
    pub services: Vec<ServiceHealth>,
    /// Pool statuses
    pub pools: Vec<PoolHealthInfo>,
    /// Active alerts
    pub alerts: Vec<NasAlert>,
    /// Last check timestamp
    pub checked_at: i64,
}

/// Health status levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    /// Everything is healthy
    Healthy,
    /// Some warnings but functional
    Warning,
    /// Critical issues
    Critical,
    /// Unknown status
    Unknown,
}

/// Service health info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceHealth {
    /// Service name
    pub service: NasService,
    /// Health status
    pub status: HealthStatus,
    /// Whether the service is running
    pub running: bool,
    /// Response time in ms
    pub response_time_ms: Option<u32>,
    /// Error message if any
    pub error: Option<String>,
}

/// Pool health info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolHealthInfo {
    /// Pool name
    pub name: String,
    /// Pool health
    pub health: PoolHealth,
    /// Health status
    pub status: HealthStatus,
    /// Usage percentage
    pub usage_percent: f64,
    /// Error count
    pub errors: u32,
    /// Last scrub timestamp
    pub last_scrub: Option<i64>,
    /// Last scrub errors
    pub scrub_errors: u32,
}

/// NAS alert
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NasAlert {
    /// Alert ID
    pub id: String,
    /// Alert level
    pub level: AlertLevel,
    /// Alert source
    pub source: AlertSource,
    /// Alert message
    pub message: String,
    /// When the alert was triggered
    pub triggered_at: i64,
    /// Whether the alert has been acknowledged
    pub acknowledged: bool,
}

/// Alert severity level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertLevel {
    Info,
    Warning,
    Error,
    Critical,
}

/// Alert source
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertSource {
    /// Storage pool alert
    Pool { name: String },
    /// Service alert
    Service { name: String },
    /// Disk alert
    Disk { device: String },
    /// Quota alert
    Quota { user: String, share: String },
    /// Replication alert
    Replication { task: String },
    /// System alert
    System,
}

/// NAS metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NasMetrics {
    /// Total shares
    pub total_shares: u32,
    /// Active shares
    pub active_shares: u32,
    /// Total users
    pub total_users: u32,
    /// Active connections (all protocols)
    pub active_connections: u32,
    /// SMB connections
    pub smb_connections: u32,
    /// NFS clients
    pub nfs_clients: u32,
    /// AFP connections
    pub afp_connections: u32,
    /// iSCSI sessions
    pub iscsi_sessions: u32,
    /// Total storage bytes
    pub total_storage_bytes: u64,
    /// Used storage bytes
    pub used_storage_bytes: u64,
    /// Data transferred (read) bytes
    pub bytes_read: u64,
    /// Data transferred (write) bytes
    pub bytes_written: u64,
    /// Snapshot count
    pub snapshot_count: u32,
    /// Replication tasks
    pub replication_tasks: u32,
}

impl Default for NasMetrics {
    fn default() -> Self {
        Self {
            total_shares: 0,
            active_shares: 0,
            total_users: 0,
            active_connections: 0,
            smb_connections: 0,
            nfs_clients: 0,
            afp_connections: 0,
            iscsi_sessions: 0,
            total_storage_bytes: 0,
            used_storage_bytes: 0,
            bytes_read: 0,
            bytes_written: 0,
            snapshot_count: 0,
            replication_tasks: 0,
        }
    }
}

/// Get overall NAS health
pub async fn get_nas_health() -> Result<NasHealth> {
    let now = chrono::Utc::now().timestamp();

    // Check services
    let services = check_services().await?;

    // Check pools
    let pools = check_pools().await?;

    // Get active alerts
    let alerts = get_active_alerts().await?;

    // Determine overall status
    let status = determine_overall_status(&services, &pools, &alerts);

    Ok(NasHealth {
        status,
        services,
        pools,
        alerts,
        checked_at: now,
    })
}

/// Check all NAS services
async fn check_services() -> Result<Vec<ServiceHealth>> {
    let mut results = Vec::new();

    for service in NasService::all() {
        let status_result = crate::nas::services::get_service_status(&service).await;

        let (status, running, error) = match status_result {
            Ok(s) => {
                let health = if s.running {
                    HealthStatus::Healthy
                } else {
                    HealthStatus::Warning
                };
                (health, s.running, s.last_error)
            }
            Err(e) => (HealthStatus::Unknown, false, Some(e.to_string())),
        };

        results.push(ServiceHealth {
            service,
            status,
            running,
            response_time_ms: None,
            error,
        });
    }

    Ok(results)
}

/// Check all storage pools
async fn check_pools() -> Result<Vec<PoolHealthInfo>> {
    let mut results = Vec::new();

    // Get ZFS pools if available
    #[cfg(feature = "nas-zfs")]
    {
        if let Ok(pools) = crate::nas::storage::pools::list_zfs_pools().await {
            for pool in pools {
                let status = match pool.health {
                    PoolHealth::Online => HealthStatus::Healthy,
                    PoolHealth::Degraded | PoolHealth::Resilvering | PoolHealth::Scrubbing => {
                        HealthStatus::Warning
                    }
                    PoolHealth::Faulted | PoolHealth::Offline | PoolHealth::Unavailable => {
                        HealthStatus::Critical
                    }
                    PoolHealth::Unknown => HealthStatus::Unknown,
                };

                // Warn if usage is high
                let status = if pool.usage_percent() > 90.0 && status == HealthStatus::Healthy {
                    HealthStatus::Warning
                } else {
                    status
                };

                results.push(PoolHealthInfo {
                    name: pool.name.clone(),
                    health: pool.health,
                    status,
                    usage_percent: pool.usage_percent(),
                    errors: 0, // Would parse from zpool status
                    last_scrub: None,
                    scrub_errors: 0,
                });
            }
        }
    }

    Ok(results)
}

/// Get active alerts
async fn get_active_alerts() -> Result<Vec<NasAlert>> {
    // In a full implementation, this would query the database
    Ok(Vec::new())
}

/// Determine overall health status
fn determine_overall_status(
    services: &[ServiceHealth],
    pools: &[PoolHealthInfo],
    alerts: &[NasAlert],
) -> HealthStatus {
    // Check for critical alerts
    if alerts.iter().any(|a| a.level == AlertLevel::Critical) {
        return HealthStatus::Critical;
    }

    // Check pools
    if pools.iter().any(|p| p.status == HealthStatus::Critical) {
        return HealthStatus::Critical;
    }

    // Check services
    if services.iter().any(|s| s.status == HealthStatus::Critical) {
        return HealthStatus::Critical;
    }

    // Check for warnings
    if alerts.iter().any(|a| a.level == AlertLevel::Warning)
        || pools.iter().any(|p| p.status == HealthStatus::Warning)
        || services.iter().any(|s| s.status == HealthStatus::Warning)
    {
        return HealthStatus::Warning;
    }

    HealthStatus::Healthy
}

/// Collect NAS metrics
pub async fn collect_metrics() -> Result<NasMetrics> {
    let mut metrics = NasMetrics::default();

    // Get SMB connections
    #[cfg(feature = "smb")]
    {
        if let Ok(count) = get_smb_connection_count().await {
            metrics.smb_connections = count;
            metrics.active_connections += count;
        }
    }

    // Get NFS clients
    #[cfg(feature = "nfs-server")]
    {
        if let Ok(count) = get_nfs_client_count().await {
            metrics.nfs_clients = count;
            metrics.active_connections += count;
        }
    }

    // Get pool statistics
    #[cfg(feature = "nas-zfs")]
    {
        if let Ok(pools) = crate::nas::storage::pools::list_zfs_pools().await {
            for pool in pools {
                metrics.total_storage_bytes += pool.total_bytes;
                metrics.used_storage_bytes += pool.used_bytes;
            }
        }
    }

    Ok(metrics)
}

/// Get SMB connection count
#[cfg(feature = "smb")]
async fn get_smb_connection_count() -> Result<u32> {
    use tokio::process::Command;

    let output = Command::new("smbstatus")
        .args(["--shares", "--parseable"])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("smbstatus failed: {}", e)))?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let count = stdout.lines().count().saturating_sub(1) as u32;
        Ok(count)
    } else {
        Ok(0)
    }
}

/// Get NFS client count
#[cfg(feature = "nfs-server")]
async fn get_nfs_client_count() -> Result<u32> {
    let clients_dir = std::path::Path::new("/proc/fs/nfsd/clients");
    if clients_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(clients_dir) {
            return Ok(entries.count() as u32);
        }
    }
    Ok(0)
}

/// Monitoring manager for NAS services
pub struct MonitoringManager {
    /// Alert storage (in production, this would be database-backed)
    alerts: std::sync::Arc<tokio::sync::RwLock<Vec<NasAlert>>>,
    /// Alerting thresholds
    thresholds: AlertThresholds,
}

impl MonitoringManager {
    /// Create a new monitoring manager
    pub fn new() -> Self {
        Self {
            alerts: std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())),
            thresholds: AlertThresholds::default(),
        }
    }

    /// Set alerting thresholds
    pub fn set_thresholds(&mut self, thresholds: AlertThresholds) {
        self.thresholds = thresholds;
    }

    /// Create a new alert
    pub async fn create_alert(
        &self,
        level: AlertLevel,
        source: AlertSource,
        message: String,
    ) -> NasAlert {
        let alert = NasAlert {
            id: uuid::Uuid::new_v4().to_string(),
            level,
            source,
            message,
            triggered_at: chrono::Utc::now().timestamp(),
            acknowledged: false,
        };

        let mut alerts = self.alerts.write().await;
        alerts.push(alert.clone());

        alert
    }

    /// Acknowledge an alert
    pub async fn acknowledge_alert(&self, alert_id: &str) -> Result<()> {
        let mut alerts = self.alerts.write().await;
        if let Some(alert) = alerts.iter_mut().find(|a| a.id == alert_id) {
            alert.acknowledged = true;
            Ok(())
        } else {
            Err(Error::NotFound(format!("Alert '{}' not found", alert_id)))
        }
    }

    /// Delete an alert
    pub async fn delete_alert(&self, alert_id: &str) -> Result<()> {
        let mut alerts = self.alerts.write().await;
        let initial_len = alerts.len();
        alerts.retain(|a| a.id != alert_id);
        if alerts.len() < initial_len {
            Ok(())
        } else {
            Err(Error::NotFound(format!("Alert '{}' not found", alert_id)))
        }
    }

    /// Delete all acknowledged alerts
    pub async fn clear_acknowledged_alerts(&self) -> u32 {
        let mut alerts = self.alerts.write().await;
        let initial_len = alerts.len();
        alerts.retain(|a| !a.acknowledged);
        (initial_len - alerts.len()) as u32
    }

    /// List all alerts
    pub async fn list_alerts(&self, include_acknowledged: bool) -> Vec<NasAlert> {
        let alerts = self.alerts.read().await;
        if include_acknowledged {
            alerts.clone()
        } else {
            alerts.iter().filter(|a| !a.acknowledged).cloned().collect()
        }
    }

    /// Get alerts by level
    pub async fn get_alerts_by_level(&self, level: AlertLevel) -> Vec<NasAlert> {
        let alerts = self.alerts.read().await;
        alerts.iter().filter(|a| a.level == level).cloned().collect()
    }

    /// Check health and create alerts for issues
    pub async fn check_and_alert(&self) -> Result<NasHealth> {
        let health = get_nas_health().await?;

        // Check pools for high usage
        for pool in &health.pools {
            if pool.usage_percent > self.thresholds.pool_usage_critical as f64 {
                self.create_alert(
                    AlertLevel::Critical,
                    AlertSource::Pool { name: pool.name.clone() },
                    format!("Pool '{}' usage at {:.1}% (critical threshold: {}%)",
                        pool.name, pool.usage_percent, self.thresholds.pool_usage_critical),
                ).await;
            } else if pool.usage_percent > self.thresholds.pool_usage_warning as f64 {
                self.create_alert(
                    AlertLevel::Warning,
                    AlertSource::Pool { name: pool.name.clone() },
                    format!("Pool '{}' usage at {:.1}% (warning threshold: {}%)",
                        pool.name, pool.usage_percent, self.thresholds.pool_usage_warning),
                ).await;
            }

            if pool.status == HealthStatus::Critical {
                self.create_alert(
                    AlertLevel::Critical,
                    AlertSource::Pool { name: pool.name.clone() },
                    format!("Pool '{}' health is critical", pool.name),
                ).await;
            }
        }

        // Check services for failures
        for service in &health.services {
            if !service.running {
                self.create_alert(
                    AlertLevel::Warning,
                    AlertSource::Service { name: format!("{:?}", service.service) },
                    format!("Service {:?} is not running", service.service),
                ).await;
            }
        }

        Ok(health)
    }

    /// Check a specific service health with timing
    pub async fn check_service_health(&self, service: &NasService) -> ServiceHealth {
        use tokio::time::Instant;

        let start = Instant::now();
        let status_result = crate::nas::services::get_service_status(service).await;
        let response_time = start.elapsed().as_millis() as u32;

        let (status, running, error) = match status_result {
            Ok(s) => {
                let health = if s.running {
                    if response_time > self.thresholds.service_response_slow_ms {
                        HealthStatus::Warning
                    } else {
                        HealthStatus::Healthy
                    }
                } else {
                    HealthStatus::Warning
                };
                (health, s.running, s.last_error)
            }
            Err(e) => (HealthStatus::Unknown, false, Some(e.to_string())),
        };

        ServiceHealth {
            service: service.clone(),
            status,
            running,
            response_time_ms: Some(response_time),
            error,
        }
    }

    /// Get disk health status
    pub async fn check_disk_health(&self) -> Result<Vec<DiskHealth>> {
        use tokio::process::Command;

        let mut disks = Vec::new();

        // Try smartctl for disk health
        let output = Command::new("smartctl")
            .args(["--scan", "--json"])
            .output()
            .await;

        if let Ok(out) = output {
            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout) {
                    if let Some(devices) = json.get("devices").and_then(|v| v.as_array()) {
                        for device in devices {
                            if let Some(name) = device.get("name").and_then(|v| v.as_str()) {
                                let health = self.get_smart_health(name).await;
                                disks.push(health);
                            }
                        }
                    }
                }
            }
        }

        // Fallback: list block devices
        if disks.is_empty() {
            let output = Command::new("lsblk")
                .args(["-d", "-n", "-o", "NAME,SIZE,MODEL,TYPE"])
                .output()
                .await;

            if let Ok(out) = output {
                if out.status.success() {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    for line in stdout.lines() {
                        let parts: Vec<&str> = line.split_whitespace().collect();
                        if parts.len() >= 2 {
                            disks.push(DiskHealth {
                                device: format!("/dev/{}", parts[0]),
                                model: parts.get(2).map(|s| s.to_string()),
                                status: HealthStatus::Unknown,
                                temperature: None,
                                power_on_hours: None,
                                smart_passed: None,
                                errors: 0,
                            });
                        }
                    }
                }
            }
        }

        Ok(disks)
    }

    /// Get SMART health for a specific disk
    async fn get_smart_health(&self, device: &str) -> DiskHealth {
        use tokio::process::Command;

        let output = Command::new("smartctl")
            .args(["-a", "--json", device])
            .output()
            .await;

        let mut health = DiskHealth {
            device: device.to_string(),
            model: None,
            status: HealthStatus::Unknown,
            temperature: None,
            power_on_hours: None,
            smart_passed: None,
            errors: 0,
        };

        if let Ok(out) = output {
            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout) {
                    // Model
                    health.model = json
                        .get("model_name")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());

                    // SMART status
                    if let Some(smart_status) = json.get("smart_status") {
                        if let Some(passed) = smart_status.get("passed").and_then(|v| v.as_bool()) {
                            health.smart_passed = Some(passed);
                            health.status = if passed {
                                HealthStatus::Healthy
                            } else {
                                HealthStatus::Critical
                            };
                        }
                    }

                    // Temperature
                    if let Some(temp) = json.get("temperature") {
                        if let Some(current) = temp.get("current").and_then(|v| v.as_u64()) {
                            health.temperature = Some(current as u32);
                        }
                    }

                    // Power-on hours
                    if let Some(hours) = json.get("power_on_time") {
                        if let Some(h) = hours.get("hours").and_then(|v| v.as_u64()) {
                            health.power_on_hours = Some(h as u32);
                        }
                    }
                }
            }
        }

        health
    }

    /// Get metrics summary
    pub async fn get_metrics_summary(&self) -> Result<MetricsSummary> {
        let metrics = collect_metrics().await?;
        let alerts = self.list_alerts(false).await;

        Ok(MetricsSummary {
            total_connections: metrics.active_connections,
            storage_usage_percent: if metrics.total_storage_bytes > 0 {
                (metrics.used_storage_bytes as f64 / metrics.total_storage_bytes as f64) * 100.0
            } else {
                0.0
            },
            active_alerts: alerts.len() as u32,
            critical_alerts: alerts.iter().filter(|a| a.level == AlertLevel::Critical).count() as u32,
        })
    }
}

impl Default for MonitoringManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Alerting thresholds
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertThresholds {
    /// Pool usage warning percentage
    pub pool_usage_warning: u8,
    /// Pool usage critical percentage
    pub pool_usage_critical: u8,
    /// Service response time warning (ms)
    pub service_response_slow_ms: u32,
    /// Disk temperature warning (Celsius)
    pub disk_temp_warning: u32,
    /// Disk temperature critical (Celsius)
    pub disk_temp_critical: u32,
}

impl Default for AlertThresholds {
    fn default() -> Self {
        Self {
            pool_usage_warning: 80,
            pool_usage_critical: 95,
            service_response_slow_ms: 1000,
            disk_temp_warning: 45,
            disk_temp_critical: 55,
        }
    }
}

/// Disk health information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskHealth {
    /// Device path
    pub device: String,
    /// Model name
    pub model: Option<String>,
    /// Health status
    pub status: HealthStatus,
    /// Temperature in Celsius
    pub temperature: Option<u32>,
    /// Power-on hours
    pub power_on_hours: Option<u32>,
    /// SMART test passed
    pub smart_passed: Option<bool>,
    /// Error count
    pub errors: u32,
}

/// Metrics summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSummary {
    /// Total active connections
    pub total_connections: u32,
    /// Storage usage percentage
    pub storage_usage_percent: f64,
    /// Number of active alerts
    pub active_alerts: u32,
    /// Number of critical alerts
    pub critical_alerts: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_metrics() {
        let metrics = NasMetrics::default();
        assert_eq!(metrics.total_shares, 0);
        assert_eq!(metrics.active_connections, 0);
    }

    #[test]
    fn test_overall_status_healthy() {
        let status = determine_overall_status(&[], &[], &[]);
        assert_eq!(status, HealthStatus::Healthy);
    }

    #[test]
    fn test_overall_status_critical_alert() {
        let alerts = vec![NasAlert {
            id: "test".to_string(),
            level: AlertLevel::Critical,
            source: AlertSource::System,
            message: "Test".to_string(),
            triggered_at: 0,
            acknowledged: false,
        }];

        let status = determine_overall_status(&[], &[], &alerts);
        assert_eq!(status, HealthStatus::Critical);
    }
}
