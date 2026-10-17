//! Certificate management API: the live HTTPS certificate, self-signed
//! regeneration, custom certificate upload, and ACME (Let's Encrypt).

use crate::error::ApiError;
use crate::tls::acme::{self, AcmeConfig};
use crate::AppState;
use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

type St = State<Arc<AppState>>;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/tls/status", get(status))
        .route("/api/tls/config", get(config))
        .route("/api/tls/certificates", get(certificates))
        .route("/api/tls/certificate/generate", post(generate))
        .route("/api/tls/certificate/upload", post(upload))
        .route("/api/tls/acme", get(get_acme).put(put_acme))
        .route("/api/tls/acme/order", post(order))
}

async fn status(State(state): St) -> Json<Value> {
    let pool = state.database.pool();
    Json(json!({
        "config": state.tls_manager.config(),
        "certificate": state.tls_manager.current_info(),
        "acme": acme::load_config(pool).await,
        "acme_last_result": acme::last_result(pool).await,
    }))
}

async fn config(State(state): St) -> Json<crate::tls::TlsConfig> {
    Json(state.tls_manager.config())
}

async fn certificates(State(state): St) -> Json<Vec<crate::tls::CertificateInfo>> {
    Json(vec![state.tls_manager.current_info()])
}

#[derive(Deserialize, Default)]
struct GenerateBody {
    #[serde(default)]
    common_name: Option<String>,
    #[serde(default)]
    organization: Option<String>,
    #[serde(default)]
    validity_days: Option<u32>,
    /// extra DNS names / IPs (the host's own names are always included)
    #[serde(default)]
    san: Vec<String>,
}

async fn generate(
    State(state): St,
    body: Option<Json<GenerateBody>>,
) -> Result<Json<crate::tls::CertificateInfo>, ApiError> {
    let b = body.map(|Json(b)| b).unwrap_or_default();
    let _ = (&b.common_name, &b.organization);
    Ok(Json(state.tls_manager.issue_local(
        &b.san,
        b.validity_days.unwrap_or(3650).clamp(1, 3650),
    )?))
}

/// The node's local CA certificate: import it once and the browser trusts
/// this node's HTTPS certificate (public: it's needed before logging in).
pub async fn ca_pem(State(state): St) -> Result<impl axum::response::IntoResponse, ApiError> {
    let pem = state.tls_manager.local_ca_pem()?;
    Ok((
        [
            (axum::http::header::CONTENT_TYPE, "application/x-pem-file"),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment; filename=\"horcrux-local-ca.pem\"",
            ),
        ],
        pem,
    ))
}

#[derive(Deserialize)]
struct UploadBody {
    /// PEM certificate chain (leaf first)
    cert_pem: String,
    /// PEM private key (PKCS#8, PKCS#1 or SEC1)
    key_pem: String,
}

async fn upload(
    State(state): St,
    Json(b): Json<UploadBody>,
) -> Result<Json<crate::tls::CertificateInfo>, ApiError> {
    Ok(Json(state.tls_manager.install(
        &b.cert_pem,
        &b.key_pem,
        "custom",
    )?))
}

async fn get_acme(State(state): St) -> Json<Value> {
    let pool = state.database.pool();
    Json(json!({
        "config": acme::load_config(pool).await,
        "last_result": acme::last_result(pool).await,
        "lets_encrypt": acme::LETS_ENCRYPT,
        "lets_encrypt_staging": acme::LETS_ENCRYPT_STAGING,
        "http_redirect_port": state.tls_manager.config().http_redirect_port,
    }))
}

async fn put_acme(State(state): St, Json(cfg): Json<AcmeConfig>) -> Result<StatusCode, ApiError> {
    acme::save_config(state.database.pool(), &cfg).await?;
    Ok(StatusCode::OK)
}

async fn order(State(state): St) -> Result<Json<crate::tls::CertificateInfo>, ApiError> {
    if state.tls_manager.config().http_redirect_port.is_none() {
        return Err(ApiError::ValidationError(
            "ACME HTTP-01 needs the plain-HTTP port: set HORCRUX_HTTP_REDIRECT_PORT=80 (or forward port 80 to it)".into(),
        ));
    }
    Ok(Json(
        acme::order_certificate(&state.tls_manager, state.database.pool())
            .await
            .map_err(|e| ApiError::ServiceUnavailable(e.to_string()))?,
    ))
}
