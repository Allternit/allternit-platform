//! MCP edge: Tasks (SEP-2663, Final, extension `io.modelcontextprotocol/tasks`).
//!
//! A client that lists the extension in a request's `_meta` client
//! capabilities gets a long-running `tools/call` back at once as a
//! `CreateTaskResult` (`resultType: "task"`). The call then runs in a tokio
//! task here at the edge and its state lives in `mcp_tasks` (D28: task state
//! at the server; the allternit-api proxy stays stateless). The client polls
//! `tasks/get`, answers `inputRequests` with `tasks/update`, or stops it with
//! `tasks/cancel`. Without the declaration nothing changes.
//!
//! The owner-approval gate (`mcp_edge::approval_required`) also becomes a task
//! for a client that declared Tasks and URL elicitation: the task waits in
//! `input_required` with a `mode: "url"` elicitation pointing at the approve
//! page. Accepting it never approves anything by itself: the edge re-checks
//! the owner's stored approval, and runs the call only once it is there.
//!
//! Row shape (migration 161, `mcp_tasks`): `owner_id` holds [`owner_key`]
//! (user, client and target, so another app of the same user can't read the
//! task); `input_requests` the pending map; while a task waits for approval,
//! `result` holds `{"pendingCall": <the original JSON-RPC request>}` and is
//! never shown; `error` holds the JSON-RPC error object as JSON text.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Value};

use super::mcp_edge::EdgeBackend;

pub const TASKS_EXTENSION_ID: &str = "io.modelcontextprotocol/tasks";

/// Tools that can take longer than one HTTP request should wait (plan v3
/// E8, Wave 0 lane 3). Only these become tasks; everything else answers
/// inline as before. Relayed bot and connector calls follow `is_task_capable`.
pub const LONG_RUNNING_TOOLS: &[&str] = &[
    // Answered at the edge for a project key with `phones` (`platform_v1/phones.rs` `mcp_tools`).
    "phone_tap",
    "phone_type",
    "phone_open_app",
    "phone_tool",
    "phone_task",
    // Answered at the edge for a key with `mail` (`platform_v1/mailboxes.rs` `MCP_TOOLS`).
    "mail_read_otp",
    // Answered at the edge (`mcp_edge::MODEL_TOOLS`).
    "decide",
    "respond",
    // Relayed to the owner's runtime (`allternit-api/src/mcp_server_routes.rs`).
    "agent_send",
    "shell.exec",
    "http.get",
    "http.post",
    "inference.execute_routed_turn",
];

/// How often a client should poll `tasks/get` (also the column default).
pub const POLL_INTERVAL_MS: i64 = 1000;
/// How long a task is kept after it was created.
pub const TTL_MS: i64 = 60 * 60 * 1000;
/// The longest a background call may run before it fails.
const JOB_LIMIT: Duration = Duration::from_secs(10 * 60);
/// A `working` row untouched this long lost its job (a restart): reported as failed.
const STALE_AFTER_MS: i64 = 11 * 60 * 1000;
/// The `inputRequests` key of the owner-approval elicitation.
pub const APPROVAL_KEY: &str = "approval";

const INTERNAL_ERROR: i64 = -32603;
const INVALID_PARAMS: i64 = mcp_protocol::codes::INVALID_PARAMS;

pub fn is_long_running(tool: &str) -> bool {
    LONG_RUNNING_TOOLS.contains(&tool)
}

/// Verbs that mark a tool as instant: a lookup that answers at once.
const INSTANT_VERBS: &[&str] = &[
    "list", "get", "read", "search", "find", "describe", "show", "status", "info", "whoami", "ping", "count", "lookup", "check",
];

/// A tool whose name says it is a quick lookup (`list_issues`, `github__get_user`,
/// `agents_list`, `deploy_status`). Judged on the tool part of a
/// `<connector>__<tool>` name, by its first or last word.
pub fn is_instant(tool: &str) -> bool {
    let name = tool.rsplit("__").next().unwrap_or(tool).to_ascii_lowercase();
    let words: Vec<&str> = name.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    match (words.first(), words.last()) {
        (Some(a), Some(b)) => INSTANT_VERBS.contains(a) || INSTANT_VERBS.contains(b),
        _ => true,
    }
}

/// THE rule for which `tools/call`s may become Tasks (plan v3 E8, full
/// coverage). `relayed_to_service` is true when the call goes to a vendor bot
/// or a connector: those wait on another service, so every one is task-capable
/// unless its name says it is instant. Everything else uses the fixed list.
pub fn is_task_capable(tool: &str, relayed_to_service: bool) -> bool {
    is_long_running(tool) || (relayed_to_service && !tool.is_empty() && !is_instant(tool))
}

fn client_capabilities(params: &Value) -> Option<&Value> {
    params.get("_meta").and_then(|m| m.get(mcp_protocol::META_CLIENT_CAPABILITIES))
}

/// Did this request list the Tasks extension in its `_meta` client capabilities?
pub fn declares_tasks(params: &Value) -> bool {
    client_capabilities(params)
        .and_then(|c| c.get("extensions"))
        .and_then(|e| e.get(TASKS_EXTENSION_ID))
        .is_some_and(|v| !v.is_null() && *v != Value::Bool(false))
}

/// Does the client handle `mode: "url"` elicitation (`elicitation.url`)?
pub fn supports_url_elicitation(params: &Value) -> bool {
    client_capabilities(params).and_then(|c| c.pointer("/elicitation/url")).is_some_and(Value::is_object)
}

/// The request with the Tasks extension taken out of its `_meta`, so the
/// runtime the edge relays a task's call to answers it inline.
pub fn without_tasks(request: &Value) -> Value {
    let mut r = request.clone();
    let path = format!("/params/_meta/{}/extensions", mcp_protocol::META_CLIENT_CAPABILITIES.replace('/', "~1"));
    if let Some(ext) = r.pointer_mut(&path).and_then(Value::as_object_mut) {
        ext.remove(TASKS_EXTENSION_ID);
    }
    r
}

/// Adds the extension to a server's capabilities (initialize, `server/discover`).
pub fn advertise(capabilities: &mut Value) {
    if let Some(caps) = capabilities.as_object_mut() {
        let ext = caps.entry("extensions").or_insert_with(|| json!({}));
        if let Some(ext) = ext.as_object_mut() {
            ext.insert(TASKS_EXTENSION_ID.into(), json!({}));
        }
    }
}

/// `mcp_tasks.owner_id`: who may see a task.
pub fn owner_key(user_id: &str, client: &str, target: &str) -> String {
    format!("{user_id}|{client}|{target}")
}

// ─── jobs ────────────────────────────────────────────────────────────────────

/// How a background call ended.
#[derive(Debug, Clone, PartialEq)]
pub enum JobOutcome {
    /// A `CallToolResult` (tool errors are `isError: true` here): `completed`.
    Result(Value),
    /// A JSON-RPC error object: `failed`.
    Error(Value),
}

pub fn job_error(message: impl Into<String>) -> JobOutcome {
    JobOutcome::Error(json!({ "code": INTERNAL_ERROR, "message": message.into() }))
}

/// A relayed JSON-RPC reply as an outcome.
pub fn outcome_from_reply(status: u16, reply: &[u8]) -> JobOutcome {
    match serde_json::from_slice::<Value>(reply) {
        Ok(v) if v.get("result").is_some() => JobOutcome::Result(v["result"].clone()),
        Ok(v) if v["error"].is_object() => JobOutcome::Error(v["error"].clone()),
        _ => job_error(format!("The runtime answered with HTTP {status}.")),
    }
}

pub type Job = Pin<Box<dyn Future<Output = JobOutcome> + Send + 'static>>;

/// What the edge lends a task: where rows live, and an owned copy of the
/// backend for the background job (`tokio::spawn` needs `'static`).
#[derive(Clone)]
pub struct TaskRuntime {
    pub store: Arc<dyn TaskStore>,
    pub backend: Arc<dyn EdgeBackend>,
}

/// Lets `tasks/update` continue a task that waited for the owner's approval.
#[async_trait::async_trait]
pub trait Resume: Send + Sync {
    /// `Ok(Some(job))` = approved now, run this; `Ok(None)` = still not approved.
    async fn approved(&self, pending_call: &Value) -> Result<Option<Job>, String>;
}

fn running() -> &'static Mutex<HashMap<String, tokio::task::AbortHandle>> {
    static RUNNING: OnceLock<Mutex<HashMap<String, tokio::task::AbortHandle>>> = OnceLock::new();
    RUNNING.get_or_init(|| Mutex::new(HashMap::new()))
}

fn spawn_job(store: Arc<dyn TaskStore>, id: String, job: Job) {
    // Held across the spawn so the job's own `remove` can't run before the insert.
    let mut map = running().lock().unwrap_or_else(|p| p.into_inner());
    let task_id = id.clone();
    let handle = tokio::spawn(async move {
        let started = std::time::Instant::now();
        let limited = tokio::time::timeout(JOB_LIMIT, job);
        tokio::pin!(limited);
        let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + PROGRESS_EVERY, PROGRESS_EVERY);
        let outcome = loop {
            tokio::select! {
                done = &mut limited => break done.unwrap_or_else(|_| job_error("The tool did not finish in time.")),
                _ = tick.tick() => {
                    let progress = working_progress(started.elapsed());
                    // No longer working (cancelled): the job is about to be aborted.
                    if let Ok(false) = store.transition(&task_id, &["working"], "working", &json!({}), Some(&progress), None).await {
                        std::future::pending::<()>().await;
                    }
                }
            }
        };
        let (to, result, error) = match outcome {
            JobOutcome::Result(r) => ("completed", Some(r), None),
            JobOutcome::Error(e) => ("failed", None, Some(e.to_string())),
        };
        // Only a still-working task finishes: a cancelled one stays cancelled.
        if let Err(e) = store.transition(&task_id, &["working"], to, &json!({}), result.as_ref(), error.as_deref()).await {
            tracing::warn!(task = %task_id, "mcp edge tasks: saving the outcome failed: {e}");
        }
        running().lock().unwrap_or_else(|p| p.into_inner()).remove(&task_id);
    });
    map.insert(id, handle.abort_handle());
}

/// How often a running task writes its progress.
const PROGRESS_EVERY: Duration = Duration::from_secs(5);

/// What a `working` task keeps in `result` (the table has no progress columns):
/// `progress` and `statusMessage`. A finished task's `result` is the tool result.
fn working_progress(elapsed: Duration) -> Value {
    let secs = elapsed.as_secs();
    json!({ "progress": { "elapsedMs": elapsed.as_millis() as u64 }, "statusMessage": format!("Still running ({secs}s).") })
}

fn abort_job(id: &str) {
    if let Some(h) = running().lock().unwrap_or_else(|p| p.into_inner()).remove(id) {
        h.abort();
    }
}

// ─── store ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct TaskRow {
    pub id: String,
    pub owner_id: String,
    pub tool: String,
    pub status: String,
    pub input_requests: Value,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub poll_interval_ms: i64,
    pub ttl_ms: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TaskRow {
    fn new(owner_id: &str, tool: &str, status: &str, input_requests: Value, result: Option<Value>) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            owner_id: owner_id.into(),
            tool: tool.into(),
            status: status.into(),
            input_requests,
            result,
            error: None,
            poll_interval_ms: POLL_INTERVAL_MS,
            ttl_ms: Some(TTL_MS),
            created_at: now,
            updated_at: now,
        }
    }

    fn expired(&self, now: DateTime<Utc>) -> bool {
        self.ttl_ms.is_some_and(|ttl| (now - self.created_at).num_milliseconds() > ttl)
    }
}

#[async_trait::async_trait]
pub trait TaskStore: Send + Sync {
    async fn insert(&self, row: &TaskRow) -> Result<(), String>;
    /// The task, only for its owner.
    async fn get(&self, id: &str, owner_id: &str) -> Result<Option<TaskRow>, String>;
    /// Move a task from one of `from` to `to`, setting the payload columns;
    /// `false` = it was in none of `from`.
    async fn transition(&self, id: &str, from: &[&str], to: &str, input_requests: &Value, result: Option<&Value>, error: Option<&str>) -> Result<bool, String>;
    /// Drop the owner's tasks past their TTL.
    async fn purge_expired(&self, owner_id: &str) -> Result<(), String>;
}

pub struct PgTaskStore {
    pub db: sqlx::PgPool,
}

#[async_trait::async_trait]
impl TaskStore for PgTaskStore {
    async fn insert(&self, r: &TaskRow) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO mcp_tasks (id, owner_id, tool, status, input_requests, result, error, poll_interval_ms, ttl_ms, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(&r.id)
        .bind(&r.owner_id)
        .bind(&r.tool)
        .bind(&r.status)
        .bind(sqlx::types::Json(&r.input_requests))
        .bind(r.result.as_ref().map(sqlx::types::Json))
        .bind(&r.error)
        .bind(r.poll_interval_ms as i32)
        .bind(r.ttl_ms)
        .bind(r.created_at)
        .bind(r.updated_at)
        .execute(&self.db)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
    }

    async fn get(&self, id: &str, owner_id: &str) -> Result<Option<TaskRow>, String> {
        use sqlx::Row;
        let row = sqlx::query(
            "SELECT id, owner_id, tool, status, input_requests, result, error, poll_interval_ms, ttl_ms, created_at, updated_at \
             FROM mcp_tasks WHERE id = $1 AND owner_id = $2",
        )
        .bind(id)
        .bind(owner_id)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| e.to_string())?;
        let Some(row) = row else { return Ok(None) };
        let json = |name: &str| -> Result<Option<Value>, String> {
            row.try_get::<Option<sqlx::types::Json<Value>>, _>(name).map(|v| v.map(|j| j.0)).map_err(|e| e.to_string())
        };
        let get = |e: sqlx::Error| e.to_string();
        Ok(Some(TaskRow {
            id: row.try_get("id").map_err(get)?,
            owner_id: row.try_get("owner_id").map_err(get)?,
            tool: row.try_get("tool").map_err(get)?,
            status: row.try_get("status").map_err(get)?,
            input_requests: json("input_requests")?.unwrap_or_else(|| json!({})),
            result: json("result")?,
            error: row.try_get("error").map_err(get)?,
            poll_interval_ms: row.try_get::<i32, _>("poll_interval_ms").map_err(get)? as i64,
            ttl_ms: row.try_get("ttl_ms").map_err(get)?,
            created_at: row.try_get("created_at").map_err(get)?,
            updated_at: row.try_get("updated_at").map_err(get)?,
        }))
    }

    async fn transition(&self, id: &str, from: &[&str], to: &str, input_requests: &Value, result: Option<&Value>, error: Option<&str>) -> Result<bool, String> {
        let from: Vec<String> = from.iter().map(|s| s.to_string()).collect();
        sqlx::query(
            "UPDATE mcp_tasks SET status = $3, input_requests = $4, result = $5, error = $6, updated_at = now() \
             WHERE id = $1 AND status = ANY($2)",
        )
        .bind(id)
        .bind(&from)
        .bind(to)
        .bind(sqlx::types::Json(input_requests))
        .bind(result.map(sqlx::types::Json))
        .bind(error)
        .execute(&self.db)
        .await
        .map(|r| r.rows_affected() == 1)
        .map_err(|e| e.to_string())
    }

    async fn purge_expired(&self, owner_id: &str) -> Result<(), String> {
        sqlx::query("DELETE FROM mcp_tasks WHERE owner_id = $1 AND ttl_ms IS NOT NULL AND created_at + ttl_ms * interval '1 millisecond' < now()")
            .bind(owner_id)
            .execute(&self.db)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

// ─── wire shapes ─────────────────────────────────────────────────────────────

fn iso(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn stored_error(row: &TaskRow) -> Value {
    let text = row.error.clone().unwrap_or_default();
    serde_json::from_str::<Value>(&text).ok().filter(Value::is_object).unwrap_or_else(|| json!({ "code": INTERNAL_ERROR, "message": text }))
}

/// A `working` row's stored progress, if the job reported any.
fn working_field<'a>(row: &'a TaskRow, key: &str) -> Option<&'a Value> {
    row.result.as_ref().filter(|_| row.status == "working").and_then(|r| r.get(key)).filter(|v| !v.is_null())
}

fn status_message(row: &TaskRow) -> String {
    match row.status.as_str() {
        "working" => working_field(row, "statusMessage").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("Running {}.", row.tool)),
        "input_required" if row.input_requests.get(APPROVAL_KEY).is_some() => "Waiting for the owner to approve this app in Allternit.".into(),
        "input_required" => "Waiting for your input.".into(),
        "completed" => "Done.".into(),
        "cancelled" => "Cancelled.".into(),
        _ => stored_error(row)["message"].as_str().unwrap_or("Failed.").to_string(),
    }
}

/// The `Task` object (SEP-2663 field names).
fn task_json(row: &TaskRow) -> Value {
    json!({
        "taskId": row.id,
        "status": row.status,
        "statusMessage": status_message(row),
        "createdAt": iso(row.created_at),
        "lastUpdatedAt": iso(row.updated_at),
        "ttlMs": row.ttl_ms,
        "pollIntervalMs": row.poll_interval_ms,
    })
}

/// `CreateTaskResult`, the `tools/call` answer.
fn create_result(row: &TaskRow) -> Value {
    let mut t = task_json(row);
    t["resultType"] = json!("task");
    t
}

/// `DetailedTask`, the `tasks/get` answer: plus what the status carries.
fn detailed(row: &TaskRow) -> Value {
    let mut t = task_json(row);
    match row.status.as_str() {
        "working" => {
            if let Some(p) = working_field(row, "progress") {
                t["progress"] = p.clone();
            }
        }
        "input_required" => t["inputRequests"] = row.input_requests.clone(),
        "completed" => t["result"] = row.result.clone().unwrap_or_else(|| json!({ "content": [] })),
        "failed" => t["error"] = stored_error(row),
        _ => {}
    }
    t
}

fn is_terminal(status: &str) -> bool {
    matches!(status, "completed" | "cancelled" | "failed")
}

// ─── entry points ────────────────────────────────────────────────────────────

/// Start `tool` as a task and run `job` in the background.
pub async fn start(store: &Arc<dyn TaskStore>, owner_id: &str, tool: &str, job: Job) -> Result<Value, String> {
    if let Err(e) = store.purge_expired(owner_id).await {
        tracing::debug!("mcp edge tasks: purge failed: {e}");
    }
    let row = TaskRow::new(owner_id, tool, "working", json!({}), None);
    store.insert(&row).await?;
    spawn_job(store.clone(), row.id.clone(), job);
    Ok(create_result(&row))
}

/// Called by `mcp_edge::serve` for a `tools/call` on the relay path and for
/// the edge-answered tools. `Some(reply)` = the call is now a task (a
/// JSON-RPC reply carrying `CreateTaskResult`); `None` = answer it inline as
/// before (no declaration, not long-running, Tasks off, or the store failed).
pub async fn intercept_tool_call(tasks: Option<&TaskRuntime>, owner_id: &str, id: &Value, params: &Value, relayed_to_service: bool, job: impl FnOnce(Arc<dyn EdgeBackend>) -> Job) -> Option<Value> {
    let tool = params["name"].as_str().unwrap_or_default();
    let rt = tasks.filter(|_| declares_tasks(params) && is_task_capable(tool, relayed_to_service))?;
    match start(&rt.store, owner_id, tool, job(rt.backend.clone())).await {
        Ok(created) => Some(mcp_protocol::rpc_ok(id, created)),
        Err(e) => {
            tracing::warn!("mcp edge tasks: creating a task failed, answering inline: {e}");
            None
        }
    }
}

/// A `tools/call` held for the owner's approval: `input_required` with a URL
/// elicitation to the approve page; `request` is kept to run once approved.
pub async fn start_awaiting_approval(store: &Arc<dyn TaskStore>, owner_id: &str, request: &Value, message: &str, approve_url: &str) -> Result<Value, String> {
    let tool = request["params"]["name"].as_str().unwrap_or_default();
    let mut row = TaskRow::new(owner_id, tool, "input_required", json!({}), Some(json!({ "pendingCall": request })));
    row.input_requests = json!({ APPROVAL_KEY: {
        "method": "elicitation/create",
        "params": { "mode": "url", "message": message, "url": approve_url, "elicitationId": format!("{}:{APPROVAL_KEY}", row.id) }
    } });
    if let Err(e) = store.purge_expired(owner_id).await {
        tracing::debug!("mcp edge tasks: purge failed: {e}");
    }
    store.insert(&row).await?;
    Ok(create_result(&row))
}

fn not_found(id: &Value) -> Value {
    mcp_protocol::rpc_err(id, INVALID_PARAMS, "Task not found")
}

/// `tasks/get`, `tasks/update`, `tasks/cancel` for `owner_id`. A task id that
/// isn't the caller's is "not found", exactly like one that never existed.
pub async fn handle(store: &Arc<dyn TaskStore>, owner_id: &str, id: &Value, method: &str, params: &Value, resume: &dyn Resume) -> Value {
    let Some(task_id) = params["taskId"].as_str() else {
        return mcp_protocol::rpc_err(id, INVALID_PARAMS, "taskId is required");
    };
    let now = Utc::now();
    let row = match store.get(task_id, owner_id).await {
        Ok(Some(r)) if !r.expired(now) => r,
        Ok(_) => return not_found(id),
        Err(e) => {
            tracing::warn!("mcp edge tasks: lookup failed: {e}");
            return mcp_protocol::rpc_err(id, INTERNAL_ERROR, "Tasks are unavailable right now");
        }
    };
    match method {
        "tasks/get" => {
            if row.status == "working" && (now - row.updated_at).num_milliseconds() > STALE_AFTER_MS {
                let err = json!({ "code": INTERNAL_ERROR, "message": "The task was lost when the server restarted. Try the call again." }).to_string();
                let _ = store.transition(&row.id, &["working"], "failed", &json!({}), None, Some(&err)).await;
            }
            reread(store, id, &row, owner_id).await
        }
        "tasks/cancel" => {
            if is_terminal(&row.status) {
                return mcp_protocol::rpc_err(id, INVALID_PARAMS, format!("Task is already {}", row.status));
            }
            abort_job(&row.id);
            let _ = store.transition(&row.id, &["working", "input_required"], "cancelled", &json!({}), None, None).await;
            reread(store, id, &row, owner_id).await
        }
        "tasks/update" => update(store, id, &row, owner_id, &params["inputResponses"], resume).await,
        _ => mcp_protocol::rpc_err(id, mcp_protocol::codes::METHOD_NOT_FOUND, format!("Method not found: {method}")),
    }
}

async fn reread(store: &Arc<dyn TaskStore>, id: &Value, row: &TaskRow, owner_id: &str) -> Value {
    match store.get(&row.id, owner_id).await {
        Ok(Some(r)) => mcp_protocol::rpc_ok(id, detailed(&r)),
        Ok(None) => not_found(id),
        Err(e) => {
            tracing::warn!("mcp edge tasks: lookup failed: {e}");
            mcp_protocol::rpc_ok(id, detailed(row))
        }
    }
}

async fn update(store: &Arc<dyn TaskStore>, id: &Value, row: &TaskRow, owner_id: &str, responses: &Value, resume: &dyn Resume) -> Value {
    if row.status != "input_required" {
        return mcp_protocol::rpc_err(id, INVALID_PARAMS, "Task is not waiting for input");
    }
    let Some(responses) = responses.as_object() else {
        return mcp_protocol::rpc_err(id, INVALID_PARAMS, "inputResponses is required");
    };
    if let Some(key) = responses.keys().find(|k| row.input_requests.get(k.as_str()).is_none()) {
        return mcp_protocol::rpc_err(id, INVALID_PARAMS, format!("Unknown input request: {key}"));
    }
    let Some(answer) = responses.get(APPROVAL_KEY) else { return reread(store, id, row, owner_id).await };
    let pending = row.result.as_ref().map(|r| r["pendingCall"].clone()).unwrap_or(Value::Null);
    match answer["action"].as_str() {
        Some("accept") => match resume.approved(&pending).await {
            Ok(Some(job)) => {
                if let Ok(true) = store.transition(&row.id, &["input_required"], "working", &json!({}), None, None).await {
                    spawn_job(store.clone(), row.id.clone(), job);
                }
            }
            // Not approved yet: the same request stays open.
            Ok(None) => {}
            Err(e) => {
                tracing::warn!("mcp edge tasks: approval lookup failed: {e}");
                return mcp_protocol::rpc_err(id, INTERNAL_ERROR, "Approvals are unavailable right now");
            }
        },
        _ => {
            let declined = json!({ "content": [{ "type": "text", "text": "The owner did not approve this app, so the call did not run." }], "isError": true });
            let _ = store.transition(&row.id, &["input_required"], "completed", &json!({}), Some(&declined), None).await;
        }
    }
    reread(store, id, row, owner_id).await
}

#[cfg(test)]
pub mod testing {
    use super::*;

    /// `mcp_tasks` in memory.
    #[derive(Default)]
    pub struct MemTaskStore {
        pub rows: Mutex<HashMap<String, TaskRow>>,
    }

    #[async_trait::async_trait]
    impl TaskStore for MemTaskStore {
        async fn insert(&self, row: &TaskRow) -> Result<(), String> {
            self.rows.lock().unwrap().insert(row.id.clone(), row.clone());
            Ok(())
        }
        async fn get(&self, id: &str, owner_id: &str) -> Result<Option<TaskRow>, String> {
            Ok(self.rows.lock().unwrap().get(id).filter(|r| r.owner_id == owner_id).cloned())
        }
        async fn transition(&self, id: &str, from: &[&str], to: &str, input_requests: &Value, result: Option<&Value>, error: Option<&str>) -> Result<bool, String> {
            let mut rows = self.rows.lock().unwrap();
            let Some(r) = rows.get_mut(id).filter(|r| from.contains(&r.status.as_str())) else { return Ok(false) };
            r.status = to.into();
            r.input_requests = input_requests.clone();
            r.result = result.cloned();
            r.error = error.map(str::to_string);
            r.updated_at = Utc::now();
            Ok(true)
        }
        async fn purge_expired(&self, owner_id: &str) -> Result<(), String> {
            let now = Utc::now();
            self.rows.lock().unwrap().retain(|_, r| r.owner_id != owner_id || !r.expired(now));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::MemTaskStore;
    use super::*;
    use mcp_protocol::META_CLIENT_CAPABILITIES;

    const OWNER: &str = "user_a|claude|agents";

    fn store() -> Arc<dyn TaskStore> {
        Arc::new(MemTaskStore::default())
    }

    fn tasks_meta() -> Value {
        json!({ META_CLIENT_CAPABILITIES: { "extensions": { TASKS_EXTENSION_ID: {} }, "elicitation": { "form": {}, "url": {} } } })
    }

    struct NoResume;
    #[async_trait::async_trait]
    impl Resume for NoResume {
        async fn approved(&self, _: &Value) -> Result<Option<Job>, String> {
            Ok(None)
        }
    }

    /// Approved once `flag` is set; the job echoes the pending call's tool.
    struct FlagResume(std::sync::atomic::AtomicBool);
    #[async_trait::async_trait]
    impl Resume for FlagResume {
        async fn approved(&self, pending: &Value) -> Result<Option<Job>, String> {
            if !self.0.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(None);
            }
            let tool = pending["params"]["name"].as_str().unwrap_or_default().to_string();
            Ok(Some(Box::pin(async move { JobOutcome::Result(json!({ "content": [{ "type": "text", "text": format!("ran {tool}") }], "isError": false })) })))
        }
    }

    async fn get(s: &Arc<dyn TaskStore>, owner: &str, task: &str) -> Value {
        handle(s, owner, &json!(2), "tasks/get", &json!({ "taskId": task }), &NoResume).await
    }

    async fn poll_until(s: &Arc<dyn TaskStore>, task: &str, status: &str) -> Value {
        for _ in 0..200 {
            let r = get(s, OWNER, task).await;
            if r["result"]["status"] == status {
                return r;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("task {task} never reached {status}");
    }

    #[test]
    fn declaration_and_list_helpers() {
        assert!(declares_tasks(&json!({ "_meta": tasks_meta() })));
        assert!(supports_url_elicitation(&json!({ "_meta": tasks_meta() })));
        assert!(!declares_tasks(&json!({ "name": "phone_tap" })));
        assert!(!declares_tasks(&json!({ "_meta": { META_CLIENT_CAPABILITIES: { "extensions": { TASKS_EXTENSION_ID: false } } } })));
        assert!(is_long_running("shell.exec") && is_long_running("phone_task") && !is_long_running("phones_list"));
        let stripped = without_tasks(&json!({ "method": "tools/call", "params": { "name": "x", "_meta": tasks_meta() } }));
        assert!(!declares_tasks(&stripped["params"]));
        let mut caps = json!({ "tools": {} });
        advertise(&mut caps);
        assert_eq!(caps["extensions"][TASKS_EXTENSION_ID], json!({}));
    }

    #[tokio::test]
    async fn tasks_create_poll_completed() {
        let s = store();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let job: Job = Box::pin(async move {
            let _ = rx.await;
            JobOutcome::Result(json!({ "content": [{ "type": "text", "text": "tapped" }], "isError": false }))
        });
        let created = start(&s, OWNER, "phone_tap", job).await.unwrap();
        assert_eq!(created["resultType"], "task");
        assert_eq!(created["status"], "working");
        assert_eq!(created["pollIntervalMs"], POLL_INTERVAL_MS);
        let task = created["taskId"].as_str().unwrap().to_string();
        assert_eq!(get(&s, OWNER, &task).await["result"]["status"], "working");
        tx.send(()).unwrap();
        let done = poll_until(&s, &task, "completed").await;
        assert_eq!(done["result"]["result"]["content"][0]["text"], "tapped");
    }

    #[tokio::test]
    async fn tasks_failed_job_carries_the_jsonrpc_error() {
        let s = store();
        let created = start(&s, OWNER, "shell.exec", Box::pin(async { job_error("offline") })).await.unwrap();
        let failed = poll_until(&s, created["taskId"].as_str().unwrap(), "failed").await;
        assert_eq!(failed["result"]["error"]["message"], "offline");
        assert_eq!(failed["result"]["statusMessage"], "offline");
    }

    #[tokio::test]
    async fn tasks_cancel_aborts_the_job_and_stays_cancelled() {
        let s = store();
        let created = start(&s, OWNER, "phone_task", Box::pin(std::future::pending())).await.unwrap();
        let task = created["taskId"].as_str().unwrap();
        let r = handle(&s, OWNER, &json!(3), "tasks/cancel", &json!({ "taskId": task }), &NoResume).await;
        assert_eq!(r["result"]["status"], "cancelled");
        assert!(!running().lock().unwrap().contains_key(task), "the background job was aborted");
        let again = handle(&s, OWNER, &json!(4), "tasks/cancel", &json!({ "taskId": task }), &NoResume).await;
        assert_eq!(again["error"]["code"], INVALID_PARAMS);
        assert_eq!(get(&s, OWNER, task).await["result"]["status"], "cancelled");
    }

    #[tokio::test]
    async fn tasks_input_required_update_completed_for_the_approval_gate() {
        let s = store();
        let request = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "agent_send", "_meta": tasks_meta() } });
        let created = start_awaiting_approval(&s, OWNER, &request, "Approve claude", "https://x/approve").await.unwrap();
        assert_eq!(created["status"], "input_required");
        let task = created["taskId"].as_str().unwrap().to_string();
        let waiting = get(&s, OWNER, &task).await;
        let ask = &waiting["result"]["inputRequests"][APPROVAL_KEY];
        assert_eq!((ask["method"].as_str(), ask["params"]["mode"].as_str(), ask["params"]["url"].as_str()), (Some("elicitation/create"), Some("url"), Some("https://x/approve")));
        assert!(waiting["result"].get("result").is_none(), "the pending call is never shown");

        let resume = FlagResume(Default::default());
        let accept = json!({ "taskId": task, "inputResponses": { APPROVAL_KEY: { "action": "accept" } } });
        // Accepting before the owner approved changes nothing.
        let r = handle(&s, OWNER, &json!(5), "tasks/update", &accept, &resume).await;
        assert_eq!(r["result"]["status"], "input_required");
        resume.0.store(true, std::sync::atomic::Ordering::SeqCst);
        handle(&s, OWNER, &json!(6), "tasks/update", &accept, &resume).await;
        let done = poll_until(&s, &task, "completed").await;
        assert_eq!(done["result"]["result"]["content"][0]["text"], "ran agent_send");

        let bad = handle(&s, OWNER, &json!(7), "tasks/update", &accept, &resume).await;
        assert_eq!(bad["error"]["code"], INVALID_PARAMS, "a finished task takes no input");
    }

    #[tokio::test]
    async fn tasks_decline_completes_with_a_tool_error() {
        let s = store();
        let request = json!({ "params": { "name": "shell.exec" } });
        let task = start_awaiting_approval(&s, OWNER, &request, "m", "u").await.unwrap()["taskId"].as_str().unwrap().to_string();
        let decline = json!({ "taskId": task, "inputResponses": { APPROVAL_KEY: { "action": "decline" } } });
        let r = handle(&s, OWNER, &json!(8), "tasks/update", &decline, &NoResume).await;
        assert_eq!((r["result"]["status"].as_str(), r["result"]["result"]["isError"].as_bool()), (Some("completed"), Some(true)));
    }

    #[tokio::test]
    async fn tasks_another_owners_id_is_not_found() {
        let s = store();
        let task = start(&s, OWNER, "decide", Box::pin(std::future::pending())).await.unwrap()["taskId"].as_str().unwrap().to_string();
        for other in ["user_b|claude|agents", "user_a|other-app|agents", "user_a|claude|bot:b1"] {
            for method in ["tasks/get", "tasks/cancel", "tasks/update"] {
                let r = handle(&s, other, &json!(9), method, &json!({ "taskId": task, "inputResponses": {} }), &NoResume).await;
                assert_eq!(r["error"]["message"], "Task not found", "{other} {method}");
            }
        }
        assert_eq!(get(&s, OWNER, &task).await["result"]["status"], "working", "the owner's task is untouched");
        abort_job(&task);
    }

    #[tokio::test]
    async fn intercept_only_for_declared_long_running_calls() {
        struct Nop;
        #[async_trait::async_trait]
        impl EdgeBackend for Nop {
            async fn verify(&self, _: &str) -> Result<Value, String> {
                Err("no".into())
            }
            async fn runtimes(&self, _: &str) -> Result<Vec<String>, String> {
                Ok(vec![])
            }
            async fn forward(&self, _: &str, _: &str, _: &str, _: &str, _: &[u8]) -> Result<(u16, Vec<u8>), super::super::mcp_edge::Unreached> {
                Ok((200, vec![]))
            }
            async fn is_approved(&self, _: &str, _: &str, _: &str) -> Result<bool, String> {
                Ok(false)
            }
        }
        let rt = TaskRuntime { store: store(), backend: Arc::new(Nop) };
        let job = |_: Arc<dyn EdgeBackend>| -> Job { Box::pin(async { JobOutcome::Result(json!({})) }) };
        let declared = json!({ "name": "http.get", "_meta": tasks_meta() });
        assert_eq!(intercept_tool_call(Some(&rt), OWNER, &json!(1), &declared, false, job).await.unwrap()["result"]["resultType"], "task");
        assert!(intercept_tool_call(Some(&rt), OWNER, &json!(1), &json!({ "name": "http.get" }), false, job).await.is_none(), "no declaration: inline");
        assert!(intercept_tool_call(Some(&rt), OWNER, &json!(1), &json!({ "name": "phones_list", "_meta": tasks_meta() }), false, job).await.is_none(), "not long-running");
        assert!(intercept_tool_call(None, OWNER, &json!(1), &declared, false, job).await.is_none(), "Tasks off");
    }

    #[test]
    fn task_capable_rule_for_relayed_bot_and_connector_tools() {
        // Relayed to a bot or connector: task-capable unless the name says instant.
        for t in ["github__create_issue", "slack__send_message", "run_report", "bot_ask", "linear__sync_all"] {
            assert!(is_task_capable(t, true), "{t} should be a task");
            assert!(!is_task_capable(t, false), "{t} stays inline off the relay path");
        }
        for t in ["github__list_issues", "github__get_user", "drive__read_file", "x__search", "deploy_status", "agents_list", "files.list", ""] {
            assert!(!is_task_capable(t, true), "{t} is instant");
        }
        // The fixed list still counts everywhere.
        assert!(is_task_capable("shell.exec", false) && is_task_capable("phone_tap", true));
    }

    #[tokio::test]
    async fn working_progress_is_returned_by_tasks_get_without_a_migration() {
        let s = store();
        let row = TaskRow::new(OWNER, "slack__send_message", "working", json!({}), Some(working_progress(Duration::from_secs(7))));
        s.insert(&row).await.unwrap();
        let t = get(&s, OWNER, &row.id).await;
        assert_eq!(t["result"]["status"], "working");
        assert_eq!(t["result"]["statusMessage"], "Still running (7s).");
        assert_eq!(t["result"]["progress"]["elapsedMs"], 7000);
        assert!(t["result"].get("result").is_none(), "progress never leaks as the tool result");
        // Finished: the stored tool result wins, no progress shown.
        s.transition(&row.id, &["working"], "completed", &json!({}), Some(&json!({ "content": [] })), None).await.unwrap();
        let done = get(&s, OWNER, &row.id).await;
        assert_eq!(done["result"]["statusMessage"], "Done.");
        assert!(done["result"].get("progress").is_none());
    }
}
