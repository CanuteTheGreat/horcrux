///! Alert system module
///! Provides threshold-based monitoring alerts with email and webhook notifications

pub mod rules;
pub mod notifications;

use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[allow(unused_imports)]
pub use rules::{AlertRule, MetricType, AlertCondition, ComparisonOperator};
pub use notifications::NotificationChannel;

/// Alert severity level
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}

/// Alert status
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertStatus {
    Firing,      // Alert condition is currently true
    Resolved,    // Alert condition is no longer true
    Acknowledged, // Alert has been acknowledged by admin
}

/// Active alert instance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub id: String,
    pub rule_id: String,
    pub rule_name: String,
    pub severity: AlertSeverity,
    pub status: AlertStatus,
    pub message: String,
    pub target: String,         // VM ID, node name, etc.
    pub metric_value: f64,
    pub threshold: f64,
    pub fired_at: i64,          // Unix timestamp when alert fired
    pub resolved_at: Option<i64>, // Unix timestamp when resolved
    pub acknowledged_at: Option<i64>,
    pub acknowledged_by: Option<String>,
}

/// Alert manager
pub struct AlertManager {
    rules: Arc<RwLock<HashMap<String, AlertRule>>>,
    active_alerts: Arc<RwLock<HashMap<String, Alert>>>,
    alert_history: Arc<RwLock<Vec<Alert>>>,
    notification_channels: Arc<RwLock<Vec<NotificationChannel>>>,
    max_history: usize,
}

impl AlertManager {
    pub fn new() -> Self {
        Self {
            rules: Arc::new(RwLock::new(HashMap::new())),
            active_alerts: Arc::new(RwLock::new(HashMap::new())),
            alert_history: Arc::new(RwLock::new(Vec::new())),
            notification_channels: Arc::new(RwLock::new(Vec::new())),
            max_history: 1000, // Keep last 1000 alerts
        }
    }

    /// Add an alert rule
    pub async fn add_rule(&self, rule: AlertRule) -> Result<()> {
        let mut rules = self.rules.write().await;

        if rules.contains_key(&rule.id) {
            return Err(horcrux_common::Error::InvalidConfig(
                format!("Alert rule {} already exists", rule.id)
            ));
        }

        rules.insert(rule.id.clone(), rule);
        Ok(())
    }

    /// Remove an alert rule
    pub async fn remove_rule(&self, rule_id: &str) -> Result<()> {
        let mut rules = self.rules.write().await;
        rules.remove(rule_id)
            .ok_or_else(|| horcrux_common::Error::System(format!("Alert rule {} not found", rule_id)))?;
        Ok(())
    }

    /// Get all alert rules
    pub async fn list_rules(&self) -> Vec<AlertRule> {
        let rules = self.rules.read().await;
        rules.values().cloned().collect()
    }

    /// Get a specific alert rule
    pub async fn get_rule(&self, rule_id: &str) -> Option<AlertRule> {
        let rules = self.rules.read().await;
        rules.get(rule_id).cloned()
    }

    /// Evaluate metric against all rules
    pub async fn evaluate_metric(&self, metric_type: MetricType, target: &str, value: f64) {
        let rules = self.rules.read().await;

        for rule in rules.values() {
            if !rule.enabled {
                continue;
            }

            if rule.condition.metric_type != metric_type {
                continue;
            }

            // Check if rule applies to this target
            if !Self::matches_target(&rule.condition.target_pattern, target) {
                continue;
            }

            // Evaluate condition
            let triggered = rule.condition.evaluate(value);

            if triggered {
                self.fire_alert(rule, target, value).await;
            } else {
                self.resolve_alert(&rule.id, target).await;
            }
        }
    }

    /// Fire an alert
    async fn fire_alert(&self, rule: &AlertRule, target: &str, value: f64) {
        let alert_key = format!("{}:{}", rule.id, target);

        let mut active_alerts = self.active_alerts.write().await;

        // Check if alert already exists and is firing
        if let Some(existing) = active_alerts.get(&alert_key) {
            if existing.status == AlertStatus::Firing {
                return; // Alert already firing, don't duplicate
            }
        }

        let alert = Alert {
            id: uuid::Uuid::new_v4().to_string(),
            rule_id: rule.id.clone(),
            rule_name: rule.name.clone(),
            severity: rule.severity.clone(),
            status: AlertStatus::Firing,
            message: format!(
                "{} - {} {} threshold {} (current: {})",
                rule.name,
                rule.condition.metric_type.to_string(),
                rule.condition.operator.to_string(),
                rule.condition.threshold,
                value
            ),
            target: target.to_string(),
            metric_value: value,
            threshold: rule.condition.threshold,
            fired_at: chrono::Utc::now().timestamp(),
            resolved_at: None,
            acknowledged_at: None,
            acknowledged_by: None,
        };

        tracing::warn!(
            "Alert fired: {} for {} (value: {}, threshold: {})",
            alert.rule_name,
            alert.target,
            alert.metric_value,
            alert.threshold
        );

        active_alerts.insert(alert_key.clone(), alert.clone());
        drop(active_alerts);

        // Send notifications
        self.send_notifications(&alert).await;

        // Add to history
        let mut history = self.alert_history.write().await;
        history.push(alert);
        if history.len() > self.max_history {
            history.remove(0);
        }
    }

    /// Resolve an alert
    async fn resolve_alert(&self, rule_id: &str, target: &str) {
        let alert_key = format!("{}:{}", rule_id, target);
        let mut active_alerts = self.active_alerts.write().await;

        if let Some(mut alert) = active_alerts.remove(&alert_key) {
            alert.status = AlertStatus::Resolved;
            alert.resolved_at = Some(chrono::Utc::now().timestamp());

            tracing::info!("Alert resolved: {} for {}", alert.rule_name, alert.target);

            // Update history
            let mut history = self.alert_history.write().await;
            history.push(alert);
            if history.len() > self.max_history {
                history.remove(0);
            }
        }
    }

    /// Acknowledge an alert
    pub async fn acknowledge_alert(&self, rule_id: &str, target: &str, user: &str) -> Result<()> {
        let alert_key = format!("{}:{}", rule_id, target);
        let mut active_alerts = self.active_alerts.write().await;

        let alert = active_alerts
            .get_mut(&alert_key)
            .ok_or_else(|| horcrux_common::Error::System("Alert not found".to_string()))?;

        alert.status = AlertStatus::Acknowledged;
        alert.acknowledged_at = Some(chrono::Utc::now().timestamp());
        alert.acknowledged_by = Some(user.to_string());

        tracing::info!("Alert acknowledged by {}: {} for {}", user, alert.rule_name, alert.target);

        Ok(())
    }

    /// Get all active alerts
    pub async fn get_active_alerts(&self) -> Vec<Alert> {
        let alerts = self.active_alerts.read().await;
        alerts.values().cloned().collect()
    }

    /// Get alert history
    pub async fn get_alert_history(&self, limit: Option<usize>) -> Vec<Alert> {
        let history = self.alert_history.read().await;
        let limit = limit.unwrap_or(100).min(self.max_history);

        history.iter()
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }

    /// Add a notification channel
    pub async fn add_notification_channel(&self, channel: NotificationChannel) -> Result<()> {
        let mut channels = self.notification_channels.write().await;
        channels.push(channel);
        Ok(())
    }

    /// List notification channels
    pub async fn list_notification_channels(&self) -> Vec<NotificationChannel> {
        let channels = self.notification_channels.read().await;
        channels.clone()
    }

    /// Send notifications for an alert
    async fn send_notifications(&self, alert: &Alert) {
        let channels = self.notification_channels.read().await;

        for channel in channels.iter() {
            let (enabled, min_severity) = match channel {
                NotificationChannel::Email { enabled, min_severity, .. } => (enabled, min_severity),
                NotificationChannel::Webhook { enabled, min_severity, .. } => (enabled, min_severity),
                NotificationChannel::Syslog { enabled, min_severity, .. } => (enabled, min_severity),
            };

            if !enabled {
                continue;
            }

            // Check if severity matches channel's minimum severity
            if !Self::should_notify(min_severity, &alert.severity) {
                continue;
            }

            // Send notification
            if let Err(e) = notifications::send_notification(channel, alert).await {
                tracing::error!("Failed to send notification via {:?}: {}", channel, e);
            }
        }
    }

    /// Check if pattern matches target
    fn matches_target(pattern: &str, target: &str) -> bool {
        if pattern == "*" {
            return true;
        }

        // Simple wildcard matching
        if pattern.ends_with('*') {
            let prefix = &pattern[..pattern.len() - 1];
            return target.starts_with(prefix);
        }

        pattern == target
    }

    /// Check if should notify based on severity
    fn should_notify(min_severity: &AlertSeverity, alert_severity: &AlertSeverity) -> bool {
        match min_severity {
            AlertSeverity::Info => true,
            AlertSeverity::Warning => matches!(alert_severity, AlertSeverity::Warning | AlertSeverity::Critical),
            AlertSeverity::Critical => matches!(alert_severity, AlertSeverity::Critical),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_target_matching() {
        assert!(AlertManager::matches_target("*", "vm-100"));
        assert!(AlertManager::matches_target("vm-*", "vm-100"));
        assert!(AlertManager::matches_target("vm-100", "vm-100"));
        assert!(!AlertManager::matches_target("vm-100", "vm-101"));
    }

    #[test]
    fn test_severity_notification() {
        assert!(AlertManager::should_notify(&AlertSeverity::Info, &AlertSeverity::Critical));
        assert!(AlertManager::should_notify(&AlertSeverity::Warning, &AlertSeverity::Critical));
        assert!(!AlertManager::should_notify(&AlertSeverity::Critical, &AlertSeverity::Warning));
    }
}
