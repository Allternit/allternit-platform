//! Channels hybrid relay: a public inbound address for each connected channel
//! (Slack, Telegram, WhatsApp, Teams, Discord) that delivers to the user's
//! runtime over its relay, waking it when it sleeps.
//!
//! 1. The runtime (signed in as the user) asks for an address for one of its
//!    channels: `POST /api/v1/channel-inbound-routes {runtimeId, provider}` →
//!    `https://api.allternit.com/channels/in/<key>`. It sets that as the
//!    platform's webhook. The key is the credential and is stored hashed.
//! 2. The platform posts to `/channels/in/<key>`. Requests that need a live
//!    answer (WhatsApp's GET handshake, Slack's url_verification, every
//!    Discord interaction) are relayed straight through. Everything else is
//!    acknowledged with 200 at once and queued.
//! 3. A worker delivers queued requests in order per address, through
//!    `relay_request_to_runtime` (which wakes a sleeping cloud computer),
//!    retrying with backoff for 24 hours. The runtime verifies the platform
//!    signature itself and dedupes by the platform's message id, so a retry
//!    never doubles a message. `x-allternit-channel-queued-at` tells it when
//!    the request arrived, for platforms that sign a timestamp (Slack).

use axum::{
    body::Bytes,
    extract::{Path, RawQuery, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get},
    Json, Router,
};
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use super::runtime_relay::{relay_request_to_runtime_with, RelayRequest};
use crate::{ApiError, ApiState};

/// Header carrying when cloud-api received a queued request (unix seconds).
pub const QUEUED_AT_HEADER: &str = "x-allternit-channel-queued-at";
/// Give up on a request this long after it arrived.
const GIVE_UP_AFTER_HOURS: i64 = 24;
/// Most requests held for one address before new ones are refused (429).
const MAX_PENDING_PER_ROUTE: i64 = 1000;
/// Largest inbound body accepted.
const MAX_BODY_BYTES: usize = 1024 * 1024;
const WORKER_INTERVAL: Duration = Duration::from_secs(5);

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/channel-inbound-routes", get(list_routes).post(create_route))
        .route("/api/v1/channel-inbound-routes/:id", delete(revoke_route))
        .route("/channels/in/:key", get(inbound).post(inbound))
}

/// The runtime path a provider's events go to. Unknown providers are refused.
pub fn target_path(provider: &str) -> Option<&'static str> {
    match provider {
        "slack" => Some("/webhooks/slack/events"),
        "telegram" => Some("/webhooks/channels/telegram"),
        "whatsapp" => Some("/webhooks/channels/whatsapp"),
        "teams" => Some("/webhooks/channels/teams"),
        "discord" => Some("/webhooks/channels/discord"),
        "sms" => Some("/webhooks/channels/sms"),
        _ => None,
    }
}

/// Headers a platform signs or the runtime needs to verify it. Everything else
/// a public caller sends is dropped.
pub fn channel_headers(headers: &HeaderMap) -> HashMap<String, String> {
    const KEEP: &[&str] = &[
        "content-type",
        "x-telegram-bot-api-secret-token",
        "x-slack-signature",
        "x-slack-request-timestamp",
        "x-slack-retry-num",
        "x-hub-signature-256",
        "x-signature-ed25519",
        "x-signature-timestamp",
        // Teams: the Bot Framework JWT or the outgoing-webhook HMAC.
        "authorization",
    ];
    headers
        .iter()
        .filter_map(|(name, value)| {
            let name = name.as_str().to_ascii_lowercase();
            if !KEEP.contains(&name.as_str()) {
                return None;
            }
            value.to_str().ok().map(|v| (name, v.to_string()))
        })
        .collect()
}

/// Requests the platform expects a real answer to, so they can't be queued.
pub fn needs_live_answer(provider: &str, method: &Method, body: &[u8]) -> bool {
    if method == Method::GET || provider == "discord" {
        return true;
    }
    if provider == "slack" {
        return serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(|t| t == "url_verification"))
            .unwrap_or(false);
    }
    false
}

/// What a delivery attempt's response means.
#[derive(Debug, PartialEq, Eq)]
pub enum Delivery {
    /// The runtime took it (2xx), or refused it for good (signature or shape: 4xx).
    Done,
    /// Try again later: computer waking or offline, runtime error, timeout.
    Retry,
}

pub fn classify(status: u16) -> Delivery {
    match status {
        200..=299 => Delivery::Done,
        408 | 425 | 429 => Delivery::Retry,
        400..=499 => Delivery::Done,
        _ => Delivery::Retry,
    }
}

/// Seconds before attempt `attempts` (1-based) is retried: 5s, 15s, 30s, 1m, 2m, then every 5m.
pub fn backoff_secs(attempts: i32) -> i64 {
    match attempts {
        i32::MIN..=1 => 5,
        2 => 15,
        3 => 30,
        4 => 60,
        5 => 120,
        _ => 300,
    }
}

pub(crate) fn sha256_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

pub(crate) fn new_key() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub(crate) fn public_base() -> String {
    std::env::var("ALLTERNIT_CLOUD_API_URL")
        .unwrap_or_else(|_| "https://api.allternit.com".to_string())
        .trim_end_matches('/')
        .to_string()
}

async fn user_id(state: &ApiState, headers: &HeaderMap) -> Result<String, ApiError> {
    crate::auth::resolve_user_scoped(&state.db, headers, "compute").await.map(|u| u.id)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateRoute {
    runtime_id: String,
    provider: String,
    label: Option<String>,
}

async fn create_route(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(body): Json<CreateRoute>,
) -> Result<Response, ApiError> {
    let user = user_id(&state, &headers).await?;
    if target_path(&body.provider).is_none() {
        return Err(ApiError::BadRequest(format!("Unsupported channel provider: {}", body.provider)));
    }
    let owns: Option<(String,)> = sqlx::query_as(
        "SELECT id FROM runtime_devices WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(&body.runtime_id)
    .bind(&user)
    .fetch_optional(&state.db)
    .await?;
    if owns.is_none() {
        return Err(ApiError::NotFound("Runtime not found".to_string()));
    }
    let key = new_key();
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO channel_inbound_routes (id, key_hash, user_id, runtime_id, provider, label) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(&id)
    .bind(sha256_hex(&key))
    .bind(&user)
    .bind(&body.runtime_id)
    .bind(&body.provider)
    .bind(&body.label)
    .execute(&state.db)
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": id,
            "provider": body.provider,
            "runtimeId": body.runtime_id,
            // Shown once: only its hash is kept.
            "url": format!("{}/channels/in/{}", public_base(), key),
        })),
    )
        .into_response())
}

async fn list_routes(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Response, ApiError> {
    let user = user_id(&state, &headers).await?;
    let rows: Vec<(String, String, String, Option<String>, DateTime<Utc>, Option<DateTime<Utc>>, i64)> = sqlx::query_as(
        "SELECT r.id, r.runtime_id, r.provider, r.label, r.created_at, r.last_inbound_at,
                (SELECT count(*) FROM channel_inbound_queue q
                  WHERE q.route_id = r.id AND q.delivered_at IS NULL AND q.dead_at IS NULL)
           FROM channel_inbound_routes r
          WHERE r.user_id = $1 AND r.revoked_at IS NULL
          ORDER BY r.created_at",
    )
    .bind(&user)
    .fetch_all(&state.db)
    .await?;
    let routes: Vec<_> = rows
        .into_iter()
        .map(|(id, runtime_id, provider, label, created_at, last_inbound_at, pending)| {
            serde_json::json!({
                "id": id, "runtimeId": runtime_id, "provider": provider, "label": label,
                "createdAt": created_at, "lastInboundAt": last_inbound_at, "pending": pending,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "routes": routes })).into_response())
}

async fn revoke_route(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let user = user_id(&state, &headers).await?;
    let done = sqlx::query(
        "UPDATE channel_inbound_routes SET revoked_at = now() WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(&id)
    .bind(&user)
    .execute(&state.db)
    .await?;
    if done.rows_affected() == 0 {
        return Err(ApiError::NotFound("Channel address not found".to_string()));
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

struct Route {
    id: String,
    user_id: String,
    runtime_id: String,
    provider: String,
}

async fn route_for_key(state: &ApiState, key: &str) -> Result<Option<Route>, ApiError> {
    let row: Option<(String, String, String, String)> = sqlx::query_as(
        "SELECT id, user_id, runtime_id, provider FROM channel_inbound_routes WHERE key_hash = $1 AND revoked_at IS NULL",
    )
    .bind(sha256_hex(key))
    .fetch_optional(&state.db)
    .await?;
    Ok(row.map(|(id, user_id, runtime_id, provider)| Route { id, user_id, runtime_id, provider }))
}

fn relay_path(provider: &str, query: &str) -> Option<String> {
    let path = target_path(provider)?;
    Some(if query.is_empty() { path.to_string() } else { format!("{path}?{query}") })
}

async fn relay(
    state: &ApiState,
    route: &Route,
    method: &str,
    query: &str,
    headers: HashMap<String, String>,
    body: &[u8],
    queued_at: Option<i64>,
) -> Result<Response, ApiError> {
    let path = relay_path(&route.provider, query)
        .ok_or_else(|| ApiError::BadRequest("Unsupported channel provider".to_string()))?;
    let mut trusted = HashMap::new();
    if let Some(at) = queued_at {
        trusted.insert(QUEUED_AT_HEADER.to_string(), at.to_string());
    }
    relay_request_to_runtime_with(
        &state.db,
        &state.contabo_runtime_service,
        &state.quota_service,
        &state.provisioning_service,
        &route.user_id,
        &route.runtime_id,
        RelayRequest {
            method: method.to_string(),
            path,
            headers,
            body: base64_encode(body),
            body_encoding: "base64".to_string(),
        },
        channel_header_names(),
        trusted,
    )
    .await
}

fn base64_encode(body: &[u8]) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    STANDARD.encode(body)
}

/// Names the relay passes through for channel deliveries (on top of its own allow-list).
fn channel_header_names() -> &'static [&'static str] {
    &[
        "x-telegram-bot-api-secret-token",
        "x-slack-signature",
        "x-slack-request-timestamp",
        "x-slack-retry-num",
        "x-hub-signature-256",
        "x-signature-ed25519",
        "x-signature-timestamp",
    ]
}

async fn inbound(
    State(state): State<Arc<ApiState>>,
    method: Method,
    Path(key): Path<String>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match inbound_inner(&state, method, &key, query.unwrap_or_default(), &headers, body).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

async fn inbound_inner(
    state: &Arc<ApiState>,
    method: Method,
    key: &str,
    query: String,
    headers: &HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    if body.len() > MAX_BODY_BYTES {
        return Ok(StatusCode::PAYLOAD_TOO_LARGE.into_response());
    }
    let Some(route) = route_for_key(state, key).await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let _ = sqlx::query("UPDATE channel_inbound_routes SET last_inbound_at = now() WHERE id = $1")
        .bind(&route.id)
        .execute(&state.db)
        .await;
    let mut forwarded = channel_headers(headers);
    let mut body = body;
    // SMS: the cloud verifies the carrier signature, dedupes and handles STOP/HELP/START
    // before anything is queued; the runtime gets a normalised, already-verified JSON body.
    let mut sms_seen: Option<(String, String)> = None;
    if route.provider == "sms" {
        if method != Method::POST {
            return Ok(StatusCode::METHOD_NOT_ALLOWED.into_response());
        }
        match super::phone::sms_edge(state, &route.id, key, headers, &body).await {
            Ok(super::phone::Edge::Respond(response)) => return Ok(response),
            Ok(super::phone::Edge::Deliver { body: normalised, number_id, message_id }) => {
                body = Bytes::from(normalised);
                forwarded = HashMap::from([("content-type".to_string(), "application/json".to_string())]);
                sms_seen = Some((number_id, message_id));
            }
            Err(error) => return Ok(error.into_response()),
        }
    }
    if needs_live_answer(&route.provider, &method, &body) {
        return relay(state, &route, method.as_str(), &query, forwarded, &body, None).await;
    }
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM channel_inbound_queue WHERE route_id = $1 AND delivered_at IS NULL AND dead_at IS NULL",
    )
    .bind(&route.id)
    .fetch_one(&state.db)
    .await?;
    if pending >= MAX_PENDING_PER_ROUTE {
        if let Some((number_id, message_id)) = &sms_seen {
            super::phone::forget_inbound(&state.db, number_id, message_id).await;
        }
        return Ok(StatusCode::TOO_MANY_REQUESTS.into_response());
    }
    let queued = sqlx::query(
        "INSERT INTO channel_inbound_queue (route_id, method, query, headers, body) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(&route.id)
    .bind(method.as_str())
    .bind(&query)
    .bind(serde_json::to_value(&forwarded).unwrap_or_default())
    .bind(base64_encode(&body))
    .execute(&state.db)
    .await;
    if let Err(error) = queued {
        if let Some((number_id, message_id)) = &sms_seen {
            super::phone::forget_inbound(&state.db, number_id, message_id).await;
        }
        return Err(error.into());
    }
    // Deliver now rather than at the next tick; the platform already has its 200.
    let state = state.clone();
    let route_id = route.id.clone();
    tokio::spawn(async move {
        if let Err(error) = deliver_route(&state, &route_id).await {
            tracing::warn!(%route_id, "channel delivery pass failed: {error}");
        }
    });
    Ok((StatusCode::OK, "ok").into_response())
}

/// Start the background delivery loop and the 7-day cleanup.
pub fn start_channel_inbound_worker(state: Arc<ApiState>) {
    tokio::spawn(async move {
        let mut ticks: u64 = 0;
        loop {
            if let Err(error) = deliver_due(&state).await {
                tracing::warn!("channel inbound worker: {error}");
            }
            ticks += 1;
            if ticks % 720 == 0 {
                let _ = sqlx::query(
                    "DELETE FROM channel_inbound_queue WHERE (delivered_at IS NOT NULL AND delivered_at < now() - interval '7 days')
                        OR (dead_at IS NOT NULL AND dead_at < now() - interval '7 days')",
                )
                .execute(&state.db)
                .await;
            }
            tokio::time::sleep(WORKER_INTERVAL).await;
        }
    });
}

async fn deliver_due(state: &Arc<ApiState>) -> Result<(), ApiError> {
    let routes: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT route_id FROM channel_inbound_queue
          WHERE delivered_at IS NULL AND dead_at IS NULL AND next_attempt_at <= now()
            AND (locked_until IS NULL OR locked_until < now())
          LIMIT 50",
    )
    .fetch_all(&state.db)
    .await?;
    for (route_id,) in routes {
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(error) = deliver_route(&state, &route_id).await {
                tracing::warn!(%route_id, "channel delivery pass failed: {error}");
            }
        });
    }
    Ok(())
}

/// Deliver one address's due requests oldest first; stop at the first that
/// must wait, so the runtime sees them in the order the platform sent them.
async fn deliver_route(state: &Arc<ApiState>, route_id: &str) -> Result<(), ApiError> {
    loop {
        // Claim the oldest undelivered request for this address, if it is due and unclaimed.
        let claimed: Option<(i64, String, String, serde_json::Value, String, DateTime<Utc>, i32)> = sqlx::query_as(
            "UPDATE channel_inbound_queue SET locked_until = now() + interval '3 minutes', attempts = attempts + 1
              WHERE id = (
                SELECT id FROM channel_inbound_queue
                 WHERE route_id = $1 AND delivered_at IS NULL AND dead_at IS NULL
                 ORDER BY id LIMIT 1 FOR UPDATE SKIP LOCKED)
                AND next_attempt_at <= now() AND (locked_until IS NULL OR locked_until < now())
              RETURNING id, method, query, headers, body, received_at, attempts",
        )
        .bind(route_id)
        .fetch_optional(&state.db)
        .await?;
        let Some((id, method, query, headers, body, received_at, attempts)) = claimed else {
            return Ok(());
        };
        let route: Option<(String, String, String)> = sqlx::query_as(
            "SELECT user_id, runtime_id, provider FROM channel_inbound_routes WHERE id = $1 AND revoked_at IS NULL",
        )
        .bind(route_id)
        .fetch_optional(&state.db)
        .await?;
        let Some((user_id, runtime_id, provider)) = route else {
            sqlx::query("UPDATE channel_inbound_queue SET dead_at = now(), last_error = 'address revoked' WHERE route_id = $1 AND delivered_at IS NULL AND dead_at IS NULL")
                .bind(route_id)
                .execute(&state.db)
                .await?;
            return Ok(());
        };
        let route = Route { id: route_id.to_string(), user_id, runtime_id, provider };
        let headers: HashMap<String, String> = serde_json::from_value(headers).unwrap_or_default();
        let body = {
            use base64::{engine::general_purpose::STANDARD, Engine as _};
            STANDARD.decode(body.as_bytes()).unwrap_or_default()
        };
        let outcome = relay(state, &route, &method, &query, headers, &body, Some(received_at.timestamp())).await;
        let (status, error) = match &outcome {
            Ok(response) => (Some(response.status().as_u16()), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let done = status.map(classify) == Some(Delivery::Done);
        if done {
            sqlx::query("UPDATE channel_inbound_queue SET delivered_at = now(), locked_until = NULL, last_status = $2 WHERE id = $1")
                .bind(id)
                .bind(status.map(i32::from))
                .execute(&state.db)
                .await?;
            continue;
        }
        let give_up = Utc::now() - received_at > chrono::Duration::hours(GIVE_UP_AFTER_HOURS);
        sqlx::query(
            "UPDATE channel_inbound_queue
                SET locked_until = NULL, last_status = $2, last_error = $3,
                    next_attempt_at = now() + make_interval(secs => $4),
                    dead_at = CASE WHEN $5 THEN now() ELSE NULL END
              WHERE id = $1",
        )
        .bind(id)
        .bind(status.map(i32::from))
        .bind(error)
        .bind(backoff_secs(attempts) as f64)
        .bind(give_up)
        .execute(&state.db)
        .await?;
        if give_up {
            continue;
        }
        return Ok(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn only_known_providers_have_a_runtime_path() {
        assert_eq!(target_path("slack"), Some("/webhooks/slack/events"));
        assert_eq!(target_path("telegram"), Some("/webhooks/channels/telegram"));
        assert_eq!(target_path("sms"), Some("/webhooks/channels/sms"));
        assert_eq!(target_path("photon"), None);
        assert_eq!(relay_path("whatsapp", "hub.mode=subscribe"), Some("/webhooks/channels/whatsapp?hub.mode=subscribe".into()));
    }

    #[test]
    fn keeps_only_signature_headers() {
        let mut h = HeaderMap::new();
        h.insert("X-Telegram-Bot-Api-Secret-Token", HeaderValue::from_static("s"));
        h.insert("content-type", HeaderValue::from_static("application/json"));
        h.insert("cookie", HeaderValue::from_static("nope"));
        h.insert("x-allternit-channel-queued-at", HeaderValue::from_static("1"));
        let kept = channel_headers(&h);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept.get("x-telegram-bot-api-secret-token").map(String::as_str), Some("s"));
        assert!(!kept.contains_key(QUEUED_AT_HEADER), "a public caller can't claim a queue time");
    }

    #[test]
    fn live_answers_for_handshakes_and_discord() {
        assert!(needs_live_answer("whatsapp", &Method::GET, b""));
        assert!(needs_live_answer("discord", &Method::POST, b"{}"));
        assert!(needs_live_answer("slack", &Method::POST, br#"{"type":"url_verification","challenge":"x"}"#));
        assert!(!needs_live_answer("slack", &Method::POST, br#"{"type":"event_callback"}"#));
        assert!(!needs_live_answer("telegram", &Method::POST, b"{}"));
    }

    #[test]
    fn delivery_outcomes() {
        assert_eq!(classify(200), Delivery::Done);
        assert_eq!(classify(401), Delivery::Done, "bad signature never gets better");
        assert_eq!(classify(429), Delivery::Retry);
        assert_eq!(classify(503), Delivery::Retry, "waking or offline");
        assert_eq!(classify(504), Delivery::Retry);
        assert_eq!(backoff_secs(1), 5);
        assert_eq!(backoff_secs(4), 60);
        assert_eq!(backoff_secs(40), 300);
    }
}
