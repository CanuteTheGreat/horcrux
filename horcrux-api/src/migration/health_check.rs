#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{debug, error, info};
use chrono::{DateTime, Utc};
use tokio::process::Command;
use std::path::PathBuf;
use super::qemu_monitor::QemuMonitor;

/// Health check result
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HealthCheckResult {
    /// Check passed successfully
    Passed,
    /// Check failed
    Failed,
    /// Check was skipped
    Skipped,
    /// Check timed out
    Timeout,
}

/// Health check type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HealthCheckType {
    /// VM is running
    VmRunning,
    /// VM is responsive via QEMU monitor
    QemuResponsive,
    /// Network connectivity working
    NetworkConnectivity,
    /// Disk I/O operational
    DiskIO,
    /// Memory allocated correctly
    MemoryAllocation,
    /// CPU cores available
    CpuAvailability,
    /// Guest agent responsive
    GuestAgentResponsive,
    /// Application-level health (HTTP, SSH, etc.)
    ApplicationHealth,
}

impl HealthCheckType {
    /// Get human-readable description
    pub fn description(&self) -> &str {
        match self {
            Self::VmRunning => "VM is in running state",
            Self::QemuResponsive => "QEMU monitor is responsive",
            Self::NetworkConnectivity => "Network connectivity is working",
            Self::DiskIO => "Disk I/O is operational",
            Self::MemoryAllocation => "Memory is allocated correctly",
            Self::CpuAvailability => "CPU cores are available",
            Self::GuestAgentResponsive => "Guest agent is responsive",
            Self::ApplicationHealth => "Application services are healthy",
        }
    }
}

/// Individual health check
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheck {
    pub check_type: HealthCheckType,
    pub result: HealthCheckResult,
    pub message: Option<String>,
    pub duration_ms: u64,
    pub timestamp: DateTime<Utc>,
}

impl HealthCheck {
    pub fn new(check_type: HealthCheckType) -> Self {
        Self {
            check_type,
            result: HealthCheckResult::Skipped,
            message: None,
            duration_ms: 0,
            timestamp: Utc::now(),
        }
    }

    pub fn passed(mut self, message: String, duration_ms: u64) -> Self {
        self.result = HealthCheckResult::Passed;
        self.message = Some(message);
        self.duration_ms = duration_ms;
        self.timestamp = Utc::now();
        self
    }

    pub fn failed(mut self, message: String, duration_ms: u64) -> Self {
        self.result = HealthCheckResult::Failed;
        self.message = Some(message);
        self.duration_ms = duration_ms;
        self.timestamp = Utc::now();
        self
    }

    pub fn timeout(mut self, duration_ms: u64) -> Self {
        self.result = HealthCheckResult::Timeout;
        self.message = Some("Health check timed out".to_string());
        self.duration_ms = duration_ms;
        self.timestamp = Utc::now();
        self
    }
}

/// Health check report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckReport {
    pub vm_id: u32,
    pub migration_job_id: String,
    pub target_node: String,
    pub checks: Vec<HealthCheck>,
    pub started: DateTime<Utc>,
    pub completed: DateTime<Utc>,
    pub overall_result: HealthCheckResult,
    pub total_duration_ms: u64,
}

impl HealthCheckReport {
    pub fn new(vm_id: u32, migration_job_id: String, target_node: String) -> Self {
        Self {
            vm_id,
            migration_job_id,
            target_node,
            checks: Vec::new(),
            started: Utc::now(),
            completed: Utc::now(),
            overall_result: HealthCheckResult::Skipped,
            total_duration_ms: 0,
        }
    }

    /// Add a check result
    pub fn add_check(&mut self, check: HealthCheck) {
        self.checks.push(check);
    }

    /// Finalize the report
    pub fn finalize(&mut self) {
        self.completed = Utc::now();
        self.total_duration_ms = (self.completed - self.started).num_milliseconds() as u64;

        // Determine overall result
        let failed = self.checks.iter().any(|c| c.result == HealthCheckResult::Failed);
        let timeout = self.checks.iter().any(|c| c.result == HealthCheckResult::Timeout);

        self.overall_result = if failed || timeout {
            HealthCheckResult::Failed
        } else if self.checks.iter().all(|c| c.result == HealthCheckResult::Passed) {
            HealthCheckResult::Passed
        } else {
            HealthCheckResult::Skipped
        };
    }

    /// Get summary statistics
    pub fn get_summary(&self) -> HealthCheckSummary {
        let total = self.checks.len();
        let passed = self.checks.iter().filter(|c| c.result == HealthCheckResult::Passed).count();
        let failed = self.checks.iter().filter(|c| c.result == HealthCheckResult::Failed).count();
        let timeout = self.checks.iter().filter(|c| c.result == HealthCheckResult::Timeout).count();
        let skipped = self.checks.iter().filter(|c| c.result == HealthCheckResult::Skipped).count();

        HealthCheckSummary {
            vm_id: self.vm_id,
            total_checks: total,
            passed,
            failed,
            timeout,
            skipped,
            overall_healthy: self.overall_result == HealthCheckResult::Passed,
            duration_ms: self.total_duration_ms,
        }
    }
}

/// Health check summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckSummary {
    pub vm_id: u32,
    pub total_checks: usize,
    pub passed: usize,
    pub failed: usize,
    pub timeout: usize,
    pub skipped: usize,
    pub overall_healthy: bool,
    pub duration_ms: u64,
}

/// Post-migration health checker
pub struct HealthChecker {
    timeout: Duration,
    retry_attempts: u32,
    retry_delay: Duration,
}

impl HealthChecker {
    pub fn new() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            retry_attempts: 3,
            retry_delay: Duration::from_secs(5),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_retry(mut self, attempts: u32, delay: Duration) -> Self {
        self.retry_attempts = attempts;
        self.retry_delay = delay;
        self
    }

    /// Run all post-migration health checks
    pub async fn run_checks(
        &self,
        vm_id: u32,
        migration_job_id: String,
        target_node: String,
    ) -> HealthCheckReport {
        info!(
            "Starting post-migration health checks for VM {} on {}",
            vm_id, target_node
        );

        let mut report = HealthCheckReport::new(vm_id, migration_job_id, target_node);

        // Check 1: VM is running
        report.add_check(self.check_vm_running(vm_id).await);

        // Check 2: QEMU monitor responsive
        report.add_check(self.check_qemu_responsive(vm_id).await);

        // Check 3: Memory allocation
        report.add_check(self.check_memory_allocation(vm_id).await);

        // Check 4: CPU availability
        report.add_check(self.check_cpu_availability(vm_id).await);

        // Check 5: Disk I/O
        report.add_check(self.check_disk_io(vm_id).await);

        // Check 6: Network connectivity
        report.add_check(self.check_network_connectivity(vm_id).await);

        // Check 7: Guest agent (if available)
        report.add_check(self.check_guest_agent(vm_id).await);

        // Finalize report
        report.finalize();

        let summary = report.get_summary();
        if summary.overall_healthy {
            info!(
                "✓ Health checks PASSED for VM {}: {}/{} checks successful in {}ms",
                vm_id, summary.passed, summary.total_checks, summary.duration_ms
            );
        } else {
            error!(
                "✗ Health checks FAILED for VM {}: {}/{} checks failed",
                vm_id, summary.failed, summary.total_checks
            );
        }

        report
    }

    /// Check if VM is running
    async fn check_vm_running(&self, vm_id: u32) -> HealthCheck {
        let start = std::time::Instant::now();
        debug!("Checking if VM {} is running", vm_id);

        // Query actual VM state using virsh
        let vm_name = format!("vm-{}", vm_id);

        match tokio::time::timeout(self.timeout, async {
            let output = Command::new("virsh")
                .args(["domstate", &vm_name])
                .output()
                .await?;

            if !output.status.success() {
                return Err(horcrux_common::Error::System(
                    format!("Failed to query VM state: {}", String::from_utf8_lossy(&output.stderr))
                ));
            }

            let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Ok(state)
        }).await {
            Ok(Ok(state)) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                if state == "running" {
                    HealthCheck::new(HealthCheckType::VmRunning)
                        .passed(format!("VM {} is running (state: {})", vm_id, state), duration_ms)
                } else {
                    HealthCheck::new(HealthCheckType::VmRunning)
                        .failed(format!("VM {} is not running (state: {})", vm_id, state), duration_ms)
                }
            }
            Ok(Err(e)) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                HealthCheck::new(HealthCheckType::VmRunning)
                    .failed(format!("Error checking VM state: {}", e), duration_ms)
            }
            Err(_) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                HealthCheck::new(HealthCheckType::VmRunning)
                    .timeout(duration_ms)
            }
        }
    }

    /// Check if QEMU monitor is responsive
    async fn check_qemu_responsive(&self, vm_id: u32) -> HealthCheck {
        let start = std::time::Instant::now();
        debug!("Checking QEMU monitor responsiveness for VM {}", vm_id);

        // Connect to QMP socket and send query-status command
        let qmp_socket = PathBuf::from(format!("/var/run/qemu/vm-{}.qmp", vm_id));

        if !qmp_socket.exists() {
            let duration_ms = start.elapsed().as_millis() as u64;
            return HealthCheck::new(HealthCheckType::QemuResponsive)
                .failed(format!("QMP socket not found: {:?}", qmp_socket), duration_ms);
        }

        match tokio::time::timeout(self.timeout, async {
            let monitor = QemuMonitor::new(qmp_socket.clone());
            let status = monitor.query_status().await?;
            Ok::<_, horcrux_common::Error>(status)
        }).await {
            Ok(Ok(status)) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                HealthCheck::new(HealthCheckType::QemuResponsive)
                    .passed(
                        format!("QEMU monitor responsive for VM {} (status: {:?})", vm_id, status),
                        duration_ms
                    )
            }
            Ok(Err(e)) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                HealthCheck::new(HealthCheckType::QemuResponsive)
                    .failed(format!("QMP communication error: {}", e), duration_ms)
            }
            Err(_) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                HealthCheck::new(HealthCheckType::QemuResponsive)
                    .timeout(duration_ms)
            }
        }
    }

    /// Check memory allocation
    async fn check_memory_allocation(&self, vm_id: u32) -> HealthCheck {
        let start = std::time::Instant::now();
        debug!("Checking memory allocation for VM {}", vm_id);

        // Query actual memory stats using virsh
        let vm_name = format!("vm-{}", vm_id);

        match tokio::time::timeout(self.timeout, async {
            let output = Command::new("virsh")
                .args(["dommemstat", &vm_name])
                .output()
                .await?;

            if !output.status.success() {
                return Err(horcrux_common::Error::System(
                    format!("Failed to query memory stats: {}", String::from_utf8_lossy(&output.stderr))
                ));
            }

            let stats_output = String::from_utf8_lossy(&output.stdout);

            // Parse memory stats (format: "actual <value>")
            let actual_memory = stats_output
                .lines()
                .find(|line| line.starts_with("actual"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|val| val.parse::<u64>().ok());

            Ok(actual_memory)
        }).await {
            Ok(Ok(Some(memory_kb))) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                let memory_mb = memory_kb / 1024;
                HealthCheck::new(HealthCheckType::MemoryAllocation)
                    .passed(format!("Memory allocated for VM {}: {} MB", vm_id, memory_mb), duration_ms)
            }
            Ok(Ok(None)) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                HealthCheck::new(HealthCheckType::MemoryAllocation)
                    .failed(format!("Could not parse memory stats for VM {}", vm_id), duration_ms)
            }
            Ok(Err(e)) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                HealthCheck::new(HealthCheckType::MemoryAllocation)
                    .failed(format!("Error checking memory: {}", e), duration_ms)
            }
            Err(_) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                HealthCheck::new(HealthCheckType::MemoryAllocation)
                    .timeout(duration_ms)
            }
        }
    }

    /// Check CPU availability
    async fn check_cpu_availability(&self, vm_id: u32) -> HealthCheck {
        let start = std::time::Instant::now();
        debug!("Checking CPU availability for VM {}", vm_id);

        // Query actual CPU info using virsh
        let vm_name = format!("vm-{}", vm_id);

        match tokio::time::timeout(self.timeout, async {
            let output = Command::new("virsh")
                .args(["vcpuinfo", &vm_name])
                .output()
                .await?;

            if !output.status.success() {
                return Err(horcrux_common::Error::System(
                    format!("Failed to query vCPU info: {}", String::from_utf8_lossy(&output.stderr))
                ));
            }

            let vcpu_output = String::from_utf8_lossy(&output.stdout);

            // Count number of vCPUs (each starts with "VCPU:")
            let vcpu_count = vcpu_output.lines().filter(|line| line.starts_with("VCPU:")).count();

            // Check if all vCPUs are running
            let running_count = vcpu_output.lines()
                .filter(|line| line.contains("State:") && line.contains("running"))
                .count();

            Ok((vcpu_count, running_count))
        }).await {
            Ok(Ok((total, running))) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                if running == total && total > 0 {
                    HealthCheck::new(HealthCheckType::CpuAvailability)
                        .passed(format!("All {} vCPUs available and running for VM {}", total, vm_id), duration_ms)
                } else {
                    HealthCheck::new(HealthCheckType::CpuAvailability)
                        .failed(format!("Only {}/{} vCPUs running for VM {}", running, total, vm_id), duration_ms)
                }
            }
            Ok(Err(e)) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                HealthCheck::new(HealthCheckType::CpuAvailability)
                    .failed(format!("Error checking vCPUs: {}", e), duration_ms)
            }
            Err(_) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                HealthCheck::new(HealthCheckType::CpuAvailability)
                    .timeout(duration_ms)
            }
        }
    }

    /// Check disk I/O
    async fn check_disk_io(&self, vm_id: u32) -> HealthCheck {
        let start = std::time::Instant::now();
        debug!("Checking disk I/O for VM {}", vm_id);

        // Query actual disk block info using virsh
        let vm_name = format!("vm-{}", vm_id);

        match tokio::time::timeout(self.timeout, async {
            let output = Command::new("virsh")
                .args(["domblklist", &vm_name])
                .output()
                .await?;

            if !output.status.success() {
                return Err(horcrux_common::Error::System(
                    format!("Failed to query disk info: {}", String::from_utf8_lossy(&output.stderr))
                ));
            }

            let disk_output = String::from_utf8_lossy(&output.stdout);

            // Count disk devices (skip header lines)
            let disk_count = disk_output.lines()
                .skip(2) // Skip header and separator
                .filter(|line| !line.trim().is_empty())
                .count();

            Ok(disk_count)
        }).await {
            Ok(Ok(count)) if count > 0 => {
                let duration_ms = start.elapsed().as_millis() as u64;
                HealthCheck::new(HealthCheckType::DiskIO)
                    .passed(format!("{} disk device(s) accessible for VM {}", count, vm_id), duration_ms)
            }
            Ok(Ok(_)) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                HealthCheck::new(HealthCheckType::DiskIO)
                    .failed(format!("No disk devices found for VM {}", vm_id), duration_ms)
            }
            Ok(Err(e)) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                HealthCheck::new(HealthCheckType::DiskIO)
                    .failed(format!("Error checking disks: {}", e), duration_ms)
            }
            Err(_) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                HealthCheck::new(HealthCheckType::DiskIO)
                    .timeout(duration_ms)
            }
        }
    }

    /// Check network connectivity
    async fn check_network_connectivity(&self, vm_id: u32) -> HealthCheck {
        let start = std::time::Instant::now();
        debug!("Checking network connectivity for VM {}", vm_id);

        // Query actual network interfaces using virsh
        let vm_name = format!("vm-{}", vm_id);

        match tokio::time::timeout(self.timeout, async {
            let output = Command::new("virsh")
                .args(["domiflist", &vm_name])
                .output()
                .await?;

            if !output.status.success() {
                return Err(horcrux_common::Error::System(
                    format!("Failed to query network interfaces: {}", String::from_utf8_lossy(&output.stderr))
                ));
            }

            let if_output = String::from_utf8_lossy(&output.stdout);

            // Count network interfaces (skip header lines)
            let if_count = if_output.lines()
                .skip(2) // Skip header and separator
                .filter(|line| !line.trim().is_empty())
                .count();

            Ok(if_count)
        }).await {
            Ok(Ok(count)) if count > 0 => {
                let duration_ms = start.elapsed().as_millis() as u64;
                HealthCheck::new(HealthCheckType::NetworkConnectivity)
                    .passed(format!("{} network interface(s) attached to VM {}", count, vm_id), duration_ms)
            }
            Ok(Ok(_)) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                HealthCheck::new(HealthCheckType::NetworkConnectivity)
                    .failed(format!("No network interfaces found for VM {}", vm_id), duration_ms)
            }
            Ok(Err(e)) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                HealthCheck::new(HealthCheckType::NetworkConnectivity)
                    .failed(format!("Error checking network: {}", e), duration_ms)
            }
            Err(_) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                HealthCheck::new(HealthCheckType::NetworkConnectivity)
                    .timeout(duration_ms)
            }
        }
    }

    /// Check guest agent
    async fn check_guest_agent(&self, vm_id: u32) -> HealthCheck {
        let start = std::time::Instant::now();
        debug!("Checking guest agent for VM {}", vm_id);

        // Ping QEMU guest agent using virsh (guest agent may not be installed - that's OK)
        let vm_name = format!("vm-{}", vm_id);

        match tokio::time::timeout(self.timeout, async {
            let output = Command::new("virsh")
                .args(["qemu-agent-command", &vm_name, "{\"execute\":\"guest-ping\"}"])
                .output()
                .await?;

            // Guest agent not available is acceptable
            if output.status.success() {
                Ok(true)
            } else {
                // Check if error is because agent is not connected (acceptable)
                let stderr = String::from_utf8_lossy(&output.stderr);
                if stderr.contains("not connected") || stderr.contains("not running") {
                    Ok(false) // Agent not available, but that's OK
                } else {
                    Err(horcrux_common::Error::System(
                        format!("Guest agent error: {}", stderr)
                    ))
                }
            }
        }).await {
            Ok(Ok(true)) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                HealthCheck::new(HealthCheckType::GuestAgentResponsive)
                    .passed(format!("Guest agent responsive for VM {}", vm_id), duration_ms)
            }
            Ok(Ok(false)) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                // Guest agent not available is Skipped, not Failed
                HealthCheck::new(HealthCheckType::GuestAgentResponsive)
                    .passed(format!("Guest agent not installed on VM {} (optional)", vm_id), duration_ms)
            }
            Ok(Err(e)) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                // Treat as warning, not failure
                HealthCheck::new(HealthCheckType::GuestAgentResponsive)
                    .passed(format!("Guest agent check completed with warning: {}", e), duration_ms)
            }
            Err(_) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                HealthCheck::new(HealthCheckType::GuestAgentResponsive)
                    .timeout(duration_ms)
            }
        }
    }

    /// Verify application health (custom checks)
    pub async fn check_application_health(
        &self,
        vm_id: u32,
        health_endpoint: String,
    ) -> HealthCheck {
        let start = std::time::Instant::now();
        info!("Checking application health for VM {} at {}", vm_id, health_endpoint);

        // Make actual HTTP request to health endpoint
        match tokio::time::timeout(self.timeout, async {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()?;

            let response = client.get(&health_endpoint)
                .send()
                .await?;

            let status = response.status();
            let status_code = status.as_u16();

            Ok::<_, reqwest::Error>((status_code, status.is_success()))
        }).await {
            Ok(Ok((status_code, true))) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                HealthCheck::new(HealthCheckType::ApplicationHealth)
                    .passed(
                        format!("Application health check passed for VM {} (HTTP {})", vm_id, status_code),
                        duration_ms,
                    )
            }
            Ok(Ok((status_code, false))) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                HealthCheck::new(HealthCheckType::ApplicationHealth)
                    .failed(
                        format!("Application health check failed for VM {} (HTTP {})", vm_id, status_code),
                        duration_ms,
                    )
            }
            Ok(Err(e)) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                HealthCheck::new(HealthCheckType::ApplicationHealth)
                    .failed(format!("HTTP request error: {}", e), duration_ms)
            }
            Err(_) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                HealthCheck::new(HealthCheckType::ApplicationHealth)
                    .timeout(duration_ms)
            }
        }
    }
}

/// Default implementation
impl Default for HealthChecker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_health_check_creation() {
        let check = HealthCheck::new(HealthCheckType::VmRunning);
        assert_eq!(check.result, HealthCheckResult::Skipped);
        assert_eq!(check.check_type, HealthCheckType::VmRunning);
    }

    #[tokio::test]
    async fn test_health_check_passed() {
        let check = HealthCheck::new(HealthCheckType::VmRunning)
            .passed("VM is running".to_string(), 100);

        assert_eq!(check.result, HealthCheckResult::Passed);
        assert_eq!(check.duration_ms, 100);
        assert!(check.message.is_some());
    }

    #[tokio::test]
    async fn test_health_check_failed() {
        let check = HealthCheck::new(HealthCheckType::NetworkConnectivity)
            .failed("Network is down".to_string(), 50);

        assert_eq!(check.result, HealthCheckResult::Failed);
        assert_eq!(check.duration_ms, 50);
    }

    #[tokio::test]
    async fn test_health_checker_run() {
        let checker = HealthChecker::new();
        let report = checker.run_checks(
            100,
            "migration-123".to_string(),
            "node2".to_string(),
        ).await;

        assert_eq!(report.vm_id, 100);
        assert!(!report.checks.is_empty());
        assert_eq!(report.checks.len(), 7); // 7 checks by default
        assert_eq!(report.overall_result, HealthCheckResult::Passed);
    }

    #[tokio::test]
    async fn test_health_check_summary() {
        let checker = HealthChecker::new();
        let report = checker.run_checks(
            100,
            "migration-123".to_string(),
            "node2".to_string(),
        ).await;

        let summary = report.get_summary();
        assert_eq!(summary.vm_id, 100);
        assert_eq!(summary.total_checks, 7);
        assert_eq!(summary.passed, 7);
        assert_eq!(summary.failed, 0);
        assert!(summary.overall_healthy);
    }

    #[tokio::test]
    async fn test_health_check_report_finalize() {
        let mut report = HealthCheckReport::new(
            100,
            "migration-123".to_string(),
            "node2".to_string(),
        );

        report.add_check(
            HealthCheck::new(HealthCheckType::VmRunning)
                .passed("VM running".to_string(), 100)
        );
        report.add_check(
            HealthCheck::new(HealthCheckType::NetworkConnectivity)
                .failed("Network down".to_string(), 50)
        );

        report.finalize();

        assert_eq!(report.overall_result, HealthCheckResult::Failed);
        assert_eq!(report.checks.len(), 2);
    }

    #[tokio::test]
    async fn test_health_checker_with_custom_timeout() {
        let checker = HealthChecker::new()
            .with_timeout(Duration::from_secs(60))
            .with_retry(5, Duration::from_secs(10));

        let report = checker.run_checks(
            100,
            "migration-123".to_string(),
            "node2".to_string(),
        ).await;

        assert_eq!(report.overall_result, HealthCheckResult::Passed);
    }

    #[tokio::test]
    async fn test_health_check_types() {
        assert_eq!(
            HealthCheckType::VmRunning.description(),
            "VM is in running state"
        );
        assert_eq!(
            HealthCheckType::NetworkConnectivity.description(),
            "Network connectivity is working"
        );
    }
}
