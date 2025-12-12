///! Audit logging system
///!
///! Provides comprehensive security audit logs for compliance and security monitoring

pub mod database;
pub mod middleware;
pub mod rotation;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::io::AsyncWriteExt;

/// Audit event type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AuditEventType {
    // Authentication
    Login,
    Logout,
    LoginFailed,
    PasswordChanged,
    TwoFactorEnabled,
    TwoFactorDisabled,

    // Authorization
    PermissionGranted,
    PermissionDenied,
    RoleAssigned,
    RoleRevoked,

    // VM Operations
    VmCreated,
    VmDeleted,
    VmStarted,
    VmStopped,
    VmRestarted,
    VmMigrated,
    VmConfigChanged,

    // Storage Operations
    StoragePoolCreated,
    StoragePoolDeleted,
    VolumeCreated,
    VolumeDeleted,
    SnapshotCreated,
    SnapshotDeleted,

    // Backup Operations
    BackupCreated,
    BackupRestored,
    BackupDeleted,

    // Cluster Operations
    NodeAdded,
    NodeRemoved,
    ClusterJoined,
    ClusterLeft,

    // Configuration Changes
    ConfigChanged,
    SecretAccessed,
    CertificateIssued,
    CertificateRevoked,

    // Security Events
    SecurityPolicyChanged,
    FirewallRuleAdded,
    FirewallRuleDeleted,
    SuspiciousActivity,
    BruteForceDetected,
}

/// Audit severity level
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum AuditSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

/// Audit event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub timestamp: DateTime<Utc>,
    pub event_type: AuditEventType,
    pub severity: AuditSeverity,
    pub user: Option<String>,
    pub source_ip: Option<String>,
    pub resource: Option<String>,  // VM ID, storage pool, etc.
    pub action: String,
    pub result: AuditResult,
    pub details: Option<String>,
    pub session_id: Option<String>,
}

/// Audit result
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AuditResult {
    Success,
    Failure,
    Partial,
}

/// Audit logger
pub struct AuditLogger {
    events: Arc<RwLock<Vec<AuditEvent>>>,
    log_file: Option<PathBuf>,
    max_events_memory: usize,
    enabled: Arc<RwLock<bool>>,
}

impl AuditLogger {
    pub fn new(log_file: Option<PathBuf>) -> Self {
        Self {
            events: Arc::new(RwLock::new(Vec::new())),
            log_file,
            max_events_memory: 10000,  // Keep last 10000 events in memory
            enabled: Arc::new(RwLock::new(true)),
        }
    }

    /// Enable audit logging
    pub async fn enable(&self) {
        let mut enabled = self.enabled.write().await;
        *enabled = true;
        tracing::info!("Audit logging enabled");
    }

    /// Disable audit logging
    pub async fn disable(&self) {
        let mut enabled = self.enabled.write().await;
        *enabled = false;
        tracing::warn!("Audit logging disabled");
    }

    /// Log an audit event
    pub async fn log(&self, event: AuditEvent) {
        if !*self.enabled.read().await {
            return;
        }

        // Log to tracing
        match event.severity {
            AuditSeverity::Info => tracing::info!(
                event_type = ?event.event_type,
                user = ?event.user,
                action = %event.action,
                "Audit event"
            ),
            AuditSeverity::Warning => tracing::warn!(
                event_type = ?event.event_type,
                user = ?event.user,
                action = %event.action,
                "Audit event"
            ),
            AuditSeverity::Error => tracing::error!(
                event_type = ?event.event_type,
                user = ?event.user,
                action = %event.action,
                "Audit event"
            ),
            AuditSeverity::Critical => tracing::error!(
                event_type = ?event.event_type,
                user = ?event.user,
                action = %event.action,
                "CRITICAL audit event"
            ),
        }

        // Write to file if configured
        if let Some(ref log_file) = self.log_file {
            if let Err(e) = self.write_to_file(log_file, &event).await {
                tracing::error!("Failed to write audit log to file: {}", e);
            }
        }

        // Store in memory
        let mut events = self.events.write().await;
        events.push(event);

        // Trim if exceeds max
        if events.len() > self.max_events_memory {
            let drain_count = events.len() - self.max_events_memory;
            events.drain(0..drain_count);
        }
    }

    /// Write event to log file
    async fn write_to_file(&self, path: &PathBuf, event: &AuditEvent) -> Result<(), std::io::Error> {
        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;

        let json = serde_json::to_string(event).unwrap_or_default();
        file.write_all(format!("{}\n", json).as_bytes()).await?;

        Ok(())
    }

    /// Query audit logs
    pub async fn query(
        &self,
        event_type: Option<AuditEventType>,
        user: Option<String>,
        severity: Option<AuditSeverity>,
        start_time: Option<DateTime<Utc>>,
        end_time: Option<DateTime<Utc>>,
        limit: Option<usize>,
    ) -> Vec<AuditEvent> {
        let events = self.events.read().await;

        let filtered: Vec<AuditEvent> = events.iter()
            .filter(|e| {
                if let Some(ref et) = event_type {
                    if &e.event_type != et {
                        return false;
                    }
                }

                if let Some(ref u) = user {
                    if e.user.as_ref() != Some(u) {
                        return false;
                    }
                }

                if let Some(ref s) = severity {
                    if &e.severity != s {
                        return false;
                    }
                }

                if let Some(start) = start_time {
                    if e.timestamp < start {
                        return false;
                    }
                }

                if let Some(end) = end_time {
                    if e.timestamp > end {
                        return false;
                    }
                }

                true
            })
            .cloned()
            .collect();

        // Apply limit
        let limit = limit.unwrap_or(100);
        filtered.iter().rev().take(limit).cloned().collect()
    }

    /// Get event count by type
    pub async fn get_event_counts(&self) -> std::collections::HashMap<String, usize> {
        let events = self.events.read().await;
        let mut counts = std::collections::HashMap::new();

        for event in events.iter() {
            let type_str = format!("{:?}", event.event_type);
            *counts.entry(type_str).or_insert(0) += 1;
        }

        counts
    }

    /// Get failed login attempts
    pub async fn get_failed_logins(&self, user: Option<String>, limit: usize) -> Vec<AuditEvent> {
        self.query(
            Some(AuditEventType::LoginFailed),
            user,
            None,
            None,
            None,
            Some(limit),
        ).await
    }

    /// Detect brute force attacks
    pub async fn detect_brute_force(&self, threshold: usize, window_minutes: i64) -> Vec<String> {
        let events = self.events.read().await;
        let cutoff_time = Utc::now() - chrono::Duration::minutes(window_minutes);

        let mut failed_attempts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

        for event in events.iter() {
            if event.event_type == AuditEventType::LoginFailed && event.timestamp > cutoff_time {
                if let Some(ref user) = event.user {
                    *failed_attempts.entry(user.clone()).or_insert(0) += 1;
                }
            }
        }

        failed_attempts.iter()
            .filter(|(_, &count)| count >= threshold)
            .map(|(user, _)| user.clone())
            .collect()
    }

    /// Get security events
    pub async fn get_security_events(&self, limit: usize) -> Vec<AuditEvent> {
        let events = self.events.read().await;

        events.iter()
            .filter(|e| matches!(e.event_type,
                AuditEventType::SuspiciousActivity |
                AuditEventType::BruteForceDetected |
                AuditEventType::PermissionDenied
            ))
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }

    /// Export audit logs to file
    pub async fn export(&self, path: &str) -> Result<(), std::io::Error> {
        let events = self.events.read().await;

        let json = serde_json::to_string_pretty(&*events)?;
        tokio::fs::write(path, json).await?;

        Ok(())
    }
}

/// Helper function to create audit event
pub fn create_event(
    event_type: AuditEventType,
    severity: AuditSeverity,
    user: Option<String>,
    source_ip: Option<String>,
    action: String,
    result: AuditResult,
) -> AuditEvent {
    AuditEvent {
        timestamp: Utc::now(),
        event_type,
        severity,
        user,
        source_ip,
        resource: None,
        action,
        result,
        details: None,
        session_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_audit_logging() {
        let logger = AuditLogger::new(None);

        let event = create_event(
            AuditEventType::Login,
            AuditSeverity::Info,
            Some("admin".to_string()),
            Some("192.168.1.100".to_string()),
            "User logged in".to_string(),
            AuditResult::Success,
        );

        logger.log(event).await;

        let events = logger.query(None, None, None, None, None, None).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].user, Some("admin".to_string()));
    }

    #[tokio::test]
    async fn test_query_by_type() {
        let logger = AuditLogger::new(None);

        logger.log(create_event(
            AuditEventType::Login,
            AuditSeverity::Info,
            Some("user1".to_string()),
            None,
            "Login".to_string(),
            AuditResult::Success,
        )).await;

        logger.log(create_event(
            AuditEventType::VmCreated,
            AuditSeverity::Info,
            Some("user1".to_string()),
            None,
            "VM created".to_string(),
            AuditResult::Success,
        )).await;

        let logins = logger.query(
            Some(AuditEventType::Login),
            None,
            None,
            None,
            None,
            None,
        ).await;

        assert_eq!(logins.len(), 1);
        assert_eq!(logins[0].event_type, AuditEventType::Login);
    }

    #[tokio::test]
    async fn test_brute_force_detection() {
        let logger = AuditLogger::new(None);

        // Simulate 5 failed login attempts
        for _ in 0..5 {
            logger.log(create_event(
                AuditEventType::LoginFailed,
                AuditSeverity::Warning,
                Some("attacker".to_string()),
                Some("10.0.0.1".to_string()),
                "Failed login".to_string(),
                AuditResult::Failure,
            )).await;
        }

        let suspects = logger.detect_brute_force(3, 10).await;
        assert_eq!(suspects.len(), 1);
        assert_eq!(suspects[0], "attacker");
    }

    #[tokio::test]
    async fn test_event_counts() {
        let logger = AuditLogger::new(None);

        logger.log(create_event(
            AuditEventType::Login,
            AuditSeverity::Info,
            None,
            None,
            "Login".to_string(),
            AuditResult::Success,
        )).await;

        logger.log(create_event(
            AuditEventType::Login,
            AuditSeverity::Info,
            None,
            None,
            "Login".to_string(),
            AuditResult::Success,
        )).await;

        logger.log(create_event(
            AuditEventType::VmCreated,
            AuditSeverity::Info,
            None,
            None,
            "VM created".to_string(),
            AuditResult::Success,
        )).await;

        let counts = logger.get_event_counts().await;
        assert_eq!(counts.get("Login"), Some(&2));
        assert_eq!(counts.get("VmCreated"), Some(&1));
    }
}
