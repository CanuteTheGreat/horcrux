//! Audit log database operations (moved from db::audit so the database
//! layer no longer depends on the audit module).

use crate::audit::{AuditEvent, AuditEventType, AuditResult, AuditSeverity};
use horcrux_common::Result;
use sqlx::sqlite::SqlitePool;
use sqlx::Row;

#[allow(dead_code)]
pub async fn log_event(pool: &SqlitePool, event: &AuditEvent) -> Result<()> {
    let event_type = format!("{:?}", event.event_type);
    let severity = format!("{:?}", event.severity);
    let result = format!("{:?}", event.result);

    sqlx::query(
        "INSERT INTO audit_logs (timestamp, event_type, severity, user, source_ip,
         resource, action, result, details, session_id)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(event.timestamp.timestamp())
    .bind(&event_type)
    .bind(&severity)
    .bind(&event.user)
    .bind(&event.source_ip)
    .bind(&event.resource)
    .bind(&event.action)
    .bind(&result)
    .bind(&event.details)
    .bind(&event.session_id)
    .execute(pool)
    .await
    .map_err(|e| horcrux_common::Error::System(format!("Failed to log audit event: {}", e)))?;

    Ok(())
}

#[allow(dead_code)]
pub async fn query_events(
    pool: &SqlitePool,
    event_type: Option<&str>,
    user: Option<&str>,
    severity: Option<&str>,
    limit: usize,
) -> Result<Vec<AuditEvent>> {
    let mut query = "SELECT * FROM audit_logs WHERE 1=1".to_string();

    if event_type.is_some() {
        query.push_str(" AND event_type = ?");
    }
    if user.is_some() {
        query.push_str(" AND user = ?");
    }
    if severity.is_some() {
        query.push_str(" AND severity = ?");
    }

    query.push_str(" ORDER BY timestamp DESC LIMIT ?");

    let mut sql_query = sqlx::query(&query);

    if let Some(et) = event_type {
        sql_query = sql_query.bind(et);
    }
    if let Some(u) = user {
        sql_query = sql_query.bind(u);
    }
    if let Some(s) = severity {
        sql_query = sql_query.bind(s);
    }
    sql_query = sql_query.bind(limit as i64);

    let rows = sql_query
        .fetch_all(pool)
        .await
        .map_err(|e| horcrux_common::Error::System(format!("Failed to query audit logs: {}", e)))?;

    let mut events = Vec::new();
    for row in rows {
        if let Ok(event) = row_to_event(&row) {
            events.push(event);
        }
    }

    Ok(events)
}

#[allow(dead_code)]
fn row_to_event(row: &sqlx::sqlite::SqliteRow) -> Result<AuditEvent> {
    let timestamp: i64 = row.get("timestamp");
    let event_type_str: String = row.get("event_type");
    let severity_str: String = row.get("severity");
    let result_str: String = row.get("result");

    // Parse event type (simplified - would need full mapping)
    let event_type = match event_type_str.as_str() {
        "Login" => AuditEventType::Login,
        "Logout" => AuditEventType::Logout,
        "LoginFailed" => AuditEventType::LoginFailed,
        _ => AuditEventType::Login, // Default
    };

    let severity = match severity_str.as_str() {
        "Info" => AuditSeverity::Info,
        "Warning" => AuditSeverity::Warning,
        "Error" => AuditSeverity::Error,
        "Critical" => AuditSeverity::Critical,
        _ => AuditSeverity::Info,
    };

    let result = match result_str.as_str() {
        "Success" => AuditResult::Success,
        "Failure" => AuditResult::Failure,
        "Partial" => AuditResult::Partial,
        _ => AuditResult::Success,
    };

    Ok(AuditEvent {
        timestamp: chrono::DateTime::from_timestamp(timestamp, 0)
            .ok_or_else(|| horcrux_common::Error::System("Invalid timestamp".to_string()))?
            .with_timezone(&chrono::Utc),
        event_type,
        severity,
        user: row.get("user"),
        source_ip: row.get("source_ip"),
        resource: row.get("resource"),
        action: row.get("action"),
        result,
        details: row.get("details"),
        session_id: row.get("session_id"),
    })
}
