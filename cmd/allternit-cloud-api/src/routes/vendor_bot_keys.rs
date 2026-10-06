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
//! - `GET    /api/v1/vendor-bots/:id/cli-keys` → `{keys: [{id, label, prefix, createdAt, lastUsedAt}]}` (revoked ones are not listed)
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
    let rows: Result<Vec<(String, String, String, chrono::DateTime<chrono::Utc>, Option<chrono::DateTime<chrono::Utc>>)>, _> = sqlx::query_as(
        "SELECT id, label, key_prefix, created_at, last_used_at FROM vendor_bot_keys WHERE user_id = $1 AND vendor_bot_id = $2 AND revoked_at IS NULL ORDER BY created_at DESC",
    )
    .bind(&user)
    .bind(&vendor_bot_id)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => Json(json!({
            "keys": rows.into_iter().map(|(id, label, prefix, created, used)| json!({
                "id": id, "label": label, "prefix": prefix, "createdAt": created.to_rfc3339(), "lastUsedAt": used.map(|u| u.to_rfc3339())
            })).collect::<Vec<_>>()
        }))
        .into_response(),
        Err(e) => internal(e),
    }
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

    #[test]
    fn the_instructions_come_from_the_one_shared_file() {
        assert!(INSTRUCTIONS.contains("allternit-bot help") && INSTRUCTIONS.contains("get_ticket"));
    }
}
