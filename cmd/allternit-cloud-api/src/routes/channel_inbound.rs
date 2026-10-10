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

use super::runtime_relay::{relay_request_to_runtime_with, relay_signed_request_to_runtime_with, RelayRequest};
use crate::{ApiError, ApiState};

/// `channel_inbound_routes.provider` of an MCP Events client subscription's queue.
pub const MCP_EVENTS_PROVIDER: &str = "mcp_events";
/// Runtime path relayed MCP Events land on (allternit-api `mcp_events_client`).
pub const MCP_EVENTS_RUNTIME_PATH: &str = "/api/v1/mcp/event-deliveries";
/// Providers whose routes are internal queues, not channel addresses: never
/// listed or revocable as a channel address.
const INTERNAL_PROVIDERS: &[&str] = &[MCP_EVENTS_PROVIDER];

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
        // Bot email: the platform's mailflare webhook, HMAC-verified by the runtime.
        "email" => Some("/api/v1/agent-email/inbound"),
        // Shared Allternit Discord app: cloud-built envelopes (routes::discord_app).
        "discord_app" => Some("/webhooks/channels/discord-app"),
        // MCP Events from a user's connected app, verified by cloud-api
        // (routes::mcp_event_callbacks) and relayed as a signed envelope.
        MCP_EVENTS_PROVIDER => Some(MCP_EVENTS_RUNTIME_PATH),
        _ => None,
    }
}

/// Headers a platform signs or the runtime needs to verify it. Everything else
/// a public caller sends is dropped. One list for both storing a queued event
/// and relaying it, so a header the runtime verifies can't be kept on the way
/// in and dropped on the way out (that dropped Telnyx's signature and 401'd
/// every relayed text).
const CHANNEL_HEADERS: &[&str] = &[
    "content-type",
    "x-telegram-bot-api-secret-token",
    "x-slack-signature",
    "x-slack-request-timestamp",
    "x-slack-retry-num",
    "x-hub-signature-256",
    "x-signature-ed25519",
    "x-signature-timestamp",
    // SMS (Telnyx Ed25519).
    "telnyx-signature-ed25519",
    "telnyx-timestamp",
    // Teams: the Bot Framework JWT or the outgoing-webhook HMAC.
    "authorization",
    // Email: mailflare's HMAC of the webhook body (same name the runtime's
    // verify_mailflare_signature checks).
    "x-email-platform-signature",
];

/// The [`CHANNEL_HEADERS`] a caller sent, lowercased.
pub fn channel_headers(headers: &HeaderMap) -> HashMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            let name = name.as_str().to_ascii_lowercase();
            if !CHANNEL_HEADERS.contains(&name.as_str()) {
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
    /// The runtime took it (2xx). Only this counts as delivered.
    Done,
    /// Try again later: computer waking or offline, runtime error, timeout, or a 404
    /// (the number or call may not have synced to the runtime yet; see [`is_dead`]).
    Retry,
    /// The runtime refused it for good (auth, signature or shape: 4xx). Never delivered:
    /// marked dead with the error so the loss is visible.
    Dead,
}

/// Attempts a runtime 404 gets before the event is given up on.
pub const NOT_FOUND_MAX_ATTEMPTS: i32 = 5;

pub fn classify(status: u16) -> Delivery {
    match status {
        200..=299 => Delivery::Done,
        404 | 408 | 425 | 429 => Delivery::Retry,
        400..=499 => Delivery::Dead,
        _ => Delivery::Retry,
    }
}

/// Whether an attempt that was not delivered ends the event: a definite refusal, a 404 that
/// outlasted [`NOT_FOUND_MAX_ATTEMPTS`] tries (the backoff gives a number time to sync), or
/// an event older than the give-up age.
pub fn is_dead(status: Option<u16>, attempts: i32, too_old: bool) -> bool {
    too_old
        || match status.map(classify) {
            Some(Delivery::Dead) => true,
            Some(Delivery::Retry) => status == Some(404) && attempts >= NOT_FOUND_MAX_ATTEMPTS,
            _ => false,
        }
}

/// The `last_error` to store for an attempt that was not delivered: the transport error, else the status.
pub fn attempt_error(status: Option<u16>, error: Option<String>) -> Option<String> {
    error.or_else(|| status.map(|s| format!("runtime answered {s}")))
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
          WHERE r.user_id = $1 AND r.revoked_at IS NULL AND r.provider <> ALL($2)
          ORDER BY r.created_at",
    )
    .bind(&user)
    .bind(INTERNAL_PROVIDERS)
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
        "UPDATE channel_inbound_routes SET revoked_at = now() WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL AND provider <> ALL($3)",
    )
    .bind(&id)
    .bind(&user)
    .bind(INTERNAL_PROVIDERS)
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
    let request = RelayRequest {
        method: method.to_string(),
        path,
        headers,
        body: base64_encode(body),
        body_encoding: "base64".to_string(),
    };
    if is_trusted_envelope(&route.provider) {
        relay_signed_request_to_runtime_with(
            &state.db,
            &state.contabo_runtime_service,
            &state.quota_service,
            &state.provisioning_service,
            &route.user_id,
            &route.runtime_id,
            request,
            channel_header_names(),
            trusted,
        )
        .await
    } else {
        relay_request_to_runtime_with(
            &state.db,
            &state.contabo_runtime_service,
            &state.quota_service,
            &state.provisioning_service,
            &route.user_id,
            &route.runtime_id,
            request,
            channel_header_names(),
            trusted,
        )
        .await
    }
}

/// Providers whose runtime path trusts cloud-api rather than a platform
/// signature (cloud-built envelopes, or the mailflare webhook cloud-api
/// already checked). Their relays are signed with the runtime's device-token
/// key; the runtime verifies with `relay_auth::RelayedAuth`.
pub fn is_trusted_envelope(provider: &str) -> bool {
    matches!(provider, "discord_app" | "email" | "sms" | MCP_EVENTS_PROVIDER)
}

fn base64_encode(body: &[u8]) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    STANDARD.encode(body)
}

/// Names the relay passes through for channel deliveries (on top of its own allow-list).
fn channel_header_names() -> &'static [&'static str] {
    CHANNEL_HEADERS
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
    // A Platform API number: same verification, dedupe and STOP/HELP/START as above,
    // then the cloud keeps the text and sends it to the developer as a webhook.
    // Nothing is queued for a runtime (these numbers have none).
    if route.provider == "platform_sms" {
        if method != Method::POST {
            return Ok(StatusCode::METHOD_NOT_ALLOWED.into_response());
        }
        return Ok(match super::phone::sms_edge(state, &route.id, key, headers, &body).await {
            Ok(super::phone::Edge::Respond(response)) => response,
            Ok(super::phone::Edge::Deliver { body: normalised, number_id, message_id }) => {
                match super::platform_v1::messages::record_inbound(&state.db, &number_id, &normalised).await {
                    Ok(()) => StatusCode::OK.into_response(),
                    Err(e) => {
                        // Let the carrier retry: undo the dedupe mark.
                        tracing::error!(number = %number_id, "platform inbound text not kept: {e:?}");
                        super::phone::forget_inbound(&state.db, &number_id, &message_id).await;
                        StatusCode::SERVICE_UNAVAILABLE.into_response()
                    }
                }
            }
            Err(error) => error.into_response(),
        });
    }
    if route.provider == "whatsapp" {
        use super::whatsapp_es::Edge;
        match super::whatsapp_es::edge(state, &route.id, &method, &query, &forwarded, &body).await? {
            Edge::Respond(response) => return Ok(response),
            Edge::Resign(signature) => {
                forwarded.insert("x-hub-signature-256".to_string(), signature);
            }
            Edge::Passthrough => {}
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

/// A queue route nobody holds a key for (the key is dropped here), for a
/// producer inside cloud-api that queues to a runtime itself
/// ([`enqueue_internal`]). `/channels/in/:key` can never reach it.
pub(crate) async fn create_internal_route(
    db: &sqlx::PgPool,
    user_id: &str,
    runtime_id: &str,
    provider: &str,
    label: &str,
) -> Result<String, sqlx::Error> {
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO channel_inbound_routes (id, key_hash, user_id, runtime_id, provider, label) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(&id)
    .bind(sha256_hex(&new_key()))
    .bind(user_id)
    .bind(runtime_id)
    .bind(provider)
    .bind(label)
    .execute(db)
    .await?;
    Ok(id)
}

/// Requests still waiting on `route_id`.
pub(crate) async fn pending_count(db: &sqlx::PgPool, route_id: &str) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM channel_inbound_queue WHERE route_id = $1 AND delivered_at IS NULL AND dead_at IS NULL",
    )
    .bind(route_id)
    .fetch_one(db)
    .await
}

/// Whether `route_id` already holds [`MAX_PENDING_PER_ROUTE`] requests.
pub(crate) async fn route_is_full(db: &sqlx::PgPool, route_id: &str) -> Result<bool, sqlx::Error> {
    Ok(pending_count(db, route_id).await? >= MAX_PENDING_PER_ROUTE)
}

/// Deliver `route_id`'s queue now in the background (the worker would reach it within its next tick).
pub(crate) fn deliver_soon(state: &Arc<ApiState>, route_id: &str) {
    let (state, route_id) = (state.clone(), route_id.to_string());
    tokio::spawn(async move {
        if let Err(error) = deliver_route(&state, &route_id).await {
            tracing::warn!(%route_id, "queued delivery pass failed: {error}");
        }
    });
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
                // MCP Events dedupe marks outlive any sender's retry window by days.
                let _ = sqlx::query("DELETE FROM mcp_event_client_seen WHERE seen_at < now() - interval '7 days'")
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
pub(crate) async fn deliver_route(state: &Arc<ApiState>, route_id: &str) -> Result<(), ApiError> {
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
            // A connector event is not a channel message: the runtime decides what (if anything) to tell the owner.
            if route.provider != MCP_EVENTS_PROVIDER {
                super::web_push::notify_channel_message(&state.db, &route.user_id, &route.id, &route.provider, &route.runtime_id);
            }
            continue;
        }
        let error = attempt_error(status, error);
        let give_up = is_dead(status, attempts, Utc::now() - received_at > chrono::Duration::hours(GIVE_UP_AFTER_HOURS));
        if give_up {
            tracing::warn!(route_id = %route.id, queue_id = id, provider = %route.provider, ?status, attempts, error = ?error, "channel event dead: not delivered to the runtime");
        }
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
        assert_eq!(target_path("email"), Some("/api/v1/agent-email/inbound"));
        assert_eq!(target_path("photon"), None);
        assert_eq!(relay_path("whatsapp", "hub.mode=subscribe"), Some("/webhooks/channels/whatsapp?hub.mode=subscribe".into()));
    }

    #[test]
    fn keeps_only_signature_headers() {
        let mut h = HeaderMap::new();
        h.insert("X-Telegram-Bot-Api-Secret-Token", HeaderValue::from_static("s"));
        h.insert("X-Email-Platform-Signature", HeaderValue::from_static("sig"));
        h.insert("content-type", HeaderValue::from_static("application/json"));
        h.insert("cookie", HeaderValue::from_static("nope"));
        h.insert("x-allternit-channel-queued-at", HeaderValue::from_static("1"));
        let kept = channel_headers(&h);
        assert_eq!(kept.len(), 3);
        assert_eq!(kept.get("x-telegram-bot-api-secret-token").map(String::as_str), Some("s"));
        assert_eq!(kept.get("x-email-platform-signature").map(String::as_str), Some("sig"));
        assert!(!kept.contains_key(QUEUED_AT_HEADER), "a public caller can't claim a queue time");
    }

    #[test]
    fn relay_passes_every_stored_signature_header() {
        // The runtime re-verifies Telnyx's signature; dropping it 401'd every text.
        for name in ["telnyx-signature-ed25519", "telnyx-timestamp", "x-email-platform-signature", "x-slack-signature"] {
            assert!(channel_header_names().contains(&name), "{name} must reach the runtime");
        }
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
        assert_eq!(classify(401), Delivery::Dead, "bad signature never gets better, and is never reported delivered");
        assert_eq!(classify(403), Delivery::Dead);
        assert_eq!(classify(404), Delivery::Retry, "the number or call may not have synced yet");
        assert_eq!(classify(429), Delivery::Retry);
        assert_eq!(classify(503), Delivery::Retry, "waking or offline");
        assert_eq!(classify(504), Delivery::Retry);
        assert!(!is_dead(Some(404), 1, false) && !is_dead(Some(404), NOT_FOUND_MAX_ATTEMPTS - 1, false));
        assert!(is_dead(Some(404), NOT_FOUND_MAX_ATTEMPTS, false), "a 404 that never heals goes dead");
        assert!(is_dead(Some(401), 1, false) && is_dead(Some(403), 1, false), "auth failures die at once");
        assert!(!is_dead(Some(503), 50, false) && !is_dead(None, 50, false), "outages keep retrying");
        assert!(is_dead(Some(503), 50, true), "until the give-up age");
        assert_eq!(attempt_error(Some(404), None).as_deref(), Some("runtime answered 404"));
        assert_eq!(attempt_error(None, Some("timeout".into())).as_deref(), Some("timeout"));
        assert_eq!(backoff_secs(1), 5);
        assert_eq!(backoff_secs(4), 60);
        assert_eq!(backoff_secs(40), 300);
    }
}
