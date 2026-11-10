//! Native iOS push (APNs) for the Allternit iPhone app. Migration `111_apns_device_tokens.sql`.
//!
//! Rides on the same pipeline as Web Push: every sender in [`super::web_push`] (the backbone push sink,
//! rings, missed calls, in-app messages, channel messages, quiet-hours summaries, page repeats) ends in
//! `web_push::send_to_user`, which also calls [`send_to_user`] here. Preferences, quiet hours, collapse and
//! rate limits are therefore applied once, upstream, for both transports.
//!
//! Token-based auth over HTTP/2 (a `.p8` signing key). Env:
//! - `APNS_KEY_P8`   the `.p8` file contents (PEM; literal `\n` escapes are accepted)
//! - `APNS_KEY_ID`   the key's 10-character id
//! - `APNS_TEAM_ID`  the Apple Developer team id
//! - `APNS_TOPIC`    the app bundle id (default `com.allternit.mobile`)
//! - `APNS_ENV`      `production` (default) or `sandbox`: used for a token whose app didn't say which
//!
//! With the key, key id or team id unset, the routes answer 503 `apns_not_configured` and nothing is sent.
//!
//! Routes (Clerk session or `compute`-scoped API key):
//! - `POST   /api/v1/device-tokens` {token, platform:"ios", environment?, deviceId?, osVersion?, appBuild?,
//!   capabilities?} → `{ok, id, readToken}`. `capabilities` lists what the app supports ([`KNOWN_CAPABILITIES`]);
//!   an old app that sends none gets `[]`. `readToken` is new on every registration (only its hash is stored)
//!   and works on the two device-read routes in [`super::device_read`] only.
//! - `DELETE /api/v1/device-tokens` {token} → `{ok, removed}`; also removes that device's activity tokens.
//! - `PUT    /api/v1/device-tokens/activity` {deviceId, kind, attributesType?, activityKey?, token, environment?}
//!   → `{ok, id}`. `kind` is `la_start` (Live Activity push-to-start), `la_update` (one running activity) or
//!   `widget` (iOS 26 widget push). The device must already be registered for this person.
//! - `DELETE /api/v1/device-tokens/activity` {token} → `{ok, removed}`
//!
//! The notification carries `{type, url, tag, data}` next to `aps`; the app opens `url` (the same app path
//! the Web Push service worker opens) when the notification is tapped.

use aws_lc_rs::{
    rand::SystemRandom,
    signature::{EcdsaKeyPair, ECDSA_P256_SHA256_FIXED_SIGNING},
};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{post, put},
    Json, Router,
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use super::web_push::PushMessage;
use crate::ApiState;

/// Most iOS devices one person may register.
const MAX_TOKENS_PER_USER: i64 = 10;
/// Most activity / widget push tokens one person may hold across devices.
const MAX_ACTIVITY_TOKENS_PER_USER: i64 = 40;
/// What an app may say it supports at registration. Anything else is dropped.
pub const KNOWN_CAPABILITIES: [&str; 4] = ["actions", "nse", "la_push", "widget_push"];
/// Activity token kinds (`apns_activity_tokens.kind`).
pub const ACTIVITY_KINDS: [&str; 3] = ["la_start", "la_update", "widget"];
/// Prefix of a device read token: `adr_<device row id hex>_<secret hex>`.
pub const READ_TOKEN_PREFIX: &str = "adr_";
/// Drop a token after this many failures in a row that weren't a clean "gone" answer.
const MAX_FAILURES: i32 = 8;
/// Apple accepts a provider token for an hour and rejects refreshing it more than every 20 minutes.
const JWT_REFRESH_SECS: i64 = 40 * 60;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/device-tokens", post(register_route).delete(unregister_route))
        .route("/api/v1/device-tokens/activity", put(activity_put_route).delete(activity_delete_route))
}

// ---------------------------------------------------------------------------
// Config + provider token
// ---------------------------------------------------------------------------

pub struct Apns {
    key: EcdsaKeyPair,
    key_id: String,
    team_id: String,
    topic: String,
    default_env: &'static str,
}

impl Apns {
    /// `None` when the env is unset or the key is unusable (APNs stays off).
    pub fn from_env() -> Option<Self> {
        let pem = std::env::var("APNS_KEY_P8").ok().filter(|v| !v.trim().is_empty())?;
        let key_id = std::env::var("APNS_KEY_ID").ok().filter(|v| !v.trim().is_empty())?;
        let team_id = std::env::var("APNS_TEAM_ID").ok().filter(|v| !v.trim().is_empty())?;
        let topic = std::env::var("APNS_TOPIC").ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| "com.allternit.mobile".into());
        let env = std::env::var("APNS_ENV").unwrap_or_default();
        match Self::new(&pem, &key_id, &team_id, &topic, &env) {
            Ok(apns) => Some(apns),
            Err(error) => {
                tracing::warn!("apns disabled, key unusable: {error}");
                None
            }
        }
    }

    pub fn new(pem: &str, key_id: &str, team_id: &str, topic: &str, env: &str) -> Result<Self, String> {
        let der = pem_to_der(pem).ok_or("APNS_KEY_P8 is not a PEM private key")?;
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &der).map_err(|e| format!("{e}"))?;
        Ok(Self { key, key_id: key_id.trim().into(), team_id: team_id.trim().into(), topic: topic.trim().into(), default_env: normalize_env(env).unwrap_or("production") })
    }

    fn sign_jwt(&self, iat: i64) -> Result<String, String> {
        let header = URL_SAFE_NO_PAD.encode(json!({ "alg": "ES256", "kid": self.key_id }).to_string());
        let claims = URL_SAFE_NO_PAD.encode(json!({ "iss": self.team_id, "iat": iat }).to_string());
        let input = format!("{header}.{claims}");
        let sig = self.key.sign(&SystemRandom::new(), input.as_bytes()).map_err(|e| format!("{e}"))?;
        Ok(format!("{input}.{}", URL_SAFE_NO_PAD.encode(sig.as_ref())))
    }

    /// The cached provider token, re-signed every 40 minutes (or after Apple says it expired).
    fn provider_token(&self) -> Result<String, String> {
        let now = chrono::Utc::now().timestamp();
        let mut cache = JWT_CACHE.lock().map_err(|_| "jwt cache poisoned")?;
        if let Some((kid, iat, jwt)) = cache.as_ref() {
            if kid == &self.key_id && now - iat < JWT_REFRESH_SECS {
                return Ok(jwt.clone());
            }
        }
        let jwt = self.sign_jwt(now)?;
        *cache = Some((self.key_id.clone(), now, jwt.clone()));
        Ok(jwt)
    }
}

static JWT_CACHE: Mutex<Option<(String, i64, String)>> = Mutex::new(None);

fn forget_provider_token() {
    if let Ok(mut cache) = JWT_CACHE.lock() {
        *cache = None;
    }
}

fn pem_to_der(pem: &str) -> Option<Vec<u8>> {
    let pem = pem.replace("\\n", "\n");
    let body: String = pem.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("-----")).collect();
    STANDARD.decode(body).ok().filter(|d| !d.is_empty())
}

fn normalize_env(env: &str) -> Option<&'static str> {
    match env.trim().to_ascii_lowercase().as_str() {
        "production" | "prod" => Some("production"),
        "sandbox" | "development" | "dev" => Some("sandbox"),
        _ => None,
    }
}

fn host_for(env: &str) -> &'static str {
    if env == "sandbox" {
        "https://api.sandbox.push.apple.com"
    } else {
        "https://api.push.apple.com"
    }
}

/// APNs only speaks HTTP/2; reqwest negotiates it over ALPN (the `http2` feature is on in Cargo.toml).
fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| reqwest::Client::builder().timeout(Duration::from_secs(10)).redirect(reqwest::redirect::Policy::none()).build().unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Payload
// ---------------------------------------------------------------------------

pub fn payload(msg: &PushMessage) -> Value {
    let clip = |s: &str, n: usize| s.chars().take(n).collect::<String>();
    json!({
        "aps": {
            "alert": { "title": clip(&msg.title, 120), "body": clip(&msg.body, 400) },
            "sound": "default",
            // The server keeps no unread count; 1 marks "something new" and the app clears it when opened.
            "badge": 1,
            "thread-id": clip(&msg.tag, 64),
            "mutable-content": 1,
        },
        "type": msg.kind,
        "url": msg.url,
        "tag": msg.tag,
        "data": msg.data,
    })
}

/// `apns-collapse-id` must be at most 64 bytes.
fn collapse_id(tag: &str) -> String {
    let mut out = String::new();
    for c in tag.chars() {
        if out.len() + c.len_utf8() > 64 {
            break;
        }
        out.push(c);
    }
    out
}

// ---------------------------------------------------------------------------
// Sending
// ---------------------------------------------------------------------------

/// Which table a push target lives in. A "gone" answer or too many failures removes the row from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenTable {
    Device,
    Activity,
}

impl TokenTable {
    fn name(self) -> &'static str {
        match self {
            TokenTable::Device => "apns_device_tokens",
            TokenTable::Activity => "apns_activity_tokens",
        }
    }
}

/// One APNs token to push to: a device token (alerts, background) or an activity / widget token.
#[derive(Clone, Debug)]
pub struct PushTarget {
    pub table: TokenTable,
    pub id: String,
    pub user_id: String,
    pub device_id: String,
    pub token: String,
    pub environment: String,
    /// Device tokens: what the app said it supports ([`KNOWN_CAPABILITIES`]). Empty for activity tokens.
    pub capabilities: Vec<String>,
    /// Activity tokens: `la_start` / `la_update` / `widget`. `None` for device tokens.
    pub kind: Option<String>,
    pub attributes_type: Option<String>,
    pub activity_key: Option<String>,
}

impl PushTarget {
    pub fn has(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c == capability)
    }
}

#[derive(sqlx::FromRow)]
struct DeviceRow {
    id: String,
    user_id: String,
    device_id: String,
    token: String,
    environment: String,
    capabilities: Value,
}

#[derive(sqlx::FromRow)]
struct ActivityRow {
    id: String,
    user_id: String,
    device_id: String,
    token: String,
    environment: String,
    kind: String,
    attributes_type: Option<String>,
    activity_key: Option<String>,
}

/// Every iOS device `user` registered, with its capabilities.
pub async fn device_targets(db: &PgPool, user: &str) -> Result<Vec<PushTarget>, sqlx::Error> {
    let rows: Vec<DeviceRow> = sqlx::query_as("SELECT id, user_id, device_id, token, environment, capabilities FROM apns_device_tokens WHERE user_id = $1").bind(user).fetch_all(db).await?;
    Ok(rows
        .into_iter()
        .map(|r| PushTarget {
            table: TokenTable::Device,
            capabilities: r.capabilities.as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default(),
            id: r.id,
            user_id: r.user_id,
            device_id: r.device_id,
            token: r.token,
            environment: r.environment,
            kind: None,
            attributes_type: None,
            activity_key: None,
        })
        .collect())
}

/// `user`'s activity / widget tokens of one `kind` (see [`ACTIVITY_KINDS`]).
pub async fn activity_targets(db: &PgPool, user: &str, kind: &str) -> Result<Vec<PushTarget>, sqlx::Error> {
    let rows: Vec<ActivityRow> = sqlx::query_as("SELECT id, user_id, device_id, token, environment, kind, attributes_type, activity_key FROM apns_activity_tokens WHERE user_id = $1 AND kind = $2").bind(user).bind(kind).fetch_all(db).await?;
    Ok(rows
        .into_iter()
        .map(|r| PushTarget {
            table: TokenTable::Activity,
            id: r.id,
            user_id: r.user_id,
            device_id: r.device_id,
            token: r.token,
            environment: r.environment,
            capabilities: Vec::new(),
            kind: Some(r.kind),
            attributes_type: r.attributes_type,
            activity_key: r.activity_key,
        })
        .collect())
}

/// Optional headers for [`send_raw`].
#[derive(Clone, Debug, Default)]
pub struct RawOptions {
    /// `apns-collapse-id` (clipped to 64 bytes).
    pub collapse_id: Option<String>,
    /// Seconds APNs keeps trying; `None` means one hour.
    pub ttl_secs: Option<u32>,
}

/// One push of any type to one token. `push_type` is the `apns-push-type` header (`alert`, `background`,
/// `liveactivity`, `widgets`), `topic_suffix` is appended to the app's bundle id (`""`,
/// `".push-type.liveactivity"`, `".push-type.widgets"`), `priority` is `apns-priority` (5 or 10).
/// Returns true when APNs accepted it. A gone token is deleted from its table (a gone device token takes the
/// device's activity tokens with it); other failures count up and drop the row after [`MAX_FAILURES`].
/// Does nothing when APNs isn't configured.
pub async fn send_raw(db: &PgPool, push_type: &str, topic_suffix: &str, priority: u8, payload: &Value, target: &PushTarget, opts: &RawOptions) -> bool {
    let Some(apns) = Apns::from_env() else { return false };
    let expiration = chrono::Utc::now().timestamp() + i64::from(opts.ttl_secs.unwrap_or(3600));
    deliver(db, &apns, target, push_type, topic_suffix, priority, expiration, opts.collapse_id.as_deref(), payload.to_string()).await
}

#[allow(clippy::too_many_arguments)]
async fn deliver(db: &PgPool, apns: &Apns, target: &PushTarget, push_type: &str, topic_suffix: &str, priority: u8, expiration: i64, collapse: Option<&str>, body: String) -> bool {
    let table = target.table.name();
    let jwt = match apns.provider_token() {
        Ok(jwt) => jwt,
        Err(error) => {
            tracing::warn!("apns: signing provider token failed: {error}");
            return false;
        }
    };
    let url = format!("{}/3/device/{}", host_for(&target.environment), target.token);
    let mut req = client()
        .post(url)
        .header("authorization", format!("bearer {jwt}"))
        .header("apns-topic", format!("{}{}", apns.topic, topic_suffix))
        .header("apns-push-type", push_type)
        .header("apns-priority", priority.to_string())
        .header("apns-expiration", expiration.to_string())
        .header("content-type", "application/json");
    if let Some(collapse) = collapse.filter(|c| !c.is_empty()) {
        req = req.header("apns-collapse-id", collapse_id(collapse));
    }
    let outcome = match req.body(body).send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let reason = resp.json::<Value>().await.ok().and_then(|v| v.get("reason").and_then(Value::as_str).map(str::to_string)).unwrap_or_default();
            Ok((status, reason))
        }
        Err(error) => Err(error.to_string()),
    };
    match outcome {
        Ok((200, _)) => {
            let _ = sqlx::query(&format!("UPDATE {table} SET last_ok_at = now(), failures = 0, last_error = NULL WHERE id = $1")).bind(&target.id).execute(db).await;
            true
        }
        Ok((status, reason)) if status == 410 || matches!(reason.as_str(), "BadDeviceToken" | "Unregistered" | "DeviceTokenNotForTopic") => {
            let _ = sqlx::query(&format!("DELETE FROM {table} WHERE id = $1")).bind(&target.id).execute(db).await;
            if target.table == TokenTable::Device {
                forget_device_activity(db, &target.user_id, &target.device_id).await;
            }
            false
        }
        other => {
            let error = match other {
                Ok((status, reason)) => {
                    if status == 403 && reason == "ExpiredProviderToken" {
                        forget_provider_token();
                    }
                    format!("apns answered {status} {reason}")
                }
                Err(e) => e,
            };
            tracing::warn!("apns push to {} failed: {error}", target.id);
            let _ = sqlx::query(&format!("UPDATE {table} SET failures = failures + 1, last_error = $2 WHERE id = $1")).bind(&target.id).bind(&error).execute(db).await;
            let _ = sqlx::query(&format!("DELETE FROM {table} WHERE id = $1 AND failures >= $2")).bind(&target.id).bind(MAX_FAILURES).execute(db).await;
            false
        }
    }
}

/// Remove a device's activity / widget tokens (its device token is gone or moved to someone else).
async fn forget_device_activity(db: &PgPool, user: &str, device_id: &str) {
    if device_id.is_empty() {
        return;
    }
    if let Err(error) = sqlx::query("DELETE FROM apns_activity_tokens WHERE user_id = $1 AND device_id = $2").bind(user).bind(device_id).execute(db).await {
        tracing::warn!("apns: removing activity tokens failed: {error}");
    }
}

/// Push `msg` to every iOS device `user` registered. Returns how many APNs accepted. Does nothing, and
/// costs nothing, when APNs isn't configured.
pub async fn send_to_user(db: &PgPool, user: &str, msg: &PushMessage) -> usize {
    let Some(apns) = Apns::from_env() else { return 0 };
    let targets = match device_targets(db, user).await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!("apns: reading tokens failed: {error}");
            return 0;
        }
    };
    if targets.is_empty() {
        return 0;
    }
    let body = payload(msg).to_string();
    let expiration = chrono::Utc::now().timestamp() + i64::from(msg.ttl_secs);
    let priority = if msg.high_urgency { 10 } else { 5 };
    let mut delivered = 0;
    for target in &targets {
        if deliver(db, &apns, target, "alert", "", priority, expiration, Some(&msg.tag), body.clone()).await {
            delivered += 1;
        }
    }
    delivered
}

/// True when APNs is configured and `user` has at least one iOS device registered.
pub async fn can_reach(db: &PgPool, user: &str) -> bool {
    if Apns::from_env().is_none() {
        return false;
    }
    sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM apns_device_tokens WHERE user_id = $1)").bind(user).fetch_one(db).await.unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

fn error(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({ "error": code }))).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterBody {
    token: String,
    #[serde(default)]
    platform: Option<String>,
    #[serde(default)]
    environment: Option<String>,
    #[serde(default)]
    device_id: Option<String>,
    #[serde(default)]
    os_version: Option<String>,
    #[serde(default)]
    app_build: Option<String>,
    #[serde(default)]
    capabilities: Option<Vec<String>>,
}

/// What the app tells us about the device at registration (all optional for old apps).
#[derive(Clone, Debug, Default)]
pub struct DeviceInfo {
    pub device_id: String,
    pub os_version: Option<String>,
    pub app_build: Option<String>,
    /// Only [`KNOWN_CAPABILITIES`], each once.
    pub capabilities: Vec<String>,
}

impl DeviceInfo {
    fn from_body(body: &RegisterBody) -> Self {
        let short = |v: &Option<String>| v.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(|v| v.chars().take(40).collect::<String>());
        let mut capabilities: Vec<String> = Vec::new();
        for c in body.capabilities.iter().flatten() {
            let c = c.trim();
            if KNOWN_CAPABILITIES.contains(&c) && !capabilities.iter().any(|have| have == c) {
                capabilities.push(c.to_string());
            }
        }
        Self {
            device_id: body.device_id.as_deref().unwrap_or_default().trim().chars().take(100).collect(),
            os_version: short(&body.os_version),
            app_build: short(&body.app_build),
            capabilities,
        }
    }
}

/// A registration: the row id and the new device read token (shown once; only its hash is stored).
#[derive(Clone, Debug)]
pub struct Registered {
    pub id: String,
    pub read_token: String,
}

fn token_ok(token: &str) -> bool {
    (32..=200).contains(&token.len()) && token.chars().all(|c| c.is_ascii_hexdigit())
}

/// sha256 hex; how a read token's secret is stored.
pub(crate) fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// A fresh read token for device row `id`: `adr_<id without "apns_">_<64 hex>`, and the hash to store.
fn new_read_token(id: &str) -> Result<(String, String), String> {
    use aws_lc_rs::rand::SecureRandom;
    let mut secret = [0u8; 32];
    SystemRandom::new().fill(&mut secret).map_err(|e| format!("{e}"))?;
    let secret = hex::encode(secret);
    let row = id.strip_prefix("apns_").unwrap_or(id);
    Ok((format!("{READ_TOKEN_PREFIX}{row}_{secret}"), sha256_hex(secret.as_bytes())))
}

async fn register_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<RegisterBody>) -> Response {
    let Some(apns) = Apns::from_env() else { return error(StatusCode::SERVICE_UNAVAILABLE, "apns_not_configured") };
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await {
        Ok(u) => u.id,
        Err(e) => return e.into_response(),
    };
    if body.platform.as_deref().is_some_and(|p| p != "ios") {
        return error(StatusCode::BAD_REQUEST, "unsupported_platform");
    }
    let token = body.token.trim().to_ascii_lowercase();
    if !token_ok(&token) {
        return error(StatusCode::BAD_REQUEST, "bad_token");
    }
    let env = body.environment.as_deref().and_then(normalize_env).unwrap_or(apns.default_env);
    let device = DeviceInfo::from_body(&body);
    match register(&state.db, &user, &token, env, &device).await {
        Ok(Some(done)) => (StatusCode::CREATED, Json(json!({ "ok": true, "id": done.id, "readToken": done.read_token }))).into_response(),
        Ok(None) => error(StatusCode::TOO_MANY_REQUESTS, "too_many_devices"),
        Err(e) => {
            tracing::error!("apns register: {e}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal")
        }
    }
}

/// Register (or move) one device token and issue a new read token for it. A token belongs to one person:
/// registering it while signed in as someone else moves it to them, and the previous owner's activity
/// tokens for that device are removed. `None` when the person is at the device cap.
pub async fn register(db: &PgPool, user: &str, token: &str, env: &str, device: &DeviceInfo) -> Result<Option<Registered>, sqlx::Error> {
    let existing: i64 = sqlx::query_scalar("SELECT count(*) FROM apns_device_tokens WHERE user_id = $1 AND token <> $2").bind(user).bind(token).fetch_one(db).await?;
    if existing >= MAX_TOKENS_PER_USER {
        return Ok(None);
    }
    let mut tx = db.begin().await?;
    let previous: Option<(String, String)> = sqlx::query_as("SELECT user_id, device_id FROM apns_device_tokens WHERE token = $1").bind(token).fetch_optional(&mut *tx).await?;
    if let Some((old_user, old_device)) = previous.filter(|(old_user, _)| old_user != user) {
        if !old_device.is_empty() {
            sqlx::query("DELETE FROM apns_activity_tokens WHERE user_id = $1 AND device_id = $2").bind(&old_user).bind(&old_device).execute(&mut *tx).await?;
        }
    }
    if !device.device_id.is_empty() {
        // A reinstall hands the same device a new token; keep one row per device.
        sqlx::query("DELETE FROM apns_device_tokens WHERE user_id = $1 AND device_id = $2 AND token <> $3").bind(user).bind(&device.device_id).bind(token).execute(&mut *tx).await?;
    }
    let id = format!("apns_{}", uuid::Uuid::new_v4().simple());
    let id: String = sqlx::query_scalar(
        "INSERT INTO apns_device_tokens (id, user_id, token, environment, device_id, os_version, app_build, capabilities) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (token) DO UPDATE SET user_id = $2, environment = $4, device_id = $5, os_version = $6, app_build = $7, capabilities = $8, failures = 0, last_error = NULL
         RETURNING id",
    )
    .bind(&id)
    .bind(user)
    .bind(token)
    .bind(env)
    .bind(&device.device_id)
    .bind(&device.os_version)
    .bind(&device.app_build)
    .bind(json!(device.capabilities))
    .fetch_one(&mut *tx)
    .await?;
    let (read_token, hash) = new_read_token(&id).map_err(|e| sqlx::Error::Protocol(format!("read token: {e}")))?;
    sqlx::query("UPDATE apns_device_tokens SET read_token_hash = $2 WHERE id = $1").bind(&id).bind(&hash).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Some(Registered { id, read_token }))
}

#[derive(Deserialize)]
struct UnregisterBody {
    token: String,
}

async fn unregister_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<UnregisterBody>) -> Response {
    // Signing out must work even if APNs was switched off on the server since.
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await {
        Ok(u) => u.id,
        Err(e) => return e.into_response(),
    };
    let token = body.token.trim().to_ascii_lowercase();
    match sqlx::query_scalar::<_, String>("DELETE FROM apns_device_tokens WHERE user_id = $1 AND token = $2 RETURNING device_id").bind(&user).bind(&token).fetch_optional(&state.db).await {
        Ok(device_id) => {
            let removed = device_id.is_some();
            if let Some(device_id) = device_id {
                forget_device_activity(&state.db, &user, &device_id).await;
            }
            Json(json!({ "ok": true, "removed": removed })).into_response()
        }
        Err(e) => {
            tracing::error!("apns unregister: {e}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal")
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActivityBody {
    device_id: String,
    kind: String,
    #[serde(default)]
    attributes_type: Option<String>,
    #[serde(default)]
    activity_key: Option<String>,
    token: String,
    #[serde(default)]
    environment: Option<String>,
}

/// Live Activity and widget push tokens are longer than device tokens.
fn activity_token_ok(token: &str) -> bool {
    (32..=400).contains(&token.len()) && token.chars().all(|c| c.is_ascii_hexdigit())
}

async fn activity_put_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<ActivityBody>) -> Response {
    let Some(apns) = Apns::from_env() else { return error(StatusCode::SERVICE_UNAVAILABLE, "apns_not_configured") };
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await {
        Ok(u) => u.id,
        Err(e) => return e.into_response(),
    };
    let kind = body.kind.trim();
    if !ACTIVITY_KINDS.contains(&kind) {
        return error(StatusCode::BAD_REQUEST, "bad_kind");
    }
    let token = body.token.trim().to_ascii_lowercase();
    if !activity_token_ok(&token) {
        return error(StatusCode::BAD_REQUEST, "bad_token");
    }
    let device_id: String = body.device_id.trim().chars().take(100).collect();
    if device_id.is_empty() {
        return error(StatusCode::BAD_REQUEST, "device_id_required");
    }
    let text = |v: &Option<String>| v.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(|v| v.chars().take(100).collect::<String>());
    let attributes_type = text(&body.attributes_type);
    let activity_key = text(&body.activity_key);
    let env = body.environment.as_deref().and_then(normalize_env).unwrap_or(apns.default_env);
    match put_activity(&state.db, &user, &device_id, kind, attributes_type.as_deref(), activity_key.as_deref(), &token, env).await {
        Ok(ActivityPut::Saved(id)) => (StatusCode::CREATED, Json(json!({ "ok": true, "id": id }))).into_response(),
        Ok(ActivityPut::UnknownDevice) => error(StatusCode::NOT_FOUND, "unknown_device"),
        Ok(ActivityPut::TooMany) => error(StatusCode::TOO_MANY_REQUESTS, "too_many_tokens"),
        Err(e) => {
            tracing::error!("apns activity token: {e}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal")
        }
    }
}

enum ActivityPut {
    Saved(String),
    UnknownDevice,
    TooMany,
}

/// Save one activity token. One push-to-start token per device and attributes type, one widget token per
/// device, one update token per running activity (`activity_key`): a newer one replaces the older.
#[allow(clippy::too_many_arguments)]
async fn put_activity(db: &PgPool, user: &str, device_id: &str, kind: &str, attributes_type: Option<&str>, activity_key: Option<&str>, token: &str, env: &str) -> Result<ActivityPut, sqlx::Error> {
    let known: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM apns_device_tokens WHERE user_id = $1 AND device_id = $2)").bind(user).bind(device_id).fetch_one(db).await?;
    if !known {
        return Ok(ActivityPut::UnknownDevice);
    }
    let held: i64 = sqlx::query_scalar("SELECT count(*) FROM apns_activity_tokens WHERE user_id = $1 AND token <> $2").bind(user).bind(token).fetch_one(db).await?;
    if held >= MAX_ACTIVITY_TOKENS_PER_USER {
        return Ok(ActivityPut::TooMany);
    }
    let mut tx = db.begin().await?;
    let replaced = match kind {
        "la_start" => Some(("attributes_type", attributes_type)),
        "la_update" => activity_key.map(|k| ("activity_key", Some(k))),
        _ => None,
    };
    match replaced {
        Some((column, value)) => {
            sqlx::query(&format!("DELETE FROM apns_activity_tokens WHERE user_id = $1 AND device_id = $2 AND kind = $3 AND {column} IS NOT DISTINCT FROM $4 AND token <> $5"))
                .bind(user)
                .bind(device_id)
                .bind(kind)
                .bind(value)
                .bind(token)
                .execute(&mut *tx)
                .await?;
        }
        None if kind == "widget" => {
            sqlx::query("DELETE FROM apns_activity_tokens WHERE user_id = $1 AND device_id = $2 AND kind = 'widget' AND token <> $3").bind(user).bind(device_id).bind(token).execute(&mut *tx).await?;
        }
        None => {}
    }
    let id = format!("apat_{}", uuid::Uuid::new_v4().simple());
    let id: String = sqlx::query_scalar(
        "INSERT INTO apns_activity_tokens (id, user_id, device_id, kind, attributes_type, activity_key, token, environment) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (token) DO UPDATE SET user_id = $2, device_id = $3, kind = $4, attributes_type = $5, activity_key = $6, environment = $8, failures = 0, last_error = NULL
         RETURNING id",
    )
    .bind(&id)
    .bind(user)
    .bind(device_id)
    .bind(kind)
    .bind(attributes_type)
    .bind(activity_key)
    .bind(token)
    .bind(env)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(ActivityPut::Saved(id))
}

#[derive(Deserialize)]
struct ActivityDeleteBody {
    token: String,
}

async fn activity_delete_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<ActivityDeleteBody>) -> Response {
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await {
        Ok(u) => u.id,
        Err(e) => return e.into_response(),
    };
    let token = body.token.trim().to_ascii_lowercase();
    match sqlx::query("DELETE FROM apns_activity_tokens WHERE user_id = $1 AND token = $2").bind(&user).bind(&token).execute(&state.db).await {
        Ok(done) => Json(json!({ "ok": true, "removed": done.rows_affected() > 0 })).into_response(),
        Err(e) => {
            tracing::error!("apns activity unregister: {e}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal")
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A throwaway P-256 PKCS#8 key generated for this test only.
    pub(crate) fn fixture_pem() -> String {
        let rng = SystemRandom::new();
        let doc = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let b64 = STANDARD.encode(doc.as_ref());
        format!("-----BEGIN PRIVATE KEY-----\\n{b64}\\n-----END PRIVATE KEY-----")
    }

    #[test]
    fn parses_escaped_pem_and_signs_a_three_part_jwt() {
        let apns = Apns::new(&fixture_pem(), "KEY1234567", "TEAM123456", "com.allternit.mobile", "").unwrap();
        assert_eq!(apns.default_env, "production");
        let jwt = apns.sign_jwt(1_700_000_000).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let header: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
        assert_eq!(header["kid"], "KEY1234567");
        let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["iss"], "TEAM123456");
        assert_eq!(URL_SAFE_NO_PAD.decode(parts[2]).unwrap().len(), 64);
    }

    #[test]
    fn rejects_garbage_key() {
        assert!(Apns::new("not a key", "K", "T", "com.allternit.mobile", "sandbox").is_err());
    }

    #[test]
    fn payload_carries_alert_and_app_url() {
        let msg = PushMessage { kind: "message", title: "Raven".into(), body: "Done".into(), url: "/b/bot_1".into(), tag: "thread-1".into(), data: json!({"event": "message.received"}), ttl_secs: 60, high_urgency: false };
        let p = payload(&msg);
        assert_eq!(p["aps"]["alert"]["title"], "Raven");
        assert_eq!(p["aps"]["thread-id"], "thread-1");
        assert_eq!(p["url"], "/b/bot_1");
        assert_eq!(p["type"], "message");
    }

    #[test]
    fn collapse_id_stays_under_64_bytes_on_a_char_boundary() {
        let id = collapse_id(&"é".repeat(40));
        assert!(id.len() <= 64);
        assert_eq!(id.chars().count(), 32);
    }
}
