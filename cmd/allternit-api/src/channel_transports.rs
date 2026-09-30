//! Teams, Discord and WhatsApp behind [`ChannelTransport`] (Piece C), plus the
//! generic channel webhook and the WhatsApp "Muse control lane".
//!
//! Everything platform-specific is split into pure functions (verify,
//! normalize, build the outbound request, parse its response) so it is tested
//! offline against each platform's documented payload shapes. The only I/O is
//! the injected [`HttpSend`]. Secrets come from `provider_account_bindings`
//! sealed `secret_ref` (JSON object per provider), never from Bot records:
//!
//! * teams:    `{ securityToken, accessToken }`  (outgoing-webhook HMAC + Bot Framework bearer)
//! * discord:  `{ publicKey, webhookUrl }`        (Ed25519 interactions key + channel webhook)
//! * whatsapp: `{ appSecret, verifyToken, accessToken, phoneNumberId }`

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use hmac::{Hmac, Mac};
use rusqlite::params;
use serde_json::{json, Value};
use sha2::Sha256;
use tracing::warn;

use crate::channel_gateway::*;
use crate::db::DbHandle;
use crate::gateway_runner::{AaiError, AaiTransport};
use crate::AppState;

type HmacSha256 = Hmac<Sha256>;

pub const PROVIDERS: [&str; 4] = ["slack", "teams", "discord", "whatsapp"];

// ---------------------------------------------------------------- http seam

#[derive(Debug, Clone)]
pub struct HttpReq {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

#[derive(Debug, Clone)]
pub struct HttpResp {
    pub status: u16,
    pub body: Value,
}

#[async_trait]
pub trait HttpSend: Send + Sync {
    /// `Err` = the request may not have reached the platform (timeout, connection).
    async fn post_json(&self, req: HttpReq) -> Result<HttpResp, String>;
}

pub struct ReqwestSend;

#[async_trait]
impl HttpSend for ReqwestSend {
    async fn post_json(&self, req: HttpReq) -> Result<HttpResp, String> {
        let mut r = reqwest::Client::new().post(&req.url).timeout(std::time::Duration::from_secs(15)).json(&req.body);
        for (k, v) in &req.headers {
            r = r.header(k, v);
        }
        let resp = r.send().await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let body = resp.json::<Value>().await.unwrap_or(Value::Null);
        Ok(HttpResp { status, body })
    }
}

/// Definite vs uncertain outcome of a platform reply.
fn judge(resp: Result<HttpResp, String>, id_of: impl Fn(&Value) -> Option<String>) -> Result<String, PostError> {
    let r = resp.map_err(PostError::Uncertain)?;
    match r.status {
        200..=299 => id_of(&r.body).ok_or_else(|| PostError::Uncertain("the platform accepted the post but returned no message id".into())),
        500..=599 => Err(PostError::Uncertain(format!("platform returned {}", r.status))),
        429 => Err(PostError::Rejected("rate limited".into())),
        s => Err(PostError::Rejected(format!("platform returned {s}: {}", r.body))),
    }
}

fn hdr<'a>(h: &'a HeaderMap, k: &str) -> Option<&'a str> {
    h.get(k).and_then(|v| v.to_str().ok())
}

fn s_of(v: &Value, ptr: &str) -> Option<String> {
    v.pointer(ptr).and_then(|x| x.as_str().map(str::to_string).or_else(|| x.as_i64().map(|n| n.to_string())))
}

/// A secret field: `key` from a JSON object secret, else the raw string.
fn pick(secret: &str, key: &str) -> String {
    serde_json::from_str::<Value>(secret).ok().and_then(|v| v.get(key).and_then(Value::as_str).map(str::to_string)).unwrap_or_else(|| if secret.trim_start().starts_with('{') { String::new() } else { secret.to_string() })
}

fn exact(requested: Option<&str>, own: Option<&str>) -> Identity {
    Identity { id: requested.map(str::to_string), exact: requested.is_none() || requested == own }
}

fn relay_text(out: &Outbound, relayed: bool) -> String {
    match (&out.identity, relayed) {
        (Some(who), true) => format!("{who}: {}", out.text),
        _ => out.text.clone(),
    }
}

fn ev(kind: InboundKind, conversation: String, channel: String, remote_id: String, message_id: String) -> Inbound {
    Inbound { kind, workspace: None, channel, conversation, thread: None, remote_id, message_id, text: None, user: None, reaction: None, added: None, cursor: None, own: false }
}

// ---------------------------------------------------------------- Teams

pub struct TeamsTransport {
    pub http: Arc<dyn HttpSend>,
    pub access_token: Option<String>,
    pub own_identity: Option<String>,
}

pub fn teams_normalize(a: &Value) -> Vec<Inbound> {
    let (Some(ty), Some(conv)) = (a["type"].as_str(), s_of(a, "/conversation/id")) else { return vec![] };
    let key = format!("teams:{conv}");
    let id = s_of(a, "/id").unwrap_or_default();
    let mut base = ev(InboundKind::Message, key, conv.clone(), id.clone(), id.clone());
    // serviceUrl is where replies go; it rides in `workspace`.
    base.workspace = s_of(a, "/serviceUrl");
    base.user = s_of(a, "/from/id");
    base.own = a.pointer("/from/role").and_then(Value::as_str) == Some("bot");
    match ty {
        "message" if !id.is_empty() => {
            base.text = s_of(a, "/text");
            base.cursor = Some(id);
            vec![base]
        }
        "messageUpdate" if !id.is_empty() => {
            base.kind = InboundKind::Edited;
            base.remote_id = format!("edit:{id}:{}", s_of(a, "/timestamp").unwrap_or_default());
            base.text = s_of(a, "/text");
            vec![base]
        }
        "messageDelete" if !id.is_empty() => {
            base.kind = InboundKind::Deleted;
            base.remote_id = format!("del:{id}");
            vec![base]
        }
        "messageReaction" => {
            let target = s_of(a, "/replyToId").unwrap_or_default();
            let ts = s_of(a, "/timestamp").unwrap_or_default();
            let mut out = Vec::new();
            for (list, added) in [("reactionsAdded", true), ("reactionsRemoved", false)] {
                for r in a.get(list).and_then(Value::as_array).into_iter().flatten() {
                    let kind = r["type"].as_str().unwrap_or_default().to_string();
                    let mut e = base.clone();
                    e.kind = InboundKind::ReactionUpdated;
                    e.message_id = target.clone();
                    e.remote_id = format!("react:{target}:{}:{kind}:{}:{ts}", e.user.clone().unwrap_or_default(), if added { "add" } else { "remove" });
                    e.reaction = Some(kind);
                    e.added = Some(added);
                    out.push(e);
                }
            }
            out
        }
        _ => vec![],
    }
}

#[async_trait]
impl ChannelTransport for TeamsTransport {
    fn provider(&self) -> &'static str {
        "teams"
    }
    /// Teams outgoing webhook: `Authorization: HMAC <base64(HMAC-SHA256(base64decode(securityToken), body))>`.
    fn verify(&self, secret: &str, headers: &HeaderMap, body: &[u8]) -> Result<(), String> {
        let token = pick(secret, "securityToken");
        let key = base64::engine::general_purpose::STANDARD.decode(token.trim()).map_err(|_| "the Teams security token is not base64")?;
        let given = hdr(headers, "authorization").and_then(|a| a.strip_prefix("HMAC ")).ok_or("missing Authorization: HMAC header")?;
        let given = base64::engine::general_purpose::STANDARD.decode(given.trim()).map_err(|_| "bad HMAC encoding")?;
        let mut mac = HmacSha256::new_from_slice(&key).map_err(|_| "bad key")?;
        mac.update(body);
        mac.verify_slice(&given).map_err(|_| "Teams HMAC mismatch".to_string())
    }
    fn normalize(&self, payload: &Value) -> Vec<Inbound> {
        teams_normalize(payload)
    }
    fn identity(&self, requested: Option<&str>) -> Identity {
        exact(requested, self.own_identity.as_deref())
    }
    async fn post(&self, out: &Outbound) -> Result<Receipt, PostError> {
        let token = self.access_token.clone().ok_or_else(|| PostError::Rejected("no Teams access token configured".into()))?;
        let service = out.workspace.clone().ok_or_else(|| PostError::Rejected("no Teams serviceUrl for this conversation".into()))?;
        let relayed = !self.identity(out.identity.as_deref()).exact;
        let url = format!("{}/v3/conversations/{}/activities", service.trim_end_matches('/'), out.channel);
        let body = json!({ "type": "message", "text": relay_text(out, relayed) });
        let resp = self.http.post_json(HttpReq { url, headers: vec![("Authorization".into(), format!("Bearer {token}"))], body }).await;
        judge(resp, |b| b["id"].as_str().map(str::to_string)).map(|remote_id| Receipt { remote_id, relayed })
    }
}

// ---------------------------------------------------------------- Discord

pub struct DiscordTransport {
    pub http: Arc<dyn HttpSend>,
    pub webhook_url: Option<String>,
    pub own_identity: Option<String>,
}

pub fn discord_normalize(p: &Value) -> Vec<Inbound> {
    let (t, d) = match p.get("t").and_then(Value::as_str) {
        Some(t) => (t, p.get("d").unwrap_or(&Value::Null)),
        None => return vec![],
    };
    let Some(channel) = s_of(d, "/channel_id") else { return vec![] };
    let conv = format!("discord:{channel}");
    let mut e = ev(InboundKind::Message, conv, channel, String::new(), String::new());
    e.workspace = s_of(d, "/guild_id");
    let seq = p.get("s").and_then(Value::as_i64).map(|n| n.to_string()).unwrap_or_default();
    match t {
        "MESSAGE_CREATE" => {
            let id = s_of(d, "/id").unwrap_or_default();
            e.remote_id = id.clone();
            e.message_id = id.clone();
            e.cursor = Some(id);
            e.text = s_of(d, "/content");
            e.user = s_of(d, "/author/id");
            e.own = d.pointer("/author/bot").and_then(Value::as_bool).unwrap_or(false);
            vec![e]
        }
        "MESSAGE_UPDATE" => {
            let id = s_of(d, "/id").unwrap_or_default();
            e.kind = InboundKind::Edited;
            e.remote_id = format!("edit:{id}:{}", s_of(d, "/edited_timestamp").unwrap_or_default());
            e.message_id = id;
            e.text = s_of(d, "/content");
            e.user = s_of(d, "/author/id");
            vec![e]
        }
        "MESSAGE_DELETE" => {
            let id = s_of(d, "/id").unwrap_or_default();
            e.kind = InboundKind::Deleted;
            e.remote_id = format!("del:{id}");
            e.message_id = id;
            vec![e]
        }
        "MESSAGE_REACTION_ADD" | "MESSAGE_REACTION_REMOVE" => {
            let mid = s_of(d, "/message_id").unwrap_or_default();
            let emoji = s_of(d, "/emoji/name").unwrap_or_default();
            let added = t.ends_with("ADD");
            e.kind = InboundKind::ReactionUpdated;
            e.user = s_of(d, "/user_id");
            e.remote_id = format!("react:{mid}:{}:{emoji}:{}:{seq}", e.user.clone().unwrap_or_default(), if added { "add" } else { "remove" });
            e.message_id = mid;
            e.reaction = Some(emoji);
            e.added = Some(added);
            vec![e]
        }
        _ => vec![],
    }
}

#[async_trait]
impl ChannelTransport for DiscordTransport {
    fn provider(&self) -> &'static str {
        "discord"
    }
    /// Discord interactions/webhook events: Ed25519 over `timestamp + body`.
    fn verify(&self, secret: &str, headers: &HeaderMap, body: &[u8]) -> Result<(), String> {
        use ed25519_dalek::{Signature, Verifier, VerifyingKey};
        let pk = hex::decode(pick(secret, "publicKey").trim()).map_err(|_| "public key is not hex")?;
        let pk: [u8; 32] = pk.try_into().map_err(|_| "public key must be 32 bytes")?;
        let sig = hex::decode(hdr(headers, "x-signature-ed25519").ok_or("missing x-signature-ed25519")?).map_err(|_| "bad signature encoding")?;
        let sig: [u8; 64] = sig.try_into().map_err(|_| "signature must be 64 bytes")?;
        let ts = hdr(headers, "x-signature-timestamp").ok_or("missing x-signature-timestamp")?;
        let mut msg = ts.as_bytes().to_vec();
        msg.extend_from_slice(body);
        VerifyingKey::from_bytes(&pk).map_err(|_| "bad public key")?.verify(&msg, &Signature::from_bytes(&sig)).map_err(|_| "Discord signature mismatch".to_string())
    }
    fn normalize(&self, payload: &Value) -> Vec<Inbound> {
        discord_normalize(payload)
    }
    fn identity(&self, requested: Option<&str>) -> Identity {
        exact(requested, self.own_identity.as_deref())
    }
    async fn post(&self, out: &Outbound) -> Result<Receipt, PostError> {
        let hook = self.webhook_url.clone().ok_or_else(|| PostError::Rejected("no Discord webhook configured".into()))?;
        let relayed = !self.identity(out.identity.as_deref()).exact;
        let mut url = format!("{hook}?wait=true");
        if let Some(t) = &out.thread {
            url.push_str(&format!("&thread_id={t}"));
        }
        let mut body = json!({ "content": out.text });
        if let (Some(name), true) = (&out.identity, relayed) {
            body["username"] = json!(name);
        }
        judge(self.http.post_json(HttpReq { url, headers: vec![], body }).await, |b| b["id"].as_str().map(str::to_string)).map(|remote_id| Receipt { remote_id, relayed })
    }
}

// ---------------------------------------------------------------- WhatsApp

pub struct WhatsAppTransport {
    pub http: Arc<dyn HttpSend>,
    pub access_token: Option<String>,
    pub own_identity: Option<String>,
}

pub fn whatsapp_normalize(p: &Value) -> Vec<Inbound> {
    let mut out = Vec::new();
    for entry in p.get("entry").and_then(Value::as_array).into_iter().flatten() {
        for change in entry.get("changes").and_then(Value::as_array).into_iter().flatten() {
            let v = &change["value"];
            let Some(pnid) = s_of(v, "/metadata/phone_number_id") else { continue };
            let mk = |kind, wa: &str, remote: String, mid: String| {
                let mut e = ev(kind, format!("whatsapp:{pnid}:{wa}"), pnid.clone(), remote, mid);
                e.thread = Some(wa.to_string());
                e.workspace = s_of(entry, "/id");
                e
            };
            for m in v.get("messages").and_then(Value::as_array).into_iter().flatten() {
                let (Some(from), Some(id)) = (s_of(m, "/from"), s_of(m, "/id")) else { continue };
                let ts = s_of(m, "/timestamp");
                match m["type"].as_str() {
                    Some("text") => {
                        let mut e = mk(InboundKind::Message, &from, id.clone(), id);
                        e.text = s_of(m, "/text/body");
                        e.user = Some(from.clone());
                        e.cursor = ts;
                        out.push(e);
                    }
                    Some("reaction") => {
                        let target = s_of(m, "/reaction/message_id").unwrap_or_default();
                        let emoji = s_of(m, "/reaction/emoji").unwrap_or_default();
                        let mut e = mk(InboundKind::ReactionUpdated, &from, format!("react:{target}:{from}:{id}"), target);
                        e.user = Some(from.clone());
                        e.added = Some(!emoji.is_empty());
                        e.reaction = Some(emoji);
                        out.push(e);
                    }
                    _ => {}
                }
            }
            for st in v.get("statuses").and_then(Value::as_array).into_iter().flatten() {
                let (Some(id), Some(status), Some(to)) = (s_of(st, "/id"), s_of(st, "/status"), s_of(st, "/recipient_id")) else { continue };
                let mut e = mk(InboundKind::Delivery, &to, format!("status:{id}:{status}"), id);
                e.text = Some(status);
                out.push(e);
            }
        }
    }
    out
}

#[async_trait]
impl ChannelTransport for WhatsAppTransport {
    fn provider(&self) -> &'static str {
        "whatsapp"
    }
    /// `X-Hub-Signature-256: sha256=<hex HMAC-SHA256(appSecret, raw body)>`.
    fn verify(&self, secret: &str, headers: &HeaderMap, body: &[u8]) -> Result<(), String> {
        let given = hdr(headers, "x-hub-signature-256").and_then(|s| s.strip_prefix("sha256=")).ok_or("missing x-hub-signature-256")?;
        let given = hex::decode(given).map_err(|_| "bad signature encoding")?;
        let mut mac = HmacSha256::new_from_slice(pick(secret, "appSecret").as_bytes()).map_err(|_| "bad key")?;
        mac.update(body);
        mac.verify_slice(&given).map_err(|_| "WhatsApp signature mismatch".to_string())
    }
    fn normalize(&self, payload: &Value) -> Vec<Inbound> {
        whatsapp_normalize(payload)
    }
    fn identity(&self, requested: Option<&str>) -> Identity {
        exact(requested, self.own_identity.as_deref())
    }
    async fn post(&self, out: &Outbound) -> Result<Receipt, PostError> {
        let token = self.access_token.clone().ok_or_else(|| PostError::Rejected("no WhatsApp access token configured".into()))?;
        let to = out.thread.clone().ok_or_else(|| PostError::Rejected("no WhatsApp recipient for this conversation".into()))?;
        let relayed = !self.identity(out.identity.as_deref()).exact;
        let url = format!("https://graph.facebook.com/v20.0/{}/messages", out.channel);
        let body = json!({ "messaging_product": "whatsapp", "to": to, "type": "text", "text": { "body": relay_text(out, relayed) } });
        judge(self.http.post_json(HttpReq { url, headers: vec![("Authorization".into(), format!("Bearer {token}"))], body }).await, |b| b.pointer("/messages/0/id").and_then(Value::as_str).map(str::to_string))
            .map(|remote_id| Receipt { remote_id, relayed })
    }
}

// ---------------------------------------------------------------- accounts + factory

#[derive(Debug, Clone)]
pub struct Account {
    pub id: String,
    pub owner: String,
    pub restricted_bot: Option<String>,
    /// Unsealed secret (JSON object text, or a raw string). Never logged.
    pub secret: String,
}

fn accounts(db: &DbHandle, provider: &str, only: Option<&str>) -> Vec<Account> {
    let Ok(conn) = db.connect() else { return vec![] };
    let sql = "SELECT id, owner, restricted_bot_id, secret_ref FROM provider_account_bindings WHERE vendor = ?1 AND secret_ref IS NOT NULL AND secret_ref <> '' AND (?2 IS NULL OR id = ?2)";
    let Ok(mut q) = conn.prepare(sql) else { return vec![] };
    q.query_map(params![provider, only], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, String>(3)?)))
        .map(|rows| {
            rows.filter_map(Result::ok)
                .map(|(id, owner, restricted_bot, sealed)| Account { id, owner, restricted_bot, secret: crate::token_crypto::open(&sealed) })
                .filter(|a| !a.secret.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

pub fn build_transport(provider: &str, secret: &str, http: Arc<dyn HttpSend>) -> Option<Arc<dyn ChannelTransport>> {
    let token = |k: &str| Some(pick(secret, k)).filter(|s| !s.is_empty());
    Some(match provider {
        "slack" => Arc::new(SlackTransport::from_env()),
        "teams" => Arc::new(TeamsTransport { http, access_token: token("accessToken"), own_identity: token("botId") }),
        "discord" => Arc::new(DiscordTransport { http, webhook_url: token("webhookUrl"), own_identity: token("botId") }),
        "whatsapp" => Arc::new(WhatsAppTransport { http, access_token: token("accessToken"), own_identity: token("phoneNumberId") }),
        _ => return None,
    })
}

/// The production transport for a binding (its provider account's secrets).
pub fn transport_for(state: &Arc<AppState>, b: &BindingRow) -> Option<Arc<dyn ChannelTransport>> {
    if b.provider == "slack" {
        return build_transport("slack", "", Arc::new(ReqwestSend));
    }
    let acct = accounts(&state.db, &b.provider, b.account.as_deref()).into_iter().next()?;
    build_transport(&b.provider, &acct.secret, Arc::new(ReqwestSend))
}

// ---------------------------------------------------------------- inbound routing

pub struct Routed {
    pub binding: Option<BindingRow>,
    pub recorded: Recorded,
    /// `(session, bot, text)` when this event should run a bot turn.
    pub turn: Option<(String, String, String)>,
}

/// A bot whose vendor lane is a channel (e.g. Muse over WhatsApp): replies on
/// its conversation are the vendor's answers, pulled by the lane transport,
/// never new user turns.
fn lane_conversation(db: &DbHandle, thread_id: &str) -> bool {
    let Ok(conn) = db.connect() else { return false };
    conn.query_row(
        "SELECT COUNT(*) FROM bot_threads t JOIN bot_execution_bindings e ON e.bot_id = t.bot_id WHERE t.id = ?1 AND e.type = 'vendor' AND e.preferred_lane = 'channel'",
        params![thread_id],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
        > 0
}

/// Binding + record for one inbound event. New conversations become a bot
/// thread only when the account is restricted to a bot (like a Slack channel
/// bound to a bot); otherwise the event is ignored.
pub async fn route_inbound<R: crate::thread_routes::ThreadRuntime>(db: &DbHandle, rt: &R, acct: &Account, provider: &str, e: &Inbound) -> Result<Routed, String> {
    let mut binding = find_binding(db, provider, &e.conversation).filter(|b| b.owner == acct.owner);
    if binding.is_none() && !e.own && e.kind == InboundKind::Message {
        let Some(bot) = &acct.restricted_bot else { return Ok(Routed { binding: None, recorded: Recorded::Duplicate, turn: None }) };
        let text = e.text.clone().unwrap_or_default();
        if text.trim().is_empty() {
            return Ok(Routed { binding: None, recorded: Recorded::Duplicate, turn: None });
        }
        let title: String = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("Message").trim().chars().take(80).collect();
        let session = crate::thread_routes::channel_thread(db, rt, bot, provider, &e.conversation, &title, &text).await?;
        let thread_id: String = db
            .connect()
            .map_err(|x| x.to_string())?
            .query_row("SELECT thread_id FROM bot_thread_sessions WHERE session_id = ?1", params![session], |r| r.get(0))
            .map_err(|x| x.to_string())?;
        binding = Some(ensure_binding(db, &acct.owner, &thread_id, provider, e).map_err(|x| x.to_string())?);
    }
    let Some(b) = binding else { return Ok(Routed { binding: None, recorded: Recorded::Duplicate, turn: None }) };
    let recorded = record_inbound(db, &b, e)?;
    let mut turn = None;
    if recorded == Recorded::New && e.kind == InboundKind::Message && !e.own && !lane_conversation(db, &b.thread_id) {
        let conn = db.connect().map_err(|x| x.to_string())?;
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT s.session_id, t.bot_id FROM bot_thread_sessions s JOIN bot_threads t ON t.id = s.thread_id WHERE s.thread_id = ?1 ORDER BY s.generation DESC LIMIT 1",
                params![b.thread_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        if let Some((session, bot)) = row {
            turn = Some((session, bot, format!("[{provider} from {}] {}", e.user.clone().unwrap_or_else(|| "someone".into()), e.text.clone().unwrap_or_default())));
        }
    }
    Ok(Routed { binding: Some(b), recorded, turn })
}

pub fn channel_webhook_router() -> Router<Arc<AppState>> {
    Router::new().route("/webhooks/channels/:provider", post(webhook_h).get(whatsapp_challenge))
}

/// Meta's subscribe handshake.
async fn whatsapp_challenge(State(state): State<Arc<AppState>>, Path(provider): Path<String>, Query(q): Query<std::collections::HashMap<String, String>>) -> Response {
    if provider != "whatsapp" || q.get("hub.mode").map(String::as_str) != Some("subscribe") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let given = q.get("hub.verify_token").cloned().unwrap_or_default();
    if !given.is_empty() && accounts(&state.db, "whatsapp", None).iter().any(|a| pick(&a.secret, "verifyToken") == given) {
        return q.get("hub.challenge").cloned().unwrap_or_default().into_response();
    }
    StatusCode::FORBIDDEN.into_response()
}

async fn webhook_h(State(state): State<Arc<AppState>>, Path(provider): Path<String>, headers: HeaderMap, body: Bytes) -> Response {
    if provider == "slack" || !PROVIDERS.contains(&provider.as_str()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let candidates = accounts(&state.db, &provider, None);
    let matched = candidates.into_iter().find_map(|a| {
        let tx = build_transport(&provider, &a.secret, Arc::new(ReqwestSend))?;
        tx.verify(&a.secret, &headers, &body).ok().map(|_| (a, tx))
    });
    let Some((acct, tx)) = matched else {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "invalid_signature" }))).into_response();
    };
    let Ok(payload) = serde_json::from_slice::<Value>(&body) else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_json" }))).into_response();
    };
    let events = tx.normalize(&payload);
    let st = state.clone();
    tokio::spawn(async move {
        let rt = crate::thread_routes::GizziRuntime { db: st.db.clone() };
        for e in events {
            match route_inbound(&st.db, &rt, &acct, tx.provider(), &e).await {
                Ok(Routed { binding: Some(b), turn: Some((session, bot, text)), .. }) => {
                    match crate::agent_session_routes::send_bot_turn(&st.db, &session, &bot, &text).await {
                        Ok(reply) => {
                            let thread = b.external_thread.clone().unwrap_or_default();
                            if let Err(err) = post_reply(&st.db, tx.as_ref(), &b, &thread, &reply).await {
                                warn!(provider = %b.provider, "channel reply failed: {err}");
                            }
                        }
                        Err(err) => warn!("channel turn failed: {err}"),
                    }
                }
                Ok(_) => {}
                Err(err) => warn!("channel inbound failed: {err}"),
            }
        }
    });
    Json(json!({ "ok": true })).into_response()
}

// ---------------------------------------------------------------- Muse control lane

/// AAI transport for Bots whose vendor lane is `channel`: turns go out over
/// the account's channel transport and the vendor's replies come back as
/// `agent.message.completed` (who / whose / how from the binding). Anything
/// on another lane is passed through to `inner` untouched.
pub struct ChannelLaneTransport {
    state: Arc<AppState>,
    inner: Arc<dyn AaiTransport>,
    http: Arc<dyn HttpSend>,
}

impl ChannelLaneTransport {
    pub fn new(state: Arc<AppState>, inner: Arc<dyn AaiTransport>) -> Self {
        Self { state, inner, http: Arc::new(ReqwestSend) }
    }
    pub fn with_http(state: Arc<AppState>, inner: Arc<dyn AaiTransport>, http: Arc<dyn HttpSend>) -> Self {
        Self { state, inner, http }
    }
}

fn who_whose_how(binding: &Value) -> (String, String, String) {
    let g = |k: &str| binding.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let title = |s: String| s.chars().next().map(|c| c.to_uppercase().collect::<String>() + &s[c.len_utf8()..]).unwrap_or_default();
    let hint = |k: &str| binding.pointer(&format!("/capabilities/identity/{k}")).and_then(Value::as_str).map(str::to_string);
    let how = match g("adapterId").as_str() {
        "whatsapp" => "WhatsApp".to_string(),
        other => title(other.to_string()),
    };
    // who = the vendor's agent, whose = the vendor, how = the surface.
    (hint("who").unwrap_or_else(|| g("externalAgentId")), hint("whose").unwrap_or_else(|| title(g("vendor"))), hint("how").unwrap_or(how))
}

#[async_trait]
impl AaiTransport for ChannelLaneTransport {
    async fn call(&self, owner: &str, op: &str, binding: &Value, input: Value) -> Result<Value, AaiError> {
        self.call_cred(owner, op, binding, None, input).await
    }
    async fn append_transcript(&self, session_id: &str, text: &str, metadata: Value) -> Result<(), String> {
        self.inner.append_transcript(session_id, text, metadata).await
    }
    async fn call_cred(&self, owner: &str, op: &str, binding: &Value, credential: Option<&Value>, input: Value) -> Result<Value, AaiError> {
        if binding["preferredLane"].as_str() != Some("channel") {
            return self.inner.call_cred(owner, op, binding, credential, input).await;
        }
        let db = &self.state.db;
        let provider = binding["adapterId"].as_str().unwrap_or_default().to_string();
        let acct = accounts(db, &provider, binding["accountBindingId"].as_str()).into_iter().find(|a| a.owner == owner).ok_or_else(|| AaiError::new("AUTH_REQUIRED", format!("connect a {provider} account for this bot")))?;
        let tx = build_transport(&provider, &acct.secret, self.http.clone()).ok_or_else(|| AaiError::new("UNSUPPORTED", format!("{provider} is not a channel lane")))?;
        let pnid = pick(&acct.secret, "phoneNumberId");
        let contact = binding["externalAgentId"].as_str().unwrap_or_default().to_string();
        if contact.is_empty() {
            return Err(AaiError::new("UNSUPPORTED", "this bot has no contact to talk to on the channel"));
        }
        let conversation = format!("{provider}:{pnid}:{contact}");
        let internal = |e: String| AaiError::new("INTERNAL", e);
        match op {
            "agent.context.open" => {
                let thread = input["threadId"].as_str().ok_or_else(|| internal("threadId is required".into()))?;
                let mut e = ev(InboundKind::Message, conversation.clone(), pnid.clone(), String::new(), String::new());
                e.thread = Some(contact);
                ensure_binding(db, owner, thread, &provider, &e).map_err(|x| internal(x.to_string()))?;
                Ok(json!({ "contextId": conversation, "guarantee": "best_effort" }))
            }
            "agent.context.message" => {
                let b = find_binding(db, &provider, &conversation).ok_or_else(|| AaiError::new("CONTEXT_NOT_FOUND", "the channel conversation is not open"))?;
                let text = input["text"].as_str().unwrap_or_default().to_string();
                let corr = input["correlationId"].as_str().map(str::to_string).unwrap_or_else(|| crate::agent_gateway_routes::id("lane"));
                let r = send(db, tx.as_ref(), owner, &b.thread_id, &SendReq { text, correlation_id: Some(corr), ..Default::default() }).await.map_err(internal)?;
                match r {
                    SendOutcome::Sent { remote_id, .. } => Ok(json!({ "accepted": true, "remoteId": remote_id })),
                    SendOutcome::Replay { state, remote_id } if state == "confirmed" => Ok(json!({ "accepted": true, "remoteId": remote_id })),
                    SendOutcome::Unconfirmed { .. } | SendOutcome::Replay { .. } => {
                        let mut e = AaiError::new("DELIVERY_UNCONFIRMED", "the message may not have been delivered");
                        e.retryable = false;
                        Err(e)
                    }
                    SendOutcome::ApprovalRequired { .. } => Err(AaiError::new("APPROVAL_REQUIRED", "posting on this channel needs approval")),
                    SendOutcome::Denied(m) => Err(AaiError::new("CHANNEL_POLICY", m)),
                    SendOutcome::Rejected(m) => Err(AaiError::new("CHANNEL_REJECTED", m)),
                    SendOutcome::ReadOnly | SendOutcome::NoBinding => Err(AaiError::new("CONTEXT_NOT_FOUND", "the channel conversation is not writable")),
                }
            }
            "agent.events" => {
                let b = find_binding(db, &provider, &conversation).ok_or_else(|| AaiError::new("CONTEXT_NOT_FOUND", "the channel conversation is not open"))?;
                let after: i64 = input["cursor"].as_str().and_then(|c| c.parse().ok()).unwrap_or(0);
                let limit = input["limit"].as_i64().unwrap_or(200).clamp(1, 500);
                let conn = db.connect().map_err(|e| internal(e.to_string()))?;
                let mut q = conn
                    .prepare("SELECT rowid, remote_id, detail_json FROM channel_message_log WHERE binding_id = ?1 AND direction = 'inbound' AND kind = 'message' AND rowid > ?2 ORDER BY rowid LIMIT ?3")
                    .map_err(|e| internal(e.to_string()))?;
                let rows: Vec<(i64, String, String)> = q
                    .query_map(params![b.id, after, limit], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                    .map_err(|e| internal(e.to_string()))?
                    .filter_map(Result::ok)
                    .collect();
                let (who, whose, how) = who_whose_how(binding);
                let cursor = rows.last().map(|r| r.0.to_string()).or_else(|| input["cursor"].as_str().map(str::to_string));
                let events: Vec<Value> = rows
                    .iter()
                    .map(|(_, remote, detail)| {
                        let d: Value = serde_json::from_str(detail).unwrap_or(Value::Null);
                        json!({
                            "type": "agent.message.completed", "remote_event_id": remote, "lane": "channel", "guarantee": "best_effort",
                            "who": who, "whose": whose, "how": how,
                            "payload": { "text": d["text"] },
                        })
                    })
                    .collect();
                Ok(json!({ "events": events, "cursor": cursor }))
            }
            "agent.context.close" => Ok(json!({ "closed": true })),
            other => Err(AaiError::new("UNSUPPORTED", format!("{other} is not available on a channel lane"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeHttp {
        sent: Mutex<Vec<HttpReq>>,
        reply: Mutex<Option<Result<HttpResp, String>>>,
    }
    #[async_trait]
    impl HttpSend for FakeHttp {
        async fn post_json(&self, req: HttpReq) -> Result<HttpResp, String> {
            self.sent.lock().unwrap().push(req);
            self.reply.lock().unwrap().clone().unwrap_or(Ok(HttpResp { status: 200, body: json!({}) }))
        }
    }
    fn reply(status: u16, body: Value) -> Option<Result<HttpResp, String>> {
        Some(Ok(HttpResp { status, body }))
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), v.parse().unwrap());
        }
        h
    }
    fn hmac_raw(key: &[u8], body: &[u8]) -> Vec<u8> {
        let mut m = HmacSha256::new_from_slice(key).unwrap();
        m.update(body);
        m.finalize().into_bytes().to_vec()
    }

    // ---- Teams (Bot Framework activity JSON)

    #[test]
    fn teams_verifies_the_outgoing_webhook_hmac() {
        let key = b"super-secret-key-bytes";
        let token = base64::engine::general_purpose::STANDARD.encode(key);
        let body = br#"{"type":"message"}"#;
        let good = format!("HMAC {}", base64::engine::general_purpose::STANDARD.encode(hmac_raw(key, body)));
        let t = TeamsTransport { http: Arc::new(FakeHttp::default()), access_token: None, own_identity: None };
        assert!(t.verify(&json!({ "securityToken": token }).to_string(), &headers(&[("authorization", &good)]), body).is_ok());
        assert!(t.verify(&token, &headers(&[("authorization", &good)]), body).is_ok());
        assert!(t.verify(&token, &headers(&[("authorization", &good)]), br#"{"type":"tampered"}"#).is_err());
        assert!(t.verify(&token, &headers(&[]), body).is_err());
    }

    #[test]
    fn teams_activities_normalize() {
        let conv = "19:abc@thread.tacv2;messageid=1700000000000";
        let msg = json!({ "type": "message", "id": "1700000000123", "timestamp": "2026-01-01T00:00:00Z", "serviceUrl": "https://smba.trafficmanager.net/amer/", "channelId": "msteams",
            "from": { "id": "29:user", "name": "Sam" }, "conversation": { "id": conv, "conversationType": "channel", "tenantId": "t1" }, "recipient": { "id": "28:bot" }, "text": "hello team" });
        let e = &teams_normalize(&msg)[0];
        assert_eq!((e.kind, e.conversation.as_str(), e.text.as_deref(), e.cursor.as_deref()), (InboundKind::Message, format!("teams:{conv}").as_str(), Some("hello team"), Some("1700000000123")));
        assert_eq!(e.workspace.as_deref(), Some("https://smba.trafficmanager.net/amer/"));
        assert!(!e.own);
        let mut bot = msg.clone();
        bot["from"]["role"] = json!("bot");
        assert!(teams_normalize(&bot)[0].own);
        let react = json!({ "type": "messageReaction", "conversation": { "id": conv }, "from": { "id": "29:user" }, "replyToId": "1700000000123", "timestamp": "t", "reactionsAdded": [{ "type": "like" }], "reactionsRemoved": [{ "type": "heart" }] });
        let r = teams_normalize(&react);
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].added, r[0].reaction.as_deref(), r[0].message_id.as_str()), (Some(true), Some("like"), "1700000000123"));
        assert_eq!(r[1].added, Some(false));
        let mut upd = msg.clone();
        upd["type"] = json!("messageUpdate");
        assert_eq!(teams_normalize(&upd)[0].kind, InboundKind::Edited);
        let mut del = msg.clone();
        del["type"] = json!("messageDelete");
        assert_eq!(teams_normalize(&del)[0].kind, InboundKind::Deleted);
        assert!(teams_normalize(&json!({ "type": "typing", "conversation": { "id": conv } })).is_empty());
    }

    #[tokio::test]
    async fn teams_posts_to_the_service_url_and_reads_the_activity_id() {
        let http = Arc::new(FakeHttp::default());
        *http.reply.lock().unwrap() = reply(201, json!({ "id": "1700000000999" }));
        let t = TeamsTransport { http: http.clone(), access_token: Some("tok".into()), own_identity: Some("28:bot".into()) };
        let out = Outbound { workspace: Some("https://smba.trafficmanager.net/amer/".into()), channel: "19:abc@thread.tacv2".into(), thread: None, text: "hi".into(), identity: None };
        assert_eq!(t.post(&out).await.unwrap(), Receipt { remote_id: "1700000000999".into(), relayed: false });
        let sent = http.sent.lock().unwrap()[0].clone();
        assert_eq!(sent.url, "https://smba.trafficmanager.net/amer/v3/conversations/19:abc@thread.tacv2/activities");
        assert_eq!(sent.headers[0].1, "Bearer tok");
        assert_eq!(sent.body["type"], "message");
        let relayed = t.post(&Outbound { identity: Some("Gizzi".into()), ..out.clone() }).await.unwrap();
        assert!(relayed.relayed);
        assert_eq!(http.sent.lock().unwrap()[1].body["text"], "Gizzi: hi");
        *http.reply.lock().unwrap() = reply(503, json!({}));
        assert!(matches!(t.post(&out).await, Err(PostError::Uncertain(_))));
        *http.reply.lock().unwrap() = reply(403, json!({ "error": "forbidden" }));
        assert!(matches!(t.post(&out).await, Err(PostError::Rejected(_))));
        *http.reply.lock().unwrap() = Some(Err("timeout".into()));
        assert!(matches!(t.post(&out).await, Err(PostError::Uncertain(_))));
    }

    // ---- Discord (gateway dispatch + webhook post)

    #[test]
    fn discord_verifies_ed25519_and_normalizes_gateway_events() {
        use ed25519_dalek::{Signer, SigningKey};
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let pk = hex::encode(sk.verifying_key().to_bytes());
        let body = br#"{"type":1}"#;
        let ts = "1700000000";
        let mut msg = ts.as_bytes().to_vec();
        msg.extend_from_slice(body);
        let sig = hex::encode(sk.sign(&msg).to_bytes());
        let d = DiscordTransport { http: Arc::new(FakeHttp::default()), webhook_url: None, own_identity: None };
        let secret = json!({ "publicKey": pk }).to_string();
        assert!(d.verify(&secret, &headers(&[("x-signature-ed25519", &sig), ("x-signature-timestamp", ts)]), body).is_ok());
        assert!(d.verify(&secret, &headers(&[("x-signature-ed25519", &sig), ("x-signature-timestamp", "1700000001")]), body).is_err());
        assert!(d.verify(&secret, &headers(&[("x-signature-ed25519", &sig), ("x-signature-timestamp", ts)]), b"{}").is_err());

        let create = json!({ "op": 0, "t": "MESSAGE_CREATE", "s": 42, "d": { "id": "1100000000000000001", "channel_id": "222", "guild_id": "333", "content": "hey", "author": { "id": "444", "username": "sam", "bot": false } } });
        let e = &discord_normalize(&create)[0];
        assert_eq!((e.conversation.as_str(), e.workspace.as_deref(), e.text.as_deref(), e.own), ("discord:222", Some("333"), Some("hey"), false));
        assert!(discord_normalize(&json!({ "t": "MESSAGE_CREATE", "d": { "id": "1", "channel_id": "222", "author": { "id": "9", "bot": true } } }))[0].own);
        assert_eq!(discord_normalize(&json!({ "t": "MESSAGE_UPDATE", "d": { "id": "1", "channel_id": "222", "content": "hey!", "edited_timestamp": "x" } }))[0].kind, InboundKind::Edited);
        assert_eq!(discord_normalize(&json!({ "t": "MESSAGE_DELETE", "d": { "id": "1", "channel_id": "222" } }))[0].kind, InboundKind::Deleted);
        let r = &discord_normalize(&json!({ "t": "MESSAGE_REACTION_REMOVE", "s": 5, "d": { "user_id": "444", "channel_id": "222", "message_id": "1", "emoji": { "name": "fire" } } }))[0];
        assert_eq!((r.reaction.as_deref(), r.added, r.message_id.as_str()), (Some("fire"), Some(false), "1"));
        assert!(discord_normalize(&json!({ "t": "TYPING_START", "d": { "channel_id": "222" } })).is_empty());
    }

    #[tokio::test]
    async fn discord_posts_through_the_webhook_and_flags_identity_relays() {
        let http = Arc::new(FakeHttp::default());
        *http.reply.lock().unwrap() = reply(200, json!({ "id": "1100000000000000777" }));
        let d = DiscordTransport { http: http.clone(), webhook_url: Some("https://discord.com/api/webhooks/1/abc".into()), own_identity: None };
        let out = Outbound { workspace: None, channel: "222".into(), thread: Some("555".into()), text: "hi".into(), identity: Some("Gizzi".into()) };
        let r = d.post(&out).await.unwrap();
        assert!(r.relayed);
        let sent = http.sent.lock().unwrap()[0].clone();
        assert_eq!(sent.url, "https://discord.com/api/webhooks/1/abc?wait=true&thread_id=555");
        assert_eq!(sent.body["username"], "Gizzi");
        assert_eq!(r.remote_id, "1100000000000000777");
    }

    // ---- WhatsApp Business Cloud API

    fn wa_payload() -> Value {
        json!({ "object": "whatsapp_business_account", "entry": [{ "id": "WABA1", "changes": [{ "field": "messages", "value": {
            "messaging_product": "whatsapp", "metadata": { "display_phone_number": "15550001111", "phone_number_id": "PN1" },
            "contacts": [{ "profile": { "name": "Sam" }, "wa_id": "15551234567" }],
            "messages": [
                { "from": "15551234567", "id": "wamid.A", "timestamp": "1700000000", "type": "text", "text": { "body": "hello" } },
                { "from": "15551234567", "id": "wamid.R", "timestamp": "1700000001", "type": "reaction", "reaction": { "message_id": "wamid.OUT", "emoji": "\u{1F44D}" } }
            ],
            "statuses": [{ "id": "wamid.OUT", "status": "delivered", "timestamp": "1700000002", "recipient_id": "15551234567" }]
        } }] }] })
    }

    #[test]
    fn whatsapp_verifies_the_app_secret_signature_and_normalizes_entries() {
        let body = wa_payload().to_string();
        let sig = format!("sha256={}", hex::encode(hmac_raw(b"app-secret", body.as_bytes())));
        let w = WhatsAppTransport { http: Arc::new(FakeHttp::default()), access_token: None, own_identity: None };
        let secret = json!({ "appSecret": "app-secret" }).to_string();
        assert!(w.verify(&secret, &headers(&[("x-hub-signature-256", &sig)]), body.as_bytes()).is_ok());
        assert!(w.verify(&secret, &headers(&[("x-hub-signature-256", &sig)]), b"{}").is_err());
        assert!(w.verify("wrong", &headers(&[("x-hub-signature-256", &sig)]), body.as_bytes()).is_err());
        let evs = w.normalize(&serde_json::from_str(&body).unwrap());
        assert_eq!(evs.len(), 3);
        assert_eq!((evs[0].kind, evs[0].conversation.as_str(), evs[0].text.as_deref(), evs[0].cursor.as_deref()), (InboundKind::Message, "whatsapp:PN1:15551234567", Some("hello"), Some("1700000000")));
        assert_eq!((evs[1].kind, evs[1].message_id.as_str(), evs[1].added), (InboundKind::ReactionUpdated, "wamid.OUT", Some(true)));
        assert_eq!((evs[2].kind, evs[2].remote_id.as_str(), evs[2].text.as_deref()), (InboundKind::Delivery, "status:wamid.OUT:delivered", Some("delivered")));
    }

    #[tokio::test]
    async fn whatsapp_posts_a_text_message_to_the_recipient() {
        let http = Arc::new(FakeHttp::default());
        *http.reply.lock().unwrap() = reply(200, json!({ "messaging_product": "whatsapp", "messages": [{ "id": "wamid.SENT" }] }));
        let w = WhatsAppTransport { http: http.clone(), access_token: Some("tok".into()), own_identity: None };
        let out = Outbound { workspace: None, channel: "PN1".into(), thread: Some("15551234567".into()), text: "hi".into(), identity: None };
        assert_eq!(w.post(&out).await.unwrap().remote_id, "wamid.SENT");
        let sent = http.sent.lock().unwrap()[0].clone();
        assert_eq!(sent.url, "https://graph.facebook.com/v20.0/PN1/messages");
        assert_eq!((sent.body["to"].as_str(), sent.body["text"]["body"].as_str()), (Some("15551234567"), Some("hi")));
    }

    // ---- routing + Muse lane

    struct Rt;
    impl crate::thread_routes::ThreadRuntime for Rt {
        async fn create_session(&self, _b: &str, _n: &str, _t: &str, _c: bool, id: &str) -> Result<String, String> {
            Ok(format!("sess-{id}"))
        }
        async fn seed(&self, _s: &str, _t: &str) -> Result<(), String> {
            Ok(())
        }
        async fn handoff(&self, _s: &str, _r: &str, _c: &str, _b: Option<Value>) -> Result<(String, Value), String> {
            Err("no".into())
        }
    }

    async fn setup(tag: &str, secret: &str, provider: &str, restricted: Option<&str>) -> (Arc<AppState>, Account) {
        let dir = std::env::temp_dir().join(format!("allternit-ct-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_helpers::app_state(&dir).await;
        let c = st.db.connect().unwrap();
        c.execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot-1','user-a','b','m','p',1,'{}')", []).unwrap();
        c.execute(
            "INSERT INTO provider_account_bindings (id, owner, vendor, auth_type, secret_ref, restricted_bot_id, state, created_at, updated_at) VALUES ('acct-1','user-a',?1,'api_key',?2,?3,'CONNECTED','2026-01-01','2026-01-01')",
            params![provider, crate::token_crypto::seal(secret), restricted],
        )
        .unwrap();
        let a = accounts(&st.db, provider, None).remove(0);
        (st, a)
    }

    #[tokio::test]
    async fn an_inbound_message_creates_one_thread_and_binding_and_replays_do_nothing() {
        let (st, acct) = setup("route", &json!({ "appSecret": "s" }).to_string(), "whatsapp", Some("bot-1")).await;
        assert_eq!(acct.secret, json!({ "appSecret": "s" }).to_string());
        let evs = whatsapp_normalize(&wa_payload());
        let first = route_inbound(&st.db, &Rt, &acct, "whatsapp", &evs[0]).await.unwrap();
        assert_eq!(first.recorded, Recorded::New);
        let (_, bot, text) = first.turn.expect("a bot turn");
        assert_eq!(bot, "bot-1");
        assert!(text.contains("hello"));
        let replay = route_inbound(&st.db, &Rt, &acct, "whatsapp", &evs[0]).await.unwrap();
        assert_eq!(replay.recorded, Recorded::Duplicate);
        assert!(replay.turn.is_none());
        let n: i64 = st.db.connect().unwrap().query_row("SELECT COUNT(*) FROM bot_threads", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        // The reaction lands on the same thread; a status event marks the outbound delivered.
        let b = first.binding.unwrap();
        st.db.connect().unwrap().execute(
            "INSERT INTO channel_message_log (id, owner, binding_id, thread_id, direction, kind, remote_id, correlation_id, state, created_at, updated_at) VALUES ('m1','user-a',?1,?2,'outbound','message','wamid.OUT','c1','unconfirmed','t','t')",
            params![b.id, b.thread_id],
        ).unwrap();
        for e in &evs[1..] {
            assert_eq!(route_inbound(&st.db, &Rt, &acct, "whatsapp", e).await.unwrap().recorded, Recorded::New);
        }
        let state: String = st.db.connect().unwrap().query_row("SELECT state FROM channel_message_log WHERE id='m1'", [], |r| r.get(0)).unwrap();
        assert_eq!(state, "confirmed");
        let ev_n: i64 = st.db.connect().unwrap().query_row("SELECT COUNT(*) FROM bot_events WHERE thread_id=?1 AND event_type IN ('channel.message.received','channel.reaction.updated','channel.message.delivery')", params![b.thread_id], |r| r.get(0)).unwrap();
        assert_eq!(ev_n, 3);
    }

    #[tokio::test]
    async fn an_account_with_no_bot_ignores_unknown_conversations() {
        let (st, acct) = setup("nobot", "s", "discord", None).await;
        let e = &discord_normalize(&json!({ "t": "MESSAGE_CREATE", "d": { "id": "1", "channel_id": "2", "content": "x", "author": { "id": "3" } } }))[0];
        let r = route_inbound(&st.db, &Rt, &acct, "discord", e).await.unwrap();
        assert!(r.binding.is_none() && r.turn.is_none());
    }

    #[tokio::test]
    async fn muse_over_whatsapp_sends_and_returns_replies_as_agent_message_completed() {
        let secret = json!({ "appSecret": "s", "accessToken": "tok", "phoneNumberId": "PN1" }).to_string();
        let (st, acct) = setup("muse", &secret, "whatsapp", None).await;
        let c = st.db.connect().unwrap();
        c.execute("INSERT INTO bot_threads (id, user_id, bot_id, title, status, last_activity_at, created_at, updated_at) VALUES ('th-m','user-a','bot-1','T','working','t','t','t')", []).unwrap();
        c.execute("INSERT INTO bot_thread_sessions (thread_id, generation, session_id, started_at) VALUES ('th-m',1,'s-th-m','t')", []).unwrap();
        c.execute(
            "INSERT INTO bot_execution_bindings (id, owner, bot_id, type, vendor, adapter_id, account_binding_id, preferred_lane, external_agent_id, capabilities_json, state)
             VALUES ('eb-m','user-a','bot-1','vendor','meta','whatsapp','acct-1','channel','15559990000','{\"identity\":{\"who\":\"Muse\"}}','READY')",
            [],
        ).unwrap();
        let binding = json!({ "id": "eb-m", "preferredLane": "channel", "vendor": "meta", "adapterId": "whatsapp", "accountBindingId": "acct-1", "externalAgentId": "15559990000", "capabilities": { "identity": { "who": "Muse" } } });
        let http = Arc::new(FakeHttp::default());
        *http.reply.lock().unwrap() = reply(200, json!({ "messages": [{ "id": "wamid.OUT1" }] }));
        struct Inner;
        #[async_trait]
        impl AaiTransport for Inner {
            async fn call(&self, _o: &str, _op: &str, _b: &Value, _i: Value) -> Result<Value, AaiError> {
                Err(AaiError::new("PASSTHROUGH", "inner"))
            }
        }
        let lane = ChannelLaneTransport::with_http(st.clone(), Arc::new(Inner), http.clone());
        let open = lane.call("user-a", "agent.context.open", &binding, json!({ "threadId": "th-m", "generation": 1 })).await.unwrap();
        let ctx = open["contextId"].as_str().unwrap().to_string();
        assert_eq!(ctx, "whatsapp:PN1:15559990000");
        lane.call("user-a", "agent.context.message", &binding, json!({ "contextId": ctx, "text": "make me a logo", "correlationId": "corr-1" })).await.unwrap();
        let sent = http.sent.lock().unwrap()[0].clone();
        assert_eq!((sent.body["to"].as_str(), sent.body["text"]["body"].as_str()), (Some("15559990000"), Some("make me a logo")));
        // Same correlation id: not sent twice.
        lane.call("user-a", "agent.context.message", &binding, json!({ "contextId": ctx, "text": "make me a logo", "correlationId": "corr-1" })).await.unwrap();
        assert_eq!(http.sent.lock().unwrap().len(), 1);
        // Muse answers on WhatsApp: the webhook records it, and no bot turn runs.
        let mut p = wa_payload();
        p["entry"][0]["changes"][0]["value"]["messages"][0]["from"] = json!("15559990000");
        p["entry"][0]["changes"][0]["value"]["messages"][0]["text"]["body"] = json!("here is your logo");
        let e = &whatsapp_normalize(&p)[0];
        let routed = route_inbound(&st.db, &Rt, &acct, "whatsapp", e).await.unwrap();
        assert_eq!(routed.recorded, Recorded::New);
        assert!(routed.turn.is_none());
        let ev = lane.call("user-a", "agent.events", &binding, json!({ "contextId": ctx })).await.unwrap();
        let first = &ev["events"][0];
        assert_eq!(first["type"], "agent.message.completed");
        assert_eq!((first["who"].as_str(), first["whose"].as_str(), first["how"].as_str()), (Some("Muse"), Some("Meta"), Some("WhatsApp")));
        assert_eq!(first["payload"]["text"], "here is your logo");
        // Resuming from the returned cursor yields nothing new.
        let again = lane.call("user-a", "agent.events", &binding, json!({ "contextId": ctx, "cursor": ev["cursor"] })).await.unwrap();
        assert!(again["events"].as_array().unwrap().is_empty());
        // Other lanes pass through untouched.
        let other = json!({ "preferredLane": "api" });
        assert_eq!(lane.call("user-a", "agent.events", &other, json!({})).await.unwrap_err().code, "PASSTHROUGH");
    }
}
