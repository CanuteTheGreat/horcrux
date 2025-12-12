//! Thin Provisioning Support
//!
//! Provides thin provisioning capabilities for storage volumes:
//! - Over-commitment tracking
//! - Space monitoring and alerts
//! - Automatic expansion
//! - Deduplication awareness

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, debug};
use horcrux_common::Result;

use super::StorageType;

/// Thin provisioning configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinProvisionConfig {
    /// Enable thin provisioning for this pool
    pub enabled: bool,
    /// Maximum over-commitment ratio (e.g., 2.0 = 200%)
    pub max_overcommit_ratio: f64,
    /// Warning threshold for actual usage (0.0 - 1.0)
    pub warning_threshold: f64,
    /// Critical threshold for actual usage (0.0 - 1.0)
    pub critical_threshold: f64,
    /// Auto-expand when reaching threshold
    pub auto_expand: bool,
    /// Auto-expand increment in GB
    pub expand_increment_gb: u64,
}

impl Default for ThinProvisionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_overcommit_ratio: 2.0,
            warning_threshold: 0.80,
            critical_threshold: 0.95,
            auto_expand: false,
            expand_increment_gb: 100,
        }
    }
}

/// Thin volume information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinVolumeInfo {
    pub name: String,
    pub pool_id: String,
    /// Virtual (provisioned) size in bytes
    pub virtual_size: u64,
    /// Actual allocated size in bytes
    pub allocated_size: u64,
    /// Percentage of virtual size actually used
    pub utilization: f64,
    /// Is this a sparse file/volume
    pub is_sparse: bool,
    /// Last checked timestamp
    pub last_checked: i64,
}

/// Pool thin provisioning status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolThinStatus {
    pub pool_id: String,
    /// Physical capacity in bytes
    pub physical_capacity: u64,
    /// Physical used in bytes
    pub physical_used: u64,
    /// Total provisioned (virtual) in bytes
    pub total_provisioned: u64,
    /// Current over-commitment ratio
    pub overcommit_ratio: f64,
    /// Warning level reached
    pub warning: bool,
    /// Critical level reached
    pub critical: bool,
    /// Number of thin volumes
    pub volume_count: usize,
}

/// Thin provisioning alert
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinAlert {
    pub pool_id: String,
    pub alert_type: ThinAlertType,
    pub message: String,
    pub timestamp: i64,
    pub acknowledged: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ThinAlertType {
    /// Pool approaching capacity
    CapacityWarning,
    /// Pool at critical capacity
    CapacityCritical,
    /// Over-commitment limit reached
    OvercommitLimit,
    /// Auto-expand triggered
    AutoExpand,
    /// Auto-expand failed
    AutoExpandFailed,
}

/// Thin provisioning manager
pub struct ThinProvisionManager {
    configs: Arc<RwLock<HashMap<String, ThinProvisionConfig>>>,
    volumes: Arc<RwLock<HashMap<String, ThinVolumeInfo>>>,
    alerts: Arc<RwLock<Vec<ThinAlert>>>,
}

impl ThinProvisionManager {
    pub fn new() -> Self {
        Self {
            configs: Arc::new(RwLock::new(HashMap::new())),
            volumes: Arc::new(RwLock::new(HashMap::new())),
            alerts: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Check if storage type supports thin provisioning
    pub fn supports_thin_provisioning(storage_type: &StorageType) -> bool {
        matches!(
            storage_type,
            StorageType::Zfs |
            StorageType::Ceph |
            StorageType::Lvm |
            StorageType::BtrFs |
            StorageType::Directory // qcow2 files are thin
        )
    }

    /// Configure thin provisioning for a pool
    pub async fn configure_pool(&self, pool_id: &str, config: ThinProvisionConfig) -> Result<()> {
        let mut configs = self.configs.write().await;
        configs.insert(pool_id.to_string(), config.clone());

        info!(
            pool_id = pool_id,
            enabled = config.enabled,
            max_overcommit = config.max_overcommit_ratio,
            "Configured thin provisioning for pool"
        );

        Ok(())
    }

    /// Get thin provisioning config for a pool
    pub async fn get_config(&self, pool_id: &str) -> Option<ThinProvisionConfig> {
        self.configs.read().await.get(pool_id).cloned()
    }

    /// Register a thin volume
    pub async fn register_volume(&self, volume: ThinVolumeInfo) {
        let mut volumes = self.volumes.write().await;
        volumes.insert(volume.name.clone(), volume);
    }

    /// Update volume allocation info
    pub async fn update_volume_allocation(
        &self,
        name: &str,
        allocated_size: u64,
    ) -> Result<()> {
        let mut volumes = self.volumes.write().await;
        let volume = volumes.get_mut(name).ok_or_else(|| {
            horcrux_common::Error::System(format!("Volume {} not found", name))
        })?;

        volume.allocated_size = allocated_size;
        volume.utilization = if volume.virtual_size > 0 {
            (allocated_size as f64) / (volume.virtual_size as f64)
        } else {
            0.0
        };
        volume.last_checked = chrono::Utc::now().timestamp();

        Ok(())
    }

    /// Check if provisioning a new volume is allowed
    pub async fn check_provision_allowed(
        &self,
        pool_id: &str,
        physical_capacity: u64,
        total_provisioned: u64,
        new_volume_size: u64,
    ) -> Result<bool> {
        let configs = self.configs.read().await;
        let config = configs.get(pool_id);

        // If no config or not enabled, allow provisioning (no restrictions)
        let config = match config {
            Some(c) if c.enabled => c,
            _ => return Ok(true),
        };

        let new_total = total_provisioned + new_volume_size;
        let new_ratio = (new_total as f64) / (physical_capacity as f64);

        if new_ratio > config.max_overcommit_ratio {
            warn!(
                pool_id = pool_id,
                new_ratio = new_ratio,
                max_ratio = config.max_overcommit_ratio,
                "Over-commitment limit would be exceeded"
            );
            return Ok(false);
        }

        Ok(true)
    }

    /// Calculate pool thin provisioning status
    pub async fn get_pool_status(
        &self,
        pool_id: &str,
        physical_capacity: u64,
        physical_used: u64,
    ) -> PoolThinStatus {
        let volumes = self.volumes.read().await;
        let pool_volumes: Vec<_> = volumes.values()
            .filter(|v| v.pool_id == pool_id)
            .collect();

        let total_provisioned: u64 = pool_volumes.iter()
            .map(|v| v.virtual_size)
            .sum();

        let overcommit_ratio = if physical_capacity > 0 {
            (total_provisioned as f64) / (physical_capacity as f64)
        } else {
            0.0
        };

        let usage_ratio = if physical_capacity > 0 {
            (physical_used as f64) / (physical_capacity as f64)
        } else {
            0.0
        };

        let configs = self.configs.read().await;
        let config = configs.get(pool_id);

        let (warning, critical) = match config {
            Some(c) => (
                usage_ratio >= c.warning_threshold,
                usage_ratio >= c.critical_threshold,
            ),
            None => (false, false),
        };

        PoolThinStatus {
            pool_id: pool_id.to_string(),
            physical_capacity,
            physical_used,
            total_provisioned,
            overcommit_ratio,
            warning,
            critical,
            volume_count: pool_volumes.len(),
        }
    }

    /// Check pools and generate alerts
    pub async fn check_pools_and_alert(
        &self,
        pool_statuses: Vec<(String, u64, u64)>, // (pool_id, capacity, used)
    ) {
        for (pool_id, capacity, used) in pool_statuses {
            let status = self.get_pool_status(&pool_id, capacity, used).await;

            if status.critical {
                self.add_alert(ThinAlert {
                    pool_id: pool_id.clone(),
                    alert_type: ThinAlertType::CapacityCritical,
                    message: format!(
                        "Pool {} at critical capacity: {:.1}% used",
                        pool_id,
                        (used as f64 / capacity as f64) * 100.0
                    ),
                    timestamp: chrono::Utc::now().timestamp(),
                    acknowledged: false,
                }).await;
            } else if status.warning {
                self.add_alert(ThinAlert {
                    pool_id: pool_id.clone(),
                    alert_type: ThinAlertType::CapacityWarning,
                    message: format!(
                        "Pool {} approaching capacity: {:.1}% used",
                        pool_id,
                        (used as f64 / capacity as f64) * 100.0
                    ),
                    timestamp: chrono::Utc::now().timestamp(),
                    acknowledged: false,
                }).await;
            }

            // Check over-commitment
            let configs = self.configs.read().await;
            if let Some(config) = configs.get(&pool_id) {
                if status.overcommit_ratio > config.max_overcommit_ratio {
                    self.add_alert(ThinAlert {
                        pool_id: pool_id.clone(),
                        alert_type: ThinAlertType::OvercommitLimit,
                        message: format!(
                            "Pool {} over-commitment ratio ({:.2}x) exceeds limit ({:.2}x)",
                            pool_id, status.overcommit_ratio, config.max_overcommit_ratio
                        ),
                        timestamp: chrono::Utc::now().timestamp(),
                        acknowledged: false,
                    }).await;
                }
            }
        }
    }

    /// Add an alert
    async fn add_alert(&self, alert: ThinAlert) {
        let mut alerts = self.alerts.write().await;

        // Check for duplicate unacknowledged alerts
        let duplicate = alerts.iter().any(|a| {
            a.pool_id == alert.pool_id &&
            a.alert_type == alert.alert_type &&
            !a.acknowledged
        });

        if !duplicate {
            warn!(
                pool_id = %alert.pool_id,
                alert_type = ?alert.alert_type,
                "{}",
                alert.message
            );
            alerts.push(alert);
        }
    }

    /// Get unacknowledged alerts
    pub async fn get_alerts(&self, pool_id: Option<&str>) -> Vec<ThinAlert> {
        let alerts = self.alerts.read().await;
        alerts.iter()
            .filter(|a| {
                !a.acknowledged &&
                pool_id.map(|p| a.pool_id == p).unwrap_or(true)
            })
            .cloned()
            .collect()
    }

    /// Acknowledge an alert
    pub async fn acknowledge_alert(&self, pool_id: &str, alert_type: &ThinAlertType) {
        let mut alerts = self.alerts.write().await;
        for alert in alerts.iter_mut() {
            if alert.pool_id == pool_id && &alert.alert_type == alert_type {
                alert.acknowledged = true;
            }
        }
    }

    /// Get all volumes for a pool
    pub async fn get_pool_volumes(&self, pool_id: &str) -> Vec<ThinVolumeInfo> {
        let volumes = self.volumes.read().await;
        volumes.values()
            .filter(|v| v.pool_id == pool_id)
            .cloned()
            .collect()
    }

    /// Calculate sparse file actual usage
    pub async fn calculate_sparse_usage(path: &str) -> Result<(u64, u64)> {
        // Use `du` to get actual disk usage vs apparent size
        let output = tokio::process::Command::new("du")
            .arg("-b")
            .arg("--apparent-size")
            .arg(path)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to check file size: {}", e)))?;

        let apparent_str = String::from_utf8_lossy(&output.stdout);
        let apparent_size: u64 = apparent_str
            .split_whitespace()
            .next()
            .unwrap_or("0")
            .parse()
            .unwrap_or(0);

        let output = tokio::process::Command::new("du")
            .arg("-b")
            .arg(path)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to check file size: {}", e)))?;

        let actual_str = String::from_utf8_lossy(&output.stdout);
        let actual_size: u64 = actual_str
            .split_whitespace()
            .next()
            .unwrap_or("0")
            .parse()
            .unwrap_or(0);

        debug!(
            path = path,
            apparent = apparent_size,
            actual = actual_size,
            "Checked sparse file usage"
        );

        Ok((apparent_size, actual_size))
    }

    /// Cleanup old alerts (older than specified hours)
    pub async fn cleanup_old_alerts(&self, max_age_hours: u32) {
        let mut alerts = self.alerts.write().await;
        let cutoff = chrono::Utc::now().timestamp() - (max_age_hours as i64 * 3600);

        alerts.retain(|a| a.timestamp > cutoff || !a.acknowledged);
    }
}

impl Default for ThinProvisionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thin_provisioning_support() {
        assert!(ThinProvisionManager::supports_thin_provisioning(&StorageType::Zfs));
        assert!(ThinProvisionManager::supports_thin_provisioning(&StorageType::Ceph));
        assert!(ThinProvisionManager::supports_thin_provisioning(&StorageType::Lvm));
        assert!(ThinProvisionManager::supports_thin_provisioning(&StorageType::BtrFs));
        assert!(!ThinProvisionManager::supports_thin_provisioning(&StorageType::Nfs));
        assert!(!ThinProvisionManager::supports_thin_provisioning(&StorageType::Iscsi));
    }

    #[tokio::test]
    async fn test_provision_allowed() {
        let manager = ThinProvisionManager::new();

        // Configure pool with 2x overcommit
        manager.configure_pool("pool1", ThinProvisionConfig {
            enabled: true,
            max_overcommit_ratio: 2.0,
            ..Default::default()
        }).await.unwrap();

        // Physical capacity: 100GB
        // Already provisioned: 150GB
        // Trying to add: 40GB (would be 190GB, 1.9x - allowed)
        let allowed = manager.check_provision_allowed(
            "pool1",
            100 * 1024 * 1024 * 1024,
            150 * 1024 * 1024 * 1024,
            40 * 1024 * 1024 * 1024,
        ).await.unwrap();
        assert!(allowed);

        // Trying to add: 60GB (would be 210GB, 2.1x - not allowed)
        let allowed = manager.check_provision_allowed(
            "pool1",
            100 * 1024 * 1024 * 1024,
            150 * 1024 * 1024 * 1024,
            60 * 1024 * 1024 * 1024,
        ).await.unwrap();
        assert!(!allowed);
    }

    #[tokio::test]
    async fn test_pool_status() {
        let manager = ThinProvisionManager::new();

        // Configure pool
        manager.configure_pool("pool1", ThinProvisionConfig {
            enabled: true,
            warning_threshold: 0.80,
            critical_threshold: 0.95,
            ..Default::default()
        }).await.unwrap();

        // Register a volume
        manager.register_volume(ThinVolumeInfo {
            name: "vol1".to_string(),
            pool_id: "pool1".to_string(),
            virtual_size: 200 * 1024 * 1024 * 1024, // 200GB provisioned
            allocated_size: 50 * 1024 * 1024 * 1024, // 50GB actual
            utilization: 0.25,
            is_sparse: true,
            last_checked: 0,
        }).await;

        // Physical: 100GB capacity, 85GB used (85% - should be warning)
        let status = manager.get_pool_status(
            "pool1",
            100 * 1024 * 1024 * 1024,
            85 * 1024 * 1024 * 1024,
        ).await;

        assert!(status.warning);
        assert!(!status.critical);
        assert_eq!(status.volume_count, 1);
        assert!((status.overcommit_ratio - 2.0).abs() < 0.01); // ~2x overcommit
    }
}
