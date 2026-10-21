//! Server-owned bot event ledger (`bot_events` table, migration V93).
//!
//! This module is the durable, server-side source of truth for bot activity:
//! clients (web, iOS, agent runtimes) append canonical events with
//! `POST /api/v1/bots/:id/events` and read them back cursor-paginated with
//! `GET /api/v1/bots/:id/events`. Each event gets a per-bot monotonic `seq`
//! assigned in the same transaction as the insert; the seq doubles as the
//! pagination cursor (the web contract in
//! `surfaces/ai.allternit.com/src/lib/bots/bot-activity-api.ts` resumes with
//! `afterSequence = last seen sequence`). Retried appends are deduped by
//! `(bot_id, idempotency_key)`.
//!
//! Operational-state fold (compute-on-read, `GET .../operational-state`):
//! the state is derived on every read from the bot's ~200 most recent events
//! plus any open `agent_runs` rows, so there is no persisted projection to
//! rebuild — a rebuild endpoint would be a no-op and is intentionally absent.
//! Status precedence mirrors the web client
//! (`surfaces/ai.allternit.com/src/lib/bots/bot-operational-state.store.ts:52-63`):
//! waiting_approval > blocked > failed > working > waiting_input > degraded >
//! completed > idle > offline. The fold is deterministic:
//!
//! 1. `pendingApprovalsCount` > 0 → `waiting_approval`. The count is the number
//!    of `task.waiting_for_approval` / `waiting_approval` events minus
//!    subsequent resolution events (`task.approved`, `task.rejected`,
//!    `task.resumed`, `approval.resolved`), floored at zero.
//! 2. Latest status-bearing lifecycle event is a block (`task.blocked`,
//!    `run.blocked`, `goal.blocked`) → `blocked`.
//! 3. Latest lifecycle event is a failure (`task.failed`, `run.failed`,
//!    `goal.failed`) → `failed`.
//! 4. Any `agent_runs` row with `status = 'running'`, or latest lifecycle
//!    event is a start/resume (`task.started`, `task.running`, `run.started`,
//!    `run.running`, `task.resumed`, `run.resumed`, `goal.started`)
//!    → `working`.
//! 5. Latest lifecycle event is an input wait (`task.waiting_for_input`,
//!    `waiting_input`, `input.requested`) → `waiting_input`.
//! 6. Latest lifecycle event is a completion (`task.completed`,
//!    `run.completed`, `goal.completed`) → `completed`.
//! 7. Otherwise → `idle`.
//!
//! `degraded` and `offline` are not derivable from the ledger alone and are
//! never emitted here. `unreadMessagesCount` is always 0 (no server-side
//! message ledger feeds it yet) and `nextRoutineRunAt` is omitted (routines
//! store raw cron expressions with no server-side schedule projection).

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::warn;

use crate::auth::AuthUser;
use crate::db::DbHandle;
use crate::{AppState, BotDesktopControlState};

/// Upper bound on events scanned by the operational-state fold.
const FOLD_WINDOW: i64 = 200;
/// Default page size for event listing (matches the web client default).
const DEFAULT_PAGE_LIMIT: i64 = 50;
/// Maximum page size for event listing.
const MAX_PAGE_LIMIT: i64 = 200;

pub fn bot_event_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/bots/:bot_id/events",
            get(list_bot_events).post(append_bot_event),
        )
        .route("/bots/:bot_id/operational-state", get(get_operational_state))
}

// ─── Wire types ─────────────────────────────────────────────────────────────

/// Append request body (snake_case — server-internal convention).
#[derive(Debug, Deserialize)]
pub struct AppendEventBody {
    pub(crate) event_type: String,
    pub(crate) actor: ActorBody,
    #[serde(default)]
    pub(crate) payload: Value,
    pub(crate) occurred_at: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) goal_id: Option<String>,
    pub(crate) wih_id: Option<String>,
    pub(crate) task_id: Option<String>,
    pub(crate) run_id: Option<String>,
    pub(crate) idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActorBody {
    pub(crate) r#type: String,
    pub(crate) id: String,
}

/// Event as returned to clients — camelCase, matching the web
/// `ActivityEvent` contract (`seq` is exposed as `sequence`).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BotEventView {
    id: String,
    sequence: i64,
    bot_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    goal_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    wih_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
    event_type: String,
    actor: ActorBody,
    payload: Value,
    occurred_at: String,
}

/// Cursor-paginated event page, matching the web `ActivityPage` contract.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ActivityPageView {
    events: Vec<BotEventView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
    has_more: bool,
}

#[derive(Debug, Deserialize)]
struct ListEventsQuery {
    after_sequence: Option<i64>,
    limit: Option<i64>,
    /// Comma-separated event type filter (e.g. `task.running,run.failed`).
    event_types: Option<String>,
}

/// Compute-on-read operational state, camelCase per the web
/// `BotOperationalState` contract.
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct OperationalStateView {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    active_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    active_run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    active_goal_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    active_task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    active_wih_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    activity_label: Option<String>,
    pending_approvals_count: i64,
    unread_messages_count: i64,
    computer_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_routine_run_at: Option<String>,
    last_event_sequence: i64,
    updated_at: String,
}

// ─── Stored row ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) struct StoredBotEvent {
    pub(crate) id: String,
    bot_id: String,
    seq: i64,
    session_id: Option<String>,
    goal_id: Option<String>,
    wih_id: Option<String>,
    task_id: Option<String>,
    run_id: Option<String>,
    event_type: String,
    actor_type: String,
    actor_id: String,
    payload: String,
    occurred_at: String,
}

impl StoredBotEvent {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(StoredBotEvent {
            id: row.get(0)?,
            bot_id: row.get(1)?,
            seq: row.get(2)?,
            session_id: row.get(3)?,
            goal_id: row.get(4)?,
            wih_id: row.get(5)?,
            task_id: row.get(6)?,
            run_id: row.get(7)?,
            event_type: row.get(8)?,
            actor_type: row.get(9)?,
            actor_id: row.get(10)?,
            payload: row.get(11)?,
            occurred_at: row.get(12)?,
        })
    }

    fn into_view(self) -> BotEventView {
        // Payloads are validated as JSON on insert; a corrupt row degrades to
        // null rather than failing the whole read.
        let payload = serde_json::from_str(&self.payload).unwrap_or(Value::Null);
        BotEventView {
            id: self.id,
            sequence: self.seq,
            bot_id: self.bot_id,
            session_id: self.session_id,
            goal_id: self.goal_id,
            wih_id: self.wih_id,
            task_id: self.task_id,
            run_id: self.run_id,
            event_type: self.event_type,
            actor: ActorBody {
                r#type: self.actor_type,
                id: self.actor_id,
            },
            payload,
            occurred_at: self.occurred_at,
        }
    }
}

const EVENT_COLUMNS: &str = "id, bot_id, seq, session_id, goal_id, wih_id, task_id, run_id, \
     event_type, actor_type, actor_id, payload, occurred_at";

// ─── Handlers ───────────────────────────────────────────────────────────────

async fn append_bot_event(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(bot_id): Path<String>,
    Json(body): Json<AppendEventBody>,
) -> impl IntoResponse {
    if !verify_bot_ownership(&state, &user.user_id, &bot_id).await {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "bot not found or access denied"})),
        )
            .into_response();
    }

    if body.event_type.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "event_type is required"})),
        )
            .into_response();
    }
    if body.actor.id.trim().is_empty() || body.actor.r#type.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "actor.type and actor.id are required"})),
        )
            .into_response();
    }
    if !body.payload.is_object() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "payload must be an object"})),
        )
            .into_response();
    }

    let occurred_at = body
        .occurred_at
        .clone()
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());

    let db = state.db.clone();
    let bot_id_for_task = bot_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        append_event(&db, &bot_id_for_task, &body, &occurred_at)
    })
    .await;

    match result {
        Ok(Ok((event, inserted))) => {
            let status = if inserted {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            (status, Json(json!({"event": event.into_view()}))).into_response()
        }
        Ok(Err(e)) => {
            warn!(bot_id, error = %e, "Failed to append bot event");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "failed to append event"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!(bot_id, error = %e, "DB task panicked appending bot event");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "failed to append event"})),
            )
                .into_response()
        }
    }
}

async fn list_bot_events(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(bot_id): Path<String>,
    Query(query): Query<ListEventsQuery>,
) -> impl IntoResponse {
    if !verify_bot_ownership(&state, &user.user_id, &bot_id).await {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "bot not found or access denied"})),
        )
            .into_response();
    }

    let limit = query
        .limit
        .unwrap_or(DEFAULT_PAGE_LIMIT)
        .clamp(1, MAX_PAGE_LIMIT);
    let after = query.after_sequence.unwrap_or(0);
    let event_types: Vec<String> = query
        .event_types
        .as_deref()
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let db = state.db.clone();
    let bot_id_for_task = bot_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        list_events(&db, &bot_id_for_task, after, &event_types, limit)
    })
    .await;

    match result {
        Ok(Ok(page)) => Json(page).into_response(),
        Ok(Err(e)) => {
            warn!(bot_id, error = %e, "Failed to list bot events");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "failed to list events"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!(bot_id, error = %e, "DB task panicked listing bot events");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "failed to list events"})),
            )
                .into_response()
        }
    }
}

async fn get_operational_state(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(bot_id): Path<String>,
) -> impl IntoResponse {
    if !verify_bot_ownership(&state, &user.user_id, &bot_id).await {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "bot not found or access denied"})),
        )
            .into_response();
    }

    let db = state.db.clone();
    let bot_id_for_task = bot_id.clone();
    let result =
        tokio::task::spawn_blocking(move || load_fold_inputs(&db, &bot_id_for_task)).await;

    let (events, running_run_ids) = match result {
        Ok(Ok(inputs)) => inputs,
        Ok(Err(e)) => {
            warn!(bot_id, error = %e, "Failed to load operational-state inputs");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "failed to compute operational state"})),
            )
                .into_response();
        }
        Err(e) => {
            warn!(bot_id, error = %e, "DB task panicked computing operational state");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "failed to compute operational state"})),
            )
                .into_response();
        }
    };

    let computer_state = {
        let sessions = state.bot_desktop_sessions.read().await;
        sessions
            .get(&bot_id)
            .map(|s| match s.control_state {
                BotDesktopControlState::BotControls => "running",
                BotDesktopControlState::HumanControls => "takeover",
                BotDesktopControlState::HumanObserving => "running",
            })
            .unwrap_or("none")
            .to_string()
    };

    let mut view = fold_operational_state(&events, &running_run_ids);
    view.computer_state = computer_state;
    view.updated_at = chrono::Utc::now().to_rfc3339();

    Json(view).into_response()
}

// ─── Persistence ────────────────────────────────────────────────────────────

/// Insert an event, assigning the next per-bot sequence in the same
/// transaction. Returns the stored event and whether it was newly inserted
/// (`false` when the idempotency key replayed an existing row).
pub(crate) fn append_event(
    db: &DbHandle,
    bot_id: &str,
    body: &AppendEventBody,
    occurred_at: &str,
) -> Result<(StoredBotEvent, bool), rusqlite::Error> {
    let mut conn = db.connect()?;

    if let Some(key) = body.idempotency_key.as_deref() {
        if let Some(existing) = find_by_idempotency_key(&conn, bot_id, key)? {
            return Ok((existing, false));
        }
    }

    let payload = body.payload.to_string();
    let event = StoredBotEvent {
        id: uuid::Uuid::new_v4().to_string(),
        bot_id: bot_id.to_string(),
        seq: 0, // assigned below, inside the transaction
        session_id: body.session_id.clone(),
        goal_id: body.goal_id.clone(),
        wih_id: body.wih_id.clone(),
        task_id: body.task_id.clone(),
        run_id: body.run_id.clone(),
        event_type: body.event_type.clone(),
        actor_type: body.actor.r#type.clone(),
        actor_id: body.actor.id.clone(),
        payload,
        occurred_at: occurred_at.to_string(),
    };

    // Reserve the write lock before reading MAX(seq), otherwise concurrent
    // deferred transactions can select the same next sequence.
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let seq: i64 = tx.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM bot_events WHERE bot_id = ?1",
        params![bot_id],
        |row| row.get(0),
    )?;
    let insert_result = tx.execute(
        "INSERT INTO bot_events (
             id, bot_id, seq, session_id, goal_id, wih_id, task_id, run_id,
             event_type, actor_type, actor_id, payload, idempotency_key, occurred_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            event.id,
            event.bot_id,
            seq,
            event.session_id,
            event.goal_id,
            event.wih_id,
            event.task_id,
            event.run_id,
            event.event_type,
            event.actor_type,
            event.actor_id,
            event.payload,
            body.idempotency_key,
            event.occurred_at,
        ],
    );
    match insert_result {
        Ok(_) => {
            tx.commit()?;
            let mut stored = event;
            stored.seq = seq;
            Ok((stored, true))
        }
        Err(e) => {
            // A concurrent append with the same idempotency key can win the
            // UNIQUE(bot_id, idempotency_key) race — replay that row instead
            // of surfacing an error.
            if let Some(key) = body.idempotency_key.as_deref() {
                if matches!(
                    e,
                    rusqlite::Error::SqliteFailure(ref err, _)
                        if err.code == rusqlite::ErrorCode::ConstraintViolation
                ) {
                    drop(tx);
                    if let Some(existing) = find_by_idempotency_key(&conn, bot_id, key)? {
                        return Ok((existing, false));
                    }
                    return Err(e);
                }
            }
            Err(e)
        }
    }
}

fn find_by_idempotency_key(
    conn: &Connection,
    bot_id: &str,
    key: &str,
) -> Result<Option<StoredBotEvent>, rusqlite::Error> {
    let sql = format!(
        "SELECT {} FROM bot_events WHERE bot_id = ?1 AND idempotency_key = ?2 LIMIT 1",
        EVENT_COLUMNS
    );
    conn.query_row(&sql, params![bot_id, key], StoredBotEvent::from_row)
        .optional()
}

/// Fetch one page of events ascending by seq. `limit + 1` rows are read so
/// `has_more` is exact; the cursor is the last returned event's sequence.
fn list_events(
    db: &DbHandle,
    bot_id: &str,
    after_sequence: i64,
    event_types: &[String],
    limit: i64,
) -> Result<ActivityPageView, rusqlite::Error> {
    let conn = db.connect()?;

    let mut sql = format!(
        "SELECT {} FROM bot_events WHERE bot_id = ?1 AND seq > ?2",
        EVENT_COLUMNS
    );
    for i in 0..event_types.len() {
        if i == 0 {
            sql.push_str(" AND event_type IN (");
        } else {
            sql.push_str(", ");
        }
        sql.push_str(&format!("?{}", i + 3));
    }
    if !event_types.is_empty() {
        sql.push(')');
    }
    sql.push_str(&format!(" ORDER BY seq ASC LIMIT ?{}", event_types.len() + 3));

    let mut param_values: Vec<rusqlite::types::Value> = vec![
        bot_id.to_string().into(),
        after_sequence.into(),
    ];
    param_values.extend(event_types.iter().map(|t| t.clone().into()));
    param_values.push((limit + 1).into());

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(param_values), StoredBotEvent::from_row)?;
    let mut events: Vec<StoredBotEvent> = rows.collect::<Result<_, _>>()?;

    let has_more = events.len() as i64 > limit;
    events.truncate(limit as usize);
    let next_cursor = if has_more {
        events.last().map(|e| e.seq.to_string())
    } else {
        None
    };

    Ok(ActivityPageView {
        events: events.into_iter().map(StoredBotEvent::into_view).collect(),
        next_cursor,
        has_more,
    })
}

/// Load the fold inputs: the bot's most recent events (ascending) and the ids
/// of its currently running `agent_runs`.
fn load_fold_inputs(
    db: &DbHandle,
    bot_id: &str,
) -> Result<(Vec<StoredBotEvent>, Vec<String>), rusqlite::Error> {
    let conn = db.connect()?;

    let sql = format!(
        "SELECT {} FROM (
             SELECT {} FROM bot_events WHERE bot_id = ?1 ORDER BY seq DESC LIMIT ?2
         ) ORDER BY seq ASC",
        EVENT_COLUMNS, EVENT_COLUMNS
    );
    let mut stmt = conn.prepare(&sql)?;
    let events = stmt
        .query_map(params![bot_id, FOLD_WINDOW], StoredBotEvent::from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    let mut run_stmt = conn.prepare(
        "SELECT id FROM agent_runs WHERE agent_id = ?1 AND status = 'running' ORDER BY created_at DESC",
    )?;
    let running_run_ids = run_stmt
        .query_map(params![bot_id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;

    Ok((events, running_run_ids))
}

// ─── Operational-state fold ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LifecycleStatus {
    Blocked,
    Failed,
    Working,
    WaitingInput,
    Completed,
}

fn lifecycle_status(event_type: &str) -> Option<LifecycleStatus> {
    match event_type {
        "task.blocked" | "run.blocked" | "goal.blocked" => Some(LifecycleStatus::Blocked),
        "task.failed" | "run.failed" | "goal.failed" => Some(LifecycleStatus::Failed),
        "task.started" | "task.running" | "run.started" | "run.running" | "task.resumed"
        | "run.resumed" | "goal.started" => Some(LifecycleStatus::Working),
        "task.waiting_for_input" | "waiting_input" | "input.requested" => {
            Some(LifecycleStatus::WaitingInput)
        }
        "task.completed" | "run.completed" | "goal.completed" => Some(LifecycleStatus::Completed),
        _ => None,
    }
}

fn is_waiting_for_approval(event_type: &str) -> bool {
    matches!(event_type, "task.waiting_for_approval" | "waiting_approval")
}

fn is_approval_resolution(event_type: &str) -> bool {
    matches!(
        event_type,
        "task.approved" | "task.rejected" | "task.resumed" | "approval.resolved"
    )
}

/// Fold the event window into the wire state. See the module header for the
/// precedence rules this implements.
fn fold_operational_state(
    events: &[StoredBotEvent],
    running_run_ids: &[String],
) -> OperationalStateView {
    let mut pending_approvals: i64 = 0;
    let mut latest_status: Option<LifecycleStatus> = None;
    let mut active_session_id = None;
    let mut active_goal_id = None;
    let mut active_task_id = None;
    let mut active_wih_id = None;
    let mut last_run_id = None;
    let mut activity_label = None;
    let mut last_seq: i64 = 0;

    for event in events {
        if is_waiting_for_approval(&event.event_type) {
            pending_approvals += 1;
        } else if is_approval_resolution(&event.event_type) {
            pending_approvals = (pending_approvals - 1).max(0);
        }

        if let Some(status) = lifecycle_status(&event.event_type) {
            latest_status = Some(status);
        }

        if event.session_id.is_some() {
            active_session_id = event.session_id.clone();
        }
        if event.goal_id.is_some() {
            active_goal_id = event.goal_id.clone();
        }
        if event.task_id.is_some() {
            active_task_id = event.task_id.clone();
        }
        if event.wih_id.is_some() {
            active_wih_id = event.wih_id.clone();
        }
        if event.run_id.is_some() {
            last_run_id = event.run_id.clone();
        }

        if let Ok(payload) = serde_json::from_str::<Value>(&event.payload) {
            if let Some(label) = payload
                .get("activityLabel")
                .or_else(|| payload.get("label"))
                .and_then(Value::as_str)
            {
                activity_label = Some(label.to_string());
            }
        }

        last_seq = event.seq;
    }

    // Precedence: waiting_approval > blocked > failed > working >
    // waiting_input > completed > idle (see module header for the source).
    let status = if pending_approvals > 0 {
        "waiting_approval"
    } else if latest_status == Some(LifecycleStatus::Blocked) {
        "blocked"
    } else if latest_status == Some(LifecycleStatus::Failed) {
        "failed"
    } else if !running_run_ids.is_empty() || latest_status == Some(LifecycleStatus::Working) {
        "working"
    } else if latest_status == Some(LifecycleStatus::WaitingInput) {
        "waiting_input"
    } else if latest_status == Some(LifecycleStatus::Completed) {
        "completed"
    } else {
        "idle"
    };

    let active_run_id = running_run_ids.first().cloned().or(last_run_id);

    OperationalStateView {
        status: status.to_string(),
        active_session_id,
        active_run_id,
        active_goal_id,
        active_task_id,
        active_wih_id,
        activity_label,
        pending_approvals_count: pending_approvals,
        unread_messages_count: 0,
        // Filled in by the handler from the in-memory desktop sessions.
        computer_state: "none".to_string(),
        next_routine_run_at: None,
        last_event_sequence: last_seq,
        // Filled in by the handler at response time.
        updated_at: String::new(),
    }
}

pub(crate) async fn verify_bot_ownership(state: &AppState, user_id: &str, bot_id: &str) -> bool {
    let db = state.db.clone();
    let bot_id = bot_id.to_string();
    let user_id = user_id.to_string();

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt =
            conn.prepare("SELECT 1 FROM agents WHERE id = ?1 AND user_id = ?2 LIMIT 1")?;
        let exists: Option<i64> = stmt
            .query_row(rusqlite::params![bot_id, user_id], |row| row.get(0))
            .ok();
        Ok::<_, rusqlite::Error>(exists.is_some())
    })
    .await;

    match result {
        Ok(Ok(true)) => true,
        Ok(Ok(false)) => false,
        Ok(Err(e)) => {
            warn!(error = %e, "DB error verifying bot ownership");
            false
        }
        Err(e) => {
            warn!(error = %e, "DB task panicked verifying bot ownership");
            false
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use std::path::Path as FsPath;
    use tower::ServiceExt;

    fn test_user(id: &str) -> AuthUser {
        AuthUser {
            user_id: id.to_string(),
            email: Some(format!("{}@example.test", id)),
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: None,
            organization_role: None,
            organization_slug: None,
        }
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "allternit-bot-events-{}-{}",
            tag,
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn seed_bot(state: &AppState, bot_id: &str, user_id: &str) {
        let conn = state.db.connect().unwrap();
        conn.execute(
            "INSERT INTO agents (id, user_id, name, model, provider) VALUES (?1, ?2, ?3, 'm', 'p')",
            params![bot_id, user_id, format!("Bot {}", bot_id)],
        )
        .unwrap();
    }

    async fn body_json(body: Body) -> Value {
        let bytes = body.collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn post_event(
        app: &Router,
        bot_id: &str,
        user_id: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/bots/{}/events", bot_id))
                    .header("content-type", "application/json")
                    .extension(test_user(user_id))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let json = body_json(resp.into_body()).await;
        (status, json)
    }

    async fn get_events(
        app: &Router,
        bot_id: &str,
        user_id: &str,
        query: &str,
    ) -> (StatusCode, Value) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/bots/{}/events{}", bot_id, query))
                    .extension(test_user(user_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let json = body_json(resp.into_body()).await;
        (status, json)
    }

    async fn get_state(app: &Router, bot_id: &str, user_id: &str) -> (StatusCode, Value) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/bots/{}/operational-state", bot_id))
                    .extension(test_user(user_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let json = body_json(resp.into_body()).await;
        (status, json)
    }

    fn event_body(event_type: &str) -> Value {
        json!({
            "event_type": event_type,
            "actor": {"type": "bot", "id": "bot-1"},
            "payload": {"detail": format!("payload for {}", event_type)},
        })
    }

    async fn setup(tag: &str) -> (Router, Arc<AppState>) {
        let temp = temp_dir(tag);
        let state = crate::test_helpers::app_state(&temp).await;
        seed_bot(&state, "bot-1", "user-a");
        let app = bot_event_router().with_state(state.clone());
        (app, state)
    }

    #[tokio::test]
    async fn append_and_read_back() {
        let (app, _state) = setup("append").await;

        let (status, body) = post_event(
            &app,
            "bot-1",
            "user-a",
            json!({
                "event_type": "task.running",
                "actor": {"type": "bot", "id": "bot-1"},
                "payload": {"task": "build"},
                "session_id": "sess-1",
                "goal_id": "goal-1",
                "task_id": "task-1",
                "occurred_at": "2026-08-19T00:00:00Z",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let event = &body["event"];
        assert_eq!(event["sequence"], 1);
        assert_eq!(event["botId"], "bot-1");
        assert_eq!(event["eventType"], "task.running");
        assert_eq!(event["actor"], json!({"type": "bot", "id": "bot-1"}));
        assert_eq!(event["payload"], json!({"task": "build"}));
        assert_eq!(event["sessionId"], "sess-1");
        assert_eq!(event["goalId"], "goal-1");
        assert_eq!(event["taskId"], "task-1");
        assert_eq!(event["occurredAt"], "2026-08-19T00:00:00Z");
        assert!(event["id"].as_str().unwrap().len() > 0);

        let (status, body) = get_events(&app, "bot-1", "user-a", "").await;
        assert_eq!(status, StatusCode::OK);
        let events = body["events"].as_array().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["sequence"], 1);
        assert_eq!(body["hasMore"], false);
        assert!(body.get("nextCursor").is_none());
    }

    #[tokio::test]
    async fn idempotent_append_replays_existing_event() {
        let (app, _state) = setup("idempotent").await;

        let mut body = event_body("task.running");
        body["idempotency_key"] = json!("key-1");

        let (status, first) = post_event(&app, "bot-1", "user-a", body.clone()).await;
        assert_eq!(status, StatusCode::CREATED);

        let (status, second) = post_event(&app, "bot-1", "user-a", body).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(first["event"]["id"], second["event"]["id"]);
        assert_eq!(first["event"]["sequence"], second["event"]["sequence"]);

        let (_, list) = get_events(&app, "bot-1", "user-a", "").await;
        assert_eq!(list["events"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn pagination_walks_pages_with_cursor() {
        let (app, _state) = setup("pagination").await;
        for i in 0..5 {
            let (status, _) = post_event(&app, "bot-1", "user-a", event_body(&format!("task.step{}", i))).await;
            assert_eq!(status, StatusCode::CREATED);
        }

        let (_, page1) = get_events(&app, "bot-1", "user-a", "?limit=2").await;
        let seqs: Vec<i64> = page1["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["sequence"].as_i64().unwrap())
            .collect();
        assert_eq!(seqs, vec![1, 2]);
        assert_eq!(page1["hasMore"], true);
        assert_eq!(page1["nextCursor"], "2");

        let (_, page2) = get_events(&app, "bot-1", "user-a", "?after_sequence=2&limit=2").await;
        let seqs: Vec<i64> = page2["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["sequence"].as_i64().unwrap())
            .collect();
        assert_eq!(seqs, vec![3, 4]);
        assert_eq!(page2["hasMore"], true);
        assert_eq!(page2["nextCursor"], "4");

        let (_, page3) = get_events(&app, "bot-1", "user-a", "?after_sequence=4&limit=2").await;
        let seqs: Vec<i64> = page3["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["sequence"].as_i64().unwrap())
            .collect();
        assert_eq!(seqs, vec![5]);
        assert_eq!(page3["hasMore"], false);
        assert!(page3.get("nextCursor").is_none());
    }

    #[tokio::test]
    async fn event_types_filter_selects_matching_events() {
        let (app, _state) = setup("filter").await;
        for t in ["task.running", "goal.completed", "task.failed"] {
            post_event(&app, "bot-1", "user-a", event_body(t)).await;
        }

        let (_, body) = get_events(&app, "bot-1", "user-a", "?event_types=task.running,task.failed").await;
        let types: Vec<&str> = body["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["eventType"].as_str().unwrap())
            .collect();
        assert_eq!(types, vec!["task.running", "task.failed"]);
    }

    #[tokio::test]
    async fn operational_state_folds_status_transitions() {
        let (app, _state) = setup("fold").await;

        // No events → idle.
        let (status, body) = get_state(&app, "bot-1", "user-a").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "idle");
        assert_eq!(body["lastEventSequence"], 0);
        assert_eq!(body["pendingApprovalsCount"], 0);
        assert_eq!(body["computerState"], "none");

        // task.running → working.
        post_event(&app, "bot-1", "user-a", event_body("task.running")).await;
        let (_, body) = get_state(&app, "bot-1", "user-a").await;
        assert_eq!(body["status"], "working");
        assert_eq!(body["lastEventSequence"], 1);

        // run.failed (later) → failed.
        post_event(&app, "bot-1", "user-a", event_body("run.failed")).await;
        let (_, body) = get_state(&app, "bot-1", "user-a").await;
        assert_eq!(body["status"], "failed");

        // goal.completed (later) → completed.
        post_event(&app, "bot-1", "user-a", event_body("goal.completed")).await;
        let (_, body) = get_state(&app, "bot-1", "user-a").await;
        assert_eq!(body["status"], "completed");
        assert_eq!(body["lastEventSequence"], 3);
    }

    #[tokio::test]
    async fn operational_state_tracks_pending_approvals() {
        let (app, _state) = setup("approvals").await;

        post_event(&app, "bot-1", "user-a", event_body("task.running")).await;
        post_event(&app, "bot-1", "user-a", event_body("task.waiting_for_approval")).await;
        let (_, body) = get_state(&app, "bot-1", "user-a").await;
        assert_eq!(body["status"], "waiting_approval");
        assert_eq!(body["pendingApprovalsCount"], 1);

        post_event(&app, "bot-1", "user-a", event_body("task.approved")).await;
        post_event(&app, "bot-1", "user-a", event_body("task.resumed")).await;
        let (_, body) = get_state(&app, "bot-1", "user-a").await;
        assert_eq!(body["status"], "working");
        assert_eq!(body["pendingApprovalsCount"], 0);
    }

    #[tokio::test]
    async fn operational_state_working_from_open_agent_run() {
        let (app, state) = setup("running-run").await;
        {
            let conn = state.db.connect().unwrap();
            conn.execute(
                "INSERT INTO agent_runs (id, agent_id, user_id, status) VALUES ('run-1', 'bot-1', 'user-a', 'running')",
                [],
            )
            .unwrap();
        }

        let (_, body) = get_state(&app, "bot-1", "user-a").await;
        assert_eq!(body["status"], "working");
        assert_eq!(body["activeRunId"], "run-1");
    }

    #[tokio::test]
    async fn non_owner_and_unknown_bot_are_forbidden() {
        let (app, _state) = setup("ownership").await;

        // Another user does not own the bot.
        let (status, _) = get_events(&app, "bot-1", "user-b", "").await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = post_event(&app, "bot-1", "user-b", event_body("task.running")).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = get_state(&app, "bot-1", "user-b").await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // Unknown bot id is indistinguishable from "not yours".
        let (status, _) = get_events(&app, "no-such-bot", "user-a", "").await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn append_rejects_non_object_payloads() {
        let (app, _state) = setup("payload-shape").await;
        let (status, body) = post_event(
            &app,
            "bot-1",
            "user-a",
            json!({
                "event_type": "task.running",
                "actor": {"type": "bot", "id": "bot-1"},
                "payload": ["not", "an", "object"],
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "payload must be an object");
    }
}
