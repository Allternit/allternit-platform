//! MCP Events client (Allternit Events P6): Allternit subscribes to events
//! from the external MCP servers a user connected (Gmail, GitHub, Linear, …
//! any server whose `server/discover` / `initialize` advertises `events`), and
//! an event wakes the bot the user chose.
//!
//! ```text
//!  UI ─POST /api/v1/mcp/connectors/:id/events─▶ runtime (this module)
//!       1. mint whsec_ secret (32 random bytes)
//!       2. PUT  cloud /api/v1/runtime/mcp-event-subscriptions/<sub_…> (relay-signed)  → callbackUrl
//!       3. events/subscribe on the connector (mcp-client), delivery.url = callbackUrl
//!          └─ the server POSTs {"type":"verification"} to the cloud, which echoes it
//!  server ─signed event─▶ cloud /mcp/events/callback/<sub_…> (verify, dedupe, queue)
//!       ─relay (24 h retry, wakes the computer)─▶ runtime POST /api/v1/mcp/event-deliveries
//!       → bot_events `connector.event.received` + a Rails ticket for the bot
//!         (the same ticket an inbound webhook trigger makes)
//! ```
//!
//! * **Identity.** The subscription id is
//!   `mcp_protocol::events::subscription_id(owner, connector url, name, arguments)`:
//!   subscribing again with the same identity updates the row (and the bot it
//!   wakes) instead of adding one. One identity wakes one bot.
//! * **Lifecycle.** A background loop ([`run`]) refreshes active subscriptions
//!   before `refreshBefore` (new secret each time: cloud first, so both secrets
//!   verify during the switch), retries `needs_reauth` once the connector has a
//!   working credential again, unsubscribes + ends subscriptions whose connector
//!   was deleted or disabled or whose OAuth grant was revoked, and finishes
//!   cloud removals that failed.
//! * **States** (`status`): `pending` → `active`; `error` (with `error.reason`:
//!   `event_not_found` -32011, `too_many_subscriptions` -32013, `unsupported`
//!   -32014 / no `events` capability, a `-32015` callback reason such as
//!   `challenge_failed`, `invalid_arguments`, `connector_unreachable`,
//!   `cloud_unreachable`, `not_paired`, `terminated`); `needs_reauth` (-32012,
//!   a 401/403 from the connector, a failed token refresh, or a revoked grant).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use mcp_client::{McpClient, McpError, TransportError};
use mcp_protocol::events::{self as ev, codes};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::{debug, info, warn};

use crate::auth::AuthUser;
use crate::mcp_apps::{self, Connector};
use crate::relay_auth::{RelaySecret, RelayedAuth};
use crate::AppState;

/// Where the cloud relays verified events (cloud-api `channel_inbound::MCP_EVENTS_RUNTIME_PATH`).
pub const DELIVERY_PATH: &str = "/api/v1/mcp/event-deliveries";
/// Cloud registration endpoint prefix (cloud-api `mcp_event_callbacks::RUNTIME_PREFIX`).
pub const CLOUD_REGISTER_PREFIX: &str = "/api/v1/runtime/mcp-event-subscriptions";
/// Ledger type written for every event that arrives.
pub const LEDGER_TYPE: &str = "connector.event.received";
/// How long a connector's `events/list` is reused.
const LIST_CACHE: Duration = Duration::from_secs(60);
/// Refresh this long before `refreshBefore` (or at half the remaining time, whichever is sooner).
const REFRESH_MARGIN_SECS: i64 = 15 * 60;
const LOOP_EVERY: Duration = Duration::from_secs(60);
const MAX_EVENT_PAGES: usize = 10;
/// Largest `data` kept in the ledger / ticket (the cloud already capped the body at 256 KiB).
const MAX_DATA_BYTES: usize = 64 * 1024;

// ---------------------------------------------------------------- cloud seam

/// The cloud half: registers subscriptions with the public receiver.
#[async_trait]
pub trait CloudRegistry: Send + Sync {
    /// Register (or rotate) `key`; returns the callback URL to give the server.
    async fn register(&self, key: &str, secret: &str, connector_id: &str, event_name: &str) -> Result<String, String>;
    /// Stop the receiver accepting events for `key`.
    async fn remove(&self, key: &str) -> Result<(), String>;
}

/// Production registry: relay-signed HTTP to cloud-api.
pub struct HttpCloud {
    pub base: String,
    pub secret: Arc<dyn RelaySecret>,
}

impl HttpCloud {
    pub fn from_process() -> Self {
        Self { base: crate::phone_sync::cloud_base(), secret: crate::relay_auth::process_secret() }
    }

    async fn send(&self, method: reqwest::Method, path: &str, body: Vec<u8>) -> Result<(u16, Value), String> {
        let (Some(token), Some(owner), Some(runtime_id)) = (self.secret.device_token(), self.secret.paired_owner(), self.secret.runtime_id()) else {
            return Err("not_paired".into());
        };
        let mut req = reqwest::Client::new()
            .request(method.clone(), format!("{}{path}", self.base))
            .timeout(Duration::from_secs(20))
            .header("content-type", "application/json");
        for (k, v) in crate::relay_auth::signed_headers(&token, &owner, method.as_str(), path, &body) {
            req = req.header(k, v);
        }
        req = req.header(crate::relay_auth::RUNTIME_ID_HEADER, runtime_id);
        let res = req.body(body).send().await.map_err(|e| format!("cloud_unreachable: {e}"))?;
        let status = res.status().as_u16();
        let value = res.json::<Value>().await.unwrap_or(Value::Null);
        Ok((status, value))
    }
}

#[async_trait]
impl CloudRegistry for HttpCloud {
    async fn register(&self, key: &str, secret: &str, connector_id: &str, event_name: &str) -> Result<String, String> {
        let body = serde_json::to_vec(&json!({ "secret": secret, "connectorId": connector_id, "eventName": event_name })).unwrap_or_default();
        let (status, v) = self.send(reqwest::Method::PUT, &format!("{CLOUD_REGISTER_PREFIX}/{key}"), body).await?;
        match (status, v.get("callbackUrl").and_then(Value::as_str)) {
            (200..=299, Some(url)) => Ok(url.to_string()),
            _ => Err(format!("cloud_refused: {status} {}", v.get("error").and_then(Value::as_str).unwrap_or_default())),
        }
    }

    async fn remove(&self, key: &str) -> Result<(), String> {
        let (status, _) = self.send(reqwest::Method::DELETE, &format!("{CLOUD_REGISTER_PREFIX}/{key}"), Vec::new()).await?;
        if (200..300).contains(&status) || status == 404 { Ok(()) } else { Err(format!("cloud_refused: {status}")) }
    }
}

fn process_cloud() -> Arc<dyn CloudRegistry> {
    static CLOUD: OnceLock<Arc<dyn CloudRegistry>> = OnceLock::new();
    CLOUD.get_or_init(|| Arc::new(HttpCloud::from_process())).clone()
}

/// Everything a subscription operation needs besides the app state.
#[derive(Clone)]
pub struct Ctx {
    pub cloud: Arc<dyn CloudRegistry>,
    /// Let connectors resolve to private addresses (local development / tests).
    pub allow_private: bool,
}

impl Ctx {
    pub fn production() -> Self {
        Self { cloud: process_cloud(), allow_private: mcp_apps::allow_private_hosts() }
    }
}

// ---------------------------------------------------------------- routers

/// `/connectors/:id/events…`, merged into `mcp_routes::mcp_router` (so it is
/// served as `/api/v1/mcp/connectors/:id/events`, next to the connector routes).
pub fn connector_events_router() -> Router<Arc<AppState>> {
    connector_events_router_with(Ctx::production())
}

pub fn connector_events_router_with(ctx: Ctx) -> Router<Arc<AppState>> {
    Router::new()
        .route("/connectors/:id/events", get(list_h).post(subscribe_h))
        .route("/connectors/:id/events/subscriptions/:sid", delete(unsubscribe_h))
        .layer(Extension(ctx))
}

/// Relayed deliveries from the cloud receiver (public surface; RelayedAuth verifies).
pub fn delivery_router(secret: Arc<dyn RelaySecret>) -> Router<Arc<AppState>> {
    Router::new().route(DELIVERY_PATH, post(delivery_h)).layer(crate::relay_auth::secret_layer(secret))
}

// ---------------------------------------------------------------- rows

#[derive(Debug, Clone)]
pub struct SubRow {
    pub id: String,
    pub user_id: String,
    pub connector_id: String,
    pub name: String,
    pub arguments: Value,
    pub bot_id: String,
    pub execution_mode: String,
    pub secret: String,
    pub callback_url: Option<String>,
    pub status: String,
    pub error_code: Option<i64>,
    pub error_reason: Option<String>,
    pub error_message: Option<String>,
    pub had_auth: bool,
    pub refresh_before: Option<String>,
    pub last_event_at: Option<String>,
    pub last_event_id: Option<String>,
    pub event_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

const COLS: &str = "id, user_id, connector_id, name, arguments, bot_id, execution_mode, secret, callback_url, status, error_code, error_reason, error_message, had_auth, refresh_before, last_event_at, last_event_id, event_count, created_at, updated_at";

fn read_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SubRow> {
    Ok(SubRow {
        id: r.get(0)?,
        user_id: r.get(1)?,
        connector_id: r.get(2)?,
        name: r.get(3)?,
        arguments: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_else(|_| json!({})),
        bot_id: r.get(5)?,
        execution_mode: r.get(6)?,
        secret: crate::token_crypto::open(&r.get::<_, String>(7)?),
        callback_url: r.get(8)?,
        status: r.get(9)?,
        error_code: r.get(10)?,
        error_reason: r.get(11)?,
        error_message: r.get(12)?,
        had_auth: r.get::<_, i64>(13)? != 0,
        refresh_before: r.get(14)?,
        last_event_at: r.get(15)?,
        last_event_id: r.get(16)?,
        event_count: r.get(17)?,
        created_at: r.get(18)?,
        updated_at: r.get(19)?,
    })
}

impl SubRow {
    /// The shape the UI reads (documented in the PR / docs dev reference).
    pub fn view(&self) -> Value {
        let error = (self.status == "error" || self.status == "needs_reauth").then(|| {
            json!({ "code": self.error_code, "reason": self.error_reason, "message": self.error_message })
        });
        json!({
            "id": self.id,
            "connectorId": self.connector_id,
            "name": self.name,
            "arguments": self.arguments,
            "botId": self.bot_id,
            "executionMode": self.execution_mode,
            "status": self.status,
            "error": error,
            "refreshBefore": self.refresh_before,
            "lastEventAt": self.last_event_at,
            "lastEventId": self.last_event_id,
            "eventCount": self.event_count,
            "createdAt": self.created_at,
            "updatedAt": self.updated_at,
        })
    }
}

async fn blocking<T: Send + 'static>(
    state: &Arc<AppState>,
    f: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T> + Send + 'static,
) -> Result<T, String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || db.connect().and_then(|c| f(&c)))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

pub async fn get_row(state: &Arc<AppState>, id: &str) -> Result<Option<SubRow>, String> {
    let id = id.to_string();
    blocking(state, move |c| c.query_row(&format!("SELECT {COLS} FROM mcp_event_subscriptions WHERE id = ?1"), params![id], read_row).optional()).await
}

async fn rows_where(state: &Arc<AppState>, clause: &'static str, args: Vec<String>) -> Result<Vec<SubRow>, String> {
    blocking(state, move |c| {
        let mut stmt = c.prepare(&format!("SELECT {COLS} FROM mcp_event_subscriptions WHERE {clause} ORDER BY created_at, id"))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), read_row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
    .await
}

/// Set the status (and error, cleared for `active`/`pending`).
async fn set_status(state: &Arc<AppState>, id: &str, status: &str, code: Option<i64>, reason: Option<&str>, message: Option<&str>) {
    let (id, status, reason, message) = (id.to_string(), status.to_string(), reason.map(String::from), message.map(|m| m.chars().take(500).collect::<String>()));
    let _ = blocking(state, move |c| {
        c.execute(
            "UPDATE mcp_event_subscriptions SET status = ?2, error_code = ?3, error_reason = ?4, error_message = ?5, updated_at = ?6 WHERE id = ?1",
            params![id, status, code, reason, message, now()],
        )
    })
    .await;
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// A fresh `whsec_` secret: 32 random bytes.
pub fn new_secret() -> String {
    use base64::Engine;
    use rand::RngCore;
    let mut key = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut key);
    format!("{}{}", mcp_protocol::webhooks::SECRET_PREFIX, base64::engine::general_purpose::STANDARD.encode(key))
}

// ---------------------------------------------------------------- errors

/// A failure with the state it leaves the subscription in.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub status: &'static str,
    pub code: Option<i64>,
    pub reason: String,
    pub message: String,
}

impl Failure {
    fn error(reason: &str, message: impl Into<String>) -> Self {
        Self { status: "error", code: None, reason: reason.into(), message: message.into() }
    }
    fn reauth(code: Option<i64>, message: impl Into<String>) -> Self {
        Self { status: "needs_reauth", code, reason: "needs_reauth".into(), message: message.into() }
    }
}

/// Map an `events/*` error from the connector onto a subscription state.
pub fn classify_error(err: &McpError) -> Failure {
    match err {
        McpError::JsonRpc { code, message, data } => {
            let code = i64::from(*code);
            match code {
                codes::NOT_FOUND => Failure { status: "error", code: Some(code), reason: "event_not_found".into(), message: message.clone() },
                codes::FORBIDDEN => Failure::reauth(Some(code), message.clone()),
                codes::RESOURCE_EXHAUSTED => Failure { status: "error", code: Some(code), reason: "too_many_subscriptions".into(), message: message.clone() },
                codes::UNSUPPORTED => Failure { status: "error", code: Some(code), reason: "unsupported".into(), message: message.clone() },
                codes::CALLBACK_ENDPOINT_ERROR => Failure {
                    status: "error",
                    code: Some(code),
                    reason: data.as_ref().and_then(|d| d.get("reason")).and_then(Value::as_str).unwrap_or("callback_endpoint_error").to_string(),
                    message: message.clone(),
                },
                -32602 => Failure { status: "error", code: Some(code), reason: "invalid_arguments".into(), message: message.clone() },
                -32601 => Failure { status: "error", code: Some(code), reason: "unsupported".into(), message: message.clone() },
                _ => Failure { status: "error", code: Some(code), reason: "connector_error".into(), message: message.clone() },
            }
        }
        McpError::Transport(TransportError::Http { status: 401 | 403, .. }) | McpError::OAuth(_) => {
            Failure::reauth(None, "the connector rejected its credentials; reconnect it")
        }
        McpError::Timeout(_) => Failure::error("connector_timeout", "the connector did not respond in time"),
        other => Failure::error("connector_unreachable", other.to_string()),
    }
}

fn apps_failure(e: mcp_apps::AppsError) -> Failure {
    if e.code == "connector_unauthorized" {
        Failure::reauth(None, e.message)
    } else {
        Failure::error(e.code, e.message)
    }
}

// ---------------------------------------------------------------- connector session

async fn session(state: &Arc<AppState>, ctx: &Ctx, user: &str, connector_id: &str) -> Result<(Connector, McpClient), Failure> {
    let connector = mcp_apps::load_connector(state, user, connector_id, ctx.allow_private)
        .await
        .map_err(apps_failure)?
        .ok_or_else(|| Failure::error("connector_not_found", "connector not found"))?;
    if connector.auth_failed() {
        return Err(Failure::reauth(None, "the connector's sign-in expired; reconnect it"));
    }
    let client = mcp_apps::open_session(&connector, ctx.allow_private).await.map_err(apps_failure)?;
    Ok((connector, client))
}

type ListCache = Mutex<HashMap<(String, String), (Instant, Result<(bool, Vec<Value>), Failure>)>>;

fn list_cache() -> &'static ListCache {
    static CACHE: OnceLock<ListCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `(supported, events)` from the connector, live, reused for [`LIST_CACHE`].
pub async fn connector_events(state: &Arc<AppState>, ctx: &Ctx, user: &str, connector_id: &str) -> Result<(bool, Vec<Value>), Failure> {
    let key = (user.to_string(), connector_id.to_string());
    if let Some((at, hit)) = list_cache().lock().ok().and_then(|c| c.get(&key).cloned()) {
        if at.elapsed() < LIST_CACHE {
            return hit;
        }
    }
    let result = async {
        let (_connector, client) = session(state, ctx, user, connector_id).await?;
        let out = if client.supports_events() {
            client.list_events(MAX_EVENT_PAGES).await.map(|e| (true, e)).map_err(|e| classify_error(&e))
        } else {
            Ok((false, Vec::new()))
        };
        mcp_apps::close_session(client).await;
        out
    }
    .await;
    // Don't cache a missing connector (it may be created a moment later).
    if !matches!(&result, Err(f) if f.reason == "connector_not_found") {
        if let Ok(mut c) = list_cache().lock() {
            c.insert(key, (Instant::now(), result.clone()));
        }
    }
    result
}

fn forget_list(user: &str, connector_id: &str) {
    if let Ok(mut c) = list_cache().lock() {
        c.remove(&(user.to_string(), connector_id.to_string()));
    }
}

// ---------------------------------------------------------------- subscribe / refresh / unsubscribe

/// What a subscribe request asks for.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeRequest {
    pub name: String,
    #[serde(default = "empty_object")]
    pub arguments: Value,
    pub bot_id: String,
    /// A webhook-trigger execution mode: `REQUIRE_APPROVAL` (default), `PLAN_ONLY`,
    /// `ACCEPT_EDITS` or `BYPASS_PERMISSIONS`.
    #[serde(default)]
    pub execution_mode: Option<String>,
}

fn empty_object() -> Value {
    json!({})
}

/// Errors a subscribe request is refused with before anything is stored.
#[derive(Debug, PartialEq)]
pub enum Refused {
    BadRequest(String),
    NotFound(&'static str),
}

/// Create or update the subscription for `(user, connector, name, arguments)` and
/// call `events/subscribe`. The row is returned whatever the outcome: its
/// `status`/`error` say whether the server took it.
pub async fn subscribe(state: &Arc<AppState>, ctx: &Ctx, user: &str, connector_id: &str, req: SubscribeRequest) -> Result<SubRow, Refused> {
    let name = req.name.trim().to_string();
    if name.is_empty() || name.len() > 200 {
        return Err(Refused::BadRequest("name is required".into()));
    }
    if !req.arguments.is_object() {
        return Err(Refused::BadRequest("arguments must be an object".into()));
    }
    let mode = req.execution_mode.unwrap_or_else(|| "REQUIRE_APPROVAL".into());
    // The webhook-trigger execution modes (webhook_trigger_routes::validate_trigger_body).
    if !matches!(mode.as_str(), "PLAN_ONLY" | "REQUIRE_APPROVAL" | "ACCEPT_EDITS" | "BYPASS_PERMISSIONS") {
        return Err(Refused::BadRequest("executionMode must be PLAN_ONLY, REQUIRE_APPROVAL, ACCEPT_EDITS or BYPASS_PERMISSIONS".into()));
    }
    if !crate::bot_event_routes::verify_bot_ownership(state, user, &req.bot_id).await {
        return Err(Refused::NotFound("bot_not_found"));
    }
    let connector = mcp_apps::load_connector(state, user, connector_id, ctx.allow_private)
        .await
        .ok()
        .flatten()
        .ok_or(Refused::NotFound("connector_not_found"))?;
    let id = ev::subscription_id(user, &connector.url, &name, &req.arguments);
    let existing = get_row(state, &id).await.ok().flatten();
    // Re-subscribing keeps a live secret (idempotent); anything else starts fresh.
    let secret = existing.as_ref().filter(|r| r.status == "active").map(|r| r.secret.clone()).unwrap_or_else(new_secret);
    {
        let (id, user, cid, name, args, bot, mode, sealed, had_auth) = (
            id.clone(),
            user.to_string(),
            connector_id.to_string(),
            name.clone(),
            ev::canonical_json(&req.arguments),
            req.bot_id.clone(),
            mode.clone(),
            crate::token_crypto::seal(&secret),
            connector.has_token(),
        );
        blocking(state, move |c| {
            c.execute(
                "INSERT INTO mcp_event_subscriptions (id, user_id, connector_id, name, arguments, bot_id, execution_mode, secret, status, had_auth, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', ?9, ?10, ?10)
                 ON CONFLICT(id) DO UPDATE SET bot_id = excluded.bot_id, execution_mode = excluded.execution_mode,
                    secret = excluded.secret, had_auth = excluded.had_auth, updated_at = excluded.updated_at,
                    status = CASE WHEN mcp_event_subscriptions.status = 'active' THEN 'active' ELSE 'pending' END",
                params![id, user, cid, name, args, bot, mode, sealed, had_auth as i64, now()],
            )
        })
        .await
        .map_err(Refused::BadRequest)?;
    }
    activate(state, ctx, &id, &secret).await;
    forget_list(user, connector_id);
    get_row(state, &id).await.ok().flatten().ok_or(Refused::NotFound("subscription_not_found"))
}

/// Register `secret` with the cloud, then `events/subscribe` with it. Leaves
/// the row `active` (with `refreshBefore`) or in the failure's state.
async fn activate(state: &Arc<AppState>, ctx: &Ctx, id: &str, secret: &str) -> bool {
    let Ok(Some(row)) = get_row(state, id).await else { return false };
    match try_activate(state, ctx, &row, secret).await {
        Ok(()) => true,
        Err(f) => {
            warn!(subscription = %id, reason = %f.reason, "mcp events: subscribe failed");
            if f.status == "needs_reauth" {
                // The server won't deliver; stop the receiver too.
                let _ = ctx.cloud.remove(id).await;
            }
            set_status(state, id, f.status, f.code, Some(&f.reason), Some(&f.message)).await;
            false
        }
    }
}

async fn try_activate(state: &Arc<AppState>, ctx: &Ctx, row: &SubRow, secret: &str) -> Result<(), Failure> {
    let (connector, client) = session(state, ctx, &row.user_id, &row.connector_id).await?;
    let result = async {
        if !client.supports_events() {
            return Err(Failure::error("unsupported", "this app does not offer events"));
        }
        let callback = ctx.cloud.register(&row.id, secret, &row.connector_id, &row.name).await.map_err(|e| {
            let reason = if e.starts_with("not_paired") { "not_paired" } else { "cloud_unreachable" };
            Failure::error(reason, e)
        })?;
        let result = client.subscribe_event(&row.name, &row.arguments, &callback, secret, None).await.map_err(|e| classify_error(&e))?;
        Ok((callback, result))
    }
    .await;
    mcp_apps::close_session(client).await;
    let (callback, result) = result?;
    let refresh_before = result.get("refreshBefore").and_then(Value::as_str).map(String::from);
    let cursor = result.get("cursor").and_then(Value::as_str).map(String::from);
    let (id, sealed, had_auth) = (row.id.clone(), crate::token_crypto::seal(secret), connector.has_token());
    blocking(state, move |c| {
        c.execute(
            "UPDATE mcp_event_subscriptions SET status = 'active', error_code = NULL, error_reason = NULL, error_message = NULL,
                secret = ?2, callback_url = ?3, refresh_before = ?4, cursor = COALESCE(?5, cursor), had_auth = ?6, updated_at = ?7
              WHERE id = ?1",
            params![id, sealed, callback, refresh_before, cursor, had_auth as i64, now()],
        )
    })
    .await
    .map_err(|e| Failure::error("internal", e))?;
    info!(subscription = %row.id, event = %row.name, "mcp events: subscribed");
    Ok(())
}

/// Renew `row` with a rotated secret (cloud first, so the old secret keeps verifying meanwhile).
pub async fn refresh(state: &Arc<AppState>, ctx: &Ctx, row: &SubRow) -> bool {
    activate(state, ctx, &row.id, &new_secret()).await
}

/// `events/unsubscribe` on the connector (best effort: it may be gone or
/// unauthorized), then remove at the cloud and delete the row. A failed cloud
/// removal leaves the row `ended` for [`run`] to finish.
pub async fn unsubscribe(state: &Arc<AppState>, ctx: &Ctx, row: &SubRow) {
    if let Some(url) = row.callback_url.as_deref() {
        match session(state, ctx, &row.user_id, &row.connector_id).await {
            Ok((_c, client)) => {
                if let Err(e) = client.unsubscribe_event(&row.name, &row.arguments, url).await {
                    debug!(subscription = %row.id, "mcp events: unsubscribe refused: {e}");
                }
                mcp_apps::close_session(client).await;
            }
            Err(f) => debug!(subscription = %row.id, reason = %f.reason, "mcp events: unsubscribe skipped"),
        }
    }
    end_at_cloud(state, ctx, row).await;
}

async fn end_at_cloud(state: &Arc<AppState>, ctx: &Ctx, row: &SubRow) {
    let id = row.id.clone();
    match ctx.cloud.remove(&row.id).await {
        Ok(()) => {
            let _ = blocking(state, move |c| c.execute("DELETE FROM mcp_event_subscriptions WHERE id = ?1", params![id])).await;
        }
        Err(e) => {
            warn!(subscription = %row.id, "mcp events: cloud removal pending: {e}");
            set_status(state, &row.id, "ended", None, Some("cloud_unreachable"), Some(&e)).await;
        }
    }
    forget_list(&row.user_id, &row.connector_id);
}

/// When an active subscription is due for renewal.
pub fn due_for_refresh(refresh_before: Option<&str>, now: chrono::DateTime<chrono::Utc>, updated_at: Option<&str>) -> bool {
    let Some(rb) = refresh_before.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()) else { return false };
    let rb = rb.with_timezone(&chrono::Utc);
    let since = updated_at.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()).map(|t| t.with_timezone(&chrono::Utc));
    // Half the granted window, capped at the margin: a 2-minute TTL refreshes after 1 minute.
    let margin = since.map(|s| ((rb - s).num_seconds() / 2).clamp(0, REFRESH_MARGIN_SECS)).unwrap_or(REFRESH_MARGIN_SECS);
    now + chrono::Duration::seconds(margin) >= rb
}

/// Failure reasons [`tick`] retries by itself.
pub fn is_transient(reason: &str) -> bool {
    matches!(reason, "connector_unreachable" | "connector_timeout" | "cloud_unreachable" | "not_paired" | "timeout" | "connection_refused" | "http_5xx")
}

/// One pass of the lifecycle loop. Returns how many rows it acted on.
pub async fn tick(state: &Arc<AppState>, ctx: &Ctx) -> usize {
    let Ok(rows) = rows_where(state, "status IN ('active', 'needs_reauth', 'ended', 'error')", vec![]).await else { return 0 };
    let mut acted = 0;
    for row in rows {
        if row.status == "ended" {
            end_at_cloud(state, ctx, &row).await;
            acted += 1;
            continue;
        }
        let connector = mcp_apps::load_connector(state, &row.user_id, &row.connector_id, ctx.allow_private).await.ok().flatten();
        let Some(connector) = connector else {
            // Connector deleted or disabled: nothing to unsubscribe with; stop the receiver and forget it.
            info!(subscription = %row.id, "mcp events: connector gone, ending subscription");
            end_at_cloud(state, ctx, &row).await;
            acted += 1;
            continue;
        };
        let revoked = row.had_auth && !connector.has_token();
        if row.status == "active" && (revoked || connector.auth_failed()) {
            // The OAuth grant was revoked (or can't be renewed): unsubscribe while we still can, stop the receiver.
            info!(subscription = %row.id, "mcp events: connector credentials lost, unsubscribing");
            if let Some(url) = row.callback_url.as_deref() {
                if let Ok(client) = mcp_apps::open_session(&connector, ctx.allow_private).await {
                    let _ = client.unsubscribe_event(&row.name, &row.arguments, url).await;
                    mcp_apps::close_session(client).await;
                }
            }
            let _ = ctx.cloud.remove(&row.id).await;
            set_status(state, &row.id, "needs_reauth", None, Some("needs_reauth"), Some("reconnect the app to resume its events")).await;
            acted += 1;
            continue;
        }
        if row.status == "error" {
            // Only failures that can heal on their own are retried; the rest wait for the user.
            if row.error_reason.as_deref().is_some_and(is_transient) {
                activate(state, ctx, &row.id, &new_secret()).await;
                acted += 1;
            }
            continue;
        }
        if row.status == "needs_reauth" {
            if connector.has_token() && !connector.auth_failed() {
                // Signed in again: resume with a fresh secret.
                activate(state, ctx, &row.id, &new_secret()).await;
                acted += 1;
            }
            continue;
        }
        if due_for_refresh(row.refresh_before.as_deref(), chrono::Utc::now(), Some(&row.updated_at)) {
            refresh(state, ctx, &row).await;
            acted += 1;
        }
    }
    acted
}

/// Background lifecycle loop (refresh, re-auth resume, connector removal / revoke).
pub async fn run(state: Arc<AppState>) {
    let ctx = Ctx::production();
    loop {
        tokio::time::sleep(LOOP_EVERY).await;
        let n = tick(&state, &ctx).await;
        if n > 0 {
            debug!(acted = n, "mcp events lifecycle pass");
        }
    }
}

// ---------------------------------------------------------------- ingest

/// What one relayed delivery did.
#[derive(Debug, PartialEq)]
pub enum Ingested {
    /// Ledger entry written and the bot's ticket created.
    Triggered { ticket_id: String, ledger_id: String },
    Duplicate,
    /// A `terminated` envelope: the subscription's state was updated.
    Terminated,
    /// No such subscription for this owner (cloud should stop retrying).
    Unknown,
    Invalid(&'static str),
}

/// Handle one delivery the cloud verified and relayed for `owner`.
pub async fn ingest(state: &Arc<AppState>, owner: &str, body: &Value) -> Result<Ingested, String> {
    let Some(key) = body.get("subscriptionKey").and_then(Value::as_str) else { return Ok(Ingested::Invalid("subscriptionKey required")) };
    let Some(event) = body.get("event").filter(|e| e.is_object()) else { return Ok(Ingested::Invalid("event required")) };
    let Some(row) = get_row(state, key).await?.filter(|r| r.user_id == owner) else { return Ok(Ingested::Unknown) };

    if event.get("type").and_then(Value::as_str) == Some("terminated") {
        let code = event.pointer("/error/code").and_then(Value::as_i64);
        let reason = event.pointer("/error/data/reason").and_then(Value::as_str).unwrap_or("terminated");
        let message = event.pointer("/error/message").and_then(Value::as_str).unwrap_or("the app ended this subscription");
        let status = if code == Some(codes::FORBIDDEN) { "needs_reauth" } else { "error" };
        set_status(state, &row.id, status, code, Some(reason), Some(message)).await;
        forget_list(&row.user_id, &row.connector_id);
        return Ok(Ingested::Terminated);
    }

    let name = event.get("name").and_then(Value::as_str).unwrap_or(&row.name).to_string();
    let event_id = event
        .get("eventId")
        .and_then(Value::as_str)
        .or_else(|| body.get("webhookId").and_then(Value::as_str))
        .unwrap_or_default()
        .chars()
        .take(200)
        .collect::<String>();
    if event_id.is_empty() {
        return Ok(Ingested::Invalid("eventId required"));
    }
    let idempotency = format!("mcp-event:{}:{event_id}", row.id);
    {
        let (bot, key) = (row.bot_id.clone(), idempotency.clone());
        let seen = blocking(state, move |c| {
            c.query_row("SELECT 1 FROM bot_events WHERE bot_id = ?1 AND idempotency_key = ?2", params![bot, key], |_| Ok(())).optional()
        })
        .await?;
        if seen.is_some() {
            return Ok(Ingested::Duplicate);
        }
    }
    let connector_name = connector_label(state, &row).await;
    let mut data = event.get("data").cloned().unwrap_or(Value::Null);
    if data.to_string().len() > MAX_DATA_BYTES {
        data = json!({ "truncated": true, "bytes": data.to_string().len() });
    }
    let payload = json!({
        "connectorId": row.connector_id,
        "connectorName": connector_name,
        "subscriptionId": row.id,
        "name": name,
        "eventId": event_id,
        "timestamp": event.get("timestamp"),
        "cursor": event.get("cursor"),
        "data": data,
    });

    // The same ticket an inbound webhook trigger makes (webhook_trigger_routes), for the subscription's bot.
    let trigger = crate::webhook_trigger_routes::WebhookTrigger {
        id: row.id.clone(),
        user_id: row.user_id.clone(),
        org_id: None,
        name: format!("{connector_name}: {name}"),
        source: format!("mcp:{connector_name}"),
        event_type: name.clone(),
        target_agent_id: row.bot_id.clone(),
        prompt_template: None,
        execution_mode: row.execution_mode.clone(),
        active: true,
        created_at: row.created_at.clone(),
        updated_at: row.updated_at.clone(),
    };
    let ticket_id = crate::webhook_trigger_routes::create_ticket_for_trigger(&state.rails, &trigger, &name, &payload)
        .await
        .map_err(|e| format!("ticket: {e}"))?;

    let mut ledger_payload = payload.clone();
    ledger_payload["ticketId"] = json!(ticket_id);
    let append = crate::bot_event_routes::AppendEventBody {
        event_type: LEDGER_TYPE.to_string(),
        actor: crate::bot_event_routes::ActorBody { r#type: "connector".into(), id: row.connector_id.clone() },
        payload: ledger_payload,
        occurred_at: None,
        session_id: None,
        goal_id: None,
        wih_id: None,
        task_id: None,
        run_id: None,
        idempotency_key: Some(idempotency),
    };
    let (db, bot) = (state.db.clone(), row.bot_id.clone());
    let at = event.get("timestamp").and_then(Value::as_str).filter(|t| chrono::DateTime::parse_from_rfc3339(t).is_ok()).map(String::from).unwrap_or_else(now);
    let (stored, _fresh) = tokio::task::spawn_blocking(move || crate::bot_event_routes::append_event(&db, &bot, &append, &at))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let (id, eid) = (row.id.clone(), event_id.clone());
    let _ = blocking(state, move |c| {
        c.execute(
            "UPDATE mcp_event_subscriptions SET last_event_at = ?2, last_event_id = ?3, event_count = event_count + 1 WHERE id = ?1",
            params![id, now(), eid],
        )
    })
    .await;
    info!(subscription = %row.id, event = %name, ticket = %ticket_id, "mcp events: bot woken");
    Ok(Ingested::Triggered { ticket_id, ledger_id: stored.id })
}

async fn connector_label(state: &Arc<AppState>, row: &SubRow) -> String {
    let (cid, uid) = (row.connector_id.clone(), row.user_id.clone());
    blocking(state, move |c| {
        c.query_row("SELECT name FROM mcp_connectors WHERE id = ?1 AND user_id = ?2", params![cid, uid], |r| r.get::<_, String>(0)).optional()
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_else(|| row.connector_id.clone())
}

// ---------------------------------------------------------------- handlers

fn err(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "error": message, "code": code }))).into_response()
}

async fn list_h(State(state): State<Arc<AppState>>, Extension(ctx): Extension<Ctx>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Response {
    let exists = mcp_apps::load_connector(&state, &user.user_id, &id, ctx.allow_private).await.ok().flatten().is_some();
    let subs = rows_where(&state, "user_id = ?1 AND connector_id = ?2 AND status <> 'ended'", vec![user.user_id.clone(), id.clone()]).await.unwrap_or_default();
    if !exists && subs.is_empty() {
        return err(StatusCode::NOT_FOUND, "connector_not_found", "connector not found");
    }
    let (supported, events, error) = match connector_events(&state, &ctx, &user.user_id, &id).await {
        Ok((s, e)) => (s, e, Value::Null),
        Err(f) => (false, vec![], json!({ "reason": f.reason, "message": f.message, "needsReauth": f.status == "needs_reauth" })),
    };
    Json(json!({
        "connectorId": id,
        "supported": supported,
        "events": events,
        "subscriptions": subs.iter().map(SubRow::view).collect::<Vec<_>>(),
        "error": error,
    }))
    .into_response()
}

async fn subscribe_h(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<Ctx>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(req): Json<SubscribeRequest>,
) -> Response {
    match subscribe(&state, &ctx, &user.user_id, &id, req).await {
        Ok(row) => {
            let status = if row.status == "active" { StatusCode::OK } else { StatusCode::BAD_GATEWAY };
            (status, Json(json!({ "subscription": row.view() }))).into_response()
        }
        Err(Refused::BadRequest(m)) => err(StatusCode::BAD_REQUEST, "bad_request", &m),
        Err(Refused::NotFound(code)) => err(StatusCode::NOT_FOUND, code, "not found"),
    }
}

async fn unsubscribe_h(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<Ctx>,
    Extension(user): Extension<AuthUser>,
    Path((id, sid)): Path<(String, String)>,
) -> Response {
    let row = get_row(&state, &sid).await.ok().flatten().filter(|r| r.user_id == user.user_id && r.connector_id == id);
    let Some(row) = row else { return err(StatusCode::NOT_FOUND, "subscription_not_found", "subscription not found") };
    unsubscribe(&state, &ctx, &row).await;
    StatusCode::NO_CONTENT.into_response()
}

async fn delivery_h(State(state): State<Arc<AppState>>, auth: RelayedAuth) -> Response {
    let body: Value = match auth.json() {
        Ok(v) => v,
        Err(r) => return r,
    };
    match ingest(&state, &auth.owner, &body).await {
        Ok(Ingested::Triggered { ticket_id, ledger_id }) => Json(json!({ "status": "triggered", "ticketId": ticket_id, "eventId": ledger_id })).into_response(),
        Ok(Ingested::Duplicate) => Json(json!({ "status": "duplicate" })).into_response(),
        Ok(Ingested::Terminated) => Json(json!({ "status": "terminated" })).into_response(),
        // 410: the cloud marks the queued event dead instead of retrying for 24 h.
        Ok(Ingested::Unknown) => err(StatusCode::GONE, "subscription_not_found", "no such subscription"),
        Ok(Ingested::Invalid(why)) => err(StatusCode::BAD_REQUEST, "invalid", why),
        // 5xx: the cloud retries (the ledger idempotency key makes that safe).
        Err(e) => {
            warn!("mcp events: delivery failed: {e}");
            err(StatusCode::SERVICE_UNAVAILABLE, "unavailable", "try again")
        }
    }
}

#[cfg(test)]
#[path = "mcp_events_client_tests.rs"]
mod tests;
