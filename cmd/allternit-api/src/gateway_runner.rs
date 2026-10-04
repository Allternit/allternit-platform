//! Agent Gateway thread runner, event bridge and dual-authority approvals
//! (WP5; migration V199). Spec: `Research/specs/agent-gateway.md` ("Object
//! model", "RemoteThreadBinding", "Events", "Approvals have two authorities",
//! "Terms, safety and failure handling").
//!
//! * A bot whose execution binding is `type=vendor` runs its turns remotely
//!   through the AAI (`POST {gateway}/aai/call`). No binding, or `type=allternit`,
//!   is today's native path, unchanged. A vendor bot that is not READY never
//!   falls back to a native brain.
//! * One `remote_thread_bindings` row per thread generation. A handoff moves the
//!   old one ACTIVE -> HANDOFF_PENDING -> CLOSED; the next turn opens a new one
//!   (possibly on a different execution binding).
//! * Vendor events land on the bot ledger (`bot_events`, tagged with the
//!   thread), deduped by `remote_event_id`; the envelope rides in the payload.
//! * Approvals have two authorities that never resolve each other.

use async_trait::async_trait;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};
use tracing::warn;

use crate::agent_gateway_routes::{exec_next, id, now, one, remote_next, rows, s, set_exec_state, EXEC_COLS, REMOTE_COLS};
use crate::auth::AuthUser;
use crate::bot_event_routes::{append_event, ActorBody, AppendEventBody};
use crate::db::DbHandle;
use crate::thread_routes::ThreadRuntime;
use crate::AppState;

// ---------------------------------------------------------------- AAI transport

/// `{ok:false,error}` from the gateway (or a transport failure).
#[derive(Debug, Clone)]
pub struct AaiError {
    pub code: String,
    pub retryable: bool,
    pub retry_after_ms: Option<u64>,
    pub human_message: String,
}

impl AaiError {
    pub fn new(code: &str, msg: impl Into<String>) -> Self {
        AaiError { code: code.into(), retryable: false, retry_after_ms: None, human_message: msg.into() }
    }
    fn from_wire(v: &Value) -> Self {
        AaiError {
            code: v["code"].as_str().unwrap_or("UNKNOWN").to_string(),
            retryable: v["retryable"].as_bool().unwrap_or(false),
            retry_after_ms: v["retryAfterMs"].as_u64(),
            human_message: v["humanMessage"].as_str().unwrap_or("the vendor gateway reported an error").to_string(),
        }
    }
}

/// One AAI call. Production = [`SubsTransport`]; tests inject a fake.
#[async_trait]
pub trait AaiTransport: Send + Sync {
    async fn call(&self, owner: &str, op: &str, binding: &Value, input: Value) -> Result<Value, AaiError>;
    /// Like [`call`] with the just-in-time unsealed provider credential, sent
    /// as a top-level `credential` next to `binding`. Never log it.
    async fn call_cred(&self, owner: &str, op: &str, binding: &Value, credential: Option<&Value>, input: Value) -> Result<Value, AaiError> {
        let _ = credential;
        self.call(owner, op, binding, input).await
    }
    /// Append a vendor reply to the session transcript (assistant message
    /// attributed to the vendor bot). Default: no transcript.
    async fn append_transcript(&self, _session_id: &str, _text: &str, _metadata: Value) -> Result<(), String> {
        Ok(())
    }
}

/// The credential for a binding's account, unsealed server-side just now.
/// `Ok(None)` = the account carries no key (browser session etc.).
/// An `api_key` account with no usable sealed key fails fast with AUTH_REQUIRED.
pub(crate) fn credential_for(db: &DbHandle, owner: &str, binding: &Value) -> Result<Option<Value>, AaiError> {
    let Some(aid) = binding["accountBindingId"].as_str() else { return Ok(None) };
    let conn = db.connect().map_err(|_| AaiError::new("INTERNAL", "could not read the provider account"))?;
    let row: Option<(String, Option<String>)> = conn
        .query_row("SELECT auth_type, secret_ref FROM provider_account_bindings WHERE id = ?1 AND owner = ?2", params![aid, owner], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()
        .map_err(|_| AaiError::new("INTERNAL", "could not read the provider account"))?;
    let Some((auth_type, secret_ref)) = row else { return Ok(None) };
    match secret_ref.filter(|x| !x.is_empty()) {
        Some(sealed) => {
            let key = crate::token_crypto::open(&sealed);
            if key.is_empty() {
                return Err(AaiError::new("AUTH_REQUIRED", "the stored provider key could not be read; add it again"));
            }
            Ok(Some(json!({ "apiKey": key })))
        }
        None if auth_type == "api_key" => Err(AaiError::new("AUTH_REQUIRED", "this account needs an API key before it can be used")),
        None => Ok(None),
    }
}

/// Every vendor call goes through here so the credential rides along.
pub(crate) async fn vcall(db: &DbHandle, tx: &dyn AaiTransport, owner: &str, op: &str, binding: &Value, input: Value) -> Result<Value, AaiError> {
    let cred = credential_for(db, owner, binding)?;
    tx.call_cred(owner, op, binding, cred.as_ref(), input).await
}

/// A non-2xx from `subscription_routes::forward`: its own "no Sessions computer" answers
/// (`sessions_computer_*`) mean no gateway is connected (`GATEWAY_OFFLINE`, 503) and carry the
/// fix in `detail`; anything else is the gateway's failure, reported with its message.
pub(crate) fn forward_error(status: axum::http::StatusCode, v: &Value) -> AaiError {
    let code = v["error"].as_str().unwrap_or_default();
    let detail = v["detail"].as_str().map(|d| format!(" ({d})")).unwrap_or_default();
    let mut e = if code.starts_with("sessions_computer_") {
        AaiError::new("GATEWAY_OFFLINE", format!("no subscription gateway is connected: {code}{detail}"))
    } else {
        let why = if code.is_empty() { String::new() } else { format!(": {code}{detail}") };
        AaiError::new("GATEWAY_UNAVAILABLE", format!("the subscription gateway answered {status}{why}"))
    };
    e.retryable = true;
    e
}

/// Reaches the subscription gateway the way `subscription_routes` does (the
/// Sessions computer's guest port + sealed token).
pub struct SubsTransport(pub Arc<AppState>);

#[async_trait]
impl AaiTransport for SubsTransport {
    async fn call(&self, owner: &str, op: &str, binding: &Value, input: Value) -> Result<Value, AaiError> {
        self.call_cred(owner, op, binding, None, input).await
    }
    async fn append_transcript(&self, session_id: &str, text: &str, metadata: Value) -> Result<(), String> {
        crate::agent_session_routes::append_vendor_message(&self.0.db, session_id, text, metadata).await
    }
    async fn call_cred(&self, owner: &str, op: &str, binding: &Value, credential: Option<&Value>, input: Value) -> Result<Value, AaiError> {
        let user = AuthUser {
            user_id: owner.to_string(),
            email: None,
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: None,
            organization_role: None,
            organization_slug: None,
        };
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(axum::http::header::CONTENT_TYPE, axum::http::HeaderValue::from_static("application/json"));
        let mut body = json!({ "op": op, "binding": binding, "input": input });
        if let Some(c) = credential {
            body["credential"] = c.clone();
        }
        let body = body.to_string();
        let resp = crate::subscription_routes::forward(&self.0, &user, "aai/call", axum::http::Method::POST, &headers, None, body.into())
            .await;
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 10 * 1024 * 1024).await.unwrap_or_default();
        let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(forward_error(status, &v));
        }
        if v["ok"].as_bool() == Some(true) {
            Ok(v["value"].clone())
        } else if v["ok"].as_bool() == Some(false) {
            Err(AaiError::from_wire(&v["error"]))
        } else {
            Err(AaiError::new("BAD_GATEWAY_REPLY", "the subscription gateway sent an unreadable reply"))
        }
    }
}

struct Runtime {
    db: DbHandle,
    tx: Arc<dyn AaiTransport>,
}
static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// The installed transport, or the production one for `state`.
pub(crate) fn transport(state: &Arc<AppState>) -> Arc<dyn AaiTransport> {
    match RUNTIME.get() {
        Some(r) => r.tx.clone(),
        None => Arc::new(crate::gateway_vendor_host::HostRoutedTransport::new(
            state.db.clone(),
            Arc::new(crate::channel_transports::ChannelLaneTransport::new(state.clone(), Arc::new(SubsTransport(state.clone())))),
        )),
    }
}

/// Wire the process-wide runner (called once from `main`).
pub fn install(db: DbHandle, tx: Arc<dyn AaiTransport>) {
    let _ = RUNTIME.set(Runtime { db, tx });
}

// ---------------------------------------------------------------- errors

#[derive(Debug)]
pub struct RunErr {
    pub status: u16,
    pub code: String,
    pub message: String,
    pub retry_after_ms: Option<u64>,
    pub approval_id: Option<String>,
}

impl RunErr {
    fn new(status: u16, code: &str, message: impl Into<String>) -> Self {
        RunErr { status, code: code.into(), message: message.into(), retry_after_ms: None, approval_id: None }
    }
    fn db(e: rusqlite::Error) -> Self {
        warn!(error = %e, "gateway runner DB error");
        Self::new(500, "DB_ERROR", "database error")
    }
}

impl IntoResponse for RunErr {
    fn into_response(self) -> Response {
        let st = StatusCode::from_u16(self.status).unwrap_or(StatusCode::BAD_GATEWAY);
        (
            st,
            Json(json!({ "error": self.message, "code": self.code, "retryAfterMs": self.retry_after_ms, "approvalId": self.approval_id })),
        )
            .into_response()
    }
}

impl From<rusqlite::Error> for RunErr {
    fn from(e: rusqlite::Error) -> Self {
        RunErr::db(e)
    }
}

// ---------------------------------------------------------------- ledger + thread helpers

/// Append to the bot ledger tagged with the thread. `Ok(true)` = newly written,
/// `Ok(false)` = the idempotency key already existed (duplicate).
pub(crate) fn led(db: &DbHandle, bot_id: &str, thread_id: &str, session: Option<&str>, event_type: &str, actor: (&str, &str), payload: Value, key: Option<String>) -> bool {
    let key = key.unwrap_or_else(|| format!("gwr:{event_type}:{}", uuid::Uuid::new_v4()));
    let body = AppendEventBody {
        event_type: event_type.to_string(),
        actor: ActorBody { r#type: actor.0.into(), id: actor.1.into() },
        payload,
        occurred_at: None,
        session_id: session.map(str::to_string),
        goal_id: None,
        wih_id: None,
        task_id: None,
        run_id: None,
        idempotency_key: Some(key.clone()),
    };
    match append_event(db, bot_id, &body, &now()) {
        Ok((_, created)) => {
            if created {
                if let Ok(conn) = db.connect() {
                    let _ = conn.execute(
                        "UPDATE bot_events SET thread_id = ?1 WHERE bot_id = ?2 AND idempotency_key = ?3",
                        params![thread_id, bot_id, key],
                    );
                }
            }
            created
        }
        Err(e) => {
            warn!(thread = %thread_id, error = %e, "gateway runner ledger failed");
            false
        }
    }
}

fn set_thread_status(db: &DbHandle, bot_id: &str, thread_id: &str, status: &str, line: &str) {
    if let Ok(conn) = db.connect() {
        let _ = conn.execute(
            "UPDATE bot_threads SET status = ?2, status_line = ?3, last_activity_at = ?4, updated_at = ?4 WHERE id = ?1",
            params![thread_id, status, line, now()],
        );
    }
    let ev = if status == "needs_you" { "thread.needs_user" } else { "thread.status_changed" };
    led(db, bot_id, thread_id, None, ev, ("system", "gateway"), json!({ "status": status, "statusLine": line }), None);
}

pub(crate) struct Cx {
    pub(crate) owner: String,
    pub(crate) thread_id: String,
    pub(crate) bot_id: String,
    pub(crate) generation: i64,
    pub(crate) session_id: String,
    pub(crate) exec: Value,
}

/// The vendor context for a session's thread, or `None` = native path.
fn resolve(db: &DbHandle, session_id: &str) -> Result<Option<Cx>, RunErr> {
    let conn = db.connect()?;
    let row: Option<(String, String, String)> = conn
        .query_row(
            "SELECT t.id, t.user_id, t.bot_id FROM bot_thread_sessions s JOIN bot_threads t ON t.id = s.thread_id WHERE s.session_id = ?1",
            params![session_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((thread_id, owner, bot_id)) = row else { return Ok(None) };
    let exec = one(&conn, &format!("SELECT {EXEC_COLS} FROM bot_execution_bindings WHERE bot_id = ?1 AND owner = ?2"), &[&bot_id, &owner])?;
    let Some(exec) = exec.filter(|e| s(e, "type") == "vendor") else { return Ok(None) };
    let (generation, sid): (i64, String) = conn.query_row(
        "SELECT generation, session_id FROM bot_thread_sessions WHERE thread_id = ?1 ORDER BY generation DESC LIMIT 1",
        params![thread_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(Some(Cx { owner, thread_id, bot_id, generation, session_id: sid, exec }))
}

fn remote_for(db: &DbHandle, thread_id: &str, generation: i64) -> rusqlite::Result<Option<Value>> {
    let conn = db.connect()?;
    one(&conn, &format!("SELECT {REMOTE_COLS} FROM remote_thread_bindings WHERE thread_id = ?1 AND generation = ?2"), &[&thread_id, &generation])
}

fn set_remote_state(db: &DbHandle, owner: &str, remote: &Value, to: &str) -> rusqlite::Result<()> {
    let from = s(remote, "state");
    if from == to || !remote_next(&from).contains(&to) {
        return Ok(());
    }
    let conn = db.connect()?;
    let t = now();
    conn.execute(
        "UPDATE remote_thread_bindings SET state = ?1, updated_at = ?2, closed_at = CASE WHEN ?1 = 'CLOSED' THEN ?2 ELSE closed_at END WHERE id = ?3",
        params![to, t, s(remote, "id")],
    )?;
    let kind = match to {
        "ACTIVE" if from == "OPENING" => Some("gateway.remote_thread.opened"),
        "HANDOFF_PENDING" => Some("gateway.remote_thread.handoff_pending"),
        "CLOSED" => Some("gateway.remote_thread.closed"),
        _ => None,
    };
    if let Some(k) = kind {
        led(
            db,
            &s(remote, "botId"),
            &s(remote, "threadId"),
            None,
            k,
            (if k.ends_with("opened") { "system" } else { "system" }, "gateway"),
            json!({ "bindingId": s(remote, "id"), "threadId": s(remote, "threadId"), "generation": remote["generation"], "lane": remote["lane"], "from": from, "to": to, "owner": owner }),
            None,
        );
    }
    Ok(())
}

// ---------------------------------------------------------------- turn

#[derive(Default, Clone)]
pub struct TurnOpts {
    /// Idempotency key: a retry with the same id never re-sends.
    pub correlation_id: Option<String>,
    /// The action is consequential: Allternit approval first.
    pub consequential: bool,
    pub allternit_approval_id: Option<String>,
}

#[derive(Debug)]
pub struct TurnReport {
    pub reply: Option<String>,
    pub events: usize,
    pub correlation_id: String,
    pub remote_binding_id: String,
}

fn blocked(db: &DbHandle, cx: &Cx, status: u16, code: &str, msg: &str) -> RunErr {
    set_thread_status(db, &cx.bot_id, &cx.thread_id, "needs_you", msg);
    led(db, &cx.bot_id, &cx.thread_id, None, "gateway.turn.blocked", ("system", "gateway"), json!({ "code": code, "message": msg }), None);
    RunErr::new(status, code, msg)
}

/// Run one user turn for `session_id`. `Ok(None)` = not a vendor thread (native
/// path, caller proceeds as before).
pub async fn run_turn<R: ThreadRuntime>(
    db: &DbHandle,
    tx: &dyn AaiTransport,
    rt: &R,
    session_id: &str,
    text: &str,
    opts: TurnOpts,
) -> Result<Option<TurnReport>, RunErr> {
    // Desktop-app adapters (Claude desktop, ChatGPT dots, Grok Bot) drive one conversation at a
    // time and replace an idle one when another thread opens, so the replaced thread's next send
    // finds its context gone (CONTEXT_NOT_FOUND: nothing was sent). The lost-context handoff has
    // already started a new generation from the checkpoint; send the same message there once
    // instead of asking the person to resend it.
    run_turn_streaming(db, tx, rt, session_id, text, opts, None).await
}

/// How often the vendor's event log is read while a turn is still running.
const STREAM_POLL: std::time::Duration = std::time::Duration::from_millis(350);

/// Where a streaming turn's reply text goes as it arrives. `push` forwards a
/// vendor `agent.message.delta`; `finish` emits whatever the final reply holds
/// beyond what was streamed (all of it for a lane that sends no deltas).
pub struct DeltaSink {
    tx: tokio::sync::mpsc::UnboundedSender<String>,
    sent: std::sync::Mutex<String>,
}

impl DeltaSink {
    pub fn new(tx: tokio::sync::mpsc::UnboundedSender<String>) -> Self {
        Self { tx, sent: Default::default() }
    }
    fn push(&self, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        self.sent.lock().unwrap().push_str(chunk);
        let _ = self.tx.send(chunk.to_string());
    }
    fn finish(&self, reply: &str) {
        let sent = self.sent.lock().unwrap().clone();
        if let Some(tail) = reply_tail(&sent, reply) {
            self.push(&tail);
        }
    }
}

/// What of `reply` is still unsent after `sent` was streamed. A reply that does
/// not continue the streamed text (the lane rewrote it) adds nothing: repeating
/// words the caller already heard is worse than ending a sentence early.
fn reply_tail(sent: &str, reply: &str) -> Option<String> {
    let tail = match reply.strip_prefix(sent) {
        Some(t) => t,
        None if sent.is_empty() => reply,
        None => return None,
    };
    (!tail.is_empty()).then(|| tail.to_string())
}

/// [`run_turn`] that forwards the reply to `sink` while the vendor is still
/// producing it: the event log is read alongside the (blocking) send, and every
/// new `agent.message.delta` goes out as it appears. Lanes that send no deltas
/// get the whole reply from `sink.finish` once the turn completes.
pub async fn run_turn_streaming<R: ThreadRuntime>(
    db: &DbHandle,
    tx: &dyn AaiTransport,
    rt: &R,
    session_id: &str,
    text: &str,
    opts: TurnOpts,
    sink: Option<&DeltaSink>,
) -> Result<Option<TurnReport>, RunErr> {
    match run_turn_once(db, tx, rt, session_id, text, opts.clone(), sink).await {
        Err(e) if e.code == "CONTEXT_LOST" => run_turn_once(db, tx, rt, session_id, text, opts, sink).await,
        r => r,
    }
}

async fn run_turn_once<R: ThreadRuntime>(
    db: &DbHandle,
    tx: &dyn AaiTransport,
    rt: &R,
    session_id: &str,
    text: &str,
    opts: TurnOpts,
    sink: Option<&DeltaSink>,
) -> Result<Option<TurnReport>, RunErr> {
    let Some(mut cx) = resolve(db, session_id)? else { return Ok(None) };
    let corr = opts.correlation_id.clone().unwrap_or_else(|| id("corr"));
    reconcile_stale(db, tx, &cx).await;

    let mut remote = remote_for(db, &cx.thread_id, cx.generation)?;
    let already_sent = match &remote {
        Some(r) => db
            .connect()?
            .query_row("SELECT COUNT(*) FROM gateway_sends WHERE remote_binding_id = ?1 AND correlation_id = ?2", params![s(r, "id"), corr], |x| x.get::<_, i64>(0))?
            > 0,
        None => false,
    };

    if !already_sent {
        let state = s(&cx.exec, "state");
        let ready = state == "READY" || (state == "DEGRADED" && !opts.consequential);
        if !ready {
            let why = if state == "DEGRADED" {
                "the vendor lane is degraded; consequential work is stopped until it is checked".to_string()
            } else {
                format!("the vendor binding is {state}, not READY; this bot will not fall back to a native brain")
            };
            return Err(blocked(db, &cx, 409, "BINDING_NOT_READY", &why));
        }
        if let Some(until) = cx.exec["health"]["rateLimitedUntil"].as_str().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()) {
            let left = (until.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_milliseconds();
            if left > 0 {
                let mut e = RunErr::new(429, "RATE_LIMITED", "the vendor rate-limited this lane; retry later");
                e.retry_after_ms = Some(left as u64);
                return Err(e);
            }
        }
        if opts.consequential {
            gate_allternit(db, &cx, &opts, &corr, text)?;
        }
    }

    // One remote binding per generation.
    let remote_row = match remote.take() {
        Some(r) if s(&r, "state") == "ACTIVE" => r,
        Some(r) if matches!(s(&r, "state").as_str(), "UNBOUND" | "OPENING") => open_remote(db, tx, &cx, Some(r)).await?,
        Some(_) => return Err(RunErr::new(409, "REMOTE_CLOSED", "this generation's vendor context is closed; hand off to a new generation")),
        None => open_remote(db, tx, &cx, None).await?,
    };
    let rid = s(&remote_row, "id");
    let ctx_id = s(&remote_row, "externalContextId");
    // Kernel turn router, shadow (O2/O14): vendor turns get the same ROUTE and a
    // tighten-only consequential GATE as native turns. Never awaited.
    let seq_before: i64 = db.connect()?.query_row("SELECT COALESCE(MAX(seq), 0) FROM bot_events WHERE bot_id = ?1", params![cx.bot_id], |r| r.get(0)).unwrap_or(0);
    if !already_sent {
        crate::gateway_routing::before_send(&corr, &s(&cx.exec, "vendor"), text, opts.consequential);
    }

    let turn_started = std::time::Instant::now();
    let (mut streamed_reply, mut sent_reply): (Option<String>, Option<String>) = (None, None);
    if !already_sent {
        let send = vcall(db, tx, &cx.owner, "agent.context.message", &cx.exec, json!({ "contextId": ctx_id, "text": text, "correlationId": corr }));
        let sent = match sink {
            None => send.await,
            Some(sink) => {
                // The send blocks until the vendor has answered; its deltas land in the event log meanwhile.
                tokio::pin!(send);
                loop {
                    tokio::select! {
                        r = &mut send => break r,
                        _ = tokio::time::sleep(STREAM_POLL) => {
                            if let Ok(Some(live)) = remote_for(db, &cx.thread_id, cx.generation) {
                                if let Ok((_, Some(r))) = pull_events_into(db, tx, &cx, &live, Some(sink)).await {
                                    streamed_reply = Some(r);
                                }
                            }
                        }
                    }
                }
            }
        };
        if let Ok(v) = &sent {
            sent_reply = v["reply"].as_str().map(str::to_string);
        }
        if let Err(e) = &sent {
            if e.code != "CONTEXT_NOT_FOUND" {
                crate::gateway_routing::record_turn(db, &cx.owner, &s(&cx.exec, "vendor"), &corr, turn_started.elapsed().as_millis() as u64, false);
            }
        }
        match sent {
            Ok(_) => {
                db.connect()?.execute(
                    "INSERT OR IGNORE INTO gateway_sends (remote_binding_id, correlation_id, sent_at) VALUES (?1, ?2, ?3)",
                    params![rid, corr, now()],
                )?;
                if opts.consequential {
                    if let Some(a) = &opts.allternit_approval_id {
                        db.connect()?.execute("UPDATE gateway_approvals SET consumed = 1 WHERE id = ?1", params![a])?;
                    }
                }
            }
            Err(e) if e.code == "CONTEXT_NOT_FOUND" => {
                return Err(context_lost(db, rt, &mut cx, &remote_row).await);
            }
            Err(e) => return Err(fail(db, &cx, Some(&remote_row), &e)),
        }
    }

    // A streaming turn already moved the cursor: continue from the stored row, not the one read before the send.
    let remote_now = if sink.is_some() { remote_for(db, &cx.thread_id, cx.generation)?.unwrap_or_else(|| remote_row.clone()) } else { remote_row.clone() };
    let (events, mut reply) = pull_events_into(db, tx, &cx, &remote_now, sink).await?;
    if let Some(sink) = sink {
        reply = reply.or(streamed_reply).or(sent_reply);
        if let Some(r) = &reply {
            sink.finish(r);
        }
    }
    if !already_sent {
        let tools: Vec<String> = db.connect().ok().and_then(|c| {
            let mut st = c.prepare("SELECT event_type, payload FROM bot_events WHERE bot_id = ?1 AND thread_id = ?2 AND seq > ?3").ok()?;
            let rows = st.query_map(params![cx.bot_id, cx.thread_id, seq_before], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).ok()?;
            Some(rows.flatten().filter_map(|(t, p)| crate::gateway_routing::tool_name(&t, &serde_json::from_str(&p).unwrap_or_default())).collect())
        }).unwrap_or_default();
        crate::gateway_routing::after_events(&corr, tools);
        crate::gateway_routing::record_turn(db, &cx.owner, &s(&cx.exec, "vendor"), &corr, turn_started.elapsed().as_millis() as u64, true);
    }
    Ok(Some(TurnReport { reply, events, correlation_id: corr, remote_binding_id: rid }))
}

/// Allternit-authority gate: consequential work needs an approved, unconsumed
/// Allternit approval on this thread before anything is sent.
fn gate_allternit(db: &DbHandle, cx: &Cx, opts: &TurnOpts, corr: &str, text: &str) -> Result<(), RunErr> {
    let conn = db.connect()?;
    if let Some(a) = &opts.allternit_approval_id {
        let ok: i64 = conn.query_row(
            "SELECT COUNT(*) FROM gateway_approvals WHERE id = ?1 AND owner = ?2 AND thread_id = ?3 AND authority = 'allternit' AND state = 'approved' AND consumed = 0",
            params![a, cx.owner, cx.thread_id],
            |r| r.get(0),
        )?;
        if ok > 0 {
            return Ok(());
        }
    }
    // A retry of the same turn reuses its pending approval instead of stacking new ones.
    let pending: Option<String> = conn
        .query_row(
            "SELECT id FROM gateway_approvals WHERE owner = ?1 AND thread_id = ?2 AND authority = 'allternit' AND state = 'pending' AND correlation_id = ?3",
            params![cx.owner, cx.thread_id, corr],
            |r| r.get(0),
        )
        .optional()?;
    let aid = match pending {
        Some(a) => a,
        None => create_approval(db, cx, "allternit", "send consequential work to the vendor", None, json!({ "text": text }), Some(corr))?,
    };
    let mut e = RunErr::new(428, "APPROVAL_REQUIRED", "this action is consequential and needs your approval before it is sent to the vendor");
    e.approval_id = Some(aid);
    Err(e)
}

async fn open_remote(db: &DbHandle, tx: &dyn AaiTransport, cx: &Cx, existing: Option<Value>) -> Result<Value, RunErr> {
    let conn = db.connect()?;
    let rid = match &existing {
        Some(r) => s(r, "id"),
        None => {
            let rid = id("rtb");
            let snap = cx.exec["capabilities"].to_string();
            conn.execute(
                "INSERT INTO remote_thread_bindings (id, owner, thread_id, generation, bot_id, execution_binding_id, capability_snapshot, lane, state, created_at, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'UNBOUND',?9,?9)",
                params![rid, cx.owner, cx.thread_id, cx.generation, cx.bot_id, s(&cx.exec, "id"), snap, cx.exec["preferredLane"].as_str(), now()],
            )?;
            rid
        }
    };
    let cur = one(&conn, &format!("SELECT {REMOTE_COLS} FROM remote_thread_bindings WHERE id = ?1"), &[&rid])?.unwrap_or(Value::Null);
    set_remote_state(db, &cx.owner, &cur, "OPENING")?;
    let mut input = json!({ "threadId": cx.thread_id, "generation": cx.generation, "correlationId": id("corr") });
    if let Some(a) = cx.exec["externalAgentId"].as_str() {
        input["externalAgentId"] = json!(a);
    }
    match vcall(db, tx, &cx.owner, "agent.context.open", &cx.exec, input).await {
        Ok(v) => {
            let ctx = v["contextId"].as_str().unwrap_or_default().to_string();
            let mut snap: Value = cur["capabilitySnapshot"].clone();
            if let (Some(g), Some(o)) = (v.get("guarantee"), snap.as_object_mut()) {
                o.insert("guarantee".into(), g.clone());
            }
            conn.execute(
                "UPDATE remote_thread_bindings SET external_context_id = ?1, capability_snapshot = ?2 WHERE id = ?3",
                params![ctx, snap.to_string(), rid],
            )?;
            let opening = one(&conn, &format!("SELECT {REMOTE_COLS} FROM remote_thread_bindings WHERE id = ?1"), &[&rid])?.unwrap_or(Value::Null);
            set_remote_state(db, &cx.owner, &opening, "ACTIVE")?;
            Ok(one(&conn, &format!("SELECT {REMOTE_COLS} FROM remote_thread_bindings WHERE id = ?1"), &[&rid])?.unwrap_or(Value::Null))
        }
        Err(e) => {
            // No vendor context was opened, so there is nothing to close: back to UNBOUND and the next
            // turn opens again (a gateway outage must not force a handoff to a new generation).
            let opening = one(&conn, &format!("SELECT {REMOTE_COLS} FROM remote_thread_bindings WHERE id = ?1"), &[&rid])?.unwrap_or(Value::Null);
            let _ = set_remote_state(db, &cx.owner, &opening, "UNBOUND");
            Err(fail(db, cx, None, &e))
        }
    }
}

/// Remote context lost. Resume only if the snapshot declares it; otherwise a
/// new generation starts from the checkpoint.
async fn context_lost<R: ThreadRuntime>(db: &DbHandle, rt: &R, cx: &mut Cx, remote: &Value) -> RunErr {
    let _ = set_remote_state(db, &cx.owner, remote, "CLOSED");
    let resume = remote["capabilitySnapshot"]["resume"].as_bool().unwrap_or(false)
        || cx.exec["capabilities"]["resume"].as_bool().unwrap_or(false);
    led(db, &cx.bot_id, &cx.thread_id, None, "gateway.remote_thread.lost", ("system", "gateway"), json!({ "generation": cx.generation, "resumeDeclared": resume }), None);
    if resume {
        return RunErr::new(409, "CONTEXT_LOST_RESUMABLE", "the vendor context was lost; retry to resume it");
    }
    let body = serde_json::from_value(json!({ "summary": "", "reason": "manual" })).expect("handoff body");
    match crate::thread_routes::do_handoff(db, rt, &cx.owner, &cx.thread_id, body).await {
        Ok(_) => RunErr::new(409, "CONTEXT_LOST", "the vendor context was lost; a new generation was started from the checkpoint, resend to continue"),
        Err(e) => blocked(db, cx, 502, "CONTEXT_LOST", &format!("the vendor context was lost and the handoff failed: {e}")),
    }
}

/// Failure table effects (spec "Terms, safety and failure handling").
fn fail(db: &DbHandle, cx: &Cx, remote: Option<&Value>, e: &AaiError) -> RunErr {
    let from = s(&cx.exec, "state");
    let mut to_state: Option<&str> = None;
    let (status, thread_msg): (u16, Option<String>) = match e.code.as_str() {
        "AUTH_REQUIRED" | "AUTH_REVOKED" => {
            to_state = Some("NEEDS_AUTH");
            (409, Some(format!("the vendor account needs you to sign in again: {}", e.human_message)))
        }
        "LANE_BLOCKED" => {
            to_state = Some("DISABLED");
            (403, Some(format!("the vendor lane is blocked and has been switched off: {}", e.human_message)))
        }
        "ADAPTER_DRIFT" => {
            to_state = Some("DEGRADED");
            (503, Some(format!("the vendor adapter drifted; consequential automation is stopped: {}", e.human_message)))
        }
        "RATE_LIMITED" => (429, None),
        "GATEWAY_OFFLINE" => (503, None),
        _ => (502, None),
    };
    if let Ok(conn) = db.connect() {
        if let Some(to) = to_state {
            if from != to && exec_next(&from).contains(&to) {
                let _ = set_exec_state(db, &conn, &cx.owner, &cx.exec, to, &e.code);
            }
        }
        if e.code == "RATE_LIMITED" {
            let ms = e.retry_after_ms.unwrap_or(30_000) as i64;
            let until = (chrono::Utc::now() + chrono::Duration::milliseconds(ms)).to_rfc3339();
            let mut h = cx.exec["health"].clone();
            if !h.is_object() {
                h = json!({});
            }
            h["rateLimitedUntil"] = json!(until);
            let _ = conn.execute("UPDATE bot_execution_bindings SET health_json = ?1, updated_at = ?2 WHERE id = ?3", params![h.to_string(), now(), s(&cx.exec, "id")]);
        }
    }
    if let Some(m) = &thread_msg {
        set_thread_status(db, &cx.bot_id, &cx.thread_id, "needs_you", m);
    }
    let ev = if e.code == "ADAPTER_DRIFT" { "gateway.adapter.drift" } else { "gateway.turn.failed" };
    led(
        db,
        &cx.bot_id,
        &cx.thread_id,
        None,
        ev,
        ("system", "gateway"),
        json!({ "code": e.code, "retryable": e.retryable, "message": e.human_message, "executionState": to_state, "remoteBindingId": remote.map(|r| s(r, "id")) }),
        None,
    );
    let mut r = RunErr::new(status, &e.code, e.human_message.clone());
    r.retry_after_ms = if e.code == "RATE_LIMITED" { e.retry_after_ms } else { None };
    r
}

// ---------------------------------------------------------------- handoff / rebind

/// Sync half of a handoff, called when a thread advances a generation: the old
/// generation's remote binding goes ACTIVE -> HANDOFF_PENDING. Closing happens
/// via [`reconcile_stale`] (spawned here when a runtime is installed, and again
/// on the next turn).
pub fn on_generation_advanced(db: &DbHandle, thread_id: &str, old_generation: i64) {
    let Ok(conn) = db.connect() else { return };
    let olds = rows(
        &conn,
        &format!("SELECT {REMOTE_COLS} FROM remote_thread_bindings WHERE thread_id = ?1 AND generation <= ?2 AND state = 'ACTIVE'"),
        &[&thread_id, &old_generation],
    )
    .unwrap_or_default();
    for r in &olds {
        let _ = set_remote_state(db, &s(r, "owner"), r, "HANDOFF_PENDING");
    }
    if olds.is_empty() {
        return;
    }
    if let (Some(rt), Ok(_)) = (RUNTIME.get(), tokio::runtime::Handle::try_current()) {
        let (db, thread_id) = (rt.db.clone(), thread_id.to_string());
        tokio::spawn(async move {
            if let Some(rt) = RUNTIME.get() {
                close_stale(&db, rt.tx.as_ref(), &thread_id, i64::MAX).await;
            }
        });
    }
}

async fn reconcile_stale(db: &DbHandle, tx: &dyn AaiTransport, cx: &Cx) {
    close_stale(db, tx, &cx.thread_id, cx.generation).await;
}

/// Close every remote binding older than `below_generation` that is still open.
pub async fn close_stale(db: &DbHandle, tx: &dyn AaiTransport, thread_id: &str, below_generation: i64) {
    let Ok(conn) = db.connect() else { return };
    let stale = rows(
        &conn,
        &format!("SELECT {REMOTE_COLS} FROM remote_thread_bindings WHERE thread_id = ?1 AND generation < ?2 AND state IN ('ACTIVE','HANDOFF_PENDING') ORDER BY generation"),
        &[&thread_id, &below_generation],
    )
    .unwrap_or_default();
    // i64::MAX (post-handoff spawn) must not close the live generation.
    let live: i64 = conn
        .query_row("SELECT COALESCE(MAX(generation), 0) FROM bot_thread_sessions WHERE thread_id = ?1", params![thread_id], |r| r.get(0))
        .unwrap_or(0);
    for r in stale.iter().filter(|r| r["generation"].as_i64().unwrap_or(0) < live) {
        let owner = s(r, "owner");
        let _ = set_remote_state(db, &owner, r, "HANDOFF_PENDING");
        let exec = one(&conn, &format!("SELECT {EXEC_COLS} FROM bot_execution_bindings WHERE id = ?1"), &[&s(r, "executionBindingId")]).ok().flatten();
        let closed = match exec {
            Some(e) => match vcall(db, tx, &owner, "agent.context.close", &e, json!({ "contextId": s(r, "externalContextId") })).await {
                Ok(_) => true,
                Err(err) => err.code == "CONTEXT_NOT_FOUND",
            },
            None => true, // binding removed: nothing left to close remotely
        };
        if closed {
            let cur = one(&conn, &format!("SELECT {REMOTE_COLS} FROM remote_thread_bindings WHERE id = ?1"), &[&s(r, "id")]).ok().flatten().unwrap_or(Value::Null);
            let _ = set_remote_state(db, &owner, &cur, "CLOSED");
        }
    }
}

// ---------------------------------------------------------------- event bridge

/// Pull `agent.events` from the binding's cursor and bridge them. Returns
/// (new events, latest completed assistant message text).
pub(crate) async fn pull_events(db: &DbHandle, tx: &dyn AaiTransport, cx: &Cx, remote: &Value) -> Result<(usize, Option<String>), RunErr> {
    pull_events_into(db, tx, cx, remote, None).await
}

/// The text a vendor `agent.message.delta` carries: `chunk` (API/SSE lanes, claude-subscription) or
/// `text` (desktop apps). A `replace` delta rewrites earlier text rather than extending it, so it is not a chunk.
fn delta_chunk(payload: &Value) -> Option<&str> {
    if payload["replace"].as_bool() == Some(true) {
        return None;
    }
    payload["chunk"].as_str().or_else(|| payload["text"].as_str())
}

pub(crate) async fn pull_events_into(db: &DbHandle, tx: &dyn AaiTransport, cx: &Cx, remote: &Value, sink: Option<&DeltaSink>) -> Result<(usize, Option<String>), RunErr> {
    let rid = s(remote, "id");
    let mut cursor = remote["syncCursor"].as_str().map(str::to_string);
    let (mut new, mut reply) = (0usize, None);
    for _ in 0..10 {
        let mut input = json!({ "contextId": s(remote, "externalContextId"), "limit": 200 });
        if let Some(c) = &cursor {
            input["cursor"] = json!(c);
        }
        let v = vcall(db, tx, &cx.owner, "agent.events", &cx.exec, input).await.map_err(|e| fail(db, cx, Some(remote), &e))?;
        let events = v["events"].as_array().cloned().unwrap_or_default();
        let mut last_remote: Option<String> = None;
        for ev in &events {
            if let Some(r) = ev["remote_event_id"].as_str() {
                last_remote = Some(r.to_string());
            }
            if bridge_event(db, cx, remote, ev)? {
                new += 1;
                if let (Some(sink), "agent.message.delta") = (sink, ev["type"].as_str().unwrap_or_default()) {
                    if let Some(chunk) = delta_chunk(&ev["payload"]) {
                        sink.push(chunk);
                    }
                }
                if ev["type"] == "agent.message.completed" {
                    // Adapters name the finished text `text` (desktop apps, managed agents) or `reply`
                    // (subscription, Hermes, loopback); `content` is a string only on older ones.
                    let p = &ev["payload"];
                    reply = p["text"].as_str().or_else(|| p["reply"].as_str()).or_else(|| p["content"].as_str()).map(str::to_string);
                    if let (Some(text), false) = (reply.as_deref(), cx.session_id.is_empty()) {
                        let pick = |k: &str, fallback: Value| ev.get(k).filter(|v| !v.is_null()).cloned().unwrap_or(fallback);
                        let meta = json!({
                            "source": "vendor",
                            "vendor": pick("vendor", cx.exec["vendor"].clone()),
                            "adapter": pick("adapter", cx.exec["adapterId"].clone()),
                            "lane": pick("lane", remote["lane"].clone()),
                            "guarantee": pick("guarantee", json!("best_effort")),
                            "remote_event_id": ev["remote_event_id"],
                        });
                        if let Err(e) = tx.append_transcript(&cx.session_id, text, meta).await {
                            warn!(error = %e, "vendor reply not appended to the session transcript");
                        }
                    }
                }
            }
        }
        let next = v["cursor"].as_str().map(str::to_string);
        db.connect()?.execute(
            "UPDATE remote_thread_bindings SET sync_cursor = COALESCE(?1, sync_cursor), last_remote_event_id = COALESCE(?2, last_remote_event_id), updated_at = ?3 WHERE id = ?4",
            params![next, last_remote, now(), rid],
        )?;
        let done = events.is_empty() || next == cursor || next.is_none();
        cursor = next;
        if done {
            break;
        }
    }
    Ok((new, reply))
}

/// Returns true when the event was new.
fn bridge_event(db: &DbHandle, cx: &Cx, remote: &Value, ev: &Value) -> Result<bool, RunErr> {
    let ty = ev["type"].as_str().unwrap_or("agent.unknown");
    let remote_event = ev["remote_event_id"].as_str();
    let pick = |k: &str, fallback: Value| ev.get(k).filter(|v| !v.is_null()).cloned().unwrap_or(fallback);
    let envelope = json!({
        "botId": cx.bot_id,
        "threadId": cx.thread_id,
        "generationId": cx.generation.to_string(),
        "source": pick("source", json!("vendor")),
        "vendor": pick("vendor", cx.exec["vendor"].clone()),
        "adapter": pick("adapter", cx.exec["adapterId"].clone()),
        "lane": pick("lane", remote["lane"].clone()),
        "guarantee": pick("guarantee", json!("best_effort")),
        // who answered / whose product / over what surface (e.g. Muse / Meta / WhatsApp)
        "who": pick("who", Value::Null),
        "whose": pick("whose", Value::Null),
        "how": pick("how", Value::Null),
        "remoteEventId": remote_event,
        "remoteContextId": pick("remote_context_id", remote["externalContextId"].clone()),
        "causationId": ev.get("causation_id"),
        "correlationId": ev.get("correlation_id"),
    });
    let key = remote_event.map(|r| format!("aai:{}:{}", s(remote, "id"), r));
    let vendor = s(&cx.exec, "vendor");
    let created = led(
        db,
        &cx.bot_id,
        &cx.thread_id,
        Some(&cx.session_id),
        ty,
        ("vendor", if vendor.is_empty() { "vendor" } else { &vendor }),
        json!({ "data": ev["payload"], "envelope": envelope }),
        key.clone(),
    );
    if !created {
        return Ok(false);
    }
    match ty {
        "agent.approval.requested" => {
            let rref = ev["payload"]["approvalId"].as_str().or(remote_event).unwrap_or_default();
            let action = ev["payload"]["action"].as_str().or_else(|| ev["payload"]["summary"].as_str()).unwrap_or("vendor approval");
            let gid = create_approval(db, cx, "vendor", action, Some(rref), ev["payload"].clone(), ev["correlation_id"].as_str())?;
            // The raw vendor event carries the vendor's approval id; stamp the gateway row id
            // too, since that is what `/gateway/approvals/:id/respond` takes.
            if let Some(k) = &key {
                db.connect()?.execute(
                    "UPDATE bot_events SET payload = json_set(payload, '$.data.gatewayApprovalId', ?1) WHERE bot_id = ?2 AND idempotency_key = ?3",
                    params![gid, cx.bot_id, k],
                )?;
            }
        }
        "agent.approval.resolved" => {
            if let Some(rref) = ev["payload"]["approvalId"].as_str() {
                let st = if ev["payload"]["decision"].as_str() == Some("deny") { "denied" } else { "approved" };
                db.connect()?.execute(
                    "UPDATE gateway_approvals SET state = ?1, resolved_at = ?2 WHERE thread_id = ?3 AND remote_ref = ?4 AND state = 'pending'",
                    params![st, now(), cx.thread_id, rref],
                )?;
                dismiss_card(db, rref_row_id(db, &cx.thread_id, rref));
            }
        }
        _ => {}
    }
    Ok(true)
}

fn rref_row_id(db: &DbHandle, thread_id: &str, rref: &str) -> Option<String> {
    db.connect().ok()?.query_row("SELECT id FROM gateway_approvals WHERE thread_id = ?1 AND remote_ref = ?2", params![thread_id, rref], |r| r.get(0)).ok()
}

/// Pull events for a thread's live remote binding on demand (background sync).
pub async fn sync_thread(db: &DbHandle, tx: &dyn AaiTransport, owner: &str, thread_id: &str) -> Result<usize, RunErr> {
    let conn = db.connect()?;
    let sid: Option<String> = conn
        .query_row(
            "SELECT s.session_id FROM bot_thread_sessions s JOIN bot_threads t ON t.id = s.thread_id WHERE t.id = ?1 AND t.user_id = ?2 ORDER BY s.generation DESC LIMIT 1",
            params![thread_id, owner],
            |r| r.get(0),
        )
        .optional()?;
    let sid = sid.ok_or_else(|| RunErr::new(404, "NOT_FOUND", "thread not found"))?;
    let Some(cx) = resolve(db, &sid)? else { return Ok(0) };
    let Some(remote) = remote_for(db, &cx.thread_id, cx.generation)?.filter(|r| s(r, "state") == "ACTIVE") else { return Ok(0) };
    Ok(pull_events(db, tx, &cx, &remote).await?.0)
}

// ---------------------------------------------------------------- approvals

pub(crate) fn create_approval(db: &DbHandle, cx: &Cx, authority: &str, action: &str, remote_ref: Option<&str>, detail: Value, corr: Option<&str>) -> Result<String, RunErr> {
    let conn = db.connect()?;
    if let Some(r) = remote_ref {
        if let Some(existing) = rref_row_id(db, &cx.thread_id, r) {
            return Ok(existing);
        }
    }
    let aid = id("gap");
    let ctx_id = remote_for(db, &cx.thread_id, cx.generation)?.map(|r| s(&r, "externalContextId"));
    conn.execute(
        "INSERT INTO gateway_approvals (id, owner, thread_id, bot_id, generation, authority, action, detail_json, remote_ref, remote_context_id, correlation_id, state, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'pending',?12)",
        params![aid, cx.owner, cx.thread_id, cx.bot_id, cx.generation, authority, action, detail.to_string(), remote_ref, ctx_id, corr, now()],
    )?;
    // The approval card the app already renders; the authority rides along so
    // the card can show the vendor badge.
    let card = json!({ "type": "gateway_approval", "approvalId": aid, "authority": authority, "threadId": cx.thread_id, "action": action, "vendor": s(&cx.exec, "vendor") });
    let _ = conn.execute(
        "INSERT OR IGNORE INTO cowork_approvals (id, user_id, content, source, dismissed) VALUES (?1, ?2, ?3, ?4, 0)",
        params![aid, cx.owner, card.to_string(), format!("gateway:{authority}")],
    );
    led(
        db,
        &cx.bot_id,
        &cx.thread_id,
        Some(&cx.session_id),
        "approval.requested",
        ("system", "gateway"),
        json!({ "approvalId": aid, "authority": authority, "action": action, "remoteRef": remote_ref }),
        None,
    );
    set_thread_status(db, &cx.bot_id, &cx.thread_id, "needs_you", &format!("Approval needed ({authority}): {action}"));
    Ok(aid)
}

fn dismiss_card(db: &DbHandle, aid: Option<String>) {
    if let (Some(a), Ok(conn)) = (aid, db.connect()) {
        let _ = conn.execute("UPDATE cowork_approvals SET dismissed = 1 WHERE id = ?1", params![a]);
    }
}

pub fn list_approvals(db: &DbHandle, owner: &str, thread_id: &str, state: Option<&str>) -> rusqlite::Result<Vec<Value>> {
    let conn = db.connect()?;
    let cols = "id, thread_id AS threadId, bot_id AS botId, generation, authority, action, detail_json, remote_ref AS remoteRef, state, actor_type AS actorType, actor_id AS actorId, created_at AS createdAt, resolved_at AS resolvedAt";
    let st = state.unwrap_or("%");
    rows(
        &conn,
        &format!("SELECT {cols} FROM gateway_approvals WHERE owner = ?1 AND thread_id = ?2 AND state LIKE ?3 ORDER BY created_at, id"),
        &[&owner, &thread_id, &st],
    )
}

/// Resolve an approval. Human actors only, for both authorities. A vendor
/// approval forwards to the vendor; an Allternit approval never touches one.
pub async fn respond_approval(
    db: &DbHandle,
    tx: &dyn AaiTransport,
    owner: &str,
    approval_id: &str,
    decision: &str,
    actor: (&str, &str),
) -> Result<Value, RunErr> {
    if actor.0 != "user" {
        return Err(RunErr::new(403, "HUMAN_REQUIRED", "approvals can only be answered by a person, never a bot"));
    }
    if decision != "approve" && decision != "deny" {
        return Err(RunErr::new(400, "BAD_DECISION", "decision must be approve or deny"));
    }
    let conn = db.connect()?;
    let ap = one(
        &conn,
        "SELECT id, thread_id, bot_id, generation, authority, remote_ref, state FROM gateway_approvals WHERE id = ?1 AND owner = ?2",
        &[&approval_id, &owner],
    )?
    .ok_or_else(|| RunErr::new(404, "NOT_FOUND", "approval not found"))?;
    if s(&ap, "state") != "pending" {
        return Err(RunErr::new(409, "ALREADY_RESOLVED", "this approval is already resolved"));
    }
    let thread_id = s(&ap, "threadId");
    let bot_id = s(&ap, "botId");
    if s(&ap, "authority") == "vendor" {
        let gen = ap["generation"].as_i64().unwrap_or(1);
        let remote = remote_for(db, &thread_id, gen)?
            .filter(|r| s(r, "state") == "ACTIVE")
            .ok_or_else(|| RunErr::new(409, "REMOTE_CLOSED", "the vendor context for this approval is no longer open"))?;
        let exec = one(&conn, &format!("SELECT {EXEC_COLS} FROM bot_execution_bindings WHERE bot_id = ?1 AND owner = ?2"), &[&bot_id, &owner])?
            .ok_or_else(|| RunErr::new(409, "NO_BINDING", "the bot has no execution binding"))?;
        let cx = Cx { owner: owner.into(), thread_id: thread_id.clone(), bot_id: bot_id.clone(), generation: gen, session_id: String::new(), exec: exec.clone() };
        vcall(
            db,
            tx,
            owner,
            "agent.approvals",
            &exec,
            json!({ "op": "respond", "contextId": s(&remote, "externalContextId"), "approvalId": s(&ap, "remoteRef"), "decision": decision, "actor": { "type": "human", "id": actor.1 } }),
        )
        .await
        .map_err(|e| fail(db, &cx, Some(&remote), &e))?;
    }
    let st = if decision == "approve" { "approved" } else { "denied" };
    conn.execute(
        "UPDATE gateway_approvals SET state = ?1, actor_type = ?2, actor_id = ?3, resolved_at = ?4 WHERE id = ?5",
        params![st, actor.0, actor.1, now(), approval_id],
    )?;
    dismiss_card(db, Some(approval_id.to_string()));
    led(db, &bot_id, &thread_id, None, "approval.resolved", ("user", actor.1), json!({ "approvalId": approval_id, "authority": s(&ap, "authority"), "state": st }), None);
    let pending: i64 = conn.query_row("SELECT COUNT(*) FROM gateway_approvals WHERE thread_id = ?1 AND state = 'pending'", params![thread_id], |r| r.get(0))?;
    if pending == 0 {
        let cur: String = conn.query_row("SELECT status FROM bot_threads WHERE id = ?1", params![thread_id], |r| r.get(0)).unwrap_or_default();
        if cur == "needs_you" {
            set_thread_status(db, &bot_id, &thread_id, "working", "");
        }
    }
    Ok(json!({ "approvalId": approval_id, "state": st }))
}

// ---------------------------------------------------------------- process-wide entry points

/// `send_bot_turn` hook: `None` = native path. `Some` = the vendor turn's reply
/// (or the reason it did not run).
pub async fn intercept_turn(session_id: &str, text: &str, opts: TurnOpts) -> Option<Result<String, String>> {
    let rt = RUNTIME.get()?;
    let runtime = crate::thread_routes::GizziRuntime { db: rt.db.clone() };
    match run_turn(&rt.db, rt.tx.as_ref(), &runtime, session_id, text, opts).await {
        Ok(None) => None,
        Ok(Some(r)) => Some(r.reply.ok_or_else(|| "the vendor accepted the turn and has not replied yet".to_string())),
        Err(e) => Some(Err(e.message)),
    }
}

/// [`intercept_turn`] that streams: reply text goes to `deltas` as the vendor produces it, and the
/// returned `Ok` is the final reply (already fully delivered on `deltas`). `None` = native path.
pub async fn intercept_turn_streaming(session_id: &str, text: &str, opts: TurnOpts, deltas: tokio::sync::mpsc::UnboundedSender<String>) -> Option<Result<String, String>> {
    let rt = RUNTIME.get()?;
    let runtime = crate::thread_routes::GizziRuntime { db: rt.db.clone() };
    let sink = DeltaSink::new(deltas);
    match run_turn_streaming(&rt.db, rt.tx.as_ref(), &runtime, session_id, text, opts, Some(&sink)).await {
        Ok(None) => None,
        Ok(Some(r)) => Some(r.reply.ok_or_else(|| "the vendor accepted the turn and has not replied yet".to_string())),
        Err(e) => Some(Err(e.message)),
    }
}

/// Whether the session's thread runs on a vendor (Agent Gateway) binding, so a
/// turn there goes through [`intercept_turn`] and has no gizzi event stream.
pub(crate) fn is_vendor_session(db: &DbHandle, session_id: &str) -> bool {
    matches!(resolve(db, session_id), Ok(Some(_)))
}

/// Stop on a vendor-bound session: cancel the turn running in the vendor's active remote context.
/// `None` = not a vendor session (the caller's native abort applies); `Some(confirmed)` otherwise.
pub(crate) async fn cancel_vendor_turn(db: &DbHandle, tx: &dyn AaiTransport, session_id: &str) -> Result<Option<bool>, RunErr> {
    let Some(cx) = resolve(db, session_id)? else { return Ok(None) };
    let Some(remote) = remote_for(db, &cx.thread_id, cx.generation)? else { return Ok(Some(false)) };
    let ctx_id = s(&remote, "externalContextId");
    if ctx_id.is_empty() {
        return Ok(Some(false));
    }
    let v = vcall(db, tx, &cx.owner, "agent.context.cancel", &cx.exec, json!({ "contextId": ctx_id })).await;
    Ok(Some(v.ok().and_then(|v| v["confirmed"].as_bool()).unwrap_or(false)))
}

/// `POST /agent-sessions/:id/abort` hook: `Some(confirmed)` when the session is vendor-bound.
pub async fn intercept_abort(session_id: &str) -> Option<bool> {
    let rt = RUNTIME.get()?;
    cancel_vendor_turn(&rt.db, rt.tx.as_ref(), session_id).await.ok().flatten()
}

/// `POST /agent-sessions/:id/messages` hook: the transcript-shaped reply, or the error response.
pub async fn intercept_message(session_id: &str, text: &str, metadata: Option<&Value>) -> Option<Response> {
    let rt = RUNTIME.get()?;
    let m = metadata.cloned().unwrap_or(Value::Null);
    let opts = TurnOpts {
        correlation_id: m["correlationId"].as_str().map(str::to_string),
        consequential: m["consequential"].as_bool().unwrap_or(false),
        allternit_approval_id: m["allternitApprovalId"].as_str().map(str::to_string),
    };
    let runtime = crate::thread_routes::GizziRuntime { db: rt.db.clone() };
    match run_turn(&rt.db, rt.tx.as_ref(), &runtime, session_id, text, opts).await {
        Ok(None) => None,
        Ok(Some(r)) => Some(
            Json(json!({
                "id": format!("gw-{}", r.correlation_id),
                "role": "assistant",
                "content": r.reply.clone().unwrap_or_default(),
                "timestamp": now(),
                "metadata": { "gateway": true, "events": r.events, "remoteBindingId": r.remote_binding_id, "pending": r.reply.is_none() },
            }))
            .into_response(),
        ),
        Err(e) => Some(e.into_response()),
    }
}

// ---------------------------------------------------------------- routes

pub fn gateway_runner_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/threads/:id/approvals", get(list_approvals_h))
        .route("/threads/:id/gateway/sync", post(sync_h))
        .route("/gateway/approvals/:id/respond", post(respond_h))
}

#[derive(Deserialize)]
struct ListQ {
    state: Option<String>,
}

async fn list_approvals_h(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(thread_id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<ListQ>,
) -> Response {
    let db = state.db.clone();
    let res = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let owned: i64 = conn.query_row("SELECT COUNT(*) FROM bot_threads WHERE id = ?1 AND user_id = ?2", params![thread_id, user.user_id], |r| r.get(0))?;
        if owned == 0 {
            return Ok(None);
        }
        list_approvals(&db, &user.user_id, &thread_id, q.state.as_deref()).map(Some)
    })
    .await;
    match res {
        Ok(Ok(Some(v))) => Json(json!({ "approvals": v })).into_response(),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, Json(json!({ "error": "thread not found" }))).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "database error" }))).into_response(),
    }
}

#[derive(Deserialize)]
struct RespondBody {
    decision: String,
    actor: Option<ActorIn>,
}
#[derive(Deserialize)]
struct ActorIn {
    #[serde(rename = "type")]
    kind: String,
    id: Option<String>,
}

async fn respond_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(aid): Path<String>, Json(b): Json<RespondBody>) -> Response {
    let kind = b.actor.as_ref().map(|a| a.kind.clone()).unwrap_or_else(|| "user".into());
    let actor_id = b.actor.as_ref().and_then(|a| a.id.clone()).unwrap_or_else(|| user.user_id.clone());
    // The authenticated caller is the actor of record; a claimed id cannot override it.
    let actor_id = if kind == "user" { user.user_id.clone() } else { actor_id };
    let Some(rt) = RUNTIME.get() else {
        // No transport installed: only the human gate can be checked.
        if kind != "user" {
            return RunErr::new(403, "HUMAN_REQUIRED", "approvals can only be answered by a person, never a bot").into_response();
        }
        return RunErr::new(503, "GATEWAY_OFFLINE", "the agent gateway is not connected").into_response();
    };
    match respond_approval(&state.db, rt.tx.as_ref(), &user.user_id, &aid, &b.decision, (&kind, &actor_id)).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn sync_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(thread_id): Path<String>) -> Response {
    let Some(rt) = RUNTIME.get() else {
        return RunErr::new(503, "GATEWAY_OFFLINE", "the agent gateway is not connected").into_response();
    };
    match sync_thread(&state.db, rt.tx.as_ref(), &user.user_id, &thread_id).await {
        Ok(n) => Json(json!({ "events": n })).into_response(),
        Err(e) => e.into_response(),
    }
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::Duration;
    use tower::ServiceExt;

    /// Fake AAI gateway: records calls, serves a scripted event log by index
    /// cursor, and fails any op it is told to.
    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<(String, Value)>>,
        events: Mutex<Vec<Value>>,
        fail: Mutex<HashMap<String, AaiError>>,
        fail_once: Mutex<HashMap<String, AaiError>>,
        opened: Mutex<i64>,
        creds: Mutex<Vec<Option<Value>>>,
        transcript: Mutex<Vec<(String, String, Value)>>,
    }

    impl Fake {
        fn count(&self, op: &str) -> usize {
            self.calls.lock().unwrap().iter().filter(|(o, _)| o == op).count()
        }
        fn push(&self, ev: Value) {
            self.events.lock().unwrap().push(ev);
        }
        fn fail_op(&self, op: &str, code: &str, retry_after_ms: Option<u64>) {
            let mut e = AaiError::new(code, format!("{code} from vendor"));
            e.retry_after_ms = retry_after_ms;
            self.fail.lock().unwrap().insert(op.into(), e);
        }
        fn fail_op_once(&self, op: &str, code: &str) {
            self.fail_once.lock().unwrap().insert(op.into(), AaiError::new(code, format!("{code} from vendor")));
        }
    }

    #[async_trait]
    impl AaiTransport for Fake {
        async fn call_cred(&self, owner: &str, op: &str, binding: &Value, credential: Option<&Value>, input: Value) -> Result<Value, AaiError> {
            self.creds.lock().unwrap().push(credential.cloned());
            self.call(owner, op, binding, input).await
        }
        async fn append_transcript(&self, session_id: &str, text: &str, metadata: Value) -> Result<(), String> {
            self.transcript.lock().unwrap().push((session_id.into(), text.into(), metadata));
            Ok(())
        }
        async fn call(&self, _owner: &str, op: &str, _binding: &Value, input: Value) -> Result<Value, AaiError> {
            self.calls.lock().unwrap().push((op.into(), input.clone()));
            if let Some(e) = self.fail.lock().unwrap().get(op) {
                return Err(e.clone());
            }
            if let Some(e) = self.fail_once.lock().unwrap().remove(op) {
                return Err(e);
            }
            Ok(match op {
                "agent.context.open" => {
                    let mut n = self.opened.lock().unwrap();
                    *n += 1;
                    json!({ "contextId": format!("ctx-{n}"), "guarantee": "best_effort" })
                }
                "agent.events" => {
                    let all = self.events.lock().unwrap();
                    let from: usize = input["cursor"].as_str().and_then(|c| c.parse().ok()).unwrap_or(0);
                    json!({ "events": all[from.min(all.len())..].to_vec(), "cursor": all.len().to_string() })
                }
                _ => json!({}),
            })
        }
    }

    struct Rt;
    impl ThreadRuntime for Rt {
        async fn create_session(&self, _b: &str, _n: &str, _t: &str, _c: bool, _th: &str) -> Result<String, String> {
            Ok("sess-new".into())
        }
        async fn seed(&self, _s: &str, _t: &str) -> Result<(), String> {
            Ok(())
        }
        async fn handoff(&self, s: &str, _r: &str, _c: &str, baton: Option<Value>) -> Result<(String, Value), String> {
            Ok((format!("{s}-next"), baton.unwrap_or_else(|| json!({ "summary": "checkpoint" }))))
        }
        async fn successors(&self, _s: &str) -> Vec<(String, String, Value)> {
            Vec::new()
        }
    }

    fn user(id: &str) -> AuthUser {
        AuthUser { user_id: id.into(), email: None, name: None, avatar_url: None, tenant_id: None, organization_id: None, organization_role: None, organization_slug: None }
    }

    /// bot-native (no binding), bot-vendor (READY vendor binding); th-native, th-vendor.
    async fn setup(tag: &str) -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("allternit-gwr-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::test_helpers::app_state(&dir).await;
        let c = state.db.connect().unwrap();
        for b in ["bot-native", "bot-vendor"] {
            c.execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES (?1, 'user-a', 'b', 'm', 'p', 1, '{}')", params![b]).unwrap();
        }
        for (t, b) in [("th-native", "bot-native"), ("th-vendor", "bot-vendor")] {
            c.execute(
                "INSERT INTO bot_threads (id, user_id, bot_id, title, status, last_activity_at, created_at, updated_at) VALUES (?1,'user-a',?2,'T','working','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
                params![t, b],
            )
            .unwrap();
            c.execute("INSERT INTO bot_thread_sessions (thread_id, generation, session_id, started_at) VALUES (?1, 1, ?2, '2026-01-01T00:00:00Z')", params![t, format!("s-{t}")]).unwrap();
        }
        c.execute(
            "INSERT INTO bot_execution_bindings (id, owner, bot_id, type, vendor, adapter_id, preferred_lane, external_agent_id, capabilities_json, state)
             VALUES ('eb-1','user-a','bot-vendor','vendor','acme','acme-adapter','api','agent-9','{\"resume\":false}','READY')",
            [],
        )
        .unwrap();
        state
    }

    fn exec_state(st: &Arc<AppState>) -> String {
        st.db.connect().unwrap().query_row("SELECT state FROM bot_execution_bindings WHERE id='eb-1'", [], |r| r.get(0)).unwrap()
    }
    fn thread_status(st: &Arc<AppState>) -> String {
        st.db.connect().unwrap().query_row("SELECT status FROM bot_threads WHERE id='th-vendor'", [], |r| r.get(0)).unwrap()
    }
    fn remote_states(st: &Arc<AppState>) -> Vec<(i64, String)> {
        let c = st.db.connect().unwrap();
        let mut q = c.prepare("SELECT generation, state FROM remote_thread_bindings WHERE thread_id='th-vendor' ORDER BY generation").unwrap();
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(|r| r.unwrap()).collect()
    }
    async fn turn(st: &Arc<AppState>, f: &Fake, session: &str, text: &str, o: TurnOpts) -> Result<Option<TurnReport>, RunErr> {
        run_turn(&st.db, f, &Rt, session, text, o).await
    }
    fn key(k: &str) -> TurnOpts {
        TurnOpts { correlation_id: Some(k.into()), ..Default::default() }
    }
    fn done(id: &str, text: &str) -> Value {
        json!({ "type": "agent.message.completed", "remote_event_id": id, "guarantee": "exact", "payload": { "text": text } })
    }
    fn count_events(st: &Arc<AppState>, ty: &str) -> i64 {
        st.db.connect().unwrap().query_row("SELECT COUNT(*) FROM bot_events WHERE thread_id='th-vendor' AND event_type=?1", params![ty], |r| r.get(0)).unwrap()
    }

    /// A lane whose `agent.context.message` blocks until released, like a real vendor mid-reply.
    struct Slow {
        inner: Arc<Fake>,
        gate: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl AaiTransport for Slow {
        async fn call(&self, owner: &str, op: &str, binding: &Value, input: Value) -> Result<Value, AaiError> {
            if op == "agent.context.message" {
                self.gate.notified().await;
            }
            self.inner.call(owner, op, binding, input).await
        }
    }

    fn delta(id: &str, key: &str, chunk: &str) -> Value {
        json!({ "type": "agent.message.delta", "remote_event_id": id, "payload": { key: chunk } })
    }

    async fn collect(rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(d) = rx.try_recv() {
            out.push(d);
        }
        out
    }

    #[tokio::test]
    async fn deltas_reach_the_sink_in_order_while_the_vendor_is_still_answering() {
        let st = setup("stream").await;
        let f = Arc::new(Fake::default());
        let gate = Arc::new(tokio::sync::Notify::new());
        let slow = Slow { inner: f.clone(), gate: gate.clone() };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = DeltaSink::new(tx);
        f.push(delta("d1", "chunk", "Sure. "));
        f.push(delta("d2", "text", "Your day "));
        let run = run_turn_streaming(&st.db, &slow, &Rt, "s-th-vendor", "hi", key("k1"), Some(&sink));
        tokio::pin!(run);
        // The send is blocked: the first sentence still arrives.
        let early = tokio::select! {
            _ = &mut run => panic!("turn finished before the vendor answered"),
            got = async { loop { tokio::time::sleep(Duration::from_millis(50)).await; let g = collect(&mut rx).await; if !g.is_empty() { break g; } } } => got,
        };
        assert_eq!(early.concat().chars().take(6).collect::<String>(), "Sure. ");
        f.push(delta("d3", "chunk", "is clear."));
        f.push(done("e1", "Sure. Your day is clear."));
        gate.notify_one();
        let r = run.await.unwrap().unwrap();
        assert_eq!(r.reply.as_deref(), Some("Sure. Your day is clear."));
        let mut all = early;
        all.extend(collect(&mut rx).await);
        assert_eq!(all.concat(), "Sure. Your day is clear.", "each word once, in order: {all:?}");
        assert_eq!(count_events(&st, "agent.message.delta"), 3, "deltas are bridged once even though read twice");
    }

    #[tokio::test]
    async fn a_lane_without_deltas_delivers_the_final_reply_once() {
        let st = setup("stream-final").await;
        let f = Fake::default();
        f.push(done("e1", "All set."));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = DeltaSink::new(tx);
        let r = run_turn_streaming(&st.db, &f, &Rt, "s-th-vendor", "hi", key("k1"), Some(&sink)).await.unwrap().unwrap();
        assert_eq!(r.reply.as_deref(), Some("All set."));
        assert_eq!(collect(&mut rx).await, vec!["All set.".to_string()]);
    }

    #[tokio::test]
    async fn the_tail_after_the_deltas_is_sent_and_rewrites_are_not_repeated() {
        let st = setup("stream-tail").await;
        let f = Fake::default();
        f.push(delta("d1", "chunk", "Hello "));
        f.push(json!({ "type": "agent.message.delta", "remote_event_id": "d2", "payload": { "text": "Hi there", "replace": true } }));
        f.push(done("e1", "Hello world."));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        run_turn_streaming(&st.db, &f, &Rt, "s-th-vendor", "hi", key("k1"), Some(&DeltaSink::new(tx))).await.unwrap().unwrap();
        assert_eq!(collect(&mut rx).await, vec!["Hello ".to_string(), "world.".to_string()]);
        // A final reply that does not continue what was spoken adds nothing.
        assert_eq!(reply_tail("Hello ", "Goodbye."), None);
        assert_eq!(reply_tail("", "Goodbye."), Some("Goodbye.".into()));
        assert_eq!(reply_tail("Done.", "Done."), None);
    }

    #[tokio::test]
    async fn a_barge_in_drops_the_turn_and_the_vendor_is_cancelled() {
        let st = setup("stream-cancel").await;
        let f = Arc::new(Fake::default());
        let slow = Slow { inner: f.clone(), gate: Arc::new(tokio::sync::Notify::new()) };
        f.push(delta("d1", "chunk", "Let me "));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = DeltaSink::new(tx);
        let run = run_turn_streaming(&st.db, &slow, &Rt, "s-th-vendor", "hi", key("k1"), Some(&sink));
        assert!(tokio::time::timeout(Duration::from_secs(5), run).await.is_err(), "the vendor never answered");
        assert_eq!(collect(&mut rx).await, vec!["Let me ".to_string()]);
        cancel_vendor_turn(&st.db, &*f, "s-th-vendor").await.unwrap();
        assert_eq!(f.count("agent.context.cancel"), 1);
    }

    #[test]
    fn forward_errors_name_the_missing_sessions_computer() {
        let e = forward_error(StatusCode::CONFLICT, &json!({ "error": "sessions_computer_not_bound", "detail": "bind a Sessions computer: PUT /api/v1/subscriptions/binding" }));
        assert_eq!(e.code, "GATEWAY_OFFLINE");
        assert!(e.human_message.contains("PUT /api/v1/subscriptions/binding"), "{}", e.human_message);
        let e = forward_error(StatusCode::SERVICE_UNAVAILABLE, &json!({ "error": "sessions_computer_not_running" }));
        assert_eq!(e.code, "GATEWAY_OFFLINE");
        let e = forward_error(StatusCode::BAD_GATEWAY, &Value::Null);
        assert_eq!((e.code.as_str(), e.human_message.as_str()), ("GATEWAY_UNAVAILABLE", "the subscription gateway answered 502 Bad Gateway"));
    }

    #[tokio::test]
    async fn native_path_is_untouched() {
        let st = setup("native").await;
        let f = Fake::default();
        assert!(turn(&st, &f, "s-th-native", "hi", TurnOpts::default()).await.unwrap().is_none());
        st.db.connect().unwrap().execute("INSERT INTO bot_execution_bindings (id, owner, bot_id, type, state) VALUES ('eb-n','user-a','bot-native','allternit','READY')", []).unwrap();
        assert!(turn(&st, &f, "s-th-native", "hi", TurnOpts::default()).await.unwrap().is_none());
        assert!(turn(&st, &f, "unknown-session", "hi", TurnOpts::default()).await.unwrap().is_none());
        assert!(f.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn stop_on_a_vendor_session_cancels_the_remote_turn() {
        let st = setup("stop").await;
        let f = Fake::default();
        f.push(done("e1", "hello"));
        turn(&st, &f, "s-th-vendor", "hi", key("k1")).await.unwrap().unwrap();
        let r = cancel_vendor_turn(&st.db, &f, "s-th-vendor").await.unwrap();
        assert_eq!(r, Some(false), "fake returns no confirmed flag");
        let calls = f.calls.lock().unwrap().clone();
        let cancel = calls.iter().find(|(op, _)| op == "agent.context.cancel").expect("cancel sent to the vendor");
        assert_eq!(cancel.1["contextId"], "ctx-1");
        // A session with no vendor binding is not ours: the native abort applies.
        assert_eq!(cancel_vendor_turn(&st.db, &f, "s-not-a-thread").await.unwrap(), None);
    }

    #[tokio::test]
    async fn vendor_turn_opens_once_sends_once_and_bridges_events() {
        let st = setup("turn").await;
        let f = Fake::default();
        f.push(done("e1", "hello from vendor"));
        let r = turn(&st, &f, "s-th-vendor", "hi", key("k1")).await.unwrap().unwrap();
        assert_eq!(r.reply.as_deref(), Some("hello from vendor"));
        assert_eq!(remote_states(&st), vec![(1, "ACTIVE".into())]);
        // A retry with the same correlation id does not double-send or re-open.
        turn(&st, &f, "s-th-vendor", "hi", key("k1")).await.unwrap();
        assert_eq!(f.count("agent.context.open"), 1);
        assert_eq!(f.count("agent.context.message"), 1);
        // A new correlation id sends again on the same context.
        turn(&st, &f, "s-th-vendor", "again", key("k2")).await.unwrap();
        assert_eq!(f.count("agent.context.open"), 1);
        assert_eq!(f.count("agent.context.message"), 2);
        // Bridge: one ledger row, envelope in payload, cursor advanced, frozen snapshot.
        assert_eq!(count_events(&st, "agent.message.completed"), 1);
        let c = st.db.connect().unwrap();
        let payload: String = c.query_row("SELECT payload FROM bot_events WHERE event_type='agent.message.completed'", [], |r| r.get(0)).unwrap();
        let p: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(p["envelope"]["vendor"], "acme");
        assert_eq!(p["envelope"]["guarantee"], "exact");
        assert_eq!(p["envelope"]["remoteEventId"], "e1");
        let (cursor, last, lane): (String, String, String) =
            c.query_row("SELECT sync_cursor, last_remote_event_id, lane FROM remote_thread_bindings", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
        assert_eq!((cursor.as_str(), last.as_str(), lane.as_str()), ("1", "e1", "api"));
        // Replaying the same events (cursor lost) dedupes by remote_event_id.
        c.execute("UPDATE remote_thread_bindings SET sync_cursor = NULL", []).unwrap();
        assert_eq!(sync_thread(&st.db, &f, "user-a", "th-vendor").await.unwrap(), 0);
        assert_eq!(count_events(&st, "agent.message.completed"), 1);
    }

    #[tokio::test]
    async fn a_reply_named_reply_is_the_turn_reply() {
        // Subscription adapters (claude-subscription, Hermes) send { reply, messageId };
        // reading only `text` turned every Telegram answer into a failure notice.
        let st = setup("reply-field").await;
        let f = Fake::default();
        f.push(json!({ "type": "agent.message.completed", "remote_event_id": "e1", "guarantee": "best_effort", "payload": { "reply": "Pong!", "messageId": "m1" } }));
        let r = turn(&st, &f, "s-th-vendor", "Ping", key("k1")).await.unwrap().unwrap();
        assert_eq!(r.reply.as_deref(), Some("Pong!"));
    }

    #[tokio::test]
    async fn a_failed_open_is_retried_on_the_next_turn_not_closed() {
        let st = setup("reopen").await;
        let f = Fake::default();
        f.fail_op("agent.context.open", "GATEWAY_OFFLINE", None);
        let e = turn(&st, &f, "s-th-vendor", "hi", key("k1")).await.unwrap_err();
        assert_eq!((e.status, e.code.as_str()), (503, "GATEWAY_OFFLINE"));
        assert_eq!(remote_states(&st), vec![(1, "UNBOUND".into())]);
        f.fail.lock().unwrap().clear();
        f.push(done("e1", "back"));
        let r = turn(&st, &f, "s-th-vendor", "hi", key("k1")).await.unwrap().unwrap();
        assert_eq!(r.reply.as_deref(), Some("back"));
        assert_eq!(remote_states(&st), vec![(1, "ACTIVE".into())], "same generation, same row");
        assert_eq!(f.count("agent.context.open"), 2);
    }

    #[tokio::test]
    async fn handoff_closes_old_binding_and_opens_a_new_one() {
        let st = setup("handoff").await;
        let f = Fake::default();
        turn(&st, &f, "s-th-vendor", "hi", key("k1")).await.unwrap();
        let body = serde_json::from_value(json!({ "summary": "checkpoint", "reason": "manual" })).unwrap();
        crate::thread_routes::do_handoff(&st.db, &Rt, "user-a", "th-vendor", body).await.unwrap();
        // Sync half ran inside the handoff.
        assert_ne!(remote_states(&st)[0].1, "ACTIVE");
        turn(&st, &f, "s-th-vendor", "next gen", key("k2")).await.unwrap();
        assert_eq!(remote_states(&st), vec![(1, "CLOSED".into()), (2, "ACTIVE".into())]);
        assert_eq!(f.count("agent.context.close"), 1);
        assert_eq!(f.count("agent.context.open"), 2);
        let ctx: Vec<String> = {
            let c = st.db.connect().unwrap();
            let mut q = c.prepare("SELECT external_context_id FROM remote_thread_bindings ORDER BY generation").unwrap();
            q.query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
        };
        assert_eq!(ctx, vec!["ctx-1", "ctx-2"]);
    }

    #[tokio::test]
    async fn lost_context_starts_a_new_generation_unless_resume_is_declared() {
        let st = setup("lost").await;
        let f = Fake::default();
        turn(&st, &f, "s-th-vendor", "hi", key("k1")).await.unwrap();
        f.fail_op("agent.context.message", "CONTEXT_NOT_FOUND", None);
        // Lost again on the automatic retry: the error reaches the caller, and it stops there.
        let e = turn(&st, &f, "s-th-vendor", "hi2", key("k2")).await.unwrap_err();
        assert_eq!(e.code, "CONTEXT_LOST");
        let c = st.db.connect().unwrap();
        let gens: i64 = c.query_row("SELECT MAX(generation) FROM bot_thread_sessions WHERE thread_id='th-vendor'", [], |r| r.get(0)).unwrap();
        assert_eq!(gens, 3);
        assert_eq!(remote_states(&st)[0].1, "CLOSED");
    }

    #[tokio::test]
    async fn a_context_replaced_by_another_thread_resends_once_on_a_new_generation() {
        let st = setup("replaced").await;
        let f = Fake::default();
        turn(&st, &f, "s-th-vendor", "hi", key("k1")).await.unwrap();
        // The desktop app moved on to another thread's conversation: this context is gone.
        f.fail_op_once("agent.context.message", "CONTEXT_NOT_FOUND");
        let r = turn(&st, &f, "s-th-vendor", "hi2", key("k2")).await.unwrap().expect("vendor path");
        assert!(r.reply.is_some() || !r.correlation_id.is_empty());
        assert_eq!(f.count("agent.context.open"), 2, "a fresh context was opened for the resend");
        assert_eq!(f.count("agent.context.message"), 3, "first send, the lost one, the resend");
        let c = st.db.connect().unwrap();
        let gens: i64 = c.query_row("SELECT MAX(generation) FROM bot_thread_sessions WHERE thread_id='th-vendor'", [], |r| r.get(0)).unwrap();
        assert_eq!(gens, 2);
    }

    #[tokio::test]
    async fn vendor_approval_needs_you_and_only_a_human_answers_it() {
        let st = setup("vapproval").await;
        let f = Fake::default();
        f.push(json!({ "type": "agent.approval.requested", "remote_event_id": "e1", "payload": { "approvalId": "va-1", "action": "delete the report" } }));
        turn(&st, &f, "s-th-vendor", "go", key("k1")).await.unwrap();
        assert_eq!(thread_status(&st), "needs_you");
        let pending = list_approvals(&st.db, "user-a", "th-vendor", Some("pending")).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0]["authority"], "vendor");
        assert_eq!(pending[0]["remoteRef"], "va-1");
        let aid = pending[0]["id"].as_str().unwrap().to_string();
        // R5/R4: the raw vendor event carries the gateway row id the respond route takes,
        // and its envelope's generationId is a string (gatewayEventSchema).
        let raw: String = st.db.connect().unwrap()
            .query_row("SELECT payload FROM bot_events WHERE event_type = 'agent.approval.requested'", [], |r| r.get(0)).unwrap();
        let raw: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(raw["data"]["gatewayApprovalId"], aid.as_str());
        assert_eq!(raw["data"]["approvalId"], "va-1");
        assert_eq!(raw["envelope"]["generationId"], "1");
        // Replayed request does not duplicate.
        f.push(json!({ "type": "agent.approval.requested", "remote_event_id": "e1b", "payload": { "approvalId": "va-1", "action": "delete the report" } }));
        sync_thread(&st.db, &f, "user-a", "th-vendor").await.unwrap();
        assert_eq!(list_approvals(&st.db, "user-a", "th-vendor", None).unwrap().len(), 1);
        // A bot cannot answer; nothing is forwarded.
        let e = respond_approval(&st.db, &f, "user-a", &aid, "approve", ("bot", "bot-vendor")).await.unwrap_err();
        assert_eq!((e.status, e.code.as_str()), (403, "HUMAN_REQUIRED"));
        assert_eq!(f.count("agent.approvals"), 0);
        // The route enforces the same rule.
        let app = gateway_runner_router().with_state(st.clone());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/gateway/approvals/{aid}/respond"))
                    .header("content-type", "application/json")
                    .extension(user("user-a"))
                    .body(Body::from(json!({ "decision": "approve", "actor": { "type": "bot", "id": "x" } }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        // A human can; it forwards to the vendor and clears needs_you.
        respond_approval(&st.db, &f, "user-a", &aid, "approve", ("user", "user-a")).await.unwrap();
        let calls = f.calls.lock().unwrap().clone();
        let fwd = calls.iter().find(|(o, _)| o == "agent.approvals").unwrap();
        assert_eq!(fwd.1["op"], "respond");
        assert_eq!(fwd.1["approvalId"], "va-1");
        // R2: ApprovalsInput actor is "human" | "system"; the router refuses anything else.
        assert_eq!(fwd.1["actor"]["type"], "human");
        assert_eq!(thread_status(&st), "working");
        // Other users cannot see or answer it.
        assert!(respond_approval(&st.db, &f, "user-b", &aid, "approve", ("user", "user-b")).await.is_err());
    }

    #[tokio::test]
    async fn allternit_approval_gates_consequential_sends_and_never_resolves_a_vendor_one() {
        let st = setup("gate").await;
        let f = Fake::default();
        // A live vendor context (plain turn), with a vendor approval already pending on it.
        f.push(json!({ "type": "agent.approval.requested", "remote_event_id": "e0", "payload": { "approvalId": "va-1", "action": "confirm" } }));
        turn(&st, &f, "s-th-vendor", "hello", key("k0")).await.unwrap();
        let sent_before = f.count("agent.context.message");
        let o = TurnOpts { correlation_id: Some("k1".into()), consequential: true, allternit_approval_id: None };
        let e = turn(&st, &f, "s-th-vendor", "wire the money", o).await.unwrap_err();
        assert_eq!((e.status, e.code.as_str()), (428, "APPROVAL_REQUIRED"));
        assert_eq!(f.count("agent.context.message"), sent_before, "nothing reaches the vendor before approval");
        assert_eq!(thread_status(&st), "needs_you");
        let aid = e.approval_id.unwrap();
        // Unapproved id still blocked.
        let o2 = TurnOpts { correlation_id: Some("k1".into()), consequential: true, allternit_approval_id: Some(aid.clone()) };
        assert_eq!(turn(&st, &f, "s-th-vendor", "wire the money", o2.clone()).await.unwrap_err().code, "APPROVAL_REQUIRED");
        respond_approval(&st.db, &f, "user-a", &aid, "approve", ("user", "user-a")).await.unwrap();
        assert_eq!(f.count("agent.approvals"), 0, "an Allternit approval is never forwarded");
        let all = list_approvals(&st.db, "user-a", "th-vendor", Some("pending")).unwrap();
        assert_eq!(all.len(), 1, "exactly the vendor approval is still pending");
        assert!(all.iter().all(|a| a["authority"] == "vendor"));
        // Now the send goes through, once; the approval is single-use.
        turn(&st, &f, "s-th-vendor", "wire the money", o2.clone()).await.unwrap();
        assert_eq!(f.count("agent.context.message"), sent_before + 1);
        let o3 = TurnOpts { correlation_id: Some("k9".into()), ..o2 };
        assert_eq!(turn(&st, &f, "s-th-vendor", "again", o3).await.unwrap_err().code, "APPROVAL_REQUIRED");
    }

    #[tokio::test]
    async fn vendor_not_ready_never_falls_back_native() {
        let st = setup("notready").await;
        let f = Fake::default();
        st.db.connect().unwrap().execute("UPDATE bot_execution_bindings SET state='NEEDS_AUTH'", []).unwrap();
        let e = turn(&st, &f, "s-th-vendor", "hi", key("k")).await.unwrap_err();
        assert_eq!(e.code, "BINDING_NOT_READY");
        assert_eq!(thread_status(&st), "needs_you");
        assert!(f.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn each_failure_code_has_its_state_effect() {
        for (code, want_exec, want_thread) in [
            ("AUTH_REQUIRED", "NEEDS_AUTH", "needs_you"),
            ("AUTH_REVOKED", "NEEDS_AUTH", "needs_you"),
            ("LANE_BLOCKED", "DISABLED", "needs_you"),
            ("ADAPTER_DRIFT", "DEGRADED", "needs_you"),
        ] {
            let st = setup(code).await;
            let f = Fake::default();
            f.fail_op("agent.context.message", code, None);
            let e = turn(&st, &f, "s-th-vendor", "hi", key("k")).await.unwrap_err();
            assert_eq!(e.code, code);
            assert_eq!(exec_state(&st), want_exec, "{code}");
            assert_eq!(thread_status(&st), want_thread, "{code}");
            // No retry storm on a blocked lane: the next turn is refused locally.
            if want_exec == "DISABLED" {
                f.fail.lock().unwrap().clear();
                let before = f.calls.lock().unwrap().len();
                assert_eq!(turn(&st, &f, "s-th-vendor", "hi", key("k2")).await.unwrap_err().code, "BINDING_NOT_READY");
                assert_eq!(f.calls.lock().unwrap().len(), before);
            }
        }
        // DEGRADED stops consequential automation but not plain chat.
        let st = setup("drift2").await;
        let f = Fake::default();
        f.fail_op("agent.context.message", "ADAPTER_DRIFT", None);
        turn(&st, &f, "s-th-vendor", "hi", key("k")).await.unwrap_err();
        f.fail.lock().unwrap().clear();
        let o = TurnOpts { correlation_id: Some("k2".into()), consequential: true, allternit_approval_id: None };
        assert_eq!(turn(&st, &f, "s-th-vendor", "pay", o).await.unwrap_err().code, "BINDING_NOT_READY");
        assert!(turn(&st, &f, "s-th-vendor", "hello", key("k3")).await.is_ok());
    }

    #[tokio::test]
    async fn rate_limit_is_respected_without_changing_state_or_mixing_contexts() {
        let st = setup("rate").await;
        let f = Fake::default();
        f.fail_op("agent.context.message", "RATE_LIMITED", Some(60_000));
        let e = turn(&st, &f, "s-th-vendor", "hi", key("k")).await.unwrap_err();
        assert_eq!((e.status, e.retry_after_ms), (429, Some(60_000)));
        assert_eq!(exec_state(&st), "READY");
        f.fail.lock().unwrap().clear();
        let before = f.count("agent.context.message");
        let e2 = turn(&st, &f, "s-th-vendor", "hi", key("k2")).await.unwrap_err();
        assert_eq!(e2.code, "RATE_LIMITED");
        assert!(e2.retry_after_ms.unwrap() > 0);
        assert_eq!(f.count("agent.context.message"), before, "nothing sent while limited");
        assert_eq!(f.count("agent.context.open"), 1, "still one context");
    }

    #[tokio::test]
    async fn events_after_cursor_pages_ascending_and_default_stays_newest_first() {
        let st = setup("after").await;
        let f = Fake::default();
        for i in 0..5 {
            f.push(done(&format!("e{i}"), &format!("m{i}")));
        }
        turn(&st, &f, "s-th-vendor", "hi", key("k")).await.unwrap();
        let get = |uri: String| {
            let st = st.clone();
            async move {
                let app = crate::thread_routes::thread_router().with_state(st);
                let resp = app.oneshot(Request::builder().uri(uri).extension(user("user-a")).body(Body::empty()).unwrap()).await.unwrap();
                let b = resp.into_body().collect().await.unwrap().to_bytes();
                serde_json::from_slice::<Value>(&b).unwrap()
            }
        };
        let newest = get("/threads/th-vendor/events".into()).await;
        let seqs: Vec<i64> = newest["events"].as_array().unwrap().iter().map(|e| e["sequence"].as_i64().unwrap()).collect();
        assert!(seqs.windows(2).all(|w| w[0] > w[1]), "default is newest-first: {seqs:?}");
        let p1 = get("/threads/th-vendor/events?after=0&limit=2".into()).await;
        let a: Vec<i64> = p1["events"].as_array().unwrap().iter().map(|e| e["sequence"].as_i64().unwrap()).collect();
        assert_eq!(a.len(), 2);
        assert!(a[0] < a[1]);
        let cur = p1["cursor"].as_i64().unwrap();
        assert_eq!(cur, a[1]);
        let p2 = get(format!("/threads/th-vendor/events?after={cur}&limit=500")).await;
        let b: Vec<i64> = p2["events"].as_array().unwrap().iter().map(|e| e["sequence"].as_i64().unwrap()).collect();
        assert!(b.iter().all(|s| *s > cur) && b.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(a.len() + b.len(), seqs.len());
    }

    fn link_account(st: &Arc<AppState>, auth_type: &str, secret: Option<&str>) {
        std::env::set_var("ALLTERNIT_ENCRYPTION_KEY", "unit-test-encryption-key");
        let c = st.db.connect().unwrap();
        let sealed = secret.map(crate::token_crypto::seal);
        c.execute(
            "INSERT INTO provider_account_bindings (id, owner, vendor, auth_type, secret_ref, state, created_at, updated_at) VALUES ('acct-1','user-a','acme',?1,?2,'CONNECTED','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
            params![auth_type, sealed],
        )
        .unwrap();
        c.execute("UPDATE bot_execution_bindings SET account_binding_id = 'acct-1'", []).unwrap();
    }

    #[tokio::test]
    async fn vendor_reply_lands_in_the_transcript_once_with_attribution() {
        let st = setup("transcript").await;
        let f = Fake::default();
        f.push(done("e1", "hello from vendor"));
        turn(&st, &f, "s-th-vendor", "hi", key("k1")).await.unwrap();
        turn(&st, &f, "s-th-vendor", "again", key("k2")).await.unwrap();
        sync_thread(&st.db, &f, "user-a", "th-vendor").await.unwrap();
        let t = f.transcript.lock().unwrap();
        assert_eq!(t.len(), 1, "a re-pulled event is never appended twice");
        assert_eq!(t[0].0, "s-th-vendor");
        assert_eq!(t[0].1, "hello from vendor");
        for (k, v) in [("source", "vendor"), ("vendor", "acme"), ("adapter", "acme-adapter"), ("lane", "api"), ("guarantee", "exact"), ("remote_event_id", "e1")] {
            assert_eq!(t[0].2[k], v, "{k}");
        }
    }

    #[tokio::test]
    async fn credential_is_attached_only_for_accounts_with_a_secret_and_never_stored_elsewhere() {
        let st = setup("cred").await;
        link_account(&st, "api_key", Some("sk-SECRET-123"));
        let f = Fake::default();
        f.push(done("e1", "ok"));
        turn(&st, &f, "s-th-vendor", "hi", key("k1")).await.unwrap();
        let creds = f.creds.lock().unwrap().clone();
        assert!(!creds.is_empty() && creds.iter().all(|c| c.as_ref().map(|v| v["apiKey"] == "sk-SECRET-123").unwrap_or(false)));
        let c = st.db.connect().unwrap();
        let events: String = c.query_row("SELECT COALESCE(group_concat(payload), '') FROM bot_events", [], |r| r.get(0)).unwrap();
        assert!(!events.contains("sk-SECRET-123"), "the key never reaches bot_events");
        let sealed: String = c.query_row("SELECT secret_ref FROM provider_account_bindings WHERE id='acct-1'", [], |r| r.get(0)).unwrap();
        assert!(!sealed.contains("sk-SECRET-123"));

        // No secret_ref on a browser-session account: no credential at all.
        let st2 = setup("cred2").await;
        link_account(&st2, "browser_session", None);
        let f2 = Fake::default();
        f2.push(done("e1", "ok"));
        turn(&st2, &f2, "s-th-vendor", "hi", key("k1")).await.unwrap();
        assert!(f2.creds.lock().unwrap().iter().all(|c| c.is_none()));
    }

    #[tokio::test]
    async fn api_key_account_without_a_key_fails_fast_to_needs_auth() {
        let st = setup("nokey").await;
        link_account(&st, "api_key", None);
        let f = Fake::default();
        let e = turn(&st, &f, "s-th-vendor", "hi", key("k1")).await.unwrap_err();
        assert_eq!(e.code, "AUTH_REQUIRED");
        assert!(f.calls.lock().unwrap().is_empty(), "nothing is sent to the vendor");
        assert_eq!(exec_state(&st), "NEEDS_AUTH");
        assert_eq!(thread_status(&st), "needs_you");
    }
}
