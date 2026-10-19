//! NAS > S3 Gateway routes (USE=s3-gateway).
//!
//! Horcrux runs a MinIO-compatible server (`minio`, or `silo`, the community
//! fork that still ships binaries) as a supervised process and administers it
//! through the MinIO client (`mcli`/`mc`), see `nas::services::s3`.
//!
//! - `GET  /api/nas/s3/status`                       process + usage
//! - `GET  /api/nas/s3/config`, `PUT` (restarts a running gateway)
//! - `POST /api/nas/s3/start`, `POST /api/nas/s3/stop`
//! - `GET|POST /api/nas/s3/buckets`, `GET|DELETE /api/nas/s3/buckets/:bucket`
//! - `GET|PUT|DELETE /api/nas/s3/buckets/:bucket/objects` (`?key=` for one
//!   object; PUT takes the raw body), `GET .../objects/download?key=`
//! - `GET|POST /api/nas/s3/users`, `DELETE /api/nas/s3/users/:name`
//! - `GET|POST /api/nas/s3/access-keys` (`?user_id=` filters),
//!   `PUT|DELETE /api/nas/s3/access-keys/:key`,
//!   `POST /api/nas/s3/access-keys/:key/rotate`
//!
//! The configuration (including the generated root password, which is never
//! returned) is stored in `config_objects` (kind `s3_gateway`); an enabled
//! gateway is started again when the API starts. Some client versions mask
//! access keys in listings (`AKIATE...0001`), so Horcrux records the keys it
//! creates (kind `s3_access_key`, never the secret) and matches them against
//! the server's list; a key made outside Horcrux is manageable when its full
//! id is shown and listed read-only when only a masked one is.
use crate::db::config_store::ConfigStore;
use crate::error::ApiError;
use crate::nas::services::s3::{
    mask_access_key, random_alnum, validate_bucket_name, validate_user_name, CreateBucketOptions,
    S3Bucket, S3GatewayConfig, S3GatewayManager, S3GatewayStatus, S3Object,
};
use crate::AppState;
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Path, Query},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Extension, Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

const KIND_CONFIG: &str = "s3_gateway";
const KIND_KEY: &str = "s3_access_key";

/// Gateway state shared by the handlers: the persisted configuration,
/// loaded on first use. The mutex also serialises start/stop/config changes.
pub struct Gateway {
    store: ConfigStore,
    data_dir: std::path::PathBuf,
    config: Mutex<Option<S3GatewayConfig>>,
}

impl Gateway {
    async fn config(&self) -> Result<S3GatewayConfig, ApiError> {
        let mut guard = self.config.lock().await;
        if let Some(c) = guard.as_ref() {
            return Ok(c.clone());
        }
        let stored = self
            .store
            .load_values::<S3GatewayConfig>(KIND_CONFIG)
            .await?
            .into_iter()
            .next();
        let cfg = match stored {
            Some(c) => c,
            None => {
                // First use: data + run dirs under the node's data dir and a
                // fresh random root password, persisted before anything runs.
                let c = S3GatewayConfig {
                    data_dirs: vec![self.data_dir.join("nas/s3").to_string_lossy().into_owned()],
                    run_dir: self
                        .data_dir
                        .join("nas/s3-gateway")
                        .to_string_lossy()
                        .into_owned(),
                    ..Default::default()
                };
                self.store.put(KIND_CONFIG, "default", &c).await?;
                c
            }
        };
        *guard = Some(cfg.clone());
        Ok(cfg)
    }

    async fn save(&self, cfg: &S3GatewayConfig) -> Result<(), ApiError> {
        self.store.put(KIND_CONFIG, "default", cfg).await?;
        *self.config.lock().await = Some(cfg.clone());
        Ok(())
    }

    async fn manager(&self) -> Result<S3GatewayManager, ApiError> {
        Ok(S3GatewayManager::with_config(self.config().await?))
    }

    /// A manager whose gateway is up, or 409 telling the user to start it.
    async fn running(&self) -> Result<S3GatewayManager, ApiError> {
        let m = self.manager().await?;
        if !m.is_running().await {
            return Err(ApiError::Conflict(
                "The S3 gateway is not running - start it first".to_string(),
            ));
        }
        Ok(m)
    }
}

/// Client/server failures are mostly the user's request (bucket exists,
/// not empty, bad name...): pass the client's message through as a 400
/// rather than a bare 500.
fn gw_err(e: horcrux_common::Error) -> ApiError {
    match e {
        horcrux_common::Error::Internal(msg) | horcrux_common::Error::System(msg) => {
            ApiError::BadRequest(msg)
        }
        other => other.into(),
    }
}

pub fn routes(state: &Arc<AppState>) -> Router<Arc<AppState>> {
    let gw = Arc::new(Gateway {
        store: ConfigStore::new(&state.database),
        data_dir: state.config.paths.data_dir.clone(),
        config: Mutex::new(None),
    });

    // Start an enabled gateway again after an API restart / host reboot.
    let boot = gw.clone();
    tokio::spawn(async move {
        match boot.manager().await {
            Ok(m) if m.get_config().enabled && !m.is_running().await => match m.start().await {
                Ok(()) => tracing::info!("S3 gateway started (enabled)"),
                Err(e) => tracing::warn!("Could not start the enabled S3 gateway: {e}"),
            },
            Ok(_) => {}
            Err(e) => tracing::warn!("Could not load the S3 gateway configuration: {e:?}"),
        }
    });

    Router::new()
        .route("/api/nas/s3/status", get(status))
        .route("/api/nas/s3/config", get(get_config).put(update_config))
        .route("/api/nas/s3/start", post(start))
        .route("/api/nas/s3/stop", post(stop))
        .route("/api/nas/s3/buckets", get(list_buckets).post(create_bucket))
        .route(
            "/api/nas/s3/buckets/:bucket",
            get(get_bucket).delete(delete_bucket),
        )
        .route(
            "/api/nas/s3/buckets/:bucket/objects",
            get(list_objects)
                .put(upload_object)
                .delete(delete_object)
                .layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/api/nas/s3/buckets/:bucket/objects/download",
            get(download_object),
        )
        .route("/api/nas/s3/users", get(list_users).post(create_user))
        .route(
            "/api/nas/s3/users/:name",
            axum::routing::delete(delete_user),
        )
        .route(
            "/api/nas/s3/access-keys",
            get(list_access_keys).post(create_access_key),
        )
        .route(
            "/api/nas/s3/access-keys/:key",
            put(update_access_key).delete(delete_access_key),
        )
        .route(
            "/api/nas/s3/access-keys/:key/rotate",
            post(rotate_access_key),
        )
        .layer(Extension(gw))
}

type Gw = Extension<Arc<Gateway>>;

// ---------------------------------------------------------------- gateway

async fn status(Extension(gw): Gw) -> Result<Json<S3GatewayStatus>, ApiError> {
    Ok(Json(gw.manager().await?.get_status().await?))
}

/// Editable gateway settings (`GET`/`PUT /api/nas/s3/config`). The root
/// password is generated and never shown.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GatewaySettings {
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub console_port: Option<u16>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub data_dirs: Option<Vec<String>>,
    #[serde(default)]
    pub browser: Option<bool>,
    #[serde(default)]
    pub server_bin: Option<String>,
    #[serde(default)]
    pub client_bin: Option<String>,
    /// read-only
    #[serde(default)]
    pub root_user: Option<String>,
}

fn settings_of(c: &S3GatewayConfig) -> GatewaySettings {
    GatewaySettings {
        address: Some(c.address.clone()),
        port: Some(c.port),
        console_port: Some(c.console_port),
        region: Some(c.region.clone()),
        data_dirs: Some(c.data_dirs.clone()),
        browser: Some(c.browser),
        server_bin: Some(c.server_bin.clone().unwrap_or_default()),
        client_bin: Some(c.client_bin.clone().unwrap_or_default()),
        root_user: Some(c.root_user.clone()),
    }
}

async fn get_config(Extension(gw): Gw) -> Result<Json<GatewaySettings>, ApiError> {
    Ok(Json(settings_of(&gw.config().await?)))
}

async fn update_config(
    Extension(gw): Gw,
    Json(req): Json<GatewaySettings>,
) -> Result<Json<GatewaySettings>, ApiError> {
    let old = gw.config().await?;
    let mut cfg = old.clone();
    if let Some(a) = req.address {
        let a = a.trim().to_string();
        if a.parse::<std::net::IpAddr>().is_err() {
            return Err(ApiError::ValidationError(format!(
                "Listen address '{}' is not an IP address (0.0.0.0 = all)",
                a
            )));
        }
        cfg.address = a;
    }
    if let Some(p) = req.port {
        cfg.port = p;
    }
    if let Some(p) = req.console_port {
        cfg.console_port = p;
    }
    if cfg.port == 0 || cfg.console_port == 0 || cfg.port == cfg.console_port {
        return Err(ApiError::ValidationError(
            "API and console ports must be set and different".to_string(),
        ));
    }
    if let Some(r) = req.region.filter(|r| !r.trim().is_empty()) {
        cfg.region = r.trim().to_string();
    }
    if let Some(dirs) = req.data_dirs {
        let dirs: Vec<String> = dirs
            .into_iter()
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty())
            .collect();
        if dirs.is_empty() || dirs.iter().any(|d| !d.starts_with('/')) {
            return Err(ApiError::ValidationError(
                "Data directories must be absolute paths".to_string(),
            ));
        }
        cfg.data_dirs = dirs;
    }
    if let Some(b) = req.browser {
        cfg.browser = b;
    }
    if let Some(b) = req.server_bin {
        cfg.server_bin = Some(b.trim().to_string()).filter(|b| !b.is_empty());
    }
    if let Some(b) = req.client_bin {
        cfg.client_bin = Some(b.trim().to_string()).filter(|b| !b.is_empty());
    }

    // A running gateway is restarted with the new settings.
    let old_mgr = S3GatewayManager::with_config(old);
    let was_running = old_mgr.is_running().await;
    if was_running {
        old_mgr.stop().await.map_err(gw_err)?;
    }
    gw.save(&cfg).await?;
    if was_running {
        S3GatewayManager::with_config(cfg.clone())
            .start()
            .await
            .map_err(gw_err)?;
    }
    Ok(Json(settings_of(&cfg)))
}

async fn start(Extension(gw): Gw) -> Result<Json<S3GatewayStatus>, ApiError> {
    let mut cfg = gw.config().await?;
    let m = S3GatewayManager::with_config(cfg.clone());
    m.start().await.map_err(|e| match e {
        horcrux_common::Error::Conflict(m) => ApiError::Conflict(m),
        e => ApiError::ServiceUnavailable(e.to_string()),
    })?;
    cfg.enabled = true;
    gw.save(&cfg).await?;
    Ok(Json(S3GatewayManager::with_config(cfg).get_status().await?))
}

async fn stop(Extension(gw): Gw) -> Result<Json<S3GatewayStatus>, ApiError> {
    let mut cfg = gw.config().await?;
    S3GatewayManager::with_config(cfg.clone())
        .stop()
        .await
        .map_err(gw_err)?;
    cfg.enabled = false;
    gw.save(&cfg).await?;
    Ok(Json(S3GatewayManager::with_config(cfg).get_status().await?))
}

// ---------------------------------------------------------------- buckets

async fn list_buckets(Extension(gw): Gw) -> Result<Json<Vec<S3Bucket>>, ApiError> {
    let m = gw.manager().await?;
    if !m.is_running().await {
        return Ok(Json(vec![]));
    }
    Ok(Json(m.list_buckets().await.map_err(gw_err)?))
}

#[derive(Debug, Deserialize)]
struct CreateBucket {
    name: String,
    #[serde(default)]
    versioning: bool,
    #[serde(default)]
    object_locking: bool,
}

async fn create_bucket(
    Extension(gw): Gw,
    Json(req): Json<CreateBucket>,
) -> Result<(StatusCode, Json<S3Bucket>), ApiError> {
    let name = req.name.trim();
    validate_bucket_name(name)?;
    let m = gw.running().await?;
    let opts = CreateBucketOptions {
        versioning: req.versioning,
        object_locking: req.object_locking,
        ..Default::default()
    };
    let created = m.create_bucket(name, Some(opts)).await.map_err(gw_err)?;
    if req.versioning && !m.get_bucket_versioning(name).await.unwrap_or(false) {
        return Err(ApiError::BadRequest(format!(
            "Bucket {} was created but versioning could not be enabled",
            name
        )));
    }
    Ok((StatusCode::CREATED, Json(created)))
}

async fn get_bucket(
    Extension(gw): Gw,
    Path(bucket): Path<String>,
) -> Result<Json<S3Bucket>, ApiError> {
    let m = gw.running().await?;
    Ok(Json(m.get_bucket(&bucket).await?))
}

#[derive(Debug, Deserialize)]
struct ForceQuery {
    #[serde(default)]
    force: bool,
}

async fn delete_bucket(
    Extension(gw): Gw,
    Path(bucket): Path<String>,
    Query(q): Query<ForceQuery>,
) -> Result<StatusCode, ApiError> {
    let m = gw.running().await?;
    m.delete_bucket(&bucket, q.force).await.map_err(gw_err)?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------- objects

#[derive(Debug, Deserialize)]
struct ObjectQuery {
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    prefix: Option<String>,
}

async fn list_objects(
    Extension(gw): Gw,
    Path(bucket): Path<String>,
    Query(q): Query<ObjectQuery>,
) -> Result<Json<Vec<S3Object>>, ApiError> {
    validate_bucket_name(&bucket)?;
    let m = gw.running().await?;
    let prefix = q.prefix.as_deref().filter(|p| !p.is_empty());
    Ok(Json(
        m.list_objects(&bucket, prefix, true)
            .await
            .map_err(gw_err)?,
    ))
}

fn object_key(q: &ObjectQuery) -> Result<String, ApiError> {
    q.key
        .clone()
        .filter(|k| !k.is_empty())
        .ok_or_else(|| ApiError::BadRequest("Missing ?key= (object key)".to_string()))
}

#[derive(Debug, Serialize)]
struct Uploaded {
    key: String,
    size: u64,
}

async fn upload_object(
    Extension(gw): Gw,
    Path(bucket): Path<String>,
    Query(q): Query<ObjectQuery>,
    body: Body,
) -> Result<(StatusCode, Json<Uploaded>), ApiError> {
    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;
    validate_bucket_name(&bucket)?;
    let key = object_key(&q)?;
    let m = gw.running().await?;
    // Stream the request body into the client's stdin through a pipe.
    let (mut tx, mut rx) = tokio::io::duplex(256 * 1024);
    let mut stream = body.into_data_stream();
    let feeder = tokio::spawn(async move {
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| e.to_string())?;
            tx.write_all(&chunk).await.map_err(|e| e.to_string())?;
        }
        tx.shutdown().await.map_err(|e| e.to_string())
    });
    let size = m.put_object(&bucket, &key, &mut rx).await.map_err(gw_err)?;
    match feeder.await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(ApiError::BadRequest(format!("Upload interrupted: {}", e))),
        Err(e) => return Err(ApiError::Internal(e.to_string())),
    }
    Ok((StatusCode::CREATED, Json(Uploaded { key, size })))
}

async fn download_object(
    Extension(gw): Gw,
    Path(bucket): Path<String>,
    Query(q): Query<ObjectQuery>,
) -> Result<Response, ApiError> {
    validate_bucket_name(&bucket)?;
    let key = object_key(&q)?;
    let m = gw.running().await?;
    let data = m.get_object(&bucket, &key).await.map_err(|e| match e {
        horcrux_common::Error::NotFound(m) => ApiError::NotFound(m),
        e => gw_err(e),
    })?;
    let file = key.rsplit('/').next().unwrap_or("object").replace('"', "");
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", file),
            ),
        ],
        data,
    )
        .into_response())
}

async fn delete_object(
    Extension(gw): Gw,
    Path(bucket): Path<String>,
    Query(q): Query<ObjectQuery>,
) -> Result<StatusCode, ApiError> {
    validate_bucket_name(&bucket)?;
    let key = object_key(&q)?;
    let m = gw.running().await?;
    m.delete_object(&bucket, &key).await.map_err(gw_err)?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------- users

/// An S3 user (`GET /api/nas/s3/users`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3UserInfo {
    pub username: String,
    pub enabled: bool,
    pub policies: Vec<String>,
    /// Only in the create response: the user's secret key (S3 password).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_key: Option<String>,
}

async fn list_users(Extension(gw): Gw) -> Result<Json<Vec<S3UserInfo>>, ApiError> {
    let m = gw.manager().await?;
    if !m.is_running().await {
        return Ok(Json(vec![]));
    }
    Ok(Json(
        m.list_users()
            .await
            .map_err(gw_err)?
            .into_iter()
            .map(|u| S3UserInfo {
                username: u.username,
                enabled: u.enabled,
                policies: u.policies,
                secret_key: None,
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
struct CreateUser {
    username: String,
    /// S3 secret key; generated when empty.
    #[serde(default)]
    secret_key: Option<String>,
    /// Built-in policy: readwrite (default), readonly, writeonly, consoleAdmin.
    #[serde(default)]
    policy: Option<String>,
}

const POLICIES: &[&str] = &[
    "readwrite",
    "readonly",
    "writeonly",
    "consoleAdmin",
    "diagnostics",
];

async fn create_user(
    Extension(gw): Gw,
    Json(req): Json<CreateUser>,
) -> Result<(StatusCode, Json<S3UserInfo>), ApiError> {
    let username = req.username.trim().to_string();
    validate_user_name(&username)?;
    let policy = req.policy.unwrap_or_else(|| "readwrite".to_string());
    if !POLICIES.contains(&policy.as_str()) {
        return Err(ApiError::ValidationError(format!(
            "Unknown policy '{}' (use one of {})",
            policy,
            POLICIES.join(", ")
        )));
    }
    let secret = match req.secret_key.filter(|s| !s.is_empty()) {
        Some(s) if s.len() < 8 => {
            return Err(ApiError::ValidationError(
                "Secret key must be at least 8 characters".to_string(),
            ))
        }
        Some(s) => s,
        None => random_alnum(40),
    };
    let m = gw.running().await?;
    if m.get_config().root_user == username {
        return Err(ApiError::Conflict(format!("{} is the root user", username)));
    }
    if m.list_users()
        .await
        .map_err(gw_err)?
        .iter()
        .any(|u| u.username == username)
    {
        return Err(ApiError::Conflict(format!(
            "S3 user {} already exists",
            username
        )));
    }
    m.create_user(&username, &secret).await.map_err(gw_err)?;
    if let Err(e) = m.set_user_policy(&username, &policy).await {
        let _ = m.delete_user(&username).await;
        return Err(gw_err(e));
    }
    Ok((
        StatusCode::CREATED,
        Json(S3UserInfo {
            username,
            enabled: true,
            policies: vec![policy],
            secret_key: Some(secret),
        }),
    ))
}

async fn delete_user(Extension(gw): Gw, Path(name): Path<String>) -> Result<StatusCode, ApiError> {
    let m = gw.running().await?;
    m.delete_user(&name).await.map_err(gw_err)?;
    // the server drops the user's access keys with it
    for (id, rec) in gw.store.load::<KeyRecord>(KIND_KEY).await? {
        if rec.user_id == name {
            gw.store.delete(KIND_KEY, &id).await?;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------- access keys

/// What Horcrux remembers about a key it created (never the secret).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct KeyRecord {
    access_key: String,
    user_id: String,
    #[serde(default)]
    description: Option<String>,
    created_at: i64,
}

/// An access key as the page lists it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessKeyInfo {
    /// Full id for keys created by Horcrux; masked (`AKIATE...0001`) for
    /// keys created elsewhere.
    pub access_key: String,
    pub user_id: String,
    pub description: Option<String>,
    pub enabled: bool,
    /// Unix time (0 = unknown, created outside Horcrux)
    pub created_at: i64,
    /// Created by Horcrux: can be rotated, disabled and deleted here
    pub managed: bool,
    /// Only in create / rotate responses
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct KeyQuery {
    #[serde(default)]
    user_id: Option<String>,
}

async fn list_access_keys(
    Extension(gw): Gw,
    Query(q): Query<KeyQuery>,
) -> Result<Json<Vec<AccessKeyInfo>>, ApiError> {
    let m = gw.manager().await?;
    if !m.is_running().await {
        return Ok(Json(vec![]));
    }
    let mut on_server = m.list_server_access_keys().await.map_err(gw_err)?;
    let mut out = Vec::new();
    for (id, rec) in gw.store.load::<KeyRecord>(KIND_KEY).await? {
        let masked = mask_access_key(&rec.access_key);
        // the client prints keys in full or masked depending on its version
        match on_server
            .iter()
            .position(|(u, k)| (*k == masked || *k == rec.access_key) && *u == rec.user_id)
        {
            Some(i) => {
                on_server.remove(i);
                let enabled = match m.access_key_info(&rec.access_key).await {
                    Ok(info) => info.enabled,
                    Err(_) => true,
                };
                out.push(AccessKeyInfo {
                    access_key: rec.access_key,
                    user_id: rec.user_id,
                    description: rec.description,
                    enabled,
                    created_at: rec.created_at,
                    managed: true,
                    secret_key: None,
                });
            }
            // removed outside Horcrux (or with its user): forget it
            None => {
                gw.store.delete(KIND_KEY, &id).await?;
            }
        }
    }
    // Keys created outside Horcrux: manageable when the client shows the
    // full id, listed read-only when it only shows a masked one.
    for (user, key) in on_server {
        let full = !key.contains("...");
        let info = if full {
            m.access_key_info(&key).await.ok()
        } else {
            None
        };
        out.push(AccessKeyInfo {
            access_key: key,
            user_id: user,
            description: info.as_ref().and_then(|i| i.description.clone()),
            enabled: info.as_ref().map(|i| i.enabled).unwrap_or(true),
            created_at: 0,
            managed: full,
            secret_key: None,
        });
    }
    if let Some(u) = q.user_id.filter(|u| !u.is_empty()) {
        out.retain(|k| k.user_id == u);
    }
    Ok(Json(out))
}

#[derive(Debug, Deserialize)]
struct CreateKey {
    /// Owner: the root user or an S3 user. Default: the root user.
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

async fn create_access_key(
    Extension(gw): Gw,
    Json(req): Json<CreateKey>,
) -> Result<(StatusCode, Json<AccessKeyInfo>), ApiError> {
    let m = gw.running().await?;
    let user = req
        .user_id
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| m.get_config().root_user.clone());
    if user != m.get_config().root_user
        && !m
            .list_users()
            .await
            .map_err(gw_err)?
            .iter()
            .any(|u| u.username == user)
    {
        return Err(ApiError::NotFound(format!("S3 user {} not found", user)));
    }
    let description = req
        .description
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty());
    if description.as_ref().map(|d| d.len() > 256).unwrap_or(false) {
        return Err(ApiError::ValidationError(
            "Description is limited to 256 characters".to_string(),
        ));
    }
    let key = m
        .create_access_key(&user, description.as_deref())
        .await
        .map_err(gw_err)?;
    let rec = KeyRecord {
        access_key: key.access_key.clone(),
        user_id: user.clone(),
        description: description.clone(),
        created_at: key.created_at,
    };
    gw.store.put(KIND_KEY, &rec.access_key, &rec).await?;
    Ok((
        StatusCode::CREATED,
        Json(AccessKeyInfo {
            access_key: key.access_key,
            user_id: user,
            description,
            enabled: true,
            created_at: key.created_at,
            managed: true,
            secret_key: Some(key.secret_key),
        }),
    ))
}

/// A key this page may change: one Horcrux created, or one made elsewhere
/// whose full id the server shows.
async fn managed_key(gw: &Gateway, m: &S3GatewayManager, key: &str) -> Result<KeyRecord, ApiError> {
    if let Some((_, r)) = gw
        .store
        .load::<KeyRecord>(KIND_KEY)
        .await?
        .into_iter()
        .find(|(id, _)| id == key)
    {
        return Ok(r);
    }
    if !key.contains("...") {
        if let Ok(info) = m.access_key_info(key).await {
            return Ok(KeyRecord {
                access_key: key.to_string(),
                user_id: info.user_id,
                description: info.description,
                created_at: 0,
            });
        }
    }
    Err(ApiError::NotFound(format!("Access key {} not found", key)))
}

#[derive(Debug, Deserialize)]
struct UpdateKey {
    enabled: bool,
}

async fn update_access_key(
    Extension(gw): Gw,
    Path(key): Path<String>,
    Json(req): Json<UpdateKey>,
) -> Result<Json<AccessKeyInfo>, ApiError> {
    let m = gw.running().await?;
    let rec = managed_key(&gw, &m, &key).await?;
    m.set_access_key_enabled(&key, req.enabled)
        .await
        .map_err(gw_err)?;
    let info = m.access_key_info(&key).await?;
    Ok(Json(AccessKeyInfo {
        access_key: rec.access_key,
        user_id: rec.user_id,
        description: rec.description,
        enabled: info.enabled,
        created_at: rec.created_at,
        managed: true,
        secret_key: None,
    }))
}

async fn rotate_access_key(
    Extension(gw): Gw,
    Path(key): Path<String>,
) -> Result<Json<AccessKeyInfo>, ApiError> {
    let m = gw.running().await?;
    let rec = managed_key(&gw, &m, &key).await?;
    let secret = m.rotate_access_key(&key).await.map_err(gw_err)?;
    let enabled = m
        .access_key_info(&key)
        .await
        .map(|i| i.enabled)
        .unwrap_or(true);
    Ok(Json(AccessKeyInfo {
        access_key: rec.access_key,
        user_id: rec.user_id,
        description: rec.description,
        enabled,
        created_at: rec.created_at,
        managed: true,
        secret_key: Some(secret),
    }))
}

async fn delete_access_key(
    Extension(gw): Gw,
    Path(key): Path<String>,
) -> Result<StatusCode, ApiError> {
    let m = gw.running().await?;
    managed_key(&gw, &m, &key).await?;
    if let Err(e) = m.delete_access_key(&key).await {
        // already gone on the server: just forget it
        let gone = matches!(
            m.access_key_info(&key).await,
            Err(horcrux_common::Error::NotFound(_))
        );
        if !gone {
            return Err(gw_err(e));
        }
    }
    gw.store.delete(KIND_KEY, &key).await?;
    Ok(StatusCode::NO_CONTENT)
}
