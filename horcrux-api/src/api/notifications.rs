//! Monitoring > Notifications channel routes.

use crate::error::{ApiError, ErrorResponse};
use crate::notification_channels::{NotificationChannel, NotificationChannelStore};
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
