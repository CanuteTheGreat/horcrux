//! Monitoring > Metrics Explorer routes.

use crate::error::ApiError;
use crate::metrics_explorer::{MetricDefinition, MetricQuery, MetricResult, MetricsStore};
use crate::AppState;
use axum::{
    routing::{get, post},
    Extension, Json, Router,
};
use std::sync::Arc;

pub fn routes(store: Arc<MetricsStore>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/metrics/definitions", get(definitions))
        .route("/api/metrics/query", post(query))
        .layer(Extension(store))
}

async fn definitions(
    Extension(store): Extension<Arc<MetricsStore>>,
) -> Json<Vec<MetricDefinition>> {
    Json(store.definitions().await)
}

async fn query(
    Extension(store): Extension<Arc<MetricsStore>>,
    Json(q): Json<MetricQuery>,
) -> Result<Json<MetricResult>, ApiError> {
    store
        .query(&q)
        .await
        .map(Json)
        .map_err(ApiError::BadRequest)
}
