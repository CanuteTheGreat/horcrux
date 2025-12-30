use crate::api::ApiClient;
use crate::output::{self, OutputFormat, format_relative_time};
use crate::AuditCommands;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tabled::Tabled;

#[derive(Debug, Serialize, Deserialize)]
struct AuditEvent {
    timestamp: i64,
    event_type: String,
    severity: String,
    user: Option<String>,
    source_ip: Option<String>,
    action: String,
    result: String,
    details: Option<String>,
}

#[derive(Tabled, Serialize)]
struct AuditEventRow {
    timestamp: String,
    #[tabled(rename = "type")]
    event_type: String,
    severity: String,
    user: String,
    action: String,
    result: String,
}

impl From<AuditEvent> for AuditEventRow {
    fn from(e: AuditEvent) -> Self {
        Self {
            timestamp: format_relative_time(e.timestamp),
            event_type: e.event_type,
            severity: e.severity,
            user: e.user.unwrap_or_else(|| "-".to_string()),
            action: e.action,
            result: e.result,
        }
    }
}

#[derive(Tabled, Serialize)]
struct FailedLoginRow {
    timestamp: String,
    user: String,
    source_ip: String,
    details: String,
}

impl From<AuditEvent> for FailedLoginRow {
    fn from(e: AuditEvent) -> Self {
        Self {
            timestamp: format_relative_time(e.timestamp),
            user: e.user.unwrap_or_else(|| "-".to_string()),
            source_ip: e.source_ip.unwrap_or_else(|| "-".to_string()),
            details: e.details.unwrap_or_else(|| "-".to_string()),
        }
    }
}

#[derive(Tabled, Serialize)]
struct SecurityEventRow {
    timestamp: String,
    #[tabled(rename = "type")]
    event_type: String,
    severity: String,
    action: String,
    result: String,
}

impl From<AuditEvent> for SecurityEventRow {
    fn from(e: AuditEvent) -> Self {
        Self {
            timestamp: format_relative_time(e.timestamp),
            event_type: e.event_type,
            severity: e.severity,
            action: e.action,
            result: e.result,
        }
    }
}

#[derive(Serialize)]
struct ExportRequest {
    path: String,
}

pub async fn handle_audit_command(
    command: AuditCommands,
    api: &ApiClient,
    output_format: &str,
) -> Result<()> {
    match command {
        AuditCommands::Query { event_type, user, severity, limit } => {
            // Build query string
            let mut params = vec![format!("limit={}", limit)];
            if let Some(et) = event_type {
                params.push(format!("event_type={}", et));
            }
            if let Some(u) = user {
                params.push(format!("user={}", u));
            }
            if let Some(s) = severity {
                params.push(format!("severity={}", s));
            }
            let query = params.join("&");

            let events: Vec<AuditEvent> = api.get(&format!("/api/audit/events?{}", query)).await?;
            let format = OutputFormat::from_str(output_format);
            let rows: Vec<AuditEventRow> = events.into_iter().map(AuditEventRow::from).collect();
            output::print_output(rows, format)?;
        }
        AuditCommands::FailedLogins { user, limit } => {
            let mut params = vec![format!("limit={}", limit)];
            if let Some(u) = user {
                params.push(format!("user={}", u));
            }
            let query = params.join("&");

            let events: Vec<AuditEvent> = api.get(&format!("/api/audit/failed-logins?{}", query)).await?;
            let format = OutputFormat::from_str(output_format);
            let rows: Vec<FailedLoginRow> = events.into_iter().map(FailedLoginRow::from).collect();
            output::print_output(rows, format)?;
        }
        AuditCommands::Security { limit } => {
            let events: Vec<AuditEvent> = api.get(&format!("/api/audit/security-events?limit={}", limit)).await?;
            let format = OutputFormat::from_str(output_format);
            let rows: Vec<SecurityEventRow> = events.into_iter().map(SecurityEventRow::from).collect();
            output::print_output(rows, format)?;
        }
        AuditCommands::Export { output: path } => {
            let request = ExportRequest { path: path.clone() };
            api.post_empty("/api/audit/export", &request).await?;
            output::print_success(&format!("Audit logs exported to {}", path));
        }
    }
    Ok(())
}
