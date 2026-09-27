//! Per-organization rate-limit enforcement for the Clerk-protected public API.
//!
//! Uses an in-memory sliding window keyed by organization_id (or user_id when
//! no organization is selected). The limit is read from
//! `organizations.api_rate_limit_rpm` when present, otherwise a default is
//! applied. Excess requests receive `429 Too Many Requests` with a
//! `Retry-After` header.
//!
//! This middleware is a companion to the LLM gateway's per-key rate limiter
//! (`llm_gateway::auth`). It should be layered on the protected router after
//! `auth_middleware` so `AuthUser` is present in request extensions.

use axum::{
    extract::{Extension, Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use once_cell::sync::Lazy;
use rusqlite::OptionalExtension;
use serde_json::json;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::warn;

use crate::auth::AuthUser;
use crate::db::DbHandle;
use crate::AppState;

/// Default per-organization public API rate limit when no override is set.
pub const DEFAULT_PUBLIC_API_RATE_LIMIT_RPM: i64 = 600;
/// Sliding-window length for the rate limiter.
const RATE_WINDOW: Duration = Duration::from_secs(60);

/// Per-organization sliding-window request timestamps, process-wide. Entries
/// are pruned on access.
static RATE_LIMIT_WINDOWS: Lazy<Mutex<HashMap<String, VecDeque<Instant>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Default per-user rate limit for bot-desktop endpoints. Desktop operations
/// (provision, take-over, etc.) are heavier than normal API calls, so they get
/// a separate, stricter bucket.
pub const DEFAULT_BOT_DESKTOP_RATE_LIMIT_RPM: i64 = 30;

/// Per-user sliding window for bot-desktop endpoints.
static BOT_DESKTOP_RATE_LIMIT_WINDOWS: Lazy<Mutex<HashMap<String, VecDeque<Instant>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn rate_limit_response(retry_after_secs: u64) -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(header::RETRY_AFTER, retry_after_secs.to_string())],
        Json(json!({
            "error": {
                "message": "Rate limit exceeded. Please retry after the Retry-After interval.",
                "type": "rate_limit_exceeded",
                "code": "allternit.rate_limited"
            }
        })),
    )
        .into_response()
}

fn internal_error(message: String) -> Response {
    warn!(%message, "rate limit middleware error");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({
            "error": {
                "message": message,
                "type": "server_error",
                "code": "allternit.internal_error"
            }
        })),
    )
        .into_response()
}

/// Public API rate-limit middleware. Runs after auth so `AuthUser` is
/// available; no-ops when there is no authenticated user.
pub async fn rate_limit_middleware(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    match rate_limit_middleware_inner(&state, request, next).await {
        Ok(response) => response,
        Err(response) => response,
    }
}

async fn rate_limit_middleware_inner(
    state: &AppState,
    request: Request,
    next: Next,
) -> Result<Response, Response> {
    // The packaged desktop's own UI boot-straps dozens of operator calls in
    // parallel. The public Clerk RPM cap is for api.allternit.com, not the
    // loopback kernel talking to itself. Only a header that matches the
    // spawn-time secret bypasses — spoofing the header on a public node does
    // nothing because no secret is configured there.
    if crate::auth::verify_desktop_access_token(request.headers(), &state.config) {
        return Ok(next.run(request).await);
    }

    let user = match request.extensions().get::<AuthUser>().cloned() {
        Some(user) => user,
        None => return Ok(next.run(request).await),
    };

    let scope = user
        .organization_id
        .clone()
        .unwrap_or_else(|| user.user_id.clone());

    let limit = lookup_org_limit(&state.db, &scope)
        .await
        .unwrap_or(None)
        .unwrap_or(DEFAULT_PUBLIC_API_RATE_LIMIT_RPM)
        .max(1) as usize;

    if !check_rate_limit(&scope, limit) {
        let retry_after = compute_retry_after(&scope);
        return Err(rate_limit_response(retry_after));
    }

    Ok(next.run(request).await)
}

/// Read the organization's `api_rate_limit_rpm` override, if one exists.
async fn lookup_org_limit(db: &DbHandle, org_id: &str) -> rusqlite::Result<Option<i64>> {
    let db = db.clone();
    let org_id = org_id.to_string();
    tokio::task::spawn_blocking(move || -> rusqlite::Result<Option<i64>> {
        let conn = db.connect()?;
        let maybe: Option<Option<i64>> = conn
            .query_row(
                "SELECT api_rate_limit_rpm FROM organizations WHERE id = ?1",
                rusqlite::params![org_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()?;
        Ok(maybe.flatten())
    })
    .await
    .map_err(|err| rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(1),
        Some(format!("rate limit lookup task failed: {err}")),
    ))?
}

/// Record a request in the sliding window and return false if the limit is
/// exceeded.
fn check_rate_limit(scope: &str, limit: usize) -> bool {
    let now = Instant::now();
    let mut windows = RATE_LIMIT_WINDOWS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let window = windows.entry(scope.to_string()).or_default();
    while window
        .front()
        .map(|t| now.duration_since(*t) > RATE_WINDOW)
        .unwrap_or(false)
    {
        window.pop_front();
    }
    if window.len() >= limit {
        return false;
    }
    window.push_back(now);
    true
}

/// Seconds until the oldest tracked request for this scope falls out of the
/// window. Returns at least 1.
fn compute_retry_after(scope: &str) -> u64 {
    let now = Instant::now();
    let windows = RATE_LIMIT_WINDOWS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    windows
        .get(scope)
        .and_then(|window| window.front())
        .map(|oldest| {
            let elapsed = now.duration_since(*oldest);
            if elapsed >= RATE_WINDOW {
                1
            } else {
                (RATE_WINDOW - elapsed).as_secs().max(1)
            }
        })
        .unwrap_or(1)
}

/// Read-only snapshot of a scope's current rate-limit state.
#[derive(Debug, Clone, Copy)]
pub struct RateLimitStatus {
    pub remaining: usize,
    pub limit: usize,
    pub reset_in_secs: u64,
}

/// Return the number of requests still allowed in the current sliding window
/// and the seconds until the oldest tracked request falls out. Read-only:
/// it prunes stale entries but never records a new request.
pub fn rate_limit_status(scope: &str, limit: usize) -> RateLimitStatus {
    let now = Instant::now();
    let mut windows = RATE_LIMIT_WINDOWS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let window = windows.entry(scope.to_string()).or_default();
    while window
        .front()
        .map(|t| now.duration_since(*t) > RATE_WINDOW)
        .unwrap_or(false)
    {
        window.pop_front();
    }
    let used = window.len().min(limit);
    let remaining = limit.saturating_sub(used);
    let reset_in_secs = window
        .front()
        .map(|t| {
            let elapsed = now.duration_since(*t);
            if elapsed >= RATE_WINDOW {
                0
            } else {
                (RATE_WINDOW - elapsed).as_secs().max(1)
            }
        })
        .unwrap_or(0);
    RateLimitStatus {
        remaining,
        limit,
        reset_in_secs,
    }
}

/// Stricter rate-limit middleware for the bot-desktop REST endpoints. Runs
/// after `auth_middleware`; keys the sliding window by authenticated user id.
pub async fn bot_desktop_rate_limit_middleware(
    Extension(user): Extension<AuthUser>,
    request: Request,
    next: Next,
) -> Response {
    let limit = DEFAULT_BOT_DESKTOP_RATE_LIMIT_RPM.max(1) as usize;
    let scope = format!("desktop:{}", user.user_id);
    if !bot_desktop_check_rate_limit(&scope, limit) {
        let retry_after = bot_desktop_compute_retry_after(&scope);
        return rate_limit_response(retry_after);
    }
    next.run(request).await
}

fn bot_desktop_check_rate_limit(scope: &str, limit: usize) -> bool {
    let now = Instant::now();
    let mut windows = BOT_DESKTOP_RATE_LIMIT_WINDOWS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let window = windows.entry(scope.to_string()).or_default();
    while window
        .front()
        .map(|t| now.duration_since(*t) > RATE_WINDOW)
        .unwrap_or(false)
    {
        window.pop_front();
    }
    if window.len() >= limit {
        return false;
    }
    window.push_back(now);
    true
}

fn bot_desktop_compute_retry_after(scope: &str) -> u64 {
    let now = Instant::now();
    let windows = BOT_DESKTOP_RATE_LIMIT_WINDOWS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    windows
        .get(scope)
        .and_then(|window| window.front())
        .map(|oldest| {
            let elapsed = now.duration_since(*oldest);
            if elapsed >= RATE_WINDOW {
                1
            } else {
                (RATE_WINDOW - elapsed).as_secs().max(1)
            }
        })
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthUser;
    use axum::{body::Body, routing::get, Router};
    use tower::ServiceExt;

    fn test_user(org_id: Option<&str>) -> AuthUser {
        AuthUser {
            user_id: "user-1".to_string(),
            email: None,
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: org_id.map(str::to_string),
            organization_role: None,
            organization_slug: None,
        }
    }

    fn request(org_id: Option<&str>) -> Request<axum::body::Body> {
        Request::builder()
            .method("GET")
            .uri("/test")
            .extension(test_user(org_id))
            .body(axum::body::Body::empty())
            .unwrap()
    }

    #[test]
    fn status_defaults_to_full_window() {
        let status = rate_limit_status("org-empty", 100);
        assert_eq!(status.limit, 100);
        assert_eq!(status.remaining, 100);
        assert_eq!(status.reset_in_secs, 0);
    }

    #[test]
    fn status_is_read_only() {
        {
            let mut windows = RATE_LIMIT_WINDOWS
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let window = windows.entry("org-ro".to_string()).or_default();
            window.push_back(Instant::now());
            window.push_back(Instant::now());
        }

        let first = rate_limit_status("org-ro", 10);
        assert_eq!(first.remaining, 8);

        let second = rate_limit_status("org-ro", 10);
        assert_eq!(second.remaining, 8);
    }

    #[tokio::test]
    async fn rejects_requests_over_limit_with_retry_after() {
        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(temp.path()).await;
        {
            let conn = state.db.connect().unwrap();
            conn.execute(
                "INSERT INTO organizations (id, name, api_rate_limit_rpm) VALUES ('org-limit', 'Limit Org', 2)",
                [],
            )
            .unwrap();
        }
        let app = Router::new()
            .route("/test", get(|| async { StatusCode::NO_CONTENT }))
            .layer(axum::middleware::from_fn_with_state(
                state,
                rate_limit_middleware,
            ));

        // Exhaust a limit of 2 RPM.
        for _ in 0..2 {
            let resp = app
                .clone()
                .oneshot(request(Some("org-limit")))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        }

        let blocked = app.clone().oneshot(request(Some("org-limit"))).await.unwrap();
        assert_eq!(blocked.status(), StatusCode::TOO_MANY_REQUESTS);
        let retry_after = blocked
            .headers()
            .get(header::RETRY_AFTER)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(!retry_after.is_empty());

        // A different organization is not affected.
        let other = app.clone().oneshot(request(Some("org-other"))).await.unwrap();
        assert_eq!(other.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn org_override_is_honored() {
        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(temp.path()).await;
        // Seed the organizations table with a low limit.
        {
            let conn = state.db.connect().unwrap();
            conn.execute(
                "INSERT INTO organizations (id, name, api_rate_limit_rpm)
                 VALUES ('org-override', 'Override Org', 1)",
                [],
            )
            .unwrap();
        }

        let app = Router::new()
            .route("/test", get(|| async { StatusCode::NO_CONTENT }))
            .layer(axum::middleware::from_fn_with_state(
                state,
                rate_limit_middleware,
            ));

        let first = app.clone().oneshot(request(Some("org-override"))).await.unwrap();
        assert_eq!(first.status(), StatusCode::NO_CONTENT);

        let second = app.clone().oneshot(request(Some("org-override"))).await.unwrap();
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    fn desktop_request(user_id: &str) -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri("/test")
            .extension(AuthUser {
                user_id: user_id.to_string(),
                email: None,
                name: None,
                avatar_url: None,
                tenant_id: None,
                organization_id: None,
                organization_role: None,
                organization_slug: None,
            })
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn bot_desktop_rate_limit_allows_default_then_blocks() {
        let app = Router::new()
            .route("/test", get(|| async { StatusCode::NO_CONTENT }))
            .layer(axum::middleware::from_fn(bot_desktop_rate_limit_middleware));

        let user_id = uuid::Uuid::new_v4().to_string();
        for i in 0..DEFAULT_BOT_DESKTOP_RATE_LIMIT_RPM as usize {
            let resp = app.clone().oneshot(desktop_request(&user_id)).await.unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::NO_CONTENT,
                "request {} should be allowed",
                i
            );
        }

        let blocked = app.clone().oneshot(desktop_request(&user_id)).await.unwrap();
        assert_eq!(blocked.status(), StatusCode::TOO_MANY_REQUESTS);
        let retry_after = blocked
            .headers()
            .get(header::RETRY_AFTER)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(!retry_after.is_empty());
    }

    #[tokio::test]
    async fn verified_desktop_access_token_bypasses_public_rate_limit() {
        let _guard = crate::test_helpers::computer_use_dir_test_lock();
        std::env::set_var("ALLTERNIT_DESKTOP_ACCESS_TOKEN", "desktop-secret");

        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(temp.path()).await;
        {
            let conn = state.db.connect().unwrap();
            conn.execute(
                "INSERT INTO organizations (id, name, api_rate_limit_rpm)
                 VALUES ('org-desktop-bypass', 'Desktop Org', 1)",
                [],
            )
            .unwrap();
        }
        let app = Router::new()
            .route("/test", get(|| async { StatusCode::NO_CONTENT }))
            .layer(axum::middleware::from_fn_with_state(
                state,
                rate_limit_middleware,
            ));

        fn with_desktop_token(org: &str, token: &str) -> Request<Body> {
            Request::builder()
                .method("GET")
                .uri("/test")
                .header("x-allternit-desktop-access-token", token)
                .extension(test_user(Some(org)))
                .body(Body::empty())
                .unwrap()
        }

        for i in 0..5 {
            let resp = app
                .clone()
                .oneshot(with_desktop_token("org-desktop-bypass", "desktop-secret"))
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::NO_CONTENT,
                "verified desktop request {i} should skip the 1 RPM cap"
            );
        }

        let spoofed = app
            .clone()
            .oneshot(with_desktop_token("org-desktop-bypass", "wrong-secret"))
            .await
            .unwrap();
        assert_eq!(spoofed.status(), StatusCode::NO_CONTENT);

        let blocked = app
            .clone()
            .oneshot(with_desktop_token("org-desktop-bypass", "wrong-secret"))
            .await
            .unwrap();
        assert_eq!(blocked.status(), StatusCode::TOO_MANY_REQUESTS);

        std::env::remove_var("ALLTERNIT_DESKTOP_ACCESS_TOKEN");
    }

    #[tokio::test]
    async fn bot_desktop_rate_limit_is_per_user() {
        let app = Router::new()
            .route("/test", get(|| async { StatusCode::NO_CONTENT }))
            .layer(axum::middleware::from_fn(bot_desktop_rate_limit_middleware));

        let user_a = uuid::Uuid::new_v4().to_string();
        // Exhaust user A.
        for _ in 0..DEFAULT_BOT_DESKTOP_RATE_LIMIT_RPM as usize {
            let _ = app.clone().oneshot(desktop_request(&user_a)).await.unwrap();
        }
        let blocked = app.clone().oneshot(desktop_request(&user_a)).await.unwrap();
        assert_eq!(blocked.status(), StatusCode::TOO_MANY_REQUESTS);

        // User B is unaffected.
        let user_b = uuid::Uuid::new_v4().to_string();
        let allowed = app.clone().oneshot(desktop_request(&user_b)).await.unwrap();
        assert_eq!(allowed.status(), StatusCode::NO_CONTENT);
    }
}
