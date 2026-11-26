//! Monitoring > Notifications channel routes.

use crate::error::{ApiError, ErrorResponse};
use crate::notification_channels::{NotificationChannel, NotificationChannelStore};
use crate::notification_matchers::{Matcher, SampleEvent};
use crate::AppState;
use axum::{
    extract::Path,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Json, Router,
};
use std::sync::Arc;

pub fn routes(store: Arc<NotificationChannelStore>) -> Router<Arc<AppState>> {
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
        .route(
            "/api/notifications/matchers/:id",
            axum::routing::put(update_matcher).delete(delete_matcher),
        )
        .route("/api/notifications/matchers/test", post(test_matchers))
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

async fn list_matchers(Extension(store): Store) -> Json<Vec<Matcher>> {
    Json(store.list_matchers().await)
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

#[derive(serde::Serialize)]
struct MatcherTestResult {
    /// false: no matcher is enabled, so every enabled channel gets the event.
    routed_by_matchers: bool,
    matched_matchers: Vec<String>,
    channels: Vec<String>,
}

/// Evaluate a sample event against the saved matchers now; nothing is sent.
async fn test_matchers(
    Extension(store): Store,
    Json(ev): Json<SampleEvent>,
) -> Json<MatcherTestResult> {
    let (routed, matched) = store.route_ids(&ev).await;
    let enabled: Vec<String> = store
        .list()
        .await
        .into_iter()
        .filter(|c| c.enabled)
        .map(|c| c.id)
        .collect();
    let channels = match &routed {
        Some(ids) => enabled.into_iter().filter(|i| ids.contains(i)).collect(),
        None => enabled,
    };
    Json(MatcherTestResult {
        routed_by_matchers: routed.is_some(),
        matched_matchers: matched,
        channels,
    })
}
