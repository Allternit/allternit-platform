//! Inbound Slack Events API webhook — lets allternit-api *serve* an agent
//! through Slack. Previously the platform only went the other direction: the
//! 181-entry connector catalog lets an agent *call* Slack as a tool, but
//! nothing handled Slack calling in (no Events API webhook existed at all).
//!
//! A channel message is routed to a Gizzi agent session — reusing the
//! session already mapped to that channel+thread in
//! `slack_channel_sessions` (migration V30), or creating one on first
//! contact — and the assistant's reply is posted back via
//! `chat.postMessage`.
//!
//! A channel bound to a bot (`slack_channel_bots`, spec P6.2) goes to that
//! bot instead: each Slack thread is one task thread on the bot
//! (`thread_routes::channel_thread`, key `slack:<channel>:<thread_ts>`), the
//! turn runs with the bot's identity, instructions and memory, and the reply
//! posts back in the Slack thread. Bindings are managed per bot at
//! `/agents/:id/slack-channels`.
//!
//! Scope limits, stated rather than silently assumed: text-only (no
//! file/image attachments), unbound channels share one default agent/model, and the reply is fetched by bounded polling
//! rather than subscribing to Gizzi's event bus — this is a one-shot
//! background task per inbound message, not a held connection, so polling is
//! the simpler correct tool here (see `wait_for_reply`).

use axum::{
    body::Bytes,
    extract::{Extension, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use hmac::{Hmac, Mac};
use rusqlite::params;
use serde_json::{json, Value};
use sha2::Sha256;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

use crate::agent_session_routes::gizzi_client;
use crate::auth::AuthUser;
use crate::config::AppConfig;
use crate::db::DbHandle;
use crate::AppState;

type HmacSha256 = Hmac<Sha256>;

pub fn slack_webhook_router() -> Router<Arc<AppState>> {
    Router::new().route("/webhooks/slack/events", post(handle_event))
}

/// Authenticated: which Slack channels feed a bot.
pub fn slack_binding_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/agents/:id/slack-channels", get(list_bindings).post(bind_channel))
        .route("/agents/:id/slack-channels/:channel", delete(unbind_channel))
}

fn owns_bot(db: &DbHandle, bot_id: &str, user_id: &str) -> bool {
    db.connect()
        .ok()
        .and_then(|c| c.query_row("SELECT 1 FROM agents WHERE id = ?1 AND user_id = ?2", params![bot_id, user_id], |_| Ok(())).ok())
        .is_some()
}

fn not_yours() -> Response {
    (StatusCode::NOT_FOUND, Json(json!({ "error": "bot not found" }))).into_response()
}

async fn list_bindings(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(bot_id): Path<String>) -> Response {
    if !owns_bot(&state.db, &bot_id, &user.user_id) {
        return not_yours();
    }
    let channels: Vec<Value> = state
        .db
        .connect()
        .ok()
        .and_then(|c| {
            let mut stmt = c.prepare("SELECT slack_channel_id, created_at FROM slack_channel_bots WHERE bot_id = ?1 ORDER BY created_at").ok()?;
            let rows = stmt
                .query_map(params![bot_id], |r| Ok(json!({ "channel": r.get::<_, String>(0)?, "createdAt": r.get::<_, String>(1)? })))
                .ok()?
                .filter_map(Result::ok)
                .collect();
            Some(rows)
        })
        .unwrap_or_default();
    Json(json!({ "channels": channels, "configured": AppConfig::load().slack_signing_secret().is_some() })).into_response()
}

#[derive(serde::Deserialize)]
struct BindBody {
    channel: String,
}

async fn bind_channel(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(bot_id): Path<String>,
    Json(body): Json<BindBody>,
) -> Response {
    let channel = body.channel.trim().trim_start_matches('#').to_string();
    if channel.is_empty() || !channel.chars().all(|c| c.is_ascii_alphanumeric()) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "use the Slack channel ID, e.g. C0123ABCD" }))).into_response();
    }
    if !owns_bot(&state.db, &bot_id, &user.user_id) {
        return not_yours();
    }
    let Ok(conn) = state.db.connect() else {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "database unavailable" }))).into_response();
    };
    // One bot per channel. Taking over a channel another of your bots has is
    // allowed; someone else's binding is not yours to move.
    let holder: Option<String> = conn
        .query_row("SELECT user_id FROM slack_channel_bots WHERE slack_channel_id = ?1", params![channel], |r| r.get(0))
        .ok();
    if holder.as_deref().is_some_and(|h| h != user.user_id) {
        return (StatusCode::CONFLICT, Json(json!({ "error": "that channel is bound to someone else's bot" }))).into_response();
    }
    let _ = conn.execute(
        "INSERT INTO slack_channel_bots (slack_channel_id, bot_id, user_id, created_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(slack_channel_id) DO UPDATE SET bot_id = excluded.bot_id, created_at = excluded.created_at",
        params![channel, bot_id, user.user_id, chrono::Utc::now().to_rfc3339()],
    );
    (StatusCode::CREATED, Json(json!({ "channel": channel, "botId": bot_id }))).into_response()
}

async fn unbind_channel(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((bot_id, channel)): Path<(String, String)>,
) -> Response {
    if !owns_bot(&state.db, &bot_id, &user.user_id) {
        return not_yours();
    }
    if let Ok(conn) = state.db.connect() {
        let _ = conn.execute("DELETE FROM slack_channel_bots WHERE slack_channel_id = ?1 AND bot_id = ?2", params![channel, bot_id]);
    }
    StatusCode::NO_CONTENT.into_response()
}

/// The bot a channel is bound to, if any.
pub fn bound_bot(db: &DbHandle, channel: &str) -> Option<String> {
    db.connect()
        .ok()?
        .query_row("SELECT bot_id FROM slack_channel_bots WHERE slack_channel_id = ?1", params![channel], |r| r.get(0))
        .ok()
}


fn gizzi_base() -> String {
    AppConfig::load()
        .terminal_server_url()
        .trim_end_matches('/')
        .to_string()
}

/// Verify Slack's request signature: `X-Slack-Signature: v0=<hex hmac>` over
/// `v0:{timestamp}:{raw_body}`, `X-Slack-Request-Timestamp` within 5 minutes
/// — same HMAC-and-replay-window shape as `webhook_routes.rs`'s Svix check,
/// different vendor scheme.
pub(crate) fn verify_slack_signature(secret: &str, headers: &HeaderMap, body: &[u8]) -> Result<(), String> {
    let timestamp = headers
        .get("x-slack-request-timestamp")
        .and_then(|v| v.to_str().ok())
        .ok_or("missing x-slack-request-timestamp header")?;
    let signature = headers
        .get("x-slack-signature")
        .and_then(|v| v.to_str().ok())
        .ok_or("missing x-slack-signature header")?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("system time error: {e}"))?
        .as_secs() as i64;
    let ts: i64 = timestamp.parse().map_err(|_| "invalid x-slack-request-timestamp")?;
    if (now - ts).abs() > 300 {
        return Err("timestamp outside tolerance (+/-5 min)".to_string());
    }

    let basestring = format!("v0:{timestamp}:{}", String::from_utf8_lossy(body));
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).map_err(|_| "invalid secret length")?;
    mac.update(basestring.as_bytes());
    let expected = format!("v0={}", hex::encode(mac.finalize().into_bytes()));

    if expected == signature {
        Ok(())
    } else {
        Err("x-slack-signature mismatch".to_string())
    }
}

async fn handle_event(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let secret = match AppConfig::load().slack_signing_secret() {
        Some(s) => s,
        None => {
            warn!(
                "Slack event received but ALLTERNIT_SLACK_SIGNING_SECRET is not configured; rejecting"
            );
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "slack_not_configured"})),
            );
        }
    };

    if let Err(e) = verify_slack_signature(&secret, &headers, &body) {
        warn!("Slack signature verification failed: {e}");
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "invalid_signature"})),
        );
    }

    let payload: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            warn!("Slack event JSON parse error: {e}");
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_json"})),
            );
        }
    };

    // URL verification handshake — Slack sends this once, when the Events
    // Subscriptions URL is first configured or changed.
    if payload.get("type").and_then(|v| v.as_str()) == Some("url_verification") {
        let challenge = payload.get("challenge").cloned().unwrap_or(Value::Null);
        return (StatusCode::OK, Json(json!({ "challenge": challenge })));
    }

    if payload.get("type").and_then(|v| v.as_str()) == Some("event_callback") {
        if let Some(event) = payload.get("event") {
            let is_bot = event.get("bot_id").is_some();
            let is_message = event.get("type").and_then(|v| v.as_str()) == Some("message");
            // Skip subtyped messages (edits, joins, etc.) — only plain new
            // messages should reach the agent.
            let is_plain = event.get("subtype").is_none();

            let ty = event.get("type").and_then(|v| v.as_str()).unwrap_or_default();
            let side = matches!(ty, "reaction_added" | "reaction_removed")
                || (is_message && (is_bot || matches!(event.get("subtype").and_then(|v| v.as_str()), Some("message_changed" | "message_deleted"))));
            if side {
                // Edits, deletes, reactions and our own echoes: recorded on the
                // thread's channel binding, never a new turn.
                let db = state.db.clone();
                let event = event.clone();
                tokio::spawn(async move {
                    let tx = crate::channel_gateway::SlackTransport::from_env();
                    if let Err(e) = crate::channel_gateway::ingest_slack_side_event(&db, &tx, &event) {
                        warn!("Slack side event failed: {e}");
                    }
                });
            }
            if is_message && is_plain && !is_bot {
                let state = state.clone();
                let event = event.clone();
                // Slack requires a 200 within 3s or it retries the same
                // event; do the actual agent round-trip in the background.
                tokio::spawn(async move {
                    if let Err(e) = handle_message_event(&state, &event).await {
                        warn!("Slack message handling failed: {e}");
                    }
                });
            }
        }
    }

    (StatusCode::OK, Json(json!({ "ok": true })))
}

#[tracing::instrument(skip_all, name = "slack_webhook.handle_message_event")]
async fn handle_message_event(state: &Arc<AppState>, event: &Value) -> Result<(), String> {
    let channel = event
        .get("channel")
        .and_then(|v| v.as_str())
        .ok_or("missing channel")?
        .to_string();
    let text = event
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let thread_ts = event
        .get("thread_ts")
        .and_then(|v| v.as_str())
        .or_else(|| event.get("ts").and_then(|v| v.as_str()))
        .ok_or("missing ts")?
        .to_string();

    if text.trim().is_empty() {
        return Ok(());
    }

    if let Some(bot_id) = bound_bot(&state.db, &channel) {
        let from = event.get("user").and_then(|v| v.as_str()).unwrap_or("someone");
        let rt = crate::thread_routes::GizziRuntime { db: state.db.clone() };
        let key = format!("slack:{channel}:{thread_ts}");
        let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("Slack message").trim();
        let title: String = first.chars().take(80).collect();
        let session = crate::thread_routes::channel_thread(&state.db, &rt, &bot_id, "slack", &key, &title, &text).await?;
        info!(channel = %channel, bot = %bot_id, session_id = %session, "routing Slack message to the bot's thread");
        // One binding per Slack thread; a replayed event (Slack retry, reconnect) is not run twice.
        let tx = crate::channel_gateway::SlackTransport::from_env();
        let gw = crate::channel_gateway::bind_slack_turn(&state.db, &tx, &session, event)?;
        if let Some((_, crate::channel_gateway::Recorded::Duplicate)) = &gw {
            return Ok(());
        }
        let turn = format!("[slack from <@{from}>] {text}");
        let reply = crate::agent_session_routes::send_bot_turn(&state.db, &session, &bot_id, &turn).await?;
        return match gw {
            Some((b, _)) => crate::channel_gateway::post_reply(&state.db, &tx, &b, &thread_ts, &reply).await,
            None => post_slack_message(&channel, &thread_ts, &reply).await,
        };
    }

    let session_id = get_or_create_session(state, &channel, &thread_ts).await?;
    info!(channel = %channel, session_id = %session_id, "routing Slack message to agent session");

    let client = gizzi_client(&HeaderMap::new());
    let msg_path = format!("/v1/session/{}/message", urlencoding::encode(&session_id));
    let msg_payload = json!({ "parts": [{ "type": "text", "text": text }] });
    send_json(&client, reqwest::Method::POST, &msg_path, Some(msg_payload)).await?;

    let reply = wait_for_reply(&client, &session_id).await?;
    post_slack_message(&channel, &thread_ts, &reply).await
}

/// Reuses the session mapped to this channel+thread, or creates a new Gizzi
/// session (surface `"slack"`) and remembers the mapping.
async fn get_or_create_session(
    state: &Arc<AppState>,
    channel: &str,
    thread_ts: &str,
) -> Result<String, String> {
    {
        let conn = state.db.connect().map_err(|e| e.to_string())?;
        let existing: Option<String> = conn
            .query_row(
                "SELECT session_id FROM slack_channel_sessions \
                 WHERE slack_channel_id = ?1 AND slack_thread_ts = ?2",
                params![channel, thread_ts],
                |row| row.get(0),
            )
            .ok();
        if let Some(session_id) = existing {
            return Ok(session_id);
        }
    }

    let client = gizzi_client(&HeaderMap::new());
    let (provider_id, model_id) = AppConfig::load().default_model();
    let create_payload = json!({
        "title": format!("Slack: {channel}"),
        "surface": "slack",
        "model": { "providerID": provider_id, "modelID": model_id },
    });
    let session = send_json(
        &client,
        reqwest::Method::POST,
        "/v1/session",
        Some(create_payload),
    )
    .await?;
    let session_id = session
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("Gizzi session creation returned no id")?
        .to_string();

    let conn = state.db.connect().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR IGNORE INTO slack_channel_sessions \
         (slack_channel_id, slack_thread_ts, session_id) VALUES (?1, ?2, ?3)",
        params![channel, thread_ts, session_id],
    )
    .map_err(|e| e.to_string())?;

    Ok(session_id)
}

/// Polls for the assistant's completed reply — bounded to ~60s, checking
/// once a second. No bus subscription: this runs inside a `tokio::spawn`ed
/// background task per inbound message, not a held client connection, so
/// there's nothing for a push-based subscription to attach to here.
async fn wait_for_reply(client: &reqwest::Client, session_id: &str) -> Result<String, String> {
    let path = format!("/v1/session/{}/messages", urlencoding::encode(session_id));
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let messages = fetch_json_array(client, &path).await?;
        let reply = messages.iter().rev().find(|m| {
            let role_is_assistant = m
                .get("info")
                .and_then(|i| i.get("role"))
                .and_then(|v| v.as_str())
                == Some("assistant");
            let is_completed = m
                .get("info")
                .and_then(|i| i.get("time"))
                .and_then(|t| t.get("completed"))
                .is_some();
            role_is_assistant && is_completed
        });

        if let Some(reply) = reply {
            let text = reply
                .get("parts")
                .and_then(|p| p.as_array())
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            if !text.trim().is_empty() {
                return Ok(text);
            }
        }
    }
    Err("timed out waiting for assistant reply".to_string())
}

async fn post_slack_message(channel: &str, thread_ts: &str, text: &str) -> Result<(), String> {
    let token = AppConfig::load()
        .slack_bot_token()
        .ok_or("ALLTERNIT_SLACK_BOT_TOKEN is not configured")?;

    let client = reqwest::Client::new();
    let resp = client
        .post("https://slack.com/api/chat.postMessage")
        .bearer_auth(token)
        .json(&json!({ "channel": channel, "thread_ts": thread_ts, "text": text }))
        .send()
        .await
        .map_err(|e| format!("chat.postMessage request failed: {e}"))?;

    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("chat.postMessage response parse failed: {e}"))?;

    if body.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        Ok(())
    } else {
        Err(format!("chat.postMessage failed: {body}"))
    }
}

async fn send_json(
    client: &reqwest::Client,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let url = format!("{}{}", gizzi_base(), path);
    let mut req = client.request(method, &url);
    if let Some(b) = body {
        req = req.json(&b);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("Gizzi request failed: {e}"))?;
    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Gizzi request failed: {text}"));
    }
    resp.json::<Value>()
        .await
        .map_err(|e| format!("Gizzi response parse failed: {e}"))
}

async fn fetch_json_array(client: &reqwest::Client, path: &str) -> Result<Vec<Value>, String> {
    let url = format!("{}{}", gizzi_base(), path);
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Gizzi request failed: {e}"))?;
    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Gizzi request failed: {text}"));
    }
    resp.json::<Vec<Value>>()
        .await
        .map_err(|e| format!("Gizzi response parse failed: {e}"))
}

#[cfg(test)]
mod binding_tests {
    use super::*;

    fn user(id: &str) -> AuthUser {
        AuthUser {
            user_id: id.into(),
            email: None,
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: None,
            organization_role: None,
            organization_slug: None,
        }
    }

    #[tokio::test]
    async fn a_channel_binds_to_one_bot_its_owner_controls() {
        let dir = std::env::temp_dir().join(format!("allternit-slack-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::test_helpers::app_state(&dir).await;
        {
            let conn = state.db.connect().unwrap();
            for (id, owner) in [("scout", "u"), ("ledger", "u"), ("other", "v")] {
                conn.execute(
                    "INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES (?1, ?2, ?1, 'm', 'p', 1, '{}')",
                    params![id, owner],
                )
                .unwrap();
            }
        }
        let bind = |bot: &str, who: &str, channel: &str| {
            bind_channel(State(state.clone()), Extension(user(who)), Path(bot.to_string()), Json(BindBody { channel: channel.into() }))
        };

        assert_eq!(bind("scout", "u", "#C01ABC").await.status(), StatusCode::CREATED);
        assert_eq!(bound_bot(&state.db, "C01ABC").as_deref(), Some("scout"));
        assert_eq!(bind("scout", "u", "general chat").await.status(), StatusCode::BAD_REQUEST);
        assert_eq!(bind("other", "u", "C02").await.status(), StatusCode::NOT_FOUND, "not your bot");
        assert_eq!(bind("other", "v", "C01ABC").await.status(), StatusCode::CONFLICT, "someone else's channel");
        // Moving a channel between your own bots is fine.
        assert_eq!(bind("ledger", "u", "C01ABC").await.status(), StatusCode::CREATED);
        assert_eq!(bound_bot(&state.db, "C01ABC").as_deref(), Some("ledger"));

        let r = unbind_channel(State(state.clone()), Extension(user("u")), Path(("ledger".into(), "C01ABC".into()))).await;
        assert_eq!(r.status(), StatusCode::NO_CONTENT);
        assert!(bound_bot(&state.db, "C01ABC").is_none());
    }
}
