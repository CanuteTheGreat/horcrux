///! Backup retention policy management
///! Implements Proxmox-style retention (keep-hourly, keep-daily, etc.)

use super::{Backup, RetentionPolicy};
use chrono::{Datelike, Timelike};
use std::collections::HashMap;

/// Retention manager
pub struct RetentionManager {}

impl RetentionManager {
    pub fn new() -> Self {
        Self {}
    }

    /// Apply retention policy and return backups to delete
    pub async fn apply_policy(&self, backups: &[&Backup], policy: &RetentionPolicy) -> Vec<String> {
        let mut to_delete = Vec::new();

        // Sort backups by timestamp (newest first)
        let mut sorted: Vec<&Backup> = backups.iter().copied().collect();
        sorted.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        // Group backups by time period
        let mut hourly = Vec::new();
        let mut daily = Vec::new();
        let mut weekly = Vec::new();
        let mut monthly = Vec::new();
        let mut yearly = Vec::new();

        let mut seen_hours = HashMap::new();
        let mut seen_days = HashMap::new();
        let mut seen_weeks = HashMap::new();
        let mut seen_months = HashMap::new();
        let mut seen_years = HashMap::new();

        for backup in &sorted {
            let dt = chrono::DateTime::from_timestamp(backup.timestamp, 0).unwrap();

            // Hourly
            let hour_key = (dt.year(), dt.month(), dt.day(), dt.hour());
            if !seen_hours.contains_key(&hour_key) {
                seen_hours.insert(hour_key, backup.id.clone());
                hourly.push(backup.id.clone());
            }

            // Daily
            let day_key = (dt.year(), dt.month(), dt.day());
            if !seen_days.contains_key(&day_key) {
                seen_days.insert(day_key, backup.id.clone());
                daily.push(backup.id.clone());
            }

            // Weekly
            let week_key = (dt.year(), dt.iso_week().week());
            if !seen_weeks.contains_key(&week_key) {
                seen_weeks.insert(week_key, backup.id.clone());
                weekly.push(backup.id.clone());
            }

            // Monthly
            let month_key = (dt.year(), dt.month());
            if !seen_months.contains_key(&month_key) {
                seen_months.insert(month_key, backup.id.clone());
                monthly.push(backup.id.clone());
            }

            // Yearly
            let year_key = dt.year();
            if !seen_years.contains_key(&year_key) {
                seen_years.insert(year_key, backup.id.clone());
                yearly.push(backup.id.clone());
            }
        }

        // Determine which backups to keep
        let mut keep = std::collections::HashSet::new();

        if let Some(n) = policy.keep_hourly {
            for id in hourly.iter().take(n as usize) {
                keep.insert(id.clone());
            }
        }

        if let Some(n) = policy.keep_daily {
            for id in daily.iter().take(n as usize) {
                keep.insert(id.clone());
            }
        }

        if let Some(n) = policy.keep_weekly {
            for id in weekly.iter().take(n as usize) {
                keep.insert(id.clone());
            }
        }

        if let Some(n) = policy.keep_monthly {
            for id in monthly.iter().take(n as usize) {
                keep.insert(id.clone());
            }
        }

        if let Some(n) = policy.keep_yearly {
            for id in yearly.iter().take(n as usize) {
                keep.insert(id.clone());
            }
        }

        // Mark backups not in keep set for deletion
        for backup in sorted {
            if !keep.contains(&backup.id) {
                to_delete.push(backup.id.clone());
            }
        }

        tracing::info!(
            "Retention policy applied: {} backups to keep, {} to delete",
            keep.len(),
            to_delete.len()
        );

        to_delete
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::{BackupMode, Compression, TargetType};
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_retention_policy() {
        let manager = RetentionManager::new();

        // Create test backups spanning multiple days
        let backups: Vec<Backup> = (0..10)
            .map(|i| Backup {
                id: format!("backup-{}", i),
                target_type: TargetType::Vm,
                target_id: "100".to_string(),
                target_name: "test".to_string(),
                timestamp: 1704067200 + (i * 86400), // One per day
                size: 1000000,
                mode: BackupMode::Snapshot,
                compression: Compression::Zstd,
                path: PathBuf::from("/tmp/backup"),
                config_included: true,
                notes: None,
            })
            .collect();

        let backup_refs: Vec<&Backup> = backups.iter().collect();

        let policy = RetentionPolicy {
            keep_hourly: None,
            keep_daily: Some(7),
            keep_weekly: None,
            keep_monthly: None,
            keep_yearly: None,
        };

        let to_delete = manager.apply_policy(&backup_refs, &policy).await;

        // Should keep 7 daily backups, delete 3
        assert_eq!(to_delete.len(), 3);
    }
}
