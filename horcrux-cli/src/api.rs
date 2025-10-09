///! API client for Horcrux server

use anyhow::Result;
use serde::de::DeserializeOwned;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct ApiClient {
    base_url: String,
    client: reqwest::Client,
    token: Arc<RwLock<Option<String>>>,
}

impl ApiClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.to_string(),
            client: reqwest::Client::new(),
            token: Arc::new(RwLock::new(None)),
        }
    }

    /// Set the authentication token
    pub async fn set_token(&self, token: String) {
        let mut t = self.token.write().await;
        *t = Some(token);
    }

    /// Get the current token
    pub async fn get_token(&self) -> Option<String> {
        let t = self.token.read().await;
        t.clone()
    }

    /// Clear the authentication token
    pub async fn clear_token(&self) {
        let mut t = self.token.write().await;
        *t = None;
    }

    /// Build request with authentication header
    async fn build_request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        let mut request = self.client.request(method, &url);

        // Add authentication header if token exists
        if let Some(token) = self.get_token().await {
            request = request.header("Authorization", format!("Bearer {}", token));
        }

        request
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self.build_request(reqwest::Method::GET, path)
            .await
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("API request failed: {} - {}", status, error_text);
        }

        let data = response.json().await?;
        Ok(data)
    }

    pub async fn post<T: DeserializeOwned, B: serde::Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let response = self.build_request(reqwest::Method::POST, path)
            .await
            .json(body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("API request failed: {} - {}", status, error_text);
        }

        let data = response.json().await?;
        Ok(data)
    }

    pub async fn post_empty<B: serde::Serialize>(&self, path: &str, body: &B) -> Result<()> {
        let response = self.build_request(reqwest::Method::POST, path)
            .await
            .json(body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("API request failed: {} - {}", status, error_text);
        }

        Ok(())
    }

    pub async fn delete(&self, path: &str) -> Result<()> {
        let response = self.build_request(reqwest::Method::DELETE, path)
            .await
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("API request failed: {} - {}", status, error_text);
        }

        Ok(())
    }
}
