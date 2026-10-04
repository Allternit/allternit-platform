//! A bot's phone number on this runtime: SMS as a [`ChannelTransport`]
//! (provider `sms`) and the one thread a caller's calls and texts share.
//!
//! * Inbound: the cloud verifies the carrier, dedupes and handles STOP/HELP
//!   (so an opted-out sender never gets here), then relays the carrier's request
//!   to `/webhooks/channels/sms`. [`SmsTransport::verify`] re-checks Telnyx's
//!   Ed25519 signature with the public key in the connection's secret.
//! * Outbound: the Allternit-owned carrier key never reaches the runtime, so a
//!   reply is `POST <cloud>/api/v1/channels/sms/send {numberId, to, text}` →
//!   `{messageId, status}`, bearer-authenticated as the user, or as the runtime itself
//!   (device credential, [`crate::phone_sync`]) when a synced number has no user token. The cloud refuses
//!   opted-out recipients, inactive numbers and cold outreach.
//! * Threads: key `phone:<botE164>:<callerE164>` ([`resolve_thread`]), the same
//!   for a text and a call from one caller.
//!
//! Secret (sealed `provider_account_bindings.secret_ref`, vendor `sms`, one per number):
//! `{ token, numberId, publicKey, cloudUrl? }`. `publicKey` is Telnyx's webhook
//! public key (base64); `cloudUrl` defaults to `ALLTERNIT_CLOUD_API_URL`, then api.allternit.com.
//! Telnyx webhook shape: https://developers.telnyx.com/docs/messaging/messages/receiving-webhooks

use std::sync::Arc;

use async_trait::async_trait;
use axum::http::HeaderMap;
use base64::Engine;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

use crate::channel_gateway::*;
use crate::channel_transports::{pick, HttpReq, HttpSend};
use crate::db::DbHandle;
use crate::thread_routes::ThreadRuntime;

/// Longest single SMS body the cloud send route accepts.
pub const SMS_MAX_CHARS: usize = 1600;

/// `+` and 8–15 digits, the E.164 shape.
pub fn is_e164(s: &str) -> bool {
    s.strip_prefix('+').is_some_and(|d| (8..=15).contains(&d.len()) && d.bytes().all(|b| b.is_ascii_digit()))
}

/// The conversation key a number and caller share across texts and calls.
pub fn phone_key(bot_e164: &str, caller_e164: &str) -> String {
    format!("phone:{bot_e164}:{caller_e164}")
}

// ---------------------------------------------------------------- numbers

#[derive(Debug, Clone, PartialEq)]
pub struct PhoneNumber {
    pub number_id: String,
    pub owner: String,
    pub bot_id: String,
    pub e164: String,
}

pub fn number(db: &DbHandle, number_id: &str) -> Option<PhoneNumber> {
    db.connect()
        .ok()?
        .query_row("SELECT number_id, owner, bot_id, e164 FROM channel_phone_numbers WHERE number_id = ?1", params![number_id], |r| {
            Ok(PhoneNumber { number_id: r.get(0)?, owner: r.get(1)?, bot_id: r.get(2)?, e164: r.get(3)? })
        })
        .optional()
        .ok()
        .flatten()
}

/// Record (or move) a number's bot. The bot must belong to `owner`.
pub fn upsert_number(db: &DbHandle, number_id: &str, owner: &str, bot_id: &str, e164: &str, account_id: Option<&str>) -> Result<(), String> {
    if !is_e164(e164) {
        return Err("the number must be E.164, like +14155550123".into());
    }
    let conn = db.connect().map_err(|e| e.to_string())?;
    conn.query_row("SELECT 1 FROM agents WHERE id = ?1 AND user_id = ?2", params![bot_id, owner], |_| Ok(())).map_err(|_| "that bot doesn't exist".to_string())?;
    let t = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO channel_phone_numbers (number_id, owner, bot_id, e164, account_id, created_at, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?6)
         ON CONFLICT(number_id) DO UPDATE SET bot_id = excluded.bot_id, e164 = excluded.e164, account_id = COALESCE(excluded.account_id, account_id), updated_at = excluded.updated_at
         WHERE owner = excluded.owner",
        params![number_id, owner, bot_id, e164, account_id, t],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------- threads

/// The thread (and its live session) for `caller_e164` on number `number_id`.
/// A text and a call from the same caller resolve to the same thread; a new
/// caller opens one on the number's bot. Sync so a voice worker can call it
/// from anywhere; inside a runtime it never blocks the executor's other tasks.
pub fn resolve_thread<R: ThreadRuntime>(db: &DbHandle, rt: &R, number_id: &str, caller_e164: &str) -> Result<(String, String), String> {
    let fut = resolve_thread_async(db, rt, number_id, caller_e164);
    match tokio::runtime::Handle::try_current() {
        Ok(h) if h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => tokio::task::block_in_place(|| h.block_on(fut)),
        // A single-threaded (or no) runtime can't be blocked on: run on a helper thread.
        _ => std::thread::scope(|s| {
            s.spawn(|| tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?.block_on(fut))
                .join()
                .unwrap_or_else(|_| Err("thread resolution panicked".into()))
        }),
    }
}

pub async fn resolve_thread_async<R: ThreadRuntime>(db: &DbHandle, rt: &R, number_id: &str, caller_e164: &str) -> Result<(String, String), String> {
    if !is_e164(caller_e164) {
        return Err("the caller's number must be E.164".into());
    }
    let n = number(db, number_id).ok_or("that phone number isn't set up on this runtime")?;
    let key = phone_key(&n.e164, caller_e164);
    let title = format!("Phone: {caller_e164}");
    let objective = format!("Calls and texts between {caller_e164} and this bot on {}.", n.e164);
    let session = crate::thread_routes::channel_thread(db, rt, &n.bot_id, "phone", &key, &title, &objective).await?;
    let thread_id: String = db
        .connect()
        .map_err(|e| e.to_string())?
        .query_row("SELECT thread_id FROM bot_thread_sessions WHERE session_id = ?1", params![session], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    Ok((thread_id, session))
}

// ---------------------------------------------------------------- connect (HTTP)

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectBody {
    pub e164: String,
    pub bot_id: String,
    /// The user's Allternit bearer the cloud send route accepts, kept sealed.
    pub token: String,
    /// Telnyx webhook public key (base64).
    pub public_key: String,
    pub cloud_url: Option<String>,
}

pub fn phone_router() -> axum::Router<Arc<crate::AppState>> {
    axum::Router::new().route("/gateway/phone-numbers/:number_id", axum::routing::put(connect_h).delete(disconnect_h))
}

fn fail(status: axum::http::StatusCode, msg: impl Into<String>) -> axum::response::Response {
    use axum::response::IntoResponse;
    (status, axum::Json(json!({ "error": msg.into() }))).into_response()
}

/// Attach a cloud-bought number to a bot on this runtime: records the number,
/// the sealed connection (`sms` account, one per number) and the bot as its
/// default, so inbound texts open threads and replies can be sent.
async fn connect_h(
    axum::extract::State(state): axum::extract::State<Arc<crate::AppState>>,
    axum::Extension(user): axum::Extension<crate::auth::AuthUser>,
    axum::extract::Path(number_id): axum::extract::Path<String>,
    axum::Json(b): axum::Json<ConnectBody>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    if !crate::token_crypto::encryption_enabled() {
        return fail(StatusCode::SERVICE_UNAVAILABLE, "no encryption key is configured; the token was not stored");
    }
    if b.token.trim().is_empty() || b.public_key.trim().is_empty() {
        return fail(StatusCode::BAD_REQUEST, "token and publicKey are required");
    }
    let secret = json!({ "token": b.token, "numberId": number_id, "publicKey": b.public_key, "cloudUrl": b.cloud_url }).to_string();
    let t = crate::agent_gateway_routes::now();
    let res = (|| -> Result<String, String> {
        upsert_number(&state.db, &number_id, &user.user_id, &b.bot_id, &b.e164, None)?;
        let conn = state.db.connect().map_err(|e| e.to_string())?;
        let existing: Option<String> = conn
            .query_row("SELECT id FROM provider_account_bindings WHERE vendor = 'sms' AND owner = ?1 AND external_account_id = ?2", params![user.user_id, number_id], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?;
        let sealed = crate::token_crypto::seal(&secret);
        let account = match existing {
            Some(id) => {
                conn.execute("UPDATE provider_account_bindings SET secret_ref = ?1, display_name = ?2, state = 'CONNECTED', updated_at = ?3 WHERE id = ?4", params![sealed, b.e164, t, id]).map_err(|e| e.to_string())?;
                id
            }
            None => {
                let id = crate::agent_gateway_routes::id("acct");
                conn.execute(
                    "INSERT INTO provider_account_bindings (id, owner, vendor, auth_type, external_account_id, display_name, secret_ref, scopes_json, state, verified_at, created_at, updated_at)
                     VALUES (?1, ?2, 'sms', 'channel_oauth', ?3, ?4, ?5, '[\"messages\"]', 'CONNECTED', ?6, ?6, ?6)",
                    params![id, user.user_id, number_id, b.e164, sealed, t],
                )
                .map_err(|e| e.to_string())?;
                id
            }
        };
        // One number answers as one bot: it is the connection's only member.
        conn.execute("DELETE FROM channel_account_bots WHERE account_id = ?1 AND bot_id <> ?2", params![account, b.bot_id]).map_err(|e| e.to_string())?;
        conn.execute("INSERT OR IGNORE INTO channel_account_bots (account_id, bot_id, owner, is_default, created_at) VALUES (?1, ?2, ?3, 1, ?4)", params![account, b.bot_id, user.user_id, t]).map_err(|e| e.to_string())?;
        conn.execute("UPDATE channel_phone_numbers SET account_id = ?2 WHERE number_id = ?1", params![number_id, account]).map_err(|e| e.to_string())?;
        Ok(account)
    })();
    match res {
        Ok(account) => axum::Json(json!({ "number": { "numberId": number_id, "e164": b.e164, "botId": b.bot_id, "accountId": account, "state": "CONNECTED" } })).into_response(),
        Err(e) if e.contains("E.164") || e.contains("bot doesn't exist") => fail(StatusCode::BAD_REQUEST, e),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn disconnect_h(
    axum::extract::State(state): axum::extract::State<Arc<crate::AppState>>,
    axum::Extension(user): axum::Extension<crate::auth::AuthUser>,
    axum::extract::Path(number_id): axum::extract::Path<String>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let Ok(conn) = state.db.connect() else { return fail(axum::http::StatusCode::SERVICE_UNAVAILABLE, "database unavailable") };
    let accounts: Vec<String> = conn
        .prepare("SELECT id FROM provider_account_bindings WHERE vendor = 'sms' AND owner = ?1 AND external_account_id = ?2")
        .and_then(|mut q| q.query_map(params![user.user_id, number_id], |r| r.get(0))?.collect())
        .unwrap_or_default();
    for a in &accounts {
        let _ = conn.execute("DELETE FROM channel_account_bots WHERE account_id = ?1", params![a]);
        let _ = conn.execute("DELETE FROM provider_account_bindings WHERE id = ?1", params![a]);
    }
    let _ = conn.execute("DELETE FROM channel_phone_numbers WHERE number_id = ?1 AND owner = ?2", params![number_id, user.user_id]);
    axum::Json(json!({ "ok": true })).into_response()
}

// ---------------------------------------------------------------- SMS transport

pub struct SmsTransport {
    pub http: Arc<dyn HttpSend>,
    pub cloud_url: String,
    pub token: Option<String>,
    /// The runtime's own device credential, used when no user token is sealed for the number
    /// (a number synced from the cloud has none). See [`crate::phone_sync`].
    pub runtime_token: Option<String>,
    pub number_id: Option<String>,
    /// Where bot files go to become links (SMS carries no attachments).
    pub uploader: Arc<dyn crate::cloud_files::FileUploader>,
}

impl SmsTransport {
    pub fn with_runtime_token(mut self, runtime_token: Option<String>) -> Self {
        self.runtime_token = runtime_token;
        self
    }
    pub fn with_uploader(mut self, uploader: Arc<dyn crate::cloud_files::FileUploader>) -> Self {
        self.uploader = uploader;
        self
    }
    /// The bearer a cloud send authenticates with: the sealed user token if there is one, else the runtime's.
    pub fn bearer(&self) -> Option<String> {
        self.token.clone().or_else(|| self.runtime_token.clone())
    }
}

pub fn build_sms(http: Arc<dyn HttpSend>, secret: &str) -> SmsTransport {
    let field = |k: &str| Some(pick(secret, k)).filter(|s| !s.is_empty());
    let cloud_url = field("cloudUrl")
        .or_else(|| std::env::var("ALLTERNIT_CLOUD_API_URL").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "https://api.allternit.com".into());
    SmsTransport { http, cloud_url: cloud_url.trim_end_matches('/').to_string(), token: field("token"), runtime_token: None, number_id: field("numberId"), uploader: Arc::new(crate::cloud_files::CloudUploader) }
}

/// Telnyx `message.received` → one inbound message. Conversation = the shared
/// phone key; `channel` = the bot's number; `thread` = the caller (reply address).
pub fn sms_normalize(p: &Value) -> Vec<Inbound> {
    let d = &p["data"];
    if d["event_type"].as_str() != Some("message.received") {
        return vec![];
    }
    let m = &d["payload"];
    let (Some(from), Some(to), Some(id)) = (m.pointer("/from/phone_number").and_then(Value::as_str), m.pointer("/to/0/phone_number").and_then(Value::as_str), m["id"].as_str()) else { return vec![] };
    if m["direction"].as_str().is_some_and(|d| d != "inbound") {
        return vec![];
    }
    let mut e = Inbound {
        kind: InboundKind::Message,
        workspace: None,
        channel: to.to_string(),
        conversation: phone_key(to, from),
        thread: Some(from.to_string()),
        remote_id: id.to_string(),
        message_id: id.to_string(),
        text: m["text"].as_str().map(str::to_string),
        user: Some(from.to_string()),
        reaction: None,
        added: None,
        cursor: None,
        own: false,
    };
    e.text = e.text.filter(|t| !t.trim().is_empty());
    vec![e]
}

/// Split `text` into parts of at most `max` chars, at a paragraph, line,
/// sentence or word break where one exists. Parts of a multi-part reply end
/// with " (1/3)" so the reader can order them.
pub fn split_sms(text: &str, max: usize) -> Vec<String> {
    let text = text.trim();
    if text.chars().count() <= max {
        return vec![text.to_string()];
    }
    let budget = max - 8; // room for " (12/12)"
    let mut parts: Vec<String> = Vec::new();
    let mut rest = text;
    while rest.chars().count() > budget {
        let cut_byte = rest.char_indices().nth(budget).map(|(i, _)| i).unwrap_or(rest.len());
        let window = &rest[..cut_byte];
        // Prefer a break in the back half of the window, so parts stay substantial.
        let floor = window.char_indices().nth(budget / 2).map(|(i, _)| i).unwrap_or(0);
        let at = ["\n\n", "\n", ". ", "! ", "? ", " "]
            .iter()
            .find_map(|sep| window.rfind(sep).filter(|&i| i >= floor).map(|i| i + sep.len()))
            .unwrap_or(cut_byte);
        parts.push(rest[..at].trim_end().to_string());
        rest = rest[at..].trim_start();
    }
    if !rest.is_empty() {
        parts.push(rest.to_string());
    }
    let n = parts.len();
    parts.iter().enumerate().map(|(i, p)| format!("{p} ({}/{n})", i + 1)).collect()
}

#[async_trait]
impl ChannelTransport for SmsTransport {
    fn provider(&self) -> &'static str {
        "sms"
    }
    /// Telnyx: Ed25519 over `<telnyx-timestamp>|<raw body>`, base64 signature, base64 public key.
    fn verify(&self, secret: &str, headers: &HeaderMap, body: &[u8]) -> Result<(), String> {
        use ed25519_dalek::{Signature, Verifier, VerifyingKey};
        let b64 = base64::engine::general_purpose::STANDARD;
        let h = |k: &str| headers.get(k).and_then(|v| v.to_str().ok());
        let pk = b64.decode(pick(secret, "publicKey").trim()).map_err(|_| "public key is not base64")?;
        let pk: [u8; 32] = pk.try_into().map_err(|_| "public key must be 32 bytes")?;
        let sig = b64.decode(h("telnyx-signature-ed25519").ok_or("missing telnyx-signature-ed25519")?).map_err(|_| "bad signature encoding")?;
        let sig: [u8; 64] = sig.try_into().map_err(|_| "signature must be 64 bytes")?;
        let mut msg = h("telnyx-timestamp").ok_or("missing telnyx-timestamp")?.as_bytes().to_vec();
        msg.push(b'|');
        msg.extend_from_slice(body);
        VerifyingKey::from_bytes(&pk).map_err(|_| "bad public key")?.verify(&msg, &Signature::from_bytes(&sig)).map_err(|_| "SMS signature mismatch".to_string())
    }
    fn normalize(&self, payload: &Value) -> Vec<Inbound> {
        sms_normalize(payload)
    }
    fn identity(&self, requested: Option<&str>) -> Identity {
        // One number is one bot: there is nobody to relay on behalf of.
        Identity { id: requested.map(str::to_string), exact: true }
    }
    async fn post(&self, out: &Outbound) -> Result<Receipt, PostError> {
        let token = self.bearer().ok_or_else(|| PostError::Rejected("no Allternit token is configured for this number".into()))?;
        let number_id = self.number_id.clone().ok_or_else(|| PostError::Rejected("this connection has no phone number id".into()))?;
        let to = out.thread.clone().filter(|t| is_e164(t)).ok_or_else(|| PostError::Rejected("no phone number to text for this conversation".into()))?;
        let url = format!("{}/api/v1/channels/sms/send", self.cloud_url);
        let parts = split_sms(&out.text, SMS_MAX_CHARS);
        let mut last = None;
        for (i, text) in parts.iter().enumerate() {
            let req = HttpReq { url: url.clone(), headers: vec![("Authorization".into(), format!("Bearer {token}"))], body: json!({ "numberId": number_id, "to": to, "text": text }) };
            let sent = match self.http.post_json(req).await {
                Err(e) => Err(PostError::Uncertain(e)),
                Ok(r) => match r.status {
                    200..=299 => r.body["messageId"].as_str().map(str::to_string).ok_or_else(|| PostError::Uncertain("the cloud accepted the text but returned no messageId".into())),
                    500..=599 => Err(PostError::Uncertain(format!("cloud returned {}", r.status))),
                    429 => Err(PostError::Rejected("rate limited".into())),
                    s => Err(PostError::Rejected(format!("cloud returned {s}: {}", r.body["error"].as_str().unwrap_or("send refused")))),
                },
            };
            match sent {
                Ok(id) => last = Some(id),
                // Part of the reply already went out: resending the whole thing would double it.
                Err(PostError::Rejected(e)) if i > 0 => return Err(PostError::Uncertain(format!("part {} of {} not sent: {e}", i + 1, parts.len()))),
                Err(e) => return Err(e),
            }
        }
        last.map(|remote_id| Receipt { remote_id, relayed: false }).ok_or_else(|| PostError::Rejected("nothing to send".into()))
    }
    /// SMS takes no attachments: each file goes to the owner's cloud storage (the runtime's own credential,
    /// charged to the owner's plan) and its permanent link is appended to the text. If any upload fails
    /// nothing is sent, so a person never gets a message that promises a file it lacks.
    async fn post_files(&self, out: &Outbound, files: &[crate::channel_files::ChannelFile]) -> Result<Receipt, PostError> {
        let bearer = self.runtime_token.clone().ok_or_else(|| PostError::Rejected("this runtime is not paired, so it cannot store files to send".into()))?;
        let mut text = out.text.trim_end().to_string();
        for f in files {
            let link = self.uploader.upload(&self.cloud_url, &bearer, &f.filename, &f.mime, f.data.clone()).await.map_err(|e| PostError::Rejected(format!("could not store {}: {e}", f.filename)))?;
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&format!("{}: {link}", f.filename));
        }
        self.post(&Outbound { text, ..out.clone() }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel_transports::{HttpResp, Account};
    use std::sync::Mutex;

    struct Rt;
    impl ThreadRuntime for Rt {
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

    #[derive(Default)]
    struct FakeHttp {
        sent: Mutex<Vec<HttpReq>>,
        status: Mutex<Vec<u16>>,
    }
    #[async_trait]
    impl HttpSend for FakeHttp {
        async fn post_json(&self, req: HttpReq) -> Result<HttpResp, String> {
            let n = {
                let mut s = self.sent.lock().unwrap();
                s.push(req);
                s.len()
            };
            let status = self.status.lock().unwrap().get(n - 1).copied().unwrap_or(200);
            Ok(HttpResp { status, body: if status < 300 { json!({ "messageId": format!("m{n}"), "status": "queued" }) } else { json!({ "error": "opted_out" }) } })
        }
    }

    fn telnyx(id: &str, from: &str, to: &str, text: &str) -> Value {
        json!({ "data": { "event_type": "message.received", "payload": { "id": id, "direction": "inbound", "from": { "phone_number": from }, "to": [{ "phone_number": to }], "text": text } } })
    }

    async fn setup(tag: &str) -> Arc<crate::AppState> {
        let dir = std::env::temp_dir().join(format!("allternit-phone-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_helpers::app_state(&dir).await;
        st.db.connect().unwrap().execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot-1','user-a','b','m','p',1,'{}')", []).unwrap();
        upsert_number(&st.db, "num-1", "user-a", "bot-1", "+14155550100", None).unwrap();
        st
    }

    #[test]
    fn e164_shapes() {
        assert!(is_e164("+14155550123"));
        assert!(!is_e164("14155550123") && !is_e164("+1415") && !is_e164("+1415555012x"));
    }

    #[test]
    fn telnyx_inbound_normalizes_to_the_shared_phone_key() {
        let e = sms_normalize(&telnyx("t1", "+14155550123", "+14155550100", "hi")).remove(0);
        assert_eq!((e.conversation.as_str(), e.channel.as_str(), e.thread.as_deref(), e.text.as_deref()), ("phone:+14155550100:+14155550123", "+14155550100", Some("+14155550123"), Some("hi")));
        assert!(sms_normalize(&json!({ "data": { "event_type": "message.finalized", "payload": {} } })).is_empty());
        let mut out = telnyx("t2", "+1", "+2", "x");
        out["data"]["payload"]["direction"] = json!("outbound");
        assert!(sms_normalize(&out).is_empty());
    }

    #[test]
    fn long_replies_split_politely_within_the_limit() {
        assert_eq!(split_sms("short", 1600), vec!["short"]);
        let text = (0..60).map(|i| format!("Sentence number {i} says something useful about the plan.")).collect::<Vec<_>>().join(" ");
        let parts = split_sms(&text, 1600);
        assert!(parts.len() > 1);
        assert!(parts.iter().all(|p| p.chars().count() <= 1600), "{:?}", parts.iter().map(|p| p.len()).collect::<Vec<_>>());
        assert!(parts[0].ends_with(&format!(" (1/{})", parts.len())));
        assert!(parts[0].trim_end_matches(|c: char| c != '.').len() > 1000, "breaks at a sentence end");
        // Nothing lost.
        let joined: String = parts.iter().map(|p| p.rsplit_once(" (").unwrap().0).collect::<Vec<_>>().join(" ");
        assert_eq!(joined.split_whitespace().count(), text.split_whitespace().count());
        // Multibyte text with no break points still splits on char boundaries.
        let wide = "é".repeat(4000);
        assert!(split_sms(&wide, 1600).iter().all(|p| p.chars().count() <= 1600));
    }

    #[tokio::test]
    async fn sms_attachments_become_permanent_links_in_the_text() {
        use crate::cloud_files::fakes::FakeUploader;
        let http = Arc::new(FakeHttp::default());
        let up = Arc::new(FakeUploader::default());
        let tx = build_sms(http.clone(), &json!({ "numberId": "num-1", "cloudUrl": "https://cloud.test" }).to_string()).with_runtime_token(Some("allternit_runtime_x".into())).with_uploader(up.clone());
        let out = Outbound { workspace: None, channel: String::new(), thread: Some("+14155550123".into()), text: "Here is the report".into(), identity: None };
        let file = |n: &str| crate::channel_files::ChannelFile { filename: n.into(), mime: "application/pdf".into(), data: b"%PDF".to_vec() };
        tx.post_files(&out, &[file("r.pdf")]).await.unwrap();
        assert_eq!(up.seen.lock().unwrap()[0], ("https://cloud.test".to_string(), "allternit_runtime_x".to_string(), "r.pdf".to_string(), 4));
        assert_eq!(http.sent.lock().unwrap()[0].body["text"], "Here is the report\nr.pdf: https://api.test/api/v1/files/r.pdf/raw?k=t");
        // A failed upload sends nothing.
        *up.fail.lock().unwrap() = Some("over your plan".into());
        let before = http.sent.lock().unwrap().len();
        assert!(matches!(tx.post_files(&out, &[file("b.pdf")]).await, Err(PostError::Rejected(m)) if m.contains("over your plan")));
        assert_eq!(http.sent.lock().unwrap().len(), before);
        // Unpaired: refused before any upload.
        let unpaired = build_sms(http.clone(), &json!({ "numberId": "num-1" }).to_string()).with_uploader(up);
        assert!(matches!(unpaired.post_files(&out, &[file("c.pdf")]).await, Err(PostError::Rejected(_))));
    }

    #[tokio::test]
    async fn post_goes_through_the_cloud_send_route() {
        let http = Arc::new(FakeHttp::default());
        let tx = build_sms(http.clone(), &json!({ "token": "tok", "numberId": "num-1", "cloudUrl": "https://cloud.test/" }).to_string());
        let r = tx.post(&Outbound { workspace: None, channel: "+14155550100".into(), thread: Some("+14155550123".into()), text: "hello".into(), identity: None }).await.unwrap();
        assert_eq!(r.remote_id, "m1");
        let sent = http.sent.lock().unwrap();
        assert_eq!(sent[0].url, "https://cloud.test/api/v1/channels/sms/send");
        assert_eq!(sent[0].body, json!({ "numberId": "num-1", "to": "+14155550123", "text": "hello" }));
        assert_eq!(sent[0].headers, vec![("Authorization".to_string(), "Bearer tok".to_string())]);
    }

    #[tokio::test]
    async fn a_synced_number_sends_as_the_runtime_and_a_user_token_still_wins() {
        let out = || Outbound { workspace: None, channel: String::new(), thread: Some("+14155550123".into()), text: "hi".into(), identity: None };
        // Synced: the connection holds no user token, so the runtime's own credential authenticates.
        let secret = json!({ "numberId": "num-1", "cloudUrl": "https://cloud.test" }).to_string();
        let http = Arc::new(FakeHttp::default());
        build_sms(http.clone(), &secret).with_runtime_token(Some("allternit_runtime_x".into())).post(&out()).await.unwrap();
        assert_eq!(http.sent.lock().unwrap()[0].headers, vec![("Authorization".to_string(), "Bearer allternit_runtime_x".to_string())]);
        // A sealed user token (the UI flow) is preferred.
        let both = json!({ "token": "user-tok", "numberId": "num-1", "cloudUrl": "https://cloud.test" }).to_string();
        let http = Arc::new(FakeHttp::default());
        build_sms(http.clone(), &both).with_runtime_token(Some("allternit_runtime_x".into())).post(&out()).await.unwrap();
        assert_eq!(http.sent.lock().unwrap()[0].headers[0].1, "Bearer user-tok");
        // Neither: refused before any request.
        let http = Arc::new(FakeHttp::default());
        assert!(matches!(build_sms(http.clone(), &secret).post(&out()).await, Err(PostError::Rejected(_))));
        assert!(http.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn refusals_are_definite_and_partial_sends_are_uncertain() {
        let secret = json!({ "token": "tok", "numberId": "num-1", "cloudUrl": "https://cloud.test" }).to_string();
        let to = |text: String| Outbound { workspace: None, channel: String::new(), thread: Some("+14155550123".into()), text, identity: None };
        let http = Arc::new(FakeHttp::default());
        http.status.lock().unwrap().push(403);
        let err = build_sms(http, &secret).post(&to("hi".into())).await.unwrap_err();
        assert!(matches!(err, PostError::Rejected(m) if m.contains("opted_out")));
        // Second part refused after the first went out: never "rejected" (a retry would double part one).
        let http = Arc::new(FakeHttp::default());
        http.status.lock().unwrap().extend([200, 403]);
        let long = "word ".repeat(700);
        let err = build_sms(http.clone(), &secret).post(&to(long)).await.unwrap_err();
        assert!(matches!(err, PostError::Uncertain(_)));
        assert_eq!(http.sent.lock().unwrap().len(), 2);
        // No recipient, no send.
        let none = Outbound { thread: None, ..to("x".into()) };
        assert!(matches!(build_sms(Arc::new(FakeHttp::default()), &secret).post(&none).await, Err(PostError::Rejected(_))));
    }

    #[test]
    fn verify_checks_the_telnyx_signature() {
        use ed25519_dalek::{Signer, SigningKey};
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let b64 = base64::engine::general_purpose::STANDARD;
        let secret = json!({ "publicKey": b64.encode(key.verifying_key().to_bytes()) }).to_string();
        let body = br#"{"data":{}}"#;
        let sig = b64.encode(key.sign(&[b"1700000000|".as_slice(), body].concat()).to_bytes());
        let tx = build_sms(Arc::new(FakeHttp::default()), "");
        let mut h = HeaderMap::new();
        h.insert("telnyx-timestamp", "1700000000".parse().unwrap());
        h.insert("telnyx-signature-ed25519", sig.parse().unwrap());
        assert!(tx.verify(&secret, &h, body).is_ok());
        assert!(tx.verify(&secret, &h, b"{}").is_err());
        assert!(tx.verify(&json!({}).to_string(), &h, body).is_err(), "no key, no trust");
        assert!(tx.verify(&secret, &HeaderMap::new(), body).is_err());
    }

    #[tokio::test]
    async fn texts_and_calls_from_one_caller_share_a_thread() {
        let st = setup("share").await;
        // A call arrives first, then a text, then another call.
        let (t1, s1) = resolve_thread_async(&st.db, &Rt, "num-1", "+14155550123").await.unwrap();
        let sms = sms_normalize(&telnyx("t1", "+14155550123", "+14155550100", "hello")).remove(0);
        let acct = Account { id: "acct-1".into(), owner: "user-a".into(), restricted_bot: Some("bot-1".into()), secret: String::new() };
        let routed = crate::channel_transports::route_inbound(&st.db, &Rt, &acct, "sms", &sms).await.unwrap();
        assert_eq!(routed.binding.as_ref().unwrap().thread_id, t1);
        assert_eq!(routed.turn.as_ref().unwrap().0, s1);
        assert_eq!(resolve_thread_async(&st.db, &Rt, "num-1", "+14155550123").await.unwrap(), (t1.clone(), s1.clone()));
        // Another caller is another thread; unknown numbers and bad numbers are refused.
        let (t2, _) = resolve_thread_async(&st.db, &Rt, "num-1", "+14155550999").await.unwrap();
        assert_ne!(t1, t2);
        assert!(resolve_thread_async(&st.db, &Rt, "nope", "+14155550123").await.is_err());
        assert!(resolve_thread_async(&st.db, &Rt, "num-1", "555").await.is_err());
    }

    #[tokio::test]
    async fn sync_resolve_thread_works_on_either_runtime_flavor() {
        let st = setup("sync").await;
        let a = resolve_thread(&st.db, &Rt, "num-1", "+14155550123").unwrap();
        assert_eq!(resolve_thread(&st.db, &Rt, "num-1", "+14155550123").unwrap(), a);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sync_resolve_thread_works_on_a_multi_thread_runtime() {
        let st = setup("sync-mt").await;
        let a = resolve_thread(&st.db, &Rt, "num-1", "+14155550124").unwrap();
        assert_eq!(resolve_thread(&st.db, &Rt, "num-1", "+14155550124").unwrap(), a);
    }

    #[test]
    fn a_number_belongs_to_its_owner() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let st = rt.block_on(setup("owner"));
        assert!(upsert_number(&st.db, "num-2", "user-b", "bot-1", "+14155550101", None).is_err(), "not their bot");
        assert!(upsert_number(&st.db, "num-2", "user-a", "bot-1", "4155550101", None).is_err());
        upsert_number(&st.db, "num-1", "user-b", "bot-1", "+14155550999", None).ok();
        assert_eq!(number(&st.db, "num-1").unwrap().e164, "+14155550100", "another owner can't take it over");
    }

    fn user(id: &str) -> crate::auth::AuthUser {
        crate::auth::AuthUser { user_id: id.into(), email: None, name: None, avatar_url: None, tenant_id: None, organization_id: None, organization_role: None, organization_slug: None }
    }

    #[tokio::test]
    async fn connecting_a_number_makes_it_receive_and_send() {
        use ed25519_dalek::{Signer, SigningKey};
        std::env::set_var("ALLTERNIT_ENCRYPTION_KEY", "test-phone-key");
        let st = setup("connect").await;
        let b64 = base64::engine::general_purpose::STANDARD;
        let key = SigningKey::from_bytes(&[9u8; 32]);
        let owner = user("user-a");
        let body = ConnectBody { e164: "+14155550177".into(), bot_id: "bot-1".into(), token: "tok".into(), public_key: b64.encode(key.verifying_key().to_bytes()), cloud_url: None };
        let resp = connect_h(axum::extract::State(st.clone()), axum::Extension(owner.clone()), axum::extract::Path("num-9".into()), axum::Json(body)).await;
        assert_eq!(resp.status(), 200);
        assert_eq!(number(&st.db, "num-9").unwrap().e164, "+14155550177");
        // The connection verifies a signed Telnyx request and sends with the stored token.
        let acct = crate::channel_transports::accounts(&st.db, "sms", None).remove(0);
        let tx = crate::channel_transports::build_transport("sms", &acct.secret, Arc::new(FakeHttp::default())).unwrap();
        let payload = telnyx("t9", "+14155550123", "+14155550177", "hi").to_string();
        let mut h = HeaderMap::new();
        h.insert("telnyx-timestamp", "1".parse().unwrap());
        h.insert("telnyx-signature-ed25519", b64.encode(key.sign(&[b"1|".as_slice(), payload.as_bytes()].concat()).to_bytes()).parse().unwrap());
        assert!(tx.verify(&acct.secret, &h, payload.as_bytes()).is_ok());
        assert_eq!(crate::channel_transports::member_bots(&st.db, &acct).len(), 1);
        // Another user can't connect over it; the owner can disconnect it.
        let other = user("user-b");
        let body = ConnectBody { e164: "+14155550177".into(), bot_id: "bot-1".into(), token: "x".into(), public_key: "x".into(), cloud_url: None };
        assert_eq!(connect_h(axum::extract::State(st.clone()), axum::Extension(other), axum::extract::Path("num-9".into()), axum::Json(body)).await.status(), 400);
        disconnect_h(axum::extract::State(st.clone()), axum::Extension(owner), axum::extract::Path("num-9".into())).await;
        assert!(number(&st.db, "num-9").is_none() && crate::channel_transports::accounts(&st.db, "sms", None).is_empty());
    }
}
