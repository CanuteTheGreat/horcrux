//! Kubernetes ConfigMap / Secret edit routes (the web UI's edit dialogs on
//! Kubernetes > Config). Listing/creating/deleting live with the other k8s
//! routes in main.rs.

use crate::error::ApiError;
use crate::kubernetes::types::{
    ConfigMapInfo, CreateSecretRequest, SecretInfo, UpdateConfigMapRequest,
};
use crate::AppState;
use axum::{
    extract::{Path, State},
    routing::post,
    Json, Router,
};
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/configmaps/:name",
            post(update_configmap),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/secrets/:name",
            post(update_secret),
        )
}

async fn update_configmap(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, name)): Path<(String, String, String)>,
    Json(req): Json<UpdateConfigMapRequest>,
) -> Result<Json<ConfigMapInfo>, ApiError> {
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    Ok(Json(
        crate::kubernetes::config_storage::configmaps::replace_configmap(
            &client, &namespace, &name, &req,
        )
        .await?,
    ))
}

async fn update_secret(
    State(state): State<Arc<AppState>>,
    Path((cluster_id, namespace, name)): Path<(String, String, String)>,
    Json(mut req): Json<CreateSecretRequest>,
) -> Result<Json<SecretInfo>, ApiError> {
    req.namespace = namespace.clone();
    let client = state.kubernetes_manager.get_client(&cluster_id).await?;
    Ok(Json(
        crate::kubernetes::config_storage::secrets::update_secret(&client, &namespace, &name, &req)
            .await?,
    ))
}
