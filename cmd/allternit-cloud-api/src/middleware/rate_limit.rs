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
        self.check_with_limit(client_id, self.config.requests_per_minute).await
    }

    /// [`Self::check`] with a per-call limit for this window (the open-lane
    /// limiter gives research accounts a higher one under the same key).
    pub async fn check_with_limit(&self, client_id: &str, limit: u32) -> Result<RateLimitInfo, RateLimitInfo> {
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
        if entry.count >= limit {
            let reset_after = self.config.window.saturating_sub(now.duration_since(entry.window_start));
            let info = RateLimitInfo {
                limit,
                remaining: 0,
                reset_after,
            };
            return Err(info);
        }

        // Allow request
        entry.count += 1;

        let remaining = limit - entry.count;
        let reset_after = self.config.window.saturating_sub(now.duration_since(entry.window_start));

        let info = RateLimitInfo {
            limit,
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

/// Default open-lane requests per minute per user (`OPEN_LANE_RPM`).
pub const OPEN_LANE_DEFAULT_RPM: u32 = 30;
/// Research accounts get this many times the open-lane limit unless
/// `OPEN_LANE_RESEARCH_RPM` sets it.
pub const OPEN_LANE_RESEARCH_FACTOR: u32 = 4;

/// Per-user open-lane limits: (standard, research) requests per minute.
/// `OPEN_LANE_RPM` (default 30) and `OPEN_LANE_RESEARCH_RPM` (default 4×
/// the standard limit). A zero or unparsable value falls back to the default.
pub fn open_lane_limits() -> (u32, u32) {
    let read = |name: &str| std::env::var(name).ok().and_then(|v| v.trim().parse::<u32>().ok()).filter(|v| *v > 0);
    let standard = read("OPEN_LANE_RPM").unwrap_or(OPEN_LANE_DEFAULT_RPM);
    let research = read("OPEN_LANE_RESEARCH_RPM").unwrap_or(standard.saturating_mul(OPEN_LANE_RESEARCH_FACTOR));
    (standard, research)
}

/// The open-lane limiter: one fixed window per user (key `open:<user id>`),
/// separate from the per-token limiter on every route. Process-wide, so it
/// needs no `ApiState` field; stale windows are dropped as it grows.
pub fn open_lane_limiter() -> &'static RateLimiter {
    static LIMITER: std::sync::OnceLock<RateLimiter> = std::sync::OnceLock::new();
    LIMITER.get_or_init(|| {
        RateLimiter::new(RateLimitConfig {
            requests_per_minute: OPEN_LANE_DEFAULT_RPM,
            window: Duration::from_secs(60),
        })
    })
}

/// Count one open-lane request for this user. `Err` carries the 429 to send
/// (before any stream starts).
pub async fn check_open_lane(limiter: &RateLimiter, user_id: &str, research: bool) -> Result<RateLimitInfo, Response> {
    check_open_lane_with(limiter, user_id, research, open_lane_limits()).await
}

async fn check_open_lane_with(
    limiter: &RateLimiter,
    user_id: &str,
    research: bool,
    (standard, research_limit): (u32, u32),
) -> Result<RateLimitInfo, Response> {
    let limit = if research { research_limit } else { standard };
    if limiter.clients.read().await.len() > 10_000 {
        limiter.cleanup().await;
    }
    limiter
        .check_with_limit(&format!("open:{user_id}"), limit)
        .await
        .map_err(|info| open_rate_limited_response(&info))
}

/// 429 `open_rate_limited` with `Retry-After` and the rate limit headers.
pub fn open_rate_limited_response(info: &RateLimitInfo) -> Response {
    let retry_after = info.reset_after.as_secs().max(1);
    let body = serde_json::json!({
        "error": {
            "type": "rate_limit_error",
            "code": "open_rate_limited",
            "message": "Too many open-model requests. Try again in a minute.",
            "retry_after": retry_after,
        }
    });
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        [(axum::http::header::RETRY_AFTER, retry_after.to_string())],
        axum::Json(body),
    )
        .into_response();
    info.add_headers(&mut response);
    response
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

    /// Open-lane limit per user: a research account gets more requests in
    /// the same minute than a standard one (default 4x), and the refusal is
    /// the JSON 429 `open_rate_limited`.
    #[tokio::test]
    async fn open_lane_limiter_gives_research_accounts_more() {
        async fn allowed(limiter: &RateLimiter, user: &str, research: bool, limits: (u32, u32)) -> u32 {
            let mut n = 0;
            while check_open_lane_with(limiter, user, research, limits).await.is_ok() {
                n += 1;
                assert!(n < 1_000, "no limit");
            }
            n
        }
        if std::env::var("OPEN_LANE_RPM").is_err() && std::env::var("OPEN_LANE_RESEARCH_RPM").is_err() {
            assert_eq!(open_lane_limits(), (30, 120));
        }
        let limiter = RateLimiter::new(RateLimitConfig::default());
        assert_eq!(allowed(&limiter, "user_std", false, (30, 120)).await, 30);
        assert_eq!(allowed(&limiter, "user_red", true, (30, 120)).await, 120);

        let response = check_open_lane_with(&limiter, "user_std", false, (30, 120)).await.unwrap_err();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(response.headers().contains_key(axum::http::header::RETRY_AFTER));
        let body = axum::body::to_bytes(response.into_body(), 1 << 16).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            (body["error"]["type"].as_str(), body["error"]["code"].as_str()),
            (Some("rate_limit_error"), Some("open_rate_limited"))
        );
    }
}
