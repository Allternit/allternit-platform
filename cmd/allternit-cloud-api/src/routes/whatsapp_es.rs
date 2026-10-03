//! WhatsApp business number through Meta's Embedded Signup.
//!
//! Allternit is the Tech Provider for each user's own business number. Meta
//! bans general-purpose assistants on a shared number (2025-10-15), so there is
//! no Allternit-owned assistant number here.
//!
//! * `POST /api/v1/channels/whatsapp/embedded-signup/complete` exchanges the
//!   Embedded Signup code, points the WABA's webhooks at the owner's relay
//!   address (`override_callback_uri`) and registers the number.
//! * Meta's GET verify handshake and the `X-Hub-Signature-256` check run here,
//!   before anything is queued. The runtime never sees `META_APP_SECRET`: the
//!   cloud re-signs the body with a per-address relay secret, which the
//!   runtime stores as the account's `appSecret`.
//! * `POST /api/v1/channels/whatsapp/send` enforces the 24-hour window.
//!
//! Env: `META_APP_ID`, `META_APP_SECRET`, `META_ES_CONFIG_ID`, `META_SYSTEM_TOKEN`.
//! Unset → 503 `{error:"whatsapp_not_configured"}`.
//!
//! Docs (Graph API v25.0):
//! * https://developers.facebook.com/docs/whatsapp/embedded-signup/onboarding-customers-as-a-tech-provider
//! * https://developers.facebook.com/documentation/business-messaging/whatsapp/webhooks/override
//! * https://developers.facebook.com/docs/whatsapp/cloud-api/reference/registration
//! * https://developers.facebook.com/docs/whatsapp/cloud-api/guides/send-messages

use async_trait::async_trait;
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, Mac};
use rand::Rng;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;

use super::channel_inbound::{new_key, public_base, sha256_hex};
use crate::{ApiError, ApiState};

type HmacSha256 = Hmac<Sha256>;

pub const GRAPH_BASE: &str = "https://graph.facebook.com/v25.0";
/// Meta's customer service window.
pub const WINDOW_HOURS: i64 = 24;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/channels/whatsapp/embedded-signup/complete", post(complete))
        .route("/api/v1/channels/whatsapp/send", post(send))
}

// ------------------------------------------------------------------ config

#[derive(Clone)]
pub struct MetaConfig {
    pub app_id: String,
    pub app_secret: String,
    pub config_id: String,
    pub system_token: Option<String>,
}

impl MetaConfig {
    pub fn from_env() -> Option<Self> {
        use crate::channels::app_env as env;
        Some(Self {
            app_id: env::first(env::META_APP_ID)?,
            app_secret: env::first(env::META_APP_SECRET)?,
            config_id: env::first(env::META_ES_CONFIG_ID)?,
            system_token: env::first(env::META_SYSTEM_TOKEN),
        })
    }
}

fn not_configured() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "whatsapp_not_configured" }))).into_response()
}

// ------------------------------------------------------------------ Graph

pub struct GraphReq {
    pub method: &'static str,
    pub url: String,
    pub bearer: Option<String>,
    pub query: Vec<(String, String)>,
    pub body: Option<Value>,
}

pub struct GraphResp {
    pub status: u16,
    pub body: Value,
}

/// The Graph API, abstracted so tests never call Meta.
#[async_trait]
pub trait Graph: Send + Sync {
    async fn call(&self, req: GraphReq) -> Result<GraphResp, String>;
}

pub struct ReqwestGraph;

#[async_trait]
impl Graph for ReqwestGraph {
    async fn call(&self, req: GraphReq) -> Result<GraphResp, String> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| e.to_string())?;
        let mut builder = if req.method == "GET" { client.get(&req.url).query(&req.query) } else { client.post(&req.url) };
        if let Some(token) = &req.bearer {
            builder = builder.bearer_auth(token);
        }
        if let Some(body) = &req.body {
            builder = builder.json(body);
        }
        let response = builder.send().await.map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        let body = response.json::<Value>().await.unwrap_or(Value::Null);
        Ok(GraphResp { status, body })
    }
}

fn graph_error(step: &str, resp: &GraphResp) -> ApiError {
    let detail = resp.body.pointer("/error/message").and_then(Value::as_str).unwrap_or("no detail");
    ApiError::ServiceUnavailable(format!("WhatsApp {step} failed ({}): {detail}", resp.status))
}

/// What onboarding produced.
#[derive(Debug, PartialEq, Eq)]
pub struct Onboarded {
    pub business_token: String,
}

/// The three Graph calls after Embedded Signup: exchange the code, subscribe
/// the app to the WABA with the owner's callback, register the number.
pub async fn onboard(
    graph: &dyn Graph,
    cfg: &MetaConfig,
    code: &str,
    waba_id: &str,
    phone_number_id: &str,
    callback_url: &str,
    verify_token: &str,
    pin: &str,
) -> Result<Onboarded, ApiError> {
    let exchange = graph
        .call(GraphReq {
            method: "GET",
            url: format!("{GRAPH_BASE}/oauth/access_token"),
            bearer: None,
            query: vec![
                ("client_id".into(), cfg.app_id.clone()),
                ("client_secret".into(), cfg.app_secret.clone()),
                ("code".into(), code.to_string()),
            ],
            body: None,
        })
        .await
        .map_err(|e| ApiError::ServiceUnavailable(format!("WhatsApp code exchange unreachable: {e}")))?;
    if exchange.status / 100 != 2 {
        return Err(graph_error("code exchange", &exchange));
    }
    let token = exchange
        .body
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::ServiceUnavailable("WhatsApp code exchange returned no access_token".into()))?
        .to_string();

    let subscribe = graph
        .call(GraphReq {
            method: "POST",
            url: format!("{GRAPH_BASE}/{waba_id}/subscribed_apps"),
            bearer: Some(token.clone()),
            query: vec![],
            body: Some(subscribe_body(callback_url, verify_token)),
        })
        .await
        .map_err(|e| ApiError::ServiceUnavailable(format!("WhatsApp subscribe unreachable: {e}")))?;
    if subscribe.status / 100 != 2 {
        return Err(graph_error("subscribed_apps", &subscribe));
    }

    let register = graph
        .call(GraphReq {
            method: "POST",
            url: format!("{GRAPH_BASE}/{phone_number_id}/register"),
            bearer: Some(token.clone()),
            query: vec![],
            body: Some(json!({ "messaging_product": "whatsapp", "pin": pin })),
        })
        .await
        .map_err(|e| ApiError::ServiceUnavailable(format!("WhatsApp register unreachable: {e}")))?;
    if register.status / 100 != 2 {
        return Err(graph_error("register", &register));
    }
    Ok(Onboarded { business_token: token })
}

pub fn subscribe_body(callback_url: &str, verify_token: &str) -> Value {
    json!({ "override_callback_uri": callback_url, "verify_token": verify_token })
}

// ------------------------------------------------------------------ edge

fn relay_secret(app_secret: &str, route_id: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(app_secret.as_bytes()).expect("hmac takes any key length");
    mac.update(format!("wa-relay-v1:{route_id}").as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac takes any key length");
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

/// `X-Hub-Signature-256: sha256=<hex HMAC-SHA256(app secret, raw body)>`.
pub fn signature_ok(app_secret: &str, header: Option<&str>, body: &[u8]) -> bool {
    let Some(given) = header.and_then(|h| h.strip_prefix("sha256=")).and_then(|h| hex::decode(h).ok()) else {
        return false;
    };
    let mut mac = HmacSha256::new_from_slice(app_secret.as_bytes()).expect("hmac takes any key length");
    mac.update(body);
    mac.verify_slice(&given).is_ok()
}

/// Meta's subscribe handshake: echo `hub.challenge` when the verify token matches.
pub fn handshake(query: &str, stored_hash: &str) -> Option<String> {
    let params: HashMap<String, String> = reqwest::Url::parse(&format!("https://x.invalid/?{query}"))
        .map(|u| u.query_pairs().into_owned().collect())
        .unwrap_or_default();
    if params.get("hub.mode").map(String::as_str) != Some("subscribe") {
        return None;
    }
    let token = params.get("hub.verify_token")?;
    if sha256_hex(token) != stored_hash {
        return None;
    }
    params.get("hub.challenge").cloned()
}

pub enum Edge {
    /// Not an Embedded Signup address: the generic relay handles it.
    Passthrough,
    /// Answered here (handshake, rejected signature, not configured).
    Respond(Response),
    /// Verified: forward with this signature, made with the address's relay secret.
    Resign(String),
}

/// Customers who wrote (or whose status changed) in a webhook body, as `(phone_number_id, wa_id, unix ts)`.
pub fn inbound_senders(body: &Value) -> Vec<(String, String, Option<i64>)> {
    let mut out = Vec::new();
    for entry in body.get("entry").and_then(Value::as_array).into_iter().flatten() {
        for change in entry.get("changes").and_then(Value::as_array).into_iter().flatten() {
            let value = &change["value"];
            let Some(phone) = value.pointer("/metadata/phone_number_id").and_then(Value::as_str) else { continue };
            for message in value.get("messages").and_then(Value::as_array).into_iter().flatten() {
                let Some(from) = message.get("from").and_then(Value::as_str) else { continue };
                let ts = message.get("timestamp").and_then(Value::as_str).and_then(|t| t.parse().ok());
                out.push((phone.to_string(), from.to_string(), ts));
            }
        }
    }
    out
}

pub async fn edge(
    state: &ApiState,
    route_id: &str,
    method: &Method,
    query: &str,
    headers: &HashMap<String, String>,
    body: &[u8],
) -> Result<Edge, ApiError> {
    let account: Option<(String,)> = sqlx::query_as(
        "SELECT verify_token_hash FROM whatsapp_es_accounts WHERE route_id = $1 AND revoked_at IS NULL",
    )
    .bind(route_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((verify_hash,)) = account else { return Ok(Edge::Passthrough) };
    let Some(cfg) = MetaConfig::from_env() else { return Ok(Edge::Respond(not_configured())) };

    if method == Method::GET {
        return Ok(Edge::Respond(match handshake(query, &verify_hash) {
            Some(challenge) => (StatusCode::OK, challenge).into_response(),
            None => StatusCode::FORBIDDEN.into_response(),
        }));
    }
    if !signature_ok(&cfg.app_secret, headers.get("x-hub-signature-256").map(String::as_str), body) {
        return Ok(Edge::Respond((StatusCode::UNAUTHORIZED, Json(json!({ "error": "invalid_signature" }))).into_response()));
    }
    if let Ok(payload) = serde_json::from_slice::<Value>(body) {
        for (phone, wa_id, ts) in inbound_senders(&payload) {
            let at = ts.and_then(|t| DateTime::from_timestamp(t, 0)).map(|t| t.min(Utc::now())).unwrap_or_else(Utc::now);
            let _ = sqlx::query(
                "INSERT INTO whatsapp_windows (phone_number_id, wa_id, last_inbound_at) VALUES ($1, $2, $3)
                 ON CONFLICT (phone_number_id, wa_id) DO UPDATE
                   SET last_inbound_at = GREATEST(whatsapp_windows.last_inbound_at, EXCLUDED.last_inbound_at)",
            )
            .bind(&phone)
            .bind(&wa_id)
            .bind(at)
            .execute(&state.db)
            .await;
        }
    }
    Ok(Edge::Resign(sign(&relay_secret(&cfg.app_secret, route_id), body)))
}

// ------------------------------------------------------------------ complete

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompleteBody {
    code: String,
    waba_id: String,
    phone_number_id: String,
    runtime_id: Option<String>,
}

fn digits_only(value: &str) -> bool {
    !value.is_empty() && value.len() <= 40 && value.bytes().all(|b| b.is_ascii_digit())
}

fn random_token() -> String {
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill(&mut bytes);
    hex::encode(bytes)
}

fn random_pin() -> String {
    format!("{:06}", rand::thread_rng().gen_range(0..1_000_000))
}

async fn complete(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<CompleteBody>) -> Response {
    match complete_inner(&state, &headers, body, &ReqwestGraph).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

async fn complete_inner(state: &ApiState, headers: &HeaderMap, body: CompleteBody, graph: &dyn Graph) -> Result<Response, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, headers, "compute").await?.id;
    let (Some(cfg), Some(cipher)) = (MetaConfig::from_env(), state.credential_cipher.clone()) else {
        return Ok(not_configured());
    };
    if body.code.trim().is_empty() || !digits_only(&body.waba_id) || !digits_only(&body.phone_number_id) {
        return Err(ApiError::BadRequest("code, wabaId and phoneNumberId are required".into()));
    }
    let runtime_id = match body.runtime_id {
        Some(id) => id,
        None => {
            let runtimes: Vec<(String,)> =
                sqlx::query_as("SELECT id FROM runtime_devices WHERE user_id = $1 AND revoked_at IS NULL LIMIT 2")
                    .bind(&user)
                    .fetch_all(&state.db)
                    .await?;
            match runtimes.as_slice() {
                [(id,)] => id.clone(),
                _ => return Err(ApiError::BadRequest("runtimeId is required".into())),
            }
        }
    };
    let owns: Option<(String,)> =
        sqlx::query_as("SELECT id FROM runtime_devices WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL")
            .bind(&runtime_id)
            .bind(&user)
            .fetch_optional(&state.db)
            .await?;
    if owns.is_none() {
        return Err(ApiError::NotFound("Runtime not found".into()));
    }

    // A number someone else already onboarded can't be taken over; the user's own is re-onboarded.
    let existing: Option<(String, String, String, String)> = sqlx::query_as(
        "SELECT id, user_id, route_id, pin_sealed FROM whatsapp_es_accounts WHERE phone_number_id = $1 AND revoked_at IS NULL",
    )
    .bind(&body.phone_number_id)
    .fetch_optional(&state.db)
    .await?;
    let mut pin = random_pin();
    if let Some((_, owner, _, pin_sealed)) = &existing {
        if owner != &user {
            return Err(ApiError::Conflict("This WhatsApp number is already connected to another account".into()));
        }
        // The number may already have two-step verification on: reuse its PIN.
        if let Ok(old) = cipher.decrypt(pin_sealed) {
            pin = old;
        }
    }

    let key = new_key();
    let route_id = uuid::Uuid::new_v4().to_string();
    let account_id = uuid::Uuid::new_v4().to_string();
    let callback = format!("{}/channels/in/{}", public_base(), key);
    let verify_token = random_token();

    let mut tx = state.db.begin().await?;
    sqlx::query(
        "INSERT INTO channel_inbound_routes (id, key_hash, user_id, runtime_id, provider, label) VALUES ($1, $2, $3, $4, 'whatsapp', 'WhatsApp business')",
    )
    .bind(&route_id)
    .bind(sha256_hex(&key))
    .bind(&user)
    .bind(&runtime_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let onboarded = match onboard(graph, &cfg, body.code.trim(), &body.waba_id, &body.phone_number_id, &callback, &verify_token, &pin).await {
        Ok(o) => o,
        Err(error) => {
            let _ = sqlx::query("UPDATE channel_inbound_routes SET revoked_at = now() WHERE id = $1")
                .bind(&route_id)
                .execute(&state.db)
                .await;
            return Err(error);
        }
    };
    let token_sealed = cipher.encrypt(&onboarded.business_token).map_err(|_| ApiError::Internal("Failed to seal the WhatsApp token".into()))?;
    let pin_sealed = cipher.encrypt(&pin).map_err(|_| ApiError::Internal("Failed to seal the WhatsApp PIN".into()))?;

    let mut tx = state.db.begin().await?;
    if let Some((old_id, _, old_route, _)) = &existing {
        sqlx::query("UPDATE whatsapp_es_accounts SET revoked_at = now() WHERE id = $1").bind(old_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE channel_inbound_routes SET revoked_at = now() WHERE id = $1").bind(old_route).execute(&mut *tx).await?;
    }
    sqlx::query(
        "INSERT INTO whatsapp_es_accounts (id, user_id, runtime_id, route_id, waba_id, phone_number_id, token_sealed, pin_sealed, verify_token_hash)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(&account_id)
    .bind(&user)
    .bind(&runtime_id)
    .bind(&route_id)
    .bind(&body.waba_id)
    .bind(&body.phone_number_id)
    .bind(&token_sealed)
    .bind(&pin_sealed)
    .bind(sha256_hex(&verify_token))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "accountId": account_id,
            "routeId": route_id,
            "runtimeId": runtime_id,
            "wabaId": body.waba_id,
            "phoneNumberId": body.phone_number_id,
            // The runtime stores this as the account's `appSecret`; it verifies only this address.
            "relaySecret": relay_secret(&cfg.app_secret, &route_id),
        })),
    )
        .into_response())
}

// ------------------------------------------------------------------ send

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendBody {
    phone_number_id: String,
    to: String,
    text: Option<String>,
    template: Option<Template>,
}

#[derive(Deserialize)]
struct Template {
    name: String,
    language: String,
    components: Option<Value>,
}

/// Inside the window anything may go out; outside it only a template.
#[derive(Debug, PartialEq, Eq)]
pub enum Plan {
    Text,
    Template,
    Refuse,
}

pub fn window_open(last_inbound: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    last_inbound.map(|at| now - at < Duration::hours(WINDOW_HOURS)).unwrap_or(false)
}

pub fn plan(open: bool, has_text: bool, has_template: bool) -> Plan {
    match (has_template, open, has_text) {
        (true, _, _) => Plan::Template,
        (false, true, true) => Plan::Text,
        _ => Plan::Refuse,
    }
}

pub fn message_body(to: &str, text: Option<&str>, template: Option<(&str, &str, Option<&Value>)>) -> Value {
    match template {
        Some((name, language, components)) => {
            let mut tpl = json!({ "name": name, "language": { "code": language } });
            if let Some(c) = components {
                tpl["components"] = c.clone();
            }
            json!({ "messaging_product": "whatsapp", "recipient_type": "individual", "to": to, "type": "template", "template": tpl })
        }
        None => json!({
            "messaging_product": "whatsapp", "recipient_type": "individual", "to": to,
            "type": "text", "text": { "body": text.unwrap_or_default() }
        }),
    }
}

async fn send(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<SendBody>) -> Response {
    match send_inner(&state, &headers, body, &ReqwestGraph).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

async fn send_inner(state: &ApiState, headers: &HeaderMap, body: SendBody, graph: &dyn Graph) -> Result<Response, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, headers, "compute").await?.id;
    let (Some(cfg), Some(cipher)) = (MetaConfig::from_env(), state.credential_cipher.clone()) else {
        return Ok(not_configured());
    };
    let to: String = body.to.chars().filter(|c| c.is_ascii_digit()).collect();
    if to.is_empty() || !digits_only(&body.phone_number_id) {
        return Err(ApiError::BadRequest("phoneNumberId and a recipient are required".into()));
    }
    let account: Option<(String,)> = sqlx::query_as(
        "SELECT token_sealed FROM whatsapp_es_accounts WHERE user_id = $1 AND phone_number_id = $2 AND revoked_at IS NULL",
    )
    .bind(&user)
    .bind(&body.phone_number_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((token_sealed,)) = account else {
        return Err(ApiError::NotFound("WhatsApp number not connected".into()));
    };
    let last: Option<(DateTime<Utc>,)> =
        sqlx::query_as("SELECT last_inbound_at FROM whatsapp_windows WHERE phone_number_id = $1 AND wa_id = $2")
            .bind(&body.phone_number_id)
            .bind(&to)
            .fetch_optional(&state.db)
            .await?;
    let open = window_open(last.map(|l| l.0), Utc::now());
    let text = body.text.as_deref().filter(|t| !t.trim().is_empty());
    let message = match plan(open, text.is_some(), body.template.is_some()) {
        Plan::Refuse => {
            return Ok((
                StatusCode::CONFLICT,
                Json(json!({ "error": "outside_24h_window", "windowOpen": open })),
            )
                .into_response())
        }
        Plan::Text => message_body(&to, text, None),
        Plan::Template => {
            let t = body.template.as_ref().expect("planned a template");
            message_body(&to, None, Some((&t.name, &t.language, t.components.as_ref())))
        }
    };
    let token = cipher.decrypt(&token_sealed).ok().or(cfg.system_token).ok_or_else(|| ApiError::Internal("No WhatsApp token available".into()))?;
    let resp = graph
        .call(GraphReq {
            method: "POST",
            url: format!("{GRAPH_BASE}/{}/messages", body.phone_number_id),
            bearer: Some(token),
            query: vec![],
            body: Some(message),
        })
        .await
        .map_err(|e| ApiError::ServiceUnavailable(format!("WhatsApp send unreachable: {e}")))?;
    if resp.status / 100 != 2 {
        return Err(graph_error("send", &resp));
    }
    let message_id = resp.body.pointer("/messages/0/id").and_then(Value::as_str).unwrap_or_default();
    Ok(Json(json!({ "messageId": message_id, "windowOpen": open })).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeGraph {
        calls: Mutex<Vec<(String, String, Option<String>, Vec<(String, String)>, Option<Value>)>>,
        fail_at: Option<&'static str>,
    }

    #[async_trait]
    impl Graph for FakeGraph {
        async fn call(&self, req: GraphReq) -> Result<GraphResp, String> {
            self.calls.lock().unwrap().push((req.method.into(), req.url.clone(), req.bearer.clone(), req.query.clone(), req.body.clone()));
            if self.fail_at.map(|f| req.url.ends_with(f)).unwrap_or(false) {
                return Ok(GraphResp { status: 400, body: json!({ "error": { "message": "bad" } }) });
            }
            Ok(GraphResp {
                status: 200,
                body: if req.url.ends_with("/oauth/access_token") { json!({ "access_token": "BIZ" }) } else { json!({ "success": true }) },
            })
        }
    }

    fn cfg() -> MetaConfig {
        MetaConfig { app_id: "APP".into(), app_secret: "SECRET".into(), config_id: "CFG".into(), system_token: None }
    }

    #[tokio::test]
    async fn onboarding_exchanges_the_code_subscribes_with_the_override_and_registers() {
        let g = FakeGraph::default();
        let out = onboard(&g, &cfg(), "CODE", "111", "222", "https://api.allternit.com/channels/in/k", "vt", "123456").await.unwrap();
        assert_eq!(out.business_token, "BIZ");
        let calls = g.calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].0, "GET");
        assert_eq!(calls[0].1, "https://graph.facebook.com/v25.0/oauth/access_token");
        assert_eq!(calls[0].3, vec![("client_id".to_string(), "APP".to_string()), ("client_secret".into(), "SECRET".into()), ("code".into(), "CODE".into())]);
        assert_eq!(calls[1].1, "https://graph.facebook.com/v25.0/111/subscribed_apps");
        assert_eq!(calls[1].2.as_deref(), Some("BIZ"));
        assert_eq!(calls[1].4, Some(json!({ "override_callback_uri": "https://api.allternit.com/channels/in/k", "verify_token": "vt" })));
        assert_eq!(calls[2].1, "https://graph.facebook.com/v25.0/222/register");
        assert_eq!(calls[2].4, Some(json!({ "messaging_product": "whatsapp", "pin": "123456" })));
    }

    #[tokio::test]
    async fn a_failed_step_stops_onboarding() {
        let g = FakeGraph { fail_at: Some("/subscribed_apps"), ..Default::default() };
        assert!(onboard(&g, &cfg(), "C", "1", "2", "u", "v", "123456").await.is_err());
        assert_eq!(g.calls.lock().unwrap().len(), 2, "no register after a failed subscribe");
    }

    #[test]
    fn verify_handshake_echoes_the_challenge_only_for_the_right_token() {
        let hash = sha256_hex("vt");
        assert_eq!(handshake("hub.mode=subscribe&hub.verify_token=vt&hub.challenge=1158201444", &hash).as_deref(), Some("1158201444"));
        assert_eq!(handshake("hub.mode=subscribe&hub.verify_token=nope&hub.challenge=1", &hash), None);
        assert_eq!(handshake("hub.mode=unsubscribe&hub.verify_token=vt&hub.challenge=1", &hash), None);
        assert_eq!(handshake("hub.mode=subscribe&hub.challenge=1", &hash), None);
    }

    #[test]
    fn signatures_are_checked_with_the_app_secret_and_resigned_per_address() {
        let body = br#"{"object":"whatsapp_business_account"}"#;
        let good = sign("SECRET", body);
        assert!(signature_ok("SECRET", Some(&good), body));
        assert!(!signature_ok("OTHER", Some(&good), body));
        assert!(!signature_ok("SECRET", None, body));
        assert!(!signature_ok("SECRET", Some("sha256=zz"), body));
        let a = relay_secret("SECRET", "route-a");
        assert_ne!(a, relay_secret("SECRET", "route-b"));
        assert_ne!(a, "SECRET");
        // The runtime verifies the re-signed body with the relay secret.
        assert!(signature_ok(&a, Some(&sign(&a, body)), body));
    }

    #[test]
    fn the_24h_window_refuses_free_text_outside_it() {
        let now = Utc::now();
        assert!(window_open(Some(now - Duration::hours(23)), now));
        assert!(!window_open(Some(now - Duration::hours(25)), now));
        assert!(!window_open(None, now));
        assert_eq!(plan(true, true, false), Plan::Text);
        assert_eq!(plan(false, true, false), Plan::Refuse, "outside_24h_window");
        assert_eq!(plan(false, true, true), Plan::Template, "a template may always go out");
        assert_eq!(plan(true, false, false), Plan::Refuse, "nothing to send");
    }

    #[test]
    fn message_bodies_follow_the_cloud_api_shape() {
        assert_eq!(
            message_body("15551234567", Some("hi"), None),
            json!({ "messaging_product": "whatsapp", "recipient_type": "individual", "to": "15551234567", "type": "text", "text": { "body": "hi" } })
        );
        let b = message_body("1555", None, Some(("hello_world", "en_US", None)));
        assert_eq!(b["template"], json!({ "name": "hello_world", "language": { "code": "en_US" } }));
    }

    #[test]
    fn inbound_senders_reads_customers_not_statuses() {
        let body = json!({ "entry": [{ "changes": [{ "value": {
            "metadata": { "phone_number_id": "PN1" },
            "messages": [{ "from": "15551234567", "id": "wamid.1", "timestamp": "1700000000", "type": "text" }],
            "statuses": [{ "id": "x", "status": "read", "recipient_id": "15559999999" }]
        } }] }] });
        assert_eq!(inbound_senders(&body), vec![("PN1".to_string(), "15551234567".to_string(), Some(1_700_000_000))]);
    }
}
