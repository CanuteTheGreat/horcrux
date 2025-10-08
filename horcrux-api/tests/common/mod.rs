//! Common test utilities and helpers

use reqwest::Client;
use std::time::Duration;
use tokio::time::sleep;

pub const API_BASE: &str = "http://localhost:8006/api";

/// Create HTTP client with default settings
pub fn create_test_client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to create HTTP client")
}

/// Wait for async operation with exponential backoff
pub async fn wait_with_backoff(initial_ms: u64, max_retries: u32) -> bool {
    let mut delay = initial_ms;
    for _ in 0..max_retries {
        sleep(Duration::from_millis(delay)).await;
        delay *= 2;
    }
    true
}

/// Retry operation until success or max attempts
pub async fn retry_until_success<F, Fut, T>(
    mut operation: F,
    max_attempts: u32,
    delay_ms: u64,
) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    for attempt in 0..max_attempts {
        if let Some(result) = operation().await {
            return Some(result);
        }
        if attempt < max_attempts - 1 {
            sleep(Duration::from_millis(delay_ms)).await;
        }
    }
    None
}

/// Test environment setup
pub struct TestEnv {
    pub client: Client,
    pub base_url: String,
    pub auth_token: Option<String>,
}

impl TestEnv {
    pub fn new() -> Self {
        TestEnv {
            client: create_test_client(),
            base_url: API_BASE.to_string(),
            auth_token: None,
        }
    }

    pub async fn login(&mut self, username: &str, password: &str) -> Result<(), String> {
        let response = self
            .client
            .post(&format!("{}/auth/login", self.base_url))
            .json(&serde_json::json!({
                "username": username,
                "password": password
            }))
            .send()
            .await
            .map_err(|e| format!("Login request failed: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("Login failed with status: {}", response.status()));
        }

        let token_data: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse token: {}", e))?;

        self.auth_token = token_data["token"].as_str().map(|s| s.to_string());
        Ok(())
    }

    pub fn authenticated_client(&self) -> Client {
        if let Some(token) = &self.auth_token {
            Client::builder()
                .timeout(Duration::from_secs(30))
                .default_headers({
                    let mut headers = reqwest::header::HeaderMap::new();
                    headers.insert(
                        reqwest::header::AUTHORIZATION,
                        format!("Bearer {}", token).parse().unwrap(),
                    );
                    headers
                })
                .build()
                .expect("Failed to create authenticated client")
        } else {
            self.client.clone()
        }
    }
}

/// Cleanup helper to remove test resources
pub async fn cleanup_test_vm(client: &Client, vm_id: &str) {
    // Try to stop first
    let _ = client
        .post(&format!("{}/vms/{}/stop", API_BASE, vm_id))
        .send()
        .await;

    sleep(Duration::from_millis(1000)).await;

    // Delete
    let _ = client
        .delete(&format!("{}/vms/{}", API_BASE, vm_id))
        .send()
        .await;
}

pub async fn cleanup_test_storage_pool(client: &Client, pool_id: &str) {
    let _ = client
        .delete(&format!("{}/storage/pools/{}", API_BASE, pool_id))
        .send()
        .await;
}

pub async fn cleanup_test_alert_rule(client: &Client, rule_name: &str) {
    let _ = client
        .delete(&format!("{}/alerts/rules/{}", API_BASE, rule_name))
        .send()
        .await;
}

pub async fn cleanup_test_firewall_rule(client: &Client, rule_name: &str) {
    let _ = client
        .delete(&format!("{}/firewall/rules/{}", API_BASE, rule_name))
        .send()
        .await;
}

/// Assert response status is successful
pub fn assert_success(response: &reqwest::Response, operation: &str) {
    assert!(
        response.status().is_success(),
        "{} failed with status: {}",
        operation,
        response.status()
    );
}

/// Generate unique test ID
pub fn generate_test_id(prefix: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    format!("{}-{}", prefix, timestamp)
}
