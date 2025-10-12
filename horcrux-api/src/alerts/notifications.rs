//! Notification channels for alerts

#![allow(dead_code)]

use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use super::{Alert, AlertSeverity};
use lettre::{Message, SmtpTransport, Transport};
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use tokio::process::Command;

/// Email notification configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailConfig {
    pub smtp_server: String,
    pub smtp_port: u16,
    pub from_address: String,
    pub to_addresses: Vec<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub use_tls: bool,
}

/// Webhook notification configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    pub url: String,
    pub method: String, // GET, POST, PUT
    pub headers: Vec<(String, String)>,
    pub auth_token: Option<String>,
}

/// Notification channel type
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum NotificationChannel {
    Email {
        name: String,
        enabled: bool,
        min_severity: AlertSeverity,
        config: EmailConfig,
    },
    Webhook {
        name: String,
        enabled: bool,
        min_severity: AlertSeverity,
        config: WebhookConfig,
    },
    Syslog {
        name: String,
        enabled: bool,
        min_severity: AlertSeverity,
    },
}

/// Send notification for an alert
pub async fn send_notification(channel: &NotificationChannel, alert: &Alert) -> Result<()> {
    match channel {
        NotificationChannel::Email { config, .. } => {
            send_email(config, alert).await
        }
        NotificationChannel::Webhook { config, .. } => {
            send_webhook(config, alert).await
        }
        NotificationChannel::Syslog { .. } => {
            send_syslog(alert).await
        }
    }
}

/// Send email notification using SMTP
async fn send_email(config: &EmailConfig, alert: &Alert) -> Result<()> {
    let subject = format!("[{}] {} - {}",
        match alert.severity {
            AlertSeverity::Critical => "CRITICAL",
            AlertSeverity::Warning => "WARNING",
            AlertSeverity::Info => "INFO",
        },
        alert.rule_name,
        alert.target
    );

    let body = format!(
        r#"Alert: {}
Target: {}
Severity: {:?}
Status: {:?}

{}

Metric Value: {}
Threshold: {}
Fired At: {}

--
Horcrux Alert System
"#,
        alert.rule_name,
        alert.target,
        alert.severity,
        alert.status,
        alert.message,
        alert.metric_value,
        alert.threshold,
        chrono::DateTime::from_timestamp(alert.fired_at, 0)
            .map(|dt| dt.to_rfc2822())
            .unwrap_or_else(|| "Unknown".to_string())
    );

    // Build email for each recipient
    for to_address in &config.to_addresses {
        // Create email message
        let email = Message::builder()
            .from(config.from_address.parse().map_err(|e| {
                horcrux_common::Error::InvalidConfig(format!("Invalid from address: {}", e))
            })?)
            .to(to_address.parse().map_err(|e| {
                horcrux_common::Error::InvalidConfig(format!("Invalid to address '{}': {}", to_address, e))
            })?)
            .subject(&subject)
            .header(ContentType::TEXT_PLAIN)
            .body(body.clone())
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to build email: {}", e))
            })?;

        // Create SMTP transport
        let mailer = if config.use_tls {
            // Use TLS connection
            let mut transport = SmtpTransport::relay(&config.smtp_server)
                .map_err(|e| {
                    horcrux_common::Error::System(format!("Failed to connect to SMTP server: {}", e))
                })?
                .port(config.smtp_port);

            // Add authentication if provided
            if let (Some(username), Some(password)) = (&config.username, &config.password) {
                transport = transport.credentials(Credentials::new(
                    username.clone(),
                    password.clone(),
                ));
            }

            transport.build()
        } else {
            // Use plain SMTP (no TLS)
            let mut transport = SmtpTransport::builder_dangerous(&config.smtp_server)
                .port(config.smtp_port);

            // Add authentication if provided
            if let (Some(username), Some(password)) = (&config.username, &config.password) {
                transport = transport.credentials(Credentials::new(
                    username.clone(),
                    password.clone(),
                ));
            }

            transport.build()
        };

        // Send email
        tokio::task::spawn_blocking(move || {
            mailer.send(&email)
        })
        .await
        .map_err(|e| {
            horcrux_common::Error::System(format!("Failed to spawn blocking task: {}", e))
        })?
        .map_err(|e| {
            horcrux_common::Error::System(format!("Failed to send email to {}: {}", to_address, e))
        })?;

        tracing::info!("Sent email notification to {} for alert: {}", to_address, alert.id);
    }

    Ok(())
}

/// Send webhook notification using HTTP client
async fn send_webhook(config: &WebhookConfig, alert: &Alert) -> Result<()> {
    // Build JSON payload
    let payload = serde_json::json!({
        "alert_id": alert.id,
        "rule_id": alert.rule_id,
        "rule_name": alert.rule_name,
        "severity": alert.severity,
        "status": alert.status,
        "message": alert.message,
        "target": alert.target,
        "metric_value": alert.metric_value,
        "threshold": alert.threshold,
        "fired_at": alert.fired_at,
    });

    // Create HTTP client
    let client = reqwest::Client::new();

    // Build request based on method
    let mut request = match config.method.to_uppercase().as_str() {
        "GET" => client.get(&config.url),
        "POST" => client.post(&config.url),
        "PUT" => client.put(&config.url),
        "PATCH" => client.patch(&config.url),
        "DELETE" => client.delete(&config.url),
        _ => {
            return Err(horcrux_common::Error::InvalidConfig(
                format!("Unsupported HTTP method: {}", config.method)
            ));
        }
    };

    // Add default Content-Type header
    request = request.header("Content-Type", "application/json");

    // Add custom headers
    for (key, value) in &config.headers {
        request = request.header(key, value);
    }

    // Add authentication token if provided
    if let Some(token) = &config.auth_token {
        request = request.header("Authorization", format!("Bearer {}", token));
    }

    // Add JSON payload
    request = request.json(&payload);

    // Send request
    let response = request.send().await
        .map_err(|e| {
            horcrux_common::Error::System(format!("Failed to send webhook: {}", e))
        })?;

    // Check response status
    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        return Err(horcrux_common::Error::System(
            format!("Webhook request failed with status {}: {}", status, error_text)
        ));
    }

    tracing::info!("Sent webhook notification for alert: {} to {}", alert.id, config.url);
    Ok(())
}

/// Send syslog notification
async fn send_syslog(alert: &Alert) -> Result<()> {
    let priority = match alert.severity {
        AlertSeverity::Critical => "crit",
        AlertSeverity::Warning => "warning",
        AlertSeverity::Info => "info",
    };

    let message = format!(
        "[HORCRUX-ALERT] {} - {}: {}",
        alert.rule_name,
        alert.target,
        alert.message
    );

    // Use logger command to send to syslog
    let _ = Command::new("logger")
        .arg("-t")
        .arg("horcrux-alerts")
        .arg("-p")
        .arg(format!("user.{}", priority))
        .arg(&message)
        .output()
        .await;

    tracing::info!("Sent syslog notification for alert: {}", alert.id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_notification_channel_serialization() {
        let channel = NotificationChannel::Email {
            name: "test".to_string(),
            enabled: true,
            min_severity: AlertSeverity::Warning,
            config: EmailConfig {
                smtp_server: "localhost".to_string(),
                smtp_port: 25,
                from_address: "alerts@example.com".to_string(),
                to_addresses: vec!["admin@example.com".to_string()],
                username: None,
                password: None,
                use_tls: false,
            },
        };

        let json = serde_json::to_string(&channel).unwrap();
        assert!(json.contains("\"type\":\"email\""));
    }
}
