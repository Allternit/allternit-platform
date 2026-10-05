//! Discord shared app, the cloud side. One Allternit Discord application is
//! added to a user's server; every Allternit bot speaks in it under its own
//! name and avatar through an app-owned webhook.
//!
//! Secrets (`ALLTERNIT_DISCORD_APP_ID`, `_PUBLIC_KEY`, `_BOT_TOKEN`,
//! `_CLIENT_SECRET`) live only in cloud-api env. With any of them unset every
//! route here answers 503 `{"error":"discord_not_configured"}` and the gateway
//! never starts.
//!
//! * `POST /api/v1/channels/discord/install` {runtimeId} → {url}: the OAuth
//!   link, carrying a signed `state`.
//! * `GET /channels/discord/oauth/callback`: stores guild → user/runtime
//!   (`discord_installs`), makes the relay route, shows a "Connected" page.
//! * `POST /channels/discord/interactions`: Ed25519-verified; PING answered at
//!   once, slash commands deferred and queued to the runtime.
//! * Gateway: one WebSocket (GUILDS, GUILD_MESSAGES, DIRECT_MESSAGES, no
//!   MESSAGE_CONTENT). A MESSAGE_CREATE that mentions the app, replies to one of
//!   its messages, or is a DM to an installer is queued to the owner runtime.
//! * `POST /api/v1/channels/discord/dm` {guildId?, userId, text?, botName?} →
//!   {channelId, messageId?}: the bot opens a DM with a Discord user who shares
//!   one of the caller's installed servers and (with `text`) posts in it.
//! * `PUT /api/v1/channels/discord/commands` {guildId, names[]}: guild slash
//!   commands, one `/<name>` per bot switched on.
//! * `POST /api/v1/channels/discord/send` {guildId, channelId, threadId?,
//!   botName, avatarUrl?, text} → {messageId}: posts through the channel's
//!   webhook with `username`/`avatar_url` per bot.
//!
//! Docs (checked 2026-10-02): https://discord.com/developers/docs/topics/oauth2
//! (bot scope, `permissions`, `integration_type`), /topics/gateway (identify,
//! resume, intents), /interactions/receiving-and-responding (PING, signatures,
//! deferred responses), /resources/webhook (execute: `username`, `avatar_url`,
//! `thread_id`, `wait`), /interactions/application-commands (bulk overwrite).

use async_trait::async_trait;
use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::channel_inbound::{deliver_route, new_key, public_base, sha256_hex};
use crate::{ApiError, ApiState};

const API: &str = "https://discord.com/api/v10";
/// `channel_inbound_routes.provider` for installs; `target_path` maps it to the
/// runtime's shared-app receiver.
pub const PROVIDER: &str = "discord_app";
/// Marks queued bodies built here, so the runtime knows it is not a raw Discord payload.
pub const ENVELOPE_SOURCE: &str = "allternit-discord-app";
const STATE_TTL_SECS: i64 = 15 * 60;
const DISCORD_MESSAGE_LIMIT: usize = 2000;
const MAX_COMMANDS: usize = 100;
/// Takes the gateway lock so only one cloud-api replica holds the socket.
const GATEWAY_LOCK_KEY: i64 = 0x414c_5444_4953_4344; // "ALTDISCD"

// Permission bits: https://discord.com/developers/docs/topics/permissions
const ADD_REACTIONS: u64 = 1 << 6;
const VIEW_CHANNEL: u64 = 1 << 10;
const SEND_MESSAGES: u64 = 1 << 11;
const EMBED_LINKS: u64 = 1 << 14;
const ATTACH_FILES: u64 = 1 << 15;
const READ_MESSAGE_HISTORY: u64 = 1 << 16;
const MANAGE_WEBHOOKS: u64 = 1 << 29;
const CREATE_PUBLIC_THREADS: u64 = 1 << 35;
const SEND_MESSAGES_IN_THREADS: u64 = 1 << 38;
/// View Channels, Send Messages, Send in Threads, Create Public Threads, Read
/// History, Embed Links, Attach Files, Add Reactions, Manage Webhooks.
pub const PERMISSIONS: u64 = ADD_REACTIONS
    | VIEW_CHANNEL
    | SEND_MESSAGES
    | EMBED_LINKS
    | ATTACH_FILES
    | READ_MESSAGE_HISTORY
    | MANAGE_WEBHOOKS
    | CREATE_PUBLIC_THREADS
    | SEND_MESSAGES_IN_THREADS;

// Gateway intents: GUILDS, GUILD_MESSAGES, DIRECT_MESSAGES. Not MESSAGE_CONTENT (1 << 15).
pub const INTENTS: u64 = (1 << 0) | (1 << 9) | (1 << 12);

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/channels/discord/install", post(install))
        .route("/api/v1/channels/discord/installs", get(list_installs))
        .route("/api/v1/channels/discord/send", post(send))
        .route("/api/v1/channels/discord/dm", post(dm))
        .route("/api/v1/channels/discord/commands", put(commands))
        .route("/channels/discord/oauth/callback", get(oauth_callback))
        .route("/channels/discord/interactions", post(interactions))
}

// ---------------------------------------------------------------- config

#[derive(Clone)]
pub struct DiscordConfig {
    pub app_id: String,
    pub public_key: String,
    pub bot_token: String,
    pub client_secret: String,
}

impl DiscordConfig {
    /// `None` unless all four env vars are set.
    pub fn from_env() -> Option<Self> {
        let get = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        Some(Self {
            app_id: get("ALLTERNIT_DISCORD_APP_ID")?,
            public_key: get("ALLTERNIT_DISCORD_PUBLIC_KEY")?,
            bot_token: get("ALLTERNIT_DISCORD_BOT_TOKEN")?,
            client_secret: get("ALLTERNIT_DISCORD_CLIENT_SECRET")?,
        })
    }
}

fn not_configured() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "discord_not_configured" }))).into_response()
}

fn json_error(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({ "error": code }))).into_response()
}

fn redirect_uri() -> String {
    format!("{}/channels/discord/oauth/callback", public_base())
}

// ---------------------------------------------------------------- Discord HTTP seam

pub enum Auth {
    Bot,
    Bearer(String),
    None,
}

pub enum Body {
    None,
    Json(Value),
    Form(Vec<(String, String)>),
}

pub struct ApiRequest {
    pub method: &'static str,
    /// Path after `/api/v10`, with any query string.
    pub path: String,
    pub auth: Auth,
    pub body: Body,
}

pub struct ApiResponse {
    pub status: u16,
    pub body: Value,
    pub retry_after: Option<f64>,
}

impl ApiResponse {
    fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Everything that talks to discord.com goes through this, so tests fake it.
#[async_trait]
pub trait DiscordApi: Send + Sync {
    async fn call(&self, request: ApiRequest) -> Result<ApiResponse, String>;
}

pub struct ReqwestDiscord {
    client: reqwest::Client,
    bot_token: String,
}

impl ReqwestDiscord {
    pub fn new(bot_token: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent("DiscordBot (https://allternit.com, 1.0)")
            .build()
            .unwrap_or_default();
        Self { client, bot_token: bot_token.to_string() }
    }
}

#[async_trait]
impl DiscordApi for ReqwestDiscord {
    async fn call(&self, request: ApiRequest) -> Result<ApiResponse, String> {
        let method = reqwest::Method::from_bytes(request.method.as_bytes()).map_err(|e| e.to_string())?;
        let mut builder = self.client.request(method, format!("{API}{}", request.path));
        builder = match request.auth {
            Auth::Bot => builder.header("authorization", format!("Bot {}", self.bot_token)),
            Auth::Bearer(token) => builder.bearer_auth(token),
            Auth::None => builder,
        };
        builder = match request.body {
            Body::None => builder,
            Body::Json(value) => builder.json(&value),
            Body::Form(fields) => builder.form(&fields),
        };
        let response = builder.send().await.map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        let header_retry = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<f64>().ok());
        let bytes = response.bytes().await.map_err(|e| e.to_string())?;
        let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        let retry_after = body.get("retry_after").and_then(Value::as_f64).or(header_retry);
        Ok(ApiResponse { status, body, retry_after })
    }
}

fn default_api(cfg: &DiscordConfig) -> Arc<dyn DiscordApi> {
    Arc::new(ReqwestDiscord::new(&cfg.bot_token))
}

// ---------------------------------------------------------------- install link + signed state

type HmacSha256 = Hmac<Sha256>;

fn state_mac(secret: &str, payload: &str) -> HmacSha256 {
    let mut mac = HmacSha256::new_from_slice(format!("allternit-discord-install-state:{secret}").as_bytes())
        .expect("hmac takes any key length");
    mac.update(payload.as_bytes());
    mac
}

/// `base64url(user|runtime|exp|nonce).hex(hmac)`. Only cloud-api (which knows the client secret) can mint one.
pub fn sign_state(secret: &str, user_id: &str, runtime_id: &str, now: i64) -> String {
    let payload = URL_SAFE_NO_PAD.encode(format!("{user_id}|{runtime_id}|{}|{}", now + STATE_TTL_SECS, new_key()));
    let sig = hex::encode(state_mac(secret, &payload).finalize().into_bytes());
    format!("{payload}.{sig}")
}

/// (user, runtime) when the signature holds and it has not expired.
pub fn verify_state(secret: &str, state: &str, now: i64) -> Option<(String, String)> {
    let (payload, sig) = state.split_once('.')?;
    let sig = hex::decode(sig).ok()?;
    state_mac(secret, payload).verify_slice(&sig).ok()?;
    let decoded = String::from_utf8(URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
    let mut parts = decoded.splitn(4, '|');
    let (user, runtime, exp) = (parts.next()?, parts.next()?, parts.next()?);
    if exp.parse::<i64>().ok()? < now {
        return None;
    }
    Some((user.to_string(), runtime.to_string()))
}

pub fn install_url(cfg: &DiscordConfig, state: &str) -> String {
    let permissions = PERMISSIONS.to_string();
    let redirect = redirect_uri();
    reqwest::Url::parse_with_params(
        "https://discord.com/oauth2/authorize",
        &[
            ("client_id", cfg.app_id.as_str()),
            ("scope", "bot applications.commands identify"),
            ("permissions", permissions.as_str()),
            ("integration_type", "0"),
            ("response_type", "code"),
            ("redirect_uri", redirect.as_str()),
            ("state", state),
        ],
    )
    .map(|u| u.to_string())
    .unwrap_or_default()
}

async fn user_id(state: &ApiState, headers: &HeaderMap) -> Result<String, ApiError> {
    crate::auth::resolve_user_scoped(&state.db, headers, "compute").await.map(|u| u.id)
}

/// `GET /api/v1/channels/discord/installs`: the servers this user added the
/// shared app to, so their runtime can record each as a connection.
async fn list_installs(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Response, ApiError> {
    if DiscordConfig::from_env().is_none() {
        return Ok(not_configured());
    }
    let user = user_id(&state, &headers).await?;
    let rows: Vec<(String, Option<String>, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT guild_id, guild_name, runtime_id, installed_at FROM discord_installs WHERE user_id = $1 AND revoked_at IS NULL ORDER BY installed_at",
    )
    .bind(&user)
    .fetch_all(&state.db)
    .await?;
    let installs: Vec<Value> = rows
        .into_iter()
        .map(|(guild_id, guild_name, runtime_id, installed_at)| json!({ "guildId": guild_id, "guildName": guild_name, "runtimeId": runtime_id, "installedAt": installed_at }))
        .collect();
    Ok(Json(json!({ "installs": installs })).into_response())
}

async fn owns_runtime(db: &sqlx::PgPool, user: &str, runtime_id: &str) -> Result<bool, ApiError> {
    let owns: Option<(String,)> =
        sqlx::query_as("SELECT id FROM runtime_devices WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL")
            .bind(runtime_id)
            .bind(user)
            .fetch_optional(db)
            .await?;
    Ok(owns.is_some())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstallBody {
    runtime_id: String,
}

async fn install(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<InstallBody>) -> Response {
    let Some(cfg) = DiscordConfig::from_env() else { return not_configured() };
    let user = match user_id(&state, &headers).await {
        Ok(user) => user,
        Err(error) => return error.into_response(),
    };
    match install_inner(&state.db, &cfg, &user, &body.runtime_id).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

pub async fn install_inner(db: &sqlx::PgPool, cfg: &DiscordConfig, user: &str, runtime_id: &str) -> Result<Response, ApiError> {
    if !owns_runtime(db, user, runtime_id).await? {
        return Err(ApiError::NotFound("Runtime not found".to_string()));
    }
    let state = sign_state(&cfg.client_secret, user, runtime_id, chrono::Utc::now().timestamp());
    Ok(Json(json!({ "url": install_url(cfg, &state) })).into_response())
}

// ---------------------------------------------------------------- OAuth callback

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    guild_id: Option<String>,
    error: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Installed {
    pub guild_id: String,
    pub guild_name: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum InstallError {
    Denied,
    BadState,
    NoGuild,
    RuntimeGone,
    Discord(String),
    Db(String),
}

impl InstallError {
    fn message(&self) -> &'static str {
        match self {
            InstallError::Denied => "Discord did not authorize the install. Nothing was connected.",
            InstallError::BadState => "This link expired or is not valid. Start again from Allternit.",
            InstallError::NoGuild => "Discord did not say which server to join. Start again from Allternit.",
            InstallError::RuntimeGone => "That Allternit computer is no longer available. Start again from Allternit.",
            InstallError::Discord(_) | InstallError::Db(_) => "Something went wrong connecting. Try again from Allternit.",
        }
    }
}

async fn oauth_callback(State(state): State<Arc<ApiState>>, Query(query): Query<CallbackQuery>) -> Response {
    let Some(cfg) = DiscordConfig::from_env() else { return not_configured() };
    let api = default_api(&cfg);
    match complete_install(&state.db, api.as_ref(), &cfg, &query, chrono::Utc::now().timestamp()).await {
        Ok(done) => (StatusCode::OK, Html(connected_page(&done))).into_response(),
        Err(error) => {
            if matches!(error, InstallError::Discord(_) | InstallError::Db(_)) {
                tracing::warn!("discord install failed: {error:?}");
            }
            let status = match error {
                InstallError::Discord(_) | InstallError::Db(_) => StatusCode::BAD_GATEWAY,
                _ => StatusCode::BAD_REQUEST,
            };
            (status, Html(error_page(error.message()))).into_response()
        }
    }
}

async fn complete_install(
    db: &sqlx::PgPool,
    api: &dyn DiscordApi,
    cfg: &DiscordConfig,
    query: &CallbackQuery,
    now: i64,
) -> Result<Installed, InstallError> {
    if query.error.is_some() {
        return Err(InstallError::Denied);
    }
    let (user, runtime_id) = query
        .state
        .as_deref()
        .and_then(|s| verify_state(&cfg.client_secret, s, now))
        .ok_or(InstallError::BadState)?;
    let code = query.code.clone().ok_or(InstallError::BadState)?;

    let token = api
        .call(ApiRequest {
            method: "POST",
            path: "/oauth2/token".to_string(),
            auth: Auth::None,
            body: Body::Form(vec![
                ("client_id".into(), cfg.app_id.clone()),
                ("client_secret".into(), cfg.client_secret.clone()),
                ("grant_type".into(), "authorization_code".into()),
                ("code".into(), code),
                ("redirect_uri".into(), redirect_uri()),
            ]),
        })
        .await
        .map_err(InstallError::Discord)?;
    if !token.ok() {
        return Err(InstallError::Discord(format!("token exchange {}", token.status)));
    }
    // With the bot scope the token response names the guild that was joined.
    let guild_id = token
        .body
        .pointer("/guild/id")
        .and_then(Value::as_str)
        .map(String::from)
        .or_else(|| query.guild_id.clone())
        .ok_or(InstallError::NoGuild)?;
    let guild_name = token.body.pointer("/guild/name").and_then(Value::as_str).map(String::from);

    // `identify`: who installed it, so a DM from them reaches this runtime.
    let discord_user = match token.body.get("access_token").and_then(Value::as_str) {
        Some(access) => api
            .call(ApiRequest {
                method: "GET",
                path: "/users/@me".to_string(),
                auth: Auth::Bearer(access.to_string()),
                body: Body::None,
            })
            .await
            .ok()
            .filter(ApiResponse::ok)
            .and_then(|r| r.body.get("id").and_then(Value::as_str).map(String::from)),
        None => None,
    };

    let owns = owns_runtime(db, &user, &runtime_id).await.map_err(|e| InstallError::Db(e.to_string()))?;
    if !owns {
        return Err(InstallError::RuntimeGone);
    }
    store_install(db, &user, &runtime_id, &guild_id, guild_name.as_deref(), discord_user.as_deref(), None)
        .await
        .map_err(|e| InstallError::Db(e.to_string()))?;
    Ok(Installed { guild_id, guild_name })
}

/// Upsert the install and its relay route (reusing the route on a reinstall for the same runtime).
async fn store_install(
    db: &sqlx::PgPool,
    user: &str,
    runtime_id: &str,
    guild_id: &str,
    guild_name: Option<&str>,
    discord_user: Option<&str>,
    permissions: Option<&str>,
) -> Result<(), sqlx::Error> {
    let existing: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT i.route_id FROM discord_installs i
           JOIN channel_inbound_routes r ON r.id = i.route_id AND r.revoked_at IS NULL AND r.runtime_id = $2 AND r.user_id = $3
          WHERE i.guild_id = $1",
    )
    .bind(guild_id)
    .bind(runtime_id)
    .bind(user)
    .fetch_optional(db)
    .await?;
    let route_id = match existing.and_then(|(id,)| id) {
        Some(id) => id,
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            // Nobody holds this route's key (it is dropped here): events are queued
            // for it internally, and the public address can never be used.
            sqlx::query(
                "INSERT INTO channel_inbound_routes (id, key_hash, user_id, runtime_id, provider, label) VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(&id)
            .bind(sha256_hex(&new_key()))
            .bind(user)
            .bind(runtime_id)
            .bind(PROVIDER)
            .bind(format!("discord-app:{guild_id}"))
            .execute(db)
            .await?;
            id
        }
    };
    sqlx::query(
        "INSERT INTO discord_installs (guild_id, user_id, runtime_id, route_id, guild_name, installed_by_discord_id, permissions)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (guild_id) DO UPDATE SET
            user_id = EXCLUDED.user_id, runtime_id = EXCLUDED.runtime_id, route_id = EXCLUDED.route_id,
            guild_name = COALESCE(EXCLUDED.guild_name, discord_installs.guild_name),
            installed_by_discord_id = COALESCE(EXCLUDED.installed_by_discord_id, discord_installs.installed_by_discord_id),
            permissions = COALESCE(EXCLUDED.permissions, discord_installs.permissions),
            updated_at = now(), revoked_at = NULL",
    )
    .bind(guild_id)
    .bind(user)
    .bind(runtime_id)
    .bind(route_id)
    .bind(guild_name)
    .bind(discord_user)
    .bind(permissions)
    .execute(db)
    .await?;
    Ok(())
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

fn page(title: &str, body: &str, deep_link: bool) -> String {
    let link = if deep_link {
        r#"<p><a class="btn" href="allternit://channels/discord/connected">Open Allternit</a></p>
<script>setTimeout(function(){location.href="allternit://channels/discord/connected"},400)</script>"#
    } else {
        r#"<p><a class="btn" href="https://ai.allternit.com">Back to Allternit</a></p>"#
    };
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>{title}</title>
<style>:root{{color-scheme:light dark}}body{{margin:0;min-height:100vh;display:grid;place-items:center;font:16px/1.5 system-ui,sans-serif;background:#fff;color:#111}}
@media(prefers-color-scheme:dark){{body{{background:#0b0b0c;color:#f2f2f2}}}}
main{{max-width:26rem;padding:2rem;text-align:center}}h1{{font-size:1.4rem;margin:0 0 .5rem}}p{{margin:.5rem 0;opacity:.8}}
.btn{{display:inline-block;margin-top:.75rem;padding:.6rem 1.1rem;border-radius:10px;background:#111;color:#fff;text-decoration:none;opacity:1}}
@media(prefers-color-scheme:dark){{.btn{{background:#f2f2f2;color:#111}}}}</style></head>
<body><main><h1>{title}</h1><p>{body}</p>{link}</main></body></html>"#
    )
}

fn connected_page(done: &Installed) -> String {
    let server = done.guild_name.as_deref().map(html_escape).unwrap_or_else(|| "your server".to_string());
    page("Connected", &format!("Allternit is in {server}. You can close this tab."), true)
}

fn error_page(message: &str) -> String {
    page("Not connected", &html_escape(message), false)
}

// ---------------------------------------------------------------- inbound envelope + routing

/// What the runtime receives for one Discord event (JSON body, camelCase).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inbound {
    /// `message` or `command`.
    pub kind: &'static str,
    pub guild_id: Option<String>,
    /// For threads: the parent channel (the webhook lives there).
    pub channel_id: String,
    pub thread_id: Option<String>,
    pub message_id: Option<String>,
    pub author_id: String,
    pub author_name: Option<String>,
    pub content: String,
    pub is_dm: bool,
    pub mentions_app: bool,
    pub reply_to_message_id: Option<String>,
    pub command_name: Option<String>,
    pub interaction_id: Option<String>,
    pub attachments: Vec<Value>,
}

impl Inbound {
    pub fn envelope(&self) -> Value {
        json!({
            "source": ENVELOPE_SOURCE,
            "kind": self.kind,
            "guildId": self.guild_id,
            "channelId": self.channel_id,
            "threadId": self.thread_id,
            "messageId": self.message_id,
            "authorId": self.author_id,
            "authorName": self.author_name,
            "content": self.content,
            "isDm": self.is_dm,
            "mentionsApp": self.mentions_app,
            "replyToMessageId": self.reply_to_message_id,
            "commandName": self.command_name,
            "interactionId": self.interaction_id,
            "attachments": self.attachments,
        })
    }
}

fn str_at<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(Value::as_str)
}

/// A MESSAGE_CREATE the app should act on, or `None`. Without the MESSAGE_CONTENT
/// intent Discord still sends content for DMs and messages that mention the app.
pub fn classify_message(data: &Value, app_id: &str) -> Option<Inbound> {
    let author_id = str_at(data, "/author/id")?;
    if author_id == app_id
        || data.pointer("/author/bot").and_then(Value::as_bool).unwrap_or(false)
        || data.get("webhook_id").is_some_and(|v| !v.is_null())
    {
        return None;
    }
    let guild_id = str_at(data, "/guild_id").map(String::from);
    let is_dm = guild_id.is_none();
    let mentions_app = data
        .get("mentions")
        .and_then(Value::as_array)
        .is_some_and(|users| users.iter().any(|u| str_at(u, "/id") == Some(app_id)));
    // A reply to something the app (or one of its webhooks) posted.
    let reply_to_app = data.get("referenced_message").is_some_and(|m| {
        str_at(m, "/author/id") == Some(app_id) || str_at(m, "/application_id") == Some(app_id)
    });
    if !is_dm && !mentions_app && !reply_to_app {
        return None;
    }
    let content = str_at(data, "/content").unwrap_or_default();
    let content = content.replace(&format!("<@{app_id}>"), "").replace(&format!("<@!{app_id}>"), "");
    let attachments = data
        .get("attachments")
        .and_then(Value::as_array)
        .map(|files| {
            files
                .iter()
                .filter_map(|f| {
                    Some(json!({
                        "url": str_at(f, "/url")?,
                        "filename": str_at(f, "/filename"),
                        "contentType": str_at(f, "/content_type"),
                    }))
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Inbound {
        kind: "message",
        guild_id,
        channel_id: str_at(data, "/channel_id")?.to_string(),
        thread_id: None,
        message_id: str_at(data, "/id").map(String::from),
        author_id: author_id.to_string(),
        author_name: str_at(data, "/member/nick")
            .or_else(|| str_at(data, "/author/global_name"))
            .or_else(|| str_at(data, "/author/username"))
            .map(String::from),
        content: content.trim().to_string(),
        is_dm,
        mentions_app,
        reply_to_message_id: str_at(data, "/message_reference/message_id").map(String::from),
        command_name: None,
        interaction_id: None,
        attachments,
    })
}

/// A slash-command interaction as an inbound event.
pub fn command_inbound(interaction: &Value) -> Option<Inbound> {
    let user = interaction.pointer("/member/user").or_else(|| interaction.get("user"))?;
    let text = interaction
        .pointer("/data/options")
        .and_then(Value::as_array)
        .and_then(|options| options.iter().find(|o| str_at(o, "/name") == Some("message")))
        .and_then(|o| str_at(o, "/value"))
        .unwrap_or_default();
    let (channel_id, thread_id) = match interaction.pointer("/channel/type").and_then(Value::as_u64) {
        Some(10..=12) => (
            str_at(interaction, "/channel/parent_id").or_else(|| str_at(interaction, "/channel_id"))?.to_string(),
            str_at(interaction, "/channel_id").map(String::from),
        ),
        _ => (str_at(interaction, "/channel_id")?.to_string(), None),
    };
    let guild_id = str_at(interaction, "/guild_id").map(String::from);
    Some(Inbound {
        kind: "command",
        is_dm: guild_id.is_none(),
        guild_id,
        channel_id,
        thread_id,
        message_id: None,
        author_id: str_at(user, "/id")?.to_string(),
        author_name: str_at(user, "/global_name").or_else(|| str_at(user, "/username")).map(String::from),
        content: text.to_string(),
        mentions_app: true,
        reply_to_message_id: None,
        command_name: str_at(interaction, "/data/name").map(String::from),
        interaction_id: str_at(interaction, "/id").map(String::from),
        attachments: vec![],
    })
}

pub struct Owner {
    pub route_id: String,
}

/// The install an event belongs to: by guild, or for a DM by the Discord user who installed.
async fn owner_for(db: &sqlx::PgPool, guild_id: Option<&str>, author_id: &str) -> Result<Option<Owner>, sqlx::Error> {
    let row: Option<(Option<String>,)> = match guild_id {
        Some(guild) => {
            sqlx::query_as("SELECT route_id FROM discord_installs WHERE guild_id = $1 AND revoked_at IS NULL")
                .bind(guild)
                .fetch_optional(db)
                .await?
        }
        None => {
            sqlx::query_as(
                "SELECT route_id FROM discord_installs WHERE installed_by_discord_id = $1 AND revoked_at IS NULL
                  ORDER BY updated_at DESC LIMIT 1",
            )
            .bind(author_id)
            .fetch_optional(db)
            .await?
        }
    };
    Ok(row.and_then(|(id,)| id).map(|route_id| Owner { route_id }))
}

/// Thread messages arrive with the thread's id as the channel; webhooks live on the parent.
async fn resolve_thread(api: &dyn DiscordApi, channel_id: &str) -> (String, Option<String>) {
    static CACHE: Mutex<Option<HashMap<String, (String, Option<String>)>>> = Mutex::new(None);
    if let Some(hit) = CACHE.lock().ok().and_then(|c| c.as_ref().and_then(|m| m.get(channel_id).cloned())) {
        return hit;
    }
    let found = api
        .call(ApiRequest { method: "GET", path: format!("/channels/{channel_id}"), auth: Auth::Bot, body: Body::None })
        .await
        .ok()
        .filter(ApiResponse::ok);
    let Some(found) = found else { return (channel_id.to_string(), None) };
    let resolved = match found.body.get("type").and_then(Value::as_u64) {
        Some(10..=12) => (
            str_at(&found.body, "/parent_id").unwrap_or(channel_id).to_string(),
            Some(channel_id.to_string()),
        ),
        _ => (channel_id.to_string(), None),
    };
    if let Ok(mut cache) = CACHE.lock() {
        let map = cache.get_or_insert_with(HashMap::new);
        if map.len() > 5000 {
            map.clear();
        }
        map.insert(channel_id.to_string(), resolved.clone());
    }
    resolved
}

/// Insert one inbound event into the runtime's relay queue (same table `/channels/in/:key` uses).
async fn enqueue(db: &sqlx::PgPool, route_id: &str, inbound: &Inbound) -> Result<(), sqlx::Error> {
    let body = serde_json::to_vec(&inbound.envelope()).unwrap_or_default();
    sqlx::query("INSERT INTO channel_inbound_queue (route_id, method, query, headers, body) VALUES ($1, 'POST', '', $2, $3)")
        .bind(route_id)
        .bind(json!({ "content-type": "application/json" }))
        .bind(b64_std(&body))
        .execute(db)
        .await?;
    let _ = sqlx::query("UPDATE channel_inbound_routes SET last_inbound_at = now() WHERE id = $1")
        .bind(route_id)
        .execute(db)
        .await;
    Ok(())
}

/// The queue stores bodies as standard base64.
fn b64_std(body: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(body)
}

/// Gateway MESSAGE_CREATE → queue. Returns the route to deliver, or `None` when ignored.
pub async fn route_message_create(
    db: &sqlx::PgPool,
    api: &dyn DiscordApi,
    cfg: &DiscordConfig,
    data: &Value,
) -> Result<Option<String>, sqlx::Error> {
    let Some(mut inbound) = classify_message(data, &cfg.app_id) else { return Ok(None) };
    let Some(owner) = owner_for(db, inbound.guild_id.as_deref(), &inbound.author_id).await? else { return Ok(None) };
    if !inbound.is_dm {
        let (channel, thread) = resolve_thread(api, &inbound.channel_id).await;
        inbound.channel_id = channel;
        inbound.thread_id = thread;
    }
    enqueue(db, &owner.route_id, &inbound).await?;
    Ok(Some(owner.route_id))
}

// ---------------------------------------------------------------- interactions

pub fn verify_signature(public_key_hex: &str, signature_hex: &str, timestamp: &str, body: &[u8]) -> bool {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let (Ok(key), Ok(sig)) = (hex::decode(public_key_hex), hex::decode(signature_hex)) else { return false };
    let (Ok(key), Ok(sig)) = (<[u8; 32]>::try_from(key.as_slice()), Signature::from_slice(&sig)) else { return false };
    let Ok(key) = VerifyingKey::from_bytes(&key) else { return false };
    let mut message = timestamp.as_bytes().to_vec();
    message.extend_from_slice(body);
    key.verify(&message, &sig).is_ok()
}

async fn interactions(State(state): State<Arc<ApiState>>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(cfg) = DiscordConfig::from_env() else { return not_configured() };
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    let (signature, timestamp) = (header("x-signature-ed25519"), header("x-signature-timestamp"));
    let api = default_api(&cfg);
    let (status, value, route) = interactions_inner(&state.db, api.clone(), &cfg, &signature, &timestamp, &body).await;
    if let Some(route_id) = route {
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(error) = deliver_route(&state, &route_id).await {
                tracing::warn!(%route_id, "discord interaction delivery failed: {error}");
            }
        });
    }
    (status, Json(value)).into_response()
}

/// Verify, answer PING, defer-and-queue a slash command. The third value is a route to deliver now.
pub async fn interactions_inner(
    db: &sqlx::PgPool,
    api: Arc<dyn DiscordApi>,
    cfg: &DiscordConfig,
    signature: &str,
    timestamp: &str,
    body: &[u8],
) -> (StatusCode, Value, Option<String>) {
    if !verify_signature(&cfg.public_key, signature, timestamp, body) {
        return (StatusCode::UNAUTHORIZED, json!({ "error": "invalid_signature" }), None);
    }
    let Ok(interaction) = serde_json::from_slice::<Value>(body) else {
        return (StatusCode::BAD_REQUEST, json!({ "error": "invalid_json" }), None);
    };
    match interaction.get("type").and_then(Value::as_u64) {
        // PING: the 3-second answer Discord requires before it accepts the endpoint.
        Some(1) => (StatusCode::OK, json!({ "type": 1 }), None),
        Some(2) => {
            let ephemeral = |text: &str| json!({ "type": 4, "data": { "content": text, "flags": 64 } });
            let Some(mut inbound) = command_inbound(&interaction) else {
                return (StatusCode::OK, ephemeral("That command could not be read."), None);
            };
            let owner = owner_for(db, inbound.guild_id.as_deref(), &inbound.author_id).await;
            let route_id = match owner {
                Ok(Some(owner)) => owner.route_id,
                Ok(None) => {
                    return (StatusCode::OK, ephemeral("This server is not connected to an Allternit computer."), None)
                }
                Err(error) => {
                    tracing::warn!("discord interaction owner lookup failed: {error}");
                    return (StatusCode::OK, ephemeral("Allternit is unavailable. Try again shortly."), None);
                }
            };
            if let Err(error) = enqueue(db, &route_id, &inbound).await {
                tracing::warn!("discord interaction enqueue failed: {error}");
                return (StatusCode::OK, ephemeral("Allternit is unavailable. Try again shortly."), None);
            }
            // Deferred and ephemeral: the bot's answer is a normal webhook message in the
            // channel, so the "thinking" placeholder is replaced with a short note.
            let (token, name) = (
                str_at(&interaction, "/token").unwrap_or_default().to_string(),
                std::mem::take(&mut inbound.command_name).unwrap_or_default(),
            );
            let app_id = cfg.app_id.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(800)).await;
                let _ = api
                    .call(ApiRequest {
                        method: "PATCH",
                        path: format!("/webhooks/{app_id}/{token}/messages/@original"),
                        auth: Auth::None,
                        body: Body::Json(json!({ "content": format!("Asked {name}. Its reply will appear here.") })),
                    })
                    .await;
            });
            (StatusCode::OK, json!({ "type": 5, "data": { "flags": 64 } }), Some(route_id))
        }
        // Components and autocomplete are not used by Allternit's commands.
        Some(3) => (StatusCode::OK, json!({ "type": 6 }), None),
        Some(4) => (StatusCode::OK, json!({ "type": 8, "data": { "choices": [] } }), None),
        _ => (StatusCode::BAD_REQUEST, json!({ "error": "unsupported_interaction" }), None),
    }
}

// ---------------------------------------------------------------- slash commands

/// Discord command names: lowercase, 1–32 of `a-z 0-9 - _`. Deduped, never "discord" or "clyde".
pub fn command_names(names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for name in names {
        let clean: String = name
            .to_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' })
            .collect();
        let clean = clean.trim_matches('-').chars().take(32).collect::<String>();
        let clean = scrub_reserved(&clean);
        if !clean.is_empty() && !out.contains(&clean) && out.len() < MAX_COMMANDS {
            out.push(clean);
        }
    }
    out
}

fn command_payload(names: &[String]) -> Value {
    Value::Array(
        names
            .iter()
            .map(|name| {
                json!({
                    "name": name, "type": 1, "description": format!("Talk to {name}"),
                    "options": [{ "type": 3, "name": "message", "description": "What to say", "required": true }],
                })
            })
            .collect(),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommandsBody {
    guild_id: String,
    names: Vec<String>,
}

/// The caller's active install of `guild_id`, or the response to return.
async fn require_install(state: &ApiState, headers: &HeaderMap, guild_id: &str) -> Result<(), Response> {
    let user = user_id(state, headers).await.map_err(IntoResponse::into_response)?;
    check_install(&state.db, &user, guild_id).await
}

async fn check_install(db: &sqlx::PgPool, user: &str, guild_id: &str) -> Result<(), Response> {
    let row: Result<Option<(String,)>, _> =
        sqlx::query_as("SELECT guild_id FROM discord_installs WHERE guild_id = $1 AND user_id = $2 AND revoked_at IS NULL")
            .bind(guild_id)
            .bind(user)
            .fetch_optional(db)
            .await;
    match row {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(json_error(StatusCode::NOT_FOUND, "discord_not_installed")),
        Err(error) => Err(ApiError::from(error).into_response()),
    }
}

async fn commands(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<CommandsBody>) -> Response {
    let Some(cfg) = DiscordConfig::from_env() else { return not_configured() };
    if let Err(response) = require_install(&state, &headers, &body.guild_id).await {
        return response;
    }
    register_commands(default_api(&cfg).as_ref(), &cfg, &body.guild_id, &body.names).await
}

pub async fn register_commands(api: &dyn DiscordApi, cfg: &DiscordConfig, guild_id: &str, names: &[String]) -> Response {
    let names = command_names(names);
    let result = api
        .call(ApiRequest {
            method: "PUT",
            path: format!("/applications/{}/guilds/{guild_id}/commands", cfg.app_id),
            auth: Auth::Bot,
            body: Body::Json(command_payload(&names)),
        })
        .await;
    match result {
        Ok(r) if r.ok() => Json(json!({ "registered": names })).into_response(),
        Ok(r) => (StatusCode::BAD_GATEWAY, Json(json!({ "error": "discord_error", "status": r.status }))).into_response(),
        Err(error) => {
            tracing::warn!("discord command registration failed: {error}");
            json_error(StatusCode::BAD_GATEWAY, "discord_unreachable")
        }
    }
}

// ---------------------------------------------------------------- send through webhooks

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendBody {
    pub guild_id: String,
    pub channel_id: String,
    pub thread_id: Option<String>,
    pub bot_name: String,
    pub avatar_url: Option<String>,
    pub text: String,
}

/// Discord refuses webhook names containing "discord" or "clyde" (any case).
fn scrub_reserved(name: &str) -> String {
    let mut out = name.to_string();
    for word in ["discord", "clyde"] {
        while let Some(at) = out.to_lowercase().find(word) {
            out.replace_range(at..at + word.len(), "");
        }
    }
    out
}

pub fn webhook_username(bot_name: &str) -> String {
    let cleaned: String = scrub_reserved(bot_name).chars().filter(|c| !matches!(c, '@' | '#' | ':' | '`')).collect();
    let cleaned: String = cleaned.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(80).collect();
    if cleaned.trim().is_empty() {
        "Allternit".to_string()
    } else {
        cleaned
    }
}

/// Split at 2,000 characters, preferring a line break.
pub fn chunk_text(text: &str) -> Vec<String> {
    let mut chunks = vec![];
    let mut rest = text.trim().to_string();
    while rest.chars().count() > DISCORD_MESSAGE_LIMIT {
        let head: String = rest.chars().take(DISCORD_MESSAGE_LIMIT).collect();
        let cut = head.rfind('\n').filter(|&i| i > DISCORD_MESSAGE_LIMIT / 2).unwrap_or(head.len());
        chunks.push(head[..cut].trim_end().to_string());
        rest = rest[cut..].trim_start().to_string();
    }
    if !rest.is_empty() {
        chunks.push(rest);
    }
    chunks
}

#[derive(Debug)]
pub enum SendError {
    Invalid(&'static str),
    WrongGuild,
    /// The caller has no (or not that) server with the app installed.
    NotInstalled,
    /// The person shares no installed server with the caller, so the bot may not DM them.
    NotInServer,
    /// Discord error 50007: the person's settings refuse DMs from this app.
    DmClosed,
    Discord(u16),
    Unreachable(String),
    Db(String),
}

impl SendError {
    fn into_response(self) -> Response {
        match self {
            SendError::Invalid(why) => json_error(StatusCode::BAD_REQUEST, why),
            SendError::WrongGuild => json_error(StatusCode::FORBIDDEN, "channel_not_in_guild"),
            SendError::NotInstalled => json_error(StatusCode::NOT_FOUND, "discord_not_installed"),
            SendError::NotInServer => json_error(StatusCode::FORBIDDEN, "user_not_in_server"),
            SendError::DmClosed => json_error(StatusCode::FORBIDDEN, "dm_closed"),
            SendError::Discord(status) => {
                (StatusCode::BAD_GATEWAY, Json(json!({ "error": "discord_error", "status": status }))).into_response()
            }
            SendError::Unreachable(error) => {
                tracing::warn!("discord send failed: {error}");
                json_error(StatusCode::BAD_GATEWAY, "discord_unreachable")
            }
            SendError::Db(error) => {
                tracing::warn!("discord send db error: {error}");
                json_error(StatusCode::INTERNAL_SERVER_ERROR, "internal")
            }
        }
    }
}

async fn send(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<SendBody>) -> Response {
    let Some(cfg) = DiscordConfig::from_env() else { return not_configured() };
    if let Err(response) = require_install(&state, &headers, &body.guild_id).await {
        return response;
    }
    match send_message(&state.db, default_api(&cfg).as_ref(), &body).await {
        Ok(ids) => Json(json!({ "messageId": ids[0], "messageIds": ids })).into_response(),
        Err(error) => error.into_response(),
    }
}

// ---------------------------------------------------------------- direct messages

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DmBody {
    pub guild_id: Option<String>,
    pub user_id: String,
    /// Without text the DM channel is only opened (a conversation start needs its id first).
    pub text: Option<String>,
    pub bot_name: Option<String>,
}

async fn dm(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<DmBody>) -> Response {
    let Some(cfg) = DiscordConfig::from_env() else { return not_configured() };
    let user = match user_id(&state, &headers).await {
        Ok(user) => user,
        Err(error) => return error.into_response(),
    };
    match dm_message(&state.db, default_api(&cfg).as_ref(), &user, &body).await {
        Ok((channel, message)) => Json(json!({ "channelId": channel, "messageId": message })).into_response(),
        Err(error) => error.into_response(),
    }
}

async fn installed_guilds(db: &sqlx::PgPool, user: &str) -> Result<Vec<String>, SendError> {
    let rows: Vec<(String,)> = sqlx::query_as("SELECT guild_id FROM discord_installs WHERE user_id = $1 AND revoked_at IS NULL ORDER BY installed_at DESC")
        .bind(user)
        .fetch_all(db)
        .await
        .map_err(|e| SendError::Db(e.to_string()))?;
    Ok(rows.into_iter().map(|(g,)| g).collect())
}

/// Open a DM with `body.user_id` and, with `text`, post in it. The bot only messages people who
/// are members of a server the caller installed the app in. Returns `(channel id, first message id)`.
/// https://discord.com/developers/docs/resources/user#create-dm
/// https://discord.com/developers/docs/resources/guild#get-guild-member
/// https://discord.com/developers/docs/resources/message#create-message
pub async fn dm_message(db: &sqlx::PgPool, api: &dyn DiscordApi, user: &str, body: &DmBody) -> Result<(String, Option<String>), SendError> {
    let target = body.user_id.trim();
    if !(15..=25).contains(&target.len()) || !target.chars().all(|c| c.is_ascii_digit()) {
        return Err(SendError::Invalid("invalid_user_id"));
    }
    let installed = installed_guilds(db, user).await?;
    let candidates: Vec<String> = match body.guild_id.as_deref().filter(|g| !g.is_empty()) {
        Some(guild) if installed.iter().any(|g| g == guild) => vec![guild.to_string()],
        _ => installed,
    };
    if candidates.is_empty() {
        return Err(SendError::NotInstalled);
    }
    let mut member = false;
    for guild in &candidates {
        let r = api.call(ApiRequest { method: "GET", path: format!("/guilds/{guild}/members/{target}"), auth: Auth::Bot, body: Body::None }).await.map_err(call_error)?;
        match r.status {
            200..=299 => {
                member = true;
                break;
            }
            404 => {}
            status => return Err(SendError::Discord(status)),
        }
    }
    if !member {
        return Err(SendError::NotInServer);
    }
    let opened = api.call(ApiRequest { method: "POST", path: "/users/@me/channels".into(), auth: Auth::Bot, body: Body::Json(json!({ "recipient_id": target })) }).await.map_err(call_error)?;
    if !opened.ok() {
        return Err(if is_dm_closed(&opened.body) { SendError::DmClosed } else { SendError::Discord(opened.status) });
    }
    let channel = str_at(&opened.body, "/id").map(String::from).ok_or(SendError::Discord(opened.status))?;
    let Some(text) = body.text.as_deref().filter(|t| !t.trim().is_empty()) else { return Ok((channel, None)) };
    // A DM can't wear a webhook's name and avatar, so the bot's name leads the message.
    let text = match body.bot_name.as_deref().map(|n| n.chars().filter(|c| !"*_~`|>\\@#".contains(*c)).take(80).collect::<String>()).filter(|n| !n.trim().is_empty()) {
        Some(name) => format!("**{}**\n{text}", name.trim()),
        None => text.to_string(),
    };
    let mut first = None;
    for chunk in chunk_text(&text) {
        let mut waited = false;
        let id = loop {
            let payload = json!({ "content": chunk, "allowed_mentions": { "parse": [] } });
            let r = api.call(ApiRequest { method: "POST", path: format!("/channels/{channel}/messages"), auth: Auth::Bot, body: Body::Json(payload) }).await.map_err(call_error)?;
            match r.status {
                200..=299 => break str_at(&r.body, "/id").map(String::from).ok_or(SendError::Discord(r.status))?,
                429 if !waited => {
                    waited = true;
                    tokio::time::sleep(Duration::from_secs_f64(r.retry_after.unwrap_or(1.0).clamp(0.1, 5.0))).await;
                }
                _ if is_dm_closed(&r.body) => return Err(SendError::DmClosed),
                status => return Err(SendError::Discord(status)),
            }
        };
        first.get_or_insert(id);
    }
    Ok((channel, first))
}

/// 50007: Cannot send messages to this user.
fn is_dm_closed(body: &Value) -> bool {
    body.get("code").and_then(Value::as_u64) == Some(50007)
}

fn call_error(error: String) -> SendError {
    SendError::Unreachable(error)
}

/// Post `text` as the bot through the channel's webhook. Returns the message ids, first chunk first.
pub async fn send_message(db: &sqlx::PgPool, api: &dyn DiscordApi, body: &SendBody) -> Result<Vec<String>, SendError> {
    let chunks = chunk_text(&body.text);
    if chunks.is_empty() {
        return Err(SendError::Invalid("empty_text"));
    }
    let avatar = body.avatar_url.as_deref().filter(|u| u.starts_with("https://") && u.len() <= 2048);
    let username = webhook_username(&body.bot_name);
    let mut ids = vec![];
    for chunk in chunks {
        let mut payload = json!({ "content": chunk, "username": username, "allowed_mentions": { "parse": [] } });
        if let Some(avatar) = avatar {
            payload["avatar_url"] = json!(avatar);
        }
        ids.push(execute_webhook(db, api, body, &payload).await?);
    }
    Ok(ids)
}

async fn execute_webhook(
    db: &sqlx::PgPool,
    api: &dyn DiscordApi,
    body: &SendBody,
    payload: &Value,
) -> Result<String, SendError> {
    let mut recreated = false;
    let mut waited = false;
    loop {
        let (webhook_id, token) = ensure_webhook(db, api, &body.guild_id, &body.channel_id, recreated).await?;
        let mut path = format!("/webhooks/{webhook_id}/{token}?wait=true");
        if let Some(thread) = &body.thread_id {
            path.push_str(&format!("&thread_id={thread}"));
        }
        let response = api
            .call(ApiRequest { method: "POST", path, auth: Auth::None, body: Body::Json(payload.clone()) })
            .await
            .map_err(call_error)?;
        match response.status {
            200..=299 => {
                return str_at(&response.body, "/id").map(String::from).ok_or(SendError::Discord(response.status))
            }
            // The webhook was deleted in Discord: forget it and make a new one, once.
            404 | 401 if !recreated && is_dead_webhook(&response.body) => {
                recreated = true;
                forget_webhook(db, &body.channel_id).await;
            }
            429 if !waited => {
                waited = true;
                tokio::time::sleep(Duration::from_secs_f64(response.retry_after.unwrap_or(1.0).clamp(0.1, 5.0))).await;
            }
            status => return Err(SendError::Discord(status)),
        }
    }
}

/// 10015 Unknown Webhook, 50027 Invalid Webhook Token.
fn is_dead_webhook(body: &Value) -> bool {
    matches!(body.get("code").and_then(Value::as_u64), Some(10015 | 50027))
}

fn webhook_cache() -> &'static Mutex<HashMap<String, (String, String)>> {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<String, (String, String)>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn forget_webhook(db: &sqlx::PgPool, channel_id: &str) {
    if let Ok(mut cache) = webhook_cache().lock() {
        cache.remove(channel_id);
    }
    let _ = sqlx::query("DELETE FROM discord_webhooks WHERE channel_id = $1").bind(channel_id).execute(db).await;
}

/// The (id, token) of the app's webhook in `channel_id`, created on first use. The channel must
/// belong to `guild_id`, so one user's install can never post into another server.
async fn ensure_webhook(
    db: &sqlx::PgPool,
    api: &dyn DiscordApi,
    guild_id: &str,
    channel_id: &str,
    force_new: bool,
) -> Result<(String, String), SendError> {
    let db_err = |e: sqlx::Error| SendError::Db(e.to_string());
    if !force_new {
        if let Some(hit) = webhook_cache().lock().ok().and_then(|c| c.get(channel_id).cloned()) {
            // Cached entries were only stored after the guild check below.
            let known: Option<(String,)> = sqlx::query_as("SELECT guild_id FROM discord_webhooks WHERE channel_id = $1")
                .bind(channel_id)
                .fetch_optional(db)
                .await
                .map_err(db_err)?;
            if known.is_some_and(|(g,)| g == guild_id) {
                return Ok(hit);
            }
        }
        let row: Option<(String, String)> =
            sqlx::query_as("SELECT guild_id, webhook_id FROM discord_webhooks WHERE channel_id = $1")
                .bind(channel_id)
                .fetch_optional(db)
                .await
                .map_err(db_err)?;
        if let Some((row_guild, webhook_id)) = row {
            if row_guild != guild_id {
                return Err(SendError::WrongGuild);
            }
            // Only the id is stored; the token comes back from Discord for app-owned webhooks.
            let fetched = api
                .call(ApiRequest { method: "GET", path: format!("/webhooks/{webhook_id}"), auth: Auth::Bot, body: Body::None })
                .await
                .map_err(call_error)?;
            if let (true, Some(token)) = (fetched.ok(), str_at(&fetched.body, "/token")) {
                let pair = (webhook_id, token.to_string());
                if let Ok(mut cache) = webhook_cache().lock() {
                    cache.insert(channel_id.to_string(), pair.clone());
                }
                return Ok(pair);
            }
            // Gone, or Discord withheld the token: drop it (best effort, so webhooks don't pile
            // up toward the 15-per-channel cap) and make a fresh one below.
            let _ = api
                .call(ApiRequest { method: "DELETE", path: format!("/webhooks/{webhook_id}"), auth: Auth::Bot, body: Body::None })
                .await;
            forget_webhook(db, channel_id).await;
        }
    }
    let channel = api
        .call(ApiRequest { method: "GET", path: format!("/channels/{channel_id}"), auth: Auth::Bot, body: Body::None })
        .await
        .map_err(call_error)?;
    if !channel.ok() {
        return Err(SendError::Discord(channel.status));
    }
    if str_at(&channel.body, "/guild_id") != Some(guild_id) {
        return Err(SendError::WrongGuild);
    }
    let created = api
        .call(ApiRequest {
            method: "POST",
            path: format!("/channels/{channel_id}/webhooks"),
            auth: Auth::Bot,
            body: Body::Json(json!({ "name": "Allternit" })),
        })
        .await
        .map_err(call_error)?;
    let (Some(id), Some(token)) = (str_at(&created.body, "/id"), str_at(&created.body, "/token")) else {
        return Err(SendError::Discord(created.status));
    };
    sqlx::query(
        "INSERT INTO discord_webhooks (channel_id, guild_id, webhook_id) VALUES ($1, $2, $3)
         ON CONFLICT (channel_id) DO UPDATE SET guild_id = EXCLUDED.guild_id, webhook_id = EXCLUDED.webhook_id",
    )
    .bind(channel_id)
    .bind(guild_id)
    .bind(id)
    .execute(db)
    .await
    .map_err(db_err)?;
    let pair = (id.to_string(), token.to_string());
    if let Ok(mut cache) = webhook_cache().lock() {
        cache.insert(channel_id.to_string(), pair.clone());
    }
    Ok(pair)
}

// ---------------------------------------------------------------- gateway

/// What the socket loop should do after a frame.
#[derive(Debug, PartialEq)]
pub enum Action {
    Send(Value),
    Reconnect { resume: bool },
    Dispatch { event: String, data: Value },
}

#[derive(Default)]
pub struct GatewayState {
    seq: Option<u64>,
    session_id: Option<String>,
    resume_url: Option<String>,
    pub heartbeat_ms: Option<u64>,
    acked: bool,
}

impl GatewayState {
    fn can_resume(&self) -> bool {
        self.session_id.is_some() && self.seq.is_some()
    }

    fn identify(&self, cfg: &DiscordConfig) -> Value {
        json!({ "op": 2, "d": {
            "token": cfg.bot_token,
            "intents": INTENTS,
            "properties": { "os": std::env::consts::OS, "browser": "allternit", "device": "allternit" },
            // TODO(sharding): Discord requires sharding past 2,500 guilds. Read `shards` from
            // GET /gateway/bot, run one socket (and advisory lock key) per shard, and route
            // DMs through shard 0.
            "shard": [0, 1],
        }})
    }

    /// The heartbeat tick: a missed ACK means the connection is dead, so resume on a new one.
    pub fn heartbeat(&mut self) -> Action {
        if !self.acked {
            return Action::Reconnect { resume: true };
        }
        self.acked = false;
        Action::Send(json!({ "op": 1, "d": self.seq }))
    }

    pub fn on_frame(&mut self, frame: &Value, cfg: &DiscordConfig) -> Vec<Action> {
        let op = frame.get("op").and_then(Value::as_u64);
        match op {
            Some(10) => {
                self.heartbeat_ms = frame.pointer("/d/heartbeat_interval").and_then(Value::as_u64);
                self.acked = true;
                if self.can_resume() {
                    vec![Action::Send(json!({ "op": 6, "d": {
                        "token": cfg.bot_token, "session_id": self.session_id, "seq": self.seq,
                    }}))]
                } else {
                    vec![Action::Send(self.identify(cfg))]
                }
            }
            Some(11) => {
                self.acked = true;
                vec![]
            }
            Some(1) => vec![Action::Send(json!({ "op": 1, "d": self.seq }))],
            Some(7) => vec![Action::Reconnect { resume: true }],
            Some(9) => {
                let resumable = frame.get("d").and_then(Value::as_bool).unwrap_or(false);
                if !resumable {
                    self.session_id = None;
                    self.seq = None;
                    self.resume_url = None;
                }
                vec![Action::Reconnect { resume: resumable }]
            }
            Some(0) => {
                if let Some(seq) = frame.get("s").and_then(Value::as_u64) {
                    self.seq = Some(seq);
                }
                let event = frame.get("t").and_then(Value::as_str).unwrap_or_default().to_string();
                let data = frame.get("d").cloned().unwrap_or(Value::Null);
                if event == "READY" {
                    self.session_id = str_at(&data, "/session_id").map(String::from);
                    self.resume_url = str_at(&data, "/resume_gateway_url").map(String::from);
                }
                vec![Action::Dispatch { event, data }]
            }
            _ => vec![],
        }
    }
}

/// Close codes that retrying cannot fix (bad token, bad intents, bad shard…).
fn fatal_close(code: u16) -> bool {
    matches!(code, 4004 | 4010 | 4011 | 4012 | 4013 | 4014)
}

/// Start the gateway client if the app is configured. One replica holds it (advisory lock).
pub fn start_discord_gateway(state: Arc<ApiState>) {
    let Some(cfg) = DiscordConfig::from_env() else {
        tracing::info!("Discord shared app not configured; gateway not started");
        return;
    };
    tokio::spawn(async move {
        let api: Arc<dyn DiscordApi> = default_api(&cfg);
        let mut backoff = 1u64;
        loop {
            // Session-level advisory lock: held for as long as this connection lives.
            let mut lock_conn = match state.db.acquire().await {
                Ok(conn) => conn,
                Err(error) => {
                    tracing::warn!("discord gateway: no db connection: {error}");
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    continue;
                }
            };
            let held: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
                .bind(GATEWAY_LOCK_KEY)
                .fetch_one(&mut *lock_conn)
                .await
                .unwrap_or(false);
            if !held {
                drop(lock_conn);
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            }
            tracing::info!("Discord gateway: starting");
            let mut gateway = GatewayState::default();
            loop {
                match run_gateway_session(&state, &cfg, api.clone(), &mut gateway).await {
                    Ok(SessionEnd::Fatal(code)) => {
                        tracing::error!(code, "Discord gateway closed with a fatal code; stopping");
                        return;
                    }
                    Ok(SessionEnd::Reconnect) => backoff = 1,
                    Err(error) => {
                        tracing::warn!("Discord gateway session ended: {error}");
                        backoff = (backoff * 2).min(60);
                    }
                }
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                // Our lock connection must still be alive, or another replica may have taken over.
                if sqlx::query("SELECT 1").execute(&mut *lock_conn).await.is_err() {
                    break;
                }
            }
        }
    });
}

enum SessionEnd {
    Reconnect,
    Fatal(u16),
}

async fn run_gateway_session(
    state: &Arc<ApiState>,
    cfg: &DiscordConfig,
    api: Arc<dyn DiscordApi>,
    gateway: &mut GatewayState,
) -> Result<SessionEnd, String> {
    use tokio_tungstenite::tungstenite::Message;
    let base = match gateway.resume_url.clone().filter(|_| gateway.can_resume()) {
        Some(url) => url,
        None => {
            let info = api
                .call(ApiRequest { method: "GET", path: "/gateway/bot".into(), auth: Auth::Bot, body: Body::None })
                .await?;
            if !info.ok() {
                return Err(format!("gateway/bot returned {}", info.status));
            }
            str_at(&info.body, "/url").unwrap_or("wss://gateway.discord.gg").to_string()
        }
    };
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("{}/?v=10&encoding=json", base.trim_end_matches('/')))
        .await
        .map_err(|e| e.to_string())?;
    let mut heartbeat: Option<tokio::time::Interval> = None;
    loop {
        let tick = async {
            match heartbeat.as_mut() {
                Some(interval) => {
                    interval.tick().await;
                }
                None => std::future::pending::<()>().await,
            }
        };
        let actions = tokio::select! {
            _ = tick => vec![gateway.heartbeat()],
            message = socket.next() => match message {
                Some(Ok(Message::Text(text))) => match serde_json::from_str::<Value>(&text) {
                    Ok(frame) => {
                        let actions = gateway.on_frame(&frame, cfg);
                        if frame.get("op").and_then(Value::as_u64) == Some(10) {
                            if let Some(ms) = gateway.heartbeat_ms {
                                let mut interval = tokio::time::interval(Duration::from_millis(ms.max(1000)));
                                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                                // The first tick is immediate; skip it so a full interval passes before the first beat.
                                interval.reset();
                                heartbeat = Some(interval);
                            }
                        }
                        actions
                    }
                    Err(_) => vec![],
                },
                Some(Ok(Message::Close(frame))) => {
                    let code = frame.map(|f| u16::from(f.code)).unwrap_or(1006);
                    return Ok(if fatal_close(code) { SessionEnd::Fatal(code) } else { SessionEnd::Reconnect });
                }
                Some(Ok(_)) => vec![],
                Some(Err(error)) => return Err(error.to_string()),
                None => return Ok(SessionEnd::Reconnect),
            },
        };
        for action in actions {
            match action {
                Action::Send(frame) => socket.send(Message::Text(frame.to_string())).await.map_err(|e| e.to_string())?,
                Action::Reconnect { .. } => {
                    let _ = socket.close(None).await;
                    return Ok(SessionEnd::Reconnect);
                }
                Action::Dispatch { event, data } => {
                    let (state, cfg, api) = (state.clone(), cfg.clone(), api.clone());
                    tokio::spawn(async move { handle_dispatch(&state, &cfg, api.as_ref(), &event, &data).await });
                }
            }
        }
    }
}

async fn handle_dispatch(state: &Arc<ApiState>, cfg: &DiscordConfig, api: &dyn DiscordApi, event: &str, data: &Value) {
    match event {
        "MESSAGE_CREATE" => match route_message_create(&state.db, api, cfg, data).await {
            Ok(Some(route_id)) => {
                if let Err(error) = deliver_route(state, &route_id).await {
                    tracing::warn!(%route_id, "discord delivery pass failed: {error}");
                }
            }
            Ok(None) => {}
            Err(error) => tracing::warn!("discord MESSAGE_CREATE not queued: {error}"),
        },
        // The app was removed from a server (not just an outage): stop routing for it.
        "GUILD_DELETE" if data.get("unavailable").and_then(Value::as_bool) != Some(true) => {
            if let Some(guild) = str_at(data, "/id") {
                let _ = sqlx::query("UPDATE discord_installs SET revoked_at = now() WHERE guild_id = $1 AND revoked_at IS NULL")
                    .bind(guild)
                    .execute(&state.db)
                    .await;
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::{seed_runtime_device, test_pool};
    use ed25519_dalek::{Signer, SigningKey};
    use std::collections::VecDeque;

    type Handler = Box<dyn Fn(&str, &str) -> ApiResponse + Send + Sync>;

    /// Fake discord.com: records (method, path, body) and answers from a closure.
    struct FakeApi {
        calls: Mutex<Vec<(String, String, Value)>>,
        handler: Handler,
    }

    impl FakeApi {
        fn new(handler: impl Fn(&str, &str) -> ApiResponse + Send + Sync + 'static) -> Arc<Self> {
            Arc::new(Self { calls: Mutex::new(vec![]), handler: Box::new(handler) })
        }
        fn calls(&self) -> Vec<(String, String, Value)> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl DiscordApi for FakeApi {
        async fn call(&self, request: ApiRequest) -> Result<ApiResponse, String> {
            let body = match &request.body {
                Body::Json(v) => v.clone(),
                Body::Form(fields) => json!(fields.iter().cloned().collect::<HashMap<_, _>>()),
                Body::None => Value::Null,
            };
            let response = (self.handler)(request.method, &request.path);
            self.calls.lock().unwrap().push((request.method.to_string(), request.path, body));
            Ok(response)
        }
    }

    fn reply(status: u16, body: Value) -> ApiResponse {
        ApiResponse { status, body, retry_after: None }
    }

    fn signing_key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn cfg() -> DiscordConfig {
        DiscordConfig {
            app_id: "app1".into(),
            public_key: hex::encode(signing_key().verifying_key().to_bytes()),
            bot_token: "bot-token".into(),
            client_secret: "client-secret".into(),
        }
    }

    fn sign(timestamp: &str, body: &[u8]) -> String {
        let mut message = timestamp.as_bytes().to_vec();
        message.extend_from_slice(body);
        hex::encode(signing_key().sign(&message).to_bytes())
    }

    /// Schema-per-test pool with the relay tables and migration 025.
    async fn db() -> sqlx::PgPool {
        let pool = test_pool().await;
        for sql in [
            include_str!("../../migrations_pg/020_channel_inbound_queue.sql"),
            include_str!("../../migrations_pg/025_discord_installs.sql"),
        ] {
            sqlx::raw_sql(&sql.replace("public.", "")).execute(&pool).await.unwrap();
        }
        pool
    }

    async fn install_for_test(pool: &sqlx::PgPool, guild: &str, discord_user: &str) -> String {
        seed_runtime_device(pool, &format!("rt-{guild}"), "user1").await;
        store_install(pool, "user1", &format!("rt-{guild}"), guild, Some("Acme"), Some(discord_user), None)
            .await
            .unwrap();
        let (route,): (String,) = sqlx::query_as("SELECT route_id FROM discord_installs WHERE guild_id = $1")
            .bind(guild)
            .fetch_one(pool)
            .await
            .unwrap();
        route
    }

    async fn queued(pool: &sqlx::PgPool, route: &str) -> Vec<Value> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT body FROM channel_inbound_queue WHERE route_id = $1 ORDER BY id")
            .bind(route)
            .fetch_all(pool)
            .await
            .unwrap();
        rows.into_iter()
            .map(|(b,)| {
                let bytes = base64::engine::general_purpose::STANDARD.decode(b).unwrap();
                serde_json::from_slice(&bytes).unwrap()
            })
            .collect()
    }

    // ---- install link

    #[test]
    fn permissions_integer_matches_the_nine_permissions() {
        // 64 + 1024 + 2048 + 16384 + 32768 + 65536 + 2^29 + 2^35 + 2^38
        assert_eq!(PERMISSIONS, 64 + 1024 + 2048 + 16384 + 32768 + 65536 + (1 << 29) + (1 << 35) + (1 << 38));
        assert_eq!(PERMISSIONS, 309_774_634_048, "the integer in the install link");
        assert_eq!(INTENTS, 1 + 512 + 4096);
        assert_eq!(INTENTS & (1 << 15), 0, "no MESSAGE_CONTENT intent");
    }

    #[test]
    fn install_link_has_scopes_permissions_and_a_verifiable_state() {
        let cfg = cfg();
        let now = 1_000_000;
        let state = sign_state(&cfg.client_secret, "user1", "rt1", now);
        let url = reqwest::Url::parse(&install_url(&cfg, &state)).unwrap();
        let q: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(url.host_str(), Some("discord.com"));
        assert_eq!(url.path(), "/oauth2/authorize");
        assert_eq!(q["client_id"], "app1");
        assert_eq!(q["scope"], "bot applications.commands identify");
        assert_eq!(q["integration_type"], "0");
        assert_eq!(q["permissions"], PERMISSIONS.to_string());
        assert!(q["redirect_uri"].ends_with("/channels/discord/oauth/callback"));
        assert_eq!(verify_state(&cfg.client_secret, &q["state"], now + 10), Some(("user1".into(), "rt1".into())));
    }

    #[test]
    fn state_rejects_tampering_expiry_and_other_secrets() {
        let state = sign_state("s", "user1", "rt1", 100);
        assert!(verify_state("s", &state, 100 + STATE_TTL_SECS + 1).is_none(), "expired");
        assert!(verify_state("other", &state, 100).is_none(), "wrong secret");
        let (payload, sig) = state.split_once('.').unwrap();
        let forged = URL_SAFE_NO_PAD.encode("attacker|rt1|9999999999|x");
        assert!(verify_state("s", &format!("{forged}.{sig}"), 100).is_none(), "payload swapped");
        assert!(verify_state("s", payload, 100).is_none(), "no signature");
    }

    #[tokio::test]
    async fn install_link_needs_a_runtime_the_user_owns() {
        let pool = db().await;
        seed_runtime_device(&pool, "rt1", "user1").await;
        assert!(install_inner(&pool, &cfg(), "user1", "rt1").await.is_ok());
        assert!(install_inner(&pool, &cfg(), "someone-else", "rt1").await.is_err());
    }

    // ---- OAuth callback

    #[tokio::test]
    async fn oauth_callback_stores_the_install_and_relay_route() {
        let pool = db().await;
        seed_runtime_device(&pool, "rt1", "user1").await;
        let api = FakeApi::new(|method, path| match (method, path) {
            ("POST", "/oauth2/token") => reply(200, json!({ "access_token": "at", "guild": { "id": "g1", "name": "Acme <Co>" } })),
            ("GET", "/users/@me") => reply(200, json!({ "id": "d1" })),
            _ => reply(404, Value::Null),
        });
        let cfg = cfg();
        let state = sign_state(&cfg.client_secret, "user1", "rt1", 500);
        let query = CallbackQuery { code: Some("code1".into()), state: Some(state), guild_id: None, error: None };
        let done = complete_install(&pool, api.as_ref(), &cfg, &query, 600).await.unwrap();
        assert_eq!(done, Installed { guild_id: "g1".into(), guild_name: Some("Acme <Co>".into()) });
        let (user, runtime, route, by): (String, String, Option<String>, Option<String>) =
            sqlx::query_as("SELECT user_id, runtime_id, route_id, installed_by_discord_id FROM discord_installs WHERE guild_id = 'g1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!((user.as_str(), runtime.as_str(), by.as_deref()), ("user1", "rt1", Some("d1")));
        let (provider,): (String,) = sqlx::query_as("SELECT provider FROM channel_inbound_routes WHERE id = $1")
            .bind(route.clone().unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(provider, PROVIDER);
        // The token exchange carried the secret and code; the page escapes the server name.
        let calls = api.calls();
        assert_eq!(calls[0].2["code"], "code1");
        assert_eq!(calls[0].2["client_secret"], "client-secret");
        assert!(connected_page(&done).contains("Acme &lt;Co&gt;"));
        // A DM from the installer reaches this runtime; a reinstall keeps the route.
        assert_eq!(owner_for(&pool, None, "d1").await.unwrap().map(|o| o.route_id), route.clone());
        complete_install(&pool, api.as_ref(), &cfg, &query, 600).await.unwrap();
        let (routes,): (i64,) = sqlx::query_as("SELECT count(*) FROM channel_inbound_routes").fetch_one(&pool).await.unwrap();
        assert_eq!(routes, 1);
    }

    #[tokio::test]
    async fn oauth_callback_refuses_bad_state_denial_and_foreign_runtime() {
        let pool = db().await;
        seed_runtime_device(&pool, "rt1", "owner").await;
        let api = FakeApi::new(|_, _| reply(200, json!({ "guild": { "id": "g9" } })));
        let cfg = cfg();
        let query = |state: Option<String>, error: Option<&str>| CallbackQuery {
            code: Some("c".into()),
            state,
            guild_id: None,
            error: error.map(String::from),
        };
        assert_eq!(complete_install(&pool, api.as_ref(), &cfg, &query(None, Some("access_denied")), 0).await, Err(InstallError::Denied));
        assert_eq!(complete_install(&pool, api.as_ref(), &cfg, &query(Some("junk".into()), None), 0).await, Err(InstallError::BadState));
        let stolen = sign_state(&cfg.client_secret, "intruder", "rt1", 0);
        assert_eq!(complete_install(&pool, api.as_ref(), &cfg, &query(Some(stolen), None), 1).await, Err(InstallError::RuntimeGone));
        let (installs,): (i64,) = sqlx::query_as("SELECT count(*) FROM discord_installs").fetch_one(&pool).await.unwrap();
        assert_eq!(installs, 0);
    }

    // ---- interactions

    #[tokio::test]
    async fn interactions_answer_ping_and_reject_bad_signatures() {
        let pool = db().await;
        let api = FakeApi::new(|_, _| reply(200, Value::Null));
        let cfg = cfg();
        let body = br#"{"type":1}"#;
        let (status, value, route) = interactions_inner(&pool, api.clone(), &cfg, &sign("1700", body), "1700", body).await;
        assert_eq!((status, value, route), (StatusCode::OK, json!({ "type": 1 }), None));
        // Wrong timestamp, tampered body, garbage and missing signatures all fail.
        let (status, _, _) = interactions_inner(&pool, api.clone(), &cfg, &sign("1700", body), "1701", body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _, _) = interactions_inner(&pool, api.clone(), &cfg, &sign("1700", body), "1700", br#"{"type":2}"#).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _, _) = interactions_inner(&pool, api.clone(), &cfg, "zz", "1700", body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _, _) = interactions_inner(&pool, api, &cfg, "", "", body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn slash_command_is_deferred_and_queued_to_the_owner_runtime() {
        let pool = db().await;
        let route = install_for_test(&pool, "g1", "d1").await;
        let api = FakeApi::new(|_, _| reply(200, Value::Null));
        let body = serde_json::to_vec(&json!({
            "type": 2, "id": "int1", "token": "tok", "guild_id": "g1", "channel_id": "55",
            "channel": { "id": "55", "type": 11, "parent_id": "44" },
            "member": { "user": { "id": "u7", "username": "sam" } },
            "data": { "name": "helper", "options": [{ "name": "message", "type": 3, "value": "hello there" }] },
        }))
        .unwrap();
        let (status, value, deliver) = interactions_inner(&pool, api.clone(), &cfg(), &sign("9", &body), "9", &body).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value, json!({ "type": 5, "data": { "flags": 64 } }));
        assert_eq!(deliver, Some(route.clone()));
        let rows = queued(&pool, &route).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["source"], ENVELOPE_SOURCE);
        assert_eq!(rows[0]["kind"], "command");
        assert_eq!(rows[0]["commandName"], "helper");
        assert_eq!(rows[0]["content"], "hello there");
        assert_eq!((rows[0]["channelId"].as_str(), rows[0]["threadId"].as_str()), (Some("44"), Some("55")));
        // An unknown server gets an ephemeral notice and nothing is queued.
        let other = body_for_guild("g404");
        let (_, value, deliver) = interactions_inner(&pool, api, &cfg(), &sign("9", &other), "9", &other).await;
        assert_eq!(value["type"], 4);
        assert_eq!(deliver, None);
    }

    fn body_for_guild(guild: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "type": 2, "id": "i2", "token": "t", "guild_id": guild, "channel_id": "1",
            "member": { "user": { "id": "u" } }, "data": { "name": "x" },
        }))
        .unwrap()
    }

    // ---- gateway

    fn message(content: &str, mention_app: bool) -> Value {
        json!({
            "id": "m1", "channel_id": "44", "guild_id": "g1", "content": content,
            "author": { "id": "u7", "username": "sam", "global_name": "Sam" },
            "mentions": if mention_app { json!([{ "id": "app1" }]) } else { json!([]) },
            "attachments": [{ "url": "https://cdn/x.png", "filename": "x.png", "content_type": "image/png" }],
        })
    }

    #[tokio::test]
    async fn gateway_message_with_a_mention_is_relayed_and_one_without_is_ignored() {
        let pool = db().await;
        let route = install_for_test(&pool, "g1", "d1").await;
        let api = FakeApi::new(|_, path| match path {
            "/channels/44" => reply(200, json!({ "id": "44", "type": 0 })),
            _ => reply(404, Value::Null),
        });
        let cfg = cfg();
        let got = route_message_create(&pool, api.as_ref(), &cfg, &message("<@app1> <@!app1> what's up", true)).await.unwrap();
        assert_eq!(got, Some(route.clone()));
        let none = route_message_create(&pool, api.as_ref(), &cfg, &message("just chatting", false)).await.unwrap();
        assert_eq!(none, None);
        let rows = queued(&pool, &route).await;
        assert_eq!(rows.len(), 1, "only the mention was queued");
        assert_eq!(rows[0]["content"], "what's up");
        assert_eq!(rows[0]["mentionsApp"], true);
        assert_eq!(rows[0]["authorName"], "Sam");
        assert_eq!(rows[0]["attachments"][0]["url"], "https://cdn/x.png");
        // A server with no install is ignored too.
        let mut elsewhere = message("<@app1> hi", true);
        elsewhere["guild_id"] = json!("g-unknown");
        assert_eq!(route_message_create(&pool, api.as_ref(), &cfg, &elsewhere).await.unwrap(), None);
    }

    #[tokio::test]
    async fn gateway_routes_replies_dms_and_thread_messages_and_skips_bots() {
        let pool = db().await;
        let route = install_for_test(&pool, "g2", "d2").await;
        let api = FakeApi::new(|_, path| match path {
            "/channels/66" => reply(200, json!({ "id": "66", "type": 11, "parent_id": "65" })),
            _ => reply(404, Value::Null),
        });
        let cfg = cfg();
        // Reply to a webhook message the app posted (mention ping turned off).
        let mut reply_msg = message("thanks", false);
        reply_msg["guild_id"] = json!("g2");
        reply_msg["referenced_message"] = json!({ "id": "w9", "application_id": "app1", "author": { "id": "wh1", "bot": true } });
        reply_msg["message_reference"] = json!({ "message_id": "w9" });
        assert!(route_message_create(&pool, api.as_ref(), &cfg, &reply_msg).await.unwrap().is_some());
        // A DM from the installer, no guild.
        let dm = json!({ "id": "m3", "channel_id": "dm1", "content": "psst", "author": { "id": "d2", "username": "owner" } });
        assert!(route_message_create(&pool, api.as_ref(), &cfg, &dm).await.unwrap().is_some());
        // A mention inside a thread: the webhook channel is the parent.
        let mut in_thread = message("<@app1> in thread", true);
        in_thread["guild_id"] = json!("g2");
        in_thread["channel_id"] = json!("66");
        assert!(route_message_create(&pool, api.as_ref(), &cfg, &in_thread).await.unwrap().is_some());
        // The app's own messages and other bots never loop back.
        let mut bot = message("<@app1> beep", true);
        bot["guild_id"] = json!("g2");
        bot["author"]["bot"] = json!(true);
        assert_eq!(route_message_create(&pool, api.as_ref(), &cfg, &bot).await.unwrap(), None);
        let mut hook = message("<@app1> from webhook", true);
        hook["guild_id"] = json!("g2");
        hook["webhook_id"] = json!("wh1");
        assert_eq!(route_message_create(&pool, api.as_ref(), &cfg, &hook).await.unwrap(), None);
        let rows = queued(&pool, &route).await;
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["replyToMessageId"], "w9");
        assert_eq!((rows[1]["isDm"].as_bool(), rows[1]["guildId"].as_str()), (Some(true), None));
        assert_eq!((rows[2]["channelId"].as_str(), rows[2]["threadId"].as_str()), (Some("65"), Some("66")));
    }

    #[test]
    fn gateway_frames_identify_without_message_content_then_resume() {
        let cfg = cfg();
        let mut gw = GatewayState::default();
        let hello = json!({ "op": 10, "d": { "heartbeat_interval": 41250 } });
        let actions = gw.on_frame(&hello, &cfg);
        let Action::Send(identify) = &actions[0] else { panic!("expected identify") };
        assert_eq!(identify["op"], 2);
        assert_eq!(identify["d"]["token"], "bot-token");
        assert_eq!(identify["d"]["intents"], INTENTS);
        assert_eq!(identify["d"]["shard"], json!([0, 1]));
        assert_eq!(gw.heartbeat_ms, Some(41250));
        // READY records the session; later dispatches advance the sequence.
        let ready = json!({ "op": 0, "t": "READY", "s": 1, "d": { "session_id": "sess", "resume_gateway_url": "wss://resume.example" } });
        assert!(matches!(&gw.on_frame(&ready, &cfg)[0], Action::Dispatch { event, .. } if event == "READY"));
        gw.on_frame(&json!({ "op": 0, "t": "MESSAGE_CREATE", "s": 2, "d": {} }), &cfg);
        // Heartbeats carry the sequence; two beats without an ACK mean the link is dead.
        assert_eq!(gw.heartbeat(), Action::Send(json!({ "op": 1, "d": 2 })));
        assert_eq!(gw.heartbeat(), Action::Reconnect { resume: true });
        gw.on_frame(&json!({ "op": 11 }), &cfg);
        assert!(matches!(gw.heartbeat(), Action::Send(_)));
        // A new socket's Hello now resumes rather than identifies.
        let Action::Send(resume) = &gw.on_frame(&hello, &cfg)[0] else { panic!("expected resume") };
        assert_eq!((resume["op"].as_u64(), resume["d"]["session_id"].as_str(), resume["d"]["seq"].as_u64()), (Some(6), Some("sess"), Some(2)));
        // Reconnect asks to resume; a non-resumable invalid session starts over.
        assert_eq!(gw.on_frame(&json!({ "op": 7 }), &cfg), vec![Action::Reconnect { resume: true }]);
        assert_eq!(gw.on_frame(&json!({ "op": 9, "d": false }), &cfg), vec![Action::Reconnect { resume: false }]);
        let Action::Send(again) = &gw.on_frame(&hello, &cfg)[0] else { panic!() };
        assert_eq!(again["op"], 2, "session was cleared, so identify");
        assert!(fatal_close(4014) && fatal_close(4004) && !fatal_close(4000));
    }

    // ---- commands

    #[tokio::test]
    async fn slash_commands_are_registered_per_guild() {
        let api = FakeApi::new(|_, _| reply(200, json!([])));
        let names = vec!["Helper Bot".to_string(), "helper-bot".to_string(), "Discord".to_string(), "Réseau".to_string()];
        let response = register_commands(api.as_ref(), &cfg(), "g1", &names).await;
        assert_eq!(response.status(), StatusCode::OK);
        let calls = api.calls();
        assert_eq!((calls[0].0.as_str(), calls[0].1.as_str()), ("PUT", "/applications/app1/guilds/g1/commands"));
        let sent = calls[0].2.as_array().unwrap();
        let sent_names: Vec<_> = sent.iter().map(|c| c["name"].as_str().unwrap()).collect();
        assert_eq!(sent_names, ["helper-bot", "r-seau"], "deduped, ascii, and never 'discord'");
        assert_eq!(sent[0]["options"][0]["name"], "message");
        assert_eq!(command_names(&[]), Vec::<String>::new());
    }

    // ---- send

    fn webhook_api() -> Arc<FakeApi> {
        FakeApi::new(|method, path| match (method, path) {
            ("GET", p) if p.starts_with("/channels/") => reply(200, json!({ "id": "c", "guild_id": "g1" })),
            ("POST", p) if p.ends_with("/webhooks") => reply(200, json!({ "id": "wh1", "token": "tok1" })),
            ("GET", "/webhooks/wh1") => reply(200, json!({ "id": "wh1", "token": "tok1" })),
            ("POST", p) if p.starts_with("/webhooks/wh1/tok1") => reply(200, json!({ "id": "msg-42" })),
            _ => reply(404, Value::Null),
        })
    }

    fn send_body(channel: &str, text: &str) -> SendBody {
        SendBody {
            guild_id: "g1".into(),
            channel_id: channel.into(),
            thread_id: None,
            bot_name: "Discord Helper".into(),
            avatar_url: Some("https://cdn.allternit.com/a.png".into()),
            text: text.into(),
        }
    }

    #[tokio::test]
    async fn send_posts_through_a_webhook_with_the_bots_name_and_avatar() {
        let pool = db().await;
        let api = webhook_api();
        let mut body = send_body("chan-send-1", "hello <@everyone>");
        body.thread_id = Some("th1".into());
        let ids = send_message(&pool, api.as_ref(), &body).await.unwrap();
        assert_eq!(ids, vec!["msg-42".to_string()]);
        let calls = api.calls();
        let post = calls.last().unwrap();
        assert_eq!(post.1, "/webhooks/wh1/tok1?wait=true&thread_id=th1");
        assert_eq!(post.2["username"], "Helper", "'discord' is not allowed in a webhook name");
        assert_eq!(post.2["avatar_url"], "https://cdn.allternit.com/a.png");
        assert_eq!(post.2["content"], "hello <@everyone>");
        assert_eq!(post.2["allowed_mentions"], json!({ "parse": [] }));
        // Created once, then reused.
        let created = |api: &FakeApi| api.calls().iter().filter(|c| c.1.ends_with("/webhooks")).count();
        assert_eq!(created(&api), 1);
        send_message(&pool, api.as_ref(), &send_body("chan-send-1", "again")).await.unwrap();
        assert_eq!(created(&api), 1);
        let (stored,): (String,) = sqlx::query_as("SELECT webhook_id FROM discord_webhooks WHERE channel_id = 'chan-send-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored, "wh1");
    }

    fn dm_api(member: bool, closed: bool) -> Arc<FakeApi> {
        FakeApi::new(move |method, path| match (method, path) {
            ("GET", p) if p.starts_with("/guilds/") && p.contains("/members/") => if member { reply(200, json!({ "user": { "id": "x" } })) } else { reply(404, json!({ "code": 10007 })) },
            ("POST", "/users/@me/channels") => reply(200, json!({ "id": "dm-1" })),
            ("POST", "/channels/dm-1/messages") => if closed { reply(403, json!({ "code": 50007 })) } else { reply(200, json!({ "id": "dmsg-1" })) },
            _ => reply(404, Value::Null),
        })
    }

    fn dm_body(text: Option<&str>) -> DmBody {
        DmBody { guild_id: None, user_id: "123456789012345678".into(), text: text.map(String::from), bot_name: Some("Finance".into()) }
    }

    #[tokio::test]
    async fn dm_opens_a_channel_and_posts_as_the_bot() {
        let pool = db().await;
        install_for_test(&pool, "g-dm-1", "installer").await;
        let api = dm_api(true, false);
        let (channel, message) = dm_message(&pool, api.as_ref(), "user1", &dm_body(Some("hello"))).await.unwrap();
        assert_eq!((channel.as_str(), message.as_deref()), ("dm-1", Some("dmsg-1")));
        let calls = api.calls();
        assert_eq!(calls[0].1, "/guilds/g-dm-1/members/123456789012345678", "membership is checked first");
        assert_eq!(calls[1].2, json!({ "recipient_id": "123456789012345678" }));
        assert_eq!(calls[2].2["content"], "**Finance**\nhello");
        assert_eq!(calls[2].2["allowed_mentions"], json!({ "parse": [] }));
        // Without text only the channel is opened.
        let api = dm_api(true, false);
        assert_eq!(dm_message(&pool, api.as_ref(), "user1", &dm_body(None)).await.unwrap(), ("dm-1".to_string(), None));
        assert_eq!(api.calls().len(), 2);
    }

    #[tokio::test]
    async fn dm_refuses_strangers_closed_dms_and_other_peoples_servers() {
        let pool = db().await;
        install_for_test(&pool, "g-dm-2", "installer").await;
        let err = |r: Result<(String, Option<String>), SendError>| r.unwrap_err();
        // The person isn't in any of the caller's servers: nothing is opened.
        let api = dm_api(false, false);
        assert!(matches!(err(dm_message(&pool, api.as_ref(), "user1", &dm_body(Some("hi"))).await), SendError::NotInServer));
        assert!(api.calls().iter().all(|c| c.1 != "/users/@me/channels"));
        // Privacy settings refuse the DM.
        assert!(matches!(err(dm_message(&pool, dm_api(true, true).as_ref(), "user1", &dm_body(Some("hi"))).await), SendError::DmClosed));
        // A user with no install, or a guildId that isn't theirs, never reaches Discord for a DM.
        assert!(matches!(err(dm_message(&pool, dm_api(true, false).as_ref(), "nobody", &dm_body(Some("hi"))).await), SendError::NotInstalled));
        let mut other = dm_body(Some("hi"));
        other.guild_id = Some("someone-elses".into());
        let api = dm_api(true, false);
        dm_message(&pool, api.as_ref(), "user1", &other).await.unwrap();
        assert_eq!(api.calls()[0].1, "/guilds/g-dm-2/members/123456789012345678", "a guild that isn't the caller's is ignored, not trusted");
        let mut bad = dm_body(Some("hi"));
        bad.user_id = "not-a-snowflake".into();
        assert!(matches!(err(dm_message(&pool, dm_api(true, false).as_ref(), "user1", &bad).await), SendError::Invalid("invalid_user_id")));
    }

    #[tokio::test]
    async fn send_never_posts_into_another_servers_channel() {
        let pool = db().await;
        let api = FakeApi::new(|_, _| reply(200, json!({ "guild_id": "someone-elses-guild" })));
        let err = send_message(&pool, api.as_ref(), &send_body("chan-send-2", "hi")).await.unwrap_err();
        assert!(matches!(err, SendError::WrongGuild));
        assert!(api.calls().iter().all(|c| !c.1.ends_with("/webhooks")), "no webhook was made");
        // A stored webhook for the channel under a different guild is refused as well.
        sqlx::query("INSERT INTO discord_webhooks (channel_id, guild_id, webhook_id) VALUES ('chan-send-3', 'other', 'w')")
            .execute(&pool)
            .await
            .unwrap();
        let err = send_message(&pool, webhook_api().as_ref(), &send_body("chan-send-3", "hi")).await.unwrap_err();
        assert!(matches!(err, SendError::WrongGuild));
    }

    #[tokio::test]
    async fn send_replaces_a_deleted_webhook_once() {
        let pool = db().await;
        let attempts = Arc::new(Mutex::new(VecDeque::from([false, true])));
        let flag = attempts.clone();
        let api = FakeApi::new(move |method, path| match (method, path) {
            ("GET", p) if p.starts_with("/channels/") => reply(200, json!({ "guild_id": "g1" })),
            ("POST", p) if p.ends_with("/webhooks") => reply(200, json!({ "id": "wh2", "token": "tok2" })),
            ("POST", p) if p.starts_with("/webhooks/") => {
                if flag.lock().unwrap().pop_front() == Some(true) {
                    reply(200, json!({ "id": "msg-7" }))
                } else {
                    reply(404, json!({ "code": 10015 }))
                }
            }
            _ => reply(404, Value::Null),
        });
        let ids = send_message(&pool, api.as_ref(), &send_body("chan-send-4", "hi")).await.unwrap();
        assert_eq!(ids, vec!["msg-7".to_string()]);
        assert_eq!(api.calls().iter().filter(|c| c.1.ends_with("/webhooks")).count(), 2);
    }

    #[test]
    fn names_and_text_are_made_safe_for_discord() {
        assert_eq!(webhook_username("Clyde & the DISCORD bot"), "& the bot");
        assert_eq!(webhook_username("discord"), "Allternit");
        assert_eq!(webhook_username("@everyone #1: `x`"), "everyone 1 x");
        assert_eq!(webhook_username(&"a".repeat(200)).chars().count(), 80);
        let long = format!("{}\n{}", "a".repeat(1500), "b".repeat(1500));
        let chunks = chunk_text(&long);
        assert_eq!(chunks.len(), 2);
        assert!(chunks.iter().all(|c| c.chars().count() <= 2000));
        assert_eq!(chunk_text("short"), vec!["short".to_string()]);
        assert!(chunk_text("   ").is_empty());
        assert_eq!(chunk_text(&"é".repeat(4500)).len(), 3, "counts characters, not bytes");
    }
}
