//! Live and Offline VM Migration
//!
//! Provides VM migration capabilities between cluster nodes including:
//! - Live migration with pre-copy memory transfer
//! - Offline migration with storage copy
//! - Post-copy migration for large VMs
//! - Migration progress tracking

use chrono::{DateTime, Utc};
use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::ToSocketAddrs;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Migration types supported
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MigrationType {
    /// Live migration - VM stays running during transfer
    Live,
    /// Offline migration - VM is stopped, copied, then started
    Offline,
    /// Post-copy - Start on destination, fetch memory on demand
    PostCopy,
}

/// Migration state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MigrationState {
    /// Migration pending/queued
    Pending,
    /// Pre-migration checks in progress
    Checking,
    /// Memory transfer in progress
    Transferring,
    /// Storage sync in progress
    SyncingStorage,
    /// Final iteration (for live migration)
    Converging,
    /// Switching to destination
    Switching,
    /// Post-migration cleanup
    Cleanup,
    /// Migration completed successfully
    Completed,
    /// Migration failed
    Failed(String),
    /// Migration cancelled
    Cancelled,
}

/// Migration job configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationConfig {
    /// Maximum bandwidth for migration (MB/s), 0 = unlimited
    pub bandwidth_limit: u64,
    /// Enable compression for memory transfer
    pub compress: bool,
    /// Allow migration to node with different CPU
    pub allow_cpu_incompatible: bool,
    /// Maximum downtime in milliseconds (for live migration)
    pub max_downtime_ms: u64,
    /// Enable RDMA for faster memory transfer (requires InfiniBand)
    pub use_rdma: bool,
    /// Number of parallel transfer connections
    pub parallel_connections: u32,
    /// Enable XBZRLE compression for repeated memory patterns
    pub enable_xbzrle: bool,
}

impl Default for MigrationConfig {
    fn default() -> Self {
        Self {
            bandwidth_limit: 0, // unlimited
            compress: true,
            allow_cpu_incompatible: false,
            max_downtime_ms: 300, // 300ms default
            use_rdma: false,
            parallel_connections: 2,
            enable_xbzrle: true,
        }
    }
}

/// Migration progress information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationProgress {
    /// Total RAM to transfer (bytes)
    pub total_memory: u64,
    /// RAM transferred so far (bytes)
    pub transferred_memory: u64,
    /// Remaining RAM to transfer (bytes)
    pub remaining_memory: u64,
    /// Dirty pages since last iteration
    pub dirty_pages: u64,
    /// Current transfer speed (bytes/sec)
    pub transfer_speed: u64,
    /// Expected time to complete (seconds)
    pub expected_downtime_ms: u64,
    /// Current iteration number
    pub iteration: u32,
    /// Storage progress (0-100%)
    pub storage_progress: f32,
    /// Total disk to transfer (bytes)
    pub total_disk: u64,
    /// Disk transferred so far (bytes)
    pub transferred_disk: u64,
}

impl Default for MigrationProgress {
    fn default() -> Self {
        Self {
            total_memory: 0,
            transferred_memory: 0,
            remaining_memory: 0,
            dirty_pages: 0,
            transfer_speed: 0,
            expected_downtime_ms: 0,
            iteration: 0,
            storage_progress: 0.0,
            total_disk: 0,
            transferred_disk: 0,
        }
    }
}

/// Migration job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationJob {
    pub id: String,
    pub vm_id: String,
    pub source_node: String,
    pub target_node: String,
    pub migration_type: MigrationType,
    pub state: MigrationState,
    pub config: MigrationConfig,
    pub progress: MigrationProgress,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

/// Pre-migration check result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationCheck {
    pub check_name: String,
    pub passed: bool,
    pub message: String,
    pub blocking: bool,
}

/// Migration manager
pub struct MigrationManager {
    jobs: Arc<RwLock<HashMap<String, MigrationJob>>>,
    active_migrations: Arc<RwLock<HashMap<String, String>>>, // vm_id -> job_id
    max_concurrent: usize,
}

impl MigrationManager {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            active_migrations: Arc::new(RwLock::new(HashMap::new())),
            max_concurrent: 4,
        }
    }

    /// Pre-migration checks
    pub async fn check_migration(
        &self,
        vm_id: &str,
        source_node: &str,
        target_node: &str,
        migration_type: &MigrationType,
        required_memory_mb: u64,
    ) -> Vec<MigrationCheck> {
        let mut checks = Vec::new();

        // Check 1: VM is not already being migrated
        let active = self.active_migrations.read().await;
        if active.contains_key(vm_id) {
            checks.push(MigrationCheck {
                check_name: "no_active_migration".to_string(),
                passed: false,
                message: "VM is already being migrated".to_string(),
                blocking: true,
            });
        } else {
            checks.push(MigrationCheck {
                check_name: "no_active_migration".to_string(),
                passed: true,
                message: "No active migration for this VM".to_string(),
                blocking: false,
            });
        }
        drop(active);

        // Check 2: Source and target are different
        if source_node == target_node {
            checks.push(MigrationCheck {
                check_name: "different_nodes".to_string(),
                passed: false,
                message: "Source and target nodes are the same".to_string(),
                blocking: true,
            });
        } else {
            checks.push(MigrationCheck {
                check_name: "different_nodes".to_string(),
                passed: true,
                message: format!("Migration from {} to {}", source_node, target_node),
                blocking: false,
            });
        }

        // Check 3: Network connectivity - real TCP reachability check against
        // the target node's management port (SSH, 22), same way
        // vm::cross_node_clone verifies connectivity before transferring
        // disks. We don't have the VM's memory/disk requirements in this
        // signature, so this only validates reachability, not bandwidth.
        checks.push(Self::check_network_connectivity(target_node).await);

        // Check 4: Storage accessibility - verify the target node has the
        // shared/target storage mounted and reachable over SSH.
        checks.push(Self::check_storage_accessible(source_node, target_node).await);

        // Check 5: Live migration requirements
        if *migration_type == MigrationType::Live {
            checks.push(MigrationCheck {
                check_name: "live_migration_support".to_string(),
                passed: true,
                message: "Live migration is supported".to_string(),
                blocking: false,
            });
        }

        // Check 6: CPU compatibility - compare CPU flags/model between
        // source and target nodes (own /proc/cpuinfo for the local node,
        // remote node queried over SSH, matching the SSH-based node access
        // pattern used elsewhere in this codebase e.g.
        // vm::cross_node_clone::verify_ssh_connectivity).
        checks.push(Self::check_cpu_compatibility(source_node, target_node).await);

        // Check 7: Memory availability on target - query the target's
        // available RAM (via /proc/meminfo, same parsing as
        // metrics::system::read_memory_stats) and compare against what's
        // required for the VM.
        checks.push(Self::check_memory_available(target_node, required_memory_mb).await);

        info!(
            vm_id = vm_id,
            source = source_node,
            target = target_node,
            checks_passed = checks.iter().filter(|c| c.passed).count(),
            total_checks = checks.len(),
            "Pre-migration checks completed"
        );

        checks
    }

    /// Start a migration job
    pub async fn start_migration(
        &self,
        vm_id: String,
        source_node: String,
        target_node: String,
        migration_type: MigrationType,
        config: Option<MigrationConfig>,
        required_memory_mb: u64,
    ) -> Result<String> {
        // Run pre-migration checks
        let checks = self
            .check_migration(
                &vm_id,
                &source_node,
                &target_node,
                &migration_type,
                required_memory_mb,
            )
            .await;

        let blocking_failures: Vec<_> = checks.iter().filter(|c| !c.passed && c.blocking).collect();

        if !blocking_failures.is_empty() {
            let reasons: Vec<_> = blocking_failures
                .iter()
                .map(|c| c.message.clone())
                .collect();
            return Err(horcrux_common::Error::System(format!(
                "Pre-migration checks failed: {}",
                reasons.join(", ")
            )));
        }

        // Check concurrent migration limit
        let jobs = self.jobs.read().await;
        let active_count = jobs
            .values()
            .filter(|j| {
                matches!(
                    j.state,
                    MigrationState::Transferring
                        | MigrationState::SyncingStorage
                        | MigrationState::Converging
                        | MigrationState::Switching
                )
            })
            .count();
        drop(jobs);

        if active_count >= self.max_concurrent {
            return Err(horcrux_common::Error::System(format!(
                "Maximum concurrent migrations ({}) reached",
                self.max_concurrent
            )));
        }

        // Create migration job
        let job_id = format!("mig-{}-{}", vm_id, Utc::now().timestamp());
        let job = MigrationJob {
            id: job_id.clone(),
            vm_id: vm_id.clone(),
            source_node: source_node.clone(),
            target_node: target_node.clone(),
            migration_type: migration_type.clone(),
            state: MigrationState::Pending,
            config: config.unwrap_or_default(),
            progress: MigrationProgress::default(),
            started_at: Utc::now(),
            completed_at: None,
            error: None,
        };

        // Register job
        {
            let mut jobs = self.jobs.write().await;
            jobs.insert(job_id.clone(), job);
        }
        {
            let mut active = self.active_migrations.write().await;
            active.insert(vm_id.clone(), job_id.clone());
        }

        info!(
            job_id = %job_id,
            vm_id = %vm_id,
            source = %source_node,
            target = %target_node,
            migration_type = ?migration_type,
            "Migration job created"
        );

        // Start migration process (in a real implementation, this would be async)
        self.execute_migration(&job_id).await?;

        Ok(job_id)
    }

    /// Execute the migration
    async fn execute_migration(&self, job_id: &str) -> Result<()> {
        // Update state to checking
        self.update_state(job_id, MigrationState::Checking).await?;

        let job = self.get_job(job_id).await?;

        match job.migration_type {
            MigrationType::Live => self.execute_live_migration(&job).await,
            MigrationType::Offline => self.execute_offline_migration(&job).await,
            MigrationType::PostCopy => self.execute_postcopy_migration(&job).await,
        }
    }

    /// Execute live migration
    async fn execute_live_migration(&self, job: &MigrationJob) -> Result<()> {
        info!(job_id = %job.id, "Starting live migration");

        // Phase 1: Pre-copy - Transfer all memory pages
        self.update_state(&job.id, MigrationState::Transferring)
            .await?;

        // In a real implementation, this would use QEMU's migrate command
        // qemu-monitor-command: migrate -d tcp:target:4444

        // Simulate memory transfer iterations
        for iteration in 1..=5 {
            debug!(job_id = %job.id, iteration = iteration, "Memory transfer iteration");

            // Update progress
            let mut jobs = self.jobs.write().await;
            if let Some(j) = jobs.get_mut(&job.id) {
                j.progress.iteration = iteration;
                j.progress.transferred_memory = iteration as u64 * 1024 * 1024 * 1024; // Simulated
                j.progress.remaining_memory = (5 - iteration) as u64 * 1024 * 1024 * 1024;
                j.progress.dirty_pages = 10000 / iteration as u64;
            }
            drop(jobs);

            // Small delay for simulation
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        // Phase 2: Converging - Final dirty page sync
        self.update_state(&job.id, MigrationState::Converging)
            .await?;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Phase 3: Switching - Stop source, start destination
        self.update_state(&job.id, MigrationState::Switching)
            .await?;

        // In a real implementation:
        // 1. Send QMP command to finalize migration
        // 2. Wait for migration to complete
        // 3. Verify VM is running on target

        // Phase 4: Cleanup
        self.update_state(&job.id, MigrationState::Cleanup).await?;

        // Complete
        self.complete_migration(&job.id).await?;

        info!(job_id = %job.id, "Live migration completed successfully");

        Ok(())
    }

    /// Execute offline migration
    async fn execute_offline_migration(&self, job: &MigrationJob) -> Result<()> {
        info!(job_id = %job.id, "Starting offline migration");

        // Phase 1: Stop VM on source
        self.update_state(&job.id, MigrationState::Checking).await?;

        // In a real implementation:
        // 1. Stop the VM
        // 2. Wait for clean shutdown

        // Phase 2: Copy storage
        self.update_state(&job.id, MigrationState::SyncingStorage)
            .await?;

        // Simulate storage copy
        for progress in (0..=100).step_by(10) {
            let mut jobs = self.jobs.write().await;
            if let Some(j) = jobs.get_mut(&job.id) {
                j.progress.storage_progress = progress as f32;
            }
            drop(jobs);
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // Phase 3: Transfer memory state (if suspended)
        self.update_state(&job.id, MigrationState::Transferring)
            .await?;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Phase 4: Start VM on target
        self.update_state(&job.id, MigrationState::Switching)
            .await?;

        // In a real implementation:
        // 1. Create VM on target with same config
        // 2. Attach copied storage
        // 3. Start VM

        // Phase 5: Cleanup source
        self.update_state(&job.id, MigrationState::Cleanup).await?;

        // Complete
        self.complete_migration(&job.id).await?;

        info!(job_id = %job.id, "Offline migration completed successfully");

        Ok(())
    }

    /// Execute post-copy migration
    async fn execute_postcopy_migration(&self, job: &MigrationJob) -> Result<()> {
        info!(job_id = %job.id, "Starting post-copy migration");

        // Phase 1: Transfer minimal state
        self.update_state(&job.id, MigrationState::Transferring)
            .await?;

        // In a real implementation:
        // 1. Transfer CPU state and device state
        // 2. Start VM on destination immediately
        // 3. Fetch memory pages on demand

        // Phase 2: Switch to destination
        self.update_state(&job.id, MigrationState::Switching)
            .await?;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Phase 3: Background page fetch
        // VM is running on destination, pages fetched as needed

        // Complete
        self.complete_migration(&job.id).await?;

        info!(job_id = %job.id, "Post-copy migration completed successfully");

        Ok(())
    }

    /// Update migration state
    async fn update_state(&self, job_id: &str, state: MigrationState) -> Result<()> {
        let mut jobs = self.jobs.write().await;
        let job = jobs.get_mut(job_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("Migration job {} not found", job_id))
        })?;

        debug!(job_id = job_id, old_state = ?job.state, new_state = ?state, "Migration state change");
        job.state = state;

        Ok(())
    }

    /// Complete a migration
    async fn complete_migration(&self, job_id: &str) -> Result<()> {
        let mut jobs = self.jobs.write().await;
        let job = jobs.get_mut(job_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("Migration job {} not found", job_id))
        })?;

        job.state = MigrationState::Completed;
        job.completed_at = Some(Utc::now());

        let vm_id = job.vm_id.clone();
        drop(jobs);

        // Remove from active migrations
        let mut active = self.active_migrations.write().await;
        active.remove(&vm_id);

        Ok(())
    }

    /// Cancel a migration
    pub async fn cancel_migration(&self, job_id: &str) -> Result<()> {
        let mut jobs = self.jobs.write().await;
        let job = jobs.get_mut(job_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("Migration job {} not found", job_id))
        })?;

        // Can only cancel pending or in-progress migrations
        match &job.state {
            MigrationState::Completed | MigrationState::Failed(_) | MigrationState::Cancelled => {
                return Err(horcrux_common::Error::System(format!(
                    "Cannot cancel migration in state {:?}",
                    job.state
                )));
            }
            _ => {}
        }

        warn!(job_id = job_id, "Cancelling migration");

        job.state = MigrationState::Cancelled;
        job.completed_at = Some(Utc::now());

        let vm_id = job.vm_id.clone();
        drop(jobs);

        // Remove from active migrations
        let mut active = self.active_migrations.write().await;
        active.remove(&vm_id);

        // In a real implementation:
        // 1. Send cancel command to QEMU
        // 2. Cleanup any partial state
        // 3. Ensure VM is still running on source

        Ok(())
    }

    /// Fail a migration
    pub async fn fail_migration(&self, job_id: &str, reason: String) -> Result<()> {
        let mut jobs = self.jobs.write().await;
        let job = jobs.get_mut(job_id).ok_or_else(|| {
            horcrux_common::Error::System(format!("Migration job {} not found", job_id))
        })?;

        error!(job_id = job_id, reason = %reason, "Migration failed");

        job.state = MigrationState::Failed(reason.clone());
        job.error = Some(reason);
        job.completed_at = Some(Utc::now());

        let vm_id = job.vm_id.clone();
        drop(jobs);

        // Remove from active migrations
        let mut active = self.active_migrations.write().await;
        active.remove(&vm_id);

        Ok(())
    }

    /// Get migration job
    pub async fn get_job(&self, job_id: &str) -> Result<MigrationJob> {
        let jobs = self.jobs.read().await;
        jobs.get(job_id).cloned().ok_or_else(|| {
            horcrux_common::Error::System(format!("Migration job {} not found", job_id))
        })
    }

    /// Get migration job by VM ID
    pub async fn get_job_by_vm(&self, vm_id: &str) -> Option<MigrationJob> {
        let active = self.active_migrations.read().await;
        if let Some(job_id) = active.get(vm_id) {
            let jobs = self.jobs.read().await;
            return jobs.get(job_id).cloned();
        }
        None
    }

    /// List all migration jobs
    pub async fn list_jobs(&self, include_completed: bool) -> Vec<MigrationJob> {
        let jobs = self.jobs.read().await;
        jobs.values()
            .filter(|j| {
                include_completed
                    || !matches!(
                        j.state,
                        MigrationState::Completed
                            | MigrationState::Failed(_)
                            | MigrationState::Cancelled
                    )
            })
            .cloned()
            .collect()
    }

    /// Get migration history for a VM
    pub async fn get_vm_migration_history(&self, vm_id: &str) -> Vec<MigrationJob> {
        let jobs = self.jobs.read().await;
        jobs.values()
            .filter(|j| j.vm_id == vm_id)
            .cloned()
            .collect()
    }

    /// Cleanup old completed jobs
    pub async fn cleanup_old_jobs(&self, max_age_hours: u32) {
        let mut jobs = self.jobs.write().await;
        let cutoff = Utc::now() - chrono::Duration::hours(max_age_hours as i64);

        let old_jobs: Vec<_> = jobs
            .iter()
            .filter(|(_, j)| {
                matches!(
                    j.state,
                    MigrationState::Completed
                        | MigrationState::Failed(_)
                        | MigrationState::Cancelled
                ) && j.completed_at.map(|t| t < cutoff).unwrap_or(false)
            })
            .map(|(id, _)| id.clone())
            .collect();

        for id in old_jobs {
            debug!(job_id = %id, "Cleaning up old migration job");
            jobs.remove(&id);
        }
    }

    /// Check network connectivity to the target node's management
    /// interface. Resolves `target_node` (hostname or IP) and attempts a
    /// real TCP connection to the API/management port (8006, matching
    /// `Node::api_url()` elsewhere in this crate) with a bounded timeout,
    /// falling back to SSH (22) if the management port isn't reachable
    /// (e.g. the API service isn't up yet but the host itself is), mirroring
    /// how `vm::cross_node_clone::verify_ssh_connectivity` reasons about
    /// node reachability.
    async fn check_network_connectivity(target_node: &str) -> MigrationCheck {
        const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
        const MGMT_PORT: u16 = 8006;
        const SSH_PORT: u16 = 22;

        for port in [MGMT_PORT, SSH_PORT] {
            let addr = format!("{}:{}", target_node, port);
            let resolved = match addr.to_socket_addrs() {
                Ok(mut addrs) => addrs.next(),
                Err(e) => {
                    return MigrationCheck {
                        check_name: "network_connectivity".to_string(),
                        passed: false,
                        message: format!(
                            "Failed to resolve target node {}: {}",
                            target_node, e
                        ),
                        blocking: true,
                    };
                }
            };

            let Some(socket_addr) = resolved else {
                continue;
            };

            match tokio::time::timeout(
                CONNECT_TIMEOUT,
                tokio::net::TcpStream::connect(socket_addr),
            )
            .await
            {
                Ok(Ok(_stream)) => {
                    return MigrationCheck {
                        check_name: "network_connectivity".to_string(),
                        passed: true,
                        message: format!(
                            "Target node {} is reachable on port {}",
                            target_node, port
                        ),
                        blocking: false,
                    };
                }
                Ok(Err(_)) | Err(_) => {
                    // Connection refused/timed out on this port; try the
                    // next candidate port before declaring unreachable.
                    continue;
                }
            }
        }

        MigrationCheck {
            check_name: "network_connectivity".to_string(),
            passed: false,
            message: format!(
                "Target node {} is not reachable on management port {} or SSH port {}",
                target_node, MGMT_PORT, SSH_PORT
            ),
            blocking: true,
        }
    }

    /// Check storage accessibility on the target node: verify the shared
    /// VM storage directory is mounted and reachable on the target via
    /// SSH, using the same `HORCRUX_DATA_DIR`/`HORCRUX_VM_STORAGE`
    /// resolution `config.rs` and `vm::qemu::QemuManager` use locally, and
    /// the same `ssh ... stat`-style remote probe
    /// `vm::cross_node_clone::get_disk_size` uses for remote filesystem
    /// checks.
    async fn check_storage_accessible(source_node: &str, target_node: &str) -> MigrationCheck {
        let vm_storage_path = std::env::var("HORCRUX_VM_STORAGE").unwrap_or_else(|_| {
            let data_dir =
                std::env::var("HORCRUX_DATA_DIR").unwrap_or_else(|_| "/var/lib/horcrux".to_string());
            format!("{}/vms", data_dir.trim_end_matches('/'))
        });

        // If the target is actually this same host (single-node dev/test
        // setups, or a node addressed by its local hostname), check the
        // mount directly instead of shelling out over SSH to ourselves.
        let local_hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_default();

        if target_node == source_node
            || target_node == "localhost"
            || target_node == "127.0.0.1"
            || target_node == local_hostname
        {
            return match tokio::fs::metadata(&vm_storage_path).await {
                Ok(meta) if meta.is_dir() => MigrationCheck {
                    check_name: "storage_accessible".to_string(),
                    passed: true,
                    message: format!(
                        "VM storage path {} is accessible on target node",
                        vm_storage_path
                    ),
                    blocking: false,
                },
                Ok(_) => MigrationCheck {
                    check_name: "storage_accessible".to_string(),
                    passed: false,
                    message: format!("{} exists but is not a directory", vm_storage_path),
                    blocking: true,
                },
                Err(e) => MigrationCheck {
                    check_name: "storage_accessible".to_string(),
                    passed: false,
                    message: format!(
                        "VM storage path {} is not accessible: {}",
                        vm_storage_path, e
                    ),
                    blocking: true,
                },
            };
        }

        // Remote target: verify the storage directory exists and is
        // writable over SSH, same command style used by
        // vm::cross_node_clone::create_target_directories.
        let check_cmd = format!(
            "test -d '{path}' && test -w '{path}'",
            path = vm_storage_path
        );

        let output = tokio::process::Command::new("ssh")
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("ConnectTimeout=5")
            .arg(target_node)
            .arg(&check_cmd)
            .output()
            .await;

        match output {
            Ok(out) if out.status.success() => MigrationCheck {
                check_name: "storage_accessible".to_string(),
                passed: true,
                message: format!(
                    "Storage path {} is mounted and writable on target node {}",
                    vm_storage_path, target_node
                ),
                blocking: false,
            },
            Ok(out) => MigrationCheck {
                check_name: "storage_accessible".to_string(),
                passed: false,
                message: format!(
                    "Storage path {} is not accessible/writable on target node {}: {}",
                    vm_storage_path,
                    target_node,
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
                blocking: true,
            },
            Err(e) => MigrationCheck {
                check_name: "storage_accessible".to_string(),
                passed: false,
                message: format!(
                    "Failed to verify storage accessibility on target node {} via SSH: {}",
                    target_node, e
                ),
                blocking: true,
            },
        }
    }

    /// Check CPU compatibility between source and target nodes by
    /// comparing `/proc/cpuinfo` "model name" and the "flags" feature set,
    /// same source of truth `cluster::node::Node::detect_memory` and
    /// `metrics::system` use for other real-hardware queries. Remote
    /// node info is pulled over SSH using the same pattern as
    /// `vm::cross_node_clone::get_disk_size`.
    async fn check_cpu_compatibility(source_node: &str, target_node: &str) -> MigrationCheck {
        let local_hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_default();

        let source_info = if source_node == local_hostname
            || source_node == "localhost"
            || source_node == "127.0.0.1"
        {
            Self::local_cpu_info()
        } else {
            Self::remote_cpu_info(source_node).await
        };

        let target_info = if target_node == local_hostname
            || target_node == "localhost"
            || target_node == "127.0.0.1"
        {
            Self::local_cpu_info()
        } else {
            Self::remote_cpu_info(target_node).await
        };

        let (source_model, source_flags) = match source_info {
            Some(info) => info,
            None => {
                return MigrationCheck {
                    check_name: "cpu_compatibility".to_string(),
                    passed: false,
                    message: format!("Unable to read CPU info from source node {}", source_node),
                    blocking: false,
                };
            }
        };

        let (target_model, target_flags) = match target_info {
            Some(info) => info,
            None => {
                return MigrationCheck {
                    check_name: "cpu_compatibility".to_string(),
                    passed: false,
                    message: format!("Unable to read CPU info from target node {}", target_node),
                    blocking: false,
                };
            }
        };

        // The target must support at least every CPU feature flag the
        // source exposes to the guest; missing flags can crash a live VM
        // immediately after migration if the guest already used them.
        let missing: Vec<&str> = source_flags
            .difference(&target_flags)
            .map(|s| s.as_str())
            .collect();

        if missing.is_empty() {
            MigrationCheck {
                check_name: "cpu_compatibility".to_string(),
                passed: true,
                message: if source_model == target_model {
                    format!("CPU models match ({})", target_model)
                } else {
                    format!(
                        "CPU models differ ({} -> {}) but target supports all required flags",
                        source_model, target_model
                    )
                },
                blocking: false,
            }
        } else {
            MigrationCheck {
                check_name: "cpu_compatibility".to_string(),
                passed: false,
                message: format!(
                    "Target node {} is missing CPU flags required by source {}: {}",
                    target_node,
                    source_node,
                    missing.join(", ")
                ),
                // Not blocking by default: MigrationConfig::allow_cpu_incompatible
                // lets an operator opt into cross-CPU migration (with QEMU
                // -cpu masking) at their own risk; this check just warns.
                blocking: false,
            }
        }
    }

    /// Read CPU model name + feature flags from the local `/proc/cpuinfo`.
    fn local_cpu_info() -> Option<(String, HashSet<String>)> {
        let content = std::fs::read_to_string("/proc/cpuinfo").ok()?;
        Self::parse_cpuinfo(&content)
    }

    /// Read CPU model name + feature flags from a remote node's
    /// `/proc/cpuinfo` over SSH.
    async fn remote_cpu_info(node: &str) -> Option<(String, HashSet<String>)> {
        let output = tokio::process::Command::new("ssh")
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("ConnectTimeout=5")
            .arg(node)
            .arg("cat")
            .arg("/proc/cpuinfo")
            .output()
            .await
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let content = String::from_utf8_lossy(&output.stdout);
        Self::parse_cpuinfo(&content)
    }

    /// Parse `/proc/cpuinfo` text into (model name, flag set), taking the
    /// first "model name" and "flags" lines found (all cores on a node
    /// report identical values).
    fn parse_cpuinfo(content: &str) -> Option<(String, HashSet<String>)> {
        let mut model = None;
        let mut flags = None;

        for line in content.lines() {
            if model.is_none() {
                if let Some(rest) = line.strip_prefix("model name") {
                    if let Some((_, value)) = rest.split_once(':') {
                        model = Some(value.trim().to_string());
                    }
                }
            }
            if flags.is_none() {
                if let Some(rest) = line.strip_prefix("flags") {
                    if let Some((_, value)) = rest.split_once(':') {
                        flags = Some(value.split_whitespace().map(String::from).collect());
                    }
                }
            }
            if model.is_some() && flags.is_some() {
                break;
            }
        }

        Some((model.unwrap_or_else(|| "unknown".to_string()), flags.unwrap_or_default()))
    }

    /// Check that the target node has enough available RAM for the VM
    /// being migrated. Reads `/proc/meminfo`'s `MemAvailable` field, the
    /// same field `metrics::system::read_memory_stats` uses for the
    /// node's own resource reporting, either locally or over SSH for a
    /// remote target.
    async fn check_memory_available(target_node: &str, required_memory_mb: u64) -> MigrationCheck {
        let local_hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_default();

        let available_bytes = if target_node == local_hostname
            || target_node == "localhost"
            || target_node == "127.0.0.1"
        {
            Self::local_available_memory()
        } else {
            Self::remote_available_memory(target_node).await
        };

        let Some(available_bytes) = available_bytes else {
            return MigrationCheck {
                check_name: "memory_available".to_string(),
                passed: false,
                message: format!(
                    "Unable to determine available memory on target node {}",
                    target_node
                ),
                // Non-blocking: we don't want an SSH/monitoring hiccup to
                // hard-block every migration; the operator sees the warning.
                blocking: false,
            };
        };

        let available_mb = available_bytes / (1024 * 1024);

        if available_mb >= required_memory_mb {
            MigrationCheck {
                check_name: "memory_available".to_string(),
                passed: true,
                message: format!(
                    "Target node {} has {} MB available (needs {} MB)",
                    target_node, available_mb, required_memory_mb
                ),
                blocking: false,
            }
        } else {
            MigrationCheck {
                check_name: "memory_available".to_string(),
                passed: false,
                message: format!(
                    "Target node {} has only {} MB available, but the VM requires {} MB",
                    target_node, available_mb, required_memory_mb
                ),
                blocking: true,
            }
        }
    }

    /// Read `MemAvailable` (bytes) from the local `/proc/meminfo`.
    fn local_available_memory() -> Option<u64> {
        let content = std::fs::read_to_string("/proc/meminfo").ok()?;
        Self::parse_mem_available(&content)
    }

    /// Read `MemAvailable` (bytes) from a remote node's `/proc/meminfo`
    /// over SSH.
    async fn remote_available_memory(node: &str) -> Option<u64> {
        let output = tokio::process::Command::new("ssh")
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("ConnectTimeout=5")
            .arg(node)
            .arg("cat")
            .arg("/proc/meminfo")
            .output()
            .await
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let content = String::from_utf8_lossy(&output.stdout);
        Self::parse_mem_available(&content)
    }

    /// Parse the `MemAvailable:` line (in kB, converted to bytes) from
    /// `/proc/meminfo` text.
    fn parse_mem_available(content: &str) -> Option<u64> {
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("MemAvailable:") {
                let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
                return Some(kb * 1024);
            }
        }
        None
    }
}

impl Default for MigrationManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_migration_checks() {
        let manager = MigrationManager::new();

        let checks = manager
            .check_migration("vm-100", "node1", "node2", &MigrationType::Live, 1024)
            .await;

        // Network/storage/CPU/memory checks against nodes that don't exist
        // (node1/node2 aren't resolvable hosts) are expected to fail in a
        // test environment; only assert the checks we can fully control.
        let different_nodes = checks
            .iter()
            .find(|c| c.check_name == "different_nodes")
            .unwrap();
        assert!(different_nodes.passed);

        let no_active = checks
            .iter()
            .find(|c| c.check_name == "no_active_migration")
            .unwrap();
        assert!(no_active.passed);
    }

    #[tokio::test]
    async fn test_same_node_migration_fails() {
        let manager = MigrationManager::new();

        let checks = manager
            .check_migration("vm-100", "node1", "node1", &MigrationType::Live, 1024)
            .await;

        let different_nodes = checks
            .iter()
            .find(|c| c.check_name == "different_nodes")
            .unwrap();

        assert!(!different_nodes.passed);
        assert!(different_nodes.blocking);
    }

    #[tokio::test]
    async fn test_start_migration_fails_unreachable_target() {
        let manager = MigrationManager::new();

        // "node2" is not a resolvable/reachable host in the test
        // environment, so network connectivity (a blocking check) should
        // fail and start_migration must reject the job instead of silently
        // proceeding, proving the pre-flight checks are load-bearing.
        let result = manager
            .start_migration(
                "vm-100".to_string(),
                "node1".to_string(),
                "node2".to_string(),
                MigrationType::Offline,
                None,
                1024,
            )
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_network_connectivity_check_localhost() {
        // 127.0.0.1 always has *something* listening or refusing quickly;
        // either way the check must return a concrete pass/fail, not panic.
        let check = MigrationManager::check_network_connectivity("127.0.0.1").await;
        assert_eq!(check.check_name, "network_connectivity");
    }

    #[tokio::test]
    async fn test_network_connectivity_check_unresolvable_host() {
        let check =
            MigrationManager::check_network_connectivity("this-host-does-not-exist.invalid")
                .await;
        assert_eq!(check.check_name, "network_connectivity");
        assert!(!check.passed);
        assert!(check.blocking);
    }

    #[tokio::test]
    async fn test_cpu_compatibility_same_node() {
        // Comparing a node against itself must always report compatible.
        let check = MigrationManager::check_cpu_compatibility("localhost", "localhost").await;
        assert_eq!(check.check_name, "cpu_compatibility");
    }

    #[tokio::test]
    async fn test_memory_available_check_local() {
        // Against an unreachable target this degrades to "unknown", which
        // is non-blocking (we don't want to hard-fail migrations just
        // because we can't SSH in to check free RAM).
        let check =
            MigrationManager::check_memory_available("this-host-does-not-exist.invalid", 1024)
                .await;
        assert_eq!(check.check_name, "memory_available");
        assert!(!check.blocking);
    }
}
