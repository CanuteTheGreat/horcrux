//! API middleware for authentication, rate limiting, CORS and RBAC
pub mod auth;
pub mod cors;
pub mod rate_limit;
pub mod rbac;

/// What the authentication and RBAC middleware need from the server state
/// (implemented by `horcrux_api::AppState`).
pub trait AuthState: Send + Sync + 'static {
    fn database(&self) -> &std::sync::Arc<crate::db::Database>;
}
