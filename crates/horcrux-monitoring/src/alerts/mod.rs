pub mod notifications;
/// Alert system module
/// Provides threshold-based monitoring alerts with email and webhook notifications
pub mod rules;

use crate::db::config_store::{self, kinds, ConfigStore};
use crate::db::Database;
use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub use notifications::NotificationChannel;
#[allow(unused_imports)]
pub use rules::{AlertCondition, AlertRule, ComparisonOperator, MetricType};

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
    Firing,       // Alert condition is currently true
    Resolved,     // Alert condition is no longer true
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
    pub target: String, // VM ID, node name, etc.
    pub metric_value: f64,
    pub threshold: f64,
    pub fired_at: i64,            // Unix timestamp when alert fired
    pub resolved_at: Option<i64>, // Unix timestamp when resolved
    pub acknowledged_at: Option<i64>,
    pub acknowledged_by: Option<String>,
}

/// Alert manager
pub struct AlertManager {
    rules: Arc<RwLock<HashMap<String, AlertRule>>>,
    active_alerts: Arc<RwLock<HashMap<String, Alert>>>,
    /// `(persistence key, entry)`, oldest first.
    alert_history: Arc<RwLock<Vec<(String, Alert)>>>,
    notification_channels: Arc<RwLock<Vec<NotificationChannel>>>,
    /// Channels managed on Monitoring > Notifications (persisted, see
    /// notification_channels.rs); every fired alert is delivered to them too.
    channel_store: std::sync::OnceLock<Arc<crate::notification_channels::NotificationChannelStore>>,
    max_history: usize,
    /// `rule_id:target` -> unix time the condition first held, for rules
    /// with a `duration_seconds` that has not elapsed yet.
    pending: Arc<RwLock<HashMap<String, i64>>>,
    /// Durable copy of rules and notification channels; `None` = in-memory.
    store: Option<ConfigStore>,
}

impl Default for AlertManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AlertManager {
    pub fn new() -> Self {
        Self {
            rules: Arc::new(RwLock::new(HashMap::new())),
            active_alerts: Arc::new(RwLock::new(HashMap::new())),
            alert_history: Arc::new(RwLock::new(Vec::new())),
            notification_channels: Arc::new(RwLock::new(Vec::new())),
            channel_store: std::sync::OnceLock::new(),
            max_history: 1000, // Keep last 1000 alerts
            pending: Arc::new(RwLock::new(HashMap::new())),
            store: None,
        }
    }

    /// Alert manager backed by the database: loads persisted alert rules and
    /// notification channels and writes every change through.
    pub async fn with_database(db: Arc<Database>) -> Result<Self> {
        let store = ConfigStore::new(&db);
        let mut mgr = Self::new();
        {
            let mut rules = mgr.rules.write().await;
            for rule in store.load_values::<AlertRule>(kinds::ALERT_RULE).await? {
                rules.insert(rule.id.clone(), rule);
            }
            let mut channels = mgr.notification_channels.write().await;
            *channels = store
                .load_values::<NotificationChannel>(kinds::NOTIFICATION_CHANNEL)
                .await?;
            tracing::info!(
                "Loaded {} alert rule(s) and {} notification channel(s) from database",
                rules.len(),
                channels.len()
            );
        }
        // Firing/acknowledged alerts and the history survive a restart: an
        // acknowledged alert used to come back as "firing" and page again.
        {
            let mut active = mgr.active_alerts.write().await;
            for (key, alert) in store.load::<Alert>(kinds::ALERT_ACTIVE).await? {
                active.insert(key, alert);
            }
            let mut history = mgr.alert_history.write().await;
            *history = store.load::<Alert>(kinds::ALERT_HISTORY).await?;
            // keys are zero-padded sequence numbers: sort = chronological
            history.sort_by(|(a, _), (b, _)| a.cmp(b));
            let excess = history.len().saturating_sub(mgr.max_history);
            for (key, _) in history.drain(..excess) {
                let _ = store.delete(kinds::ALERT_HISTORY, &key).await;
            }
            tracing::info!(
                "Loaded {} active alert(s) and {} alert history entr(ies) from database",
                active.len(),
                history.len()
            );
        }
        mgr.store = Some(store);
        Ok(mgr)
    }

    /// Write an active alert through to the database (logged, not fatal:
    /// alerting must keep working if the disk is full).
    async fn persist_active(&self, key: &str, alert: &Alert) {
        if let Err(e) = config_store::put_opt(&self.store, kinds::ALERT_ACTIVE, key, alert).await {
            tracing::error!("Failed to persist active alert {}: {}", key, e);
        }
    }

    async fn forget_active(&self, key: &str) {
        if let Err(e) = config_store::delete_opt(&self.store, kinds::ALERT_ACTIVE, key).await {
            tracing::error!("Failed to remove persisted alert {}: {}", key, e);
        }
    }

    /// Append to the history (memory + database), trimming to `max_history`.
    async fn push_history(&self, alert: Alert) {
        let mut history = self.alert_history.write().await;
        // Ordered, unique key: nanoseconds, bumped past the last entry's.
        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0).max(0) as u128;
        let last = history
            .last()
            .and_then(|(k, _)| k.parse::<u128>().ok())
            .unwrap_or(0);
        let key = format!("{:030}", now.max(last + 1));
        if let Err(e) = config_store::put_opt(&self.store, kinds::ALERT_HISTORY, &key, &alert).await
        {
            tracing::error!("Failed to persist alert history entry: {}", e);
        }
        history.push((key, alert));
        while history.len() > self.max_history {
            let (old, _) = history.remove(0);
            let _ = config_store::delete_opt(&self.store, kinds::ALERT_HISTORY, &old).await;
        }
    }

    /// Name of a notification channel (its identity for persistence).
    fn channel_name(channel: &NotificationChannel) -> &str {
        match channel {
            NotificationChannel::Email { name, .. }
            | NotificationChannel::Webhook { name, .. }
            | NotificationChannel::Syslog { name, .. } => name,
        }
    }

    /// Deliver fired alerts to the persisted notification channels as well.
    pub fn attach_channel_store(
        &self,
        store: Arc<crate::notification_channels::NotificationChannelStore>,
    ) {
        let _ = self.channel_store.set(store);
    }

    /// Add an alert rule
    pub async fn add_rule(&self, rule: AlertRule) -> Result<()> {
        let mut rules = self.rules.write().await;

        if rules.contains_key(&rule.id) {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Alert rule {} already exists",
                rule.id
            )));
        }

        config_store::put_opt(&self.store, kinds::ALERT_RULE, &rule.id, &rule).await?;
        rules.insert(rule.id.clone(), rule);
        Ok(())
    }

    /// Remove an alert rule (its active alerts are resolved).
    pub async fn remove_rule(&self, rule_id: &str) -> Result<()> {
        let mut rules = self.rules.write().await;
        if !rules.contains_key(rule_id) {
            return Err(horcrux_common::Error::NotFound(format!(
                "Alert rule {} not found",
                rule_id
            )));
        }
        config_store::delete_opt(&self.store, kinds::ALERT_RULE, rule_id).await?;
        rules.remove(rule_id);
        drop(rules);
        self.resolve_rule_alerts(rule_id).await;
        Ok(())
    }

    /// Replace an existing rule (the id in the path wins over the body's).
    /// Its active alerts are resolved: they were raised under the old
    /// condition and will re-fire on the next evaluation if still true.
    pub async fn update_rule(&self, rule_id: &str, mut rule: AlertRule) -> Result<AlertRule> {
        rule.id = rule_id.to_string();
        let mut rules = self.rules.write().await;
        if !rules.contains_key(rule_id) {
            return Err(horcrux_common::Error::NotFound(format!(
                "Alert rule {} not found",
                rule_id
            )));
        }
        config_store::put_opt(&self.store, kinds::ALERT_RULE, rule_id, &rule).await?;
        rules.insert(rule_id.to_string(), rule.clone());
        drop(rules);
        self.resolve_rule_alerts(rule_id).await;
        Ok(rule)
    }

    /// Enable or disable a rule. Disabling resolves its active alerts
    /// (a disabled rule is never evaluated, so they would stay firing).
    pub async fn set_rule_enabled(&self, rule_id: &str, enabled: bool) -> Result<AlertRule> {
        let rule = {
            let mut rules = self.rules.write().await;
            let rule = rules.get_mut(rule_id).ok_or_else(|| {
                horcrux_common::Error::NotFound(format!("Alert rule {} not found", rule_id))
            })?;
            let mut updated = rule.clone();
            updated.enabled = enabled;
            config_store::put_opt(&self.store, kinds::ALERT_RULE, rule_id, &updated).await?;
            *rule = updated.clone();
            updated
        };
        if !enabled {
            self.resolve_rule_alerts(rule_id).await;
        }
        Ok(rule)
    }

    /// Resolve every active alert raised by a rule.
    async fn resolve_rule_alerts(&self, rule_id: &str) {
        let targets: Vec<String> = self
            .active_alerts
            .read()
            .await
            .values()
            .filter(|a| a.rule_id == rule_id)
            .map(|a| a.target.clone())
            .collect();
        for target in targets {
            self.resolve_alert(rule_id, &target).await;
        }
        self.pending
            .write()
            .await
            .retain(|key, _| !key.starts_with(&format!("{rule_id}:")));
    }

    /// The persisted Monitoring > Notifications channels, once attached.
    pub fn channel_store(
        &self,
    ) -> Option<&Arc<crate::notification_channels::NotificationChannelStore>> {
        self.channel_store.get()
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

    /// Evaluate metric against all rules. A rule fires once its condition
    /// has held for `duration_seconds` (0 = on the first matching sample)
    /// and resolves on the first sample where it no longer holds.
    pub async fn evaluate_metric(&self, metric_type: MetricType, target: &str, value: f64) {
        self.evaluate_metric_at(metric_type, target, value, chrono::Utc::now().timestamp())
            .await
    }

    /// [`Self::evaluate_metric`] with an explicit clock (unix seconds).
    pub async fn evaluate_metric_at(
        &self,
        metric_type: MetricType,
        target: &str,
        value: f64,
        now: i64,
    ) {
        let rules: Vec<AlertRule> = self.rules.read().await.values().cloned().collect();

        for rule in &rules {
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

            let key = format!("{}:{}", rule.id, target);
            if rule.condition.evaluate(value) {
                let since = *self.pending.write().await.entry(key).or_insert(now);
                if now - since >= rule.condition.duration_seconds as i64 {
                    self.fire_alert(rule, target, value).await;
                }
            } else {
                self.pending.write().await.remove(&key);
                self.resolve_alert(&rule.id, target).await;
            }
        }
    }

    /// Fire an alert
    async fn fire_alert(&self, rule: &AlertRule, target: &str, value: f64) {
        let alert_key = format!("{}:{}", rule.id, target);

        let mut active_alerts = self.active_alerts.write().await;

        // Already firing (or acknowledged - it stays quiet until it
        // resolves): don't duplicate it or re-notify on every sample.
        if active_alerts.contains_key(&alert_key) {
            return;
        }
        // The caller evaluated a snapshot of the rules: don't fire for a rule
        // that has been disabled or deleted since (its alert would never be
        // resolved again).
        if !self
            .rules
            .read()
            .await
            .get(&rule.id)
            .is_some_and(|r| r.enabled)
        {
            return;
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
        // (under the lock, so a concurrent resolve can't leave a stale row)
        self.persist_active(&alert_key, &alert).await;
        drop(active_alerts);

        // Send notifications
        self.send_notifications(&alert).await;

        // Add to history
        self.push_history(alert).await;
    }

    /// Resolve an alert
    async fn resolve_alert(&self, rule_id: &str, target: &str) {
        let alert_key = format!("{}:{}", rule_id, target);
        let mut active_alerts = self.active_alerts.write().await;

        if let Some(mut alert) = active_alerts.remove(&alert_key) {
            self.forget_active(&alert_key).await;
            drop(active_alerts);
            alert.status = AlertStatus::Resolved;
            alert.resolved_at = Some(chrono::Utc::now().timestamp());

            tracing::info!("Alert resolved: {} for {}", alert.rule_name, alert.target);

            // Update history
            self.push_history(alert).await;
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
        let acked = alert.clone();
        // Must survive a restart, or the alert re-fires and pages again.
        config_store::put_opt(&self.store, kinds::ALERT_ACTIVE, &alert_key, &acked).await?;
        drop(active_alerts);

        tracing::info!(
            "Alert acknowledged by {}: {} for {}",
            user,
            acked.rule_name,
            acked.target
        );

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

        history
            .iter()
            .rev()
            .take(limit)
            .map(|(_, a)| a.clone())
            .collect()
    }

    /// Add a notification channel
    pub async fn add_notification_channel(&self, channel: NotificationChannel) -> Result<()> {
        let mut channels = self.notification_channels.write().await;
        let name = Self::channel_name(&channel).to_string();
        // The name identifies a channel (and is its persistence key).
        if channels.iter().any(|c| Self::channel_name(c) == name) {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Notification channel {} already exists",
                name
            )));
        }
        config_store::put_opt(&self.store, kinds::NOTIFICATION_CHANNEL, &name, &channel).await?;
        channels.push(channel);
        Ok(())
    }

    /// List notification channels
    pub async fn list_notification_channels(&self) -> Vec<NotificationChannel> {
        let channels = self.notification_channels.read().await;
        channels.clone()
    }

    /// Send notifications for an alert (also used for backup job results)
    pub async fn send_notifications(&self, alert: &Alert) {
        if let Some(store) = self.channel_store.get() {
            store
                .dispatch(&crate::notification_channels::Message::from_alert(alert))
                .await;
        }
        let channels = self.notification_channels.read().await;

        for channel in channels.iter() {
            let (enabled, min_severity) = match channel {
                NotificationChannel::Email {
                    enabled,
                    min_severity,
                    ..
                } => (enabled, min_severity),
                NotificationChannel::Webhook {
                    enabled,
                    min_severity,
                    ..
                } => (enabled, min_severity),
                NotificationChannel::Syslog {
                    enabled,
                    min_severity,
                    ..
                } => (enabled, min_severity),
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
        if let Some(prefix) = pattern.strip_suffix('*') {
            return target.starts_with(prefix);
        }

        pattern == target
    }

    /// Check if should notify based on severity
    fn should_notify(min_severity: &AlertSeverity, alert_severity: &AlertSeverity) -> bool {
        match min_severity {
            AlertSeverity::Info => true,
            AlertSeverity::Warning => matches!(
                alert_severity,
                AlertSeverity::Warning | AlertSeverity::Critical
            ),
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
        assert!(AlertManager::should_notify(
            &AlertSeverity::Info,
            &AlertSeverity::Critical
        ));
        assert!(AlertManager::should_notify(
            &AlertSeverity::Warning,
            &AlertSeverity::Critical
        ));
        assert!(!AlertManager::should_notify(
            &AlertSeverity::Critical,
            &AlertSeverity::Warning
        ));
    }

    fn cpu_rule(id: &str, threshold: f64, duration_seconds: u64) -> AlertRule {
        AlertRule {
            id: id.to_string(),
            name: format!("{id} cpu"),
            description: String::new(),
            severity: AlertSeverity::Warning,
            enabled: true,
            condition: AlertCondition {
                metric_type: MetricType::CpuUsage,
                operator: ComparisonOperator::GreaterThan,
                threshold,
                target_pattern: "vm-*".to_string(),
                duration_seconds,
            },
        }
    }

    #[tokio::test]
    async fn acknowledged_alerts_and_history_survive_a_restart() {
        // QA: after `docker restart` an acknowledged alert came back as
        // "firing" (acknowledged_by null), paged the webhook again, and the
        // history was empty - active alerts and history lived in memory only.
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}", dir.path().join("horcrux.db").display());
        let open = || async {
            let db = Database::new(&url).await.unwrap();
            db.migrate().await.unwrap();
            AlertManager::with_database(Arc::new(db)).await.unwrap()
        };

        let mgr = open().await;
        mgr.add_rule(cpu_rule("r1", 80.0, 0)).await.unwrap();
        mgr.evaluate_metric(MetricType::CpuUsage, "vm-1", 95.0)
            .await;
        mgr.acknowledge_alert("r1", "vm-1", "admin").await.unwrap();
        drop(mgr);

        // "restart": a new manager on a new connection to the same file
        let mgr = open().await;
        let active = mgr.get_active_alerts().await;
        assert_eq!(active.len(), 1, "{active:?}");
        assert_eq!(active[0].status, AlertStatus::Acknowledged);
        assert_eq!(active[0].acknowledged_by.as_deref(), Some("admin"));
        assert_eq!(
            mgr.get_alert_history(None).await.len(),
            1,
            "fired entry kept"
        );
        // still true after the restart: stays acknowledged, no second page
        mgr.evaluate_metric(MetricType::CpuUsage, "vm-1", 96.0)
            .await;
        assert_eq!(mgr.get_alert_history(None).await.len(), 1, "not re-fired");
        assert_eq!(
            mgr.get_active_alerts().await[0].status,
            AlertStatus::Acknowledged
        );
        // resolves, and that is durable too
        mgr.evaluate_metric(MetricType::CpuUsage, "vm-1", 10.0)
            .await;
        drop(mgr);

        let mgr = open().await;
        assert!(mgr.get_active_alerts().await.is_empty());
        let history = mgr.get_alert_history(None).await;
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].status, AlertStatus::Resolved, "newest first");
        assert_eq!(history[1].status, AlertStatus::Firing);
        assert_eq!(history[0].id, history[1].id);
    }

    #[tokio::test]
    async fn rule_fires_only_after_its_duration_and_resolves() {
        let mgr = AlertManager::new();
        mgr.add_rule(cpu_rule("r1", 80.0, 60)).await.unwrap();

        mgr.evaluate_metric_at(MetricType::CpuUsage, "vm-1", 95.0, 1000)
            .await;
        assert!(
            mgr.get_active_alerts().await.is_empty(),
            "pending, not firing"
        );
        mgr.evaluate_metric_at(MetricType::CpuUsage, "vm-1", 95.0, 1059)
            .await;
        assert!(mgr.get_active_alerts().await.is_empty());
        mgr.evaluate_metric_at(MetricType::CpuUsage, "vm-1", 95.0, 1060)
            .await;
        let active = mgr.get_active_alerts().await;
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].target, "vm-1");
        assert_eq!(active[0].metric_value, 95.0);
        // Other metrics / targets outside the pattern don't touch it.
        mgr.evaluate_metric_at(MetricType::MemoryUsage, "vm-1", 1.0, 1070)
            .await;
        mgr.evaluate_metric_at(MetricType::CpuUsage, "ct-1", 99.0, 1070)
            .await;
        assert_eq!(mgr.get_active_alerts().await.len(), 1);

        // A dip below the threshold resolves it and restarts the clock.
        mgr.evaluate_metric_at(MetricType::CpuUsage, "vm-1", 10.0, 1080)
            .await;
        assert!(mgr.get_active_alerts().await.is_empty());
        assert_eq!(
            mgr.get_alert_history(None).await[0].status,
            AlertStatus::Resolved
        );
        mgr.evaluate_metric_at(MetricType::CpuUsage, "vm-1", 95.0, 1090)
            .await;
        assert!(mgr.get_active_alerts().await.is_empty());
    }

    #[tokio::test]
    async fn acknowledged_alert_is_not_refired() {
        let mgr = AlertManager::new();
        mgr.add_rule(cpu_rule("r1", 80.0, 0)).await.unwrap();
        mgr.evaluate_metric_at(MetricType::CpuUsage, "vm-1", 95.0, 1000)
            .await;
        mgr.acknowledge_alert("r1", "vm-1", "admin").await.unwrap();
        mgr.evaluate_metric_at(MetricType::CpuUsage, "vm-1", 96.0, 1010)
            .await;
        let active = mgr.get_active_alerts().await;
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].status, AlertStatus::Acknowledged);
        assert_eq!(active[0].acknowledged_by.as_deref(), Some("admin"));
        assert_eq!(mgr.get_alert_history(None).await.len(), 1, "fired once");
    }

    #[tokio::test]
    async fn update_toggle_and_remove_rules() {
        let mgr = AlertManager::new();
        mgr.add_rule(cpu_rule("r1", 80.0, 0)).await.unwrap();
        mgr.evaluate_metric_at(MetricType::CpuUsage, "vm-1", 95.0, 1000)
            .await;
        assert_eq!(mgr.get_active_alerts().await.len(), 1);

        // Disabling resolves its alert; the rule is no longer evaluated.
        let r = mgr.set_rule_enabled("r1", false).await.unwrap();
        assert!(!r.enabled);
        assert!(mgr.get_active_alerts().await.is_empty());
        mgr.evaluate_metric_at(MetricType::CpuUsage, "vm-1", 95.0, 1010)
            .await;
        assert!(mgr.get_active_alerts().await.is_empty());
        mgr.set_rule_enabled("r1", true).await.unwrap();

        // Update replaces the condition; the path id wins over the body's.
        let mut changed = cpu_rule("ignored", 99.0, 0);
        changed.name = "renamed".into();
        let r = mgr.update_rule("r1", changed).await.unwrap();
        assert_eq!(r.id, "r1");
        let stored = mgr.get_rule("r1").await.unwrap();
        assert_eq!(stored.name, "renamed");
        assert_eq!(stored.condition.threshold, 99.0);
        mgr.evaluate_metric_at(MetricType::CpuUsage, "vm-1", 95.0, 1020)
            .await;
        assert!(
            mgr.get_active_alerts().await.is_empty(),
            "95 < new threshold"
        );

        assert!(matches!(
            mgr.update_rule("nope", cpu_rule("nope", 1.0, 0)).await,
            Err(horcrux_common::Error::NotFound(_))
        ));
        assert!(matches!(
            mgr.set_rule_enabled("nope", true).await,
            Err(horcrux_common::Error::NotFound(_))
        ));
        mgr.remove_rule("r1").await.unwrap();
        assert!(matches!(
            mgr.remove_rule("r1").await,
            Err(horcrux_common::Error::NotFound(_))
        ));
    }
}
