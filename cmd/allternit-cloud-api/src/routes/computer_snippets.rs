//! Computers console snippets (surfaces/docs/dev/computers-console-contract.mdx,
//! step 2.9): saved commands, and running one on many computers with approval.
//!
//! Routes (signed in; every snippet belongs to its creator, nobody else sees
//! or runs it). Every error body is `{ error, message }`.
//!
//! * `GET /api/v1/computers/snippets` -> `{ snippets: [{ id, name, command, description?, elevated, timeoutSec?, createdAt, updatedAt }] }`
//! * `POST /api/v1/computers/snippets` `{ name, command, description?, elevated?, timeoutSec? }` -> 201 snippet
//! * `PATCH|PUT /api/v1/computers/snippets/:id` (any of the same fields) -> snippet
//! * `DELETE /api/v1/computers/snippets/:id` -> `{ removed: true }`
//! * `POST /api/v1/computers/snippets/:id/run` `{ computers: [consoleId], approvalConfirmed: true }`
//!   -> `{ runs: [{ computerId, approvalId, changeId, status, error, message }] }`
//! * `GET /api/v1/computers/snippets/runs?computerId=&limit=` -> `{ runs }`, newest
//!   first, for the Activity tab.
//!
//! Limits: 100 snippets per user, name 1-80 characters, description up to 280,
//! command 1 byte to 32 KB (the node refuses more than 64 KB), `timeoutSec` 1-1800,
//! at most 25 computers per run.
//!
//! ## Running
//!
//! `approvalConfirmed` must be `true`: the person looked at the command and the
//! list of computers and said yes (the clients show a hold-to-confirm sheet).
//! Without it nothing is minted or sent (400 `approval_required`). Then, for
//! each computer, in parallel and never waking anything:
//!
//! 1. the id is resolved with the console access rule
//!    (`computers_host::resolve_target`); not yours = `not_found`;
//! 2. a sleeping or stopped cloud computer = `asleep`, one with no live relay
//!    = `offline`; a cloud computer with no Allternit node = `not_supported`;
//! 3. the cloud mints an approval bound to this user, this computer's runtime,
//!    type `snippet.run` and target = the snippet id
//!    (`computers_host::mint_approval`, the same flow as every approved action);
//! 4. `POST /v1/host/actions` goes through the relay with
//!    `{ type: "snippet.run", target: <snippet id>, approval_id, snippet: { id, name, script, elevated, timeoutSec } }`.
//!
//! ### How the node gets the command (decision, step 2.9)
//!
//! The command travels inside the approved action; the node never fetches it
//! and nothing is saved on the node. host-probe's `ActionRequest` gained an
//! optional `snippet` object for this (its `id` must equal `target`; without
//! it the engine still reads `<store>/snippets/<id>.json`, so a hand-placed
//! file keeps working). The approval binds the snippet id, not the command
//! text, so the cloud (which only sends the command the owner saved) is the
//! authority on the text; the node's own checks (64 KB, elevation, masked
//! output, change record) still apply.
//!
//! ### Statuses
//!
//! `done` (the command ran and finished; `changeId` is the node's change
//! record with the masked output), `failed` (it ran or was refused with an
//! error; `changeId` when the node kept a record), `needs_elevation` (the
//! snippet is marked admin and the computer's helper is not set up),
//! `refused`, `asleep`, `offline`, `not_supported` (cloud computer without the
//! node, or a node too old to run snippets), `not_found`. `error` and
//! `message` carry one plain sentence for every status except `done`.
//! Every outcome is stored in `computer_snippet_runs` (migration 158) and
//! audit-logged as `computer.snippet.run`.

use axum::{
    body::to_bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;

use crate::routes::computers_host::{
    gate, mint_approval, relay_key, resolve_target, Gate, TargetKind,
};
use crate::routes::paired_computers::{signed_in_caller, Caller};
use crate::routes::platform_v1::new_id;
use crate::routes::runtime_relay::{relay_request_no_wake, NoWakeRelay, RelayRequest};
use crate::services::audit::{write_audit_log, AuditEvent};
use crate::{ApiError, ApiState};

pub const SNIPPET_ACTION: &str = "snippet.run";
pub const MAX_SNIPPETS: i64 = 100;
pub const MAX_NAME: usize = 80;
pub const MAX_DESCRIPTION: usize = 280;
pub const MAX_COMMAND_BYTES: usize = 32 * 1024;
pub const MAX_TIMEOUT_SEC: i64 = 1800;
pub const MAX_COMPUTERS: usize = 25;
const MAX_NODE_ANSWER: usize = 256 * 1024;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route(
            "/api/v1/computers/snippets",
            get(list_snippets).post(create_snippet),
        )
        .route("/api/v1/computers/snippets/runs", get(list_runs))
        .route(
            "/api/v1/computers/snippets/:id",
            get(get_snippet)
                .patch(update_snippet)
                .put(update_snippet)
                .delete(delete_snippet),
        )
        .route("/api/v1/computers/snippets/:id/run", post(run_snippet))
}

fn fail(status: StatusCode, error: &str, message: &str) -> Response {
    (status, Json(json!({ "error": error, "message": message }))).into_response()
}

fn db_fail(e: sqlx::Error) -> Response {
    tracing::warn!("computer snippets: {e}");
    fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "db_error",
        "Something went wrong saving that. Try again in a moment.",
    )
}

fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

// ------------------------------------------------------------- the rows ----

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct SnippetRow {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub command: String,
    pub description: Option<String>,
    pub elevated: bool,
    pub timeout_sec: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLS: &str =
    "id, user_id, name, command, description, elevated, timeout_sec, created_at, updated_at";

impl SnippetRow {
    fn json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "command": self.command,
            "description": self.description,
            "elevated": self.elevated,
            "timeoutSec": self.timeout_sec,
            "createdAt": rfc3339(self.created_at),
            "updatedAt": rfc3339(self.updated_at),
        })
    }
}

// ---------------------------------------------------------- the checks ----

pub fn check_name(name: &str) -> Result<String, &'static str> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
        return Err("Give the snippet a name of 1 to 80 characters.");
    }
    Ok(name.to_string())
}

pub fn check_command(command: &str) -> Result<String, &'static str> {
    if command.trim().is_empty() {
        return Err("The snippet needs a command to run.");
    }
    if command.len() > MAX_COMMAND_BYTES {
        return Err(
            "The command is longer than 32 KB. Shorten it or keep it as a script on the computer.",
        );
    }
    if command.contains('\0') {
        return Err("The command has a character that cannot be sent.");
    }
    Ok(command.to_string())
}

pub fn check_description(description: Option<String>) -> Result<Option<String>, &'static str> {
    let d = description
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty());
    match d {
        Some(d) if d.chars().count() > MAX_DESCRIPTION => {
            Err("The description can be up to 280 characters.")
        }
        other => Ok(other),
    }
}

pub fn check_timeout(timeout: Option<i64>) -> Result<Option<i32>, &'static str> {
    match timeout {
        None => Ok(None),
        Some(t) if (1..=MAX_TIMEOUT_SEC).contains(&t) => Ok(Some(t as i32)),
        Some(_) => Err("The time limit must be 1 to 1800 seconds."),
    }
}

// ---------------------------------------------------------------- CRUD ----

async fn load(db: &PgPool, user_id: &str, id: &str) -> Result<Option<SnippetRow>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {COLS} FROM computer_snippets WHERE id = $1 AND user_id = $2"
    ))
    .bind(id)
    .bind(user_id)
    .fetch_optional(db)
    .await
}

fn not_found() -> Response {
    fail(
        StatusCode::NOT_FOUND,
        "not_found",
        "That snippet was not found.",
    )
}

async fn list_snippets(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let rows: Vec<SnippetRow> = match sqlx::query_as(&format!(
        "SELECT {COLS} FROM computer_snippets WHERE user_id = $1 ORDER BY lower(name), id"
    ))
    .bind(&caller.user_id)
    .fetch_all(&state.db)
    .await
    {
        Ok(r) => r,
        Err(e) => return Ok(db_fail(e)),
    };
    Ok(
        Json(json!({ "snippets": rows.iter().map(SnippetRow::json).collect::<Vec<_>>() }))
            .into_response(),
    )
}

async fn get_snippet(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    Ok(match load(&state.db, &caller.user_id, &id).await {
        Ok(Some(row)) => Json(row.json()).into_response(),
        Ok(None) => not_found(),
        Err(e) => db_fail(e),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnippetBody {
    name: Option<String>,
    command: Option<String>,
    description: Option<String>,
    elevated: Option<bool>,
    timeout_sec: Option<i64>,
}

async fn create_snippet(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(body): Json<SnippetBody>,
) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let name = match check_name(body.name.as_deref().unwrap_or("")) {
        Ok(n) => n,
        Err(m) => return Ok(fail(StatusCode::BAD_REQUEST, "invalid_name", m)),
    };
    let command = match check_command(body.command.as_deref().unwrap_or("")) {
        Ok(c) => c,
        Err(m) => return Ok(fail(StatusCode::BAD_REQUEST, "invalid_command", m)),
    };
    let description = match check_description(body.description) {
        Ok(d) => d,
        Err(m) => return Ok(fail(StatusCode::BAD_REQUEST, "invalid_description", m)),
    };
    let timeout = match check_timeout(body.timeout_sec) {
        Ok(t) => t,
        Err(m) => return Ok(fail(StatusCode::BAD_REQUEST, "invalid_timeout", m)),
    };
    let count: i64 =
        match sqlx::query_scalar("SELECT count(*) FROM computer_snippets WHERE user_id = $1")
            .bind(&caller.user_id)
            .fetch_one(&state.db)
            .await
        {
            Ok(n) => n,
            Err(e) => return Ok(db_fail(e)),
        };
    if count >= MAX_SNIPPETS {
        return Ok(fail(
            StatusCode::CONFLICT,
            "too_many_snippets",
            "You have reached the limit of 100 snippets. Delete one first.",
        ));
    }
    let id = new_id("snp_");
    let inserted: Result<SnippetRow, _> = sqlx::query_as(&format!(
        "INSERT INTO computer_snippets (id, user_id, name, command, description, elevated, timeout_sec)
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {COLS}"
    ))
    .bind(&id)
    .bind(&caller.user_id)
    .bind(&name)
    .bind(&command)
    .bind(&description)
    .bind(body.elevated.unwrap_or(false))
    .bind(timeout)
    .fetch_one(&state.db)
    .await;
    Ok(match inserted {
        Ok(row) => (StatusCode::CREATED, Json(row.json())).into_response(),
        Err(e) => db_fail(e),
    })
}

async fn update_snippet(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<SnippetBody>,
) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let current = match load(&state.db, &caller.user_id, &id).await {
        Ok(Some(row)) => row,
        Ok(None) => return Ok(not_found()),
        Err(e) => return Ok(db_fail(e)),
    };
    let name = match body.name.as_deref() {
        Some(n) => match check_name(n) {
            Ok(n) => n,
            Err(m) => return Ok(fail(StatusCode::BAD_REQUEST, "invalid_name", m)),
        },
        None => current.name.clone(),
    };
    let command = match body.command.as_deref() {
        Some(c) => match check_command(c) {
            Ok(c) => c,
            Err(m) => return Ok(fail(StatusCode::BAD_REQUEST, "invalid_command", m)),
        },
        None => current.command.clone(),
    };
    // An empty description clears it; a missing one keeps it.
    let description = match body.description {
        Some(d) => match check_description(Some(d)) {
            Ok(d) => d,
            Err(m) => return Ok(fail(StatusCode::BAD_REQUEST, "invalid_description", m)),
        },
        None => current.description.clone(),
    };
    let timeout = match body.timeout_sec {
        Some(t) => match check_timeout(Some(t)) {
            Ok(t) => t,
            Err(m) => return Ok(fail(StatusCode::BAD_REQUEST, "invalid_timeout", m)),
        },
        None => current.timeout_sec,
    };
    let updated: Result<SnippetRow, _> = sqlx::query_as(&format!(
        "UPDATE computer_snippets SET name = $3, command = $4, description = $5, elevated = $6, timeout_sec = $7, updated_at = now()
         WHERE id = $1 AND user_id = $2 RETURNING {COLS}"
    ))
    .bind(&id)
    .bind(&caller.user_id)
    .bind(&name)
    .bind(&command)
    .bind(&description)
    .bind(body.elevated.unwrap_or(current.elevated))
    .bind(timeout)
    .fetch_one(&state.db)
    .await;
    Ok(match updated {
        Ok(row) => Json(row.json()).into_response(),
        Err(e) => db_fail(e),
    })
}

async fn delete_snippet(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    Ok(
        match sqlx::query("DELETE FROM computer_snippets WHERE id = $1 AND user_id = $2")
            .bind(&id)
            .bind(&caller.user_id)
            .execute(&state.db)
            .await
        {
            Ok(r) if r.rows_affected() > 0 => Json(json!({ "removed": true })).into_response(),
            Ok(_) => not_found(),
            Err(e) => db_fail(e),
        },
    )
}

// ------------------------------------------------------------- the runs ----

#[derive(Deserialize)]
struct RunsQuery {
    #[serde(rename = "computerId")]
    computer_id: Option<String>,
    limit: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct RunRow {
    id: String,
    snippet_id: String,
    snippet_name: String,
    computer_id: String,
    approval_id: Option<String>,
    change_id: Option<String>,
    status: String,
    error: Option<String>,
    created_at: DateTime<Utc>,
}

async fn list_runs(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Query(q): Query<RunsQuery>,
) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let rows: Result<Vec<RunRow>, _> = sqlx::query_as(
        "SELECT id, snippet_id, snippet_name, computer_id, approval_id, change_id, status, error, created_at
         FROM computer_snippet_runs
         WHERE user_id = $1 AND ($2::text IS NULL OR computer_id = $2)
         ORDER BY created_at DESC, id DESC LIMIT $3",
    )
    .bind(&caller.user_id)
    .bind(q.computer_id.as_deref().filter(|c| !c.is_empty()))
    .bind(limit)
    .fetch_all(&state.db)
    .await;
    let rows = match rows {
        Ok(r) => r,
        Err(e) => return Ok(db_fail(e)),
    };
    let runs: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "id": r.id,
                "snippetId": r.snippet_id,
                "snippetName": r.snippet_name,
                "computerId": r.computer_id,
                "approvalId": r.approval_id,
                "changeId": r.change_id,
                "status": r.status,
                "error": r.error,
                "message": r.error,
                "createdAt": rfc3339(r.created_at),
            })
        })
        .collect();
    Ok(Json(json!({ "runs": runs })).into_response())
}

/// What one computer did with the snippet.
#[derive(Debug, Clone, PartialEq)]
pub struct RunOutcome {
    pub status: &'static str,
    pub change_id: Option<String>,
    pub error: Option<String>,
    pub approval_id: Option<String>,
}

impl RunOutcome {
    fn without_node(status: &'static str, error: &str) -> Self {
        RunOutcome {
            status,
            change_id: None,
            error: Some(error.to_string()),
            approval_id: None,
        }
    }
}

/// Turn the node's answer to `POST /v1/host/actions` into a run outcome.
pub fn outcome_of(http_status: u16, body: &Value, approval_id: &str) -> RunOutcome {
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            body.get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    let error_kind = body.get("error").and_then(Value::as_str).unwrap_or("");
    let change_id = body
        .get("changeId")
        .or_else(|| body.get("change_id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let with = |status: &'static str, text: &str| RunOutcome {
        status,
        change_id: change_id.clone(),
        error: Some(
            message
                .clone()
                .filter(|m| !m.is_empty() && m.contains(' '))
                .unwrap_or_else(|| text.to_string()),
        ),
        approval_id: Some(approval_id.to_string()),
    };
    match http_status {
        200..=299 => RunOutcome { status: "done", change_id, error: None, approval_id: Some(approval_id.to_string()) },
        403 if error_kind == "needs_elevation" => RunOutcome {
            status: "needs_elevation",
            change_id,
            error: Some("This snippet needs admin rights and the admin helper is not set up on that computer.".into()),
            approval_id: Some(approval_id.to_string()),
        },
        403 => with("refused", "That computer refused the approval. Try again."),
        404 | 405 | 501 => RunOutcome {
            status: "not_supported",
            change_id: None,
            error: Some("That computer's Allternit software cannot run snippets yet. Update it and try again.".into()),
            approval_id: Some(approval_id.to_string()),
        },
        409 => with("refused", "That computer refused to run the snippet."),
        _ => with("failed", "The snippet did not finish on that computer."),
    }
}

/// Run `snippet` on one computer and say what happened. Never wakes it.
async fn run_on(
    state: &ApiState,
    caller: &Caller,
    snippet: &SnippetRow,
    computer_id: &str,
) -> RunOutcome {
    let target = match resolve_target(&state.db, caller, computer_id).await {
        Ok(Some(t)) => t,
        Ok(None) => return RunOutcome::without_node("not_found", "That computer was not found."),
        Err(e) => {
            tracing::warn!("computer snippets: resolve {computer_id}: {e}");
            return RunOutcome::without_node(
                "failed",
                "Something went wrong looking up that computer.",
            );
        }
    };
    let runtime_id = match gate(&target) {
        Gate::Asleep { .. } => {
            return RunOutcome::without_node(
                "asleep",
                "That computer is asleep. Wake it first, then run the snippet.",
            )
        }
        Gate::Offline { .. } if target.kind == TargetKind::Cloud && target.runtime_id.is_none() => {
            return RunOutcome::without_node(
                "not_supported",
                "This cloud computer has no Allternit node yet, so it cannot run snippets.",
            );
        }
        Gate::Offline { .. } => {
            return RunOutcome::without_node("offline", "That computer is offline.")
        }
        Gate::Relay { runtime_id } => runtime_id,
    };
    let key = match relay_key(&state.db, &runtime_id).await {
        Ok(k) => k,
        Err(_) => {
            return RunOutcome::without_node(
                "not_supported",
                "That computer is not set up to take approved actions.",
            )
        }
    };
    let (approval_id, _exp) = mint_approval(
        &key,
        &caller.user_id,
        &runtime_id,
        SNIPPET_ACTION,
        &snippet.id,
        Utc::now().timestamp(),
    );
    let body = json!({
        "type": SNIPPET_ACTION,
        "target": snippet.id,
        "approval_id": approval_id,
        "snippet": {
            "id": snippet.id,
            "name": snippet.name,
            "script": snippet.command,
            "elevated": snippet.elevated,
            "timeoutSec": snippet.timeout_sec,
        },
    });
    let mut headers = HashMap::new();
    headers.insert("content-type".to_string(), "application/json".to_string());
    let request = RelayRequest {
        method: "POST".into(),
        path: "/v1/host/actions".into(),
        headers,
        body: body.to_string(),
        body_encoding: "utf8".into(),
    };
    let answer = match relay_request_no_wake(
        &state.db,
        &target.owner_id,
        &runtime_id,
        request,
        HashMap::new(),
    )
    .await
    {
        Ok(NoWakeRelay::Response(r)) => r,
        Ok(NoWakeRelay::Offline) => {
            return RunOutcome {
                approval_id: Some(approval_id),
                ..RunOutcome::without_node("offline", "That computer is offline.")
            }
        }
        Err(e) => {
            tracing::warn!("computer snippets: relay to {runtime_id}: {e}");
            return RunOutcome {
                approval_id: Some(approval_id),
                ..RunOutcome::without_node(
                    "failed",
                    "Could not reach that computer. Try again in a moment.",
                )
            };
        }
    };
    let status = answer.status().as_u16();
    let bytes = to_bytes(answer.into_body(), MAX_NODE_ANSWER)
        .await
        .unwrap_or_default();
    let node: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    outcome_of(status, &node, &approval_id)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunBody {
    #[serde(default)]
    computers: Vec<String>,
    #[serde(default)]
    approval_confirmed: bool,
}

async fn run_snippet(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<RunBody>,
) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let snippet = match load(&state.db, &caller.user_id, &id).await {
        Ok(Some(row)) => row,
        Ok(None) => return Ok(not_found()),
        Err(e) => return Ok(db_fail(e)),
    };
    if !body.approval_confirmed {
        return Ok(fail(
            StatusCode::BAD_REQUEST,
            "approval_required",
            "Confirm the command and the computers before it runs. Nothing was sent.",
        ));
    }
    let mut computers: Vec<String> = Vec::new();
    for c in body
        .computers
        .iter()
        .map(|c| c.trim())
        .filter(|c| !c.is_empty())
    {
        if !computers.iter().any(|x| x == c) {
            computers.push(c.to_string());
        }
    }
    if computers.is_empty() {
        return Ok(fail(
            StatusCode::BAD_REQUEST,
            "no_computers",
            "Pick at least one computer to run it on.",
        ));
    }
    if computers.len() > MAX_COMPUTERS {
        return Ok(fail(
            StatusCode::BAD_REQUEST,
            "too_many_computers",
            "Run it on up to 25 computers at a time.",
        ));
    }
    let outcomes = futures::future::join_all(
        computers
            .iter()
            .map(|c| run_on(&state, &caller, &snippet, c)),
    )
    .await;
    let mut runs = Vec::with_capacity(outcomes.len());
    for (computer_id, outcome) in computers.iter().zip(outcomes) {
        let run_id = new_id("snr_");
        if let Err(e) = sqlx::query(
            "INSERT INTO computer_snippet_runs (id, user_id, snippet_id, snippet_name, computer_id, approval_id, change_id, status, error)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(&run_id)
        .bind(&caller.user_id)
        .bind(&snippet.id)
        .bind(&snippet.name)
        .bind(computer_id)
        .bind(&outcome.approval_id)
        .bind(&outcome.change_id)
        .bind(outcome.status)
        .bind(&outcome.error)
        .execute(&state.db)
        .await
        {
            tracing::warn!("computer snippets: store run: {e}");
        }
        write_audit_log(
            &state.db,
            AuditEvent {
                action: "computer.snippet.run".into(),
                resource_type: "computer".into(),
                resource_id: Some(computer_id.clone()),
                user_id: Some(caller.user_id.clone()),
                user_email: None,
                details: Some(json!({
                    "snippetId": snippet.id,
                    "status": outcome.status,
                    "changeId": outcome.change_id,
                    "jti": outcome.approval_id.as_deref().and_then(|a| a.split('.').nth(1)),
                })),
                success: outcome.status == "done",
            },
        )
        .await;
        runs.push(json!({
            "computerId": computer_id,
            "approvalId": outcome.approval_id,
            "changeId": outcome.change_id,
            "status": outcome.status,
            "error": outcome.error,
            "message": outcome.error,
        }));
    }
    Ok(Json(json!({ "runs": runs })).into_response())
}

#[cfg(test)]
#[path = "computer_snippets_tests.rs"]
mod tests;
