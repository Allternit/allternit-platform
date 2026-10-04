//! Runtime side of phone calls (migration V218). Spec: the FROZEN call contract
//! in `HANDOFF-realtime-voice-2026-10-02.md` §4.1.
//!
//! The voice worker never talks to this runtime directly: cloud-api relays a
//! call's start, its events and its bot turns here, signed with the runtime's
//! device token. Four routes (all `/api/v1/voice/calls*`, public to the Clerk
//! middleware because they authenticate themselves, see [`crate::relay_auth`]):
//!
//! * `POST   /api/v1/voice/calls`                  start a call, resolve its thread
//! * `POST   /api/v1/voice/calls/{callId}/events`  write `call.*` events to the thread
//! * `POST   /api/v1/voice/calls/{callId}/turn`    one bot turn, streamed as SSE
//! * `DELETE /api/v1/voice/calls/{callId}/turn`    barge-in: abort the turn
//!
//! Auth: `x-allternit-runtime-sig: v1=<hex HMAC-SHA256(sha256_hex(device_token),
//! "<ts>.<METHOD>.<path>.<hex sha256(body)>")>`, `x-allternit-runtime-ts` (unix
//! seconds, ±300 s) and `x-allternit-owner`. Unsigned requests are never
//! accepted. The verifier and the device-token source live in
//! [`crate::relay_auth`]; with no token available the routes answer 503 to
//! signed requests and 401 to unsigned ones.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Extension, Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tracing::warn;

use crate::db::DbHandle;
use crate::gateway_runner::led;
use crate::relay_auth::{unix_now, EnvOrFileRelaySecret, RelayedAuth, RelaySecret};
use crate::AppState;

const MAX_EVENTS: usize = 200;
const MAX_TURN_CHARS: usize = 4000;

/// Kept server-side so a phone bot answers the way a person talks on the phone.
const SPOKEN_PREFACE: &str = "[Live phone call. Answer the way you would speak aloud: one to three short sentences, plain words, no markdown, lists or emoji. If you need a tool, use it, then say the result briefly.]";

fn fail(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

// ---------------------------------------------------------------- seams

/// Resolves the thread and session for a caller on a number. Production uses
/// `channel_phone::resolve_thread_async`; tests plug in a fake `ThreadRuntime`.
#[async_trait]
pub trait ThreadResolver: Send + Sync {
    async fn resolve(&self, db: &DbHandle, number_id: &str, caller_e164: &str) -> Result<(String, String), String>;
}

struct GizziResolver;

#[async_trait]
impl ThreadResolver for GizziResolver {
    async fn resolve(&self, db: &DbHandle, number_id: &str, caller_e164: &str) -> Result<(String, String), String> {
        let rt = crate::thread_routes::GizziRuntime { db: db.clone() };
        crate::channel_phone::resolve_thread_async(db, &rt, number_id, caller_e164).await
    }
}

/// How a turn's reply reaches the caller.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnReply {
    /// The turner already sent the `text.delta`s on `events`.
    Streamed,
    /// The finished reply; the call chunks it into sentences.
    Final(String),
}

/// Runs one bot turn in a call's session. `events` receives `text.delta` and
/// `tool` events as they happen (a turner that cannot stream returns
/// [`TurnReply::Final`] instead).
#[async_trait]
pub trait VoiceTurner: Send + Sync {
    async fn run(&self, db: &DbHandle, session_id: &str, bot_id: &str, text: &str, events: mpsc::UnboundedSender<Value>) -> Result<TurnReply, String>;
    async fn abort(&self, session_id: &str);
}

/// The channel path: `send_bot_turn` goes through `gateway_runner::run_turn`
/// for vendor bots and gizzi otherwise, so tools, memory and approvals behave
/// exactly as they do for a Slack or SMS message. It reports only the final
/// reply, so it produces no `tool` events: it is the fallback for sessions
/// that cannot be streamed.
struct ChannelTurner;

#[async_trait]
impl VoiceTurner for ChannelTurner {
    async fn run(&self, db: &DbHandle, session_id: &str, bot_id: &str, text: &str, _events: mpsc::UnboundedSender<Value>) -> Result<TurnReply, String> {
        crate::agent_session_routes::send_bot_turn(db, session_id, bot_id, text).await.map(TurnReply::Final)
    }

    /// What the Stop button does: a vendor-bound session cancels on the vendor,
    /// everything else aborts in gizzi.
    async fn abort(&self, session_id: &str) {
        if crate::gateway_runner::intercept_abort(session_id).await.is_some() {
            return;
        }
        let client = crate::agent_session_routes::gizzi_client(&HeaderMap::new());
        let url = format!("{}/v1/session/{}/abort", crate::agent_session_routes::gizzi_base(), urlencoding::encode(session_id));
        if let Err(e) = client.post(url).json(&json!({})).send().await {
            warn!("voice turn abort failed: {e}");
        }
    }
}

/// The production turner: a native gizzi session is streamed (deltas and tool
/// steps as they happen, see [`crate::voice_turn_stream`]); a vendor-bound or
/// placed (remote) session has no local event stream, so it runs the channel
/// path and answers once it is done. That fallback is logged, never silent.
struct GizziStreamTurner {
    fallback: ChannelTurner,
}

/// Why a session cannot be streamed locally, if it cannot.
fn no_stream_reason(db: &DbHandle, session_id: &str) -> Option<&'static str> {
    if crate::gateway_runner::is_vendor_session(db, session_id) {
        Some("vendor-bound")
    } else if crate::placement::session_target(db, session_id).is_some() {
        Some("placed on another Allternit")
    } else {
        None
    }
}

#[async_trait]
impl VoiceTurner for GizziStreamTurner {
    async fn run(&self, db: &DbHandle, session_id: &str, bot_id: &str, text: &str, events: mpsc::UnboundedSender<Value>) -> Result<TurnReply, String> {
        if let Some(why) = no_stream_reason(db, session_id) {
            tracing::info!(session_id, "voice turn: session is {why}; answering with the final reply instead of streaming");
            return self.fallback.run(db, session_id, bot_id, text, events).await;
        }
        let (client, path, payload) = crate::agent_session_routes::native_turn_request(db, session_id, bot_id, text).await?;
        crate::voice_turn_stream::stream_gizzi_turn(&client, &crate::agent_session_routes::gizzi_base(), session_id, &path, payload, &events).await
    }

    async fn abort(&self, session_id: &str) {
        self.fallback.abort(session_id).await
    }
}

/// Writes the owner-facing summary of a finished call. Production runs the
/// bot's own model in an ephemeral gizzi session (never the call's session,
/// never in thread history); tests plug in a fake.
#[async_trait]
pub trait CallSummarizer: Send + Sync {
    /// `Ok` = the model's JSON `{ "text": .., "followUps": [..] }`.
    async fn summarize(&self, db: &DbHandle, session_id: &str, bot_id: &str, prompt: &str) -> Result<Value, String>;
}

struct GizziSummarizer;

#[async_trait]
impl CallSummarizer for GizziSummarizer {
    async fn summarize(&self, db: &DbHandle, session_id: &str, bot_id: &str, prompt: &str) -> Result<Value, String> {
        let model = crate::agent_session_routes::bot_turn_model(db, session_id, bot_id);
        let pair = match (model["providerID"].as_str(), model["modelID"].as_str()) {
            (Some(p), Some(m)) => Some((p.to_string(), m.to_string())),
            _ => None,
        };
        let schema = summary_schema();
        let reply = crate::usage_ledger::scope(crate::usage_ledger::LedgerCtx::surface("bot"), async {
            crate::structured_output::complete_structured(prompt, Some(SUMMARY_SYSTEM), pair.as_ref(), &schema).await
        })
        .await
        .ok_or_else(|| "gizzi gave no summary".to_string())?;
        reply.value.ok_or_else(|| reply.error.unwrap_or_else(|| "the model returned no valid summary".to_string()))
    }
}

const SUMMARY_SYSTEM: &str = "You write short owner-facing summaries of phone calls a bot took. Plain words, no markdown. Never invent anything that is not in the transcript or the tool log.";

fn summary_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "text": { "type": "string", "description": "3-6 sentences: who called, what they wanted, what the bot did (including tool steps), and how the call ended." },
            "followUps": { "type": "array", "items": { "type": "string" }, "description": "Concrete follow-ups for the owner; empty when there are none." }
        },
        "required": ["text", "followUps"],
        "additionalProperties": false
    })
}

const MAX_SUMMARY_TRANSCRIPT_CHARS: usize = 24_000;

/// The call's transcript and tool steps, from its own ledger rows, as the prompt.
fn summary_prompt(db: &DbHandle, call: &CallRow, call_id: &str) -> Result<Option<String>, String> {
    let conn = db.connect().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT event_type, actor_type, payload FROM bot_events WHERE bot_id = ?1 AND thread_id = ?2 AND idempotency_key LIKE ?3 ORDER BY rowid")
        .map_err(|e| e.to_string())?;
    let like = format!("call:{call_id}:%");
    let rows = stmt
        .query_map(params![call.bot_id, call.thread_id, like], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
        .map_err(|e| e.to_string())?;
    let caller = if call.direction == "inbound" { &call.from_e164 } else { &call.to_e164 };
    let (mut lines, mut finals, mut reason, mut duration) = (Vec::new(), 0, String::new(), None);
    for row in rows {
        let (kind, actor, payload) = row.map_err(|e| e.to_string())?;
        let p: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
        match kind.as_str() {
            "call.transcript.delta" => {
                let text = p["text"].as_str().unwrap_or("").trim();
                if !text.is_empty() {
                    finals += 1;
                    let who = match actor.as_str() {
                        "caller" => "Caller",
                        "human" => "Owner (took over)",
                        _ => "Bot",
                    };
                    lines.push(format!("{who}: {text}"));
                }
            }
            "agent.tool.started" => lines.push(format!("[Bot used tool: {}]", p["data"]["name"].as_str().unwrap_or("tool"))),
            "agent.tool.failed" => lines.push(format!("[Tool failed: {}]", p["data"]["name"].as_str().unwrap_or("tool"))),
            "call.ended" => {
                reason = p["reason"].as_str().unwrap_or("").to_string();
                duration = p["durationSec"].as_u64();
            }
            _ => {}
        }
    }
    if finals == 0 {
        return Ok(None);
    }
    let mut transcript = lines.join("\n");
    if transcript.len() > MAX_SUMMARY_TRANSCRIPT_CHARS {
        let mut cut = transcript.len() - MAX_SUMMARY_TRANSCRIPT_CHARS;
        while !transcript.is_char_boundary(cut) {
            cut += 1;
        }
        transcript = format!("[earlier part of the call omitted]\n{}", &transcript[cut..]);
    }
    Ok(Some(format!(
        "Summarize this {} phone call for the bot's owner. Caller number: {caller}. Duration: {} seconds. Ended: {}.\n\nTranscript and tool log:\n{transcript}",
        call.direction,
        duration.map_or("unknown".to_string(), |d| d.to_string()),
        if reason.is_empty() { "unknown" } else { &reason },
    )))
}

/// Background: summarise an ended call and write `call.summary` to the thread.
/// A call with no final transcript segment gets none; any failure is logged
/// and dropped (no event, no retry).
async fn write_call_summary(db: DbHandle, summarizer: Arc<dyn CallSummarizer>, call: CallRow, call_id: String) {
    let prompt = match summary_prompt(&db, &call, &call_id) {
        Ok(Some(p)) => p,
        Ok(None) => return,
        Err(e) => return warn!(call_id, "call summary: couldn't read the call's events: {e}"),
    };
    let done = tokio::time::timeout(Duration::from_secs(150), summarizer.summarize(&db, &call.session_id, &call.bot_id, &prompt)).await;
    let value = match done {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return warn!(call_id, "call summary failed: {e}"),
        Err(_) => return warn!(call_id, "call summary timed out"),
    };
    let text = value["text"].as_str().unwrap_or("").trim().to_string();
    if text.is_empty() {
        return warn!(call_id, "call summary: the model returned no text");
    }
    let follow_ups: Vec<String> = value["followUps"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).take(10).collect()).unwrap_or_default();
    let payload = json!({ "callId": call_id, "text": text, "followUps": follow_ups, "n": 0 });
    led(&db, &call.bot_id, &call.thread_id, Some(&call.session_id), "call.summary", ("bot", &call.bot_id), payload, Some(format!("call:{call_id}:call.summary:0")));
}

struct InFlight {
    generation: u64,
    session_id: String,
    task: tokio::task::AbortHandle,
    out: mpsc::UnboundedSender<Value>,
}

pub struct VoiceDeps {
    pub secret: Arc<dyn RelaySecret>,
    pub resolver: Box<dyn ThreadResolver>,
    pub turner: Arc<dyn VoiceTurner>,
    pub summarizer: Arc<dyn CallSummarizer>,
    turns: Mutex<HashMap<String, InFlight>>,
    next_generation: AtomicU64,
}

impl VoiceDeps {
    pub fn new(secret: Arc<dyn RelaySecret>, resolver: Box<dyn ThreadResolver>, turner: Arc<dyn VoiceTurner>, summarizer: Arc<dyn CallSummarizer>) -> Self {
        Self { secret, resolver, turner, summarizer, turns: Mutex::new(HashMap::new()), next_generation: AtomicU64::new(1) }
    }

    pub fn production() -> Self {
        Self::new(Arc::new(EnvOrFileRelaySecret::from_process_env()), Box::new(GizziResolver), Arc::new(GizziStreamTurner { fallback: ChannelTurner }), Arc::new(GizziSummarizer))
    }

    /// Stop the call's running turn, if any (optionally only a given generation).
    /// Tells the client its stream ended aborted, cancels the task, then aborts
    /// the turn in the session. Returns whether a turn was running.
    async fn abort_turn(&self, call_id: &str, only_generation: Option<u64>) -> bool {
        let running = {
            let mut turns = self.turns.lock().unwrap();
            match turns.get(call_id) {
                Some(t) if only_generation.map_or(true, |g| g == t.generation) => turns.remove(call_id),
                _ => None,
            }
        };
        let Some(t) = running else { return false };
        let _ = t.out.send(json!({ "type": "done", "aborted": true }));
        t.task.abort();
        let turner = self.turner.clone();
        let session = t.session_id;
        // The remote abort can be slow; the caller (barge-in) never waits on it for long.
        let _ = tokio::time::timeout(Duration::from_secs(3), turner.abort(&session)).await;
        true
    }
}

// ---------------------------------------------------------------- calls

#[derive(Debug, Clone)]
struct CallRow {
    owner_id: String,
    bot_id: String,
    thread_id: String,
    session_id: String,
    from_e164: String,
    to_e164: String,
    direction: String,
    state: String,
}

fn call_row(db: &DbHandle, call_id: &str) -> Result<Option<CallRow>, String> {
    db.connect()
        .map_err(|e| e.to_string())?
        .query_row(
            "SELECT owner_id, bot_id, thread_id, session_id, from_e164, to_e164, direction, state FROM voice_calls WHERE call_id = ?1",
            params![call_id],
            |r| Ok(CallRow { owner_id: r.get(0)?, bot_id: r.get(1)?, thread_id: r.get(2)?, session_id: r.get(3)?, from_e164: r.get(4)?, to_e164: r.get(5)?, direction: r.get(6)?, state: r.get(7)? }),
        )
        .optional()
        .map_err(|e| e.to_string())
}

/// The call, only if it belongs to `owner`. Someone else's call is a 404, not a 403.
fn owned_call(db: &DbHandle, call_id: &str, owner: &str) -> Result<CallRow, Response> {
    match call_row(db, call_id) {
        Ok(Some(c)) if c.owner_id == owner => Ok(c),
        Ok(_) => Err(fail(StatusCode::NOT_FOUND, "unknown call")),
        Err(e) => Err(fail(StatusCode::INTERNAL_SERVER_ERROR, &e)),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateBody {
    call_id: String,
    bot_id: String,
    owner_id: String,
    number_id: String,
    from: String,
    to: String,
    direction: String,
    room: String,
    started_at: String,
}

async fn create_h(State(state): State<Arc<AppState>>, Extension(deps): Extension<Arc<VoiceDeps>>, auth: RelayedAuth) -> Response {
    let b: CreateBody = match auth.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    if b.call_id.is_empty() || b.call_id.len() > 128 || b.room.is_empty() || b.started_at.is_empty() {
        return fail(StatusCode::BAD_REQUEST, "callId, room and startedAt are required");
    }
    if b.owner_id != auth.owner {
        return fail(StatusCode::FORBIDDEN, "ownerId does not match the signed owner");
    }
    if b.direction != "inbound" && b.direction != "outbound" {
        return fail(StatusCode::BAD_REQUEST, "direction must be inbound or outbound");
    }
    if !crate::channel_phone::is_e164(&b.from) || !crate::channel_phone::is_e164(&b.to) {
        return fail(StatusCode::BAD_REQUEST, "from and to must be E.164");
    }
    // A repeated callId (cloud-api retries while a computer wakes) returns the same ids.
    match call_row(&state.db, &b.call_id) {
        Ok(Some(c)) if c.owner_id == auth.owner => return Json(json!({ "threadId": c.thread_id, "sessionId": c.session_id })).into_response(),
        Ok(Some(_)) => return fail(StatusCode::CONFLICT, "callId belongs to another owner"),
        Ok(None) => {}
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
    // Backstop: the runtime's number table can be empty (fresh or restored computer). The body is
    // signed by cloud-api for this owner, so register the number from it instead of failing every call.
    let number = match crate::channel_phone::number(&state.db, &b.number_id) {
        Some(n) => n,
        None => {
            let bot_e164 = if b.direction == "inbound" { &b.to } else { &b.from };
            if let Err(e) = crate::channel_phone::upsert_number(&state.db, &b.number_id, &auth.owner, &b.bot_id, bot_e164, None) {
                return fail(StatusCode::UNPROCESSABLE_ENTITY, &format!("couldn't set up that phone number on this runtime: {e}"));
            }
            tracing::info!(number_id = %b.number_id, bot_id = %b.bot_id, "voice call: registered unknown phone number from the signed call start");
            match crate::channel_phone::number(&state.db, &b.number_id) {
                Some(n) => n,
                None => return fail(StatusCode::INTERNAL_SERVER_ERROR, "phone number was not stored"),
            }
        }
    };
    if number.owner != auth.owner || number.bot_id != b.bot_id {
        return fail(StatusCode::FORBIDDEN, "that number does not belong to this owner and bot");
    }
    let caller = if b.direction == "inbound" { &b.from } else { &b.to };
    let (thread_id, session_id) = match deps.resolver.resolve(&state.db, &b.number_id, caller).await {
        Ok(ids) => ids,
        Err(e) => return fail(StatusCode::BAD_GATEWAY, &e),
    };
    let inserted = state.db.connect().and_then(|conn| {
        conn.execute(
            "INSERT OR IGNORE INTO voice_calls (call_id, owner_id, bot_id, number_id, thread_id, session_id, from_e164, to_e164, direction, room, state, started_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'active',?11)",
            params![b.call_id, b.owner_id, b.bot_id, b.number_id, thread_id, session_id, b.from, b.to, b.direction, b.room, b.started_at],
        )
    });
    if let Err(e) = inserted {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    // Lost an insert race with a concurrent retry: answer with whatever won.
    match call_row(&state.db, &b.call_id) {
        Ok(Some(c)) if c.owner_id == auth.owner => Json(json!({ "threadId": c.thread_id, "sessionId": c.session_id })).into_response(),
        _ => fail(StatusCode::INTERNAL_SERVER_ERROR, "call was not stored"),
    }
}

// ---------------------------------------------------------------- events

#[derive(Deserialize)]
struct EventsBody {
    events: Vec<EventIn>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventIn {
    r#type: String,
    n: u64,
    #[serde(default)]
    payload: Value,
    occurred_at: Option<String>,
}

fn actor_for<'a>(call: &'a CallRow, ev: &'a EventIn, owner: &'a str) -> (&'static str, String) {
    let caller = if call.direction == "inbound" { &call.from_e164 } else { &call.to_e164 };
    match ev.payload["speaker"].as_str() {
        Some("caller") => ("caller", caller.clone()),
        Some("human") => ("human", ev.payload["by"].as_str().filter(|s| !s.is_empty()).unwrap_or(owner).to_string()),
        _ => ("bot", call.bot_id.clone()),
    }
}

async fn events_h(State(state): State<Arc<AppState>>, Extension(deps): Extension<Arc<VoiceDeps>>, Path(call_id): Path<String>, auth: RelayedAuth) -> Response {
    let call = match owned_call(&state.db, &call_id, &auth.owner) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let body: EventsBody = match auth.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    if body.events.len() > MAX_EVENTS {
        return fail(StatusCode::BAD_REQUEST, "too many events in one request");
    }
    if body.events.iter().any(|e| !e.r#type.starts_with("call.") || e.r#type.len() > 64) {
        return fail(StatusCode::BAD_REQUEST, "event types must be call.*");
    }
    let (mut written, mut duplicates, mut ignored) = (0, 0, 0);
    for ev in body.events {
        // Interim partials belong to the live stream, not the ledger.
        if ev.r#type == "call.transcript.delta" && ev.payload["final"].as_bool() != Some(true) {
            ignored += 1;
            continue;
        }
        let mut payload = if ev.payload.is_object() { ev.payload.clone() } else { json!({}) };
        payload["callId"] = json!(call_id);
        payload["n"] = json!(ev.n);
        if let Some(at) = &ev.occurred_at {
            payload["occurredAt"] = json!(at);
        }
        let (kind, id) = actor_for(&call, &ev, &auth.owner);
        let key = format!("call:{call_id}:{}:{}", ev.r#type, ev.n);
        if led(&state.db, &call.bot_id, &call.thread_id, Some(&call.session_id), &ev.r#type, (kind, &id), payload, Some(key)) {
            written += 1;
        } else {
            duplicates += 1;
        }
        if ev.r#type == "call.ended" {
            let at = ev.occurred_at.clone().unwrap_or_else(|| crate::agent_gateway_routes::now());
            let _ = state.db.connect().and_then(|c| c.execute("UPDATE voice_calls SET state='ended', ended_at=COALESCE(ended_at, ?1) WHERE call_id=?2", params![at, call_id]));
            // A call that ended with a turn still running has nobody to speak to.
            deps.abort_turn(&call_id, None).await;
            // Only the first call.ended of a call spawns a summary (the key makes a replay a no-op anyway).
            let (db, summarizer, call, id) = (state.db.clone(), deps.summarizer.clone(), call.clone(), call_id.clone());
            tokio::spawn(write_call_summary(db, summarizer, call, id));
        }
    }
    Json(json!({ "written": written, "duplicates": duplicates, "ignored": ignored })).into_response()
}

// ---------------------------------------------------------------- turn

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TurnBody {
    text: String,
    #[allow(dead_code)]
    segment_id: Option<String>,
}

/// Sentence-sized pieces (whitespace kept) so a TTS can start on the first one.
fn speakable_chunks(reply: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = reply.chars().peekable();
    while let Some(c) = chars.next() {
        cur.push(c);
        if matches!(c, '.' | '!' | '?' | '\n') && chars.peek().map_or(true, |n| n.is_whitespace()) {
            while let Some(n) = chars.peek().copied().filter(|n| n.is_whitespace()) {
                cur.push(n);
                chars.next();
            }
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// Sends a turner's event to the call stream. A `tool` step is also written to
/// the thread as `agent.tool.started` / `.completed` / `.failed` (the native
/// channel path writes none, so nothing is written twice); the internal
/// `toolCallId` is not part of the stream's event shape.
fn forward_turn_event(db: &DbHandle, call: &CallRow, call_id: &str, mut ev: Value, out: &mpsc::UnboundedSender<Value>) {
    if ev["type"] == "tool" {
        let id = ev.as_object_mut().and_then(|o| o.remove("toolCallId")).and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        let (status, name) = (ev["status"].as_str().unwrap_or(""), ev["name"].as_str().unwrap_or("tool").to_string());
        let kind = match status {
            "started" => "agent.tool.started",
            "done" => "agent.tool.completed",
            _ => "agent.tool.failed",
        };
        let payload = json!({ "data": { "name": name, "callId": id }, "envelope": { "source": "voice.call", "callId": call_id } });
        led(db, &call.bot_id, &call.thread_id, Some(&call.session_id), kind, ("bot", &call.bot_id), payload, Some(format!("call:{call_id}:tool:{id}:{status}")));
    }
    let _ = out.send(ev);
}

/// Aborts the turn when the SSE response is dropped (the relay hung up) while
/// that turn is still the call's current one.
struct StreamGuard {
    deps: Arc<VoiceDeps>,
    call_id: String,
    generation: u64,
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        let (deps, call_id, generation) = (self.deps.clone(), self.call_id.clone(), self.generation);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                deps.abort_turn(&call_id, Some(generation)).await;
            });
        }
    }
}

async fn turn_h(State(state): State<Arc<AppState>>, Extension(deps): Extension<Arc<VoiceDeps>>, Path(call_id): Path<String>, auth: RelayedAuth) -> Response {
    let call = match owned_call(&state.db, &call_id, &auth.owner) {
        Ok(c) => c,
        Err(r) => return r,
    };
    if call.state != "active" {
        return fail(StatusCode::CONFLICT, "the call has ended");
    }
    let body: TurnBody = match auth.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    let text = body.text.trim().to_string();
    if text.is_empty() || text.chars().count() > MAX_TURN_CHARS {
        return fail(StatusCode::BAD_REQUEST, "text must be 1 to 4000 characters");
    }
    // One in-flight turn per call: a new one aborts the old one first.
    deps.abort_turn(&call_id, None).await;

    let (tx, rx) = mpsc::unbounded_channel::<Value>();
    let generation = deps.next_generation.fetch_add(1, Ordering::Relaxed);
    let task = {
        let (deps, db, tx, call, call_id) = (deps.clone(), state.db.clone(), tx.clone(), call.clone(), call_id.clone());
        let prompt = format!("{SPOKEN_PREFACE}\n\n{text}");
        tokio::spawn(async move {
            let (itx, mut irx) = mpsc::unbounded_channel::<Value>();
            let result = {
                let run = deps.turner.run(&db, &call.session_id, &call.bot_id, &prompt, itx);
                tokio::pin!(run);
                loop {
                    tokio::select! {
                        r = &mut run => break r,
                        Some(ev) = irx.recv() => forward_turn_event(&db, &call, &call_id, ev, &tx),
                    }
                }
            };
            while let Ok(ev) = irx.try_recv() {
                forward_turn_event(&db, &call, &call_id, ev, &tx);
            }
            match result {
                Ok(reply) => {
                    if let TurnReply::Final(reply) = reply {
                        for chunk in speakable_chunks(&reply) {
                            let _ = tx.send(json!({ "type": "text.delta", "text": chunk }));
                        }
                    }
                    let _ = tx.send(json!({ "type": "done" }));
                }
                Err(message) => {
                    let _ = tx.send(json!({ "type": "error", "message": message }));
                }
            }
            let mut turns = deps.turns.lock().unwrap();
            if turns.get(&call_id).map_or(false, |t| t.generation == generation) {
                turns.remove(&call_id);
            }
        })
    };
    deps.turns.lock().unwrap().insert(call_id.clone(), InFlight { generation, session_id: call.session_id.clone(), task: task.abort_handle(), out: tx });

    let guard = StreamGuard { deps, call_id, generation };
    let stream = futures::stream::unfold((rx, guard, false), |(mut rx, guard, finished)| async move {
        if finished {
            return None;
        }
        let v = rx.recv().await?;
        let last = matches!(v["type"].as_str(), Some("done") | Some("error"));
        Some((Ok::<_, Infallible>(Event::default().data(v.to_string())), (rx, guard, last)))
    });
    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

async fn abort_h(State(state): State<Arc<AppState>>, Extension(deps): Extension<Arc<VoiceDeps>>, Path(call_id): Path<String>, auth: RelayedAuth) -> Response {
    if let Err(r) = owned_call(&state.db, &call_id, &auth.owner) {
        return r;
    }
    deps.abort_turn(&call_id, None).await;
    StatusCode::NO_CONTENT.into_response()
}

// ---------------------------------------------------------------- router

pub fn voice_calls_router() -> Router<Arc<AppState>> {
    voice_calls_router_with(Arc::new(VoiceDeps::production()))
}

pub fn voice_calls_router_with(deps: Arc<VoiceDeps>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/voice/calls", post(create_h))
        .route("/api/v1/voice/calls/:call_id/events", post(events_h))
        .route("/api/v1/voice/calls/:call_id/turn", post(turn_h).delete(abort_h))
        .layer(Extension(deps.secret.clone()))
        .layer(Extension(deps))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay_auth::{relay_key_from_device_token, sign_relay, OWNER_HEADER, SIG_HEADER, TS_HEADER};
    use axum::body::Body;
    use axum::http::{HeaderMap, Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const TOKEN: &str = "allternit_runtime_test_token";
    const OWNER: &str = "user-a";

    struct Secret(Option<(&'static str, &'static str)>);
    impl RelaySecret for Secret {
        fn device_token(&self) -> Option<String> {
            self.0.map(|s| s.0.to_string())
        }
        fn paired_owner(&self) -> Option<String> {
            self.0.map(|s| s.1.to_string())
        }
    }

    struct FakeResolver;
    #[async_trait]
    impl ThreadResolver for FakeResolver {
        async fn resolve(&self, _db: &DbHandle, number_id: &str, caller: &str) -> Result<(String, String), String> {
            Ok((format!("thr-{number_id}-{caller}"), format!("sess-{number_id}-{caller}")))
        }
    }

    #[derive(Default)]
    struct FakeSummarizer {
        reply: Mutex<Option<Result<Value, String>>>,
        prompts: Mutex<Vec<(String, String)>>,
    }
    #[async_trait]
    impl CallSummarizer for FakeSummarizer {
        async fn summarize(&self, _db: &DbHandle, session_id: &str, _bot: &str, prompt: &str) -> Result<Value, String> {
            self.prompts.lock().unwrap().push((session_id.into(), prompt.into()));
            self.reply.lock().unwrap().clone().unwrap_or_else(|| Ok(json!({ "text": "A customer asked about Friday. The bot checked the calendar.", "followUps": ["Call back to confirm Friday", "  "] })))
        }
    }

    #[derive(Default)]
    struct FakeTurner {
        reply: Mutex<Option<Result<String, String>>>,
        /// When set, the turner streams these deltas itself (and returns `Streamed`).
        streamed: Mutex<Vec<&'static str>>,
        /// Block until aborted (task cancelled).
        hang: Mutex<bool>,
        prompts: Mutex<Vec<(String, String)>>,
        aborted: Mutex<Vec<String>>,
    }
    #[async_trait]
    impl VoiceTurner for FakeTurner {
        async fn run(&self, _db: &DbHandle, session_id: &str, _bot: &str, text: &str, events: mpsc::UnboundedSender<Value>) -> Result<TurnReply, String> {
            self.prompts.lock().unwrap().push((session_id.into(), text.into()));
            let streamed = self.streamed.lock().unwrap().clone();
            let _ = events.send(json!({ "type": "tool", "name": "calendar", "status": "started", "toolCallId": "tc1" }));
            for d in &streamed {
                let _ = events.send(json!({ "type": "text.delta", "text": d }));
            }
            if *self.hang.lock().unwrap() {
                futures::future::pending::<()>().await;
            }
            let _ = events.send(json!({ "type": "tool", "name": "calendar", "status": "done", "toolCallId": "tc1" }));
            if !streamed.is_empty() {
                return Ok(TurnReply::Streamed);
            }
            self.reply.lock().unwrap().clone().unwrap_or_else(|| Ok("Sure. I can do that. Anything else?".into())).map(TurnReply::Final)
        }
        async fn abort(&self, session_id: &str) {
            self.aborted.lock().unwrap().push(session_id.into());
        }
    }

    fn now_ts() -> i64 {
        unix_now()
    }

    fn headers(sig: &str, ts: i64, owner: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(SIG_HEADER, sig.parse().unwrap());
        h.insert(TS_HEADER, ts.to_string().parse().unwrap());
        h.insert(OWNER_HEADER, owner.parse().unwrap());
        h
    }

    fn signed(token: &str, ts: i64, owner: &str, method: &str, path: &str, body: &[u8]) -> HeaderMap {
        headers(&format!("v1={}", sign_relay(&relay_key_from_device_token(token), ts, method, path, body)), ts, owner)
    }

    // ---------------------------------------------------------- routes

    struct H {
        app: Router,
        state: Arc<AppState>,
        turner: Arc<FakeTurner>,
        summarizer: Arc<FakeSummarizer>,
    }

    async fn setup(tag: &str, secret: Option<(&'static str, &'static str)>) -> H {
        let dir = std::env::temp_dir().join(format!("allternit-voice-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::test_helpers::app_state(&dir).await;
        state.db.connect().unwrap().execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot-1','user-a','b','m','p',1,'{}')", []).unwrap();
        crate::channel_phone::upsert_number(&state.db, "num-1", OWNER, "bot-1", "+14155550100", None).unwrap();
        let turner = Arc::new(FakeTurner::default());
        let summarizer = Arc::new(FakeSummarizer::default());
        let deps = Arc::new(VoiceDeps::new(Arc::new(Secret(secret)), Box::new(FakeResolver), turner.clone(), summarizer.clone()));
        let app = voice_calls_router_with(deps).with_state(state.clone());
        H { app, state, turner, summarizer }
    }

    async fn send(app: &Router, method: &str, path: &str, body: Value, sign: bool) -> (StatusCode, String) {
        let bytes = if body.is_null() { Vec::new() } else { serde_json::to_vec(&body).unwrap() };
        let mut req = Request::builder().method(method).uri(path).header("content-type", "application/json");
        if sign {
            for (k, v) in signed(TOKEN, now_ts(), OWNER, method, path, &bytes).iter() {
                req = req.header(k, v);
            }
        }
        let resp = app.clone().oneshot(req.body(Body::from(bytes)).unwrap()).await.unwrap();
        let status = resp.status();
        (status, String::from_utf8_lossy(&resp.into_body().collect().await.unwrap().to_bytes()).to_string())
    }

    fn call_body(id: &str) -> Value {
        json!({ "callId": id, "botId": "bot-1", "ownerId": OWNER, "numberId": "num-1", "from": "+14155550123", "to": "+14155550100", "direction": "inbound", "room": "call-1", "startedAt": "2026-10-03T10:00:00Z" })
    }

    const CALLS: &str = "/api/v1/voice/calls";

    #[tokio::test]
    async fn unsigned_is_401_and_unconfigured_signed_is_503() {
        let h = setup("auth", Some((TOKEN, OWNER))).await;
        assert_eq!(send(&h.app, "POST", CALLS, call_body("c1"), false).await.0, StatusCode::UNAUTHORIZED);
        let u = setup("unconf", None).await;
        assert_eq!(send(&u.app, "POST", CALLS, call_body("c1"), false).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(send(&u.app, "POST", CALLS, call_body("c1"), true).await.0, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn create_is_idempotent_and_checks_ownership() {
        let h = setup("create", Some((TOKEN, OWNER))).await;
        let (s1, b1) = send(&h.app, "POST", CALLS, call_body("c1"), true).await;
        assert_eq!(s1, StatusCode::OK, "{b1}");
        let v1: Value = serde_json::from_str(&b1).unwrap();
        assert_eq!(v1["threadId"], "thr-num-1-+14155550123");
        assert_eq!(v1["sessionId"], "sess-num-1-+14155550123");
        let (s2, b2) = send(&h.app, "POST", CALLS, call_body("c1"), true).await;
        assert_eq!((s2, serde_json::from_str::<Value>(&b2).unwrap()), (StatusCode::OK, v1));
        let n: i64 = h.state.db.connect().unwrap().query_row("SELECT COUNT(*) FROM voice_calls", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        // Outbound resolves the thread by the number dialed.
        let mut out = call_body("c2");
        (out["direction"], out["from"], out["to"]) = (json!("outbound"), json!("+14155550100"), json!("+14155550199"));
        let v: Value = serde_json::from_str(&send(&h.app, "POST", CALLS, out, true).await.1).unwrap();
        assert_eq!(v["threadId"], "thr-num-1-+14155550199");
        // Validation.
        let mut bad = call_body("c3");
        bad["ownerId"] = json!("user-b");
        assert_eq!(send(&h.app, "POST", CALLS, bad, true).await.0, StatusCode::FORBIDDEN);
        let mut bad = call_body("c3");
        bad["numberId"] = json!("nope");
        bad["botId"] = json!("bot-2");
        assert_eq!(send(&h.app, "POST", CALLS, bad, true).await.0, StatusCode::UNPROCESSABLE_ENTITY);
        let mut bad = call_body("c3");
        bad["botId"] = json!("bot-2");
        assert_eq!(send(&h.app, "POST", CALLS, bad, true).await.0, StatusCode::FORBIDDEN);
        let mut bad = call_body("c3");
        bad["from"] = json!("4155550123");
        assert_eq!(send(&h.app, "POST", CALLS, bad, true).await.0, StatusCode::BAD_REQUEST);
        assert_eq!(send(&h.app, "POST", CALLS, json!({ "nope": 1 }), true).await.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn unknown_number_is_registered_from_the_signed_start() {
        let h = setup("backstop", Some((TOKEN, OWNER))).await;
        let mut body = call_body("c1");
        (body["numberId"], body["to"]) = (json!("num-new"), json!("+14155550120"));
        let (s, b) = send(&h.app, "POST", CALLS, body, true).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        let n = crate::channel_phone::number(&h.state.db, "num-new").expect("registered");
        assert_eq!((n.owner.as_str(), n.bot_id.as_str(), n.e164.as_str()), (OWNER, "bot-1", "+14155550120"));
        // Outbound registers the bot's number from `from`.
        let mut out = call_body("c2");
        (out["numberId"], out["direction"], out["from"], out["to"]) = (json!("num-out"), json!("outbound"), json!("+14155550111"), json!("+14155550199"));
        assert_eq!(send(&h.app, "POST", CALLS, out, true).await.0, StatusCode::OK);
        assert_eq!(crate::channel_phone::number(&h.state.db, "num-out").unwrap().e164, "+14155550111");
        // A bot that isn't the owner's is refused with 422 and nothing is registered.
        let mut foreign = call_body("c3");
        (foreign["numberId"], foreign["botId"], foreign["to"]) = (json!("num-x"), json!("bot-9"), json!("+14155550130"));
        let (s, b) = send(&h.app, "POST", CALLS, foreign, true).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{b}");
        assert!(crate::channel_phone::number(&h.state.db, "num-x").is_none());
    }

    fn ledger(h: &H, call: &str) -> Vec<(String, String, String, Value)> {
        let conn = h.state.db.connect().unwrap();
        let mut stmt = conn.prepare("SELECT event_type, actor_type, actor_id, payload FROM bot_events WHERE thread_id = ?1 AND idempotency_key LIKE ?2 ORDER BY rowid").unwrap();
        stmt.query_map(params!["thr-num-1-+14155550123", format!("call:{call}:%")], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, serde_json::from_str::<Value>(&r.get::<_, String>(3)?).unwrap()))).unwrap().map(Result::unwrap).collect()
    }

    #[tokio::test]
    async fn events_write_in_order_idempotently_and_skip_interim() {
        let h = setup("events", Some((TOKEN, OWNER))).await;
        send(&h.app, "POST", CALLS, call_body("c1"), true).await;
        let path = format!("{CALLS}/c1/events");
        let batch = json!({ "events": [
            { "type": "call.started", "n": 0, "payload": { "direction": "inbound", "from": "+14155550123", "to": "+14155550100", "numberId": "num-1" }, "occurredAt": "2026-10-03T10:00:00Z" },
            { "type": "call.transcript.delta", "n": 1, "payload": { "speaker": "caller", "text": "hel", "final": false, "segmentId": "s1" } },
            { "type": "call.transcript.delta", "n": 2, "payload": { "speaker": "caller", "text": "hello", "final": true, "segmentId": "s1" } },
            { "type": "call.transcript.delta", "n": 3, "payload": { "speaker": "bot", "text": "Hi there", "final": true, "segmentId": "s2" } },
            { "type": "call.transcript.delta", "n": 4, "payload": { "speaker": "human", "by": "user-a", "text": "taking over", "final": true, "segmentId": "s3" } },
        ] });
        let (st, body) = send(&h.app, "POST", &path, batch.clone(), true).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(serde_json::from_str::<Value>(&body).unwrap(), json!({ "written": 4, "duplicates": 0, "ignored": 1 }));
        let rows = ledger(&h, "c1");
        let kinds: Vec<_> = rows.iter().map(|r| (r.0.as_str(), r.1.as_str(), r.2.as_str())).collect();
        assert_eq!(
            kinds,
            vec![
                ("call.started", "bot", "bot-1"),
                ("call.transcript.delta", "caller", "+14155550123"),
                ("call.transcript.delta", "bot", "bot-1"),
                ("call.transcript.delta", "human", "user-a"),
            ]
        );
        assert!(rows.iter().all(|r| r.3["callId"] == "c1"));
        assert_eq!(rows[1].3["text"], "hello");
        // Replay: nothing new.
        let (_, body) = send(&h.app, "POST", &path, batch, true).await;
        assert_eq!(serde_json::from_str::<Value>(&body).unwrap(), json!({ "written": 0, "duplicates": 4, "ignored": 1 }));
        assert_eq!(ledger(&h, "c1").len(), 4);
        // call.ended closes the call.
        let end = json!({ "events": [{ "type": "call.ended", "n": 5, "payload": { "durationSec": 12, "reason": "hangup" }, "occurredAt": "2026-10-03T10:00:12Z" }] });
        assert_eq!(send(&h.app, "POST", &path, end, true).await.0, StatusCode::OK);
        let (state, ended): (String, Option<String>) = h.state.db.connect().unwrap().query_row("SELECT state, ended_at FROM voice_calls WHERE call_id='c1'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((state.as_str(), ended.as_deref()), ("ended", Some("2026-10-03T10:00:12Z")));
        // Unknown call, bad type.
        assert_eq!(send(&h.app, "POST", &format!("{CALLS}/nope/events"), batch_of("call.started"), true).await.0, StatusCode::NOT_FOUND);
        assert_eq!(send(&h.app, "POST", &path, batch_of("agent.tool.started"), true).await.0, StatusCode::BAD_REQUEST);
    }

    async fn wait_for_summary_rows(h: &H, call: &str, want: usize) -> Vec<(String, String, String, Value)> {
        for _ in 0..100 {
            let rows: Vec<_> = ledger(h, call).into_iter().filter(|r| r.0 == "call.summary").collect();
            if rows.len() >= want {
                return rows;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Vec::new()
    }

    #[tokio::test]
    async fn an_ended_call_gets_one_summary_in_an_ephemeral_session() {
        let h = setup("summary", Some((TOKEN, OWNER))).await;
        send(&h.app, "POST", CALLS, call_body("c1"), true).await;
        let path = format!("{CALLS}/c1/events");
        let batch = json!({ "events": [
            { "type": "call.transcript.delta", "n": 1, "payload": { "speaker": "caller", "text": "Are you open Friday?", "final": true } },
            { "type": "call.transcript.delta", "n": 2, "payload": { "speaker": "bot", "text": "Let me check.", "final": true } },
            { "type": "call.ended", "n": 3, "payload": { "durationSec": 40, "reason": "hangup" } },
        ] });
        send(&h.app, "POST", &path, batch.clone(), true).await;
        let rows = wait_for_summary_rows(&h, "c1", 1).await;
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].1.as_str(), rows[0].2.as_str()), ("bot", "bot-1"));
        assert_eq!(rows[0].3["text"], "A customer asked about Friday. The bot checked the calendar.");
        assert_eq!(rows[0].3["followUps"], json!(["Call back to confirm Friday"]));
        assert_eq!(rows[0].3["callId"], "c1");
        let prompts = h.summarizer.prompts.lock().unwrap().clone();
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].1.contains("Caller: Are you open Friday?") && prompts[0].1.contains("Bot: Let me check."), "{}", prompts[0].1);
        // A replayed call.ended does not write a second summary.
        send(&h.app, "POST", &path, batch, true).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(ledger(&h, "c1").iter().filter(|r| r.0 == "call.summary").count(), 1);
    }

    #[tokio::test]
    async fn no_summary_without_a_final_segment_or_when_the_model_fails() {
        let h = setup("nosummary", Some((TOKEN, OWNER))).await;
        send(&h.app, "POST", CALLS, call_body("c1"), true).await;
        send(&h.app, "POST", &format!("{CALLS}/c1/events"), json!({ "events": [{ "type": "call.ended", "n": 1, "payload": { "reason": "missed" } }] }), true).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(h.summarizer.prompts.lock().unwrap().is_empty());
        assert!(ledger(&h, "c1").iter().all(|r| r.0 != "call.summary"));

        send(&h.app, "POST", CALLS, call_body("c2"), true).await;
        *h.summarizer.reply.lock().unwrap() = Some(Err("gizzi down".into()));
        let batch = json!({ "events": [
            { "type": "call.transcript.delta", "n": 1, "payload": { "speaker": "caller", "text": "hi", "final": true } },
            { "type": "call.ended", "n": 2, "payload": {} },
        ] });
        assert_eq!(send(&h.app, "POST", &format!("{CALLS}/c2/events"), batch, true).await.0, StatusCode::OK);
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(h.summarizer.prompts.lock().unwrap().len(), 1);
        assert!(ledger(&h, "c2").iter().all(|r| r.0 != "call.summary"));
    }

    fn batch_of(kind: &str) -> Value {
        json!({ "events": [{ "type": kind, "n": 9, "payload": {} }] })
    }

    fn sse_events(body: &str) -> Vec<Value> {
        body.lines().filter_map(|l| l.strip_prefix("data:")).map(|d| serde_json::from_str(d.trim()).unwrap()).collect()
    }

    #[tokio::test]
    async fn turn_streams_the_reply_with_a_spoken_preface() {
        let h = setup("turn", Some((TOKEN, OWNER))).await;
        send(&h.app, "POST", CALLS, call_body("c1"), true).await;
        let (st, body) = send(&h.app, "POST", &format!("{CALLS}/c1/turn"), json!({ "text": "  what's on today? ", "segmentId": "s9" }), true).await;
        assert_eq!(st, StatusCode::OK);
        let ev = sse_events(&body);
        let kinds: Vec<_> = ev.iter().map(|e| e["type"].as_str().unwrap()).collect();
        assert_eq!(kinds, vec!["tool", "tool", "text.delta", "text.delta", "text.delta", "done"]);
        assert_eq!((ev[0]["name"].as_str(), ev[0]["status"].as_str(), ev[1]["status"].as_str()), (Some("calendar"), Some("started"), Some("done")));
        let spoken: String = ev.iter().filter(|e| e["type"] == "text.delta").map(|e| e["text"].as_str().unwrap()).collect();
        assert_eq!(spoken, "Sure. I can do that. Anything else?");
        let prompts = h.turner.prompts.lock().unwrap();
        assert_eq!(prompts[0].0, "sess-num-1-+14155550123");
        assert!(prompts[0].1.starts_with("[Live phone call.") && prompts[0].1.ends_with("what's on today?"));
    }

    #[tokio::test]
    async fn turn_errors_stream_and_bad_requests_are_rejected() {
        let h = setup("turn-err", Some((TOKEN, OWNER))).await;
        send(&h.app, "POST", CALLS, call_body("c1"), true).await;
        *h.turner.reply.lock().unwrap() = Some(Err("gizzi refused".into()));
        let (_, body) = send(&h.app, "POST", &format!("{CALLS}/c1/turn"), json!({ "text": "hi" }), true).await;
        assert_eq!(sse_events(&body).last().unwrap(), &json!({ "type": "error", "message": "gizzi refused" }));
        let turn = format!("{CALLS}/c1/turn");
        assert_eq!(send(&h.app, "POST", &turn, json!({ "text": "   " }), true).await.0, StatusCode::BAD_REQUEST);
        assert_eq!(send(&h.app, "POST", &format!("{CALLS}/nope/turn"), json!({ "text": "hi" }), true).await.0, StatusCode::NOT_FOUND);
        assert_eq!(send(&h.app, "POST", &turn, json!({ "text": "hi" }), false).await.0, StatusCode::UNAUTHORIZED);
        send(&h.app, "POST", &format!("{CALLS}/c1/events"), json!({ "events": [{ "type": "call.ended", "n": 1, "payload": {} }] }), true).await;
        assert_eq!(send(&h.app, "POST", &turn, json!({ "text": "hi" }), true).await.0, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn a_new_turn_aborts_the_running_one_and_delete_aborts_too() {
        let h = setup("abort", Some((TOKEN, OWNER))).await;
        send(&h.app, "POST", CALLS, call_body("c1"), true).await;
        let turn = format!("{CALLS}/c1/turn");
        // Nothing running: still 204.
        assert_eq!(send(&h.app, "DELETE", &turn, Value::Null, true).await.0, StatusCode::NO_CONTENT);
        assert!(h.turner.aborted.lock().unwrap().is_empty());
        // Unsigned DELETE is refused; an unknown call is a 404.
        assert_eq!(send(&h.app, "DELETE", &turn, Value::Null, false).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(send(&h.app, "DELETE", &format!("{CALLS}/nope/turn"), Value::Null, true).await.0, StatusCode::NOT_FOUND);

        // Turn 1 hangs; turn 2 aborts it, then completes normally.
        *h.turner.hang.lock().unwrap() = true;
        let mut req = Request::builder().method("POST").uri(&turn);
        let body1 = serde_json::to_vec(&json!({ "text": "first" })).unwrap();
        for (k, v) in signed(TOKEN, now_ts(), OWNER, "POST", &turn, &body1).iter() {
            req = req.header(k, v);
        }
        let first = h.app.clone().oneshot(req.body(Body::from(body1)).unwrap()).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        *h.turner.hang.lock().unwrap() = false;
        let (_, second) = send(&h.app, "POST", &turn, json!({ "text": "second" }), true).await;
        assert_eq!(sse_events(&second).last().unwrap(), &json!({ "type": "done" }));
        let first_body = String::from_utf8_lossy(&first.into_body().collect().await.unwrap().to_bytes()).to_string();
        assert_eq!(sse_events(&first_body).last().unwrap(), &json!({ "type": "done", "aborted": true }));
        assert_eq!(h.turner.aborted.lock().unwrap().as_slice(), ["sess-num-1-+14155550123"]);

        // DELETE aborts a hanging turn and its stream ends aborted.
        *h.turner.hang.lock().unwrap() = true;
        let mut req = Request::builder().method("POST").uri(&turn);
        let body3 = serde_json::to_vec(&json!({ "text": "third" })).unwrap();
        for (k, v) in signed(TOKEN, now_ts(), OWNER, "POST", &turn, &body3).iter() {
            req = req.header(k, v);
        }
        let third = h.app.clone().oneshot(req.body(Body::from(body3)).unwrap()).await.unwrap();
        assert_eq!(send(&h.app, "DELETE", &turn, Value::Null, true).await.0, StatusCode::NO_CONTENT);
        let third_body = String::from_utf8_lossy(&third.into_body().collect().await.unwrap().to_bytes()).to_string();
        assert_eq!(sse_events(&third_body).last().unwrap(), &json!({ "type": "done", "aborted": true }));
        assert_eq!(h.turner.aborted.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_streaming_turner_forwards_deltas_in_order_and_tools_reach_the_thread() {
        let h = setup("stream", Some((TOKEN, OWNER))).await;
        send(&h.app, "POST", CALLS, call_body("c1"), true).await;
        *h.turner.streamed.lock().unwrap() = vec!["Your ", "day is ", "clear."];
        let (st, body) = send(&h.app, "POST", &format!("{CALLS}/c1/turn"), json!({ "text": "today?" }), true).await;
        assert_eq!(st, StatusCode::OK);
        let ev = sse_events(&body);
        assert_eq!(
            ev,
            vec![
                json!({ "type": "tool", "name": "calendar", "status": "started" }),
                json!({ "type": "text.delta", "text": "Your " }),
                json!({ "type": "text.delta", "text": "day is " }),
                json!({ "type": "text.delta", "text": "clear." }),
                json!({ "type": "tool", "name": "calendar", "status": "done" }),
                json!({ "type": "done" }),
            ]
        );
        // The steps are in the thread once each, as the existing agent.tool.* events.
        let kinds: Vec<String> = ledger(&h, "c1").into_iter().map(|e| e.0).collect();
        let tools: Vec<_> = h.state.db.connect().unwrap().prepare("SELECT event_type FROM bot_events WHERE event_type LIKE 'agent.tool.%' ORDER BY rowid").unwrap().query_map([], |r| r.get::<_, String>(0)).unwrap().filter_map(Result::ok).collect();
        assert_eq!(tools, vec!["agent.tool.started", "agent.tool.completed"]);
        assert_eq!(kinds, tools); // no call.* duplicates of the steps
    }

    #[tokio::test]
    async fn delete_stops_a_streaming_turn_within_the_deadline() {
        let h = setup("stream-abort", Some((TOKEN, OWNER))).await;
        send(&h.app, "POST", CALLS, call_body("c1"), true).await;
        *h.turner.streamed.lock().unwrap() = vec!["Let me "];
        *h.turner.hang.lock().unwrap() = true;
        let turn = format!("{CALLS}/c1/turn");
        let mut req = Request::builder().method("POST").uri(&turn);
        let body = serde_json::to_vec(&json!({ "text": "go" })).unwrap();
        for (k, v) in signed(TOKEN, now_ts(), OWNER, "POST", &turn, &body).iter() {
            req = req.header(k, v);
        }
        let resp = h.app.clone().oneshot(req.body(Body::from(body)).unwrap()).await.unwrap();
        let reader = tokio::spawn(async move { String::from_utf8_lossy(&resp.into_body().collect().await.unwrap().to_bytes()).to_string() });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(send(&h.app, "DELETE", &turn, Value::Null, true).await.0, StatusCode::NO_CONTENT);
        let t0 = std::time::Instant::now();
        let body = tokio::time::timeout(Duration::from_millis(100), reader).await.expect("stream closed within 100 ms").unwrap();
        assert!(t0.elapsed() < Duration::from_millis(100));
        let ev = sse_events(&body);
        assert!(ev.iter().any(|e| e["text"] == "Let me "));
        assert_eq!(ev.last().unwrap(), &json!({ "type": "done", "aborted": true }));
        assert_eq!(h.turner.aborted.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_placed_session_answers_with_the_final_reply_instead_of_streaming() {
        let h = setup("fallback", Some((TOKEN, OWNER))).await;
        let hits = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = hits.clone();
        let remote = Router::new().fallback(move |req: Request<Body>| {
            let seen = seen.clone();
            async move {
                seen.lock().unwrap().push(format!("{} {}", req.method(), req.uri().path()));
                Json(json!({ "metadata": { "parts": [{ "type": "text", "text": "From the server." }] } }))
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, remote).await.unwrap() });
        let conn = h.state.db.connect().unwrap();
        conn.execute(
            "INSERT INTO remote_backend_targets (id, user_id, name, status, gateway_url, encrypted_gateway_token) VALUES ('tgt1','user-a','Server','ready',?1,?2)",
            params![format!("http://{addr}"), crate::token_crypto::seal("atok_1")],
        )
        .unwrap();
        crate::placement::record(&h.state.db, "sess-placed", "tgt1");
        assert_eq!(no_stream_reason(&h.state.db, "sess-placed"), Some("placed on another Allternit"));
        assert_eq!(no_stream_reason(&h.state.db, "sess-native"), None);

        let (tx, _rx) = mpsc::unbounded_channel();
        let turner = GizziStreamTurner { fallback: ChannelTurner };
        let reply = tokio::time::timeout(Duration::from_secs(5), turner.run(&h.state.db, "sess-placed", "bot-1", "hi", tx)).await.expect("never hangs").unwrap();
        assert_eq!(reply, TurnReply::Final("From the server.".into()));
        assert_eq!(hits.lock().unwrap().as_slice(), ["POST /api/v1/agent-sessions/sess-placed/messages"]);
    }

    #[test]
    fn speakable_chunks_split_on_sentences_and_lose_nothing() {
        let t = "One. Two! Three?\nFour without end";
        let c = speakable_chunks(t);
        assert!(c.len() >= 4);
        assert_eq!(c.concat(), t);
        assert_eq!(speakable_chunks("3.5 percent"), vec!["3.5 percent"]);
        assert!(speakable_chunks("").is_empty());
    }
}
