//! In-process managed session worker.
//!
//! `POST /beta/sessions/:id/run`, the public `POST /api/v1/sessions` (with
//! `input`) / `user.message` events, and deployment runs (manual trigger or
//! the cron `deployment_scheduler`) all enqueue a `beta_work_tasks` row.
//! This worker drains that queue inside the API process:
//!
//! 1. Lease the oldest available session- or deployment-tied task without a
//!    `sandbox_image` ([`beta_work_routes::lease_next_task`] with
//!    [`LeaseScope::InProcess`]; sandbox jobs stay with external
//!    `/beta/work` workers). Expired leases (`leased`/`running` past
//!    `lease_expires_at`, e.g. after a restart) are re-leased the same way.
//! 2. Drive one Gizzi turn at `TERMINAL_SERVER_URL` the way
//!    `agent_routes::execute_agent_run` does: the agent's provider/model and
//!    system prompt, plus a `bot` marker whose `allowedTools`/`askTools`
//!    come from the session's `effective_permissions` (the same marker bot
//!    turns carry, see `agent_session_routes::bot_turn_marker`). The Gizzi
//!    session id is kept on `beta_sessions.metadata.gizzi_session_id` so
//!    later turns continue the same conversation.
//! 3. While the turn runs, heartbeat the lease and watch for a stop: the
//!    task cancelled (`/beta/work/:id/stop`, public `user.interrupt`) or a
//!    `user_interrupt` event after the run was requested
//!    (`/beta/sessions/:id/interrupt`). Either aborts the Gizzi turn.
//! 4. Append `tool_calls` (so `ApprovalStore` gating applies) and
//!    `agent_message` events through `beta_session_routes::append_run_event`,
//!    which also enforces the session's execution budget, then ack the task
//!    (`turn_completed` + `session_idle`) or fail it (`turn_failed` +
//!    `session_idle`, with `retryable` and `code` in the event data).
//!
//! Gizzi unreachable fails the task with `code: "gizzi_unreachable"`,
//! `retryable: true` and makes the worker back off (2s doubling to 60s)
//! before it leases again, so a down runtime does not drain the queue.
//!
//! On by default; `ALLTERNIT_SESSION_WORKER=0` turns it off (tasks then wait
//! for an external `/beta/work` worker). `ALLTERNIT_SESSION_WORKER_CONCURRENCY`
//! (default 2) bounds concurrent turns, and
//! `ALLTERNIT_SESSION_WORKER_TURN_TIMEOUT_SECS` (default 600) bounds one turn.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use tokio::sync::{broadcast, Semaphore};
use tracing::{info, warn};

use crate::beta_session_routes::{self as beta, UsageDelta};
use crate::beta_work_routes::{self as work, LeaseScope, TaskRow};
use crate::error::ApiError;
use crate::AppState;

const ENABLE_ENV: &str = "ALLTERNIT_SESSION_WORKER";
const CONCURRENCY_ENV: &str = "ALLTERNIT_SESSION_WORKER_CONCURRENCY";
const TURN_TIMEOUT_ENV: &str = "ALLTERNIT_SESSION_WORKER_TURN_TIMEOUT_SECS";

/// Whether this node runs the in-process worker (`ALLTERNIT_SESSION_WORKER`
/// unset or anything but `0`/`false`/`off`/`no`).
pub fn enabled() -> bool {
    !matches!(
        std::env::var(ENABLE_ENV)
            .ok()
            .map(|v| v.trim().to_ascii_lowercase())
            .as_deref(),
        Some("0") | Some("false") | Some("off") | Some("no")
    )
}

/// `executor` in the `/run` response: who will execute a queued task.
pub(crate) fn executor_label() -> &'static str {
    if enabled() {
        "in_process"
    } else {
        "external"
    }
}

#[derive(Clone, Debug)]
pub struct WorkerConfig {
    pub worker_id: String,
    pub gizzi_url: String,
    pub concurrency: usize,
    /// Sleep between lease attempts when the queue is empty.
    pub poll_interval: Duration,
    /// How often a running turn checks for stop/interrupt.
    pub control_interval: Duration,
    /// How often a running turn renews its lease (well under `LEASE_SECONDS`).
    pub heartbeat_interval: Duration,
    pub turn_timeout: Duration,
}

impl WorkerConfig {
    pub fn from_env(state: &AppState) -> Self {
        let env_u64 = |name: &str| std::env::var(name).ok().and_then(|v| v.trim().parse::<u64>().ok());
        Self {
            worker_id: format!("in-process-{}", uuid::Uuid::new_v4()),
            gizzi_url: state.config.terminal_server_url().trim_end_matches('/').to_string(),
            concurrency: env_u64(CONCURRENCY_ENV).unwrap_or(2).clamp(1, 64) as usize,
            poll_interval: Duration::from_secs(1),
            control_interval: Duration::from_secs(1),
            heartbeat_interval: Duration::from_secs((work::LEASE_SECONDS as u64 / 3).max(1)),
            turn_timeout: Duration::from_secs(env_u64(TURN_TIMEOUT_ENV).unwrap_or(600).max(1)),
        }
    }
}

/// How one leased task ended.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TaskOutcome {
    Succeeded,
    Failed { code: &'static str, error: String, retryable: bool },
    /// Stopped or interrupted while running.
    Cancelled,
    /// The lease was lost (expired and re-leased); the new holder owns it.
    LostLease,
}

#[derive(Debug)]
struct Failure {
    code: &'static str,
    error: String,
    retryable: bool,
}

impl Failure {
    fn new(code: &'static str, error: impl Into<String>, retryable: bool) -> Self {
        Self { code, error: error.into(), retryable }
    }
}

/// Exponential backoff after Gizzi was unreachable: 2s doubling to 60s,
/// reset by the next turn that reaches Gizzi.
#[derive(Default)]
struct GizziBackoff {
    failures: AtomicU32,
    until: Mutex<Option<Instant>>,
}

impl GizziBackoff {
    fn observe(&self, outcome: &TaskOutcome) {
        match outcome {
            TaskOutcome::Failed { code: "gizzi_unreachable", .. } => {
                let n = self.failures.fetch_add(1, Ordering::SeqCst).min(5);
                let delay = Duration::from_secs((2u64 << n).min(60));
                warn!(?delay, "session worker: gizzi unreachable, backing off");
                *self.until.lock().unwrap() = Some(Instant::now() + delay);
            }
            TaskOutcome::Succeeded | TaskOutcome::Failed { .. } => {
                self.failures.store(0, Ordering::SeqCst);
                *self.until.lock().unwrap() = None;
            }
            _ => {}
        }
    }

    fn remaining(&self) -> Option<Duration> {
        let until = (*self.until.lock().unwrap())?;
        until.checked_duration_since(Instant::now())
    }
}

/// Spawn the worker unless `ALLTERNIT_SESSION_WORKER=0`.
pub fn spawn_session_worker(
    state: Arc<AppState>,
    shutdown_rx: broadcast::Receiver<()>,
) -> Option<tokio::task::JoinHandle<()>> {
    if !enabled() {
        info!("Session worker disabled ({ENABLE_ENV}=0); session runs wait for an external /beta/work worker");
        return None;
    }
    let config = WorkerConfig::from_env(&state);
    info!(
        worker_id = %config.worker_id,
        gizzi = %config.gizzi_url,
        concurrency = config.concurrency,
        "Starting in-process session worker"
    );
    Some(tokio::spawn(run_worker(state, config, shutdown_rx)))
}

pub async fn run_worker(
    state: Arc<AppState>,
    config: WorkerConfig,
    mut shutdown_rx: broadcast::Receiver<()>,
) {
    let config = Arc::new(config);
    let permits = Arc::new(Semaphore::new(config.concurrency.max(1)));
    let backoff = Arc::new(GizziBackoff::default());
    loop {
        if let Some(wait) = backoff.remaining() {
            tokio::select! {
                _ = shutdown_rx.recv() => break,
                _ = tokio::time::sleep(wait) => {}
            }
        }
        let permit = tokio::select! {
            _ = shutdown_rx.recv() => break,
            permit = permits.clone().acquire_owned() => match permit {
                Ok(permit) => permit,
                Err(_) => break,
            },
        };
        match lease(&state, &config.worker_id).await {
            Ok(Some(task)) => {
                let state = state.clone();
                let config = config.clone();
                let backoff = backoff.clone();
                tokio::spawn(async move {
                    let task_id = task.id.clone();
                    let outcome = execute_task(&state, &config, task).await;
                    info!(task_id = %task_id, ?outcome, "session worker finished task");
                    backoff.observe(&outcome);
                    drop(permit);
                });
                continue;
            }
            Ok(None) => {}
            Err(e) => warn!(error = %e, "session worker: lease failed"),
        }
        drop(permit);
        tokio::select! {
            _ = shutdown_rx.recv() => break,
            _ = tokio::time::sleep(config.poll_interval) => {}
        }
    }
    info!("Session worker stopped");
}

/// Lease one task and execute it inline. `None` when nothing was available.
pub(crate) async fn run_once(state: &Arc<AppState>, config: &WorkerConfig) -> Option<TaskOutcome> {
    let task = lease(state, &config.worker_id).await.ok().flatten()?;
    Some(execute_task(state, config, task).await)
}

async fn lease(state: &Arc<AppState>, worker_id: &str) -> Result<Option<TaskRow>, String> {
    let db = state.db.clone();
    let worker_id = worker_id.to_string();
    tokio::task::spawn_blocking(move || {
        let mut conn = db.connect()?;
        work::lease_next_task(&mut conn, LeaseScope::InProcess, &worker_id, Utc::now())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

async fn blocking<T, F>(state: &Arc<AppState>, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&mut rusqlite::Connection) -> rusqlite::Result<T> + Send + 'static,
{
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        let mut conn = db.connect()?;
        f(&mut conn)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

// ─── Run context ─────────────────────────────────────────────────────────────

#[derive(Debug)]
struct AgentRow {
    id: String,
    name: String,
    model: String,
    provider: String,
    system_prompt: Option<String>,
    /// The agent's `config` JSON (`{}` when unset): the bot's own brain
    /// accounts ride from it onto the turn's marker.
    config: Value,
}

#[derive(Debug)]
struct RunContext {
    user_id: String,
    organization_id: Option<String>,
    agent: AgentRow,
    effective_permissions: Value,
    gizzi_session_id: Option<String>,
    /// `user_interrupt` events after this sequence stop the run.
    interrupt_baseline: i64,
    /// Deployment-tied tasks: the `beta_deployment_runs` row to finish, and
    /// `metadata.prompt` as the prompt when the payload carries none.
    deployment_run_id: Option<String>,
    fallback_prompt: Option<String>,
}

fn load_context(conn: &rusqlite::Connection, task: &TaskRow) -> rusqlite::Result<Result<RunContext, Failure>> {
    let user_id: String = conn.query_row(
        "SELECT user_id FROM beta_work_tasks WHERE id = ?1",
        params![task.id],
        |row| row.get(0),
    )?;
    let organization_id: Option<String> = conn
        .query_row(
            "SELECT organization_id FROM users WHERE id = ?1",
            params![user_id],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten()
        .flatten();
    let parse = |raw: Option<String>| {
        raw.and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .unwrap_or_else(|| json!({}))
    };

    let mut ctx_agent_id: Option<String> = task
        .payload
        .get("agent_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut effective_permissions = json!({});
    let mut gizzi_session_id = None;
    let mut interrupt_baseline = 0;
    let mut deployment_run_id = None;
    let mut fallback_prompt = None;

    if let Some(session_id) = &task.session_id {
        let session: Option<(Option<String>, String, Option<String>)> = conn
            .query_row(
                "SELECT agent_id, status, metadata FROM beta_sessions WHERE id = ?1",
                params![session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((agent_id, status, metadata)) = session else {
            return Ok(Err(Failure::new("session_not_found", "session not found", false)));
        };
        if status != "active" {
            return Ok(Err(Failure::new("session_archived", "session is archived", false)));
        }
        if let Some(resource) = beta::run_budget_exhausted(conn, session_id)? {
            return Ok(Err(Failure::new(
                "budget_exhausted",
                format!("session budget exhausted ({resource})"),
                false,
            )));
        }
        let metadata = parse(metadata);
        effective_permissions = metadata
            .get("effective_permissions")
            .cloned()
            .unwrap_or_else(|| json!({}));
        gizzi_session_id = metadata
            .get("gizzi_session_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        ctx_agent_id = agent_id.or(ctx_agent_id);
        // Interrupts count from the run's own `run_requested` event (so one
        // sent while the task was still queued applies); tasks enqueued
        // without one count from now.
        interrupt_baseline = conn
            .query_row(
                "SELECT sequence FROM beta_session_events
                 WHERE session_id = ?1 AND event_type = 'run_requested'
                   AND json_extract(data, '$.task_id') = ?2
                 ORDER BY sequence DESC LIMIT 1",
                params![session_id, task.id],
                |row| row.get(0),
            )
            .optional()?
            .map_or_else(
                || {
                    conn.query_row(
                        "SELECT COALESCE(MAX(sequence), 0) FROM beta_session_events WHERE session_id = ?1",
                        params![session_id],
                        |row| row.get(0),
                    )
                },
                Ok,
            )?;
    } else if let Some(deployment_id) = &task.deployment_id {
        let deployment: Option<(Option<String>, Option<String>)> = conn
            .query_row(
                "SELECT agent_id, metadata FROM beta_deployments WHERE id = ?1",
                params![deployment_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((agent_id, metadata)) = deployment else {
            return Ok(Err(Failure::new("deployment_not_found", "deployment not found", false)));
        };
        ctx_agent_id = ctx_agent_id.or(agent_id);
        fallback_prompt = parse(metadata)
            .get("prompt")
            .and_then(Value::as_str)
            .map(str::to_string);
        deployment_run_id = task
            .payload
            .get("deployment_run_id")
            .and_then(Value::as_str)
            .map(str::to_string);
    }

    let Some(agent_id) = ctx_agent_id else {
        return Ok(Err(Failure::new("no_agent", "task has no agent to run", false)));
    };
    let agent = conn
        .query_row(
            "SELECT id, name, model, provider, system_prompt, config FROM agents WHERE id = ?1 AND user_id = ?2",
            params![agent_id, user_id],
            |row| {
                Ok(AgentRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    model: row.get(2)?,
                    provider: row.get(3)?,
                    system_prompt: row.get(4)?,
                    config: parse(row.get(5)?),
                })
            },
        )
        .optional()?;
    let Some(agent) = agent else {
        return Ok(Err(Failure::new("agent_not_found", format!("agent {agent_id} not found"), false)));
    };
    Ok(Ok(RunContext {
        user_id,
        organization_id,
        agent,
        effective_permissions,
        gizzi_session_id,
        interrupt_baseline,
        deployment_run_id,
        fallback_prompt,
    }))
}

// ─── Prompt + Gizzi payload ──────────────────────────────────────────────────

fn content_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str).or_else(|| p.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// The turn's user text and extra system text from the task payload:
/// `system` messages join the system prompt; one user message is sent as
/// is, several messages are sent as a `role: content` transcript.
fn build_turn(payload: &Value, fallback_prompt: Option<&str>) -> Result<(String, Vec<String>), Failure> {
    let mut system = Vec::new();
    let mut turns: Vec<(String, String)> = Vec::new();
    for message in payload.get("messages").and_then(Value::as_array).into_iter().flatten() {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("user");
        let text = content_text(message.get("content").unwrap_or(&Value::Null));
        if text.trim().is_empty() {
            continue;
        }
        if role == "system" {
            system.push(text);
        } else {
            turns.push((role.to_string(), text));
        }
    }
    if let Some(memory) = payload.get("memory_context").filter(|m| !m.is_null()) {
        system.push(format!("Memory context (bound memory stores, JSON):\n{memory}"));
    }
    let text = match turns.as_slice() {
        [] => match fallback_prompt.map(str::trim).filter(|p| !p.is_empty()) {
            Some(prompt) => prompt.to_string(),
            None => return Err(Failure::new("empty_prompt", "task has no user message to run", false)),
        },
        [(role, text)] if role == "user" => text.clone(),
        many => many
            .iter()
            .map(|(role, text)| format!("{role}: {text}"))
            .collect::<Vec<_>>()
            .join("\n\n"),
    };
    Ok((text, system))
}

/// The `bot` marker (gizzi `runtime/bots/bot-turn.ts`) for this run: the
/// tool allowlist is the run's `tools` when given, else every tool in
/// `effective_permissions`; `askTools` are the tools whose effective
/// permission is `ask`. An empty allowlist means "no allowlist". The bot's
/// own accounts (`cliAccountId` / `subsAccountId`) come from its config.
fn bot_marker(agent: &AgentRow, effective_permissions: &Value, payload_tools: Option<&Value>) -> Value {
    let perms = effective_permissions.as_object();
    let mut allowed: Vec<String> = payload_tools
        .and_then(Value::as_array)
        .map(|tools| tools.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .filter(|tools: &Vec<String>| !tools.is_empty())
        .unwrap_or_else(|| perms.map(|m| m.keys().cloned().collect()).unwrap_or_default());
    allowed.sort();
    allowed.dedup();
    let mut ask: Vec<String> = perms
        .map(|m| {
            m.iter()
                .filter(|(_, v)| v.as_str() == Some("ask"))
                .map(|(k, _)| k.clone())
                .collect()
        })
        .unwrap_or_default();
    ask.sort();
    let mut marker = json!({
        "id": agent.id,
        "name": agent.name,
        "allowedTools": allowed,
        "askTools": ask,
        "gated": false,
    });
    // The bot's own CLI / subscription accounts, as on every other bot turn.
    crate::agent_session_routes::bot_account_fields(&agent.config, &mut marker);
    marker
}

/// `(providerID, modelID)`: an inline Cloud Agent stores `provider/model`
/// in `model` under the placeholder provider `allternit`.
fn model_ref(agent: &AgentRow) -> (String, String) {
    let provider = agent.provider.trim();
    if provider.is_empty() || provider == "allternit" {
        if let Some((p, m)) = agent.model.split_once('/') {
            return (p.to_string(), m.to_string());
        }
    }
    (provider.to_string(), agent.model.clone())
}

fn message_payload(ctx: &RunContext, payload: &Value, text: &str, extra_system: &[String]) -> Value {
    let (provider, model) = model_ref(&ctx.agent);
    let mut body = json!({
        "model": {"providerID": provider, "modelID": model},
        "agent": "build",
        "parts": [{"type": "text", "text": text}],
        "bot": bot_marker(&ctx.agent, &ctx.effective_permissions, payload.get("tools")),
    });
    let mut system: Vec<String> = ctx
        .agent
        .system_prompt
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .into_iter()
        .collect();
    system.extend(extra_system.iter().cloned());
    if !system.is_empty() {
        // "+" appends to gizzi's assembled system prompt (execute_agent_run).
        body["system"] = json!(format!("+{}", system.join("\n\n")));
    }
    body
}

// ─── Gizzi client ────────────────────────────────────────────────────────────

#[derive(Debug)]
enum GizziError {
    Unreachable(String),
    Timeout,
    Http(u16, String),
    Decode(String),
}

impl GizziError {
    fn into_failure(self, gizzi_url: &str) -> Failure {
        match self {
            GizziError::Unreachable(e) => Failure::new(
                "gizzi_unreachable",
                format!("Gizzi runtime unreachable at {gizzi_url} ({e}); start it (or set TERMINAL_SERVER_URL) and run the session again"),
                true,
            ),
            GizziError::Timeout => Failure::new("gizzi_timeout", "Gizzi turn timed out", true),
            GizziError::Http(status, body) => Failure::new(
                "gizzi_error",
                format!("Gizzi returned {status}: {}", body.chars().take(500).collect::<String>()),
                status >= 500,
            ),
            GizziError::Decode(e) => Failure::new("gizzi_error", format!("Gizzi response decode: {e}"), false),
        }
    }
}

async fn gizzi_post(
    client: &reqwest::Client,
    url: String,
    agent_id: &str,
    task_id: &str,
    body: &Value,
) -> Result<Value, GizziError> {
    let response = client
        .post(url)
        .header("x-allternit-agent-id", agent_id)
        .header("x-allternit-run-id", task_id)
        .json(body)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                GizziError::Timeout
            } else {
                GizziError::Unreachable(e.to_string())
            }
        })?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(GizziError::Http(status.as_u16(), text));
    }
    response.json::<Value>().await.map_err(|e| GizziError::Decode(e.to_string()))
}

async fn create_gizzi_session(
    client: &reqwest::Client,
    gizzi: &str,
    agent: &AgentRow,
    task_id: &str,
) -> Result<String, GizziError> {
    let session = gizzi_post(
        client,
        format!("{gizzi}/v1/session"),
        &agent.id,
        task_id,
        &json!({"title": format!("Agent session: {}", agent.name), "surface": "chat"}),
    )
    .await?;
    session
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| GizziError::Decode("gizzi returned no session id".into()))
}

/// Post the turn on `session_id`; a reused session Gizzi no longer knows
/// (404, e.g. Gizzi restarted with a fresh store) is replaced once.
/// Returns the Gizzi session id actually used and the assistant message.
async fn drive_turn(
    client: &reqwest::Client,
    gizzi: &str,
    agent: &AgentRow,
    task_id: &str,
    session_id: String,
    reused: bool,
    body: &Value,
) -> Result<(String, Value), GizziError> {
    let url = |sid: &str| format!("{gizzi}/v1/session/{}/message", urlencoding::encode(sid));
    match gizzi_post(client, url(&session_id), &agent.id, task_id, body).await {
        Err(GizziError::Http(404, _)) if reused => {
            let fresh = create_gizzi_session(client, gizzi, agent, task_id).await?;
            let message = gizzi_post(client, url(&fresh), &agent.id, task_id, body).await?;
            Ok((fresh, message))
        }
        other => other.map(|message| (session_id, message)),
    }
}

async fn abort_gizzi(gizzi: &str, session_id: &str) {
    let Ok(client) = reqwest::Client::builder().timeout(Duration::from_secs(5)).build() else {
        return;
    };
    let _ = client
        .post(format!("{gizzi}/v1/session/{}/abort", urlencoding::encode(session_id)))
        .json(&json!({}))
        .send()
        .await;
}

// ─── Control (heartbeat / stop / interrupt) ──────────────────────────────────

#[derive(Debug, PartialEq)]
enum Control {
    Continue,
    Cancelled,
    Interrupted,
    Lost,
}

fn check_control(
    conn: &rusqlite::Connection,
    task_id: &str,
    worker_id: &str,
    session_id: Option<&str>,
    interrupt_baseline: i64,
    heartbeat: bool,
) -> rusqlite::Result<Control> {
    let row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT status, lease_worker_id FROM beta_work_tasks WHERE id = ?1",
            params![task_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    match row {
        None => return Ok(Control::Lost),
        Some((status, _)) if status == "cancelled" => return Ok(Control::Cancelled),
        Some((status, holder))
            if !matches!(status.as_str(), "leased" | "running") || holder.as_deref() != Some(worker_id) =>
        {
            return Ok(Control::Lost)
        }
        _ => {}
    }
    if let Some(session_id) = session_id {
        let interrupted: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM beta_session_events
             WHERE session_id = ?1 AND event_type = 'user_interrupt' AND sequence > ?2)",
            params![session_id, interrupt_baseline],
            |row| row.get(0),
        )?;
        if interrupted {
            return Ok(Control::Interrupted);
        }
    }
    if heartbeat && work::heartbeat_task_tx(conn, task_id, None, worker_id)?.is_none() {
        return Ok(Control::Lost);
    }
    Ok(Control::Continue)
}

async fn control(
    state: &Arc<AppState>,
    config: &WorkerConfig,
    task: &TaskRow,
    baseline: i64,
    heartbeat: bool,
) -> Control {
    let task_id = task.id.clone();
    let worker_id = config.worker_id.clone();
    let session_id = task.session_id.clone();
    match blocking(state, move |conn| {
        check_control(conn, &task_id, &worker_id, session_id.as_deref(), baseline, heartbeat)
    })
    .await
    {
        Ok(control) => control,
        Err(e) => {
            // A transient DB error is not a stop; the lease still has time.
            warn!(error = %e, "session worker: control check failed");
            Control::Continue
        }
    }
}

// ─── Execution ───────────────────────────────────────────────────────────────

async fn execute_task(state: &Arc<AppState>, config: &WorkerConfig, task: TaskRow) -> TaskOutcome {
    let started = Instant::now();
    let ctx = {
        let task_for_ctx = task.clone();
        match blocking(state, move |conn| load_context(conn, &task_for_ctx)).await {
            Ok(Ok(ctx)) => ctx,
            Ok(Err(failure)) => return fail(state, config, &task, None, failure).await,
            Err(e) => return fail(state, config, &task, None, Failure::new("internal", e, true)).await,
        }
    };

    // First heartbeat marks the task `running` (and catches a stop that
    // landed between lease and start).
    match control(state, config, &task, ctx.interrupt_baseline, true).await {
        Control::Continue => {}
        other => return stopped(state, config, &task, &ctx, other).await,
    }

    let (text, extra_system) = match build_turn(&task.payload, ctx.fallback_prompt.as_deref()) {
        Ok(turn) => turn,
        Err(failure) => return fail(state, config, &task, Some(&ctx), failure).await,
    };
    let body = message_payload(&ctx, &task.payload, &text, &extra_system);
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(config.turn_timeout)
        .build()
    {
        Ok(client) => client,
        Err(e) => return fail(state, config, &task, Some(&ctx), Failure::new("internal", e.to_string(), true)).await,
    };

    let gizzi = config.gizzi_url.as_str();
    let (session_id, reused) = match ctx.gizzi_session_id.clone() {
        Some(id) => (id, true),
        None => match create_gizzi_session(&client, gizzi, &ctx.agent, &task.id).await {
            Ok(id) => (id, false),
            Err(e) => return fail(state, config, &task, Some(&ctx), e.into_failure(gizzi)).await,
        },
    };

    let turn = drive_turn(&client, gizzi, &ctx.agent, &task.id, session_id.clone(), reused, &body);
    tokio::pin!(turn);
    let mut ticker = tokio::time::interval(config.control_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await;
    let mut last_heartbeat = Instant::now();
    let result = loop {
        tokio::select! {
            result = &mut turn => break result,
            _ = ticker.tick() => {
                let heartbeat = last_heartbeat.elapsed() >= config.heartbeat_interval;
                match control(state, config, &task, ctx.interrupt_baseline, heartbeat).await {
                    Control::Continue => if heartbeat { last_heartbeat = Instant::now(); },
                    other => {
                        // Dropping `turn` cancels the HTTP request; abort
                        // tells Gizzi to stop the model loop as well.
                        abort_gizzi(gizzi, &session_id).await;
                        return stopped(state, config, &task, &ctx, other).await;
                    }
                }
            }
        }
    };

    let (used_session_id, message) = match result {
        Ok(ok) => ok,
        Err(e) => return fail(state, config, &task, Some(&ctx), e.into_failure(gizzi)).await,
    };
    if let Some(session_id) = task.session_id.clone() {
        if ctx.gizzi_session_id.as_deref() != Some(used_session_id.as_str()) {
            let gizzi_session_id = used_session_id.clone();
            if let Err(e) = blocking(state, move |conn| {
                conn.execute(
                    "UPDATE beta_sessions SET metadata = json_set(COALESCE(metadata, '{}'), '$.gizzi_session_id', ?1)
                     WHERE id = ?2",
                    params![gizzi_session_id, session_id],
                )
            })
            .await
            {
                warn!(error = %e, "session worker: could not record gizzi session id");
            }
        }
    }

    // Brain/provider errors (auth, unknown model) fail the turn.
    if let Some(err) = message.pointer("/info/error").filter(|e| !e.is_null()) {
        let msg = err
            .get("message")
            .or_else(|| err.pointer("/data/message"))
            .and_then(Value::as_str)
            .unwrap_or("gizzi brain error");
        return fail(state, config, &task, Some(&ctx), Failure::new("brain_error", msg, false)).await;
    }

    let parts = message.get("parts").and_then(Value::as_array).cloned().unwrap_or_default();
    let output: String = parts
        .iter()
        .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|p| p.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    let tool_calls: Vec<Value> = parts
        .iter()
        .filter(|p| p.get("type").and_then(Value::as_str) == Some("tool"))
        .map(|p| {
            json!({
                "id": p.get("callID").or_else(|| p.get("id")).cloned().unwrap_or(Value::Null),
                "name": p.get("tool").cloned().unwrap_or(Value::Null),
                "input": p.pointer("/state/input").cloned().unwrap_or(Value::Null),
                "status": p.pointer("/state/status").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();
    let tokens = message.pointer("/info/tokens");
    let token_count = ["input", "output", "reasoning"]
        .iter()
        .filter_map(|k| tokens.and_then(|t| t.get(*k)).and_then(Value::as_u64))
        .sum::<u64>();

    // Stop/interrupt that raced the end of the turn wins over the result.
    match control(state, config, &task, ctx.interrupt_baseline, false).await {
        Control::Continue => {}
        other => return stopped(state, config, &task, &ctx, other).await,
    }

    if let Some(session_id) = &task.session_id {
        let mut events = Vec::new();
        if !tool_calls.is_empty() {
            events.push((
                "tool_calls",
                json!({"task_id": task.id, "tool_calls": tool_calls}),
                UsageDelta { tokens: 0, turns: 0, tool_calls: tool_calls.len() as u64 },
            ));
        }
        events.push((
            "agent_message",
            json!({
                "task_id": task.id,
                "role": "assistant",
                "content": output,
                "gizzi_session_id": used_session_id,
                "tokens": tokens.cloned().unwrap_or(Value::Null),
            }),
            UsageDelta { tokens: token_count, turns: 1, tool_calls: 0 },
        ));
        for (event_type, data, usage) in events {
            match beta::append_run_event(
                state.clone(),
                ctx.user_id.clone(),
                ctx.organization_id.clone(),
                session_id.clone(),
                event_type.to_string(),
                data,
                usage,
            )
            .await
            {
                Ok((true, _)) => {}
                Ok((false, event)) => {
                    let resource = event.pointer("/data/resource").and_then(Value::as_str).unwrap_or("budget");
                    return fail(
                        state,
                        config,
                        &task,
                        Some(&ctx),
                        Failure::new("budget_exceeded", format!("session budget exceeded ({resource})"), false),
                    )
                    .await;
                }
                Err(ApiError::NotFound(_)) => {
                    return fail(
                        state,
                        config,
                        &task,
                        Some(&ctx),
                        Failure::new("session_archived", "session is no longer active", false),
                    )
                    .await;
                }
                Err(e) => {
                    return fail(state, config, &task, Some(&ctx), Failure::new("internal", e.to_string(), true)).await;
                }
            }
        }
    }

    let result = json!({
        "output": output,
        "gizzi_session_id": used_session_id,
        "usage": {"tokens": token_count, "tool_calls": tool_calls.len()},
        "duration_ms": started.elapsed().as_millis() as u64,
    });
    let task_id = task.id.clone();
    let worker_id = config.worker_id.clone();
    let run_id = ctx.deployment_run_id.clone();
    let result_for_run = result.clone();
    match blocking(state, move |conn| {
        let acked = work::ack_task_tx(conn, &task_id, None, &worker_id, Some(result))?;
        if acked.is_some() {
            if let Some(run_id) = run_id {
                finish_deployment_run(conn, &run_id, "succeeded", Some(&result_for_run), None)?;
            }
        }
        Ok(acked.is_some())
    })
    .await
    {
        Ok(true) => TaskOutcome::Succeeded,
        Ok(false) => TaskOutcome::LostLease,
        Err(e) => {
            warn!(error = %e, "session worker: ack failed");
            TaskOutcome::LostLease
        }
    }
}

fn finish_deployment_run(
    conn: &rusqlite::Connection,
    run_id: &str,
    status: &str,
    result: Option<&Value>,
    error: Option<&str>,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE beta_deployment_runs SET status = ?1, result = ?2, error = ?3,
         finished_at = CURRENT_TIMESTAMP WHERE id = ?4 AND status = 'running'",
        params![status, result.map(Value::to_string), error, run_id],
    )?;
    Ok(())
}

/// Fail the task (`turn_failed` + `session_idle` with `code`/`retryable`).
async fn fail(
    state: &Arc<AppState>,
    config: &WorkerConfig,
    task: &TaskRow,
    ctx: Option<&RunContext>,
    failure: Failure,
) -> TaskOutcome {
    warn!(task_id = %task.id, code = failure.code, error = %failure.error, "session worker: task failed");
    let task_id = task.id.clone();
    let worker_id = config.worker_id.clone();
    let run_id = ctx.and_then(|c| c.deployment_run_id.clone()).or_else(|| {
        task.payload
            .get("deployment_run_id")
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    let error = failure.error.clone();
    let detail = json!({"code": failure.code, "retryable": failure.retryable});
    let ended = blocking(state, move |conn| {
        let affected = work::stop_task_tx(conn, &task_id, None, Some(&worker_id), "failed", Some(&error), detail)?;
        if affected > 0 {
            if let Some(run_id) = run_id {
                finish_deployment_run(conn, &run_id, "failed", None, Some(&error))?;
            }
        }
        Ok(affected)
    })
    .await;
    match ended {
        Ok(0) => TaskOutcome::LostLease,
        Ok(_) => TaskOutcome::Failed { code: failure.code, error: failure.error, retryable: failure.retryable },
        Err(e) => {
            warn!(error = %e, "session worker: could not record task failure");
            TaskOutcome::LostLease
        }
    }
}

/// The run was stopped while executing. An interrupt event ends the task as
/// `cancelled` here (with `turn_failed` + `session_idle`); a cancelled task
/// already had its terminal events written by whoever cancelled it.
async fn stopped(
    state: &Arc<AppState>,
    config: &WorkerConfig,
    task: &TaskRow,
    ctx: &RunContext,
    control: Control,
) -> TaskOutcome {
    match control {
        Control::Interrupted => {
            let task_id = task.id.clone();
            let worker_id = config.worker_id.clone();
            let run_id = ctx.deployment_run_id.clone();
            let ended = blocking(state, move |conn| {
                let affected = work::stop_task_tx(
                    conn,
                    &task_id,
                    None,
                    Some(&worker_id),
                    "cancelled",
                    Some("interrupted"),
                    json!({"code": "interrupted", "retryable": false}),
                )?;
                if affected > 0 {
                    if let Some(run_id) = run_id {
                        finish_deployment_run(conn, &run_id, "cancelled", None, Some("interrupted"))?;
                    }
                }
                Ok(affected)
            })
            .await;
            match ended {
                Ok(n) if n > 0 => TaskOutcome::Cancelled,
                _ => TaskOutcome::LostLease,
            }
        }
        Control::Cancelled => TaskOutcome::Cancelled,
        Control::Lost | Control::Continue => TaskOutcome::LostLease,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::beta_session_routes::tests as beta_test;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::Router;
    use http_body_util::BodyExt;
    use std::sync::atomic::AtomicUsize;
    use tower::ServiceExt;

    /// A fake Gizzi runtime: `POST /v1/session`, `/v1/session/:id/message`
    /// (records the body; `hang` never answers) and `/v1/session/:id/abort`.
    #[derive(Clone, Default)]
    struct FakeGizzi {
        hang: bool,
        bodies: Arc<Mutex<Vec<Value>>>,
        sessions: Arc<AtomicUsize>,
        aborts: Arc<AtomicUsize>,
    }

    async fn start_fake_gizzi(hang: bool) -> (String, FakeGizzi) {
        use axum::routing::post;
        let fake = FakeGizzi { hang, ..Default::default() };
        let app = Router::new()
            .route(
                "/v1/session",
                post(|axum::extract::State(fake): axum::extract::State<FakeGizzi>| async move {
                    let n = fake.sessions.fetch_add(1, Ordering::SeqCst) + 1;
                    axum::Json(json!({"id": format!("gz-{n}")}))
                }),
            )
            .route(
                "/v1/session/:id/message",
                post(
                    |axum::extract::State(fake): axum::extract::State<FakeGizzi>,
                     axum::extract::Path(id): axum::extract::Path<String>,
                     axum::Json(body): axum::Json<Value>| async move {
                        fake.bodies.lock().unwrap().push(body);
                        if fake.hang {
                            tokio::time::sleep(Duration::from_secs(60)).await;
                        }
                        axum::Json(json!({
                            "info": {"id": "msg-1", "sessionID": id, "role": "assistant",
                                     "tokens": {"input": 12, "output": 8}},
                            "parts": [
                                {"type": "tool", "tool": "bash", "callID": "call-1",
                                 "state": {"status": "completed", "input": {"command": "ls"}}},
                                {"type": "text", "text": "hello from gizzi"}
                            ]
                        }))
                    },
                ),
            )
            .route(
                "/v1/session/:id/abort",
                post(|axum::extract::State(fake): axum::extract::State<FakeGizzi>| async move {
                    fake.aborts.fetch_add(1, Ordering::SeqCst);
                    axum::Json(json!({"ok": true}))
                }),
            )
            .with_state(fake.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}"), fake)
    }

    fn config(gizzi_url: &str) -> WorkerConfig {
        WorkerConfig {
            worker_id: "test-worker".into(),
            gizzi_url: gizzi_url.into(),
            concurrency: 1,
            poll_interval: Duration::from_millis(50),
            control_interval: Duration::from_millis(50),
            heartbeat_interval: Duration::from_millis(100),
            turn_timeout: Duration::from_secs(10),
        }
    }

    async fn send(router: &Router, method: &str, path: &str, body: &Value) -> (StatusCode, Value) {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("content-type", "application/json")
                    .extension(beta_test::test_user("user-a"))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({})))
    }

    /// Create a public Cloud Agent session (inline agent, `always_ask`,
    /// tool `bash`) with `input`, which enqueues one run.
    async fn create_session(router: &Router, input: &str) -> String {
        let (status, payload) = send(
            router,
            "POST",
            "/sessions",
            &json!({
                "agent": {"model": "anthropic/claude-test", "instructions": "Be terse.", "tools": ["bash"]},
                "computer": {"kind": "none"},
                "permission": "always_ask",
                "input": input,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{payload}");
        payload["session"]["id"].as_str().unwrap().to_string()
    }

    async fn public_events(router: &Router, id: &str) -> Vec<Value> {
        let (status, payload) = send(router, "GET", &format!("/sessions/{id}/events"), &json!({})).await;
        assert_eq!(status, StatusCode::OK);
        payload["events"].as_array().unwrap().clone()
    }

    fn types(events: &[Value]) -> Vec<String> {
        events.iter().map(|e| e["type"].as_str().unwrap().to_string()).collect()
    }

    fn task_status(state: &Arc<AppState>, session_id: &str) -> Vec<String> {
        let conn = state.db.connect().unwrap();
        let mut stmt = conn
            .prepare("SELECT status FROM beta_work_tasks WHERE session_id = ?1 ORDER BY created_at, rowid")
            .unwrap();
        stmt.query_map(params![session_id], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<String>, _>>()
            .unwrap()
    }

    #[tokio::test]
    async fn worker_executes_session_run_and_appends_events() {
        let temp = beta_test::temp_dir("worker-exec");
        let state = beta_test::test_app_state(&temp).await;
        let router = crate::cloud_agents_routes::cloud_agents_router().with_state(state.clone());
        let (url, fake) = start_fake_gizzi(false).await;
        let id = create_session(&router, "list the files").await;

        let outcome = run_once(&state, &config(&url)).await;
        assert_eq!(outcome, Some(TaskOutcome::Succeeded));
        assert_eq!(task_status(&state, &id), vec!["succeeded"]);

        // The Gizzi turn carried the agent's model, instructions, prompt and
        // the effective permissions as the bot allowlist / ask-list.
        let body = fake.bodies.lock().unwrap()[0].clone();
        assert_eq!(body["model"], json!({"providerID": "anthropic", "modelID": "claude-test"}));
        assert_eq!(body["system"], json!("+Be terse."));
        assert_eq!(body["parts"][0]["text"], json!("list the files"));
        assert_eq!(body["bot"]["allowedTools"], json!(["bash"]));
        assert_eq!(body["bot"]["askTools"], json!(["bash"]));

        let events = public_events(&router, &id).await;
        let ty = types(&events);
        let pos = |t: &str| ty.iter().position(|x| x == t).unwrap_or_else(|| panic!("{t} missing: {ty:?}"));
        assert!(pos("turn.started") < pos("agent.tool_use"));
        assert!(pos("agent.tool_use") < pos("agent.message"));
        assert!(pos("agent.message") < pos("turn.completed"));
        assert!(pos("turn.completed") < pos("session.idle"));
        let tool_use = &events[pos("agent.tool_use")];
        assert_eq!(tool_use["data"]["tool_calls"][0]["name"], json!("bash"));
        assert_eq!(tool_use["data"]["requires_approval"], json!(true), "ApprovalStore gating applies");
        assert_eq!(events[pos("agent.message")]["data"]["content"], json!("hello from gizzi"));
        let (_, session) = send(&router, "GET", &format!("/sessions/{id}"), &json!({})).await;
        assert_eq!(session["session"]["status"], json!("idle"));

        // Usage was charged to the session budget counters.
        let (tokens, turns, tool_calls): (i64, i64, i64) = state
            .db
            .connect()
            .unwrap()
            .query_row(
                "SELECT tokens_used, turns_used, tool_calls_used FROM beta_sessions WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!((tokens, turns, tool_calls), (20, 1, 1));

        // A follow-up message continues the same Gizzi session.
        let (status, _) = send(
            &router,
            "POST",
            &format!("/sessions/{id}/events"),
            &json!({"events": [{"type": "user.message", "content": "and again"}]}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(run_once(&state, &config(&url)).await, Some(TaskOutcome::Succeeded));
        assert_eq!(fake.sessions.load(Ordering::SeqCst), 1);
        assert_eq!(fake.bodies.lock().unwrap().len(), 2);

        // Queue drained.
        assert_eq!(run_once(&state, &config(&url)).await, None);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn gizzi_down_fails_the_run_as_retryable_and_backs_off() {
        let temp = beta_test::temp_dir("worker-down");
        let state = beta_test::test_app_state(&temp).await;
        let router = crate::cloud_agents_routes::cloud_agents_router().with_state(state.clone());
        // A port nothing listens on.
        let closed = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", listener.local_addr().unwrap())
        };
        let id = create_session(&router, "hello").await;

        let outcome = run_once(&state, &config(&closed)).await.expect("a task was leased");
        match &outcome {
            TaskOutcome::Failed { code, retryable, error } => {
                assert_eq!(*code, "gizzi_unreachable");
                assert!(*retryable);
                assert!(error.contains("unreachable"), "{error}");
            }
            other => panic!("unexpected outcome {other:?}"),
        }
        assert_eq!(task_status(&state, &id), vec!["failed"]);
        let events = public_events(&router, &id).await;
        let failed = events.iter().find(|e| e["type"] == "turn.failed").expect("turn.failed");
        assert_eq!(failed["data"]["code"], json!("gizzi_unreachable"));
        assert_eq!(failed["data"]["retryable"], json!(true));
        assert!(types(&events).contains(&"session.idle".to_string()));
        let (_, session) = send(&router, "GET", &format!("/sessions/{id}"), &json!({})).await;
        assert_eq!(session["session"]["status"], json!("idle"));

        // The worker backs off instead of draining the queue.
        let backoff = GizziBackoff::default();
        backoff.observe(&outcome);
        assert!(backoff.remaining().is_some());
        backoff.observe(&TaskOutcome::Succeeded);
        assert!(backoff.remaining().is_none());
        let _ = std::fs::remove_dir_all(&temp);
    }

    async fn wait_for_turn(fake: &FakeGizzi, n: usize) {
        for _ in 0..200 {
            if fake.bodies.lock().unwrap().len() >= n {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("gizzi turn never started");
    }

    #[tokio::test]
    async fn interrupt_stops_a_running_turn() {
        let temp = beta_test::temp_dir("worker-interrupt");
        let state = beta_test::test_app_state(&temp).await;
        let router = crate::cloud_agents_routes::cloud_agents_router().with_state(state.clone());
        let beta_router = crate::beta_session_routes::beta_session_router().with_state(state.clone());
        let (url, fake) = start_fake_gizzi(true).await;

        // Beta interrupt (event only): the worker aborts and cancels.
        let id = create_session(&router, "long job").await;
        let run = tokio::spawn({
            let state = state.clone();
            let config = config(&url);
            async move { run_once(&state, &config).await }
        });
        wait_for_turn(&fake, 1).await;
        let (status, _) = send(&beta_router, "POST", &format!("/beta/sessions/{id}/interrupt"), &json!({"data": {}})).await;
        assert_eq!(status, StatusCode::OK);
        let outcome = tokio::time::timeout(Duration::from_secs(5), run).await.unwrap().unwrap();
        assert_eq!(outcome, Some(TaskOutcome::Cancelled));
        assert_eq!(fake.aborts.load(Ordering::SeqCst), 1);
        assert_eq!(task_status(&state, &id), vec!["cancelled"]);
        let events = public_events(&router, &id).await;
        let failed = events.iter().find(|e| e["type"] == "turn.failed").expect("turn.failed");
        assert_eq!(failed["data"]["code"], json!("interrupted"));
        assert!(!types(&events).contains(&"agent.message".to_string()));

        // Public user.interrupt (cancels the task): stop without a second
        // terminal event.
        let id = create_session(&router, "another long job").await;
        let run = tokio::spawn({
            let state = state.clone();
            let config = config(&url);
            async move { run_once(&state, &config).await }
        });
        wait_for_turn(&fake, 2).await;
        let (status, _) = send(
            &router,
            "POST",
            &format!("/sessions/{id}/events"),
            &json!({"events": [{"type": "user.interrupt"}]}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let outcome = tokio::time::timeout(Duration::from_secs(5), run).await.unwrap().unwrap();
        assert_eq!(outcome, Some(TaskOutcome::Cancelled));
        assert_eq!(fake.aborts.load(Ordering::SeqCst), 2);
        let events = public_events(&router, &id).await;
        let ty = types(&events);
        assert_eq!(ty.iter().filter(|t| *t == "turn.failed").count(), 1, "{ty:?}");
        assert_eq!(ty.last().map(String::as_str), Some("session.idle"));
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn expired_lease_is_released_to_the_worker_and_sandbox_tasks_are_skipped() {
        let temp = beta_test::temp_dir("worker-relese");
        let state = beta_test::test_app_state(&temp).await;
        let router = crate::cloud_agents_routes::cloud_agents_router().with_state(state.clone());
        let (url, _fake) = start_fake_gizzi(false).await;
        let id = create_session(&router, "resume me").await;

        // A sandbox job on the same session belongs to external workers.
        {
            let conn = state.db.connect().unwrap();
            conn.execute(
                "INSERT INTO beta_work_tasks (id, user_id, session_id, payload, sandbox_image)
                 VALUES ('sandbox-task', 'user-a', ?1, '{}', 'alpine:latest')",
                params![id],
            )
            .unwrap();
        }

        // Held by a previous process whose lease has not expired: not leased.
        let set_lease = |expires: chrono::DateTime<Utc>| {
            state
                .db
                .connect()
                .unwrap()
                .execute(
                    "UPDATE beta_work_tasks SET status = 'running', lease_worker_id = 'dead-worker',
                     lease_expires_at = ?1 WHERE session_id = ?2 AND sandbox_image IS NULL",
                    params![expires.to_rfc3339(), id],
                )
                .unwrap();
        };
        set_lease(Utc::now() + chrono::Duration::seconds(30));
        assert_eq!(run_once(&state, &config(&url)).await, None);

        // Once the lease expires (the process died), the worker takes it.
        set_lease(Utc::now() - chrono::Duration::seconds(1));
        assert_eq!(run_once(&state, &config(&url)).await, Some(TaskOutcome::Succeeded));
        let statuses: Vec<(String, String)> = {
            let conn = state.db.connect().unwrap();
            let mut stmt = conn
                .prepare("SELECT id, status FROM beta_work_tasks WHERE session_id = ?1 ORDER BY id")
                .unwrap();
            stmt.query_map(params![id], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert!(statuses.contains(&("sandbox-task".to_string(), "queued".to_string())), "{statuses:?}");
        assert!(statuses.iter().any(|(_, s)| s == "succeeded"), "{statuses:?}");
        assert_eq!(run_once(&state, &config(&url)).await, None);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn build_turn_shapes_prompt_and_system() {
        let (text, system) = build_turn(
            &json!({"messages": [{"role": "system", "content": "Be brief."}, {"role": "user", "content": "hi"}],
                    "memory_context": {"stores": []}}),
            None,
        )
        .unwrap();
        assert_eq!(text, "hi");
        assert_eq!(system.len(), 2);
        let (text, _) = build_turn(
            &json!({"messages": [{"role": "user", "content": "a"}, {"role": "assistant", "content": "b"}, {"role": "user", "content": "c"}]}),
            None,
        )
        .unwrap();
        assert_eq!(text, "user: a\n\nassistant: b\n\nuser: c");
        assert_eq!(build_turn(&json!({"messages": []}), Some("nightly report")).unwrap().0, "nightly report");
        assert_eq!(build_turn(&json!({"messages": []}), None).unwrap_err().code, "empty_prompt");
    }
}
