//! Slack shared app at the cloud (one Allternit Slack app for every user).
//!
//! Today a Slack channel reaches a runtime through the per-user relay
//! (`channel_inbound.rs`, a custom app per workspace). This module is the
//! shared-app model from channel-packs.md: the single Allternit Slack app
//! posts its Events API stream here, and the cloud:
//!
//! 1. verifies Slack's request signature in the edge (`v0:<ts>:<body>` HMAC,
//!    5-minute window) and answers `url_verification` inline;
//! 2. acknowledges everything else within Slack's 3-second budget and queues
//!    the raw event in `slack_event_queue`, keyed by `team_id`;
//! 3. a worker relays queued events to the installing user's runtime
//!    (`slack_installs.runtime_id`, woken if asleep) at the runtime's
//!    shared-app webhook `/webhooks/channels/slack-app`;
//! 4. sends on behalf of bots: `POST /api/v1/channels/slack/send`
//!    (authenticated as the user, like channel-inbound-routes) posts with
//!    `chat.postMessage` and per-bot `username`/`icon_url` (the
//!    `chat:write.customize` scope), so every bot answers under its own name.
//!
//! Install is OAuth v2 (`https://slack.com/oauth/v2/authorize` →
//! `oauth.v2.access`). The per-team bot token is sealed with the platform
//! credential cipher and never leaves this service. App credentials live only
//! in env vars (`SLACK_CLIENT_ID` / `SLACK_CLIENT_SECRET` /
//! `SLACK_SIGNING_SECRET`); when any is unset every route answers 503
//! `{error:"slack_not_configured"}` so callers can fall back — never a crash.
//!
//! Docs (verified 2026-10-02): Events API request URL & url_verification and
//! the 3-second acknowledgement rule — https://docs.slack.dev/apis/events-api/ ;
//! request signing (`X-Slack-Signature`, `X-Slack-Request-Timestamp`) —
//! https://docs.slack.dev/apis/web-api/verifying-requests-from-slack (scheme
//! `v0=<hmac-sha256("v0:" + ts + ":" + raw_body)>`; 5-minute tolerance);
//! OAuth v2 — https://docs.slack.dev/reference/methods/oauth.v2.access/ ;
//! posting — https://docs.slack.dev/reference/methods/chat.postMessage/
//! (`username`/`icon_url` need the `chat:write.customize` scope).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::Engine as _;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;

use super::runtime_relay::{relay_signed_request_to_runtime_with, RelayRequest};
use crate::{auth::resolve_user_scoped, ApiError, ApiState};

type HmacSha256 = Hmac<Sha256>;

/// Runtime path the shared-app events are relayed to (allternit-api
/// `channel_slack_app::slack_app_webhook_router`).
pub const RUNTIME_EVENTS_PATH: &str = "/webhooks/channels/slack-app";
/// Bot scopes the shared app requests at install (mirror of
/// `docs/slack-app-manifest.json`).
pub const SCOPES: &str = "chat:write,chat:write.customize,app_mentions:read,channels:history,groups:history,im:history,im:write,assistant:write,commands,users:read,files:write";
/// A signed OAuth state lives this long.
const STATE_TTL_SECS: i64 = 600;
/// Give up on a queued event this long after it arrived.
const GIVE_UP_AFTER_HOURS: i64 = 24;
const WORKER_INTERVAL: Duration = Duration::from_secs(5);

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/channels/slack/install", get(install_h))
        .route("/api/v1/channels/slack/install/callback", get(install_callback_h))
        .route("/api/v1/channels/slack/events", post(events_h))
        .route("/api/v1/channels/slack/commands", post(commands_h))
        .route("/api/v1/channels/slack/send", post(send_h).layer(axum::extract::DefaultBodyLimit::max(30 * 1024 * 1024)))
        .route("/api/v1/channels/slack/installs", get(list_installs_h))
        .route("/api/v1/channels/slack/installs/:team_id/claim", post(claim_h))
}

// ---------------------------------------------------------------- config + http seam

#[derive(Clone)]
pub struct SlackAppConfig {
    pub client_id: String,
    pub client_secret: String,
    pub signing_secret: String,
}

/// Shared-app credentials: cloud env only, never per-user, never Postgres.
pub fn app_config() -> Option<SlackAppConfig> {
    let client_id = crate::channels::app_env::first(crate::channels::app_env::SLACK_CLIENT_ID);
    let client_secret = crate::channels::app_env::first(crate::channels::app_env::SLACK_CLIENT_SECRET);
    let signing_secret = crate::channels::app_env::first(crate::channels::app_env::SLACK_SIGNING_SECRET);
    match (client_id, client_secret, signing_secret) {
        (Some(client_id), Some(client_secret), Some(signing_secret)) => Some(SlackAppConfig { client_id, client_secret, signing_secret }),
        _ => None,
    }
}

fn not_configured() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "slack_not_configured" }))).into_response()
}

/// Slack Web API calls, behind a seam so tests never touch the network.
#[async_trait]
pub trait SlackHttp: Send + Sync {
    /// POST `application/json`; `bearer` is the bot/user token.
    async fn post_json(&self, url: &str, bearer: Option<&str>, body: &Value) -> Result<(u16, Value), String>;
    /// POST `application/x-www-form-urlencoded`; `basic` is (client_id, client_secret)
    /// per Slack's recommendation to use HTTP Basic for OAuth.
    async fn post_form(&self, url: &str, basic: Option<(&str, &str)>, form: &[(String, String)]) -> Result<(u16, Value), String>;
    /// POST a form with the bot token as bearer (`files.getUploadURLExternal`).
    async fn post_form_bearer(&self, _url: &str, _bearer: &str, _form: &[(String, String)]) -> Result<(u16, Value), String> {
        Err("form upload is not supported by this client".into())
    }
    /// POST raw file bytes to a one-time upload URL Slack handed out.
    async fn post_bytes(&self, _url: &str, _content_type: &str, _bytes: Vec<u8>) -> Result<u16, String> {
        Err("file upload is not supported by this client".into())
    }
}

pub struct ReqwestSlackHttp;

#[async_trait]
impl SlackHttp for ReqwestSlackHttp {
    async fn post_json(&self, url: &str, bearer: Option<&str>, body: &Value) -> Result<(u16, Value), String> {
        let mut req = reqwest::Client::new()
            .post(url)
            .timeout(Duration::from_secs(15))
            .json(body);
        if let Some(token) = bearer {
            req = req.bearer_auth(token);
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let body = resp.json::<Value>().await.unwrap_or(Value::Null);
        Ok((status, body))
    }

    async fn post_form(&self, url: &str, basic: Option<(&str, &str)>, form: &[(String, String)]) -> Result<(u16, Value), String> {
        let mut req = reqwest::Client::new()
            .post(url)
            .timeout(Duration::from_secs(15))
            .form(form);
        if let Some((id, secret)) = basic {
            req = req.basic_auth(id, Some(secret));
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let body = resp.json::<Value>().await.unwrap_or(Value::Null);
        Ok((status, body))
    }

    async fn post_form_bearer(&self, url: &str, bearer: &str, form: &[(String, String)]) -> Result<(u16, Value), String> {
        let resp = reqwest::Client::new().post(url).timeout(Duration::from_secs(15)).bearer_auth(bearer).form(form).send().await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        Ok((status, resp.json::<Value>().await.unwrap_or(Value::Null)))
    }

    async fn post_bytes(&self, url: &str, content_type: &str, bytes: Vec<u8>) -> Result<u16, String> {
        let resp = reqwest::Client::new().post(url).timeout(Duration::from_secs(60)).header("content-type", content_type).body(bytes).send().await.map_err(|e| e.to_string())?;
        Ok(resp.status().as_u16())
    }
}

// ---------------------------------------------------------------- request signing

/// Verify Slack's request signature: `X-Slack-Signature: v0=<hex HmacSha256>`
/// over `v0:{timestamp}:{raw body}`, `X-Slack-Request-Timestamp` within a
/// 5-minute window of `now` (unix seconds). Constant-time compare.
pub fn verify_signature(secret: &str, headers: &HeaderMap, body: &[u8], now: i64) -> Result<(), String> {
    let timestamp = headers
        .get("x-slack-request-timestamp")
        .and_then(|v| v.to_str().ok())
        .ok_or("missing x-slack-request-timestamp header")?;
    let signature = headers
        .get("x-slack-signature")
        .and_then(|v| v.to_str().ok())
        .ok_or("missing x-slack-signature header")?;
    let ts: i64 = timestamp.parse().map_err(|_| "invalid x-slack-request-timestamp")?;
    if (now - ts).abs() > 300 {
        return Err("timestamp outside tolerance (+/-5 min)".to_string());
    }
    let basestring = format!("v0:{timestamp}:{}", String::from_utf8_lossy(body));
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|_| "invalid secret length")?;
    mac.update(basestring.as_bytes());
    let expected = format!("v0={}", hex::encode(mac.finalize().into_bytes()));
    let given = signature.as_bytes();
    let want = expected.as_bytes();
    if given.len() == want.len() && given.iter().zip(want.iter()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0 {
        Ok(())
    } else {
        Err("x-slack-signature mismatch".to_string())
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------- signed OAuth state

fn state_mac(secret: &str) -> HmacSha256 {
    HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac key")
}

/// `base64url(json{u,iat,exp}).base64url(hmac)` — carries the installing user
/// through the browser redirect without trusting caller-supplied fields.
pub fn sign_state(secret: &str, user_id: &str, now: i64) -> String {
    let payload = json!({ "u": user_id, "iat": now, "exp": now + STATE_TTL_SECS });
    let body = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string());
    let mut mac = state_mac(secret);
    mac.update(body.as_bytes());
    let sig = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    format!("{body}.{sig}")
}

/// Returns the user id a state was signed for.
pub fn verify_state(secret: &str, state: &str, now: i64) -> Result<String, String> {
    let (body, sig) = state.split_once('.').ok_or("malformed state")?;
    let mut mac = state_mac(secret);
    mac.update(body.as_bytes());
    let want = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    if sig != want {
        return Err("state signature mismatch".to_string());
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(body)
        .map_err(|_| "state is not base64url")?;
    let payload: Value = serde_json::from_slice(&bytes).map_err(|_| "state is not json")?;
    let exp = payload.get("exp").and_then(Value::as_i64).ok_or("state has no exp")?;
    if now > exp {
        return Err("state expired".to_string());
    }
    payload.get("u").and_then(Value::as_str).map(str::to_string).ok_or_else(|| "state has no user".to_string())
}

// ---------------------------------------------------------------- token sealing

fn seal(state: &ApiState, plaintext: &str) -> String {
    match &state.credential_cipher {
        Some(cipher) => cipher
            .encrypt(plaintext)
            .unwrap_or_else(|_| plaintext.to_string()),
        None => {
            tracing::warn!("Storing Slack bot token PLAINTEXT (ALLTERNIT_CREDENTIALS_KEY unset)");
            plaintext.to_string()
        }
    }
}

fn open(state: &ApiState, stored: &str) -> String {
    match &state.credential_cipher {
        Some(cipher) => cipher.decrypt(stored).unwrap_or_else(|_| stored.to_string()),
        None => stored.to_string(),
    }
}

// ---------------------------------------------------------------- install (OAuth v2)

fn redirect_uri() -> String {
    let base = std::env::var("ALLTERNIT_CLOUD_API_URL").unwrap_or_else(|_| "https://api.allternit.com".to_string());
    format!("{}/api/v1/channels/slack/install/callback", base.trim_end_matches('/'))
}

fn app_base() -> String {
    std::env::var("ALLTERNIT_APP_URL").unwrap_or_else(|_| "https://ai.allternit.com".to_string())
}

async fn install_h(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Response, ApiError> {
    let Some(cfg) = app_config() else { return Ok(not_configured()) };
    let user = resolve_user_scoped(&state.db, &headers, "compute").await?;
    let url = format!(
        "https://slack.com/oauth/v2/authorize?client_id={}&scope={}&redirect_uri={}&state={}",
        urlencoding::encode(&cfg.client_id),
        urlencoding::encode(SCOPES),
        urlencoding::encode(&redirect_uri()),
        sign_state(&cfg.client_secret, &user.id, unix_now()),
    );
    Ok(Json(json!({ "url": url, "configured": true })).into_response())
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

/// Exchange the OAuth code and store the install. Shared by the route and the
/// tests (fake `http`).
pub async fn complete_install(
    state: &ApiState,
    http: &dyn SlackHttp,
    cfg: &SlackAppConfig,
    code: &str,
    user_id: &str,
) -> Result<Value, ApiError> {
    let form = vec![
        ("code".to_string(), code.to_string()),
        ("redirect_uri".to_string(), redirect_uri()),
    ];
    let (status, resp) = http
        .post_form("https://slack.com/api/oauth.v2.access", Some((&cfg.client_id, &cfg.client_secret)), &form)
        .await
        .map_err(|e| ApiError::ServiceUnavailable(format!("Slack OAuth exchange failed: {e}")))?;
    if status != 200 || resp.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(ApiError::ServiceUnavailable(format!(
            "Slack OAuth exchange refused: {}",
            resp.get("error").and_then(Value::as_str).unwrap_or("unknown")
        )));
    }
    let team_id = resp
        .pointer("/team/id")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::ServiceUnavailable("Slack OAuth response has no team.id".to_string()))?
        .to_string();
    let token = resp
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::ServiceUnavailable("Slack OAuth response has no access_token".to_string()))?
        .to_string();
    let expires_at = resp.get("expires_in").and_then(Value::as_i64).map(|secs| {
        chrono::Utc::now() + chrono::Duration::seconds(secs.saturating_sub(60))
    });
    let sealed = seal(state, &token);
    let refresh = resp.get("refresh_token").and_then(Value::as_str).unwrap_or_default();
    sqlx::query(
        "INSERT INTO slack_installs (team_id, team_name, user_id, bot_user_id, app_id, scopes, bot_token, refresh_token, expires_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())
         ON CONFLICT (team_id) DO UPDATE SET
            team_name = EXCLUDED.team_name, user_id = EXCLUDED.user_id, bot_user_id = EXCLUDED.bot_user_id,
            app_id = EXCLUDED.app_id, scopes = EXCLUDED.scopes, bot_token = EXCLUDED.bot_token,
            refresh_token = EXCLUDED.refresh_token, expires_at = EXCLUDED.expires_at, updated_at = NOW()",
    )
    .bind(&team_id)
    .bind(resp.pointer("/team/name").and_then(Value::as_str).unwrap_or_default())
    .bind(user_id)
    .bind(resp.get("bot_user_id").and_then(Value::as_str).unwrap_or_default())
    .bind(resp.get("app_id").and_then(Value::as_str).unwrap_or_default())
    .bind(resp.get("scope").and_then(Value::as_str).unwrap_or_default())
    .bind(&sealed)
    .bind(if refresh.is_empty() { String::new() } else { seal(state, refresh) })
    .bind(expires_at)
    .execute(&state.db)
    .await?;
    Ok(json!({ "ok": true, "teamId": team_id }))
}

async fn install_callback_h(State(state): State<Arc<ApiState>>, Query(q): Query<CallbackQuery>) -> Response {
    let redirect = |params: &str| {
        (
            StatusCode::SEE_OTHER,
            [("location", format!("{}/settings/channels?{}", app_base().trim_end_matches('/'), params))],
        )
            .into_response()
    };
    let Some(cfg) = app_config() else {
        return redirect("slack=error&reason=slack_not_configured");
    };
    if q.error.is_some() {
        return redirect(&format!("slack=error&reason={}", urlencoding::encode(q.error.as_deref().unwrap_or("access_denied"))));
    }
    let (Some(code), Some(state_param)) = (q.code.as_deref(), q.state.as_deref()) else {
        return redirect("slack=error&reason=missing_code");
    };
    let user_id = match verify_state(&cfg.client_secret, state_param, unix_now()) {
        Ok(u) => u,
        Err(_) => return redirect("slack=error&reason=bad_state"),
    };
    match complete_install(&state, &ReqwestSlackHttp, &cfg, code, &user_id).await {
        Ok(v) => redirect(&format!("slack=connected&team={}", v["teamId"].as_str().unwrap_or_default())),
        Err(e) => redirect(&format!("slack=error&reason={}", urlencoding::encode(&e.to_string()))),
    }
}

// ---------------------------------------------------------------- events

/// The raw event envelope Slack POSTed, after the edge checks.
#[derive(Clone, Debug)]
pub struct EventEnvelope {
    pub team_id: String,
    pub api_app_id: String,
    pub event_id: String,
    pub event: Value,
}

/// Edge decisions for a signed Events API payload: the url_verification
/// handshake is answered from the cloud (a runtime may be asleep); everything
/// else is queued for the runtime.
#[derive(Debug)]
pub enum EventsEdge {
    BadSignature,
    BadJson,
    UrlVerification(Value),
    Event(EventEnvelope),
    Ignored,
}

pub fn events_edge(cfg: &SlackAppConfig, headers: &HeaderMap, body: &[u8], now: i64) -> EventsEdge {
    if verify_signature(&cfg.signing_secret, headers, body, now).is_err() {
        return EventsEdge::BadSignature;
    }
    let Ok(payload) = serde_json::from_slice::<Value>(body) else {
        return EventsEdge::BadJson;
    };
    match payload.get("type").and_then(Value::as_str) {
        Some("url_verification") => EventsEdge::UrlVerification(payload.get("challenge").cloned().unwrap_or(Value::Null)),
        Some("event_callback") => {
            let event = payload.get("event").cloned().unwrap_or(Value::Null);
            // Slack's own retry of a failed delivery replays event_id.
            let event_id = event.get("event_id").and_then(Value::as_str).unwrap_or_default().to_string();
            let team_id = event
                .get("team")
                .and_then(Value::as_str)
                .or_else(|| payload.get("team_id").and_then(Value::as_str))
                .unwrap_or_default()
                .to_string();
            if team_id.is_empty() {
                return EventsEdge::Ignored;
            }
            EventsEdge::Event(EventEnvelope {
                team_id,
                api_app_id: payload.get("api_app_id").and_then(Value::as_str).unwrap_or_default().to_string(),
                event_id,
                event,
            })
        }
        _ => EventsEdge::Ignored,
    }
}

/// Queue one event for the installing user's runtime. Returns false when the
/// workspace never installed the app (a removed install still queueing old
/// events is dead within 24h by the worker).
pub async fn enqueue_event(state: &ApiState, env: &EventEnvelope) -> Result<bool, ApiError> {
    enqueue_payload(
        state,
        &env.team_id,
        &env.api_app_id,
        &env.event_id,
        &json!({ "teamId": env.team_id, "apiAppId": env.api_app_id, "eventId": env.event_id, "kind": "event", "event": env.event }),
    )
    .await
}

/// Queue an arbitrary relay payload (an event_callback envelope, or a slash
/// command with `kind: "command"`) for one team.
pub async fn enqueue_payload(state: &ApiState, team_id: &str, api_app_id: &str, event_id: &str, payload: &Value) -> Result<bool, ApiError> {
    let known: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT user_id, runtime_id FROM slack_installs WHERE team_id = $1",
    )
    .bind(team_id)
    .fetch_optional(&state.db)
    .await?;
    if known.is_none() {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO slack_event_queue (team_id, api_app_id, event_id, payload) VALUES ($1, $2, $3, $4)",
    )
    .bind(team_id)
    .bind(api_app_id)
    .bind(event_id)
    .bind(payload)
    .execute(&state.db)
    .await?;
    Ok(true)
}

/// `application/x-www-form-urlencoded` parsed without extra deps.
fn parse_form(body: &[u8]) -> Result<HashMap<String, String>, ()> {
    let text = std::str::from_utf8(body).map_err(|_| ())?;
    let mut out = HashMap::new();
    for pair in text.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').ok_or(())?;
        out.insert(urlencoding::decode(k).map_err(|_| ())?.into_owned(), urlencoding::decode(v).map_err(|_| ())?.replace('+', " "));
    }
    Ok(out)
}

/// Slash commands (`/allternit`, manifest `features.slash_commands`) POST
/// form-encoded, signed like every other Slack request. Slack expects the
/// 200 within 3 seconds; the command is queued like an event with
/// `kind: "command"` and the runtime turns it into one thread per run.
async fn commands_h(State(state): State<Arc<ApiState>>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(cfg) = app_config() else { return not_configured() };
    if verify_signature(&cfg.signing_secret, &headers, &body, unix_now()).is_err() {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "invalid_signature" }))).into_response();
    }
    let Ok(form) = parse_form(&body) else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_form" }))).into_response();
    };
    let team_id = form.get("team_id").cloned().unwrap_or_default();
    if team_id.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "missing team_id" }))).into_response();
    }
    let trigger = form.get("trigger_id").cloned().unwrap_or_default();
    let api_app_id = form.get("api_app_id").cloned().unwrap_or_default();
    let payload = json!({
        "teamId": team_id,
        "apiAppId": api_app_id,
        "eventId": format!("cmd:{trigger}"),
        "kind": "command",
        "event": form,
    });
    let state2 = state.clone();
    tokio::spawn(async move {
        match enqueue_payload(&state2, &team_id, &api_app_id, &format!("cmd:{trigger}"), &payload).await {
            Ok(true) => {
                let state3 = state2.clone();
                tokio::spawn(async move {
                    if let Err(e) = deliver_team(&state3, &team_id).await {
                        tracing::warn!(%team_id, "slack command delivery pass failed: {e}");
                    }
                });
            }
            Ok(false) => tracing::warn!(%team_id, "slack command from an unknown team dropped"),
            Err(e) => tracing::warn!("slack command enqueue failed: {e}"),
        }
    });
    // An empty 200: the real answer arrives as a normal bot message.
    StatusCode::OK.into_response()
}

async fn events_h(State(state): State<Arc<ApiState>>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(cfg) = app_config() else { return not_configured() };
    match events_edge(&cfg, &headers, &body, unix_now()) {
        EventsEdge::BadSignature => (StatusCode::UNAUTHORIZED, Json(json!({ "error": "invalid_signature" }))).into_response(),
        EventsEdge::BadJson => (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_json" }))).into_response(),
        EventsEdge::UrlVerification(challenge) => Json(json!({ "challenge": challenge })).into_response(),
        EventsEdge::Ignored => Json(json!({ "ok": true })).into_response(),
        EventsEdge::Event(env) => {
            // Ack inside Slack's 3-second budget; deliver in the background.
            let state2 = state.clone();
            tokio::spawn(async move {
                match enqueue_event(&state2, &env).await {
                    Ok(true) => {
                        let state3 = state2.clone();
                        let team = env.team_id.clone();
                        tokio::spawn(async move {
                            if let Err(e) = deliver_team(&state3, &team).await {
                                tracing::warn!(%team, "slack event delivery pass failed: {e}");
                            }
                        });
                    }
                    Ok(false) => tracing::warn!(team = %env.team_id, "slack event for an unknown team dropped"),
                    Err(e) => tracing::warn!("slack event enqueue failed: {e}"),
                }
            });
            Json(json!({ "ok": true })).into_response()
        }
    }
}

/// Relay payload for one queued event, delivered on the trusted cloud→runtime
/// channel (the runtime re-checks the team against its own connection).
fn runtime_payload(env: &Value) -> String {
    env.to_string()
}

/// Deliver one team's queued events oldest first, waking the runtime when it
/// sleeps. Retries with the shared channel backoff for 24 hours.
pub async fn deliver_team(state: &ApiState, team_id: &str) -> Result<(), ApiError> {
    loop {
        let claimed: Option<(i64, Value, chrono::DateTime<chrono::Utc>, i32)> = sqlx::query_as(
            "UPDATE slack_event_queue SET locked_until = NOW() + interval '3 minutes', attempts = attempts + 1
              WHERE id = (
                SELECT id FROM slack_event_queue
                 WHERE team_id = $1 AND delivered_at IS NULL AND dead_at IS NULL
                 ORDER BY id LIMIT 1 FOR UPDATE SKIP LOCKED)
                AND next_attempt_at <= NOW() AND (locked_until IS NULL OR locked_until < NOW())
              RETURNING id, payload, received_at, attempts",
        )
        .bind(team_id)
        .fetch_optional(&state.db)
        .await?;
        let Some((id, payload, received_at, attempts)) = claimed else {
            return Ok(());
        };
        let install: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT user_id, runtime_id FROM slack_installs WHERE team_id = $1",
        )
        .bind(team_id)
        .fetch_optional(&state.db)
        .await?;
        let Some((user_id, Some(runtime_id))) = install else {
            let give_up = chrono::Utc::now() - received_at > chrono::Duration::hours(GIVE_UP_AFTER_HOURS);
            sqlx::query(
                "UPDATE slack_event_queue SET locked_until = NULL, last_error = $2,
                    next_attempt_at = NOW() + make_interval(secs => $3),
                    dead_at = CASE WHEN $4 THEN NOW() ELSE NULL END
                  WHERE id = $1",
            )
            .bind(id)
            .bind("install or runtime not claimed")
            .bind(super::channel_inbound::backoff_secs(attempts) as f64)
            .bind(give_up)
            .execute(&state.db)
            .await?;
            if give_up {
                continue;
            }
            return Ok(());
        };
        let outcome = relay_signed_request_to_runtime_with(
            &state.db,
            &state.contabo_runtime_service,
            &state.quota_service,
            &state.provisioning_service,
            &user_id,
            &runtime_id,
            RelayRequest {
                method: "POST".to_string(),
                path: RUNTIME_EVENTS_PATH.to_string(),
                headers: HashMap::from([("content-type".to_string(), "application/json".to_string())]),
                body: base64::engine::general_purpose::STANDARD.encode(runtime_payload(&payload)),
                body_encoding: "base64".to_string(),
            },
            &["content-type"],
            HashMap::new(),
        )
        .await;
        let (status, error) = match &outcome {
            Ok(response) => (Some(response.status().as_u16()), None),
            Err(e) => (None, Some(e.to_string())),
        };
        let done = status.map(super::channel_inbound::classify) == Some(super::channel_inbound::Delivery::Done);
        if done {
            sqlx::query("UPDATE slack_event_queue SET delivered_at = NOW(), locked_until = NULL, last_status = $2 WHERE id = $1")
                .bind(id)
                .bind(status.map(i32::from))
                .execute(&state.db)
                .await?;
            continue;
        }
        let give_up = chrono::Utc::now() - received_at > chrono::Duration::hours(GIVE_UP_AFTER_HOURS);
        sqlx::query(
            "UPDATE slack_event_queue SET locked_until = NULL, last_status = $2, last_error = $3,
                next_attempt_at = NOW() + make_interval(secs => $4), dead_at = CASE WHEN $5 THEN NOW() ELSE NULL END
              WHERE id = $1",
        )
        .bind(id)
        .bind(status.map(i32::from))
        .bind(error)
        .bind(super::channel_inbound::backoff_secs(attempts) as f64)
        .bind(give_up)
        .execute(&state.db)
        .await?;
        if give_up {
            continue;
        }
        return Ok(());
    }
}

/// Background loop: keep delivering queued events (retry pass + waking
/// runtimes). Started from `main`/`lib` next to the channel inbound worker.
pub fn start_slack_event_worker(state: Arc<ApiState>) {
    tokio::spawn(async move {
        let mut ticks: u64 = 0;
        loop {
            match sqlx::query_scalar::<_, String>(
                "SELECT DISTINCT team_id FROM slack_event_queue WHERE delivered_at IS NULL AND dead_at IS NULL
                  AND next_attempt_at <= NOW() AND (locked_until IS NULL OR locked_until < NOW()) LIMIT 50",
            )
            .fetch_all(&state.db)
            .await
            {
                Ok(teams) => {
                    for team in teams {
                        let state = state.clone();
                        tokio::spawn(async move {
                            if let Err(e) = deliver_team(&state, &team).await {
                                tracing::warn!(%team, "slack event delivery pass failed: {e}");
                            }
                        });
                    }
                }
                Err(e) => tracing::warn!("slack event worker: {e}"),
            }
            ticks += 1;
            if ticks % 720 == 0 {
                let _ = sqlx::query(
                    "DELETE FROM slack_event_queue WHERE (delivered_at IS NOT NULL AND delivered_at < NOW() - interval '7 days')
                        OR (dead_at IS NOT NULL AND dead_at < NOW() - interval '7 days')",
                )
                .execute(&state.db)
                .await;
            }
            tokio::time::sleep(WORKER_INTERVAL).await;
        }
    });
}

// ---------------------------------------------------------------- installs list + claim

async fn list_installs_h(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Response, ApiError> {
    if app_config().is_none() {
        return Ok(not_configured());
    }
    let user = resolve_user_scoped(&state.db, &headers, "compute").await?;
    let rows: Vec<(String, String, String, chrono::DateTime<chrono::Utc>, Option<String>)> = sqlx::query_as(
        "SELECT team_id, team_name, bot_user_id, installed_at, runtime_id FROM slack_installs WHERE user_id = $1 ORDER BY installed_at",
    )
    .bind(&user.id)
    .fetch_all(&state.db)
    .await?;
    let installs: Vec<Value> = rows
        .into_iter()
        .map(|(team_id, team_name, bot_user_id, installed_at, runtime_id)| {
            json!({ "teamId": team_id, "teamName": team_name, "botUserId": bot_user_id, "installedAt": installed_at, "runtimeId": runtime_id })
        })
        .collect();
    Ok(Json(json!({ "installs": installs })).into_response())
}

#[derive(Deserialize)]
pub struct ClaimBody {
    pub runtime_id: String,
}

/// The user's runtime claims its team's events (called by the runtime's Slack
/// connect flow, after the OAuth install happened in the browser).
pub async fn claim_install(state: &ApiState, user_id: &str, team_id: &str, runtime_id: &str) -> Result<(), ApiError> {
    let owns: Option<(String,)> = sqlx::query_as(
        "SELECT id FROM runtime_devices WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(runtime_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?;
    if owns.is_none() {
        return Err(ApiError::NotFound("Runtime not found".to_string()));
    }
    let res = sqlx::query("UPDATE slack_installs SET runtime_id = $1, updated_at = NOW() WHERE team_id = $2 AND user_id = $3")
        .bind(runtime_id)
        .bind(team_id)
        .bind(user_id)
        .execute(&state.db)
        .await?;
    if res.rows_affected() == 0 {
        return Err(ApiError::NotFound("Slack install not found".to_string()));
    }
    Ok(())
}

async fn claim_h(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(team_id): Path<String>,
    Json(body): Json<ClaimBody>,
) -> Result<Response, ApiError> {
    if app_config().is_none() {
        return Ok(not_configured());
    }
    let user = resolve_user_scoped(&state.db, &headers, "compute").await?;
    claim_install(&state, &user.id, &team_id, &body.runtime_id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ---------------------------------------------------------------- send

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SendBody {
    pub channel: String,
    pub text: String,
    pub thread_ts: Option<String>,
    pub username: Option<String>,
    pub icon_url: Option<String>,
    /// Files to upload into the message's thread after it posts.
    #[serde(default)]
    pub files: Vec<SendFile>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SendFile {
    pub filename: String,
    #[serde(default)]
    pub mime_type: Option<String>,
    pub data_base64: String,
}

/// Same caps the runtime checks (`channel_files`), enforced again here: this route is reachable with any user token.
const MAX_FILES: usize = 5;
const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;
const MAX_TOTAL_FILE_BYTES: usize = 20 * 1024 * 1024;

/// Build the `chat.postMessage` body. Per-bot name/icon ride `username` /
/// `icon_url` (the shared app's `chat:write.customize` scope) so every bot
/// answers under its own identity.
pub fn build_post_message(b: &SendBody) -> Value {
    let mut body = json!({ "channel": b.channel, "text": b.text });
    if let Some(t) = &b.thread_ts {
        body["thread_ts"] = json!(t);
    }
    if let Some(u) = &b.username {
        body["username"] = json!(u);
    }
    if let Some(i) = &b.icon_url {
        body["icon_url"] = json!(i);
    }
    body
}

struct InstallToken {
    token: String,
    refresh_token: String,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

async fn install_token(state: &ApiState, team_id: &str) -> Result<Option<InstallToken>, ApiError> {
    let row: Option<(String, String, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT bot_token, refresh_token, expires_at FROM slack_installs WHERE team_id = $1",
    )
    .bind(team_id)
    .fetch_optional(&state.db)
    .await?;
    Ok(row.map(|(token, refresh, exp)| InstallToken {
        token: open(state, &token),
        refresh_token: open(state, &refresh),
        expires_at: exp,
    }))
}

/// Refresh an expiring token when Slack issued a refresh token (token
/// rotation). Best effort: on any failure the current token is used as-is.
async fn maybe_refresh(state: &ApiState, http: &dyn SlackHttp, cfg: &SlackAppConfig, team_id: &str, token: &InstallToken) {
    let needs = token.expires_at.is_some_and(|exp| exp < chrono::Utc::now()) && !token.refresh_token.is_empty();
    if !needs {
        return;
    }
    let form = vec![
        ("grant_type".to_string(), "refresh_token".to_string()),
        ("refresh_token".to_string(), token.refresh_token.clone()),
    ];
    let Ok((200, resp)) = http
        .post_form("https://slack.com/api/oauth.v2.access", Some((&cfg.client_id, &cfg.client_secret)), &form)
        .await
    else {
        return;
    };
    if resp.get("ok").and_then(Value::as_bool) != Some(true) {
        return;
    }
    let Some(new_token) = resp.get("access_token").and_then(Value::as_str) else { return };
    let expires_at = resp.get("expires_in").and_then(Value::as_i64).map(|secs| {
        chrono::Utc::now() + chrono::Duration::seconds(secs.saturating_sub(60))
    });
    let new_refresh = resp.get("refresh_token").and_then(Value::as_str).unwrap_or(&token.refresh_token).to_string();
    let _ = sqlx::query(
        "UPDATE slack_installs SET bot_token = $2, refresh_token = $3, expires_at = $4, updated_at = NOW() WHERE team_id = $1",
    )
    .bind(team_id)
    .bind(seal(state, new_token))
    .bind(seal(state, &new_refresh))
    .bind(expires_at)
    .execute(&state.db)
    .await;
}

/// POST one message as the user's install. Shared by the route and tests.
pub async fn send_message(state: &ApiState, http: &dyn SlackHttp, cfg: &SlackAppConfig, user_id: &str, body: &SendBody) -> Result<Value, ApiError> {
    let files = decode_files(&body.files)?;
    let team: Option<(String,)> = sqlx::query_as(
        "SELECT team_id FROM slack_installs WHERE user_id = $1 ORDER BY installed_at DESC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((team_id,)) = team else {
        return Err(ApiError::BadRequest("slack_not_installed".to_string()));
    };
    let Some(mut token) = install_token(state, &team_id).await? else {
        return Err(ApiError::BadRequest("slack_not_installed".to_string()));
    };
    maybe_refresh(state, http, cfg, &team_id, &token).await;
    if let Some(fresh) = install_token(state, &team_id).await? {
        token = fresh;
    }
    let (status, resp) = http
        .post_json("https://slack.com/api/chat.postMessage", Some(&token.token), &build_post_message(body))
        .await
        .map_err(|e| ApiError::ServiceUnavailable(format!("chat.postMessage failed: {e}")))?;
    match (status, resp.get("ok").and_then(Value::as_bool), resp.get("ts").and_then(Value::as_str)) {
        (200..=299, Some(true), Some(ts)) => {
            if !files.is_empty() {
                let root = body.thread_ts.as_deref().unwrap_or(ts);
                let channel = resp.get("channel").and_then(Value::as_str).unwrap_or(&body.channel);
                upload_files(http, &token.token, channel, root, &files).await.map_err(|e| ApiError::ServiceUnavailable(format!("slack_file_upload_failed: {e} (the message itself was posted, ts {ts})")))?;
            }
            Ok(json!({ "ok": true, "ts": ts, "teamId": team_id, "files": files.len() }))
        }
        (429, _, _) => Err(ApiError::TooManyRequests(resp.get("error").and_then(Value::as_str).unwrap_or("ratelimited").to_string())),
        _ => Err(ApiError::ServiceUnavailable(format!(
            "chat.postMessage failed: {}",
            resp.get("error").and_then(Value::as_str).unwrap_or("unknown")
        ))),
    }
}

/// Decode and cap the files before anything is posted.
fn decode_files(files: &[SendFile]) -> Result<Vec<(String, String, Vec<u8>)>, ApiError> {
    use base64::Engine as _;
    if files.len() > MAX_FILES {
        return Err(ApiError::BadRequest("too_many_attachments".to_string()));
    }
    let mut total = 0usize;
    let mut out = Vec::with_capacity(files.len());
    for f in files {
        if f.data_base64.len() / 4 * 3 > MAX_FILE_BYTES + 3 {
            return Err(ApiError::BadRequest("attachment_too_large".to_string()));
        }
        let compact: String = f.data_base64.chars().filter(|c| !c.is_whitespace()).collect();
        let bytes = base64::engine::general_purpose::STANDARD.decode(compact.as_bytes()).map_err(|_| ApiError::BadRequest("invalid_attachment".to_string()))?;
        total += bytes.len();
        if bytes.is_empty() || bytes.len() > MAX_FILE_BYTES || total > MAX_TOTAL_FILE_BYTES {
            return Err(ApiError::BadRequest("attachment_too_large".to_string()));
        }
        let name: String = f.filename.rsplit(['/', '\\']).next().unwrap_or("attachment").chars().filter(|c| !c.is_control()).take(120).collect();
        out.push((if name.trim().is_empty() { "attachment".to_string() } else { name }, f.mime_type.clone().filter(|m| !m.is_empty()).unwrap_or_else(|| "application/octet-stream".into()), bytes));
    }
    Ok(out)
}

/// Slack's external upload flow: get an upload URL per file, send the bytes, then complete once
/// for all files into the thread. https://api.slack.com/messaging/files#uploading_files
async fn upload_files(http: &dyn SlackHttp, token: &str, channel: &str, thread_ts: &str, files: &[(String, String, Vec<u8>)]) -> Result<(), String> {
    let mut done = vec![];
    for (name, mime, bytes) in files {
        let form = vec![("filename".to_string(), name.clone()), ("length".to_string(), bytes.len().to_string())];
        let (status, resp) = http.post_form_bearer("https://slack.com/api/files.getUploadURLExternal", token, &form).await?;
        let (Some(url), Some(file_id)) = (resp.get("upload_url").and_then(Value::as_str), resp.get("file_id").and_then(Value::as_str)) else {
            return Err(format!("files.getUploadURLExternal {status}: {}", resp.get("error").and_then(Value::as_str).unwrap_or("no upload url")));
        };
        let put = http.post_bytes(url, mime, bytes.clone()).await?;
        if !(200..300).contains(&put) {
            return Err(format!("uploading {name} returned {put}"));
        }
        done.push(json!({ "id": file_id, "title": name }));
    }
    let (_, resp) = http
        .post_json("https://slack.com/api/files.completeUploadExternal", Some(token), &json!({ "files": done, "channel_id": channel, "thread_ts": thread_ts }))
        .await?;
    if resp.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(format!("files.completeUploadExternal: {}", resp.get("error").and_then(Value::as_str).unwrap_or("unknown")))
    }
}

async fn send_h(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<SendBody>) -> Result<Response, ApiError> {
    let Some(cfg) = app_config() else { return Ok(not_configured()) };
    let user = resolve_user_scoped(&state.db, &headers, "compute").await?;
    let result = send_message(&state, &ReqwestSlackHttp, &cfg, &user.id, &body).await?;
    Ok(Json(result).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const SECRET: &str = "test-signing-secret";

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), v.parse().unwrap());
        }
        h
    }

    fn signed(secret: &str, ts: i64, body: &str) -> HeaderMap {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(format!("v0:{ts}:{body}").as_bytes());
        headers(&[
            ("x-slack-request-timestamp", &ts.to_string()),
            ("x-slack-signature", &format!("v0={}", hex::encode(mac.finalize().into_bytes()))),
        ])
    }

    #[test]
    fn a_fresh_signed_request_verifies_and_a_replayed_one_does_not() {
        let body = r#"{"type":"url_verification","challenge":"c"}"#;
        let now = 1_700_000_000;
        assert!(verify_signature(SECRET, &signed(SECRET, now, body), body.as_bytes(), now).is_ok());
        assert!(verify_signature(SECRET, &signed(SECRET, now - 301, body), body.as_bytes(), now).is_err(), "older than 5 minutes");
        assert!(verify_signature(SECRET, &signed(SECRET, now + 301, body), body.as_bytes(), now).is_err(), "from the future");
        assert!(verify_signature("other", &signed(SECRET, now, body), body.as_bytes(), now).is_err(), "wrong secret");
        assert!(verify_signature(SECRET, &signed(SECRET, now, body), b"{}", now).is_err(), "tampered body");
        assert!(verify_signature(SECRET, &headers(&[]), body.as_bytes(), now).is_err(), "missing headers");
    }

    #[test]
    fn url_verification_is_answered_at_the_edge_and_events_are_queued() {
        let cfg = SlackAppConfig { client_id: "c".into(), client_secret: SECRET.into(), signing_secret: SECRET.into() };
        let now = 1_700_000_000;
        let challenge = r#"{"type":"url_verification","challenge":"abc123"}"#;
        match events_edge(&cfg, &signed(SECRET, now, challenge), challenge.as_bytes(), now) {
            EventsEdge::UrlVerification(c) => assert_eq!(c, json!("abc123")),
            other => panic!("expected url_verification, got {other:?}"),
        }
        let event = r#"{"type":"event_callback","team_id":"T1","api_app_id":"A1","event":{"type":"message","team":"T1","channel":"C1","user":"U1","text":"hi","ts":"1700000001.000100"}}"#;
        match events_edge(&cfg, &signed(SECRET, now, event), event.as_bytes(), now) {
            EventsEdge::Event(env) => {
                assert_eq!((env.team_id.as_str(), env.api_app_id.as_str(), env.event["text"].as_str()), ("T1", "A1", Some("hi")));
            }
            other => panic!("expected event, got {other:?}"),
        }
        // A replayed timestamp and an unsigned body never reach the queue.
        assert!(matches!(events_edge(&cfg, &signed(SECRET, now - 900, event), event.as_bytes(), now), EventsEdge::BadSignature));
        assert!(matches!(events_edge(&cfg, &headers(&[]), event.as_bytes(), now), EventsEdge::BadSignature));
        // Unknown envelope types get a plain ack.
        let other = r#"{"type":"app_rate_limited"}"#;
        assert!(matches!(events_edge(&cfg, &signed(SECRET, now, other), other.as_bytes(), now), EventsEdge::Ignored));
    }

    #[test]
    fn oauth_state_roundtrips_and_cannot_be_forged_or_replayed() {
        let now = 1_700_000_000;
        let state = sign_state(SECRET, "user-1", now);
        assert_eq!(verify_state(SECRET, &state, now).unwrap(), "user-1");
        assert_eq!(verify_state(SECRET, &state, now + 599).unwrap(), "user-1");
        assert!(verify_state(SECRET, &state, now + 601).is_err(), "expired");
        // The body is base64 JSON, so tamper with an encoded byte, not the plain user id.
        let mut tampered = state.clone();
        let first = if tampered.starts_with('A') { "B" } else { "A" };
        tampered.replace_range(0..1, first);
        assert!(verify_state(SECRET, &tampered, now).is_err(), "tampered body");
        assert!(verify_state("other", &state, now).is_err(), "wrong secret");
        assert!(verify_state(SECRET, "nonsense", now).is_err());
    }

    #[derive(Default)]
    struct FakeHttp {
        calls: Mutex<Vec<(String, Value)>>,
        reply: Mutex<Option<(u16, Value)>>,
        form: Mutex<Vec<(String, String)>>,
        /// (url, bearer, form) of each bearer form post, and (url, content type, length) of each byte upload.
        bearer_forms: Mutex<Vec<(String, String, Vec<(String, String)>)>>,
        byte_uploads: Mutex<Vec<(String, String, usize)>>,
    }

    #[async_trait]
    impl SlackHttp for FakeHttp {
        async fn post_form_bearer(&self, url: &str, bearer: &str, form: &[(String, String)]) -> Result<(u16, Value), String> {
            self.bearer_forms.lock().unwrap().push((url.into(), bearer.into(), form.to_vec()));
            let n = self.bearer_forms.lock().unwrap().len();
            Ok((200, json!({ "ok": true, "upload_url": format!("https://files.slack.test/upload/{n}"), "file_id": format!("F{n}") })))
        }
        async fn post_bytes(&self, url: &str, content_type: &str, bytes: Vec<u8>) -> Result<u16, String> {
            self.byte_uploads.lock().unwrap().push((url.into(), content_type.into(), bytes.len()));
            Ok(200)
        }
        async fn post_json(&self, url: &str, _bearer: Option<&str>, body: &Value) -> Result<(u16, Value), String> {
            self.calls.lock().unwrap().push((url.to_string(), body.clone()));
            Ok(self.reply.lock().unwrap().clone().unwrap_or((200, json!({ "ok": true }))))
        }
        async fn post_form(&self, url: &str, _basic: Option<(&str, &str)>, form: &[(String, String)]) -> Result<(u16, Value), String> {
            self.form.lock().unwrap().extend(form.iter().cloned());
            Ok(self.reply.lock().unwrap().clone().unwrap_or((200, json!({ "ok": true }))))
        }
    }

    /// Minimal ApiState (schema-per-test pg) with the 026 schema applied —
    /// the migration file itself is the DDL source, so a schema drift fails here.
    async fn test_state() -> Arc<ApiState> {
        let state = crate::routes::test_support::test_state(Arc::new(
            crate::routes::test_support::MockGateway::new(Some(crate::routes::test_support::MockGateway::healthy_node()), vec![]),
        ))
        .await;
        sqlx::raw_sql(include_str!("../../migrations_pg/026_slack_installs.sql"))
            .execute(&state.db)
            .await
            .unwrap();
        state
    }

    fn send_file(name: &str, bytes: &[u8]) -> SendFile {
        use base64::Engine as _;
        SendFile { filename: name.into(), mime_type: Some("application/pdf".into()), data_base64: base64::engine::general_purpose::STANDARD.encode(bytes) }
    }

    #[tokio::test]
    async fn files_upload_through_slacks_external_flow_into_the_thread() {
        let http = FakeHttp::default();
        let files = decode_files(&[send_file("../r.pdf", b"%PDF-1"), send_file("b.pdf", b"%PDF-22")]).unwrap();
        assert_eq!(files[0].0, "r.pdf", "path parts are dropped from the name");
        upload_files(&http, "xoxb-1", "C1", "1700000001.000100", &files).await.unwrap();
        let forms = http.bearer_forms.lock().unwrap().clone();
        assert_eq!(forms[0].0, "https://slack.com/api/files.getUploadURLExternal");
        assert_eq!(forms[0].1, "xoxb-1");
        assert_eq!(forms[0].2, vec![("filename".to_string(), "r.pdf".to_string()), ("length".to_string(), "6".to_string())]);
        assert_eq!(*http.byte_uploads.lock().unwrap(), vec![("https://files.slack.test/upload/1".to_string(), "application/pdf".to_string(), 6), ("https://files.slack.test/upload/2".to_string(), "application/pdf".to_string(), 7)]);
        let (url, body) = http.calls.lock().unwrap().last().unwrap().clone();
        assert_eq!(url, "https://slack.com/api/files.completeUploadExternal");
        assert_eq!(body, json!({ "files": [{ "id": "F1", "title": "r.pdf" }, { "id": "F2", "title": "b.pdf" }], "channel_id": "C1", "thread_ts": "1700000001.000100" }));
        // Slack refusing the completion is an error the route reports.
        *http.reply.lock().unwrap() = Some((200, json!({ "ok": false, "error": "channel_not_found" })));
        assert!(upload_files(&http, "xoxb-1", "C9", "1.1", &files).await.unwrap_err().contains("channel_not_found"));
    }

    #[test]
    fn files_are_capped_and_checked_before_anything_posts() {
        assert!(decode_files(&[]).unwrap().is_empty());
        let six: Vec<SendFile> = (0..6).map(|i| send_file(&format!("{i}.pdf"), b"x")).collect();
        assert!(decode_files(&six).is_err());
        assert!(decode_files(&[SendFile { filename: "a".into(), mime_type: None, data_base64: "!!!".into() }]).is_err());
        assert!(decode_files(&[send_file("empty", b"")]).is_err());
        assert!(decode_files(&[send_file("big", &vec![0u8; MAX_FILE_BYTES + 1])]).is_err());
        assert_eq!(decode_files(&[SendFile { filename: "  ".into(), mime_type: None, data_base64: "aGk=".into() }]).unwrap()[0].1, "application/octet-stream");
    }

    #[test]
    fn chat_post_message_carries_the_bot_identity_and_thread() {
        let body = SendBody {
            channel: "C1".into(),
            text: "hello".into(),
            thread_ts: Some("1700000001.000100".into()),
            username: Some("Scout".into()),
            icon_url: Some("https://cdn.example/avatar.png".into()),
            files: vec![],
        };
        assert_eq!(
            build_post_message(&body),
            json!({ "channel": "C1", "text": "hello", "thread_ts": "1700000001.000100", "username": "Scout", "icon_url": "https://cdn.example/avatar.png" })
        );
        let bare = SendBody { thread_ts: None, username: None, icon_url: None, ..body };
        let built = build_post_message(&bare);
        assert!(built.get("thread_ts").is_none() && built.get("username").is_none() && built.get("icon_url").is_none());
    }

    #[tokio::test]
    async fn oauth_exchange_stores_the_install_sealed() {
        let state = test_state().await;
        let http = FakeHttp::default();
        *http.reply.lock().unwrap() = Some((
            200,
            json!({ "ok": true, "access_token": "xoxb-1", "token_type": "bot", "scope": "chat:write,commands",
                "bot_user_id": "UBOT", "app_id": "A1", "team": { "name": "Acme", "id": "T1" },
                "authed_user": { "id": "U1" } }),
        ));
        let cfg = SlackAppConfig { client_id: "c".into(), client_secret: SECRET.into(), signing_secret: SECRET.into() };
        let result = complete_install(&state, &http, &cfg, "the-code", "user-1").await.unwrap();
        assert_eq!(result["teamId"], "T1");
        assert_eq!(
            http.form.lock().unwrap().clone(),
            vec![("code".to_string(), "the-code".to_string()), ("redirect_uri".to_string(), redirect_uri())]
        );
        let (user, token): (String, String) = sqlx::query_as("SELECT user_id, bot_token FROM slack_installs WHERE team_id = 'T1'")
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!(user, "user-1");
        assert_eq!(open(&state, &token), "xoxb-1", "token round-trips through the cipher (or plaintext in dev)");
        // Re-installing the same team replaces the row, not duplicates it.
        complete_install(&state, &http, &cfg, "the-code", "user-1").await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM slack_installs WHERE team_id = 'T1'")
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn events_queue_by_team_and_send_uses_the_install() {
        let state = test_state().await;
        let http = FakeHttp::default();
        *http.reply.lock().unwrap() = Some((
            200,
            json!({ "ok": true, "access_token": "xoxb-1", "team": { "name": "Acme", "id": "T1" }, "bot_user_id": "UBOT" }),
        ));
        let cfg = SlackAppConfig { client_id: "c".into(), client_secret: SECRET.into(), signing_secret: SECRET.into() };
        complete_install(&state, &http, &cfg, "code", "user-1").await.unwrap();

        let env = EventEnvelope {
            team_id: "T1".into(),
            api_app_id: "A1".into(),
            event_id: "Ev1".into(),
            event: json!({ "type": "message", "channel": "C1", "text": "hi", "ts": "1700000001.000100" }),
        };
        assert!(enqueue_event(&state, &env).await.unwrap());
        let unknown = EventEnvelope { team_id: "TNOPE".into(), ..env.clone() };
        assert!(!enqueue_event(&state, &unknown).await.unwrap(), "unknown teams are not queued");

        // send: bot identity rides the post; Slack's ts comes back.
        *http.reply.lock().unwrap() = Some((200, json!({ "ok": true, "channel": "C1", "ts": "1700000002.000200" })));
        let sent = send_message(
            &state,
            &http,
            &cfg,
            "user-1",
            &SendBody { channel: "C1".into(), text: "hi".into(), thread_ts: Some("1700000001.000100".into()), username: Some("Scout".into()), icon_url: None, files: vec![] },
        )
        .await
        .unwrap();
        assert_eq!(sent["ts"], "1700000002.000200");
        let (url, body) = http.calls.lock().unwrap().clone().remove(0);
        assert_eq!(url, "https://slack.com/api/chat.postMessage");
        assert_eq!(body["username"], "Scout");
        assert_eq!(body["thread_ts"], "1700000001.000100");

        // Slack saying no is a definite rejection, surfaced as an error.
        *http.reply.lock().unwrap() = Some((200, json!({ "ok": false, "error": "channel_not_found" })));
        let err = send_message(&state, &http, &cfg, "user-1", &SendBody { channel: "C1".into(), text: "x".into(), thread_ts: None, username: None, icon_url: None, files: vec![] }).await.unwrap_err();
        assert!(err.to_string().contains("channel_not_found"), "{err}");
        // A user with no install gets a clear 400.
        assert!(send_message(&state, &http, &cfg, "user-2", &SendBody { channel: "C1".into(), text: "x".into(), thread_ts: None, username: None, icon_url: None, files: vec![] }).await.is_err());
    }
}
