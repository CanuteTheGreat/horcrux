///! Snapshot Disk Quota Management
///!
///! Provides quota enforcement for VM snapshots to prevent excessive disk usage:
///! - Per-VM snapshot quotas
///! - Per-storage-pool quotas
///! - Global snapshot quotas
///! - Automatic quota enforcement on snapshot creation
///! - Quota warning and violation alerts
///! - Automatic cleanup of oldest snapshots when quota exceeded

use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

/// Quota configuration for snapshot storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotQuota {
    pub id: String,
    pub name: String,
    pub quota_type: QuotaType,
    pub target_id: String, // VM ID, pool name, or "global"
    pub max_size_bytes: u64,
    pub max_count: Option<u32>,
    pub warning_threshold_percent: u8, // Alert when usage exceeds this percentage
    pub auto_cleanup_enabled: bool,
    pub cleanup_policy: CleanupPolicy,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Type of quota
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum QuotaType {
    PerVm,        // Quota applies to single VM
    PerPool,      // Quota applies to storage pool
    Global,       // Global quota across all snapshots
}

/// Cleanup policy when quota is exceeded
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CleanupPolicy {
    OldestFirst,        // Delete oldest snapshots first
    LargestFirst,       // Delete largest snapshots first
    LeastUsedFirst,     // Delete least accessed snapshots first
    Manual,             // Require manual intervention
}

/// Current quota usage statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaUsage {
    pub quota_id: String,
    pub current_size_bytes: u64,
    pub current_count: u32,
    pub max_size_bytes: u64,
    pub max_count: Option<u32>,
    pub usage_percent: f32,
    pub is_warning: bool,
    pub is_exceeded: bool,
    pub snapshots: Vec<SnapshotUsageInfo>,
}

/// Information about individual snapshot usage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotUsageInfo {
    pub snapshot_id: String,
    pub vm_id: String,
    pub name: String,
    pub size_bytes: u64,
    pub created_at: i64,
    pub last_accessed_at: Option<i64>,
}

/// Quota check result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaCheckResult {
    pub can_create: bool,
    pub quota_id: Option<String>,
    pub available_bytes: u64,
    pub required_bytes: u64,
    pub reason: Option<String>,
    pub suggested_cleanup: Vec<String>, // List of snapshot IDs to delete
}

/// Snapshot quota manager
pub struct SnapshotQuotaManager {
    quotas: Arc<RwLock<HashMap<String, SnapshotQuota>>>,
    usage_cache: Arc<RwLock<HashMap<String, QuotaUsage>>>,
}

impl SnapshotQuotaManager {
    pub fn new() -> Self {
        Self {
            quotas: Arc::new(RwLock::new(HashMap::new())),
            usage_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a new snapshot quota
    pub async fn create_quota(
        &self,
        name: String,
        quota_type: QuotaType,
        target_id: String,
        max_size_bytes: u64,
        max_count: Option<u32>,
        warning_threshold_percent: u8,
        auto_cleanup_enabled: bool,
        cleanup_policy: CleanupPolicy,
    ) -> Result<SnapshotQuota> {
        let quota = SnapshotQuota {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            quota_type,
            target_id,
            max_size_bytes,
            max_count,
            warning_threshold_percent,
            auto_cleanup_enabled,
            cleanup_policy,
            created_at: chrono::Utc::now().timestamp(),
            updated_at: chrono::Utc::now().timestamp(),
        };

        info!(
            "Creating snapshot quota '{}' for {}: {} bytes, {} snapshots",
            quota.name,
            quota.target_id,
            quota.max_size_bytes,
            quota.max_count.unwrap_or(0)
        );

        let mut quotas = self.quotas.write().await;
        quotas.insert(quota.id.clone(), quota.clone());

        Ok(quota)
    }

    /// Get quota by ID
    pub async fn get_quota(&self, quota_id: &str) -> Option<SnapshotQuota> {
        self.quotas.read().await.get(quota_id).cloned()
    }

    /// List all quotas
    pub async fn list_quotas(&self) -> Vec<SnapshotQuota> {
        self.quotas.read().await.values().cloned().collect()
    }

    /// Update quota configuration
    pub async fn update_quota(
        &self,
        quota_id: &str,
        max_size_bytes: Option<u64>,
        max_count: Option<Option<u32>>,
        warning_threshold_percent: Option<u8>,
        auto_cleanup_enabled: Option<bool>,
        cleanup_policy: Option<CleanupPolicy>,
    ) -> Result<SnapshotQuota> {
        let mut quotas = self.quotas.write().await;
        let quota = quotas
            .get_mut(quota_id)
            .ok_or_else(|| horcrux_common::Error::System("Quota not found".to_string()))?;

        if let Some(size) = max_size_bytes {
            quota.max_size_bytes = size;
        }
        if let Some(count) = max_count {
            quota.max_count = count;
        }
        if let Some(threshold) = warning_threshold_percent {
            quota.warning_threshold_percent = threshold;
        }
        if let Some(auto_cleanup) = auto_cleanup_enabled {
            quota.auto_cleanup_enabled = auto_cleanup;
        }
        if let Some(policy) = cleanup_policy {
            quota.cleanup_policy = policy;
        }

        quota.updated_at = chrono::Utc::now().timestamp();

        info!("Updated quota: {}", quota_id);
        Ok(quota.clone())
    }

    /// Delete a quota
    pub async fn delete_quota(&self, quota_id: &str) -> Result<()> {
        let mut quotas = self.quotas.write().await;
        quotas
            .remove(quota_id)
            .ok_or_else(|| horcrux_common::Error::System("Quota not found".to_string()))?;

        // Clean up usage cache
        let mut cache = self.usage_cache.write().await;
        cache.remove(quota_id);

        info!("Deleted quota: {}", quota_id);
        Ok(())
    }

    /// Check if creating a snapshot would violate quota
    pub async fn check_quota(
        &self,
        vm_id: &str,
        pool_id: Option<&str>,
        estimated_size_bytes: u64,
    ) -> Result<QuotaCheckResult> {
        // Check VM-specific quota
        if let Some(vm_quota) = self.find_quota_for_vm(vm_id).await {
            let usage = self.calculate_usage(&vm_quota.id, Some(vm_id), None).await?;

            if let Some(result) = self.evaluate_quota(&vm_quota, &usage, estimated_size_bytes).await {
                return Ok(result);
            }
        }

        // Check pool-specific quota
        if let Some(pool) = pool_id {
            if let Some(pool_quota) = self.find_quota_for_pool(pool).await {
                let usage = self.calculate_usage(&pool_quota.id, None, Some(pool)).await?;

                if let Some(result) = self.evaluate_quota(&pool_quota, &usage, estimated_size_bytes).await {
                    return Ok(result);
                }
            }
        }

        // Check global quota
        if let Some(global_quota) = self.find_global_quota().await {
            let usage = self.calculate_usage(&global_quota.id, None, None).await?;

            if let Some(result) = self.evaluate_quota(&global_quota, &usage, estimated_size_bytes).await {
                return Ok(result);
            }
        }

        // No quota restrictions or all checks passed
        Ok(QuotaCheckResult {
            can_create: true,
            quota_id: None,
            available_bytes: u64::MAX,
            required_bytes: estimated_size_bytes,
            reason: None,
            suggested_cleanup: vec![],
        })
    }

    /// Evaluate a specific quota against current usage
    async fn evaluate_quota(
        &self,
        quota: &SnapshotQuota,
        usage: &QuotaUsage,
        estimated_size_bytes: u64,
    ) -> Option<QuotaCheckResult> {
        let new_size = usage.current_size_bytes + estimated_size_bytes;
        let new_count = usage.current_count + 1;

        // Check size quota
        let size_exceeded = new_size > quota.max_size_bytes;

        // Check count quota
        let count_exceeded = if let Some(max_count) = quota.max_count {
            new_count > max_count
        } else {
            false
        };

        if size_exceeded || count_exceeded {
            let reason = if size_exceeded && count_exceeded {
                format!(
                    "Both size quota ({} bytes) and count quota ({}) would be exceeded",
                    quota.max_size_bytes,
                    quota.max_count.unwrap_or(0)
                )
            } else if size_exceeded {
                format!(
                    "Size quota would be exceeded: {} / {} bytes",
                    new_size, quota.max_size_bytes
                )
            } else {
                format!(
                    "Count quota would be exceeded: {} / {} snapshots",
                    new_count,
                    quota.max_count.unwrap_or(0)
                )
            };

            // Calculate suggested cleanup if auto-cleanup is enabled
            let suggested_cleanup = if quota.auto_cleanup_enabled {
                self.calculate_cleanup_suggestions(quota, usage, estimated_size_bytes).await
            } else {
                vec![]
            };

            Some(QuotaCheckResult {
                can_create: quota.auto_cleanup_enabled && !suggested_cleanup.is_empty(),
                quota_id: Some(quota.id.clone()),
                available_bytes: quota.max_size_bytes.saturating_sub(usage.current_size_bytes),
                required_bytes: estimated_size_bytes,
                reason: Some(reason),
                suggested_cleanup,
            })
        } else {
            // Check warning threshold
            let usage_percent = (new_size as f64 / quota.max_size_bytes as f64 * 100.0) as f32;
            if usage_percent >= quota.warning_threshold_percent as f32 {
                warn!(
                    "Quota '{}' approaching limit: {:.1}% used",
                    quota.name, usage_percent
                );
            }

            None // Quota check passed
        }
    }

    /// Calculate which snapshots to cleanup to make space
    async fn calculate_cleanup_suggestions(
        &self,
        quota: &SnapshotQuota,
        usage: &QuotaUsage,
        required_space: u64,
    ) -> Vec<String> {
        let mut snapshots = usage.snapshots.clone();
        let mut cleanup = Vec::new();
        let mut freed_space = 0u64;

        let space_to_free = (usage.current_size_bytes + required_space)
            .saturating_sub(quota.max_size_bytes);

        // Sort snapshots based on cleanup policy
        match quota.cleanup_policy {
            CleanupPolicy::OldestFirst => {
                snapshots.sort_by_key(|s| s.created_at);
            }
            CleanupPolicy::LargestFirst => {
                snapshots.sort_by_key(|s| std::cmp::Reverse(s.size_bytes));
            }
            CleanupPolicy::LeastUsedFirst => {
                snapshots.sort_by_key(|s| s.last_accessed_at.unwrap_or(0));
            }
            CleanupPolicy::Manual => {
                return vec![]; // No automatic cleanup
            }
        }

        // Select snapshots to delete
        for snapshot in snapshots {
            cleanup.push(snapshot.snapshot_id.clone());
            freed_space += snapshot.size_bytes;

            if freed_space >= space_to_free {
                break;
            }
        }

        info!(
            "Cleanup suggestions for quota '{}': {} snapshots to free {} bytes",
            quota.name,
            cleanup.len(),
            freed_space
        );

        cleanup
    }

    /// Calculate current quota usage
    async fn calculate_usage(
        &self,
        quota_id: &str,
        vm_id: Option<&str>,
        pool_id: Option<&str>,
    ) -> Result<QuotaUsage> {
        // In a real implementation, this would query the snapshot manager
        // and storage backend to get actual usage data

        // For now, return cached or empty usage
        let cache = self.usage_cache.read().await;
        if let Some(usage) = cache.get(quota_id) {
            return Ok(usage.clone());
        }

        let quota = self.get_quota(quota_id).await
            .ok_or_else(|| horcrux_common::Error::System("Quota not found".to_string()))?;

        Ok(QuotaUsage {
            quota_id: quota_id.to_string(),
            current_size_bytes: 0,
            current_count: 0,
            max_size_bytes: quota.max_size_bytes,
            max_count: quota.max_count,
            usage_percent: 0.0,
            is_warning: false,
            is_exceeded: false,
            snapshots: vec![],
        })
    }

    /// Update usage cache with actual snapshot data
    pub async fn update_usage_cache(
        &self,
        quota_id: &str,
        snapshots: Vec<SnapshotUsageInfo>,
    ) -> Result<QuotaUsage> {
        let quota = self.get_quota(quota_id).await
            .ok_or_else(|| horcrux_common::Error::System("Quota not found".to_string()))?;

        let current_size_bytes: u64 = snapshots.iter().map(|s| s.size_bytes).sum();
        let current_count = snapshots.len() as u32;

        let usage_percent = if quota.max_size_bytes > 0 {
            (current_size_bytes as f64 / quota.max_size_bytes as f64 * 100.0) as f32
        } else {
            0.0
        };

        let is_warning = usage_percent >= quota.warning_threshold_percent as f32;
        let is_exceeded = current_size_bytes > quota.max_size_bytes
            || quota.max_count.map_or(false, |max| current_count > max);

        let usage = QuotaUsage {
            quota_id: quota_id.to_string(),
            current_size_bytes,
            current_count,
            max_size_bytes: quota.max_size_bytes,
            max_count: quota.max_count,
            usage_percent,
            is_warning,
            is_exceeded,
            snapshots,
        };

        let mut cache = self.usage_cache.write().await;
        cache.insert(quota_id.to_string(), usage.clone());

        if is_exceeded {
            error!(
                "Quota '{}' exceeded: {:.1}% used ({} / {} bytes)",
                quota.name, usage_percent, current_size_bytes, quota.max_size_bytes
            );
        } else if is_warning {
            warn!(
                "Quota '{}' warning: {:.1}% used ({} / {} bytes)",
                quota.name, usage_percent, current_size_bytes, quota.max_size_bytes
            );
        }

        Ok(usage)
    }

    /// Get quota usage statistics
    pub async fn get_usage(&self, quota_id: &str) -> Result<QuotaUsage> {
        let cache = self.usage_cache.read().await;
        cache
            .get(quota_id)
            .cloned()
            .ok_or_else(|| horcrux_common::Error::System("Usage data not available".to_string()))
    }

    /// Find quota for a specific VM
    async fn find_quota_for_vm(&self, vm_id: &str) -> Option<SnapshotQuota> {
        let quotas = self.quotas.read().await;
        quotas
            .values()
            .find(|q| q.quota_type == QuotaType::PerVm && q.target_id == vm_id)
            .cloned()
    }

    /// Find quota for a storage pool
    async fn find_quota_for_pool(&self, pool_id: &str) -> Option<SnapshotQuota> {
        let quotas = self.quotas.read().await;
        quotas
            .values()
            .find(|q| q.quota_type == QuotaType::PerPool && q.target_id == pool_id)
            .cloned()
    }

    /// Find global quota
    async fn find_global_quota(&self) -> Option<SnapshotQuota> {
        let quotas = self.quotas.read().await;
        quotas
            .values()
            .find(|q| q.quota_type == QuotaType::Global)
            .cloned()
    }

    /// Enforce quota by deleting suggested snapshots
    pub async fn enforce_quota(
        &self,
        quota_id: &str,
        snapshot_ids_to_delete: Vec<String>,
    ) -> Result<u64> {
        let mut freed_bytes = 0u64;

        info!(
            "Enforcing quota '{}': deleting {} snapshots",
            quota_id,
            snapshot_ids_to_delete.len()
        );

        // In real implementation, this would call snapshot manager to delete
        // For now, just update the cache
        let mut cache = self.usage_cache.write().await;
        if let Some(usage) = cache.get_mut(quota_id) {
            usage.snapshots.retain(|s| {
                if snapshot_ids_to_delete.contains(&s.snapshot_id) {
                    freed_bytes += s.size_bytes;
                    false
                } else {
                    true
                }
            });

            usage.current_size_bytes = usage.current_size_bytes.saturating_sub(freed_bytes);
            usage.current_count = usage.snapshots.len() as u32;
            usage.usage_percent = if usage.max_size_bytes > 0 {
                (usage.current_size_bytes as f64 / usage.max_size_bytes as f64 * 100.0) as f32
            } else {
                0.0
            };
            usage.is_exceeded = usage.current_size_bytes > usage.max_size_bytes;
        }

        info!("Quota enforcement freed {} bytes", freed_bytes);
        Ok(freed_bytes)
    }

    /// Get quota statistics summary
    pub async fn get_quota_summary(&self) -> QuotaSummary {
        let quotas = self.quotas.read().await;
        let cache = self.usage_cache.read().await;

        let total_quotas = quotas.len();
        let exceeded_quotas = cache.values().filter(|u| u.is_exceeded).count();
        let warning_quotas = cache.values().filter(|u| u.is_warning && !u.is_exceeded).count();

        let total_size_bytes: u64 = cache.values().map(|u| u.current_size_bytes).sum();
        let total_max_bytes: u64 = quotas.values().map(|q| q.max_size_bytes).sum();

        QuotaSummary {
            total_quotas,
            exceeded_quotas,
            warning_quotas,
            healthy_quotas: total_quotas - exceeded_quotas - warning_quotas,
            total_size_bytes,
            total_max_bytes,
            overall_usage_percent: if total_max_bytes > 0 {
                (total_size_bytes as f64 / total_max_bytes as f64 * 100.0) as f32
            } else {
                0.0
            },
        }
    }
}

impl Clone for SnapshotQuotaManager {
    fn clone(&self) -> Self {
        Self {
            quotas: Arc::clone(&self.quotas),
            usage_cache: Arc::clone(&self.usage_cache),
        }
    }
}

/// Summary of all quota statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaSummary {
    pub total_quotas: usize,
    pub exceeded_quotas: usize,
    pub warning_quotas: usize,
    pub healthy_quotas: usize,
    pub total_size_bytes: u64,
    pub total_max_bytes: u64,
    pub overall_usage_percent: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_quota() {
        let manager = SnapshotQuotaManager::new();

        let quota = manager
            .create_quota(
                "Test VM Quota".to_string(),
                QuotaType::PerVm,
                "vm-100".to_string(),
                10 * 1024 * 1024 * 1024, // 10 GB
                Some(50),
                80,
                true,
                CleanupPolicy::OldestFirst,
            )
            .await
            .unwrap();

        assert_eq!(quota.name, "Test VM Quota");
        assert_eq!(quota.quota_type, QuotaType::PerVm);
        assert_eq!(quota.target_id, "vm-100");
        assert_eq!(quota.max_size_bytes, 10 * 1024 * 1024 * 1024);
        assert_eq!(quota.max_count, Some(50));
        assert_eq!(quota.warning_threshold_percent, 80);
        assert!(quota.auto_cleanup_enabled);
        assert_eq!(quota.cleanup_policy, CleanupPolicy::OldestFirst);
    }

    #[tokio::test]
    async fn test_list_quotas() {
        let manager = SnapshotQuotaManager::new();

        let _ = manager
            .create_quota(
                "Quota 1".to_string(),
                QuotaType::PerVm,
                "vm-100".to_string(),
                10_000_000_000,
                None,
                80,
                false,
                CleanupPolicy::Manual,
            )
            .await;

        let _ = manager
            .create_quota(
                "Quota 2".to_string(),
                QuotaType::PerPool,
                "pool-1".to_string(),
                50_000_000_000,
                Some(100),
                75,
                true,
                CleanupPolicy::LargestFirst,
            )
            .await;

        let quotas = manager.list_quotas().await;
        assert_eq!(quotas.len(), 2);
    }

    #[tokio::test]
    async fn test_update_quota() {
        let manager = SnapshotQuotaManager::new();

        let quota = manager
            .create_quota(
                "Test Quota".to_string(),
                QuotaType::Global,
                "global".to_string(),
                100_000_000_000,
                Some(1000),
                80,
                false,
                CleanupPolicy::Manual,
            )
            .await
            .unwrap();

        let updated = manager
            .update_quota(
                &quota.id,
                Some(200_000_000_000),
                Some(Some(2000)),
                Some(90),
                Some(true),
                Some(CleanupPolicy::OldestFirst),
            )
            .await
            .unwrap();

        assert_eq!(updated.max_size_bytes, 200_000_000_000);
        assert_eq!(updated.max_count, Some(2000));
        assert_eq!(updated.warning_threshold_percent, 90);
        assert!(updated.auto_cleanup_enabled);
        assert_eq!(updated.cleanup_policy, CleanupPolicy::OldestFirst);
    }

    #[tokio::test]
    async fn test_delete_quota() {
        let manager = SnapshotQuotaManager::new();

        let quota = manager
            .create_quota(
                "Delete Test".to_string(),
                QuotaType::PerVm,
                "vm-200".to_string(),
                10_000_000_000,
                None,
                80,
                false,
                CleanupPolicy::Manual,
            )
            .await
            .unwrap();

        let result = manager.delete_quota(&quota.id).await;
        assert!(result.is_ok());

        let retrieved = manager.get_quota(&quota.id).await;
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_check_quota_passes() {
        let manager = SnapshotQuotaManager::new();

        let quota = manager
            .create_quota(
                "Test Quota".to_string(),
                QuotaType::PerVm,
                "vm-100".to_string(),
                10 * 1024 * 1024 * 1024, // 10 GB
                Some(50),
                80,
                false,
                CleanupPolicy::Manual,
            )
            .await
            .unwrap();

        // Initialize empty usage
        let _ = manager.update_usage_cache(&quota.id, vec![]).await;

        let result = manager
            .check_quota("vm-100", None, 1024 * 1024 * 1024) // 1 GB
            .await
            .unwrap();

        assert!(result.can_create);
        assert!(result.reason.is_none());
    }

    #[tokio::test]
    async fn test_check_quota_exceeded() {
        let manager = SnapshotQuotaManager::new();

        let quota = manager
            .create_quota(
                "Small Quota".to_string(),
                QuotaType::PerVm,
                "vm-100".to_string(),
                5 * 1024 * 1024 * 1024, // 5 GB
                Some(10),
                80,
                false,
                CleanupPolicy::Manual,
            )
            .await
            .unwrap();

        // Simulate existing snapshots using 4 GB
        let snapshots = vec![
            SnapshotUsageInfo {
                snapshot_id: "snap-1".to_string(),
                vm_id: "vm-100".to_string(),
                name: "snapshot-1".to_string(),
                size_bytes: 4 * 1024 * 1024 * 1024,
                created_at: 1000,
                last_accessed_at: Some(2000),
            },
        ];

        let _ = manager.update_usage_cache(&quota.id, snapshots).await;

        // Try to create 2 GB snapshot (would exceed 5 GB limit)
        let result = manager
            .check_quota("vm-100", None, 2 * 1024 * 1024 * 1024)
            .await
            .unwrap();

        assert!(!result.can_create);
        assert!(result.reason.is_some());
    }

    #[tokio::test]
    async fn test_cleanup_suggestions() {
        let manager = SnapshotQuotaManager::new();

        let quota = manager
            .create_quota(
                "Auto Cleanup Quota".to_string(),
                QuotaType::PerVm,
                "vm-100".to_string(),
                5 * 1024 * 1024 * 1024, // 5 GB
                None,
                80,
                true,
                CleanupPolicy::OldestFirst,
            )
            .await
            .unwrap();

        // Create 3 snapshots totaling 4.5 GB
        let snapshots = vec![
            SnapshotUsageInfo {
                snapshot_id: "snap-1".to_string(),
                vm_id: "vm-100".to_string(),
                name: "oldest".to_string(),
                size_bytes: 2 * 1024 * 1024 * 1024,
                created_at: 1000,
                last_accessed_at: Some(1500),
            },
            SnapshotUsageInfo {
                snapshot_id: "snap-2".to_string(),
                vm_id: "vm-100".to_string(),
                name: "middle".to_string(),
                size_bytes: 1 * 1024 * 1024 * 1024,
                created_at: 2000,
                last_accessed_at: Some(2500),
            },
            SnapshotUsageInfo {
                snapshot_id: "snap-3".to_string(),
                vm_id: "vm-100".to_string(),
                name: "newest".to_string(),
                size_bytes: 1536 * 1024 * 1024,
                created_at: 3000,
                last_accessed_at: Some(3500),
            },
        ];

        let _ = manager.update_usage_cache(&quota.id, snapshots).await;

        // Try to add 2 GB snapshot (would need cleanup)
        let result = manager
            .check_quota("vm-100", None, 2 * 1024 * 1024 * 1024)
            .await
            .unwrap();

        assert!(result.can_create); // Can create because auto-cleanup is enabled
        assert!(!result.suggested_cleanup.is_empty());
        // Should suggest deleting oldest snapshot first
        assert_eq!(result.suggested_cleanup[0], "snap-1");
    }

    #[tokio::test]
    async fn test_quota_summary() {
        let manager = SnapshotQuotaManager::new();

        let _ = manager
            .create_quota(
                "Quota 1".to_string(),
                QuotaType::PerVm,
                "vm-100".to_string(),
                10_000_000_000,
                None,
                80,
                false,
                CleanupPolicy::Manual,
            )
            .await;

        let _ = manager
            .create_quota(
                "Quota 2".to_string(),
                QuotaType::Global,
                "global".to_string(),
                100_000_000_000,
                None,
                80,
                false,
                CleanupPolicy::Manual,
            )
            .await;

        let summary = manager.get_quota_summary().await;

        assert_eq!(summary.total_quotas, 2);
        assert_eq!(summary.exceeded_quotas, 0);
        assert_eq!(summary.warning_quotas, 0);
        assert_eq!(summary.healthy_quotas, 2);
    }
}
