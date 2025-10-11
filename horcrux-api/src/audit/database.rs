///! Database-backed audit logging
///!
///! Provides persistent audit trail storage using SQLite with advanced querying capabilities

use super::{AuditEvent, AuditEventType, AuditResult, AuditSeverity};
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use tracing::{error, info};

use horcrux_common::Result;

/// Database audit logger
pub struct DatabaseAuditLogger {
    db: SqlitePool,
}

impl DatabaseAuditLogger {
    /// Create new database audit logger
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }

    /// Initialize audit log schema
    pub async fn init_schema(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS audit_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                event_type TEXT NOT NULL,
                severity TEXT NOT NULL,
                username TEXT,
                source_ip TEXT,
                resource TEXT,
                action TEXT NOT NULL,
                result TEXT NOT NULL,
                details TEXT,
                session_id TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_audit_timestamp ON audit_events(timestamp);
            CREATE INDEX IF NOT EXISTS idx_audit_username ON audit_events(username);
            CREATE INDEX IF NOT EXISTS idx_audit_event_type ON audit_events(event_type);
            CREATE INDEX IF NOT EXISTS idx_audit_severity ON audit_events(severity);
            CREATE INDEX IF NOT EXISTS idx_audit_resource ON audit_events(resource);
            CREATE INDEX IF NOT EXISTS idx_audit_session ON audit_events(session_id);
            "#,
        )
        .execute(&self.db)
        .await
        .map_err(|e| {
            horcrux_common::Error::System(format!("Failed to initialize audit schema: {}", e))
        })?;

        info!("Database audit logging schema initialized");
        Ok(())
    }

    /// Log an audit event to database
    pub async fn log(&self, event: &AuditEvent) -> Result<()> {
        let event_type_str = format!("{:?}", event.event_type);
        let severity_str = format!("{:?}", event.severity);
        let result_str = format!("{:?}", event.result);

        sqlx::query(
            r#"
            INSERT INTO audit_events (
                timestamp, event_type, severity, username, source_ip,
                resource, action, result, details, session_id
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(event.timestamp.timestamp())
        .bind(&event_type_str)
        .bind(&severity_str)
        .bind(&event.user)
        .bind(&event.source_ip)
        .bind(&event.resource)
        .bind(&event.action)
        .bind(&result_str)
        .bind(&event.details)
        .bind(&event.session_id)
        .execute(&self.db)
        .await
        .map_err(|e| {
            error!("Failed to insert audit event: {}", e);
            horcrux_common::Error::System(format!("Failed to log audit event: {}", e))
        })?;

        Ok(())
    }

    /// Query audit events with filters
    pub async fn query(
        &self,
        event_type: Option<AuditEventType>,
        user: Option<String>,
        severity: Option<AuditSeverity>,
        start_time: Option<DateTime<Utc>>,
        end_time: Option<DateTime<Utc>>,
        limit: Option<usize>,
    ) -> Result<Vec<AuditEvent>> {
        let mut sql = String::from("SELECT * FROM audit_events WHERE 1=1");
        let mut conditions = Vec::new();

        if let Some(et) = event_type {
            conditions.push(format!("event_type = '{:?}'", et));
        }

        if let Some(ref u) = user {
            conditions.push(format!("username = '{}'", u));
        }

        if let Some(s) = severity {
            conditions.push(format!("severity = '{:?}'", s));
        }

        if let Some(start) = start_time {
            conditions.push(format!("timestamp >= {}", start.timestamp()));
        }

        if let Some(end) = end_time {
            conditions.push(format!("timestamp < {}", end.timestamp()));
        }

        for condition in conditions {
            sql.push_str(" AND ");
            sql.push_str(&condition);
        }

        sql.push_str(" ORDER BY timestamp DESC");

        if let Some(l) = limit {
            sql.push_str(&format!(" LIMIT {}", l));
        }

        let rows = sqlx::query(&sql).fetch_all(&self.db).await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to query audit events: {}", e))
        })?;

        let mut events = Vec::new();
        for row in rows {
            let timestamp_secs: i64 = row.get("timestamp");
            let event_type_str: String = row.get("event_type");
            let severity_str: String = row.get("severity");
            let result_str: String = row.get("result");

            events.push(AuditEvent {
                timestamp: DateTime::from_timestamp(timestamp_secs, 0).unwrap_or_else(|| Utc::now()),
                event_type: Self::parse_event_type(&event_type_str),
                severity: Self::parse_severity(&severity_str),
                user: row.get("username"),
                source_ip: row.get("source_ip"),
                resource: row.get("resource"),
                action: row.get("action"),
                result: Self::parse_result(&result_str),
                details: row.get("details"),
                session_id: row.get("session_id"),
            });
        }

        Ok(events)
    }

    /// Get event counts by type
    pub async fn get_event_counts(&self) -> Result<std::collections::HashMap<String, i64>> {
        let rows = sqlx::query("SELECT event_type, COUNT(*) as count FROM audit_events GROUP BY event_type")
            .fetch_all(&self.db)
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to get event counts: {}", e))
            })?;

        let mut counts = std::collections::HashMap::new();
        for row in rows {
            let event_type: String = row.get("event_type");
            let count: i64 = row.get("count");
            counts.insert(event_type, count);
        }

        Ok(counts)
    }

    /// Get failed login attempts
    pub async fn get_failed_logins(&self, user: Option<String>, limit: usize) -> Result<Vec<AuditEvent>> {
        self.query(
            Some(AuditEventType::LoginFailed),
            user,
            None,
            None,
            None,
            Some(limit),
        )
        .await
    }

    /// Detect brute force attacks
    pub async fn detect_brute_force(
        &self,
        threshold: i64,
        window_minutes: i64,
    ) -> Result<Vec<String>> {
        let cutoff_time = Utc::now() - chrono::Duration::minutes(window_minutes);

        let rows = sqlx::query(
            r#"
            SELECT username, COUNT(*) as count
            FROM audit_events
            WHERE event_type = 'LoginFailed'
              AND timestamp > ?
              AND username IS NOT NULL
            GROUP BY username
            HAVING count >= ?
            "#,
        )
        .bind(cutoff_time.timestamp())
        .bind(threshold)
        .fetch_all(&self.db)
        .await
        .map_err(|e| {
            horcrux_common::Error::System(format!("Failed to detect brute force: {}", e))
        })?;

        let mut suspects = Vec::new();
        for row in rows {
            let username: String = row.get("username");
            suspects.push(username);
        }

        Ok(suspects)
    }

    /// Get security events (suspicious activity, permission denials, etc.)
    pub async fn get_security_events(&self, limit: usize) -> Result<Vec<AuditEvent>> {
        let rows = sqlx::query(
            r#"
            SELECT * FROM audit_events
            WHERE event_type IN ('SuspiciousActivity', 'BruteForceDetected', 'PermissionDenied')
            ORDER BY timestamp DESC
            LIMIT ?
            "#,
        )
        .bind(limit as i64)
        .fetch_all(&self.db)
        .await
        .map_err(|e| {
            horcrux_common::Error::System(format!("Failed to get security events: {}", e))
        })?;

        let mut events = Vec::new();
        for row in rows {
            let timestamp_secs: i64 = row.get("timestamp");
            let event_type_str: String = row.get("event_type");
            let severity_str: String = row.get("severity");
            let result_str: String = row.get("result");

            events.push(AuditEvent {
                timestamp: DateTime::from_timestamp(timestamp_secs, 0).unwrap_or_else(|| Utc::now()),
                event_type: Self::parse_event_type(&event_type_str),
                severity: Self::parse_severity(&severity_str),
                user: row.get("username"),
                source_ip: row.get("source_ip"),
                resource: row.get("resource"),
                action: row.get("action"),
                result: Self::parse_result(&result_str),
                details: row.get("details"),
                session_id: row.get("session_id"),
            });
        }

        Ok(events)
    }

    /// Clean up old audit events (retention policy)
    pub async fn cleanup_old_events(&self, days: i64) -> Result<u64> {
        let cutoff = Utc::now() - chrono::Duration::days(days);

        let result = sqlx::query("DELETE FROM audit_events WHERE timestamp < ?")
            .bind(cutoff.timestamp())
            .execute(&self.db)
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to cleanup old events: {}", e))
            })?;

        info!(
            "Cleaned up {} old audit events (older than {} days)",
            result.rows_affected(),
            days
        );

        Ok(result.rows_affected())
    }

    /// Get audit statistics
    pub async fn get_stats(&self) -> Result<AuditStats> {
        // Total events
        let total_row = sqlx::query("SELECT COUNT(*) as count FROM audit_events")
            .fetch_one(&self.db)
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to count events: {}", e))
            })?;
        let total_events: i64 = total_row.get("count");

        // Events by severity
        let severity_rows =
            sqlx::query("SELECT severity, COUNT(*) as count FROM audit_events GROUP BY severity")
                .fetch_all(&self.db)
                .await
                .map_err(|e| {
                    horcrux_common::Error::System(format!("Failed to get severity counts: {}", e))
                })?;

        let mut by_severity = std::collections::HashMap::new();
        for row in severity_rows {
            let severity: String = row.get("severity");
            let count: i64 = row.get("count");
            by_severity.insert(severity, count);
        }

        // Events by result
        let result_rows =
            sqlx::query("SELECT result, COUNT(*) as count FROM audit_events GROUP BY result")
                .fetch_all(&self.db)
                .await
                .map_err(|e| {
                    horcrux_common::Error::System(format!("Failed to get result counts: {}", e))
                })?;

        let mut by_result = std::collections::HashMap::new();
        for row in result_rows {
            let result: String = row.get("result");
            let count: i64 = row.get("count");
            by_result.insert(result, count);
        }

        // Top users
        let user_rows = sqlx::query(
            "SELECT username, COUNT(*) as count FROM audit_events WHERE username IS NOT NULL GROUP BY username ORDER BY count DESC LIMIT 10"
        )
        .fetch_all(&self.db)
        .await
        .map_err(|e| {
            horcrux_common::Error::System(format!("Failed to get top users: {}", e))
        })?;

        let mut top_users = Vec::new();
        for row in user_rows {
            let username: String = row.get("username");
            let count: i64 = row.get("count");
            top_users.push((username, count));
        }

        // Top event types
        let type_rows = sqlx::query(
            "SELECT event_type, COUNT(*) as count FROM audit_events GROUP BY event_type ORDER BY count DESC LIMIT 10"
        )
        .fetch_all(&self.db)
        .await
        .map_err(|e| {
            horcrux_common::Error::System(format!("Failed to get top event types: {}", e))
        })?;

        let mut top_event_types = Vec::new();
        for row in type_rows {
            let event_type: String = row.get("event_type");
            let count: i64 = row.get("count");
            top_event_types.push((event_type, count));
        }

        Ok(AuditStats {
            total_events,
            by_severity,
            by_result,
            top_users,
            top_event_types,
        })
    }

    // Helper functions to parse enums from strings
    fn parse_event_type(s: &str) -> AuditEventType {
        match s {
            "Login" => AuditEventType::Login,
            "Logout" => AuditEventType::Logout,
            "LoginFailed" => AuditEventType::LoginFailed,
            "PasswordChanged" => AuditEventType::PasswordChanged,
            "PermissionGranted" => AuditEventType::PermissionGranted,
            "PermissionDenied" => AuditEventType::PermissionDenied,
            "RoleAssigned" => AuditEventType::RoleAssigned,
            "RoleRevoked" => AuditEventType::RoleRevoked,
            "VmCreated" => AuditEventType::VmCreated,
            "VmDeleted" => AuditEventType::VmDeleted,
            "VmStarted" => AuditEventType::VmStarted,
            "VmStopped" => AuditEventType::VmStopped,
            "VmRestarted" => AuditEventType::VmRestarted,
            "VmMigrated" => AuditEventType::VmMigrated,
            "VmConfigChanged" => AuditEventType::VmConfigChanged,
            "StoragePoolCreated" => AuditEventType::StoragePoolCreated,
            "StoragePoolDeleted" => AuditEventType::StoragePoolDeleted,
            "VolumeCreated" => AuditEventType::VolumeCreated,
            "VolumeDeleted" => AuditEventType::VolumeDeleted,
            "SnapshotCreated" => AuditEventType::SnapshotCreated,
            "SnapshotDeleted" => AuditEventType::SnapshotDeleted,
            "BackupCreated" => AuditEventType::BackupCreated,
            "BackupRestored" => AuditEventType::BackupRestored,
            "BackupDeleted" => AuditEventType::BackupDeleted,
            "NodeAdded" => AuditEventType::NodeAdded,
            "NodeRemoved" => AuditEventType::NodeRemoved,
            "ClusterJoined" => AuditEventType::ClusterJoined,
            "ClusterLeft" => AuditEventType::ClusterLeft,
            "ConfigChanged" => AuditEventType::ConfigChanged,
            "SecretAccessed" => AuditEventType::SecretAccessed,
            "CertificateIssued" => AuditEventType::CertificateIssued,
            "CertificateRevoked" => AuditEventType::CertificateRevoked,
            "SecurityPolicyChanged" => AuditEventType::SecurityPolicyChanged,
            "FirewallRuleAdded" => AuditEventType::FirewallRuleAdded,
            "FirewallRuleDeleted" => AuditEventType::FirewallRuleDeleted,
            "SuspiciousActivity" => AuditEventType::SuspiciousActivity,
            "BruteForceDetected" => AuditEventType::BruteForceDetected,
            _ => AuditEventType::SuspiciousActivity, // Default fallback
        }
    }

    fn parse_severity(s: &str) -> AuditSeverity {
        match s {
            "Info" => AuditSeverity::Info,
            "Warning" => AuditSeverity::Warning,
            "Error" => AuditSeverity::Error,
            "Critical" => AuditSeverity::Critical,
            _ => AuditSeverity::Info,
        }
    }

    fn parse_result(s: &str) -> AuditResult {
        match s {
            "Success" => AuditResult::Success,
            "Failure" => AuditResult::Failure,
            "Partial" => AuditResult::Partial,
            _ => AuditResult::Success,
        }
    }
}

/// Audit statistics summary
#[derive(Debug, Clone, serde::Serialize)]
pub struct AuditStats {
    pub total_events: i64,
    pub by_severity: std::collections::HashMap<String, i64>,
    pub by_result: std::collections::HashMap<String, i64>,
    pub top_users: Vec<(String, i64)>,
    pub top_event_types: Vec<(String, i64)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::create_event;

    async fn create_test_db() -> SqlitePool {
        SqlitePool::connect("sqlite::memory:")
            .await
            .expect("Failed to create test database")
    }

    #[tokio::test]
    async fn test_database_audit_init() {
        let db = create_test_db().await;
        let logger = DatabaseAuditLogger::new(db);

        let result = logger.init_schema().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_database_audit_log() {
        let db = create_test_db().await;
        let logger = DatabaseAuditLogger::new(db);
        logger.init_schema().await.ok();

        let event = create_event(
            AuditEventType::Login,
            AuditSeverity::Info,
            Some("testuser".to_string()),
            Some("192.168.1.1".to_string()),
            "User logged in".to_string(),
            AuditResult::Success,
        );

        let result = logger.log(&event).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_database_audit_query() {
        let db = create_test_db().await;
        let logger = DatabaseAuditLogger::new(db);
        logger.init_schema().await.ok();

        let event = create_event(
            AuditEventType::VmCreated,
            AuditSeverity::Info,
            Some("admin".to_string()),
            None,
            "VM created".to_string(),
            AuditResult::Success,
        );

        logger.log(&event).await.ok();

        let results = logger
            .query(
                Some(AuditEventType::VmCreated),
                None,
                None,
                None,
                None,
                Some(10),
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].user, Some("admin".to_string()));
    }

    #[tokio::test]
    async fn test_database_brute_force_detection() {
        let db = create_test_db().await;
        let logger = DatabaseAuditLogger::new(db);
        logger.init_schema().await.ok();

        // Log 5 failed login attempts
        for _ in 0..5 {
            let event = create_event(
                AuditEventType::LoginFailed,
                AuditSeverity::Warning,
                Some("attacker".to_string()),
                Some("10.0.0.1".to_string()),
                "Failed login".to_string(),
                AuditResult::Failure,
            );
            logger.log(&event).await.ok();
        }

        let suspects = logger.detect_brute_force(3, 10).await.unwrap();
        assert_eq!(suspects.len(), 1);
        assert_eq!(suspects[0], "attacker");
    }

    #[tokio::test]
    async fn test_database_event_counts() {
        let db = create_test_db().await;
        let logger = DatabaseAuditLogger::new(db);
        logger.init_schema().await.ok();

        // Log multiple events
        for _ in 0..3 {
            let event = create_event(
                AuditEventType::Login,
                AuditSeverity::Info,
                Some("user1".to_string()),
                None,
                "Login".to_string(),
                AuditResult::Success,
            );
            logger.log(&event).await.ok();
        }

        for _ in 0..2 {
            let event = create_event(
                AuditEventType::VmCreated,
                AuditSeverity::Info,
                Some("user1".to_string()),
                None,
                "VM created".to_string(),
                AuditResult::Success,
            );
            logger.log(&event).await.ok();
        }

        let counts = logger.get_event_counts().await.unwrap();
        assert_eq!(*counts.get("Login").unwrap(), 3);
        assert_eq!(*counts.get("VmCreated").unwrap(), 2);
    }

    #[tokio::test]
    async fn test_database_get_stats() {
        let db = create_test_db().await;
        let logger = DatabaseAuditLogger::new(db);
        logger.init_schema().await.ok();

        // Log some events
        for i in 0..5 {
            let event = create_event(
                AuditEventType::Login,
                AuditSeverity::Info,
                Some(format!("user{}", i)),
                None,
                "Login".to_string(),
                AuditResult::Success,
            );
            logger.log(&event).await.ok();
        }

        let stats = logger.get_stats().await.unwrap();
        assert_eq!(stats.total_events, 5);
        assert!(!stats.top_users.is_empty());
    }
}
