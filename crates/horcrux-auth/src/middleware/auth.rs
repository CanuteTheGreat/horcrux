//! Authentication middleware
//!
//! Accepts a bearer JWT (issued at login), the `session_id` cookie, or an
//! `X-API-Key`. Every credential is tied to a live database row: a JWT names
//! its login session (`sid`), so logging out, deleting the session, or
//! disabling/deleting the user revokes it immediately instead of leaving a
//! 24-hour bearer token valid.
use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::OnceLock;

/// Authentication error response
#[derive(Serialize)]
pub struct AuthError {
    pub error: String,
    pub message: String,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let json = Json(self);
        (StatusCode::UNAUTHORIZED, json).into_response()
    }
}

/// How the caller authenticated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthMethod {
    Token,
    Cookie,
    ApiKey,
    ClusterNode,
}

/// Extracted user information from authentication
#[derive(Clone, Debug)]
pub struct AuthUser {
    pub user_id: String,
    pub username: String,
    #[allow(dead_code)]
    pub role: String,
    /// `Some("tfa_setup")` for the restricted login a user gets when a
    /// second factor is mandatory but not configured yet: only the
    /// second-factor setup endpoints accept it.
    pub scope: Option<String>,
    /// Login session backing this credential (not set for API keys).
    pub session_id: Option<String>,
    pub method: AuthMethod,
}

/// Scope of the restricted "set up your second factor first" login.
pub const SCOPE_TFA_SETUP: &str = "tfa_setup";

/// JWT claims structure
#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String, // subject (user ID)
    username: String,
    role: String,
    exp: usize, // expiration time (seconds since epoch)
    iat: usize, // issued at (seconds since epoch)
    /// login session id (absent only on cluster-node service tokens)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
}

static JWT_SECRET: OnceLock<String> = OnceLock::new();

/// Secret that signs and verifies node-to-node service tokens (`cluster-*`
/// roles) once this node has joined a cluster: the joined cluster's secret,
/// kept in `<data_dir>/cluster.secret`. Without it service tokens use the
/// node's own token secret (nodes that were given the same `JWT_SECRET`).
static CLUSTER_SECRET: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);

/// The secret this node's service tokens are signed with (and which it
/// hands to a node joining its cluster).
pub fn cluster_service_secret() -> String {
    CLUSTER_SECRET
        .read()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_else(|| get_jwt_secret().to_string())
}

/// Adopt a cluster's service secret (join): persisted so it survives restarts.
pub fn set_cluster_secret(data_dir: &std::path::Path, secret: &str) -> std::io::Result<()> {
    if secret.len() < 16 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "cluster secret too short",
        ));
    }
    std::fs::create_dir_all(data_dir)?;
    write_private(&data_dir.join("cluster.secret"), secret.as_bytes())?;
    if let Ok(mut g) = CLUSTER_SECRET.write() {
        *g = Some(secret.to_string());
    }
    Ok(())
}

/// Forget the adopted cluster secret (leave cluster).
pub fn clear_cluster_secret(data_dir: &std::path::Path) {
    let _ = std::fs::remove_file(data_dir.join("cluster.secret"));
    if let Ok(mut g) = CLUSTER_SECRET.write() {
        *g = None;
    }
}

/// Load the token signing secret: `JWT_SECRET` if set, otherwise a random
/// secret generated on first start and kept in `<data_dir>/jwt.secret`
/// (mode 0600). There is no built-in default any more - a published default
/// let anyone mint tokens. Every node of a cluster must use the same secret
/// (set `JWT_SECRET`, or copy the file) so their service tokens verify.
pub fn init_jwt_secret(data_dir: &std::path::Path) -> std::io::Result<()> {
    if JWT_SECRET.get().is_some() {
        return Ok(());
    }
    let secret = match std::env::var("JWT_SECRET") {
        Ok(s) if s.len() >= 16 => s,
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "JWT_SECRET must be at least 16 characters",
            ))
        }
        Err(_) => {
            let path = data_dir.join("jwt.secret");
            match std::fs::read_to_string(&path) {
                Ok(s) if s.trim().len() >= 32 => s.trim().to_string(),
                _ => {
                    std::fs::create_dir_all(data_dir)?;
                    let s = crate::auth::random_token(64);
                    write_private(&path, s.as_bytes())?;
                    tracing::info!("Generated token signing secret at {}", path.display());
                    s
                }
            }
        }
    };
    let _ = JWT_SECRET.set(secret);
    if let Ok(s) = std::fs::read_to_string(data_dir.join("cluster.secret")) {
        if s.trim().len() >= 16 {
            if let Ok(mut g) = CLUSTER_SECRET.write() {
                *g = Some(s.trim().to_string());
            }
        }
    }
    Ok(())
}

/// Write a file readable only by the service user.
pub fn write_private(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    opts.mode(0o600);
    let mut f = opts.open(path)?;
    f.write_all(data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn get_jwt_secret() -> &'static str {
    JWT_SECRET.get_or_init(|| {
        // Not initialized (unit tests, library use): a random per-process
        // secret is safe - tokens just don't outlive the process.
        std::env::var("JWT_SECRET").unwrap_or_else(|_| crate::auth::random_token(64))
    })
}

fn unauthorized(error: &str, message: impl Into<String>) -> AuthError {
    AuthError {
        error: error.to_string(),
        message: message.into(),
    }
}

/// Load the account behind a credential and make sure it may still log in.
async fn load_user<S: super::AuthState>(
    state: &Arc<S>,
    user_id: &str,
) -> Result<horcrux_common::auth::User, AuthError> {
    match crate::auth::store::find_user(state.database().pool(), user_id).await {
        Ok(Some(u)) if u.enabled => Ok(u),
        Ok(Some(_)) => Err(unauthorized("account_disabled", "This account is disabled")),
        _ => Err(unauthorized("invalid_session", "Session user not found")),
    }
}

/// Authentication middleware (requires AppState with database)
pub async fn auth_middleware<S: super::AuthState>(
    State(state): State<Arc<S>>,
    headers: HeaderMap,
    mut request: Request,
    next: Next,
) -> Result<Response, AuthError> {
    // Bearer token
    if let Some(token) = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
    {
        let claims = validate_jwt_token(token)
            .map_err(|e| unauthorized("invalid_token", format!("Invalid JWT token: {}", e)))?;
        if claims.role == CLUSTER_NODE_ROLE
            || claims.role == CLUSTER_MIGRATION_ROLE
            || claims.role == CLUSTER_HA_ROLE
        {
            // Node-to-node service tokens are scoped: the usage token to the
            // usage endpoint, the migration token to the migration peer API.
            if claims.role == CLUSTER_NODE_ROLE
                && !cluster_node_may_access(request.method(), request.uri().path())
            {
                return Err(unauthorized(
                    "insufficient_scope",
                    "cluster-node service tokens are only valid for GET /api/cluster/local-usage",
                ));
            }
            if claims.role == CLUSTER_MIGRATION_ROLE
                && !cluster_migration_may_access(request.uri().path())
            {
                return Err(unauthorized(
                    "insufficient_scope",
                    "cluster-migration service tokens are only valid for /api/migration/peer/",
                ));
            }
            if claims.role == CLUSTER_HA_ROLE && !cluster_ha_may_access(request.uri().path()) {
                return Err(unauthorized(
                    "insufficient_scope",
                    "cluster-ha service tokens are only valid for /api/ha/peer/",
                ));
            }
            request.extensions_mut().insert(AuthUser {
                user_id: claims.sub,
                username: claims.username,
                role: claims.role,
                scope: None,
                session_id: None,
                method: AuthMethod::ClusterNode,
            });
            return Ok(next.run(request).await);
        }
        let sid = claims
            .sid
            .clone()
            .ok_or_else(|| unauthorized("invalid_token", "token has no session"))?;
        let session = crate::db::users::get_session(state.database().pool(), &sid)
            .await
            .map_err(|_| unauthorized("invalid_session", "Session expired or logged out"))?;
        if session.user_id != claims.sub {
            return Err(unauthorized("invalid_token", "token/session mismatch"));
        }
        let user = load_user(&state, &claims.sub).await?;
        let scope = crate::auth::store::session_scope(state.database().pool(), &sid)
            .await
            .ok()
            .flatten();
        request.extensions_mut().insert(AuthUser {
            user_id: user.id,
            username: user.username,
            role: user.role,
            scope,
            session_id: Some(sid),
            method: AuthMethod::Token,
        });
        return Ok(next.run(request).await);
    }

    // Session cookie
    if let Some(session_id) = session_cookie(&headers) {
        let session = crate::db::users::get_session(state.database().pool(), &session_id)
            .await
            .map_err(|_| unauthorized("invalid_session", "Invalid or expired session"))?;
        let user = load_user(&state, &session.user_id).await?;
        let scope = crate::auth::store::session_scope(state.database().pool(), &session_id)
            .await
            .ok()
            .flatten();
        request.extensions_mut().insert(AuthUser {
            user_id: user.id,
            username: user.username,
            role: user.role,
            scope,
            session_id: Some(session_id),
            method: AuthMethod::Cookie,
        });
        return Ok(next.run(request).await);
    }

    // API key
    if let Some(key_str) = headers.get("x-api-key").and_then(|h| h.to_str().ok()) {
        let auth_user = validate_api_key(state.database(), key_str)
            .await
            .map_err(|e| unauthorized("invalid_api_key", e))?;
        request.extensions_mut().insert(auth_user);
        return Ok(next.run(request).await);
    }

    Err(unauthorized(
        "unauthorized",
        "Authentication required. Provide Bearer token, session cookie, or API key.",
    ))
}

/// The `session_id` cookie value, if any.
pub fn session_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all("cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|s| s.split(';'))
        .find_map(|c| c.trim().strip_prefix("session_id=").map(str::to_string))
        .filter(|s| !s.is_empty())
}

/// Validate JWT token with proper signature verification
fn validate_jwt_token(token: &str) -> Result<Claims, String> {
    let secret = get_jwt_secret();
    let decoding_key = DecodingKey::from_secret(secret.as_bytes());

    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;

    match decode::<Claims>(token, &decoding_key, &validation) {
        Ok(t) => Ok(t.claims),
        Err(e) => {
            // A joined node's peers sign service tokens with the cluster
            // secret; only service roles are accepted that way.
            let cluster = cluster_service_secret();
            if cluster != secret {
                let key = DecodingKey::from_secret(cluster.as_bytes());
                if let Ok(t) = decode::<Claims>(token, &key, &validation) {
                    if t.claims.role.starts_with("cluster-") {
                        return Ok(t.claims);
                    }
                }
            }
            Err(format!("JWT validation failed: {}", e))
        }
    }
}

/// Role carried by the node-to-node service token the cluster usage monitor
/// sends to its peers. It is NOT a user role: a token with this role may only
/// call [`CLUSTER_NODE_ALLOWED_PATH`] (see [`cluster_node_may_access`]).
pub const CLUSTER_NODE_ROLE: &str = "cluster-node";

/// The one route a `cluster-node` service token is accepted on.
pub const CLUSTER_NODE_ALLOWED_PATH: &str = "/api/cluster/local-usage";

/// Role of the service token a node sends to the target node of a VM
/// migration. Valid only on [`CLUSTER_MIGRATION_PATH_PREFIX`] routes (the
/// migration peer API), which in turn accept nothing else.
pub const CLUSTER_MIGRATION_ROLE: &str = "cluster-migration";

/// Routes a `cluster-migration` token is accepted on.
pub const CLUSTER_MIGRATION_PATH_PREFIX: &str = "/api/migration/peer/";

/// Role of the service token HA supervisors send each other (placement
/// sync, VM recovery). Valid only on [`CLUSTER_HA_PATH_PREFIX`] routes.
pub const CLUSTER_HA_ROLE: &str = "cluster-ha";

/// Routes a `cluster-ha` token is accepted on.
pub const CLUSTER_HA_PATH_PREFIX: &str = "/api/ha/peer/";

/// Whether a `cluster-ha` token may call `path`.
pub fn cluster_ha_may_access(path: &str) -> bool {
    path.starts_with(CLUSTER_HA_PATH_PREFIX)
}

/// Generate the short-lived service token for one HA peer request.
pub fn generate_cluster_ha_token(local_hostname: &str) -> Result<String, String> {
    let subject = format!("{}:{}", CLUSTER_HA_ROLE, local_hostname);
    sign(
        Claims {
            sub: subject.clone(),
            username: subject,
            role: CLUSTER_HA_ROLE.to_string(),
            exp: 0,
            iat: 0,
            sid: None,
            scope: None,
        },
        CLUSTER_MIGRATION_TOKEN_TTL_SECS,
    )
}

/// Lifetime of a `cluster-migration` token: one is minted per request, so
/// this only has to cover the start of that request (a long disk upload is
/// authenticated when it begins).
pub const CLUSTER_MIGRATION_TOKEN_TTL_SECS: usize = 120;

/// Whether a `cluster-migration` token may call `path`.
pub fn cluster_migration_may_access(path: &str) -> bool {
    path.starts_with(CLUSTER_MIGRATION_PATH_PREFIX)
}

/// Generate the short-lived service token for one migration peer request.
pub fn generate_cluster_migration_token(local_hostname: &str) -> Result<String, String> {
    let subject = format!("{}:{}", CLUSTER_MIGRATION_ROLE, local_hostname);
    sign(
        Claims {
            sub: subject.clone(),
            username: subject,
            role: CLUSTER_MIGRATION_ROLE.to_string(),
            exp: 0,
            iat: 0,
            sid: None,
            scope: None,
        },
        CLUSTER_MIGRATION_TOKEN_TTL_SECS,
    )
}

/// Lifetime of a `cluster-node` service token. The usage monitor mints a
/// fresh one every poll round (10s), so this only has to cover one round
/// trip; keep it short so a token captured off the wire is near-useless.
pub const CLUSTER_NODE_TOKEN_TTL_SECS: usize = 30;

/// Lifetime of a normal user login token.
pub const USER_TOKEN_TTL_SECS: usize = 86400; // 24 hours

/// Lifetime of the restricted second-factor-setup login.
pub const TFA_SETUP_TOKEN_TTL_SECS: usize = 900;

/// Whether a request authenticated by a `cluster-node` service token may
/// proceed. Only a read of this node's own usage is allowed -- the token
/// travels to every registered peer address, so it must never grant general
/// API access.
pub fn cluster_node_may_access(method: &axum::http::Method, path: &str) -> bool {
    method == axum::http::Method::GET && path == CLUSTER_NODE_ALLOWED_PATH
}

/// Generate the short-lived, route-scoped service token a node sends to its
/// peers' `local-usage` endpoint.
pub fn generate_cluster_node_token(local_hostname: &str) -> Result<String, String> {
    let subject = format!("{}:{}", CLUSTER_NODE_ROLE, local_hostname);
    sign(
        Claims {
            sub: subject.clone(),
            username: subject,
            role: CLUSTER_NODE_ROLE.to_string(),
            exp: 0,
            iat: 0,
            sid: None,
            scope: None,
        },
        CLUSTER_NODE_TOKEN_TTL_SECS,
    )
}

/// Generate a login token bound to the database session `session_id`.
pub fn generate_session_token(
    user_id: &str,
    username: &str,
    role: &str,
    session_id: &str,
    scope: Option<&str>,
    ttl_secs: usize,
) -> Result<String, String> {
    sign(
        Claims {
            sub: user_id.to_string(),
            username: username.to_string(),
            role: role.to_string(),
            exp: 0,
            iat: 0,
            sid: Some(session_id.to_string()),
            scope: scope.map(str::to_string),
        },
        ttl_secs,
    )
}

fn sign(mut claims: Claims, ttl_secs: usize) -> Result<String, String> {
    let now = chrono::Utc::now().timestamp() as usize;
    claims.iat = now;
    claims.exp = now + ttl_secs;
    let signing = if claims.role.starts_with("cluster-") {
        cluster_service_secret()
    } else {
        get_jwt_secret().to_string()
    };
    let encoding_key = EncodingKey::from_secret(signing.as_bytes());
    encode(&Header::new(Algorithm::HS256), &claims, &encoding_key)
        .map_err(|e| format!("Failed to generate JWT: {}", e))
}

/// Validate API key against database
async fn validate_api_key(
    db: &Arc<crate::db::Database>,
    api_key: &str,
) -> Result<AuthUser, String> {
    use argon2::{Argon2, PasswordHash, PasswordVerifier};

    // API keys should be in the format: "hx_<random_string>"
    if !api_key.starts_with("hx_") || api_key.len() < 32 {
        return Err("Invalid API key format".to_string());
    }

    let pool = db.pool();

    let rows = sqlx::query("SELECT * FROM api_keys WHERE enabled = 1")
        .fetch_all(pool)
        .await
        .map_err(|e| format!("Database query failed: {}", e))?;

    for row in rows {
        use sqlx::Row;
        let key_hash: String = row.get("key_hash");
        let expires_at: Option<i64> = row.get("expires_at");

        if let Some(exp) = expires_at {
            if exp < chrono::Utc::now().timestamp() {
                continue; // Skip expired keys
            }
        }

        let Ok(parsed_hash) = PasswordHash::new(&key_hash) else {
            continue;
        };

        if Argon2::default()
            .verify_password(api_key.as_bytes(), &parsed_hash)
            .is_ok()
        {
            let user_id: String = row.get("user_id");
            let _ =
                sqlx::query("UPDATE api_keys SET last_used_at = CURRENT_TIMESTAMP WHERE id = ?")
                    .bind(row.get::<String, _>("id"))
                    .execute(pool)
                    .await;

            // api_keys.user_id is users.id (a UUID), not the username.
            return match crate::db::users::get_user_by_id(pool, &user_id).await {
                Ok(user) => Ok(AuthUser {
                    user_id: user.id,
                    username: user.username,
                    role: user.role,
                    scope: None,
                    session_id: None,
                    method: AuthMethod::ApiKey,
                }),
                Err(_) => Err("API key belongs to a disabled or deleted user".to_string()),
            };
        }
    }

    Err("Invalid API key".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_token_round_trip() {
        let token =
            generate_session_token("user123", "admin", "admin", "sid1", None, 86400).unwrap();
        assert_eq!(token.split('.').count(), 3);
        let claims = validate_jwt_token(&token).unwrap();
        assert_eq!(claims.sub, "user123");
        assert_eq!(claims.username, "admin");
        assert_eq!(claims.sid.as_deref(), Some("sid1"));
        assert_eq!(claims.exp - claims.iat, USER_TOKEN_TTL_SECS);
    }

    #[test]
    fn test_cluster_node_token_is_short_lived_and_scoped() {
        let token = generate_cluster_node_token("node1").unwrap();
        let claims = validate_jwt_token(&token).unwrap();
        assert_eq!(claims.role, CLUSTER_NODE_ROLE);
        assert_eq!(claims.sub, "cluster-node:node1");
        assert_eq!(claims.exp - claims.iat, CLUSTER_NODE_TOKEN_TTL_SECS);
        assert_eq!(claims.exp - claims.iat, 30);
        assert!(claims.sid.is_none());

        use axum::http::Method;
        assert!(cluster_node_may_access(
            &Method::GET,
            "/api/cluster/local-usage"
        ));
        assert!(!cluster_node_may_access(
            &Method::POST,
            "/api/cluster/local-usage"
        ));
        assert!(!cluster_node_may_access(&Method::GET, "/api/users"));
        assert!(!cluster_node_may_access(
            &Method::POST,
            "/api/cluster/nodes/x"
        ));
    }

    #[test]
    fn test_cluster_migration_token_is_scoped_to_the_peer_api() {
        let token = generate_cluster_migration_token("node1").unwrap();
        let claims = validate_jwt_token(&token).unwrap();
        assert_eq!(claims.role, CLUSTER_MIGRATION_ROLE);
        assert_eq!(claims.exp - claims.iat, CLUSTER_MIGRATION_TOKEN_TTL_SECS);
        assert!(claims.sid.is_none());
        assert!(cluster_migration_may_access("/api/migration/peer/prepare"));
        assert!(cluster_migration_may_access(
            "/api/migration/peer/j1/file/disk0"
        ));
        assert!(!cluster_migration_may_access("/api/migration/jobs"));
        assert!(!cluster_migration_may_access("/api/vms"));
        // The usage token can't reach the peer API and vice versa.
        use axum::http::Method;
        assert!(!cluster_node_may_access(
            &Method::POST,
            "/api/migration/peer/prepare"
        ));
        assert!(!cluster_migration_may_access("/api/cluster/local-usage"));
    }

    #[test]
    fn test_validate_invalid_token() {
        assert!(validate_jwt_token("invalid").is_err());
        // a token signed with the old published default secret is rejected
        let forged = encode(
            &Header::new(Algorithm::HS256),
            &Claims {
                sub: "u".into(),
                username: "admin".into(),
                role: "admin".into(),
                exp: (chrono::Utc::now().timestamp() + 600) as usize,
                iat: 0,
                sid: Some("x".into()),
                scope: None,
            },
            &EncodingKey::from_secret(b"horcrux-default-jwt-secret-change-in-production-please"),
        )
        .unwrap();
        assert!(validate_jwt_token(&forged).is_err());
    }

    #[test]
    fn session_cookie_parsing() {
        let mut h = HeaderMap::new();
        h.insert("cookie", "a=b; session_id=abc123; c=d".parse().unwrap());
        assert_eq!(session_cookie(&h).as_deref(), Some("abc123"));
        let mut h = HeaderMap::new();
        h.insert("cookie", "session_id=".parse().unwrap());
        assert!(session_cookie(&h).is_none());
    }
}
