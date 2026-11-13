//! `POST /internal/v1/sessions/{id}/tool_calls`: the agent loop (the gizzi
//! child, `GIZZI_TOOL_HOST=remote`) hands a sandbox tool call to the queue
//! and blocks until a worker posts the result.
//!
//! Loopback only, internal token only (`auth::Internal`). Not in the SDKs.
//!
//! - The path id is the gizzi session id, mapped to our session by the
//!   agent loop. Our own session id is accepted too.
//! - Idempotent on `tool_call_id`: a repeat waits for the same call and
//!   returns its result.
//! - Queues the session's work item if it has none.
//! - While no worker holds the session's item, the session shows
//!   `waiting_for_worker` (reason `no_worker`), with a status event.
//! - 504 `no_worker` when no worker took the call within
//!   `sessions.no_worker_timeout` (or `timeout_ms`); 408 `timeout` when a
//!   worker took it but sent no result within `timeout_ms`. Either way the
//!   call is closed as an error, so a late worker does not run a command the
//!   model already gave up on.

use std::sync::Arc;
use std::time::Duration as StdDuration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::auth::Internal;
use super::routes_work::{ensure_work, WorkHub};
use super::store::{NewToolCall, SessionRecord, StoreError, ToolCallRecord, ToolCallStatus, WorkStatus};
use super::{check_len, on_post, ApiError, ApiJson, RouteBook, SelfHostState};

pub const DEFAULT_TIMEOUT_MS: u64 = 120_000;
/// A call nobody took for this long puts the session in `waiting_for_worker`.
const WAITING_GRACE: StdDuration = StdDuration::from_secs(1);
/// Re-check that the session has an open work item this often (a worker may
/// have completed the item just as the call came in).
const ENSURE_EVERY: StdDuration = StdDuration::from_secs(2);
const POLL_EVERY: StdDuration = StdDuration::from_secs(1);

pub fn mount(book: RouteBook) -> RouteBook {
    book.add("/internal/v1/sessions/:id/tool_calls", vec![on_post(submit_tool_call)])
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmitToolCallBody {
    pub tool_call_id: String,
    pub name: String,
    pub input: Map<String, Value>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// Counts the call as in flight for the session while it lives.
struct InFlight {
    hub: Arc<WorkHub>,
    session_id: String,
}

impl InFlight {
    fn new(hub: Arc<WorkHub>, session_id: &str) -> Self {
        hub.begin_call(session_id);
        Self {
            hub,
            session_id: session_id.to_string(),
        }
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.hub.end_call(&self.session_id);
    }
}

fn result_json(c: &ToolCallRecord) -> Value {
    json!({ "output": c.output.clone().unwrap_or_default(), "is_error": c.is_error })
}

fn no_worker() -> ApiError {
    ApiError::new(
        StatusCode::GATEWAY_TIMEOUT,
        "api_error",
        "no_worker",
        "No worker took the tool call. Start a worker for the session's environment.",
    )
}

fn timed_out() -> ApiError {
    ApiError::new(
        StatusCode::REQUEST_TIMEOUT,
        "api_error",
        "timeout",
        "A worker took the tool call but sent no result in time.",
    )
}

/// The session the path names: a gizzi session id first, else our own id.
async fn resolve_session(st: &SelfHostState, id: &str) -> Result<SessionRecord, ApiError> {
    let ours = st.work.session_for_gizzi(id).unwrap_or_else(|| id.to_string());
    st.store.get_session(&ours).await.map_err(|e| match e {
        StoreError::NotFound => ApiError::not_found("No session with that id."),
        other => other.into(),
    })
}

/// `running` → `waiting_for_worker` (reason `no_worker`), with its event.
async fn enter_waiting(st: &SelfHostState, session_id: &str) {
    match st
        .store
        .transition_session_status(session_id, &["running"], "waiting_for_worker", Some("no_worker"))
        .await
    {
        Ok(Some(_)) => {
            let _ = st
                .append_event(
                    session_id,
                    "status",
                    json!({ "status": "waiting_for_worker", "reason": "no_worker" }),
                )
                .await;
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(error = %e, session_id, "could not mark the session waiting"),
    }
}

/// Back to `running` once no other call of the session is still waiting.
async fn leave_waiting(st: &SelfHostState, session_id: &str) {
    if st.work.in_flight(session_id) > 1 {
        return;
    }
    if let Ok(Some(_)) = st
        .store
        .transition_session_status(session_id, &["waiting_for_worker"], "running", None)
        .await
    {
        let _ = st
            .append_event(session_id, "status", json!({ "status": "running", "reason": null }))
            .await;
    }
}

async fn give_up(st: &SelfHostState, call_id: &str, message: &str) {
    if let Err(e) = st.store.submit_tool_result(call_id, message, true).await {
        tracing::warn!(error = %e, tool_call_id = call_id, "could not close the abandoned tool call");
    }
}

async fn submit_tool_call(
    State(st): State<SelfHostState>,
    _internal: Internal,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<SubmitToolCallBody>,
) -> Result<Json<Value>, ApiError> {
    check_len("tool_call_id", &body.tool_call_id, 1, 200)?;
    check_len("name", &body.name, 1, 200)?;
    let timeout = StdDuration::from_millis(body.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS).max(1));
    let session = resolve_session(&st, &id).await?;
    if session.archived_at.is_some() {
        return Err(ApiError::conflict(
            "session_archived",
            "The session is archived and runs no more tools.",
        ));
    }

    let (call, created) = st
        .store
        .insert_tool_call(NewToolCall {
            id: body.tool_call_id.clone(),
            session_id: session.id.clone(),
            name: body.name.clone(),
            input: Value::Object(body.input),
        })
        .await?;
    if call.status == ToolCallStatus::Done {
        return Ok(Json(result_json(&call)));
    }
    if created {
        tracing::info!(session_id = %session.id, tool_call_id = %call.id, name = %call.name, "tool call queued");
        if let Some(turn) = super::agent_loop::current_turn_trace(&st.store, &session.id).await {
            super::telemetry::start_tool_call(&st.store, &turn, &session.id, &call.id, &call.name).await;
        }
    }

    let _in_flight = InFlight::new(st.work.clone(), &session.id);
    let notify = st.work.call_notify(&call.id);
    st.work.notify_session(&session.id);

    let started = tokio::time::Instant::now();
    let deadline = started + timeout;
    let no_worker_deadline = started + st.settings.no_worker_timeout().min(timeout);
    let mut claimed = false;
    let mut waiting = false;
    let mut last_ensure: Option<tokio::time::Instant> = None;
    let mut item_leased = false;

    let out = loop {
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        let cur = st.store.get_tool_call(&call.id).await?;
        let now = tokio::time::Instant::now();
        match cur.status {
            ToolCallStatus::Done => break Ok(Json(result_json(&cur))),
            ToolCallStatus::Claimed => {
                claimed = true;
                if waiting {
                    leave_waiting(&st, &session.id).await;
                    waiting = false;
                }
            }
            ToolCallStatus::Pending => {
                if last_ensure.map_or(true, |t| now - t >= ENSURE_EVERY) {
                    item_leased = ensure_work(&st, &session).await?.status == WorkStatus::Leased;
                    last_ensure = Some(now);
                }
                if !item_leased && !waiting && now - started >= WAITING_GRACE {
                    enter_waiting(&st, &session.id).await;
                    waiting = true;
                }
                if !claimed && now >= no_worker_deadline {
                    give_up(&st, &call.id, "No worker took this tool call, so it did not run.").await;
                    break Err(no_worker());
                }
            }
        }
        if now >= deadline {
            if claimed {
                give_up(&st, &call.id, "The worker sent no result in time.").await;
                break Err(timed_out());
            }
            give_up(&st, &call.id, "No worker took this tool call, so it did not run.").await;
            break Err(no_worker());
        }
        let wait = deadline.saturating_duration_since(now).min(POLL_EVERY);
        let _ = tokio::time::timeout(wait.max(StdDuration::from_millis(10)), notified).await;
    };

    if waiting {
        leave_waiting(&st, &session.id).await;
    }
    st.work.forget_call(&call.id);
    out
}
