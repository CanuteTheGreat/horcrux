//! NAS Job Scheduler
//!
//! Handles cron-like scheduling of NAS jobs including:
//! - Snapshot schedules
//! - Replication tasks
//! - Pool scrubs
//! - Quota checks
//! - Health checks

use horcrux_common::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{Duration, Instant};
use chrono::{DateTime, Utc, Datelike, Timelike, Weekday};

/// Job type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobType {
    /// Snapshot creation
    Snapshot,
    /// Snapshot retention cleanup
    RetentionCleanup,
    /// Replication task
    Replication,
    /// Pool scrub
    Scrub,
    /// Pool resilver check
    Resilver,
    /// Quota enforcement
    QuotaCheck,
    /// Service health check
    HealthCheck,
    /// S3 bucket cleanup
    S3Cleanup,
    /// SMART check
    SmartCheck,
    /// Custom script
    Custom,
}

impl std::fmt::Display for JobType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobType::Snapshot => write!(f, "snapshot"),
            JobType::RetentionCleanup => write!(f, "retention_cleanup"),
            JobType::Replication => write!(f, "replication"),
            JobType::Scrub => write!(f, "scrub"),
            JobType::Resilver => write!(f, "resilver"),
            JobType::QuotaCheck => write!(f, "quota_check"),
            JobType::HealthCheck => write!(f, "health_check"),
            JobType::S3Cleanup => write!(f, "s3_cleanup"),
            JobType::SmartCheck => write!(f, "smart_check"),
            JobType::Custom => write!(f, "custom"),
        }
    }
}

/// Scheduled job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledJob {
    /// Job ID
    pub id: String,
    /// Job name
    pub name: String,
    /// Job type
    pub job_type: JobType,
    /// Cron schedule expression
    pub schedule: String,
    /// Target (dataset, pool, etc.)
    pub target: String,
    /// Job parameters (JSON)
    pub params: HashMap<String, serde_json::Value>,
    /// Enabled
    pub enabled: bool,
    /// Run on startup (if missed)
    pub run_on_startup: bool,
    /// Last run timestamp
    pub last_run: Option<i64>,
    /// Last run status
    pub last_status: Option<JobStatus>,
    /// Last run duration (ms)
    pub last_duration_ms: Option<u64>,
    /// Last error message
    pub last_error: Option<String>,
    /// Next scheduled run
    pub next_run: Option<i64>,
    /// Priority (lower = higher priority)
    pub priority: u32,
    /// Timeout in seconds (0 = no timeout)
    pub timeout_secs: u64,
    /// Max retries
    pub max_retries: u32,
    /// Created at
    pub created_at: i64,
    /// Modified at
    pub modified_at: i64,
}

impl Default for ScheduledJob {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: String::new(),
            job_type: JobType::Snapshot,
            schedule: "0 * * * *".to_string(), // Hourly
            target: String::new(),
            params: HashMap::new(),
            enabled: true,
            run_on_startup: false,
            last_run: None,
            last_status: None,
            last_duration_ms: None,
            last_error: None,
            next_run: None,
            priority: 100,
            timeout_secs: 3600, // 1 hour
            max_retries: 0,
            created_at: Utc::now().timestamp(),
            modified_at: Utc::now().timestamp(),
        }
    }
}

/// Job execution status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    /// Pending execution
    Pending,
    /// Currently running
    Running,
    /// Completed successfully
    Success,
    /// Failed
    Failed,
    /// Timed out
    Timeout,
    /// Cancelled
    Cancelled,
    /// Skipped (e.g., already running)
    Skipped,
}

/// Job execution record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobExecution {
    /// Execution ID
    pub id: String,
    /// Job ID
    pub job_id: String,
    /// Job name
    pub job_name: String,
    /// Job type
    pub job_type: JobType,
    /// Target
    pub target: String,
    /// Status
    pub status: JobStatus,
    /// Started at
    pub started_at: i64,
    /// Ended at
    pub ended_at: Option<i64>,
    /// Duration in milliseconds
    pub duration_ms: Option<u64>,
    /// Error message
    pub error: Option<String>,
    /// Output/result (JSON)
    pub output: Option<serde_json::Value>,
    /// Triggered by (schedule, manual, startup)
    pub trigger: JobTrigger,
    /// Retry count
    pub retry_count: u32,
}

/// Job trigger type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobTrigger {
    /// Triggered by schedule
    Schedule,
    /// Manually triggered
    Manual,
    /// Triggered on startup (missed job)
    Startup,
    /// Triggered as retry
    Retry,
}

/// Parsed cron schedule
#[derive(Debug, Clone)]
pub struct CronSchedule {
    /// Minutes (0-59)
    pub minutes: Vec<u8>,
    /// Hours (0-23)
    pub hours: Vec<u8>,
    /// Days of month (1-31)
    pub days: Vec<u8>,
    /// Months (1-12)
    pub months: Vec<u8>,
    /// Days of week (0-6, 0=Sunday)
    pub weekdays: Vec<u8>,
}

impl CronSchedule {
    /// Parse cron expression
    pub fn parse(expr: &str) -> Result<Self> {
        let parts: Vec<&str> = expr.split_whitespace().collect();
        if parts.len() != 5 {
            return Err(Error::Validation(format!(
                "Invalid cron expression '{}': expected 5 fields (minute hour day month weekday)",
                expr
            )));
        }

        Ok(Self {
            minutes: Self::parse_field(parts[0], 0, 59)?,
            hours: Self::parse_field(parts[1], 0, 23)?,
            days: Self::parse_field(parts[2], 1, 31)?,
            months: Self::parse_field(parts[3], 1, 12)?,
            weekdays: Self::parse_field(parts[4], 0, 6)?,
        })
    }

    /// Parse a single cron field
    fn parse_field(field: &str, min: u8, max: u8) -> Result<Vec<u8>> {
        let mut values = Vec::new();

        for part in field.split(',') {
            if part == "*" {
                return Ok((min..=max).collect());
            }

            // Handle step (*/n or x-y/n)
            let (range_part, step) = if let Some((r, s)) = part.split_once('/') {
                (r, s.parse::<u8>().map_err(|_| {
                    Error::Validation(format!("Invalid step value: {}", s))
                })?)
            } else {
                (part, 1)
            };

            // Handle range (x-y)
            let range_values: Vec<u8> = if range_part == "*" {
                (min..=max).collect()
            } else if let Some((start, end)) = range_part.split_once('-') {
                let start = start.parse::<u8>().map_err(|_| {
                    Error::Validation(format!("Invalid range start: {}", start))
                })?;
                let end = end.parse::<u8>().map_err(|_| {
                    Error::Validation(format!("Invalid range end: {}", end))
                })?;
                if start > end || start < min || end > max {
                    return Err(Error::Validation(format!(
                        "Invalid range: {}-{} (must be {}-{})",
                        start, end, min, max
                    )));
                }
                (start..=end).collect()
            } else {
                let val = range_part.parse::<u8>().map_err(|_| {
                    Error::Validation(format!("Invalid value: {}", range_part))
                })?;
                if val < min || val > max {
                    return Err(Error::Validation(format!(
                        "Value {} out of range {}-{}",
                        val, min, max
                    )));
                }
                vec![val]
            };

            // Apply step
            for (i, &v) in range_values.iter().enumerate() {
                if i % (step as usize) == 0 {
                    if !values.contains(&v) {
                        values.push(v);
                    }
                }
            }
        }

        values.sort();
        Ok(values)
    }

    /// Check if the schedule matches the given time
    pub fn matches(&self, dt: &DateTime<Utc>) -> bool {
        let minute = dt.minute() as u8;
        let hour = dt.hour() as u8;
        let day = dt.day() as u8;
        let month = dt.month() as u8;
        let weekday = dt.weekday().num_days_from_sunday() as u8;

        self.minutes.contains(&minute)
            && self.hours.contains(&hour)
            && self.days.contains(&day)
            && self.months.contains(&month)
            && self.weekdays.contains(&weekday)
    }

    /// Calculate the next run time after the given time
    pub fn next_run_after(&self, after: &DateTime<Utc>) -> Option<DateTime<Utc>> {
        let mut current = *after + chrono::Duration::minutes(1);
        // Set seconds to 0
        current = current.with_second(0)?.with_nanosecond(0)?;

        // Search for up to 2 years
        let max_iterations = 365 * 24 * 60 * 2;

        for _ in 0..max_iterations {
            if self.matches(&current) {
                return Some(current);
            }
            current = current + chrono::Duration::minutes(1);
        }

        None
    }
}

/// Scheduler state
struct SchedulerState {
    /// Jobs by ID
    jobs: HashMap<String, ScheduledJob>,
    /// Running jobs (job_id -> execution_id)
    running: HashMap<String, String>,
    /// Execution history
    history: Vec<JobExecution>,
    /// Max history entries
    max_history: usize,
}

/// NAS Job Scheduler
pub struct NasScheduler {
    state: Arc<RwLock<SchedulerState>>,
    running: Arc<RwLock<bool>>,
}

impl NasScheduler {
    /// Create a new scheduler
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(SchedulerState {
                jobs: HashMap::new(),
                running: HashMap::new(),
                history: Vec::new(),
                max_history: 10000,
            })),
            running: Arc::new(RwLock::new(false)),
        }
    }

    /// Add a job to the scheduler
    pub async fn add_job(&self, mut job: ScheduledJob) -> Result<String> {
        // Validate schedule
        let _ = CronSchedule::parse(&job.schedule)?;

        // Calculate next run
        job.next_run = CronSchedule::parse(&job.schedule)
            .ok()
            .and_then(|s| s.next_run_after(&Utc::now()))
            .map(|dt| dt.timestamp());

        let id = job.id.clone();
        self.state.write().await.jobs.insert(id.clone(), job);

        Ok(id)
    }

    /// Remove a job from the scheduler
    pub async fn remove_job(&self, job_id: &str) -> Result<()> {
        let mut state = self.state.write().await;

        if state.running.contains_key(job_id) {
            return Err(Error::Conflict(format!("Job {} is currently running", job_id)));
        }

        state.jobs.remove(job_id);
        Ok(())
    }

    /// Update a job
    pub async fn update_job(&self, job: ScheduledJob) -> Result<()> {
        // Validate schedule
        let _ = CronSchedule::parse(&job.schedule)?;

        let mut state = self.state.write().await;

        if !state.jobs.contains_key(&job.id) {
            return Err(Error::NotFound(format!("Job {} not found", job.id)));
        }

        let mut updated_job = job;
        updated_job.modified_at = Utc::now().timestamp();
        updated_job.next_run = CronSchedule::parse(&updated_job.schedule)
            .ok()
            .and_then(|s| s.next_run_after(&Utc::now()))
            .map(|dt| dt.timestamp());

        state.jobs.insert(updated_job.id.clone(), updated_job);
        Ok(())
    }

    /// Get a job by ID
    pub async fn get_job(&self, job_id: &str) -> Option<ScheduledJob> {
        self.state.read().await.jobs.get(job_id).cloned()
    }

    /// List all jobs
    pub async fn list_jobs(&self) -> Vec<ScheduledJob> {
        self.state.read().await.jobs.values().cloned().collect()
    }

    /// List jobs by type
    pub async fn list_jobs_by_type(&self, job_type: JobType) -> Vec<ScheduledJob> {
        self.state.read().await
            .jobs
            .values()
            .filter(|j| j.job_type == job_type)
            .cloned()
            .collect()
    }

    /// Enable/disable a job
    pub async fn set_job_enabled(&self, job_id: &str, enabled: bool) -> Result<()> {
        let mut state = self.state.write().await;

        if let Some(job) = state.jobs.get_mut(job_id) {
            job.enabled = enabled;
            job.modified_at = Utc::now().timestamp();
            Ok(())
        } else {
            Err(Error::NotFound(format!("Job {} not found", job_id)))
        }
    }

    /// Run a job immediately (manual trigger)
    pub async fn run_job_now(&self, job_id: &str) -> Result<String> {
        let job = self.state.read().await
            .jobs
            .get(job_id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("Job {} not found", job_id)))?;

        self.execute_job(&job, JobTrigger::Manual).await
    }

    /// Get job execution history
    pub async fn get_history(&self, limit: usize) -> Vec<JobExecution> {
        let state = self.state.read().await;
        state.history.iter().rev().take(limit).cloned().collect()
    }

    /// Get execution history for a specific job
    pub async fn get_job_history(&self, job_id: &str, limit: usize) -> Vec<JobExecution> {
        let state = self.state.read().await;
        state.history
            .iter()
            .filter(|e| e.job_id == job_id)
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }

    /// Get currently running jobs
    pub async fn get_running_jobs(&self) -> Vec<(String, String)> {
        self.state.read().await
            .running
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Check if a job is running
    pub async fn is_job_running(&self, job_id: &str) -> bool {
        self.state.read().await.running.contains_key(job_id)
    }

    /// Start the scheduler loop
    pub async fn start(&self) -> Result<()> {
        let mut running = self.running.write().await;
        if *running {
            return Err(Error::Conflict("Scheduler already running".to_string()));
        }
        *running = true;
        drop(running);

        tracing::info!("NAS scheduler started");

        // Run startup jobs
        self.run_startup_jobs().await;

        // Start the scheduler loop
        let state = self.state.clone();
        let running = self.running.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));

            loop {
                interval.tick().await;

                if !*running.read().await {
                    break;
                }

                // Check for jobs to run
                let now = Utc::now();
                let jobs: Vec<ScheduledJob> = state.read().await
                    .jobs
                    .values()
                    .filter(|j| j.enabled && j.next_run.is_some())
                    .filter(|j| j.next_run.unwrap() <= now.timestamp())
                    .cloned()
                    .collect();

                for job in jobs {
                    // Skip if already running
                    if state.read().await.running.contains_key(&job.id) {
                        tracing::debug!("Job {} already running, skipping", job.id);
                        continue;
                    }

                    // Execute job
                    let state_clone = state.clone();
                    let job_clone = job.clone();
                    tokio::spawn(async move {
                        if let Err(e) = Self::execute_job_internal(&state_clone, &job_clone, JobTrigger::Schedule).await {
                            tracing::error!("Job {} failed: {}", job_clone.id, e);
                        }
                    });
                }
            }

            tracing::info!("NAS scheduler stopped");
        });

        Ok(())
    }

    /// Stop the scheduler
    pub async fn stop(&self) {
        *self.running.write().await = false;
    }

    /// Run startup jobs (for missed executions)
    async fn run_startup_jobs(&self) {
        let jobs: Vec<ScheduledJob> = self.state.read().await
            .jobs
            .values()
            .filter(|j| j.enabled && j.run_on_startup)
            .filter(|j| {
                if let Some(last_run) = j.last_run {
                    // If last run was before last scheduled time, run it
                    if let Some(next) = j.next_run {
                        return last_run < next;
                    }
                }
                true
            })
            .cloned()
            .collect();

        for job in jobs {
            tracing::info!("Running startup job: {} ({})", job.name, job.id);
            let _ = self.execute_job(&job, JobTrigger::Startup).await;
        }
    }

    /// Execute a job
    async fn execute_job(&self, job: &ScheduledJob, trigger: JobTrigger) -> Result<String> {
        Self::execute_job_internal(&self.state, job, trigger).await
    }

    /// Internal job execution
    async fn execute_job_internal(
        state: &Arc<RwLock<SchedulerState>>,
        job: &ScheduledJob,
        trigger: JobTrigger,
    ) -> Result<String> {
        let execution_id = uuid::Uuid::new_v4().to_string();
        let started_at = Utc::now().timestamp();

        // Create execution record
        let mut execution = JobExecution {
            id: execution_id.clone(),
            job_id: job.id.clone(),
            job_name: job.name.clone(),
            job_type: job.job_type,
            target: job.target.clone(),
            status: JobStatus::Running,
            started_at,
            ended_at: None,
            duration_ms: None,
            error: None,
            output: None,
            trigger,
            retry_count: 0,
        };

        // Mark as running
        state.write().await.running.insert(job.id.clone(), execution_id.clone());

        // Execute based on job type
        let start = Instant::now();
        let result = Self::run_job_task(job).await;
        let duration_ms = start.elapsed().as_millis() as u64;

        // Update execution record
        execution.ended_at = Some(Utc::now().timestamp());
        execution.duration_ms = Some(duration_ms);

        match result {
            Ok(output) => {
                execution.status = JobStatus::Success;
                execution.output = output;
            }
            Err(e) => {
                execution.status = JobStatus::Failed;
                execution.error = Some(e.to_string());
            }
        }

        // Update state
        let mut state_write = state.write().await;

        // Remove from running
        state_write.running.remove(&job.id);

        // Update job
        if let Some(job_entry) = state_write.jobs.get_mut(&job.id) {
            job_entry.last_run = Some(started_at);
            job_entry.last_status = Some(execution.status);
            job_entry.last_duration_ms = Some(duration_ms);
            job_entry.last_error = execution.error.clone();

            // Calculate next run
            job_entry.next_run = CronSchedule::parse(&job_entry.schedule)
                .ok()
                .and_then(|s| s.next_run_after(&Utc::now()))
                .map(|dt| dt.timestamp());
        }

        // Add to history
        state_write.history.push(execution);
        if state_write.history.len() > state_write.max_history {
            state_write.history.remove(0);
        }

        Ok(execution_id)
    }

    /// Run the actual job task
    async fn run_job_task(job: &ScheduledJob) -> Result<Option<serde_json::Value>> {
        match job.job_type {
            JobType::Snapshot => {
                Self::run_snapshot_job(job).await
            }
            JobType::RetentionCleanup => {
                Self::run_retention_job(job).await
            }
            JobType::Replication => {
                Self::run_replication_job(job).await
            }
            JobType::Scrub => {
                Self::run_scrub_job(job).await
            }
            JobType::HealthCheck => {
                Self::run_health_check_job(job).await
            }
            JobType::QuotaCheck => {
                Self::run_quota_check_job(job).await
            }
            JobType::SmartCheck => {
                Self::run_smart_check_job(job).await
            }
            JobType::Custom => {
                Self::run_custom_job(job).await
            }
            _ => {
                tracing::warn!("Unsupported job type: {:?}", job.job_type);
                Ok(None)
            }
        }
    }

    /// Run snapshot job
    async fn run_snapshot_job(job: &ScheduledJob) -> Result<Option<serde_json::Value>> {
        use crate::nas::storage::snapshots;

        let dataset = &job.target;
        let prefix = job.params.get("prefix")
            .and_then(|v| v.as_str())
            .unwrap_or("auto");

        let recursive = job.params.get("recursive")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let snapshot_name = format!(
            "{}@{}_{}",
            dataset,
            prefix,
            Utc::now().format("%Y-%m-%d_%H-%M-%S")
        );

        if recursive {
            let manager = snapshots::SnapshotManager::new();
            let result = manager.create_recursive_snapshot(dataset, &format!(
                "{}_{}",
                prefix,
                Utc::now().format("%Y-%m-%d_%H-%M-%S")
            )).await?;
            Ok(Some(serde_json::json!({
                "created": result.created,
                "errors": result.errors
            })))
        } else {
            snapshots::create_snapshot(&snapshot_name).await?;
            Ok(Some(serde_json::json!({
                "snapshot": snapshot_name
            })))
        }
    }

    /// Run retention cleanup job
    async fn run_retention_job(job: &ScheduledJob) -> Result<Option<serde_json::Value>> {
        use crate::nas::storage::snapshots;

        let dataset = &job.target;

        // Get retention policy from params
        let keep_hourly = job.params.get("keep_hourly")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);
        let keep_daily = job.params.get("keep_daily")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);
        let keep_weekly = job.params.get("keep_weekly")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);
        let keep_monthly = job.params.get("keep_monthly")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);
        let keep_yearly = job.params.get("keep_yearly")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);

        let policy = snapshots::RetentionPolicy {
            name: "scheduled".to_string(),
            keep_hourly,
            keep_daily,
            keep_weekly,
            keep_monthly,
            keep_yearly,
            min_age_days: None,
            max_age_days: job.params.get("max_age_days")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32),
            protect_holds: true,
            protect_manual: true,
        };

        let manager = snapshots::SnapshotManager::new();
        let result = manager.apply_retention_policy(dataset, &policy).await?;

        Ok(Some(serde_json::json!({
            "deleted": result.deleted,
            "errors": result.errors
        })))
    }

    /// Run replication job
    async fn run_replication_job(job: &ScheduledJob) -> Result<Option<serde_json::Value>> {
        use crate::nas::storage::replication;

        let task_id = &job.target;

        // Create basic replication task from params
        let source = job.params.get("source")
            .and_then(|v| v.as_str())
            .unwrap_or(&job.target);
        let target_host = job.params.get("target_host")
            .and_then(|v| v.as_str())
            .unwrap_or("localhost");
        let target_dataset = job.params.get("target_dataset")
            .and_then(|v| v.as_str())
            .unwrap_or(source);

        let task = crate::nas::storage::ReplicationTask {
            id: task_id.clone(),
            name: job.name.clone(),
            source_dataset: source.to_string(),
            target_host: target_host.to_string(),
            target_dataset: target_dataset.to_string(),
            direction: crate::nas::storage::ReplicationDirection::Push,
            transport: crate::nas::storage::ReplicationTransport::Ssh,
            schedule: job.schedule.clone(),
            recursive: job.params.get("recursive").and_then(|v| v.as_bool()).unwrap_or(true),
            retention: None,
            compression: job.params.get("compression").and_then(|v| v.as_bool()).unwrap_or(true),
            bandwidth_limit: job.params.get("bandwidth_limit").and_then(|v| v.as_u64()).map(|v| v as u32),
            enabled: true,
            last_run: None,
            last_status: None,
            created_at: job.created_at,
        };

        replication::run_replication(&task).await?;

        Ok(Some(serde_json::json!({
            "task_id": task_id,
            "source": source,
            "target": format!("{}:{}", target_host, target_dataset)
        })))
    }

    /// Run scrub job
    #[cfg(feature = "nas-zfs")]
    async fn run_scrub_job(job: &ScheduledJob) -> Result<Option<serde_json::Value>> {
        use tokio::process::Command;

        let pool = &job.target;

        let output = Command::new("zpool")
            .args(["scrub", pool])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("zpool scrub failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Scrub failed: {}", stderr)));
        }

        Ok(Some(serde_json::json!({
            "pool": pool,
            "status": "started"
        })))
    }

    #[cfg(not(feature = "nas-zfs"))]
    async fn run_scrub_job(job: &ScheduledJob) -> Result<Option<serde_json::Value>> {
        Err(Error::Internal("ZFS not enabled".to_string()))
    }

    /// Run health check job
    async fn run_health_check_job(job: &ScheduledJob) -> Result<Option<serde_json::Value>> {
        let mut results = HashMap::new();

        // Check services
        let services = vec!["smb", "nfs", "ftp", "minio", "tgtd"];
        for service in services {
            let running = crate::nas::services::is_service_running(service).await;
            results.insert(service.to_string(), serde_json::json!({ "running": running }));
        }

        // Check pool health (if ZFS)
        #[cfg(feature = "nas-zfs")]
        {
            if let Ok(pools) = crate::nas::storage::pools::list_pools().await {
                for pool in pools {
                    results.insert(
                        format!("pool_{}", pool.name),
                        serde_json::json!({
                            "status": pool.status,
                            "health": pool.health
                        })
                    );
                }
            }
        }

        Ok(Some(serde_json::json!(results)))
    }

    /// Run quota check job
    async fn run_quota_check_job(job: &ScheduledJob) -> Result<Option<serde_json::Value>> {
        use crate::nas::storage::quotas;

        let threshold_percent = job.params.get("threshold")
            .and_then(|v| v.as_u64())
            .unwrap_or(90);

        let usages = quotas::list_quota_usage(Some(&job.target)).await?;

        let violations: Vec<_> = usages.iter()
            .filter(|u| {
                if let (Some(used), Some(quota)) = (u.space_used, u.quota_bytes) {
                    let percent = (used as f64 / quota as f64) * 100.0;
                    percent >= threshold_percent as f64
                } else {
                    false
                }
            })
            .collect();

        Ok(Some(serde_json::json!({
            "checked": usages.len(),
            "violations": violations.len(),
            "threshold_percent": threshold_percent
        })))
    }

    /// Run SMART check job
    async fn run_smart_check_job(job: &ScheduledJob) -> Result<Option<serde_json::Value>> {
        use tokio::process::Command;

        let device = &job.target;

        let output = Command::new("smartctl")
            .args(["-H", device])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("smartctl failed: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let healthy = stdout.contains("PASSED") || stdout.contains("OK");

        Ok(Some(serde_json::json!({
            "device": device,
            "healthy": healthy,
            "output": stdout.to_string()
        })))
    }

    /// Run custom job (executes a script)
    async fn run_custom_job(job: &ScheduledJob) -> Result<Option<serde_json::Value>> {
        use tokio::process::Command;

        let script = job.params.get("script")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::Validation("Custom job requires 'script' parameter".to_string()))?;

        let output = Command::new("sh")
            .args(["-c", script])
            .env("HORCRUX_JOB_ID", &job.id)
            .env("HORCRUX_JOB_NAME", &job.name)
            .env("HORCRUX_JOB_TARGET", &job.target)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Script execution failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Script failed: {}", stderr)));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(Some(serde_json::json!({
            "exit_code": output.status.code(),
            "stdout": stdout.to_string()
        })))
    }
}

impl Default for NasScheduler {
    fn default() -> Self {
        Self::new()
    }
}

/// Create predefined snapshot schedules
pub fn create_snapshot_schedules(dataset: &str) -> Vec<ScheduledJob> {
    vec![
        ScheduledJob {
            id: uuid::Uuid::new_v4().to_string(),
            name: format!("{} hourly snapshots", dataset),
            job_type: JobType::Snapshot,
            schedule: "0 * * * *".to_string(),
            target: dataset.to_string(),
            params: {
                let mut p = HashMap::new();
                p.insert("prefix".to_string(), serde_json::json!("hourly"));
                p.insert("recursive".to_string(), serde_json::json!(false));
                p
            },
            enabled: true,
            priority: 50,
            ..Default::default()
        },
        ScheduledJob {
            id: uuid::Uuid::new_v4().to_string(),
            name: format!("{} daily snapshots", dataset),
            job_type: JobType::Snapshot,
            schedule: "0 0 * * *".to_string(),
            target: dataset.to_string(),
            params: {
                let mut p = HashMap::new();
                p.insert("prefix".to_string(), serde_json::json!("daily"));
                p.insert("recursive".to_string(), serde_json::json!(false));
                p
            },
            enabled: true,
            priority: 50,
            ..Default::default()
        },
        ScheduledJob {
            id: uuid::Uuid::new_v4().to_string(),
            name: format!("{} weekly snapshots", dataset),
            job_type: JobType::Snapshot,
            schedule: "0 0 * * 0".to_string(),
            target: dataset.to_string(),
            params: {
                let mut p = HashMap::new();
                p.insert("prefix".to_string(), serde_json::json!("weekly"));
                p.insert("recursive".to_string(), serde_json::json!(false));
                p
            },
            enabled: true,
            priority: 50,
            ..Default::default()
        },
    ]
}

/// Create retention cleanup schedule
pub fn create_retention_schedule(
    dataset: &str,
    hourly: Option<u32>,
    daily: Option<u32>,
    weekly: Option<u32>,
    monthly: Option<u32>,
) -> ScheduledJob {
    ScheduledJob {
        id: uuid::Uuid::new_v4().to_string(),
        name: format!("{} retention cleanup", dataset),
        job_type: JobType::RetentionCleanup,
        schedule: "30 * * * *".to_string(), // Run at :30 each hour
        target: dataset.to_string(),
        params: {
            let mut p = HashMap::new();
            if let Some(v) = hourly {
                p.insert("keep_hourly".to_string(), serde_json::json!(v));
            }
            if let Some(v) = daily {
                p.insert("keep_daily".to_string(), serde_json::json!(v));
            }
            if let Some(v) = weekly {
                p.insert("keep_weekly".to_string(), serde_json::json!(v));
            }
            if let Some(v) = monthly {
                p.insert("keep_monthly".to_string(), serde_json::json!(v));
            }
            p
        },
        enabled: true,
        priority: 100,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cron_parse_simple() {
        let cron = CronSchedule::parse("0 * * * *").unwrap();
        assert_eq!(cron.minutes, vec![0]);
        assert_eq!(cron.hours.len(), 24);
    }

    #[test]
    fn test_cron_parse_range() {
        let cron = CronSchedule::parse("0 9-17 * * *").unwrap();
        assert_eq!(cron.hours, vec![9, 10, 11, 12, 13, 14, 15, 16, 17]);
    }

    #[test]
    fn test_cron_parse_step() {
        let cron = CronSchedule::parse("*/15 * * * *").unwrap();
        assert_eq!(cron.minutes, vec![0, 15, 30, 45]);
    }

    #[test]
    fn test_cron_parse_list() {
        let cron = CronSchedule::parse("0 8,12,18 * * *").unwrap();
        assert_eq!(cron.hours, vec![8, 12, 18]);
    }

    #[test]
    fn test_cron_next_run() {
        let cron = CronSchedule::parse("0 * * * *").unwrap();
        let now = Utc::now();
        let next = cron.next_run_after(&now);
        assert!(next.is_some());
        assert!(next.unwrap() > now);
    }

    #[test]
    fn test_job_default() {
        let job = ScheduledJob::default();
        assert!(job.enabled);
        assert_eq!(job.priority, 100);
        assert_eq!(job.timeout_secs, 3600);
    }

    #[tokio::test]
    async fn test_scheduler_add_job() {
        let scheduler = NasScheduler::new();
        let job = ScheduledJob {
            name: "Test job".to_string(),
            schedule: "0 * * * *".to_string(),
            target: "tank/data".to_string(),
            ..Default::default()
        };

        let id = scheduler.add_job(job).await.unwrap();
        assert!(!id.is_empty());

        let retrieved = scheduler.get_job(&id).await;
        assert!(retrieved.is_some());
    }
}
