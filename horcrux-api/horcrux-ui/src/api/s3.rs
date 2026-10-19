//! NAS > S3 Gateway API client (mirrors horcrux-api `api::s3_gateway` and
//! `nas::services::s3`; checked against the live API by tests/api_contract.rs).
use super::{delete_json, fetch_json, post_json, put_json, ApiError};
use serde::{Deserialize, Serialize};

/// `GET /api/nas/s3/status`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct S3GatewayStatus {
    pub running: bool,
    pub enabled: bool,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub version: Option<String>,
    /// listen address:port
    pub address: String,
    pub console_address: String,
    pub port: u16,
    pub console_port: u16,
    pub scheme: String,
    pub bucket_count: u32,
    pub object_count: u64,
    pub storage_used_bytes: u64,
    pub storage_available_bytes: u64,
    pub region: String,
    pub tls_enabled: bool,
    #[serde(default)]
    pub uptime_secs: Option<u64>,
    #[serde(default)]
    pub server_binary: Option<String>,
    #[serde(default)]
    pub client_binary: Option<String>,
    /// what must be installed before the gateway can start
    #[serde(default)]
    pub missing: Vec<String>,
    pub root_user: String,
    #[serde(default)]
    pub data_dirs: Vec<String>,
}

/// `GET|PUT /api/nas/s3/config`
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct S3GatewaySettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub console_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_dirs: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_bin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_bin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_user: Option<String>,
}

/// A bucket (`GET /api/nas/s3/buckets`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct S3Bucket {
    pub name: String,
    /// Unix time
    pub created_at: i64,
    #[serde(default)]
    pub size_bytes: u64,
    #[serde(default)]
    pub object_count: u64,
    #[serde(default)]
    pub versioning: bool,
    #[serde(default)]
    pub object_locking: bool,
}

/// `POST /api/nas/s3/buckets`
#[derive(Debug, Clone, Default, Serialize)]
pub struct CreateS3Bucket {
    pub name: String,
    pub versioning: bool,
}

/// An object (`GET /api/nas/s3/buckets/:bucket/objects`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct S3Object {
    pub key: String,
    pub size: u64,
    /// Unix time
    pub last_modified: i64,
    #[serde(default)]
    pub etag: String,
}

/// An S3 user (`GET /api/nas/s3/users`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct S3User {
    pub username: String,
    pub enabled: bool,
    #[serde(default)]
    pub policies: Vec<String>,
    /// only in the create response
    #[serde(default)]
    pub secret_key: Option<String>,
}

/// `POST /api/nas/s3/users`
#[derive(Debug, Clone, Default, Serialize)]
pub struct CreateS3User {
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_key: Option<String>,
    pub policy: String,
}

/// An access key (`GET /api/nas/s3/access-keys`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct S3AccessKey {
    /// full id (managed) or masked (`AKIATE...0001`, created elsewhere)
    pub access_key: String,
    pub user_id: String,
    #[serde(default)]
    pub description: Option<String>,
    pub enabled: bool,
    /// Unix time, 0 = unknown
    pub created_at: i64,
    /// created by Horcrux: rotate/disable/delete work
    pub managed: bool,
    /// only in create / rotate responses
    #[serde(default)]
    pub secret_key: Option<String>,
}

/// `POST /api/nas/s3/access-keys`
#[derive(Debug, Clone, Default, Serialize)]
pub struct CreateS3AccessKey {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

const BASE: &str = "/nas/s3";

/// Percent-encode a query value / path segment.
pub fn enc(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

pub async fn status() -> Result<S3GatewayStatus, ApiError> {
    fetch_json(&format!("{BASE}/status")).await
}
pub async fn settings() -> Result<S3GatewaySettings, ApiError> {
    fetch_json(&format!("{BASE}/config")).await
}
pub async fn save_settings(s: &S3GatewaySettings) -> Result<S3GatewaySettings, ApiError> {
    put_json(&format!("{BASE}/config"), s).await
}
pub async fn start() -> Result<S3GatewayStatus, ApiError> {
    post_json(&format!("{BASE}/start"), &serde_json::json!({})).await
}
pub async fn stop() -> Result<S3GatewayStatus, ApiError> {
    post_json(&format!("{BASE}/stop"), &serde_json::json!({})).await
}
pub async fn buckets() -> Result<Vec<S3Bucket>, ApiError> {
    fetch_json(&format!("{BASE}/buckets")).await
}
pub async fn create_bucket(req: &CreateS3Bucket) -> Result<S3Bucket, ApiError> {
    post_json(&format!("{BASE}/buckets"), req).await
}
pub async fn delete_bucket(name: &str, force: bool) -> Result<(), ApiError> {
    delete_json(&format!("{BASE}/buckets/{}?force={}", enc(name), force)).await
}
pub async fn objects(bucket: &str) -> Result<Vec<S3Object>, ApiError> {
    fetch_json(&format!("{BASE}/buckets/{}/objects", enc(bucket))).await
}
pub async fn delete_object(bucket: &str, key: &str) -> Result<(), ApiError> {
    delete_json(&format!(
        "{BASE}/buckets/{}/objects?key={}",
        enc(bucket),
        enc(key)
    ))
    .await
}
/// URL that downloads an object (needs the auth header: fetched as a blob).
pub fn download_url(bucket: &str, key: &str) -> String {
    format!(
        "/api{BASE}/buckets/{}/objects/download?key={}",
        enc(bucket),
        enc(key)
    )
}
/// Fetch an object (with the auth header) and hand it to the browser as a
/// file download.
pub async fn download_object(bucket: &str, key: &str) -> Result<(), ApiError> {
    use wasm_bindgen::JsCast;
    let resp = super::authed::get(&download_url(bucket, key))
        .send()
        .await
        .map_err(|e| ApiError {
            message: e.to_string(),
        })?;
    if !resp.ok() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiError {
            message: super::error_text(status, &text),
        });
    }
    let bytes = resp.binary().await.map_err(|e| ApiError {
        message: e.to_string(),
    })?;
    let err = |e: wasm_bindgen::JsValue| ApiError {
        message: format!("{:?}", e),
    };
    let parts = js_sys::Array::new();
    parts.push(&js_sys::Uint8Array::from(bytes.as_slice()));
    let blob = web_sys::Blob::new_with_u8_array_sequence(&parts).map_err(err)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(err)?;
    let doc = web_sys::window()
        .and_then(|w| w.document())
        .ok_or_else(|| ApiError {
            message: "no document".into(),
        })?;
    let a: web_sys::HtmlAnchorElement =
        doc.create_element("a")
            .map_err(err)?
            .dyn_into()
            .map_err(|_| ApiError {
                message: "anchor".into(),
            })?;
    a.set_href(&url);
    a.set_download(key.rsplit('/').next().unwrap_or("object"));
    a.click();
    let _ = web_sys::Url::revoke_object_url(&url);
    Ok(())
}

/// Upload a browser `File`/`Blob` as an object.
pub async fn upload_object(bucket: &str, key: &str, body: &web_sys::Blob) -> Result<(), ApiError> {
    let url = format!(
        "/api{BASE}/buckets/{}/objects?key={}",
        enc(bucket),
        enc(key)
    );
    let resp = super::authed::put(&url)
        .header("Content-Type", "application/octet-stream")
        .body(wasm_bindgen::JsValue::from(body.clone()))
        .send()
        .await
        .map_err(|e| ApiError {
            message: e.to_string(),
        })?;
    if resp.ok() {
        Ok(())
    } else {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Err(ApiError {
            message: super::error_text(status, &text),
        })
    }
}
pub async fn users() -> Result<Vec<S3User>, ApiError> {
    fetch_json(&format!("{BASE}/users")).await
}
pub async fn create_user(req: &CreateS3User) -> Result<S3User, ApiError> {
    post_json(&format!("{BASE}/users"), req).await
}
pub async fn delete_user(name: &str) -> Result<(), ApiError> {
    delete_json(&format!("{BASE}/users/{}", enc(name))).await
}
pub async fn access_keys() -> Result<Vec<S3AccessKey>, ApiError> {
    fetch_json(&format!("{BASE}/access-keys")).await
}
pub async fn create_access_key(req: &CreateS3AccessKey) -> Result<S3AccessKey, ApiError> {
    post_json(&format!("{BASE}/access-keys"), req).await
}
pub async fn rotate_access_key(key: &str) -> Result<S3AccessKey, ApiError> {
    post_json(
        &format!("{BASE}/access-keys/{}/rotate", enc(key)),
        &serde_json::json!({}),
    )
    .await
}
pub async fn set_access_key_enabled(key: &str, enabled: bool) -> Result<S3AccessKey, ApiError> {
    put_json(
        &format!("{BASE}/access-keys/{}", enc(key)),
        &serde_json::json!({ "enabled": enabled }),
    )
    .await
}
pub async fn delete_access_key(key: &str) -> Result<(), ApiError> {
    delete_json(&format!("{BASE}/access-keys/{}", enc(key))).await
}
