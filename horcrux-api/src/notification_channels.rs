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
    fn masked(&self) -> Self {
        let mask = |s: &str| {
            if s.is_empty() {
                String::new()
            } else {
                SECRET_MASK.to_string()
            }
        };
        let mut c = self.clone();
        match &mut c {
            ChannelConfig::Email { password, .. } => *password = mask(password),
            ChannelConfig::Webhook { auth: Some(a), .. } => match a {
                WebhookAuth::BasicAuth { password, .. } => *password = mask(password),
                WebhookAuth::BearerToken { token } => *token = mask(token),
                WebhookAuth::ApiKey { key, .. } => *key = mask(key),
            },
            _ => {}
        }
        c
    }

    /// Put stored secrets back where an update sent the mask.
    fn unmask_from(&mut self, stored: &ChannelConfig) {
        match (self, stored) {
            (
                ChannelConfig::Email { password, .. },
                ChannelConfig::Email {
                    password: old_pw, ..
                },
            ) if password == SECRET_MASK => *password = old_pw.clone(),
            (
                ChannelConfig::Webhook { auth: Some(a), .. },
                ChannelConfig::Webhook {
                    auth: Some(old), ..
                },
            ) => match (a, old) {
                (
                    WebhookAuth::BasicAuth { password, .. },
                    WebhookAuth::BasicAuth { password: p, .. },
                ) if password == SECRET_MASK => *password = p.clone(),
                (WebhookAuth::BearerToken { token }, WebhookAuth::BearerToken { token: t })
                    if token == SECRET_MASK =>
                {
                    *token = t.clone()
                }
                (WebhookAuth::ApiKey { key, .. }, WebhookAuth::ApiKey { key: k, .. })
                    if key == SECRET_MASK =>
                {
                    *key = k.clone()
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn validate(&self) -> Result<(), String> {
        fn http_url(what: &str, u: &str) -> Result<(), String> {
            match url::Url::parse(u.trim()) {
                Ok(p) if p.scheme() == "http" || p.scheme() == "https" => Ok(()),
                _ => Err(format!("{} must be an http(s) URL, got '{}'", what, u)),
            }
        }
        match self {
            ChannelConfig::Email {
                address,
                smtp_server,
                smtp_port,
                ..
            } => {
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
                if integration_key.trim().is_empty() && routing_key.trim().is_empty() {
                    return Err("PagerDuty routing/integration key is required".into());
                }
                pd_severity(severity)?;
                Ok(())
            }
            ChannelConfig::Webhook { url, method, .. } => {
                http_url("webhook URL", url)?;
                match method.to_uppercase().as_str() {
                    "POST" | "PUT" | "PATCH" | "GET" => Ok(()),
                    m => Err(format!("unsupported webhook method '{}'", m)),
                }
            }
        }
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

    /// Deliver a fired alert to every enabled channel.
    pub async fn dispatch(&self, msg: &Message) {
        let channels: Vec<NotificationChannel> = self
            .channels
            .read()
            .await
            .iter()
            .filter(|c| c.enabled)
            .cloned()
            .collect();
        for c in channels {
            let result = deliver(&self.http, &c, msg).await;
            if let Err(e) = &result {
                tracing::warn!("notification via '{}' ({}) failed: {}", c.name, c.id, e);
            }
            self.record(&c.id, result.is_ok()).await;
        }
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
}
