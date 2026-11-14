#![allow(clippy::type_complexity)]
//! Automatic Snapshot Scheduling
//!
//! Provides cron-like scheduling for automated VM snapshots:
//! - Scheduled snapshot creation (hourly, daily, weekly, monthly)
//! - Retention policies (keep last N snapshots)
//! - Background task execution
//! - Failure handling and retry logic
use super::snapshot::{VmSnapshot, VmSnapshotManager};
use crate::db::config_store::{self, kinds, ConfigStore};
use crate::db::Database;
use chrono::Datelike;
use horcrux_common::{Result, VmConfig};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration};
use tracing::{error, info, warn};

/// True when `name` is a snapshot this schedule created itself
/// (`<schedule>_<YYYYmmdd_HHMMSS>`, see `execute_snapshot`).
pub(crate) fn is_scheduled_snapshot_name(name: &str, schedule_name: &str) -> bool {
    let Some(ts) = name
        .strip_prefix(schedule_name)
        .and_then(|rest| rest.strip_prefix('_'))
    else {
        return false;
    };
    ts.len() == 15
        && ts.char_indices().all(|(i, c)| match i {
            8 => c == '_',
            _ => c.is_ascii_digit(),
        })
}

/// Snapshot schedule configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotSchedule {
    #[serde(default)]
    pub id: String,
    pub vm_id: String,
    pub name: String,
    pub frequency: ScheduleFrequency,
    pub retention_count: u32, // Number of snapshots to keep
    pub enabled: bool,
    pub include_memory: bool,
    #[serde(default)]
    pub last_run: Option<i64>,
    #[serde(default)]
    pub next_run: i64,
    #[serde(default)]
    pub created_at: i64,
}

/// Schedule frequency options
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ScheduleFrequency {
    Hourly,
    Daily { hour: u8 },            // 0-23
    Weekly { day: u8, hour: u8 },  // day: 0-6 (Sun-Sat), hour: 0-23
    Monthly { day: u8, hour: u8 }, // day: 1-31, hour: 0-23
    Custom { cron: String },       // Custom cron expression
}

/// One cron field: the set of allowed values.
#[derive(Debug, Clone, PartialEq)]
struct CronField {
    allowed: Vec<bool>,
    any: bool,
}

impl CronField {
    fn parse(src: &str, min: u32, max: u32, what: &str) -> std::result::Result<Self, String> {
        let mut allowed = vec![false; (max + 1) as usize];
        let any = src == "*";
        for part in src.split(',') {
            let (range, step) = match part.split_once('/') {
                Some((r, st)) => {
                    let st: u32 = st
                        .parse()
                        .map_err(|_| format!("{what}: bad step in '{part}'"))?;
                    if st == 0 {
                        return Err(format!("{what}: step must be at least 1"));
                    }
                    (r, st)
                }
                None => (part, 1),
            };
            let (lo, hi) = if range == "*" {
                (min, max)
            } else if let Some((a, b)) = range.split_once('-') {
                let a: u32 = a.parse().map_err(|_| format!("{what}: bad value '{a}'"))?;
                let b: u32 = b.parse().map_err(|_| format!("{what}: bad value '{b}'"))?;
                (a, b)
            } else {
                let a: u32 = range
                    .parse()
                    .map_err(|_| format!("{what}: bad value '{range}'"))?;
                // "5/10" means from 5 to the end in steps of 10
                if part.contains('/') {
                    (a, max)
                } else {
                    (a, a)
                }
            };
            if lo < min || hi > max || lo > hi {
                return Err(format!("{what}: '{part}' is outside {min}-{max}"));
            }
            let mut v = lo;
            while v <= hi {
                allowed[v as usize] = true;
                v += step;
            }
        }
        Ok(Self { allowed, any })
    }

    fn has(&self, v: u32) -> bool {
        self.allowed.get(v as usize).copied().unwrap_or(false)
    }
}

/// A standard five-field cron expression (minute hour day-of-month month
/// day-of-week; `*`, lists, ranges and `/step`; day-of-week 0 or 7 = Sunday;
/// when both day fields are restricted a day matches either, as in cron(8)).
/// Evaluated in UTC like the other frequencies.
#[derive(Debug, Clone, PartialEq)]
pub struct CronExpr {
    minute: CronField,
    hour: CronField,
    dom: CronField,
    month: CronField,
    dow: CronField,
}

impl CronExpr {
    pub fn parse(src: &str) -> std::result::Result<Self, String> {
        let f: Vec<&str> = src.split_whitespace().collect();
        if f.len() != 5 {
            return Err(format!(
                "cron needs 5 fields (minute hour day-of-month month day-of-week), got {}",
                f.len()
            ));
        }
        let mut dow = CronField::parse(f[4], 0, 7, "day-of-week")?;
        if dow.allowed[7] {
            dow.allowed[0] = true;
        }
        Ok(Self {
            minute: CronField::parse(f[0], 0, 59, "minute")?,
            hour: CronField::parse(f[1], 0, 23, "hour")?,
            dom: CronField::parse(f[2], 1, 31, "day-of-month")?,
            month: CronField::parse(f[3], 1, 12, "month")?,
            dow,
        })
    }

    fn day_matches(&self, d: chrono::NaiveDate) -> bool {
        let dom = self.dom.has(d.day());
        let dow = self.dow.has(d.weekday().num_days_from_sunday());
        match (self.dom.any, self.dow.any) {
            (false, false) => dom || dow,
            (false, true) => dom,
            (true, false) => dow,
            (true, true) => true,
        }
    }

    /// First matching minute strictly after `timestamp` (unix seconds), within
    /// the next 5 years; `None` for an expression that never fires (31 Feb).
    pub fn next_after(&self, timestamp: i64) -> Option<i64> {
        let start = chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp, 0)?.naive_utc();
        let mut day = start.date();
        let limit = day + chrono::Duration::days(366 * 5);
        let mut first = true;
        while day <= limit {
            if self.month.has(day.month()) && self.day_matches(day) {
                for h in 0..24u32 {
                    if !self.hour.has(h) {
                        continue;
                    }
                    for m in 0..60u32 {
                        if !self.minute.has(m) {
                            continue;
                        }
                        let cand = day.and_hms_opt(h, m, 0)?;
                        if first && cand <= start {
                            continue;
                        }
                        return Some(cand.and_utc().timestamp());
                    }
                }
            }
            first = false;
            day += chrono::Duration::days(1);
        }
        None
    }
}

impl ScheduleFrequency {
    /// Reject a custom cron that does not parse or never fires.
    pub fn validate(&self) -> Result<()> {
        if let ScheduleFrequency::Custom { cron } = self {
            let c = CronExpr::parse(cron).map_err(horcrux_common::Error::Validation)?;
            if c.next_after(chrono::Utc::now().timestamp()).is_none() {
                return Err(horcrux_common::Error::Validation(format!(
                    "cron '{cron}' never fires"
                )));
            }
        }
        Ok(())
    }

    /// Calculate next run time from given timestamp
    pub fn next_run_after(&self, timestamp: i64) -> i64 {
        let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp, 0)
            .unwrap_or_else(chrono::Utc::now);

        let next = match self {
            ScheduleFrequency::Hourly => dt + chrono::Duration::hours(1),
            ScheduleFrequency::Daily { hour } => {
                let mut next = dt
                    .date_naive()
                    .and_hms_opt(*hour as u32, 0, 0)
                    .unwrap_or(dt.naive_utc());
                if next <= dt.naive_utc() {
                    next = (dt + chrono::Duration::days(1))
                        .date_naive()
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
                    - current_weekday.number_from_sunday())
                    % 7) as i64;

                let mut next = (dt + chrono::Duration::days(days_until))
                    .date_naive()
                    .and_hms_opt(*hour as u32, 0, 0)
                    .unwrap_or(naive_dt);

                if next <= naive_dt {
                    next = (dt + chrono::Duration::days(7))
                        .date_naive()
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
            ScheduleFrequency::Custom { cron } => {
                // A cron that does not parse is rejected when the schedule is
                // saved (`validate`); hourly is only the safe fallback.
                match CronExpr::parse(cron)
                    .ok()
                    .and_then(|c| c.next_after(timestamp))
                {
                    Some(t) => return t,
                    None => dt + chrono::Duration::hours(1),
                }
            }
        };

        next.timestamp()
    }
}

/// Snapshot scheduler manager
pub struct SnapshotScheduler {
    schedules: Arc<RwLock<HashMap<String, SnapshotSchedule>>>,
    snapshot_manager: Arc<RwLock<VmSnapshotManager>>,
    /// Durable copy of `schedules`; `None` = in-memory only.
    store: Option<ConfigStore>,
}

impl SnapshotScheduler {
    pub fn new(snapshot_manager: Arc<RwLock<VmSnapshotManager>>) -> Self {
        Self {
            schedules: Arc::new(RwLock::new(HashMap::new())),
            snapshot_manager,
            store: None,
        }
    }

    /// Scheduler backed by the database: loads persisted schedules (with
    /// their last/next run times, so a schedule that came due while the
    /// process was down runs on the first tick) and writes every change
    /// through.
    pub async fn with_database(
        snapshot_manager: Arc<RwLock<VmSnapshotManager>>,
        db: Arc<Database>,
    ) -> Result<Self> {
        let store = ConfigStore::new(&db);
        let mut sched = Self::new(snapshot_manager);
        {
            let mut schedules = sched.schedules.write().await;
            for s in store
                .load_values::<SnapshotSchedule>(kinds::SNAPSHOT_SCHEDULE)
                .await?
            {
                schedules.insert(s.id.clone(), s);
            }
            info!(
                "Loaded {} snapshot schedule(s) from database",
                schedules.len()
            );
        }
        sched.store = Some(store);
        Ok(sched)
    }

    /// Add a new snapshot schedule
    pub async fn add_schedule(&self, mut schedule: SnapshotSchedule) -> Result<()> {
        schedule.frequency.validate()?;
        // Calculate next run if not set
        if schedule.next_run == 0 {
            schedule.next_run = schedule
                .frequency
                .next_run_after(chrono::Utc::now().timestamp());
        }

        let schedule_id = schedule.id.clone();
        let mut schedules = self.schedules.write().await;
        config_store::put_opt(
            &self.store,
            kinds::SNAPSHOT_SCHEDULE,
            &schedule_id,
            &schedule,
        )
        .await?;
        schedules.insert(schedule_id.clone(), schedule);
        info!("Added snapshot schedule: {}", schedule_id);
        Ok(())
    }

    /// Remove a snapshot schedule
    pub async fn remove_schedule(&self, schedule_id: &str) -> Result<()> {
        let mut schedules = self.schedules.write().await;
        config_store::delete_opt(&self.store, kinds::SNAPSHOT_SCHEDULE, schedule_id).await?;
        schedules.remove(schedule_id);
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

    /// Update an existing schedule. Unknown ids are rejected: the background
    /// loop updates run times from a snapshot of the schedule list, and an
    /// upsert here would resurrect (and persist) a schedule deleted while a
    /// snapshot was being taken.
    pub async fn update_schedule(&self, schedule: SnapshotSchedule) -> Result<()> {
        schedule.frequency.validate()?;
        let schedule_id = schedule.id.clone();
        let mut schedules = self.schedules.write().await;
        if !schedules.contains_key(&schedule_id) {
            return Err(horcrux_common::Error::NotFound(format!(
                "Snapshot schedule {} not found",
                schedule_id
            )));
        }
        config_store::put_opt(
            &self.store,
            kinds::SNAPSHOT_SCHEDULE,
            &schedule_id,
            &schedule,
        )
        .await?;
        schedules.insert(schedule_id, schedule);
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
        let snapshot = self
            .snapshot_manager
            .write()
            .await
            .create_snapshot(
                vm_config,
                snapshot_name,
                Some(format!(
                    "Automatic snapshot from schedule: {}",
                    schedule.name
                )),
                schedule.include_memory,
            )
            .await?;

        // Clean up old snapshots based on retention policy
        self.cleanup_old_snapshots(&vm_config.id, &schedule.name, schedule.retention_count)
            .await?;

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
        // (exact `<schedule>_<YYYYmmdd_HHMMSS>`: a bare prefix match would
        // also prune hand-made snapshots such as `nightly-before-upgrade`
        // and the snapshots of a schedule named `nightly2`)
        snapshots.retain(|s| is_scheduled_snapshot_name(&s.name, schedule_name));

        // Sort by creation time (newest first)
        snapshots.sort_by_key(|x| std::cmp::Reverse(x.created_at));

        // Delete snapshots beyond retention count
        if snapshots.len() > retention_count as usize {
            let to_delete = &snapshots[retention_count as usize..];
            drop(manager); // Release read lock before getting write lock

            for snapshot in to_delete {
                info!(
                    "Deleting old snapshot due to retention policy: {}",
                    snapshot.id
                );
                match self
                    .snapshot_manager
                    .write()
                    .await
                    .delete_snapshot(&snapshot.id)
                    .await
                {
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
        vm_getter: Arc<
            dyn Fn(&str) -> futures::future::BoxFuture<'static, Option<VmConfig>> + Send + Sync,
        >,
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
                                error!(
                                    "VM {} not found for schedule {}",
                                    schedule.vm_id, schedule_id
                                );
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
                                    error!(
                                        "Failed to update schedule after error {}: {}",
                                        schedule_id, e
                                    );
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

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        chrono::NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
            .and_utc()
            .timestamp()
    }

    fn cron_next(expr: &str, after: i64) -> i64 {
        ScheduleFrequency::Custom { cron: expr.into() }.next_run_after(after)
    }

    #[test]
    fn custom_cron_every_five_minutes() {
        // 2026-11-12 10:02 -> 10:05, and from exactly 10:05 -> 10:10 (strictly after)
        assert_eq!(
            cron_next("*/5 * * * *", ts(2026, 11, 12, 10, 2)),
            ts(2026, 11, 12, 10, 5)
        );
        assert_eq!(
            cron_next("*/5 * * * *", ts(2026, 11, 12, 10, 5)),
            ts(2026, 11, 12, 10, 10)
        );
    }

    #[test]
    fn custom_cron_not_hourly_any_more() {
        // 02:30 every day, asked at 03:00 -> next day 02:30 (was: +1 h)
        assert_eq!(
            cron_next("30 2 * * *", ts(2026, 11, 12, 3, 0)),
            ts(2026, 11, 13, 2, 30)
        );
        assert_eq!(
            cron_next("30 2 * * *", ts(2026, 11, 12, 1, 0)),
            ts(2026, 11, 12, 2, 30)
        );
    }

    #[test]
    fn custom_cron_weekday_lists_and_ranges() {
        // 2026-11-12 is a Thursday; Mon-Fri 09:00 from Fri 10:00 -> Mon 09:00
        assert_eq!(
            cron_next("0 9 * * 1-5", ts(2026, 11, 13, 10, 0)),
            ts(2026, 11, 16, 9, 0)
        );
        // Sunday as 0 and as 7
        assert_eq!(
            cron_next("0 0 * * 0", ts(2026, 11, 12, 0, 0)),
            ts(2026, 11, 15, 0, 0)
        );
        assert_eq!(
            cron_next("0 0 * * 7", ts(2026, 11, 12, 0, 0)),
            ts(2026, 11, 15, 0, 0)
        );
        assert_eq!(
            cron_next("15,45 8 * * *", ts(2026, 11, 12, 8, 20)),
            ts(2026, 11, 12, 8, 45)
        );
    }

    #[test]
    fn custom_cron_day_of_month_or_weekday() {
        // both restricted: 1st of the month OR Monday (cron(8) semantics)
        assert_eq!(
            cron_next("0 0 1 * 1", ts(2026, 11, 12, 0, 0)),
            ts(2026, 11, 16, 0, 0)
        );
        assert_eq!(
            cron_next("0 0 1 * 1", ts(2026, 11, 29, 12, 0)),
            ts(2026, 11, 30, 0, 0)
        );
        // month and year roll-over
        assert_eq!(
            cron_next("0 0 1 1 *", ts(2026, 11, 12, 0, 0)),
            ts(2027, 1, 1, 0, 0)
        );
    }

    #[test]
    fn custom_cron_rejects_bad_expressions() {
        for bad in [
            "",
            "* * * *",
            "60 * * * *",
            "* 24 * * *",
            "* * 0 * *",
            "* * * 13 *",
            "*/0 * * * *",
            "a * * * *",
            "5-1 * * * *",
            "* * * * 8",
        ] {
            let f = ScheduleFrequency::Custom { cron: bad.into() };
            assert!(f.validate().is_err(), "{bad:?} must be rejected");
        }
        // never fires: 31 February
        assert!(ScheduleFrequency::Custom {
            cron: "0 0 31 2 *".into()
        }
        .validate()
        .is_err());
        assert!(ScheduleFrequency::Custom {
            cron: "0 3 * * 1-5".into()
        }
        .validate()
        .is_ok());
        assert!(ScheduleFrequency::Hourly.validate().is_ok());
    }

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
    fn retention_only_touches_snapshots_the_schedule_made() {
        use super::is_scheduled_snapshot_name as own;
        assert!(own("nightly_20261113_080000", "nightly"));
        assert!(!own("nightly-before-upgrade", "nightly"));
        assert!(!own("nightly", "nightly"));
        assert!(!own("nightly2_20261113_080000", "nightly"));
        assert!(!own("nightly_manual", "nightly"));
        assert!(!own("nightly_20261113_08000x", "nightly"));
        assert!(!own("nightly_20261113-080000", "nightly"));
        assert!(own("a_b_20261113_080000", "a_b"));
        assert!(!own("a_b_20261113_080000", "a"));
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

/// Persistence through the config store (moved from db/config_store.rs so
/// the database layer no longer depends on this module).
#[cfg(test)]
#[allow(unused_imports)]
mod config_store_persistence_tests {
    use crate::db::config_store::{kinds, ConfigStore};
    use crate::db::Database;

    async fn store() -> (tempfile::TempDir, Database, ConfigStore) {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}", dir.path().join("t.db").display());
        let db = Database::new(&url).await.unwrap();
        db.migrate().await.unwrap();
        let s = ConfigStore::new(&db);
        (dir, db, s)
    }

    #[tokio::test]
    async fn snapshot_schedule_update_does_not_resurrect_deleted_schedule() {
        use crate::vm::snapshot::VmSnapshotManager;
        use crate::vm::snapshot_scheduler::{
            ScheduleFrequency, SnapshotSchedule, SnapshotScheduler,
        };
        let (dir, db, s) = store().await;
        let db = std::sync::Arc::new(db);
        let snaps = std::sync::Arc::new(tokio::sync::RwLock::new(VmSnapshotManager::new(
            dir.path().join("snaps").to_string_lossy().to_string(),
        )));
        let sched = SnapshotScheduler::with_database(snaps.clone(), db.clone())
            .await
            .unwrap();
        let schedule = SnapshotSchedule {
            id: "s1".into(),
            vm_id: "vm1".into(),
            name: "nightly".into(),
            frequency: ScheduleFrequency::Hourly,
            retention_count: 2,
            enabled: true,
            include_memory: false,
            last_run: None,
            next_run: 0,
            created_at: 0,
        };
        sched.add_schedule(schedule.clone()).await.unwrap();
        sched.remove_schedule("s1").await.unwrap();
        assert!(sched.update_schedule(schedule).await.is_err());
        assert!(s
            .load_values::<SnapshotSchedule>(kinds::SNAPSHOT_SCHEDULE)
            .await
            .unwrap()
            .is_empty());
        let reloaded = SnapshotScheduler::with_database(snaps, db).await.unwrap();
        assert!(reloaded.list_schedules().await.is_empty());
    }
}
