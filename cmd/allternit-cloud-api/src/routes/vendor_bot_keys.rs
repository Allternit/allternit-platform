//! Scoped keys for the `allternit-bot` CLI. Migration `040_vendor_bot_keys.sql`.
//!
//! A vendor agent in its own sandbox has a shell but no OAuth browser. Its owner issues it a key on
//! the vendor bot's connector page; the CLI sends `Authorization: Bearer abk_…` to the MCP edge
//! (`mcp_edge.rs`), which calls [`verify_key`], learns the owner and the one bot the key opens, and
//! relays to the owner's runtime exactly like an OAuth call (the runtime still applies every gate).
//! Only the SHA-256 of a key is stored; the plaintext comes back once, at issue.
//!
//! Routes (Clerk session or `compute`-scoped API key; 503 `mcp_edge_not_configured` until
//! `MCP_PUBLIC_URL` is set, since a key is useless without the edge):
//! - `POST   /api/v1/vendor-bots/:id/cli-keys` {label?} → 201 `{id, key, prefix, label, createdAt, mcpUrl, instructionsUrl}`
//! - `GET    /api/v1/vendor-bots/:id/cli-keys` → `{keys: [{id, label, prefix, createdAt, lastUsedAt, subscriptions}]}`
//!   (revoked ones are not listed). `subscriptions` are the MCP Events subscriptions the agent made with that
//!   key on this bot's connector (principal `cli-key:<id>`, target `bot:<id>`), the same shape as
//!   `GET /api/v1/mcp/approvals`: `[{id, name, arguments, createdAt, refreshBefore, active, lastDeliveryAt,
//!   lastError}]`, or `null` when they couldn't be read (the keys still list). The owner stops one with
//!   `DELETE /api/v1/mcp/events/subscriptions/:id`; revoking the key ends all of them.
//! - `DELETE /api/v1/vendor-bots/:id/cli-keys/:keyId` → `{ok: true}`; 404 for a key that isn't the caller's
//! - `GET    /bots/:id/instructions` and `GET /mcp/bots/:id/instructions` (key as Bearer) → `text/plain`,
//!   what an agent reads to learn the tools; 401 for a missing, wrong, revoked or other-bot key.

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get},
    Json, Router,
};
use rand::{distributions::Alphanumeric, Rng};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::sync::Arc;

use crate::ApiState;

pub const KEY_PREFIX: &str = "abk_";
/// Most live keys one vendor bot may hold.
const MAX_KEYS_PER_BOT: i64 = 10;
/// The same text allternit-api serves through `allternit-bot help`; one file, so it can't drift.
const INSTRUCTIONS: &str = include_str!("../../../allternit-api/assets/vendor-bot-instructions.txt");

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/vendor-bots/:id/cli-keys", get(list_route).post(issue_route))
        .route("/api/v1/vendor-bots/:id/cli-keys/:key_id", delete(revoke_route))
        .route("/bots/:id/instructions", get(instructions_route))
        .route("/mcp/bots/:id/instructions", get(instructions_route))
}

fn hash_key(key: &str) -> String {
    hex::encode(Sha256::digest(key.as_bytes()))
}

fn not_configured() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "mcp_edge_not_configured" }))).into_response()
}

fn internal(error: sqlx::Error) -> Response {
    tracing::error!("vendor bot keys: {error}");
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "internal" }))).into_response()
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::AUTHORIZATION)?.to_str().ok()?.strip_prefix("Bearer ").map(str::trim).filter(|t| !t.is_empty())
}

/// The owner and key id behind `key`, when it is a live key for exactly `vendor_bot_id`.
pub async fn verify_key(db: &PgPool, key: &str, vendor_bot_id: &str) -> Result<Option<(String, String)>, sqlx::Error> {
    if !key.starts_with(KEY_PREFIX) || key.len() > 128 {
        return Ok(None);
    }
    let row: Option<(String, String)> = sqlx::query_as(
        "UPDATE vendor_bot_keys SET last_used_at = CASE WHEN last_used_at IS NULL OR last_used_at < now() - interval '1 minute' THEN now() ELSE last_used_at END
         WHERE key_hash = $1 AND vendor_bot_id = $2 AND revoked_at IS NULL RETURNING user_id, id",
    )
    .bind(hash_key(key))
    .bind(vendor_bot_id)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct IssueBody {
    label: String,
}

async fn issue_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(vendor_bot_id): Path<String>, body: Option<Json<IssueBody>>) -> Response {
    let Some(base) = crate::routes::mcp_edge::public_mcp_url_for_keys() else { return not_configured() };
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await {
        Ok(u) => u.id,
        Err(e) => return e.into_response(),
    };
    let label: String = body.map(|b| b.0.label).unwrap_or_default().trim().chars().take(60).collect();
    match issue(&state.db, &user, &vendor_bot_id, &label).await {
        Ok(Some((id, key, created_at))) => {
            let mcp_url = format!("{}/bots/{vendor_bot_id}", base.trim_end_matches('/'));
            (
                StatusCode::CREATED,
                Json(json!({
                    "id": id, "key": key, "prefix": &key[..12], "label": label, "createdAt": created_at,
                    "mcpUrl": mcp_url, "instructionsUrl": format!("{mcp_url}/instructions")
                })),
            )
                .into_response()
        }
        Ok(None) => (StatusCode::TOO_MANY_REQUESTS, Json(json!({ "error": "too_many_keys" }))).into_response(),
        Err(e) => internal(e),
    }
}

/// `None` = the bot already has the most keys it may. Returns `(id, plaintext, created_at)`.
pub async fn issue(db: &PgPool, user: &str, vendor_bot_id: &str, label: &str) -> Result<Option<(String, String, String)>, sqlx::Error> {
    let live: i64 = sqlx::query_scalar("SELECT count(*) FROM vendor_bot_keys WHERE user_id = $1 AND vendor_bot_id = $2 AND revoked_at IS NULL").bind(user).bind(vendor_bot_id).fetch_one(db).await?;
    if live >= MAX_KEYS_PER_BOT {
        return Ok(None);
    }
    let secret: String = rand::thread_rng().sample_iter(&Alphanumeric).take(40).map(char::from).collect();
    let key = format!("{KEY_PREFIX}{secret}");
    let id = format!("vbk_{}", uuid::Uuid::new_v4().simple());
    let created: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "INSERT INTO vendor_bot_keys (id, user_id, vendor_bot_id, label, key_hash, key_prefix) VALUES ($1,$2,$3,$4,$5,$6) RETURNING created_at",
    )
    .bind(&id)
    .bind(user)
    .bind(vendor_bot_id)
    .bind(label)
    .bind(hash_key(&key))
    .bind(&key[..12])
    .fetch_one(db)
    .await?;
    Ok(Some((id, key, created.to_rfc3339())))
}

async fn list_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(vendor_bot_id): Path<String>) -> Response {
    if crate::routes::mcp_edge::public_mcp_url_for_keys().is_none() {
        return not_configured();
    }
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await {
        Ok(u) => u.id,
        Err(e) => return e.into_response(),
    };
    match list_keys(&state.db, &user, &vendor_bot_id).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => internal(e),
    }
}

/// `{keys: [...]}` for the owner's live keys on one vendor bot, each with its event subscriptions.
pub async fn list_keys(db: &PgPool, user: &str, vendor_bot_id: &str) -> Result<serde_json::Value, sqlx::Error> {
    let rows: Vec<(String, String, String, chrono::DateTime<chrono::Utc>, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT id, label, key_prefix, created_at, last_used_at FROM vendor_bot_keys WHERE user_id = $1 AND vendor_bot_id = $2 AND revoked_at IS NULL ORDER BY created_at DESC",
    )
    .bind(user)
    .bind(vendor_bot_id)
    .fetch_all(db)
    .await?;
    // The keys still list if the subscriptions can't be read (`null`).
    let subs = crate::routes::mcp_events::owner_subscriptions(db, user).await.map_err(|e| tracing::error!("vendor bot keys: listing subscriptions failed: {e}")).ok();
    let target = format!("bot:{vendor_bot_id}");
    let now = chrono::Utc::now();
    Ok(json!({
        "keys": rows.into_iter().map(|(id, label, prefix, created, used)| {
            let client = format!("cli-key:{id}");
            let subscriptions = match &subs {
                Some(subs) => subs.iter().filter(|s| s.client == client && s.target == target).map(|s| s.to_json(now)).collect(),
                None => serde_json::Value::Null,
            };
            json!({
                "id": id, "label": label, "prefix": prefix, "createdAt": created.to_rfc3339(), "lastUsedAt": used.map(|u| u.to_rfc3339()),
                "subscriptions": subscriptions,
            })
        }).collect::<Vec<_>>()
    }))
}

async fn revoke_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path((vendor_bot_id, key_id)): Path<(String, String)>) -> Response {
    if crate::routes::mcp_edge::public_mcp_url_for_keys().is_none() {
        return not_configured();
    }
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await {
        Ok(u) => u.id,
        Err(e) => return e.into_response(),
    };
    match sqlx::query("UPDATE vendor_bot_keys SET revoked_at = now() WHERE id = $1 AND user_id = $2 AND vendor_bot_id = $3 AND revoked_at IS NULL")
        .bind(&key_id)
        .bind(&user)
        .bind(&vendor_bot_id)
        .execute(&state.db)
        .await
    {
        Ok(r) if r.rows_affected() > 0 => {
            // A key is its own MCP principal (`cli-key:<id>`): its event subscriptions end with it.
            if let Err(e) = crate::routes::mcp_events::terminate_principal(&state.db, &user, &format!("cli-key:{key_id}"), &format!("bot:{vendor_bot_id}"), "key_revoked").await {
                tracing::error!("vendor bot keys: ending subscriptions failed: {e}");
            }
            Json(json!({ "ok": true })).into_response()
        }
        Ok(_) => (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response(),
        Err(e) => internal(e),
    }
}

async fn instructions_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(vendor_bot_id): Path<String>) -> Response {
    let Some(base) = crate::routes::mcp_edge::public_mcp_url_for_keys() else { return not_configured() };
    let unauthorized = || {
        let mut r = (StatusCode::UNAUTHORIZED, Json(json!({ "error": "invalid_key" }))).into_response();
        r.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        r
    };
    let Some(key) = bearer(&headers) else { return unauthorized() };
    match verify_key(&state.db, key, &vendor_bot_id).await {
        Ok(Some(_)) => {
            let text = format!("{INSTRUCTIONS}\nMCP endpoint for this bot (POST JSON-RPC, same key as Bearer):\n  {}/bots/{vendor_bot_id}\n", base.trim_end_matches('/'));
            let mut r = (StatusCode::OK, text).into_response();
            r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
            r
        }
        Ok(None) => unauthorized(),
        Err(e) => internal(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::test_pool;

    async fn pool() -> PgPool {
        let db = test_pool().await;
        sqlx::raw_sql(&include_str!("../../migrations_pg/040_vendor_bot_keys.sql").replace("public.", "")).execute(&db).await.unwrap();
        db
    }

    #[tokio::test]
    async fn a_key_opens_exactly_its_bot_and_only_until_revoked() {
        let db = pool().await;
        let (id, key, _) = issue(&db, "user_a", "bot_1", "sandbox").await.unwrap().unwrap();
        assert!(key.starts_with("abk_") && key.len() == 44);
        assert_eq!(verify_key(&db, &key, "bot_1").await.unwrap(), Some(("user_a".into(), id.clone())));
        assert_eq!(verify_key(&db, &key, "bot_2").await.unwrap(), None, "a key for another bot");
        assert_eq!(verify_key(&db, "abk_nope", "bot_1").await.unwrap(), None);
        assert_eq!(verify_key(&db, "ak_something", "bot_1").await.unwrap(), None, "wrong prefix");
        sqlx::query("UPDATE vendor_bot_keys SET revoked_at = now() WHERE id = $1").bind(&id).execute(&db).await.unwrap();
        assert_eq!(verify_key(&db, &key, "bot_1").await.unwrap(), None, "revoked");
    }

    #[tokio::test]
    async fn only_a_hash_is_stored_and_the_cap_holds() {
        let db = pool().await;
        let (_, key, _) = issue(&db, "user_a", "bot_1", "").await.unwrap().unwrap();
        let rows: Vec<(String, String)> = sqlx::query_as("SELECT key_hash, key_prefix FROM vendor_bot_keys").fetch_all(&db).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_ne!(rows[0].0, key);
        assert_eq!(rows[0].0.len(), 64);
        assert_eq!(rows[0].1, &key[..12]);
        for _ in 1..MAX_KEYS_PER_BOT {
            assert!(issue(&db, "user_a", "bot_1", "").await.unwrap().is_some());
        }
        assert!(issue(&db, "user_a", "bot_1", "").await.unwrap().is_none(), "the 11th key");
        assert!(issue(&db, "user_a", "bot_2", "").await.unwrap().is_some(), "the cap is per bot");
    }

    #[tokio::test]
    async fn key_subscriptions_are_listed_and_stoppable_only_by_their_owner() {
        let db = test_pool().await;
        crate::routes::test_support::events_backbone_schema(&db).await;
        sqlx::raw_sql(&include_str!("../../migrations_pg/040_vendor_bot_keys.sql").replace("public.", "")).execute(&db).await.unwrap();
        let (key_id, _, _) = issue(&db, "user_a", "bot_1", "sandbox").await.unwrap().unwrap();
        let (other_key, _, _) = issue(&db, "user_a", "bot_1", "").await.unwrap().unwrap();
        // Subscriptions the edge stored for the key's principal, another key's, an OAuth app's on the same bot,
        // and someone else's.
        for (id, user, client, target, name) in [
            ("sub_k1", "user_a", format!("cli-key:{key_id}"), "bot:bot_1", "vendor.ticket.created"),
            ("sub_k2", "user_a", format!("cli-key:{other_key}"), "bot:bot_1", "message.received"),
            ("sub_app", "user_a", "chatgpt".to_string(), "bot:bot_1", "vendor.ticket.created"),
            ("sub_b", "user_b", format!("cli-key:{key_id}"), "bot:bot_1", "vendor.ticket.created"),
        ] {
            sqlx::query(
                "INSERT INTO platform_webhooks (id, kind, signer, user_id, client_id, target, url, events, secret, arguments, refresh_before, verified_at) \
                 VALUES ($1, 'mcp_subscription', 'standard_webhooks', $2, $3, $4, 'https://93.184.216.34/cb', ARRAY[$5], 'whsec_x', '{\"thread_id\":\"t1\"}', now() + interval '1 day', now())",
            )
            .bind(id).bind(user).bind(&client).bind(target).bind(name)
            .execute(&db).await.unwrap();
        }
        let v = list_keys(&db, "user_a", "bot_1").await.unwrap();
        let keys = v["keys"].as_array().unwrap();
        assert_eq!(keys.len(), 2);
        let k1 = keys.iter().find(|k| k["id"] == key_id.as_str()).unwrap();
        let subs = k1["subscriptions"].as_array().unwrap();
        assert_eq!(subs.len(), 1, "{k1}");
        let s = &subs[0];
        assert_eq!((s["id"].as_str(), s["name"].as_str(), s["active"].as_bool()), (Some("sub_k1"), Some("vendor.ticket.created"), Some(true)));
        assert_eq!(s["arguments"], json!({ "thread_id": "t1" }));
        for key in ["createdAt", "refreshBefore", "lastDeliveryAt", "lastError"] {
            assert!(s.get(key).is_some(), "{key}");
        }
        assert!(s.get("url").is_none() && s.get("secret").is_none());
        let k2 = keys.iter().find(|k| k["id"] == other_key.as_str()).unwrap();
        assert_eq!(k2["subscriptions"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap()).collect::<Vec<_>>(), ["sub_k2"]);
        // Another bot's keys list shows none of these.
        assert!(list_keys(&db, "user_a", "bot_2").await.unwrap()["keys"].as_array().unwrap().is_empty());

        // Only the owner can stop a key's subscription (the same DELETE /api/v1/mcp/events/subscriptions/:id).
        assert!(!crate::routes::mcp_events::end_owner_subscription(&db, "user_b", "sub_k1").await.unwrap());
        assert!(crate::routes::mcp_events::end_owner_subscription(&db, "user_a", "sub_k1").await.unwrap());
        let v = list_keys(&db, "user_a", "bot_1").await.unwrap();
        let k1 = v["keys"].as_array().unwrap().iter().find(|k| k["id"] == key_id.as_str()).unwrap().clone();
        assert!(k1["subscriptions"].as_array().unwrap().is_empty());
        // user_b's row with the same principal is untouched.
        let left: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar("SELECT deleted_at FROM platform_webhooks WHERE id = 'sub_b'").fetch_one(&db).await.unwrap();
        assert!(left.is_none());
    }

    #[test]
    fn the_instructions_come_from_the_one_shared_file() {
        assert!(INSTRUCTIONS.contains("allternit-bot help") && INSTRUCTIONS.contains("get_ticket"));
    }
}
