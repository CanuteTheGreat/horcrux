///! Webhook notification system
///! Sends HTTP POST requests to configured endpoints when events occur

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

/// Webhook configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    pub id: String,
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub events: Vec<WebhookEventType>,
    pub secret: Option<String>,
    pub retry_count: u32,
    pub timeout_seconds: u64,
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

/// Webhook manager
pub struct WebhookManager {
    webhooks: Arc<RwLock<HashMap<String, WebhookConfig>>>,
    deliveries: Arc<RwLock<Vec<WebhookDelivery>>>,
    client: reqwest::Client,
}

impl WebhookManager {
    pub fn new() -> Self {
        Self {
            webhooks: Arc::new(RwLock::new(HashMap::new())),
            deliveries: Arc::new(RwLock::new(Vec::new())),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap(),
        }
    }

    /// Add a webhook configuration
    pub async fn add_webhook(&self, mut config: WebhookConfig) -> Result<WebhookConfig> {
        if config.id.is_empty() {
            config.id = uuid::Uuid::new_v4().to_string();
        }

        let mut webhooks = self.webhooks.write().await;
        webhooks.insert(config.id.clone(), config.clone());

        tracing::info!("Added webhook: {} ({})", config.name, config.id);
        Ok(config)
    }

    /// Remove a webhook
    pub async fn remove_webhook(&self, id: &str) -> Result<()> {
        let mut webhooks = self.webhooks.write().await;
        webhooks.remove(id);

        tracing::info!("Removed webhook: {}", id);
        Ok(())
    }

    /// List all webhooks
    pub async fn list_webhooks(&self) -> Vec<WebhookConfig> {
        let webhooks = self.webhooks.read().await;
        webhooks.values().cloned().collect()
    }

    /// Get a specific webhook
    pub async fn get_webhook(&self, id: &str) -> Option<WebhookConfig> {
        let webhooks = self.webhooks.read().await;
        webhooks.get(id).cloned()
    }

    /// Update a webhook
    pub async fn update_webhook(&self, id: &str, config: WebhookConfig) -> Result<()> {
        let mut webhooks = self.webhooks.write().await;

        if !webhooks.contains_key(id) {
            return Err(horcrux_common::Error::System(format!("Webhook {} not found", id)));
        }

        webhooks.insert(id.to_string(), config);
        tracing::info!("Updated webhook: {}", id);
        Ok(())
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
            client: self.client.clone(),
        }
    }

    /// Deliver webhook to endpoint
    async fn deliver_webhook(&self, webhook: &WebhookConfig, event: WebhookEvent) -> Result<()> {
        let mut retry_count = 0;
        let max_retries = webhook.retry_count;

        loop {
            let result = self
                .send_webhook_request(webhook, &event)
                .await;

            match result {
                Ok(status_code) => {
                    // Record successful delivery
                    let delivery = WebhookDelivery {
                        webhook_id: webhook.id.clone(),
                        event_type: event.event_type.clone(),
                        timestamp: chrono::Utc::now(),
                        success: true,
                        status_code: Some(status_code),
                        error: None,
                        retry_count,
                    };

                    let mut deliveries = self.deliveries.write().await;
                    deliveries.push(delivery);

                    tracing::info!(
                        "Webhook delivered successfully: {} -> {} ({})",
                        event.event_type,
                        webhook.name,
                        status_code
                    );

                    return Ok(());
                }
                Err(e) => {
                    retry_count += 1;

                    if retry_count >= max_retries {
                        // Record failed delivery
                        let delivery = WebhookDelivery {
                            webhook_id: webhook.id.clone(),
                            event_type: event.event_type.clone(),
                            timestamp: chrono::Utc::now(),
                            success: false,
                            status_code: None,
                            error: Some(e.to_string()),
                            retry_count,
                        };

                        let mut deliveries = self.deliveries.write().await;
                        deliveries.push(delivery);

                        tracing::error!(
                            "Webhook delivery failed after {} retries: {} -> {} ({})",
                            retry_count,
                            event.event_type,
                            webhook.name,
                            e
                        );

                        return Err(e);
                    }

                    // Exponential backoff: 1s, 2s, 4s, etc.
                    let delay = std::time::Duration::from_secs(2_u64.pow(retry_count - 1));
                    tokio::time::sleep(delay).await;
                }
            }
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

    /// Generate HMAC signature for webhook payload
    fn generate_hmac_signature(&self, secret: &str, payload: &str) -> String {
        use base64::Engine;
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        // Simple hash-based signature (in production, use proper HMAC-SHA256)
        let mut hasher = DefaultHasher::new();
        secret.hash(&mut hasher);
        payload.hash(&mut hasher);
        let hash = hasher.finish();

        let digest = format!("{:x}", hash);
        base64::engine::general_purpose::STANDARD.encode(digest)
    }

    /// Get recent webhook deliveries
    pub async fn get_deliveries(&self, webhook_id: Option<&str>, limit: usize) -> Vec<WebhookDelivery> {
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

    #[test]
    fn test_webhook_event_type_serialization() {
        let event_type = WebhookEventType::VmCreated;
        assert_eq!(event_type.as_str(), "vm.created");

        let event_type = WebhookEventType::AlertTriggered;
        assert_eq!(event_type.as_str(), "alert.triggered");
    }
}
