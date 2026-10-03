//! Slack shared app on the runtime (allternit-api).
//!
//! The single Allternit Slack app lives at the cloud
//! (`allternit-cloud-api routes::slack_app`): it verifies Slack's signature,
//! answers `url_verification`, acks within 3 seconds and queues events by
//! team. The cloud relays them here over the trusted cloud→runtime channel at
//! [`SLACK_APP_EVENTS_PATH`]. This module is that receiving end, plus:
//!
//! * [`SlackAppTransport`] — the outbound transport for shared-app
//!   connections: it asks the cloud to post (`chat.postMessage` with per-bot
//!   `username`/`icon_url`), because the per-team bot token never leaves the
//!   cloud. Legacy per-workspace installs keep the env-token
//!   [`crate::channel_gateway::SlackTransport`].
//! * The mention hook — Slack's app mention is "@Allternit" (one shared app,
//!   so it can't carry a bot name in the mention token itself) and the app
//!   ships a `/allternit` slash command. [`rewrite_slack_mentions`] rewrites
//!   both into the "@name" form `route_inbound` already understands, in this
//!   module only — `route_inbound` itself is untouched.
//! * The connect route — after the browser OAuth install lands in the cloud,
//!   the runtime records the local connection (one Slack connection per team,
//!   bots switched on per connection like the other messaging connectors).
//!
//! Migration V216 moved legacy `slack_channel_bots` rows into
//! `channel_account_bots` on a 'legacy' connection per owner; those keep
//! posting through the env-token transport until the owner connects the
//! shared app.

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::{Extension, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::channel_gateway::{ChannelTransport, Identity, InboundKind, Outbound, PostError, Receipt};
use crate::channel_transports::{accounts, HttpReq, HttpSend};
use crate::db::DbHandle;
use crate::AppState;

/// Where the cloud delivers shared-app events (see allternit-cloud-api
/// `routes::slack_app::RUNTIME_EVENTS_PATH`).
pub const SLACK_APP_EVENTS_PATH: &str = "/webhooks/channels/slack-app";
/// Bearer the runtime uses to ask the cloud to send. Integration concern of
/// the Desktop/hosted runtime (the cloud route also accepts the caller's
/// Clerk session / API token, forwarded by UI-driven flows).
pub const CLOUD_TOKEN_ENV: &str = "ALLTERNIT_CLOUD_TOKEN";

// ---------------------------------------------------------------- transport

/// True when a connection secret describes a shared-app connection (team
/// metadata sealed at connect time) rather than a legacy env-token install.
pub fn is_shared_secret(secret: &str) -> bool {
    serde_json::from_str::<Value>(secret)
        .ok()
        .is_some_and(|v| v.get("teamId").and_then(Value::as_str).is_some_and(|t| !t.is_empty()))
}

/// Outbound for a shared-app Slack connection: posts go to the cloud's
/// `/api/v1/channels/slack/send`, which holds the sealed per-team bot token
/// and posts with `username`/`icon_url` (the shared app's
/// `chat:write.customize` scope), so every bot answers under its own name.
pub struct SlackAppTransport {
    pub http: Arc<dyn HttpSend>,
    /// `ALLTERNIT_CLOUD_API_URL` / company `cloudApiUrl`.
    pub cloud: Option<String>,
    /// Bearer for the cloud send route (`CLOUD_TOKEN_ENV`).
    pub token: Option<String>,
    /// Which team's connection this transport serves (diagnostics only; the
    /// cloud resolves the install from the authenticated user).
    pub team_id: String,
    /// When present, `Outbound.identity` is resolved to the bot's display
    /// name and avatar for the post.
    pub db: Option<DbHandle>,
}

impl SlackAppTransport {
    pub fn from_secret(secret: &str, http: Arc<dyn HttpSend>, db: Option<DbHandle>) -> Option<Self> {
        if !is_shared_secret(secret) {
            return None;
        }
        let team_id = crate::channel_transports::pick(secret, "teamId");
        Some(Self {
            http,
            cloud: crate::config::AppConfig::load().cloud_api_url(),
            token: std::env::var(CLOUD_TOKEN_ENV).ok().filter(|s| !s.is_empty()),
            team_id,
            db,
        })
    }

    /// The bot's display name and avatar for a posting identity, when known.
    fn bot_identity(&self, identity: Option<&str>) -> (Option<String>, Option<String>) {
        let Some(id) = identity else { return (None, None) };
        // No bot row to look up: post under the identity as given rather than
        // dropping it (the name still beats the app's default).
        let fallback = (Some(id.to_string()), None);
        let Some(db) = self.db.as_ref() else { return fallback };
        let Ok(conn) = db.connect() else { return fallback };
        let row: Option<(String, Option<String>)> = conn
            .query_row(
                "SELECT COALESCE(NULLIF(name, ''), id), avatar FROM agents WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        row.map(|(name, avatar)| (Some(name), avatar.filter(|a| a.starts_with("https://")))).unwrap_or(fallback)
    }
}

#[async_trait]
impl ChannelTransport for SlackAppTransport {
    fn provider(&self) -> &'static str {
        "slack"
    }
    /// Shared-app events arrive over the authenticated cloud→runtime relay
    /// (the cloud already verified Slack's signature); there is nothing to
    /// verify again here.
    fn verify(&self, _secret: &str, _headers: &HeaderMap, _body: &[u8]) -> Result<(), String> {
        Err("slack shared-app events arrive via the verified cloud relay, not a public webhook".into())
    }
    fn normalize(&self, payload: &Value) -> Vec<crate::channel_gateway::Inbound> {
        crate::channel_gateway::SlackTransport { token: None, own_identity: None }.normalize(payload)
    }
    fn identity(&self, requested: Option<&str>) -> Identity {
        // chat:write.customize: the app can post under any bot name/icon.
        Identity { id: requested.map(str::to_string), exact: true }
    }
    async fn post(&self, out: &Outbound) -> Result<Receipt, PostError> {
        let base = self
            .cloud
            .clone()
            .ok_or_else(|| PostError::Rejected("no cloud API URL configured for the Slack shared app".into()))?;
        let token = self
            .token
            .clone()
            .ok_or_else(|| PostError::Rejected(format!("{CLOUD_TOKEN_ENV} is not set; the runtime cannot ask the cloud to send")))?;
        let (username, icon_url) = self.bot_identity(out.identity.as_deref());
        let mut body = json!({ "channel": out.channel, "text": out.text });
        if let Some(t) = &out.thread {
            body["threadTs"] = json!(t);
        }
        if let Some(u) = username {
            body["username"] = json!(u);
        }
        if let Some(i) = icon_url {
            body["iconUrl"] = json!(i);
        }
        let resp = self
            .http
            .post_json(HttpReq {
                url: format!("{}/api/v1/channels/slack/send", base.trim_end_matches('/')),
                headers: vec![("authorization".into(), format!("Bearer {token}"))],
                body,
            })
            .await
            .map_err(|e| PostError::Uncertain(format!("cloud slack send failed: {e}")))?;
        match resp.status {
            200..=299 if resp.body.get("ok").and_then(Value::as_bool) == Some(true) => {
                let ts = resp.body.get("ts").and_then(Value::as_str).unwrap_or_default().to_string();
                Ok(Receipt { remote_id: ts, relayed: false })
            }
            429 => Err(PostError::Rejected("rate limited by the Slack shared app lane".into())),
            500..=599 => Err(PostError::Uncertain(format!("cloud slack send returned {}", resp.status))),
            _ => Err(PostError::Rejected(format!(
                "cloud slack send refused: {}",
                resp.body.get("error").and_then(Value::as_str).unwrap_or("unknown")
            ))),
        }
    }
}

// ---------------------------------------------------------------- mention hook

/// Rewrite Slack's shared-app mention forms into the "@name" form
/// `route_inbound` understands:
///
/// * `/allternit engineer ship it` → `@engineer ship it` (slash command; the
///   first word is the bot name, the rest the message),
/// * `@Allternit engineer ship it` → `@engineer ship it` (app mention in a
///   message, e.g. "@Allternit <bot> …"),
/// * `@Allternit ship it` → `ship it` (app mention alone = the default bot).
///
/// Everything else passes through unchanged. Names keep their original case;
/// `mentioned_bot` compares without case or punctuation.
pub fn rewrite_slack_mentions(text: &str, member_names: &[String]) -> String {
    let trimmed = text.trim_start();
    for prefix in ["/allternit", "@Allternit"] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            // The first word becomes "@name" only when it is a bot switched on
            // here; otherwise it is part of the message for the default bot.
            return name_first_word_when_a_bot(rest.trim(), member_names);
        }
    }
    text.to_string()
}

// ---------------------------------------------------------------- inbound webhook

/// The cloud relays Slack's raw event inside this envelope (see
/// allternit-cloud-api `slack_app`).
#[derive(Deserialize)]
struct RelayEnvelope {
    team_id: Option<String>,
    #[allow(dead_code)]
    api_app_id: Option<String>,
    event_id: Option<String>,
    event: Option<Value>,
    /// Slash commands are queued with kind "command" (their payload shape is
    /// the command form, not an event_callback event).
    kind: Option<String>,
}

pub fn slack_app_webhook_router() -> Router<Arc<AppState>> {
    Router::new().route(SLACK_APP_EVENTS_PATH, post(shared_events_h))
}

/// The connection an inbound team maps to: a shared-app connection whose
/// sealed metadata names the team, else the owner's 'legacy' connection
/// (V216) answered by the env-token transport. `None` = no connection at all.
fn account_for_team(db: &DbHandle, team_id: &str) -> Option<crate::channel_transports::Account> {
    let shared = accounts(db, "slack", None)
        .into_iter()
        .find(|a| crate::channel_transports::pick(&a.secret, "teamId") == team_id);
    if shared.is_some() {
        return shared;
    }
    // Legacy (custom-app) connections migrated by V216: no team id is known
    // for them; answer their events on the first legacy connection.
    let conn = db.connect().ok()?;
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT id, owner FROM provider_account_bindings
             WHERE vendor = 'slack' AND auth_type = 'channel_oauth' AND external_account_id = 'legacy'
             ORDER BY created_at LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    row.map(|(id, owner)| crate::channel_transports::Account { id, owner, restricted_bot: None, secret: String::new() })
}

fn transport_for_account(state: &Arc<AppState>, acct: &crate::channel_transports::Account) -> Option<Arc<dyn ChannelTransport>> {
    if is_shared_secret(&acct.secret) {
        return SlackAppTransport::from_secret(&acct.secret, Arc::new(crate::channel_transports::ReqwestSend), Some(state.db.clone()))
            .map(|t| Arc::new(t) as Arc<dyn ChannelTransport>);
    }
    Some(Arc::new(crate::channel_gateway::SlackTransport::from_env()))
}

/// A queued slash command becomes a message-shaped Inbound in its own
/// conversation (`slack:<channel>:cmd:<trigger>`), so each command run is one
/// thread and the answer posts top-level in the channel.
fn command_inbound(team_id: &str, cmd: &Value) -> Option<crate::channel_gateway::Inbound> {
    let channel = cmd.get("channel_id").and_then(Value::as_str)?.to_string();
    let trigger = cmd.get("trigger_id").and_then(Value::as_str).unwrap_or_default().to_string();
    let text = cmd.get("text").and_then(Value::as_str).unwrap_or_default().to_string();
    let user = cmd.get("user_id").and_then(Value::as_str).map(str::to_string);
    Some(crate::channel_gateway::Inbound {
        kind: InboundKind::Message,
        workspace: Some(team_id.to_string()),
        conversation: format!("slack:{channel}:cmd:{trigger}"),
        channel,
        thread: None,
        remote_id: format!("cmd:{trigger}"),
        message_id: format!("cmd:{trigger}"),
        text: Some(format!("/allternit {}", text).trim_end().to_string()),
        user,
        reaction: None,
        added: None,
        cursor: None,
        own: false,
    })
}

/// Strip leading Slack user-mention tokens ("<@U123>" or "<@U123|name>")
/// from app_mention text; what remains is what the person typed.
fn strip_app_mention_token(text: &str) -> String {
    let mut rest = text.trim_start();
    loop {
        let Some(open) = rest.strip_prefix('<') else { break };
        let Some(close_idx) = open.find('>') else { break };
        let token = &open[..close_idx];
        if !(token.starts_with("@U") || token.starts_with("@W")) {
            break;
        }
        rest = open[close_idx + 1..].trim_start();
    }
    rest.to_string()
}

/// "@Allternit engineer ship it" arrives (after the mention token is
/// stripped) as "engineer ship it". When the first word is one of the
/// connection's member bots, tag it "@engineer"; otherwise the default bot
/// keeps the whole text.
fn name_first_word_when_a_bot(text: &str, member_names: &[String]) -> String {
    let mut words = text.splitn(2, char::is_whitespace);
    let Some(first) = words.next() else { return String::new() };
    if first.is_empty() {
        return String::new();
    }
    let handle = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<String>();
    let is_bot = member_names.iter().any(|n| handle(n) == handle(first));
    match (is_bot, words.next()) {
        (true, Some(rest)) => format!("@{first} {}", rest.trim_start()),
        (true, None) => format!("@{first}"),
        _ => text.to_string(),
    }
}

/// A plain top-level public/private channel message (no thread, no mention):
/// `message.channels` is subscribed threads-only, so these never open a
/// thread. DMs (`D…`) and app mentions are not affected.
fn is_plain_channel_root(ev: &Value) -> bool {
    ev.get("type").and_then(Value::as_str) == Some("message")
        && ev.get("subtype").is_none()
        && ev.get("thread_ts").is_none()
        && ev
            .get("channel")
            .and_then(Value::as_str)
            .map(|c| c.starts_with('C') || c.starts_with('G'))
            .unwrap_or(false)
}

async fn shared_events_h(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Ok(envelope) = serde_json::from_slice::<RelayEnvelope>(&body) else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_json" }))).into_response();
    };
    let Some(team_id) = envelope.team_id.as_deref() else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "missing teamId" }))).into_response();
    };
    let Some(acct) = account_for_team(&state.db, team_id) else {
        tracing::warn!(%team_id, "slack shared-app event for a team with no connection; ignored");
        return Json(json!({ "ok": true, "ignored": true })).into_response();
    };
    let Some(tx) = transport_for_account(&state, &acct) else {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "slack_not_configured" }))).into_response();
    };
    let mut events: Vec<crate::channel_gateway::Inbound> = match envelope.kind.as_deref() {
        Some("command") => envelope.event.as_ref().and_then(|cmd| command_inbound(team_id, cmd)).into_iter().collect(),
        _ => {
            let mut ev = envelope.event.clone().unwrap_or(Value::Null);
            // app_mention arrives as its own event type; render it as a message
            // and strip the app's bot-user token ("<@U…>"), leaving the text a
            // person typed after "@Allternit".
            let mut from_mention = false;
            if ev.get("type").and_then(Value::as_str) == Some("app_mention") {
                from_mention = true;
                let mut m = ev.clone();
                m["type"] = json!("message");
                if let Some(text) = m.get("text").and_then(Value::as_str) {
                    let stripped = strip_app_mention_token(text);
                    // "@Allternit engineer ship it" leaves "engineer" as the
                    // first word; name it only when it is a member bot.
                    let names: Vec<String> = crate::channel_transports::member_bots(&state.db, &acct).iter().map(|b| b.name.clone()).collect();
                    m["text"] = json!(name_first_word_when_a_bot(&stripped, &names));
                }
                ev = m;
            }
            // message.channels is subscribed threads-only: plain top-level
            // channel chatter never opens a thread (DMs do; mentions do).
            if !from_mention && is_plain_channel_root(&ev) {
                return Json(json!({ "ok": true, "ignored": "channel_root" })).into_response();
            }
            tx.normalize(&json!({ "event": ev, "team_id": team_id }))
        }
    };
    for e in &mut events {
        // The mention hook lives here, in the Slack module: "@Allternit <bot>"
        // and "/allternit <bot>" become "@<bot>" before routing.
        if e.kind == InboundKind::Message {
            if let Some(text) = e.text.take() {
                let names: Vec<String> = crate::channel_transports::member_bots(&state.db, &acct).iter().map(|b| b.name.clone()).collect();
                e.text = Some(rewrite_slack_mentions(&text, &names));
            }
        }
    }
    let st = state.clone();
    tokio::spawn(async move {
        crate::channel_transports::dispatch_events(&st, &acct, tx, events).await;
    });
    Json(json!({ "ok": true })).into_response()
}

// ---------------------------------------------------------------- connect

pub fn slack_app_connect_router() -> Router<Arc<AppState>> {
    Router::new().route("/gateway/channel-accounts/slack", post(slack_connect_h))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SlackConnectBody {
    team_id: Option<String>,
    runtime_id: Option<String>,
}

/// Record (or refresh) the local connection for an installed team. Shared by
/// the connect route and the tests.
pub fn upsert_shared_connection(db: &DbHandle, owner: &str, team_id: &str, team_name: &str) -> Result<Value, (StatusCode, String)> {
    let keys = json!({ "teamId": team_id, "teamName": team_name, "sharedApp": true }).to_string();
    let Some(sealed) = crate::agent_gateway_routes::seal_strict(&keys) else {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "no encryption key is configured; the connection was not stored".into()));
    };
    let conn = db.connect().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM provider_account_bindings WHERE owner = ?1 AND vendor = 'slack' AND auth_type = 'channel_oauth' AND external_account_id = ?2",
            params![owner, team_id],
            |r| r.get(0),
        )
        .ok();
    let t = crate::agent_gateway_routes::now();
    let id = match existing {
        Some(id) => {
            conn.execute(
                "UPDATE provider_account_bindings SET display_name = ?1, state = 'CONNECTED', verified_at = ?2, updated_at = ?2 WHERE id = ?3 AND owner = ?4",
                params![team_name, t, id, owner],
            )
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            id
        }
        None => {
            let id = crate::agent_gateway_routes::id("acct");
            conn.execute(
                "INSERT INTO provider_account_bindings (id, owner, vendor, auth_type, external_account_id, display_name, secret_ref, scopes_json, state, verified_at, created_at, updated_at)
                 VALUES (?1, ?2, 'slack', 'channel_oauth', ?3, ?4, ?5, '[\"messages\"]', 'CONNECTED', ?6, ?6, ?6)",
                params![id, owner, team_id, team_name, sealed, t],
            )
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            id
        }
    };
    Ok(json!({ "account": { "id": id, "vendor": "slack", "displayName": team_name, "handle": team_id, "state": "CONNECTED" } }))
}

async fn slack_connect_h(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<crate::auth::AuthUser>,
    headers: HeaderMap,
    Json(body): Json<SlackConnectBody>,
) -> Response {
    let Some(base) = crate::config::AppConfig::load().cloud_api_url() else {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "slack_not_configured" }))).into_response();
    };
    // Ask the cloud which teams this user installed the shared app on (the
    // browser OAuth landed there; per-team tokens never reach a runtime).
    let mut req = reqwest::Client::new()
        .get(format!("{}/api/v1/channels/slack/installs", base.trim_end_matches('/')))
        .timeout(std::time::Duration::from_secs(10));
    if let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        req = req.header("authorization", auth);
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, Json(json!({ "error": format!("cloud unreachable: {e}") }))).into_response(),
    };
    if resp.status() == StatusCode::SERVICE_UNAVAILABLE {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "slack_not_configured" }))).into_response();
    }
    if !resp.status().is_success() {
        return (StatusCode::BAD_GATEWAY, Json(json!({ "error": format!("cloud returned {}", resp.status()) }))).into_response();
    }
    let Ok(installs) = resp.json::<Value>().await else {
        return (StatusCode::BAD_GATEWAY, Json(json!({ "error": "cloud returned an unreadable installs list" }))).into_response();
    };
    let install = installs
        .get("installs")
        .and_then(Value::as_array)
        .and_then(|list| match body.team_id.as_deref() {
            Some(want) => list.iter().find(|i| i["teamId"].as_str() == Some(want)).cloned(),
            None => list.first().cloned(),
        });
    let Some(install) = install else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "no Slack install found — add the Allternit app to Slack first" }))).into_response();
    };
    let team_id = install["teamId"].as_str().unwrap_or_default().to_string();
    let team_name = install["teamName"].as_str().unwrap_or(team_id.as_str()).to_string();
    let result = match upsert_shared_connection(&state.db, &user.user_id, &team_id, &team_name) {
        Ok(v) => v,
        Err((status, msg)) => return (status, Json(json!({ "error": msg }))).into_response(),
    };
    // Tell the cloud which runtime delivers this team's events, when the
    // caller (Desktop wizard) knows its runtime device id.
    if let (Some(runtime_id), Some(auth)) = (body.runtime_id.clone(), headers.get("authorization").and_then(|v| v.to_str().ok()).map(str::to_string)) {
        let base = base.clone();
        let team_id2 = team_id.clone();
        tokio::spawn(async move {
            let _ = reqwest::Client::new()
                .post(format!("{}/api/v1/channels/slack/installs/{}/claim", base.trim_end_matches('/'), team_id2))
                .header("authorization", auth)
                .json(&json!({ "runtimeId": runtime_id }))
                .send()
                .await;
        });
    }
    Json(result).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeHttp {
        sent: Mutex<Vec<HttpReq>>,
        reply: Mutex<Option<Result<crate::channel_transports::HttpResp, String>>>,
    }
    #[async_trait]
    impl HttpSend for FakeHttp {
        async fn post_json(&self, req: HttpReq) -> Result<crate::channel_transports::HttpResp, String> {
            self.sent.lock().unwrap().push(req);
            self.reply
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(Ok(crate::channel_transports::HttpResp { status: 200, body: json!({}) }))
        }
    }

    #[test]
    fn mention_forms_become_at_name_for_route_inbound() {
        assert_eq!(rewrite_slack_mentions("/allternit engineer ship it", &["engineer".to_string(), "ops".to_string()]), "@engineer ship it");
        assert_eq!(rewrite_slack_mentions("/allternit ship it", &["engineer".to_string(), "ops".to_string()]), "ship it", "no name: the command word is the message");
        assert_eq!(rewrite_slack_mentions("/allternit", &["engineer".to_string(), "ops".to_string()]), "", "bare command: default bot, empty message");
        assert_eq!(rewrite_slack_mentions("@Allternit engineer ship it", &["engineer".to_string(), "ops".to_string()]), "@engineer ship it");
        assert_eq!(rewrite_slack_mentions("@Allternit ship it", &["engineer".to_string(), "ops".to_string()]), "ship it", "no name: default bot keeps the text");
        assert_eq!(rewrite_slack_mentions("@Allternit", &["engineer".to_string(), "ops".to_string()]), "");
        assert_eq!(rewrite_slack_mentions("plain message, no mention", &["engineer".to_string(), "ops".to_string()]), "plain message, no mention");
        assert_eq!(rewrite_slack_mentions("  /allternit   ops   restart the build  ", &["engineer".to_string(), "ops".to_string()]), "@ops restart the build");
    }

    #[test]
    fn app_mention_tokens_are_stripped_and_a_member_name_is_tagged() {
        assert_eq!(strip_app_mention_token("<@U9> engineer ship it"), "engineer ship it");
        assert_eq!(strip_app_mention_token("<@U9|allternit> ship it"), "ship it");
        assert_eq!(strip_app_mention_token("<@U9><@U10> hi"), "hi");
        assert_eq!(strip_app_mention_token("<@U9>"), "");
        assert_eq!(strip_app_mention_token("no mention"), "no mention");
        let members = vec!["Engineer".to_string(), "ops".to_string()];
        assert_eq!(name_first_word_when_a_bot("engineer ship it", &members), "@engineer ship it");
        assert_eq!(name_first_word_when_a_bot("Engineer", &members), "@Engineer");
        assert_eq!(name_first_word_when_a_bot("ship it", &members), "ship it", "not a bot: default bot keeps the text");
        assert_eq!(name_first_word_when_a_bot("", &members), "");
    }

    #[test]
    fn only_plain_top_level_channel_messages_are_ignored() {
        let root = json!({ "type": "message", "channel": "C1", "user": "U1", "text": "hi", "ts": "1700000001.000100" });
        assert!(is_plain_channel_root(&root));
        let mut reply = root.clone();
        reply["thread_ts"] = json!("1700000000.000100");
        assert!(!is_plain_channel_root(&reply), "thread replies are delivered");
        let mut dm = root.clone();
        dm["channel"] = json!("D1");
        assert!(!is_plain_channel_root(&dm), "DMs open conversations");
        let mut mention = root.clone();
        mention["type"] = json!("app_mention");
        assert!(!is_plain_channel_root(&mention));
        let mut bot = root.clone();
        bot["subtype"] = json!("bot_message");
        assert!(!is_plain_channel_root(&bot));
    }

    #[test]
    fn shared_secret_detection() {
        assert!(is_shared_secret(&json!({ "teamId": "T1", "teamName": "Acme" }).to_string()));
        assert!(!is_shared_secret(""));
        assert!(!is_shared_secret(&json!({ "teamName": "Acme" }).to_string()), "no teamId: not a shared-app secret");
        assert!(!is_shared_secret("xoxb-not-json"));
    }

    #[test]
    fn slash_commands_become_one_conversation_each() {
        let cmd = json!({ "channel_id": "C1", "user_id": "U1", "text": "engineer ship it", "trigger_id": "13345271933.738474920.8088930838d88f008e0", "team_id": "T1" });
        let e = command_inbound("T1", &cmd).unwrap();
        assert_eq!(e.conversation, "slack:C1:cmd:13345271933.738474920.8088930838d88f008e0");
        assert_eq!(e.text.as_deref(), Some("/allternit engineer ship it"));
        assert_eq!(e.user.as_deref(), Some("U1"));
        assert!(command_inbound("T1", &json!({})).is_none());
    }

    #[tokio::test]
    async fn shared_transport_posts_through_the_cloud_with_the_bot_identity() {
        let http = Arc::new(FakeHttp::default());
        *http.reply.lock().unwrap() = Some(Ok(crate::channel_transports::HttpResp {
            status: 200,
            body: json!({ "ok": true, "ts": "1700000002.000200", "teamId": "T1" }),
        }));
        let t = SlackAppTransport {
            http: http.clone(),
            cloud: Some("https://cloud.test".into()),
            token: Some("tok".into()),
            team_id: "T1".into(),
            db: None,
        };
        let out = Outbound {
            workspace: Some("T1".into()),
            channel: "C1".into(),
            thread: Some("1700000001.000100".into()),
            text: "hello".into(),
            identity: None,
        };
        let r = t.post(&out).await.unwrap();
        assert_eq!(r, Receipt { remote_id: "1700000002.000200".into(), relayed: false });
        let sent = http.sent.lock().unwrap()[0].clone();
        assert_eq!(sent.url, "https://cloud.test/api/v1/channels/slack/send");
        assert_eq!(sent.headers[0].1, "Bearer tok");
        assert_eq!(sent.body, json!({ "channel": "C1", "text": "hello", "threadTs": "1700000001.000100" }));
        // With an identity, the cloud posts under that name (chat:write.customize).
        let r = t.post(&Outbound { identity: Some("bot-1".into()), ..out.clone() }).await.unwrap();
        assert!(!r.relayed, "customize posts are the exact identity, not a relay");
        assert_eq!(http.sent.lock().unwrap()[1].body["username"], "bot-1", "no db: identity passed as-is");
        // Definite refusal vs uncertainty.
        *http.reply.lock().unwrap() = Some(Ok(crate::channel_transports::HttpResp { status: 200, body: json!({ "ok": false, "error": "channel_not_found" }) }));
        assert!(matches!(t.post(&out).await, Err(PostError::Rejected(_))));
        *http.reply.lock().unwrap() = Some(Ok(crate::channel_transports::HttpResp { status: 503, body: json!({}) }));
        assert!(matches!(t.post(&out).await, Err(PostError::Uncertain(_))));
        *http.reply.lock().unwrap() = Some(Err("timeout".into()));
        assert!(matches!(t.post(&out).await, Err(PostError::Uncertain(_))));
        // Missing cloud/token is a definite no.
        let no_cloud = SlackAppTransport { cloud: None, ..SlackAppTransport { http: http.clone(), cloud: Some("x".into()), token: Some("t".into()), team_id: "T1".into(), db: None } };
        assert!(matches!(no_cloud.post(&out).await, Err(PostError::Rejected(_))));
        let no_token = SlackAppTransport { token: None, ..SlackAppTransport { http: http.clone(), cloud: Some("x".into()), token: Some("t".into()), team_id: "T1".into(), db: None } };
        assert!(matches!(no_token.post(&out).await, Err(PostError::Rejected(_))));
    }

    #[tokio::test]
    async fn connect_upserts_one_connection_per_team() {
        let dir = std::env::temp_dir().join(format!("allternit-slackapp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_helpers::app_state(&dir).await;
        let r = upsert_shared_connection(&st.db, "user-a", "T1", "Acme").unwrap();
        let id = r["account"]["id"].as_str().unwrap().to_string();
        let row: (String, String, String) = st
            .db
            .connect()
            .unwrap()
            .query_row(
                "SELECT external_account_id, display_name, state FROM provider_account_bindings WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(row, ("T1".to_string(), "Acme".to_string(), "CONNECTED".to_string()));
        // Same team again: one row, id stable.
        let r2 = upsert_shared_connection(&st.db, "user-a", "T1", "Acme2").unwrap();
        assert_eq!(r2["account"]["id"].as_str().unwrap(), id);
        let n: i64 = st.db.connect().unwrap().query_row("SELECT COUNT(*) FROM provider_account_bindings WHERE vendor = 'slack'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        let name: String = st.db.connect().unwrap().query_row("SELECT display_name FROM provider_account_bindings WHERE id = ?1", params![id], |r| r.get(0)).unwrap();
        assert_eq!(name, "Acme2");
        // The sealed secret round-trips as a shared-app secret.
        let secret_ref: String = st.db.connect().unwrap().query_row("SELECT secret_ref FROM provider_account_bindings WHERE id = ?1", params![id], |r| r.get(0)).unwrap();
        let secret = crate::token_crypto::open(&secret_ref);
        assert!(is_shared_secret(&secret));
        assert_eq!(crate::channel_transports::pick(&secret, "teamId"), "T1");
    }

    /// The V216 migration moves legacy slack_channel_bots rows onto a
    /// 'legacy' connection with a default bot — the membership model the
    /// shared-app routing and the Messaging UI read.
    #[tokio::test]
    async fn v216_migrates_legacy_channel_bindings_into_memberships() {
        let dir = std::env::temp_dir().join(format!("allternit-v216-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_helpers::app_state(&dir).await;
        let c = st.db.connect().unwrap();
        for (bot, owner, created) in [("scout", "u", "2026-01-01"), ("ledger", "u", "2026-01-02"), ("other", "v", "2026-01-03")] {
            c.execute(
                "INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES (?1, ?2, ?1, 'm', 'p', 1, '{}')",
                params![bot, owner],
            )
            .unwrap();
            c.execute(
                "INSERT INTO slack_channel_bots (slack_channel_id, bot_id, user_id, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![format!("C{bot}"), bot, owner, created],
            )
            .unwrap();
        }
        // The migration runs in the fresh-boot path (test_helpers::app_state
        // applies embedded migrations BEFORE these inserts), so run its SQL
        // here the way refinery did at boot.
        let v216 = include_str!("../migrations/V216__slack_shared_app.sql");
        c.execute_batch(v216).unwrap();
        let acct: (String, String) = c
            .query_row(
                "SELECT id, owner FROM provider_account_bindings WHERE vendor = 'slack' AND auth_type = 'channel_oauth' AND external_account_id = 'legacy' AND owner = 'u'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let members: Vec<(String, i64)> = c
            .prepare("SELECT bot_id, is_default FROM channel_account_bots WHERE account_id = ?1 ORDER BY created_at")
            .unwrap()
            .query_map(params![acct.0], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(members, vec![("scout".to_string(), 1), ("ledger".to_string(), 0)], "earliest binding answers new conversations");
        // The other owner got their own connection.
        let n: i64 = c.query_row("SELECT COUNT(*) FROM provider_account_bindings WHERE vendor = 'slack' AND auth_type = 'channel_oauth'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);
        // Idempotent: running the migration SQL twice changes nothing.
        c.execute_batch(v216).unwrap();
        let n2: i64 = c.query_row("SELECT COUNT(*) FROM channel_account_bots", [], |r| r.get(0)).unwrap();
        assert_eq!(n2, 3);
    }
}
