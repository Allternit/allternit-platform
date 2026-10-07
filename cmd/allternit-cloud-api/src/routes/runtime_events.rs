//! `POST /api/v1/runtime/events` — runtimes forward their `bot_events`
//! ledger up to the cloud event backbone (SPEC-allternit-events §2.4).
//!
//! ## Contract (the runtime forwarder builds against exactly this)
//!
//! **Auth** — the runtime relay signature, in the runtime → cloud direction,
//! keyed with the same device-token key cloud-api uses to sign relayed
//! requests down to the runtime (`runtime_relay::sign_runtime_request`,
//! allternit-api `relay_auth`):
//!
//! ```text
//! x-allternit-runtime-id:  <runtime_devices.id this runtime paired as>
//! x-allternit-runtime-ts:  <unix seconds, within ±300 s of the cloud's clock>
//! x-allternit-runtime-sig: v1=<hex HMAC-SHA256(relay_key, "<ts>.POST./api/v1/runtime/events.<hex sha256(raw body)>")>
//! relay_key = lowercase hex sha256(device_token), used as its ASCII bytes
//!             (allternit-api: relay_auth::relay_key_from_device_token)
//! ```
//!
//! The device must be live (not revoked, credential not expired). The key of a
//! just-rotated credential is accepted during its grace window. The owner is
//! the runtime's paired user; the body can't name another one.
//!
//! **Body** (≤ 1 MiB, ≤ 100 events):
//!
//! ```json
//! { "events": [ { "id": "<ledger id, unique per runtime>", "type": "channel.message.received",
//!                 "at": "2026-10-05T12:00:00Z", "bot_id": "b1", "thread_id": "t1", "data": { … } } ] }
//! ```
//!
//! `id` (1–128 chars) is the idempotency key, scoped to the runtime: sending
//! the same id again is a `duplicate`, never a second event. `type` is the
//! ledger name; [`allternit_events::from_runtime`] maps it onto the registry
//! (`channel.message.received` → `message.received`, `agent.approval.requested`
//! → `approval.requested`, `run.completed` → `agent.run.completed`, registry
//! names pass through). Unmapped types (and our own outbound echoes) are
//! `ignored`, not errors, so the forwarder can send its whole outbox cursor.
//! `at` is optional (RFC 3339; default now). `bot_id` / `thread_id` are copied
//! into `data` (unless `data` already has them) because subscriptions filter
//! on `data`. `data` must be a JSON object of at most 250 KiB serialized
//! (`too_large` otherwise; MCP deliveries are capped at 256 KiB).
//!
//! **Answer** `200 {"results":[{"id","status","eventId"?}], "accepted", "duplicates", "ignored", "rejected"}`
//! with `status` ∈ `accepted | duplicate | ignored | too_large | invalid`, one
//! per input event, in order. The forwarder advances its cursor past every
//! result (a `too_large`/`invalid` event will never be accepted). `401` bad or
//! missing signature, `400` malformed body, `413` body too big; on `5xx` the
//! forwarder retries the same batch (idempotent).

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use super::allternit_events;
use super::runtime_relay::sign_runtime_request;
use crate::ApiState;

pub const PATH: &str = "/api/v1/runtime/events";
pub const RUNTIME_ID_HEADER: &str = "x-allternit-runtime-id";
const SIG_HEADER: &str = "x-allternit-runtime-sig";
const TS_HEADER: &str = "x-allternit-runtime-ts";
pub const MAX_SKEW_SECS: i64 = 300;
pub const MAX_BODY_BYTES: usize = 1024 * 1024;
pub const MAX_EVENTS: usize = 100;
/// Leaves room for the delivery envelope inside the 256 KiB MCP cap.
pub const MAX_DATA_BYTES: usize = 250 * 1024;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route(PATH, post(ingest_route))
}

#[derive(Debug, Deserialize)]
struct Batch {
    events: Vec<Value>,
}

#[derive(Debug, Deserialize)]
struct RuntimeEvent {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    at: Option<String>,
    #[serde(default)]
    bot_id: Option<String>,
    #[serde(default)]
    thread_id: Option<String>,
    #[serde(default)]
    data: Option<Value>,
}

fn json_error(status: StatusCode, error: &str) -> Response {
    (status, Json(json!({ "error": error }))).into_response()
}

/// The relay keys a runtime may sign with: its current credential hash, plus
/// the previous one while its rotation grace is open. `None` = unknown,
/// revoked or expired runtime.
async fn runtime_keys(db: &PgPool, runtime_id: &str) -> Result<Option<(String, Vec<String>)>, sqlx::Error> {
    let row: Option<(Option<String>, Option<String>, Option<DateTime<Utc>>, Option<String>)> = sqlx::query_as(
        "SELECT user_id, credential_hash, credential_expires_at, status FROM runtime_devices WHERE id = $1 AND revoked_at IS NULL",
    )
    .bind(runtime_id)
    .fetch_optional(db)
    .await?;
    let Some((Some(user_id), Some(hash), expires, status)) = row else { return Ok(None) };
    if expires.is_some_and(|e| e <= Utc::now()) || status.as_deref() == Some("revoked") {
        return Ok(None);
    }
    let mut keys = vec![hash];
    // Older schemas (and test fixtures) may not have the rotation columns.
    let previous: Result<Option<(Option<String>, Option<DateTime<Utc>>)>, _> =
        sqlx::query_as("SELECT previous_credential_hash, previous_credential_expires_at FROM runtime_devices WHERE id = $1")
            .bind(runtime_id)
            .fetch_optional(db)
            .await;
    if let Ok(Some((Some(prev), Some(until)))) = previous {
        if until > Utc::now() {
            keys.push(prev);
        }
    }
    Ok(Some((user_id, keys)))
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Verify the signature headers against the runtime's keys. Returns the owner.
async fn authenticate(db: &PgPool, headers: &HeaderMap, body: &[u8], now: i64) -> Result<(String, String), Response> {
    authenticate_runtime(db, headers, "POST", PATH, body, now).await
}

/// Verify a runtime → cloud request signed for `(method, path, body)` (the
/// scheme above, any runtime → cloud route). Returns `(owner, runtime_id)`.
pub(crate) async fn authenticate_runtime(
    db: &PgPool,
    headers: &HeaderMap,
    method: &str,
    path: &str,
    body: &[u8],
    now: i64,
) -> Result<(String, String), Response> {
    let h = |n: &str| headers.get(n).and_then(|v| v.to_str().ok()).map(str::trim).filter(|v| !v.is_empty());
    let unauthorized = |why: &str| json_error(StatusCode::UNAUTHORIZED, why);
    let (Some(runtime_id), Some(ts), Some(sig)) = (h(RUNTIME_ID_HEADER), h(TS_HEADER), h(SIG_HEADER)) else {
        return Err(unauthorized("runtime_signature_required"));
    };
    let ts: i64 = ts.parse().map_err(|_| unauthorized("bad_timestamp"))?;
    if (now - ts).abs() > MAX_SKEW_SECS {
        return Err(unauthorized("stale_timestamp"));
    }
    let keys = match runtime_keys(db, runtime_id).await {
        Ok(Some(k)) => k,
        Ok(None) => return Err(unauthorized("invalid_runtime")),
        Err(e) => {
            tracing::error!("runtime events: device lookup failed: {e}");
            return Err(json_error(StatusCode::INTERNAL_SERVER_ERROR, "internal"));
        }
    };
    let (user_id, keys) = keys;
    let ok = keys.iter().any(|k| ct_eq(sign_runtime_request(k, ts, method, path, body).as_bytes(), sig.as_bytes()));
    if !ok {
        return Err(unauthorized("invalid_signature"));
    }
    Ok((user_id, runtime_id.to_string()))
}

/// Ingest one batch for an authenticated runtime. Pure of HTTP so tests can drive it.
pub async fn ingest(db: &PgPool, user_id: &str, runtime_id: &str, events: Vec<Value>) -> Result<Value, sqlx::Error> {
    let source = format!("runtime:{runtime_id}");
    let mut results = Vec::with_capacity(events.len());
    let (mut accepted, mut duplicates, mut ignored, mut rejected) = (0, 0, 0, 0);
    for raw in events {
        let raw_id = raw.get("id").and_then(Value::as_str).map(str::to_string);
        let Ok(ev) = serde_json::from_value::<RuntimeEvent>(raw) else {
            rejected += 1;
            results.push(json!({ "id": raw_id, "status": "invalid" }));
            continue;
        };
        let at = match ev.at.as_deref() {
            None => None,
            Some(s) => match DateTime::parse_from_rfc3339(s) {
                Ok(t) => Some(t.with_timezone(&Utc)),
                Err(_) => {
                    rejected += 1;
                    results.push(json!({ "id": ev.id, "status": "invalid" }));
                    continue;
                }
            },
        };
        let mut data = match ev.data {
            None | Some(Value::Null) => json!({}),
            Some(Value::Object(m)) => Value::Object(m),
            Some(_) => {
                rejected += 1;
                results.push(json!({ "id": ev.id, "status": "invalid" }));
                continue;
            }
        };
        if ev.id.is_empty() || ev.id.len() > 128 || ev.kind.is_empty() {
            rejected += 1;
            results.push(json!({ "id": ev.id, "status": "invalid" }));
            continue;
        }
        let Some(name) = allternit_events::from_runtime(&ev.kind, &data) else {
            ignored += 1;
            results.push(json!({ "id": ev.id, "status": "ignored" }));
            continue;
        };
        for (key, value) in [("bot_id", ev.bot_id), ("thread_id", ev.thread_id)] {
            if let Some(v) = value {
                data.as_object_mut().expect("object").entry(key).or_insert(json!(v));
            }
        }
        if serde_json::to_vec(&data).map(|b| b.len()).unwrap_or(usize::MAX) > MAX_DATA_BYTES {
            rejected += 1;
            results.push(json!({ "id": ev.id, "status": "too_large" }));
            continue;
        }
        match allternit_events::emit_user_event(db, user_id, name, &data, at, &source, Some(&ev.id)).await? {
            Some(event_id) => {
                platform_fan_out(db, user_id, name, &data).await?;
                accepted += 1;
                results.push(json!({ "id": ev.id, "status": "accepted", "eventId": event_id }));
            }
            None => {
                duplicates += 1;
                results.push(json!({ "id": ev.id, "status": "duplicate" }));
            }
        }
    }
    Ok(json!({ "results": results, "accepted": accepted, "duplicates": duplicates, "ignored": ignored, "rejected": rejected }))
}

/// A Platform API project's hosted runtime (owner `platform:<project>`): the
/// project's webhooks get the event too, for the account of the agent it came
/// from (`bot_id` = the agent id). The account comes from our own agent row,
/// never from the runtime, and an event with no agent of this project is not
/// sent to any webhook. Runs once per event (after the idempotent store).
pub(crate) async fn platform_fan_out(db: &PgPool, user_id: &str, name: &str, data: &Value) -> Result<(), sqlx::Error> {
    let Some(project) = user_id.strip_prefix("platform:") else { return Ok(()) };
    if !crate::routes::platform_v1::events::EVENT_TYPES.contains(&name) {
        return Ok(());
    }
    let Some(agent) = data["bot_id"].as_str() else { return Ok(()) };
    let account: Option<String> = sqlx::query_scalar("SELECT account_id FROM platform_agents WHERE id = $1 AND project_id = $2")
        .bind(agent)
        .bind(project)
        .fetch_optional(db)
        .await?;
    let Some(account) = account else { return Ok(()) };
    let pick = |k: &str| data.get(k).cloned().unwrap_or(Value::Null);
    let payload = match name {
        "message.received" => json!({
            "agent_id": agent, "channel": pick("channel").as_str().map(str::to_string).or_else(|| pick("provider").as_str().map(str::to_string)),
            "from": pick("from"), "text": pick("text"), "thread_id": pick("thread_id"),
        }),
        "approval.requested" => json!({
            "agent_id": agent, "approval_id": pick("approvalId"), "action": pick("action"), "summary": pick("summary"), "thread_id": pick("thread_id"),
        }),
        "inbox.item.created" => json!({
            "agent_id": agent, "item_id": pick("itemId"), "kind": pick("kind"), "title": pick("title"), "severity": pick("severity"),
        }),
        _ => return Ok(()),
    };
    crate::routes::platform_v1::events::emit_event(db, project, Some(&account), name, payload).await?;
    Ok(())
}

async fn ingest_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, body: Bytes) -> Response {
    if body.len() > MAX_BODY_BYTES {
        return json_error(StatusCode::PAYLOAD_TOO_LARGE, "body_too_large");
    }
    let (user_id, runtime_id) = match authenticate(&state.db, &headers, &body, Utc::now().timestamp()).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let batch: Batch = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(_) => return json_error(StatusCode::BAD_REQUEST, "invalid_body"),
    };
    if batch.events.len() > MAX_EVENTS {
        return json_error(StatusCode::PAYLOAD_TOO_LARGE, "too_many_events");
    }
    match ingest(&state.db, &user_id, &runtime_id, batch.events).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => {
            tracing::error!(runtime = %runtime_id, "runtime events ingest failed: {e}");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "internal")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::runtime_pairing::sha256_hex;
    use crate::routes::test_support::{events_backbone_schema as events_schema, seed_runtime_device, test_pool};

    fn signed(runtime: &str, key: &str, ts: i64, body: &[u8]) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(RUNTIME_ID_HEADER, runtime.parse().unwrap());
        h.insert(TS_HEADER, ts.to_string().parse().unwrap());
        h.insert(SIG_HEADER, sign_runtime_request(key, ts, "POST", PATH, body).parse().unwrap());
        h
    }

    #[tokio::test]
    async fn the_relay_signature_authenticates_the_runtime_and_its_owner() {
        let db = test_pool().await;
        seed_runtime_device(&db, "rt-1", "user-1").await;
        let key = sha256_hex(b"token-of-rt-1");
        let body = br#"{"events":[]}"#;
        let now = Utc::now().timestamp();
        assert_eq!(authenticate(&db, &signed("rt-1", &key, now, body), body, now).await.unwrap(), ("user-1".into(), "rt-1".into()));
        // Wrong key, tampered body, stale clock, unknown runtime, missing headers.
        assert!(authenticate(&db, &signed("rt-1", &sha256_hex(b"nope"), now, body), body, now).await.is_err());
        assert!(authenticate(&db, &signed("rt-1", &key, now, body), br#"{"events":[1]}"#, now).await.is_err());
        assert!(authenticate(&db, &signed("rt-1", &key, now - 301, body), body, now).await.is_err());
        assert!(authenticate(&db, &signed("rt-x", &key, now, body), body, now).await.is_err());
        assert!(authenticate(&db, &HeaderMap::new(), body, now).await.is_err());
        sqlx::query("UPDATE runtime_devices SET revoked_at = now() WHERE id = 'rt-1'").execute(&db).await.unwrap();
        assert!(authenticate(&db, &signed("rt-1", &key, now, body), body, now).await.is_err());
    }

    #[tokio::test]
    async fn ingest_maps_dedupes_ignores_and_fans_out_to_matching_subscriptions() {
        let db = test_pool().await;
        events_schema(&db).await;
        // A live, verified subscription for bot b1's messages, one for b2, one unverified.
        for (id, args, verified) in [("sub_b1", json!({ "bot_id": "b1" }), true), ("sub_b2", json!({ "bot_id": "b2" }), true), ("sub_unverified", json!({}), false)] {
            sqlx::query(
                "INSERT INTO platform_webhooks (id, kind, signer, user_id, client_id, target, url, events, secret, arguments, refresh_before, verified_at) \
                 VALUES ($1, 'mcp_subscription', 'standard_webhooks', 'user-1', 'c', 'agents', 'https://93.184.216.34/cb', ARRAY['message.received'], 'whsec_x', $2, now() + interval '1 day', CASE WHEN $3 THEN now() END)",
            )
            .bind(id)
            .bind(&args)
            .bind(verified)
            .execute(&db)
            .await
            .unwrap();
        }
        let batch = vec![
            json!({ "id": "e1", "type": "channel.message.received", "at": "2026-10-05T12:00:00Z", "bot_id": "b1", "thread_id": "t1", "data": { "text": "hi" } }),
            json!({ "id": "e2", "type": "channel.message.received", "bot_id": "b1", "data": { "own": true } }),
            json!({ "id": "e3", "type": "inbox.changed", "data": {} }),
            json!({ "id": "e4", "type": "message.status", "data": {} }),
            json!({ "id": "e5", "type": "approval.requested", "data": "not an object" }),
            json!({ "type": "approval.requested" }),
            json!({ "id": "e6", "type": "approval.requested", "data": { "blob": "x".repeat(MAX_DATA_BYTES) } }),
        ];
        let r = ingest(&db, "user-1", "rt-1", batch.clone()).await.unwrap();
        let statuses: Vec<_> = r["results"].as_array().unwrap().iter().map(|x| x["status"].as_str().unwrap().to_string()).collect();
        assert_eq!(statuses, ["accepted", "ignored", "ignored", "ignored", "invalid", "invalid", "too_large"]);
        let (name, data, occurred): (String, Value, Option<DateTime<Utc>>) =
            sqlx::query_as("SELECT type, data, occurred_at FROM platform_events WHERE subject = 'user' AND user_id = 'user-1'").fetch_one(&db).await.unwrap();
        assert_eq!(name, "message.received");
        assert_eq!((data["bot_id"].as_str(), data["thread_id"].as_str(), data["text"].as_str()), (Some("b1"), Some("t1"), Some("hi")));
        assert_eq!(occurred.unwrap().to_rfc3339(), "2026-10-05T12:00:00+00:00");
        let delivered_to: Vec<String> = sqlx::query_scalar("SELECT webhook_id FROM platform_webhook_deliveries").fetch_all(&db).await.unwrap();
        assert_eq!(delivered_to, vec!["sub_b1".to_string()]);

        // The same id again is a duplicate (a forwarder retry), not a second event.
        let again = ingest(&db, "user-1", "rt-1", batch[..1].to_vec()).await.unwrap();
        assert_eq!(again["results"][0]["status"], "duplicate");
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM platform_events").fetch_one(&db).await.unwrap(), 1);
        // Idempotency is per runtime: another runtime's e1 is its own event.
        let other = ingest(&db, "user-1", "rt-2", batch[..1].to_vec()).await.unwrap();
        assert_eq!(other["results"][0]["status"], "accepted");
    }
}
