//! Idempotency middleware for the Clerk-protected public API surface.
//!
//! Honors the `Idempotency-Key` header on POST/PUT/PATCH requests. The cache
//! key is scoped by organization (or by user when no organization is selected)
//! plus the caller-provided idempotency key. Within the TTL a duplicate key
//! replays the stored response body and status; while the first request is
//! still in flight, duplicates receive `409 Conflict`.
//!
//! The middleware is intended to be layered on the protected router *after*
//! `auth_middleware` so `AuthUser` is present in request extensions.

use axum::{
    body::{Body, Bytes},
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use http_body_util::BodyExt;
use rusqlite::OptionalExtension;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

use crate::auth::AuthUser;
use crate::db::DbHandle;
use crate::AppState;

/// Completed cache entries are replayed for this long.
const IDEMPOTENCY_TTL: Duration = Duration::from_secs(86400);
/// An `in_progress` row older than this is treated as abandoned and may be
/// retried (e.g. the server process died mid-request).
const IN_PROGRESS_STALE_SECS: i64 = 600;

/// Headers stored on a cached response and replayed to duplicates.
/// `content-length` is intentionally omitted because `Body::from` recomputes it.
const CACHED_HEADER_NAMES: &[&str] = &["content-type"];

#[derive(Debug, Clone)]
struct CacheEntry {
    response_status: u16,
    response_headers: serde_json::Map<String, serde_json::Value>,
    response_body: Vec<u8>,
}

/// Reject a duplicate key that is already being processed.
fn conflict_response() -> Response {
    (
        StatusCode::CONFLICT,
        Json(json!({
            "error": {
                "message": "A request with this Idempotency-Key is already in progress.",
                "type": "idempotency_conflict",
                "code": "allternit.idempotency_conflict"
            }
        })),
    )
        .into_response()
}

fn bad_request(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "error": {
                "message": message,
                "type": "invalid_request_error",
                "code": "allternit.invalid_request"
            }
        })),
    )
        .into_response()
}

fn internal_error(message: String) -> Response {
    warn!(%message, "idempotency middleware error");
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

/// Public API idempotency middleware. Runs after auth so `AuthUser` is
/// available; no-ops for GET/DELETE/HEAD/OPTIONS or when the header is absent.
pub async fn idempotency_middleware(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    match idempotency_middleware_inner(&state.db, request, next).await {
        Ok(response) => response,
        Err(response) => response,
    }
}

async fn idempotency_middleware_inner(
    db: &DbHandle,
    request: Request,
    next: Next,
) -> Result<Response, Response> {
    // The Agency API create endpoint owns its idempotency (durable, ledger-
    // backed, 409 ERR_IDEMPOTENCY_KEY_REUSED, `Idempotency-Replayed`; v0.3 Q10).
    if request.uri().path() == "/v1/agency" {
        return Ok(next.run(request).await);
    }
    // Only mutable methods benefit from idempotency guarantees.
    if !matches!(
        request.method(),
        &axum::http::Method::POST
            | &axum::http::Method::PUT
            | &axum::http::Method::PATCH
    ) {
        return Ok(next.run(request).await);
    }

    let user = match request.extensions().get::<AuthUser>().cloned() {
        Some(user) => user,
        None => {
            // Protected routes should always have AuthUser. If not, do not
            // attempt to cache anything.
            return Ok(next.run(request).await);
        }
    };

    let idem_key = match request
        .headers()
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(key) if key.is_ascii() && key.len() <= 255 => key.to_string(),
        Some(_) => {
            return Err(bad_request(
                "Idempotency-Key must be 1-255 ASCII characters.",
            ))
        }
        None => return Ok(next.run(request).await),
    };

    let scope = user
        .organization_id
        .clone()
        .unwrap_or_else(|| user.user_id.clone());
    let method = request.method().to_string();
    let path = request.uri().path().to_string();

    // 1. Look up or reserve a cache slot synchronously on the DB thread.
    let db2 = db.clone();
    let idem_key2 = idem_key.clone();
    let scope2 = scope.clone();
    let method_for_lookup = method.clone();
    let path_for_lookup = path.clone();
    let lookup = tokio::task::spawn_blocking(move || {
        lookup_or_reserve(&db2, &scope2, &idem_key2, &method_for_lookup, &path_for_lookup)
    })
    .await
    .map_err(|err| internal_error(format!("idempotency lookup task failed: {err}")))?
    .map_err(|err| internal_error(format!("idempotency lookup failed: {err}")))?;

    match lookup {
        LookupResult::Replay(entry) => return Ok(replay_response(entry)),
        LookupResult::Conflict => return Err(conflict_response()),
        LookupResult::Proceed => {}
    }

    // 2. Run the handler and capture the response.
    let response = next.run(request).await;
    let (parts, body) = response.into_parts();
    let collected = body
        .collect()
        .await
        .map_err(|err| internal_error(format!("failed to collect response body: {err}")))?;
    let bytes = collected.to_bytes();

    let captured = capture_response(&parts.headers, parts.status, &bytes);
    let replay = build_replay_response(&captured);

    // 3. Persist the captured response so duplicates replay it.
    let db2 = db.clone();
    let idem_key2 = idem_key.clone();
    let scope2 = scope.clone();
    let method2 = method.clone();
    let path2 = path.clone();
    let captured2 = captured.clone();
    if let Err(err) = tokio::task::spawn_blocking(move || {
        complete(
            &db2,
            &scope2,
            &idem_key2,
            &method2,
            &path2,
            &captured2,
        )
    })
    .await
    {
        warn!(error = %err, "failed to persist idempotency response");
    }

    Ok(replay)
}

enum LookupResult {
    /// A completed, non-expired entry exists — replay it.
    Replay(CacheEntry),
    /// Another request with this key is currently in flight.
    Conflict,
    /// No usable entry; caller should run the handler.
    Proceed,
}

fn lookup_or_reserve(
    db: &DbHandle,
    scope: &str,
    idem_key: &str,
    method: &str,
    path: &str,
) -> rusqlite::Result<LookupResult> {
    let conn = db.connect()?;

    // Purge expired entries lazily.
    conn.execute(
        "DELETE FROM idempotency_cache WHERE expires_at < datetime('now')",
        [],
    )?;

    let existing = conn
        .query_row(
            "SELECT response_status, response_headers, response_body,
                    CAST(strftime('%s', 'now') AS INTEGER)
                      - CAST(strftime('%s', created_at) AS INTEGER) AS age_secs
             FROM idempotency_cache
             WHERE organization_id = ?1 AND idempotency_key = ?2",
            rusqlite::params![scope, idem_key],
            |row| {
                let status: Option<u16> = row.get(0)?;
                let headers_json: Option<String> = row.get(1)?;
                let body: Option<Vec<u8>> = row.get(2)?;
                let age_secs: i64 = row.get(3)?;
                Ok((status, headers_json, body, age_secs))
            },
        )
        .optional()?;


    if let Some((status, headers_json, body, age_secs)) = existing {
        match (status, body, age_secs > IN_PROGRESS_STALE_SECS) {
            // Abandoned in-flight row: delete it and let this request proceed.
            (None, None, true) => {
                conn.execute(
                    "DELETE FROM idempotency_cache
                     WHERE organization_id = ?1 AND idempotency_key = ?2",
                    rusqlite::params![scope, idem_key],
                )?;
            }
            // In-flight and not stale: conflict.
            (None, None, false) => return Ok(LookupResult::Conflict),
            // Completed entry: replay.
            (Some(status), Some(body), _) => {
                let headers = headers_json
                    .and_then(|json| serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&json).ok())
                    .unwrap_or_default();
                return Ok(LookupResult::Replay(CacheEntry {
                    response_status: status,
                    response_headers: headers,
                    response_body: body,
                }));
            }
            // Any other state is malformed; treat as conflict to be safe.
            _ => return Ok(LookupResult::Conflict),
        }
    }

    // Reserve an in-flight slot. A unique index race means another request
    // beat us here; report it as a conflict.
    let id = uuid::Uuid::new_v4().to_string();
    let expires_at = (chrono::Utc::now() + chrono::Duration::seconds(IDEMPOTENCY_TTL.as_secs() as i64))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    match conn.execute(
        "INSERT INTO idempotency_cache
         (id, organization_id, idempotency_key, request_method, request_path,
          response_status, response_headers, response_body, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, NULL, ?6)",
        rusqlite::params![id, scope, idem_key, method, path, expires_at],
    ) {
        Ok(_) => Ok(LookupResult::Proceed),
        Err(rusqlite::Error::SqliteFailure(failure, _))
            if failure.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            Ok(LookupResult::Conflict)
        }
        Err(err) => Err(err),
    }
}

fn complete(
    db: &DbHandle,
    scope: &str,
    idem_key: &str,
    method: &str,
    path: &str,
    captured: &CacheEntry,
) -> rusqlite::Result<()> {
    let conn = db.connect()?;
    let headers_json = serde_json::to_string(&captured.response_headers).unwrap_or_default();
    let expires_at = (chrono::Utc::now() + chrono::Duration::seconds(IDEMPOTENCY_TTL.as_secs() as i64))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    conn.execute(
        "UPDATE idempotency_cache
         SET request_method = ?3,
             request_path = ?4,
             response_status = ?5,
             response_headers = ?6,
             response_body = ?7,
             expires_at = ?8
         WHERE organization_id = ?1 AND idempotency_key = ?2",
        rusqlite::params![
            scope,
            idem_key,
            method,
            path,
            captured.response_status,
            headers_json,
            &captured.response_body,
            expires_at,
        ],
    )?;
    Ok(())
}

fn capture_response(headers: &HeaderMap, status: StatusCode, body: &Bytes) -> CacheEntry {
    let mut response_headers = serde_json::Map::new();
    for name in CACHED_HEADER_NAMES {
        if let Some(value) = headers.get(*name).and_then(|v| v.to_str().ok()) {
            response_headers.insert(name.to_string(), json!(value));
        }
    }
    CacheEntry {
        response_status: status.as_u16(),
        response_headers,
        response_body: body.to_vec(),
    }
}

fn build_replay_response(captured: &CacheEntry) -> Response {
    let mut builder = Response::builder().status(captured.response_status);
    for (name, value) in &captured.response_headers {
        if let Some(value_str) = value.as_str() {
            if let Ok(header_value) = HeaderValue::from_str(value_str) {
                builder = builder.header(name, header_value);
            }
        }
    }
    builder
        .body(Body::from(Bytes::from(captured.response_body.clone())))
        .unwrap_or_else(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, "invalid cached response").into_response()
        })
}

fn replay_response(entry: CacheEntry) -> Response {
    build_replay_response(&entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthUser;
    use axum::{routing::post, Router};
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

    fn idem_request(org_id: Option<&str>, key: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/test")
            .header("idempotency-key", key)
            .extension(test_user(org_id))
            .body(Body::empty())
            .unwrap()
    }

    async fn body_bytes(body: Body) -> Bytes {
        body.collect().await.unwrap().to_bytes()
    }

    #[tokio::test]
    async fn first_request_caches_response_and_replay_returns_same_body() {
        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(temp.path()).await;
        let app = Router::new()
            .route("/test", post(|| async { Json(json!({ "created": true })) }))
            .layer(axum::middleware::from_fn_with_state(
                state,
                idempotency_middleware,
            ));

        let first = app.clone().oneshot(idem_request(Some("org-1"), "key-abc")).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let first_bytes = body_bytes(first.into_body()).await;

        let second = app.clone().oneshot(idem_request(Some("org-1"), "key-abc")).await.unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        let second_bytes = body_bytes(second.into_body()).await;

        assert_eq!(first_bytes, second_bytes);
    }

    #[tokio::test]
    async fn replay_is_scoped_by_organization() {
        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(temp.path()).await;
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter2 = counter.clone();
        let app = Router::new()
            .route(
                "/test",
                post(move || {
                    let counter = counter2.clone();
                    async move {
                        let n = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Json(json!({ "n": n }))
                    }
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                state,
                idempotency_middleware,
            ));

        let org_a = app.clone().oneshot(idem_request(Some("org-a"), "shared-key")).await.unwrap();
        assert_eq!(org_a.status(), StatusCode::OK);
        let body_a = body_bytes(org_a.into_body()).await;
        assert_eq!(body_a, Bytes::from_static(br#"{"n":0}"#));

        let org_b = app.clone().oneshot(idem_request(Some("org-b"), "shared-key")).await.unwrap();
        assert_eq!(org_b.status(), StatusCode::OK);
        let body_b = body_bytes(org_b.into_body()).await;
        assert_eq!(body_b, Bytes::from_static(br#"{"n":1}"#));

        // Replay within org-a returns the cached first response, not org-b's.
        let replay_a = app.clone().oneshot(idem_request(Some("org-a"), "shared-key")).await.unwrap();
        assert_eq!(replay_a.status(), StatusCode::OK);
        let replay_a_body = body_bytes(replay_a.into_body()).await;
        assert_eq!(replay_a_body, Bytes::from_static(br#"{"n":0}"#));

        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn in_progress_request_returns_conflict() {
        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(temp.path()).await;
        let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
        let started_tx = std::sync::Arc::new(tokio::sync::Mutex::new(Some(started_tx)));
        let (tx, rx) = tokio::sync::oneshot::channel();
        let rx = std::sync::Arc::new(tokio::sync::Mutex::new(Some(rx)));
        let rx2 = rx.clone();
        let started_tx2 = started_tx.clone();
        let app = Router::new()
            .route(
                "/test",
                post(move || {
                    let rx = rx2.clone();
                    let started_tx = started_tx2.clone();
                    async move {
                        // The handler only runs after the middleware has
                        // reserved the in-flight slot, so signalling here
                        // (instead of sleeping) guarantees the duplicate
                        // below observes the reservation.
                        let _ = started_tx.lock().await.take().unwrap().send(());
                        // Block until the test signals us to complete.
                        let _ = rx.lock().await.take().unwrap().await;
                        Json(json!({ "done": true }))
                    }
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                state,
                idempotency_middleware,
            ));

        let app2 = app.clone();
        let first_handle = tokio::spawn(async move {
            app2.oneshot(idem_request(Some("org-1"), "in-flight")).await.unwrap()
        });

        // Wait for the first request to reserve its in-flight slot. Bounded
        // only as a hang guard, not as a scheduling assumption.
        tokio::time::timeout(std::time::Duration::from_secs(10), started_rx)
            .await
            .expect("first request did not reach the handler (slot never reserved)");

        let duplicate = app.clone().oneshot(idem_request(Some("org-1"), "in-flight")).await.unwrap();
        assert_eq!(duplicate.status(), StatusCode::CONFLICT);

        let _ = tx.send(());
        let first = first_handle.await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn non_mutable_methods_bypass_idempotency() {
        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(temp.path()).await;
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter2 = counter.clone();
        let app = Router::new()
            .route(
                "/test",
                axum::routing::get(move || {
                    let counter = counter2.clone();
                    async move {
                        let n = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Json(json!({ "n": n }))
                    }
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                state,
                idempotency_middleware,
            ));

        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/test")
                    .header("idempotency-key", "get-key")
                    .extension(test_user(Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let second = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/test")
                    .header("idempotency-key", "get-key")
                    .extension(test_user(Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::OK);

        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
}
