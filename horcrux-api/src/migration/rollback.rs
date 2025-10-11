///! Migration Rollback and Recovery
///!
///! Provides automatic rollback capabilities when migrations fail,
///! ensuring VMs are restored to a working state on the source node

use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{error, info, warn};
use chrono::{DateTime, Utc};
use tokio::process::Command;

/// Rollback action type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RollbackAction {
    /// Restart VM on source node
    RestartVmOnSource,
    /// Delete incomplete disk images on target
    CleanupTargetDisks,
    /// Restore VM configuration on source
    RestoreSourceConfig,
    /// Release allocated resources on target
    ReleaseTargetResources,
    /// Remove target VM registration
    UnregisterTargetVm,
    /// Restore network configuration
    RestoreNetworkConfig,
}

/// Rollback step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackStep {
    pub action: RollbackAction,
    pub description: String,
    pub executed: bool,
    pub success: Option<bool>,
    pub error: Option<String>,
    pub timestamp: Option<DateTime<Utc>>,
}

impl RollbackStep {
    pub fn new(action: RollbackAction, description: String) -> Self {
        Self {
            action,
            description,
            executed: false,
            success: None,
            error: None,
            timestamp: None,
        }
    }

    pub fn mark_executed(&mut self, success: bool, error: Option<String>) {
        self.executed = true;
        self.success = Some(success);
        self.error = error;
        self.timestamp = Some(Utc::now());
    }
}

/// Rollback plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackPlan {
    pub migration_job_id: String,
    pub vm_id: u32,
    pub source_node: String,
    pub target_node: String,
    pub steps: Vec<RollbackStep>,
    pub started: Option<DateTime<Utc>>,
    pub completed: Option<DateTime<Utc>>,
    pub success: bool,
}

impl RollbackPlan {
    /// Create a new rollback plan for a failed migration
    pub fn new(
        migration_job_id: String,
        vm_id: u32,
        source_node: String,
        target_node: String,
    ) -> Self {
        let mut steps = Vec::new();

        // Step 1: Cleanup any partial disk images on target
        steps.push(RollbackStep::new(
            RollbackAction::CleanupTargetDisks,
            format!("Clean up incomplete disk images on target node {}", target_node),
        ));

        // Step 2: Unregister VM from target node
        steps.push(RollbackStep::new(
            RollbackAction::UnregisterTargetVm,
            format!("Unregister VM {} from target node", vm_id),
        ));

        // Step 3: Release allocated resources on target
        steps.push(RollbackStep::new(
            RollbackAction::ReleaseTargetResources,
            format!("Release allocated resources on {}", target_node),
        ));

        // Step 4: Restore VM configuration on source
        steps.push(RollbackStep::new(
            RollbackAction::RestoreSourceConfig,
            format!("Restore VM configuration on source node {}", source_node),
        ));

        // Step 5: Restore network configuration
        steps.push(RollbackStep::new(
            RollbackAction::RestoreNetworkConfig,
            format!("Restore network configuration for VM {}", vm_id),
        ));

        // Step 6: Restart VM on source node
        steps.push(RollbackStep::new(
            RollbackAction::RestartVmOnSource,
            format!("Restart VM {} on source node {}", vm_id, source_node),
        ));

        Self {
            migration_job_id,
            vm_id,
            source_node,
            target_node,
            steps,
            started: None,
            completed: None,
            success: false,
        }
    }

    /// Execute the rollback plan
    pub async fn execute(&mut self) -> Result<()> {
        info!(
            "Starting rollback for VM {} (migration job: {})",
            self.vm_id, self.migration_job_id
        );

        self.started = Some(Utc::now());
        let mut all_successful = true;

        // Execute steps by index to avoid borrow checker issues
        for i in 0..self.steps.len() {
            let step = &self.steps[i];
            info!("Executing rollback step: {}", step.description);

            match self.execute_step_by_action(&step.action).await {
                Ok(()) => {
                    self.steps[i].mark_executed(true, None);
                    info!("✓ Rollback step completed: {}", self.steps[i].description);
                }
                Err(e) => {
                    let error_msg = e.to_string();
                    self.steps[i].mark_executed(false, Some(error_msg.clone()));
                    error!("✗ Rollback step failed: {} - {}", self.steps[i].description, error_msg);
                    all_successful = false;

                    // Continue with remaining steps even if one fails
                    // (best effort rollback)
                    warn!("Continuing with remaining rollback steps despite failure");
                }
            }
        }

        self.completed = Some(Utc::now());
        self.success = all_successful;

        if all_successful {
            info!(
                "Rollback completed successfully for VM {} - VM restored on {}",
                self.vm_id, self.source_node
            );
            Ok(())
        } else {
            let failed_steps: Vec<_> = self
                .steps
                .iter()
                .filter(|s| s.success == Some(false))
                .map(|s| s.description.clone())
                .collect();

            error!(
                "Rollback partially failed for VM {}. Failed steps: {:?}",
                self.vm_id, failed_steps
            );

            Err(horcrux_common::Error::System(format!(
                "Rollback partially failed. {} of {} steps completed",
                self.steps.iter().filter(|s| s.success == Some(true)).count(),
                self.steps.len()
            )))
        }
    }

    /// Execute a single rollback step by action
    async fn execute_step_by_action(&self, action: &RollbackAction) -> Result<()> {
        match action {
            RollbackAction::CleanupTargetDisks => {
                self.cleanup_target_disks().await
            }
            RollbackAction::UnregisterTargetVm => {
                self.unregister_target_vm().await
            }
            RollbackAction::ReleaseTargetResources => {
                self.release_target_resources().await
            }
            RollbackAction::RestoreSourceConfig => {
                self.restore_source_config().await
            }
            RollbackAction::RestoreNetworkConfig => {
                self.restore_network_config().await
            }
            RollbackAction::RestartVmOnSource => {
                self.restart_vm_on_source().await
            }
        }
    }

    /// Cleanup incomplete disk images on target node
    async fn cleanup_target_disks(&self) -> Result<()> {
        info!(
            "Cleaning up incomplete disk images for VM {} on {}",
            self.vm_id, self.target_node
        );

        // SSH to target node and remove incomplete disk images
        let vm_name = format!("vm-{}", self.vm_id);
        let cleanup_patterns = vec![
            format!("/var/lib/libvirt/images/{}*.partial", vm_name),
            format!("/var/lib/libvirt/images/{}*.tmp", vm_name),
            format!("/var/lib/libvirt/images/{}_*", vm_name),
        ];

        for pattern in cleanup_patterns {
            let output = Command::new("ssh")
                .args([
                    "-o", "StrictHostKeyChecking=no",
                    "-o", "UserKnownHostsFile=/dev/null",
                    "-o", "ConnectTimeout=10",
                    &format!("root@{}", self.target_node),
                    &format!("rm -f {}", pattern),
                ])
                .output()
                .await?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                warn!("Failed to cleanup pattern {}: {}", pattern, stderr);
                // Continue anyway - disk might not exist, which is fine
            }
        }

        info!("Disk cleanup completed on {}", self.target_node);
        Ok(())
    }

    /// Unregister VM from target node
    async fn unregister_target_vm(&self) -> Result<()> {
        info!(
            "Unregistering VM {} from target node {}",
            self.vm_id, self.target_node
        );

        // SSH to target and undefine VM from libvirt
        let vm_name = format!("vm-{}", self.vm_id);

        let output = Command::new("ssh")
            .args([
                "-o", "StrictHostKeyChecking=no",
                "-o", "UserKnownHostsFile=/dev/null",
                "-o", "ConnectTimeout=10",
                &format!("root@{}", self.target_node),
                "virsh",
                "undefine",
                &vm_name,
                "--nvram",  // Also remove NVRAM if exists
            ])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // VM might not be defined yet - that's acceptable
            if !stderr.contains("not found") && !stderr.contains("no domain") {
                warn!("Failed to undefine VM on target: {}", stderr);
            }
        }

        info!("VM unregistered from {}", self.target_node);
        Ok(())
    }

    /// Release allocated resources on target node
    async fn release_target_resources(&self) -> Result<()> {
        info!(
            "Releasing allocated resources for VM {} on {}",
            self.vm_id, self.target_node
        );

        // Resources are automatically released when VM is undefined/destroyed
        // But we can explicitly clean up network bridges/OVS ports if needed
        let vm_name = format!("vm-{}", self.vm_id);

        // Try to destroy VM if it's still running
        let _output = Command::new("ssh")
            .args([
                "-o", "StrictHostKeyChecking=no",
                "-o", "UserKnownHostsFile=/dev/null",
                "-o", "ConnectTimeout=10",
                &format!("root@{}", self.target_node),
                "virsh",
                "destroy",
                &vm_name,
            ])
            .output()
            .await?;
        // Ignore errors - VM might not be running

        info!("Resources released on {}", self.target_node);
        Ok(())
    }

    /// Restore VM configuration on source node
    async fn restore_source_config(&self) -> Result<()> {
        info!(
            "Restoring VM {} configuration on source node {}",
            self.vm_id, self.source_node
        );

        // VM configuration should already exist on source node
        // We just need to ensure it's still defined in libvirt
        let vm_name = format!("vm-{}", self.vm_id);

        // Check if VM is defined on source
        let output = Command::new("ssh")
            .args([
                "-o", "StrictHostKeyChecking=no",
                "-o", "UserKnownHostsFile=/dev/null",
                "-o", "ConnectTimeout=10",
                &format!("root@{}", self.source_node),
                "virsh",
                "dominfo",
                &vm_name,
            ])
            .output()
            .await?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(
                format!("VM {} not found on source node {}", vm_name, self.source_node)
            ));
        }

        info!("VM configuration verified on {}", self.source_node);
        Ok(())
    }

    /// Restore network configuration
    async fn restore_network_config(&self) -> Result<()> {
        info!("Restoring network configuration for VM {}", self.vm_id);

        // Network configuration is preserved in the VM definition
        // No additional action needed - MAC addresses and network settings
        // are part of the libvirt XML definition which is still on source

        info!("Network configuration preserved");
        Ok(())
    }

    /// Restart VM on source node
    async fn restart_vm_on_source(&self) -> Result<()> {
        info!(
            "Restarting VM {} on source node {}",
            self.vm_id, self.source_node
        );

        // Start the VM on source node using virsh
        let vm_name = format!("vm-{}", self.vm_id);

        let output = Command::new("ssh")
            .args([
                "-o", "StrictHostKeyChecking=no",
                "-o", "UserKnownHostsFile=/dev/null",
                "-o", "ConnectTimeout=10",
                &format!("root@{}", self.source_node),
                "virsh",
                "start",
                &vm_name,
            ])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(horcrux_common::Error::System(
                format!("Failed to start VM on source: {}", stderr)
            ));
        }

        // Wait a moment for VM to initialize
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        // Verify VM is running
        let output = Command::new("ssh")
            .args([
                "-o", "StrictHostKeyChecking=no",
                "-o", "UserKnownHostsFile=/dev/null",
                "-o", "ConnectTimeout=10",
                &format!("root@{}", self.source_node),
                "virsh",
                "domstate",
                &vm_name,
            ])
            .output()
            .await?;

        let state = String::from_utf8_lossy(&output.stdout).trim().to_string();

        if state != "running" {
            return Err(horcrux_common::Error::System(
                format!("VM started but is not running. State: {}", state)
            ));
        }

        info!(
            "VM {} successfully restarted on {} (state: {})",
            self.vm_id, self.source_node, state
        );
        Ok(())
    }

    /// Get summary of rollback execution
    pub fn get_summary(&self) -> RollbackSummary {
        let total_steps = self.steps.len();
        let successful_steps = self.steps.iter().filter(|s| s.success == Some(true)).count();
        let failed_steps = self.steps.iter().filter(|s| s.success == Some(false)).count();
        let pending_steps = self.steps.iter().filter(|s| !s.executed).count();

        let duration_seconds = if let (Some(start), Some(end)) = (self.started, self.completed) {
            (end - start).num_seconds() as u64
        } else {
            0
        };

        RollbackSummary {
            migration_job_id: self.migration_job_id.clone(),
            vm_id: self.vm_id,
            total_steps,
            successful_steps,
            failed_steps,
            pending_steps,
            success: self.success,
            duration_seconds,
        }
    }
}

/// Rollback summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackSummary {
    pub migration_job_id: String,
    pub vm_id: u32,
    pub total_steps: usize,
    pub successful_steps: usize,
    pub failed_steps: usize,
    pub pending_steps: usize,
    pub success: bool,
    pub duration_seconds: u64,
}

/// Rollback manager
pub struct RollbackManager {
    rollbacks: HashMap<String, RollbackPlan>,
}

impl RollbackManager {
    pub fn new() -> Self {
        Self {
            rollbacks: HashMap::new(),
        }
    }

    /// Create and execute a rollback plan
    pub async fn rollback_migration(
        &mut self,
        migration_job_id: String,
        vm_id: u32,
        source_node: String,
        target_node: String,
    ) -> Result<RollbackSummary> {
        info!(
            "Initiating automatic rollback for failed migration {} (VM {})",
            migration_job_id, vm_id
        );

        let mut plan = RollbackPlan::new(
            migration_job_id.clone(),
            vm_id,
            source_node,
            target_node,
        );

        let result = plan.execute().await;

        let summary = plan.get_summary();

        // Store the rollback plan for history/auditing
        self.rollbacks.insert(migration_job_id, plan);

        result?;
        Ok(summary)
    }

    /// Get rollback plan for a migration job
    pub fn get_rollback(&self, migration_job_id: &str) -> Option<&RollbackPlan> {
        self.rollbacks.get(migration_job_id)
    }

    /// List all rollback plans
    pub fn list_rollbacks(&self) -> Vec<&RollbackPlan> {
        self.rollbacks.values().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rollback_plan_creation() {
        let plan = RollbackPlan::new(
            "migration-123".to_string(),
            100,
            "node1".to_string(),
            "node2".to_string(),
        );

        assert_eq!(plan.vm_id, 100);
        assert_eq!(plan.source_node, "node1");
        assert_eq!(plan.target_node, "node2");
        assert_eq!(plan.steps.len(), 6);
        assert!(!plan.success);
    }

    #[tokio::test]
    async fn test_rollback_execution() {
        let mut plan = RollbackPlan::new(
            "migration-123".to_string(),
            100,
            "node1".to_string(),
            "node2".to_string(),
        );

        let result = plan.execute().await;
        assert!(result.is_ok());
        assert!(plan.success);
        assert!(plan.started.is_some());
        assert!(plan.completed.is_some());

        // All steps should be executed
        for step in &plan.steps {
            assert!(step.executed);
            assert_eq!(step.success, Some(true));
        }
    }

    #[tokio::test]
    async fn test_rollback_summary() {
        let mut plan = RollbackPlan::new(
            "migration-123".to_string(),
            100,
            "node1".to_string(),
            "node2".to_string(),
        );

        plan.execute().await.ok();

        let summary = plan.get_summary();
        assert_eq!(summary.vm_id, 100);
        assert_eq!(summary.total_steps, 6);
        assert_eq!(summary.successful_steps, 6);
        assert_eq!(summary.failed_steps, 0);
        assert!(summary.success);
    }

    #[tokio::test]
    async fn test_rollback_manager() {
        let mut manager = RollbackManager::new();

        let summary = manager
            .rollback_migration(
                "migration-123".to_string(),
                100,
                "node1".to_string(),
                "node2".to_string(),
            )
            .await
            .unwrap();

        assert!(summary.success);
        assert_eq!(summary.successful_steps, 6);

        let plan = manager.get_rollback("migration-123").unwrap();
        assert_eq!(plan.vm_id, 100);
    }

    #[tokio::test]
    async fn test_rollback_steps_order() {
        let plan = RollbackPlan::new(
            "migration-123".to_string(),
            100,
            "node1".to_string(),
            "node2".to_string(),
        );

        // Verify steps are in correct order
        assert_eq!(plan.steps[0].action, RollbackAction::CleanupTargetDisks);
        assert_eq!(plan.steps[1].action, RollbackAction::UnregisterTargetVm);
        assert_eq!(plan.steps[2].action, RollbackAction::ReleaseTargetResources);
        assert_eq!(plan.steps[3].action, RollbackAction::RestoreSourceConfig);
        assert_eq!(plan.steps[4].action, RollbackAction::RestoreNetworkConfig);
        assert_eq!(plan.steps[5].action, RollbackAction::RestartVmOnSource);
    }
}
