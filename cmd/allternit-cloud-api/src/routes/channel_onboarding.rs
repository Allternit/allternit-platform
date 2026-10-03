//! Telegram Managed Bots onboarding (Bot API 9.6, phase 1 backend — see
//! docs/CHANNELS_MAP.md wave 2 and docs/TELEGRAM_MANAGED_PHASE_1_TASK.md).
//!
//! Today, connecting Telegram means pasting a @BotFather token into the
//! runtime. This module is the no-pasted-secrets flow: the user taps one deep
//! link, Telegram creates a child bot managed by Allternit's manager bot, and
//! the manager bot fetches the child token over the Bot API and relays it
//! straight to the user's runtime. The token is never written to this
//! database and never logged.
//!
//! Flow:
//! 1. `POST /api/v1/channel-onboarding/telegram` (Clerk/API token, "compute")
//!    records a `telegram_onboarding` row (state `waiting`, 30 min expiry) and
//!    returns the `https://t.me/newbot/<manager>/<suggested>?name=<name>` link.
//! 2. The user confirms in Telegram; Telegram POSTs the manager bot's webhook
//!    (`POST /channels/telegram-manager/:secret`) a `managed_bot` update
//!    (`ManagedBotUpdated`). We match it to the one non-expired `waiting` row
//!    with the same suggested username (ambiguous or unknown → ignored), call
//!    `getManagedBotToken`, and relay `{botToken, allternitBotId, pairNonce,
//!    onboardingId}` to the user's runtime over the runtime relay, signed as
//!    the user with a data-plane JWT. Success → `connected`, else `failed`.
//! 3. The user opens the new bot and sends `/start <nonce>`; the runtime pairs
//!    the Telegram user id and calls `POST …/:id/paired` here (the pairNonce
//!    is the capability), moving the row to `paired`.
//!
//! Every route answers 503 `{error:"telegram_managed_not_configured"}` when
//! `ALLTERNIT_TELEGRAM_MANAGER_BOT_TOKEN` / `ALLTERNIT_TELEGRAM_MANAGER_WEBHOOK_SECRET`
//! are unset, so the UI can fall back to the paste-a-token form.

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

use super::runtime_relay::{relay_request_to_runtime_with, RelayRequest};
use crate::{ApiError, ApiState};

const TOKEN_ENV: &str = "ALLTERNIT_TELEGRAM_MANAGER_BOT_TOKEN";
const SECRET_ENV: &str = "ALLTERNIT_TELEGRAM_MANAGER_WEBHOOK_SECRET";
/// Bot API base. Production never sets this; tests point it at a fake
/// Telegram HTTP server. A local Bot API server can also be substituted.
const API_BASE_ENV: &str = "ALLTERNIT_TELEGRAM_API_BASE";
const DEFAULT_API_BASE: &str = "https://api.telegram.org";
/// The runtime route the child token is relayed to.
pub(crate) const MANAGED_CONNECT_PATH: &str = "/api/v1/gateway/channel-accounts/telegram/managed";
/// Where the manager bot's webhook lives on this service.
const MANAGER_WEBHOOK_SUFFIX: &str = "/channels/telegram-manager";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/channel-onboarding/telegram", post(create_h))
        .route(
            "/api/v1/channel-onboarding/telegram/available",
            get(available_h),
        )
        .route("/api/v1/channel-onboarding/telegram/:id", get(status_h))
        .route(
            "/api/v1/channel-onboarding/telegram/:id/paired",
            post(paired_h),
        )
        .route("/channels/telegram-manager/:secret", post(manager_webhook_h))
}

/// The manager-bot credentials. `None` = the whole surface is off.
struct ManagerConfig {
    token: String,
    webhook_secret: String,
}

fn managed_config() -> Option<ManagerConfig> {
    let token = std::env::var(TOKEN_ENV).ok().filter(|s| !s.trim().is_empty())?;
    let webhook_secret = std::env::var(SECRET_ENV).ok().filter(|s| !s.trim().is_empty())?;
    Some(ManagerConfig { token, webhook_secret })
}

fn api_base() -> String {
    std::env::var(API_BASE_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_API_BASE.to_string())
}

fn not_configured() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "telegram_managed_not_configured" })),
    )
        .into_response()
}

fn public_base() -> String {
    std::env::var("ALLTERNIT_CLOUD_API_URL")
        .unwrap_or_else(|_| "https://api.allternit.com".to_string())
        .trim_end_matches('/')
        .to_string()
}

// ---------------------------------------------------------------- Telegram client

/// Minimal Bot API client for the manager bot (getMe, setWebhook,
/// getManagedBotToken). Only what this module needs; child-bot calls live in
/// the runtime (allternit-api).
struct TelegramApi {
    base: String,
    token: String,
}

impl TelegramApi {
    fn new(token: String) -> Self {
        Self { base: api_base(), token }
    }

    async fn call(&self, method: &str, body: Value) -> Result<Value, String> {
        let url = format!("{}/bot{}/{method}", self.base, self.token);
        let response = reqwest::Client::new()
            .post(&url)
            .timeout(std::time::Duration::from_secs(15))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Telegram {method} request failed: {e}"))?;
        let status = response.status().as_u16();
        let payload: Value = response
            .json()
            .await
            .map_err(|e| format!("Telegram {method} response unreadable: {e}"))?;
        if (200..300).contains(&status) && payload.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(payload)
        } else {
            Err(format!(
                "Telegram {method} failed ({status}): {}",
                payload.get("description").and_then(Value::as_str).unwrap_or("unexpected reply")
            ))
        }
    }

    async fn get_me(&self) -> Result<String, String> {
        self.call("getMe", json!({}))
            .await?
            .get("result")
            .and_then(|r| r.get("username"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| "Telegram getMe returned no username".to_string())
    }

    async fn set_webhook(&self, url: &str, secret: &str) -> Result<(), String> {
        self.call("setWebhook", json!({ "url": url, "secret_token": secret }))
            .await
            .map(|_| ())
    }

    /// Bot API 9.6: the manager bot fetches a child bot's token by the child
    /// bot's user id. The token is returned to the caller and never stored.
    async fn get_managed_bot_token(&self, bot_user_id: i64) -> Result<String, String> {
        self.call("getManagedBotToken", json!({ "user_id": bot_user_id }))
            .await?
            .get("result")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| "Telegram getManagedBotToken returned no token".to_string())
    }
}

/// Point the manager bot at this service's webhook and validate its token.
/// Called at startup and idempotent (setWebhook overwrites). Behind the env
/// vars: a no-op when managed onboarding is not configured.
pub fn start_telegram_manager_onboarding() {
    tokio::spawn(async move {
        let Some(config) = managed_config() else { return };
        let api = TelegramApi::new(config.token);
        let url = format!("{}{}/{}", public_base(), MANAGER_WEBHOOK_SUFFIX, config.webhook_secret);
        loop {
            match api.set_webhook(&url, &config.webhook_secret).await {
                Ok(()) => {
                    match api.get_me().await {
                        Ok(username) => tracing::info!(%username, "Telegram manager bot webhook set"),
                        Err(error) => tracing::warn!("Telegram manager bot getMe after setWebhook: {error}"),
                    }
                    break;
                }
                Err(error) => {
                    tracing::warn!("Telegram manager bot setWebhook failed: {error}; retrying in 60s");
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                }
            }
        }
    });
}

// ---------------------------------------------------------------- pure helpers

/// `botName` -> suggested Telegram username: lowercase `[a-z0-9_]`, 5–32
/// chars, ends in `bot`, with a short random suffix so two wizards naming
/// bots the same do not collide. `suffix` is injected so tests stay
/// deterministic (callers pass 4 chars from `[a-z0-9]`).
fn suggested_username(bot_name: &str, suffix: &str) -> String {
    let mut core: String = bot_name
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_whitespace() {
                '_'
            } else {
                c
            }
        })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    while core.starts_with('_') {
        core.remove(0);
    }
    while core.ends_with('_') {
        core.pop();
    }
    // 23 + "_" + 4 + "_" + "bot" = 32, the Telegram maximum.
    core.truncate(23);
    while core.ends_with('_') {
        core.pop();
    }
    if core.is_empty() {
        core = "allternit".to_string();
    }
    format!("{core}_{suffix}_bot")
}

fn random_suffix() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut bytes = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| ALPHABET[(*b as usize) % ALPHABET.len()] as char).collect()
}

/// Single-use, URL-safe nonce (32 hex chars, ≤64 as the schema demands).
fn new_nonce() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Percent-encode a value for a query string (RFC 3986 unreserved kept).
fn query_escape(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Both the path secret and Telegram's echo of our `secret_token` must match
/// the configured webhook secret.
fn manager_secret_ok(path_secret: &str, header_secret: Option<&str>, configured: &str) -> bool {
    path_secret == configured && header_secret == Some(configured)
}

// ---------------------------------------------------------------- onboarding API

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateOnboarding {
    runtime_id: String,
    bot_id: String,
    bot_name: String,
}

async fn user_id(state: &ApiState, headers: &HeaderMap) -> Result<String, ApiError> {
    crate::auth::resolve_user_scoped(&state.db, headers, "compute")
        .await
        .map(|u| u.id)
}

/// Whether managed onboarding is configured — the runtime's `…/managed/
/// available` route asks this so the UI knows which flow to offer. Public on
/// purpose: it reveals only that the feature flag exists, like the 503 vs
/// response on the create route.
async fn available_h() -> Response {
    Json(json!({ "available": managed_config().is_some() })).into_response()
}

async fn create_h(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(body): Json<CreateOnboarding>,
) -> Result<Response, ApiError> {
    let Some(config) = managed_config() else {
        return Ok(not_configured());
    };
    let user = user_id(&state, &headers).await?;
    let bot_name = body.bot_name.trim();
    if bot_name.is_empty() || body.bot_id.trim().is_empty() || body.runtime_id.trim().is_empty() {
        return Err(ApiError::BadRequest("runtimeId, botId and botName are required".to_string()));
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
    // The deep link needs the manager bot's @username; getMe also validates
    // the token so a misconfigured manager bot fails here, not at the user.
    let api = TelegramApi::new(config.token);
    let manager_username = api
        .get_me()
        .await
        .map_err(|e| ApiError::ServiceUnavailable(format!("telegram_manager_unreachable: {e}")))?;
    let suggested = suggested_username(bot_name, &random_suffix());
    let nonce = new_nonce();
    let row: (i64,) = sqlx::query_as(
        "INSERT INTO telegram_onboarding (user_id, runtime_id, allternit_bot_id, bot_name, suggested_username, nonce)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(&user)
    .bind(&body.runtime_id)
    .bind(&body.bot_id)
    .bind(bot_name)
    .bind(&suggested)
    .bind(&nonce)
    .fetch_one(&state.db)
    .await?;
    let create_url = format!(
        "https://t.me/newbot/{manager_username}/{suggested}?name={}",
        query_escape(bot_name)
    );
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "onboardingId": row.0.to_string(),
            "createUrl": create_url,
            "state": "waiting",
            "expiresInMinutes": 30,
        })),
    )
        .into_response())
}

#[derive(Debug)]
#[allow(dead_code)]
struct OnboardingRow {
    id: i64,
    state: String,
    bot_username: Option<String>,
    error: Option<String>,
    expired: bool,
    nonce: String,
}

async fn load_onboarding(state: &ApiState, id: i64, user: &str) -> Result<OnboardingRow, ApiError> {
    // Lazily expire: a row past expires_at can never match or deliver, so
    // reading it flips the terminal state.
    let _ = sqlx::query(
        "UPDATE telegram_onboarding SET state = 'expired', updated_at = now()
          WHERE id = $1 AND state IN ('waiting', 'created') AND expires_at <= now()",
    )
    .bind(id)
    .execute(&state.db)
    .await;
    let row: Option<(String, Option<String>, Option<String>, String)> = sqlx::query_as(
        "SELECT state, tg_bot_username, error, nonce FROM telegram_onboarding WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user)
    .fetch_optional(&state.db)
    .await?;
    let (state_name, bot_username, error, nonce) =
        row.ok_or_else(|| ApiError::NotFound("Onboarding not found".to_string()))?;
    let expired = state_name == "expired";
    Ok(OnboardingRow { id, state: state_name, bot_username, error, expired, nonce })
}

async fn status_h(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    if managed_config().is_none() {
        return Ok(not_configured());
    }
    let user = user_id(&state, &headers).await?;
    let id: i64 = id.parse().map_err(|_| ApiError::BadRequest("invalid onboarding id".to_string()))?;
    let row = load_onboarding(&state, id, &user).await?;
    let pairable = !matches!(row.state.as_str(), "paired" | "failed" | "expired");
    let mut out = json!({ "state": row.state });
    if let Some(username) = row.bot_username {
        out["botUsername"] = json!(username);
        if pairable && !row.nonce.is_empty() {
            out["pairUrl"] = json!(format!("https://t.me/{username}?start={}", row.nonce));
        }
    }
    if let Some(error) = row.error {
        out["error"] = json!(error);
    }
    Ok(Json(out).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairedBody {
    pair_nonce: String,
}

/// The runtime reports that the user's Telegram `/start <nonce>` arrived.
/// The pairNonce is the capability: it was relayed only to the user's
/// runtime (sealed there), so knowing it proves the runtime paired the chat.
async fn paired_h(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Json(body): Json<PairedBody>,
) -> Result<Response, ApiError> {
    if managed_config().is_none() {
        return Ok(not_configured());
    }
    let id: i64 = id.parse().map_err(|_| ApiError::BadRequest("invalid onboarding id".to_string()))?;
    let updated = sqlx::query(
        "UPDATE telegram_onboarding SET state = 'paired', updated_at = now()
          WHERE id = $1 AND state = 'connected' AND nonce = $2",
    )
    .bind(id)
    .bind(&body.pair_nonce)
    .execute(&state.db)
    .await?
    .rows_affected();
    if updated == 0 {
        let pending: Option<String> =
            sqlx::query_scalar("SELECT state FROM telegram_onboarding WHERE id = $1")
                .bind(id)
                .fetch_optional(&state.db)
                .await?;
        return match pending {
            None => Err(ApiError::NotFound("Onboarding not found".to_string())),
            Some(_) => Ok((
                StatusCode::CONFLICT,
                Json(json!({ "error": "pairing_not_pending" })),
            )
                .into_response()),
        };
    }
    Ok(Json(json!({ "ok": true, "state": "paired" })).into_response())
}

// ---------------------------------------------------------------- manager webhook

#[derive(Debug)]
struct MatchedOnboarding {
    id: i64,
    user_id: String,
    runtime_id: String,
    allternit_bot_id: String,
    nonce: String,
}

/// Match a `managed_bot` update to the one waiting, unexpired onboarding row
/// with the same suggested username. Telegram usernames are case-insensitive;
/// the deep-link name is a pre-fill the user can edit, so an edited name
/// matches nothing and the row expires (the wizard just runs again).
async fn match_waiting_onboarding(
    db: &sqlx::PgPool,
    bot_username: &str,
) -> Result<Option<MatchedOnboarding>, ApiError> {
    let rows: Vec<(i64, String, String, String, String)> = sqlx::query_as(
        "SELECT id, user_id, runtime_id, allternit_bot_id, nonce FROM telegram_onboarding
          WHERE lower(suggested_username) = lower($1) AND state = 'waiting' AND expires_at > now()",
    )
    .bind(bot_username)
    .fetch_all(db)
    .await?;
    Ok(match rows.as_slice() {
        [one] => Some(MatchedOnboarding {
            id: one.0,
            user_id: one.1.clone(),
            runtime_id: one.2.clone(),
            allternit_bot_id: one.3.clone(),
            nonce: one.4.clone(),
        }),
        [] => None,
        many => {
            tracing::warn!(
                count = many.len(),
                username = bot_username,
                "managed_bot update matched more than one waiting onboarding; refusing all"
            );
            None
        }
    })
}

async fn mark_onboarding(db: &sqlx::PgPool, id: i64, state: &str, error: Option<&str>) {
    let _ = sqlx::query(
        "UPDATE telegram_onboarding SET state = $2, error = $3, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(state)
    .bind(error)
    .execute(db)
    .await;
}

/// Relay the child bot token to the user's runtime, authenticated as the user
/// with a data-plane JWT (the runtime verifies it against cloud-api's JWKS
/// and serves the request as the JWT's subject). The token rides in the relay
/// body only — never the database, never the logs.
async fn deliver_token_to_runtime(
    state: &ApiState,
    onboarding: &MatchedOnboarding,
    bot_token: &str,
) -> Result<(), String> {
    let body = json!({
        "botToken": bot_token,
        "allternitBotId": onboarding.allternit_bot_id,
        "pairNonce": onboarding.nonce,
        "onboardingId": onboarding.id.to_string(),
    });
    let jwt = crate::auth::dataplane_jwt::mint(
        &onboarding.user_id,
        &onboarding.runtime_id,
        "runtime:execute",
    )
    .map_err(|e| format!("data-plane token unavailable: {e}"))?;
    let mut trusted = HashMap::new();
    trusted.insert("authorization".to_string(), format!("Bearer {jwt}"));
    let response = relay_request_to_runtime_with(
        &state.db,
        &state.contabo_runtime_service,
        &state.quota_service,
        &state.provisioning_service,
        &onboarding.user_id,
        &onboarding.runtime_id,
        RelayRequest {
            method: "POST".to_string(),
            path: MANAGED_CONNECT_PATH.to_string(),
            headers: HashMap::new(),
            body: STANDARD.encode(body.to_string()),
            body_encoding: "base64".to_string(),
        },
        &[],
        trusted,
    )
    .await
    .map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(format!("runtime answered {status}"))
    }
}

async fn manager_webhook_h(
    State(state): State<Arc<ApiState>>,
    Path(secret): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let configured = match managed_config() {
        Some(config) => config,
        None => return not_configured(),
    };
    let header_secret = headers
        .get("x-telegram-bot-api-secret-token")
        .and_then(|v| v.to_str().ok());
    if !manager_secret_ok(&secret, header_secret, &configured.webhook_secret) {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "invalid_secret" }))).into_response();
    }
    // Telegram retries non-200s; everything past authentication is acked so a
    // retry storm cannot double-deliver (the row is claimed before the relay).
    let ack = || Json(json!({ "ok": true })).into_response();
    let Ok(update) = serde_json::from_slice::<Value>(&body) else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_json" }))).into_response();
    };
    let Some(managed) = update.get("managed_bot") else { return ack() };
    // Bot API 9.6 ManagedBotUpdated: `user` created the bot, `bot` is the new
    // bot (its token comes from getManagedBotToken, never from the update).
    let (Some(creator_id), Some(bot_id), Some(bot_username)) = (
        managed.pointer("/user/id").and_then(Value::as_i64),
        managed.pointer("/bot/id").and_then(Value::as_i64),
        managed.pointer("/bot/username").and_then(Value::as_str),
    ) else {
        tracing::warn!("managed_bot update missing user/bot fields; ignored");
        return ack();
    };
    let matched = match match_waiting_onboarding(&state.db, bot_username).await {
        Ok(matched) => matched,
        Err(error) => {
            // Fail loud so Telegram's retry redelivers the update.
            tracing::error!("telegram_onboarding match failed: {error}");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "match_failed" })))
                .into_response();
        }
    };
    let Some(onboarding) = matched else {
        return ack();
    };
    // Claim: exactly one webhook pass may proceed from here. A Telegram retry
    // now finds no `waiting` row and falls into the ack above.
    let claimed = sqlx::query(
        "UPDATE telegram_onboarding SET state = 'created', tg_bot_id = $2, tg_bot_username = $3,
                tg_creator_user_id = $4, updated_at = now()
          WHERE id = $1 AND state = 'waiting'",
    )
    .bind(onboarding.id)
    .bind(bot_id.to_string())
    .bind(bot_username)
    .bind(creator_id.to_string())
    .execute(&state.db)
    .await
    .map(|r| r.rows_affected())
    .unwrap_or(0);
    if claimed == 0 {
        return ack();
    }
    let api = TelegramApi::new(configured.token);
    let bot_token = match api.get_managed_bot_token(bot_id).await {
        Ok(token) => token,
        Err(error) => {
            tracing::warn!(onboarding_id = onboarding.id, "getManagedBotToken failed: {error}");
            mark_onboarding(&state.db, onboarding.id, "failed", Some(&error)).await;
            return ack();
        }
    };
    match deliver_token_to_runtime(&state, &onboarding, &bot_token).await {
        Ok(()) => mark_onboarding(&state.db, onboarding.id, "connected", None).await,
        Err(error) => {
            tracing::warn!(onboarding_id = onboarding.id, "managed token relay failed: {error}");
            mark_onboarding(&state.db, onboarding.id, "failed", Some(&error)).await;
        }
    }
    ack()
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use crate::routes::runtime_relay::{
        answer_test_request, register_test_connection, CloudMessage,
    };
    use super::*;
    use crate::auth::dataplane_jwt::SEED_ENV as DP_SEED_ENV;
    use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
    use crate::routes::test_support::{authed_request, seed_runtime_device, test_state, MockGateway, DEV_USER};
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tower::ServiceExt;

    const MANAGER_TOKEN: &str = "900001:manager-token";
    const WEBHOOK_SECRET: &str = "whsec-managed-test";
    const CHILD_TOKEN: &str = "555123:AAHchild-managed-token";

    // ------------------------------------------------------------ fake Telegram

    struct FakeTelegram {
        base_url: String,
        seen: Arc<Mutex<Vec<(String, Value)>>>,
        _handle: tokio::task::JoinHandle<()>,
    }

    /// Minimal Bot API stand-in: records every `(method, body)` and answers
    /// the three calls the module makes.
    fn fake_telegram() -> FakeTelegram {
        let seen: Arc<Mutex<Vec<(String, Value)>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_for_handler = seen.clone();
        // The Bot API path is `/bot<token>/<method>` with no slash after
        // `bot`, so a static-prefix route cannot match; the fallback sees
        // every path (the fake only ever receives POSTs).
        let app = Router::new().fallback(
            post(move |uri: axum::http::Uri, Json(body): Json<Value>| {
                let seen = seen_for_handler.clone();
                async move {
                    let method = uri.path().rsplit('/').next().unwrap_or_default().to_string();
                    seen.lock().unwrap().push((method.clone(), body));
                    let result = match method.as_str() {
                        "getMe" => json!({
                            "id": 900001,
                            "is_bot": true,
                            "first_name": "Allternit Manager",
                            "username": "allternit_manager_bot",
                        }),
                        "getManagedBotToken" => json!(CHILD_TOKEN),
                        "setWebhook" => json!(true),
                        _ => json!(null),
                    };
                    Json(json!({ "ok": true, "result": result }))
                }
            }),
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        FakeTelegram { base_url, seen, _handle: handle }
    }

    // ------------------------------------------------------------ env + db helpers

    fn set_managed_env(api_base: &str) {
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
        std::env::set_var(TOKEN_ENV, MANAGER_TOKEN);
        std::env::set_var(SECRET_ENV, WEBHOOK_SECRET);
        std::env::set_var(API_BASE_ENV, api_base);
        std::env::set_var(DP_SEED_ENV, STANDARD.encode([7u8; 32]));
    }

    fn clear_managed_env() {
        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
        std::env::remove_var(TOKEN_ENV);
        std::env::remove_var(SECRET_ENV);
        std::env::remove_var(API_BASE_ENV);
        std::env::remove_var(DP_SEED_ENV);
    }

    /// Same columns as migrations_pg/022, unqualified: test_support's pool is
    /// schema-per-test via search_path, so a `public.` prefix would leak.
    async fn create_onboarding_table(pool: &sqlx::PgPool) {
        sqlx::query(
            r#"
            CREATE TABLE telegram_onboarding (
                id                 bigserial PRIMARY KEY,
                user_id            text NOT NULL,
                runtime_id         text NOT NULL,
                allternit_bot_id   text NOT NULL,
                bot_name           text NOT NULL,
                suggested_username text NOT NULL,
                nonce              text NOT NULL UNIQUE,
                state              text NOT NULL DEFAULT 'waiting'
                                   CHECK (state IN ('waiting','created','connected','paired','failed','expired')),
                tg_bot_id          text,
                tg_bot_username    text,
                tg_creator_user_id text,
                error              text,
                created_at         timestamptz NOT NULL DEFAULT now(),
                updated_at         timestamptz NOT NULL DEFAULT now(),
                expires_at         timestamptz NOT NULL DEFAULT now() + interval '30 minutes'
            )
            "#,
        )
        .execute(pool)
        .await
        .unwrap();
    }

    async fn body_json(response: Response) -> Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn webhook_request(secret: &str, update: &Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(format!("{MANAGER_WEBHOOK_SUFFIX}/{secret}"))
            .header("content-type", "application/json")
            .header("x-telegram-bot-api-secret-token", secret)
            .body(Body::from(update.to_string()))
            .unwrap()
    }

    async fn onboarding_row(pool: &sqlx::PgPool, id: i64) -> (String, Option<String>, String) {
        sqlx::query_as(
            "SELECT state, tg_bot_username, nonce FROM telegram_onboarding WHERE id = $1",
        )
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    // ------------------------------------------------------------ pure helpers

    #[test]
    fn suggested_username_is_lowercase_suffixed_and_within_telegram_limits() {
        assert_eq!(suggested_username("Acme Support", "a1b2"), "acme_support_a1b2_bot");
        assert_eq!(suggested_username("My_Bot", "zz99"), "my_bot_zz99_bot");
        let long = suggested_username(
            "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-and-then-some-more",
            "a1b2",
        );
        assert_eq!(long.len(), 32, "23-char core + _ + 4 + _bot: {long}");
        assert!(long.ends_with("_a1b2_bot"));
        // Symbols-only names still produce a valid, unique-ish username.
        assert_eq!(suggested_username("🚀✨", "q7q7"), "allternit_q7q7_bot");
        assert_eq!(suggested_username("", "q7q7"), "allternit_q7q7_bot");
    }

    #[test]
    fn query_escape_keeps_unreserved_and_encodes_the_rest() {
        assert_eq!(query_escape("Acme Support"), "Acme%20Support");
        assert_eq!(query_escape("a&b=c+d/e?f"), "a%26b%3Dc%2Bd%2Fe%3Ff");
        assert_eq!(query_escape("azAZ09-._~"), "azAZ09-._~");
    }

    #[test]
    fn manager_secret_requires_path_and_header_match() {
        assert!(manager_secret_ok("s", Some("s"), "s"));
        assert!(!manager_secret_ok("s", Some("other"), "s"));
        assert!(!manager_secret_ok("other", Some("s"), "s"));
        assert!(!manager_secret_ok("s", None, "s"));
    }

    // ------------------------------------------------------------ routes

    #[tokio::test]
    #[serial_test::serial]
    async fn full_flow_create_webhook_token_relay_and_status() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tg = fake_telegram();
        set_managed_env(&tg.base_url);
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        create_onboarding_table(&state.db).await;
        seed_runtime_device(&state.db, "tg-managed-rt-1", DEV_USER).await;
        let app = routes().with_state(state.clone());
        let (connection, mut outgoing) = register_test_connection("tg-managed-rt-1").await;

        // 1. create
        let response = app
            .clone()
            .oneshot(authed_request(
                "POST",
                "/api/v1/channel-onboarding/telegram",
                r#"{"runtimeId":"tg-managed-rt-1","botId":"bot-allternit-1","botName":"Acme Support"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let created = body_json(response).await;
        assert_eq!(created["state"], "waiting");
        assert_eq!(created["expiresInMinutes"], 30);
        let onboarding_id: i64 = created["onboardingId"].as_str().unwrap().parse().unwrap();
        let create_url = created["createUrl"].as_str().unwrap();
        assert!(
            create_url.starts_with("https://t.me/newbot/allternit_manager_bot/"),
            "deep link carries the manager bot: {create_url}"
        );
        assert!(create_url.contains("name=Acme%20Support"), "name pre-filled: {create_url}");

        // 2. Telegram delivers the managed_bot update to the manager webhook.
        let (state_name, _, db_nonce) = onboarding_row(&state.db, onboarding_id).await;
        assert_eq!(state_name, "waiting");
        let suggested = sqlx::query_scalar::<_, String>(
            "SELECT suggested_username FROM telegram_onboarding WHERE id = $1",
        )
        .bind(onboarding_id)
        .fetch_one(&state.db)
        .await
        .unwrap();
        let update = json!({
            "update_id": 7001,
            "managed_bot": {
                "user": { "id": 4242, "is_bot": false, "first_name": "Eoj" },
                "bot": { "id": 555123, "is_bot": true, "first_name": "Acme Support", "username": suggested },
            },
        });
        let webhook_task = tokio::spawn({
            let app = app.clone();
            let update = update.clone();
            async move { app.oneshot(webhook_request(WEBHOOK_SECRET, &update)).await.unwrap() }
        });

        // 3. the token delivery arrives on the runtime's relay connection.
        let envelope = tokio::time::timeout(Duration::from_secs(30), outgoing.recv())
            .await
            .expect("relay request arrives")
            .expect("connection open");
        let CloudMessage::Request { request_id, method, path, headers, body, body_encoding } =
            envelope
        else {
            panic!("expected a relay request envelope");
        };
        assert_eq!(method, "POST");
        assert_eq!(path, MANAGED_CONNECT_PATH);
        assert_eq!(body_encoding, "base64");
        let relay_body: Value =
            serde_json::from_slice(&STANDARD.decode(body).unwrap()).unwrap();
        assert_eq!(relay_body["botToken"], CHILD_TOKEN);
        assert_eq!(relay_body["allternitBotId"], "bot-allternit-1");
        assert_eq!(relay_body["onboardingId"], onboarding_id.to_string());
        let pair_nonce = relay_body["pairNonce"].as_str().unwrap().to_string();
        assert_eq!(pair_nonce, db_nonce, "the relayed nonce is the row's nonce");
        // The delivery is signed as the user for this runtime only.
        let authz = headers
            .get("authorization")
            .expect("trusted authorization header on the relay");
        let jwt = authz.strip_prefix("Bearer ").expect("bearer scheme");
        let claims: Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(jwt.split('.').nth(1).expect("JWT payload segment"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(claims["sub"], DEV_USER);
        assert_eq!(claims["aud"], "tg-managed-rt-1");
        assert_eq!(claims["scope"], "runtime:execute");

        // 4. the runtime answers; the webhook completes and marks connected.
        answer_test_request(&connection, &request_id, 200, r#"{"ok":true}"#).await;
        let response = webhook_task.await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let (state_name, tg_username, _) = onboarding_row(&state.db, onboarding_id).await;
        assert_eq!(state_name, "connected");
        assert_eq!(tg_username.as_deref(), Some(suggested.as_str()));
        let seen = tg.seen.lock().unwrap();
        assert!(
            seen.iter().any(|(m, b)| m == "getManagedBotToken" && b["user_id"] == 555123),
            "token fetched by child bot user id: {seen:?}"
        );
        drop(seen);

        // 5. status reports the connected bot and a pair deep link.
        let response = app
            .clone()
            .oneshot(authed_request(
                "GET",
                &format!("/api/v1/channel-onboarding/telegram/{onboarding_id}"),
                "",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let status = body_json(response).await;
        assert_eq!(status["state"], "connected");
        assert_eq!(status["botUsername"], suggested);
        let pair_url = status["pairUrl"].as_str().expect("pairUrl while pairable");
        assert_eq!(pair_url, format!("https://t.me/{suggested}?start={pair_nonce}"));

        clear_managed_env();
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn webhook_rejects_wrong_secret() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tg = fake_telegram();
        set_managed_env(&tg.base_url);
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        create_onboarding_table(&state.db).await;
        let app = routes().with_state(state);
        let update = json!({
            "update_id": 1,
            "managed_bot": {
                "user": { "id": 1 },
                "bot": { "id": 2, "username": "whatever_x_bot" },
            },
        });
        let response = app
            .oneshot(webhook_request("wrong-secret", &update))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(body_json(response).await["error"], "invalid_secret");
        clear_managed_env();
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn unknown_username_is_acked_and_left_waiting() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tg = fake_telegram();
        set_managed_env(&tg.base_url);
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        create_onboarding_table(&state.db).await;
        seed_runtime_device(&state.db, "tg-managed-rt-2", DEV_USER).await;
        let app = routes().with_state(state.clone());
        // No relay connection registered: any token delivery attempt would
        // flip the row to failed, so `waiting` also proves nothing relayed.
        let response = app
            .clone()
            .oneshot(authed_request(
                "POST",
                "/api/v1/channel-onboarding/telegram",
                r#"{"runtimeId":"tg-managed-rt-2","botId":"bot-x","botName":"Acme"}"#,
            ))
            .await
            .unwrap();
        let created = body_json(response).await;
        let onboarding_id: i64 = created["onboardingId"].as_str().unwrap().parse().unwrap();

        let update = json!({
            "update_id": 2,
            "managed_bot": {
                "user": { "id": 99 },
                "bot": { "id": 555999, "username": "someone_else_bot" },
            },
        });
        let response = app.oneshot(webhook_request(WEBHOOK_SECRET, &update)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(onboarding_row(&state.db, onboarding_id).await.0, "waiting");
        clear_managed_env();
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn ambiguous_username_match_relays_nothing() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tg = fake_telegram();
        set_managed_env(&tg.base_url);
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        create_onboarding_table(&state.db).await;
        // Two users ran the wizard with the same name; the suffix collided
        // (direct insert bypasses the random suffix).
        for (user, runtime, nonce) in [
            ("user_a", "tg-managed-rt-3a", "nonce-a"),
            ("user_b", "tg-managed-rt-3b", "nonce-b"),
        ] {
            sqlx::query(
                "INSERT INTO telegram_onboarding
                     (user_id, runtime_id, allternit_bot_id, bot_name, suggested_username, nonce)
                 VALUES ($1, $2, 'bot', 'Dup', 'dup_name_bot', $3)",
            )
            .bind(user)
            .bind(runtime)
            .bind(nonce)
            .execute(&state.db)
            .await
            .unwrap();
        }
        let app = routes().with_state(state.clone());
        let update = json!({
            "update_id": 3,
            "managed_bot": {
                "user": { "id": 5 },
                "bot": { "id": 555777, "username": "Dup_Name_Bot" },
            },
        });
        let response = app.oneshot(webhook_request(WEBHOOK_SECRET, &update)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let states: Vec<(String,)> =
            sqlx::query_as("SELECT state FROM telegram_onboarding ORDER BY id")
                .fetch_all(&state.db)
                .await
                .unwrap();
        assert_eq!(
            states,
            vec![("waiting".to_string(),), ("waiting".to_string(),)],
            "ambiguous match must refuse both"
        );
        clear_managed_env();
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn expired_row_never_matches_and_status_reports_expired() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tg = fake_telegram();
        set_managed_env(&tg.base_url);
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        create_onboarding_table(&state.db).await;
        seed_runtime_device(&state.db, "tg-managed-rt-4", DEV_USER).await;
        let app = routes().with_state(state.clone());
        let response = app
            .clone()
            .oneshot(authed_request(
                "POST",
                "/api/v1/channel-onboarding/telegram",
                r#"{"runtimeId":"tg-managed-rt-4","botId":"bot-y","botName":"Late"}"#,
            ))
            .await
            .unwrap();
        let created = body_json(response).await;
        let onboarding_id: i64 = created["onboardingId"].as_str().unwrap().parse().unwrap();
        sqlx::query("UPDATE telegram_onboarding SET expires_at = now() - interval '1 minute' WHERE id = $1")
            .bind(onboarding_id)
            .execute(&state.db)
            .await
            .unwrap();
        let suggested = sqlx::query_scalar::<_, String>(
            "SELECT suggested_username FROM telegram_onboarding WHERE id = $1",
        )
        .bind(onboarding_id)
        .fetch_one(&state.db)
        .await
        .unwrap();

        // Webhook: no match, acked.
        let update = json!({
            "update_id": 4,
            "managed_bot": {
                "user": { "id": 6 },
                "bot": { "id": 555555, "username": suggested },
            },
        });
        let response = app.clone().oneshot(webhook_request(WEBHOOK_SECRET, &update)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Status: the lazy expire flips the row on read.
        let response = app
            .oneshot(authed_request(
                "GET",
                &format!("/api/v1/channel-onboarding/telegram/{onboarding_id}"),
                "",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json(response).await["state"], "expired");
        clear_managed_env();
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn unconfigured_surface_answers_503_not_configured() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_managed_env();
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        let app = routes().with_state(state);

        let response = app
            .clone()
            .oneshot(authed_request(
                "POST",
                "/api/v1/channel-onboarding/telegram",
                r#"{"runtimeId":"r","botId":"b","botName":"n"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            body_json(response).await,
            json!({ "error": "telegram_managed_not_configured" })
        );

        let response = app
            .oneshot(Request::builder()
                .method("GET")
                .uri("/api/v1/channel-onboarding/telegram/available")
                .body(Body::empty())
                .unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json(response).await, json!({ "available": false }));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn paired_route_accepts_exactly_once() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tg = fake_telegram();
        set_managed_env(&tg.base_url);
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        create_onboarding_table(&state.db).await;
        let right = sqlx::query(
            "INSERT INTO telegram_onboarding
                 (user_id, runtime_id, allternit_bot_id, bot_name, suggested_username, nonce, state)
             VALUES ('user_dev', 'tg-managed-rt-5', 'bot-1', 'B', 'b_x_bot', 'nonce-right', 'connected') RETURNING id",
        )
        .fetch_one(&state.db)
        .await
        .unwrap();
        let right_id: i64 = sqlx::Row::get(&right, "id");
        let other = sqlx::query(
            "INSERT INTO telegram_onboarding
                 (user_id, runtime_id, allternit_bot_id, bot_name, suggested_username, nonce, state)
             VALUES ('user_dev', 'tg-managed-rt-5', 'bot-2', 'B', 'b2_x_bot', 'nonce-other', 'connected') RETURNING id",
        )
        .fetch_one(&state.db)
        .await
        .unwrap();
        let other_id: i64 = sqlx::Row::get(&other, "id");
        let app = routes().with_state(state);
        let pair = |id: &str, nonce: &str| {
            let app = app.clone();
            let id = id.to_string();
            let nonce = nonce.to_string();
            async move {
                app.oneshot(Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/channel-onboarding/telegram/{id}/paired"))
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"pairNonce":"{nonce}"}}"#)))
                    .unwrap())
                .await
                .unwrap()
            }
        };

        // Use the ids the inserts returned: the shared CI database has advanced the sequence.
        let response = pair(&right_id.to_string(), "nonce-right").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json(response).await, json!({ "ok": true, "state": "paired" }));

        // Second call: pairing is single-use.
        let response = pair(&right_id.to_string(), "nonce-right").await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(response).await["error"], "pairing_not_pending");

        // Wrong nonce against a still-connected row: conflict, not paired.
        let response = pair(&other_id.to_string(), "nonce-right").await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(response).await["error"], "pairing_not_pending");

        // Unknown id: 404.
        let response = pair("999999", "nonce-right").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        clear_managed_env();
    }
}
