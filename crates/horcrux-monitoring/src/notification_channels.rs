//! Notification channels (Monitoring > Notifications in the web UI).
//!
//! A channel is a configured delivery target: e-mail (SMTP), Slack, Microsoft
//! Teams, Discord, PagerDuty (Events API v2) or a generic webhook. Channels
//! are persisted in the `notification_channels` table, every delivery
//! (test message or fired alert) is really sent, and the per-channel
//! success/failure counters and last-used time reflect those real attempts.
//!
//! Firing alerts reach these channels through
//! [`crate::alerts::AlertManager::attach_channel_store`].

use crate::alerts::{Alert, AlertSeverity};
use crate::db::Database;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

/// Placeholder returned instead of stored secrets; sending it back on update
/// keeps the stored value.
pub const SECRET_MASK: &str = "********";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum WebhookAuth {
    BasicAuth { username: String, password: String },
    BearerToken { token: String },
    ApiKey { key: String, header: String },
}

fn default_ntfy_server() -> String {
    "https://ntfy.sh".to_string()
}

/// Per-type settings. The serde layout is the web UI's
/// `NotificationChannelConfig` (internally tagged by variant name).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ChannelConfig {
    Email {
        /// Recipient(s), comma separated
        address: String,
        smtp_server: String,
        smtp_port: u16,
        #[serde(default)]
        username: String,
        #[serde(default)]
        password: String,
        #[serde(default)]
        use_tls: bool,
    },
    Slack {
        webhook_url: String,
        channel: Option<String>,
        username: Option<String>,
        icon_emoji: Option<String>,
    },
    Teams {
        webhook_url: String,
    },
    /// ntfy (https://ntfy.sh or a self-hosted server): the message is POSTed
    /// as plain text to `<server_url>/<topic>` (docs/ALERT-DELIVERY.md).
    Ntfy {
        /// Server base URL (default `https://ntfy.sh`)
        #[serde(default = "default_ntfy_server")]
        server_url: String,
        topic: String,
        /// Access token, sent as a bearer token (optional)
        #[serde(default)]
        token: String,
        /// Fixed ntfy priority (min, low, default, high, urgent); empty =
        /// follow the alert severity
        #[serde(default)]
        priority: Option<String>,
    },
    Discord {
        webhook_url: String,
        username: Option<String>,
        avatar_url: Option<String>,
    },
    PagerDuty {
        integration_key: String,
        #[serde(default)]
        routing_key: String,
        severity: String,
    },
    Webhook {
        url: String,
        method: String,
        #[serde(default)]
        headers: HashMap<String, String>,
        auth: Option<WebhookAuth>,
    },
}

impl ChannelConfig {
    pub fn type_name(&self) -> &'static str {
        match self {
            ChannelConfig::Email { .. } => "email",
            ChannelConfig::Slack { .. } => "slack",
            ChannelConfig::Teams { .. } => "teams",
            ChannelConfig::Ntfy { .. } => "ntfy",
            ChannelConfig::Discord { .. } => "discord",
            ChannelConfig::PagerDuty { .. } => "pagerduty",
            ChannelConfig::Webhook { .. } => "webhook",
        }
    }

    /// Copy with secrets replaced by [`SECRET_MASK`] (for API responses).
    ///
    /// Besides passwords and tokens this covers the Slack/Teams/Discord
    /// webhook URLs (the URL *is* the posting credential; only its
    /// scheme and host are shown), the PagerDuty routing key, and for a
    /// generic webhook its URL the same way (receivers commonly take the
    /// credential in the path, query or `user:pass@`, e.g. Home Assistant,
    /// n8n or Zapier hooks) and every custom header value (typically an API
    /// token).
    /// Reading channels needs only Sys.Audit, so none of these may leave.
    fn masked(&self) -> Self {
        let mut c = self.clone();
        match &mut c {
            ChannelConfig::Email { password, .. } => *password = mask(password),
            ChannelConfig::Ntfy { token, .. } => *token = mask(token),
            ChannelConfig::Slack { webhook_url, .. }
            | ChannelConfig::Teams { webhook_url }
            | ChannelConfig::Discord { webhook_url, .. } => *webhook_url = mask_url(webhook_url),
            ChannelConfig::PagerDuty {
                integration_key,
                routing_key,
                ..
            } => {
                *integration_key = mask(integration_key);
                *routing_key = mask(routing_key);
            }
            ChannelConfig::Webhook {
                url, headers, auth, ..
            } => {
                *url = mask_url(url);
                for v in headers.values_mut() {
                    *v = mask(v);
                }
                match auth {
                    Some(WebhookAuth::BasicAuth { password, .. }) => *password = mask(password),
                    Some(WebhookAuth::BearerToken { token }) => *token = mask(token),
                    Some(WebhookAuth::ApiKey { key, .. }) => *key = mask(key),
                    None => {}
                }
            }
        }
        c
    }

    /// Put stored secrets back where an update sent the mask (as the web
    /// UI's edit form does: it re-sends what it was given). A masked header
    /// the stored channel doesn't have is dropped rather than sent as the
    /// literal mask.
    fn unmask_from(&mut self, stored: &ChannelConfig) {
        fn keep(new: &mut String, old: &str) {
            if *new == SECRET_MASK {
                *new = old.to_string();
            }
        }
        match (self, stored) {
            (
                ChannelConfig::Email { password, .. },
                ChannelConfig::Email {
                    password: old_pw, ..
                },
            ) => keep(password, old_pw),
            (
                ChannelConfig::Slack { webhook_url, .. },
                ChannelConfig::Slack {
                    webhook_url: old, ..
                },
            )
            | (ChannelConfig::Teams { webhook_url }, ChannelConfig::Teams { webhook_url: old })
            | (
                ChannelConfig::Discord { webhook_url, .. },
                ChannelConfig::Discord {
                    webhook_url: old, ..
                },
            ) => restore_masked_url(webhook_url, old),
            (
                ChannelConfig::PagerDuty {
                    integration_key,
                    routing_key,
                    ..
                },
                ChannelConfig::PagerDuty {
                    integration_key: old_ik,
                    routing_key: old_rk,
                    ..
                },
            ) => {
                // The UI's form has one key field and sends it as both; a
                // channel created over the API may have only one of them.
                let old_key = if old_rk.is_empty() { old_ik } else { old_rk };
                if *integration_key == SECRET_MASK {
                    *integration_key = if old_ik.is_empty() {
                        old_key.clone()
                    } else {
                        old_ik.clone()
                    };
                }
                if *routing_key == SECRET_MASK {
                    *routing_key = old_key.clone();
                }
            }
            (
                ChannelConfig::Webhook {
                    url, headers, auth, ..
                },
                ChannelConfig::Webhook {
                    url: old_url,
                    headers: old_headers,
                    auth: old_auth,
                    ..
                },
            ) => {
                restore_masked_url(url, old_url);
                headers.retain(|k, v| {
                    if v != SECRET_MASK {
                        return true;
                    }
                    match old_headers.get(k) {
                        Some(old) => {
                            *v = old.clone();
                            true
                        }
                        None => false,
                    }
                });
                match (auth, old_auth) {
                    (
                        Some(WebhookAuth::BasicAuth { password, .. }),
                        Some(WebhookAuth::BasicAuth { password: p, .. }),
                    ) => keep(password, p),
                    (
                        Some(WebhookAuth::BearerToken { token }),
                        Some(WebhookAuth::BearerToken { token: t }),
                    ) => keep(token, t),
                    (
                        Some(WebhookAuth::ApiKey { key, .. }),
                        Some(WebhookAuth::ApiKey { key: k, .. }),
                    ) => keep(key, k),
                    _ => {}
                }
            }
            (ChannelConfig::Ntfy { token, .. }, ChannelConfig::Ntfy { token: old, .. }) => {
                keep(token, old)
            }
            _ => {}
        }
    }

    fn validate(&self) -> Result<(), String> {
        fn http_url(what: &str, u: &str) -> Result<(), String> {
            if u.contains(SECRET_MASK) {
                // A masked URL from a list/get response, sent where there is
                // no stored URL to restore (a new channel, or a type change).
                return Err(format!("{} is masked; enter the real URL", what));
            }
            match url::Url::parse(u.trim()) {
                Ok(p) if p.scheme() == "http" || p.scheme() == "https" => Ok(()),
                _ => Err(format!("{} must be an http(s) URL, got '{}'", what, u)),
            }
        }
        // A secret that is still the placeholder has nothing to be restored
        // from (a create, or a type change): refuse it rather than store and
        // deliver the literal mask as a password / token / header value.
        fn not_masked(what: &str, v: &str) -> Result<(), String> {
            if v == SECRET_MASK {
                return Err(format!("{} is masked; enter the real value", what));
            }
            Ok(())
        }
        match self {
            ChannelConfig::Email {
                address,
                smtp_server,
                smtp_port,
                password,
                ..
            } => {
                not_masked("SMTP password", password)?;
                if smtp_server.trim().is_empty() || *smtp_port == 0 {
                    return Err("SMTP server and port are required".into());
                }
                let recipients = recipients(address);
                if recipients.is_empty() {
                    return Err("at least one recipient address is required".into());
                }
                for r in recipients {
                    r.parse::<lettre::Address>()
                        .map_err(|e| format!("invalid e-mail address '{}': {}", r, e))?;
                }
                Ok(())
            }
            ChannelConfig::Slack { webhook_url, .. } => http_url("Slack webhook URL", webhook_url),
            ChannelConfig::Teams { webhook_url } => http_url("Teams webhook URL", webhook_url),
            ChannelConfig::Ntfy {
                server_url,
                topic,
                token,
                priority,
            } => {
                http_url("ntfy server URL", server_url)?;
                not_masked("ntfy token", token)?;
                let t = topic.trim();
                if t.is_empty()
                    || t.len() > 64
                    || !t
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                {
                    return Err("ntfy topic is required: 1-64 letters, digits, '-' or '_'".into());
                }
                if let Some(p) = priority.as_deref().filter(|p| !p.is_empty()) {
                    ntfy_priority(p)?;
                }
                Ok(())
            }
            ChannelConfig::Discord { webhook_url, .. } => {
                http_url("Discord webhook URL", webhook_url)
            }
            ChannelConfig::PagerDuty {
                integration_key,
                routing_key,
                severity,
            } => {
                not_masked("PagerDuty integration key", integration_key)?;
                not_masked("PagerDuty routing key", routing_key)?;
                if integration_key.trim().is_empty() && routing_key.trim().is_empty() {
                    return Err("PagerDuty routing/integration key is required".into());
                }
                pd_severity(severity)?;
                Ok(())
            }
            ChannelConfig::Webhook {
                url,
                method,
                headers,
                auth,
            } => {
                http_url("webhook URL", url)?;
                for (k, v) in headers {
                    not_masked(&format!("header '{}'", k), v)?;
                }
                match auth {
                    Some(WebhookAuth::BasicAuth { password, .. }) => {
                        not_masked("basic auth password", password)?
                    }
                    Some(WebhookAuth::BearerToken { token }) => not_masked("bearer token", token)?,
                    Some(WebhookAuth::ApiKey { key, .. }) => not_masked("API key", key)?,
                    None => {}
                }
                match method.to_uppercase().as_str() {
                    "POST" | "PUT" | "PATCH" | "GET" => Ok(()),
                    m => Err(format!("unsupported webhook method '{}'", m)),
                }
            }
        }
    }
}

fn mask(s: &str) -> String {
    if s.is_empty() {
        String::new()
    } else {
        SECRET_MASK.to_string()
    }
}

/// `https://hooks.slack.com/services/T0/B0/xyz` -> `https://hooks.slack.com/********`:
/// enough to tell channels apart, nothing that can post. The userinfo, path,
/// query and fragment are all dropped, since any of them can carry the
/// receiver's credential.
pub(crate) fn mask_url(u: &str) -> String {
    if u.is_empty() {
        return String::new();
    }
    match url::Url::parse(u.trim()) {
        Ok(p) if p.host_str().is_some() => {
            let port = p.port().map(|n| format!(":{}", n)).unwrap_or_default();
            format!(
                "{}://{}{}/{}",
                p.scheme(),
                p.host_str().unwrap_or_default(),
                port,
                SECRET_MASK
            )
        }
        _ => SECRET_MASK.to_string(),
    }
}

/// Undo [`mask_url`] on an update: a URL sent back as the bare mask or as
/// exactly the masked form of the stored one keeps the stored URL. Anything
/// else is a real edit and replaces it.
pub(crate) fn restore_masked_url(new: &mut String, old: &str) {
    if !old.is_empty() && (*new == SECRET_MASK || *new == mask_url(old)) {
        *new = old.to_string();
    }
}

fn recipients(address: &str) -> Vec<&str> {
    address
        .split([',', ';'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

fn ntfy_priority(s: &str) -> Result<&'static str, String> {
    match s.to_lowercase().as_str() {
        "min" => Ok("min"),
        "low" => Ok("low"),
        "default" => Ok("default"),
        "high" => Ok("high"),
        "urgent" | "max" => Ok("urgent"),
        other => Err(format!(
            "ntfy priority must be min, low, default, high or urgent (got '{}')",
            other
        )),
    }
}

fn pd_severity(s: &str) -> Result<&'static str, String> {
    match s.to_lowercase().as_str() {
        "critical" => Ok("critical"),
        "error" => Ok("error"),
        "warning" => Ok("warning"),
        "info" => Ok("info"),
        other => Err(format!(
            "PagerDuty severity must be critical, error, warning or info (got '{}')",
            other
        )),
    }
}

/// A notification channel as stored and returned by the API (the web UI's
/// `NotificationChannel`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NotificationChannel {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub channel_type: String,
    pub enabled: bool,
    pub config: ChannelConfig,
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub last_used: Option<DateTime<Utc>>,
    #[serde(default)]
    pub success_count: u32,
    #[serde(default)]
    pub failure_count: u32,
}

/// What is being delivered.
#[derive(Debug, Clone)]
pub struct Message {
    pub title: String,
    pub body: String,
    /// info / warning / critical
    pub severity: String,
    pub event: String,
    pub alert: Option<Alert>,
}

impl Message {
    pub fn test(channel_name: &str) -> Self {
        let host = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "horcrux".into());
        Self {
            title: format!("Horcrux test notification ({})", channel_name),
            body: format!(
                "This is a test notification from Horcrux on {} sent at {}. If you can read this, the '{}' channel works.",
                host,
                Utc::now().to_rfc3339(),
                channel_name
            ),
            severity: "info".into(),
            event: "test".into(),
            alert: None,
        }
    }

    pub fn from_alert(alert: &Alert) -> Self {
        let severity = match alert.severity {
            AlertSeverity::Info => "info",
            AlertSeverity::Warning => "warning",
            AlertSeverity::Critical => "critical",
        };
        Self {
            title: format!(
                "[{}] {} - {}",
                severity.to_uppercase(),
                alert.rule_name,
                alert.target
            ),
            body: format!(
                "{}\nTarget: {}\nValue: {} (threshold {})\nStatus: {:?}",
                alert.message, alert.target, alert.metric_value, alert.threshold, alert.status
            ),
            severity: severity.into(),
            event: "alert".into(),
            alert: Some(alert.clone()),
        }
    }
}

#[derive(Debug)]
pub enum StoreError {
    NotFound(String),
    Invalid(String),
    Db(String),
}

impl From<StoreError> for crate::error::ApiError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NotFound(m) => crate::error::ApiError::NotFound(m),
            StoreError::Invalid(m) => crate::error::ApiError::BadRequest(m),
            StoreError::Db(m) => crate::error::ApiError::Internal(m),
        }
    }
}

/// How a delivery is tried (`[notifications]` in config.toml, env
/// `HORCRUX_NOTIFY_*`): total attempts, the wait before the second attempt
/// (milliseconds) and the factor each further wait grows by, and the time
/// limit of one attempt (HTTP request or SMTP session), in seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct DeliveryPolicy {
    pub attempts: u32,
    pub backoff_ms: u64,
    pub backoff_factor: u64,
    pub timeout_secs: u64,
}

impl Default for DeliveryPolicy {
    fn default() -> Self {
        Self {
            attempts: 3,
            backoff_ms: 1000,
            backoff_factor: 2,
            timeout_secs: 20,
        }
    }
}

impl DeliveryPolicy {
    /// Wait before attempt number `attempt` (1-based; attempt 1 is immediate).
    pub fn wait_before(&self, attempt: u32) -> Duration {
        if attempt <= 1 {
            return Duration::ZERO;
        }
        let mult = self.backoff_factor.saturating_pow(attempt - 2);
        Duration::from_millis(self.backoff_ms.saturating_mul(mult))
    }
}

/// One notification that could not be delivered after every attempt; handed
/// to the audit hook ([`NotificationChannelStore::attach_audit`]).
#[derive(Debug, Clone)]
pub struct DeliveryFailure {
    pub channel_id: String,
    pub channel_name: String,
    pub channel_type: String,
    pub event: String,
    pub title: String,
    pub severity: String,
    pub attempts: u32,
    pub error: String,
}

type AuditHook = Arc<dyn Fn(DeliveryFailure) + Send + Sync>;

/// SMTP session time limit (seconds), set from the delivery policy.
static SMTP_TIMEOUT_SECS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(20);

/// Persistent channel registry.
pub struct NotificationChannelStore {
    db: Arc<Database>,
    channels: RwLock<Vec<NotificationChannel>>,
    http: reqwest::Client,
    policy: DeliveryPolicy,
    audit: std::sync::OnceLock<AuditHook>,
}

fn db_err(e: sqlx::Error) -> StoreError {
    StoreError::Db(format!("notification channels: {}", e))
}

impl NotificationChannelStore {
    /// Load all channels from the database (default delivery policy).
    pub async fn load(db: Arc<Database>) -> Result<Self, StoreError> {
        Self::load_with_policy(db, DeliveryPolicy::default()).await
    }

    /// Load all channels; deliveries follow `policy`.
    pub async fn load_with_policy(
        db: Arc<Database>,
        policy: DeliveryPolicy,
    ) -> Result<Self, StoreError> {
        let rows = sqlx::query(
            "SELECT id, name, description, enabled, config, created_at, last_used, success_count, failure_count FROM notification_channels ORDER BY created_at",
        )
        .fetch_all(db.pool())
        .await
        .map_err(db_err)?;
        let mut channels = Vec::new();
        for r in rows {
            let config: String = r.get("config");
            let config: ChannelConfig = match serde_json::from_str(&config) {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(
                        "notification channel {} has an unreadable config: {}",
                        r.get::<String, _>("id"),
                        e
                    );
                    continue;
                }
            };
            let ts = |s: Option<String>| {
                s.and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|d| d.with_timezone(&Utc))
            };
            channels.push(NotificationChannel {
                id: r.get("id"),
                name: r.get("name"),
                description: r.get("description"),
                channel_type: config.type_name().to_string(),
                enabled: r.get::<i64, _>("enabled") != 0,
                config,
                created_at: ts(Some(r.get("created_at"))).unwrap_or_else(Utc::now),
                last_used: ts(r.get("last_used")),
                success_count: r.get::<i64, _>("success_count") as u32,
                failure_count: r.get::<i64, _>("failure_count") as u32,
            });
        }
        SMTP_TIMEOUT_SECS.store(
            policy.timeout_secs.max(1),
            std::sync::atomic::Ordering::Relaxed,
        );
        Ok(Self {
            db,
            channels: RwLock::new(channels),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(policy.timeout_secs.max(1)))
                .build()
                .expect("reqwest client"),
            policy,
            audit: std::sync::OnceLock::new(),
        })
    }

    async fn save(&self, c: &NotificationChannel) -> Result<(), StoreError> {
        let config = serde_json::to_string(&c.config).map_err(|e| StoreError::Db(e.to_string()))?;
        sqlx::query(
            "INSERT INTO notification_channels (id, name, description, channel_type, enabled, config, created_at, last_used, success_count, failure_count)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, description = excluded.description,
               channel_type = excluded.channel_type, enabled = excluded.enabled, config = excluded.config,
               last_used = excluded.last_used, success_count = excluded.success_count,
               failure_count = excluded.failure_count",
        )
        .bind(&c.id)
        .bind(&c.name)
        .bind(&c.description)
        .bind(&c.channel_type)
        .bind(c.enabled as i64)
        .bind(config)
        .bind(c.created_at.to_rfc3339())
        .bind(c.last_used.map(|t| t.to_rfc3339()))
        .bind(c.success_count as i64)
        .bind(c.failure_count as i64)
        .execute(self.db.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    fn public(c: &NotificationChannel) -> NotificationChannel {
        let mut c = c.clone();
        c.config = c.config.masked();
        c
    }

    fn check(c: &NotificationChannel) -> Result<(), StoreError> {
        if c.name.trim().is_empty() {
            return Err(StoreError::Invalid("channel name is required".into()));
        }
        if !c.channel_type.is_empty() && c.channel_type.to_lowercase() != c.config.type_name() {
            return Err(StoreError::Invalid(format!(
                "channel_type '{}' does not match the {} configuration",
                c.channel_type,
                c.config.type_name()
            )));
        }
        c.config.validate().map_err(StoreError::Invalid)
    }

    pub async fn list(&self) -> Vec<NotificationChannel> {
        self.channels
            .read()
            .await
            .iter()
            .map(Self::public)
            .collect()
    }

    pub async fn get(&self, id: &str) -> Result<NotificationChannel, StoreError> {
        self.channels
            .read()
            .await
            .iter()
            .find(|c| c.id == id)
            .map(Self::public)
            .ok_or_else(|| StoreError::NotFound(format!("notification channel '{}' not found", id)))
    }

    pub async fn create(
        &self,
        mut c: NotificationChannel,
    ) -> Result<NotificationChannel, StoreError> {
        Self::check(&c)?;
        c.id = uuid::Uuid::new_v4().to_string();
        c.channel_type = c.config.type_name().to_string();
        c.created_at = Utc::now();
        c.last_used = None;
        c.success_count = 0;
        c.failure_count = 0;
        self.save(&c).await?;
        self.channels.write().await.push(c.clone());
        Ok(Self::public(&c))
    }

    pub async fn update(
        &self,
        id: &str,
        mut c: NotificationChannel,
    ) -> Result<NotificationChannel, StoreError> {
        let mut channels = self.channels.write().await;
        let existing = channels.iter_mut().find(|x| x.id == id).ok_or_else(|| {
            StoreError::NotFound(format!("notification channel '{}' not found", id))
        })?;
        c.config.unmask_from(&existing.config);
        Self::check(&c)?;
        let updated = NotificationChannel {
            id: existing.id.clone(),
            name: c.name,
            description: c.description.filter(|d| !d.is_empty()),
            channel_type: c.config.type_name().to_string(),
            enabled: c.enabled,
            config: c.config,
            created_at: existing.created_at,
            last_used: existing.last_used,
            success_count: existing.success_count,
            failure_count: existing.failure_count,
        };
        self.save(&updated).await?;
        *existing = updated.clone();
        Ok(Self::public(&updated))
    }

    pub async fn delete(&self, id: &str) -> Result<(), StoreError> {
        let mut channels = self.channels.write().await;
        let before = channels.len();
        channels.retain(|c| c.id != id);
        if channels.len() == before {
            return Err(StoreError::NotFound(format!(
                "notification channel '{}' not found",
                id
            )));
        }
        sqlx::query("DELETE FROM notification_channels WHERE id = ?")
            .bind(id)
            .execute(self.db.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn record(&self, id: &str, ok: bool) {
        let updated = {
            let mut channels = self.channels.write().await;
            let Some(c) = channels.iter_mut().find(|c| c.id == id) else {
                return;
            };
            c.last_used = Some(Utc::now());
            if ok {
                c.success_count += 1;
            } else {
                c.failure_count += 1;
            }
            c.clone()
        };
        if let Err(e) = self.save(&updated).await {
            tracing::warn!("could not record delivery for channel {}: {:?}", id, e);
        }
    }

    /// Send a test message through one channel (even when disabled: testing
    /// a channel before enabling it is the point).
    pub async fn test(&self, id: &str) -> Result<Result<(), String>, StoreError> {
        let channel = self
            .channels
            .read()
            .await
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .ok_or_else(|| {
                StoreError::NotFound(format!("notification channel '{}' not found", id))
            })?;
        let result = deliver(&self.http, &channel, &Message::test(&channel.name)).await;
        self.record(id, result.is_ok()).await;
        Ok(result)
    }

    /// Deliver a fired alert to every enabled channel (all channels at the
    /// same time; each one retried with backoff under the delivery policy,
    /// and a delivery that fails every attempt is audited). Returns
    /// (delivered, failed) channel counts.
    pub async fn dispatch(&self, msg: &Message) -> (usize, usize) {
        let channels: Vec<NotificationChannel> = self
            .channels
            .read()
            .await
            .iter()
            .filter(|c| c.enabled)
            .cloned()
            .collect();
        let results = futures::future::join_all(
            channels
                .iter()
                .map(|c| async move { (c, self.deliver_retrying(c, msg).await) }),
        )
        .await;
        let (mut delivered, mut failed) = (0, 0);
        for (c, result) in results {
            if let Err(e) = &result {
                tracing::warn!("notification via '{}' ({}) failed: {}", c.name, c.id, e);
                failed += 1;
            } else {
                delivered += 1;
            }
            self.record(&c.id, result.is_ok()).await;
        }
        (delivered, failed)
    }

    /// Install the hook that records a delivery that failed every attempt in
    /// the audit log (set once).
    pub fn attach_audit(&self, hook: impl Fn(DeliveryFailure) + Send + Sync + 'static) {
        let _ = self.audit.set(Arc::new(hook));
    }

    /// The delivery policy in force.
    pub fn policy(&self) -> &DeliveryPolicy {
        &self.policy
    }

    async fn deliver_retrying(
        &self,
        channel: &NotificationChannel,
        msg: &Message,
    ) -> Result<(), String> {
        let attempts = self.policy.attempts.max(1);
        let mut last = String::new();
        for attempt in 1..=attempts {
            let wait = self.policy.wait_before(attempt);
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
            match deliver(&self.http, channel, msg).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    tracing::warn!(
                        "notification via '{}' failed (attempt {}/{}): {}",
                        channel.name,
                        attempt,
                        attempts,
                        e
                    );
                    last = e;
                }
            }
        }
        if let Some(hook) = self.audit.get() {
            hook(DeliveryFailure {
                channel_id: channel.id.clone(),
                channel_name: channel.name.clone(),
                channel_type: channel.config.type_name().to_string(),
                event: msg.event.clone(),
                title: msg.title.clone(),
                severity: msg.severity.clone(),
                attempts,
                error: last.clone(),
            });
        }
        Err(last)
    }
}

// ---------------------------------------------------------------------------
// Delivery
// ---------------------------------------------------------------------------

async fn send_http(req: reqwest::RequestBuilder, what: &str) -> Result<(), String> {
    let resp = req
        .send()
        .await
        .map_err(|e| format!("{}: request failed: {}", what, e))?;
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let body = resp.text().await.unwrap_or_default();
    Err(format!(
        "{}: HTTP {}: {}",
        what,
        status.as_u16(),
        body.chars().take(300).collect::<String>()
    ))
}

fn pagerduty_url() -> String {
    std::env::var("HORCRUX_PAGERDUTY_EVENTS_URL")
        .unwrap_or_else(|_| "https://events.pagerduty.com/v2/enqueue".to_string())
}

/// Really send `msg` through `channel`.
pub async fn deliver(
    http: &reqwest::Client,
    channel: &NotificationChannel,
    msg: &Message,
) -> Result<(), String> {
    let text = format!("{}\n{}", msg.title, msg.body);
    match &channel.config {
        ChannelConfig::Email {
            address,
            smtp_server,
            smtp_port,
            username,
            password,
            use_tls,
        } => {
            send_email(
                address,
                smtp_server,
                *smtp_port,
                username,
                password,
                *use_tls,
                msg,
            )
            .await
        }
        ChannelConfig::Slack {
            webhook_url,
            channel: slack_channel,
            username,
            icon_emoji,
        } => {
            let mut body = serde_json::json!({ "text": text });
            if let Some(c) = slack_channel.as_deref().filter(|s| !s.is_empty()) {
                body["channel"] = c.into();
            }
            if let Some(u) = username.as_deref().filter(|s| !s.is_empty()) {
                body["username"] = u.into();
            }
            if let Some(i) = icon_emoji.as_deref().filter(|s| !s.is_empty()) {
                body["icon_emoji"] = i.into();
            }
            send_http(http.post(webhook_url.trim()).json(&body), "Slack").await
        }
        ChannelConfig::Teams { webhook_url } => {
            let body = serde_json::json!({
                "@type": "MessageCard",
                "@context": "https://schema.org/extensions",
                "summary": msg.title,
                "themeColor": match msg.severity.as_str() { "critical" => "D13438", "warning" => "FFB900", _ => "0078D4" },
                "title": msg.title,
                "text": msg.body.replace('\n', "<br>"),
            });
            send_http(http.post(webhook_url.trim()).json(&body), "Teams").await
        }
        ChannelConfig::Ntfy {
            server_url,
            topic,
            token,
            priority,
        } => {
            let url = format!(
                "{}/{}",
                server_url.trim().trim_end_matches('/'),
                topic.trim()
            );
            let prio = match priority.as_deref().filter(|p| !p.is_empty()) {
                Some(p) => ntfy_priority(p)?,
                None => match msg.severity.as_str() {
                    "critical" => "urgent",
                    "warning" => "high",
                    _ => "default",
                },
            };
            // Header values must be one line of visible ASCII.
            let title: String = msg
                .title
                .chars()
                .map(|c| {
                    if c.is_ascii() && !c.is_control() {
                        c
                    } else {
                        ' '
                    }
                })
                .collect();
            let mut req = http
                .post(url)
                .header("Title", title)
                .header("Priority", prio)
                .header("Tags", format!("{},{}", msg.event, msg.severity))
                .body(text.clone());
            if !token.is_empty() {
                req = req.bearer_auth(token);
            }
            send_http(req, "ntfy").await
        }
        ChannelConfig::Discord {
            webhook_url,
            username,
            avatar_url,
        } => {
            let mut content = text.clone();
            if content.chars().count() > 1900 {
                content = content.chars().take(1900).collect::<String>() + "...";
            }
            let mut body = serde_json::json!({ "content": content });
            if let Some(u) = username.as_deref().filter(|s| !s.is_empty()) {
                body["username"] = u.into();
            }
            if let Some(a) = avatar_url.as_deref().filter(|s| !s.is_empty()) {
                body["avatar_url"] = a.into();
            }
            send_http(http.post(webhook_url.trim()).json(&body), "Discord").await
        }
        ChannelConfig::PagerDuty {
            integration_key,
            routing_key,
            severity,
        } => {
            let key = if routing_key.trim().is_empty() {
                integration_key.trim()
            } else {
                routing_key.trim()
            };
            // Alert severity wins; the channel's severity applies to tests.
            let sev = match msg.event.as_str() {
                "alert" => pd_severity(&msg.severity).unwrap_or("error"),
                _ => pd_severity(severity).unwrap_or("info"),
            };
            let source = hostname::get()
                .map(|h| h.to_string_lossy().to_string())
                .unwrap_or_else(|_| "horcrux".into());
            let mut body = serde_json::json!({
                "routing_key": key,
                "event_action": "trigger",
                "payload": {
                    "summary": msg.title,
                    "source": source,
                    "severity": sev,
                    "component": "horcrux",
                    "custom_details": { "message": msg.body },
                },
            });
            if let Some(a) = &msg.alert {
                body["dedup_key"] = format!("horcrux-{}-{}", a.rule_id, a.target).into();
            }
            send_http(http.post(pagerduty_url()).json(&body), "PagerDuty").await
        }
        ChannelConfig::Webhook {
            url,
            method,
            headers,
            auth,
        } => {
            let payload = serde_json::json!({
                "event": msg.event,
                "title": msg.title,
                "message": msg.body,
                "severity": msg.severity,
                "channel": channel.name,
                "timestamp": Utc::now().to_rfc3339(),
                "alert": msg.alert,
            });
            let url = url.trim();
            let mut req = match method.to_uppercase().as_str() {
                "PUT" => http.put(url),
                "PATCH" => http.patch(url),
                "GET" => http.get(url),
                _ => http.post(url),
            };
            if !method.eq_ignore_ascii_case("GET") {
                req = req.json(&payload);
            }
            for (k, v) in headers {
                req = req.header(k.as_str(), v.as_str());
            }
            req = match auth {
                Some(WebhookAuth::BasicAuth { username, password }) => {
                    req.basic_auth(username, Some(password))
                }
                Some(WebhookAuth::BearerToken { token }) => req.bearer_auth(token),
                Some(WebhookAuth::ApiKey { key, header }) => req.header(header.as_str(), key),
                None => req,
            };
            send_http(req, "webhook").await
        }
    }
}

async fn send_email(
    address: &str,
    server: &str,
    port: u16,
    username: &str,
    password: &str,
    use_tls: bool,
    msg: &Message,
) -> Result<(), String> {
    use lettre::message::header::ContentType;
    use lettre::{AsyncTransport, Message as Mail};

    let host = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "localhost".into());
    let from = if username.contains('@') {
        username.to_string()
    } else {
        format!("horcrux@{}", host)
    };
    let from: lettre::message::Mailbox = format!("Horcrux <{}>", from)
        .parse()
        .map_err(|e| format!("invalid sender address '{}': {}", from, e))?;
    let mut builder = Mail::builder().from(from).subject(&msg.title);
    for r in recipients(address) {
        builder = builder.to(r
            .parse()
            .map_err(|e| format!("invalid recipient '{}': {}", r, e))?);
    }
    let mail = builder
        .header(ContentType::TEXT_PLAIN)
        .body(format!("{}\n\n--\nHorcrux on {}\n", msg.body, host))
        .map_err(|e| format!("could not build the e-mail: {}", e))?;

    let transport = smtp_transport(server, port, username, password, use_tls)?;
    let result = transport
        .send(mail)
        .await
        .map(|_| ())
        .map_err(|e| format!("SMTP delivery via {}:{} failed: {}", server, port, e));
    if result.is_err() {
        // A failed send may mean the pooled connections are dead (server
        // restart, idle timeout): drop the cached transport so the next
        // notification starts from a fresh pool.
        evict_smtp_transport(server, port, username, password, use_tls);
    }
    result
}

type SmtpPool = lettre::AsyncSmtpTransport<lettre::Tokio1Executor>;

/// Pooled SMTP transports, one per (server, port, TLS mode, login). lettre's
/// pool keeps connections open and reuses them, so a burst of alerts to the
/// same relay shares a few connections instead of one TCP+TLS+AUTH handshake
/// each. The password is part of the key (hashed) so editing a channel's
/// credentials never reuses a transport that still holds the old login.
fn smtp_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, SmtpPool>> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, SmtpPool>>,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn smtp_cache_key(
    server: &str,
    port: u16,
    username: &str,
    password: &str,
    use_tls: bool,
) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    password.hash(&mut h);
    format!("{server}|{port}|{use_tls}|{username}|{:x}", h.finish())
}

fn evict_smtp_transport(server: &str, port: u16, username: &str, password: &str, use_tls: bool) {
    if let Ok(mut c) = smtp_cache().lock() {
        c.remove(&smtp_cache_key(server, port, username, password, use_tls));
    }
}

fn smtp_transport(
    server: &str,
    port: u16,
    username: &str,
    password: &str,
    use_tls: bool,
) -> Result<SmtpPool, String> {
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{AsyncSmtpTransport, Tokio1Executor};

    let key = smtp_cache_key(server, port, username, password, use_tls);
    if let Ok(c) = smtp_cache().lock() {
        if let Some(t) = c.get(&key) {
            return Ok(t.clone());
        }
    }
    let mut builder = if use_tls {
        if port == 465 {
            AsyncSmtpTransport::<Tokio1Executor>::relay(server)
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(server)
        }
        .map_err(|e| format!("SMTP TLS setup for {}: {}", server, e))?
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(server)
    }
    .port(port)
    .timeout(Some(Duration::from_secs(
        SMTP_TIMEOUT_SECS.load(std::sync::atomic::Ordering::Relaxed),
    )));
    if !username.is_empty() {
        builder = builder.credentials(Credentials::new(username.to_string(), password.to_string()));
    }
    let transport = builder.build();
    if let Ok(mut c) = smtp_cache().lock() {
        if c.len() >= 32 {
            c.clear(); // channels are few; this only bounds a pathological churn
        }
        c.insert(key, transport.clone());
    }
    Ok(transport)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn smtp_transports_are_cached_per_login() {
        let n = || smtp_cache().lock().unwrap().len();
        let before = n();
        smtp_transport("pool-test.invalid", 25, "u", "p1", false).unwrap();
        assert_eq!(n(), before + 1);
        smtp_transport("pool-test.invalid", 25, "u", "p1", false).unwrap();
        assert_eq!(n(), before + 1, "same login reuses the pooled transport");
        smtp_transport("pool-test.invalid", 25, "u", "p2", false).unwrap();
        assert_eq!(n(), before + 2, "a changed password gets a fresh transport");
        evict_smtp_transport("pool-test.invalid", 25, "u", "p1", false);
        evict_smtp_transport("pool-test.invalid", 25, "u", "p2", false);
        assert_eq!(n(), before);
    }

    fn ui_json() -> &'static str {
        // Exactly what the web UI's create form posts.
        r#"{"id":"channel-1","name":"ops","description":null,"channel_type":"email","enabled":true,
            "config":{"type":"Email","address":"ops@example.org","smtp_server":"mail.example.org","smtp_port":587,"username":"u","password":"secret","use_tls":true},
            "created_at":"2026-10-16T10:00:00Z","last_used":null,"success_count":0,"failure_count":0}"#
    }

    #[test]
    fn delivery_policy_defaults_and_backoff_growth() {
        let d = DeliveryPolicy::default();
        assert_eq!(
            (d.attempts, d.backoff_ms, d.backoff_factor, d.timeout_secs),
            (3, 1000, 2, 20)
        );
        assert_eq!(d.wait_before(1), Duration::ZERO);
        assert_eq!(d.wait_before(2), Duration::from_secs(1));
        assert_eq!(d.wait_before(3), Duration::from_secs(2));
        assert_eq!(d.wait_before(4), Duration::from_secs(4));
        let p = DeliveryPolicy {
            attempts: 4,
            backoff_ms: 50,
            backoff_factor: 3,
            timeout_secs: 1,
        };
        assert_eq!(p.wait_before(3), Duration::from_millis(150));
    }

    fn ntfy(server: &str, topic: &str, token: &str, prio: Option<&str>) -> ChannelConfig {
        ChannelConfig::Ntfy {
            server_url: server.into(),
            topic: topic.into(),
            token: token.into(),
            priority: prio.map(String::from),
        }
    }

    #[test]
    fn ntfy_channel_validates_masks_and_restores_its_token() {
        ntfy("https://ntfy.example", "alerts_1", "", None)
            .validate()
            .unwrap();
        ntfy("http://127.0.0.1:8080", "a-b", "tk", Some("urgent"))
            .validate()
            .unwrap();
        for bad in [
            ntfy("ftp://x", "t", "", None),
            ntfy("https://n.example", "", "", None),
            ntfy("https://n.example", "a/b", "", None),
            ntfy("https://n.example", "t", "", Some("loud")),
            ntfy("https://n.example", "t", SECRET_MASK, None),
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
        let c = ntfy("https://n.example", "t", "secret-token", None);
        let masked = c.masked();
        assert_eq!(masked, ntfy("https://n.example", "t", SECRET_MASK, None));
        let mut back = masked;
        back.unmask_from(&c);
        assert_eq!(back, c);
        // the server URL defaults when omitted
        let v: ChannelConfig = serde_json::from_str(r#"{"type":"Ntfy","topic":"t"}"#).unwrap();
        assert_eq!(v, ntfy("https://ntfy.sh", "t", "", None));
        assert_eq!(v.type_name(), "ntfy");
    }

    #[test]
    fn ui_channel_json_parses_and_validates() {
        let c: NotificationChannel = serde_json::from_str(ui_json()).unwrap();
        assert_eq!(c.config.type_name(), "email");
        NotificationChannelStore::check(&c).unwrap();
    }

    #[test]
    fn secrets_are_masked_and_restored() {
        let c: NotificationChannel = serde_json::from_str(ui_json()).unwrap();
        let masked = c.config.masked();
        match &masked {
            ChannelConfig::Email { password, .. } => assert_eq!(password, SECRET_MASK),
            _ => unreachable!(),
        }
        let mut back = masked.clone();
        back.unmask_from(&c.config);
        assert_eq!(back, c.config);
    }

    #[test]
    fn invalid_configs_are_rejected() {
        let bad = ChannelConfig::Webhook {
            url: "ftp://x".into(),
            method: "POST".into(),
            headers: HashMap::new(),
            auth: None,
        };
        assert!(bad.validate().is_err());
        let bad = ChannelConfig::PagerDuty {
            integration_key: "k".into(),
            routing_key: String::new(),
            severity: "loud".into(),
        };
        assert!(bad.validate().is_err());
        let bad = ChannelConfig::Email {
            address: "not-an-address".into(),
            smtp_server: "x".into(),
            smtp_port: 25,
            username: String::new(),
            password: String::new(),
            use_tls: false,
        };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn webhook_urls_keys_and_headers_are_masked_and_restored() {
        let slack = ChannelConfig::Slack {
            webhook_url: "https://hooks.slack.com/services/T0/B0/xyzzy".into(),
            channel: Some("#ops".into()),
            username: None,
            icon_emoji: None,
        };
        let shown = slack.masked();
        match &shown {
            ChannelConfig::Slack { webhook_url, .. } => {
                assert_eq!(webhook_url, "https://hooks.slack.com/********");
            }
            _ => unreachable!(),
        }
        let mut back = shown.clone();
        back.unmask_from(&slack);
        assert_eq!(back, slack);
        // A new URL replaces the stored one; the bare mask keeps it.
        let mut new = ChannelConfig::Slack {
            webhook_url: "https://hooks.slack.com/services/T0/B0/new".into(),
            channel: None,
            username: None,
            icon_emoji: None,
        };
        new.unmask_from(&slack);
        assert!(
            matches!(&new, ChannelConfig::Slack { webhook_url, .. } if webhook_url.ends_with("/new"))
        );
        let mut bare = ChannelConfig::Teams {
            webhook_url: SECRET_MASK.into(),
        };
        let teams = ChannelConfig::Teams {
            webhook_url: "https://example.webhook.office.com:8443/webhookb2/abc".into(),
        };
        assert_eq!(
            teams.masked(),
            ChannelConfig::Teams {
                webhook_url: "https://example.webhook.office.com:8443/********".into()
            }
        );
        bare.unmask_from(&teams);
        assert_eq!(bare, teams);
        // A masked URL with nothing to restore it from is refused, not stored.
        assert!(teams.masked().validate().is_err());
        let discord = ChannelConfig::Discord {
            webhook_url: "https://discord.com/api/webhooks/1/tok".into(),
            username: None,
            avatar_url: None,
        };
        let shown = discord.masked();
        assert!(!serde_json::to_string(&shown).unwrap().contains("tok"));
        let mut back = shown;
        back.unmask_from(&discord);
        assert_eq!(back, discord);

        // PagerDuty: created over the API with only a routing key; the UI
        // sends its single key field (the masked integration key) as both.
        let pd = ChannelConfig::PagerDuty {
            integration_key: String::new(),
            routing_key: "R0UTE".into(),
            severity: "error".into(),
        };
        let shown = pd.masked();
        assert!(!serde_json::to_string(&shown).unwrap().contains("R0UTE"));
        let mut from_ui = ChannelConfig::PagerDuty {
            integration_key: SECRET_MASK.into(),
            routing_key: SECRET_MASK.into(),
            severity: "error".into(),
        };
        from_ui.unmask_from(&pd);
        assert_eq!(
            from_ui,
            ChannelConfig::PagerDuty {
                integration_key: "R0UTE".into(),
                routing_key: "R0UTE".into(),
                severity: "error".into(),
            }
        );

        // Generic webhook: header values are masked; a masked header the
        // stored channel lacks is dropped, not sent as "********".
        let mut headers = HashMap::new();
        headers.insert("X-Api-Key".to_string(), "k3y".to_string());
        let wh = ChannelConfig::Webhook {
            url: "https://example.org/hook".into(),
            method: "POST".into(),
            headers,
            auth: Some(WebhookAuth::BearerToken {
                token: "t0k".into(),
            }),
        };
        let shown = wh.masked();
        let json = serde_json::to_string(&shown).unwrap();
        assert!(!json.contains("k3y") && !json.contains("t0k"), "{json}");
        let mut back = shown.clone();
        if let ChannelConfig::Webhook { headers, .. } = &mut back {
            headers.insert("X-Stray".into(), SECRET_MASK.into());
            headers.insert("X-New".into(), "v".into());
        }
        back.unmask_from(&wh);
        match back {
            ChannelConfig::Webhook { headers, auth, .. } => {
                assert_eq!(headers.get("X-Api-Key").map(String::as_str), Some("k3y"));
                assert_eq!(headers.get("X-New").map(String::as_str), Some("v"));
                assert!(!headers.contains_key("X-Stray"));
                assert_eq!(
                    auth,
                    Some(WebhookAuth::BearerToken {
                        token: "t0k".into()
                    })
                );
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn generic_webhook_url_is_masked_and_restored() {
        // Path, query and userinfo all carry credentials in the wild.
        let stored = ChannelConfig::Webhook {
            url: "https://ops:pw-s3cret@ha.example.org:8443/api/webhook/id-s3cret?token=q-s3cret#f-s3cret"
                .into(),
            method: "POST".into(),
            headers: HashMap::new(),
            auth: None,
        };
        let shown = stored.masked();
        match &shown {
            ChannelConfig::Webhook { url, .. } => {
                assert_eq!(url, "https://ha.example.org:8443/********")
            }
            _ => unreachable!(),
        }
        let json = serde_json::to_string(&shown).unwrap();
        assert!(!json.contains("s3cret") && !json.contains("ops"), "{json}");

        // Re-sent as shown (the UI's edit form), or as the bare mask: the
        // stored URL survives, so the channel still validates and delivers.
        for sent in ["https://ha.example.org:8443/********", SECRET_MASK] {
            let mut back = shown.clone();
            if let ChannelConfig::Webhook { url, .. } = &mut back {
                *url = sent.into();
            }
            back.unmask_from(&stored);
            assert_eq!(back, stored, "sent {sent}");
            assert!(back.validate().is_ok());
        }

        // A real edit replaces it; a masked URL for a different host is not
        // ours to restore and is refused rather than stored literally.
        let mut back = shown.clone();
        if let ChannelConfig::Webhook { url, .. } = &mut back {
            *url = "https://new.example.org/hook".into();
        }
        back.unmask_from(&stored);
        match &back {
            ChannelConfig::Webhook { url, .. } => assert_eq!(url, "https://new.example.org/hook"),
            _ => unreachable!(),
        }
        let mut back = shown.clone();
        if let ChannelConfig::Webhook { url, .. } = &mut back {
            *url = "https://other.example.org/********".into();
        }
        back.unmask_from(&stored);
        assert!(back.validate().is_err());
        // Nothing to restore from (a new channel): refused too.
        assert!(shown.validate().is_err());
    }

    #[test]
    fn masked_secrets_are_refused_without_a_stored_value() {
        let wh = |headers: &[(&str, &str)], auth: Option<WebhookAuth>| ChannelConfig::Webhook {
            url: "https://h.example.org/hook".into(),
            method: "POST".into(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            auth,
        };
        assert!(wh(&[("X-A", "real")], None).validate().is_ok());
        assert!(wh(&[("X-A", SECRET_MASK)], None).validate().is_err());
        for auth in [
            WebhookAuth::BearerToken {
                token: SECRET_MASK.into(),
            },
            WebhookAuth::BasicAuth {
                username: "u".into(),
                password: SECRET_MASK.into(),
            },
            WebhookAuth::ApiKey {
                key: SECRET_MASK.into(),
                header: "X-Key".into(),
            },
        ] {
            assert!(wh(&[], Some(auth)).validate().is_err());
        }
        let email = |pw: &str| ChannelConfig::Email {
            address: "a@errant.solutions".into(),
            smtp_server: "smtp.example.org".into(),
            smtp_port: 587,
            username: "u".into(),
            password: pw.into(),
            use_tls: true,
        };
        assert!(email("pw").validate().is_ok());
        assert!(email(SECRET_MASK).validate().is_err());
        let pd = |k: &str| ChannelConfig::PagerDuty {
            integration_key: k.into(),
            routing_key: k.into(),
            severity: "error".into(),
        };
        assert!(pd("key").validate().is_ok());
        assert!(pd(SECRET_MASK).validate().is_err());

        // Same type update re-sending the mask still restores the stored
        // secret; a type change (nothing to restore) is refused.
        let stored = wh(
            &[("X-A", "real")],
            Some(WebhookAuth::BearerToken {
                token: "tok".into(),
            }),
        );
        let mut back = stored.masked();
        back.unmask_from(&stored);
        assert_eq!(back, stored);
        assert!(back.validate().is_ok());
        let mut changed = email(SECRET_MASK);
        changed.unmask_from(&stored);
        assert!(changed.validate().is_err());
    }
}
