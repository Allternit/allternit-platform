//! `/v1/sessions`: create, read, archive, messages, interrupt, events and
//! the events SSE stream.
//!
//! Turns are run by a [`TurnRunner`]. The server binary uses
//! `agent_loop::GizziTurnRunner` (the supervised gizzi child).
//! [`NotImplementedTurnRunner`] (501 `not_implemented`) is the default of a
//! bare [`SelfHostState`], for tests and tools.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::Json;
use futures::Stream;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use tokio::sync::broadcast::error::RecvError;

use super::auth::User;
use super::routes_agents::check_model;
use super::store::{
    DynStore, EventRecord, NewSession, SessionFilter, SessionRecord, StoreError, StoreResult,
    EVENT_TYPES, SESSION_STATUSES,
};
use super::{
    check_len, list_envelope, on_get, on_post, page_limit, rfc3339, rfc3339_opt, ApiError,
    ApiJson, ApiQuery, RouteBook, SelfHostState,
};

pub const SSE_KEEPALIVE: Duration = Duration::from_secs(15);
const REPLAY_PAGE: i64 = 500;

pub fn mount(book: RouteBook) -> RouteBook {
    book.add("/v1/sessions", vec![on_get(list_sessions), on_post(create_session)])
        .add("/v1/sessions/:id", vec![on_get(get_session)])
        .add("/v1/sessions/:id/archive", vec![on_post(archive_session)])
        .add("/v1/sessions/:id/messages", vec![on_post(send_message)])
        .add("/v1/sessions/:id/interrupt", vec![on_post(interrupt_session)])
        .add("/v1/sessions/:id/events", vec![on_get(list_events)])
        .add("/v1/sessions/:id/events/stream", vec![on_get(stream_events)])
}

// ─── Turn runner ────────────────────────────────────────────────────────────

/// Runs the agent loop for a session's turns.
///
/// Contract with the routes:
/// - `available` is asked before the user message is stored. An error there
///   means no turn can start now, and nothing is stored.
/// - `start_turn` is called after the session moved to `running` and the
///   user message event was stored. It must return quickly and run the turn
///   in the background. From then on the runner owns the session's status
///   and events (`state.append_event`, `store.set_session_status`), including
///   `waiting_for_worker` and the final `idle`/`error`. If it returns an
///   error, the route puts the status back and returns that error.
/// - `interrupt` stops the running turn. The route then cancels open tool
///   calls and sets `interrupted`. The session's work item stays: its worker
///   keeps serving the session until it is archived or idle.
#[async_trait]
pub trait TurnRunner: Send + Sync + 'static {
    fn available(&self) -> Result<(), ApiError> {
        Ok(())
    }

    /// For `/health/ready`.
    fn loop_status(&self) -> LoopStatus {
        LoopStatus::Disabled
    }

    async fn start_turn(
        &self,
        state: &SelfHostState,
        session: &SessionRecord,
        user_event: &EventRecord,
    ) -> Result<(), ApiError>;

    async fn interrupt(&self, state: &SelfHostState, session_id: &str) -> Result<(), ApiError>;
}

/// The agent loop's readiness, as `/health/ready` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopStatus {
    /// This runner needs no child process (tests, the 501 stub).
    Disabled,
    Ready,
    /// Starting or restarting the gizzi child.
    Starting,
    /// It cannot run (for example the gizzi binary is missing).
    Unavailable(String),
}

impl LoopStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            LoopStatus::Disabled => "disabled",
            LoopStatus::Ready => "ready",
            LoopStatus::Starting => "starting",
            LoopStatus::Unavailable(_) => "unavailable",
        }
    }

    /// Does this status let `/health/ready` answer 200?
    pub fn is_ready(&self) -> bool {
        matches!(self, LoopStatus::Disabled | LoopStatus::Ready)
    }
}

/// Every turn is 501. The default runner of a bare state.
pub struct NotImplementedTurnRunner;

const NO_LOOP: &str = "The agent loop is not built into this server yet.";

#[async_trait]
impl TurnRunner for NotImplementedTurnRunner {
    fn available(&self) -> Result<(), ApiError> {
        Err(ApiError::not_implemented(NO_LOOP))
    }

    async fn start_turn(
        &self,
        _state: &SelfHostState,
        _session: &SessionRecord,
        _user_event: &EventRecord,
    ) -> Result<(), ApiError> {
        Err(ApiError::not_implemented(NO_LOOP))
    }

    async fn interrupt(&self, _state: &SelfHostState, _session_id: &str) -> Result<(), ApiError> {
        Ok(())
    }
}

// ─── JSON shapes ────────────────────────────────────────────────────────────

pub fn session_json(s: &SessionRecord) -> Value {
    json!({
        "id": s.id,
        "object": "session",
        "agent_id": s.agent_id,
        "environment_id": s.environment_id,
        "title": s.title,
        "status": s.status,
        "status_reason": s.status_reason,
        "last_seq": s.last_seq,
        "metadata": s.metadata,
        "created_at": rfc3339(s.created_at),
        "updated_at": rfc3339(s.updated_at),
        "archived_at": rfc3339_opt(s.archived_at),
    })
}

pub fn event_json(e: &EventRecord) -> Value {
    json!({
        "id": e.id,
        "object": "event",
        "session_id": e.session_id,
        "seq": e.seq,
        "type": e.event_type,
        "data": e.data,
        "created_at": rfc3339(e.created_at),
    })
}

fn not_found_session(e: StoreError) -> ApiError {
    match e {
        StoreError::NotFound => ApiError::not_found("No session with that id."),
        other => other.into(),
    }
}

/// Statuses a new turn may start from.
const STARTABLE: &[&str] = &["idle", "interrupted", "error"];
/// Statuses with a turn in flight.
const ACTIVE: &[&str] = &["running", "waiting_for_worker"];

// ─── Handlers ───────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
pub struct ListSessionsQuery {
    pub limit: Option<i64>,
    pub after: Option<String>,
    pub agent_id: Option<String>,
    pub environment_id: Option<String>,
    pub status: Option<String>,
    pub include_archived: Option<bool>,
}

async fn list_sessions(
    State(st): State<SelfHostState>,
    _user: User,
    ApiQuery(q): ApiQuery<ListSessionsQuery>,
) -> Result<Json<Value>, ApiError> {
    let limit = page_limit(q.limit)?;
    if let Some(s) = &q.status {
        if !SESSION_STATUSES.contains(&s.as_str()) {
            return Err(ApiError::invalid("invalid_query", format!("Unknown status {s:?}.")));
        }
    }
    let filter = SessionFilter {
        agent_id: q.agent_id,
        environment_id: q.environment_id,
        // Asking for archived sessions by status implies including them.
        include_archived: q.include_archived.unwrap_or(false)
            || q.status.as_deref() == Some("archived"),
        status: q.status,
    };
    let rows = st
        .store
        .list_sessions(&filter, limit + 1, q.after.as_deref())
        .await?;
    Ok(Json(list_envelope(rows, limit, |s| s.id.clone(), session_json)))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSessionBody {
    pub agent_id: String,
    pub environment_id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub metadata: Option<Map<String, Value>>,
}

async fn create_session(
    State(st): State<SelfHostState>,
    _user: User,
    ApiJson(body): ApiJson<CreateSessionBody>,
) -> Result<impl IntoResponse, ApiError> {
    if let Some(t) = &body.title {
        check_len("title", t, 0, 200)?;
    }
    let agent = st.store.get_agent(&body.agent_id).await.map_err(|e| match e {
        StoreError::NotFound => ApiError::not_found("No agent with that id."),
        other => other.into(),
    })?;
    st.store
        .get_environment(&body.environment_id)
        .await
        .map_err(super::routes_environments::not_found_env)?;
    // Refused before anything is stored or sent.
    check_model(&st.settings, &agent.model)?;
    let s = st
        .store
        .create_session(NewSession {
            agent_id: body.agent_id,
            environment_id: body.environment_id,
            title: body.title,
            metadata: Value::Object(body.metadata.unwrap_or_default()),
        })
        .await?;
    Ok((StatusCode::CREATED, Json(session_json(&s))))
}

async fn get_session(
    State(st): State<SelfHostState>,
    _user: User,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let s = st.store.get_session(&id).await.map_err(not_found_session)?;
    Ok(Json(session_json(&s)))
}

/// Stop the session's turn: runner and open tool calls. The work item is
/// left alone; see [`end_session_work`].
async fn stop_session_work(st: &SelfHostState, id: &str, why: &str) -> Result<(), ApiError> {
    st.turn_runner.interrupt(st, id).await?;
    st.store.cancel_open_tool_calls(id, why).await?;
    Ok(())
}

/// The session was archived. A queued item nobody claimed is cancelled. A
/// claimed one stays leased: its worker's next `tool_calls/next` answers
/// 204 `X-Session-Closed: true` and the worker completes it.
async fn end_session_work(st: &SelfHostState, id: &str) -> Result<(), ApiError> {
    if let Some(w) = st.store.find_open_work_for_session(id).await? {
        if w.status == super::store::WorkStatus::Ready {
            st.store.cancel_open_work_for_session(id).await?;
        }
    }
    st.work.notify_session(id);
    Ok(())
}

async fn archive_session(
    State(st): State<SelfHostState>,
    _user: User,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let cur = st.store.get_session(&id).await.map_err(not_found_session)?;
    if cur.archived_at.is_some() {
        return Ok(Json(session_json(&cur)));
    }
    stop_session_work(&st, &id, "Cancelled: the session was archived.").await?;
    st.store.archive_session(&id).await?;
    end_session_work(&st, &id).await?;
    st.append_event(&id, "status", json!({ "status": "archived", "reason": null }))
        .await?;
    let s = st.store.get_session(&id).await?;
    Ok(Json(session_json(&s)))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendMessageBody {
    pub content: String,
    #[serde(default)]
    pub metadata: Option<Map<String, Value>>,
}

async fn send_message(
    State(st): State<SelfHostState>,
    _user: User,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<SendMessageBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_len("content", &body.content, 1, 200_000)?;
    let cur = st.store.get_session(&id).await.map_err(not_found_session)?;
    if cur.archived_at.is_some() {
        return Err(ApiError::conflict(
            "session_archived",
            "The session is archived and takes no new messages.",
        ));
    }
    // Ask before storing anything, so a server that cannot run turns does
    // not leave an unanswered message in the log. The model map may have
    // changed since the session was made: an unmapped model is refused here,
    // before any outbound call (the runner checks again).
    let model = cur
        .agent_snapshot
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    check_model(&st.settings, model)?;
    st.turn_runner.available()?;
    let previous = cur.status.clone();
    let Some(session) = st
        .store
        .transition_session_status(&id, STARTABLE, "running", None)
        .await?
    else {
        return Err(ApiError::conflict("session_busy", "A turn is already running."));
    };
    let mut data = json!({ "role": "user", "content": body.content });
    if let Some(m) = body.metadata {
        data["metadata"] = Value::Object(m);
    }
    let event = match st.append_event(&id, "message", data).await {
        Ok(e) => e,
        Err(e) => {
            let _ = st.store.set_session_status(&id, &previous, None).await;
            return Err(e.into());
        }
    };
    if let Err(e) = st.turn_runner.start_turn(&st, &session, &event).await {
        let _ = st
            .store
            .transition_session_status(&id, &["running"], &previous, cur.status_reason.as_deref())
            .await;
        return Err(e);
    }
    Ok((StatusCode::ACCEPTED, Json(event_json(&event))))
}

async fn interrupt_session(
    State(st): State<SelfHostState>,
    _user: User,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let cur = st.store.get_session(&id).await.map_err(not_found_session)?;
    if !ACTIVE.contains(&cur.status.as_str()) {
        return Ok(Json(session_json(&cur)));
    }
    stop_session_work(&st, &id, "Cancelled: the turn was interrupted.").await?;
    if let Some(s) = st
        .store
        .transition_session_status(&id, ACTIVE, "interrupted", None)
        .await?
    {
        st.append_event(&id, "status", json!({ "status": "interrupted", "reason": null }))
            .await?;
        return Ok(Json(session_json(&s)));
    }
    // The turn finished on its own meanwhile.
    let s = st.store.get_session(&id).await?;
    Ok(Json(session_json(&s)))
}

#[derive(Debug, Default, Deserialize)]
pub struct ListEventsQuery {
    pub limit: Option<i64>,
    pub after: Option<String>,
    pub after_seq: Option<i64>,
    pub types: Option<String>,
}

fn parse_types(raw: Option<&str>) -> Result<Option<Vec<String>>, ApiError> {
    let Some(raw) = raw.filter(|r| !r.trim().is_empty()) else {
        return Ok(None);
    };
    let mut out = Vec::new();
    for t in raw.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        if !EVENT_TYPES.contains(&t) {
            return Err(ApiError::invalid("invalid_query", format!("Unknown event type {t:?}.")));
        }
        out.push(t.to_string());
    }
    Ok(Some(out))
}

fn parse_seq(field: &str, raw: &str) -> Result<i64, ApiError> {
    raw.trim()
        .parse::<i64>()
        .ok()
        .filter(|n| *n >= 0)
        .ok_or_else(|| ApiError::invalid("invalid_cursor", format!("{field} must be a seq number.")))
}

async fn list_events(
    State(st): State<SelfHostState>,
    _user: User,
    Path(id): Path<String>,
    ApiQuery(q): ApiQuery<ListEventsQuery>,
) -> Result<Json<Value>, ApiError> {
    let limit = page_limit(q.limit)?;
    st.store.get_session(&id).await.map_err(not_found_session)?;
    let after = match (q.after_seq, q.after.as_deref()) {
        (Some(n), _) if n < 0 => {
            return Err(ApiError::invalid("invalid_query", "after_seq must be 0 or more."))
        }
        (Some(n), _) => n,
        (None, Some(c)) => parse_seq("after", c)?,
        (None, None) => 0,
    };
    let types = parse_types(q.types.as_deref())?;
    let rows = st
        .store
        .list_events(&id, after, limit + 1, types.as_deref())
        .await?;
    Ok(Json(list_envelope(rows, limit, |e| e.seq.to_string(), event_json)))
}

#[derive(Debug, Default, Deserialize)]
pub struct StreamQuery {
    pub after_seq: Option<i64>,
}

fn sse_frame(e: &EventRecord) -> SseEvent {
    SseEvent::default()
        .id(e.seq.to_string())
        .event(e.event_type.clone())
        .data(event_json(e).to_string())
}

/// Every stored event after `after`, oldest first.
async fn replay(store: &DynStore, session_id: &str, after: i64) -> StoreResult<Vec<EventRecord>> {
    let mut out = Vec::new();
    let mut cursor = after;
    loop {
        let page = store.list_events(session_id, cursor, REPLAY_PAGE, None).await?;
        let n = page.len() as i64;
        if let Some(last) = page.last() {
            cursor = last.seq;
        }
        out.extend(page);
        if n < REPLAY_PAGE {
            return Ok(out);
        }
    }
}

/// SSE: replay from the store after `Last-Event-ID` (or `after_seq`), then
/// live events from the broadcaster. A gap in the live seq, or a lagged
/// receiver, is filled from the store, so no event is lost or repeated.
async fn stream_events(
    State(st): State<SelfHostState>,
    _user: User,
    Path(id): Path<String>,
    ApiQuery(q): ApiQuery<StreamQuery>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>, ApiError> {
    st.store.get_session(&id).await.map_err(not_found_session)?;
    let start = match headers.get("last-event-id").and_then(|v| v.to_str().ok()) {
        Some(raw) if !raw.trim().is_empty() => parse_seq("Last-Event-ID", raw)?,
        _ => match q.after_seq {
            Some(n) if n < 0 => {
                return Err(ApiError::invalid("invalid_query", "after_seq must be 0 or more."))
            }
            Some(n) => n,
            None => 0,
        },
    };
    // Subscribe before the replay so nothing falls between the two.
    let mut rx = st.subscribe();
    let store = st.store.clone();
    let stream = async_stream::stream! {
        let mut last = start;
        let mut pending = match replay(&store, &id, last).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, session_id = %id, "SSE replay failed");
                return;
            }
        };
        loop {
            for ev in pending.drain(..) {
                if ev.seq > last {
                    last = ev.seq;
                    yield Ok(sse_frame(&ev));
                }
            }
            match rx.recv().await {
                Ok(ev) => {
                    if ev.session_id != id || ev.seq <= last {
                        continue;
                    }
                    if ev.seq == last + 1 {
                        last = ev.seq;
                        yield Ok(sse_frame(&ev));
                        continue;
                    }
                    // Out of order or missed: the store has everything up to
                    // this seq (appends are serialized per session).
                    match replay(&store, &id, last).await {
                        Ok(v) => pending = v,
                        Err(e) => {
                            tracing::warn!(error = %e, session_id = %id, "SSE gap fill failed");
                            return;
                        }
                    }
                }
                Err(RecvError::Lagged(_)) => match replay(&store, &id, last).await {
                    Ok(v) => pending = v,
                    Err(e) => {
                        tracing::warn!(error = %e, session_id = %id, "SSE lag fill failed");
                        return;
                    }
                },
                Err(RecvError::Closed) => return,
            }
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(SSE_KEEPALIVE).text("keepalive")))
}

/// For callers that want the stub explicitly.
pub fn not_implemented_runner() -> Arc<dyn TurnRunner> {
    Arc::new(NotImplementedTurnRunner)
}
