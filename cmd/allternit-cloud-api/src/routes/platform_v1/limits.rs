//! Postgres-backed fixed-window rate limiting and idempotency for `/v1`.
//!
//! Rate limits: one counter per key and one per project per 60 s window
//! (`platform_rate_windows`), so limits hold across processes. The limit is the
//! project's `rpm_override` or its plan default. Every response carries
//! `x-ratelimit-limit`, `x-ratelimit-remaining` and `x-ratelimit-reset` (unix
//! seconds when the window ends); a 429 adds `retry-after`.
//!
//! Idempotency: `Idempotency-Key` on a POST is stored per API key for 24 h. The
//! same key and the same request replays the stored response (header
//! `idempotent-replayed: true`); the same key with a different request is a 409
//! `idempotency_conflict`; a retry that arrives while the first request is still
//! running is a 409 `idempotency_in_progress`.

use axum::{
    body::{to_bytes, Body},
    extract::Request,
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use super::{PlatformCaller, PlatformError};

pub const WINDOW_SECS: i64 = 60;
const IDEMPOTENCY_TTL_HOURS: i32 = 24;
const MAX_REQUEST_BODY: usize = 2 * 1024 * 1024;
const MAX_STORED_RESPONSE: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct RateInfo {
    pub limit: u32,
    pub remaining: u32,
    pub reset_unix: i64,
}

impl RateInfo {
    pub fn apply(&self, headers: &mut HeaderMap) {
        let set = |headers: &mut HeaderMap, name: &'static str, value: String| {
            if let Ok(v) = HeaderValue::from_str(&value) {
                headers.insert(HeaderName::from_static(name), v);
            }
        };
        set(headers, "x-ratelimit-limit", self.limit.to_string());
        set(headers, "x-ratelimit-remaining", self.remaining.to_string());
        set(headers, "x-ratelimit-reset", self.reset_unix.to_string());
    }
}

/// Count this request against the key and project windows. `Ok` carries the
/// header values; `Err` is the 429 plus the same info for headers.
pub async fn check_rate_limit(
    db: &PgPool,
    caller: &PlatformCaller,
) -> Result<RateInfo, (PlatformError, RateInfo)> {
    let now = chrono::Utc::now().timestamp();
    let window_start = now - now.rem_euclid(WINDOW_SECS);
    let reset_unix = window_start + WINDOW_SECS;
    let limit = caller.rpm_limit();
    let fail_open = |error: sqlx::Error| {
        // A counter outage must not take the API down; log it and let it through.
        tracing::error!(%error, "platform rate limiter unavailable; allowing request");
        RateInfo { limit, remaining: limit, reset_unix }
    };

    let mut used = 0i32;
    for bucket in [format!("key:{}", caller.key_id), format!("project:{}", caller.project_id)] {
        match incr(db, &bucket, window_start).await {
            Ok(count) => used = used.max(count),
            Err(error) => return Ok(fail_open(error)),
        }
    }
    if rand::random::<u8>() < 3 {
        // ~1% of requests tidy up old windows.
        let _ = sqlx::query(
            "DELETE FROM platform_rate_windows WHERE window_start < NOW() - INTERVAL '1 hour'",
        )
        .execute(db)
        .await;
    }

    let info = RateInfo {
        limit,
        remaining: (limit as i64 - used as i64).max(0) as u32,
        reset_unix,
    };
    if used as i64 > limit as i64 {
        return Err((
            PlatformError::rate_limit(
                "rate_limit_exceeded",
                format!("Rate limit of {limit} requests per minute exceeded. Retry after the window resets."),
            ),
            info,
        ));
    }
    Ok(info)
}

async fn incr(db: &PgPool, bucket: &str, window_start: i64) -> Result<i32, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        INSERT INTO platform_rate_windows (bucket, window_start, count)
        VALUES ($1, to_timestamp($2::float8), 1)
        ON CONFLICT (bucket, window_start)
        DO UPDATE SET count = platform_rate_windows.count + 1
        RETURNING count
        "#,
    )
    .bind(bucket)
    .bind(window_start as f64)
    .fetch_one(db)
    .await
}

fn json_response(status: StatusCode, body: Vec<u8>) -> Response {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

/// Run `next` under the request's `Idempotency-Key`, if it has one.
pub async fn run_idempotent(
    db: &PgPool,
    caller: &PlatformCaller,
    request: Request,
    next: Next,
) -> Response {
    let Some(raw_key) = request.headers().get("idempotency-key") else {
        return next.run(request).await;
    };
    let idem_key = match raw_key.to_str() {
        Ok(k) if !k.is_empty() && k.len() <= 255 => k.to_string(),
        _ => {
            return PlatformError::invalid_request(
                "invalid_idempotency_key",
                "Idempotency-Key must be 1 to 255 visible ASCII characters.",
            )
            .with_param("Idempotency-Key")
            .into_response()
        }
    };

    let (parts, body) = request.into_parts();
    let bytes = match to_bytes(body, MAX_REQUEST_BODY).await {
        Ok(b) => b,
        Err(_) => {
            return PlatformError::invalid_request("body_too_large", "The request body is too large.")
                .into_response()
        }
    };
    let mut hasher = Sha256::new();
    hasher.update(parts.method.as_str().as_bytes());
    hasher.update(b"\n");
    hasher.update(parts.uri.path_and_query().map(|p| p.as_str()).unwrap_or("").as_bytes());
    hasher.update(b"\n");
    hasher.update(&bytes);
    let request_hash = hex::encode(hasher.finalize());

    match claim(db, &caller.key_id, &idem_key, &request_hash).await {
        Ok(Claim::Won) => {}
        Ok(Claim::Replay { status, body }) => {
            let mut response = json_response(status, body);
            response
                .headers_mut()
                .insert("idempotent-replayed", HeaderValue::from_static("true"));
            return response;
        }
        Ok(Claim::Conflict) => {
            return PlatformError::conflict(
                "idempotency_conflict",
                "This Idempotency-Key was already used with a different request.",
            )
            .with_param("Idempotency-Key")
            .into_response()
        }
        Ok(Claim::InProgress) => {
            return PlatformError::conflict(
                "idempotency_in_progress",
                "A request with this Idempotency-Key is still being processed.",
            )
            .with_param("Idempotency-Key")
            .into_response()
        }
        Err(error) => return PlatformError::from(error).into_response(),
    }

    let response = next.run(Request::from_parts(parts, Body::from(bytes))).await;
    let (resp_parts, resp_body) = response.into_parts();

    let streaming = resp_parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.starts_with("text/event-stream"))
        .unwrap_or(false);
    let status = resp_parts.status;
    // 5xx and 429 are transient: let the client retry with the same key.
    let cacheable = !streaming && status.as_u16() < 500 && status != StatusCode::TOO_MANY_REQUESTS;
    if !cacheable {
        let _ = release(db, &caller.key_id, &idem_key).await;
        return Response::from_parts(resp_parts, resp_body);
    }

    let bytes = match to_bytes(resp_body, MAX_STORED_RESPONSE).await {
        Ok(b) => b,
        Err(_) => {
            let _ = release(db, &caller.key_id, &idem_key).await;
            return PlatformError::api_error("response_too_large", "The response could not be stored.")
                .into_response();
        }
    };
    let stored: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    if let Err(error) = sqlx::query(
        "UPDATE platform_idempotency SET status = $3, body = $4 WHERE key_id = $1 AND idem_key = $2",
    )
    .bind(&caller.key_id)
    .bind(&idem_key)
    .bind(status.as_u16() as i32)
    .bind(&stored)
    .execute(db)
    .await
    {
        tracing::error!(%error, "failed to store idempotent response");
        let _ = release(db, &caller.key_id, &idem_key).await;
    }
    Response::from_parts(resp_parts, Body::from(bytes))
}

enum Claim {
    Won,
    Replay { status: StatusCode, body: Vec<u8> },
    Conflict,
    InProgress,
}

async fn claim(db: &PgPool, key_id: &str, idem_key: &str, hash: &str) -> Result<Claim, sqlx::Error> {
    sqlx::query(
        "DELETE FROM platform_idempotency \
         WHERE key_id = $1 AND idem_key = $2 AND created_at < NOW() - make_interval(hours => $3)",
    )
    .bind(key_id)
    .bind(idem_key)
    .bind(IDEMPOTENCY_TTL_HOURS)
    .execute(db)
    .await?;
    if rand::random::<u8>() < 3 {
        let _ = sqlx::query(
            "DELETE FROM platform_idempotency WHERE created_at < NOW() - make_interval(hours => $1)",
        )
        .bind(IDEMPOTENCY_TTL_HOURS)
        .execute(db)
        .await;
    }

    let inserted = sqlx::query(
        "INSERT INTO platform_idempotency (key_id, idem_key, request_hash, status) \
         VALUES ($1, $2, $3, 0) ON CONFLICT (key_id, idem_key) DO NOTHING",
    )
    .bind(key_id)
    .bind(idem_key)
    .bind(hash)
    .execute(db)
    .await?;
    if inserted.rows_affected() == 1 {
        return Ok(Claim::Won);
    }

    let row: Option<(String, i32, Option<serde_json::Value>)> = sqlx::query_as(
        "SELECT request_hash, status, body FROM platform_idempotency WHERE key_id = $1 AND idem_key = $2",
    )
    .bind(key_id)
    .bind(idem_key)
    .fetch_optional(db)
    .await?;
    Ok(match row {
        None => Claim::InProgress,
        Some((stored_hash, _, _)) if stored_hash != hash => Claim::Conflict,
        Some((_, 0, _)) => Claim::InProgress,
        Some((_, status, body)) => {
            let body = match body {
                None | Some(serde_json::Value::Null) => Vec::new(),
                Some(value) => serde_json::to_vec(&value).unwrap_or_default(),
            };
            Claim::Replay {
                status: StatusCode::from_u16(status as u16).unwrap_or(StatusCode::OK),
                body,
            }
        }
    })
}

async fn release(db: &PgPool, key_id: &str, idem_key: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM platform_idempotency WHERE key_id = $1 AND idem_key = $2 AND status = 0")
        .bind(key_id)
        .bind(idem_key)
        .execute(db)
        .await
        .map(|_| ())
}
