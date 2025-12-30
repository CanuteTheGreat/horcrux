//! Replication management
//!
//! Handles ZFS send/receive, Btrfs send/receive, and rsync-based replication.
//! Supports multiple transports, bandwidth limiting, and progress tracking.

use horcrux_common::{Error, Result};
use crate::nas::storage::{ReplicationDirection, ReplicationTask, ReplicationTransport, RetentionPolicy, StorageType};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tokio::io::{AsyncBufReadExt, BufReader};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Extended replication task configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtendedReplicationTask {
    /// Base task
    #[serde(flatten)]
    pub base: ReplicationTask,
    /// Storage type for source
    pub source_type: StorageType,
    /// SSH configuration
    pub ssh_config: Option<SshConfig>,
    /// Rsync configuration (for non-ZFS)
    pub rsync_config: Option<RsyncConfig>,
    /// Whether to use resumable transfers
    pub resumable: bool,
    /// Send raw encrypted blocks (ZFS)
    pub raw: bool,
    /// Include properties in replication
    pub properties: bool,
    /// Use bookmarks for incremental (ZFS)
    pub use_bookmarks: bool,
    /// Pre-replication script
    pub pre_script: Option<String>,
    /// Post-replication script
    pub post_script: Option<String>,
    /// Verify after replication
    pub verify: bool,
    /// Max retries on failure
    pub max_retries: u32,
    /// Retry delay in seconds
    pub retry_delay: u32,
    /// Alert on failure
    pub alert_on_failure: bool,
    /// Alert email
    pub alert_email: Option<String>,
    /// Last successful snapshot replicated
    pub last_snapshot: Option<String>,
    /// Estimated bytes for next replication
    pub estimated_bytes: Option<u64>,
}

impl Default for ExtendedReplicationTask {
    fn default() -> Self {
        Self {
            base: ReplicationTask {
                id: String::new(),
                name: String::new(),
                source_dataset: String::new(),
                target_host: String::new(),
                target_dataset: String::new(),
                direction: ReplicationDirection::Push,
                transport: ReplicationTransport::Ssh,
                schedule: "0 * * * *".to_string(),
                recursive: true,
                retention: Some(RetentionPolicy::default()),
                compression: true,
                bandwidth_limit: None,
                enabled: true,
                last_run: None,
                last_status: None,
                created_at: chrono::Utc::now().timestamp(),
            },
            source_type: StorageType::Zfs,
            ssh_config: None,
            rsync_config: None,
            resumable: true,
            raw: false,
            properties: true,
            use_bookmarks: true,
            pre_script: None,
            post_script: None,
            verify: false,
            max_retries: 3,
            retry_delay: 60,
            alert_on_failure: false,
            alert_email: None,
            last_snapshot: None,
            estimated_bytes: None,
        }
    }
}

/// SSH connection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshConfig {
    /// SSH port (default 22)
    pub port: u16,
    /// SSH username
    pub username: String,
    /// SSH identity file (private key)
    pub identity_file: Option<String>,
    /// SSH options
    pub options: HashMap<String, String>,
    /// Connection timeout in seconds
    pub connect_timeout: u32,
    /// Cipher to use
    pub cipher: Option<String>,
    /// Compression
    pub compression: bool,
    /// Control master for connection reuse
    pub control_master: bool,
    /// Control path
    pub control_path: Option<String>,
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            port: 22,
            username: "root".to_string(),
            identity_file: None,
            options: HashMap::new(),
            connect_timeout: 30,
            cipher: None,
            compression: true,
            control_master: true,
            control_path: Some("/tmp/horcrux-ssh-%r@%h:%p".to_string()),
        }
    }
}

impl SshConfig {
    /// Build SSH command arguments
    pub fn build_args(&self, host: &str) -> Vec<String> {
        let mut args = vec![
            "-p".to_string(), self.port.to_string(),
            "-o".to_string(), format!("ConnectTimeout={}", self.connect_timeout),
            "-o".to_string(), "BatchMode=yes".to_string(),
            "-o".to_string(), "StrictHostKeyChecking=accept-new".to_string(),
        ];

        if let Some(ref identity) = self.identity_file {
            args.push("-i".to_string());
            args.push(identity.clone());
        }

        if self.compression {
            args.push("-C".to_string());
        }

        if let Some(ref cipher) = self.cipher {
            args.push("-c".to_string());
            args.push(cipher.clone());
        }

        if self.control_master {
            args.push("-o".to_string());
            args.push("ControlMaster=auto".to_string());
            if let Some(ref path) = self.control_path {
                args.push("-o".to_string());
                args.push(format!("ControlPath={}", path));
            }
            args.push("-o".to_string());
            args.push("ControlPersist=60".to_string());
        }

        for (key, value) in &self.options {
            args.push("-o".to_string());
            args.push(format!("{}={}", key, value));
        }

        // Add user@host
        args.push(format!("{}@{}", self.username, host));

        args
    }
}

/// Rsync configuration for file-based replication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsyncConfig {
    /// Archive mode (-a)
    pub archive: bool,
    /// Verbose output (-v)
    pub verbose: bool,
    /// Show progress (--progress)
    pub progress: bool,
    /// Delete extraneous files (--delete)
    pub delete: bool,
    /// Preserve hard links (-H)
    pub hard_links: bool,
    /// Preserve ACLs (-A)
    pub acls: bool,
    /// Preserve extended attributes (-X)
    pub xattrs: bool,
    /// Sparse file handling (-S)
    pub sparse: bool,
    /// Compress during transfer (-z)
    pub compress: bool,
    /// Checksum files (--checksum)
    pub checksum: bool,
    /// Exclude patterns
    pub exclude: Vec<String>,
    /// Include patterns
    pub include: Vec<String>,
    /// Partial transfers (--partial)
    pub partial: bool,
    /// Partial directory
    pub partial_dir: Option<String>,
    /// Bandwidth limit in KB/s
    pub bwlimit: Option<u32>,
    /// SSH command
    pub ssh_command: Option<String>,
    /// Custom rsync path on remote
    pub rsync_path: Option<String>,
    /// Numeric IDs (--numeric-ids)
    pub numeric_ids: bool,
    /// Inplace updates (--inplace)
    pub inplace: bool,
    /// Whole file transfer (--whole-file)
    pub whole_file: bool,
    /// Filter rules
    pub filter: Vec<String>,
    /// Itemize changes (-i)
    pub itemize: bool,
    /// Log file
    pub log_file: Option<String>,
}

impl Default for RsyncConfig {
    fn default() -> Self {
        Self {
            archive: true,
            verbose: false,
            progress: true,
            delete: true,
            hard_links: true,
            acls: true,
            xattrs: true,
            sparse: true,
            compress: true,
            checksum: false,
            exclude: vec![],
            include: vec![],
            partial: true,
            partial_dir: Some(".rsync-partial".to_string()),
            bwlimit: None,
            ssh_command: None,
            rsync_path: None,
            numeric_ids: true,
            inplace: false,
            whole_file: false,
            filter: vec![],
            itemize: true,
            log_file: None,
        }
    }
}

impl RsyncConfig {
    /// Build rsync command arguments
    pub fn build_args(&self) -> Vec<String> {
        let mut args = vec![];

        if self.archive {
            args.push("-a".to_string());
        }

        if self.verbose {
            args.push("-v".to_string());
        }

        if self.progress {
            args.push("--progress".to_string());
        }

        if self.delete {
            args.push("--delete".to_string());
        }

        if self.hard_links {
            args.push("-H".to_string());
        }

        if self.acls {
            args.push("-A".to_string());
        }

        if self.xattrs {
            args.push("-X".to_string());
        }

        if self.sparse {
            args.push("-S".to_string());
        }

        if self.compress {
            args.push("-z".to_string());
        }

        if self.checksum {
            args.push("--checksum".to_string());
        }

        if self.partial {
            args.push("--partial".to_string());
            if let Some(ref dir) = self.partial_dir {
                args.push(format!("--partial-dir={}", dir));
            }
        }

        if let Some(limit) = self.bwlimit {
            args.push(format!("--bwlimit={}", limit));
        }

        if let Some(ref ssh) = self.ssh_command {
            args.push("-e".to_string());
            args.push(ssh.clone());
        }

        if let Some(ref path) = self.rsync_path {
            args.push(format!("--rsync-path={}", path));
        }

        if self.numeric_ids {
            args.push("--numeric-ids".to_string());
        }

        if self.inplace {
            args.push("--inplace".to_string());
        }

        if self.whole_file {
            args.push("--whole-file".to_string());
        }

        if self.itemize {
            args.push("-i".to_string());
        }

        for pattern in &self.exclude {
            args.push(format!("--exclude={}", pattern));
        }

        for pattern in &self.include {
            args.push(format!("--include={}", pattern));
        }

        for rule in &self.filter {
            args.push(format!("--filter={}", rule));
        }

        if let Some(ref log) = self.log_file {
            args.push(format!("--log-file={}", log));
        }

        args
    }
}

/// Replication progress information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationProgress {
    /// Task ID
    pub task_id: String,
    /// Current state
    pub state: ReplicationState,
    /// Bytes transferred
    pub bytes_transferred: u64,
    /// Total bytes (if known)
    pub bytes_total: Option<u64>,
    /// Transfer rate in bytes per second
    pub rate_bytes_per_sec: u64,
    /// Elapsed time in seconds
    pub elapsed_secs: u64,
    /// Estimated time remaining in seconds
    pub eta_secs: Option<u64>,
    /// Current file/snapshot being processed
    pub current_item: Option<String>,
    /// Files transferred (rsync)
    pub files_transferred: Option<u64>,
    /// Total files (rsync)
    pub files_total: Option<u64>,
    /// Percentage complete
    pub percent: f64,
    /// Error message if any
    pub error: Option<String>,
    /// Start timestamp
    pub started_at: i64,
}

/// Replication state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplicationState {
    /// Waiting to start
    Pending,
    /// Running pre-script
    PreScript,
    /// Calculating/estimating
    Estimating,
    /// Sending data
    Sending,
    /// Receiving data
    Receiving,
    /// Running post-script
    PostScript,
    /// Verifying
    Verifying,
    /// Completed successfully
    Completed,
    /// Failed
    Failed,
    /// Cancelled
    Cancelled,
}

/// Replication history entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationHistoryEntry {
    /// Entry ID
    pub id: String,
    /// Task ID
    pub task_id: String,
    /// Task name
    pub task_name: String,
    /// Start timestamp
    pub started_at: i64,
    /// End timestamp
    pub ended_at: Option<i64>,
    /// Duration in seconds
    pub duration_secs: u64,
    /// Bytes transferred
    pub bytes_transferred: u64,
    /// Success
    pub success: bool,
    /// Error message if failed
    pub error: Option<String>,
    /// Source snapshot used
    pub source_snapshot: String,
    /// Incremental from snapshot
    pub incremental_from: Option<String>,
    /// Whether it was resumable
    pub resumed: bool,
    /// Retries needed
    pub retries: u32,
    /// Average rate in bytes/sec
    pub avg_rate: u64,
}

/// Replication manager
pub struct ReplicationManager {
    /// Active replications (task_id -> progress)
    active: Arc<RwLock<HashMap<String, ReplicationProgress>>>,
    /// History
    history: Arc<RwLock<Vec<ReplicationHistoryEntry>>>,
    /// Max history entries
    max_history: usize,
}

impl ReplicationManager {
    /// Create a new replication manager
    pub fn new() -> Self {
        Self {
            active: Arc::new(RwLock::new(HashMap::new())),
            history: Arc::new(RwLock::new(Vec::new())),
            max_history: 1000,
        }
    }

    /// Get active replications
    pub async fn get_active(&self) -> Vec<ReplicationProgress> {
        self.active.read().await.values().cloned().collect()
    }

    /// Get progress for a specific task
    pub async fn get_progress(&self, task_id: &str) -> Option<ReplicationProgress> {
        self.active.read().await.get(task_id).cloned()
    }

    /// Get replication history
    pub async fn get_history(&self, limit: usize) -> Vec<ReplicationHistoryEntry> {
        let history = self.history.read().await;
        history.iter().rev().take(limit).cloned().collect()
    }

    /// Get history for a specific task
    pub async fn get_task_history(&self, task_id: &str, limit: usize) -> Vec<ReplicationHistoryEntry> {
        let history = self.history.read().await;
        history
            .iter()
            .filter(|e| e.task_id == task_id)
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }

    /// Run a replication task
    pub async fn run_task(&self, task: &ExtendedReplicationTask) -> Result<ReplicationHistoryEntry> {
        let task_id = task.base.id.clone();
        let start_time = chrono::Utc::now().timestamp();

        // Initialize progress
        let progress = ReplicationProgress {
            task_id: task_id.clone(),
            state: ReplicationState::Pending,
            bytes_transferred: 0,
            bytes_total: task.estimated_bytes,
            rate_bytes_per_sec: 0,
            elapsed_secs: 0,
            eta_secs: None,
            current_item: None,
            files_transferred: None,
            files_total: None,
            percent: 0.0,
            error: None,
            started_at: start_time,
        };

        self.active.write().await.insert(task_id.clone(), progress);

        // Run with retries
        let mut last_error = None;
        let mut retries = 0;
        let mut resumed = false;

        for attempt in 0..=task.max_retries {
            if attempt > 0 {
                retries = attempt;
                tokio::time::sleep(tokio::time::Duration::from_secs(task.retry_delay as u64)).await;
            }

            match self.execute_replication(task, attempt > 0).await {
                Ok(bytes) => {
                    // Success
                    let end_time = chrono::Utc::now().timestamp();
                    let duration = (end_time - start_time) as u64;

                    // Remove from active
                    self.active.write().await.remove(&task_id);

                    // Create history entry
                    let entry = ReplicationHistoryEntry {
                        id: uuid::Uuid::new_v4().to_string(),
                        task_id: task_id.clone(),
                        task_name: task.base.name.clone(),
                        started_at: start_time,
                        ended_at: Some(end_time),
                        duration_secs: duration,
                        bytes_transferred: bytes,
                        success: true,
                        error: None,
                        source_snapshot: task.last_snapshot.clone().unwrap_or_default(),
                        incremental_from: None,
                        resumed,
                        retries,
                        avg_rate: if duration > 0 { bytes / duration } else { 0 },
                    };

                    // Add to history
                    let mut history = self.history.write().await;
                    history.push(entry.clone());
                    if history.len() > self.max_history {
                        history.remove(0);
                    }

                    return Ok(entry);
                }
                Err(e) => {
                    last_error = Some(e.to_string());
                    resumed = task.resumable;

                    // Update progress with error
                    if let Some(progress) = self.active.write().await.get_mut(&task_id) {
                        progress.state = ReplicationState::Failed;
                        progress.error = Some(last_error.clone().unwrap());
                    }
                }
            }
        }

        // All retries failed
        let end_time = chrono::Utc::now().timestamp();
        let duration = (end_time - start_time) as u64;

        // Remove from active
        self.active.write().await.remove(&task_id);

        // Create failed history entry
        let entry = ReplicationHistoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            task_id: task_id.clone(),
            task_name: task.base.name.clone(),
            started_at: start_time,
            ended_at: Some(end_time),
            duration_secs: duration,
            bytes_transferred: 0,
            success: false,
            error: last_error.clone(),
            source_snapshot: String::new(),
            incremental_from: None,
            resumed,
            retries,
            avg_rate: 0,
        };

        // Add to history
        let mut history = self.history.write().await;
        history.push(entry.clone());

        // Send alert if configured
        if task.alert_on_failure {
            if let Some(ref email) = task.alert_email {
                let _ = send_failure_alert(email, &task.base.name, last_error.as_deref().unwrap_or("Unknown error")).await;
            }
        }

        Err(Error::Internal(format!(
            "Replication failed after {} retries: {}",
            task.max_retries,
            last_error.unwrap_or_else(|| "Unknown error".to_string())
        )))
    }

    /// Execute the actual replication
    async fn execute_replication(&self, task: &ExtendedReplicationTask, is_retry: bool) -> Result<u64> {
        let task_id = task.base.id.clone();

        // Run pre-script
        if let Some(ref script) = task.pre_script {
            self.update_state(&task_id, ReplicationState::PreScript).await;
            run_script(script, &task.base).await?;
        }

        // Execute based on storage type
        let bytes = match task.source_type {
            StorageType::Zfs => {
                self.run_zfs_replication(task, is_retry).await?
            }
            StorageType::Btrfs => {
                self.run_btrfs_replication(task).await?
            }
            _ => {
                // Use rsync for other types
                self.run_rsync_replication(task).await?
            }
        };

        // Run post-script
        if let Some(ref script) = task.post_script {
            self.update_state(&task_id, ReplicationState::PostScript).await;
            run_script(script, &task.base).await?;
        }

        // Verify if requested
        if task.verify {
            self.update_state(&task_id, ReplicationState::Verifying).await;
            self.verify_replication(task).await?;
        }

        self.update_state(&task_id, ReplicationState::Completed).await;
        Ok(bytes)
    }

    /// Update progress state
    async fn update_state(&self, task_id: &str, state: ReplicationState) {
        if let Some(progress) = self.active.write().await.get_mut(task_id) {
            progress.state = state;
        }
    }

    /// Update progress bytes
    async fn update_progress(&self, task_id: &str, bytes: u64, rate: u64, item: Option<String>) {
        if let Some(progress) = self.active.write().await.get_mut(task_id) {
            progress.bytes_transferred = bytes;
            progress.rate_bytes_per_sec = rate;
            progress.current_item = item;
            progress.elapsed_secs = (chrono::Utc::now().timestamp() - progress.started_at) as u64;

            if let Some(total) = progress.bytes_total {
                if total > 0 {
                    progress.percent = (bytes as f64 / total as f64) * 100.0;
                    if rate > 0 {
                        progress.eta_secs = Some((total - bytes) / rate);
                    }
                }
            }
        }
    }

    /// Run ZFS send/receive replication
    #[cfg(feature = "nas-zfs")]
    async fn run_zfs_replication(&self, task: &ExtendedReplicationTask, is_retry: bool) -> Result<u64> {
        let task_id = task.base.id.clone();

        // Get latest snapshot
        self.update_state(&task_id, ReplicationState::Estimating).await;
        let latest_snapshot = get_latest_snapshot(&task.base.source_dataset).await?;

        // Get last replicated snapshot on target
        let last_replicated = match task.base.transport {
            ReplicationTransport::Local => {
                get_local_last_snapshot(&task.base.target_dataset).await.ok().flatten()
            }
            _ => {
                get_remote_last_snapshot(
                    &task.base.target_host,
                    &task.base.target_dataset,
                    task.ssh_config.as_ref(),
                ).await.ok().flatten()
            }
        };

        // Check for resume token if retry
        let resume_token = if is_retry && task.resumable {
            match task.base.transport {
                ReplicationTransport::Local => {
                    get_local_resume_token(&task.base.target_dataset).await.ok().flatten()
                }
                _ => {
                    get_remote_resume_token(
                        &task.base.target_host,
                        &task.base.target_dataset,
                        task.ssh_config.as_ref(),
                    ).await.ok().flatten()
                }
            }
        } else {
            None
        };

        // Estimate size
        let estimated_size = if resume_token.is_none() {
            estimate_zfs_send_size(
                &latest_snapshot,
                last_replicated.as_deref(),
                task.base.recursive,
            ).await.unwrap_or(0)
        } else {
            0 // Can't estimate resumable
        };

        // Update progress with estimate
        if let Some(progress) = self.active.write().await.get_mut(&task_id) {
            progress.bytes_total = Some(estimated_size);
            progress.current_item = Some(latest_snapshot.clone());
        }

        self.update_state(&task_id, ReplicationState::Sending).await;

        // Build zfs send command
        let send_cmd = build_zfs_send_command(
            &latest_snapshot,
            last_replicated.as_deref(),
            resume_token.as_deref(),
            task,
        );

        // Build receive command
        let recv_cmd = build_zfs_receive_command(&task.base.target_dataset, task.resumable);

        // Build full pipeline
        let bytes_transferred = match task.base.transport {
            ReplicationTransport::Local => {
                self.run_local_zfs_pipeline(&task_id, &send_cmd, &recv_cmd, task.base.bandwidth_limit).await?
            }
            ReplicationTransport::Ssh => {
                self.run_ssh_zfs_pipeline(
                    &task_id,
                    &send_cmd,
                    &recv_cmd,
                    &task.base.target_host,
                    task.ssh_config.as_ref(),
                    task.base.bandwidth_limit,
                ).await?
            }
            ReplicationTransport::Netcat => {
                self.run_netcat_zfs_pipeline(
                    &task_id,
                    &send_cmd,
                    &recv_cmd,
                    &task.base.target_host,
                    task.ssh_config.as_ref(),
                    task.base.bandwidth_limit,
                ).await?
            }
        };

        // Create bookmark for next incremental (if enabled)
        if task.use_bookmarks {
            let bookmark_name = format!("{}#horcrux_repl_{}",
                latest_snapshot.split('@').next().unwrap_or(&task.base.source_dataset),
                task.base.id.replace('-', "_")
            );
            let _ = create_zfs_bookmark(&latest_snapshot, &bookmark_name).await;
        }

        Ok(bytes_transferred)
    }

    #[cfg(not(feature = "nas-zfs"))]
    async fn run_zfs_replication(&self, _task: &ExtendedReplicationTask, _is_retry: bool) -> Result<u64> {
        Err(Error::Internal("ZFS support not enabled".to_string()))
    }

    /// Run local ZFS pipeline with progress tracking
    #[cfg(feature = "nas-zfs")]
    async fn run_local_zfs_pipeline(
        &self,
        task_id: &str,
        send_cmd: &str,
        recv_cmd: &str,
        bandwidth_limit: Option<u32>,
    ) -> Result<u64> {
        let throttle = if let Some(limit) = bandwidth_limit {
            format!(" | pv -q -L {}k", limit)
        } else {
            " | pv -f".to_string()
        };

        let full_cmd = format!("{}{} | {}", send_cmd, throttle, recv_cmd);

        let mut child = Command::new("sh")
            .args(["-c", &full_cmd])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| Error::Internal(format!("Failed to spawn replication: {}", e)))?;

        let stderr = child.stderr.take().unwrap();
        let mut reader = BufReader::new(stderr).lines();
        let mut total_bytes = 0u64;

        // Parse pv output for progress
        while let Ok(Some(line)) = reader.next_line().await {
            if let Some((bytes, rate)) = parse_pv_output(&line) {
                total_bytes = bytes;
                self.update_progress(task_id, bytes, rate, None).await;
            }
        }

        let status = child.wait().await
            .map_err(|e| Error::Internal(format!("Replication process error: {}", e)))?;

        if !status.success() {
            return Err(Error::Internal("ZFS replication failed".to_string()));
        }

        Ok(total_bytes)
    }

    #[cfg(not(feature = "nas-zfs"))]
    async fn run_local_zfs_pipeline(
        &self,
        _task_id: &str,
        _send_cmd: &str,
        _recv_cmd: &str,
        _bandwidth_limit: Option<u32>,
    ) -> Result<u64> {
        Err(Error::Internal("ZFS support not enabled".to_string()))
    }

    /// Run SSH ZFS pipeline
    #[cfg(feature = "nas-zfs")]
    async fn run_ssh_zfs_pipeline(
        &self,
        task_id: &str,
        send_cmd: &str,
        recv_cmd: &str,
        host: &str,
        ssh_config: Option<&SshConfig>,
        bandwidth_limit: Option<u32>,
    ) -> Result<u64> {
        let ssh_args = ssh_config
            .map(|c| c.build_args(host).join(" "))
            .unwrap_or_else(|| format!("root@{}", host));

        let throttle = if let Some(limit) = bandwidth_limit {
            format!(" | pv -q -L {}k", limit)
        } else {
            " | pv -f".to_string()
        };

        // mbuffer on both sides for network buffering
        let use_mbuffer = super::command_exists("mbuffer").await;

        let full_cmd = if use_mbuffer {
            format!(
                "{}{} | mbuffer -q -s 128k -m 128M | ssh {} 'mbuffer -q -s 128k -m 128M | {}'",
                send_cmd, throttle, ssh_args, recv_cmd
            )
        } else {
            format!("{}{} | ssh {} '{}'", send_cmd, throttle, ssh_args, recv_cmd)
        };

        let mut child = Command::new("sh")
            .args(["-c", &full_cmd])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| Error::Internal(format!("Failed to spawn replication: {}", e)))?;

        let stderr = child.stderr.take().unwrap();
        let mut reader = BufReader::new(stderr).lines();
        let mut total_bytes = 0u64;

        while let Ok(Some(line)) = reader.next_line().await {
            if let Some((bytes, rate)) = parse_pv_output(&line) {
                total_bytes = bytes;
                self.update_progress(task_id, bytes, rate, None).await;
            }
        }

        let status = child.wait().await
            .map_err(|e| Error::Internal(format!("Replication process error: {}", e)))?;

        if !status.success() {
            return Err(Error::Internal("ZFS SSH replication failed".to_string()));
        }

        Ok(total_bytes)
    }

    #[cfg(not(feature = "nas-zfs"))]
    async fn run_ssh_zfs_pipeline(
        &self,
        _task_id: &str,
        _send_cmd: &str,
        _recv_cmd: &str,
        _host: &str,
        _ssh_config: Option<&SshConfig>,
        _bandwidth_limit: Option<u32>,
    ) -> Result<u64> {
        Err(Error::Internal("ZFS support not enabled".to_string()))
    }

    /// Run netcat ZFS pipeline for LAN transfers
    #[cfg(feature = "nas-zfs")]
    async fn run_netcat_zfs_pipeline(
        &self,
        task_id: &str,
        send_cmd: &str,
        recv_cmd: &str,
        host: &str,
        ssh_config: Option<&SshConfig>,
        bandwidth_limit: Option<u32>,
    ) -> Result<u64> {
        let port = 9999;
        let ssh_args = ssh_config
            .map(|c| c.build_args(host).join(" "))
            .unwrap_or_else(|| format!("root@{}", host));

        // Start receiver on remote first
        let recv_start_cmd = format!(
            "ssh {} 'nc -l -p {} | {} &'",
            ssh_args, port, recv_cmd
        );

        let output = Command::new("sh")
            .args(["-c", &recv_start_cmd])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to start remote receiver: {}", e)))?;

        if !output.status.success() {
            return Err(Error::Internal("Failed to start remote netcat receiver".to_string()));
        }

        // Small delay for receiver to start
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // Send data via netcat
        let throttle = if let Some(limit) = bandwidth_limit {
            format!(" | pv -q -L {}k", limit)
        } else {
            " | pv -f".to_string()
        };

        let send_full_cmd = format!("{}{} | nc {} {}", send_cmd, throttle, host, port);

        let mut child = Command::new("sh")
            .args(["-c", &send_full_cmd])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| Error::Internal(format!("Failed to spawn sender: {}", e)))?;

        let stderr = child.stderr.take().unwrap();
        let mut reader = BufReader::new(stderr).lines();
        let mut total_bytes = 0u64;

        while let Ok(Some(line)) = reader.next_line().await {
            if let Some((bytes, rate)) = parse_pv_output(&line) {
                total_bytes = bytes;
                self.update_progress(task_id, bytes, rate, None).await;
            }
        }

        let status = child.wait().await
            .map_err(|e| Error::Internal(format!("Sender process error: {}", e)))?;

        if !status.success() {
            return Err(Error::Internal("ZFS netcat replication failed".to_string()));
        }

        Ok(total_bytes)
    }

    #[cfg(not(feature = "nas-zfs"))]
    async fn run_netcat_zfs_pipeline(
        &self,
        _task_id: &str,
        _send_cmd: &str,
        _recv_cmd: &str,
        _host: &str,
        _ssh_config: Option<&SshConfig>,
        _bandwidth_limit: Option<u32>,
    ) -> Result<u64> {
        Err(Error::Internal("ZFS support not enabled".to_string()))
    }

    /// Run Btrfs send/receive replication
    #[cfg(feature = "nas-btrfs")]
    async fn run_btrfs_replication(&self, task: &ExtendedReplicationTask) -> Result<u64> {
        let task_id = task.base.id.clone();

        // Get latest snapshot
        self.update_state(&task_id, ReplicationState::Estimating).await;

        // Find latest read-only snapshot in source
        let source_snapshots = list_btrfs_snapshots(&task.base.source_dataset).await?;
        let latest = source_snapshots
            .into_iter()
            .filter(|s| s.readonly)
            .max_by_key(|s| s.created_at)
            .ok_or_else(|| Error::NotFound("No read-only snapshots found".to_string()))?;

        // Get last replicated snapshot on target
        let target_snapshots = if task.base.transport == ReplicationTransport::Local {
            list_btrfs_snapshots(&task.base.target_dataset).await.ok()
        } else {
            list_remote_btrfs_snapshots(
                &task.base.target_host,
                &task.base.target_dataset,
                task.ssh_config.as_ref(),
            ).await.ok()
        };

        let parent = target_snapshots
            .and_then(|snaps| snaps.into_iter().max_by_key(|s| s.created_at))
            .map(|s| s.path);

        self.update_state(&task_id, ReplicationState::Sending).await;

        // Build btrfs send command
        let mut send_args = vec!["send".to_string()];
        if let Some(ref p) = parent {
            send_args.push("-p".to_string());
            send_args.push(p.clone());
        }
        send_args.push(latest.path.clone());

        let send_cmd = format!("btrfs {}", send_args.join(" "));

        // Build receive command
        let recv_cmd = format!("btrfs receive {}", task.base.target_dataset);

        // Execute based on transport
        let bytes_transferred = match task.base.transport {
            ReplicationTransport::Local => {
                self.run_local_btrfs_pipeline(&task_id, &send_cmd, &recv_cmd, task.base.bandwidth_limit).await?
            }
            ReplicationTransport::Ssh => {
                self.run_ssh_btrfs_pipeline(
                    &task_id,
                    &send_cmd,
                    &recv_cmd,
                    &task.base.target_host,
                    task.ssh_config.as_ref(),
                    task.base.bandwidth_limit,
                ).await?
            }
            ReplicationTransport::Netcat => {
                // Netcat for btrfs similar to ZFS
                self.run_ssh_btrfs_pipeline(
                    &task_id,
                    &send_cmd,
                    &recv_cmd,
                    &task.base.target_host,
                    task.ssh_config.as_ref(),
                    task.base.bandwidth_limit,
                ).await?
            }
        };

        Ok(bytes_transferred)
    }

    #[cfg(not(feature = "nas-btrfs"))]
    async fn run_btrfs_replication(&self, _task: &ExtendedReplicationTask) -> Result<u64> {
        Err(Error::Internal("Btrfs support not enabled".to_string()))
    }

    /// Run local Btrfs pipeline
    #[cfg(feature = "nas-btrfs")]
    async fn run_local_btrfs_pipeline(
        &self,
        task_id: &str,
        send_cmd: &str,
        recv_cmd: &str,
        bandwidth_limit: Option<u32>,
    ) -> Result<u64> {
        let throttle = if let Some(limit) = bandwidth_limit {
            format!(" | pv -q -L {}k", limit)
        } else {
            " | pv -f".to_string()
        };

        let full_cmd = format!("sudo {}{} | sudo {}", send_cmd, throttle, recv_cmd);

        let mut child = Command::new("sh")
            .args(["-c", &full_cmd])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| Error::Internal(format!("Failed to spawn btrfs replication: {}", e)))?;

        let stderr = child.stderr.take().unwrap();
        let mut reader = BufReader::new(stderr).lines();
        let mut total_bytes = 0u64;

        while let Ok(Some(line)) = reader.next_line().await {
            if let Some((bytes, rate)) = parse_pv_output(&line) {
                total_bytes = bytes;
                self.update_progress(task_id, bytes, rate, None).await;
            }
        }

        let status = child.wait().await
            .map_err(|e| Error::Internal(format!("Btrfs replication error: {}", e)))?;

        if !status.success() {
            return Err(Error::Internal("Btrfs replication failed".to_string()));
        }

        Ok(total_bytes)
    }

    #[cfg(not(feature = "nas-btrfs"))]
    async fn run_local_btrfs_pipeline(
        &self,
        _task_id: &str,
        _send_cmd: &str,
        _recv_cmd: &str,
        _bandwidth_limit: Option<u32>,
    ) -> Result<u64> {
        Err(Error::Internal("Btrfs support not enabled".to_string()))
    }

    /// Run SSH Btrfs pipeline
    #[cfg(feature = "nas-btrfs")]
    async fn run_ssh_btrfs_pipeline(
        &self,
        task_id: &str,
        send_cmd: &str,
        recv_cmd: &str,
        host: &str,
        ssh_config: Option<&SshConfig>,
        bandwidth_limit: Option<u32>,
    ) -> Result<u64> {
        let ssh_args = ssh_config
            .map(|c| c.build_args(host).join(" "))
            .unwrap_or_else(|| format!("root@{}", host));

        let throttle = if let Some(limit) = bandwidth_limit {
            format!(" | pv -q -L {}k", limit)
        } else {
            " | pv -f".to_string()
        };

        let full_cmd = format!("sudo {}{} | ssh {} 'sudo {}'", send_cmd, throttle, ssh_args, recv_cmd);

        let mut child = Command::new("sh")
            .args(["-c", &full_cmd])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| Error::Internal(format!("Failed to spawn btrfs SSH replication: {}", e)))?;

        let stderr = child.stderr.take().unwrap();
        let mut reader = BufReader::new(stderr).lines();
        let mut total_bytes = 0u64;

        while let Ok(Some(line)) = reader.next_line().await {
            if let Some((bytes, rate)) = parse_pv_output(&line) {
                total_bytes = bytes;
                self.update_progress(task_id, bytes, rate, None).await;
            }
        }

        let status = child.wait().await
            .map_err(|e| Error::Internal(format!("Btrfs SSH replication error: {}", e)))?;

        if !status.success() {
            return Err(Error::Internal("Btrfs SSH replication failed".to_string()));
        }

        Ok(total_bytes)
    }

    #[cfg(not(feature = "nas-btrfs"))]
    async fn run_ssh_btrfs_pipeline(
        &self,
        _task_id: &str,
        _send_cmd: &str,
        _recv_cmd: &str,
        _host: &str,
        _ssh_config: Option<&SshConfig>,
        _bandwidth_limit: Option<u32>,
    ) -> Result<u64> {
        Err(Error::Internal("Btrfs support not enabled".to_string()))
    }

    /// Run rsync-based replication
    async fn run_rsync_replication(&self, task: &ExtendedReplicationTask) -> Result<u64> {
        let task_id = task.base.id.clone();

        self.update_state(&task_id, ReplicationState::Sending).await;

        let rsync_config = task.rsync_config.clone().unwrap_or_default();
        let mut args = rsync_config.build_args();

        // Add source
        args.push(format!("{}/", task.base.source_dataset));

        // Add destination
        let dest = if task.base.transport == ReplicationTransport::Local {
            task.base.target_dataset.clone()
        } else {
            let ssh_config = task.ssh_config.clone().unwrap_or_default();
            let ssh_cmd = format!(
                "ssh -p {} -o BatchMode=yes",
                ssh_config.port
            );
            args.insert(0, "-e".to_string());
            args.insert(1, ssh_cmd);
            format!(
                "{}@{}:{}",
                ssh_config.username,
                task.base.target_host,
                task.base.target_dataset
            )
        };
        args.push(dest);

        // Add stats for output parsing
        args.push("--stats".to_string());

        let mut child = Command::new("rsync")
            .args(&args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| Error::Internal(format!("Failed to spawn rsync: {}", e)))?;

        let stdout = child.stdout.take().unwrap();
        let mut reader = BufReader::new(stdout).lines();
        let mut total_bytes = 0u64;
        let mut files_transferred = 0u64;

        while let Ok(Some(line)) = reader.next_line().await {
            // Parse rsync progress output
            if let Some((bytes, rate, files)) = parse_rsync_output(&line) {
                total_bytes = bytes;
                files_transferred = files;
                self.update_progress(&task_id, bytes, rate, None).await;
                if let Some(progress) = self.active.write().await.get_mut(&task_id) {
                    progress.files_transferred = Some(files_transferred);
                }
            }
        }

        let status = child.wait().await
            .map_err(|e| Error::Internal(format!("Rsync error: {}", e)))?;

        if !status.success() {
            // Rsync exit codes
            let code = status.code().unwrap_or(-1);
            let error = match code {
                1 => "Syntax or usage error",
                2 => "Protocol incompatibility",
                3 => "Errors selecting input/output files, dirs",
                4 => "Requested action not supported",
                5 => "Error starting client-server protocol",
                6 => "Daemon unable to append to log-file",
                10 => "Error in socket I/O",
                11 => "Error in file I/O",
                12 => "Error in rsync protocol data stream",
                13 => "Errors with program diagnostics",
                14 => "Error in IPC code",
                20 => "Received SIGUSR1 or SIGINT",
                21 => "Some error returned by waitpid()",
                22 => "Error allocating core memory buffers",
                23 => "Partial transfer due to error",
                24 => "Partial transfer due to vanished source files",
                25 => "The --max-delete limit stopped deletions",
                30 => "Timeout in data send/receive",
                35 => "Timeout waiting for daemon connection",
                _ => "Unknown error",
            };
            return Err(Error::Internal(format!("Rsync failed (exit {}): {}", code, error)));
        }

        Ok(total_bytes)
    }

    /// Verify replication completed correctly
    async fn verify_replication(&self, task: &ExtendedReplicationTask) -> Result<()> {
        match task.source_type {
            StorageType::Zfs => {
                // Compare latest snapshot written bytes
                #[cfg(feature = "nas-zfs")]
                {
                    let source_snap = get_latest_snapshot(&task.base.source_dataset).await?;
                    let source_written = get_zfs_snapshot_written(&source_snap).await?;

                    let target_snap = match task.base.transport {
                        ReplicationTransport::Local => {
                            get_latest_snapshot(&task.base.target_dataset).await?
                        }
                        _ => {
                            get_remote_latest_snapshot(
                                &task.base.target_host,
                                &task.base.target_dataset,
                                task.ssh_config.as_ref(),
                            ).await?
                        }
                    };

                    let target_written = match task.base.transport {
                        ReplicationTransport::Local => {
                            get_zfs_snapshot_written(&target_snap).await?
                        }
                        _ => {
                            get_remote_zfs_snapshot_written(
                                &task.base.target_host,
                                &target_snap,
                                task.ssh_config.as_ref(),
                            ).await?
                        }
                    };

                    if source_written != target_written {
                        return Err(Error::Internal(format!(
                            "Verification failed: source={} target={} bytes",
                            source_written, target_written
                        )));
                    }
                }
                #[cfg(not(feature = "nas-zfs"))]
                return Err(Error::Internal("ZFS not enabled".to_string()));
            }
            _ => {
                // For rsync, run with --dry-run --checksum to verify
                let rsync_config = task.rsync_config.clone().unwrap_or_default();
                let mut args = rsync_config.build_args();
                args.push("--dry-run".to_string());
                args.push("--checksum".to_string());
                args.push(format!("{}/", task.base.source_dataset));

                let dest = if task.base.transport == ReplicationTransport::Local {
                    task.base.target_dataset.clone()
                } else {
                    format!("root@{}:{}", task.base.target_host, task.base.target_dataset)
                };
                args.push(dest);

                let output = Command::new("rsync")
                    .args(&args)
                    .output()
                    .await
                    .map_err(|e| Error::Internal(format!("Verify failed: {}", e)))?;

                let stdout = String::from_utf8_lossy(&output.stdout);
                // If dry-run shows changes, verification failed
                if stdout.lines().any(|l| l.starts_with('>') || l.starts_with('<')) {
                    return Err(Error::Internal("Verification failed: files differ".to_string()));
                }
            }
        }

        Ok(())
    }

    /// Cancel a running replication
    pub async fn cancel(&self, task_id: &str) -> Result<()> {
        // Find and kill the process
        let output = Command::new("pkill")
            .args(["-f", &format!("horcrux.*{}", task_id)])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to cancel: {}", e)))?;

        // Update state
        if let Some(progress) = self.active.write().await.get_mut(task_id) {
            progress.state = ReplicationState::Cancelled;
        }

        Ok(())
    }
}

impl Default for ReplicationManager {
    fn default() -> Self {
        Self::new()
    }
}

// Helper functions

/// Build ZFS send command
#[cfg(feature = "nas-zfs")]
fn build_zfs_send_command(
    snapshot: &str,
    incremental_from: Option<&str>,
    resume_token: Option<&str>,
    task: &ExtendedReplicationTask,
) -> String {
    let mut args = vec!["zfs", "send"];

    // Resume from token if available
    if let Some(token) = resume_token {
        args.push("-t");
        args.push(token);
        return args.join(" ");
    }

    // Verbose for progress
    args.push("-v");

    // Recursive
    if task.base.recursive {
        args.push("-R");
    }

    // Properties
    if task.properties {
        args.push("-p");
    }

    // Raw (for encrypted datasets)
    if task.raw {
        args.push("-w");
    }

    // Compressed stream
    if task.base.compression {
        args.push("-c");
    }

    // Large blocks
    args.push("-L");

    // Embedded data
    args.push("-e");

    // Incremental
    if let Some(prev) = incremental_from {
        // Use -I for intermediate snapshots in recursive
        if task.base.recursive {
            args.push("-I");
        } else {
            args.push("-i");
        }
        args.push(prev);
    }

    args.push(snapshot);
    args.join(" ")
}

/// Build ZFS receive command
#[cfg(feature = "nas-zfs")]
fn build_zfs_receive_command(target: &str, resumable: bool) -> String {
    let mut args = vec!["zfs", "receive", "-F"];

    if resumable {
        args.push("-s"); // Save partially received state
    }

    args.push("-u"); // Don't mount
    args.push(target);

    args.join(" ")
}

/// Get latest ZFS snapshot
#[cfg(feature = "nas-zfs")]
async fn get_latest_snapshot(dataset: &str) -> Result<String> {
    let output = Command::new("zfs")
        .args([
            "list", "-H", "-t", "snapshot", "-r", "-s", "creation",
            "-o", "name", dataset,
        ])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("Failed to list snapshots: {}", e)))?;

    if !output.status.success() {
        return Err(Error::Internal("Failed to list snapshots".to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .last()
        .map(|s| s.to_string())
        .ok_or_else(|| Error::NotFound("No snapshots found".to_string()))
}

/// Get local last snapshot for dataset
#[cfg(feature = "nas-zfs")]
async fn get_local_last_snapshot(dataset: &str) -> Result<Option<String>> {
    let output = Command::new("zfs")
        .args([
            "list", "-H", "-t", "snapshot", "-r", "-s", "creation",
            "-o", "name", dataset,
        ])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("Failed to list snapshots: {}", e)))?;

    if !output.status.success() {
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().last().map(|s| s.to_string()))
}

/// Get remote last snapshot
#[cfg(feature = "nas-zfs")]
async fn get_remote_last_snapshot(
    host: &str,
    dataset: &str,
    ssh_config: Option<&SshConfig>,
) -> Result<Option<String>> {
    let ssh_args = ssh_config
        .map(|c| c.build_args(host))
        .unwrap_or_else(|| vec![format!("root@{}", host)]);

    let mut cmd = Command::new("ssh");
    for arg in &ssh_args {
        cmd.arg(arg);
    }
    cmd.args([
        "zfs", "list", "-H", "-t", "snapshot", "-r", "-s", "creation",
        "-o", "name", dataset,
    ]);

    let output = cmd.output().await
        .map_err(|e| Error::Internal(format!("Failed to query remote: {}", e)))?;

    if !output.status.success() {
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().last().map(|s| s.to_string()))
}

/// Get remote latest snapshot
#[cfg(feature = "nas-zfs")]
async fn get_remote_latest_snapshot(
    host: &str,
    dataset: &str,
    ssh_config: Option<&SshConfig>,
) -> Result<String> {
    get_remote_last_snapshot(host, dataset, ssh_config)
        .await?
        .ok_or_else(|| Error::NotFound("No remote snapshots found".to_string()))
}

/// Get local resume token
#[cfg(feature = "nas-zfs")]
async fn get_local_resume_token(dataset: &str) -> Result<Option<String>> {
    let output = Command::new("zfs")
        .args(["get", "-H", "-o", "value", "receive_resume_token", dataset])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("Failed to get resume token: {}", e)))?;

    if !output.status.success() {
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout == "-" || stdout.is_empty() {
        Ok(None)
    } else {
        Ok(Some(stdout))
    }
}

/// Get remote resume token
#[cfg(feature = "nas-zfs")]
async fn get_remote_resume_token(
    host: &str,
    dataset: &str,
    ssh_config: Option<&SshConfig>,
) -> Result<Option<String>> {
    let ssh_args = ssh_config
        .map(|c| c.build_args(host))
        .unwrap_or_else(|| vec![format!("root@{}", host)]);

    let mut cmd = Command::new("ssh");
    for arg in &ssh_args {
        cmd.arg(arg);
    }
    cmd.args(["zfs", "get", "-H", "-o", "value", "receive_resume_token", dataset]);

    let output = cmd.output().await
        .map_err(|e| Error::Internal(format!("Failed to get remote resume token: {}", e)))?;

    if !output.status.success() {
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout == "-" || stdout.is_empty() {
        Ok(None)
    } else {
        Ok(Some(stdout))
    }
}

/// Estimate ZFS send size
#[cfg(feature = "nas-zfs")]
async fn estimate_zfs_send_size(
    snapshot: &str,
    incremental_from: Option<&str>,
    recursive: bool,
) -> Result<u64> {
    let mut args = vec!["send", "-n", "-v", "-P"];

    if recursive {
        args.push("-R");
    }

    if let Some(prev) = incremental_from {
        args.push("-i");
        args.push(prev);
    }

    args.push(snapshot);

    let output = Command::new("zfs")
        .args(&args)
        .output()
        .await
        .map_err(|e| Error::Internal(format!("Failed to estimate size: {}", e)))?;

    // Parse size from parsable output
    let stderr = String::from_utf8_lossy(&output.stderr);
    for line in stderr.lines() {
        if line.starts_with("size") {
            if let Some(size_str) = line.split_whitespace().nth(1) {
                if let Ok(size) = size_str.parse::<u64>() {
                    return Ok(size);
                }
            }
        }
    }

    // Try parsing "total estimated size is X" format
    for line in stderr.lines() {
        if line.contains("total estimated size is") {
            if let Some(size_str) = line.split("is").nth(1) {
                if let Some(bytes) = super::parse_size(size_str.trim()) {
                    return Ok(bytes);
                }
            }
        }
    }

    Ok(0)
}

/// Create ZFS bookmark
#[cfg(feature = "nas-zfs")]
async fn create_zfs_bookmark(snapshot: &str, bookmark: &str) -> Result<()> {
    let output = Command::new("zfs")
        .args(["bookmark", snapshot, bookmark])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("Failed to create bookmark: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Internal(format!("Bookmark creation failed: {}", stderr)));
    }

    Ok(())
}

/// Get ZFS snapshot written bytes
#[cfg(feature = "nas-zfs")]
async fn get_zfs_snapshot_written(snapshot: &str) -> Result<u64> {
    let output = Command::new("zfs")
        .args(["get", "-H", "-p", "-o", "value", "written", snapshot])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("Failed to get written: {}", e)))?;

    if !output.status.success() {
        return Err(Error::Internal("Failed to get written property".to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.trim().parse::<u64>()
        .map_err(|_| Error::Internal("Failed to parse written value".to_string()))
}

/// Get remote ZFS snapshot written bytes
#[cfg(feature = "nas-zfs")]
async fn get_remote_zfs_snapshot_written(
    host: &str,
    snapshot: &str,
    ssh_config: Option<&SshConfig>,
) -> Result<u64> {
    let ssh_args = ssh_config
        .map(|c| c.build_args(host))
        .unwrap_or_else(|| vec![format!("root@{}", host)]);

    let mut cmd = Command::new("ssh");
    for arg in &ssh_args {
        cmd.arg(arg);
    }
    cmd.args(["zfs", "get", "-H", "-p", "-o", "value", "written", snapshot]);

    let output = cmd.output().await
        .map_err(|e| Error::Internal(format!("Failed to get remote written: {}", e)))?;

    if !output.status.success() {
        return Err(Error::Internal("Failed to get remote written".to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.trim().parse::<u64>()
        .map_err(|_| Error::Internal("Failed to parse remote written value".to_string()))
}

/// Btrfs snapshot info
#[derive(Debug, Clone)]
struct BtrfsSnapshot {
    path: String,
    readonly: bool,
    created_at: i64,
}

/// List Btrfs snapshots
#[cfg(feature = "nas-btrfs")]
async fn list_btrfs_snapshots(path: &str) -> Result<Vec<BtrfsSnapshot>> {
    let output = Command::new("btrfs")
        .args(["subvolume", "list", "-s", path])
        .output()
        .await
        .map_err(|e| Error::Internal(format!("Failed to list btrfs snapshots: {}", e)))?;

    if !output.status.success() {
        return Err(Error::Internal("Failed to list btrfs snapshots".to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut snapshots = Vec::new();

    for line in stdout.lines() {
        // Format: ID <id> gen <gen> cgen <cgen> top level <level> otime <time> path <path>
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 11 {
            let path_idx = parts.iter().position(|&p| p == "path").unwrap_or(0) + 1;
            if path_idx < parts.len() {
                snapshots.push(BtrfsSnapshot {
                    path: parts[path_idx..].join("/"),
                    readonly: true, // -s lists only snapshots
                    created_at: 0, // Would need btrfs subvolume show for this
                });
            }
        }
    }

    Ok(snapshots)
}

#[cfg(not(feature = "nas-btrfs"))]
async fn list_btrfs_snapshots(_path: &str) -> Result<Vec<BtrfsSnapshot>> {
    Err(Error::Internal("Btrfs not enabled".to_string()))
}

/// List remote Btrfs snapshots
#[cfg(feature = "nas-btrfs")]
async fn list_remote_btrfs_snapshots(
    host: &str,
    path: &str,
    ssh_config: Option<&SshConfig>,
) -> Result<Vec<BtrfsSnapshot>> {
    let ssh_args = ssh_config
        .map(|c| c.build_args(host))
        .unwrap_or_else(|| vec![format!("root@{}", host)]);

    let mut cmd = Command::new("ssh");
    for arg in &ssh_args {
        cmd.arg(arg);
    }
    cmd.args(["btrfs", "subvolume", "list", "-s", path]);

    let output = cmd.output().await
        .map_err(|e| Error::Internal(format!("Failed to list remote btrfs snapshots: {}", e)))?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut snapshots = Vec::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 11 {
            let path_idx = parts.iter().position(|&p| p == "path").unwrap_or(0) + 1;
            if path_idx < parts.len() {
                snapshots.push(BtrfsSnapshot {
                    path: parts[path_idx..].join("/"),
                    readonly: true,
                    created_at: 0,
                });
            }
        }
    }

    Ok(snapshots)
}

#[cfg(not(feature = "nas-btrfs"))]
async fn list_remote_btrfs_snapshots(
    _host: &str,
    _path: &str,
    _ssh_config: Option<&SshConfig>,
) -> Result<Vec<BtrfsSnapshot>> {
    Err(Error::Internal("Btrfs not enabled".to_string()))
}

/// Parse pv output for progress
fn parse_pv_output(line: &str) -> Option<(u64, u64)> {
    // pv output format: "123MiB 0:01:23 [45.6MiB/s]"
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 3 {
        return None;
    }

    let bytes = super::parse_size(parts[0])?;

    // Parse rate
    let rate_str = parts.get(2)?;
    let rate_str = rate_str.trim_start_matches('[').trim_end_matches(']').trim_end_matches("/s");
    let rate = super::parse_size(rate_str)?;

    Some((bytes, rate))
}

/// Parse rsync output for progress
fn parse_rsync_output(line: &str) -> Option<(u64, u64, u64)> {
    // Rsync progress: "1,234,567 100%   12.34MB/s    0:00:01"
    // Or itemize: ">f+++++++++ path/to/file"
    // Or stats: "Total transferred file size: 1,234,567 bytes"

    if line.contains("transferred file size:") {
        // Final stats line
        if let Some(size_str) = line.split(':').nth(1) {
            let size_str = size_str.trim().replace(',', "").split_whitespace().next()?;
            let bytes = size_str.parse::<u64>().ok()?;
            return Some((bytes, 0, 0));
        }
    }

    // Progress line
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() >= 4 && parts[1].ends_with('%') {
        let bytes_str = parts[0].replace(',', "");
        let bytes = bytes_str.parse::<u64>().ok()?;

        let rate_str = parts[2].trim_end_matches("/s");
        let rate = super::parse_size(rate_str).unwrap_or(0);

        return Some((bytes, rate, 0));
    }

    None
}

/// Run a pre/post script
async fn run_script(script: &str, task: &ReplicationTask) -> Result<()> {
    // Set environment variables for script
    let output = Command::new("sh")
        .args(["-c", script])
        .env("HORCRUX_TASK_ID", &task.id)
        .env("HORCRUX_TASK_NAME", &task.name)
        .env("HORCRUX_SOURCE_DATASET", &task.source_dataset)
        .env("HORCRUX_TARGET_HOST", &task.target_host)
        .env("HORCRUX_TARGET_DATASET", &task.target_dataset)
        .output()
        .await
        .map_err(|e| Error::Internal(format!("Script execution failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Internal(format!("Script failed: {}", stderr)));
    }

    Ok(())
}

/// Send failure alert email
async fn send_failure_alert(email: &str, task_name: &str, error: &str) -> Result<()> {
    let subject = format!("Horcrux Replication Failed: {}", task_name);
    let body = format!(
        "Replication task '{}' has failed.\n\nError: {}\n\nTimestamp: {}",
        task_name,
        error,
        chrono::Utc::now().to_rfc3339()
    );

    // Try sendmail first, then mail command
    let result = Command::new("sendmail")
        .args(["-t"])
        .stdin(std::process::Stdio::piped())
        .spawn();

    if let Ok(mut child) = result {
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let message = format!(
                "To: {}\nSubject: {}\n\n{}",
                email, subject, body
            );
            let _ = stdin.write_all(message.as_bytes()).await;
        }
        let _ = child.wait().await;
        return Ok(());
    }

    // Fallback to mail command
    let _ = Command::new("mail")
        .args(["-s", &subject, email])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map(|mut child| {
            if let Some(stdin) = child.stdin.take() {
                // Write body to stdin
                let _ = stdin;
            }
        });

    Ok(())
}

/// Test SSH connection to target
pub async fn test_ssh_connection(host: &str, ssh_config: Option<&SshConfig>) -> Result<bool> {
    let ssh_args = ssh_config
        .map(|c| c.build_args(host))
        .unwrap_or_else(|| vec![format!("root@{}", host)]);

    let mut cmd = Command::new("ssh");
    for arg in &ssh_args {
        cmd.arg(arg);
    }
    cmd.args(["echo", "ok"]);

    let output = cmd.output().await
        .map_err(|e| Error::Internal(format!("SSH connection test failed: {}", e)))?;

    Ok(output.status.success())
}

/// Check if ZFS is available on remote host
#[cfg(feature = "nas-zfs")]
pub async fn check_remote_zfs(host: &str, ssh_config: Option<&SshConfig>) -> Result<bool> {
    let ssh_args = ssh_config
        .map(|c| c.build_args(host))
        .unwrap_or_else(|| vec![format!("root@{}", host)]);

    let mut cmd = Command::new("ssh");
    for arg in &ssh_args {
        cmd.arg(arg);
    }
    cmd.args(["which", "zfs"]);

    let output = cmd.output().await
        .map_err(|e| Error::Internal(format!("Remote ZFS check failed: {}", e)))?;

    Ok(output.status.success())
}

/// Check remote disk space
pub async fn check_remote_space(
    host: &str,
    path: &str,
    ssh_config: Option<&SshConfig>,
) -> Result<(u64, u64)> {
    let ssh_args = ssh_config
        .map(|c| c.build_args(host))
        .unwrap_or_else(|| vec![format!("root@{}", host)]);

    let mut cmd = Command::new("ssh");
    for arg in &ssh_args {
        cmd.arg(arg);
    }
    cmd.args(["df", "-B1", path]);

    let output = cmd.output().await
        .map_err(|e| Error::Internal(format!("Remote space check failed: {}", e)))?;

    if !output.status.success() {
        return Err(Error::Internal("Failed to check remote space".to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 4 {
            let total = parts[1].parse::<u64>().unwrap_or(0);
            let available = parts[3].parse::<u64>().unwrap_or(0);
            return Ok((total, available));
        }
    }

    Err(Error::Internal("Failed to parse remote space".to_string()))
}

// Legacy function for backward compatibility
/// Run a replication task (legacy API)
pub async fn run_replication(task: &ReplicationTask) -> Result<()> {
    let extended = ExtendedReplicationTask {
        base: task.clone(),
        source_type: StorageType::Zfs,
        ssh_config: Some(SshConfig::default()),
        rsync_config: None,
        resumable: true,
        raw: false,
        properties: true,
        use_bookmarks: true,
        pre_script: None,
        post_script: None,
        verify: false,
        max_retries: 3,
        retry_delay: 60,
        alert_on_failure: false,
        alert_email: None,
        last_snapshot: None,
        estimated_bytes: None,
    };

    let manager = ReplicationManager::new();
    manager.run_task(&extended).await?;
    Ok(())
}

/// Apply retention policy to snapshots (legacy API)
pub async fn apply_retention(dataset: &str, policy: &RetentionPolicy) -> Result<u32> {
    let snapshots = super::snapshots::list_snapshots(dataset).await?;
    let mut deleted = 0;

    let mut snapshots = snapshots;
    snapshots.sort_by_key(|s| s.created_at);

    let now = chrono::Utc::now().timestamp();
    let keep_threshold = policy.keep_days
        .map(|d| now - (d as i64 * 86400))
        .unwrap_or(0);

    for snapshot in &snapshots {
        if snapshot.hold {
            continue;
        }

        if snapshot.created_at >= keep_threshold {
            continue;
        }

        let age_days = (now - snapshot.created_at) / 86400;

        let should_keep = match age_days {
            0..=1 => policy.hourly.map(|_| true).unwrap_or(false),
            2..=7 => policy.daily.map(|_| true).unwrap_or(false),
            8..=30 => policy.weekly.map(|_| true).unwrap_or(false),
            31..=365 => policy.monthly.map(|_| true).unwrap_or(false),
            _ => policy.yearly.map(|_| true).unwrap_or(false),
        };

        if !should_keep {
            if super::snapshots::delete_snapshot(&snapshot.full_name).await.is_ok() {
                deleted += 1;
            }
        }
    }

    Ok(deleted)
}

/// Estimate replication size (legacy API)
#[cfg(feature = "nas-zfs")]
pub async fn estimate_replication_size(
    source: &str,
    incremental_from: Option<&str>,
) -> Result<u64> {
    estimate_zfs_send_size(source, incremental_from, false).await
}

#[cfg(not(feature = "nas-zfs"))]
pub async fn estimate_replication_size(
    _source: &str,
    _incremental_from: Option<&str>,
) -> Result<u64> {
    Err(Error::Internal("ZFS not enabled".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ssh_config_args() {
        let config = SshConfig {
            port: 2222,
            username: "backup".to_string(),
            identity_file: Some("/root/.ssh/backup_key".to_string()),
            compression: true,
            ..Default::default()
        };

        let args = config.build_args("192.168.1.100");
        assert!(args.contains(&"-p".to_string()));
        assert!(args.contains(&"2222".to_string()));
        assert!(args.contains(&"-i".to_string()));
        assert!(args.contains(&"-C".to_string()));
        assert!(args.iter().any(|a| a.contains("backup@192.168.1.100")));
    }

    #[test]
    fn test_rsync_config_args() {
        let config = RsyncConfig {
            archive: true,
            delete: true,
            compress: true,
            exclude: vec!["*.tmp".to_string(), ".cache".to_string()],
            bwlimit: Some(10000),
            ..Default::default()
        };

        let args = config.build_args();
        assert!(args.contains(&"-a".to_string()));
        assert!(args.contains(&"--delete".to_string()));
        assert!(args.contains(&"-z".to_string()));
        assert!(args.contains(&"--exclude=*.tmp".to_string()));
        assert!(args.contains(&"--exclude=.cache".to_string()));
        assert!(args.contains(&"--bwlimit=10000".to_string()));
    }

    #[test]
    fn test_parse_pv_output() {
        let line = "123MiB 0:01:23 [45.6MiB/s]";
        let result = parse_pv_output(line);
        assert!(result.is_some());
        let (bytes, rate) = result.unwrap();
        assert!(bytes > 0);
        assert!(rate > 0);
    }

    #[test]
    fn test_extended_replication_task_default() {
        let task = ExtendedReplicationTask::default();
        assert!(task.resumable);
        assert!(task.properties);
        assert!(task.use_bookmarks);
        assert_eq!(task.max_retries, 3);
    }

    #[test]
    fn test_replication_state_serialization() {
        let state = ReplicationState::Sending;
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, "\"sending\"");
    }
}
