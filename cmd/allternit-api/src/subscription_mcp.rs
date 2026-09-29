//! Subscription Fabric MCP server (SURFACES_PLAN §3 step 5, HARDENING D16).
//!
//! `POST /api/v1/subscriptions/mcp` — MCP Streamable HTTP (single JSON-RPC
//! endpoint, plain JSON replies), authenticated as the user like every other
//! `/api/v1` route. Other agents (gizzi via config `mcp: {type: "remote",
//! url, headers}`, Claude Code, bots) mount it to reach the user's
//! subscriptions.
//!
//! D16: an agent can only **prepare** a subscription task here. `tools/call`
//! never submits anything to the gateway — it writes an approval card
//! (`cowork_approvals`, kind `subscription_task`) and answers
//! `pending_approval`. The task runs only when the person approves that card
//! (`POST /api/v1/cowork/approvals`), which mints the human action at that
//! moment and submits through the forwarder (`execute_prepared`). Rejected or
//! unanswered cards never run. `task_status` reports where a prepared task is.

use axum::{
    body::Bytes,
    extract::{Extension, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::warn;

use crate::auth::AuthUser;
use crate::subscription_routes::{
    acknowledged_version, forward, mint_bound_human_action, provider_disclosure, DISCLOSURE_VERSION, HUMAN_ACTION_HEADER,
};
use crate::AppState;

const PROTOCOL_VERSION: &str = "2025-03-26";
const SERVER_NAME: &str = "allternit-subscriptions";
/// `cowork_approvals.content.kind` of a prepared subscription task.
pub const PREPARED_KIND: &str = "subscription_task";
const PREPARED_SOURCE: &str = "subscription-mcp";
const MAX_PROMPT_CHARS: usize = 20_000;

struct CapabilityTool {
    name: &'static str,
    capability: &'static str,
    verb: &'static str,
    description: &'static str,
}

const TOOLS: &[CapabilityTool] = &[
    CapabilityTool {
        name: "image_generate",
        capability: "image.generate",
        verb: "Make an image",
        description: "Prepare an image made by one of the user's connected subscriptions.",
    },
    CapabilityTool {
        name: "presentation_create",
        capability: "presentation.create",
        verb: "Create a presentation",
        description: "Prepare a slide deck (PPTX) made by one of the user's connected subscriptions.",
    },
    CapabilityTool {
        name: "document_create",
        capability: "document.create",
        verb: "Create a document",
        description: "Prepare a document (DOCX/PDF/Markdown) written by one of the user's connected subscriptions.",
    },
    CapabilityTool {
        name: "research_deep",
        capability: "research.deep",
        verb: "Run deep research",
        description: "Prepare a deep research task (a cited report, often 10-25 minutes) on one of the user's connected subscriptions.",
    },
];

const PREPARE_NOTE: &str = " This only prepares the task: the user sees an approval card in the Allternit app and the \
task runs only if they approve it. The result is `pending_approval` with an approval_id; call task_status with it \
later. Never tell the user the task has run until task_status says so.";

#[derive(Debug, Deserialize)]
pub struct RpcRequest {
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

fn success(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn rpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message.into()}})
}

fn tool_text(value: Value, is_error: bool) -> Value {
    let text = match value {
        Value::String(s) => s,
        other => other.to_string(),
    };
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

fn provider_name(provider: &str) -> String {
    provider_disclosure(provider).map(|p| p.name.to_string()).unwrap_or_else(|| provider.to_string())
}

/// The task a subscription approval card confirms, exactly as it will be
/// submitted. The card's text is built from it and the human action minted on
/// approval is bound to its digest, so what the person reads is what runs.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CardTask {
    pub capability: String,
    pub provider: String,
    pub prompt: String,
    pub options: Value,
}

impl CardTask {
    /// Read `{capability, provider, prompt, options?}` from a JSON object
    /// (the card's `task`, a prepared task, or gizzi's ask metadata). None
    /// when any of the three required fields is missing or blank: a card that
    /// cannot show its prompt can never be approved into a task.
    pub(crate) fn from_json(value: &Value) -> Option<Self> {
        let field = |k: &str| value.get(k).and_then(Value::as_str).filter(|s| !s.trim().is_empty()).map(str::to_string);
        Some(Self {
            capability: field("capability")?,
            provider: field("provider")?,
            prompt: field("prompt")?,
            options: match value.get("options") {
                None | Some(Value::Null) => json!({}),
                Some(v) => v.clone(),
            },
        })
    }

    pub(crate) fn to_json(&self) -> Value {
        json!({"capability": self.capability, "provider": self.provider, "prompt": self.prompt, "options": self.options})
    }

    pub(crate) fn digest(&self) -> String {
        crate::subscription_routes::task_digest(&self.capability, &self.provider, &self.prompt, Some(&self.options))
    }

    /// The card's headline. It carries the whole prompt because every
    /// approval surface renders `summary` (the chat card, the companion chat,
    /// bot capsules), and the person must see what is sent, not a title an
    /// agent chose.
    pub(crate) fn summary(&self) -> String {
        let name = provider_name(&self.provider);
        let mut out = format!(
            "{} with your {name} subscription. {name} receives this prompt, exactly as written: \"{}\"",
            capability_verb(&self.capability),
            self.prompt
        );
        if self.options.as_object().map_or(false, |o| !o.is_empty()) {
            out.push_str(&format!(" Options: {}.", self.options));
        }
        out
    }
}

/// The task an approval card showed the person, or None when the card has
/// no complete task or its headline is not the one built from that task
/// (so the text the person approved and the task that would run differ).
/// Only a task returned here may be approved into a human action.
pub(crate) fn displayed_card_task(content: &str) -> Option<CardTask> {
    let value: Value = serde_json::from_str(content).ok()?;
    displayed_task(&value)
}

fn displayed_task(content: &Value) -> Option<CardTask> {
    let task = CardTask::from_json(content.get("task")?)?;
    (content.get("summary").and_then(Value::as_str) == Some(task.summary().as_str())).then_some(task)
}

/// Plain verb phrase for a capability, for approval cards.
pub(crate) fn capability_verb(capability: &str) -> String {
    TOOLS
        .iter()
        .find(|t| t.capability == capability)
        .map(|t| t.verb.to_string())
        .unwrap_or_else(|| format!("Run {capability}"))
}

pub async fn handle_rpc(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(req): Json<RpcRequest>,
) -> Response {
    // Notifications (no id) get no body.
    let Some(id) = req.id.clone() else {
        return StatusCode::ACCEPTED.into_response();
    };
    let reply = match req.method.as_str() {
        "initialize" => success(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION")},
                "instructions": "Tools here prepare tasks for the user's own subscriptions (ChatGPT, Claude, Kimi). \
Nothing runs until the user approves it in the Allternit app.",
            }),
        ),
        "ping" => success(id, json!({})),
        "tools/list" => success(id, json!({"tools": tool_list(&state, &user).await})),
        "tools/call" => {
            let name = req.params.get("name").and_then(Value::as_str).unwrap_or_default();
            let args = req.params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            let (value, is_error) = match call_tool(&state, &user, name, &args).await {
                Ok(v) => (v, false),
                Err(e) => (Value::String(e), true),
            };
            success(id, tool_text(value, is_error))
        }
        other => rpc_error(id, -32601, format!("Method not found: {other}")),
    };
    Json(reply).into_response()
}

// ── Gateway access (in-process, through the forwarder) ───────────────────────

async fn gateway_json(
    state: &Arc<AppState>,
    user: &AuthUser,
    method: Method,
    path: &str,
    headers: HeaderMap,
    body: Option<Value>,
) -> Result<Value, (StatusCode, Value)> {
    let mut headers = headers;
    let bytes = match body {
        Some(b) => {
            headers.insert("content-type", HeaderValue::from_static("application/json"));
            Bytes::from(b.to_string())
        }
        None => Bytes::new(),
    };
    let res = forward(state, user, path, method, &headers, None, bytes).await;
    let status = res.status();
    let raw = axum::body::to_bytes(res.into_body(), 10 * 1024 * 1024).await.unwrap_or_default();
    let value: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
    if status.is_success() {
        Ok(value)
    } else {
        Err((status, value))
    }
}

/// Providers entitled to run `capability` now, from GET v1/capabilities.
fn available_providers(capabilities: &Value, capability: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for entry in capabilities.as_array().into_iter().flatten() {
        if entry.get("capability").and_then(Value::as_str) != Some(capability) {
            continue;
        }
        if entry.get("status").and_then(Value::as_str) == Some("disabled") {
            continue;
        }
        let entitled = entry
            .get("entitlements")
            .and_then(Value::as_array)
            .map(|list| list.iter().any(|e| e.get("available").and_then(Value::as_bool) == Some(true)))
            .unwrap_or(false);
        if let (true, Some(provider)) = (entitled, entry.get("provider").and_then(Value::as_str)) {
            if !out.iter().any(|p| p == provider) {
                out.push(provider.to_string());
            }
        }
    }
    out
}

async fn capabilities(state: &Arc<AppState>, user: &AuthUser) -> Result<Value, String> {
    gateway_json(state, user, Method::GET, "v1/capabilities", HeaderMap::new(), None)
        .await
        .map_err(|(status, body)| gateway_error_text(status, &body))
}

fn gateway_error_text(status: StatusCode, body: &Value) -> String {
    match body.get("error").and_then(Value::as_str) {
        Some("sessions_computer_not_bound") => "No Sessions computer is set up for subscriptions yet.".into(),
        Some("sessions_computer_missing") => "The user's Sessions computer was not found.".into(),
        Some("sessions_computer_not_running") => "The user's Sessions computer is not running.".into(),
        Some(code) => format!("The subscription gateway refused the request ({status} {code})."),
        None => format!("The subscription gateway is not reachable ({status})."),
    }
}

// ── Tools ────────────────────────────────────────────────────────────────────

fn status_tool() -> Value {
    json!({
        "name": "task_status",
        "description": "Where a prepared subscription task is: pending_approval, rejected, running, needs_user \
(the provider asked the user something; the user answers it, never the agent), completed (with text and \
artifact download paths) or failed.",
        "inputSchema": {
            "type": "object",
            "properties": {"approval_id": {"type": "string", "description": "From the prepare result"}},
            "required": ["approval_id"]
        }
    })
}

async fn tool_list(state: &Arc<AppState>, user: &AuthUser) -> Vec<Value> {
    let caps = capabilities(state, user).await.unwrap_or(Value::Null);
    let mut tools: Vec<Value> = TOOLS
        .iter()
        .filter_map(|tool| {
            let providers = available_providers(&caps, tool.capability);
            if providers.is_empty() {
                return None;
            }
            Some(json!({
                "name": tool.name,
                "description": format!("{}{} Available through: {}.", tool.description, PREPARE_NOTE, providers.iter().map(|p| provider_name(p)).collect::<Vec<_>>().join(", ")),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "prompt": {"type": "string", "description": "What to make, in the user's words"},
                        "title": {"type": "string", "description": "Short label for the task. The approval card shows the full prompt, not just this"},
                        "provider": {"type": "string", "enum": providers, "description": "Which subscription (default the first)"}
                    },
                    "required": ["prompt"]
                }
            }))
        })
        .collect();
    tools.push(status_tool());
    tools
}

async fn call_tool(state: &Arc<AppState>, user: &AuthUser, name: &str, args: &Value) -> Result<Value, String> {
    if name == "task_status" {
        let id = args.get("approval_id").and_then(Value::as_str).unwrap_or_default();
        return task_status(state, user, id).await;
    }
    let tool = TOOLS.iter().find(|t| t.name == name).ok_or_else(|| format!("Unknown tool: {name}"))?;
    prepare(state, user, tool, args).await
}

/// Write the approval card for a task. Never submits to the gateway.
async fn prepare(state: &Arc<AppState>, user: &AuthUser, tool: &CapabilityTool, args: &Value) -> Result<Value, String> {
    let prompt = args.get("prompt").and_then(Value::as_str).map(str::trim).unwrap_or_default();
    if prompt.is_empty() {
        return Err("prompt is required".into());
    }
    if prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(format!("prompt is longer than {MAX_PROMPT_CHARS} characters"));
    }
    let caps = capabilities(state, user).await?;
    let providers = available_providers(&caps, tool.capability);
    let provider = match args.get("provider").and_then(Value::as_str) {
        Some(p) => p.to_string(),
        None => providers.first().cloned().ok_or_else(|| {
            format!("No connected subscription can {} right now. Nothing was prepared.", tool.verb.to_lowercase())
        })?,
    };
    if !providers.contains(&provider) {
        return Err(format!("{} cannot do this right now. Nothing was prepared.", provider_name(&provider)));
    }
    let name = provider_name(&provider);
    match acknowledged_version(&state.db, &user.user_id, &provider) {
        Ok(Some(v)) if v == DISCLOSURE_VERSION => {}
        Ok(_) => {
            return Err(format!(
                "The user has not acknowledged the {name} subscription disclosure in the Allternit app yet. Nothing was prepared."
            ))
        }
        Err(e) => {
            warn!(error = %e, "subscription mcp: disclosure lookup failed");
            return Err("database error".into());
        }
    }
    let title: String = args
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or(prompt)
        .chars()
        .take(80)
        .collect();

    let task = CardTask {
        capability: tool.capability.to_string(),
        provider: provider.clone(),
        prompt: prompt.to_string(),
        options: json!({}),
    };
    let approval_id = format!("subsprep_{}", uuid::Uuid::new_v4().simple());
    // D16: the card shows the prompt that runs — `summary` carries it in full
    // (the agent's `title` is only a label) — and `task` is what approval
    // binds and submits.
    let content = json!({
        "kind": PREPARED_KIND,
        "actionId": approval_id,
        "sessionId": "",
        "riskLevel": "high",
        "summary": task.summary(),
        "details": {
            "actionType": "subscription",
            "target": format!("{provider}:{}", tool.capability),
            "prompt": prompt,
            "consequence": format!("An agent prepared this. It runs on your {name} subscription on your Sessions computer only if you approve. Nothing has been sent to {name}."),
        },
        "requestedAt": chrono::Utc::now().to_rfc3339(),
        "title": title,
        "task": task.to_json(),
        "execution": Value::Null,
    });
    let db = state.db.clone();
    let uid = user.user_id.clone();
    let row_id = approval_id.clone();
    let text = content.to_string();
    tokio::task::spawn_blocking(move || {
        db.connect()?.execute(
            "INSERT INTO cowork_approvals (id, user_id, content, source) VALUES (?1, ?2, ?3, ?4)",
            params![row_id, uid, text, PREPARED_SOURCE],
        )
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| {
        warn!(error = %e, "subscription mcp: could not write the approval card");
        "database error".to_string()
    })?;

    Ok(json!({
        "status": "pending_approval",
        "approval_id": approval_id,
        "capability": tool.capability,
        "provider": provider,
        "message": format!("Prepared. The user will see an approval card with the full prompt; it runs on {name} only if they approve. Check with task_status."),
    }))
}

struct PreparedRow {
    content: Value,
    decision: Option<String>,
    dismissed: bool,
}

async fn load_prepared(state: &Arc<AppState>, user_id: &str, approval_id: &str) -> Result<Option<PreparedRow>, String> {
    let db = state.db.clone();
    let uid = user_id.to_string();
    let id = approval_id.to_string();
    let row = tokio::task::spawn_blocking(move || {
        db.connect()?
            .query_row(
                "SELECT content, decision, dismissed FROM cowork_approvals WHERE id = ?1 AND user_id = ?2",
                params![id, uid],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, i64>(2)?)),
            )
            .optional()
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    Ok(row.and_then(|(content, decision, dismissed)| {
        let content: Value = serde_json::from_str(&content).ok()?;
        (content.get("kind").and_then(Value::as_str) == Some(PREPARED_KIND)).then_some(PreparedRow {
            content,
            decision,
            dismissed: dismissed != 0,
        })
    }))
}

async fn store_execution(state: &Arc<AppState>, user_id: &str, approval_id: &str, content: &Value) {
    let db = state.db.clone();
    let uid = user_id.to_string();
    let id = approval_id.to_string();
    let text = content.to_string();
    let stored = tokio::task::spawn_blocking(move || {
        db.connect()?.execute(
            "UPDATE cowork_approvals SET content = ?3 WHERE id = ?1 AND user_id = ?2",
            params![id, uid, text],
        )
    })
    .await;
    if !matches!(stored, Ok(Ok(1))) {
        warn!(approval_id, "subscription mcp: could not record the execution");
    }
}

/// Run a prepared task after the person approved its card. Called only from
/// the approval decide route, as the approving user. Mints the human action
/// here — the person's approval is the human act — and submits through the
/// forwarder, which checks the disclosure and stamps `initiated_by`. Runs at
/// most once per card: a recorded execution is returned as is.
pub(crate) async fn execute_prepared(state: &Arc<AppState>, user: &AuthUser, approval_id: &str) -> Value {
    let row = match load_prepared(state, &user.user_id, approval_id).await {
        Ok(Some(row)) => row,
        Ok(None) => return json!({"error": "not_a_prepared_task"}),
        Err(e) => {
            warn!(error = %e, "subscription mcp: load failed");
            return json!({"error": "database error"});
        }
    };
    if !row.content["execution"].is_null() {
        return row.content["execution"].clone();
    }
    let Some(task) = displayed_task(&row.content) else {
        return json!({"error": "prepared_task_invalid", "detail": "The card does not show a complete task. Nothing was sent."});
    };
    // Bound to the task on the card: the forwarder refuses this action for
    // any other prompt.
    let action = match mint_bound_human_action(&state.db, &user.user_id, "approval.confirm", &task.digest()) {
        Ok((action, _)) => action,
        Err(e) => {
            warn!(error = %e, "subscription mcp: could not mint the human action");
            return json!({"error": "database error"});
        }
    };
    let mut headers = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(&action) {
        headers.insert(HUMAN_ACTION_HEADER, v);
    }
    let body = json!({
        "capability": task.capability,
        "prompt": task.prompt,
        "routing": {"provider": task.provider},
        "options": task.options,
        "priority": "normal",
        "idempotency_key": format!("mcp-{approval_id}"),
    });
    let execution = match gateway_json(state, user, Method::POST, "v1/tasks", headers, Some(body)).await {
        Ok(task) => json!({
            "task_id": task.get("task_id").cloned().unwrap_or(Value::Null),
            "submitted_at": chrono::Utc::now().to_rfc3339(),
        }),
        Err((status, body)) => json!({
            "error": body.get("error").cloned().unwrap_or_else(|| json!(status.as_u16())),
            "detail": gateway_error_text(status, &body),
        }),
    };
    let mut content = row.content;
    content["execution"] = execution.clone();
    store_execution(state, &user.user_id, approval_id, &content).await;
    execution
}

async fn task_status(state: &Arc<AppState>, user: &AuthUser, approval_id: &str) -> Result<Value, String> {
    let row = load_prepared(state, &user.user_id, approval_id)
        .await?
        .ok_or_else(|| format!("No prepared subscription task {approval_id}."))?;
    match row.decision.as_deref() {
        None if !row.dismissed => {
            return Ok(json!({"approval_id": approval_id, "status": "pending_approval",
                "message": "Waiting for the user to approve or reject it in the Allternit app. Nothing has run."}))
        }
        None => return Ok(json!({"approval_id": approval_id, "status": "dismissed", "message": "The user dismissed it. It did not run."})),
        Some("rejected") => {
            return Ok(json!({"approval_id": approval_id, "status": "rejected", "message": "The user rejected it. It did not run."}))
        }
        _ => {}
    }
    let execution = &row.content["execution"];
    let Some(task_id) = execution["task_id"].as_str() else {
        return Ok(json!({
            "approval_id": approval_id,
            "status": "failed",
            "message": execution["detail"].as_str().unwrap_or("The approved task could not be started."),
        }));
    };
    let task = gateway_json(state, user, Method::GET, &format!("v1/tasks/{task_id}"), HeaderMap::new(), None)
        .await
        .map_err(|(status, body)| gateway_error_text(status, &body))?;
    let status = task["status"].as_str().unwrap_or("unknown").to_string();
    let artifacts: Vec<Value> = task["result"]["artifact_ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|id| json!({"artifact_id": id, "download": format!("/api/v1/subscriptions/gateway/v1/artifacts/{id}/download")}))
        .collect();
    let mut out = json!({
        "approval_id": approval_id,
        "task_id": task_id,
        "status": status,
        "text": task["result"]["text"].clone(),
        "artifacts": artifacts,
    });
    if status == "needs_user" {
        out["question"] = task["status_detail"].clone();
        out["message"] = json!("The provider asked the user something. The user answers it; do not answer it for them.");
    }
    if status == "failed" {
        out["error"] = task["error"].clone();
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, Uri};
    use axum::Router;
    use std::sync::Mutex;
    use tower::ServiceExt;

    const USER: &str = "user-1";

    fn user() -> AuthUser {
        AuthUser {
            user_id: USER.into(),
            email: None,
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: None,
            organization_role: None,
            organization_slug: None,
        }
    }

    type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;

    /// Fake gateway: presentation.create available on chatgpt, tasks accepted.
    async fn fake_gateway() -> (String, Seen) {
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        let app = Router::new().fallback(move |method: Method, uri: Uri, body: Bytes| {
            let record = record.clone();
            async move {
                let json_body = serde_json::from_slice(&body).unwrap_or(Value::Null);
                record.lock().unwrap().push((method.to_string(), uri.path().to_string(), json_body));
                match (method.as_str(), uri.path()) {
                    ("GET", "/v1/capabilities") => Json(json!([
                        {"capability": "presentation.create", "provider": "chatgpt", "status": "stable",
                         "entitlements": [{"account_id": "a", "available": true}]},
                        {"capability": "document.create", "provider": "chatgpt", "status": "stable",
                         "entitlements": [{"account_id": "a", "available": false}]},
                        {"capability": "chat.create", "provider": "chatgpt", "status": "stable",
                         "entitlements": [{"account_id": "a", "available": true}]}
                    ]))
                    .into_response(),
                    ("POST", "/v1/tasks") => (StatusCode::CREATED, Json(json!({"task_id": "task-9", "status": "queued"}))).into_response(),
                    ("GET", "/v1/tasks/task-9") => Json(json!({"task_id": "task-9", "status": "completed",
                        "result": {"text": "Deck ready.", "artifact_ids": ["art-1"]}})).into_response(),
                    _ => (StatusCode::NOT_FOUND, Json(json!({"error": "not_found"}))).into_response(),
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), seen)
    }

    async fn setup(ack: bool) -> (Router, Arc<AppState>, Seen) {
        let (url, seen) = fake_gateway().await;
        let temp = tempfile::tempdir().unwrap();
        let driver: Arc<dyn allternit_driver_interface::ExecutionDriver> =
            Arc::new(crate::computer_ws::tests::StubDriver(Some(url)));
        let state = crate::test_helpers::app_state_with_driver(temp.path(), Some(driver)).await;
        std::mem::forget(temp);
        let conn = state.db.connect().unwrap();
        conn.execute(
            "INSERT INTO computers (id, kind, provider, status, owner_type, owner_id, name, os, native_id, billing_source)
             VALUES ('computer-1', 'cloud_desktop', 'incus', 'running', 'user', ?1, 'sessions', 'ubuntu-24.04', 'sandbox-1', 'credits')",
            params![USER],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO subs_gateway_bindings (user_id, computer_id, guest_port, token_sealed) VALUES (?1, 'computer-1', 7788, ?2)",
            params![USER, crate::token_crypto::seal("gw-token")],
        )
        .unwrap();
        if ack {
            conn.execute(
                "INSERT INTO subs_disclosure_acks (user_id, provider, version) VALUES (?1, 'chatgpt', ?2)",
                params![USER, DISCLOSURE_VERSION],
            )
            .unwrap();
        }
        let app = crate::subscription_routes::router().layer(Extension(user())).with_state(state.clone());
        (app, state, seen)
    }

    async fn rpc(app: &Router, method: &str, params: Value) -> Value {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let req = Request::builder()
            .method("POST")
            .uri("/subscriptions/mcp")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn tool_result(reply: &Value) -> (Value, bool) {
        let text = reply["result"]["content"][0]["text"].as_str().unwrap();
        (serde_json::from_str(text).unwrap_or(Value::String(text.into())), reply["result"]["isError"] == true)
    }

    fn task_posts(seen: &Seen) -> usize {
        seen.lock().unwrap().iter().filter(|(m, p, _)| m == "POST" && p == "/v1/tasks").count()
    }

    #[tokio::test]
    async fn lists_only_available_capabilities_plus_status() {
        let (app, _state, _seen) = setup(true).await;
        let reply = rpc(&app, "tools/list", json!({})).await;
        let names: Vec<&str> = reply["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["presentation_create", "task_status"]);
        let init = rpc(&app, "initialize", json!({})).await;
        assert_eq!(init["result"]["serverInfo"]["name"], SERVER_NAME);
    }

    #[tokio::test]
    async fn d16_a_tool_call_only_prepares_and_never_reaches_post_v1_tasks() {
        let (app, state, seen) = setup(true).await;
        let reply = rpc(&app, "tools/call", json!({"name": "presentation_create", "arguments": {"prompt": "Q3 review deck"}})).await;
        let (result, is_error) = tool_result(&reply);
        assert!(!is_error, "{reply}");
        assert_eq!(result["status"], "pending_approval");
        let approval_id = result["approval_id"].as_str().unwrap().to_string();
        assert_eq!(task_posts(&seen), 0, "preparing must not submit");

        // The card is a normal approval row for this user.
        let (content, source): (String, String) = state
            .db
            .connect()
            .unwrap()
            .query_row(
                "SELECT content, source FROM cowork_approvals WHERE id = ?1 AND user_id = ?2",
                params![approval_id, USER],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let content: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(source, PREPARED_SOURCE);
        assert_eq!(content["kind"], PREPARED_KIND);
        assert_eq!(content["actionId"], approval_id);
        assert!(content["summary"].as_str().unwrap().contains("ChatGPT"));

        // Status polling does not run it either.
        let reply = rpc(&app, "tools/call", json!({"name": "task_status", "arguments": {"approval_id": approval_id}})).await;
        assert_eq!(tool_result(&reply).0["status"], "pending_approval");
        assert_eq!(task_posts(&seen), 0);

        // Nothing in the MCP surface can mint a human action.
        let minted: i64 = state
            .db
            .connect()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM subs_human_actions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(minted, 0);
    }

    #[tokio::test]
    async fn approval_runs_it_once_with_a_fresh_human_action() {
        let (app, state, seen) = setup(true).await;
        let reply = rpc(&app, "tools/call", json!({"name": "presentation_create", "arguments": {"prompt": "deck", "title": "Q3"}})).await;
        let approval_id = tool_result(&reply).0["approval_id"].as_str().unwrap().to_string();
        state
            .db
            .connect()
            .unwrap()
            .execute("UPDATE cowork_approvals SET dismissed = 1, decision = 'approved' WHERE id = ?1", params![approval_id])
            .unwrap();

        let execution = execute_prepared(&state, &user(), &approval_id).await;
        assert_eq!(execution["task_id"], "task-9", "{execution}");
        {
            let seen = seen.lock().unwrap();
            let (_, _, sent) = seen.iter().find(|(m, p, _)| m == "POST" && p == "/v1/tasks").unwrap();
            assert_eq!(sent["capability"], "presentation.create");
            assert_eq!(sent["routing"]["provider"], "chatgpt");
            assert_eq!(sent["initiated_by"]["kind"], "human");
            assert_eq!(sent["initiated_by"]["user_id"], USER);
            assert!(sent["initiated_by"]["action_id"].as_str().unwrap().starts_with("ha_"));
        }
        // Once: a second approval relay returns the recorded execution.
        let again = execute_prepared(&state, &user(), &approval_id).await;
        assert_eq!(again, execution);
        assert_eq!(task_posts(&seen), 1);

        let reply = rpc(&app, "tools/call", json!({"name": "task_status", "arguments": {"approval_id": approval_id}})).await;
        let (status, _) = tool_result(&reply);
        assert_eq!(status["status"], "completed");
        assert_eq!(status["artifacts"][0]["download"], "/api/v1/subscriptions/gateway/v1/artifacts/art-1/download");
    }

    #[tokio::test]
    async fn refuses_to_prepare_without_disclosure_or_for_unavailable_providers() {
        let (app, _state, seen) = setup(false).await;
        let reply = rpc(&app, "tools/call", json!({"name": "presentation_create", "arguments": {"prompt": "deck"}})).await;
        let (text, is_error) = tool_result(&reply);
        assert!(is_error);
        assert!(text.as_str().unwrap().contains("disclosure"));

        let reply = rpc(&app, "tools/call", json!({"name": "presentation_create", "arguments": {"prompt": "deck", "provider": "kimi"}})).await;
        assert!(tool_result(&reply).1);
        let reply = rpc(&app, "tools/call", json!({"name": "document_create", "arguments": {"prompt": "doc"}})).await;
        assert!(tool_result(&reply).1);
        assert_eq!(task_posts(&seen), 0);
    }

    /// POST /cowork/approvals as the given caller kind (None = no marker).
    async fn decide(state: &Arc<AppState>, approval_id: &str, decision: &str, caller: Option<crate::auth::CallerKind>) -> (StatusCode, Value) {
        let mut app = crate::cowork_routes::cowork_router().layer(Extension(user()));
        if let Some(kind) = caller {
            app = app.layer(Extension(kind));
        }
        let app = app.with_state(state.clone());
        let req = Request::builder()
            .method("POST")
            .uri("/cowork/approvals")
            .header("content-type", "application/json")
            .header("x-allternit-user-id", USER)
            .body(Body::from(json!({"actionId": approval_id, "decision": decision}).to_string()))
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    fn card(state: &Arc<AppState>, approval_id: &str) -> (Value, Option<String>) {
        state
            .db
            .connect()
            .unwrap()
            .query_row(
                "SELECT content, decision FROM cowork_approvals WHERE id = ?1",
                params![approval_id],
                |r| Ok((serde_json::from_str(&r.get::<_, String>(0)?).unwrap(), r.get(1)?)),
            )
            .unwrap()
    }

    #[tokio::test]
    async fn d16_the_card_shows_the_prompt_that_runs_not_the_agents_title() {
        let (app, state, seen) = setup(true).await;
        let secret = "Paste of the confidential board memo: revenue down 12%";
        let reply = rpc(
            &app,
            "tools/call",
            json!({"name": "presentation_create", "arguments": {"prompt": secret, "title": "Q3 summary"}}),
        )
        .await;
        let approval_id = tool_result(&reply).0["approval_id"].as_str().unwrap().to_string();
        let (content, _) = card(&state, &approval_id);
        let summary = content["summary"].as_str().unwrap();
        assert!(summary.contains(secret), "the card headline must carry the prompt: {summary}");
        assert!(!summary.contains("Q3 summary"), "the agent's title is not the headline: {summary}");
        assert_eq!(content["details"]["prompt"], secret);

        let (status, body) = decide(&state, &approval_id, "approved", Some(crate::auth::CallerKind::Person)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let seen = seen.lock().unwrap();
        let (_, _, sent) = seen.iter().find(|(m, p, _)| m == "POST" && p == "/v1/tasks").unwrap();
        assert_eq!(sent["prompt"], secret, "what runs is what the card showed");
    }

    #[tokio::test]
    async fn d16_an_agent_credential_cannot_approve_a_prepared_card() {
        let (app, state, seen) = setup(true).await;
        let reply = rpc(&app, "tools/call", json!({"name": "presentation_create", "arguments": {"prompt": "deck"}})).await;
        let approval_id = tool_result(&reply).0["approval_id"].as_str().unwrap().to_string();

        // The MCP client's own bearer (an agent credential), or a request with
        // no caller marker at all, cannot approve.
        for caller in [Some(crate::auth::CallerKind::Agent), None] {
            let (status, body) = decide(&state, &approval_id, "approved", caller).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
            assert_eq!(body["error"], "person_required");
        }
        assert_eq!(task_posts(&seen), 0);
        let minted: i64 = state
            .db
            .connect()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM subs_human_actions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(minted, 0, "a refused approval mints nothing");
        // The card is still waiting for the person.
        assert_eq!(card(&state, &approval_id).1, None);
        let reply = rpc(&app, "tools/call", json!({"name": "task_status", "arguments": {"approval_id": approval_id}})).await;
        assert_eq!(tool_result(&reply).0["status"], "pending_approval");

        // The person approves it: it runs once.
        let (status, body) = decide(&state, &approval_id, "approved", Some(crate::auth::CallerKind::Person)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["execution"]["task_id"], "task-9");
        assert_eq!(task_posts(&seen), 1);
    }

    #[tokio::test]
    async fn an_agent_credential_may_still_reject_a_card() {
        let (app, state, seen) = setup(true).await;
        let reply = rpc(&app, "tools/call", json!({"name": "presentation_create", "arguments": {"prompt": "deck"}})).await;
        let approval_id = tool_result(&reply).0["approval_id"].as_str().unwrap().to_string();
        let (status, _) = decide(&state, &approval_id, "rejected", Some(crate::auth::CallerKind::Agent)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(card(&state, &approval_id).1.as_deref(), Some("rejected"));
        assert_eq!(task_posts(&seen), 0);
    }

    #[tokio::test]
    async fn d16_a_gizzi_subscription_ask_cannot_be_approved_by_an_agent() {
        let (_app, state, _seen) = setup(true).await;
        // The row the agent-chat bridge writes for a tool-belt ask.
        let content = crate::v1_routes::gizzi_permission_approval_content(&json!({
            "id": "per_sub", "sessionID": "ses_1", "permission": "subscription",
            "patterns": ["chatgpt:presentation.create"],
            "metadata": {"capability": "presentation.create", "provider": "chatgpt", "prompt": "deck", "providerName": "ChatGPT"}
        }));
        state
            .db
            .connect()
            .unwrap()
            .execute(
                "INSERT INTO cowork_approvals (id, user_id, content, source) VALUES ('per_sub', ?1, ?2, 'gizzi-permission')",
                params![USER, content.to_string()],
            )
            .unwrap();
        let (status, body) = decide(&state, "per_sub", "approved", Some(crate::auth::CallerKind::Agent)).await;
        assert_eq!((status, body["error"].as_str()), (StatusCode::FORBIDDEN, Some("person_required")));
        assert_eq!(card(&state, "per_sub").1, None, "still pending");
    }

    #[test]
    fn a_card_whose_headline_does_not_match_its_task_is_not_approvable() {
        let task = CardTask {
            capability: "image.generate".into(),
            provider: "chatgpt".into(),
            prompt: "a cat".into(),
            options: json!({}),
        };
        let good = json!({"summary": task.summary(), "task": task.to_json()});
        assert_eq!(displayed_card_task(&good.to_string()), Some(task.clone()));
        let forged = json!({"summary": "Make an image with your ChatGPT subscription: \"a cat\"", "task": {"capability": "image.generate", "provider": "chatgpt", "prompt": "something else"}});
        assert_eq!(displayed_card_task(&forged.to_string()), None);
        assert_eq!(displayed_card_task(&json!({"summary": "x"}).to_string()), None);
        assert_eq!(displayed_card_task("not json"), None);
    }

    #[tokio::test]
    async fn a_rejected_card_never_runs() {
        let (app, state, seen) = setup(true).await;
        let reply = rpc(&app, "tools/call", json!({"name": "presentation_create", "arguments": {"prompt": "deck"}})).await;
        let approval_id = tool_result(&reply).0["approval_id"].as_str().unwrap().to_string();
        state
            .db
            .connect()
            .unwrap()
            .execute("UPDATE cowork_approvals SET dismissed = 1, decision = 'rejected' WHERE id = ?1", params![approval_id])
            .unwrap();
        let reply = rpc(&app, "tools/call", json!({"name": "task_status", "arguments": {"approval_id": approval_id}})).await;
        assert_eq!(tool_result(&reply).0["status"], "rejected");
        assert_eq!(task_posts(&seen), 0);
    }
}
