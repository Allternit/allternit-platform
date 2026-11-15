//! Rate limiting middleware
//!
//! Provides token bucket rate limiting per API key.

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// Rate limit configuration
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Maximum requests per window
    pub requests_per_minute: u32,
    /// Window duration
    pub window: Duration,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            requests_per_minute: 60,
            window: Duration::from_secs(60),
        }
    }
}

/// Rate limit tracker for a single client
#[derive(Debug)]
struct RateLimitEntry {
    /// Number of requests in current window
    count: u32,
    /// Window start time
    window_start: Instant,
}

/// Rate limiter state
#[derive(Debug)]
pub struct RateLimiter {
    config: RateLimitConfig,
    /// Map of client identifier -> rate limit entry
    clients: RwLock<HashMap<String, RateLimitEntry>>,
}

impl RateLimiter {
    /// Create a new rate limiter
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            clients: RwLock::new(HashMap::new()),
        }
    }

    /// Check if a request is allowed
    /// Returns Ok(RateLimitInfo) if allowed, Err(RateLimitInfo) if rate limited
    pub async fn check(&self, client_id: &str) -> Result<RateLimitInfo, RateLimitInfo> {
        let now = Instant::now();
        let mut clients = self.clients.write().await;

        let entry = clients
            .entry(client_id.to_string())
            .or_insert(RateLimitEntry {
                count: 0,
                window_start: now,
            });

        // Check if window has expired
        if now.duration_since(entry.window_start) > self.config.window {
            // Reset window
            entry.count = 0;
            entry.window_start = now;
        }

        // Check limit
        if entry.count >= self.config.requests_per_minute {
            let reset_after = self.config.window - now.duration_since(entry.window_start);
            let info = RateLimitInfo {
                limit: self.config.requests_per_minute,
                remaining: 0,
                reset_after,
            };
            return Err(info);
        }

        // Allow request
        entry.count += 1;

        let remaining = self.config.requests_per_minute - entry.count;
        let reset_after = self.config.window - now.duration_since(entry.window_start);

        let info = RateLimitInfo {
            limit: self.config.requests_per_minute,
            remaining,
            reset_after,
        };
        Ok(info)
    }

    /// Clean up old entries periodically
    pub async fn cleanup(&self) {
        let now = Instant::now();
        let mut clients = self.clients.write().await;
        clients.retain(|_, entry| now.duration_since(entry.window_start) <= self.config.window);
    }
}

/// Rate limit info for response headers
#[derive(Debug, Clone)]
pub struct RateLimitInfo {
    pub limit: u32,
    pub remaining: u32,
    pub reset_after: Duration,
}

impl RateLimitInfo {
    /// Add rate limit headers to response
    pub fn add_headers<B>(&self, response: &mut axum::response::Response<B>) {
        let headers = response.headers_mut();
        let _ = headers.insert("X-RateLimit-Limit", self.limit.to_string().parse().unwrap());
        let _ = headers.insert(
            "X-RateLimit-Remaining",
            self.remaining.to_string().parse().unwrap(),
        );
        let _ = headers.insert(
            "X-RateLimit-Reset",
            self.reset_after.as_secs().to_string().parse().unwrap(),
        );
    }
}

/// Extract client identifier from request
fn extract_client_id(request: &Request) -> String {
    // Try to get from Authorization header (token)
    if let Some(auth) = request.headers().get("authorization") {
        if let Ok(auth_str) = auth.to_str() {
            // Use hash of auth header as identifier
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            auth_str.hash(&mut hasher);
            return format!("token:{}", hasher.finish());
        }
    }

    // Fall back to IP address
    if let Some(connect_info) = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
    {
        return format!("ip:{}", connect_info.0.ip());
    }

    // Last resort: use connection info
    "unknown".to_string()
}

/// Rate limiting middleware
pub async fn rate_limit_middleware(
    State(rate_limiter): State<Arc<RateLimiter>>,
    request: Request,
    next: Next,
) -> Response {
    let client_id = extract_client_id(&request);

    match rate_limiter.check(&client_id).await {
        Ok(info) => {
            let mut response = next.run(request).await;
            info.add_headers(&mut response);
            response
        }
        Err(info) => {
            warn!("Rate limit exceeded for client: {}", client_id);

            let body = serde_json::json!({
                "error": "RATE_LIMIT_EXCEEDED",
                "message": "Too many requests. Please slow down.",
                "retry_after": info.reset_after.as_secs(),
            });

            let mut response = (
                StatusCode::TOO_MANY_REQUESTS,
                [(
                    axum::http::header::RETRY_AFTER,
                    info.reset_after.as_secs().to_string(),
                )],
                axum::Json(body),
            )
                .into_response();

            info.add_headers(&mut response);
            response
        }
    }
}

/// Create rate limiter and spawn cleanup task
pub fn create_rate_limiter(config: RateLimitConfig) -> Arc<RateLimiter> {
    let limiter = Arc::new(RateLimiter::new(config));

    // Spawn cleanup task
    let cleanup_limiter = limiter.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(300)); // Every 5 minutes
        loop {
            interval.tick().await;
            cleanup_limiter.cleanup().await;
            debug!("Rate limiter cleanup completed");
        }
    });

    limiter
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn limiter_blocks_over_limit_per_key() {
        let limiter = RateLimiter::new(RateLimitConfig {
            requests_per_minute: 30,
            window: Duration::from_secs(60),
        });

        for _ in 0..30 {
            assert!(limiter.check("user_free").await.is_ok());
        }
        let info = limiter.check("user_free").await.unwrap_err();
        assert_eq!(info.limit, 30);
        assert_eq!(info.remaining, 0);

        // The limiter is keyed per caller: another caller is unaffected.
        assert!(limiter.check("user_other").await.is_ok());

        // The 429 response carries the rate limit headers.
        let mut response = (StatusCode::TOO_MANY_REQUESTS, axum::Json(serde_json::json!({}))).into_response();
        info.add_headers(&mut response);
        assert_eq!(
            response.headers()["X-RateLimit-Limit"].to_str().unwrap(),
            "30"
        );
        assert_eq!(
            response.headers()["X-RateLimit-Remaining"].to_str().unwrap(),
            "0"
        );
        assert!(response.headers().contains_key("X-RateLimit-Reset"));
    }
}
