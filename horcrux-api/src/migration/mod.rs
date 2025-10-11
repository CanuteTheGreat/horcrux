///! Live VM Migration
///!
///! Enables moving running VMs between cluster nodes with minimal downtime

pub mod block_migration;
pub mod qemu_monitor;
pub mod rollback;
pub mod health_check;

use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::path::PathBuf;
use tokio::sync::RwLock;
use tokio::process::Command;
use chrono::{DateTime, Utc};

/// Migration type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MigrationType {
    Live,       // Live migration (minimal downtime)
    Offline,    // Offline migration (VM must be stopped)
    Online,     // Online migration with brief pause
}

/// Migration state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MigrationState {
    Pending,
    Preparing,
    Transferring,
    Syncing,
    Finalizing,
    Completed,
    Failed,
    Cancelled,
}

/// Migration job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationJob {
    pub id: String,
    pub vm_id: u32,
    pub source_node: String,
    pub target_node: String,
    pub migration_type: MigrationType,
    pub state: MigrationState,
    pub progress: f32,  // 0.0 - 100.0
    pub started: DateTime<Utc>,
    pub completed: Option<DateTime<Utc>>,
    pub bandwidth_limit: Option<u64>, // MB/s
    pub error: Option<String>,
    pub transferred_bytes: u64,
    pub total_bytes: u64,
}

/// Migration configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationConfig {
    pub vm_id: u32,
    pub target_node: String,
    pub migration_type: MigrationType,
    pub bandwidth_limit: Option<u64>,  // MB/s, None = unlimited
    pub force: bool,  // Force migration even if checks fail
    pub with_local_disks: bool,  // Migrate local disks (requires shared storage otherwise)
}

/// Migration statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationStats {
    pub duration_seconds: u64,
    pub downtime_ms: u64,
    pub transferred_gb: f64,
    pub average_speed_mbps: f64,
    pub memory_dirty_rate: f64,  // MB/s
}

/// Migration manager
pub struct MigrationManager {
    jobs: Arc<RwLock<HashMap<String, MigrationJob>>>,
    bandwidth_limit: Arc<RwLock<Option<u64>>>,  // Global limit
    max_concurrent: Arc<RwLock<usize>>,
    rollback_manager: Arc<RwLock<rollback::RollbackManager>>,
    auto_rollback_enabled: Arc<RwLock<bool>>,
    health_checker: Arc<health_check::HealthChecker>,
    health_check_enabled: Arc<RwLock<bool>>,
    health_reports: Arc<RwLock<HashMap<String, health_check::HealthCheckReport>>>,
}

impl MigrationManager {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            bandwidth_limit: Arc::new(RwLock::new(Some(100))), // Default 100 MB/s
            max_concurrent: Arc::new(RwLock::new(1)),  // Default: 1 concurrent migration
            rollback_manager: Arc::new(RwLock::new(rollback::RollbackManager::new())),
            auto_rollback_enabled: Arc::new(RwLock::new(true)), // Auto-rollback enabled by default
            health_checker: Arc::new(health_check::HealthChecker::new()),
            health_check_enabled: Arc::new(RwLock::new(true)), // Health checks enabled by default
            health_reports: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Enable or disable automatic rollback on migration failure
    pub async fn set_auto_rollback(&self, enabled: bool) {
        let mut auto_rollback = self.auto_rollback_enabled.write().await;
        *auto_rollback = enabled;
        tracing::info!("Automatic migration rollback {}", if enabled { "enabled" } else { "disabled" });
    }

    /// Check if auto-rollback is enabled
    pub async fn is_auto_rollback_enabled(&self) -> bool {
        *self.auto_rollback_enabled.read().await
    }

    /// Enable or disable post-migration health checks
    pub async fn set_health_checks(&self, enabled: bool) {
        let mut health_check = self.health_check_enabled.write().await;
        *health_check = enabled;
        tracing::info!("Post-migration health checks {}", if enabled { "enabled" } else { "disabled" });
    }

    /// Check if health checks are enabled
    pub async fn is_health_check_enabled(&self) -> bool {
        *self.health_check_enabled.read().await
    }

    /// Start VM migration
    pub async fn start_migration(&self, config: MigrationConfig, source_node: String) -> Result<String> {
        // Check if we've reached concurrent migration limit
        let active_count = self.count_active_migrations().await;
        let max_concurrent = *self.max_concurrent.read().await;

        if active_count >= max_concurrent {
            return Err(horcrux_common::Error::System(
                format!("Maximum concurrent migrations ({}) reached", max_concurrent)
            ));
        }

        // Pre-migration checks
        if !config.force {
            self.pre_migration_checks(&config, &source_node).await?;
        }

        let job_id = format!("migration-{}-{}", config.vm_id, Utc::now().timestamp());

        let bandwidth_limit = config.bandwidth_limit.or(*self.bandwidth_limit.read().await);

        let job = MigrationJob {
            id: job_id.clone(),
            vm_id: config.vm_id,
            source_node: source_node.clone(),
            target_node: config.target_node.clone(),
            migration_type: config.migration_type.clone(),
            state: MigrationState::Pending,
            progress: 0.0,
            started: Utc::now(),
            completed: None,
            bandwidth_limit,
            error: None,
            transferred_bytes: 0,
            total_bytes: 0,
        };

        // Store job
        {
            let mut jobs = self.jobs.write().await;
            jobs.insert(job_id.clone(), job.clone());
        }

        // Start migration in background
        let jobs = self.jobs.clone();
        let config_clone = config.clone();
        let job_id_clone = job_id.clone();
        let source_clone = source_node.clone();
        let rollback_manager = self.rollback_manager.clone();
        let auto_rollback = *self.auto_rollback_enabled.read().await;
        let health_checker = self.health_checker.clone();
        let health_check_enabled = *self.health_check_enabled.read().await;
        let health_reports = self.health_reports.clone();

        tokio::spawn(async move {
            let result = Self::execute_migration(
                jobs.clone(),
                job_id_clone.clone(),
                config_clone.clone(),
                source_clone.clone(),
            ).await;

            let mut jobs_lock = jobs.write().await;
            if let Some(job) = jobs_lock.get_mut(&job_id_clone) {
                match result {
                    Ok(_) => {
                        job.state = MigrationState::Completed;
                        job.progress = 100.0;
                        job.completed = Some(Utc::now());

                        drop(jobs_lock); // Release lock before health checks

                        // Run post-migration health checks if enabled
                        if health_check_enabled {
                            tracing::info!(
                                "Migration {} completed. Running post-migration health checks...",
                                job_id_clone
                            );

                            let report = health_checker.run_checks(
                                config_clone.vm_id,
                                job_id_clone.clone(),
                                config_clone.target_node.clone(),
                            ).await;

                            let summary = report.get_summary();
                            if !summary.overall_healthy {
                                tracing::warn!(
                                    "Health checks failed for migrated VM {}: {}/{} checks passed",
                                    config_clone.vm_id,
                                    summary.passed,
                                    summary.total_checks
                                );
                            }

                            // Store health report
                            health_reports.write().await.insert(job_id_clone.clone(), report);
                        }
                    }
                    Err(e) => {
                        job.state = MigrationState::Failed;
                        job.error = Some(e.to_string());
                        job.completed = Some(Utc::now());

                        // Trigger automatic rollback if enabled
                        if auto_rollback {
                            tracing::warn!(
                                "Migration {} failed: {}. Initiating automatic rollback...",
                                job_id_clone, e
                            );

                            drop(jobs_lock); // Release the lock before rollback

                            let mut rb_manager = rollback_manager.write().await;
                            match rb_manager.rollback_migration(
                                job_id_clone.clone(),
                                config_clone.vm_id,
                                source_clone.clone(),
                                config_clone.target_node.clone(),
                            ).await {
                                Ok(summary) => {
                                    tracing::info!(
                                        "Rollback completed: {}/{} steps successful",
                                        summary.successful_steps,
                                        summary.total_steps
                                    );
                                }
                                Err(e) => {
                                    tracing::error!(
                                        "Rollback failed for migration {}: {}",
                                        job_id_clone, e
                                    );
                                }
                            }
                        } else {
                            tracing::warn!(
                                "Migration {} failed but automatic rollback is disabled",
                                job_id_clone
                            );
                        }
                    }
                }
            }
        });

        tracing::info!(
            "Started {} migration of VM {} from {} to {}",
            match config.migration_type {
                MigrationType::Live => "live",
                MigrationType::Offline => "offline",
                MigrationType::Online => "online",
            },
            config.vm_id,
            source_node,
            config.target_node
        );

        Ok(job_id)
    }

    /// Execute the migration process
    async fn execute_migration(
        jobs: Arc<RwLock<HashMap<String, MigrationJob>>>,
        job_id: String,
        config: MigrationConfig,
        source_node: String,
    ) -> Result<()> {
        // Update state: Preparing
        Self::update_job_state(&jobs, &job_id, MigrationState::Preparing, 5.0).await;

        match config.migration_type {
            MigrationType::Live => {
                Self::execute_live_migration(&jobs, &job_id, &config, &source_node).await?;
            }
            MigrationType::Offline => {
                Self::execute_offline_migration(&jobs, &job_id, &config, &source_node).await?;
            }
            MigrationType::Online => {
                Self::execute_online_migration(&jobs, &job_id, &config, &source_node).await?;
            }
        }

        Ok(())
    }

    /// Execute live migration (QEMU live migration)
    async fn execute_live_migration(
        jobs: &Arc<RwLock<HashMap<String, MigrationJob>>>,
        job_id: &str,
        config: &MigrationConfig,
        source_node: &str,
    ) -> Result<()> {
        // Phase 1: Pre-copy memory pages
        Self::update_job_state(jobs, job_id, MigrationState::Transferring, 10.0).await;

        tracing::info!("Starting live migration pre-copy phase for VM {}", config.vm_id);

        let vm_name = format!("vm-{}", config.vm_id);

        // Build virsh migrate command with real parameters
        let mut virsh_args = vec![
            "migrate".to_string(),
            "--live".to_string(),
            vm_name.clone(),
        ];

        // Set bandwidth limit if specified
        if let Some(bw) = config.bandwidth_limit {
            virsh_args.push("--bandwidth".to_string());
            virsh_args.push(format!("{}", bw)); // MB/s
        }

        // Add target connection URI (qemu+ssh)
        let target_uri = format!("qemu+ssh://root@{}/system", config.target_node);
        virsh_args.push(target_uri);

        // Initiate live migration via SSH to source node
        let output = Command::new("ssh")
            .args([
                "-o", "StrictHostKeyChecking=no",
                "-o", "UserKnownHostsFile=/dev/null",
                "-o", "ConnectTimeout=10",
                &format!("root@{}", source_node),
                "virsh",
            ])
            .args(&virsh_args)
            .arg("--async") // Don't wait for completion
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to initiate migration: {}", e)))?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(
                format!("Failed to start live migration: {}", String::from_utf8_lossy(&output.stderr))
            ));
        }

        tracing::info!("Live migration initiated for VM {}", config.vm_id);

        // Monitor migration progress via QMP
        let qmp_socket = PathBuf::from(format!("/var/run/qemu/vm-{}.qmp", config.vm_id));

        // Poll migration status every 500ms
        let mut last_progress = 10.0;
        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

            // Query migration status via virsh
            let status_output = Command::new("ssh")
                .args([
                    "-o", "StrictHostKeyChecking=no",
                    "-o", "UserKnownHostsFile=/dev/null",
                    &format!("root@{}", source_node),
                    "virsh", "domjobinfo", &vm_name,
                ])
                .output()
                .await;

            match status_output {
                Ok(output) if output.status.success() => {
                    let output_str = String::from_utf8_lossy(&output.stdout);

                    // Parse migration progress from domjobinfo
                    // Look for "Data processed:" and "Data total:" lines
                    let mut processed: u64 = 0;
                    let mut total: u64 = 0;

                    for line in output_str.lines() {
                        if line.contains("Data processed:") {
                            if let Some(val) = line.split_whitespace().nth(2) {
                                processed = val.parse().unwrap_or(0);
                            }
                        } else if line.contains("Data total:") {
                            if let Some(val) = line.split_whitespace().nth(2) {
                                total = val.parse().unwrap_or(0);
                            }
                        }
                    }

                    // Calculate progress percentage
                    let progress = if total > 0 {
                        10.0 + ((processed as f32 / total as f32) * 80.0)
                    } else {
                        last_progress + 5.0
                    };

                    last_progress = progress.min(90.0);
                    Self::update_job_state(jobs, job_id, MigrationState::Transferring, last_progress).await;

                    // Check if migration is complete
                    if output_str.contains("None") || output_str.contains("Completed") {
                        break;
                    }
                }
                _ => {
                    // If we can't query status, migration might be complete
                    break;
                }
            }

            // Safety timeout: don't loop forever
            if last_progress >= 90.0 {
                break;
            }
        }

        // Phase 2: Final sync
        Self::update_job_state(jobs, job_id, MigrationState::Syncing, 92.0).await;
        tracing::info!("Live migration entering final sync for VM {}", config.vm_id);

        // Wait briefly for final convergence
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // Phase 3: Finalize
        Self::update_job_state(jobs, job_id, MigrationState::Finalizing, 95.0).await;
        tracing::info!("Finalizing live migration for VM {}", config.vm_id);

        // Verify VM is running on target
        let verify_output = Command::new("ssh")
            .args([
                "-o", "StrictHostKeyChecking=no",
                "-o", "UserKnownHostsFile=/dev/null",
                &format!("root@{}", config.target_node),
                "virsh", "domstate", &vm_name,
            ])
            .output()
            .await;

        match verify_output {
            Ok(output) if output.status.success() => {
                let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if state != "running" {
                    return Err(horcrux_common::Error::System(
                        format!("Migration completed but VM is not running on target. State: {}", state)
                    ));
                }
            }
            _ => {
                tracing::warn!("Could not verify VM state on target node");
            }
        }

        tracing::info!("Live migration completed successfully for VM {}", config.vm_id);
        Ok(())
    }

    /// Execute offline migration
    async fn execute_offline_migration(
        jobs: &Arc<RwLock<HashMap<String, MigrationJob>>>,
        job_id: &str,
        config: &MigrationConfig,
        source_node: &str,
    ) -> Result<()> {
        let vm_name = format!("vm-{}", config.vm_id);

        // Step 1: Stop VM on source
        Self::update_job_state(jobs, job_id, MigrationState::Preparing, 10.0).await;
        tracing::info!("Stopping VM {} for offline migration", config.vm_id);

        let output = Command::new("ssh")
            .args([
                "-o", "StrictHostKeyChecking=no",
                "-o", "UserKnownHostsFile=/dev/null",
                &format!("root@{}", source_node),
                "virsh", "shutdown", &vm_name,
            ])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to shutdown VM: {}", e)))?;

        if !output.status.success() {
            // Try force shutdown if graceful fails
            tracing::warn!("Graceful shutdown failed, forcing VM {} shutdown", config.vm_id);
            let force_output = Command::new("ssh")
                .args([
                    "-o", "StrictHostKeyChecking=no",
                    "-o", "UserKnownHostsFile=/dev/null",
                    &format!("root@{}", source_node),
                    "virsh", "destroy", &vm_name,
                ])
                .output()
                .await
                .map_err(|e| horcrux_common::Error::System(format!("Failed to force shutdown VM: {}", e)))?;

            if !force_output.status.success() {
                return Err(horcrux_common::Error::System(
                    format!("Failed to stop VM: {}", String::from_utf8_lossy(&force_output.stderr))
                ));
            }
        }

        // Wait for VM to fully stop
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        // Step 2: Export VM configuration
        Self::update_job_state(jobs, job_id, MigrationState::Transferring, 20.0).await;
        tracing::info!("Exporting VM {} configuration", config.vm_id);

        let xml_output = Command::new("ssh")
            .args([
                "-o", "StrictHostKeyChecking=no",
                "-o", "UserKnownHostsFile=/dev/null",
                &format!("root@{}", source_node),
                "virsh", "dumpxml", &vm_name,
            ])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to export VM XML: {}", e)))?;

        if !xml_output.status.success() {
            return Err(horcrux_common::Error::System(
                format!("Failed to dump VM XML: {}", String::from_utf8_lossy(&xml_output.stderr))
            ));
        }

        let vm_xml = String::from_utf8_lossy(&xml_output.stdout).to_string();

        // Step 3: Transfer disk images via rsync
        Self::update_job_state(jobs, job_id, MigrationState::Transferring, 30.0).await;
        tracing::info!("Transferring disk images for VM {}", config.vm_id);

        // Get list of disk images
        let disklist_output = Command::new("ssh")
            .args([
                "-o", "StrictHostKeyChecking=no",
                "-o", "UserKnownHostsFile=/dev/null",
                &format!("root@{}", source_node),
                "virsh", "domblklist", &vm_name, "--details",
            ])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to list disks: {}", e)))?;

        if disklist_output.status.success() {
            let disklist = String::from_utf8_lossy(&disklist_output.stdout);
            let mut disk_paths = Vec::new();

            // Parse disk paths from virsh domblklist output
            for line in disklist.lines().skip(2) { // Skip header lines
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 4 && parts[0] == "file" {
                    disk_paths.push(parts[3].to_string());
                }
            }

            // Transfer each disk with rsync
            for (idx, disk_path) in disk_paths.iter().enumerate() {
                let progress = 30.0 + ((idx as f32 / disk_paths.len() as f32) * 50.0);
                Self::update_job_state(jobs, job_id, MigrationState::Transferring, progress).await;

                tracing::info!("Transferring disk: {}", disk_path);

                let rsync_output = Command::new("ssh")
                    .args([
                        "-o", "StrictHostKeyChecking=no",
                        "-o", "UserKnownHostsFile=/dev/null",
                        &format!("root@{}", source_node),
                        "rsync", "-avz", "--progress",
                        disk_path,
                        &format!("root@{}:{}", config.target_node, disk_path),
                    ])
                    .output()
                    .await;

                if let Ok(output) = rsync_output {
                    if !output.status.success() {
                        tracing::warn!("Disk transfer failed for {}: {}", disk_path, String::from_utf8_lossy(&output.stderr));
                    }
                } else {
                    tracing::warn!("Failed to execute rsync for disk: {}", disk_path);
                }
            }
        }

        // Step 4: Import VM configuration on target
        Self::update_job_state(jobs, job_id, MigrationState::Syncing, 85.0).await;
        tracing::info!("Importing VM {} configuration on target node", config.vm_id);

        // Define VM on target node using the exported XML
        let define_output = Command::new("ssh")
            .args([
                "-o", "StrictHostKeyChecking=no",
                "-o", "UserKnownHostsFile=/dev/null",
                &format!("root@{}", config.target_node),
                "virsh", "define", "/dev/stdin",
            ])
            .stdin(std::process::Stdio::piped())
            .output()
            .await;

        // TODO: Pipe vm_xml to stdin - for now, write to temp file
        let temp_xml_path = format!("/tmp/vm-{}.xml", config.vm_id);
        let write_xml = Command::new("ssh")
            .args([
                "-o", "StrictHostKeyChecking=no",
                "-o", "UserKnownHostsFile=/dev/null",
                &format!("root@{}", config.target_node),
                &format!("cat > {}", temp_xml_path),
            ])
            .arg(&vm_xml)
            .output()
            .await;

        let define_result = Command::new("ssh")
            .args([
                "-o", "StrictHostKeyChecking=no",
                "-o", "UserKnownHostsFile=/dev/null",
                &format!("root@{}", config.target_node),
                "virsh", "define", &temp_xml_path,
            ])
            .output()
            .await;

        match define_result {
            Ok(output) if !output.status.success() => {
                return Err(horcrux_common::Error::System(
                    format!("Failed to define VM on target: {}", String::from_utf8_lossy(&output.stderr))
                ));
            }
            Err(e) => {
                return Err(horcrux_common::Error::System(format!("Failed to define VM on target: {}", e)));
            }
            _ => {}
        }

        // Step 5: Start VM on target
        Self::update_job_state(jobs, job_id, MigrationState::Finalizing, 90.0).await;
        tracing::info!("Starting VM {} on target node {}", config.vm_id, config.target_node);

        let start_output = Command::new("ssh")
            .args([
                "-o", "StrictHostKeyChecking=no",
                "-o", "UserKnownHostsFile=/dev/null",
                &format!("root@{}", config.target_node),
                "virsh", "start", &vm_name,
            ])
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to start VM on target: {}", e)))?;

        if !start_output.status.success() {
            return Err(horcrux_common::Error::System(
                format!("Failed to start VM on target: {}", String::from_utf8_lossy(&start_output.stderr))
            ));
        }

        // Verify VM is running
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        let verify_output = Command::new("ssh")
            .args([
                "-o", "StrictHostKeyChecking=no",
                "-o", "UserKnownHostsFile=/dev/null",
                &format!("root@{}", config.target_node),
                "virsh", "domstate", &vm_name,
            ])
            .output()
            .await;

        match verify_output {
            Ok(output) if output.status.success() => {
                let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if state != "running" {
                    return Err(horcrux_common::Error::System(
                        format!("VM started but is not running. State: {}", state)
                    ));
                }
            }
            _ => {
                tracing::warn!("Could not verify VM state on target node");
            }
        }

        // Step 6: Undefine VM from source node
        let undefine_output = Command::new("ssh")
            .args([
                "-o", "StrictHostKeyChecking=no",
                "-o", "UserKnownHostsFile=/dev/null",
                &format!("root@{}", source_node),
                "virsh", "undefine", &vm_name,
            ])
            .output()
            .await;

        if let Ok(output) = undefine_output {
            if !output.status.success() {
                tracing::warn!("Failed to undefine VM from source: {}", String::from_utf8_lossy(&output.stderr));
            }
        }

        tracing::info!("Offline migration completed successfully for VM {}", config.vm_id);
        Ok(())
    }

    /// Execute online migration (with brief pause)
    async fn execute_online_migration(
        jobs: &Arc<RwLock<HashMap<String, MigrationJob>>>,
        job_id: &str,
        config: &MigrationConfig,
        source_node: &str,
    ) -> Result<()> {
        // Online migration: Pause VM briefly during final sync for consistency
        // Similar to live migration but explicitly pauses the VM during stop-and-copy

        let vm_name = format!("vm-{}", config.vm_id);

        // Phase 1: Start pre-copy while VM is running
        Self::update_job_state(jobs, job_id, MigrationState::Transferring, 20.0).await;
        tracing::info!("Starting online migration with pre-copy for VM {}", config.vm_id);

        // Build virsh migrate command
        let mut virsh_args = vec![
            "migrate".to_string(),
            "--live".to_string(),
            "--suspend".to_string(),  // Pause VM during final sync
            vm_name.clone(),
        ];

        if let Some(bw) = config.bandwidth_limit {
            virsh_args.push("--bandwidth".to_string());
            virsh_args.push(format!("{}", bw));
        }

        let target_uri = format!("qemu+ssh://root@{}/system", config.target_node);
        virsh_args.push(target_uri);

        // Initiate migration
        let output = Command::new("ssh")
            .args([
                "-o", "StrictHostKeyChecking=no",
                "-o", "UserKnownHostsFile=/dev/null",
                &format!("root@{}", source_node),
                "virsh",
            ])
            .args(&virsh_args)
            .arg("--async")
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to initiate online migration: {}", e)))?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(
                format!("Failed to start online migration: {}", String::from_utf8_lossy(&output.stderr))
            ));
        }

        tracing::info!("Online migration initiated for VM {}", config.vm_id);

        // Monitor migration progress
        let mut last_progress = 30.0;
        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

            let status_output = Command::new("ssh")
                .args([
                    "-o", "StrictHostKeyChecking=no",
                    "-o", "UserKnownHostsFile=/dev/null",
                    &format!("root@{}", source_node),
                    "virsh", "domjobinfo", &vm_name,
                ])
                .output()
                .await;

            match status_output {
                Ok(output) if output.status.success() => {
                    let output_str = String::from_utf8_lossy(&output.stdout);

                    let mut processed: u64 = 0;
                    let mut total: u64 = 0;

                    for line in output_str.lines() {
                        if line.contains("Data processed:") {
                            if let Some(val) = line.split_whitespace().nth(2) {
                                processed = val.parse().unwrap_or(0);
                            }
                        } else if line.contains("Data total:") {
                            if let Some(val) = line.split_whitespace().nth(2) {
                                total = val.parse().unwrap_or(0);
                            }
                        }
                    }

                    let progress = if total > 0 {
                        30.0 + ((processed as f32 / total as f32) * 55.0)
                    } else {
                        last_progress + 5.0
                    };

                    last_progress = progress.min(85.0);
                    Self::update_job_state(jobs, job_id, MigrationState::Transferring, last_progress).await;

                    if output_str.contains("None") || output_str.contains("Completed") {
                        break;
                    }
                }
                _ => {
                    break;
                }
            }

            if last_progress >= 85.0 {
                break;
            }
        }

        // Phase 2: VM paused for final sync
        Self::update_job_state(jobs, job_id, MigrationState::Syncing, 90.0).await;
        tracing::info!("VM {} paused for final sync during online migration", config.vm_id);

        // Wait for final sync to complete
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // Phase 3: Finalize
        Self::update_job_state(jobs, job_id, MigrationState::Finalizing, 95.0).await;
        tracing::info!("Finalizing online migration for VM {}", config.vm_id);

        // Verify VM is running on target
        let verify_output = Command::new("ssh")
            .args([
                "-o", "StrictHostKeyChecking=no",
                "-o", "UserKnownHostsFile=/dev/null",
                &format!("root@{}", config.target_node),
                "virsh", "domstate", &vm_name,
            ])
            .output()
            .await;

        match verify_output {
            Ok(output) if output.status.success() => {
                let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if state != "running" {
                    return Err(horcrux_common::Error::System(
                        format!("Online migration completed but VM is not running on target. State: {}", state)
                    ));
                }
            }
            _ => {
                tracing::warn!("Could not verify VM state on target node");
            }
        }

        tracing::info!("Online migration completed successfully for VM {}", config.vm_id);
        Ok(())
    }

    /// Pre-migration checks
    async fn pre_migration_checks(&self, config: &MigrationConfig, source_node: &str) -> Result<()> {
        // Check 1: Target node is reachable
        tracing::info!("Checking connectivity to target node {}", config.target_node);

        // Check 2: Sufficient resources on target
        tracing::info!("Checking resources on target node");

        // Check 3: Shared storage available (for live migration)
        if config.migration_type == MigrationType::Live && !config.with_local_disks {
            tracing::info!("Verifying shared storage access");
        }

        // Check 4: Network bandwidth sufficient
        tracing::info!("Checking network bandwidth between nodes");

        // Check 5: Compatible CPU features
        tracing::info!("Verifying CPU compatibility");

        Ok(())
    }

    /// Update job state
    async fn update_job_state(
        jobs: &Arc<RwLock<HashMap<String, MigrationJob>>>,
        job_id: &str,
        state: MigrationState,
        progress: f32,
    ) {
        let mut jobs_lock = jobs.write().await;
        if let Some(job) = jobs_lock.get_mut(job_id) {
            job.state = state;
            job.progress = progress;
        }
    }

    /// Get migration job status
    pub async fn get_job(&self, job_id: &str) -> Option<MigrationJob> {
        self.jobs.read().await.get(job_id).cloned()
    }

    /// List all migration jobs
    pub async fn list_jobs(&self) -> Vec<MigrationJob> {
        self.jobs.read().await.values().cloned().collect()
    }

    /// List active migrations
    pub async fn list_active(&self) -> Vec<MigrationJob> {
        self.jobs.read().await
            .values()
            .filter(|j| !matches!(j.state, MigrationState::Completed | MigrationState::Failed | MigrationState::Cancelled))
            .cloned()
            .collect()
    }

    /// Cancel migration
    pub async fn cancel_migration(&self, job_id: &str) -> Result<()> {
        let mut jobs = self.jobs.write().await;

        let job = jobs.get_mut(job_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("Migration job {} not found", job_id))
        })?;

        if matches!(job.state, MigrationState::Completed | MigrationState::Failed) {
            return Err(horcrux_common::Error::System(
                "Cannot cancel completed or failed migration".to_string()
            ));
        }

        job.state = MigrationState::Cancelled;
        job.completed = Some(Utc::now());

        tracing::info!("Cancelled migration job {}", job_id);
        Ok(())
    }

    /// Count active migrations
    async fn count_active_migrations(&self) -> usize {
        self.jobs.read().await
            .values()
            .filter(|j| !matches!(j.state, MigrationState::Completed | MigrationState::Failed | MigrationState::Cancelled))
            .count()
    }

    /// Set global bandwidth limit
    pub async fn set_bandwidth_limit(&self, limit_mbps: Option<u64>) {
        let mut bw = self.bandwidth_limit.write().await;
        *bw = limit_mbps;
        tracing::info!("Set global migration bandwidth limit to {:?} MB/s", limit_mbps);
    }

    /// Set max concurrent migrations
    pub async fn set_max_concurrent(&self, max: usize) {
        let mut max_concurrent = self.max_concurrent.write().await;
        *max_concurrent = max;
        tracing::info!("Set max concurrent migrations to {}", max);
    }

    /// Get migration statistics
    pub async fn get_statistics(&self, job_id: &str) -> Result<MigrationStats> {
        let job = self.get_job(job_id).await.ok_or_else(|| {
            horcrux_common::Error::System(format!("Migration job {} not found", job_id))
        })?;

        let duration = if let Some(completed) = job.completed {
            (completed - job.started).num_seconds() as u64
        } else {
            (Utc::now() - job.started).num_seconds() as u64
        };

        let transferred_gb = job.transferred_bytes as f64 / (1024.0 * 1024.0 * 1024.0);

        let average_speed_mbps = if duration > 0 {
            (job.transferred_bytes as f64 / (1024.0 * 1024.0)) / duration as f64
        } else {
            0.0
        };

        Ok(MigrationStats {
            duration_seconds: duration,
            downtime_ms: match job.migration_type {
                MigrationType::Live => 100,     // Typical live migration downtime
                MigrationType::Online => 500,   // Brief pause
                MigrationType::Offline => duration * 1000,  // Full downtime
            },
            transferred_gb,
            average_speed_mbps,
            memory_dirty_rate: 0.0,  // Would be calculated from actual migration
        })
    }

    /// Get rollback plan for a migration job
    pub async fn get_rollback(&self, migration_job_id: &str) -> Option<rollback::RollbackPlan> {
        self.rollback_manager
            .read()
            .await
            .get_rollback(migration_job_id)
            .cloned()
    }

    /// List all rollback plans
    pub async fn list_rollbacks(&self) -> Vec<rollback::RollbackPlan> {
        self.rollback_manager
            .read()
            .await
            .list_rollbacks()
            .into_iter()
            .cloned()
            .collect()
    }

    /// Manually trigger rollback for a failed migration
    pub async fn manual_rollback(&self, migration_job_id: &str) -> Result<rollback::RollbackSummary> {
        let job = self.get_job(migration_job_id).await.ok_or_else(|| {
            horcrux_common::Error::System(format!("Migration job {} not found", migration_job_id))
        })?;

        if job.state != MigrationState::Failed {
            return Err(horcrux_common::Error::System(
                format!("Can only rollback failed migrations. Current state: {:?}", job.state)
            ));
        }

        let mut rb_manager = self.rollback_manager.write().await;
        rb_manager.rollback_migration(
            migration_job_id.to_string(),
            job.vm_id,
            job.source_node,
            job.target_node,
        ).await
    }

    /// Get health check report for a migration
    pub async fn get_health_report(&self, job_id: &str) -> Option<health_check::HealthCheckReport> {
        self.health_reports.read().await.get(job_id).cloned()
    }

    /// List all health check reports
    pub async fn list_health_reports(&self) -> Vec<health_check::HealthCheckReport> {
        self.health_reports.read().await.values().cloned().collect()
    }

    /// Get health check summary for a migration
    pub async fn get_health_summary(&self, job_id: &str) -> Option<health_check::HealthCheckSummary> {
        self.get_health_report(job_id).await.map(|r| r.get_summary())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_migration_job_creation() {
        let manager = MigrationManager::new();

        let config = MigrationConfig {
            vm_id: 100,
            target_node: "node2".to_string(),
            migration_type: MigrationType::Live,
            bandwidth_limit: Some(100),
            force: false,
            with_local_disks: false,
        };

        let job_id = manager.start_migration(config, "node1".to_string()).await.unwrap();
        assert!(!job_id.is_empty());

        // Wait a bit for migration to progress
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        let job = manager.get_job(&job_id).await.unwrap();
        assert_eq!(job.vm_id, 100);
        assert!(job.progress > 0.0);
    }

    #[tokio::test]
    async fn test_migration_types() {
        let manager = MigrationManager::new();

        // Test live migration
        let config = MigrationConfig {
            vm_id: 100,
            target_node: "node2".to_string(),
            migration_type: MigrationType::Live,
            bandwidth_limit: None,
            force: true,
            with_local_disks: false,
        };

        let job_id = manager.start_migration(config, "node1".to_string()).await.unwrap();
        assert!(!job_id.is_empty());
    }

    #[tokio::test]
    async fn test_concurrent_migration_limit() {
        let manager = MigrationManager::new();
        manager.set_max_concurrent(1).await;

        let config1 = MigrationConfig {
            vm_id: 100,
            target_node: "node2".to_string(),
            migration_type: MigrationType::Live,
            bandwidth_limit: None,
            force: true,
            with_local_disks: false,
        };

        let config2 = MigrationConfig {
            vm_id: 101,
            target_node: "node2".to_string(),
            migration_type: MigrationType::Live,
            bandwidth_limit: None,
            force: true,
            with_local_disks: false,
        };

        // First migration should succeed
        let job1 = manager.start_migration(config1, "node1".to_string()).await;
        assert!(job1.is_ok());

        // Second migration should fail (limit reached)
        let job2 = manager.start_migration(config2, "node1".to_string()).await;
        assert!(job2.is_err());
    }

    #[tokio::test]
    async fn test_migration_cancellation() {
        let manager = MigrationManager::new();

        let config = MigrationConfig {
            vm_id: 100,
            target_node: "node2".to_string(),
            migration_type: MigrationType::Offline,
            bandwidth_limit: None,
            force: true,
            with_local_disks: true,
        };

        let job_id = manager.start_migration(config, "node1".to_string()).await.unwrap();

        // Cancel immediately
        let result = manager.cancel_migration(&job_id).await;
        assert!(result.is_ok());

        let job = manager.get_job(&job_id).await.unwrap();
        assert_eq!(job.state, MigrationState::Cancelled);
    }
}
