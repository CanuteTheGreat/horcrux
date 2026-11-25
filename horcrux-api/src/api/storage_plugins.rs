//! Third-party storage plugins (Proxmox VE custom storage plugins, without
//! Perl in the API process: each plugin is a packaged executable speaking
//! versioned JSON over stdio; docs/storage-plugins.md).
//!
//! - `GET /api/storage/plugins`            installed plugins with their schema
//! - `GET /api/storage/plugins/:id`        one plugin's description (424 if missing)
//! - `GET /api/storage/plugin-settings`    plugin dir, timeouts, limits
//! - `PUT /api/storage/plugin-settings`    change them (Sys.Modify on `/`:
//!   the plugin directory decides what runs as root)
//!
//! Pools of a plugin go through the normal pool routes:
//! `POST /api/storage/pools {name, storage_type: "plugin:<id>", config}`.
use crate::error::ApiError;
use crate::storage::plugin::{self, PluginList};
use crate::AppState;
use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use horcrux_common::storage_plugins::StoragePluginSettings;
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/storage/plugins", get(list_plugins))
        .route("/api/storage/plugins/:id", get(get_plugin))
        .route(
            "/api/storage/plugin-settings",
            get(get_settings).put(put_settings),
        )
}

async fn list_plugins(State(state): State<Arc<AppState>>) -> Json<PluginList> {
    Json(state.storage_manager.plugin_host().list().await)
}

async fn get_plugin(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<plugin::sdk::PluginDescription>, ApiError> {
    state
        .storage_manager
        .plugin_host()
        .describe(&id)
        .await
        .map(Json)
        .map_err(|e| match e {
            horcrux_common::Error::InvalidConfig(m) => ApiError::BadRequest(m),
            other => ApiError::from(other),
        })
}

async fn get_settings() -> Json<StoragePluginSettings> {
    Json(plugin::settings::get())
}

async fn put_settings(
    Json(s): Json<StoragePluginSettings>,
) -> Result<Json<StoragePluginSettings>, ApiError> {
    plugin::settings::set(s)
        .map(Json)
        .map_err(ApiError::BadRequest)
}
