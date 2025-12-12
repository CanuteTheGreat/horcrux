///! Automatic Snapshot Scheduling
///!
///! Provides cron-like scheduling for automated VM snapshots:
///! - Scheduled snapshot creation (hourly, daily, weekly, monthly)
///! - Retention policies (keep last N snapshots)
///! - Background task execution
///! - Failure handling and retry logic

use super::snapshot::{VmSnapshot, VmSnapshotManager};
use chrono::Datelike;
use horcrux_common::{Result, VmConfig};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration};
use tracing::{error, info, warn};

/// Snapshot schedule configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotSchedule {
    pub id: String,
    pub vm_id: String,
    pub name: String,
    pub frequency: ScheduleFrequency,
    pub retention_count: u32,  // Number of snapshots to keep
    pub enabled: bool,
    pub include_memory: bool,
    pub last_run: Option<i64>,
    pub next_run: i64,
    pub created_at: i64,
}

/// Schedule frequency options
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ScheduleFrequency {
    Hourly,
    Daily { hour: u8 },        // 0-23
    Weekly { day: u8, hour: u8 }, // day: 0-6 (Sun-Sat), hour: 0-23
    Monthly { day: u8, hour: u8 }, // day: 1-31, hour: 0-23
    Custom { cron: String },    // Custom cron expression
}

impl ScheduleFrequency {
    /// Calculate next run time from given timestamp
    pub fn next_run_after(&self, timestamp: i64) -> i64 {
        let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp, 0)
            .unwrap_or_else(|| chrono::Utc::now());

        let next = match self {
            ScheduleFrequency::Hourly => {
                dt + chrono::Duration::hours(1)
            }
            ScheduleFrequency::Daily { hour } => {
                let mut next = dt.date_naive().and_hms_opt(*hour as u32, 0, 0)
                    .unwrap_or(dt.naive_utc());
                if next <= dt.naive_utc() {
                    next = (dt + chrono::Duration::days(1)).date_naive()
                        .and_hms_opt(*hour as u32, 0, 0)
                        .unwrap_or(dt.naive_utc());
                }
                chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(next, chrono::Utc)
            }
            ScheduleFrequency::Weekly { day, hour } => {
                // day: 0=Sunday, 1=Monday, ..., 6=Saturday (Sunday-based numbering)
                // Convert to chrono::Weekday which uses Monday-based numbering (0=Mon, 6=Sun)
                let target_weekday = match *day {
                    0 => chrono::Weekday::Sun,
                    1 => chrono::Weekday::Mon,
                    2 => chrono::Weekday::Tue,
                    3 => chrono::Weekday::Wed,
                    4 => chrono::Weekday::Thu,
                    5 => chrono::Weekday::Fri,
                    6 => chrono::Weekday::Sat,
                    _ => chrono::Weekday::Mon, // Default to Monday for invalid values
                };
                let naive_dt = dt.naive_utc();
                let current_weekday = naive_dt.weekday();
                let days_until = ((target_weekday.number_from_sunday() + 7
                    - current_weekday.number_from_sunday()) % 7) as i64;

                let mut next = (dt + chrono::Duration::days(days_until))
                    .date_naive()
                    .and_hms_opt(*hour as u32, 0, 0)
                    .unwrap_or(naive_dt);

                if next <= naive_dt {
                    next = (dt + chrono::Duration::days(7)).date_naive()
                        .and_hms_opt(*hour as u32, 0, 0)
                        .unwrap_or(naive_dt);
                }
                chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(next, chrono::Utc)
            }
            ScheduleFrequency::Monthly { day, hour } => {
                let target_day = (*day).min(28); // Ensure day exists in all months
                let naive_dt = dt.naive_utc();
                let current_date = naive_dt.date();
                let mut next = current_date
                    .with_day(target_day as u32)
                    .and_then(|d| d.and_hms_opt(*hour as u32, 0, 0))
                    .unwrap_or(naive_dt);

                if next <= naive_dt {
                    // Move to next month
                    let month = naive_dt.month();
                    let year = naive_dt.year();
                    let next_month = if month == 12 {
                        chrono::NaiveDate::from_ymd_opt(year + 1, 1, target_day as u32)
                    } else {
                        chrono::NaiveDate::from_ymd_opt(year, month + 1, target_day as u32)
                    };
                    next = next_month
                        .and_then(|d| d.and_hms_opt(*hour as u32, 0, 0))
                        .unwrap_or(naive_dt);
                }
                chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(next, chrono::Utc)
            }
            ScheduleFrequency::Custom { cron: _ } => {
                // For simplicity, treat custom as hourly
                // In production, use a cron library like 'cron' crate
                dt + chrono::Duration::hours(1)
            }
        };

        next.timestamp()
    }
}

/// Snapshot scheduler manager
pub struct SnapshotScheduler {
    schedules: Arc<RwLock<HashMap<String, SnapshotSchedule>>>,
    snapshot_manager: Arc<RwLock<VmSnapshotManager>>,
}

impl SnapshotScheduler {
    pub fn new(snapshot_manager: Arc<RwLock<VmSnapshotManager>>) -> Self {
        Self {
            schedules: Arc::new(RwLock::new(HashMap::new())),
            snapshot_manager,
        }
    }

    /// Add a new snapshot schedule
    pub async fn add_schedule(&self, mut schedule: SnapshotSchedule) -> Result<()> {
        // Calculate next run if not set
        if schedule.next_run == 0 {
            schedule.next_run = schedule.frequency.next_run_after(chrono::Utc::now().timestamp());
        }

        let schedule_id = schedule.id.clone();
        self.schedules.write().await.insert(schedule_id.clone(), schedule);
        info!("Added snapshot schedule: {}", schedule_id);
        Ok(())
    }

    /// Remove a snapshot schedule
    pub async fn remove_schedule(&self, schedule_id: &str) -> Result<()> {
        self.schedules.write().await.remove(schedule_id);
        info!("Removed snapshot schedule: {}", schedule_id);
        Ok(())
    }

    /// List all schedules
    pub async fn list_schedules(&self) -> Vec<SnapshotSchedule> {
        self.schedules.read().await.values().cloned().collect()
    }

    /// Get a specific schedule
    pub async fn get_schedule(&self, schedule_id: &str) -> Option<SnapshotSchedule> {
        self.schedules.read().await.get(schedule_id).cloned()
    }

    /// Update a schedule
    pub async fn update_schedule(&self, schedule: SnapshotSchedule) -> Result<()> {
        let schedule_id = schedule.id.clone();
        self.schedules.write().await.insert(schedule_id, schedule);
        Ok(())
    }

    /// Execute a scheduled snapshot
    async fn execute_snapshot(
        &self,
        schedule: &SnapshotSchedule,
        vm_config: &VmConfig,
    ) -> Result<VmSnapshot> {
        info!(
            "Executing scheduled snapshot for VM {} (schedule: {})",
            vm_config.id, schedule.name
        );

        // Generate snapshot name with timestamp
        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        let snapshot_name = format!("{}_{}", schedule.name, timestamp);

        // Create snapshot
        let snapshot = self.snapshot_manager.write().await.create_snapshot(
            vm_config,
            snapshot_name,
            Some(format!("Automatic snapshot from schedule: {}", schedule.name)),
            schedule.include_memory,
        ).await?;

        // Clean up old snapshots based on retention policy
        self.cleanup_old_snapshots(&vm_config.id, &schedule.name, schedule.retention_count).await?;

        info!("Scheduled snapshot created: {}", snapshot.id);
        Ok(snapshot)
    }

    /// Clean up old snapshots exceeding retention count
    async fn cleanup_old_snapshots(
        &self,
        vm_id: &str,
        schedule_name: &str,
        retention_count: u32,
    ) -> Result<()> {
        let manager = self.snapshot_manager.read().await;
        let mut snapshots = manager.list_snapshots(vm_id);

        // Filter snapshots from this schedule (by name prefix)
        snapshots.retain(|s| s.name.starts_with(schedule_name));

        // Sort by creation time (newest first)
        snapshots.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        // Delete snapshots beyond retention count
        if snapshots.len() > retention_count as usize {
            let to_delete = &snapshots[retention_count as usize..];
            drop(manager); // Release read lock before getting write lock

            for snapshot in to_delete {
                info!("Deleting old snapshot due to retention policy: {}", snapshot.id);
                match self.snapshot_manager.write().await.delete_snapshot(&snapshot.id).await {
                    Ok(_) => info!("Deleted snapshot: {}", snapshot.id),
                    Err(e) => warn!("Failed to delete snapshot {}: {}", snapshot.id, e),
                }
            }
        }

        Ok(())
    }

    /// Start the scheduler background task
    pub fn start_scheduler(
        self: Arc<Self>,
        vm_getter: Arc<dyn Fn(&str) -> futures::future::BoxFuture<'static, Option<VmConfig>> + Send + Sync>,
    ) {
        tokio::spawn(async move {
            info!("Snapshot scheduler started");
            let mut check_interval = interval(Duration::from_secs(60)); // Check every minute

            loop {
                check_interval.tick().await;

                let now = chrono::Utc::now().timestamp();
                let schedules = self.schedules.read().await.clone();

                for (schedule_id, mut schedule) in schedules {
                    if !schedule.enabled {
                        continue;
                    }

                    // Check if it's time to run
                    if now >= schedule.next_run {
                        // Get VM configuration
                        let vm_config = match vm_getter(&schedule.vm_id).await {
                            Some(config) => config,
                            None => {
                                error!("VM {} not found for schedule {}", schedule.vm_id, schedule_id);
                                continue;
                            }
                        };

                        // Execute snapshot
                        match self.execute_snapshot(&schedule, &vm_config).await {
                            Ok(snapshot) => {
                                info!("Scheduled snapshot created successfully: {}", snapshot.id);

                                // Update schedule
                                schedule.last_run = Some(now);
                                schedule.next_run = schedule.frequency.next_run_after(now);

                                if let Err(e) = self.update_schedule(schedule.clone()).await {
                                    error!("Failed to update schedule {}: {}", schedule_id, e);
                                }
                            }
                            Err(e) => {
                                error!(
                                    "Failed to create scheduled snapshot for VM {} (schedule {}): {}",
                                    schedule.vm_id, schedule_id, e
                                );

                                // Still update next_run to avoid repeated failures
                                schedule.next_run = schedule.frequency.next_run_after(now);
                                if let Err(e) = self.update_schedule(schedule).await {
                                    error!("Failed to update schedule after error {}: {}", schedule_id, e);
                                }
                            }
                        }
                    }
                }
            }
        });
    }

    /// Stop the scheduler (for graceful shutdown)
    pub async fn stop(&self) {
        // Clear all schedules
        self.schedules.write().await.clear();
        info!("Snapshot scheduler stopped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    #[test]
    fn test_hourly_frequency() {
        let freq = ScheduleFrequency::Hourly;
        let now = chrono::Utc::now().timestamp();
        let next = freq.next_run_after(now);

        // Should be approximately 1 hour later
        assert!(next > now);
        assert!(next - now >= 3600);
        assert!(next - now <= 3601);
    }

    #[test]
    fn test_daily_frequency() {
        let freq = ScheduleFrequency::Daily { hour: 14 }; // 2 PM
        let now = chrono::Utc::now().timestamp();
        let next = freq.next_run_after(now);

        // Should be in the future
        assert!(next > now);

        // Next run should be at 14:00
        let next_dt = chrono::DateTime::<chrono::Utc>::from_timestamp(next, 0).unwrap();
        assert_eq!(next_dt.hour(), 14);
        assert_eq!(next_dt.minute(), 0);
    }

    #[test]
    fn test_weekly_frequency() {
        // Test with a specific known timestamp: Tuesday, 2024-01-02 15:00:00 UTC
        let tuesday_ts = chrono::NaiveDate::from_ymd_opt(2024, 1, 2)
            .unwrap()
            .and_hms_opt(15, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp();

        let freq = ScheduleFrequency::Weekly { day: 1, hour: 10 }; // Monday 10 AM
        let next = freq.next_run_after(tuesday_ts);

        // Should be in the future
        assert!(next > tuesday_ts);

        // Next run should be on next Monday (2024-01-08) at 10:00 UTC
        let next_dt = chrono::DateTime::<chrono::Utc>::from_timestamp(next, 0).unwrap();
        assert_eq!(next_dt.naive_utc().weekday(), chrono::Weekday::Mon);
        assert_eq!(next_dt.hour(), 10);
        assert_eq!(next_dt.minute(), 0);
    }
}
