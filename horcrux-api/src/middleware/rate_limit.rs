///! Rate limiting middleware
///!
///! Implements token bucket algorithm to limit request rates per IP/user

use axum::{
    extract::{ConnectInfo, Request},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Rate limit configuration
#[derive(Clone, Debug)]
pub struct RateLimitConfig {
    /// Maximum number of requests allowed in the window
    pub max_requests: u32,
    /// Time window duration
    pub window: Duration,
    /// Whether to use user-based limiting (fallback to IP)
    pub per_user: bool,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_requests: 100,
            window: Duration::from_secs(60),
            per_user: true,
        }
    }
}

/// Token bucket for rate limiting
#[derive(Clone, Debug)]
struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
    max_tokens: f64,
    refill_rate: f64, // tokens per second
}

impl TokenBucket {
    fn new(max_tokens: u32, window: Duration) -> Self {
        let refill_rate = max_tokens as f64 / window.as_secs_f64();
        Self {
            tokens: max_tokens as f64,
            last_refill: Instant::now(),
            max_tokens: max_tokens as f64,
            refill_rate,
        }
    }

    fn try_consume(&mut self) -> bool {
        // Refill tokens based on time elapsed
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        let new_tokens = elapsed * self.refill_rate;
        self.tokens = (self.tokens + new_tokens).min(self.max_tokens);
        self.last_refill = now;

        // Try to consume a token
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    fn remaining(&self) -> u32 {
        self.tokens.floor() as u32
    }

    fn reset_after(&self) -> Duration {
        if self.tokens >= self.max_tokens {
            Duration::from_secs(0)
        } else {
            let tokens_needed = 1.0 - self.tokens;
            let seconds = tokens_needed / self.refill_rate;
            Duration::from_secs_f64(seconds.max(0.0))
        }
    }
}

/// Rate limiter state
pub struct RateLimiter {
    buckets: Arc<RwLock<HashMap<String, TokenBucket>>>,
    config: RateLimitConfig,
}

impl RateLimiter {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            buckets: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Check if request is allowed
    async fn check(&self, key: &str) -> RateLimitResult {
        let mut buckets = self.buckets.write().await;

        let bucket = buckets.entry(key.to_string()).or_insert_with(|| {
            TokenBucket::new(self.config.max_requests, self.config.window)
        });

        let allowed = bucket.try_consume();
        let remaining = bucket.remaining();
        let reset_after = bucket.reset_after();

        RateLimitResult {
            allowed,
            limit: self.config.max_requests,
            remaining,
            reset_after,
        }
    }

    /// Clean up old entries periodically
    pub async fn cleanup(&self) {
        let mut buckets = self.buckets.write().await;
        let now = Instant::now();

        buckets.retain(|_, bucket| {
            // Keep buckets that were accessed in the last 5 minutes
            now.duration_since(bucket.last_refill) < Duration::from_secs(300)
        });
    }
}

/// Rate limit check result
struct RateLimitResult {
    allowed: bool,
    limit: u32,
    remaining: u32,
    reset_after: Duration,
}

/// Rate limit error response
#[derive(Serialize)]
pub struct RateLimitError {
    error: String,
    message: String,
    retry_after: u64, // seconds
}

impl IntoResponse for RateLimitError {
    fn into_response(self) -> Response {
        let json = Json(self);
        (StatusCode::TOO_MANY_REQUESTS, json).into_response()
    }
}

/// Rate limiting middleware
pub async fn rate_limit_middleware(
    limiter: Arc<RateLimiter>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Result<Response, RateLimitError> {
    // Determine rate limit key
    let key = if limiter.config.per_user {
        // Try to get user from request extensions (set by auth middleware)
        if let Some(auth_user) = request.extensions().get::<crate::middleware::auth::AuthUser>() {
            format!("user:{}", auth_user.user_id)
        } else {
            // Fallback to IP
            format!("ip:{}", addr.ip())
        }
    } else {
        format!("ip:{}", addr.ip())
    };

    // Check rate limit
    let result = limiter.check(&key).await;

    if result.allowed {
        // Add rate limit headers to response
        let mut response = next.run(request).await;
        let headers = response.headers_mut();

        headers.insert(
            "X-RateLimit-Limit",
            result.limit.to_string().parse().unwrap(),
        );
        headers.insert(
            "X-RateLimit-Remaining",
            result.remaining.to_string().parse().unwrap(),
        );
        headers.insert(
            "X-RateLimit-Reset",
            result.reset_after.as_secs().to_string().parse().unwrap(),
        );

        Ok(response)
    } else {
        Err(RateLimitError {
            error: "rate_limit_exceeded".to_string(),
            message: format!(
                "Rate limit exceeded. Maximum {} requests per {} seconds.",
                result.limit,
                limiter.config.window.as_secs()
            ),
            retry_after: result.reset_after.as_secs(),
        })
    }
}

/// Create a rate limiter with default config
pub fn create_default_limiter() -> Arc<RateLimiter> {
    Arc::new(RateLimiter::new(RateLimitConfig::default()))
}

/// Create a rate limiter with custom config
pub fn create_limiter(config: RateLimitConfig) -> Arc<RateLimiter> {
    Arc::new(RateLimiter::new(config))
}

/// Start background cleanup task
pub fn start_cleanup_task(limiter: Arc<RateLimiter>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            limiter.cleanup().await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_bucket() {
        let mut bucket = TokenBucket::new(10, Duration::from_secs(10));

        // Should be able to consume 10 tokens
        for _ in 0..10 {
            assert!(bucket.try_consume());
        }

        // 11th token should fail
        assert!(!bucket.try_consume());
    }

    #[tokio::test]
    async fn test_rate_limiter() {
        let config = RateLimitConfig {
            max_requests: 5,
            window: Duration::from_secs(1),
            per_user: false,
        };
        let limiter = RateLimiter::new(config);

        // First 5 requests should succeed
        for _ in 0..5 {
            let result = limiter.check("test-key").await;
            assert!(result.allowed);
        }

        // 6th request should fail
        let result = limiter.check("test-key").await;
        assert!(!result.allowed);
    }

    #[tokio::test]
    async fn test_token_refill() {
        let config = RateLimitConfig {
            max_requests: 2,
            window: Duration::from_millis(100),
            per_user: false,
        };
        let limiter = RateLimiter::new(config);

        // Consume all tokens
        assert!(limiter.check("test-key").await.allowed);
        assert!(limiter.check("test-key").await.allowed);
        assert!(!limiter.check("test-key").await.allowed);

        // Wait for refill
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Should have tokens again
        assert!(limiter.check("test-key").await.allowed);
    }
}
