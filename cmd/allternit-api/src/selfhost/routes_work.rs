//! Worker side of the queue: `/v1/environments/{id}/work/claim`,
//! `/v1/work/{work_id}/{heartbeat,renew,complete,release}`,
//! `/v1/work/{work_id}/tool_calls/next` and `/v1/tool_calls/{id}/result`.
//!
//! Protocol: `docs/SELFHOST_WORKER_PROTOCOL.md`.
//!
//! - One open work item per session. The agent loop queues it the first time
//!   a turn needs a sandbox tool ([`ensure_work`]); model-only turns queue
//!   nothing.
//! - A claim long-polls up to `block_ms` (max 30 s) and wakes as soon as an
//!   item is queued or handed back. An expired lease is claimable at once;
//!   `reclaim_older_than_ms` also takes a live lease that has not been
//!   renewed for that long.
//! - A background sweeper ([`spawn_lease_sweeper`]) puts expired leases back
//!   to ready every 5 s. Tool calls a dead worker held go back to pending on
//!   the next claim (at least once, idempotent on `tool_call_id`).
//! - The item ends when the session is archived, or idle for
//!   `sessions.idle_timeout`. From then on `tool_calls/next` answers 204
//!   with `X-Session-Closed: true` and the worker completes the item.
//!
//! The wake-ups ([`WorkHub`]) live in memory: v1 runs one server replica
//! (plan A9). Every wait also re-reads the store about once a second, so a
//! missed wake-up only costs latency.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Notify;

use super::auth::{EnvKey, SessionSecret};
use super::routes_environments::{not_found_env, policy_json};
use super::store::{
    ClaimRequest, SessionRecord, StoreError, ToolCallRecord, WorkClaim, WorkItem, WorkOutcome,
    WorkStatus, LEASE_TTL_SECS, MAX_TOOL_RESULT_BYTES,
};
use super::{check_len, on_post, rfc3339, rfc3339_opt, ApiError, ApiJson, RouteBook, SelfHostState};

/// Longest long-poll a worker may ask for.
pub const MAX_BLOCK_MS: u64 = 30_000;
/// Re-read the store at least this often while long-polling.
const POLL_EVERY: StdDuration = StdDuration::from_secs(1);
/// How often the sweeper returns expired leases to the queue.
pub const SWEEP_EVERY: StdDuration = StdDuration::from_secs(5);

pub const WORKER_HOSTNAME_HEADER: &str = "x-worker-hostname";
pub const WORKER_VERSION_HEADER: &str = "x-worker-version";
pub const SESSION_CLOSED_HEADER: &str = "x-session-closed";

/// Statuses with a turn in flight. A session in one of them is never idle.
const ACTIVE: &[&str] = &["running", "waiting_for_worker"];

pub fn mount(book: RouteBook) -> RouteBook {
    book.add("/v1/environments/:id/work/claim", vec![on_post(claim)])
        .add("/v1/work/:work_id/heartbeat", vec![on_post(heartbeat)])
        .add("/v1/work/:work_id/renew", vec![on_post(renew)])
        .add("/v1/work/:work_id/complete", vec![on_post(complete)])
        .add("/v1/work/:work_id/release", vec![on_post(release)])
        .add("/v1/work/:work_id/tool_calls/next", vec![on_post(next_tool_call)])
        .add("/v1/tool_calls/:id/result", vec![on_post(submit_result)])
}

// ─── Wake-ups ───────────────────────────────────────────────────────────────

/// In-memory wake-ups and bookkeeping for the queue.
#[derive(Default)]
pub struct WorkHub {
    /// Something became claimable (queued, handed back, swept).
    work: Notify,
    /// Per session: a tool call was queued, or the session closed.
    sessions: Mutex<HashMap<String, Arc<Notify>>>,
    /// Per tool call: it was claimed or got its result.
    calls: Mutex<HashMap<String, Arc<Notify>>>,
    /// Per session: tool calls the agent loop is waiting on.
    in_flight: Mutex<HashMap<String, usize>>,
    /// gizzi session id → our session id (set by the agent loop).
    gizzi_sessions: Mutex<HashMap<String, String>>,
    /// Serializes [`ensure_work`] so a session never gets two open items.
    ensure: tokio::sync::Mutex<()>,
}

fn entry(map: &Mutex<HashMap<String, Arc<Notify>>>, key: &str) -> Arc<Notify> {
    let mut m = map.lock().unwrap_or_else(|e| e.into_inner());
    m.entry(key.to_string()).or_default().clone()
}

impl WorkHub {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn work_notify(&self) -> &Notify {
        &self.work
    }

    pub fn notify_work(&self) {
        self.work.notify_waiters();
    }

    pub fn session_notify(&self, session_id: &str) -> Arc<Notify> {
        entry(&self.sessions, session_id)
    }

    pub fn notify_session(&self, session_id: &str) {
        self.session_notify(session_id).notify_waiters();
    }

    pub fn call_notify(&self, tool_call_id: &str) -> Arc<Notify> {
        entry(&self.calls, tool_call_id)
    }

    pub fn notify_call(&self, tool_call_id: &str) {
        let n = self
            .calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(tool_call_id)
            .cloned();
        if let Some(n) = n {
            n.notify_waiters();
        }
    }

    pub fn forget_call(&self, tool_call_id: &str) {
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(tool_call_id);
    }

    /// Count a tool call the loop waits on. Returns the count after.
    pub fn begin_call(&self, session_id: &str) -> usize {
        let mut m = self.in_flight.lock().unwrap_or_else(|e| e.into_inner());
        let n = m.entry(session_id.to_string()).or_insert(0);
        *n += 1;
        *n
    }

    /// Returns the count after.
    pub fn end_call(&self, session_id: &str) -> usize {
        let mut m = self.in_flight.lock().unwrap_or_else(|e| e.into_inner());
        match m.get_mut(session_id) {
            Some(n) if *n > 1 => {
                *n -= 1;
                *n
            }
            _ => {
                m.remove(session_id);
                0
            }
        }
    }

    pub fn in_flight(&self, session_id: &str) -> usize {
        self.in_flight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .copied()
            .unwrap_or(0)
    }

    pub fn map_gizzi_session(&self, gizzi_session_id: &str, session_id: &str) {
        self.gizzi_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(gizzi_session_id.to_string(), session_id.to_string());
    }

    pub fn session_for_gizzi(&self, gizzi_session_id: &str) -> Option<String> {
        self.gizzi_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(gizzi_session_id)
            .cloned()
    }
}

/// Wait for `notify` (already enabled by the caller) or `max`.
async fn wait_on(notified: std::pin::Pin<&mut tokio::sync::futures::Notified<'_>>, max: StdDuration) {
    let _ = tokio::time::timeout(max, notified).await;
}

// ─── Shared helpers (also used by the internal route) ───────────────────────

/// The session's open work item, queued now if it has none. Never makes a
/// second open item for a session.
pub async fn ensure_work(st: &SelfHostState, session: &SessionRecord) -> Result<WorkItem, ApiError> {
    let _guard = st.work.ensure.lock().await;
    if let Some(w) = st.store.find_open_work_for_session(&session.id).await? {
        return Ok(w);
    }
    let w = st
        .store
        .enqueue_work(&session.environment_id, &session.id)
        .await?;
    tracing::info!(session_id = %session.id, work_id = %w.id, "work item queued");
    if let Some(parent) = super::agent_loop::current_turn_trace(&st.store, &session.id).await {
        super::telemetry::record_work_enqueued(&st.store, &w.id, &parent).await;
    }
    st.work.notify_work();
    Ok(w)
}

/// Is the session finished, as far as its worker is concerned? Archived, or
/// no turn running, no tool call pending and quiet for `idle_timeout`.
pub fn session_closed(st: &SelfHostState, s: &SessionRecord) -> bool {
    if s.archived_at.is_some() || s.status == "archived" {
        return true;
    }
    if ACTIVE.contains(&s.status.as_str()) || st.work.in_flight(&s.id) > 0 {
        return false;
    }
    let idle = Duration::from_std(st.settings.idle_timeout()).unwrap_or(Duration::hours(1));
    Utc::now() - s.updated_at >= idle
}

/// Public `reclaim_older_than_ms` (time since the last renew) to the store's
/// cutoff offset. No value: every expired lease (`0`). A value below the
/// lease length also takes live leases not renewed for that long (a
/// negative offset). A value at or above it changes nothing, so the option
/// can only make more items claimable, never fewer.
pub fn reclaim_cutoff_ms(reclaim_older_than_ms: Option<u64>) -> i64 {
    let ttl_ms = LEASE_TTL_SECS * 1000;
    match reclaim_older_than_ms {
        None => 0,
        Some(ms) => (ms.min(ttl_ms as u64) as i64) - ttl_ms,
    }
}

/// Starts the 5 s lease sweeper. Dropping the handle does not stop it; abort
/// it to stop.
pub fn spawn_lease_sweeper(st: SelfHostState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(SWEEP_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            match st.store.requeue_expired_work(Utc::now(), Duration::zero()).await {
                Ok(0) => {}
                Ok(n) => {
                    super::telemetry::record_expired_leases(n as u64);
                    tracing::info!(count = n, "expired leases returned to the queue");
                    st.work.notify_work();
                }
                Err(e) => tracing::warn!(error = %e, "lease sweep failed"),
            }
        }
    })
}

// ─── JSON shapes ────────────────────────────────────────────────────────────

pub fn lease_json(w: &WorkItem) -> Value {
    json!({
        "work_id": w.id,
        "lease_generation": w.lease_generation,
        "lease_expires_at": rfc3339_opt(w.lease_expires_at),
        "status": w.status.lease_status(),
    })
}

fn claim_json(c: &WorkClaim, policy: Value) -> Value {
    json!({
        "work_id": c.item.id,
        "session_id": c.item.session_id,
        "session_secret": c.session_secret,
        "lease_generation": c.item.lease_generation,
        "lease_expires_at": c.item.lease_expires_at.map(rfc3339),
        "environment": policy,
    })
}

pub fn tool_call_json(c: &ToolCallRecord) -> Value {
    let input = if c.input.is_object() { c.input.clone() } else { json!({}) };
    json!({ "tool_call_id": c.id, "name": c.name, "input": input })
}

fn header_opt(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| v.chars().take(200).collect())
}

fn not_found_work(e: StoreError) -> ApiError {
    match e {
        StoreError::NotFound => ApiError::not_found("No work item with that id."),
        other => other.into(),
    }
}

fn check_block_ms(v: Option<u64>) -> Result<StdDuration, ApiError> {
    match v {
        None => Ok(StdDuration::ZERO),
        Some(ms) if ms <= MAX_BLOCK_MS => Ok(StdDuration::from_millis(ms)),
        Some(_) => Err(ApiError::invalid(
            "invalid_field",
            format!("block_ms must be between 0 and {MAX_BLOCK_MS}."),
        )),
    }
}

// ─── Claim ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimBody {
    pub worker_id: String,
    #[serde(default)]
    pub block_ms: Option<u64>,
    #[serde(default)]
    pub reclaim_older_than_ms: Option<u64>,
}

async fn claim(
    State(st): State<SelfHostState>,
    EnvKey(grant): EnvKey,
    Path(env_id): Path<String>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<ClaimBody>,
) -> Result<Response, ApiError> {
    grant.require_environment(&env_id)?;
    check_len("worker_id", &body.worker_id, 1, 200)?;
    let block = check_block_ms(body.block_ms)?;
    let env = st.store.get_environment(&env_id).await.map_err(not_found_env)?;
    let hostname = header_opt(&headers, WORKER_HOSTNAME_HEADER);
    let version = header_opt(&headers, WORKER_VERSION_HEADER);
    let cutoff = reclaim_cutoff_ms(body.reclaim_older_than_ms);
    let deadline = tokio::time::Instant::now() + block;

    // Recorded even when there is no work, so an idle worker shows as online.
    st.store
        .touch_worker(&env_id, &body.worker_id, None, hostname.as_deref(), version.as_deref(), Utc::now())
        .await?;

    loop {
        let notified = st.work.work_notify().notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        let got = st
            .store
            .claim_work(ClaimRequest {
                environment_id: env_id.clone(),
                worker_id: body.worker_id.clone(),
                reclaim_older_than_ms: Some(cutoff),
                hostname: hostname.clone(),
                version: version.clone(),
                now: Utc::now(),
            })
            .await?;
        if let Some(c) = got {
            // An item of a session that ended has nothing to serve: close it
            // and look again.
            let session = st.store.get_session(&c.item.session_id).await;
            let finished = match &session {
                Ok(s) => s.archived_at.is_some(),
                Err(StoreError::NotFound) => true,
                Err(_) => false,
            };
            if finished {
                let _ = st
                    .store
                    .complete_work(
                        &c.item.id,
                        c.item.lease_generation,
                        WorkOutcome::Cancelled,
                        Some("The session was archived."),
                        Utc::now(),
                    )
                    .await;
                continue;
            }
            tracing::info!(
                work_id = %c.item.id,
                session_id = %c.item.session_id,
                worker_id = %body.worker_id,
                generation = c.item.lease_generation,
                "work claimed"
            );
            // Pending calls may now be served, and the loop may stop waiting
            // for a worker.
            st.work.notify_session(&c.item.session_id);
            let _claim_span = super::telemetry::work_claim_span(&st.store, &c.item, &body.worker_id).await;
            return Ok((StatusCode::OK, Json(claim_json(&c, policy_json(&env)))).into_response());
        }

        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
        wait_on(notified, (deadline - now).min(POLL_EVERY)).await;
    }
}

// ─── Lease routes ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseBody {
    pub lease_generation: i64,
}

/// The item, after checking the environment key owns its environment.
async fn owned_item(st: &SelfHostState, grant: &super::auth::EnvGrant, work_id: &str) -> Result<WorkItem, ApiError> {
    let item = st.store.get_work(work_id).await.map_err(not_found_work)?;
    grant.require_environment(&item.environment_id)?;
    Ok(item)
}

async fn touch(st: &SelfHostState, item: &WorkItem, headers: &HeaderMap) {
    let Some(worker_id) = item.worker_id.as_deref() else {
        return;
    };
    let current = (item.status == WorkStatus::Leased).then_some(item.id.as_str());
    if let Err(e) = st
        .store
        .touch_worker(
            &item.environment_id,
            worker_id,
            current,
            header_opt(headers, WORKER_HOSTNAME_HEADER).as_deref(),
            header_opt(headers, WORKER_VERSION_HEADER).as_deref(),
            Utc::now(),
        )
        .await
    {
        tracing::warn!(error = %e, "touch_worker failed");
    }
}

async fn heartbeat(
    State(st): State<SelfHostState>,
    EnvKey(grant): EnvKey,
    Path(work_id): Path<String>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<LeaseBody>,
) -> Result<Json<Value>, ApiError> {
    owned_item(&st, &grant, &work_id).await?;
    let item = st
        .store
        .heartbeat_work(&work_id, body.lease_generation, Utc::now())
        .await?;
    touch(&st, &item, &headers).await;
    Ok(Json(lease_json(&item)))
}

async fn renew(
    State(st): State<SelfHostState>,
    EnvKey(grant): EnvKey,
    Path(work_id): Path<String>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<LeaseBody>,
) -> Result<Json<Value>, ApiError> {
    owned_item(&st, &grant, &work_id).await?;
    let item = st
        .store
        .renew_work(&work_id, body.lease_generation, Utc::now())
        .await?;
    touch(&st, &item, &headers).await;
    Ok(Json(lease_json(&item)))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompleteBody {
    pub lease_generation: i64,
    #[serde(default)]
    pub outcome: Option<WorkOutcome>,
    #[serde(default)]
    pub error_message: Option<String>,
}

async fn complete(
    State(st): State<SelfHostState>,
    EnvKey(grant): EnvKey,
    Path(work_id): Path<String>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<CompleteBody>,
) -> Result<Json<Value>, ApiError> {
    if let Some(m) = &body.error_message {
        check_len("error_message", m, 0, 2000)?;
    }
    owned_item(&st, &grant, &work_id).await?;
    let item = st
        .store
        .complete_work(
            &work_id,
            body.lease_generation,
            body.outcome.unwrap_or_default(),
            body.error_message.as_deref(),
            Utc::now(),
        )
        .await?;
    touch(&st, &item, &headers).await;
    // A tool call queued in the gap between "closed" and this completion is
    // picked up by the waiting internal route, which queues a new item.
    st.work.notify_session(&item.session_id);
    Ok(Json(lease_json(&item)))
}

async fn release(
    State(st): State<SelfHostState>,
    EnvKey(grant): EnvKey,
    Path(work_id): Path<String>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<LeaseBody>,
) -> Result<Json<Value>, ApiError> {
    let before = owned_item(&st, &grant, &work_id).await?;
    let item = st
        .store
        .release_work(&work_id, body.lease_generation, Utc::now())
        .await?;
    // The released item no longer names its worker: record the worker (from
    // before the release) as alive and idle.
    let mut idle = before;
    idle.status = WorkStatus::Ready;
    touch(&st, &idle, &headers).await;
    st.work.notify_work();
    st.work.notify_session(&item.session_id);
    Ok(Json(lease_json(&item)))
}

// ─── Tool calls ─────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NextBody {
    #[serde(default)]
    pub block_ms: Option<u64>,
}

fn closed_response() -> Response {
    let mut res = StatusCode::NO_CONTENT.into_response();
    res.headers_mut().insert(
        HeaderName::from_static(SESSION_CLOSED_HEADER),
        HeaderValue::from_static("true"),
    );
    res
}

async fn next_tool_call(
    State(st): State<SelfHostState>,
    SessionSecret(grant): SessionSecret,
    Path(work_id): Path<String>,
    ApiJson(body): ApiJson<NextBody>,
) -> Result<Response, ApiError> {
    grant.require_work(&work_id)?;
    let block = check_block_ms(body.block_ms)?;
    let deadline = tokio::time::Instant::now() + block;
    let notify = st.work.session_notify(&grant.session_id);

    loop {
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        if let Some(call) = st
            .store
            .next_tool_call(&work_id, grant.lease_generation)
            .await?
        {
            // Wakes the internal route waiting on this call.
            st.work.notify_call(&call.id);
            return Ok((StatusCode::OK, Json(tool_call_json(&call))).into_response());
        }
        match st.store.get_session(&grant.session_id).await {
            Ok(s) if session_closed(&st, &s) => return Ok(closed_response()),
            Err(StoreError::NotFound) => return Ok(closed_response()),
            Ok(_) => {}
            Err(e) => return Err(e.into()),
        }

        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
        wait_on(notified, (deadline - now).min(POLL_EVERY)).await;
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultBody {
    pub output: String,
    #[serde(default)]
    pub is_error: bool,
}

async fn submit_result(
    State(st): State<SelfHostState>,
    SessionSecret(grant): SessionSecret,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<ResultBody>,
) -> Result<Json<Value>, ApiError> {
    let call = st.store.get_tool_call(&id).await.map_err(|e| match e {
        StoreError::NotFound => ApiError::not_found("No tool call with that id."),
        other => other.into(),
    })?;
    grant.require_session(&call.session_id)?;
    if body.output.len() > MAX_TOOL_RESULT_BYTES {
        return Err(StoreError::ResultTooLarge {
            size: body.output.len(),
            max: MAX_TOOL_RESULT_BYTES,
        }
        .into());
    }
    let outcome = st
        .store
        .submit_tool_result(&id, &body.output, body.is_error)
        .await?;
    if outcome.stored {
        let _result_span = super::telemetry::work_result_span(&st.store, &outcome.call).await;
        st.work.notify_call(&id);
    }
    let status = if outcome.call.is_error { "errored" } else { "completed" };
    Ok(Json(json!({ "tool_call_id": outcome.call.id, "status": status })))
}
