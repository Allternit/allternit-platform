//! MCP Events client, cloud half (SPEC-allternit-events §2 item 6, P6):
//! the public webhook receiver for events from the external MCP servers a
//! user connected (Gmail, GitHub, Linear, ...).
//!
//! The user's runtime owns each subscription (it holds the connector's
//! credentials, mints the `whsec_` secret and calls `events/subscribe`). It
//! registers the subscription here first, so the receiver can answer the
//! server's verification challenge during that `events/subscribe` call.
//!
//! ## Runtime → cloud (relay-signed, like `POST /api/v1/runtime/events`)
//!
//! `x-allternit-runtime-id`, `x-allternit-runtime-ts`, `x-allternit-runtime-sig`
//! over `(method, path, body)`; see `routes::runtime_events`.
//!
//! * `PUT /api/v1/runtime/mcp-event-subscriptions/:key` body
//!   `{"secret":"whsec_…","connectorId":"…","eventName":"…"}` → `200
//!   {"key","callbackUrl","status":"active"}`. Idempotent. A different secret
//!   rotates: the old one keeps verifying for [`ROTATION_GRACE`]. Re-activates
//!   an ended or terminated row. `key` must be `sub_` + 24 hex (the runtime's
//!   `mcp_protocol::events::subscription_id`); a key owned by another user is 409.
//! * `DELETE /api/v1/runtime/mcp-event-subscriptions/:key` → `204`; the
//!   receiver answers 410 from then on and queued events are dropped.
//!
//! ## External server → cloud (public)
//!
//! `POST /mcp/events/callback/:key`, Standard Webhooks signed with the
//! subscription's secret:
//!
//! | case | answer |
//! |---|---|
//! | body > 256 KiB | 413 |
//! | unknown key, or not active | 410 (the server stops) |
//! | signature missing / wrong / timestamp older than 5 min | 401 |
//! | not JSON | 400 |
//! | `{"type":"verification","challenge":c}` | 200 `{"challenge":c}` |
//! | an eventId (or `webhook-id`) already accepted | 200 `{"status":"duplicate"}` |
//! | queue for this subscription full (1000) | 429 |
//! | event or `terminated` envelope | queued, 200 `{"status":"accepted"}` |
//!
//! Queued items reach the runtime in order over the channels relay queue
//! (`channel_inbound`: Postgres, 24 h backoff, wakes a sleeping computer) as
//! a relay-signed `POST /api/v1/mcp/event-deliveries` with body
//! `{"subscriptionKey","webhookId","receivedAt","event":<the server's JSON>}`.
//! A `terminated` envelope also ends the row here (later deliveries get 410).

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{post, put},
    Json, Router,
};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use mcp_protocol::{events as ev, webhooks as sw};

use super::channel_inbound::{self, MCP_EVENTS_PROVIDER};
use super::runtime_events::authenticate_runtime;
use crate::ApiState;

pub const CALLBACK_PREFIX: &str = "/mcp/events/callback";
pub const RUNTIME_PREFIX: &str = "/api/v1/runtime/mcp-event-subscriptions";
/// After a rotation the previous secret keeps verifying this long.
pub const ROTATION_GRACE: ChronoDuration = ChronoDuration::hours(1);

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route(&format!("{CALLBACK_PREFIX}/:key"), post(callback_route))
        .route(&format!("{RUNTIME_PREFIX}/:key"), put(register_route).delete(remove_route))
}

/// The public callback URL for `key`.
pub fn callback_url(key: &str) -> String {
    format!("{}{CALLBACK_PREFIX}/{key}", channel_inbound::public_base())
}

/// `sub_` + 24 lowercase hex: the shape `subscription_id` produces.
pub fn valid_key(key: &str) -> bool {
    key.strip_prefix("sub_").is_some_and(|h| h.len() == 24 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

fn json_error(status: StatusCode, error: &str) -> Response {
    (status, Json(json!({ "error": error }))).into_response()
}

fn seal(state: &ApiState, plaintext: &str) -> String {
    match &state.credential_cipher {
        Some(cipher) => cipher.encrypt(plaintext).unwrap_or_else(|_| plaintext.to_string()),
        None => plaintext.to_string(),
    }
}

fn open(state: &ApiState, stored: &str) -> String {
    match &state.credential_cipher {
        Some(cipher) => cipher.decrypt(stored).unwrap_or_else(|_| stored.to_string()),
        None => stored.to_string(),
    }
}

// ---------------------------------------------------------------- runtime registration

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Register {
    secret: String,
    connector_id: String,
    event_name: String,
}

async fn register_route(State(state): State<Arc<ApiState>>, Path(key): Path<String>, headers: HeaderMap, body: Bytes) -> Response {
    let path = format!("{RUNTIME_PREFIX}/{key}");
    let (user, runtime) = match authenticate_runtime(&state.db, &headers, "PUT", &path, &body, Utc::now().timestamp()).await {
        Ok(who) => who,
        Err(r) => return r,
    };
    if !valid_key(&key) {
        return json_error(StatusCode::BAD_REQUEST, "invalid_key");
    }
    let Ok(reg) = serde_json::from_slice::<Register>(&body) else {
        return json_error(StatusCode::BAD_REQUEST, "invalid_body");
    };
    if sw::parse_secret(&reg.secret).is_err() || reg.connector_id.is_empty() || reg.event_name.is_empty() || reg.event_name.len() > 200 {
        return json_error(StatusCode::BAD_REQUEST, "invalid_body");
    }
    match register(&state, &user, &runtime, &key, &reg.secret, &reg.connector_id, &reg.event_name).await {
        Ok(Ok(())) => Json(json!({ "key": key, "callbackUrl": callback_url(&key), "status": "active" })).into_response(),
        Ok(Err(conflict)) => json_error(StatusCode::CONFLICT, conflict),
        Err(e) => {
            tracing::error!(%key, "mcp event subscription register failed: {e}");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "internal")
        }
    }
}

/// Upsert one subscription. `Ok(Err(_))` = the key belongs to someone else.
pub async fn register(
    state: &ApiState,
    user: &str,
    runtime: &str,
    key: &str,
    secret: &str,
    connector_id: &str,
    event_name: &str,
) -> Result<Result<(), &'static str>, sqlx::Error> {
    let db = &state.db;
    let existing: Option<(String, String, String, String)> =
        sqlx::query_as("SELECT user_id, runtime_id, route_id, secret FROM mcp_event_client_subscriptions WHERE key = $1")
            .bind(key)
            .fetch_optional(db)
            .await?;
    let Some((owner, _old_runtime, route_id, stored)) = existing else {
        let route_id = channel_inbound::create_internal_route(db, user, runtime, MCP_EVENTS_PROVIDER, &format!("mcp-events:{key}")).await?;
        sqlx::query(
            "INSERT INTO mcp_event_client_subscriptions (key, user_id, runtime_id, route_id, connector_id, event_name, secret)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(key)
        .bind(user)
        .bind(runtime)
        .bind(&route_id)
        .bind(connector_id)
        .bind(event_name)
        .bind(seal(state, secret))
        .execute(db)
        .await?;
        return Ok(Ok(()));
    };
    if owner != user {
        return Ok(Err("key_taken"));
    }
    // The route must point at this runtime and be live (a re-paired runtime, or one revoked on removal).
    let live: Option<(String,)> = sqlx::query_as(
        "SELECT id FROM channel_inbound_routes WHERE id = $1 AND runtime_id = $2 AND revoked_at IS NULL",
    )
    .bind(&route_id)
    .bind(runtime)
    .fetch_optional(db)
    .await?;
    let route_id = match live {
        Some((id,)) => id,
        None => {
            // Retire the old queue (it pointed at another runtime) so its leftovers aren't relayed there.
            sqlx::query("UPDATE channel_inbound_routes SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
                .bind(&route_id)
                .execute(db)
                .await?;
            channel_inbound::create_internal_route(db, user, runtime, MCP_EVENTS_PROVIDER, &format!("mcp-events:{key}")).await?
        }
    };
    let rotated = open(state, &stored) != secret;
    sqlx::query(
        "UPDATE mcp_event_client_subscriptions SET
            runtime_id = $2, route_id = $3, connector_id = $4, event_name = $5,
            previous_secret = CASE WHEN $6 THEN secret ELSE previous_secret END,
            previous_secret_until = CASE WHEN $6 THEN now() + make_interval(secs => $8) ELSE previous_secret_until END,
            secret = CASE WHEN $6 THEN $7 ELSE secret END,
            status = 'active', ended_at = NULL, updated_at = now()
          WHERE key = $1",
    )
    .bind(key)
    .bind(runtime)
    .bind(&route_id)
    .bind(connector_id)
    .bind(event_name)
    .bind(rotated)
    .bind(seal(state, secret))
    .bind(ROTATION_GRACE.num_seconds() as f64)
    .execute(db)
    .await?;
    Ok(Ok(()))
}

async fn remove_route(State(state): State<Arc<ApiState>>, Path(key): Path<String>, headers: HeaderMap, body: Bytes) -> Response {
    let path = format!("{RUNTIME_PREFIX}/{key}");
    let (user, _runtime) = match authenticate_runtime(&state.db, &headers, "DELETE", &path, &body, Utc::now().timestamp()).await {
        Ok(who) => who,
        Err(r) => return r,
    };
    match remove(&state.db, &user, &key).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!(%key, "mcp event subscription remove failed: {e}");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "internal")
        }
    }
}

/// End `key` for `user` and revoke its queue route. Idempotent; `false` when nothing was live.
pub async fn remove(db: &PgPool, user: &str, key: &str) -> Result<bool, sqlx::Error> {
    let route: Option<(String,)> = sqlx::query_as(
        "UPDATE mcp_event_client_subscriptions SET status = 'ended', ended_at = now(), updated_at = now()
          WHERE key = $1 AND user_id = $2 AND status <> 'ended' RETURNING route_id",
    )
    .bind(key)
    .bind(user)
    .fetch_optional(db)
    .await?;
    let Some((route_id,)) = route else { return Ok(false) };
    sqlx::query("UPDATE channel_inbound_routes SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
        .bind(&route_id)
        .execute(db)
        .await?;
    Ok(true)
}

// ---------------------------------------------------------------- receiver

/// What the receiver did with one POST (kept apart from HTTP so tests drive it).
#[derive(Debug, PartialEq, Eq)]
pub enum Received {
    TooLarge,
    Gone,
    Unauthorized(&'static str),
    BadRequest,
    Challenge(String),
    Duplicate,
    Full,
    /// Queued on this route.
    Accepted { route_id: String },
}

impl IntoResponse for Received {
    fn into_response(self) -> Response {
        match self {
            Received::TooLarge => json_error(StatusCode::PAYLOAD_TOO_LARGE, "too_large"),
            Received::Gone => json_error(StatusCode::GONE, "subscription_not_found"),
            Received::Unauthorized(why) => json_error(StatusCode::UNAUTHORIZED, why),
            Received::BadRequest => json_error(StatusCode::BAD_REQUEST, "invalid_body"),
            Received::Challenge(c) => Json(json!({ "challenge": c })).into_response(),
            Received::Duplicate => Json(json!({ "status": "duplicate" })).into_response(),
            Received::Full => json_error(StatusCode::TOO_MANY_REQUESTS, "queue_full"),
            Received::Accepted { .. } => Json(json!({ "status": "accepted" })).into_response(),
        }
    }
}

async fn callback_route(State(state): State<Arc<ApiState>>, Path(key): Path<String>, headers: HeaderMap, body: Bytes) -> Response {
    match receive(&state, &key, &headers, &body, Utc::now()).await {
        Ok(Received::Accepted { route_id }) => {
            channel_inbound::deliver_soon(&state, &route_id);
            Received::Accepted { route_id }.into_response()
        }
        Ok(r) => r.into_response(),
        Err(e) => {
            tracing::error!(%key, "mcp event callback failed: {e}");
            // 5xx: the server retries, and the dedupe mark was not written.
            json_error(StatusCode::SERVICE_UNAVAILABLE, "unavailable")
        }
    }
}

/// Verify, dedupe and queue one delivery.
pub async fn receive(state: &ApiState, key: &str, headers: &HeaderMap, body: &[u8], now: DateTime<Utc>) -> Result<Received, sqlx::Error> {
    if body.len() > ev::MAX_EVENT_BYTES {
        return Ok(Received::TooLarge);
    }
    if !valid_key(key) {
        return Ok(Received::Gone);
    }
    let db = &state.db;
    let row: Option<(String, String, Option<String>, Option<DateTime<Utc>>, String)> = sqlx::query_as(
        "SELECT route_id, secret, previous_secret, previous_secret_until, event_name FROM mcp_event_client_subscriptions WHERE key = $1 AND status = 'active'",
    )
    .bind(key)
    .fetch_optional(db)
    .await?;
    let Some((route_id, secret, previous, previous_until, event_name)) = row else { return Ok(Received::Gone) };

    let h = |n: &str| headers.get(n).and_then(|v| v.to_str().ok()).map(str::trim).filter(|v| !v.is_empty());
    let (Some(msg_id), Some(ts), Some(sig)) = (h(sw::HEADER_ID), h(sw::HEADER_TIMESTAMP), h(sw::HEADER_SIGNATURE)) else {
        return Ok(Received::Unauthorized("signature_required"));
    };
    let mut keys = vec![open(state, &secret)];
    if let (Some(p), Some(until)) = (previous, previous_until) {
        if until > now {
            keys.push(open(state, &p));
        }
    }
    let mut verdict = Err(sw::VerifyError::NoMatchingSignature);
    for k in keys.iter().filter_map(|s| sw::parse_secret(s).ok()) {
        verdict = sw::verify(&k, msg_id, ts, sig, body, now.timestamp());
        if verdict.is_ok() || matches!(verdict, Err(sw::VerifyError::Expired | sw::VerifyError::BadTimestamp)) {
            break;
        }
    }
    match verdict {
        Ok(()) => {}
        Err(sw::VerifyError::Expired) => return Ok(Received::Unauthorized("stale_timestamp")),
        Err(sw::VerifyError::BadTimestamp) => return Ok(Received::Unauthorized("bad_timestamp")),
        Err(sw::VerifyError::NoMatchingSignature) => return Ok(Received::Unauthorized("invalid_signature")),
    }

    let Ok(event) = serde_json::from_slice::<Value>(body) else { return Ok(Received::BadRequest) };
    if !event.is_object() {
        return Ok(Received::BadRequest);
    }
    let kind = event.get("type").and_then(Value::as_str);
    if kind == Some("verification") {
        let Some(challenge) = event.get("challenge").and_then(Value::as_str) else { return Ok(Received::BadRequest) };
        let _ = sqlx::query("UPDATE mcp_event_client_subscriptions SET verified_at = now() WHERE key = $1").bind(key).execute(db).await;
        return Ok(Received::Challenge(challenge.to_string()));
    }
    let event_id = if kind == Some("terminated") {msg_id.to_string()} else {
        if kind.is_some() || event.get("name").and_then(Value::as_str)!=Some(event_name.as_str()) {return Ok(Received::BadRequest)}
        let Some(id)=event.get("eventId").and_then(Value::as_str).filter(|id|!id.is_empty()&&id.len()<=200&&*id==msg_id) else{return Ok(Received::BadRequest)};
        if event.get("timestamp").and_then(Value::as_str).is_none_or(|at|DateTime::parse_from_rfc3339(at).is_err()) || event.get("data").is_none() {return Ok(Received::BadRequest)}
        id.to_string()
    };
    if channel_inbound::route_is_full(db, &route_id).await? {
        return Ok(Received::Full);
    }

    // Mark seen and queue in one transaction: a failure leaves neither, so the server's retry is accepted.
    let mut tx = db.begin().await?;
    let fresh = sqlx::query("INSERT INTO mcp_event_client_seen (subscription_key, event_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(key)
        .bind(&event_id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        == 1;
    if !fresh {
        return Ok(Received::Duplicate);
    }
    let envelope = json!({
        "subscriptionKey": key,
        "webhookId": msg_id,
        "receivedAt": now.to_rfc3339(),
        "event": event,
    });
    let body = serde_json::to_vec(&envelope).unwrap_or_default();
    sqlx::query("INSERT INTO channel_inbound_queue (route_id, method, query, headers, body) VALUES ($1, 'POST', '', $2, $3)")
        .bind(&route_id)
        .bind(json!({ "content-type": "application/json" }))
        .bind(b64(&body))
        .execute(&mut *tx)
        .await?;
    let terminated = kind == Some("terminated");
    sqlx::query(
        "UPDATE mcp_event_client_subscriptions SET
            last_event_at = CASE WHEN $2 THEN last_event_at ELSE now() END,
            event_count = event_count + CASE WHEN $2 THEN 0 ELSE 1 END,
            status = CASE WHEN $2 THEN 'terminated' ELSE status END,
            ended_at = CASE WHEN $2 THEN now() ELSE ended_at END,
            updated_at = now()
          WHERE key = $1",
    )
    .bind(key)
    .bind(terminated)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let _ = sqlx::query("UPDATE channel_inbound_routes SET last_inbound_at = now() WHERE id = $1").bind(&route_id).execute(db).await;
    Ok(Received::Accepted { route_id })
}

fn b64(body: &[u8]) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    STANDARD.encode(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::runtime_pairing::sha256_hex;
    use crate::routes::runtime_relay::sign_runtime_request;
    use crate::routes::test_support::{events_backbone_schema, seed_runtime_device, test_state, MockGateway};
    use axum::body::Body;
    use axum::http::Request;
    use base64::Engine as _;
    use tower::ServiceExt;

    const USER: &str = "user-1";
    const RT: &str = "rt-1";

    async fn state() -> Arc<ApiState> {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        events_backbone_schema(&state.db).await;
        sqlx::raw_sql(&include_str!("../../migrations_pg/059_mcp_event_client_subscriptions.sql").replace("public.", ""))
            .execute(&state.db)
            .await
            .expect("059 applies");
        seed_runtime_device(&state.db, RT, USER).await;
        seed_runtime_device(&state.db, "rt-2", "user-2").await;
        state
    }

    fn secret(n: u8) -> String {
        format!("whsec_{}", base64::engine::general_purpose::STANDARD.encode([n; 32]))
    }

    fn key() -> String {
        ev::subscription_id(USER, "https://mail.example.com/mcp", "email.received", &json!({}))
    }

    /// A runtime → cloud request signed with `runtime`'s device key.
    fn runtime_req(runtime: &str, method: &str, path: &str, body: &[u8]) -> Request<Body> {
        let ts = Utc::now().timestamp();
        let relay_key = sha256_hex(format!("token-of-{runtime}").as_bytes());
        Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .header("x-allternit-runtime-id", runtime)
            .header("x-allternit-runtime-ts", ts.to_string())
            .header("x-allternit-runtime-sig", sign_runtime_request(&relay_key, ts, method, path, body))
            .body(Body::from(body.to_vec()))
            .unwrap()
    }

    async fn put(state: &Arc<ApiState>, runtime: &str, key: &str, secret: &str) -> (StatusCode, Value) {
        let body = serde_json::to_vec(&json!({ "secret": secret, "connectorId": "conn-1", "eventName": "email.received" })).unwrap();
        let res = routes().with_state(state.clone()).oneshot(runtime_req(runtime, "PUT", &format!("{RUNTIME_PREFIX}/{key}"), &body)).await.unwrap();
        let status = res.status();
        let v = serde_json::from_slice(&axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap()).unwrap_or(Value::Null);
        (status, v)
    }

    fn signed(secret: &str, msg_id: &str, ts: i64, body: &[u8]) -> HeaderMap {
        let key = sw::parse_secret(secret).unwrap();
        let mut h = HeaderMap::new();
        for (k, v) in sw::headers(&key, msg_id, ts, body) {
            h.insert(k, v.parse().unwrap());
        }
        h
    }

    fn event(id: &str) -> Vec<u8> {
        serde_json::to_vec(&ev::event_envelope(id, "email.received", "2026-10-06T10:00:00Z", json!({ "subject": "hi" }), None)).unwrap()
    }

    async fn deliver(state: &Arc<ApiState>, key: &str, secret: &str, msg_id: &str, body: &[u8]) -> Received {
        receive(state, key, &signed(secret, msg_id, Utc::now().timestamp(), body), body, Utc::now()).await.unwrap()
    }

    async fn queued(state: &Arc<ApiState>, route: &str) -> Vec<Value> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT body FROM channel_inbound_queue WHERE route_id = $1 ORDER BY id")
            .bind(route)
            .fetch_all(&state.db)
            .await
            .unwrap();
        rows.into_iter().map(|(b,)| serde_json::from_slice(&base64::engine::general_purpose::STANDARD.decode(b).unwrap()).unwrap()).collect()
    }

    /// Runtime registers → server's challenge is answered → a signed event is
    /// verified, queued for the runtime's relay and attempted over it.
    #[tokio::test]
    async fn register_challenge_event_queue_relay() {
        let state = state().await;
        let key = key();
        let (status, v) = put(&state, RT, &key, &secret(1)).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        assert_eq!(v["callbackUrl"], callback_url(&key));
        assert!(v["callbackUrl"].as_str().unwrap().ends_with(&format!("/mcp/events/callback/{key}")));

        // Verification challenge (signed) is echoed through the public router.
        let challenge = serde_json::to_vec(&ev::verification_envelope("ch_123")).unwrap();
        let mut req = Request::builder().method("POST").uri(format!("{CALLBACK_PREFIX}/{key}"));
        for (k, val) in signed(&secret(1), "msg_v", Utc::now().timestamp(), &challenge).iter() {
            req = req.header(k, val);
        }
        let res = routes().with_state(state.clone()).oneshot(req.body(Body::from(challenge)).unwrap()).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let echoed = axum::body::to_bytes(res.into_body(), 4096).await.unwrap();
        assert!(ev::challenge_echoed(&echoed, "ch_123"));

        // An event is queued as the runtime envelope, on an internal mcp_events route.
        let Received::Accepted { route_id } = deliver(&state, &key, &secret(1), "evt_1", &event("evt_1")).await else { panic!("accepted") };
        let (provider, runtime): (String, String) = sqlx::query_as("SELECT provider, runtime_id FROM channel_inbound_routes WHERE id = $1").bind(&route_id).fetch_one(&state.db).await.unwrap();
        assert_eq!((provider.as_str(), runtime.as_str()), (MCP_EVENTS_PROVIDER, RT));
        let q = queued(&state, &route_id).await;
        assert_eq!(q.len(), 1);
        assert_eq!(q[0]["subscriptionKey"], key.as_str());
        assert_eq!(q[0]["webhookId"], "evt_1");
        assert_eq!(q[0]["event"]["eventId"], "evt_1");
        assert_eq!(q[0]["event"]["data"]["subject"], "hi");
        assert_eq!(channel_inbound::target_path(MCP_EVENTS_PROVIDER), Some("/api/v1/mcp/event-deliveries"));
        assert!(channel_inbound::is_trusted_envelope(MCP_EVENTS_PROVIDER), "relayed signed with the device key");

        // The relay worker attempts it (runtime offline in this harness → kept for retry, not dropped).
        channel_inbound::deliver_route(&state, &route_id).await.unwrap();
        let (attempts, dead, delivered): (i32, Option<DateTime<Utc>>, Option<DateTime<Utc>>) =
            sqlx::query_as("SELECT attempts, dead_at, delivered_at FROM channel_inbound_queue WHERE route_id = $1").bind(&route_id).fetch_one(&state.db).await.unwrap();
        assert_eq!(attempts, 1);
        assert!(dead.is_none() && delivered.is_none(), "an offline runtime is retried for 24 h");
        let (count, last): (i64, Option<DateTime<Utc>>) = sqlx::query_as("SELECT event_count, last_event_at FROM mcp_event_client_subscriptions WHERE key = $1").bind(&key).fetch_one(&state.db).await.unwrap();
        assert_eq!(count, 1);
        assert!(last.is_some());
    }

    #[tokio::test]
    async fn bad_signatures_and_stale_timestamps_are_rejected() {
        let state = state().await;
        let key = key();
        put(&state, RT, &key, &secret(1)).await;
        let body = event("evt_1");
        let now = Utc::now();
        // Wrong secret, tampered body, no headers, 6 min old.
        assert_eq!(deliver(&state, &key, &secret(2), "m", &body).await, Received::Unauthorized("invalid_signature"));
        let h = signed(&secret(1), "m", now.timestamp(), &body);
        assert_eq!(receive(&state, &key, &h, &event("evt_2"), now).await.unwrap(), Received::Unauthorized("invalid_signature"));
        assert_eq!(receive(&state, &key, &HeaderMap::new(), &body, now).await.unwrap(), Received::Unauthorized("signature_required"));
        let old = signed(&secret(1), "m", now.timestamp() - 360, &body);
        assert_eq!(receive(&state, &key, &old, &body, now).await.unwrap(), Received::Unauthorized("stale_timestamp"));
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM channel_inbound_queue").fetch_one(&state.db).await.unwrap();
        assert_eq!(n, 0, "nothing unverified is queued");
        // Too big is refused before anything else.
        let big = vec![b' '; ev::MAX_EVENT_BYTES + 1];
        assert_eq!(receive(&state, &key, &HeaderMap::new(), &big, now).await.unwrap(), Received::TooLarge);
    }

    #[tokio::test]
    async fn replays_are_deduped_by_event_id() {
        let state = state().await;
        let key = key();
        put(&state, RT, &key, &secret(1)).await;
        assert!(matches!(deliver(&state, &key, &secret(1), "evt_1", &event("evt_1")).await, Received::Accepted { .. }));
        // Retries preserve wire identity; a correctly signed mismatching ID is refused.
        assert_eq!(deliver(&state, &key, &secret(1), "evt_1", &event("evt_1")).await, Received::Duplicate);
        assert_eq!(deliver(&state, &key, &secret(1), "msg_9", &event("evt_1")).await, Received::BadRequest);
        assert!(matches!(deliver(&state, &key, &secret(1), "evt_2", &event("evt_2")).await, Received::Accepted { .. }));
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM channel_inbound_queue").fetch_one(&state.db).await.unwrap();
        assert_eq!(n, 2);
    }

    #[tokio::test]
    async fn unknown_removed_and_terminated_subscriptions_answer_410() {
        let state = state().await;
        let key = key();
        assert_eq!(deliver(&state, &key, &secret(1), "m", &event("e")).await, Received::Gone);
        assert_eq!(deliver(&state, "not-a-key", &secret(1), "m", &event("e")).await, Received::Gone);
        put(&state, RT, &key, &secret(1)).await;
        // Removal by the runtime (signed DELETE) → 410 and its route is revoked.
        let res = routes().with_state(state.clone()).oneshot(runtime_req(RT, "DELETE", &format!("{RUNTIME_PREFIX}/{key}"), b"")).await.unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        assert_eq!(deliver(&state, &key, &secret(1), "m", &event("e")).await, Received::Gone);
        // Re-registering re-activates; a `terminated` envelope is relayed once, then 410.
        put(&state, RT, &key, &secret(1)).await;
        let term = serde_json::to_vec(&ev::terminated_envelope(&key, ev::codes::FORBIDDEN, json!({ "reason": "approval_revoked" }))).unwrap();
        let Received::Accepted { route_id } = deliver(&state, &key, &secret(1), "msg_t", &term).await else { panic!("terminated is relayed") };
        assert_eq!(queued(&state, &route_id).await[0]["event"]["type"], "terminated");
        assert_eq!(deliver(&state, &key, &secret(1), "m2", &event("e2")).await, Received::Gone);
        let r = Received::Gone.into_response();
        assert_eq!(r.status(), StatusCode::GONE);
    }

    #[tokio::test]
    async fn rotation_keeps_the_old_secret_for_the_grace_window() {
        let state = state().await;
        let key = key();
        put(&state, RT, &key, &secret(1)).await;
        put(&state, RT, &key, &secret(2)).await;
        assert!(matches!(deliver(&state, &key, &secret(2), "a", &event("a")).await, Received::Accepted { .. }));
        assert!(matches!(deliver(&state, &key, &secret(1), "b", &event("b")).await, Received::Accepted { .. }), "old secret within grace");
        sqlx::query("UPDATE mcp_event_client_subscriptions SET previous_secret_until = now() - interval '1 second'").execute(&state.db).await.unwrap();
        assert_eq!(deliver(&state, &key, &secret(1), "c", &event("c")).await, Received::Unauthorized("invalid_signature"));
        // Same secret again is not a rotation (previous stays as it was).
        put(&state, RT, &key, &secret(2)).await;
        assert!(matches!(deliver(&state, &key, &secret(2), "d", &event("d")).await, Received::Accepted { .. }));
    }

    #[tokio::test]
    async fn registration_is_owner_bound_and_signed() {
        let state = state().await;
        let key = key();
        assert_eq!(put(&state, RT, &key, &secret(1)).await.0, StatusCode::OK);
        assert_eq!(put(&state, "rt-2", &key, &secret(3)).await.0, StatusCode::CONFLICT, "another user's runtime can't take a key");
        assert_eq!(put(&state, RT, "sub_short", &secret(1)).await.0, StatusCode::BAD_REQUEST);
        assert_eq!(put(&state, RT, &key, "whsec_short").await.0, StatusCode::BAD_REQUEST);
        // Unsigned / signed for another path.
        let body = serde_json::to_vec(&json!({ "secret": secret(1), "connectorId": "c", "eventName": "e" })).unwrap();
        let unsigned = Request::builder().method("PUT").uri(format!("{RUNTIME_PREFIX}/{key}")).body(Body::from(body.clone())).unwrap();
        assert_eq!(routes().with_state(state.clone()).oneshot(unsigned).await.unwrap().status(), StatusCode::UNAUTHORIZED);
        let mut wrong = runtime_req(RT, "PUT", &format!("{RUNTIME_PREFIX}/sub_000000000000000000000000"), &body);
        *wrong.uri_mut() = format!("{RUNTIME_PREFIX}/{key}").parse().unwrap();
        assert_eq!(routes().with_state(state.clone()).oneshot(wrong).await.unwrap().status(), StatusCode::UNAUTHORIZED);
        // The internal route never shows up as a channel address.
        let shown: i64 = sqlx::query_scalar("SELECT count(*) FROM channel_inbound_routes WHERE provider <> ALL($1)")
            .bind(&[MCP_EVENTS_PROVIDER][..])
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!(shown, 0);
    }

    #[test]
    fn keys_have_the_subscription_id_shape() {
        assert!(valid_key(&key()));
        assert!(!valid_key("sub_XYZ") && !valid_key("sub_") && !valid_key("../etc") && !valid_key(&format!("{}0", key())));
    }

    /// The whole cloud-api router builds with these routes merged (no axum overlap panic).
    #[tokio::test]
    async fn full_router_builds_with_the_receiver() {
        let state = state().await;
        let app = crate::create_router(state.clone());
        let res = app
            .oneshot(Request::builder().method("POST").uri(format!("{CALLBACK_PREFIX}/{}", key())).body(Body::from("{}")).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::GONE, "public, reachable without a session");
    }
}
