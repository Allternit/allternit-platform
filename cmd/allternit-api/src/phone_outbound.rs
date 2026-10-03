//! A bot (or its owner) starts a text or a call from the bot's phone number.
//!
//! Everything goes through the cloud, which holds the carrier keys and the consent gate:
//! * text → `POST <cloud>/api/v1/channels/sms/send` via the number's `sms` transport and
//!   [`channel_gateway::send`], so the text is a bot message in the
//!   `phone:<botE164>:<toE164>` thread, the same thread inbound texts and calls use.
//! * call → `POST <cloud>/api/v1/phone/calls/outbound` {numberId, to, botId, purpose}. With the
//!   cloud's outbound trunk configured it dials and answers `{consentRef, room, dialing:true}`;
//!   a `call.started` placeholder (callId = room) goes into the thread.
//! * contact → `POST <cloud>/api/v1/phone/numbers/:id/consent` {e164, source:"owner_contacts"}.
//!
//! The cloud answers 403 `no_consent` when the person never texted or called the number first and
//! isn't a contact, or has said STOP. That surfaces as [`OutError::NoConsent`].
//!
//! UI routes (under `/api/v1`, signed-in user):
//! * `POST /phone/text`     {botId?, numberId?, to, text}      → {threadId, messageId}
//! * `POST /phone/call`     {botId?, numberId?, to, purpose}   → {threadId, room}; 403 {error:"no_consent"}
//! * `POST /phone/contacts` {numberId, e164, label}            → {ok:true}
//!
//! Bot tools (the internal MCP endpoint, next to `allternit_mail.*`; listed only for owners with a
//! number): `phone_text {to, text}` and `phone_call {to, purpose}`, each with an optional `agent_id`.

use std::sync::Arc;

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::channel_gateway::{ensure_binding_on, send, Inbound, InboundKind, SendOutcome, SendReq};
use crate::channel_phone::{build_sms, is_e164, phone_key, resolve_thread_async, PhoneNumber, SMS_MAX_CHARS};
use crate::channel_transports::{HttpReq, HttpSend, ReqwestSend};
use crate::db::DbHandle;
use crate::thread_routes::ThreadRuntime;
use crate::{auth::AuthUser, AppState};

#[derive(Debug, PartialEq)]
pub enum OutError {
    /// Cloud 403 `no_consent`: the person hasn't contacted this number and isn't a contact.
    NoConsent(String),
    /// The cloud can't dial yet (outbound trunk not set up).
    CallsUnavailable,
    /// Cloud `recipient_opted_out`: the person replied STOP.
    OptedOut(String),
    /// Cloud `sms_not_active`: the number is still waiting on carrier registration.
    NotActive,
    /// The cloud's daily text cap for this number.
    DailyLimit,
    BadRequest(String),
    NotFound(String),
    Failed(String),
}

impl OutError {
    /// A plain sentence the bot can say or the UI can show.
    pub fn sentence(&self) -> String {
        match self {
            OutError::NoConsent(to) => format!("{to} hasn't texted or called this number and isn't on your allowed contacts, so I can't reach out. Add them as a contact first."),
            OutError::CallsUnavailable => "Outbound calling isn't switched on yet.".into(),
            OutError::OptedOut(to) => format!("{to} replied STOP, so no more messages can be sent to them."),
            OutError::NotActive => "Texts out aren't active on this number yet: it is still waiting on carrier registration.".into(),
            OutError::DailyLimit => "The daily limit for texts from this number has been reached. Try again tomorrow.".into(),
            OutError::BadRequest(m) | OutError::NotFound(m) | OutError::Failed(m) => m.clone(),
        }
    }
    fn status(&self) -> StatusCode {
        match self {
            OutError::NoConsent(_) => StatusCode::FORBIDDEN,
            OutError::CallsUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            OutError::OptedOut(_) | OutError::NotActive => StatusCode::FORBIDDEN,
            OutError::DailyLimit => StatusCode::TOO_MANY_REQUESTS,
            OutError::BadRequest(_) => StatusCode::BAD_REQUEST,
            OutError::NotFound(_) => StatusCode::NOT_FOUND,
            OutError::Failed(_) => StatusCode::BAD_GATEWAY,
        }
    }
    fn code(&self) -> &'static str {
        match self {
            OutError::NoConsent(_) => "no_consent",
            OutError::CallsUnavailable => "calls_unavailable",
            OutError::OptedOut(_) => "recipient_opted_out",
            OutError::NotActive => "sms_not_active",
            OutError::DailyLimit => "daily_limit",
            OutError::BadRequest(_) => "bad_request",
            OutError::NotFound(_) => "not_found",
            OutError::Failed(_) => "send_failed",
        }
    }
}

impl IntoResponse for OutError {
    fn into_response(self) -> Response {
        (self.status(), Json(json!({ "error": self.code(), "message": self.sentence() }))).into_response()
    }
}

/// The number a request means: `number_id` if given, else the one number of `bot_id`, else the
/// owner's only number.
pub fn pick_number(db: &DbHandle, owner: &str, bot_id: Option<&str>, number_id: Option<&str>) -> Result<PhoneNumber, OutError> {
    let conn = db.connect().map_err(|e| OutError::Failed(e.to_string()))?;
    let mut sql = "SELECT number_id, owner, bot_id, e164 FROM channel_phone_numbers WHERE owner = ?1".to_string();
    let mut args: Vec<String> = vec![owner.to_string()];
    for (col, v) in [("number_id", number_id), ("bot_id", bot_id)] {
        if let Some(v) = v.filter(|v| !v.trim().is_empty()) {
            args.push(v.to_string());
            sql.push_str(&format!(" AND {col} = ?{}", args.len()));
        }
    }
    let mut q = conn.prepare(&sql).map_err(|e| OutError::Failed(e.to_string()))?;
    let mut rows: Vec<PhoneNumber> = q
        .query_map(rusqlite::params_from_iter(args.iter()), |r| Ok(PhoneNumber { number_id: r.get(0)?, owner: r.get(1)?, bot_id: r.get(2)?, e164: r.get(3)? }))
        .map_err(|e| OutError::Failed(e.to_string()))?
        .filter_map(Result::ok)
        .collect();
    match rows.len() {
        0 => Err(OutError::NotFound("This bot doesn't have a phone number on this computer.".into())),
        1 => Ok(rows.remove(0)),
        _ => Err(OutError::BadRequest("There is more than one phone number here; say which one (numberId).".into())),
    }
}

/// The cloud base URL and bearer for a number, from its sealed `sms` connection.
fn cloud_for(db: &DbHandle, n: &PhoneNumber) -> Result<(String, String, String), OutError> {
    let account = db
        .connect()
        .ok()
        .and_then(|c| c.query_row("SELECT account_id FROM channel_phone_numbers WHERE number_id = ?1", params![n.number_id], |r| r.get::<_, Option<String>>(0)).optional().ok().flatten().flatten())
        .ok_or_else(|| OutError::NotFound("This phone number isn't connected here yet.".into()))?;
    let acct = crate::channel_transports::accounts(db, "sms", Some(&account)).into_iter().next().ok_or_else(|| OutError::NotFound("This phone number isn't connected here yet.".into()))?;
    let tx = build_sms(Arc::new(ReqwestSend), &acct.secret);
    let token = tx.token.clone().ok_or_else(|| OutError::NotFound("This phone number has no Allternit sign-in stored.".into()))?;
    Ok((tx.cloud_url.clone(), token, acct.secret))
}

async fn cloud_post(http: &dyn HttpSend, url: String, token: &str, body: Value) -> Result<(u16, Value), OutError> {
    let req = HttpReq { url, headers: vec![("Authorization".into(), format!("Bearer {token}"))], body };
    let r = http.post_json(req).await.map_err(|e| OutError::Failed(format!("Couldn't reach Allternit: {e}")))?;
    Ok((r.status, r.body))
}

/// The refusal slug inside the SMS transport's rejection text. The transport
/// words a 429 as "rate limited"; the cloud's only 429 on a send is its daily cap.
pub fn plain_slug(msg: &str) -> &str {
    if msg.contains("rate limited") {
        return "daily_limit";
    }
    ["no_consent", "recipient_opted_out", "sms_not_active", "daily_limit"].into_iter().find(|s| msg.contains(s)).unwrap_or("")
}

/// Text `to` from the bot's number; the text lands in the phone thread as a bot message.
pub async fn text<R: ThreadRuntime>(db: &DbHandle, rt: &R, http: Arc<dyn HttpSend>, n: &PhoneNumber, to: &str, text: &str) -> Result<(String, String), OutError> {
    text_attributed(db, rt, http, n, to, text, None).await
}

/// [`text`] with an optional attribution line appended ("(Sent by …)"), for a vendor bot
/// acting through its directing bot's number. Same gates, same thread writes.
pub async fn text_attributed<R: ThreadRuntime>(db: &DbHandle, rt: &R, http: Arc<dyn HttpSend>, n: &PhoneNumber, to: &str, text: &str, attribution: Option<&str>) -> Result<(String, String), OutError> {
    if !is_e164(to) {
        return Err(OutError::BadRequest("The number to text must look like +14155550123.".into()));
    }
    if text.trim().is_empty() {
        return Err(OutError::BadRequest("There's nothing to send.".into()));
    }
    let body = match attribution {
        Some(who) => format!("{}\n\n(Sent by {who}.)", text.trim()),
        None => text.to_string(),
    };
    if body.chars().count() > SMS_MAX_CHARS * 4 {
        return Err(OutError::BadRequest("That text is too long. Shorten it and try again.".into()));
    }
    let (_, _, secret) = cloud_for(db, n)?;
    let (thread_id, _) = resolve_thread_async(db, rt, &n.number_id, to).await.map_err(OutError::Failed)?;
    let account: Option<String> = db.connect().ok().and_then(|c| c.query_row("SELECT account_id FROM channel_phone_numbers WHERE number_id = ?1", params![n.number_id], |r| r.get(0)).ok().flatten());
    // Same conversation key and reply address an inbound text from `to` produces.
    let ev = Inbound {
        kind: InboundKind::Message,
        workspace: None,
        channel: n.e164.clone(),
        conversation: phone_key(&n.e164, to),
        thread: Some(to.to_string()),
        remote_id: String::new(),
        message_id: String::new(),
        text: None,
        user: Some(to.to_string()),
        reaction: None,
        added: None,
        cursor: None,
        own: false,
    };
    ensure_binding_on(db, &n.owner, &thread_id, "sms", &ev, account.as_deref()).map_err(|e| OutError::Failed(e.to_string()))?;
    let tx = build_sms(http, &secret);
    match send(db, &tx, &n.owner, &thread_id, &SendReq { text: body, ..Default::default() }).await {
        Ok(SendOutcome::Sent { remote_id, .. }) => Ok((thread_id, remote_id)),
        Ok(SendOutcome::Rejected(m)) => Err(match plain_slug(&m) {
            "no_consent" => OutError::NoConsent(to.to_string()),
            "recipient_opted_out" => OutError::OptedOut(to.to_string()),
            "sms_not_active" => OutError::NotActive,
            "daily_limit" => OutError::DailyLimit,
            _ => OutError::Failed(format!("The text wasn't sent: {m}")),
        }),
        Ok(SendOutcome::ReadOnly) => Err(OutError::Failed("That conversation is read-only.".into())),
        Ok(SendOutcome::Unconfirmed { .. }) => Err(OutError::Failed("I couldn't confirm the text went out; check the thread before resending.".into())),
        Ok(SendOutcome::Denied(m)) => Err(OutError::Failed(m)),
        Ok(SendOutcome::ApprovalRequired { .. }) => Err(OutError::Failed("That text is waiting for your approval.".into())),
        Ok(other) => Err(OutError::Failed(format!("The text wasn't sent ({other:?})."))),
        Err(e) => Err(OutError::Failed(e)),
    }
}

/// Ask the cloud to ring `to`; returns the thread and the call room.
pub async fn call<R: ThreadRuntime>(db: &DbHandle, rt: &R, http: Arc<dyn HttpSend>, n: &PhoneNumber, to: &str, purpose: &str) -> Result<(String, String), OutError> {
    if !is_e164(to) {
        return Err(OutError::BadRequest("The number to call must look like +14155550123.".into()));
    }
    if purpose.trim().is_empty() {
        return Err(OutError::BadRequest("Say what the call is for.".into()));
    }
    let (base, token, _) = cloud_for(db, n)?;
    let (thread_id, _) = resolve_thread_async(db, rt, &n.number_id, to).await.map_err(OutError::Failed)?;
    let (status, body) = cloud_post(http.as_ref(), format!("{base}/api/v1/phone/calls/outbound"), &token, json!({ "numberId": n.number_id, "to": to, "botId": n.bot_id, "purpose": purpose })).await?;
    match status {
        200..=299 => {}
        403 => return Err(OutError::NoConsent(to.to_string())),
        s => return Err(OutError::Failed(format!("The call wasn't placed (Allternit answered {s}: {}).", body["error"].as_str().unwrap_or("refused")))),
    }
    let Some(room) = body["room"].as_str().filter(|_| body["dialing"].as_bool() == Some(true)) else { return Err(OutError::CallsUnavailable) };
    crate::gateway_runner::led(
        db,
        &n.bot_id,
        &thread_id,
        None,
        "call.started",
        ("bot", &n.bot_id),
        json!({ "callId": room, "direction": "outbound", "from": n.e164, "to": to, "numberId": n.number_id, "purpose": purpose, "placeholder": true }),
        Some(format!("call:{room}:call.started:0")),
    );
    Ok((thread_id, room.to_string()))
}

/// Record the owner's explicit consent to text and call `e164` from this number.
pub async fn add_contact(db: &DbHandle, http: Arc<dyn HttpSend>, n: &PhoneNumber, e164: &str, label: &str) -> Result<(), OutError> {
    if !is_e164(e164) {
        return Err(OutError::BadRequest("The contact's number must look like +14155550123.".into()));
    }
    let (base, token, _) = cloud_for(db, n)?;
    let (status, body) = cloud_post(http.as_ref(), format!("{base}/api/v1/phone/numbers/{}/consent", n.number_id), &token, json!({ "e164": e164, "source": "owner_contacts", "evidence": label })).await?;
    match status {
        200..=299 => Ok(()),
        s => Err(OutError::Failed(format!("The contact wasn't saved (Allternit answered {s}: {}).", body["error"].as_str().unwrap_or("refused")))),
    }
}

// ---------------------------------------------------------------- HTTP

fn runtime(state: &Arc<AppState>) -> crate::coordinator_routes::GizziCoordinator {
    crate::coordinator_routes::GizziCoordinator { state: state.clone() }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TextBody {
    bot_id: Option<String>,
    number_id: Option<String>,
    to: String,
    text: String,
}

async fn text_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(b): Json<TextBody>) -> Response {
    let run = async {
        let n = pick_number(&state.db, &user.user_id, b.bot_id.as_deref(), b.number_id.as_deref())?;
        let (thread_id, message_id) = text(&state.db, &runtime(&state), Arc::new(ReqwestSend), &n, &b.to, &b.text).await?;
        Ok::<_, OutError>(Json(json!({ "threadId": thread_id, "messageId": message_id })).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CallBody {
    bot_id: Option<String>,
    number_id: Option<String>,
    to: String,
    purpose: String,
}

async fn call_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(b): Json<CallBody>) -> Response {
    let run = async {
        let n = pick_number(&state.db, &user.user_id, b.bot_id.as_deref(), b.number_id.as_deref())?;
        let (thread_id, room) = call(&state.db, &runtime(&state), Arc::new(ReqwestSend), &n, &b.to, &b.purpose).await?;
        Ok::<_, OutError>(Json(json!({ "threadId": thread_id, "room": room })).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContactBody {
    number_id: Option<String>,
    e164: String,
    #[serde(default)]
    label: String,
}

async fn contact_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(b): Json<ContactBody>) -> Response {
    let run = async {
        let n = pick_number(&state.db, &user.user_id, None, b.number_id.as_deref())?;
        add_contact(&state.db, Arc::new(ReqwestSend), &n, &b.e164, &b.label).await?;
        Ok::<_, OutError>((StatusCode::CREATED, Json(json!({ "ok": true }))).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

pub fn phone_outbound_router() -> Router<Arc<AppState>> {
    Router::new().route("/phone/text", post(text_h)).route("/phone/call", post(call_h)).route("/phone/contacts", post(contact_h))
}

// ---------------------------------------------------------------- bot tools

/// `phone_text` / `phone_call` descriptors, only for an owner who has a number.
pub fn mcp_tools(db: &DbHandle, owner: &str) -> Vec<Value> {
    let has_number = db
        .connect()
        .ok()
        .and_then(|c| c.query_row("SELECT COUNT(*) FROM channel_phone_numbers WHERE owner = ?1", params![owner], |r| r.get::<_, i64>(0)).ok())
        .unwrap_or(0)
        > 0;
    if !has_number {
        return vec![];
    }
    vec![
        json!({
            "name": "phone_text",
            "title": "Send a text message",
            "description": "Send an SMS from your own phone number. Works only for people who texted or called this number first, or that your owner added as contacts; STOP always wins. If it refuses, tell the person why.",
            "inputSchema": { "type": "object", "properties": {
                "to": { "type": "string", "description": "E.164, like +14155550123" },
                "text": { "type": "string", "minLength": 1 },
                "agent_id": { "type": "string" }
            }, "required": ["to", "text"], "additionalProperties": false }
        }),
        json!({
            "name": "phone_call",
            "title": "Place a phone call",
            "description": "Call someone from your own phone number to do something specific. Same consent rule as phone_text.",
            "inputSchema": { "type": "object", "properties": {
                "to": { "type": "string", "description": "E.164, like +14155550123" },
                "purpose": { "type": "string", "minLength": 1, "description": "What the call is for" },
                "agent_id": { "type": "string" }
            }, "required": ["to", "purpose"], "additionalProperties": false }
        }),
    ]
}

pub fn is_tool(name: &str) -> bool {
    matches!(name, "phone_text" | "phone_call")
}

/// Run a tool. A refusal is an `ok:false` result with a plain sentence, not a tool error.
pub async fn call_mcp_tool(state: &Arc<AppState>, owner: &str, name: &str, args: Value) -> Result<Value, (StatusCode, Json<Value>)> {
    call_mcp_tool_with(&state.db, &runtime(state), Arc::new(ReqwestSend), owner, name, args).await
}

pub async fn call_mcp_tool_with<R: ThreadRuntime>(db: &DbHandle, rt: &R, http: Arc<dyn HttpSend>, owner: &str, name: &str, args: Value) -> Result<Value, (StatusCode, Json<Value>)> {
    let arg = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let refuse = |e: OutError| Ok(json!({ "ok": false, "error": e.code(), "message": e.sentence() }));
    let n = match pick_number(db, owner, Some(arg("agent_id").as_str()), None) {
        Ok(n) => n,
        Err(e) => return refuse(e),
    };
    let out = match name {
        "phone_text" => text(db, rt, http, &n, &arg("to"), &arg("text")).await.map(|(thread_id, message_id)| json!({ "ok": true, "threadId": thread_id, "messageId": message_id })),
        "phone_call" => call(db, rt, http, &n, &arg("to"), &arg("purpose")).await.map(|(thread_id, room)| json!({ "ok": true, "threadId": thread_id, "room": room, "message": "Calling now." })),
        other => return Err((StatusCode::NOT_FOUND, Json(json!({ "error": format!("unknown tool {other}") })))),
    };
    match out {
        Ok(v) => Ok(v),
        Err(e) => refuse(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel_transports::HttpResp;
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

    /// Answers each POST in order; records `(url, body)`.
    #[derive(Default)]
    struct FakeHttp {
        sent: Mutex<Vec<(String, Value)>>,
        replies: Mutex<Vec<(u16, Value)>>,
    }
    #[async_trait::async_trait]
    impl HttpSend for FakeHttp {
        async fn post_json(&self, req: HttpReq) -> Result<HttpResp, String> {
            self.sent.lock().unwrap().push((req.url, req.body));
            let (status, body) = self.replies.lock().unwrap().remove(0);
            Ok(HttpResp { status, body })
        }
    }
    fn fake(replies: Vec<(u16, Value)>) -> Arc<FakeHttp> {
        let f = FakeHttp::default();
        *f.replies.lock().unwrap() = replies;
        Arc::new(f)
    }

    async fn setup(tag: &str) -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("phone-out-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_helpers::app_state(&dir).await;
        st.db.connect().unwrap().execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot-1','user-a','b','m','p',1,'{}')", []).unwrap();
        crate::channel_phone::upsert_number(&st.db, "num-1", "user-a", "bot-1", "+14155550100", None).unwrap();
        // The sealed sms connection a connected number has.
        let sealed = crate::token_crypto::seal(&json!({ "token": "tok", "numberId": "num-1", "publicKey": "k", "cloudUrl": "https://cloud.test" }).to_string());
        let conn = st.db.connect().unwrap();
        conn.execute(
            "INSERT INTO provider_account_bindings (id, owner, vendor, auth_type, external_account_id, display_name, secret_ref, scopes_json, state, verified_at, created_at, updated_at)
             VALUES ('acct-1','user-a','sms','channel_oauth','num-1','+14155550100',?1,'[]','CONNECTED','t','t','t')",
            params![sealed],
        )
        .unwrap();
        conn.execute("UPDATE channel_phone_numbers SET account_id = 'acct-1' WHERE number_id = 'num-1'", []).unwrap();
        st
    }

    fn events(st: &Arc<AppState>, thread: &str, ty: &str) -> i64 {
        st.db.connect().unwrap().query_row("SELECT COUNT(*) FROM bot_events WHERE thread_id = ?1 AND event_type = ?2", params![thread, ty], |r| r.get(0)).unwrap()
    }

    #[tokio::test]
    async fn text_goes_through_the_cloud_into_the_phone_thread() {
        let st = setup("text").await;
        let n = pick_number(&st.db, "user-a", None, None).unwrap();
        let http = fake(vec![(200, json!({ "ok": true, "messageId": "m1" }))]);
        let (thread, msg) = text(&st.db, &Rt, http.clone(), &n, "+14155550123", "hi there").await.unwrap();
        assert_eq!(msg, "m1");
        let sent = http.sent.lock().unwrap();
        assert_eq!(sent[0].0, "https://cloud.test/api/v1/channels/sms/send");
        assert_eq!(sent[0].1, json!({ "numberId": "num-1", "to": "+14155550123", "text": "hi there" }));
        // Same thread an inbound text from that person would use.
        assert_eq!(crate::channel_phone::resolve_thread_async(&st.db, &Rt, "num-1", "+14155550123").await.unwrap().0, thread);
        assert_eq!(events(&st, &thread, "channel.message.sent"), 1);
    }

    #[tokio::test]
    async fn no_consent_is_a_plain_sentence_and_the_text_shows_as_failed() {
        let st = setup("noc").await;
        let n = pick_number(&st.db, "user-a", None, None).unwrap();
        let http = fake(vec![(403, json!({ "error": "no_consent" }))]);
        let err = text(&st.db, &Rt, http, &n, "+14155550123", "hi").await.unwrap_err();
        assert_eq!(err, OutError::NoConsent("+14155550123".into()));
        assert!(err.sentence().contains("hasn't texted or called this number"));
        let (thread, _) = crate::channel_phone::resolve_thread_async(&st.db, &Rt, "num-1", "+14155550123").await.unwrap();
        // The thread shows it as a failed message, never as a delivered one.
        let delivery: String = st.db.connect().unwrap().query_row("SELECT json_extract(payload, '$.delivery') FROM bot_events WHERE thread_id = ?1 AND event_type = 'channel.message.sent'", params![thread], |r| r.get(0)).unwrap();
        assert_eq!(delivery, "failed", "a refused text isn't a delivered bot message");
        // The tool surfaces it as an ok:false result, not a tool error.
        let http = fake(vec![(403, json!({ "error": "no_consent" }))]);
        let out = call_mcp_tool_with(&st.db, &Rt, http, "user-a", "phone_call", json!({ "to": "+14155550123", "purpose": "remind" })).await.unwrap();
        assert_eq!((out["ok"].clone(), out["error"].clone()), (json!(false), json!("no_consent")));
    }

    #[tokio::test]
    async fn attributed_text_shares_the_gates_and_every_cloud_refusal_has_its_own_error() {
        let st = setup("attr").await;
        let n = pick_number(&st.db, "user-a", None, None).unwrap();
        let http = fake(vec![(200, json!({ "messageId": "m1" }))]);
        text_attributed(&st.db, &Rt, http.clone(), &n, "+14155550123", "Table is ready", Some("Vendor via Bot")).await.unwrap();
        let sent = http.sent.lock().unwrap()[0].clone();
        assert_eq!(sent.0, "https://cloud.test/api/v1/channels/sms/send");
        assert_eq!(sent.1["text"], "Table is ready\n\n(Sent by Vendor via Bot.)");
        for (status, slug, want) in [
            (403, "recipient_opted_out", OutError::OptedOut("+14155550123".into())),
            (403, "sms_not_active", OutError::NotActive),
            (429, "daily_limit", OutError::DailyLimit),
            (403, "no_consent", OutError::NoConsent("+14155550123".into())),
        ] {
            let http = fake(vec![(status, json!({ "error": slug }))]);
            assert_eq!(text(&st.db, &Rt, http.clone(), &n, "+14155550123", "hi").await.unwrap_err(), want, "{slug}");
        }
        let http = fake(vec![]);
        let long = "x".repeat(crate::channel_phone::SMS_MAX_CHARS * 4 + 1);
        assert!(matches!(text(&st.db, &Rt, http.clone(), &n, "+14155550123", &long).await, Err(OutError::BadRequest(_))));
        assert!(http.sent.lock().unwrap().is_empty(), "too-long text never reaches the cloud");
    }

    #[tokio::test]
    async fn call_writes_the_placeholder_and_is_inert_without_a_dial() {
        let st = setup("call").await;
        let n = pick_number(&st.db, "user-a", Some("bot-1"), None).unwrap();
        let http = fake(vec![(200, json!({ "consentRef": "cc_1", "basis": "inbound_text", "room": "call-out-abc", "dialing": true }))]);
        let (thread, room) = call(&st.db, &Rt, http.clone(), &n, "+14155550123", "confirm appointment").await.unwrap();
        assert_eq!(room, "call-out-abc");
        assert_eq!(http.sent.lock().unwrap()[0].1, json!({ "numberId": "num-1", "to": "+14155550123", "botId": "bot-1", "purpose": "confirm appointment" }));
        assert_eq!(events(&st, &thread, "call.started"), 1);
        // The cloud has no outbound trunk: consent-only answer, nothing dialed, nothing written.
        let http = fake(vec![(200, json!({ "consentRef": "cc_2", "basis": "inbound_text" }))]);
        assert_eq!(call(&st.db, &Rt, http, &n, "+14155550124", "x").await.unwrap_err(), OutError::CallsUnavailable);
    }

    #[tokio::test]
    async fn contacts_record_owner_consent_and_tools_need_a_number() {
        let st = setup("contact").await;
        let n = pick_number(&st.db, "user-a", None, Some("num-1")).unwrap();
        let http = fake(vec![(201, json!({ "ok": true }))]);
        add_contact(&st.db, http.clone(), &n, "+14155550123", "Mum").await.unwrap();
        let sent = http.sent.lock().unwrap();
        assert_eq!(sent[0].0, "https://cloud.test/api/v1/phone/numbers/num-1/consent");
        assert_eq!(sent[0].1, json!({ "e164": "+14155550123", "source": "owner_contacts", "evidence": "Mum" }));
        assert_eq!(mcp_tools(&st.db, "user-a").len(), 2);
        assert!(mcp_tools(&st.db, "user-b").is_empty());
        assert!(matches!(pick_number(&st.db, "user-b", None, None), Err(OutError::NotFound(_))));
    }
}
