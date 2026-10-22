//! Webhook notification system
//! Sends HTTP POST requests to configured endpoints when events occur
use crate::notification_channels::{mask_url, restore_masked_url, SECRET_MASK};
use horcrux_common::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Webhook event type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum WebhookEventType {
    VmCreated,
    VmStarted,
    VmStopped,
    /// Any VM state change (running/stopped/paused), with old and new state.
    VmStateChanged,
    /// HA restarted a VM on another node.
    HaFailover,
    VmDeleted,
    VmMigrationStarted,
    VmMigrationCompleted,
    VmMigrationFailed,
    BackupCreated,
    BackupFailed,
    BackupDeleted,
    NodeJoined,
    NodeLeft,
    NodeFailed,
    AlertTriggered,
    AlertResolved,
    StoragePoolAdded,
    StoragePoolRemoved,
    AuthenticationFailed,
    Custom(String),
}

impl WebhookEventType {
    pub fn as_str(&self) -> &str {
        match self {
            WebhookEventType::VmCreated => "vm.created",
            WebhookEventType::VmStarted => "vm.started",
            WebhookEventType::VmStopped => "vm.stopped",
            WebhookEventType::VmStateChanged => "vm.state_changed",
            WebhookEventType::HaFailover => "ha.failover",
            WebhookEventType::VmDeleted => "vm.deleted",
            WebhookEventType::VmMigrationStarted => "vm.migration.started",
            WebhookEventType::VmMigrationCompleted => "vm.migration.completed",
            WebhookEventType::VmMigrationFailed => "vm.migration.failed",
            WebhookEventType::BackupCreated => "backup.created",
            WebhookEventType::BackupFailed => "backup.failed",
            WebhookEventType::BackupDeleted => "backup.deleted",
            WebhookEventType::NodeJoined => "node.joined",
            WebhookEventType::NodeLeft => "node.left",
            WebhookEventType::NodeFailed => "node.failed",
            WebhookEventType::AlertTriggered => "alert.triggered",
            WebhookEventType::AlertResolved => "alert.resolved",
            WebhookEventType::StoragePoolAdded => "storage.pool.added",
            WebhookEventType::StoragePoolRemoved => "storage.pool.removed",
            WebhookEventType::AuthenticationFailed => "auth.failed",
            WebhookEventType::Custom(ref s) => s,
        }
    }
}

/// Webhook configuration. Every field has a default, so a create request
/// only needs the fields it sets (the documented `name`/`url`/`events`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct WebhookConfig {
    pub id: String,
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub events: Vec<WebhookEventType>,
    /// HMAC-SHA256 key for `X-Horcrux-Signature`. Never returned by the API
    /// (see [`WebhookConfig::masked`]).
    pub secret: Option<String>,
    /// Extra attempts after a failed first delivery.
    pub retry_count: u32,
    pub timeout_seconds: u64,
    /// Extra request headers. Their values are credentials more often than
    /// not (`Authorization`, API keys), so the API never returns them either.
    pub headers: HashMap<String, String>,
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: String::new(),
            url: String::new(),
            enabled: true,
            events: Vec::new(),
            secret: None,
            retry_count: 3,
            timeout_seconds: 30,
            headers: HashMap::new(),
        }
    }
}

/// Most retries a webhook may ask for.
pub const MAX_WEBHOOK_RETRIES: u32 = 10;
/// Longest per-request timeout a webhook may ask for.
pub const MAX_WEBHOOK_TIMEOUT_SECS: u64 = 300;
/// Longest wait between two attempts.
const MAX_BACKOFF_SECS: u64 = 60;
/// Delivery records kept in memory (oldest dropped first).
pub const MAX_DELIVERY_RECORDS: usize = 1000;

impl WebhookConfig {
    /// Refuse a configuration that could never be delivered.
    pub fn validate(&self) -> Result<()> {
        let invalid = |m: String| Err(horcrux_common::Error::Validation(m));
        if self.name.trim().is_empty() {
            return invalid("webhook name must not be empty".into());
        }
        if self.url.contains(SECRET_MASK) {
            // The masked URL of a list/get response, sent where there is no
            // stored URL to restore (a create, or an edit of another host).
            return invalid("webhook url is masked; enter the real URL".into());
        }
        match url::Url::parse(&self.url) {
            Ok(u) if (u.scheme() == "http" || u.scheme() == "https") && u.has_host() => {}
            _ => {
                return invalid(format!(
                    "webhook url must be an http(s) URL, got '{}'",
                    self.url
                ))
            }
        }
        if !(1..=MAX_WEBHOOK_TIMEOUT_SECS).contains(&self.timeout_seconds) {
            return invalid(format!(
                "timeout_seconds must be 1..={}, got {}",
                MAX_WEBHOOK_TIMEOUT_SECS, self.timeout_seconds
            ));
        }
        if self.retry_count > MAX_WEBHOOK_RETRIES {
            return invalid(format!(
                "retry_count must be at most {}, got {}",
                MAX_WEBHOOK_RETRIES, self.retry_count
            ));
        }
        for (k, v) in &self.headers {
            if reqwest::header::HeaderName::from_bytes(k.as_bytes()).is_err() {
                return invalid(format!("'{}' is not a valid HTTP header name", k));
            }
            if reqwest::header::HeaderValue::from_str(v).is_err() {
                return invalid(format!("header '{}' has an invalid value", k));
            }
        }
        Ok(())
    }

    /// Copy for API responses: the secret and every header value replaced
    /// by [`SECRET_MASK`], and the URL cut to scheme, host and port (its
    /// path, query or `user:pass@` is often the receiver's credential).
    /// Reading webhooks only needs Sys.Audit, and these let the reader forge
    /// signed events or call the receiver directly.
    pub fn masked(&self) -> Self {
        let mut c = self.clone();
        c.url = mask_url(&c.url);
        if c.secret.as_deref().is_some_and(|s| !s.is_empty()) {
            c.secret = Some(SECRET_MASK.to_string());
        }
        for v in c.headers.values_mut() {
            if !v.is_empty() {
                *v = SECRET_MASK.to_string();
            }
        }
        c
    }

    /// An update that sends [`SECRET_MASK`] back keeps the stored value. A
    /// masked header the stored webhook doesn't have is dropped rather than
    /// sent as the literal mask.
    fn unmask_from(&mut self, stored: &WebhookConfig) {
        restore_masked_url(&mut self.url, &stored.url);
        if self.secret.as_deref() == Some(SECRET_MASK) {
            self.secret = stored.secret.clone();
        }
        self.headers.retain(|k, v| {
            if v != SECRET_MASK {
                return true;
            }
            match stored.headers.get(k) {
                Some(old) => {
                    *v = old.clone();
                    true
                }
                None => false,
            }
        });
    }
}

/// Webhook event payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookEvent {
    pub event_type: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub source: String,
    pub data: serde_json::Value,
}

/// Webhook delivery status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookDelivery {
    pub webhook_id: String,
    pub event_type: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub success: bool,
    pub status_code: Option<u16>,
    pub error: Option<String>,
    pub retry_count: u32,
}

/// Delivery counters of one webhook (kept for its lifetime, unlike the
/// bounded delivery log).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WebhookStats {
    pub success_count: u64,
    pub failure_count: u64,
    pub last_triggered: Option<chrono::DateTime<chrono::Utc>>,
}

/// What the API returns for a webhook: its masked configuration plus its
/// delivery counters (the web UI's Webhooks table shows both).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookSummary {
    #[serde(flatten)]
    pub config: WebhookConfig,
    #[serde(flatten)]
    pub stats: WebhookStats,
}

/// Webhook manager
pub struct WebhookManager {
    webhooks: Arc<RwLock<HashMap<String, WebhookConfig>>>,
    deliveries: Arc<RwLock<Vec<WebhookDelivery>>>,
    stats: Arc<RwLock<HashMap<String, WebhookStats>>>,
    client: reqwest::Client,
    /// Durable copy of the webhook configurations; `None` = in memory only.
    store: Option<crate::db::config_store::ConfigStore>,
}

impl Default for WebhookManager {
    fn default() -> Self {
        Self::new()
    }
}

impl WebhookManager {
    pub fn new() -> Self {
        Self {
            webhooks: Arc::new(RwLock::new(HashMap::new())),
            deliveries: Arc::new(RwLock::new(Vec::new())),
            stats: Arc::new(RwLock::new(HashMap::new())),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap(),
            store: None,
        }
    }

    /// Webhook manager backed by the database: loads the saved webhooks and
    /// writes every change through (they used to vanish on restart).
    pub async fn with_database(db: &crate::db::Database) -> Result<Self> {
        let store = crate::db::config_store::ConfigStore::new(db);
        let mut mgr = Self::new();
        {
            let mut hooks = mgr.webhooks.write().await;
            for w in store
                .load_values::<WebhookConfig>(crate::db::config_store::kinds::WEBHOOK)
                .await?
            {
                hooks.insert(w.id.clone(), w);
            }
            tracing::info!("Loaded {} webhook(s) from database", hooks.len());
        }
        mgr.store = Some(store);
        Ok(mgr)
    }

    async fn persist(&self, config: &WebhookConfig) -> Result<()> {
        crate::db::config_store::put_opt(
            &self.store,
            crate::db::config_store::kinds::WEBHOOK,
            &config.id,
            config,
        )
        .await
    }

    /// Add a webhook configuration. An id that is already taken is refused
    /// (it used to replace that webhook silently).
    pub async fn add_webhook(&self, mut config: WebhookConfig) -> Result<WebhookConfig> {
        if config.id.trim().is_empty() {
            config.id = uuid::Uuid::new_v4().to_string();
        }
        config.validate()?;

        let mut webhooks = self.webhooks.write().await;
        if webhooks.contains_key(&config.id) {
            return Err(horcrux_common::Error::AlreadyExists(format!(
                "webhook {}",
                config.id
            )));
        }
        self.persist(&config).await?;
        webhooks.insert(config.id.clone(), config.clone());

        tracing::info!("Added webhook: {} ({})", config.name, config.id);
        Ok(config)
    }

    /// Remove a webhook
    pub async fn remove_webhook(&self, id: &str) -> Result<()> {
        let mut webhooks = self.webhooks.write().await;
        if webhooks.remove(id).is_none() {
            return Err(horcrux_common::Error::NotFound(format!("webhook {}", id)));
        }
        crate::db::config_store::delete_opt(
            &self.store,
            crate::db::config_store::kinds::WEBHOOK,
            id,
        )
        .await?;
        drop(webhooks);
        self.stats.write().await.remove(id);

        tracing::info!("Removed webhook: {}", id);
        Ok(())
    }

    /// List all webhooks (unmasked: for internal use, not API responses)
    pub async fn list_webhooks(&self) -> Vec<WebhookConfig> {
        let webhooks = self.webhooks.read().await;
        webhooks.values().cloned().collect()
    }

    /// Get a specific webhook (unmasked: for internal use, not API responses)
    pub async fn get_webhook(&self, id: &str) -> Option<WebhookConfig> {
        let webhooks = self.webhooks.read().await;
        webhooks.get(id).cloned()
    }

    /// Every webhook as the API shows it (masked, with counters), by name.
    pub async fn summaries(&self) -> Vec<WebhookSummary> {
        let webhooks = self.webhooks.read().await;
        let stats = self.stats.read().await;
        let mut out: Vec<WebhookSummary> = webhooks
            .values()
            .map(|w| WebhookSummary {
                config: w.masked(),
                stats: stats.get(&w.id).cloned().unwrap_or_default(),
            })
            .collect();
        out.sort_by(|a, b| {
            a.config
                .name
                .cmp(&b.config.name)
                .then_with(|| a.config.id.cmp(&b.config.id))
        });
        out
    }

    /// One webhook as the API shows it.
    pub async fn summary(&self, id: &str) -> Option<WebhookSummary> {
        let config = self.webhooks.read().await.get(id)?.masked();
        let stats = self.stats.read().await.get(id).cloned().unwrap_or_default();
        Some(WebhookSummary { config, stats })
    }

    /// Update a webhook. The id in the path wins over one in the body, and
    /// a masked secret/header value keeps the stored one.
    pub async fn update_webhook(&self, id: &str, mut config: WebhookConfig) -> Result<()> {
        let mut webhooks = self.webhooks.write().await;

        let Some(stored) = webhooks.get(id) else {
            return Err(horcrux_common::Error::NotFound(format!("webhook {}", id)));
        };
        config.id = id.to_string();
        config.unmask_from(stored);
        config.validate()?;

        self.persist(&config).await?;
        webhooks.insert(id.to_string(), config);
        tracing::info!("Updated webhook: {}", id);
        Ok(())
    }

    /// Send one `custom` test event to this webhook only, once (no
    /// retries), and report how it went. Works on a disabled webhook too.
    pub async fn test_webhook(&self, id: &str) -> Result<WebhookDelivery> {
        let webhook = self
            .get_webhook(id)
            .await
            .ok_or_else(|| horcrux_common::Error::NotFound(format!("webhook {}", id)))?;
        let event = WebhookEvent {
            event_type: "test".to_string(),
            timestamp: chrono::Utc::now(),
            source: "horcrux".to_string(),
            data: serde_json::json!({
                "message": "This is a test webhook event",
                "webhook_id": id,
            }),
        };
        let result = self.send_webhook_request(&webhook, &event).await;
        let delivery = Self::delivery_record(&webhook, &event, &result, 0);
        self.record_delivery(delivery.clone()).await;
        Ok(delivery)
    }

    /// Trigger a webhook event
    pub async fn trigger_event(
        &self,
        event_type: WebhookEventType,
        data: serde_json::Value,
    ) -> Result<()> {
        let event = WebhookEvent {
            event_type: event_type.as_str().to_string(),
            timestamp: chrono::Utc::now(),
            source: "horcrux".to_string(),
            data,
        };

        // Find all webhooks that subscribe to this event type
        let webhooks = self.webhooks.read().await;
        let matching_webhooks: Vec<WebhookConfig> = webhooks
            .values()
            .filter(|w| w.enabled && w.events.contains(&event_type))
            .cloned()
            .collect();
        drop(webhooks);

        // Send to all matching webhooks
        for webhook in matching_webhooks {
            let event_clone = event.clone();
            let manager_clone = self.clone_for_delivery();

            tokio::spawn(async move {
                manager_clone
                    .deliver_webhook(&webhook, event_clone)
                    .await
                    .ok();
            });
        }

        Ok(())
    }

    /// Clone for async delivery
    fn clone_for_delivery(&self) -> Self {
        Self {
            webhooks: self.webhooks.clone(),
            deliveries: self.deliveries.clone(),
            stats: self.stats.clone(),
            client: self.client.clone(),
            store: None,
        }
    }

    fn delivery_record(
        webhook: &WebhookConfig,
        event: &WebhookEvent,
        result: &Result<u16>,
        retries: u32,
    ) -> WebhookDelivery {
        WebhookDelivery {
            webhook_id: webhook.id.clone(),
            event_type: event.event_type.clone(),
            timestamp: chrono::Utc::now(),
            success: result.is_ok(),
            status_code: result.as_ref().ok().copied(),
            error: result.as_ref().err().map(|e| e.to_string()),
            retry_count: retries,
        }
    }

    /// Log a finished delivery (bounded) and count it for its webhook.
    async fn record_delivery(&self, delivery: WebhookDelivery) {
        {
            let mut stats = self.stats.write().await;
            let s = stats.entry(delivery.webhook_id.clone()).or_default();
            if delivery.success {
                s.success_count += 1;
            } else {
                s.failure_count += 1;
            }
            s.last_triggered = Some(delivery.timestamp);
        }
        let mut deliveries = self.deliveries.write().await;
        deliveries.push(delivery);
        if deliveries.len() > MAX_DELIVERY_RECORDS {
            let excess = deliveries.len() - MAX_DELIVERY_RECORDS;
            deliveries.drain(0..excess);
        }
    }

    /// Wait before retry number `retry` (1-based): 1s, 2s, 4s, ... capped.
    fn backoff(retry: u32) -> std::time::Duration {
        let secs = 2_u64.saturating_pow(retry.saturating_sub(1));
        std::time::Duration::from_secs(secs.min(MAX_BACKOFF_SECS))
    }

    /// Deliver webhook to endpoint: one attempt plus up to `retry_count`
    /// retries (a `retry_count` of 0 or 1 used to mean one attempt either
    /// way, and 3 meant three attempts in all).
    async fn deliver_webhook(&self, webhook: &WebhookConfig, event: WebhookEvent) -> Result<()> {
        let mut retries = 0;

        loop {
            let result = self.send_webhook_request(webhook, &event).await;
            if result.is_ok() || retries >= webhook.retry_count {
                let delivery = Self::delivery_record(webhook, &event, &result, retries);
                self.record_delivery(delivery).await;
                match &result {
                    Ok(code) => tracing::info!(
                        "Webhook delivered successfully: {} -> {} ({})",
                        event.event_type,
                        webhook.name,
                        code
                    ),
                    Err(e) => tracing::error!(
                        "Webhook delivery failed after {} retries: {} -> {} ({})",
                        retries,
                        event.event_type,
                        webhook.name,
                        e
                    ),
                }
                return result.map(|_| ());
            }
            retries += 1;
            tokio::time::sleep(Self::backoff(retries)).await;
        }
    }

    /// Send HTTP request to webhook endpoint
    async fn send_webhook_request(
        &self,
        webhook: &WebhookConfig,
        event: &WebhookEvent,
    ) -> Result<u16> {
        let mut request = self
            .client
            .post(&webhook.url)
            .json(event)
            .timeout(std::time::Duration::from_secs(webhook.timeout_seconds));

        // Add custom headers
        for (key, value) in &webhook.headers {
            request = request.header(key, value);
        }

        // Add HMAC signature if secret is configured
        if let Some(ref secret) = webhook.secret {
            let payload = serde_json::to_string(event)
                .map_err(|e| horcrux_common::Error::System(e.to_string()))?;

            let signature = self.generate_hmac_signature(secret, &payload);
            request = request.header("X-Horcrux-Signature", signature);
        }

        let response = request
            .send()
            .await
            .map_err(|e| horcrux_common::Error::System(e.to_string()))?;

        let status_code = response.status().as_u16();

        if !response.status().is_success() {
            return Err(horcrux_common::Error::System(format!(
                "Webhook returned error status: {}",
                status_code
            )));
        }

        Ok(status_code)
    }

    /// Generate a real HMAC-SHA256 signature for webhook payload, hex-encoded
    /// with a `sha256=` prefix (matches the common GitHub/Stripe webhook
    /// signature convention so receivers can verify with off-the-shelf
    /// libraries). Previously this used `DefaultHasher`, which is not a
    /// cryptographic hash and provided effectively no authenticity guarantee
    /// despite being advertised to consumers as "HMAC".
    fn generate_hmac_signature(&self, secret: &str, payload: &str) -> String {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        type HmacSha256 = Hmac<Sha256>;

        let mut mac =
            HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take a key of any size");
        mac.update(payload.as_bytes());
        let result = mac.finalize().into_bytes();

        format!("sha256={}", hex::encode(result))
    }

    /// Get recent webhook deliveries
    pub async fn get_deliveries(
        &self,
        webhook_id: Option<&str>,
        limit: usize,
    ) -> Vec<WebhookDelivery> {
        let deliveries = self.deliveries.read().await;

        let filtered: Vec<WebhookDelivery> = if let Some(id) = webhook_id {
            deliveries
                .iter()
                .filter(|d| d.webhook_id == id)
                .cloned()
                .collect()
        } else {
            deliveries.clone()
        };

        // Return most recent first
        filtered.into_iter().rev().take(limit).collect()
    }

    /// Clear old delivery records
    pub async fn cleanup_old_deliveries(&self, keep_count: usize) {
        let mut deliveries = self.deliveries.write().await;

        if deliveries.len() > keep_count {
            let remove_count = deliveries.len() - keep_count;
            deliveries.drain(0..remove_count);
            tracing::debug!("Cleaned up {} old webhook deliveries", remove_count);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_webhook_manager() {
        let manager = WebhookManager::new();

        let config = WebhookConfig {
            id: "test-webhook".to_string(),
            name: "Test Webhook".to_string(),
            url: "https://example.com/webhook".to_string(),
            enabled: true,
            events: vec![WebhookEventType::VmCreated],
            ..Default::default()
        };

        manager.add_webhook(config.clone()).await.unwrap();

        let webhooks = manager.list_webhooks().await;
        assert_eq!(webhooks.len(), 1);
        assert_eq!(webhooks[0].name, "Test Webhook");
    }

    fn hook(id: &str, url: &str) -> WebhookConfig {
        WebhookConfig {
            id: id.to_string(),
            name: format!("hook {id}"),
            url: url.to_string(),
            events: vec![WebhookEventType::VmCreated],
            ..Default::default()
        }
    }

    /// A local address nothing listens on (connection refused at once).
    async fn dead_url() -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        drop(l);
        format!("http://{addr}/hook")
    }

    #[test]
    fn a_create_body_only_needs_the_documented_fields() {
        let c: WebhookConfig = serde_json::from_value(serde_json::json!({
            "name": "Slack", "url": "https://hooks.example.org/x",
            "events": ["vm_started", {"custom": "test"}], "secret": "k"
        }))
        .unwrap();
        assert!(c.enabled);
        assert_eq!(c.retry_count, 3);
        assert_eq!(c.timeout_seconds, 30);
        assert_eq!(
            c.events,
            vec![
                WebhookEventType::VmStarted,
                WebhookEventType::Custom("test".into())
            ]
        );
        c.validate().unwrap();
    }

    #[test]
    fn validate_refuses_undeliverable_configs() {
        let ok = hook("v", "https://hooks.example.org/x");
        ok.validate().unwrap();
        let bad = |f: &dyn Fn(&mut WebhookConfig)| {
            let mut c = ok.clone();
            f(&mut c);
            assert!(
                matches!(c.validate(), Err(horcrux_common::Error::Validation(_))),
                "{c:?} must be refused"
            );
        };
        bad(&|c: &mut WebhookConfig| c.name = "  ".into());
        bad(&|c: &mut WebhookConfig| c.url = "ftp://example.org/x".into());
        bad(&|c: &mut WebhookConfig| c.url = "not a url".into());
        bad(&|c: &mut WebhookConfig| c.url = "http://".into());
        bad(&|c: &mut WebhookConfig| c.timeout_seconds = 0);
        bad(&|c: &mut WebhookConfig| c.timeout_seconds = MAX_WEBHOOK_TIMEOUT_SECS + 1);
        bad(&|c: &mut WebhookConfig| c.retry_count = MAX_WEBHOOK_RETRIES + 1);
        bad(&|c: &mut WebhookConfig| {
            c.headers.insert("Bad Header".into(), "v".into());
        });
        bad(&|c: &mut WebhookConfig| {
            c.headers.insert("X-Ok".into(), "line\nbreak".into());
        });
        let mut edge = ok.clone();
        edge.timeout_seconds = MAX_WEBHOOK_TIMEOUT_SECS;
        edge.retry_count = MAX_WEBHOOK_RETRIES;
        edge.headers
            .insert("Authorization".into(), "Bearer t".into());
        edge.validate().unwrap();
        edge.timeout_seconds = 1;
        edge.retry_count = 0;
        edge.validate().unwrap();
    }

    #[tokio::test]
    async fn api_views_never_show_the_secret_or_header_values() {
        let m = WebhookManager::new();
        let mut c = hook(
            "s",
            "https://bot:pw-key@hooks.example.org/x/path-key?t=q-key",
        );
        c.secret = Some("hmac-key".into());
        c.headers
            .insert("Authorization".into(), "Bearer tok".into());
        c.headers.insert("X-Empty".into(), String::new());
        m.add_webhook(c).await.unwrap();

        let all = m.summaries().await;
        let one = m.summary("s").await.unwrap();
        for s in [&all[0], &one] {
            assert_eq!(s.config.secret.as_deref(), Some(SECRET_MASK));
            assert_eq!(s.config.headers["Authorization"], SECRET_MASK);
            assert_eq!(s.config.headers["X-Empty"], "");
            assert_eq!(s.config.url, "https://hooks.example.org/********");
            let text = serde_json::to_string(s).unwrap();
            assert!(
                !text.contains("-key") && !text.contains("Bearer tok"),
                "{text}"
            );
        }
        // No secret stays no secret.
        m.add_webhook(hook("n", "https://hooks.example.org/y"))
            .await
            .unwrap();
        assert_eq!(m.summary("n").await.unwrap().config.secret, None);
        assert!(m.summary("missing").await.is_none());
    }

    #[tokio::test]
    async fn an_update_keeps_masked_secrets_and_the_path_id() {
        let m = WebhookManager::new();
        let mut c = hook("u", "https://hooks.example.org/x?token=t");
        c.secret = Some("hmac-key".into());
        c.headers
            .insert("Authorization".into(), "Bearer tok".into());
        m.add_webhook(c).await.unwrap();
        // A create can't carry a masked URL: there is nothing to restore.
        let masked = m.summary("u").await.unwrap().config;
        let mut copy = masked.clone();
        copy.id = "copy".into();
        assert!(matches!(
            m.add_webhook(copy).await,
            Err(horcrux_common::Error::Validation(_))
        ));

        // What the UI sends back: the masked view, edited, with a stray id.
        let mut edit = m.summary("u").await.unwrap().config;
        edit.id = "other".into();
        edit.name = "renamed".into();
        edit.headers.insert("X-New".into(), "fresh".into());
        edit.headers.insert("X-Ghost".into(), SECRET_MASK.into());
        m.update_webhook("u", edit).await.unwrap();

        let stored = m.get_webhook("u").await.unwrap();
        assert_eq!(stored.id, "u");
        assert_eq!(stored.url, "https://hooks.example.org/x?token=t");
        assert_eq!(stored.name, "renamed");
        assert_eq!(stored.secret.as_deref(), Some("hmac-key"));
        assert_eq!(stored.headers["Authorization"], "Bearer tok");
        assert_eq!(stored.headers["X-New"], "fresh");
        assert!(!stored.headers.contains_key("X-Ghost"));
        assert!(m.get_webhook("other").await.is_none());
        assert_eq!(m.list_webhooks().await.len(), 1);

        // A new value replaces the secret; None removes it.
        let mut edit = m.summary("u").await.unwrap().config;
        edit.secret = Some("new-key".into());
        m.update_webhook("u", edit).await.unwrap();
        assert_eq!(
            m.get_webhook("u").await.unwrap().secret.as_deref(),
            Some("new-key")
        );
        let mut edit = m.summary("u").await.unwrap().config;
        edit.secret = None;
        m.update_webhook("u", edit).await.unwrap();
        assert_eq!(m.get_webhook("u").await.unwrap().secret, None);

        // Invalid edits and unknown ids are refused, the stored one is kept.
        let mut edit = m.summary("u").await.unwrap().config;
        edit.url = "ftp://x".into();
        assert!(matches!(
            m.update_webhook("u", edit.clone()).await,
            Err(horcrux_common::Error::Validation(_))
        ));
        assert_eq!(
            m.get_webhook("u").await.unwrap().url,
            "https://hooks.example.org/x?token=t"
        );
        assert!(matches!(
            m.update_webhook("nope", edit).await,
            Err(horcrux_common::Error::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn create_refuses_taken_ids_and_invalid_configs() {
        let m = WebhookManager::new();
        m.add_webhook(hook("d", "https://hooks.example.org/x"))
            .await
            .unwrap();
        let mut again = hook("d", "https://hooks.example.org/other");
        again.name = "second".into();
        assert!(matches!(
            m.add_webhook(again).await,
            Err(horcrux_common::Error::AlreadyExists(_))
        ));
        assert_eq!(m.get_webhook("d").await.unwrap().name, "hook d");
        assert!(matches!(
            m.add_webhook(hook("e", "file:///etc/passwd")).await,
            Err(horcrux_common::Error::Validation(_))
        ));
        assert!(m.get_webhook("e").await.is_none());
        // A blank id gets a fresh one.
        let made = m
            .add_webhook(hook(" ", "https://hooks.example.org/x"))
            .await
            .unwrap();
        assert_eq!(made.id.len(), 36);
        assert!(m.get_webhook(&made.id).await.is_some());
    }

    #[tokio::test]
    async fn remove_reports_unknown_ids_and_drops_counters() {
        let m = WebhookManager::new();
        assert!(matches!(
            m.remove_webhook("ghost").await,
            Err(horcrux_common::Error::NotFound(_))
        ));
        m.add_webhook(hook("r", &dead_url().await)).await.unwrap();
        m.test_webhook("r").await.unwrap();
        m.remove_webhook("r").await.unwrap();
        assert!(m.get_webhook("r").await.is_none());
        // Same id again starts from zero.
        m.add_webhook(hook("r", "https://hooks.example.org/x"))
            .await
            .unwrap();
        assert_eq!(m.summary("r").await.unwrap().stats, WebhookStats::default());
    }

    #[tokio::test]
    async fn test_sends_to_that_webhook_only_and_counts_it() {
        let m = WebhookManager::new();
        assert!(matches!(
            m.test_webhook("ghost").await,
            Err(horcrux_common::Error::NotFound(_))
        ));
        let mut c = hook("t", &dead_url().await);
        c.enabled = false; // testing a disabled hook is allowed
        c.retry_count = 5; // and never retried
        m.add_webhook(c).await.unwrap();
        m.add_webhook(hook("other", &dead_url().await))
            .await
            .unwrap();

        let d = m.test_webhook("t").await.unwrap();
        assert_eq!(d.webhook_id, "t");
        assert_eq!(d.event_type, "test");
        assert!(!d.success);
        assert!(d.error.is_some() && d.status_code.is_none());
        assert_eq!(d.retry_count, 0);

        let s = m.summary("t").await.unwrap().stats;
        assert_eq!((s.success_count, s.failure_count), (0, 1));
        assert!(s.last_triggered.is_some());
        assert_eq!(
            m.summary("other").await.unwrap().stats,
            WebhookStats::default()
        );
        assert_eq!(m.get_deliveries(Some("t"), 10).await.len(), 1);
        assert!(m.get_deliveries(Some("other"), 10).await.is_empty());
    }

    #[tokio::test]
    async fn delivery_makes_one_attempt_plus_retry_count_retries() {
        let m = WebhookManager::new();
        let mut c = hook("x", &dead_url().await);
        c.retry_count = 0;
        let event = WebhookEvent {
            event_type: "vm.created".into(),
            timestamp: chrono::Utc::now(),
            source: "horcrux".into(),
            data: serde_json::Value::Null,
        };
        assert!(m.deliver_webhook(&c, event.clone()).await.is_err());
        c.retry_count = 1;
        assert!(m.deliver_webhook(&c, event).await.is_err());
        let d = m.get_deliveries(Some("x"), 10).await;
        // most recent first
        assert_eq!(d.iter().map(|d| d.retry_count).collect::<Vec<_>>(), [1, 0]);
        assert_eq!(WebhookManager::backoff(1).as_secs(), 1);
        assert_eq!(WebhookManager::backoff(2).as_secs(), 2);
        assert_eq!(WebhookManager::backoff(4).as_secs(), 8);
        assert_eq!(WebhookManager::backoff(7).as_secs(), MAX_BACKOFF_SECS);
        assert_eq!(WebhookManager::backoff(40).as_secs(), MAX_BACKOFF_SECS);
    }

    #[tokio::test]
    async fn the_delivery_log_is_bounded_but_counters_are_not() {
        let m = WebhookManager::new();
        let c = hook("b", "https://hooks.example.org/x");
        let event = WebhookEvent {
            event_type: "vm.created".into(),
            timestamp: chrono::Utc::now(),
            source: "horcrux".into(),
            data: serde_json::Value::Null,
        };
        for i in 0..(MAX_DELIVERY_RECORDS + 5) {
            let r: Result<u16> = Ok(200 + (i % 2) as u16);
            m.record_delivery(WebhookManager::delivery_record(&c, &event, &r, 0))
                .await;
        }
        assert_eq!(m.deliveries.read().await.len(), MAX_DELIVERY_RECORDS);
        let s = m.stats.read().await["b"].clone();
        assert_eq!(s.success_count, (MAX_DELIVERY_RECORDS + 5) as u64);
        assert_eq!(s.failure_count, 0);
        // the newest records are the ones kept
        let newest = m.get_deliveries(None, 1).await;
        assert_eq!(
            newest[0].status_code,
            Some(200 + ((MAX_DELIVERY_RECORDS + 4) % 2) as u16)
        );
    }

    #[test]
    fn test_webhook_event_type_serialization() {
        let event_type = WebhookEventType::VmCreated;
        assert_eq!(event_type.as_str(), "vm.created");

        let event_type = WebhookEventType::AlertTriggered;
        assert_eq!(event_type.as_str(), "alert.triggered");
    }

    #[test]
    fn test_hmac_signature_is_real_hmac_sha256() {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        let manager = WebhookManager::new();
        let secret = "test-secret";
        let payload = r#"{"event_type":"vm.created"}"#;

        let signature = manager.generate_hmac_signature(secret, payload);

        // Must use the documented `sha256=<hex>` convention.
        assert!(signature.starts_with("sha256="));
        let hex_digest = signature.strip_prefix("sha256=").unwrap();
        assert_eq!(hex_digest.len(), 64, "SHA-256 hex digest must be 64 chars");

        // Recompute independently and confirm it matches exactly (i.e. this
        // is a real, reproducible HMAC-SHA256 and not a weak/non-crypto hash).
        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(payload.as_bytes());
        let expected = hex::encode(mac.finalize().into_bytes());
        assert_eq!(hex_digest, expected);

        // Different secrets must produce different signatures.
        let other = manager.generate_hmac_signature("different-secret", payload);
        assert_ne!(signature, other);
    }
}
