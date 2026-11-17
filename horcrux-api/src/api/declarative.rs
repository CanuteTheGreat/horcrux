//! Declarative configuration over HTTP (see `crate::declarative` and
//! `docs/DECLARATIVE-GITOPS.md`):
//!
//! One-shot
//! - `GET  /api/declarative/export`   the live state as a spec
//! - `POST /api/declarative/plan`     `{spec, prune?}` -> the diff, nothing changes
//! - `POST /api/declarative/apply`    `{spec, prune?, dry_run?, source?}` -> plan, then do it (recorded in the history)
//!
//! Stored desired state and drift
//! - `GET|PUT|DELETE /api/declarative/desired`, `POST .../desired/validate`, `POST .../desired/apply`
//! - `GET /api/declarative/drift[?refresh=true]`, `POST .../drift/check`, `GET .../drift/diff` (spec vs live as text)
//! - `GET|PUT /api/declarative/settings`   drift interval, history cap, alert hook
//!
//! GitOps
//! - `GET|PUT|DELETE /api/declarative/gitops/config`, `GET .../gitops/status`
//! - `POST .../gitops/sync`, `POST .../gitops/approve/:hash`, `POST .../gitops/reject/:hash`
//! - `POST .../gitops/webhook` (public: authenticated by the HMAC signature)
//!
//! History
//! - `GET /api/declarative/history`, `GET .../history/:id`, `POST .../history/:id/rollback`
//!
//! `spec` is YAML/JSON text, or a JSON object.
use crate::declarative::gitops::{self, GitConfig, SyncError, Trigger};
use crate::declarative::service::{self, Origin, SvcError};
use crate::declarative::{self, drift, exec, store, Spec};
use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::AppState;
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::HeaderMap,
    routing::{get, post},
    Extension, Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub fn routes(state: &Arc<AppState>) -> Router<Arc<AppState>> {
    drift::spawn(state.clone());
    Router::new()
        .route("/api/declarative/export", get(export))
        .route("/api/declarative/plan", post(plan))
        .route("/api/declarative/apply", post(apply))
        .route(
            "/api/declarative/desired",
            get(get_desired).put(put_desired).delete(delete_desired),
        )
        .route("/api/declarative/desired/validate", post(validate_desired))
        .route("/api/declarative/desired/apply", post(apply_desired))
        .route("/api/declarative/drift", get(get_drift))
        .route("/api/declarative/drift/check", post(check_drift))
        .route("/api/declarative/drift/diff", get(drift_diff))
        .route(
            "/api/declarative/settings",
            get(get_settings).put(put_settings),
        )
        .route(
            "/api/declarative/gitops/config",
            get(get_git_config)
                .put(put_git_config)
                .delete(delete_git_config),
        )
        .route("/api/declarative/gitops/status", get(git_status))
        .route("/api/declarative/gitops/sync", post(git_sync))
        .route("/api/declarative/gitops/approve/:hash", post(git_approve))
        .route("/api/declarative/gitops/reject/:hash", post(git_reject))
        .route("/api/declarative/history", get(history))
        .route("/api/declarative/history/:id", get(history_one))
        .route("/api/declarative/history/:id/rollback", post(rollback))
        // who is calling, for the kinds' safety checks (kinds::ctx)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            declarative::kinds::ctx::caller_layer,
        ))
}

/// Routes that carry their own authentication (outside the login layer).
pub fn public_routes() -> Router<Arc<AppState>> {
    Router::new().route("/api/declarative/gitops/webhook", post(webhook))
}

#[derive(Deserialize)]
struct Req {
    spec: Value,
    #[serde(default)]
    prune: Option<bool>,
    #[serde(default)]
    dry_run: bool,
    /// `cli` or `ui` (recorded in the history); anything else counts as `api`.
    #[serde(default)]
    source: Option<String>,
}

fn spec_text(v: &Value) -> Result<String, ApiError> {
    match v {
        Value::String(s) => Ok(s.clone()),
        other => serde_yaml::to_string(other).map_err(|e| ApiError::ValidationError(e.to_string())),
    }
}

fn read_spec(v: &Value) -> Result<Spec, ApiError> {
    declarative::parse(&spec_text(v)?).map_err(ApiError::ValidationError)
}

fn actor(u: &Option<Extension<AuthUser>>) -> String {
    u.as_ref()
        .map(|Extension(u)| u.username.clone())
        .unwrap_or_else(|| "unknown".into())
}

fn svc_err(e: SvcError) -> ApiError {
    match e {
        SvcError::Invalid(m) => ApiError::ValidationError(m),
        SvcError::NotFound(m) => ApiError::NotFound(m),
        SvcError::Busy(m) => ApiError::Conflict(m),
        SvcError::Internal(m) => ApiError::Internal(m),
    }
}

fn sync_err(e: SyncError) -> ApiError {
    // a missing binary is the node's gap, not a failure of the request
    fn missing(m: &str, reason: &'static str, what: &str, hint: &str) -> Option<ApiError> {
        m.contains(what).then(|| ApiError::NotAvailable {
            reason,
            message: what.into(),
            hint: hint.into(),
        })
    }
    if let Some(a) = missing(
        e.message(),
        "git-missing",
        "the git binary is not installed on this node",
        "Install dev-vcs/git (emerge dev-vcs/git), then sync again.",
    )
    .or_else(|| {
        missing(
            e.message(),
            "gpg-missing",
            "gpg is not installed on this node",
            "Install app-crypt/gnupg, or use SSH signatures (signing.format: ssh).",
        )
    }) {
        return a;
    }
    match e {
        SyncError::Fetch(m) => ApiError::ServiceUnavailable(m),
        SyncError::Refused(m) => ApiError::ValidationError(m),
        SyncError::Busy(m) | SyncError::Conflict(m) => ApiError::Conflict(m),
        SyncError::NotFound(m) => ApiError::NotFound(m),
        SyncError::Other(m) => ApiError::Internal(m),
    }
}

fn json_of<T: serde::Serialize>(v: T) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        serde_json::to_value(v).map_err(|e| ApiError::Internal(e.to_string()))?,
    ))
}

// ------------------------------------------------------------- one-shot

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
    user: Option<Extension<AuthUser>>,
    Json(r): Json<Req>,
) -> Result<Json<Value>, ApiError> {
    let text = spec_text(&r.spec)?;
    let origin = Origin::new(&actor(&user), service::client_source(r.source.as_deref()));
    let rep = service::apply_text(&state, &text, r.prune, r.dry_run, origin)
        .await
        .map_err(svc_err)?;
    json_of(rep)
}

// -------------------------------------------------------------- desired

async fn get_desired(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    match store::desired(&state).await {
        Some(d) => json_of(json!({
            "configured": true, "spec": d.spec, "hash": d.hash,
            "updated_at": d.updated_at, "updated_by": d.updated_by,
            "source": d.source, "commit": d.commit,
        })),
        None => Ok(Json(json!({"configured": false}))),
    }
}

#[derive(Deserialize)]
struct DesiredReq {
    spec: Value,
    #[serde(default)]
    source: Option<String>,
}

async fn put_desired(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Json(r): Json<DesiredReq>,
) -> Result<Json<Value>, ApiError> {
    let text = spec_text(&r.spec)?;
    declarative::parse(&text).map_err(ApiError::ValidationError)?;
    let d = store::put_desired(
        &state,
        &text,
        &actor(&user),
        service::client_source(r.source.as_deref()),
        None,
    )
    .await
    .map_err(ApiError::Internal)?;
    // fresh drift view right away, so the caller sees what the spec implies
    let rep = drift::check(&state).await;
    Ok(Json(json!({
        "hash": d.hash, "updated_at": d.updated_at,
        "in_sync": rep.in_sync, "drift_count": rep.count,
    })))
}

async fn delete_desired(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    let was = store::clear_desired(&state)
        .await
        .map_err(ApiError::Internal)?;
    drift::check(&state).await;
    Ok(Json(json!({"cleared": was})))
}

/// Parse and plan a candidate spec without storing it. Problems come back as
/// `{ok: false, error}` (HTTP 200) so the editor can show them inline.
async fn validate_desired(
    State(state): State<Arc<AppState>>,
    Json(r): Json<Req>,
) -> Result<Json<Value>, ApiError> {
    let text = spec_text(&r.spec)?;
    match declarative::parse(&text) {
        Err(e) => Ok(Json(json!({"ok": false, "error": e}))),
        Ok(spec) => {
            let prune = r.prune.unwrap_or(spec.prune);
            let p = declarative::plan(&spec, &exec::gather(&state).await, prune);
            let items = drift::items_of(&p);
            Ok(Json(json!({
                "ok": p.errors.is_empty(), "in_sync": p.in_sync(),
                "errors": p.errors, "items": items, "plan": p,
            })))
        }
    }
}

#[derive(Deserialize, Default)]
struct DryRun {
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    source: Option<String>,
}

/// Apply the stored desired spec (the Drift tab's "Apply now").
async fn apply_desired(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    body: Option<Json<DryRun>>,
) -> Result<Json<Value>, ApiError> {
    let r = body.map(|Json(b)| b).unwrap_or_default();
    let d = store::desired(&state)
        .await
        .ok_or_else(|| ApiError::NotFound("no desired spec is stored".into()))?;
    let origin = Origin::new(&actor(&user), service::client_source(r.source.as_deref()));
    let rep = service::apply_text(&state, &d.spec, None, r.dry_run, origin)
        .await
        .map_err(svc_err)?;
    json_of(rep)
}

// ---------------------------------------------------------------- drift

#[derive(Deserialize, Default)]
struct DriftQuery {
    #[serde(default)]
    refresh: bool,
}

async fn get_drift(
    State(state): State<Arc<AppState>>,
    Query(q): Query<DriftQuery>,
) -> Result<Json<Value>, ApiError> {
    let r = if q.refresh {
        drift::check(&state).await
    } else {
        drift::cached_or_check(&state).await
    };
    json_of(r)
}

async fn check_drift(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    json_of(drift::check(&state).await)
}

/// Desired spec and the matching slice of the live state as normalised YAML,
/// for a text diff.
async fn drift_diff(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        drift::diff_texts(&state)
            .await
            .map_err(ApiError::ValidationError)?,
    ))
}

async fn get_settings(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    json_of(store::settings(&state).await)
}

async fn put_settings(
    State(state): State<Arc<AppState>>,
    Json(s): Json<store::Settings>,
) -> Result<Json<Value>, ApiError> {
    store::put_settings(&state, &s)
        .await
        .map_err(ApiError::ValidationError)?;
    json_of(s)
}

// --------------------------------------------------------------- gitops

async fn get_git_config(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    match store::gitops_config(&state).await {
        Some(c) => json_of(json!({"configured": true, "config": c})),
        None => Ok(Json(json!({"configured": false}))),
    }
}

async fn put_git_config(
    State(state): State<Arc<AppState>>,
    Json(c): Json<GitConfig>,
) -> Result<Json<Value>, ApiError> {
    c.validate().map_err(ApiError::ValidationError)?;
    store::put_gitops_config(&state, &c)
        .await
        .map_err(ApiError::Internal)?;
    json_of(json!({"configured": true, "config": c}))
}

async fn delete_git_config(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    for p in store::pending(&state).await {
        store::delete_pending(&state, &p.hash).await;
    }
    let _ = store::put_gitops_state(&state, &Default::default()).await;
    store::delete_gitops_config(&state)
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(json!({"configured": false})))
}

async fn git_status(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    Ok(Json(gitops::status(&state).await))
}

async fn git_sync(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    let o = gitops::sync(&state, Trigger::Manual)
        .await
        .map_err(sync_err)?;
    json_of(o)
}

async fn git_approve(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Path(hash): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let rep = gitops::approve(&state, &hash, &actor(&user))
        .await
        .map_err(sync_err)?;
    json_of(rep)
}

#[derive(Deserialize, Default)]
struct RejectReq {
    #[serde(default)]
    reason: Option<String>,
}

async fn git_reject(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Path(hash): Path<String>,
    body: Option<Json<RejectReq>>,
) -> Result<Json<Value>, ApiError> {
    let r = body.map(|Json(b)| b).unwrap_or_default();
    gitops::reject(&state, &hash, &actor(&user), r.reason.as_deref())
        .await
        .map_err(sync_err)?;
    Ok(Json(json!({"rejected": hash})))
}

/// Push webhook (GitHub/Forgejo/Gitea style, HMAC-SHA256 over the raw body).
/// It only wakes the sync loop: the fetch, signature check and apply rules are
/// the same as for a scheduled sync.
async fn webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    match gitops::verify_webhook(&state, &headers, &body).await {
        Ok(()) => {
            drift::rt().kick_git.notify_one();
            Ok(Json(json!({"accepted": true})))
        }
        Err((200, why)) => Ok(Json(json!({"accepted": false, "reason": why}))),
        Err((401, _)) => Err(ApiError::AuthenticationFailed),
        Err((404, why)) => Err(ApiError::NotFound(why)),
        Err((_, why)) => Err(ApiError::ServiceUnavailable(why)),
    }
}

// -------------------------------------------------------------- history

#[derive(Deserialize, Default)]
struct HistoryQuery {
    #[serde(default)]
    limit: Option<usize>,
}

async fn history(
    State(state): State<Arc<AppState>>,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<Value>, ApiError> {
    let all = store::history(&state).await;
    let total = all.len();
    let items: Vec<_> = all
        .iter()
        .take(q.limit.unwrap_or(100).clamp(1, 5000))
        .map(|e| e.summarize())
        .collect();
    json_of(json!({"total": total, "items": items}))
}

async fn history_one(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let e = store::history_entry(&state, &id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("history entry {id} not found")))?;
    json_of(e)
}

#[derive(Deserialize, Default)]
struct RollbackReq {
    #[serde(default)]
    dry_run: bool,
    /// Restore the spec of this entry instead of the one before `:id`.
    #[serde(default)]
    to: Option<String>,
    /// Also store the restored spec as the desired spec.
    #[serde(default)]
    set_desired: bool,
}

async fn rollback(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Path(id): Path<String>,
    body: Option<Json<RollbackReq>>,
) -> Result<Json<Value>, ApiError> {
    let r = body.map(|Json(b)| b).unwrap_or_default();
    let out = service::rollback(
        &state,
        &id,
        r.to.as_deref(),
        r.dry_run,
        r.set_desired,
        &actor(&user),
    )
    .await
    .map_err(svc_err)?;
    Ok(Json(out))
}
