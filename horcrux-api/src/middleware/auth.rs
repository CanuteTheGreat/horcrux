///! Authentication middleware
///!
///! Validates JWT tokens or session cookies for API requests

use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation, Algorithm};
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

/// Extracted user information from authentication
#[derive(Clone, Debug)]
pub struct AuthUser {
    pub user_id: String,
    pub username: String,
    #[allow(dead_code)]
    pub role: String,
}

/// JWT claims structure
#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,      // subject (user ID)
    username: String,
    role: String,
    exp: usize,       // expiration time (seconds since epoch)
    iat: usize,       // issued at (seconds since epoch)
}

/// JWT secret key (singleton)
/// In production, this should be loaded from environment variable or config file
fn get_jwt_secret() -> &'static str {
    static JWT_SECRET: OnceLock<String> = OnceLock::new();
    JWT_SECRET.get_or_init(|| {
        std::env::var("JWT_SECRET").unwrap_or_else(|_| {
            tracing::warn!("JWT_SECRET not set, using default (INSECURE for production!)");
            "horcrux-default-jwt-secret-change-in-production-please".to_string()
        })
    })
}

/// Authentication middleware (requires AppState with database)
pub async fn auth_middleware(
    State(state): State<Arc<crate::AppState>>,
    headers: HeaderMap,
    mut request: Request,
    next: Next,
) -> Result<Response, AuthError> {
    // Check for Authorization header (Bearer token)
    if let Some(auth_header) = headers.get("authorization") {
        if let Ok(auth_str) = auth_header.to_str() {
            if let Some(token) = auth_str.strip_prefix("Bearer ") {
                // Validate JWT token with signature verification
                match validate_jwt_token(token) {
                    Ok(claims) => {
                        // Add user info to request extensions
                        let auth_user = AuthUser {
                            user_id: claims.sub,
                            username: claims.username,
                            role: claims.role,
                        };
                        request.extensions_mut().insert(auth_user);
                        return Ok(next.run(request).await);
                    }
                    Err(e) => {
                        return Err(AuthError {
                            error: "invalid_token".to_string(),
                            message: format!("Invalid JWT token: {}", e),
                        });
                    }
                }
            }
        }
    }

    // Check for session cookie with database validation
    if let Some(cookie_header) = headers.get("cookie") {
        if let Ok(cookie_str) = cookie_header.to_str() {
            // Parse cookies and look for session ID
            for cookie in cookie_str.split(';') {
                let cookie = cookie.trim();
                if let Some(session_id) = cookie.strip_prefix("session_id=") {
                    // Validate session with database
                    match crate::db::users::get_session(state.database.pool(), session_id).await {
                        Ok(session) => {
                            // Get user details
                            match crate::db::users::get_user_by_username(state.database.pool(), &session.username).await {
                                Ok(user) => {
                                    let auth_user = AuthUser {
                                        user_id: user.id,
                                        username: user.username,
                                        role: user.role,
                                    };
                                    request.extensions_mut().insert(auth_user);
                                    return Ok(next.run(request).await);
                                }
                                Err(_) => {
                                    return Err(AuthError {
                                        error: "invalid_session".to_string(),
                                        message: "Session user not found".to_string(),
                                    });
                                }
                            }
                        }
                        Err(_) => {
                            return Err(AuthError {
                                error: "invalid_session".to_string(),
                                message: "Invalid or expired session".to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    // Check for API key header with database validation
    if let Some(api_key) = headers.get("x-api-key") {
        if let Ok(key_str) = api_key.to_str() {
            // Validate API key with database
            match validate_api_key(&state.database, key_str).await {
                Ok(auth_user) => {
                    request.extensions_mut().insert(auth_user);
                    return Ok(next.run(request).await);
                }
                Err(e) => {
                    return Err(AuthError {
                        error: "invalid_api_key".to_string(),
                        message: e,
                    });
                }
            }
        }
    }

    Err(AuthError {
        error: "unauthorized".to_string(),
        message: "Authentication required. Provide Bearer token, session cookie, or API key.".to_string(),
    })
}

/// Optional authentication middleware (doesn't fail on missing auth)
#[allow(dead_code)]
pub async fn optional_auth_middleware(
    headers: HeaderMap,
    mut request: Request,
    next: Next,
) -> Response {
    // Try to extract auth but don't fail if missing
    if let Some(auth_header) = headers.get("authorization") {
        if let Ok(auth_str) = auth_header.to_str() {
            if let Some(token) = auth_str.strip_prefix("Bearer ") {
                if let Ok(claims) = validate_jwt_token(token) {
                    let auth_user = AuthUser {
                        user_id: claims.sub,
                        username: claims.username,
                        role: claims.role,
                    };
                    request.extensions_mut().insert(auth_user);
                }
            }
        }
    }

    next.run(request).await
}

/// Validate JWT token with proper signature verification
fn validate_jwt_token(token: &str) -> Result<Claims, String> {
    let secret = get_jwt_secret();
    let decoding_key = DecodingKey::from_secret(secret.as_bytes());

    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;

    let token_data = decode::<Claims>(token, &decoding_key, &validation)
        .map_err(|e| format!("JWT validation failed: {}", e))?;

    Ok(token_data.claims)
}

/// Generate JWT token for a user with proper HMAC-SHA256 signature
pub fn generate_jwt_token(user_id: &str, username: &str, role: &str) -> Result<String, String> {
    let now = chrono::Utc::now().timestamp() as usize;
    let expiration = now + 86400; // 24 hours

    let claims = Claims {
        sub: user_id.to_string(),
        username: username.to_string(),
        role: role.to_string(),
        exp: expiration,
        iat: now,
    };

    let secret = get_jwt_secret();
    let encoding_key = EncodingKey::from_secret(secret.as_bytes());
    let header = Header::new(Algorithm::HS256);

    encode(&header, &claims, &encoding_key)
        .map_err(|e| format!("Failed to generate JWT: {}", e))
}

/// Validate API key against database
async fn validate_api_key(db: &Arc<crate::db::Database>, api_key: &str) -> Result<AuthUser, String> {
    use argon2::{Argon2, PasswordHash, PasswordVerifier};

    // API keys should be in the format: "hx_<random_string>"
    if !api_key.starts_with("hx_") || api_key.len() < 32 {
        return Err("Invalid API key format".to_string());
    }

    // Query all enabled API keys from database
    // API keys are validated using Argon2 password hashing (secure)
    let pool = db.pool();

    let rows = sqlx::query("SELECT * FROM api_keys WHERE enabled = 1")
        .fetch_all(pool)
        .await
        .map_err(|e| format!("Database query failed: {}", e))?;

    for row in rows {
        use sqlx::Row;
        let key_hash: String = row.get("key_hash");
        let expires_at: Option<i64> = row.get("expires_at");

        // Check expiration
        if let Some(exp) = expires_at {
            let now = chrono::Utc::now().timestamp();
            if exp < now {
                continue; // Skip expired keys
            }
        }

        // Verify API key hash (using Argon2)
        let parsed_hash = PasswordHash::new(&key_hash)
            .map_err(|_| "Invalid hash format".to_string())?;

        if Argon2::default().verify_password(api_key.as_bytes(), &parsed_hash).is_ok() {
            // Key is valid, get user info
            let user_id: String = row.get("user_id");

            // Update last_used_at
            let _ = sqlx::query("UPDATE api_keys SET last_used_at = CURRENT_TIMESTAMP WHERE id = ?")
                .bind(row.get::<String, _>("id"))
                .execute(pool)
                .await;

            // Get user details
            match crate::db::users::get_user_by_username(pool, &user_id).await {
                Ok(user) => {
                    return Ok(AuthUser {
                        user_id: user.id,
                        username: user.username,
                        role: user.role,
                    });
                }
                Err(_) => {
                    // Try using user_id directly
                    return Ok(AuthUser {
                        user_id: user_id.clone(),
                        username: user_id,
                        role: "user".to_string(),
                    });
                }
            }
        }
    }

    Err("Invalid API key".to_string())
}

/// Role-based access control middleware
#[allow(dead_code)]
pub fn require_role(required_role: &'static str) -> impl Fn(Request, Next) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Response, StatusCode>> + Send>> + Clone {
    move |request: Request, next: Next| {
        Box::pin(async move {
            // Get user from request extensions
            let auth_user = request.extensions().get::<AuthUser>().cloned();

            match auth_user {
                Some(user) => {
                    // Check role
                    if user.role == required_role || user.role == "admin" {
                        Ok(next.run(request).await)
                    } else {
                        Err(StatusCode::FORBIDDEN)
                    }
                }
                None => Err(StatusCode::UNAUTHORIZED),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_jwt_token() {
        let token = generate_jwt_token("user123", "admin", "admin").unwrap();
        assert!(token.contains('.'));

        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
    }

    #[test]
    fn test_validate_jwt_token_format() {
        let token = generate_jwt_token("user123", "admin", "admin").unwrap();
        let claims = validate_jwt_token(&token).unwrap();

        assert_eq!(claims.sub, "user123");
        assert_eq!(claims.username, "admin");
        assert_eq!(claims.role, "admin");
    }

    #[test]
    fn test_validate_invalid_token() {
        let result = validate_jwt_token("invalid");
        assert!(result.is_err());
    }
}
