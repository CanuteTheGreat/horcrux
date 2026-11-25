//! Monitoring > Notifications channel routes.

use crate::error::{ApiError, ErrorResponse};
use crate::notification_channels::{NotificationChannel, NotificationChannelStore};
use crate::AppState;
use axum::{
    extract::Path,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Extension, Json, Router,
};
use horcrux_common::notify::{Event, Matcher};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

static STORE: std::sync::OnceLock<Arc<NotificationChannelStore>> = std::sync::OnceLock::new();

/// The node's channel and matcher store (set when the routes are built).
pub fn store() -> Option<Arc<NotificationChannelStore>> {
    STORE.get().cloned()
}

pub fn routes(store: Arc<NotificationChannelStore>) -> Router<Arc<AppState>> {
    let _ = STORE.set(store.clone());
    Router::new()
        .route(
            "/api/notifications/channels",
            get(list_channels).post(create_channel),
        )
        .route(
            "/api/notifications/channels/:id",
            get(get_channel).put(update_channel).delete(delete_channel),
        )
        .route("/api/notifications/channels/:id/test", post(test_channel))
        .route(
            "/api/notifications/matchers",
            get(list_matchers).post(create_matcher),
        )
        .route("/api/notifications/matchers/test", post(test_matchers))
        .route("/api/notifications/matchers/order", put(order_matchers))
        .route(
            "/api/notifications/matchers/:id",
            get(get_matcher).put(update_matcher).delete(delete_matcher),
        )
        .route("/api/notifications/events", post(send_event))
        .layer(Extension(store))
}

type Store = Extension<Arc<NotificationChannelStore>>;

async fn list_channels(Extension(store): Store) -> Json<Vec<NotificationChannel>> {
    Json(store.list().await)
}

async fn get_channel(
    Extension(store): Store,
    Path(id): Path<String>,
) -> Result<Json<NotificationChannel>, ApiError> {
    Ok(Json(store.get(&id).await?))
}

async fn create_channel(
    Extension(store): Store,
    Json(channel): Json<NotificationChannel>,
) -> Result<(StatusCode, Json<NotificationChannel>), ApiError> {
    Ok((StatusCode::CREATED, Json(store.create(channel).await?)))
}

async fn update_channel(
    Extension(store): Store,
    Path(id): Path<String>,
    Json(channel): Json<NotificationChannel>,
) -> Result<Json<NotificationChannel>, ApiError> {
    Ok(Json(store.update(&id, channel).await?))
}

async fn delete_channel(
    Extension(store): Store,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    store.delete(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Send a real test message. 204 when the target accepted it; 502 with the
/// target's error (SMTP reply, HTTP status/body) when it did not.
async fn test_channel(
    Extension(store): Store,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    match store.test(&id).await? {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => Ok((
            StatusCode::BAD_GATEWAY,
            Json(ErrorResponse::new(
                502,
                "NOTIFICATION_DELIVERY_FAILED",
                format!("test notification was not delivered: {}", e),
            )),
        )
            .into_response()),
    }
}

// ------------------------------------------------------------- matchers

async fn list_matchers(Extension(store): Store) -> Json<Vec<Matcher>> {
    Json(store.list_matchers().await)
}

async fn get_matcher(
    Extension(store): Store,
    Path(id): Path<String>,
) -> Result<Json<Matcher>, ApiError> {
    Ok(Json(store.get_matcher(&id).await?))
}

async fn create_matcher(
    Extension(store): Store,
    Json(m): Json<Matcher>,
) -> Result<(StatusCode, Json<Matcher>), ApiError> {
    Ok((StatusCode::CREATED, Json(store.create_matcher(m).await?)))
}

async fn update_matcher(
    Extension(store): Store,
    Path(id): Path<String>,
    Json(m): Json<Matcher>,
) -> Result<Json<Matcher>, ApiError> {
    Ok(Json(store.update_matcher(&id, m).await?))
}

async fn delete_matcher(
    Extension(store): Store,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    store.delete_matcher(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct OrderBody {
    ids: Vec<String>,
}

async fn order_matchers(
    Extension(store): Store,
    Json(b): Json<OrderBody>,
) -> Result<StatusCode, ApiError> {
    store.reorder_matchers(&b.ids).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct TestBody {
    event: Event,
    /// Judge calendar conditions at this instant instead of the event's.
    #[serde(default)]
    at: Option<chrono::DateTime<chrono::Utc>>,
    /// Only these saved matchers (default: all).
    #[serde(default)]
    matcher_ids: Vec<String>,
    /// Evaluate this unsaved matcher instead of the saved ones.
    #[serde(default)]
    matcher: Option<Matcher>,
}

/// Dry run: which matchers and channels a sample event hits, with the
/// reason per node. Nothing is sent.
async fn test_matchers(
    Extension(store): Store,
    Json(b): Json<TestBody>,
) -> Result<Json<horcrux_common::notify::TestResult>, ApiError> {
    if let Some(d) = &b.matcher {
        let ids: Vec<String> = store
            .channel_names()
            .await
            .into_iter()
            .map(|c| c.0)
            .collect();
        horcrux_monitoring::notification_matchers::validate(d, &ids)
            .map_err(ApiError::BadRequest)?;
    }
    Ok(Json(
        store
            .test_event(&b.event, b.at, &b.matcher_ids, b.matcher)
            .await,
    ))
}

#[derive(Serialize)]
struct SendResult {
    delivered: usize,
    failed: usize,
    channels: Vec<String>,
}

/// Send a synthetic event through the real routing (what "Send test event"
/// in the UI does).
async fn send_event(
    Extension(store): Store,
    Json(ev): Json<Event>,
) -> Result<Json<SendResult>, ApiError> {
    let channels = store.targets_for(&ev).await;
    let names = channels.iter().map(|c| c.name.clone()).collect();
    let (delivered, failed) = store.dispatch_event(&ev).await;
    Ok(Json(SendResult {
        delivered,
        failed,
        channels: names,
    }))
}
