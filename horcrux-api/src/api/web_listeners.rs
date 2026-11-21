//! Web UI / API listener settings (docs/WEB-LISTENERS.md).
//!
//! * `GET /api/system/web`: the stored settings, the ones the running
//!   process listens on, the built-in defaults, and whether a restart is due.
//! * `PUT /api/system/web`: validate and store new settings
//!   (`<data_dir>/web.json`). Listeners are bound at start, so the change
//!   applies at the next `horcrux-api` restart (`restart_required`).
//! * `DELETE /api/system/web`: drop the stored override (config.toml /
//!   environment / the standard defaults apply again at the next restart).
//!
//! Sys.Audit / Sys.Modify on `/system` (RBAC maps every `/api/system/*`).
use crate::error::ApiError;
use crate::web_settings::{self, WebSettings};
use crate::AppState;
use axum::{
    extract::State,
    http::{header, HeaderMap},
    routing::get,
    Extension, Json, Router,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route(
        "/api/system/web",
        get(get_settings).put(put_settings).delete(reset_settings),
    )
}

#[derive(Serialize)]
struct View {
    /// What `web.json` / config.toml / the environment resolved to at start.
    running: WebSettings,
    /// What the next start will use.
    configured: WebSettings,
    /// Built-in defaults (standard ports).
    defaults: WebSettings,
    /// True when `web.json` exists (the API/UI/declarative layer is in charge).
    stored: bool,
    restart_required: bool,
    /// Human-readable notes (for example "the port you are connected on is not kept").
    notes: Vec<String>,
}

fn view(state: &AppState, notes: Vec<String>) -> View {
    let running = WebSettings::from(&state.config.server);
    let data_dir = &state.config.paths.data_dir;
    let stored_settings = web_settings::load_stored(data_dir);
    let configured = {
        let fresh = crate::config::HorcruxConfig::load();
        WebSettings::from(&fresh.server)
    };
    View {
        restart_required: configured != running,
        stored: stored_settings.is_some(),
        running,
        configured,
        defaults: WebSettings::default(),
        notes,
    }
}

async fn get_settings(State(state): State<Arc<AppState>>) -> Json<View> {
    Json(view(&state, Vec::new()))
}

fn audit(state: &AppState, user: Option<String>, action: &str) {
    let logger = state.audit_logger.clone();
    let ev = crate::audit::AuditEvent {
        timestamp: chrono::Utc::now(),
        event_type: crate::audit::AuditEventType::ConfigChanged,
        severity: crate::audit::AuditSeverity::Info,
        user,
        source_ip: None,
        resource: Some("web-listeners".into()),
        action: action.into(),
        result: crate::audit::AuditResult::Success,
        details: None,
        session_id: None,
    };
    tokio::spawn(async move {
        logger.log(ev).await;
    });
}

/// The port the request arrived on (from its Host header), if any.
fn request_port(headers: &HeaderMap) -> Option<u16> {
    let host = headers.get(header::HOST)?.to_str().ok()?;
    match host.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || h.starts_with('[') => p.parse().ok(),
        _ => None,
    }
}

async fn put_settings(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<crate::middleware::auth::AuthUser>>,
    headers: HeaderMap,
    Json(new): Json<WebSettings>,
) -> Result<Json<Value>, ApiError> {
    new.validate().map_err(ApiError::ValidationError)?;
    let mut notes = Vec::new();
    let reached_on = request_port(&headers).unwrap_or(if state.config.server.tls_enabled {
        443
    } else {
        80
    });
    if !new.all_ports().contains(&reached_on) {
        notes.push(format!(
            "you are connected on port {reached_on}, which the new settings no longer listen on: reconnect on {}",
            new.port
        ));
    }
    web_settings::save(&state.config.paths.data_dir, &new).map_err(ApiError::ValidationError)?;
    audit(
        &state,
        user.map(|Extension(u)| u.username),
        "web listener settings changed",
    );
    let v = view(&state, notes);
    Ok(Json(json!(v)))
}

async fn reset_settings(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<crate::middleware::auth::AuthUser>>,
) -> Result<Json<Value>, ApiError> {
    web_settings::reset(&state.config.paths.data_dir).map_err(ApiError::Internal)?;
    audit(
        &state,
        user.map(|Extension(u)| u.username),
        "web listener settings reset",
    );
    Ok(Json(json!(view(&state, Vec::new()))))
}
