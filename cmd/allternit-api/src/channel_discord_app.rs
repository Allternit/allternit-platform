//! Discord through Allternit's one shared app (runtime side).
//!
//! The app's bot token lives only in cloud-api. This runtime never talks to
//! Discord for a shared-app connection: [`DiscordAppTransport::post`] asks the
//! cloud to send, `POST /api/v1/channels/discord/send`, authenticated with the
//! runtime-device token. The cloud posts through an app-owned webhook so each
//! bot speaks under its own name and avatar. The user-token Discord path in
//! `channel_transports` / `discord_gateway` stays as "Advanced".
//!
//! A connection is in app mode when its sealed secret has `"mode":"app"`
//! (plus optional `guildId`, `cloudToken`, `publicKey`, `relaySecret`).
//!
//! Mentions reach a bot three ways, all turned into the "@name" form that
//! `channel_transports::route_inbound` already understands, by [`rewrite`]
//! and [`DiscordAppTransport::normalize`]:
//!   * "@Allternit <name> ..." (Discord sends it as `<@appid> <name> ...`)
//!   * the slash command `/<name> [message]` (an interaction, type 2)
//!   * a reply to a message a bot posted (`message_reference`, mapped by
//!     message id through `discord_app_messages`)
//!
//! API: https://discord.com/developers/docs/resources/webhook#execute-webhook
//! (username, avatar_url, thread_id), https://discord.com/developers/docs/interactions/application-commands

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use axum::http::HeaderMap;
use hmac::{Hmac, Mac};
use rusqlite::params;
use serde_json::{json, Value};
use sha2::Sha256;

use crate::channel_gateway::*;
use crate::channel_transports::{discord_normalize, mentioned_bot, pick, Account, HttpReq, HttpSend, MemberBot};
use crate::db::DbHandle;

static DB: OnceLock<DbHandle> = OnceLock::new();

/// Where cloud-api delivers shared-app events (`target_path("discord_app")`).
pub const DISCORD_APP_EVENTS_PATH: &str = "/webhooks/channels/discord-app";

/// The delivery address for cloud-built Discord envelopes. Cloud-api already
/// checked Discord's signature; it signs the relay to this runtime with the
/// device token and [`RelayedAuth`] verifies that, so only the relay can reach
/// it (a raw Discord payload never can).
pub fn discord_app_router() -> axum::Router<Arc<crate::AppState>> {
    discord_app_router_with(crate::relay_auth::process_secret())
}

pub fn discord_app_router_with(secret: Arc<dyn crate::relay_auth::RelaySecret>) -> axum::Router<Arc<crate::AppState>> {
    axum::Router::new().route(DISCORD_APP_EVENTS_PATH, axum::routing::post(discord_app_webhook)).layer(crate::relay_auth::secret_layer(secret))
}

async fn discord_app_webhook(axum::extract::State(state): axum::extract::State<Arc<crate::AppState>>, auth: crate::relay_auth::RelayedAuth) -> axum::response::Response {
    use axum::{http::StatusCode, response::IntoResponse, Json};
    let Ok(envelope) = serde_json::from_slice::<Value>(&auth.body) else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_json" }))).into_response();
    };
    if envelope["source"].as_str() != Some("allternit-discord-app") {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "not a discord app envelope" }))).into_response();
    }
    init(state.db.clone());
    let guild = envelope["guildId"].as_str().unwrap_or_default();
    // The owner's app-mode connection, preferring the one for this server.
    let candidates: Vec<Account> = crate::channel_transports::accounts(&state.db, "discord", None)
        .into_iter()
        .filter(|a| a.owner == auth.owner && is_app_secret(&a.secret))
        .collect();
    let Some(acct) = candidates.iter().find(|a| !guild.is_empty() && pick(&a.secret, "guildId") == guild).or_else(|| candidates.first()).cloned() else {
        return Json(json!({ "ok": true, "ignored": true })).into_response();
    };
    let events = envelope_events(&state.db, &envelope);
    if events.is_empty() {
        return Json(json!({ "ok": true, "ignored": true })).into_response();
    }
    let Some(tx) = crate::channel_transports::build_transport("discord", &acct.secret, Arc::new(crate::channel_transports::ReqwestSend)) else {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "discord_not_configured" }))).into_response();
    };
    // Ack fast: bot turns can outlive the relay's wait. The queue retries.
    tokio::spawn(async move { crate::channel_transports::dispatch_events(&state, &acct, tx, events).await });
    Json(json!({ "ok": true })).into_response()
}

/// One cloud envelope as the "@name text" message `route_inbound` understands.
fn envelope_events(db: &DbHandle, env: &Value) -> Vec<Inbound> {
    let s = |k: &str| env[k].as_str().filter(|v| !v.is_empty()).map(str::to_string);
    let Some(channel) = s("channelId") else { return vec![] };
    let Some(user) = s("authorId") else { return vec![] };
    let mut text = env["content"].as_str().unwrap_or_default().trim().to_string();
    let id = if env["kind"].as_str() == Some("command") { s("interactionId") } else { s("messageId") }.unwrap_or_default();
    if env["kind"].as_str() == Some("command") {
        let Some(name) = s("commandName") else { return vec![] };
        let rest = if text.is_empty() { "hi".to_string() } else { text };
        text = format!("@{} {rest}", name.replace(' ', ""));
    } else if let Some(bot) = s("replyToMessageId").and_then(|m| bot_of_message(db, &m)) {
        text = format!("@{} {text}", clean(&bot));
    }
    if text.is_empty() {
        return vec![];
    }
    vec![Inbound {
        kind: InboundKind::Message,
        workspace: s("guildId"),
        conversation: format!("discord:{channel}"),
        channel,
        thread: s("threadId"),
        remote_id: id.clone(),
        message_id: id.clone(),
        text: Some(text),
        user: Some(user),
        reaction: None,
        added: None,
        cursor: Some(id),
        own: false,
    }]
}

/// Register the runtime database (called once at boot; [`rewrite`] also does it).
pub fn init(db: DbHandle) {
    let _ = DB.set(db);
}

/// True when the connection's secret selects the shared app.
pub fn is_app_secret(secret: &str) -> bool {
    pick(secret, "mode") == "app"
}

fn clean(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Discord rejects webhook names containing "discord" or "clyde".
pub(crate) fn safe_name(name: &str) -> String {
    let mut out = name.to_string();
    for banned in ["discord", "clyde"] {
        while let Some(i) = out.to_lowercase().find(banned) {
            out.replace_range(i..i + banned.len(), "");
        }
    }
    let out: String = out.split_whitespace().collect::<Vec<_>>().join(" ");
    let out: String = out.chars().take(80).collect();
    if out.is_empty() { "Allternit".into() } else { out }
}

pub struct DiscordAppTransport {
    pub http: Arc<dyn HttpSend>,
    pub db: Option<DbHandle>,
    pub cloud_base: Option<String>,
    pub cloud_token: Option<String>,
    pub guild_id: Option<String>,
    pub public_key: Option<String>,
    pub relay_secret: Option<String>,
}

impl DiscordAppTransport {
    pub fn from_secret(secret: &str, http: Arc<dyn HttpSend>) -> Self {
        let some = |s: String| Some(s).filter(|s| !s.is_empty());
        let env = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
        Self {
            http,
            db: DB.get().cloned(),
            // A paired runtime knows its cloud and its own device credential; the env
            // vars stay as overrides (tests, headless hosts).
            cloud_base: env("ALLTERNIT_CLOUD_API_URL").or_else(|| some(crate::phone_sync::cloud_base())),
            cloud_token: some(pick(secret, "cloudToken")).or_else(|| env("ALLTERNIT_RUNTIME_DEVICE_TOKEN")).or_else(crate::phone_sync::runtime_bearer),
            guild_id: some(pick(secret, "guildId")),
            public_key: some(pick(secret, "publicKey")).or_else(|| env("ALLTERNIT_DISCORD_APP_PUBLIC_KEY")),
            relay_secret: some(pick(secret, "relaySecret")),
        }
    }

    /// Bot name (and avatar) for an outbound post. `identity` names a bot
    /// explicitly; otherwise a leading "Name: " (added when several bots share
    /// the connection) or the conversation's own bot.
    fn speaker(&self, out: &Outbound) -> (String, Option<String>, String) {
        let Some(db) = self.db.as_ref().or(DB.get()) else {
            return (safe_name(out.identity.as_deref().unwrap_or("Allternit")), None, out.text.clone());
        };
        let Ok(conn) = db.connect() else { return (safe_name(out.identity.as_deref().unwrap_or("Allternit")), None, out.text.clone()) };
        let binding = find_binding(db, "discord", &format!("discord:{}", out.channel));
        let owner = binding.as_ref().map(|b| b.owner.clone());
        let bots: Vec<(String, Option<String>)> = owner
            .as_ref()
            .and_then(|o| {
                conn.prepare("SELECT COALESCE(NULLIF(name, ''), id), avatar FROM agents WHERE user_id = ?1")
                    .and_then(|mut q| q.query_map(params![o], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?.collect())
                    .ok()
            })
            .unwrap_or_default();
        let (mut name, mut text) = (out.identity.clone(), out.text.clone());
        if name.is_none() {
            if let Some((head, rest)) = out.text.split_once(": ") {
                if bots.iter().any(|(n, _)| clean(n) == clean(head)) {
                    name = Some(head.to_string());
                    text = rest.to_string();
                }
            }
        }
        if name.is_none() {
            name = binding.as_ref().and_then(|b| {
                conn.query_row("SELECT COALESCE(NULLIF(a.name, ''), a.id) FROM bot_threads t JOIN agents a ON a.id = t.bot_id WHERE t.id = ?1", params![b.thread_id], |r| r.get::<_, String>(0)).ok()
            });
        }
        let name = name.unwrap_or_else(|| "Allternit".into());
        let avatar = bots.iter().find(|(n, _)| clean(n) == clean(&name)).and_then(|(_, a)| a.clone()).filter(|a| a.starts_with("https://"));
        (safe_name(&name), avatar, text)
    }

    fn remember(&self, message_id: &str, channel: &str, bot: &str) {
        let Some(db) = self.db.as_ref().or(DB.get()) else { return };
        let Ok(conn) = db.connect() else { return };
        let _ = conn.execute("INSERT OR REPLACE INTO discord_app_messages (message_id, channel_id, bot_name) VALUES (?1, ?2, ?3)", params![message_id, channel, bot]);
        let _ = conn.execute("DELETE FROM discord_app_messages WHERE created_at < datetime('now', '-30 days')", []);
    }
}

/// Bot a replied-to message belongs to.
fn bot_of_message(db: &DbHandle, message_id: &str) -> Option<String> {
    db.connect().ok()?.query_row("SELECT bot_name FROM discord_app_messages WHERE message_id = ?1", params![message_id], |r| r.get(0)).ok()
}

/// Slash command `/<name> [text]` (an interaction, type 2) as a message "@name text".
fn interaction_events(p: &Value) -> Vec<Inbound> {
    if p["type"].as_i64() != Some(2) {
        return vec![];
    }
    let (Some(name), Some(channel)) = (p.pointer("/data/name").and_then(Value::as_str), p["channel_id"].as_str()) else { return vec![] };
    let rest = p
        .pointer("/data/options")
        .and_then(Value::as_array)
        .and_then(|o| o.iter().find_map(|x| x["value"].as_str()))
        .unwrap_or("");
    let id = p["id"].as_str().unwrap_or_default().to_string();
    let mut e = Inbound {
        kind: InboundKind::Message,
        workspace: p["guild_id"].as_str().map(str::to_string),
        channel: channel.to_string(),
        conversation: format!("discord:{channel}"),
        thread: None,
        remote_id: id.clone(),
        message_id: id.clone(),
        text: Some(format!("@{} {rest}", name.replace(' ', "")).trim().to_string()),
        user: p.pointer("/member/user/id").or_else(|| p.pointer("/user/id")).and_then(Value::as_str).map(str::to_string),
        reaction: None,
        added: None,
        cursor: Some(id),
        own: false,
    };
    // "/name" with nothing else still needs text to start a turn.
    if rest.is_empty() {
        e.text = Some(format!("@{} hi", name.replace(' ', "")));
    }
    vec![e]
}

/// Rewrite a Discord message so its target bot is an "@name" at the front.
/// No-op for connections that are not in app mode.
pub fn rewrite(db: &DbHandle, acct: &Account, bots: &[MemberBot], e: &Inbound) -> Inbound {
    init(db.clone());
    let mut e = e.clone();
    if !is_app_secret(&acct.secret) || e.kind != InboundKind::Message || e.own {
        return e;
    }
    let text = e.text.clone().unwrap_or_default();
    if mentioned_bot(&text, bots).is_some() {
        return e;
    }
    // "<@appid> name rest" or "@Allternit name rest": drop the app mention, find a bot name at the front.
    let stripped = {
        let t = text.trim_start();
        if t.starts_with("<@") {
            t.split_once('>').map(|(_, r)| r.trim_start().to_string())
        } else {
            t.strip_prefix("@Allternit").or_else(|| t.strip_prefix("@allternit")).map(|r| r.trim_start().to_string())
        }
    };
    if let Some(rest) = stripped {
        let words: Vec<&str> = rest.split_whitespace().collect();
        for n in (1..=words.len().min(4)).rev() {
            let cand = clean(&words[..n].join(" "));
            if let Some(b) = bots.iter().find(|b| clean(&b.name) == cand || clean(&b.id) == cand) {
                e.text = Some(format!("@{} {}", clean(&b.name), words[n..].join(" ")).trim().to_string());
                return e;
            }
        }
        e.text = Some(rest);
        return e;
    }
    e
}

#[async_trait]
impl ChannelTransport for DiscordAppTransport {
    fn provider(&self) -> &'static str {
        "discord"
    }

    /// A relayed delivery is accepted when it carries either Discord's own
    /// Ed25519 signature (interactions) checked against the app's public key,
    /// or an HMAC-SHA256 of the body (`x-allternit-relay-signature`, hex)
    /// under the connection's `relaySecret` (gateway events).
    fn verify(&self, _secret: &str, headers: &HeaderMap, body: &[u8]) -> Result<(), String> {
        let h = |k: &str| headers.get(k).and_then(|v| v.to_str().ok());
        if let (Some(sig), Some(secret)) = (h("x-allternit-relay-signature"), self.relay_secret.as_deref()) {
            let given = hex::decode(sig.trim()).map_err(|_| "bad relay signature encoding")?;
            let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|_| "bad relay secret")?;
            mac.update(body);
            return mac.verify_slice(&given).map_err(|_| "relay signature mismatch".to_string());
        }
        let key = self.public_key.clone().ok_or("no Discord app public key configured")?;
        crate::channel_transports::DiscordTransport { http: self.http.clone(), webhook_url: None, own_identity: None }.verify(&json!({ "publicKey": key }).to_string(), headers, body)
    }

    fn normalize(&self, payload: &Value) -> Vec<Inbound> {
        let mut events = interaction_events(payload);
        if events.is_empty() {
            events = discord_normalize(payload);
            // A reply to a message one of our bots posted goes to that bot.
            if let (Some(db), Some(reply_to)) = (self.db.as_ref().or(DB.get()), payload.pointer("/d/message_reference/message_id").and_then(Value::as_str)) {
                if let Some(bot) = bot_of_message(db, reply_to) {
                    for e in events.iter_mut().filter(|e| e.kind == InboundKind::Message) {
                        e.text = e.text.as_ref().map(|t| format!("@{} {t}", clean(&bot)));
                    }
                }
            }
        }
        events
    }

    fn identity(&self, requested: Option<&str>) -> Identity {
        // Every bot speaks under its own name through the app's webhook.
        Identity { id: requested.map(str::to_string), exact: requested.is_none() }
    }

    async fn post(&self, out: &Outbound) -> Result<Receipt, PostError> {
        let (Some(base), Some(token)) = (self.cloud_base.as_deref(), self.cloud_token.as_deref()) else {
            return Err(PostError::Rejected("discord_not_configured".into()));
        };
        let (bot, avatar, text) = self.speaker(out);
        // A direct message can't go through a channel webhook: the cloud's bot opens the DM and posts.
        if let Some(user) = crate::channel_discord_dm::dm_user(out.workspace.as_deref()) {
            let body = json!({ "userId": user, "text": text, "botName": bot });
            let url = format!("{}/api/v1/channels/discord/dm", base.trim_end_matches('/'));
            let resp = self.http.post_json(HttpReq { url, headers: vec![("Authorization".into(), format!("Bearer {token}"))], body }).await.map_err(PostError::Uncertain)?;
            return match crate::channel_discord_dm::judge(resp.status, &resp.body) {
                Ok(v) => {
                    let id = v["messageId"].as_str().map(str::to_string).ok_or_else(|| PostError::Uncertain("the cloud accepted the direct message but returned no messageId".into()))?;
                    self.remember(&id, &out.channel, &bot);
                    Ok(Receipt { remote_id: id, relayed: true })
                }
                Err(crate::channel_discord_dm::DmError::Uncertain(why)) => Err(PostError::Uncertain(why)),
                Err(e) => Err(PostError::Rejected(crate::channel_discord_dm::sentence(&e).0.into())),
            };
        }
        let guild = out.workspace.clone().or_else(|| self.guild_id.clone()).ok_or_else(|| PostError::Rejected("no Discord server for this conversation".into()))?;
        let mut body = json!({ "guildId": guild, "channelId": out.channel, "botName": bot, "text": text });
        if let Some(t) = out.thread.as_ref().filter(|t| !t.is_empty()) {
            body["threadId"] = json!(t);
        }
        if let Some(a) = avatar {
            body["avatarUrl"] = json!(a);
        }
        let url = format!("{}/api/v1/channels/discord/send", base.trim_end_matches('/'));
        let resp = self.http.post_json(HttpReq { url, headers: vec![("Authorization".into(), format!("Bearer {token}"))], body }).await.map_err(PostError::Uncertain)?;
        match resp.status {
            200..=299 => {
                let id = resp.body["messageId"].as_str().map(str::to_string).ok_or_else(|| PostError::Uncertain("the cloud accepted the post but returned no messageId".into()))?;
                self.remember(&id, &out.channel, &bot);
                Ok(Receipt { remote_id: id, relayed: true })
            }
            503 => Err(PostError::Rejected(resp.body["error"].as_str().unwrap_or("discord_not_configured").to_string())),
            429 => Err(PostError::Rejected("rate limited".into())),
            500..=599 => Err(PostError::Uncertain(format!("cloud returned {}", resp.status))),
            s => Err(PostError::Rejected(format!("cloud returned {s}: {}", resp.body))),
        }
    }
}

// ---------------------------------------------------------------- connect

/// `POST /api/v1/gateway/channel-accounts/discord {guildId?}`: after "Add to
/// Discord" lands on the cloud, record the server on this runtime as an
/// app-mode connection (idempotent). 404 until the install is there.
pub fn discord_app_connect_router() -> axum::Router<Arc<crate::AppState>> {
    axum::Router::new().route("/gateway/channel-accounts/discord", axum::routing::post(discord_connect_h))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiscordConnectBody {
    guild_id: Option<String>,
}

/// Record (or refresh) the local app-mode connection for an installed guild.
pub fn upsert_app_connection(db: &DbHandle, owner: &str, guild_id: &str, guild_name: &str) -> Result<Value, (axum::http::StatusCode, String)> {
    use axum::http::StatusCode;
    let keys = json!({ "mode": "app", "guildId": guild_id }).to_string();
    let Some(sealed) = crate::agent_gateway_routes::seal_strict(&keys) else {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "no encryption key is configured; the connection was not stored".into()));
    };
    let conn = db.connect().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM provider_account_bindings WHERE owner = ?1 AND vendor = 'discord' AND auth_type = 'channel_oauth' AND external_account_id = ?2",
            params![owner, guild_id],
            |r| r.get(0),
        )
        .ok();
    let t = crate::agent_gateway_routes::now();
    let name = if guild_name.is_empty() { "Discord server" } else { guild_name };
    let id = match existing {
        Some(id) => {
            conn.execute(
                "UPDATE provider_account_bindings SET display_name = ?1, secret_ref = ?2, state = 'CONNECTED', verified_at = ?3, updated_at = ?3 WHERE id = ?4 AND owner = ?5",
                params![name, sealed, t, id, owner],
            )
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            id
        }
        None => {
            let id = crate::agent_gateway_routes::id("acct");
            conn.execute(
                "INSERT INTO provider_account_bindings (id, owner, vendor, auth_type, external_account_id, display_name, secret_ref, scopes_json, state, verified_at, created_at, updated_at)
                 VALUES (?1, ?2, 'discord', 'channel_oauth', ?3, ?4, ?5, '[\"messages\"]', 'CONNECTED', ?6, ?6, ?6)",
                params![id, owner, guild_id, name, sealed, t],
            )
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            id
        }
    };
    Ok(json!({ "account": { "id": id, "vendor": "discord", "displayName": name, "handle": guild_id, "state": "CONNECTED" } }))
}

async fn discord_connect_h(
    axum::extract::State(state): axum::extract::State<Arc<crate::AppState>>,
    axum::Extension(user): axum::Extension<crate::auth::AuthUser>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<DiscordConnectBody>,
) -> axum::response::Response {
    use axum::{http::StatusCode, response::IntoResponse, Json};
    let base = crate::phone_sync::cloud_base();
    // Ask the cloud which servers this user added the shared app to (the bot token never reaches a runtime).
    let mut req = reqwest::Client::new()
        .get(format!("{}/api/v1/channels/discord/installs", base.trim_end_matches('/')))
        .timeout(std::time::Duration::from_secs(10));
    if let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        req = req.header("authorization", auth);
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, Json(json!({ "error": format!("cloud unreachable: {e}") }))).into_response(),
    };
    if resp.status() == StatusCode::SERVICE_UNAVAILABLE {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "discord_not_configured" }))).into_response();
    }
    if !resp.status().is_success() {
        return (StatusCode::BAD_GATEWAY, Json(json!({ "error": format!("cloud returned {}", resp.status()) }))).into_response();
    }
    let installs: Value = resp.json().await.unwrap_or(Value::Null);
    let list = installs.get("installs").and_then(Value::as_array).cloned().unwrap_or_default();
    let install = match body.guild_id.as_deref() {
        Some(g) => list.iter().find(|i| i.get("guildId").and_then(Value::as_str) == Some(g)).cloned(),
        None => list.last().cloned(), // newest install
    };
    let Some(install) = install else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not_installed" }))).into_response();
    };
    let guild = install.get("guildId").and_then(Value::as_str).unwrap_or_default();
    let name = install.get("guildName").and_then(Value::as_str).unwrap_or_default();
    match upsert_app_connection(&state.db, &user.user_id, guild, name) {
        Ok(v) => Json(v).into_response(),
        Err((code, msg)) => (code, Json(json!({ "error": msg }))).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use std::sync::Mutex;

    use crate::channel_transports::HttpResp;

    #[derive(Default)]
    struct FakeHttp {
        sent: Mutex<Vec<HttpReq>>,
        status: Mutex<Option<u16>>,
    }
    #[async_trait]
    impl HttpSend for FakeHttp {
        async fn post_json(&self, req: HttpReq) -> Result<HttpResp, String> {
            self.sent.lock().unwrap().push(req);
            let status = self.status.lock().unwrap().unwrap_or(200);
            Ok(HttpResp { status, body: if status == 200 { json!({ "messageId": "m-100" }) } else { json!({ "error": "discord_not_configured" }) } })
        }
    }

    fn bots() -> Vec<MemberBot> {
        vec![MemberBot { id: "b1".into(), name: "Live Check".into() }, MemberBot { id: "b2".into(), name: "Finance".into() }]
    }
    fn acct() -> Account {
        Account { id: "a".into(), owner: "u".into(), restricted_bot: None, secret: json!({ "mode": "app" }).to_string() }
    }
    fn msg(text: &str) -> Inbound {
        discord_normalize(&json!({ "t": "MESSAGE_CREATE", "s": 1, "d": { "id": "9", "channel_id": "c1", "guild_id": "g1", "content": text, "author": { "id": "u1" } } })).remove(0)
    }
    async fn db(tag: &str) -> DbHandle {
        let dir = std::env::temp_dir().join(format!("allternit-dapp-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::test_helpers::app_state(&dir).await.db.clone()
    }

    fn transport(http: Arc<FakeHttp>, db: Option<DbHandle>) -> DiscordAppTransport {
        DiscordAppTransport { http, db, cloud_base: Some("https://api.test/".into()), cloud_token: Some("allternit_runtime_x".into()), guild_id: None, public_key: None, relay_secret: Some("s3".into()) }
    }

    #[tokio::test]
    async fn the_mention_hook_picks_the_right_bot() {
        let d = db("hook").await;
        let r = |t: &str| rewrite(&d, &acct(), &bots(), &msg(t)).text.unwrap();
        assert_eq!(r("<@123> finance what is margin?"), "@finance what is margin?");
        assert_eq!(r("<@!123> Live Check ping"), "@livecheck ping");
        assert_eq!(r("@Allternit finance hello"), "@finance hello");
        assert_eq!(r("<@123> nobody home"), "nobody home");
        // Not in app mode: untouched.
        let mut plain = acct();
        plain.secret = "{}".into();
        assert_eq!(rewrite(&d, &plain, &bots(), &msg("<@123> finance x")).text.unwrap(), "<@123> finance x");
    }

    #[tokio::test]
    async fn a_dm_conversation_posts_through_the_cloud_dm_route_not_a_webhook() {
        let http = Arc::new(FakeHttp::default());
        let t = transport(http.clone(), None);
        let out = Outbound { workspace: Some("@dm:123456789012345678".into()), channel: "dmchan".into(), thread: None, text: "hello".into(), identity: Some("Finance".into()) };
        let r = t.post(&out).await;
        // The fake answers {messageId} only for the webhook route's shape; the DM route needs the same field.
        assert_eq!(r.unwrap().remote_id, "m-100");
        let sent = http.sent.lock().unwrap()[0].clone();
        assert_eq!(sent.url, "https://api.test/api/v1/channels/discord/dm");
        assert_eq!(sent.body["userId"], "123456789012345678");
        assert_eq!(sent.body["botName"], "Finance");
        assert_eq!(sent.body["text"], "hello");
        assert!(sent.body.get("guildId").is_none() && sent.body.get("channelId").is_none());
        // The person's settings refuse it: a definite, plain reason.
        *http.status.lock().unwrap() = Some(403);
        let refused = t.post(&out).await;
        assert!(matches!(refused, Err(PostError::Rejected(_))), "{refused:?}");
    }

    #[test]
    fn a_slash_command_becomes_an_at_mention() {
        let t = transport(Arc::new(FakeHttp::default()), None);
        let evs = t.normalize(&json!({ "type": 2, "id": "i1", "channel_id": "c1", "guild_id": "g1", "member": { "user": { "id": "u1" } }, "data": { "name": "finance", "options": [{ "name": "message", "value": "what is margin?" }] } }));
        assert_eq!(evs[0].text.as_deref(), Some("@finance what is margin?"));
        assert_eq!(evs[0].conversation, "discord:c1");
        assert_eq!(evs[0].workspace.as_deref(), Some("g1"));
    }

    #[tokio::test]
    async fn the_post_goes_through_the_cloud_with_the_bots_name_and_a_reply_maps_back() {
        let d = db("post").await;
        let http = Arc::new(FakeHttp::default());
        let t = transport(http.clone(), Some(d.clone()));
        let out = Outbound { workspace: Some("g1".into()), channel: "c1".into(), thread: Some("t9".into()), text: "hi".into(), identity: Some("Finance".into()) };
        let r = t.post(&out).await.unwrap();
        assert_eq!(r.remote_id, "m-100");
        let sent = http.sent.lock().unwrap();
        assert_eq!(sent[0].url, "https://api.test/api/v1/channels/discord/send");
        assert_eq!(sent[0].headers[0], ("Authorization".into(), "Bearer allternit_runtime_x".into()));
        assert_eq!(sent[0].body, json!({ "guildId": "g1", "channelId": "c1", "threadId": "t9", "botName": "Finance", "text": "hi" }));
        drop(sent);
        // A reply to message m-100 is routed to Finance.
        let evs = t.normalize(&json!({ "t": "MESSAGE_CREATE", "s": 2, "d": { "id": "10", "channel_id": "c1", "guild_id": "g1", "content": "and tax?", "author": { "id": "u1" }, "message_reference": { "message_id": "m-100" } } }));
        assert_eq!(evs[0].text.as_deref(), Some("@finance and tax?"));
        // Unknown replied-to message: unchanged.
        let evs = t.normalize(&json!({ "t": "MESSAGE_CREATE", "s": 3, "d": { "id": "11", "channel_id": "c1", "content": "x", "author": { "id": "u1" }, "message_reference": { "message_id": "other" } } }));
        assert_eq!(evs[0].text.as_deref(), Some("x"));
    }

    #[tokio::test]
    async fn unconfigured_cloud_is_a_definite_rejection() {
        let http = Arc::new(FakeHttp::default());
        let out = Outbound { workspace: Some("g1".into()), channel: "c1".into(), thread: None, text: "hi".into(), identity: None };
        let mut t = transport(http.clone(), None);
        *http.status.lock().unwrap() = Some(503);
        assert_eq!(t.post(&out).await, Err(PostError::Rejected("discord_not_configured".into())));
        t.cloud_base = None;
        assert!(matches!(t.post(&out).await, Err(PostError::Rejected(_))));
        assert!(http.sent.lock().unwrap().len() == 1);
    }

    #[test]
    fn webhook_names_never_contain_discord_or_clyde() {
        assert_eq!(safe_name("Discord Helper"), "Helper");
        assert_eq!(safe_name("CLYDE"), "Allternit");
        assert_eq!(safe_name("Finance"), "Finance");
    }

    #[test]
    fn relayed_events_need_a_valid_signature() {
        let t = transport(Arc::new(FakeHttp::default()), None);
        let body = br#"{"t":"MESSAGE_CREATE"}"#;
        let mut mac = Hmac::<Sha256>::new_from_slice(b"s3").unwrap();
        mac.update(body);
        let mut h = HeaderMap::new();
        h.insert("x-allternit-relay-signature", hex::encode(mac.finalize().into_bytes()).parse().unwrap());
        assert!(t.verify("", &h, body).is_ok());
        assert!(t.verify("", &h, b"tampered").is_err());
        assert!(t.verify("", &HeaderMap::new(), body).is_err());
    }

    fn env(extra: Value) -> Value {
        let mut e = json!({ "source": "allternit-discord-app", "kind": "message", "guildId": "g1", "channelId": "c1",
            "authorId": "u1", "content": "<@app> scout hello", "messageId": "m1", "threadId": null,
            "replyToMessageId": null, "commandName": null, "interactionId": null });
        for (k, v) in extra.as_object().unwrap() {
            e[k] = v.clone();
        }
        e
    }

    #[tokio::test]
    async fn envelopes_become_message_inbounds() {
        let dir = std::env::temp_dir().join(format!("allternit-da-env-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_helpers::app_state(&dir).await;
        // A plain message keeps its text; the mention hook rewrites it later.
        let e = envelope_events(&st.db, &env(json!({}))).remove(0);
        assert_eq!((e.conversation.as_str(), e.channel.as_str(), e.workspace.as_deref()), ("discord:c1", "c1", Some("g1")));
        assert_eq!((e.text.as_deref(), e.user.as_deref(), e.remote_id.as_str()), (Some("<@app> scout hello"), Some("u1"), "m1"));
        assert!(!e.own);
        // A thread keeps the parent channel for the conversation.
        let e = envelope_events(&st.db, &env(json!({ "threadId": "t9" }))).remove(0);
        assert_eq!((e.channel.as_str(), e.thread.as_deref()), ("c1", Some("t9")));
        // A slash command is "@name text", keyed by the interaction id.
        let e = envelope_events(&st.db, &env(json!({ "kind": "command", "commandName": "Scout Bot", "content": "", "interactionId": "i1" }))).remove(0);
        assert_eq!((e.text.as_deref(), e.remote_id.as_str()), (Some("@ScoutBot hi"), "i1"));
        // A reply to a message one of our bots posted goes to that bot.
        st.db.connect().unwrap().execute("INSERT INTO discord_app_messages (message_id, channel_id, bot_name) VALUES ('bm1','c1','Scout')", []).unwrap();
        let e = envelope_events(&st.db, &env(json!({ "content": "thanks", "replyToMessageId": "bm1" }))).remove(0);
        assert_eq!(e.text.as_deref(), Some("@scout thanks"));
        // Nothing to say, or no author: dropped.
        assert!(envelope_events(&st.db, &env(json!({ "content": "  " }))).is_empty());
        assert!(envelope_events(&st.db, &env(json!({ "authorId": null }))).is_empty());
    }

    #[tokio::test]
    async fn the_route_only_accepts_signed_cloud_envelopes() {
        use tower::ServiceExt;
        let dir = std::env::temp_dir().join(format!("allternit-da-sig-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_helpers::app_state(&dir).await;
        let secret = Arc::new(crate::relay_auth::StaticRelaySecret { token: "tok".into(), owner: "user-a".into() });
        let app = discord_app_router_with(secret).with_state(st);
        let good = env(json!({})).to_string();
        let status = |req: axum::http::Request<axum::body::Body>| {
            let app = app.clone();
            async move { app.oneshot(req).await.unwrap().status() }
        };
        // A raw (unsigned) body, even a well-formed envelope, never gets in.
        assert_eq!(status(crate::relay_auth::relayed_post(DISCORD_APP_EVENTS_PATH, good.as_bytes(), None)).await, StatusCode::UNAUTHORIZED);
        assert_eq!(status(crate::relay_auth::relayed_post(DISCORD_APP_EVENTS_PATH, good.as_bytes(), Some(("tok", "user-b")))).await, StatusCode::UNAUTHORIZED);
        // Signed: only an allternit envelope is accepted, and with no app-mode
        // connection for this owner it is acknowledged and ignored.
        let foreign = json!({ "t": "MESSAGE_CREATE", "d": {} }).to_string();
        assert_eq!(status(crate::relay_auth::relayed_post(DISCORD_APP_EVENTS_PATH, foreign.as_bytes(), Some(("tok", "user-a")))).await, StatusCode::BAD_REQUEST);
        assert_eq!(status(crate::relay_auth::relayed_post(DISCORD_APP_EVENTS_PATH, good.as_bytes(), Some(("tok", "user-a")))).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn connect_records_one_app_mode_connection_per_server() {
        let dir = std::env::temp_dir().join(format!("allternit-discordapp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_helpers::app_state(&dir).await;
        let r = upsert_app_connection(&st.db, "user-a", "G1", "Acme HQ").unwrap();
        let id = r["account"]["id"].as_str().unwrap().to_string();
        let r2 = upsert_app_connection(&st.db, "user-a", "G1", "").unwrap();
        assert_eq!(r2["account"]["id"].as_str().unwrap(), id, "same server: one connection");
        let n: i64 = st.db.connect().unwrap().query_row("SELECT COUNT(*) FROM provider_account_bindings WHERE vendor = 'discord'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        let secret_ref: String = st.db.connect().unwrap().query_row("SELECT secret_ref FROM provider_account_bindings WHERE id = ?1", params![id], |r| r.get(0)).unwrap();
        let secret = crate::token_crypto::open(&secret_ref);
        assert!(is_app_secret(&secret), "the transport sees it as a shared-app connection");
        assert_eq!(pick(&secret, "guildId"), "G1");
        // A transport built from it falls back to this runtime's own cloud + credential.
        let t = DiscordAppTransport::from_secret(&secret, std::sync::Arc::new(crate::channel_transports::ReqwestSend));
        assert_eq!(t.guild_id.as_deref(), Some("G1"));
        assert!(t.cloud_base.is_some());
    }

}
