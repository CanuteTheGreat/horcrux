///! CORS (Cross-Origin Resource Sharing) middleware
///!
///! Configures CORS headers for API access from web applications

use axum::{
    body::Body,
    http::{header, HeaderValue, Method, Request},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// CORS configuration
#[derive(Clone, Debug)]
pub struct CorsConfig {
    /// Allowed origins (use "*" for any origin)
    pub allowed_origins: Vec<String>,
    /// Allowed HTTP methods
    pub allowed_methods: Vec<Method>,
    /// Allowed headers
    pub allowed_headers: Vec<String>,
    /// Exposed headers
    pub exposed_headers: Vec<String>,
    /// Whether to allow credentials
    pub allow_credentials: bool,
    /// Max age for preflight cache (seconds)
    pub max_age: u32,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            allowed_origins: vec!["*".to_string()],
            allowed_methods: vec![
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::DELETE,
                Method::PATCH,
                Method::OPTIONS,
            ],
            allowed_headers: vec![
                "authorization".to_string(),
                "content-type".to_string(),
                "x-api-key".to_string(),
                "x-requested-with".to_string(),
            ],
            exposed_headers: vec![
                "x-ratelimit-limit".to_string(),
                "x-ratelimit-remaining".to_string(),
                "x-ratelimit-reset".to_string(),
            ],
            allow_credentials: true,
            max_age: 3600,
        }
    }
}

impl CorsConfig {
    /// Create a restrictive CORS config for production
    pub fn restrictive(allowed_origins: Vec<String>) -> Self {
        Self {
            allowed_origins,
            allow_credentials: true,
            ..Default::default()
        }
    }

    /// Create a permissive CORS config for development
    pub fn permissive() -> Self {
        Self::default()
    }
}

/// CORS middleware
pub async fn cors_middleware(
    config: CorsConfig,
    request: Request<Body>,
    next: Next,
) -> Response {
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let method = request.method().clone();

    // Check if origin is allowed
    let origin_allowed = config.allowed_origins.contains(&"*".to_string())
        || config.allowed_origins.contains(&origin.to_string());

    // Handle preflight OPTIONS request
    if method == Method::OPTIONS {
        let mut response = Response::new(Body::empty());

        if origin_allowed {
            // Add CORS headers
            let headers = response.headers_mut();

            // Access-Control-Allow-Origin
            if config.allowed_origins.contains(&"*".to_string()) {
                headers.insert(
                    header::ACCESS_CONTROL_ALLOW_ORIGIN,
                    HeaderValue::from_static("*"),
                );
            } else if !origin.is_empty() {
                headers.insert(
                    header::ACCESS_CONTROL_ALLOW_ORIGIN,
                    HeaderValue::from_str(&origin).unwrap(),
                );
            }

            // Access-Control-Allow-Methods
            let methods = config
                .allowed_methods
                .iter()
                .map(|m| m.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_METHODS,
                HeaderValue::from_str(&methods).unwrap(),
            );

            // Access-Control-Allow-Headers
            let allowed_headers = config.allowed_headers.join(", ");
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_HEADERS,
                HeaderValue::from_str(&allowed_headers).unwrap(),
            );

            // Access-Control-Max-Age
            headers.insert(
                header::ACCESS_CONTROL_MAX_AGE,
                HeaderValue::from_str(&config.max_age.to_string()).unwrap(),
            );

            // Access-Control-Allow-Credentials
            if config.allow_credentials {
                headers.insert(
                    header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
                    HeaderValue::from_static("true"),
                );
            }
        }

        return response;
    }

    // Handle actual request
    let mut response = next.run(request).await;

    if origin_allowed {
        let headers = response.headers_mut();

        // Access-Control-Allow-Origin
        if config.allowed_origins.contains(&"*".to_string()) {
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_static("*"),
            );
        } else if !origin.is_empty() {
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_str(&origin).unwrap(),
            );
        }

        // Access-Control-Expose-Headers
        if !config.exposed_headers.is_empty() {
            let exposed = config.exposed_headers.join(", ");
            headers.insert(
                header::ACCESS_CONTROL_EXPOSE_HEADERS,
                HeaderValue::from_str(&exposed).unwrap(),
            );
        }

        // Access-Control-Allow-Credentials
        if config.allow_credentials {
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
                HeaderValue::from_static("true"),
            );
        }
    }

    response
}

/// Create CORS layer with default config
pub fn create_default_cors() -> CorsConfig {
    CorsConfig::default()
}

/// Create CORS layer with custom origins
pub fn create_cors(allowed_origins: Vec<String>) -> CorsConfig {
    CorsConfig::restrictive(allowed_origins)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = CorsConfig::default();
        assert_eq!(config.allowed_origins, vec!["*"]);
        assert!(config.allow_credentials);
        assert_eq!(config.max_age, 3600);
    }

    #[test]
    fn test_restrictive_config() {
        let config = CorsConfig::restrictive(vec![
            "https://example.com".to_string(),
            "https://app.example.com".to_string(),
        ]);
        assert_eq!(config.allowed_origins.len(), 2);
        assert!(config.allow_credentials);
    }
}
