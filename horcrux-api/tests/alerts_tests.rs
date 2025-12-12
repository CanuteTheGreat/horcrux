//! Alerts Module Tests
//! Tests for alert rules, notifications, and alert management

use horcrux_api::alerts::{
    AlertManager, AlertRule, AlertSeverity, AlertStatus, Alert, MetricType,
};
use horcrux_api::alerts::notifications::{
    NotificationChannel, EmailConfig, WebhookConfig,
};

// ============== Alert Manager Tests ==============

#[tokio::test]
async fn test_alert_manager_creation() {
    let manager = AlertManager::new();
    let rules = manager.list_rules().await;
    assert!(rules.is_empty());
}

#[tokio::test]
async fn test_add_alert_rule() {
    let manager = AlertManager::new();

    let rule = AlertRule::high_cpu_usage("vm-*", 90.0);

    assert!(manager.add_rule(rule).await.is_ok());
    assert_eq!(manager.list_rules().await.len(), 1);
}

#[tokio::test]
async fn test_alert_severities() {
    let severities = vec![
        AlertSeverity::Info,
        AlertSeverity::Warning,
        AlertSeverity::Critical,
    ];

    for severity in severities {
        let json = serde_json::to_string(&severity).unwrap();
        let _: AlertSeverity = serde_json::from_str(&json).unwrap();
    }
}

#[tokio::test]
async fn test_alert_statuses() {
    let statuses = vec![
        AlertStatus::Firing,
        AlertStatus::Resolved,
        AlertStatus::Acknowledged,
    ];

    for status in statuses {
        let json = serde_json::to_string(&status).unwrap();
        let _: AlertStatus = serde_json::from_str(&json).unwrap();
    }
}

#[tokio::test]
async fn test_metric_types() {
    let metrics = vec![
        MetricType::CpuUsage,
        MetricType::MemoryUsage,
        MetricType::DiskUsage,
        MetricType::DiskIo,
        MetricType::NetworkIo,
        MetricType::NodeLoad,
        MetricType::VmCount,
    ];

    for metric in metrics {
        let json = serde_json::to_string(&metric).unwrap();
        let _: MetricType = serde_json::from_str(&json).unwrap();
    }
}

#[tokio::test]
async fn test_remove_alert_rule() {
    let manager = AlertManager::new();

    let rule = AlertRule::high_memory_usage("*", 80.0);
    let rule_id = rule.id.clone();

    manager.add_rule(rule).await.unwrap();
    assert_eq!(manager.list_rules().await.len(), 1);

    manager.remove_rule(&rule_id).await.unwrap();
    assert!(manager.list_rules().await.is_empty());
}

#[tokio::test]
async fn test_get_alert_rule() {
    let manager = AlertManager::new();

    let rule = AlertRule::disk_full("*", 95.0);
    let rule_id = rule.id.clone();

    manager.add_rule(rule).await.unwrap();

    assert!(manager.get_rule(&rule_id).await.is_some());
    assert!(manager.get_rule("nonexistent").await.is_none());
}

#[tokio::test]
async fn test_list_active_alerts() {
    let manager = AlertManager::new();
    let alerts = manager.get_active_alerts().await;
    assert!(alerts.is_empty());
}

#[tokio::test]
async fn test_get_alert_history() {
    let manager = AlertManager::new();
    let history = manager.get_alert_history(Some(100)).await;
    assert!(history.is_empty());
}

#[tokio::test]
async fn test_acknowledge_nonexistent_alert() {
    let manager = AlertManager::new();
    let result = manager.acknowledge_alert("nonexistent", "target", "admin").await;
    assert!(result.is_err());
}

// ============== Predefined Rules Tests ==============

#[tokio::test]
async fn test_high_cpu_usage_rule() {
    let rule = AlertRule::high_cpu_usage("vm-*", 80.0);
    assert_eq!(rule.name, "High CPU Usage");
    assert_eq!(rule.severity, AlertSeverity::Warning);
    assert_eq!(rule.condition.threshold, 80.0);
    assert!(rule.enabled);
}

#[tokio::test]
async fn test_high_memory_usage_rule() {
    let rule = AlertRule::high_memory_usage("*", 85.0);
    assert_eq!(rule.name, "High Memory Usage");
    assert_eq!(rule.severity, AlertSeverity::Warning);
    assert_eq!(rule.condition.threshold, 85.0);
}

#[tokio::test]
async fn test_disk_full_rule() {
    let rule = AlertRule::disk_full("*", 90.0);
    assert_eq!(rule.name, "Disk Almost Full");
    assert_eq!(rule.severity, AlertSeverity::Critical);
    assert_eq!(rule.condition.threshold, 90.0);
}

#[tokio::test]
async fn test_high_node_load_rule() {
    let rule = AlertRule::high_node_load("node-*", 4.0);
    assert_eq!(rule.name, "High Node Load");
    assert_eq!(rule.severity, AlertSeverity::Warning);
    assert_eq!(rule.condition.threshold, 4.0);
}

// ============== Alert Serialization Tests ==============

#[tokio::test]
async fn test_alert_serialization() {
    let alert = Alert {
        id: "alert-123".to_string(),
        rule_id: "rule-1".to_string(),
        rule_name: "High CPU".to_string(),
        target: "vm-100".to_string(),
        severity: AlertSeverity::Critical,
        status: AlertStatus::Firing,
        message: "CPU usage at 95%".to_string(),
        metric_value: 95.0,
        threshold: 90.0,
        fired_at: chrono::Utc::now().timestamp(),
        resolved_at: None,
        acknowledged_at: None,
        acknowledged_by: None,
    };

    let json = serde_json::to_string(&alert).unwrap();
    let deserialized: Alert = serde_json::from_str(&json).unwrap();

    assert_eq!(alert.id, deserialized.id);
    assert_eq!(alert.rule_id, deserialized.rule_id);
    assert_eq!(alert.severity, deserialized.severity);
}

#[tokio::test]
async fn test_alert_rule_serialization() {
    let rule = AlertRule::high_cpu_usage("*", 80.0);

    let json = serde_json::to_string(&rule).unwrap();
    let deserialized: AlertRule = serde_json::from_str(&json).unwrap();

    assert_eq!(rule.name, deserialized.name);
    assert_eq!(rule.condition.threshold, deserialized.condition.threshold);
}

// ============== Notification Channel Tests ==============

#[test]
fn test_email_channel_serialization() {
    let channel = NotificationChannel::Email {
        name: "Admin Email".to_string(),
        enabled: true,
        min_severity: AlertSeverity::Warning,
        config: EmailConfig {
            smtp_server: "smtp.example.com".to_string(),
            smtp_port: 587,
            from_address: "alerts@example.com".to_string(),
            to_addresses: vec!["admin@example.com".to_string()],
            username: Some("user@example.com".to_string()),
            password: Some("secret".to_string()),
            use_tls: true,
        },
    };

    let json = serde_json::to_string(&channel).unwrap();
    assert!(json.contains("\"type\":\"email\""));
    let _: NotificationChannel = serde_json::from_str(&json).unwrap();
}

#[test]
fn test_webhook_channel_serialization() {
    let channel = NotificationChannel::Webhook {
        name: "Slack Webhook".to_string(),
        enabled: true,
        min_severity: AlertSeverity::Critical,
        config: WebhookConfig {
            url: "https://hooks.example.com/alerts".to_string(),
            method: "POST".to_string(),
            headers: vec![
                ("Content-Type".to_string(), "application/json".to_string()),
            ],
            auth_token: Some("token123".to_string()),
        },
    };

    let json = serde_json::to_string(&channel).unwrap();
    assert!(json.contains("\"type\":\"webhook\""));
    let _: NotificationChannel = serde_json::from_str(&json).unwrap();
}

#[test]
fn test_syslog_channel_serialization() {
    let channel = NotificationChannel::Syslog {
        name: "Local Syslog".to_string(),
        enabled: true,
        min_severity: AlertSeverity::Info,
    };

    let json = serde_json::to_string(&channel).unwrap();
    assert!(json.contains("\"type\":\"syslog\""));
    let _: NotificationChannel = serde_json::from_str(&json).unwrap();
}

// ============== Notification Channel Management Tests ==============

#[tokio::test]
async fn test_add_notification_channel() {
    let manager = AlertManager::new();

    let channel = NotificationChannel::Syslog {
        name: "Test Syslog".to_string(),
        enabled: true,
        min_severity: AlertSeverity::Warning,
    };

    assert!(manager.add_notification_channel(channel).await.is_ok());
    assert_eq!(manager.list_notification_channels().await.len(), 1);
}

#[tokio::test]
async fn test_list_notification_channels() {
    let manager = AlertManager::new();

    // Initially empty
    assert!(manager.list_notification_channels().await.is_empty());

    // Add multiple channels
    manager.add_notification_channel(NotificationChannel::Syslog {
        name: "Syslog".to_string(),
        enabled: true,
        min_severity: AlertSeverity::Info,
    }).await.unwrap();

    manager.add_notification_channel(NotificationChannel::Webhook {
        name: "Webhook".to_string(),
        enabled: false,
        min_severity: AlertSeverity::Critical,
        config: WebhookConfig {
            url: "https://example.com".to_string(),
            method: "POST".to_string(),
            headers: vec![],
            auth_token: None,
        },
    }).await.unwrap();

    assert_eq!(manager.list_notification_channels().await.len(), 2);
}

// ============== Duplicate Rule Prevention Tests ==============

#[tokio::test]
async fn test_duplicate_rule_prevention() {
    let manager = AlertManager::new();

    let rule1 = AlertRule::high_cpu_usage("*", 80.0);
    let rule_id = rule1.id.clone();

    // First add should succeed
    assert!(manager.add_rule(rule1).await.is_ok());

    // Create another rule with same ID
    let mut rule2 = AlertRule::high_memory_usage("*", 90.0);
    rule2.id = rule_id;

    // Second add with same ID should fail
    assert!(manager.add_rule(rule2).await.is_err());
}
