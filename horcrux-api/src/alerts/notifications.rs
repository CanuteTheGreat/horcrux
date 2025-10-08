///! Notification channels for alerts

use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use super::{Alert, AlertSeverity};

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

/// Send email notification
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

    // In a real implementation, we'd use a proper email library
    // For now, use system's mail command if available
    for to_address in &config.to_addresses {
        let output = Command::new("mail")
            .arg("-s")
            .arg(&subject)
            .arg(to_address)
            .stdin(std::process::Stdio::piped())
            .spawn();

        if let Ok(mut child) = output {
            if let Some(mut stdin) = child.stdin.take() {
                use tokio::io::AsyncWriteExt;
                let _ = stdin.write_all(body.as_bytes()).await;
            }
            let _ = child.wait().await;
        }
    }

    tracing::info!("Sent email notification for alert: {}", alert.id);
    Ok(())
}

/// Send webhook notification
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

    // In a real implementation, we'd use reqwest or similar
    // For now, use curl
    let mut cmd = Command::new("curl");
    cmd.arg("-X")
        .arg(&config.method)
        .arg(&config.url)
        .arg("-H")
        .arg("Content-Type: application/json");

    for (key, value) in &config.headers {
        cmd.arg("-H").arg(format!("{}: {}", key, value));
    }

    if let Some(token) = &config.auth_token {
        cmd.arg("-H").arg(format!("Authorization: Bearer {}", token));
    }

    cmd.arg("-d").arg(payload.to_string());

    let output = cmd.output().await
        .map_err(|e| horcrux_common::Error::System(format!("Failed to send webhook: {}", e)))?;

    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        return Err(horcrux_common::Error::System(
            format!("Webhook request failed: {}", error)
        ));
    }

    tracing::info!("Sent webhook notification for alert: {}", alert.id);
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
