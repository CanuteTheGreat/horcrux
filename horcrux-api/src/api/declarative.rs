//! Declarative configuration over HTTP (see `crate::declarative`):
//!
//! - `GET  /api/declarative/export`  the live state as a spec (YAML text in
//!   `spec`, and the parsed form in `parsed`)
//! - `POST /api/declarative/plan`    `{spec, prune?}` -> the diff, nothing changes
//! - `POST /api/declarative/apply`   `{spec, prune?, dry_run?}` -> plan, then do it
//!
//! `spec` is YAML or JSON text, or a JSON object.
use crate::declarative::{self, exec, Spec};
use crate::error::ApiError;
use crate::AppState;
use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/declarative/export", get(export))
        .route("/api/declarative/plan", post(plan))
        .route("/api/declarative/apply", post(apply))
}

#[derive(Deserialize)]
struct Req {
    spec: Value,
    #[serde(default)]
    prune: Option<bool>,
    #[serde(default)]
    dry_run: bool,
}

fn read_spec(v: &Value) -> Result<Spec, ApiError> {
    let text = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    declarative::parse(&text).map_err(ApiError::ValidationError)
}

async fn export(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    let spec = declarative::export(&exec::gather(&state).await);
    let yaml = serde_yaml::to_string(&spec).map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(json!({"spec": yaml, "parsed": spec})))
}

async fn plan(
    State(state): State<Arc<AppState>>,
    Json(r): Json<Req>,
) -> Result<Json<Value>, ApiError> {
    let spec = read_spec(&r.spec)?;
    let prune = r.prune.unwrap_or(spec.prune);
    let p = declarative::plan(&spec, &exec::gather(&state).await, prune);
    Ok(Json(json!({"in_sync": p.in_sync(), "plan": p})))
}

async fn apply(
    State(state): State<Arc<AppState>>,
    Json(r): Json<Req>,
) -> Result<Json<Value>, ApiError> {
    let spec = read_spec(&r.spec)?;
    let prune = r.prune.unwrap_or(spec.prune);
    let rep = exec::apply(&state, &spec, prune, r.dry_run)
        .await
        .map_err(ApiError::Conflict)?;
    Ok(Json(
        serde_json::to_value(rep).map_err(|e| ApiError::Internal(e.to_string()))?,
    ))
}
