//! MCP Events at the `mcp.allternit.com` edge: `events/list`,
//! `events/subscribe`, `events/unsubscribe`, answered by the edge itself
//! (never relayed to a runtime) after its token and approval checks.
//!
//! Webhook delivery only (what ChatGPT implements). Profile:
//!
//! * **Principal** = verified user + OAuth client (or `cli-key:<id>`) + edge
//!   target (`agents` | `bot:<id>`). Subscription id =
//!   `mcp_protocol::events::subscription_id(principal, url, name, arguments)`,
//!   so subscribing again with the same identity is a refresh, not a second row.
//! * **Scope**: the agents server sees the agents audience of the registry
//!   (`routes::allternit_events`); a vendor-bot connector sees the bot audience,
//!   and `arguments.bot_id` is bound to the path's bot (another bot's id is
//!   `-32012 Forbidden`).
//! * **Subscribe**: https callback, no private / local hosts (and the address
//!   is re-checked at every delivery), `whsec_` secret of 24–64 bytes. Before a
//!   new subscription (or a new secret, or a verification older than
//!   [`VERIFY_CACHE`]) is active, the callback gets a signed
//!   `{"type":"verification","challenge":…}` and must answer 2xx echoing
//!   `{"challenge":…}`; otherwise `-32015 CallbackEndpointError` with
//!   `data.reason` (`connection_refused | timeout | tls_error | http_4xx |
//!   http_5xx | challenge_failed`). TTL: omitted = 24 h, `null` = clamped to
//!   7 days (we never grant "no expiry"), otherwise clamped to [1 min, 7 days].
//!   A refresh with a new secret rotates it: deliveries are signed with both
//!   keys for [`ROTATION_GRACE`]. Events are not replayable: `cursor` is
//!   always `null`. At most [`MAX_SUBSCRIPTIONS`] live subscriptions per
//!   principal (`-32013`, `data: {limit: "subscriptions", max}`).
//! * **Unsubscribe** is idempotent (`{}` whether or not one existed).
//! * **Revoke**: when the owner revokes the app's approval (or a CLI key), the
//!   principal's subscriptions end and each callback gets a signed
//!   `{"type":"terminated", …, "error":{"code":-32012,"message":"Forbidden",
//!   "data":{"reason":"approval_revoked"}}}`.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rand::RngCore;
use serde_json::{json, Value};
use sqlx::PgPool;

use mcp_protocol::events::{self as ev, codes, DeliveryMode};

use super::allternit_events::{self, Audience};
use super::platform_v1::events::{post_vetted, standard_headers, PostError, KIND_MCP, SIGNER_STANDARD};

/// Live subscriptions one principal may hold.
pub const MAX_SUBSCRIPTIONS: i64 = 50;
/// A verified callback isn't re-challenged on refresh within this window.
pub const VERIFY_CACHE: ChronoDuration = ChronoDuration::hours(24);
/// After a secret rotation, deliveries carry signatures for both keys this long.
pub const ROTATION_GRACE: ChronoDuration = ChronoDuration::hours(1);

/// Who is subscribing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub user_id: String,
    pub client: String,
    /// `agents` | `bot:<vendorBotId>` (`mcp_oauth_approvals.target`).
    pub target: String,
}

impl Principal {
    fn key(&self) -> String {
        format!("{}|{}|{}", self.user_id, self.client, self.target)
    }
    fn bot(&self) -> Option<&str> {
        self.target.strip_prefix("bot:")
    }
    fn audience(&self) -> Audience {
        if self.bot().is_some() {
            Audience::Bot
        } else {
            Audience::Agents
        }
    }
}

/// One stored subscription (a `platform_webhooks` row of kind `mcp_subscription`).
#[derive(Debug, Clone, PartialEq)]
pub struct Subscription {
    pub id: String,
    pub principal: Principal,
    pub name: String,
    pub arguments: Value,
    pub url: String,
    pub secret: String,
    pub previous_secret: Option<String>,
    pub previous_secret_until: Option<DateTime<Utc>>,
    pub refresh_before: Option<DateTime<Utc>>,
    pub verified_at: Option<DateTime<Utc>>,
    pub last_delivery_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

/// Why a callback check failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackError {
    /// The URL itself is unacceptable (resolves to a private address, …): `-32602`.
    Invalid(String),
    /// The endpoint didn't pass: `-32015` with this `data.reason`.
    Endpoint(&'static str),
}

/// Storage + callback seam, so the protocol logic is tested without Postgres or a network.
#[async_trait::async_trait]
pub trait EventsStore: Send + Sync {
    /// The live (not ended) subscription with this id.
    async fn get(&self, id: &str) -> Result<Option<Subscription>, String>;
    async fn live_count(&self, principal: &Principal) -> Result<i64, String>;
    /// Insert or replace (re-activating an ended row with the same id).
    async fn save(&self, sub: &Subscription) -> Result<(), String>;
    /// End it; `false` when there was nothing live.
    async fn remove(&self, id: &str, reason: &str) -> Result<bool, String>;
    /// POST a signed verification challenge; `Ok` when 2xx echoed it.
    async fn verify_callback(&self, url: &str, secret: &str, subscription_id: &str, challenge: &str) -> Result<(), CallbackError>;
}

fn err(id: &Value, code: i64, data: Value) -> Value {
    mcp_protocol::rpc_err_data(id, code, ev::code_name(code), data)
}

fn invalid(id: &Value, message: &str) -> Value {
    mcp_protocol::rpc_err_data(id, mcp_protocol::codes::INVALID_PARAMS, message, json!({}))
}

fn random_hex(n: usize) -> String {
    let mut b = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

/// Check the arguments against the event's filters and bind `bot_id` on a
/// bot connector. Returns the arguments that identify the subscription.
fn bind_arguments(principal: &Principal, event: &allternit_events::EventType, arguments: &Value) -> Result<Value, (i64, String)> {
    let Some(obj) = arguments.as_object() else {
        return Err((mcp_protocol::codes::INVALID_PARAMS, "arguments must be an object".into()));
    };
    let audience = principal.audience();
    let allowed: Vec<&str> = event.filter_keys(audience).collect();
    let mut out = serde_json::Map::new();
    for (k, v) in obj {
        if principal.bot().is_some() && k == "bot_id" {
            if v.as_str() != principal.bot() {
                return Err((codes::FORBIDDEN, "this connector only sees its own bot's events".into()));
            }
            continue;
        }
        if !allowed.contains(&k.as_str()) {
            return Err((mcp_protocol::codes::INVALID_PARAMS, format!("unknown argument '{k}' for {}", event.name)));
        }
        let Some(s) = v.as_str().filter(|s| !s.is_empty() && s.len() <= 256) else {
            return Err((mcp_protocol::codes::INVALID_PARAMS, format!("argument '{k}' must be a non-empty string")));
        };
        out.insert(k.clone(), json!(s));
    }
    if let Some(bot) = principal.bot() {
        out.insert("bot_id".into(), json!(bot));
    }
    Ok(Value::Object(out))
}

/// Answer one `events/*` request. `id` is the JSON-RPC id. The caller
/// decorates the result for the request's era (`mcp_protocol::finish`).
pub async fn handle(store: &dyn EventsStore, principal: &Principal, id: &Value, method: &str, params: &Value, now: DateTime<Utc>) -> Value {
    match method {
        "events/list" => {
            let events = allternit_events::mcp_list(principal.audience());
            mcp_protocol::rpc_ok(id, json!({ "events": events }))
        }
        "events/subscribe" => subscribe(store, principal, id, params, now).await,
        "events/unsubscribe" => unsubscribe(store, principal, id, params).await,
        other => mcp_protocol::rpc_err(id, mcp_protocol::codes::METHOD_NOT_FOUND, format!("Method not found: {other}")),
    }
}

fn store_failed(id: &Value, error: String) -> Value {
    tracing::error!("mcp events store: {error}");
    mcp_protocol::rpc_err(id, -32603, "Internal error")
}

async fn subscribe(store: &dyn EventsStore, principal: &Principal, id: &Value, params: &Value, now: DateTime<Utc>) -> Value {
    let p: ev::SubscribeParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => return invalid(id, &format!("invalid events/subscribe params: {e}")),
    };
    let Some(event) = allternit_events::find(&p.name).filter(|e| e.visible_to(principal.audience())) else {
        return err(id, codes::NOT_FOUND, json!({ "kind": "event", "name": p.name }));
    };
    if p.delivery.mode != DeliveryMode::Webhook {
        let mode = serde_json::to_value(p.delivery.mode).unwrap_or(Value::Null);
        return err(id, codes::UNSUPPORTED, json!({ "feature": "deliveryMode", "value": mode }));
    }
    let (url, _) = match ev::validate_webhook(&p.delivery, true) {
        Ok(v) => v,
        Err(e) => return invalid(id, &e),
    };
    let secret = p.delivery.secret.clone().unwrap_or_default();
    let arguments = match bind_arguments(principal, event, &p.arguments) {
        Ok(a) => a,
        Err((code, msg)) if code == codes::FORBIDDEN => return err(id, code, json!({ "reason": msg })),
        Err((_, msg)) => return invalid(id, &msg),
    };
    let sub_id = ev::subscription_id(&principal.key(), &url, event.name, &arguments);
    let existing = match store.get(&sub_id).await {
        Ok(e) => e,
        Err(e) => return store_failed(id, e),
    };
    if existing.is_none() {
        match store.live_count(principal).await {
            Ok(n) if n >= MAX_SUBSCRIPTIONS => return err(id, codes::RESOURCE_EXHAUSTED, json!({ "limit": "subscriptions", "max": MAX_SUBSCRIPTIONS })),
            Ok(_) => {}
            Err(e) => return store_failed(id, e),
        }
    }
    let needs_challenge = match &existing {
        None => true,
        Some(s) => s.secret != secret || s.verified_at.is_none_or(|v| now - v > VERIFY_CACHE),
    };
    let mut verified_at = existing.as_ref().and_then(|s| s.verified_at);
    if needs_challenge {
        match store.verify_callback(&url, &secret, &sub_id, &random_hex(24)).await {
            Ok(()) => verified_at = Some(now),
            Err(CallbackError::Invalid(msg)) => return invalid(id, &msg),
            Err(CallbackError::Endpoint(reason)) => return err(id, codes::CALLBACK_ENDPOINT_ERROR, json!({ "reason": reason })),
        }
    }
    let ttl_ms = ev::grant_ttl_ms(p.ttl_ms);
    let refresh_before = now + ChronoDuration::milliseconds(ttl_ms as i64);
    let (previous_secret, previous_secret_until) = match &existing {
        Some(old) if old.secret != secret => (Some(old.secret.clone()), Some(now + ROTATION_GRACE)),
        Some(old) => (old.previous_secret.clone(), old.previous_secret_until),
        None => (None, None),
    };
    let sub = Subscription {
        id: sub_id.clone(),
        principal: principal.clone(),
        name: event.name.to_string(),
        arguments,
        url,
        secret,
        previous_secret,
        previous_secret_until,
        refresh_before: Some(refresh_before),
        verified_at,
        last_delivery_at: existing.as_ref().and_then(|s| s.last_delivery_at),
        last_error: existing.as_ref().and_then(|s| s.last_error.clone()),
    };
    if let Err(e) = store.save(&sub).await {
        return store_failed(id, e);
    }
    mcp_protocol::rpc_ok(
        id,
        json!({
            "id": sub_id,
            "refreshBefore": refresh_before.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "cursor": null,
            "truncated": false,
            "deliveryStatus": {
                "active": true,
                "lastDeliveryAt": sub.last_delivery_at.map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
                "lastError": sub.last_error,
            }
        }),
    )
}

async fn unsubscribe(store: &dyn EventsStore, principal: &Principal, id: &Value, params: &Value) -> Value {
    let p: ev::UnsubscribeParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => return invalid(id, &format!("invalid events/unsubscribe params: {e}")),
    };
    let (url, _) = match ev::validate_webhook(&p.delivery, false) {
        Ok(v) => v,
        Err(e) => return invalid(id, &e),
    };
    // Unknown names and foreign bots can't match anything we hold: still `{}`.
    if let Some(event) = allternit_events::find(&p.name).filter(|e| e.visible_to(principal.audience())) {
        if let Ok(arguments) = bind_arguments(principal, event, &p.arguments) {
            let sub_id = ev::subscription_id(&principal.key(), &url, event.name, &arguments);
            if let Err(e) = store.remove(&sub_id, "unsubscribed").await {
                return store_failed(id, e);
            }
        }
    }
    mcp_protocol::rpc_ok(id, json!({}))
}

// ─── production store ────────────────────────────────────────────────────────

pub struct PgEventsStore<'a> {
    pub db: &'a PgPool,
}

type Row = (
    String,
    String,
    String,
    String,
    Vec<String>,
    Value,
    String,
    String,
    Option<String>,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    Option<String>,
);

#[async_trait::async_trait]
impl EventsStore for PgEventsStore<'_> {
    async fn get(&self, id: &str) -> Result<Option<Subscription>, String> {
        let row: Option<Row> = sqlx::query_as(
            "SELECT w.id, w.user_id, w.client_id, w.target, w.events, w.arguments, w.url, w.secret, w.previous_secret, w.previous_secret_until, \
                    w.refresh_before, w.verified_at, \
                    (SELECT max(delivered_at) FROM platform_webhook_deliveries d WHERE d.webhook_id = w.id), \
                    (SELECT d.last_error FROM platform_webhook_deliveries d WHERE d.webhook_id = w.id AND d.last_error IS NOT NULL ORDER BY d.created_at DESC LIMIT 1) \
             FROM platform_webhooks w WHERE w.id = $1 AND w.kind = 'mcp_subscription' AND w.deleted_at IS NULL",
        )
        .bind(id)
        .fetch_optional(self.db)
        .await
        .map_err(|e| e.to_string())?;
        Ok(row.map(|r| Subscription {
            id: r.0,
            principal: Principal { user_id: r.1, client: r.2, target: r.3 },
            name: r.4.into_iter().next().unwrap_or_default(),
            arguments: r.5,
            url: r.6,
            secret: r.7,
            previous_secret: r.8,
            previous_secret_until: r.9,
            refresh_before: r.10,
            verified_at: r.11,
            last_delivery_at: r.12,
            last_error: r.13,
        }))
    }

    async fn live_count(&self, p: &Principal) -> Result<i64, String> {
        sqlx::query_scalar(
            "SELECT count(*) FROM platform_webhooks WHERE kind = 'mcp_subscription' AND deleted_at IS NULL AND user_id = $1 AND client_id = $2 AND target = $3 \
               AND (refresh_before IS NULL OR refresh_before > now())",
        )
        .bind(&p.user_id)
        .bind(&p.client)
        .bind(&p.target)
        .fetch_one(self.db)
        .await
        .map_err(|e| e.to_string())
    }

    async fn save(&self, s: &Subscription) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO platform_webhooks (id, kind, signer, project_id, user_id, client_id, target, url, events, arguments, secret, previous_secret, previous_secret_until, refresh_before, verified_at, updated_at) \
             VALUES ($1, $2, $3, NULL, $4, $5, $6, $7, ARRAY[$8], $9, $10, $11, $12, $13, $14, now()) \
             ON CONFLICT (id) DO UPDATE SET secret = EXCLUDED.secret, previous_secret = EXCLUDED.previous_secret, previous_secret_until = EXCLUDED.previous_secret_until, \
               refresh_before = EXCLUDED.refresh_before, verified_at = EXCLUDED.verified_at, deleted_at = NULL, deactivated_reason = NULL, updated_at = now()",
        )
        .bind(&s.id)
        .bind(KIND_MCP)
        .bind(SIGNER_STANDARD)
        .bind(&s.principal.user_id)
        .bind(&s.principal.client)
        .bind(&s.principal.target)
        .bind(&s.url)
        .bind(&s.name)
        .bind(&s.arguments)
        .bind(&s.secret)
        .bind(&s.previous_secret)
        .bind(s.previous_secret_until)
        .bind(s.refresh_before)
        .bind(s.verified_at)
        .execute(self.db)
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn remove(&self, id: &str, reason: &str) -> Result<bool, String> {
        let live: Option<String> = sqlx::query_scalar("SELECT id FROM platform_webhooks WHERE id = $1 AND kind = 'mcp_subscription' AND deleted_at IS NULL")
            .bind(id)
            .fetch_optional(self.db)
            .await
            .map_err(|e| e.to_string())?;
        if live.is_none() {
            return Ok(false);
        }
        super::platform_v1::events::deactivate_endpoint(self.db, id, reason).await.map_err(|e| e.to_string())?;
        Ok(true)
    }

    async fn verify_callback(&self, url: &str, secret: &str, subscription_id: &str, challenge: &str) -> Result<(), CallbackError> {
        verify_callback_http(url, secret, subscription_id, challenge).await
    }
}

/// POST `{"type":"verification","challenge":…}` signed (Standard Webhooks +
/// `X-MCP-Subscription-Id`) and require a 2xx echoing the challenge.
pub async fn verify_callback_http(url: &str, secret: &str, subscription_id: &str, challenge: &str) -> Result<(), CallbackError> {
    let key = mcp_protocol::webhooks::parse_secret(secret).map_err(|e| CallbackError::Invalid(e.to_string()))?;
    let body = serde_json::to_vec(&ev::verification_envelope(challenge)).unwrap_or_default();
    let msg_id = format!("msg_verify_{}", random_hex(12));
    let mut headers = standard_headers(&[&key], &msg_id, Utc::now().timestamp(), &body);
    headers.push((ev::HEADER_SUBSCRIPTION_ID.to_string(), subscription_id.to_string()));
    headers.push(("user-agent".into(), "Allternit-Webhooks/1.0".into()));
    headers.push(("content-type".into(), "application/json".into()));
    match post_vetted(url, &headers, &body, 4096).await {
        Ok((status, reply)) if (200..300).contains(&status) => {
            if ev::challenge_echoed(&reply, challenge) {
                Ok(())
            } else {
                Err(CallbackError::Endpoint(ev::callback_reasons::CHALLENGE_FAILED))
            }
        }
        Ok((status, _)) if (400..500).contains(&status) => Err(CallbackError::Endpoint(ev::callback_reasons::HTTP_4XX)),
        Ok((status, _)) if status >= 500 => Err(CallbackError::Endpoint(ev::callback_reasons::HTTP_5XX)),
        Ok(_) => Err(CallbackError::Endpoint(ev::callback_reasons::CHALLENGE_FAILED)),
        Err(PostError::Refused(msg)) => Err(CallbackError::Invalid(msg)),
        Err(PostError::Timeout) => Err(CallbackError::Endpoint(ev::callback_reasons::TIMEOUT)),
        Err(PostError::Tls) => Err(CallbackError::Endpoint(ev::callback_reasons::TLS_ERROR)),
        Err(PostError::Connect | PostError::Other) => Err(CallbackError::Endpoint(ev::callback_reasons::CONNECTION_REFUSED)),
    }
}

/// The owner revoked `client`'s approval for `target` (or a CLI key): end that
/// principal's subscriptions now and tell each callback, signed, in the
/// background (one attempt; the subscription is gone either way). Returns
/// how many ended.
pub async fn terminate_principal(db: &PgPool, user_id: &str, client: &str, target: &str, reason: &'static str) -> Result<usize, sqlx::Error> {
    let subs: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT id, url, secret FROM platform_webhooks WHERE kind = 'mcp_subscription' AND deleted_at IS NULL AND user_id = $1 AND client_id = $2 AND target = $3",
    )
    .bind(user_id)
    .bind(client)
    .bind(target)
    .fetch_all(db)
    .await?;
    for (id, _, _) in &subs {
        super::platform_v1::events::deactivate_endpoint(db, id, reason).await?;
    }
    let n = subs.len();
    if n > 0 {
        tokio::spawn(async move {
            for (id, url, secret) in subs {
                send_terminated(&id, &url, &secret, reason).await;
            }
        });
    }
    Ok(n)
}

/// The signed `terminated` envelope for one subscription.
pub fn terminated_request(subscription_id: &str, secret: &str, reason: &str, now_unix: i64) -> Option<(Vec<(String, String)>, Vec<u8>)> {
    let key = mcp_protocol::webhooks::parse_secret(secret).ok()?;
    let body = serde_json::to_vec(&ev::terminated_envelope(subscription_id, codes::FORBIDDEN, json!({ "reason": reason }))).ok()?;
    let mut headers = standard_headers(&[&key], &format!("msg_terminated_{subscription_id}"), now_unix, &body);
    headers.push((ev::HEADER_SUBSCRIPTION_ID.to_string(), subscription_id.to_string()));
    headers.push(("user-agent".into(), "Allternit-Webhooks/1.0".into()));
    headers.push(("content-type".into(), "application/json".into()));
    Some((headers, body))
}

async fn send_terminated(id: &str, url: &str, secret: &str, reason: &str) {
    let Some((headers, body)) = terminated_request(id, secret, reason, Utc::now().timestamp()) else { return };
    if let Err(e) = post_vetted(url, &headers, &body, 0).await {
        tracing::info!(subscription = %id, "mcp events: terminated envelope not delivered: {}", e.message());
    }
}

/// Test doubles shared with `mcp_edge` tests.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// In-memory store with a scripted callback.
    #[derive(Default)]
    pub struct MemStore {
        pub subs: Mutex<HashMap<String, Subscription>>,
        pub challenges: Mutex<Vec<(String, String)>>,
        pub callback: Mutex<Option<CallbackError>>,
    }

    #[async_trait::async_trait]
    impl EventsStore for MemStore {
        async fn get(&self, id: &str) -> Result<Option<Subscription>, String> {
            Ok(self.subs.lock().unwrap().get(id).cloned())
        }
        async fn live_count(&self, p: &Principal) -> Result<i64, String> {
            Ok(self.subs.lock().unwrap().values().filter(|s| &s.principal == p).count() as i64)
        }
        async fn save(&self, s: &Subscription) -> Result<(), String> {
            self.subs.lock().unwrap().insert(s.id.clone(), s.clone());
            Ok(())
        }
        async fn remove(&self, id: &str, _: &str) -> Result<bool, String> {
            Ok(self.subs.lock().unwrap().remove(id).is_some())
        }
        async fn verify_callback(&self, url: &str, secret: &str, _: &str, _: &str) -> Result<(), CallbackError> {
            self.challenges.lock().unwrap().push((url.into(), secret.into()));
            match self.callback.lock().unwrap().clone() {
                Some(e) => Err(e),
                None => Ok(()),
            }
        }
    }

    pub fn secret(b: u8) -> String {
        format!("whsec_{}", B64.encode([b; 32]))
    }

}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    use std::collections::HashMap;
    use std::sync::Mutex;

    fn agents() -> Principal {
        Principal { user_id: "u1".into(), client: "chatgpt".into(), target: "agents".into() }
    }

    fn bot(id: &str) -> Principal {
        Principal { user_id: "u1".into(), client: "chatgpt".into(), target: format!("bot:{id}") }
    }

    fn sub_params(name: &str, args: Value, secret: &str, ttl: Option<Value>) -> Value {
        let mut p = json!({ "name": name, "arguments": args, "delivery": { "mode": "webhook", "url": "https://cb.example.com/hook/1", "secret": secret } });
        if let Some(t) = ttl {
            p["ttlMs"] = t;
        }
        p
    }

    fn t0() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-05T12:00:00Z").unwrap().with_timezone(&Utc)
    }

    async fn sub(store: &MemStore, p: &Principal, params: Value, now: DateTime<Utc>) -> Value {
        handle(store, p, &json!(1), "events/subscribe", &params, now).await
    }

    #[tokio::test]
    async fn list_is_scoped_to_the_server() {
        let store = MemStore::default();
        let a = handle(&store, &agents(), &json!(1), "events/list", &json!({}), t0()).await;
        let names: Vec<_> = a["result"]["events"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"approval.requested") && !names.contains(&"vendor.ticket.created"));
        assert_eq!(a["result"]["events"][0]["delivery"][0], "webhook");
        let b = handle(&store, &bot("b1"), &json!(1), "events/list", &json!({}), t0()).await;
        let names: Vec<_> = b["result"]["events"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"vendor.ticket.created") && !names.contains(&"approval.requested"));
    }

    #[tokio::test]
    async fn subscribe_is_idempotent_by_identity_and_verifies_once() {
        let store = MemStore::default();
        let r1 = sub(&store, &agents(), sub_params("approval.requested", json!({ "bot_id": "b1" }), &secret(1), None), t0()).await;
        let id = r1["result"]["id"].as_str().unwrap().to_string();
        assert!(id.starts_with("sub_"));
        assert_eq!(r1["result"]["cursor"], Value::Null);
        assert_eq!(r1["result"]["truncated"], false);
        assert_eq!(r1["result"]["deliveryStatus"]["active"], true);
        assert_eq!(r1["result"]["refreshBefore"], "2026-10-06T12:00:00Z", "24 h default");
        // Same identity, same secret, an hour later: refresh, same id, no new challenge.
        let r2 = sub(&store, &agents(), sub_params("approval.requested", json!({ "bot_id": "b1" }), &secret(1), None), t0() + ChronoDuration::hours(1)).await;
        assert_eq!(r2["result"]["id"], id.as_str());
        assert_eq!(r2["result"]["refreshBefore"], "2026-10-06T13:00:00Z");
        assert_eq!(store.challenges.lock().unwrap().len(), 1);
        assert_eq!(store.subs.lock().unwrap().len(), 1);
        // Different arguments = a different subscription.
        let r3 = sub(&store, &agents(), sub_params("approval.requested", json!({ "bot_id": "b2" }), &secret(1), None), t0()).await;
        assert_ne!(r3["result"]["id"], id.as_str());
        // A stale verification is re-challenged.
        sub(&store, &agents(), sub_params("approval.requested", json!({ "bot_id": "b1" }), &secret(1), None), t0() + ChronoDuration::hours(30)).await;
        assert_eq!(store.challenges.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn refresh_with_a_new_secret_rotates_and_keeps_the_old_one_for_the_grace_window() {
        let store = MemStore::default();
        let r = sub(&store, &agents(), sub_params("thread.needs_user", json!({}), &secret(1), None), t0()).await;
        let id = r["result"]["id"].as_str().unwrap().to_string();
        let later = t0() + ChronoDuration::minutes(5);
        sub(&store, &agents(), sub_params("thread.needs_user", json!({}), &secret(2), None), later).await;
        let s = store.subs.lock().unwrap().get(&id).cloned().unwrap();
        assert_eq!(s.secret, secret(2));
        assert_eq!(s.previous_secret.as_deref(), Some(secret(1).as_str()));
        assert_eq!(s.previous_secret_until, Some(later + ROTATION_GRACE));
        assert_eq!(store.challenges.lock().unwrap().last().unwrap().1, secret(2), "the new secret is challenged");
    }

    #[tokio::test]
    async fn ttl_rules() {
        let store = MemStore::default();
        let cases = [
            (Some(json!(3_600_000)), "2026-10-05T13:00:00Z"),
            (Some(Value::Null), "2026-10-12T12:00:00Z"),
            (Some(json!(10)), "2026-10-05T12:01:00Z"),
            (Some(json!(99_999_999_999u64)), "2026-10-12T12:00:00Z"),
        ];
        for (i, (ttl, want)) in cases.into_iter().enumerate() {
            let r = sub(&store, &agents(), sub_params("approval.requested", json!({ "thread_id": format!("t{i}") }), &secret(1), ttl), t0()).await;
            assert_eq!(r["result"]["refreshBefore"], want, "{r}");
        }
    }

    #[tokio::test]
    async fn validation_errors_use_the_extension_codes() {
        let store = MemStore::default();
        let code = |v: &Value| v["error"]["code"].as_i64().unwrap();
        let unknown = sub(&store, &agents(), sub_params("nope.event", json!({}), &secret(1), None), t0()).await;
        assert_eq!((code(&unknown), unknown["error"]["message"].as_str(), unknown["error"]["data"]["kind"].as_str()), (codes::NOT_FOUND, Some("NotFound"), Some("event")));
        // Platform-only and bot-only events are not on the agents server.
        assert_eq!(code(&sub(&store, &agents(), sub_params("message.status", json!({}), &secret(1), None), t0()).await), codes::NOT_FOUND);
        assert_eq!(code(&sub(&store, &agents(), sub_params("vendor.ticket.created", json!({}), &secret(1), None), t0()).await), codes::NOT_FOUND);
        let mut poll = sub_params("approval.requested", json!({}), &secret(1), None);
        poll["delivery"]["mode"] = json!("poll");
        let unsupported = sub(&store, &agents(), poll, t0()).await;
        assert_eq!((code(&unsupported), unsupported["error"]["data"]["value"].as_str()), (codes::UNSUPPORTED, Some("poll")));
        let short = format!("whsec_{}", B64.encode([1u8; 8]));
        assert_eq!(code(&sub(&store, &agents(), sub_params("approval.requested", json!({}), &short, None), t0()).await), -32602);
        let mut private = sub_params("approval.requested", json!({}), &secret(1), None);
        private["delivery"]["url"] = json!("https://10.0.0.1/x");
        assert_eq!(code(&sub(&store, &agents(), private, t0()).await), -32602);
        assert_eq!(code(&sub(&store, &agents(), sub_params("approval.requested", json!({ "evil": "x" }), &secret(1), None), t0()).await), -32602);
        assert!(store.challenges.lock().unwrap().is_empty(), "nothing invalid is ever challenged");
    }

    #[tokio::test]
    async fn a_failed_challenge_is_32015_with_the_reason_and_nothing_is_stored() {
        let store = MemStore::default();
        *store.callback.lock().unwrap() = Some(CallbackError::Endpoint(ev::callback_reasons::CHALLENGE_FAILED));
        let r = sub(&store, &agents(), sub_params("approval.requested", json!({}), &secret(1), None), t0()).await;
        assert_eq!(r["error"]["code"], codes::CALLBACK_ENDPOINT_ERROR);
        assert_eq!(r["error"]["message"], "CallbackEndpointError");
        assert_eq!(r["error"]["data"]["reason"], "challenge_failed");
        assert!(store.subs.lock().unwrap().is_empty());
        *store.callback.lock().unwrap() = Some(CallbackError::Endpoint(ev::callback_reasons::TIMEOUT));
        let r = sub(&store, &agents(), sub_params("approval.requested", json!({}), &secret(1), None), t0()).await;
        assert_eq!(r["error"]["data"]["reason"], "timeout");
    }

    #[tokio::test]
    async fn a_bot_connector_is_bound_to_its_bot() {
        let store = MemStore::default();
        let r = sub(&store, &bot("b1"), sub_params("vendor.ticket.created", json!({}), &secret(1), None), t0()).await;
        let id = r["result"]["id"].as_str().unwrap().to_string();
        assert_eq!(store.subs.lock().unwrap()[&id].arguments, json!({ "bot_id": "b1" }));
        // Naming its own bot is the same subscription; another bot is Forbidden.
        let same = sub(&store, &bot("b1"), sub_params("vendor.ticket.created", json!({ "bot_id": "b1" }), &secret(1), None), t0()).await;
        assert_eq!(same["result"]["id"], id.as_str());
        let other = sub(&store, &bot("b1"), sub_params("vendor.ticket.created", json!({ "bot_id": "b2" }), &secret(1), None), t0()).await;
        assert_eq!(other["error"]["code"], codes::FORBIDDEN);
        // The same callback for another bot's connector is a different subscription.
        let b2 = sub(&store, &bot("b2"), sub_params("vendor.ticket.created", json!({}), &secret(1), None), t0()).await;
        assert_ne!(b2["result"]["id"], id.as_str());
    }

    #[tokio::test]
    async fn quota_is_per_principal() {
        let store = MemStore::default();
        for i in 0..MAX_SUBSCRIPTIONS {
            let r = sub(&store, &agents(), sub_params("approval.requested", json!({ "thread_id": format!("t{i}") }), &secret(1), None), t0()).await;
            assert!(r.get("result").is_some());
        }
        let over = sub(&store, &agents(), sub_params("approval.requested", json!({ "thread_id": "one-more" }), &secret(1), None), t0()).await;
        assert_eq!((over["error"]["code"].as_i64(), over["error"]["data"]["limit"].as_str()), (Some(codes::RESOURCE_EXHAUSTED), Some("subscriptions")));
        // Refreshing an existing one is still fine; another principal has its own quota.
        assert!(sub(&store, &agents(), sub_params("approval.requested", json!({ "thread_id": "t0" }), &secret(1), None), t0()).await.get("result").is_some());
        assert!(sub(&store, &bot("b1"), sub_params("thread.needs_user", json!({}), &secret(1), None), t0()).await.get("result").is_some());
    }

    #[tokio::test]
    async fn unsubscribe_is_idempotent_and_binds_like_subscribe() {
        let store = MemStore::default();
        sub(&store, &bot("b1"), sub_params("thread.needs_user", json!({}), &secret(1), None), t0()).await;
        let un = json!({ "name": "thread.needs_user", "arguments": {}, "delivery": { "url": "https://cb.example.com/hook/1" } });
        assert_eq!(handle(&store, &bot("b1"), &json!(2), "events/unsubscribe", &un, t0()).await["result"], json!({}));
        assert!(store.subs.lock().unwrap().is_empty());
        assert_eq!(handle(&store, &bot("b1"), &json!(3), "events/unsubscribe", &un, t0()).await["result"], json!({}));
        let unknown = json!({ "name": "x.y", "delivery": { "url": "https://cb.example.com/hook/1" } });
        assert_eq!(handle(&store, &bot("b1"), &json!(4), "events/unsubscribe", &unknown, t0()).await["result"], json!({}));
    }

    #[test]
    fn terminated_is_signed_standard_webhooks_with_the_subscription_header() {
        let (headers, body) = terminated_request("sub_abc", &secret(3), "approval_revoked", 1_700_000_000).unwrap();
        let h: HashMap<_, _> = headers.into_iter().collect();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["type"], "terminated");
        assert_eq!(v["error"]["code"], codes::FORBIDDEN);
        assert_eq!(v["error"]["data"]["reason"], "approval_revoked");
        assert_eq!(h["X-MCP-Subscription-Id"], "sub_abc");
        let key = mcp_protocol::webhooks::parse_secret(&secret(3)).unwrap();
        assert!(mcp_protocol::webhooks::verify(&key, &h["webhook-id"], &h["webhook-timestamp"], &h["webhook-signature"], &body, 1_700_000_000).is_ok());
    }

    // ── Postgres: the real store, the delivery worker, revoke ─────────────────

    mod pg {
        use super::*;
        use crate::routes::platform_v1::events::{deliver_due_with, envelope, sign, Poster};
        use crate::routes::test_support::{events_backbone_schema, test_pool};

        /// Records every POST and answers with a scripted status per URL.
        #[derive(Default)]
        struct Rec {
            sent: Mutex<Vec<(String, HashMap<String, String>, Vec<u8>)>>,
            status: Mutex<HashMap<String, u16>>,
        }

        #[async_trait::async_trait]
        impl Poster for Rec {
            async fn post(&self, url: &str, headers: &[(String, String)], body: &[u8]) -> Result<(u16, Vec<u8>), PostError> {
                self.sent.lock().unwrap().push((url.into(), headers.iter().cloned().collect(), body.to_vec()));
                Ok((self.status.lock().unwrap().get(url).copied().unwrap_or(200), vec![]))
            }
        }

        async fn db() -> PgPool {
            let db = test_pool().await;
            events_backbone_schema(&db).await;
            // Twice: 057 must be re-runnable.
            sqlx::raw_sql(&include_str!("../../migrations_pg/057_allternit_events_backbone.sql").replace("public.", "")).execute(&db).await.unwrap();
            db
        }

        /// Subscribe through the real Postgres store (challenge scripted OK).
        struct PgOk<'a>(PgEventsStore<'a>);
        #[async_trait::async_trait]
        impl EventsStore for PgOk<'_> {
            async fn get(&self, id: &str) -> Result<Option<Subscription>, String> { self.0.get(id).await }
            async fn live_count(&self, p: &Principal) -> Result<i64, String> { self.0.live_count(p).await }
            async fn save(&self, s: &Subscription) -> Result<(), String> { self.0.save(s).await }
            async fn remove(&self, id: &str, r: &str) -> Result<bool, String> { self.0.remove(id, r).await }
            async fn verify_callback(&self, _: &str, _: &str, _: &str, _: &str) -> Result<(), CallbackError> { Ok(()) }
        }

        async fn subscribe_pg(db: &PgPool, p: &Principal, name: &str, args: Value, url: &str, secret: &str, now: DateTime<Utc>) -> String {
            let store = PgOk(PgEventsStore { db });
            let mut params = sub_params(name, args, secret, None);
            params["delivery"]["url"] = json!(url);
            let r = handle(&store, p, &json!(1), "events/subscribe", &params, now).await;
            r["result"]["id"].as_str().unwrap_or_else(|| panic!("{r}")).to_string()
        }

        #[tokio::test]
        async fn store_round_trip_refresh_and_unsubscribe() {
            let db = db().await;
            let now = Utc::now();
            let id = subscribe_pg(&db, &agents(), "approval.requested", json!({ "bot_id": "b1" }), "https://93.184.216.34/a", &secret(1), now).await;
            let again = subscribe_pg(&db, &agents(), "approval.requested", json!({ "bot_id": "b1" }), "https://93.184.216.34/a", &secret(2), now).await;
            assert_eq!(id, again);
            let store = PgEventsStore { db: &db };
            let s = store.get(&id).await.unwrap().unwrap();
            assert_eq!((s.name.as_str(), s.secret.clone(), s.previous_secret.clone()), ("approval.requested", secret(2), Some(secret(1))));
            assert_eq!(s.arguments, json!({ "bot_id": "b1" }));
            assert_eq!(store.live_count(&agents()).await.unwrap(), 1);
            let n: i64 = sqlx::query_scalar("SELECT count(*) FROM platform_webhooks").fetch_one(&db).await.unwrap();
            assert_eq!(n, 1);
            assert!(store.remove(&id, "unsubscribed").await.unwrap());
            assert!(store.get(&id).await.unwrap().is_none());
            assert!(!store.remove(&id, "unsubscribed").await.unwrap());
            // Subscribing again revives the same row.
            subscribe_pg(&db, &agents(), "approval.requested", json!({ "bot_id": "b1" }), "https://93.184.216.34/a", &secret(2), now).await;
            assert!(store.get(&id).await.unwrap().is_some());
        }

        #[tokio::test]
        async fn mcp_deliveries_are_standard_webhooks_signed_and_filtered_by_arguments() {
            let db = db().await;
            let now = Utc::now();
            let b1 = subscribe_pg(&db, &agents(), "message.received", json!({ "bot_id": "b1" }), "https://93.184.216.34/b1", &secret(1), now).await;
            subscribe_pg(&db, &agents(), "message.received", json!({ "bot_id": "b2" }), "https://93.184.216.34/b2", &secret(1), now).await;
            let evt = allternit_events::emit_user_event(&db, "u1", "message.received", &json!({ "bot_id": "b1", "text": "hi" }), Some(now), "runtime:rt", Some("e1")).await.unwrap().unwrap();
            // Another user's event never reaches u1's subscriptions.
            allternit_events::emit_user_event(&db, "u2", "message.received", &json!({ "bot_id": "b1" }), None, "runtime:rt2", Some("e1")).await.unwrap();
            let rec = Rec::default();
            assert_eq!(deliver_due_with(&db, &rec).await.unwrap(), 1);
            let sent = rec.sent.lock().unwrap();
            let (url, h, body) = &sent[0];
            assert_eq!(url, "https://93.184.216.34/b1");
            assert_eq!(h["X-MCP-Subscription-Id"], b1);
            assert_eq!(h["webhook-id"], evt, "webhook-id is the event id, stable across retries");
            let v: Value = serde_json::from_slice(body).unwrap();
            assert_eq!((v["eventId"].as_str(), v["name"].as_str(), v["data"]["text"].as_str(), v["cursor"].clone()), (Some(evt.as_str()), Some("message.received"), Some("hi"), Value::Null));
            assert!(v["timestamp"].as_str().unwrap().ends_with('Z'));
            // Signed with the current key and, during the rotation window, the previous one too.
            let key = mcp_protocol::webhooks::parse_secret(&secret(1)).unwrap();
            assert!(mcp_protocol::webhooks::verify(&key, &h["webhook-id"], &h["webhook-timestamp"], &h["webhook-signature"], body, Utc::now().timestamp()).is_ok());
            assert!(!h.contains_key("allternit-signature"));
        }

        #[tokio::test]
        async fn a_rotated_secret_dual_signs_during_the_grace_window() {
            let db = db().await;
            let now = Utc::now();
            subscribe_pg(&db, &agents(), "thread.needs_user", json!({}), "https://93.184.216.34/r", &secret(1), now).await;
            subscribe_pg(&db, &agents(), "thread.needs_user", json!({}), "https://93.184.216.34/r", &secret(2), now).await;
            allternit_events::emit_user_event(&db, "u1", "thread.needs_user", &json!({}), None, "cloud", None).await.unwrap();
            let rec = Rec::default();
            deliver_due_with(&db, &rec).await.unwrap();
            let sent = rec.sent.lock().unwrap();
            let (_, h, body) = &sent[0];
            assert_eq!(h["webhook-signature"].split(' ').count(), 2);
            for s in [secret(1), secret(2)] {
                let key = mcp_protocol::webhooks::parse_secret(&s).unwrap();
                assert!(mcp_protocol::webhooks::verify(&key, &h["webhook-id"], &h["webhook-timestamp"], &h["webhook-signature"], body, Utc::now().timestamp()).is_ok());
            }
        }

        #[tokio::test]
        async fn gone_ends_the_subscription_413_is_not_retried_and_5xx_is() {
            let db = db().await;
            let now = Utc::now();
            let gone = subscribe_pg(&db, &agents(), "call.ended", json!({ "bot_id": "g" }), "https://93.184.216.34/gone", &secret(1), now).await;
            subscribe_pg(&db, &agents(), "call.ended", json!({ "bot_id": "t" }), "https://93.184.216.34/big", &secret(1), now).await;
            subscribe_pg(&db, &agents(), "call.ended", json!({ "bot_id": "f" }), "https://93.184.216.34/flaky", &secret(1), now).await;
            for bot in ["g", "t", "f"] {
                allternit_events::emit_user_event(&db, "u1", "call.ended", &json!({ "bot_id": bot }), None, "cloud", None).await.unwrap();
            }
            let rec = Rec::default();
            rec.status.lock().unwrap().extend([("https://93.184.216.34/gone".to_string(), 410), ("https://93.184.216.34/big".to_string(), 413), ("https://93.184.216.34/flaky".to_string(), 503)]);
            assert_eq!(deliver_due_with(&db, &rec).await.unwrap(), 3);
            let rows: Vec<(String, String, i32)> = sqlx::query_as("SELECT w.url, d.state, d.attempts FROM platform_webhook_deliveries d JOIN platform_webhooks w ON w.id = d.webhook_id ORDER BY w.url").fetch_all(&db).await.unwrap();
            assert_eq!(rows, vec![
                ("https://93.184.216.34/big".into(), "failed".into(), 1),
                ("https://93.184.216.34/flaky".into(), "pending".into(), 1),
                ("https://93.184.216.34/gone".into(), "failed".into(), 1),
            ]);
            let (ended, reason): (bool, Option<String>) = sqlx::query_as("SELECT deleted_at IS NOT NULL, deactivated_reason FROM platform_webhooks WHERE id = $1").bind(&gone).fetch_one(&db).await.unwrap();
            assert!(ended);
            assert_eq!(reason.as_deref(), Some("gone"));
            // A gone subscription gets nothing further.
            allternit_events::emit_user_event(&db, "u1", "call.ended", &json!({ "bot_id": "g" }), None, "cloud", None).await.unwrap();
            let to_gone: i64 = sqlx::query_scalar("SELECT count(*) FROM platform_webhook_deliveries WHERE webhook_id = $1").bind(&gone).fetch_one(&db).await.unwrap();
            assert_eq!(to_gone, 1);
        }

        #[tokio::test]
        async fn oversize_and_expired_deliveries_are_never_sent() {
            let db = db().await;
            let now = Utc::now();
            let id = subscribe_pg(&db, &agents(), "usage.threshold", json!({}), "https://93.184.216.34/u", &secret(1), now).await;
            let big = json!({ "blob": "x".repeat(mcp_protocol::events::MAX_EVENT_BYTES) });
            allternit_events::emit_user_event(&db, "u1", "usage.threshold", &big, None, "cloud", None).await.unwrap();
            allternit_events::emit_user_event(&db, "u1", "usage.threshold", &json!({ "meter": "m" }), None, "cloud", None).await.unwrap();
            sqlx::query("UPDATE platform_webhooks SET refresh_before = now() - interval '1 second' WHERE id = $1").bind(&id).execute(&db).await.unwrap();
            let rec = Rec::default();
            deliver_due_with(&db, &rec).await.unwrap();
            assert!(rec.sent.lock().unwrap().is_empty());
            let errors: Vec<String> = sqlx::query_scalar("SELECT last_error FROM platform_webhook_deliveries WHERE state = 'failed' ORDER BY last_error").fetch_all(&db).await.unwrap();
            assert_eq!(errors.len(), 2);
            assert!(errors.iter().any(|e| e.contains("256 KiB")) || errors.iter().all(|e| e.contains("expired")), "{errors:?}");
            // An expired subscription isn't fanned out to at all.
            allternit_events::emit_user_event(&db, "u1", "usage.threshold", &json!({}), None, "cloud", None).await.unwrap();
            let n: i64 = sqlx::query_scalar("SELECT count(*) FROM platform_webhook_deliveries").fetch_one(&db).await.unwrap();
            assert_eq!(n, 2);
        }

        #[tokio::test]
        async fn revoking_the_approval_ends_only_that_principals_subscriptions() {
            let db = db().await;
            let now = Utc::now();
            let mine = subscribe_pg(&db, &agents(), "approval.requested", json!({}), "https://93.184.216.34/m", &secret(1), now).await;
            let other_client = Principal { client: "claude".into(), ..agents() };
            let theirs = subscribe_pg(&db, &other_client, "approval.requested", json!({}), "https://93.184.216.34/m", &secret(1), now).await;
            let bot_sub = subscribe_pg(&db, &bot("b1"), "thread.needs_user", json!({}), "https://93.184.216.34/m", &secret(1), now).await;
            allternit_events::emit_user_event(&db, "u1", "approval.requested", &json!({}), None, "cloud", None).await.unwrap();
            assert_eq!(terminate_principal(&db, "u1", "chatgpt", "agents", "approval_revoked").await.unwrap(), 1);
            let live: Vec<String> = sqlx::query_scalar("SELECT id FROM platform_webhooks WHERE deleted_at IS NULL ORDER BY id").fetch_all(&db).await.unwrap();
            let mut want = vec![theirs, bot_sub];
            want.sort();
            assert_eq!(live, want);
            let (reason,): (Option<String>,) = sqlx::query_as("SELECT deactivated_reason FROM platform_webhooks WHERE id = $1").bind(&mine).fetch_one(&db).await.unwrap();
            assert_eq!(reason.as_deref(), Some("approval_revoked"));
            // Its queued delivery is dropped, the other client's still goes.
            let pending: Vec<String> = sqlx::query_scalar("SELECT webhook_id FROM platform_webhook_deliveries WHERE state = 'pending'").fetch_all(&db).await.unwrap();
            assert!(!pending.contains(&mine) && pending.len() == 1);
        }

        #[tokio::test]
        async fn platform_webhooks_keep_the_051_body_and_signature_byte_for_byte() {
            let db = db().await;
            let project = "prj_1".to_string();
            sqlx::query("INSERT INTO platform_projects (id, owner_user_id, name) VALUES ('prj_1', 'o', 'p')").execute(&db).await.unwrap();
            sqlx::query("INSERT INTO platform_webhooks (id, project_id, url, events, secret) VALUES ('wh_1', $1, 'https://93.184.216.34/p', ARRAY['*'], 'whsec_plain')").bind(&project).execute(&db).await.unwrap();
            let evt = crate::routes::platform_v1::events::emit_event(&db, &project, Some("acct_1"), "message.received", json!({ "x": 1 })).await.unwrap();
            let rec = Rec::default();
            deliver_due_with(&db, &rec).await.unwrap();
            let sent = rec.sent.lock().unwrap();
            let (_, h, body) = &sent[0];
            let created: DateTime<Utc> = sqlx::query_scalar("SELECT created_at FROM platform_events WHERE id = $1").bind(&evt).fetch_one(&db).await.unwrap();
            let want = serde_json::to_vec(&envelope(&evt, "message.received", created, &project, Some("acct_1"), &json!({ "x": 1 }))).unwrap();
            assert_eq!(body, &want);
            let ts: i64 = h["allternit-signature"].split(',').next().unwrap().trim_start_matches("t=").parse().unwrap();
            assert_eq!(h["allternit-signature"], sign("whsec_plain", ts, body));
            assert_eq!(h["user-agent"], "Allternit-Webhooks/1.0");
            assert_eq!(h["content-type"], "application/json");
            assert!(!h.contains_key("webhook-signature") && !h.contains_key("X-MCP-Subscription-Id"));
            // A Platform 410 keeps the 051 rule: retried, endpoint kept.
            drop(sent);
            sqlx::query("UPDATE platform_webhook_deliveries SET state = 'pending', next_attempt_at = now()").execute(&db).await.unwrap();
            rec.status.lock().unwrap().insert("https://93.184.216.34/p".into(), 410);
            deliver_due_with(&db, &rec).await.unwrap();
            let (state, deleted): (String, bool) = sqlx::query_as("SELECT d.state, w.deleted_at IS NOT NULL FROM platform_webhook_deliveries d JOIN platform_webhooks w ON w.id = d.webhook_id").fetch_one(&db).await.unwrap();
            assert_eq!((state.as_str(), deleted), ("pending", false));
        }

        #[tokio::test]
        async fn a_platform_endpoint_can_opt_into_standard_webhooks() {
            let db = db().await;
            sqlx::query("INSERT INTO platform_projects (id, owner_user_id, name) VALUES ('prj_1', 'o', 'p')").execute(&db).await.unwrap();
            sqlx::query("INSERT INTO platform_webhooks (id, project_id, url, events, secret, signer) VALUES ('wh_1', 'prj_1', 'https://93.184.216.34/p', ARRAY['message.status'], $1, 'standard_webhooks')")
                .bind(secret(5))
                .execute(&db)
                .await
                .unwrap();
            let evt = crate::routes::platform_v1::events::emit_event(&db, "prj_1", None, "message.status", json!({ "id": "m1" })).await.unwrap();
            let rec = Rec::default();
            deliver_due_with(&db, &rec).await.unwrap();
            let sent = rec.sent.lock().unwrap();
            let (_, h, body) = &sent[0];
            // Same Platform event object; Standard Webhooks headers; no MCP header.
            let v: Value = serde_json::from_slice(body).unwrap();
            assert_eq!((v["object"].as_str(), v["type"].as_str(), v["project_id"].as_str()), (Some("event"), Some("message.status"), Some("prj_1")));
            assert_eq!(h["webhook-id"], evt);
            let key = mcp_protocol::webhooks::parse_secret(&secret(5)).unwrap();
            assert!(mcp_protocol::webhooks::verify(&key, &h["webhook-id"], &h["webhook-timestamp"], &h["webhook-signature"], body, Utc::now().timestamp()).is_ok());
            assert!(!h.contains_key("allternit-signature") && !h.contains_key("X-MCP-Subscription-Id"));
        }
    }
}
