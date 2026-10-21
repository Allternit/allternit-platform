//! Public Cloud Agents API (`/sessions`).
//!
//! Phase 1 facade over the durable beta session layer (`beta_session_routes`).
//! Both surfaces read and write the same `beta_sessions` /
//! `beta_session_events` / `beta_work_tasks` tables; the public surface
//! differs in three ways:
//!
//! 1. Routes live directly under `/api/v1/sessions` (no `/beta` prefix).
//! 2. Session JSON uses public status names (`idle`, `running`, `waiting`,
//!    `failed`, `archived`) derived from the row status plus in-flight work,
//!    rather than the stored `active`/`archived` vocabulary.
//! 3. Events are emitted with Allternit type names (`session.created`,
//!    `user.message`, `agent.message`, …). The stored history keeps the
//!    legacy vocabulary; translation happens on read.
//! 4. Session permission modes (`always_allow` | `always_ask` | `auto`):
//!    at create time the mode is merged with the agent's per-tool permission
//!    map into `metadata.effective_permissions` (`allow`/`ask` per tool).
//!    Enforcement is payload-and-flag based — the merged map ships to
//!    workers in the lease payload (see `beta_work_routes`) and in-process
//!    gating stamps `requires_approval` on stored `tool_calls` events (see
//!    `beta_session_routes::append_event`).
//!
//! Completions/Responses (`agents_v1_routes`) are a separate Layer A surface
//! and are untouched here. There is deliberately no public `/runtimes`
//! resource and no `OpenAI-Beta` header requirement.

use axum::{
    extract::{Extension, Path, Query, State},
    response::sse::{Event, KeepAlive, Sse},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures::Stream;
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashSet, VecDeque};
use std::{convert::Infallible, sync::Arc, time::Duration};

use crate::{
    auth::AuthUser,
    beta_session_routes as beta, error::ApiError, webhook_subscription_routes, AppState,
};

pub fn cloud_agents_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/sessions", get(list_cloud_sessions).post(create_cloud_session))
        .route("/sessions/:id", get(get_cloud_session))
        .route("/sessions/:id/archive", post(archive_cloud_session))
        .route(
            "/sessions/:id/events",
            get(list_cloud_events).post(send_cloud_events),
        )
        .route("/sessions/:id/events/stream", get(stream_cloud_events))
        .route("/sessions/:id/turns", get(list_cloud_turns))
        .route("/sessions/:id/threads", get(list_cloud_threads))
        .route("/sessions/:id/outputs", get(list_cloud_outputs))
}

// ─── Request bodies ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum AgentRef {
    Id(String),
    Ref {
        id: String,
        version: Option<i64>,
    },
    Inline(InlineAgent),
}

#[derive(Debug, Deserialize)]
struct InlineAgent {
    model: String,
    #[serde(default)]
    instructions: Option<String>,
    #[serde(default)]
    tools: Option<Value>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ComputerSpec {
    #[serde(default = "default_computer_kind")]
    kind: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    os: Option<String>,
    #[serde(default)]
    template_id: Option<String>,
    #[serde(default)]
    provider: Option<String>,
}

fn default_computer_kind() -> String {
    "none".to_string()
}

#[derive(Debug, Deserialize)]
struct CloudBudget {
    max_tokens: Option<u64>,
    max_turns: Option<u64>,
    max_tool_calls: Option<u64>,
    /// User-visible cost ceiling in USD. Telemetry only — Allternit does not
    /// charge this today. Stored so a later billing change has a field.
    #[serde(default)]
    max_cost_usd: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct CreateCloudSessionBody {
    #[serde(default)]
    agent: Option<AgentRef>,
    #[serde(default)]
    computer: Option<ComputerSpec>,
    #[serde(default)]
    input: Option<Value>,
    #[serde(default)]
    stream: bool,
    #[serde(default)]
    vault_ids: Option<Value>,
    #[serde(default)]
    budget: Option<CloudBudget>,
    #[serde(default = "empty_object")]
    metadata: Value,
    #[serde(default)]
    brain_id: Option<Value>,
    #[serde(default)]
    memory_store_ids: Option<Value>,
    #[serde(default)]
    parent_thread_id: Option<String>,
    #[serde(default)]
    permission: Option<String>,
    /// Bot Agents BA-8: packaged bot id. Stored on metadata; used as the
    /// session agent when `agent` is omitted.
    #[serde(default)]
    bot_id: Option<String>,
}

fn empty_object() -> Value {
    json!({})
}

#[derive(Debug, Deserialize)]
struct SendEvent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    content: Option<Value>,
    #[serde(default)]
    data: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct SendEventsBody {
    events: Vec<SendEvent>,
}

#[derive(Debug, Deserialize)]
struct EventQuery {
    after: Option<i64>,
}

// ─── Public shape mapping ────────────────────────────────────────────────────

/// Public status names. Stored rows keep the legacy `active`/`archived`
/// vocabulary (so the beta alias keeps working byte-for-byte); the public
/// surface derives `idle`/`running` from in-flight work on the session.
fn public_status(db_status: &str, in_flight: bool) -> &'static str {
    match db_status {
        "archived" => "archived",
        "failed" => "failed",
        "waiting" => "waiting",
        "idle" => "idle",
        "running" => "running",
        // Legacy `active` rows: running only while work is actually queued.
        _ => {
            if in_flight {
                "running"
            } else {
                "idle"
            }
        }
    }
}

fn public_budget(session: &beta::SessionRow) -> Value {
    let tokens = session
        .budget
        .get("tokens_used")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let estimated_cost_cents =
        crate::pricing::compute_cost_cents("llm_tokens", "tokens", tokens as f64);
    let estimated_cost_usd = estimated_cost_cents as f64 / 100.0;
    let max_cost_usd = session
        .metadata
        .get("max_cost_usd")
        .and_then(Value::as_f64);
    let over_budget = max_cost_usd
        .map(|max| estimated_cost_usd > max)
        .unwrap_or(false);
    let mut budget = session.budget.clone();
    if let Some(object) = budget.as_object_mut() {
        object.insert(
            "estimated_cost_cents".into(),
            json!(estimated_cost_cents),
        );
        object.insert("estimated_cost_usd".into(), json!(estimated_cost_usd));
        object.insert("charged".into(), json!(false));
        object.insert("max_cost_usd".into(), json!(max_cost_usd));
        object.insert("over_budget".into(), json!(over_budget));
    }
    budget
}

fn public_session(session: &beta::SessionRow, in_flight: bool) -> Value {
    let vault_ids = session
        .metadata
        .get("vault_ids")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let memory_store_ids = session
        .metadata
        .get("memory_store_ids")
        .cloned()
        .unwrap_or_else(|| json!([]));
    json!({
        "id": session.id,
        "agent_id": session.agent_id,
        "name": session.name,
        "status": public_status(&session.status, in_flight),
        "metadata": session.metadata,
        "budget": public_budget(session),
        "computer": {
            "kind": session.computer_kind.clone().unwrap_or_else(|| "none".to_string()),
            "id": session.computer_id,
        },
        "brain_id": session.brain_id,
        "vault_ids": vault_ids,
        "memory_store_ids": memory_store_ids,
        "bot_id": session.metadata.get("bot_id"),
        "parent_thread_id": session.parent_thread_id,
        "permission": session.metadata.get("permission"),
        "effective_permissions": session.metadata.get("effective_permissions"),
        "created_at": session.created_at,
        "updated_at": session.updated_at,
        "archived_at": session.archived_at,
    })
}

/// Collect tool names from an agent's `tools` JSON column (strings or
/// `{name: …}` objects). Shared shape with `agent_routes::collect_tool_names`.
fn collect_tool_names(value: Option<&Value>) -> Vec<String> {
    let mut names = Vec::new();
    let Some(items) = value.and_then(Value::as_array) else {
        return names;
    };
    for item in items {
        match item {
            Value::String(s) => names.push(s.clone()),
            Value::Object(map) => {
                if let Some(name) = map.get("name").and_then(Value::as_str) {
                    names.push(name.to_string());
                }
            }
            _ => {}
        }
    }
    names
}

/// Merge an agent's per-tool permission map with the session permission
/// mode into the effective permission map stored on session metadata:
///
/// * `always_allow` — every tool forced to `allow`.
/// * `always_ask` — every tool forced to `ask`.
/// * `auto` (and the no-mode default) — the per-tool map decides; a tool
///   with no entry (or `auto`) defaults to `allow` (documented default).
///
/// Returns `{}` for agent-less sessions. Values are normalized to
/// `allow`/`ask` so workers only ever see those two.
fn compute_effective_permissions(
    tools: Option<&str>,
    tool_permissions: Option<&str>,
    mode: Option<&str>,
) -> Value {
    let tools_value = tools.and_then(|raw| serde_json::from_str::<Value>(raw).ok());
    let perms_value = tool_permissions.and_then(|raw| serde_json::from_str::<Value>(raw).ok());
    let mut tool_names: Vec<String> = collect_tool_names(tools_value.as_ref());
    if let Some(map) = perms_value.as_ref().and_then(Value::as_object) {
        for key in map.keys() {
            if !tool_names.contains(key) {
                tool_names.push(key.clone());
            }
        }
    }
    let resolve = |tool: &str| -> &'static str {
        match mode {
            Some("always_allow") => "allow",
            Some("always_ask") => "ask",
            _ => match perms_value
                .as_ref()
                .and_then(|perms| perms.get(tool))
                .and_then(Value::as_str)
            {
                Some("always_ask") => "ask",
                _ => "allow",
            },
        }
    };
    let mut map = serde_json::Map::new();
    for tool in tool_names {
        map.insert(tool.clone(), json!(resolve(&tool)));
    }
    json!(map)
}

async fn bind_session_computer(
    state: Arc<AppState>,
    session_id: &str,
    computer_id: &str,
    kind: &str,
) -> Result<(), ApiError> {
    let db = state.db.clone();
    let session_id = session_id.to_string();
    let computer_id_owned = computer_id.to_string();
    let kind = kind.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "UPDATE beta_sessions SET computer_id = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
            params![computer_id_owned, session_id],
        )?;
        beta::insert_event(
            &conn,
            &session_id,
            "computer_ready",
            &json!({"kind": kind, "computer_id": computer_id_owned, "plane": "computer_cloud"}),
        )?;
        Ok::<(), rusqlite::Error>(())
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e| ApiError::DbError(e.to_string()))
}

fn parse_brain_id(value: &Option<Value>) -> Result<Option<String>, ApiError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(id)) if !id.trim().is_empty() => Ok(Some(id.clone())),
        Some(_) => Err(ApiError::BadRequest(
            "brain_id must be a string or null".into(),
        )),
    }
}

fn parse_vault_ids(value: &Option<Value>) -> Result<Vec<String>, ApiError> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => {
            let mut ids = Vec::with_capacity(items.len());
            for item in items {
                match item.as_str() {
                    Some(id) if !id.trim().is_empty() => ids.push(id.to_string()),
                    _ => {
                        return Err(ApiError::BadRequest(
                            "vault_ids must be an array of strings".into(),
                        ))
                    }
                }
            }
            Ok(ids)
        }
        Some(_) => Err(ApiError::BadRequest(
            "vault_ids must be an array of strings".into(),
        )),
    }
}

fn parse_memory_store_ids(value: &Option<Value>) -> Result<Vec<String>, ApiError> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => {
            let mut ids = Vec::with_capacity(items.len());
            for item in items {
                match item.as_str() {
                    Some(id) if !id.trim().is_empty() => ids.push(id.to_string()),
                    _ => {
                        return Err(ApiError::BadRequest(
                            "memory_store_ids must be an array of strings".into(),
                        ))
                    }
                }
            }
            Ok(ids)
        }
        Some(_) => Err(ApiError::BadRequest(
            "memory_store_ids must be an array of strings".into(),
        )),
    }
}

/// Translate one stored (legacy) event row into zero or more public events.
/// Stored `session_archived`, budget/context bookkeeping, and token deltas
/// are intentionally omitted from the public stream.
fn translate_event(
    event_id: &str,
    sequence: i64,
    session_id: &str,
    event_type: &str,
    data: Value,
    created_at: &str,
) -> Vec<Value> {
    let base = |ty: &str, data: Value, public_id: String| {
        json!({
            "id": public_id,
            "sequence": sequence,
            "type": ty,
            "session_id": session_id,
            "created_at": created_at,
            "data": data,
        })
    };
    match event_type {
        "session_created" => vec![base("session.created", data, event_id.to_string())],
        "computer_ready" => vec![base("computer.ready", data, event_id.to_string())],
        "computer_pending" => vec![base("computer.pending", data, event_id.to_string())],
        "computer_failed" => vec![base("computer.failed", data, event_id.to_string())],
        "run_requested" => vec![
            base(
                "turn.started",
                data.clone(),
                format!("{event_id}:turn.started"),
            ),
            base(
                "session.running",
                data,
                format!("{event_id}:session.running"),
            ),
        ],
        "user_interrupt" => vec![base("user.interrupt", data, event_id.to_string())],
        "tool_calls" => vec![base("agent.tool_use", data, event_id.to_string())],
        "turn_completed" => vec![base("turn.completed", data, event_id.to_string())],
        "turn_failed" => vec![base("turn.failed", data, event_id.to_string())],
        "session_idle" => vec![base("session.idle", data, event_id.to_string())],
        "session_failed" => vec![base("session.failed", data, event_id.to_string())],
        _ => vec![],
    }
}

fn turns_from_events(events: &[Value]) -> Vec<Value> {
    let mut open: Option<Value> = None;
    let mut turns = Vec::new();
    for event in events {
        let ty = event.get("type").and_then(Value::as_str).unwrap_or("");
        match ty {
            "turn.started" => {
                if let Some(previous) = open.take() {
                    turns.push(previous);
                }
                open = Some(json!({
                    "id": event.get("id"),
                    "status": "running",
                    "started_at": event.get("created_at"),
                    "completed_at": Value::Null,
                }));
            }
            "turn.completed" | "turn.failed" => {
                let status = if ty == "turn.completed" {
                    "completed"
                } else {
                    "failed"
                };
                if let Some(mut turn) = open.take() {
                    turn["status"] = json!(status);
                    turn["completed_at"] = event.get("created_at").cloned().unwrap_or(Value::Null);
                    turns.push(turn);
                }
            }
            _ => {}
        }
    }
    if let Some(turn) = open {
        turns.push(turn);
    }
    turns
}

fn parse_public_events(
    session_id: &str,
    rows: Vec<(i64, String, String, String, String)>,
) -> Vec<Value> {
    // (sequence, event_id, event_type, data, created_at), ascending.
    rows.into_iter()
        .flat_map(|(sequence, event_id, event_type, data, created_at)| {
            let data = serde_json::from_str(&data).unwrap_or_else(|_| json!({}));
            translate_event(&event_id, sequence, session_id, &event_type, data, &created_at)
        })
        .collect()
}

/// Session ids with queued/leased/running work tasks, scoped to one user.
async fn sessions_with_in_flight_work(state: Arc<AppState>, user_id: String) -> HashSet<String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT DISTINCT session_id FROM beta_work_tasks
             WHERE user_id = ?1 AND session_id IS NOT NULL
             AND status IN ('queued', 'leased', 'running')",
        )?;
        let ids = stmt
            .query_map(params![user_id], |row| row.get::<_, String>(0))?
            .collect::<Result<HashSet<String>, _>>()?;
        Ok::<HashSet<String>, rusqlite::Error>(ids)
    })
    .await
    .ok()
    .and_then(Result::ok)
    .unwrap_or_default()
}

// ─── Create / list / get / archive ───────────────────────────────────────────

async fn list_cloud_sessions(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result<Json<Value>, ApiError> {
    let in_flight = sessions_with_in_flight_work(state.clone(), user.user_id.clone()).await;
    let db = state.db.clone();
    let user_id = user.user_id;
    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(&format!(
            "{} WHERE user_id = ?1 ORDER BY created_at DESC",
            beta::SESSION_SELECT
        ))?;
        let rows = stmt
            .query_map(params![user_id], beta::read_session)?
            .collect::<Result<Vec<beta::SessionRow>, _>>()?;
        Ok::<Vec<beta::SessionRow>, rusqlite::Error>(rows)
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))??;
    let sessions = rows
        .iter()
        .map(|session| public_session(session, in_flight.contains(&session.id)))
        .collect::<Vec<Value>>();
    Ok(Json(json!({ "sessions": sessions })))
}

/// Extract the user-message text from the `input` field of a create body.
fn parse_input(input: &Option<Value>) -> Result<Option<String>, ApiError> {
    match input {
        None => Ok(None),
        Some(Value::String(text)) => {
            if text.trim().is_empty() {
                return Err(ApiError::BadRequest("input must not be empty".into()));
            }
            Ok(Some(text.clone()))
        }
        Some(Value::Object(map)) => {
            let ty = map.get("type").and_then(Value::as_str).unwrap_or("");
            if ty != "user.message" {
                return Err(ApiError::BadRequest(format!(
                    "unsupported input event type: {ty}"
                )));
            }
            match map.get("content").and_then(Value::as_str) {
                Some(text) if !text.trim().is_empty() => Ok(Some(text.to_string())),
                _ => Err(ApiError::BadRequest(
                    "input.content must be a non-empty string".into(),
                )),
            }
        }
        Some(_) => Err(ApiError::BadRequest(
            "input must be a string or a user.message object".into(),
        )),
    }
}

async fn create_cloud_session(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<CreateCloudSessionBody>,
) -> Result<Response, ApiError> {
    if !body.metadata.is_object() {
        return Err(ApiError::BadRequest("metadata must be an object".into()));
    }
    let computer_kind = body
        .computer
        .as_ref()
        .map(|computer| computer.kind.clone())
        .unwrap_or_else(default_computer_kind);
    let provision_kind = computer_kind.clone();
    match computer_kind.as_str() {
        "none" | "local" | "sandbox" | "desktop" | "fabric" => {}
        other => {
            return Err(ApiError::BadRequest(format!(
                "unsupported computer.kind: {other}"
            )))
        }
    }
    let input_text = parse_input(&body.input)?;
    let brain_id = parse_brain_id(&body.brain_id)?;
    let vault_ids = parse_vault_ids(&body.vault_ids)?;
    let memory_store_ids = parse_memory_store_ids(&body.memory_store_ids)?;
    let permission = match body.permission.as_deref() {
        None => None,
        Some("always_allow" | "always_ask" | "auto") => body.permission.clone(),
        Some(other) => {
            return Err(ApiError::BadRequest(format!(
                "unsupported permission: {other} (always_allow | always_ask | auto)"
            )))
        }
    };
    let parent_thread_id = body.parent_thread_id.clone();
    let bot_id = body
        .bot_id
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let db = state.db.clone();
    let organization_id = user.organization_id.clone();
    let user_id = user.user_id.clone();
    let session_id = uuid::Uuid::new_v4().to_string();
    let result_session_id = session_id.clone();
    let webhook_session_id = session_id.clone();
    let run_input = input_text.is_some();
    let permission_for_db = permission.clone();
    let metadata = {
        let mut metadata = body.metadata.clone();
        let object = metadata
            .as_object_mut()
            .expect("metadata validated as object");
        if !vault_ids.is_empty() {
            object.insert("vault_ids".to_string(), json!(vault_ids.clone()));
        }
        if !memory_store_ids.is_empty() {
            object.insert("memory_store_ids".to_string(), json!(memory_store_ids.clone()));
        }
        if let Some(brain_id) = &brain_id {
            object.insert("brain_id".to_string(), json!(brain_id));
        }
        if let Some(permission) = &permission {
            object.insert("permission".to_string(), json!(permission));
        }
        if let Some(bot_id) = &bot_id {
            object.insert("bot_id".to_string(), json!(bot_id));
        }
        if let Some(max_cost_usd) = body.budget.as_ref().and_then(|b| b.max_cost_usd) {
            object.insert("max_cost_usd".to_string(), json!(max_cost_usd));
        }
        metadata
    };
    let budget = body.budget;
    let agent_ref = match (body.agent, bot_id.clone()) {
        (Some(agent), _) => Some(agent),
        (None, Some(bot_id)) => Some(AgentRef::Id(bot_id)),
        (None, None) => None,
    };

    let session = tokio::task::spawn_blocking(move || {
        let mut conn = db.connect()?;
        let tx = conn.transaction()?;
        let mut metadata = metadata;

        if let Some(parent_id) = &parent_thread_id {
            let exists = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM beta_sessions WHERE id = ?1 AND user_id = ?2)",
                params![parent_id, user_id],
                |row| row.get::<_, bool>(0),
            )?;
            if !exists {
                return Err(rusqlite::Error::InvalidParameterName(
                    "parent_thread_id not found".to_string(),
                ));
            }
        }
        if let Some(brain_id) = &brain_id {
            let exists = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM brains WHERE id = ?1 AND user_id = ?2)",
                params![brain_id, user_id],
                |row| row.get::<_, bool>(0),
            )?;
            if !exists {
                return Err(rusqlite::Error::InvalidParameterName(
                    "brain not found".to_string(),
                ));
            }
        }
        for vault_id in &vault_ids {
            let exists = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM allternit_vaults WHERE id = ?1 AND created_by = ?2)",
                params![vault_id, user_id],
                |row| row.get::<_, bool>(0),
            )?;
            if !exists {
                return Err(rusqlite::Error::InvalidParameterName(
                    "vault not found".to_string(),
                ));
            }
        }
        for store_id in &memory_store_ids {
            let exists = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM beta_memory_stores WHERE id = ?1 AND user_id = ?2)",
                params![store_id, user_id],
                |row| row.get::<_, bool>(0),
            )?;
            if !exists {
                return Err(rusqlite::Error::InvalidParameterName(
                    "memory store not found".to_string(),
                ));
            }
        }

        // Resolve the agent reference to an agents-row id.
        let agent_id = match &agent_ref {
            None => None,
            Some(AgentRef::Id(id)) => {
                let exists = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM agents WHERE id = ?1 AND user_id = ?2)",
                    params![id, user_id],
                    |row| row.get::<_, bool>(0),
                )?;
                if !exists {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
                Some(id.clone())
            }
            Some(AgentRef::Ref { id, version }) => {
                let row_version = tx
                    .query_row(
                        "SELECT version FROM agents WHERE id = ?1 AND user_id = ?2",
                        params![id, user_id],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()?;
                let row_version = match row_version {
                    Some(version) => version,
                    None => return Err(rusqlite::Error::QueryReturnedNoRows),
                };
                if let Some(expected) = version {
                    if *expected != row_version {
                        return Err(rusqlite::Error::IntegralValueOutOfRange(0, *expected));
                    }
                }
                Some(id.clone())
            }
            Some(AgentRef::Inline(inline)) => {
                if inline.model.trim().is_empty() {
                    return Err(rusqlite::Error::InvalidParameterName(
                        "agent.model must be a non-empty string".to_string(),
                    ));
                }
                let agent_id = uuid::Uuid::new_v4().to_string();
                tx.execute(
                    "INSERT INTO agents (id, user_id, name, model, provider, system_prompt,
                                         tools, status, type)
                     VALUES (?1, ?2, ?3, ?4, 'allternit', ?5, ?6, 'idle', 'worker')",
                    params![
                        agent_id,
                        user_id,
                        inline.name.clone().unwrap_or_else(|| "cloud-agent".to_string()),
                        inline.model.as_str(),
                        inline.instructions.clone(),
                        inline.tools.as_ref().map(Value::to_string),
                    ],
                )?;
                Some(agent_id)
            }
        };

        // Effective permissions: merge the agent's per-tool permission map
        // with the session permission mode and store the result on metadata
        // so both the session JSON and the worker lease payload can read it.
        let effective_permissions = match agent_id.as_deref() {
            Some(aid) => {
                let agent_columns: Option<(Option<String>, Option<String>)> = tx
                    .query_row(
                        "SELECT tools, tool_permissions FROM agents WHERE id = ?1 AND user_id = ?2",
                        params![aid, user_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?;
                match agent_columns {
                    Some((tools, tool_permissions)) => compute_effective_permissions(
                        tools.as_deref(),
                        tool_permissions.as_deref(),
                        permission_for_db.as_deref(),
                    ),
                    None => json!({}),
                }
            }
            None => json!({}),
        };
        metadata
            .as_object_mut()
            .expect("metadata validated as object")
            .insert("effective_permissions".to_string(), effective_permissions);

        tx.execute(
            "INSERT INTO beta_sessions
             (id, user_id, agent_id, metadata, max_tokens, max_turns, max_tool_calls,
              status, computer_kind, brain_id, parent_thread_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'active', ?8, ?9, ?10)",
            params![
                session_id,
                user_id,
                agent_id,
                metadata.to_string(),
                budget.as_ref().and_then(|b| b.max_tokens),
                budget.as_ref().and_then(|b| b.max_turns),
                budget.as_ref().and_then(|b| b.max_tool_calls),
                computer_kind,
                brain_id,
                parent_thread_id,
            ],
        )?;
        beta::insert_event(&tx, &session_id, "session_created", &json!({}))?;
        match computer_kind.as_str() {
            // `none`: nothing to provision; the computer is immediately ready.
            "none" => {
                beta::insert_event(
                    &tx,
                    &session_id,
                    "computer_ready",
                    &json!({"kind": "none"}),
                )?;
            }
            // `local`: descriptor only this release — no worker is awaited.
            "local" => {
                beta::insert_event(
                    &tx,
                    &session_id,
                    "computer_pending",
                    &json!({"kind": "local"}),
                )?;
            }
            "sandbox" | "desktop" | "fabric" => {
                beta::insert_event(
                    &tx,
                    &session_id,
                    "computer_pending",
                    &json!({"kind": computer_kind, "plane": "computer_cloud"}),
                )?;
            }
            _ => {}
        }
        if let Some(text) = input_text {
            let task_id = uuid::Uuid::new_v4().to_string();
            let payload = json!({
                "messages": [{"role": "user", "content": text}],
                "tools": Value::Null,
            });
            tx.execute(
                "INSERT INTO beta_work_tasks (id, user_id, session_id, payload)
                 VALUES (?1, ?2, ?3, ?4)",
                params![task_id, user_id, session_id, payload.to_string()],
            )?;
            beta::insert_event(
                &tx,
                &session_id,
                "run_requested",
                &json!({"task_id": task_id, "message_count": 1}),
            )?;
        }
        let session = tx.query_row(
            &format!("{} WHERE id = ?1", beta::SESSION_SELECT),
            params![session_id],
            beta::read_session,
        )?;
        tx.commit()?;
        Ok::<beta::SessionRow, rusqlite::Error>(session)
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            ApiError::BadRequest("agent not found".into())
        }
        rusqlite::Error::IntegralValueOutOfRange(_, expected) => ApiError::BadRequest(format!(
            "agent version mismatch: expected version {expected}"
        )),
        rusqlite::Error::InvalidParameterName(msg) => ApiError::BadRequest(msg),
        other => ApiError::DbError(other.to_string()),
    })?;

    let computer_bind_id = body.computer.as_ref().and_then(|c| c.id.clone());
    let computer_os = body.computer.as_ref().and_then(|c| c.os.clone());
    let computer_template = body
        .computer
        .as_ref()
        .and_then(|c| c.template_id.clone());
    let computer_provider = body.computer.as_ref().and_then(|c| c.provider.clone());
    let mut session = session;

    if let Some(existing_id) = computer_bind_id {
        match crate::computer_routes::fetch_computer(&state, &user, &existing_id).await {
            Ok(Some(computer)) => {
                bind_session_computer(
                    state.clone(),
                    &result_session_id,
                    &computer.id,
                    &provision_kind,
                )
                .await?;
                session.computer_id = Some(computer.id);
            }
            Ok(None) => {
                return Err(ApiError::BadRequest("computer not found".into()));
            }
            Err(_) => {
                return Err(ApiError::BadRequest("computer not found".into()));
            }
        }
    } else if matches!(provision_kind.as_str(), "sandbox" | "desktop" | "fabric")
        && permission.as_deref() != Some("always_ask")
    {
        let persistence = if provision_kind == "sandbox" {
            crate::computer_routes::Persistence::Ephemeral
        } else {
            crate::computer_routes::Persistence::Session
        };
        match crate::computer_routes::provision_cloud_desktop_for_session(
            state.clone(),
            user.clone(),
            result_session_id.clone(),
            persistence,
            computer_os,
            computer_template,
            computer_provider,
        )
        .await
        {
            Ok(computer_id) => {
                bind_session_computer(
                    state.clone(),
                    &result_session_id,
                    &computer_id,
                    &provision_kind,
                )
                .await?;
                session.computer_id = Some(computer_id);
            }
            Err((status, message)) => {
                let db = state.db.clone();
                let sid = result_session_id.clone();
                let kind = provision_kind.clone();
                let fail_msg = message.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    let conn = db.connect()?;
                    beta::insert_event(
                        &conn,
                        &sid,
                        "computer_failed",
                        &json!({"kind": kind, "error": fail_msg, "plane": "computer_cloud"}),
                    )
                })
                .await;
                return Ok((
                    status,
                    Json(json!({
                        "error": message,
                        "code": "computer_unavailable"
                    })),
                )
                    .into_response());
            }
        }
    }

    let public = public_session(&session, run_input);
    webhook_subscription_routes::deliver_session_event(
        state.clone(),
        organization_id.as_deref(),
        &webhook_session_id,
        &json!({"type": "session.created", "session_id": result_session_id}),
    )
    .await;
    webhook_subscription_routes::deliver_registered_event(
        state.clone(),
        organization_id.as_deref(),
        webhook_subscription_routes::events::SESSION_CREATED,
        json!({"session_id": result_session_id}),
    )
    .await;

    if body.stream {
        // SSE of this session's events, replayed from the beginning so the
        // first frames include session.created.
        let stream = public_event_stream(state, result_session_id.clone(), 0);
        return Ok(stream.into_response());
    }
    Ok((axum::http::StatusCode::CREATED, Json(json!({ "session": public }))).into_response())
}

async fn get_cloud_session(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let session = beta::load_session(state.clone(), user.user_id.clone(), id.clone()).await?;
    let in_flight = sessions_with_in_flight_work(state, user.user_id).await;
    Ok(Json(json!({ "session": public_session(&session, in_flight.contains(&id)) })))
}

async fn archive_cloud_session(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let db = state.db.clone();
    let webhook_org = user.organization_id.clone();
    let webhook_session_id = id.clone();
    tokio::task::spawn_blocking(move || {
        let mut conn = db.connect()?;
        let tx = conn.transaction()?;
        let affected = tx.execute(
            "UPDATE beta_sessions SET status = 'archived', archived_at = CURRENT_TIMESTAMP,
             updated_at = CURRENT_TIMESTAMP WHERE id = ?1 AND user_id = ?2",
            params![id, user.user_id],
        )?;
        if affected == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        beta::insert_event(&tx, &id, "session_archived", &json!({}))?;
        tx.commit()
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e| {
        if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
            ApiError::NotFound("session not found".into())
        } else {
            ApiError::DbError(e.to_string())
        }
    })?;
    webhook_subscription_routes::deliver_registered_event(
        state.clone(),
        webhook_org.as_deref(),
        webhook_subscription_routes::events::SESSION_ARCHIVED,
        json!({"session_id": webhook_session_id}),
    )
    .await;
    Ok(Json(json!({ "archived": true })))
}

// ─── Events ──────────────────────────────────────────────────────────────────

async fn list_cloud_events(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    beta::load_session(state.clone(), user.user_id.clone(), id.clone()).await?;
    let db = state.db.clone();
    let user_id = user.user_id;
    let session_id = id.clone();
    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT e.sequence, e.id, e.event_type, e.data, e.created_at
             FROM beta_session_events e
             JOIN beta_sessions s ON s.id = e.session_id
             WHERE e.session_id = ?1 AND s.user_id = ?2
             ORDER BY e.sequence ASC",
        )?;
        let rows = stmt
            .query_map(params![session_id, user_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok::<Vec<(i64, String, String, String, String)>, rusqlite::Error>(rows)
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e| ApiError::DbError(e.to_string()))?;
    Ok(Json(json!({ "events": parse_public_events(&id, rows) })))
}

/// `POST /sessions/:id/events` — accept `user.message`, `user.interrupt`,
/// and `user.tool_result`. `user.message` enqueues a run on the node's work
/// queue exactly like `POST /beta/sessions/:id/run`; `user.interrupt`
/// cancels queued/leased work and returns the session to `idle`.
async fn send_cloud_events(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<SendEventsBody>,
) -> Result<Json<Value>, ApiError> {
    if body.events.is_empty() {
        return Err(ApiError::BadRequest("events must be a non-empty array".into()));
    }
    for event in &body.events {
        if !matches!(
            event.kind.as_str(),
            "user.message" | "user.interrupt" | "user.tool_result"
        ) {
            return Err(ApiError::BadRequest(format!(
                "unsupported event type: {}",
                event.kind
            )));
        }
        if event.kind == "user.message" {
            match event.content.as_ref().and_then(Value::as_str) {
                Some(text) if !text.trim().is_empty() => {}
                _ => {
                    return Err(ApiError::BadRequest(
                        "user.message content must be a non-empty string".into(),
                    ))
                }
            }
        }
    }
    let session = beta::load_session(state.clone(), user.user_id.clone(), id.clone()).await?;
    if session.status == "archived" {
        return Err(ApiError::BadRequest(
            "archived sessions cannot accept events".into(),
        ));
    }

    let db = state.db.clone();
    let user_id = user.user_id;
    let organization_id = user.organization_id.clone();
    let mut delivered: Vec<Value> = Vec::new();
    for event in body.events {
        let db = db.clone();
        let session_id = id.clone();
        let user_id = user_id.clone();
        let accepted = tokio::task::spawn_blocking(move || {
            let mut conn = db.connect()?;
            let tx = conn.transaction()?;
            match event.kind.as_str() {
                "user.message" => {
                    let text = event
                        .content
                        .as_ref()
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let task_id = uuid::Uuid::new_v4().to_string();
                    let payload = json!({
                        "messages": [{"role": "user", "content": text}],
                        "tools": Value::Null,
                    });
                    tx.execute(
                        "INSERT INTO beta_work_tasks (id, user_id, session_id, payload)
                         VALUES (?1, ?2, ?3, ?4)",
                        params![task_id, user_id, session_id, payload.to_string()],
                    )?;
                    let event = beta::insert_event(
                        &tx,
                        &session_id,
                        "run_requested",
                        &json!({"task_id": task_id, "message_count": 1}),
                    )?;
                    tx.commit()?;
                    Ok::<Value, rusqlite::Error>(event)
                }
                "user.interrupt" => {
                    tx.execute(
                        "UPDATE beta_work_tasks SET status = 'cancelled', updated_at = CURRENT_TIMESTAMP
                         WHERE session_id = ?1 AND user_id = ?2
                         AND status IN ('queued', 'leased', 'running')",
                        params![session_id, user_id],
                    )?;
                    let event = beta::insert_event(
                        &tx,
                        &session_id,
                        "user_interrupt",
                        &event.data.clone().unwrap_or_else(|| json!({})),
                    )?;
                    tx.commit()?;
                    Ok::<Value, rusqlite::Error>(event)
                }
                // user.tool_result: accepted for forward compatibility; there is
                // no in-line consumer in Phase 1, so it is recorded for the
                // record and skipped by the public event projection.
                _ => {
                    let event = beta::insert_event(
                        &tx,
                        &session_id,
                        "user_tool_result",
                        &event
                            .data
                            .clone()
                            .or_else(|| event.content.clone())
                            .unwrap_or_else(|| json!({})),
                    )?;
                    tx.commit()?;
                    Ok::<Value, rusqlite::Error>(event)
                }
            }
        })
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .map_err(|e| ApiError::DbError(e.to_string()))?;
        delivered.push(accepted);
    }
    for event in &delivered {
        webhook_subscription_routes::deliver_session_event(
            state.clone(),
            organization_id.as_deref(),
            &id,
            event,
        )
        .await;
    }
    Ok(Json(json!({ "accepted": true })))
}

/// SSE stream of public (Allternit-named) events. Stored rows are translated
/// on read, so legacy history replayed on connect already uses public names.
fn public_event_stream(
    state: Arc<AppState>,
    id: String,
    after: i64,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let db = state.db.clone();
    let session_id = id.clone();
    let stream = futures::stream::unfold(
        (db, id, after, VecDeque::new()),
        move |(db, id, mut cursor, mut pending)| {
            let session_id = session_id.clone();
            async move {
                loop {
                    if let Some(event) = pending.pop_front() {
                        return Some((Ok(event), (db, id, cursor, pending)));
                    }
                    match beta::next_event(db.clone(), id.clone(), cursor).await {
                        Some((sequence, event_id, event_type, data, created_at)) => {
                            let data = serde_json::from_str(&data).unwrap_or_else(|_| json!({}));
                            for translated in
                                translate_event(&event_id, sequence, &session_id, &event_type, data, &created_at)
                            {
                                let ty = translated
                                    .get("type")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string();
                                pending.push_back(
                                    Event::default()
                                        .id(sequence.to_string())
                                        .event(ty)
                                        .data(translated.to_string()),
                                );
                            }
                            cursor = sequence;
                        }
                        None => tokio::time::sleep(Duration::from_millis(250)).await,
                    }
                }
            }
        },
    );
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn stream_cloud_events(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Query(query): Query<EventQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    beta::load_session(state.clone(), user.user_id, id.clone()).await?;
    Ok(public_event_stream(state, id, query.after.unwrap_or(0)))
}

async fn list_cloud_turns(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    beta::load_session(state.clone(), user.user_id.clone(), id.clone()).await?;
    let db = state.db.clone();
    let user_id = user.user_id;
    let session_id = id.clone();
    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT e.sequence, e.id, e.event_type, e.data, e.created_at
             FROM beta_session_events e
             JOIN beta_sessions s ON s.id = e.session_id
             WHERE e.session_id = ?1 AND s.user_id = ?2
             ORDER BY e.sequence ASC",
        )?;
        let rows = stmt
            .query_map(params![session_id, user_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok::<Vec<(i64, String, String, String, String)>, rusqlite::Error>(rows)
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e| ApiError::DbError(e.to_string()))?;
    let events = parse_public_events(&id, rows);
    Ok(Json(json!({ "turns": turns_from_events(&events) })))
}

async fn list_cloud_threads(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    beta::load_session(state.clone(), user.user_id.clone(), id.clone()).await?;
    let in_flight = sessions_with_in_flight_work(state.clone(), user.user_id.clone()).await;
    let db = state.db.clone();
    let user_id = user.user_id;
    let parent_id = id.clone();
    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(&format!(
            "{} WHERE user_id = ?1 AND parent_thread_id = ?2 ORDER BY created_at DESC",
            beta::SESSION_SELECT
        ))?;
        let rows = stmt
            .query_map(params![user_id, parent_id], beta::read_session)?
            .collect::<Result<Vec<beta::SessionRow>, _>>()?;
        Ok::<Vec<beta::SessionRow>, rusqlite::Error>(rows)
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))??;
    let threads = rows
        .iter()
        .map(|session| public_session(session, in_flight.contains(&session.id)))
        .collect::<Vec<Value>>();
    Ok(Json(json!({ "threads": threads })))
}

async fn list_cloud_outputs(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    beta::load_session(state.clone(), user.user_id.clone(), id.clone()).await?;
    let db = state.db.clone();
    let files = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, session_id, org_id, filename, mime_type, storage_path, size_bytes, created_at
             FROM session_files WHERE session_id = ?1 ORDER BY created_at DESC, id",
        )?;
        let rows = stmt
            .query_map(params![id], beta::read_session_file)?
            .collect::<Result<Vec<beta::SessionFileRow>, _>>()?;
        Ok::<Vec<beta::SessionFileRow>, rusqlite::Error>(rows)
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))??;
    let outputs: Vec<Value> = files
        .into_iter()
        .map(|file| {
            json!({
                "id": file.id,
                "filename": file.filename,
                "mime_type": file.mime_type,
                "size_bytes": file.size_bytes,
                "created_at": file.created_at,
            })
        })
        .collect();
    Ok(Json(json!({ "outputs": outputs })))
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::beta_session_routes::tests as beta_test;

    fn request(path: &str, method: &str, body: &Value, user: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .extension(beta_test::test_user(user))
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn post_json(
        router: &Router,
        path: &str,
        body: &Value,
        user: &str,
    ) -> (StatusCode, Value) {
        let response = router
            .clone()
            .oneshot(request(path, "POST", body, user))
            .await
            .unwrap();
        let status = response.status();
        let payload: Value = serde_json::from_slice(
            &response.into_body().collect().await.unwrap().to_bytes(),
        )
        .unwrap_or_else(|_| json!({}));
        (status, payload)
    }

    async fn get_json(router: &Router, path: &str, user: &str) -> (StatusCode, Value) {
        let response = router
            .clone()
            .oneshot(request(path, "GET", &json!({}), user))
            .await
            .unwrap();
        let status = response.status();
        let payload: Value = serde_json::from_slice(
            &response.into_body().collect().await.unwrap().to_bytes(),
        )
        .unwrap_or_else(|_| json!({}));
        (status, payload)
    }

    async fn send_json(
        router: &Router,
        method: &str,
        path: &str,
        body: &Value,
        user: &str,
    ) -> (StatusCode, Value) {
        let response = router
            .clone()
            .oneshot(request(path, method, body, user))
            .await
            .unwrap();
        let status = response.status();
        let payload: Value = serde_json::from_slice(
            &response.into_body().collect().await.unwrap().to_bytes(),
        )
        .unwrap_or_else(|_| json!({}));
        (status, payload)
    }

    /// Insert an agents row directly (tools + per-tool permission map) so
    /// session-create tests control the agent definition exactly.
    fn insert_agent(
        state: &Arc<AppState>,
        user: &str,
        tools: Value,
        tool_permissions: Option<Value>,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let conn = state.db.connect().unwrap();
        conn.execute(
            "INSERT INTO agents (id, user_id, name, model, provider, tools, tool_permissions)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                user,
                "perm-agent",
                "kimi-k2",
                "allternit",
                tools.to_string(),
                tool_permissions.map(|p| p.to_string())
            ],
        )
        .unwrap();
        id
    }

    #[tokio::test]
    async fn create_with_inline_agent_none_computer_and_input() {
        let temp = beta_test::temp_dir("cloud-create");
        let state = beta_test::test_app_state(&temp).await;
        let router = cloud_agents_router().with_state(state.clone());

        let body = json!({
            "agent": {"model": "kimi-k2", "instructions": "Be terse.", "name": "helper"},
            "computer": {"kind": "none"},
            "input": "hello agent",
            "budget": {"max_cost_usd": 5.0},
            "metadata": {"origin": "test"}
        });
        let (status, payload) = post_json(&router, "/sessions", &body, "user-a").await;
        assert_eq!(status, StatusCode::CREATED);
        let session = &payload["session"];
        assert_eq!(session["status"], "running");
        assert_eq!(session["computer"]["kind"], "none");
        assert_eq!(session["budget"]["charged"], false);
        assert_eq!(session["budget"]["estimated_cost_cents"], 0);
        assert_eq!(session["budget"]["over_budget"], false);
        assert_eq!(session["budget"]["max_cost_usd"], 5.0);
        assert!(session["agent_id"].is_string());

        // The inline agent became a real agents row; instructions → system_prompt.
        let conn = state.db.connect().unwrap();
        let (name, prompt): (String, Option<String>) = conn
            .query_row(
                "SELECT name, system_prompt FROM agents WHERE id = ?1",
                params![session["agent_id"].as_str().unwrap()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(name, "helper");
        assert_eq!(prompt.as_deref(), Some("Be terse."));

        // Public event list uses Allternit type names.
        let session_id = session["id"].as_str().unwrap();
        let (status, events_payload) =
            get_json(&router, &format!("/sessions/{session_id}/events"), "user-a").await;
        assert_eq!(status, StatusCode::OK);
        let types: Vec<&str> = events_payload["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["type"].as_str().unwrap())
            .collect();
        assert_eq!(types, vec!["session.created", "computer.ready", "turn.started", "session.running"]);
    }

    #[tokio::test]
    async fn sandbox_computer_without_driver_is_503() {
        let temp = beta_test::temp_dir("cloud-sandbox");
        let state = beta_test::test_app_state(&temp).await;
        let router = cloud_agents_router().with_state(state);

        let body = json!({
            "agent": {"model": "kimi-k2", "instructions": "hi"},
            "computer": {"kind": "sandbox"}
        });
        let (status, payload) = post_json(&router, "/sessions", &body, "user-a").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(payload["code"], "computer_unavailable");
        assert!(
            payload["error"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .contains("driver")
                || payload["error"].as_str().unwrap_or("").contains("Computer Cloud")
                || payload["error"].as_str().unwrap_or("").contains("desktop"),
            "{payload}"
        );
    }

    #[tokio::test]
    async fn local_computer_emits_pending() {
        let temp = beta_test::temp_dir("cloud-local");
        let state = beta_test::test_app_state(&temp).await;
        let router = cloud_agents_router().with_state(state);

        let body = json!({
            "agent": {"model": "kimi-k2", "instructions": "hi"},
            "computer": {"kind": "local"}
        });
        let (status, payload) = post_json(&router, "/sessions", &body, "user-a").await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(payload["session"]["computer"]["kind"], "local");
        assert_eq!(payload["session"]["status"], "idle");

        let id = payload["session"]["id"].as_str().unwrap();
        let (status, events_payload) =
            get_json(&router, &format!("/sessions/{id}/events"), "user-a").await;
        assert_eq!(status, StatusCode::OK);
        let types: Vec<&str> = events_payload["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["type"].as_str().unwrap())
            .collect();
        assert_eq!(types, vec!["session.created", "computer.pending"]);
    }

    #[tokio::test]
    async fn archive_blocks_send() {
        let temp = beta_test::temp_dir("cloud-archive");
        let state = beta_test::test_app_state(&temp).await;
        let router = cloud_agents_router().with_state(state);

        let create = json!({
            "agent": {"model": "kimi-k2", "instructions": "hi"},
            "computer": {"kind": "none"}
        });
        let (status, payload) = post_json(&router, "/sessions", &create, "user-a").await;
        assert_eq!(status, StatusCode::CREATED);
        let id = payload["session"]["id"].as_str().unwrap().to_string();

        let (status, _) = post_json(&router, &format!("/sessions/{id}/archive"), &json!({}), "user-a").await;
        assert_eq!(status, StatusCode::OK);

        let (status, payload) = get_json(&router, &format!("/sessions/{id}"), "user-a").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(payload["session"]["status"], "archived");

        let send = json!({"events": [{"type": "user.message", "content": "still there?"}]});
        let (status, _) = post_json(&router, &format!("/sessions/{id}/events"), &send, "user-a").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn interrupt_returns_session_to_idle() {
        let temp = beta_test::temp_dir("cloud-interrupt");
        let state = beta_test::test_app_state(&temp).await;
        let router = cloud_agents_router().with_state(state);

        let create = json!({
            "agent": {"model": "kimi-k2", "instructions": "hi"},
            "computer": {"kind": "none"},
            "input": "do a thing"
        });
        let (status, payload) = post_json(&router, "/sessions", &create, "user-a").await;
        assert_eq!(status, StatusCode::CREATED);
        let id = payload["session"]["id"].as_str().unwrap().to_string();
        assert_eq!(payload["session"]["status"], "running");

        let send = json!({"events": [{"type": "user.interrupt"}]});
        let (status, _) = post_json(&router, &format!("/sessions/{id}/events"), &send, "user-a").await;
        assert_eq!(status, StatusCode::OK);

        let (status, payload) = get_json(&router, &format!("/sessions/{id}"), "user-a").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(payload["session"]["status"], "idle");

        let (status, events_payload) =
            get_json(&router, &format!("/sessions/{id}/events"), "user-a").await;
        assert_eq!(status, StatusCode::OK);
        let types: Vec<&str> = events_payload["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["type"].as_str().unwrap())
            .collect();
        assert!(types.contains(&"user.interrupt"));
    }

    #[tokio::test]
    async fn legacy_event_types_translate_to_allternit_names() {
        let temp = beta_test::temp_dir("cloud-translate");
        let state = beta_test::test_app_state(&temp).await;
        let router = cloud_agents_router().with_state(state.clone());

        let create = json!({
            "agent": {"model": "kimi-k2", "instructions": "hi"},
            "computer": {"kind": "none"}
        });
        let (status, payload) = post_json(&router, "/sessions", &create, "user-a").await;
        assert_eq!(status, StatusCode::CREATED);
        let id = payload["session"]["id"].as_str().unwrap().to_string();

        // Simulate history written by the legacy beta surface.
        let conn = state.db.connect().unwrap();
        beta::insert_event(&conn, &id, "run_requested", &json!({"task_id": "t1", "message_count": 1})).unwrap();
        beta::insert_event(&conn, &id, "user_interrupt", &json!({})).unwrap();
        beta::insert_event(&conn, &id, "budget_updated", &json!({"max_tokens": 5})).unwrap();
        beta::insert_event(&conn, &id, "session_archived", &json!({})).unwrap();
        beta::insert_event(&conn, &id, "content_block_delta", &json!({"delta": "x"})).unwrap();
        beta::insert_event(&conn, &id, "tool_calls", &json!([{"name": "bash"}])).unwrap();
        drop(conn);

        let (status, events_payload) =
            get_json(&router, &format!("/sessions/{id}/events"), "user-a").await;
        assert_eq!(status, StatusCode::OK);
        let types: Vec<&str> = events_payload["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["type"].as_str().unwrap())
            .collect();
        assert_eq!(
            types,
            vec![
                "session.created",
                "computer.ready",
                "turn.started",
                "session.running",
                "user.interrupt",
                "agent.tool_use",
            ]
        );
        // Stored snake_case names must not leak. `agent.tool_use` is a public
        // Allternit type (underscore in the action, not a legacy separator).
        assert!(!types.iter().any(|ty| {
            matches!(
                *ty,
                "session_created"
                    | "run_requested"
                    | "user_interrupt"
                    | "budget_updated"
                    | "session_archived"
                    | "content_block_delta"
                    | "tool_calls"
                    | "computer_ready"
                    | "computer_pending"
            )
        }));
    }

    #[tokio::test]
    async fn beta_alias_still_works_on_same_table() {
        let temp = beta_test::temp_dir("cloud-alias");
        let state = beta_test::test_app_state(&temp).await;
        let router = beta::beta_session_router()
            .merge(cloud_agents_router())
            .with_state(state);

        // Legacy create is still 201.
        let (status, payload) = post_json(
            &router,
            "/beta/sessions",
            &json!({"name": "legacy", "metadata": {}}),
            "user-a",
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let id = payload["id"].as_str().unwrap().to_string();

        // Legacy retrieve is still 200.
        let (status, _) = get_json(&router, &format!("/beta/sessions/{id}"), "user-a").await;
        assert_eq!(status, StatusCode::OK);

        // The same row is visible through the public facade, status mapped.
        let (status, payload) = get_json(&router, &format!("/sessions/{id}"), "user-a").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(payload["session"]["status"], "idle");
        assert_eq!(payload["session"]["name"], "legacy");
    }

    #[tokio::test]
    async fn ack_work_task_emits_turn_completed_and_session_goes_idle() {
        let temp = beta_test::temp_dir("cloud-ack");
        let state = beta_test::test_app_state(&temp).await;
        let router = cloud_agents_router()
            .merge(crate::beta_work_routes::beta_work_router())
            .with_state(state.clone());

        let (status, payload) = post_json(
            &router,
            "/sessions",
            &json!({
                "agent": {"model": "kimi-k2", "instructions": "hi"},
                "computer": {"kind": "none"},
                "input": "hello"
            }),
            "user-a",
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(payload["session"]["status"], "running");
        let session_id = payload["session"]["id"].as_str().unwrap().to_string();

        let conn = state.db.connect().unwrap();
        let task_id: String = conn
            .query_row(
                "SELECT id FROM beta_work_tasks WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .unwrap();
        drop(conn);

        let (status, payload) = get_json(
            &router,
            "/beta/work/queue?worker_id=worker-a",
            "user-a",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(payload["task"]["id"], task_id);

        let (status, _) = post_json(
            &router,
            &format!("/beta/work/{task_id}/ack"),
            &json!({"worker_id": "worker-a", "result": {"ok": true}}),
            "user-a",
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, payload) = get_json(&router, &format!("/sessions/{session_id}"), "user-a").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(payload["session"]["status"], "idle");

        let (status, events_payload) =
            get_json(&router, &format!("/sessions/{session_id}/events"), "user-a").await;
        assert_eq!(status, StatusCode::OK);
        let types: Vec<&str> = events_payload["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["type"].as_str().unwrap())
            .collect();
        assert!(types.contains(&"turn.completed"), "{types:?}");
        assert!(types.contains(&"session.idle"), "{types:?}");

        let (status, turns_payload) =
            get_json(&router, &format!("/sessions/{session_id}/turns"), "user-a").await;
        assert_eq!(status, StatusCode::OK);
        let turns = turns_payload["turns"].as_array().unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0]["status"], "completed");
        assert!(turns[0]["id"].as_str().unwrap().contains("turn.started"));
    }

    #[tokio::test]
    async fn stored_turn_completed_translates_to_public_name() {
        let temp = beta_test::temp_dir("cloud-turn-translate");
        let state = beta_test::test_app_state(&temp).await;
        let router = cloud_agents_router().with_state(state.clone());

        let (status, payload) = post_json(
            &router,
            "/sessions",
            &json!({
                "agent": {"model": "kimi-k2", "instructions": "hi"},
                "computer": {"kind": "none"}
            }),
            "user-a",
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let id = payload["session"]["id"].as_str().unwrap().to_string();

        let conn = state.db.connect().unwrap();
        beta::emit_turn_terminal(&conn, &id, "completed", &json!({"task_id": "t1"})).unwrap();
        drop(conn);

        let (status, events_payload) =
            get_json(&router, &format!("/sessions/{id}/events"), "user-a").await;
        assert_eq!(status, StatusCode::OK);
        let types: Vec<&str> = events_payload["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["type"].as_str().unwrap())
            .collect();
        assert!(types.contains(&"turn.completed"), "{types:?}");
        assert!(types.contains(&"session.idle"), "{types:?}");
        assert!(!types.contains(&"turn_completed"));
        assert!(!types.contains(&"session_idle"));
    }

    #[tokio::test]
    async fn unknown_brain_id_is_400() {
        let temp = beta_test::temp_dir("cloud-brain-miss");
        let state = beta_test::test_app_state(&temp).await;
        let router = cloud_agents_router().with_state(state);
        let (status, payload) = post_json(
            &router,
            "/sessions",
            &json!({
                "agent": {"model": "kimi-k2", "instructions": "hi"},
                "computer": {"kind": "none"},
                "brain_id": "does-not-exist"
            }),
            "user-a",
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            payload["error"].as_str().unwrap_or("").contains("brain"),
            "{payload}"
        );
    }

    #[tokio::test]
    async fn known_brain_id_binds_on_session() {
        let temp = beta_test::temp_dir("cloud-brain-hit");
        let state = beta_test::test_app_state(&temp).await;
        let conn = state.db.connect().unwrap();
        conn.execute(
            "INSERT INTO brains (id, user_id, path) VALUES (?1, ?2, ?3)",
            params!["brain-1", "user-a", "/tmp/brain-1.git"],
        )
        .unwrap();
        drop(conn);
        let router = cloud_agents_router().with_state(state);
        let (status, payload) = post_json(
            &router,
            "/sessions",
            &json!({
                "agent": {"model": "kimi-k2", "instructions": "hi"},
                "computer": {"kind": "none"},
                "brain_id": "brain-1"
            }),
            "user-a",
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(payload["session"]["brain_id"], "brain-1");
    }

    #[tokio::test]
    async fn unknown_vault_id_is_400() {
        let temp = beta_test::temp_dir("cloud-vault-miss");
        let state = beta_test::test_app_state(&temp).await;
        let router = cloud_agents_router().with_state(state);
        let (status, payload) = post_json(
            &router,
            "/sessions",
            &json!({
                "agent": {"model": "kimi-k2", "instructions": "hi"},
                "computer": {"kind": "none"},
                "vault_ids": ["missing-vault"]
            }),
            "user-a",
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            payload["error"].as_str().unwrap_or("").contains("vault"),
            "{payload}"
        );
    }

    #[tokio::test]
    async fn unknown_memory_store_id_is_400() {
        let temp = beta_test::temp_dir("cloud-store-miss");
        let state = beta_test::test_app_state(&temp).await;
        let router = cloud_agents_router().with_state(state);
        let (status, payload) = post_json(
            &router,
            "/sessions",
            &json!({
                "agent": {"model": "kimi-k2", "instructions": "hi"},
                "computer": {"kind": "none"},
                "memory_store_ids": ["missing-store"]
            }),
            "user-a",
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            payload["error"]
                .as_str()
                .unwrap_or("")
                .contains("memory store"),
            "{payload}"
        );
    }

    #[tokio::test]
    async fn memory_store_ids_bind_and_echo_on_session() {
        let temp = beta_test::temp_dir("cloud-store-hit");
        let state = beta_test::test_app_state(&temp).await;
        let conn = state.db.connect().unwrap();
        conn.execute(
            "INSERT INTO beta_memory_stores (id, user_id, name) VALUES (?1, ?2, ?3)",
            params!["store-1", "user-a", "knowledge"],
        )
        .unwrap();
        // Another user's store must not bind.
        conn.execute(
            "INSERT INTO beta_memory_stores (id, user_id, name) VALUES (?1, ?2, ?3)",
            params!["store-other", "user-b", "foreign"],
        )
        .unwrap();
        drop(conn);
        let router = cloud_agents_router().with_state(state);

        let (status, payload) = post_json(
            &router,
            "/sessions",
            &json!({
                "agent": {"model": "kimi-k2", "instructions": "hi"},
                "computer": {"kind": "none"},
                "memory_store_ids": ["store-1"]
            }),
            "user-a",
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(payload["session"]["memory_store_ids"], json!(["store-1"]));
        assert_eq!(
            payload["session"]["metadata"]["memory_store_ids"],
            json!(["store-1"])
        );

        // Session read echoes the binding.
        let id = payload["session"]["id"].as_str().unwrap().to_string();
        let (status, read_back) = get_json(&router, &format!("/sessions/{id}"), "user-a").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(read_back["session"]["memory_store_ids"], json!(["store-1"]));

        // Cross-user binding is rejected.
        let (status, payload) = post_json(
            &router,
            "/sessions",
            &json!({
                "agent": {"model": "kimi-k2", "instructions": "hi"},
                "computer": {"kind": "none"},
                "memory_store_ids": ["store-other"]
            }),
            "user-a",
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn fabric_and_desktop_without_driver_are_503() {
        let temp = beta_test::temp_dir("cloud-kind-400");
        let state = beta_test::test_app_state(&temp).await;
        let router = cloud_agents_router().with_state(state);
        let (status, payload) = post_json(
            &router,
            "/sessions",
            &json!({
                "agent": {"model": "kimi-k2", "instructions": "hi"},
                "computer": {"kind": "desktop"}
            }),
            "user-a",
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(payload["code"], "computer_unavailable");
        assert!(
            payload["error"].as_str().unwrap_or("").contains("driver")
                || payload["error"].as_str().unwrap_or("").contains("Computer")
                || payload["error"].as_str().unwrap_or("").contains("desktop")
                || payload["error"].as_str().unwrap_or("").contains("Tart")
                || payload["error"].as_str().unwrap_or("").contains("substrate"),
            "{payload}"
        );

        let (status, payload) = post_json(
            &router,
            "/sessions",
            &json!({
                "agent": {"model": "kimi-k2", "instructions": "hi"},
                "computer": {"kind": "desktop"},
                "permission": "always_ask"
            }),
            "user-a",
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{payload}");
        assert_eq!(payload["session"]["permission"], "always_ask");
        assert_eq!(payload["session"]["computer"]["kind"], "desktop");

        let (status, payload) = post_json(
            &router,
            "/sessions",
            &json!({
                "agent": {"model": "kimi-k2", "instructions": "hi"},
                "computer": {"kind": "fabric"}
            }),
            "user-a",
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(payload["code"], "computer_unavailable");
        assert!(
            payload["error"].as_str().unwrap_or("").contains("driver")
                || payload["error"].as_str().unwrap_or("").contains("Computer")
                || payload["error"].as_str().unwrap_or("").contains("desktop")
                || payload["error"].as_str().unwrap_or("").contains("Tart")
                || payload["error"].as_str().unwrap_or("").contains("substrate"),
            "{payload}"
        );
    }

    #[tokio::test]
    async fn parent_thread_and_permission_and_empty_outputs() {
        let temp = beta_test::temp_dir("cloud-thread");
        let state = beta_test::test_app_state(&temp).await;
        let router = cloud_agents_router().with_state(state);

        let (status, parent) = post_json(
            &router,
            "/sessions",
            &json!({
                "agent": {"model": "kimi-k2", "instructions": "hi"},
                "computer": {"kind": "none"},
                "permission": "always_ask"
            }),
            "user-a",
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(parent["session"]["permission"], "always_ask");
        let parent_id = parent["session"]["id"].as_str().unwrap().to_string();

        let (status, child) = post_json(
            &router,
            "/sessions",
            &json!({
                "agent": {"model": "kimi-k2", "instructions": "hi"},
                "computer": {"kind": "none"},
                "parent_thread_id": parent_id
            }),
            "user-a",
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(child["session"]["parent_thread_id"], parent_id);

        let (status, threads) =
            get_json(&router, &format!("/sessions/{parent_id}/threads"), "user-a").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(threads["threads"].as_array().unwrap().len(), 1);

        let (status, outputs) =
            get_json(&router, &format!("/sessions/{parent_id}/outputs"), "user-a").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(outputs["outputs"].as_array().unwrap().len(), 0);

        let (status, _) = post_json(
            &router,
            "/sessions",
            &json!({
                "agent": {"model": "kimi-k2", "instructions": "hi"},
                "computer": {"kind": "none"},
                "parent_thread_id": "missing"
            }),
            "user-a",
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    // ── Phase 2 (G5): permission-mode enforcement ────────────────────────

    #[tokio::test]
    async fn session_create_always_ask_forces_all_tools_to_ask() {
        let temp = beta_test::temp_dir("perm-ask");
        let state = beta_test::test_app_state(&temp).await;
        let router = cloud_agents_router().with_state(state.clone());
        let agent_id = insert_agent(&state, "user-a", json!(["web_search", "bash"]), None);

        let (status, payload) = post_json(
            &router,
            "/sessions",
            &json!({
                "agent": agent_id,
                "computer": {"kind": "none"},
                "permission": "always_ask"
            }),
            "user-a",
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{payload}");
        assert_eq!(
            payload["session"]["effective_permissions"],
            json!({"web_search": "ask", "bash": "ask"})
        );
        // Echoed on the stored metadata too.
        assert_eq!(
            payload["session"]["metadata"]["effective_permissions"]["bash"],
            "ask"
        );
    }

    #[tokio::test]
    async fn session_create_auto_merges_per_tool_permissions() {
        let temp = beta_test::temp_dir("perm-auto");
        let state = beta_test::test_app_state(&temp).await;
        let router = cloud_agents_router().with_state(state.clone());
        let agent_id = insert_agent(
            &state,
            "user-a",
            json!(["web_search", "bash"]),
            Some(json!({"bash": "always_ask"})),
        );

        for mode in [json!("auto"), Value::Null] {
            let mut body = json!({
                "agent": agent_id,
                "computer": {"kind": "none"}
            });
            if !mode.is_null() {
                body["permission"] = mode;
            }
            let (status, payload) = post_json(&router, "/sessions", &body, "user-a").await;
            assert_eq!(status, StatusCode::CREATED, "{payload}");
            // Per-tool map decides under auto; unset tools default to allow.
            assert_eq!(
                payload["session"]["effective_permissions"],
                json!({"web_search": "allow", "bash": "ask"})
            );
        }
    }

    #[tokio::test]
    async fn session_create_always_allow_overrides_per_tool_map() {
        let temp = beta_test::temp_dir("perm-allow");
        let state = beta_test::test_app_state(&temp).await;
        let router = cloud_agents_router().with_state(state.clone());
        let agent_id = insert_agent(
            &state,
            "user-a",
            json!(["bash"]),
            Some(json!({"bash": "always_ask"})),
        );

        let (status, payload) = post_json(
            &router,
            "/sessions",
            &json!({
                "agent": agent_id,
                "computer": {"kind": "none"},
                "permission": "always_allow"
            }),
            "user-a",
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{payload}");
        assert_eq!(
            payload["session"]["effective_permissions"],
            json!({"bash": "allow"})
        );
    }

    #[tokio::test]
    async fn lease_payload_carries_effective_permissions() {
        let temp = beta_test::temp_dir("perm-lease");
        let state = beta_test::test_app_state(&temp).await;
        let router = cloud_agents_router()
            .merge(crate::beta_work_routes::beta_work_router())
            .with_state(state.clone());
        let agent_id = insert_agent(&state, "user-a", json!(["bash"]), None);

        let (status, payload) = post_json(
            &router,
            "/sessions",
            &json!({
                "agent": agent_id,
                "computer": {"kind": "none"},
                "input": "run something",
                "permission": "always_ask"
            }),
            "user-a",
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{payload}");

        let (status, payload) = get_json(&router, "/beta/work/queue?worker_id=w1", "user-a").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            payload["task"]["payload"]["effective_permissions"],
            json!({"bash": "ask"})
        );
    }

    #[tokio::test]
    async fn tool_calls_event_is_flagged_and_enqueued_when_effective_ask() {
        let temp = beta_test::temp_dir("perm-gating");
        let state = beta_test::test_app_state(&temp).await;
        let router = cloud_agents_router()
            .merge(beta::beta_session_router())
            .with_state(state.clone());
        let agent_id = insert_agent(
            &state,
            "user-a",
            json!(["web_search", "bash"]),
            Some(json!({"bash": "always_ask"})),
        );

        // Mode always_ask: both tools gate.
        let (status, payload) = post_json(
            &router,
            "/sessions",
            &json!({
                "agent": agent_id,
                "computer": {"kind": "none"},
                "permission": "always_ask"
            }),
            "user-a",
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{payload}");
        let ask_session = payload["session"]["id"].as_str().unwrap().to_string();

        let (status, payload) = send_json(
            &router,
            "POST",
            &format!("/beta/sessions/{ask_session}/events"),
            &json!({
                "type": "tool_calls",
                "data": {"tool_calls": [{"name": "bash", "args": {"command": "ls"}}]}
            }),
            "user-a",
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{payload}");
        let event = &payload["event"];
        assert_eq!(event["data"]["requires_approval"], true);
        assert_eq!(event["data"]["tool_calls"][0]["requires_approval"], true);
        let approval_id = event["data"]["tool_calls"][0]["approval_id"]
            .as_str()
            .unwrap()
            .to_string();
        // The approval landed in the shared store the approvals UI reads.
        let approval = state.approval_store.get(&approval_id).unwrap();
        assert_eq!(approval.tool, "bash");
        assert_eq!(
            approval.status,
            crate::permission_policy::ApprovalStatus::Pending
        );

        // Mode auto with only bash → ask: web_search flows unflagged.
        let (status, payload) = post_json(
            &router,
            "/sessions",
            &json!({
                "agent": agent_id,
                "computer": {"kind": "none"},
                "permission": "auto"
            }),
            "user-a",
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{payload}");
        let auto_session = payload["session"]["id"].as_str().unwrap().to_string();

        let (status, payload) = send_json(
            &router,
            "POST",
            &format!("/beta/sessions/{auto_session}/events"),
            &json!({
                "type": "tool_calls",
                "data": {"tool_calls": [
                    {"name": "bash", "args": {"command": "ls"}},
                    {"name": "web_search", "args": {"query": "x"}}
                ]}
            }),
            "user-a",
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{payload}");
        let data = &payload["event"]["data"];
        assert_eq!(data["requires_approval"], true);
        assert_eq!(data["tool_calls"][0]["requires_approval"], true);
        assert!(
            data["tool_calls"][1].get("requires_approval").is_none(),
            "web_search should be unflagged: {data}"
        );
    }
}
