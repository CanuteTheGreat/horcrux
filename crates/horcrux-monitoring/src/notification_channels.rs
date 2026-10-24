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

/// Persistent channel registry.
pub struct NotificationChannelStore {
    db: Arc<Database>,
    channels: RwLock<Vec<NotificationChannel>>,
    http: reqwest::Client,
}

fn db_err(e: sqlx::Error) -> StoreError {
    StoreError::Db(format!("notification channels: {}", e))
}

impl NotificationChannelStore {
    /// Load all channels from the database.
    pub async fn load(db: Arc<Database>) -> Result<Self, StoreError> {
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
        Ok(Self {
            db,
            channels: RwLock::new(channels),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .expect("reqwest client"),
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

    /// Deliver a fired alert to every enabled channel. Returns
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
        let (mut delivered, mut failed) = (0, 0);
        for c in channels {
            let result = deliver(&self.http, &c, msg).await;
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
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{AsyncSmtpTransport, AsyncTransport, Message as Mail, Tokio1Executor};

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

    let mut transport = if use_tls {
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
    .timeout(Some(Duration::from_secs(20)));
    if !username.is_empty() {
        transport =
            transport.credentials(Credentials::new(username.to_string(), password.to_string()));
    }
    transport
        .build()
        .send(mail)
        .await
        .map(|_| ())
        .map_err(|e| format!("SMTP delivery via {}:{} failed: {}", server, port, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ui_json() -> &'static str {
        // Exactly what the web UI's create form posts.
        r#"{"id":"channel-1","name":"ops","description":null,"channel_type":"email","enabled":true,
            "config":{"type":"Email","address":"ops@example.org","smtp_server":"mail.example.org","smtp_port":587,"username":"u","password":"secret","use_tls":true},
            "created_at":"2026-10-16T10:00:00Z","last_used":null,"success_count":0,"failure_count":0}"#
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
