//! Share a chat thread by link.
//!
//! A share is a snapshot the client publishes; the public page reads it from
//! here, so the link works while the owner's runtime is offline.
//!
//! * `POST /api/v2/thread-shares` (signed in): body `{sessionId, title, snapshot}`
//!   (at most 2 MB). Answers `{token, url, updatedAt}`: 201 for a new share,
//!   200 when the caller already shares that session (same token, new snapshot).
//! * `PUT /api/v2/thread-shares/:token` (owner): body `{snapshot, title?}`,
//!   re-snapshots and answers `{token, url, updatedAt}`.
//! * `DELETE /api/v2/thread-shares/:token` (owner): deletes the row, 204. The
//!   link is dead from then on.
//! * `GET /api/v2/thread-shares?sessionId=` (signed in): the caller's share of
//!   that session, `{token, url, updatedAt}`, or 404.
//! * `GET /api/v2/public/threads/:token` (no auth, public rate limit):
//!   `{title, messages, updatedAt}`; 404 `{"error":"not_found"}` when unknown.
//!   No-store, noindex.
//!
//! A token that is unknown or belongs to someone else answers 404 to the owner
//! routes, so a token's existence is not revealed.
//!
//! Privacy is enforced at write time: [`sanitize_snapshot`] rebuilds every
//! message from an allowlist (same stance as `floor_shares::sanitize_projection`),
//! so tool inputs and outputs, ids, system prompts and anything else a client
//! sends are never stored.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use base64::Engine;
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};

use super::artifacts_v2::{caller, ts};
use crate::ApiState;

pub const MAX_BODY: usize = 2 * 1024 * 1024;
/// The extractor allows a little over [`MAX_BODY`] so a body just past the cap
/// gets the handler's JSON `too_large`; far bigger ones get axum's plain 413.
const EXTRACT_LIMIT: usize = MAX_BODY + 64 * 1024;
pub const MAX_TEXT_CHARS: usize = 20_000;
pub const MAX_IMAGE_BYTES: usize = 300 * 1024;
pub const MAX_IMAGES: usize = 10;
pub const MAX_TOOL_NAME_CHARS: usize = 80;
const MAX_TOOLS_PER_MESSAGE: usize = 50;
const MAX_MESSAGES: usize = 2_000;
const MAX_TITLE_CHARS: usize = 300;
const MAX_SESSION_ID_CHARS: usize = 256;
const IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/webp", "image/gif"];
const DEFAULT_PUBLIC_BASE_URL: &str = "https://ai.allternit.com/t";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route(
            "/api/v2/thread-shares",
            post(create_share).get(lookup_share).layer(DefaultBodyLimit::max(EXTRACT_LIMIT)),
        )
        .route(
            "/api/v2/thread-shares/:token",
            put(update_share).delete(delete_share).layer(DefaultBodyLimit::max(EXTRACT_LIMIT)),
        )
        .route("/api/v2/public/threads/:token", get(public_thread))
}

// ---------------------------------------------------------------------------
// Sanitizer
// ---------------------------------------------------------------------------

/// A `data:image/<png|jpeg|webp|gif>;base64,` URL whose decoded bytes are at
/// most [`MAX_IMAGE_BYTES`].
fn safe_image(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("data:") else { return false };
    let Some((mime, payload)) = rest.split_once(";base64,") else { return false };
    if !IMAGE_TYPES.contains(&mime) || payload.is_empty() || payload.len() > MAX_IMAGE_BYTES.div_ceil(3) * 4 {
        return false;
    }
    base64::engine::general_purpose::STANDARD
        .decode(payload)
        .is_ok_and(|bytes| !bytes.is_empty() && bytes.len() <= MAX_IMAGE_BYTES)
}

/// A tool entry may be a name or an object with a `name`; only the name is kept.
fn tool_name(v: &Value) -> Option<String> {
    let raw = v.as_str().or_else(|| v.get("name").and_then(Value::as_str))?;
    let name: String = raw.trim().chars().take(MAX_TOOL_NAME_CHARS).collect();
    (!name.is_empty()).then_some(name)
}

/// Rebuilds a thread from an allowlist. `input` is an array of messages; each
/// kept message is exactly `{role, text, images, tools}`:
///
/// * `role`: `user` or `assistant`; any other role drops the message.
/// * `text`: a string, cut to [`MAX_TEXT_CHARS`] characters.
/// * `images`: data URLs (png, jpeg, webp, gif) of at most [`MAX_IMAGE_BYTES`]
///   each, [`MAX_IMAGES`] across the whole thread; the rest are dropped.
/// * `tools`: tool names only, each cut to [`MAX_TOOL_NAME_CHARS`].
///
/// Every other field is dropped, and a message left with nothing to show is
/// dropped too.
pub fn sanitize_snapshot(input: &Value) -> Vec<Value> {
    let mut images_left = MAX_IMAGES;
    let Some(messages) = input.as_array() else { return Vec::new() };
    messages
        .iter()
        .filter_map(|m| {
            let role = m["role"].as_str().filter(|r| matches!(*r, "user" | "assistant"))?;
            let text: String = m["text"].as_str().map(|t| t.chars().take(MAX_TEXT_CHARS).collect()).unwrap_or_default();
            let images: Vec<&str> = m["images"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).filter(|s| safe_image(s)).collect())
                .unwrap_or_default();
            let images: Vec<&str> = images.into_iter().take(images_left).collect();
            images_left -= images.len();
            let tools: Vec<String> = m["tools"]
                .as_array()
                .map(|a| a.iter().filter_map(tool_name).take(MAX_TOOLS_PER_MESSAGE).collect())
                .unwrap_or_default();
            if text.trim().is_empty() && images.is_empty() && tools.is_empty() {
                return None;
            }
            Some(json!({ "role": role, "text": text, "images": images, "tools": tools }))
        })
        .take(MAX_MESSAGES)
        .collect()
}

fn clean_title(v: &Value) -> String {
    v.as_str().map(|s| s.trim().chars().take(MAX_TITLE_CHARS).collect()).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// 160 random bits, URL-safe base64 without padding (27 characters).
fn new_token() -> String {
    let mut b = [0u8; 20];
    rand::thread_rng().fill_bytes(&mut b);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

fn plausible_token(token: &str) -> bool {
    (16..=128).contains(&token.len()) && token.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

/// `THREAD_SHARES_PUBLIC_BASE_URL` (default `https://ai.allternit.com/t`), the
/// same pattern as `ARTIFACTS_PUBLIC_BASE_URL` for `/a/:id`.
fn public_url(token: &str) -> String {
    let base = std::env::var("THREAD_SHARES_PUBLIC_BASE_URL")
        .ok()
        .map(|b| b.trim().trim_end_matches('/').to_string())
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| DEFAULT_PUBLIC_BASE_URL.to_string());
    format!("{base}/{token}")
}

fn json_error(status: StatusCode, error: &str) -> Response {
    (status, Json(json!({ "error": error }))).into_response()
}

fn no_store(mut response: Response) -> Response {
    let h = response.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    h.insert("x-robots-tag", HeaderValue::from_static("noindex"));
    response
}

fn internal(context: &str, e: sqlx::Error) -> Response {
    tracing::error!("thread share {context}: {e}");
    json_error(StatusCode::INTERNAL_SERVER_ERROR, "internal")
}

fn share_json(token: &str, updated_at: DateTime<Utc>) -> Value {
    json!({ "token": token, "url": public_url(token), "updatedAt": ts(updated_at) })
}

async fn user_id(state: &ApiState, headers: &HeaderMap) -> Result<String, Response> {
    caller(state, headers).await.map(|c| c.id).map_err(IntoResponse::into_response)
}

struct Snapshot {
    messages: Value,
    count: i32,
    bytes: i32,
}

/// Parses a request body and sanitizes `snapshot`. An empty result is a 422.
fn parse_snapshot(parsed: &Value) -> Result<Snapshot, Response> {
    if !parsed["snapshot"].is_array() {
        return Err(json_error(StatusCode::BAD_REQUEST, "bad_body"));
    }
    let messages = sanitize_snapshot(&parsed["snapshot"]);
    if messages.is_empty() {
        return Err(json_error(StatusCode::UNPROCESSABLE_ENTITY, "empty_snapshot"));
    }
    let count = messages.len() as i32;
    let messages = Value::Array(messages);
    let bytes = messages.to_string().len() as i32;
    Ok(Snapshot { messages, count, bytes })
}

fn parse_body(body: &Bytes) -> Result<Value, Response> {
    if body.len() > MAX_BODY {
        return Err(json_error(StatusCode::PAYLOAD_TOO_LARGE, "too_large"));
    }
    serde_json::from_slice::<Value>(body)
        .ok()
        .filter(Value::is_object)
        .ok_or_else(|| json_error(StatusCode::BAD_REQUEST, "bad_body"))
}

// ---------------------------------------------------------------------------
// Owner routes
// ---------------------------------------------------------------------------

async fn create_share(State(state): State<Arc<ApiState>>, headers: HeaderMap, body: Bytes) -> Response {
    let user = match user_id(&state, &headers).await {
        Ok(u) => u,
        Err(r) => return r,
    };
    let parsed = match parse_body(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(session_id) = parsed["sessionId"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.chars().count() <= MAX_SESSION_ID_CHARS)
    else {
        return json_error(StatusCode::BAD_REQUEST, "bad_session_id");
    };
    let snap = match parse_snapshot(&parsed) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let res = sqlx::query_as::<_, (String, DateTime<Utc>, bool)>(
        "INSERT INTO thread_shares (token, user_id, session_id, title, snapshot, message_count, bytes) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         ON CONFLICT (user_id, session_id) DO UPDATE SET title = EXCLUDED.title, snapshot = EXCLUDED.snapshot, \
         message_count = EXCLUDED.message_count, bytes = EXCLUDED.bytes, updated_at = now() \
         RETURNING token, updated_at, (xmax = 0)",
    )
    .bind(new_token())
    .bind(&user)
    .bind(session_id)
    .bind(clean_title(&parsed["title"]))
    .bind(&snap.messages)
    .bind(snap.count)
    .bind(snap.bytes)
    .fetch_one(&state.db)
    .await;
    match res {
        Ok((token, updated_at, inserted)) => {
            let status = if inserted { StatusCode::CREATED } else { StatusCode::OK };
            no_store((status, Json(share_json(&token, updated_at))).into_response())
        }
        Err(e) => internal("create", e),
    }
}

async fn update_share(
    State(state): State<Arc<ApiState>>,
    Path(token): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let user = match user_id(&state, &headers).await {
        Ok(u) => u,
        Err(r) => return r,
    };
    if !plausible_token(&token) {
        return json_error(StatusCode::NOT_FOUND, "not_found");
    }
    let parsed = match parse_body(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let snap = match parse_snapshot(&parsed) {
        Ok(s) => s,
        Err(r) => return r,
    };
    // A missing title keeps the stored one.
    let title = parsed.get("title").filter(|t| t.is_string()).map(clean_title);
    let res = sqlx::query_scalar::<_, DateTime<Utc>>(
        "UPDATE thread_shares SET snapshot = $3, message_count = $4, bytes = $5, \
         title = COALESCE($6, title), updated_at = now() \
         WHERE token = $1 AND user_id = $2 RETURNING updated_at",
    )
    .bind(&token)
    .bind(&user)
    .bind(&snap.messages)
    .bind(snap.count)
    .bind(snap.bytes)
    .bind(title)
    .fetch_optional(&state.db)
    .await;
    match res {
        Ok(Some(updated_at)) => no_store(Json(share_json(&token, updated_at)).into_response()),
        Ok(None) => json_error(StatusCode::NOT_FOUND, "not_found"),
        Err(e) => internal("update", e),
    }
}

async fn delete_share(State(state): State<Arc<ApiState>>, Path(token): Path<String>, headers: HeaderMap) -> Response {
    let user = match user_id(&state, &headers).await {
        Ok(u) => u,
        Err(r) => return r,
    };
    if !plausible_token(&token) {
        return json_error(StatusCode::NOT_FOUND, "not_found");
    }
    match sqlx::query("DELETE FROM thread_shares WHERE token = $1 AND user_id = $2")
        .bind(&token)
        .bind(&user)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => json_error(StatusCode::NOT_FOUND, "not_found"),
        Err(e) => internal("delete", e),
    }
}

#[derive(Deserialize)]
struct LookupQuery {
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
}

async fn lookup_share(State(state): State<Arc<ApiState>>, headers: HeaderMap, Query(q): Query<LookupQuery>) -> Response {
    let user = match user_id(&state, &headers).await {
        Ok(u) => u,
        Err(r) => return r,
    };
    let Some(session_id) = q.session_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return json_error(StatusCode::BAD_REQUEST, "bad_session_id");
    };
    let res = sqlx::query_as::<_, (String, DateTime<Utc>)>(
        "SELECT token, updated_at FROM thread_shares WHERE user_id = $1 AND session_id = $2",
    )
    .bind(&user)
    .bind(session_id)
    .fetch_optional(&state.db)
    .await;
    match res {
        Ok(Some((token, updated_at))) => no_store(Json(share_json(&token, updated_at)).into_response()),
        Ok(None) => no_store(json_error(StatusCode::NOT_FOUND, "not_found")),
        Err(e) => internal("lookup", e),
    }
}

// ---------------------------------------------------------------------------
// GET /api/v2/public/threads/:token (no auth)
// ---------------------------------------------------------------------------

async fn public_thread(State(state): State<Arc<ApiState>>, Path(token): Path<String>) -> Response {
    let found = if plausible_token(&token) {
        sqlx::query_as::<_, (String, Value, DateTime<Utc>)>(
            "SELECT title, snapshot, updated_at FROM thread_shares WHERE token = $1",
        )
        .bind(&token)
        .fetch_optional(&state.db)
        .await
    } else {
        Ok(None)
    };
    no_store(match found {
        Ok(Some((title, messages, updated_at))) => {
            Json(json!({ "title": title, "messages": messages, "updatedAt": ts(updated_at) })).into_response()
        }
        Ok(None) => json_error(StatusCode::NOT_FOUND, "not_found"),
        Err(e) => internal("public", e),
    })
}

#[cfg(test)]
#[path = "thread_shares_tests.rs"]
mod tests;
