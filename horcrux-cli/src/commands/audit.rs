use crate::api::ApiClient;
use crate::output;
use crate::AuditCommands;
use anyhow::Result;
use serde::{Deserialize, Serialize};

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

            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&events)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&events)?);
            } else {
                println!("{:<20} {:<15} {:<10} {:<15} {:<20} {}",
                    "TIMESTAMP", "TYPE", "SEVERITY", "USER", "ACTION", "RESULT");
                println!("{}", "-".repeat(100));
                for event in events {
                    let ts = chrono::DateTime::from_timestamp(event.timestamp, 0)
                        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                        .unwrap_or_else(|| "Unknown".to_string());
                    let user = event.user.unwrap_or_else(|| "-".to_string());
                    println!("{:<20} {:<15} {:<10} {:<15} {:<20} {}",
                        ts, event.event_type, event.severity, user, event.action, event.result);
                }
            }
        }
        AuditCommands::FailedLogins { user, limit } => {
            let mut params = vec![format!("limit={}", limit)];
            if let Some(u) = user {
                params.push(format!("user={}", u));
            }
            let query = params.join("&");

            let events: Vec<AuditEvent> = api.get(&format!("/api/audit/failed-logins?{}", query)).await?;

            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&events)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&events)?);
            } else {
                println!("Failed Login Attempts:");
                println!("{:<20} {:<20} {:<30} {}",
                    "TIMESTAMP", "USER", "SOURCE IP", "DETAILS");
                println!("{}", "-".repeat(90));
                for event in events {
                    let ts = chrono::DateTime::from_timestamp(event.timestamp, 0)
                        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                        .unwrap_or_else(|| "Unknown".to_string());
                    let user = event.user.unwrap_or_else(|| "-".to_string());
                    let ip = event.source_ip.unwrap_or_else(|| "-".to_string());
                    let details = event.details.unwrap_or_else(|| "-".to_string());
                    println!("{:<20} {:<20} {:<30} {}",
                        ts, user, ip, details);
                }
            }
        }
        AuditCommands::Security { limit } => {
            let events: Vec<AuditEvent> = api.get(&format!("/api/audit/security-events?limit={}", limit)).await?;

            if output_format == "json" {
                println!("{}", serde_json::to_string_pretty(&events)?);
            } else if output_format == "yaml" {
                println!("{}", serde_yaml::to_string(&events)?);
            } else {
                println!("Security Events:");
                println!("{:<20} {:<15} {:<10} {:<20} {}",
                    "TIMESTAMP", "TYPE", "SEVERITY", "ACTION", "RESULT");
                println!("{}", "-".repeat(80));
                for event in events {
                    let ts = chrono::DateTime::from_timestamp(event.timestamp, 0)
                        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                        .unwrap_or_else(|| "Unknown".to_string());
                    println!("{:<20} {:<15} {:<10} {:<20} {}",
                        ts, event.event_type, event.severity, event.action, event.result);
                }
            }
        }
        AuditCommands::Export { output: path } => {
            let request = ExportRequest { path: path.clone() };
            api.post_empty("/api/audit/export", &request).await?;
            output::print_success(&format!("Audit logs exported to {}", path));
        }
    }
    Ok(())
}
