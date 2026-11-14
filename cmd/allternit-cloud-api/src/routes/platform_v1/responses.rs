//! `POST /v1/responses`, `GET /v1/responses/{id}` and
//! `GET /v1/responses/{id}/input_items` (OpenAI Responses shape). Owned by
//! **lane 2 (Responses + Messages)** of the Platform API parity program.
//!
//! A request names exactly one of:
//!
//! * `model` (scope `inference`): runs on the in-process model router through
//!   [`bridge::run_items`] (spend check, hard lines and metering live there).
//!   The project pays: `tokens_in` / `tokens_out` usage rows at list price + 15%,
//!   idempotent per response (`resp:{id}:{meter}`), toward the spend cap.
//!   Client `tools` come back as `function_call` items (status `completed`, as at
//!   OpenAI); the client runs them and continues with `previous_response_id` and
//!   `function_call_output` items. `instructions` apply to this turn only.
//! * `agent` (scope `agents`): runs a turn of the agent on the project's hosted
//!   runtime, exactly like `POST /v1/conversations/{id}/messages` (same
//!   [`TurnLock`], [`conversations::prepare`], `host.stream` turn and
//!   [`conversations::meter_turn`], idempotent per response id). Only the new
//!   input text is sent; the runtime session holds the history.
//!
//! **Chains are conversations.** `conversation` names one; `previous_response_id`
//! resolves to its response's conversation and must be that conversation's latest
//! response (`409 previous_response_not_latest`); with neither, a conversation is
//! created (for the agent, or agent-less for a model). A conversation never mixes
//! modes (`400 invalid_request`). `store: false` (model mode only) stores
//! nothing: no conversation, no row, no items, so `GET` is a 404.
//!
//! **Items.** Input items are appended with the response id before the turn runs;
//! output items are appended `in_progress` when they open (so streamed item ids
//! are the stored ids) and updated once when they close.
//!
//! **Approvals.** When an agent's twin `call` tool needs the owner's approval,
//! that tool call fails ("try again once they approve") and the turn ends
//! `completed`. The approval arrives after `done` (its row links to this
//! response), so the live answer can't know; reads derive it instead: while a
//! linked approval is pending, `GET` returns `status: requires_action` and
//! `required_actions: [{type: approval, approval_id}]` ([`response_json_with`]).
//! The client resolves it (`POST /v1/approvals/{id}`) and sends a new response
//! with `previous_response_id`.
//!
//! **Every turn ends.** Once a response row exists, the turn runs in its own
//! task that owns the runtime stream, the items and the final write; the
//! request handler only awaits it. A client that hangs up (or Cloudflare's 524
//! on a non-streaming request after 100 s) drops the handler, never the turn,
//! so the response still ends `completed`, `failed` or `incomplete`. A hosted
//! turn past [`AgentHost::turn_cap`] (10 minutes) ends `failed` with
//! `turn_timeout` and a failed assistant item, and frees the conversation. Rows
//! left `in_progress` anyway (a crash or deploy mid-turn) are failed with
//! `turn_abandoned` by [`fail_abandoned`], which the platform worker runs at
//! startup and every 5 minutes.

use std::{collections::HashMap, convert::Infallible, sync::Arc, time::Duration};

use async_trait::async_trait;
use axum::{
    extract::{Path, State},
    response::{sse::Event, IntoResponse, Response, Sse},
    routing::{get, post},
    Extension, Json,
};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool};

use super::{
    agents, build_page,
    conversations::{self, ConvRow, TurnLock, TurnMeter},
    hosting::{host_for, AgentHost, SseLines, AGENT_TURN_CAP},
    items::{self, Item, NewItem},
    new_id, ApiJson, ApiQuery, PageParams, PlatformCaller, PlatformError, RouteTable,
};
use crate::model_router::items_bridge::{self as bridge, Completion, Delta, RunItems, RunOutput, UsageRef};
use crate::ApiState;

/// Same cap as a conversation message.
const MAX_AGENT_TEXT: usize = 16_000;
const MAX_INPUT_ITEMS: usize = 500;
const MAX_TOOLS: usize = 128;
/// Hosted tool arguments and results shown in items and events (spec).
const MAX_TOOL_CHARS: usize = 8_000;

pub fn register(table: RouteTable) -> RouteTable {
    table
        .add("/v1/responses", &["POST"], post(create))
        .add("/v1/responses/:id", &["GET"], get(fetch))
        .add("/v1/responses/:id/input_items", &["GET"], get(input_items))
}

type HostExt = Option<Extension<Arc<dyn AgentHost>>>;

// ---- request ------------------------------------------------------------------

/// The request body. Unknown fields (OpenAI options we don't run, like
/// `parallel_tool_calls` or `truncation`) are accepted and ignored so the
/// official SDKs work unchanged.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct CreateBody {
    model: Option<String>,
    agent: Option<String>,
    #[serde(default)]
    input: Value,
    instructions: Option<String>,
    #[serde(default)]
    tools: Vec<Value>,
    tool_choice: Option<Value>,
    #[serde(default)]
    stream: bool,
    store: Option<bool>,
    /// `conv_…`, or OpenAI's `{ "id": "conv_…" }`.
    conversation: Option<Value>,
    previous_response_id: Option<String>,
    metadata: Option<Value>,
    max_output_tokens: Option<u32>,
    temperature: Option<f32>,
}

fn invalid(param: &str, message: impl Into<String>) -> PlatformError {
    PlatformError::invalid_request("invalid_request", message).with_param(param)
}

fn mixed_modes() -> PlatformError {
    invalid("conversation", "A conversation runs either model turns or agent turns, not both.")
}

fn not_found() -> PlatformError {
    PlatformError::not_found("response_not_found", "No such response.")
}

fn nonempty(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

fn conversation_param(v: &Option<Value>) -> Result<Option<String>, PlatformError> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.trim().to_string())),
        Some(Value::Object(o)) => match o.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            Some(id) => Ok(Some(id.to_string())),
            None => Err(invalid("conversation", "conversation must be a conv_ id or {\"id\": \"conv_…\"}.")),
        },
        _ => Err(invalid("conversation", "conversation must be a conv_ id or {\"id\": \"conv_…\"}.")),
    }
}

fn check_metadata(m: &Option<Value>) -> Result<Value, PlatformError> {
    let v = m.clone().unwrap_or_else(|| json!({}));
    if !v.is_object() || v.to_string().len() > 8 * 1024 {
        return Err(PlatformError::invalid_request("invalid_metadata", "metadata must be a JSON object of at most 8 KB.").with_param("metadata"));
    }
    Ok(v)
}

/// Responses function tools → chat tools.
fn chat_tools(tools: &[Value]) -> Result<Vec<Value>, PlatformError> {
    if tools.len() > MAX_TOOLS {
        return Err(invalid("tools", format!("At most {MAX_TOOLS} tools.")));
    }
    tools
        .iter()
        .enumerate()
        .map(|(i, t)| {
            if t["type"].as_str() != Some("function") {
                return Err(invalid(&format!("tools[{i}].type"), "Only function tools are supported (type \"function\")."));
            }
            let name = t["name"].as_str().filter(|n| valid_tool_name(n)).ok_or_else(|| {
                invalid(&format!("tools[{i}].name"), "A tool name is 1-64 letters, digits, underscores or dashes.")
            })?;
            let params = t.get("parameters").filter(|p| !p.is_null());
            if params.is_some_and(|p| !p.is_object()) {
                return Err(invalid(&format!("tools[{i}].parameters"), "parameters must be a JSON Schema object."));
            }
            Ok(bridge::chat_tool(name, t["description"].as_str(), params))
        })
        .collect()
}

pub(crate) fn valid_tool_name(n: &str) -> bool {
    (1..=64).contains(&n.len()) && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Responses `tool_choice` → chat `tool_choice`.
fn chat_tool_choice(tc: &Option<Value>) -> Result<Option<Value>, PlatformError> {
    match tc {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if matches!(s.as_str(), "auto" | "none" | "required") => Ok(Some(json!(s))),
        Some(v) if v["type"].as_str() == Some("function") => match v["name"].as_str().or_else(|| v["function"]["name"].as_str()) {
            Some(name) => Ok(Some(json!({ "type": "function", "function": { "name": name } }))),
            None => Err(invalid("tool_choice", "A function tool_choice needs a name.")),
        },
        Some(_) => Err(invalid("tool_choice", "tool_choice must be auto, none, required or {\"type\":\"function\",\"name\":…}.")),
    }
}

/// An `input_image` given only by `file_id`: there is no Files API on this surface.
pub(crate) fn refuse_file_images(input: &[NewItem]) -> Result<(), PlatformError> {
    for (i, it) in input.iter().enumerate() {
        for (j, p) in it.content.as_array().into_iter().flatten().enumerate() {
            if p["type"].as_str() == Some("input_image") && p["image_url"].as_str().is_none() {
                return Err(PlatformError::invalid_request("invalid_input", "Images by file_id aren't supported; send image_url (an https or data: URL).")
                    .with_param(&format!("input[{i}].content[{j}]")));
            }
        }
    }
    Ok(())
}

/// The text an agent turn takes: the user messages' text parts.
fn agent_text(input: &[NewItem]) -> Result<String, PlatformError> {
    let mut parts = Vec::new();
    for (i, it) in input.iter().enumerate() {
        if it.kind != "message" || it.role.as_deref() != Some("user") {
            return Err(invalid(&format!("input[{i}]"), "An agent turn takes user messages only (its tools run on the runtime)."));
        }
        let mut text = String::new();
        for (j, p) in it.content.as_array().into_iter().flatten().enumerate() {
            match p["type"].as_str() {
                Some("input_text") => text.push_str(p["text"].as_str().unwrap_or("")),
                _ => return Err(invalid(&format!("input[{i}].content[{j}]"), "An agent turn takes text only.")),
            }
        }
        parts.push(text);
    }
    let text = parts.join("\n\n").trim().to_string();
    if text.is_empty() || text.chars().count() > MAX_AGENT_TEXT {
        return Err(invalid("input", format!("An agent turn takes 1 to {MAX_AGENT_TEXT} characters of text.")));
    }
    Ok(text)
}

// ---- rows ---------------------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
struct RespRow {
    id: String,
    account_id: Option<String>,
    agent_id: Option<String>,
    model: Option<String>,
    conversation_id: Option<String>,
    previous_response_id: Option<String>,
    status: String,
    output: Value,
    usage: Value,
    error: Option<Value>,
    metadata: Value,
    created_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
}

const RESP_COLUMNS: &str =
    "id, account_id, agent_id, model, conversation_id, previous_response_id, status, output, usage, error, metadata, created_at, completed_at";
const ITEM_COLUMNS: &str = "id, seq, project_id, conversation_id, response_id, type, role, status, content, legacy_id, error, created_at";

impl RespRow {
    fn new(id: String, account_id: Option<String>, agent_id: Option<String>, model: Option<String>, conversation_id: Option<String>, previous: Option<String>, metadata: Value) -> Self {
        Self {
            id,
            account_id,
            agent_id,
            model,
            conversation_id,
            previous_response_id: previous,
            status: "in_progress".into(),
            output: json!([]),
            usage: json!({}),
            error: None,
            metadata,
            created_at: Utc::now(),
            completed_at: None,
        }
    }
}

/// The public response object as the turn itself reports it (no pending
/// approvals yet: they arrive after `done`). `incomplete_details` says why a
/// response is `incomplete`, as OpenAI's does; the only cause is the model
/// stopping at `max_output_tokens` ([`model_end`]).
fn response_json(r: &RespRow) -> Value {
    response_json_with(r, &[])
}

/// The public response object with `pending` = the ids of the approvals its
/// turn raised that are still pending, oldest first
/// (`approvals::pending_for_response`). Any → `status: requires_action` and one
/// `required_actions` entry each, once the turn is over; none → the stored status.
fn response_json_with(r: &RespRow, pending: &[String]) -> Value {
    let incomplete = (r.status == "incomplete").then(|| json!({ "reason": "max_output_tokens" }));
    let waiting = !pending.is_empty() && r.status != "in_progress";
    let status = if waiting { "requires_action" } else { r.status.as_str() };
    let actions: Vec<Value> = if waiting { pending.iter().map(|id| json!({ "type": "approval", "approval_id": id })).collect() } else { Vec::new() };
    json!({
        "id": r.id, "object": "response", "status": status, "model": r.model, "agent_id": r.agent_id,
        "conversation_id": r.conversation_id, "previous_response_id": r.previous_response_id,
        "output": r.output,
        "incomplete_details": incomplete,
        "required_actions": actions,
        "usage": { "input_tokens": r.usage["input_tokens"].as_i64().unwrap_or(0), "output_tokens": r.usage["output_tokens"].as_i64().unwrap_or(0) },
        "error": r.error, "metadata": r.metadata, "created_at": r.created_at, "completed_at": r.completed_at,
    })
}

async fn insert_row(db: &PgPool, project_id: &str, r: &RespRow) -> Result<RespRow, PlatformError> {
    Ok(sqlx::query_as::<_, RespRow>(&format!(
        "INSERT INTO platform_responses (id, project_id, account_id, agent_id, model, conversation_id, previous_response_id, status, metadata, store) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'in_progress', $8, true) RETURNING {RESP_COLUMNS}"
    ))
    .bind(&r.id)
    .bind(project_id)
    .bind(&r.account_id)
    .bind(&r.agent_id)
    .bind(&r.model)
    .bind(&r.conversation_id)
    .bind(&r.previous_response_id)
    .bind(&r.metadata)
    .fetch_one(db)
    .await?)
}

/// A stored response the caller may see (`resp_` rows only; `mdl_` rows are
/// `/v1/messages` threads).
async fn load_row(db: &PgPool, caller: &PlatformCaller, id: &str) -> Result<RespRow, PlatformError> {
    let account = caller.account_filter(None)?;
    sqlx::query_as::<_, RespRow>(&format!(
        "SELECT {RESP_COLUMNS} FROM platform_responses \
         WHERE id = $1 AND project_id = $2 AND store AND left(id, 5) = 'resp_' AND ($3::text IS NULL OR account_id = $3)"
    ))
    .bind(id)
    .bind(&caller.project_id)
    .bind(&account)
    .fetch_optional(db)
    .await?
    .ok_or_else(not_found)
}

/// The id of a conversation's latest response or model message, if any.
pub(crate) async fn latest_in_conversation(db: &PgPool, conversation_id: &str) -> Result<Option<String>, PlatformError> {
    Ok(sqlx::query_scalar::<_, String>("SELECT id FROM platform_responses WHERE conversation_id = $1 ORDER BY created_at DESC, id DESC LIMIT 1")
        .bind(conversation_id)
        .fetch_optional(db)
        .await?)
}

/// An agent-less conversation for a model chain (`account_id` = the key's account, if bound).
pub(crate) async fn open_model_conversation(db: &PgPool, caller: &PlatformCaller) -> Result<String, PlatformError> {
    let id = new_id("conv_");
    sqlx::query("INSERT INTO platform_conversations (id, project_id, account_id, agent_id, metadata) VALUES ($1, $2, $3, NULL, '{}'::jsonb)")
        .bind(&id)
        .bind(&caller.project_id)
        .bind(&caller.account_id)
        .execute(db)
        .await?;
    Ok(id)
}

async fn drop_conversation(db: &PgPool, id: &str) {
    if let Err(error) = sqlx::query("DELETE FROM platform_conversations WHERE id = $1").bind(id).execute(db).await {
        tracing::warn!(%error, conversation = id, "platform: couldn't remove an unused response conversation");
    }
}

/// The chain a request continues: `conversation`, or the conversation of
/// `previous_response_id` (which must be its latest response).
async fn resolve_chain(db: &PgPool, caller: &PlatformCaller, conversation: Option<&str>, previous: Option<&str>) -> Result<Option<ConvRow>, PlatformError> {
    if let Some(c) = conversation {
        return conversations::fetch_conversation(db, caller, c).await.map(Some);
    }
    let Some(prev) = previous else { return Ok(None) };
    let row = load_row(db, caller, prev).await.map_err(|e| e.with_param("previous_response_id"))?;
    let conv_id = row.conversation_id.ok_or_else(|| not_found().with_param("previous_response_id"))?;
    if latest_in_conversation(db, &conv_id).await?.as_deref() != Some(prev) {
        return Err(PlatformError::conflict("previous_response_not_latest", "previous_response_id is not the latest response of its conversation; continue from the latest one.")
            .with_param("previous_response_id"));
    }
    conversations::fetch_conversation(db, caller, &conv_id).await.map(Some)
}

// ---- output items: open, stream, close -------------------------------------------

/// Where output items go: the items table, or nowhere (`store: false`).
#[async_trait]
trait ItemSink: Send {
    async fn open(&mut self, item: NewItem) -> Result<Item, PlatformError>;
    async fn close(&mut self, item: &Item) -> Result<(), PlatformError>;
}

struct DbSink {
    db: PgPool,
    project_id: String,
    conversation_id: Option<String>,
    response_id: String,
}

#[async_trait]
impl ItemSink for DbSink {
    async fn open(&mut self, item: NewItem) -> Result<Item, PlatformError> {
        items::append(&self.db, &self.project_id, self.conversation_id.as_deref(), Some(&self.response_id), vec![item])
            .await?
            .pop()
            .ok_or_else(|| PlatformError::api_error("internal_error", "An item wasn't stored."))
    }

    async fn close(&mut self, item: &Item) -> Result<(), PlatformError> {
        sqlx::query("UPDATE platform_items SET status = $2, content = $3, error = $4 WHERE id = $1")
            .bind(&item.id)
            .bind(&item.status)
            .bind(&item.content)
            .bind(&item.error)
            .execute(&self.db)
            .await?;
        Ok(())
    }
}

/// `store: false` (and tests): items get ids but are kept nowhere.
struct MemSink {
    project_id: String,
    response_id: String,
    seq: i64,
}

#[async_trait]
impl ItemSink for MemSink {
    async fn open(&mut self, n: NewItem) -> Result<Item, PlatformError> {
        self.seq += 1;
        Ok(Item {
            id: new_id("item_"),
            seq: self.seq,
            project_id: self.project_id.clone(),
            conversation_id: None,
            response_id: Some(self.response_id.clone()),
            kind: n.kind,
            role: n.role,
            status: n.status,
            content: n.content,
            legacy_id: None,
            error: n.error,
            created_at: Utc::now(),
        })
    }

    async fn close(&mut self, _item: &Item) -> Result<(), PlatformError> {
        Ok(())
    }
}

/// An SSE event before numbering: (event name, data).
type Ev = (String, Value);

fn ev(name: &str, data: Value) -> Ev {
    (name.to_string(), data)
}

/// Cut a hosted tool's input/output to [`MAX_TOOL_CHARS`] (a string stays a string).
fn cut(v: &Value) -> Value {
    match v {
        Value::Null => Value::Null,
        Value::String(s) if s.chars().count() <= MAX_TOOL_CHARS => v.clone(),
        Value::String(s) => Value::String(s.chars().take(MAX_TOOL_CHARS).collect()),
        other => {
            let text = other.to_string();
            if text.chars().count() <= MAX_TOOL_CHARS {
                other.clone()
            } else {
                Value::String(text.chars().take(MAX_TOOL_CHARS).collect())
            }
        }
    }
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq)]
enum End {
    Completed,
    /// The model stopped at `max_output_tokens`.
    Incomplete,
    Failed(String),
}

/// Builds the output items of one response and the events that describe them.
#[derive(Default)]
struct OutputFold {
    output: Vec<Item>,
    /// The assistant message being written: its output index and text so far.
    text: Option<(usize, String)>,
    /// Hosted tool steps by the runtime's `toolCallId`.
    tools: HashMap<String, usize>,
    /// Client function calls by the model's tool-call index.
    calls: Vec<(usize, usize)>,
}

impl OutputFold {
    async fn open(&mut self, sink: &mut dyn ItemSink, item: NewItem) -> Result<(usize, Vec<Ev>), PlatformError> {
        let stored = sink.open(item).await?;
        let index = self.output.len();
        let added = ev("response.output_item.added", json!({ "output_index": index, "item": items::item_json(&stored) }));
        self.output.push(stored);
        Ok((index, vec![added]))
    }

    async fn close(&mut self, sink: &mut dyn ItemSink, index: usize) -> Result<Vec<Ev>, PlatformError> {
        sink.close(&self.output[index]).await?;
        Ok(vec![ev("response.output_item.done", json!({ "output_index": index, "item": items::item_json(&self.output[index]) }))])
    }

    async fn text(&mut self, sink: &mut dyn ItemSink, delta: &str) -> Result<Vec<Ev>, PlatformError> {
        let mut out = Vec::new();
        if self.text.is_none() {
            let (index, added) = self.open(sink, NewItem::assistant_text("").with_status("in_progress")).await?;
            out.extend(added);
            self.text = Some((index, String::new()));
        }
        let Some((index, text)) = self.text.as_mut() else { return Ok(out) };
        text.push_str(delta);
        out.push(ev("response.output_text.delta", json!({ "item_id": self.output[*index].id, "output_index": index, "content_index": 0, "delta": delta })));
        Ok(out)
    }

    async fn close_text(&mut self, sink: &mut dyn ItemSink, end: &End) -> Result<Vec<Ev>, PlatformError> {
        let Some((index, text)) = self.text.take() else { return Ok(vec![]) };
        {
            let it = &mut self.output[index];
            it.content = json!([{ "type": "output_text", "text": text }]);
            match end {
                End::Completed => it.status = "completed".into(),
                End::Incomplete => it.status = "incomplete".into(),
                End::Failed(f) => {
                    it.status = "failed".into();
                    it.error = Some(f.clone());
                }
            }
        }
        let mut out = vec![ev("response.output_text.done", json!({ "item_id": self.output[index].id, "output_index": index, "content_index": 0, "text": text }))];
        out.extend(self.close(sink, index).await?);
        Ok(out)
    }

    /// A runtime `tool.started`.
    async fn tool_started(&mut self, sink: &mut dyn ItemSink, e: &Value) -> Result<Vec<Ev>, PlatformError> {
        let mut out = self.close_text(sink, &End::Completed).await?;
        let name = e["name"].as_str().unwrap_or("tool");
        let call_id = e["toolCallId"].as_str().filter(|c| !c.is_empty());
        let (index, added) = self.open(sink, NewItem::tool_call(name, cut(&e["input"]), Value::Null, "in_progress", None).with_call_id(call_id)).await?;
        out.extend(added);
        if let Some(id) = call_id {
            self.tools.insert(id.to_string(), index);
        }
        let mut data = json!({ "item_id": self.output[index].id, "output_index": index, "name": name, "input": cut(&e["input"]) });
        if let Some(id) = call_id {
            data["call_id"] = json!(id);
        }
        out.push(ev("response.tool_call.started", data));
        Ok(out)
    }

    /// A runtime `tool.completed` / `tool.failed`.
    async fn tool_ended(&mut self, sink: &mut dyn ItemSink, e: &Value, failed: bool) -> Result<Vec<Ev>, PlatformError> {
        let mut out = Vec::new();
        let known = e["toolCallId"].as_str().and_then(|id| self.tools.remove(id));
        let index = match known {
            Some(i) => i,
            None => {
                out.extend(self.close_text(sink, &End::Completed).await?);
                let name = e["name"].as_str().unwrap_or("tool");
                let call_id = e["toolCallId"].as_str().filter(|c| !c.is_empty());
                let (i, added) = self.open(sink, NewItem::tool_call(name, cut(&e["input"]), Value::Null, "in_progress", None).with_call_id(call_id)).await?;
                out.extend(added);
                i
            }
        };
        let error = e["error"].as_str().map(|s| s.chars().take(MAX_TOOL_CHARS).collect::<String>());
        {
            let it = &mut self.output[index];
            if !e["input"].is_null() {
                it.content["input"] = cut(&e["input"]);
            }
            it.content["output"] = cut(&e["output"]);
            it.content["error"] = json!(error);
            it.status = if failed { "failed" } else { "completed" }.into();
        }
        let it = &self.output[index];
        let mut data = json!({ "item_id": it.id, "output_index": index, "name": it.content["name"], "input": it.content["input"] });
        if !it.content["call_id"].is_null() {
            data["call_id"] = it.content["call_id"].clone();
        }
        if failed {
            data["error"] = json!(error);
            out.push(ev("response.tool_call.failed", data));
        } else {
            data["output"] = it.content["output"].clone();
            out.push(ev("response.tool_call.completed", data));
        }
        out.extend(self.close(sink, index).await?);
        Ok(out)
    }

    /// The model started a client function call.
    async fn call_started(&mut self, sink: &mut dyn ItemSink, call_index: usize, id: &str, name: &str) -> Result<Vec<Ev>, PlatformError> {
        let mut out = self.close_text(sink, &End::Completed).await?;
        let (index, added) = self.open(sink, NewItem::function_call(id, name, "").with_status("in_progress")).await?;
        out.extend(added);
        self.calls.push((call_index, index));
        Ok(out)
    }

    fn call_args(&mut self, call_index: usize, delta: &str) -> Vec<Ev> {
        let Some(&(_, index)) = self.calls.iter().find(|(c, _)| *c == call_index) else { return vec![] };
        let it = &mut self.output[index];
        let args = format!("{}{delta}", it.content["arguments"].as_str().unwrap_or(""));
        it.content["arguments"] = json!(args);
        vec![ev("response.function_call_arguments.delta", json!({ "item_id": it.id, "output_index": index, "delta": delta }))]
    }

    /// Close whatever is still open.
    async fn finish(&mut self, sink: &mut dyn ItemSink, end: &End) -> Result<Vec<Ev>, PlatformError> {
        let mut out = self.close_text(sink, end).await?;
        for (_, index) in std::mem::take(&mut self.calls) {
            {
                let it = &mut self.output[index];
                if it.content["arguments"].as_str().unwrap_or("").is_empty() {
                    it.content["arguments"] = json!("{}");
                }
                it.status = match end {
                    End::Completed => "completed",
                    End::Incomplete => "incomplete",
                    End::Failed(_) => "failed",
                }
                .into();
            }
            let it = &self.output[index];
            out.push(ev("response.function_call_arguments.done", json!({ "item_id": it.id, "output_index": index, "arguments": it.content["arguments"] })));
            out.extend(self.close(sink, index).await?);
        }
        for (_, index) in std::mem::take(&mut self.tools) {
            {
                let it = &mut self.output[index];
                it.status = "failed".into();
                it.content["error"] = json!("the turn ended before the tool finished");
            }
            out.extend(self.close(sink, index).await?);
        }
        Ok(out)
    }

    fn output_json(&self) -> Value {
        Value::Array(self.output.iter().map(items::item_json).collect())
    }

    /// A finished model turn (non-streaming) as the same item sequence a stream makes.
    async fn completion(&mut self, sink: &mut dyn ItemSink, c: &Completion) -> Result<(), PlatformError> {
        if !c.text.is_empty() {
            self.text(sink, &c.text).await?;
        }
        for (i, tc) in c.tool_calls.iter().enumerate() {
            self.call_started(sink, i, &tc.id, &tc.name).await?;
            self.call_args(i, &tc.arguments);
        }
        Ok(())
    }
}

/// Numbers events and sends them to the stream (if any; non-streaming turns
/// build the same items with no listener).
struct Emitter {
    tx: Option<tokio::sync::mpsc::UnboundedSender<Event>>,
    seq: u64,
}

impl Emitter {
    fn send(&mut self, evs: Vec<Ev>) {
        for (name, mut data) in evs {
            if let Some(tx) = &self.tx {
                data["type"] = json!(name);
                data["sequence_number"] = json!(self.seq);
                // The developer hung up: keep going so the turn is still stored.
                let _ = tx.send(items::sse(&name, &data));
            }
            self.seq += 1;
        }
    }
}

/// Everything the end of a turn needs.
struct Finish {
    db: PgPool,
    store: bool,
    row: RespRow,
    /// `error.code` when the turn fails: `turn_failed`, or `turn_timeout`.
    error_code: &'static str,
}

impl Finish {
    /// Write the outcome (stored responses) and send the closing events. Returns
    /// the response JSON.
    async fn done(mut self, emit: &mut Emitter, fold: &OutputFold, end: &End, usage: Value) -> Value {
        self.row.status = match end {
            End::Completed => "completed",
            End::Incomplete => "incomplete",
            End::Failed(_) => "failed",
        }
        .into();
        self.row.output = fold.output_json();
        self.row.usage = usage;
        self.row.error = match end {
            End::Failed(f) => Some(json!({ "code": self.error_code, "message": f })),
            _ => None,
        };
        self.row.completed_at = Some(Utc::now());
        if self.store {
            let stored = sqlx::query_as::<_, RespRow>(&format!(
                "UPDATE platform_responses SET status = $2, output = $3, usage = $4, error = $5, completed_at = NOW() WHERE id = $1 RETURNING {RESP_COLUMNS}"
            ))
            .bind(&self.row.id)
            .bind(&self.row.status)
            .bind(&self.row.output)
            .bind(&self.row.usage)
            .bind(&self.row.error)
            .fetch_one(&self.db)
            .await;
            match stored {
                Ok(r) => self.row = r,
                Err(error) => tracing::error!(%error, response = %self.row.id, "platform: response outcome not stored"),
            }
        }
        let body = response_json(&self.row);
        let last = match end {
            End::Completed => "response.completed",
            End::Incomplete => "response.incomplete",
            End::Failed(_) => "response.failed",
        };
        emit.send(vec![ev(last, json!({ "response": body }))]);
        body
    }
}

// ---- agent turns --------------------------------------------------------------

/// How long a turn may run, in words.
fn cap_words(cap: Duration) -> String {
    match cap.as_secs() {
        s if s >= 60 && s % 60 == 0 => format!("{} minutes", s / 60),
        s if s >= 1 => format!("{s} seconds"),
        _ => format!("{} ms", cap.as_millis()),
    }
}

/// Read an agent turn to its end, building items and events as it goes. Runs
/// in its own task (see [`agent_turn`]); at most `cap` is spent waiting on the
/// runtime, then the turn ends `failed` (`turn_timeout`).
async fn drive_agent(
    mut upstream: crate::routes::voice_calls_cloud::RelayStream,
    mut sink: DbSink,
    mut fin: Finish,
    meter: TurnMeter,
    lock: TurnLock,
    mut emit: Emitter,
    cap: Duration,
) -> Value {
    emit.send(vec![ev("response.created", json!({ "response": response_json(&fin.row) }))]);
    let mut fold = OutputFold::default();
    let (mut lines, mut failure, mut done, mut usage) = (SseLines::default(), None::<String>, false, Value::Null);
    let deadline = tokio::time::Instant::now() + cap;
    let mut timed_out = false;
    'read: loop {
        let chunk = match tokio::time::timeout_at(deadline, upstream.next()).await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break 'read,
            Err(_) => {
                timed_out = true;
                failure = Some(format!("the agent didn't finish within {}", cap_words(cap)));
                break 'read;
            }
        };
        let bytes = match chunk {
            Ok(b) => b,
            Err(e) => {
                failure = Some(format!("the agent's reply was cut off: {e}"));
                break 'read;
            }
        };
        for e in lines.push(&bytes) {
            let step = match e["type"].as_str() {
                Some("text.delta") => fold.text(&mut sink, e["text"].as_str().unwrap_or("")).await,
                Some("tool.started") => fold.tool_started(&mut sink, &e).await,
                Some("tool.completed") => fold.tool_ended(&mut sink, &e, false).await,
                Some("tool.failed") => fold.tool_ended(&mut sink, &e, true).await,
                Some("done") => {
                    usage = e["usage"].clone();
                    done = true;
                    // A runtime that sent the reply only on `done`.
                    match e["text"].as_str().filter(|t| !t.is_empty() && fold.output.is_empty()) {
                        Some(t) => fold.text(&mut sink, t).await,
                        None => Ok(vec![]),
                    }
                }
                Some("error") => {
                    failure = Some(e["message"].as_str().unwrap_or("the agent's turn failed").to_string());
                    usage = e["usage"].clone();
                    Ok(vec![])
                }
                _ => Ok(vec![]),
            };
            match step {
                Ok(evs) => emit.send(evs),
                Err(err) => {
                    failure = Some(format!("the reply couldn't be stored: {}", err.message));
                    break 'read;
                }
            }
        }
    }
    // Stop reading from the runtime now (a timed-out turn closes its relay).
    drop(upstream);
    let failure = failure.or_else(|| (!done).then(|| "the agent's reply ended early".to_string()));
    let end = failure.clone().map(End::Failed).unwrap_or(End::Completed);
    if timed_out {
        fin.error_code = "turn_timeout";
        // The conversation shows the failed reply even when the runtime never answered.
        let answered = fold.text.is_some() || fold.output.iter().any(|it| it.kind == "message" && it.role.as_deref() == Some("assistant"));
        if !answered {
            match fold.open(&mut sink, NewItem::assistant_text("").with_status("in_progress")).await {
                Ok((index, added)) => {
                    fold.text = Some((index, String::new()));
                    emit.send(added);
                }
                Err(err) => tracing::error!(response = %fin.row.id, "platform: failed reply not stored: {}", err.message),
            }
        }
    }
    match fold.finish(&mut sink, &end).await {
        Ok(evs) => emit.send(evs),
        Err(err) => tracing::error!(response = %fin.row.id, "platform: output items not closed: {}", err.message),
    }
    conversations::meter_turn(&fin.db, &meter, &fin.row.id, &usage, failure.as_deref()).await;
    let tokens = json!({ "input_tokens": usage["inputTokens"].as_i64().unwrap_or(0), "output_tokens": usage["outputTokens"].as_i64().unwrap_or(0) });
    let body = fin.done(&mut emit, &fold, &end, tokens).await;
    drop(lock);
    body
}

enum Created {
    Json(Value),
    Stream(tokio::sync::mpsc::UnboundedReceiver<Event>),
}

fn emitter(stream: bool) -> (Emitter, Option<tokio::sync::mpsc::UnboundedReceiver<Event>>) {
    if stream {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (Emitter { tx: Some(tx), seq: 0 }, Some(rx))
    } else {
        (Emitter { tx: None, seq: 0 }, None)
    }
}

async fn agent_turn(state: &Arc<ApiState>, caller: &PlatformCaller, host: Arc<dyn AgentHost>, agent_id: &str, body: CreateBody, input: Vec<NewItem>, chain: Option<ConvRow>, metadata: Value) -> Result<Created, PlatformError> {
    caller.require("agents")?;
    if body.store == Some(false) {
        return Err(invalid("store", "Agent turns are always stored; store: false works with model only."));
    }
    if !body.tools.is_empty() {
        return Err(invalid("tools", "An agent runs its own tools; client tools work with model only."));
    }
    for (param, set) in [
        ("instructions", body.instructions.is_some()),
        ("tool_choice", body.tool_choice.is_some()),
        ("temperature", body.temperature.is_some()),
        ("max_output_tokens", body.max_output_tokens.is_some()),
    ] {
        if set {
            return Err(invalid(param, format!("{param} works with model only; an agent runs on its own settings.")));
        }
    }
    let agent = agents::fetch_visible(state, caller, agent_id).await.map_err(|e| e.with_param("agent"))?;
    let text = agent_text(&input)?;
    let db = &state.db;
    super::spend_allowed(db, &caller.project_id).await?;
    let (conv, created) = match chain {
        Some(c) if c.agent_id.as_deref() == Some(agent.id.as_str()) => (c, false),
        Some(_) => return Err(mixed_modes()),
        None => {
            let id = conversations::open(db, &caller.project_id, &agent.account_id, &agent.id, &json!({})).await?;
            (conversations::fetch_conversation(db, caller, &id).await?, true)
        }
    };
    let started = async {
        let lock = TurnLock::take(&conv.id).ok_or_else(|| PlatformError::conflict("conversation_busy", "The agent is still answering the previous turn in this conversation."))?;
        let (rt, session, own_key) = conversations::prepare(state, host.as_ref(), caller, &conv).await?;
        let meter = TurnMeter::new(caller, &conv, own_key)?.with_cloud_price(&state.model_router).await;
        let path = format!("/api/v1/platform/agents/{}/sessions/{}/turn", meter.agent_id, session);
        let upstream = host.stream(&rt, &path, &json!({ "text": text })).await?;
        Ok::<_, PlatformError>((lock, meter, upstream))
    }
    .await;
    let (lock, meter, upstream) = match started {
        Ok(s) => s,
        Err(e) => {
            if created {
                drop_conversation(db, &conv.id).await;
            }
            return Err(e);
        }
    };
    // Stored once the runtime took the turn: a refused turn leaves no response behind.
    let row = RespRow::new(new_id("resp_"), conv.account_id.clone(), Some(agent.id.clone()), None, Some(conv.id.clone()), body.previous_response_id.clone(), metadata);
    let row = insert_row(db, &caller.project_id, &row).await?;
    if let Err(e) = items::append(db, &caller.project_id, Some(&conv.id), Some(&row.id), input).await {
        fail_one(db, &row.id, "turn_failed", "The request couldn't be stored.").await;
        return Err(e);
    }
    let resp_id = row.id.clone();
    let sink = DbSink { db: db.clone(), project_id: caller.project_id.clone(), conversation_id: Some(conv.id.clone()), response_id: row.id.clone() };
    let fin = Finish { db: db.clone(), store: true, row, error_code: "turn_failed" };
    let (emit, rx) = emitter(body.stream);
    // The turn owns everything it needs: dropping this handler (the client hung
    // up) leaves it running to its stored end.
    let turn = tokio::spawn(drive_agent(upstream, sink, fin, meter, lock, emit, host.turn_cap()));
    match rx {
        None => Ok(Created::Json(joined(db, &resp_id, turn).await?)),
        Some(rx) => Ok(Created::Stream(rx)),
    }
}

/// The answer of a detached turn the handler waits on. A turn task that
/// panicked leaves its response failed, not running.
async fn joined(db: &PgPool, resp_id: &str, turn: tokio::task::JoinHandle<Value>) -> Result<Value, PlatformError> {
    match turn.await {
        Ok(body) => Ok(body),
        Err(error) => {
            tracing::error!(%error, response = resp_id, "platform: response turn task died");
            fail_one(db, resp_id, "turn_failed", "The turn stopped unexpectedly.").await;
            Err(PlatformError::api_error("internal_error", "An internal error occurred. Please retry."))
        }
    }
}

// ---- responses that can't finish ---------------------------------------------

/// A row still `in_progress` this long after it was created has no live turn
/// behind it (turns end by [`AGENT_TURN_CAP`]); the margin covers the last writes.
pub const ABANDON_AFTER: Duration = Duration::from_secs(AGENT_TURN_CAP.as_secs() + 5 * 60);

/// Fail `in_progress` responses: `id` alone, or every row older than
/// `older_than`. Each gets `status: failed`, `error {code, message}` and
/// `completed_at`; its open items fail with it, and a response with no assistant
/// message gets a failed one so its conversation shows the failure. Rows that
/// already ended are never touched. Returns how many responses it failed.
async fn fail_in_progress(db: &PgPool, id: Option<&str>, older_than: Duration, code: &str, message: &str) -> Result<u64, sqlx::Error> {
    // Uses idx_platform_responses_in_progress (migration 138): only running rows are indexed.
    // Postgres runs every data-modifying CTE to completion whether or not the
    // final SELECT reads it.
    let n: i64 = sqlx::query_scalar(
        "WITH stuck AS ( \
           UPDATE platform_responses \
              SET status = 'failed', error = jsonb_build_object('code', $3::text, 'message', $4::text), completed_at = NOW() \
            WHERE status = 'in_progress' AND ($1::text IS NULL OR id = $1) \
              AND created_at <= NOW() - make_interval(secs => $2::float8) \
           RETURNING id, project_id, conversation_id \
         ), closed AS ( \
           UPDATE platform_items SET status = 'failed', error = COALESCE(error, $4) \
            WHERE response_id IN (SELECT id FROM stuck) AND status = 'in_progress' \
           RETURNING response_id \
         ), replies AS ( \
           INSERT INTO platform_items (id, project_id, conversation_id, response_id, type, role, status, content, error) \
           SELECT 'item_' || substr(md5(random()::text || s.id), 1, 24), s.project_id, s.conversation_id, s.id, \
                  'message', 'assistant', 'failed', '[{\"type\":\"output_text\",\"text\":\"\"}]'::jsonb, $4 \
             FROM stuck s \
            WHERE NOT EXISTS (SELECT 1 FROM platform_items i WHERE i.response_id = s.id AND i.type = 'message' AND i.role = 'assistant') \
           RETURNING 1 \
         ) \
         SELECT COUNT(*) FROM stuck",
    )
    .bind(id)
    .bind(older_than.as_secs_f64())
    .bind(code)
    .bind(message)
    .fetch_one(db)
    .await?;
    Ok(n as u64)
}

/// Fail one response whose turn can't finish (best effort; logged).
async fn fail_one(db: &PgPool, id: &str, code: &str, message: &str) {
    if let Err(error) = fail_in_progress(db, Some(id), Duration::ZERO, code, message).await {
        tracing::error!(%error, response = id, "platform: couldn't fail a response that can't finish");
    }
}

/// The sweep: responses (and `/v1/messages` thread turns) still `in_progress`
/// after [`ABANDON_AFTER`] → `failed` with `turn_abandoned`. Cheap when nothing
/// is stuck (a partial index holds only running rows). Run by the platform
/// worker at startup and every 5 minutes.
pub async fn fail_abandoned(db: &PgPool) -> Result<u64, sqlx::Error> {
    fail_abandoned_after(db, ABANDON_AFTER).await
}

async fn fail_abandoned_after(db: &PgPool, older_than: Duration) -> Result<u64, sqlx::Error> {
    fail_in_progress(db, None, older_than, "turn_abandoned", "The turn stopped without an answer; send the request again.").await
}

// ---- model turns --------------------------------------------------------------

/// Map a finished model turn's stop onto how the response ended.
fn model_end(finish_reason: Option<&str>) -> End {
    match finish_reason {
        Some("length") => End::Incomplete,
        _ => End::Completed,
    }
}

async fn drive_model(run: RunOutput, mut sink: Box<dyn ItemSink>, fin: Finish, lock: Option<TurnLock>, mut emit: Emitter) -> Value {
    emit.send(vec![ev("response.created", json!({ "response": response_json(&fin.row) }))]);
    let mut fold = OutputFold::default();
    let (end, usage) = match run {
        RunOutput::Complete(c) => match fold.completion(sink.as_mut(), &c).await {
            Ok(()) => (model_end(c.finish_reason.as_deref()), (c.input_tokens, c.output_tokens)),
            Err(err) => (End::Failed(format!("the reply couldn't be stored: {}", err.message)), (c.input_tokens, c.output_tokens)),
        },
        RunOutput::Stream(mut rx) => {
            let mut result = None;
            while let Some(d) = rx.recv().await {
                let step = match d {
                    Delta::Text(t) => fold.text(sink.as_mut(), &t).await,
                    Delta::ToolStart { index, id, name } => fold.call_started(sink.as_mut(), index, &id, &name).await,
                    Delta::ToolArgs { index, delta } => Ok(fold.call_args(index, &delta)),
                    Delta::Done(c) => {
                        result = Some((model_end(c.finish_reason.as_deref()), (c.input_tokens, c.output_tokens)));
                        break;
                    }
                    Delta::Failed(f) => {
                        result = Some((End::Failed(f), (0, 0)));
                        break;
                    }
                };
                match step {
                    Ok(evs) => emit.send(evs),
                    Err(err) => {
                        // Keep draining so the router task settles; the reply is lost.
                        result = Some((End::Failed(format!("the reply couldn't be stored: {}", err.message)), (0, 0)));
                        while rx.recv().await.is_some() {}
                        break;
                    }
                }
            }
            result.unwrap_or_else(|| (End::Failed("the model's answer ended early".into()), (0, 0)))
        }
    };
    match fold.finish(sink.as_mut(), &end).await {
        Ok(evs) => emit.send(evs),
        Err(err) => tracing::error!(response = %fin.row.id, "platform: output items not closed: {}", err.message),
    }
    let body = fin.done(&mut emit, &fold, &end, json!({ "input_tokens": usage.0, "output_tokens": usage.1 })).await;
    drop(lock);
    body
}

async fn model_turn(state: &Arc<ApiState>, caller: &PlatformCaller, model: &str, body: CreateBody, input: Vec<NewItem>, chain: Option<ConvRow>, conversation_given: bool, metadata: Value) -> Result<Created, PlatformError> {
    caller.require("inference")?;
    let tools = chat_tools(&body.tools)?;
    let tool_choice = chat_tool_choice(&body.tool_choice)?;
    if body.temperature.is_some_and(|t| !(0.0..=2.0).contains(&t)) {
        return Err(invalid("temperature", "temperature must be between 0 and 2."));
    }
    if body.max_output_tokens == Some(0) {
        return Err(invalid("max_output_tokens", "max_output_tokens must be at least 1."));
    }
    refuse_file_images(&input)?;
    let store = body.store.unwrap_or(true);
    if !store && conversation_given {
        return Err(invalid("store", "store: false can't add to a conversation; drop conversation or store."));
    }
    if chain.as_ref().is_some_and(|c| c.agent_id.is_some()) {
        return Err(mixed_modes());
    }
    let db = &state.db;
    // Stored chains take the conversation's turn lock (no interleaved turns).
    let lock = match (&chain, store) {
        (Some(c), true) => Some(TurnLock::take(&c.id).ok_or_else(|| PlatformError::conflict("conversation_busy", "The previous turn in this conversation is still running."))?),
        _ => None,
    };
    let history = match &chain {
        Some(c) => items::history(db, &c.id, None).await?,
        None => Vec::new(),
    };
    // The id is minted first so the usage rows can point at it.
    let resp_id = new_id("resp_");
    let account_id = match (&chain, store) {
        (Some(c), true) => c.account_id.clone(),
        _ => caller.account_id.clone(),
    };
    let run = bridge::run_items(
        state,
        caller,
        RunItems {
            model,
            history: &history,
            input: &input,
            instructions: nonempty(&body.instructions),
            tools,
            tool_choice,
            max_tokens: body.max_output_tokens,
            temperature: body.temperature,
            stream: body.stream,
            usage: UsageRef { id: resp_id.clone(), prefix: "resp", account_id: account_id.clone() },
        },
    )
    .await?;

    // The model took the turn: now store it.
    let (sink, row): (Box<dyn ItemSink>, RespRow) = if store {
        let conv_id = match &chain {
            Some(c) => c.id.clone(),
            None => open_model_conversation(db, caller).await?,
        };
        let row = RespRow::new(resp_id, account_id, None, Some(model.to_string()), Some(conv_id.clone()), body.previous_response_id.clone(), metadata);
        let row = insert_row(db, &caller.project_id, &row).await?;
        items::append(db, &caller.project_id, Some(&conv_id), Some(&row.id), input).await?;
        (Box::new(DbSink { db: db.clone(), project_id: caller.project_id.clone(), conversation_id: Some(conv_id), response_id: row.id.clone() }) as Box<dyn ItemSink>, row)
    } else {
        let row = RespRow::new(resp_id.clone(), account_id, None, Some(model.to_string()), None, body.previous_response_id.clone(), metadata);
        (Box::new(MemSink { project_id: caller.project_id.clone(), response_id: resp_id, seq: 0 }) as Box<dyn ItemSink>, row)
    };
    let fin = Finish { db: db.clone(), store, row, error_code: "turn_failed" };
    let resp_id = fin.row.id.clone();
    let (emit, rx) = emitter(body.stream);
    // Detached like an agent turn: a client that hangs up can't leave the row running.
    let turn = tokio::spawn(drive_model(run, sink, fin, lock, emit));
    match rx {
        None => Ok(Created::Json(joined(db, &resp_id, turn).await?)),
        Some(rx) => Ok(Created::Stream(rx)),
    }
}

// ---- handlers -----------------------------------------------------------------

/// Plan and Copilot models are person-sent chat only (CONTRACT-connections
/// section 9.3): a Platform response never runs one.
pub(crate) fn refuse_person_only_model(model: Option<&str>) -> Result<(), PlatformError> {
    match model {
        Some(model) if crate::routes::model_router::is_person_only_model(model) => Err(PlatformError::permission(
            "plan_person_only",
            crate::routes::model_router::PLAN_PERSON_ONLY_MESSAGE,
        )),
        _ => Ok(()),
    }
}

async fn create_inner(state: &Arc<ApiState>, caller: &PlatformCaller, host: Arc<dyn AgentHost>, body: CreateBody) -> Result<Created, PlatformError> {
    let model = nonempty(&body.model).map(str::to_string);
    let agent = nonempty(&body.agent).map(str::to_string);
    let conversation = conversation_param(&body.conversation)?;
    let previous = nonempty(&body.previous_response_id).map(str::to_string);
    if conversation.is_some() && previous.is_some() {
        return Err(invalid("previous_response_id", "Give conversation or previous_response_id, not both."));
    }
    // Scope before any lookup, so a key without it learns nothing.
    match (&model, &agent) {
        (Some(_), None) => caller.require("inference")?,
        (None, Some(_)) => caller.require("agents")?,
        _ => return Err(invalid("model", "Give exactly one of model or agent.")),
    }
    refuse_person_only_model(model.as_deref())?;
    let metadata = check_metadata(&body.metadata)?;
    let input = items::parse_input(&body.input)?;
    if input.len() > MAX_INPUT_ITEMS {
        return Err(PlatformError::invalid_request("invalid_input", format!("At most {MAX_INPUT_ITEMS} input items.")).with_param("input"));
    }
    let chain = resolve_chain(&state.db, caller, conversation.as_deref(), previous.as_deref()).await?;
    match (model, agent) {
        (Some(m), None) => model_turn(state, caller, &m, body, input, chain, conversation.is_some(), metadata).await,
        (None, Some(a)) => agent_turn(state, caller, host, &a, body, input, chain, metadata).await,
        _ => unreachable!("checked above"),
    }
}

fn into_response(created: Created) -> Response {
    match created {
        Created::Json(v) => Json(v).into_response(),
        Created::Stream(rx) => {
            let events = futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|e| (Ok::<_, Infallible>(e), rx)) });
            Sse::new(events).into_response()
        }
    }
}

async fn create(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: HostExt,
    ApiJson(body): ApiJson<CreateBody>,
) -> Result<Response, PlatformError> {
    let host = host_for(&state, layered);
    Ok(into_response(create_inner(&state, &caller, host, body).await?))
}

/// `POST /v1/responses` without streaming, for the MCP edge's `respond` tool
/// (lane 5): same checks, same turn, same storage; `stream` is forced off.
/// `body` is the request JSON as a developer would send it.
pub async fn respond_for_mcp(state: &Arc<ApiState>, caller: &PlatformCaller, body: Value) -> Result<Value, PlatformError> {
    let mut body: CreateBody = serde_json::from_value(body).map_err(|e| PlatformError::invalid_request("invalid_json", e.to_string()))?;
    body.stream = false;
    match create_inner(state, caller, host_for(state, None), body).await? {
        Created::Json(v) => Ok(v),
        Created::Stream(_) => Err(PlatformError::api_error("internal_error", "An internal error occurred. Please retry.")),
    }
}

fn require_read(caller: &PlatformCaller) -> Result<(), PlatformError> {
    if caller.has_scope("inference") || caller.has_scope("agents") {
        Ok(())
    } else {
        caller.require("inference")
    }
}

async fn fetch(State(state): State<Arc<ApiState>>, caller: PlatformCaller, Path(id): Path<String>) -> Result<Json<Value>, PlatformError> {
    require_read(&caller)?;
    let row = load_row(&state.db, &caller, &id).await?;
    let pending = super::approvals::pending_for_response(&state.db, &caller.project_id, &row.id).await?;
    Ok(Json(response_json_with(&row, &pending)))
}

async fn input_items(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(id): Path<String>,
    ApiQuery(page): ApiQuery<PageParams>,
) -> Result<Json<Value>, PlatformError> {
    require_read(&caller)?;
    let row = load_row(&state.db, &caller, &id).await?;
    let limit = page.limit()?;
    let after = page.cursor()?.map(|(_, id)| id);
    let output_ids: Vec<String> = row.output.as_array().into_iter().flatten().filter_map(|i| i["id"].as_str().map(str::to_string)).collect();
    // Output items are the ones listed in `output`; while the turn runs (empty
    // `output`) they are the ones still open.
    let rows = sqlx::query_as::<_, Item>(&format!(
        "SELECT {ITEM_COLUMNS} FROM platform_items \
         WHERE response_id = $1 AND project_id = $2 AND NOT (id = ANY($3)) \
           AND ($6 = 'completed' OR $6 = 'failed' OR $6 = 'incomplete' OR status <> 'in_progress') \
           AND ($4::text IS NULL OR seq > (SELECT seq FROM platform_items WHERE id = $4 AND response_id = $1)) \
         ORDER BY seq LIMIT $5"
    ))
    .bind(&row.id)
    .bind(&caller.project_id)
    .bind(&output_ids)
    .bind(&after)
    .bind(limit + 1)
    .bind(&row.status)
    .fetch_all(&state.db)
    .await?;
    let page = build_page(rows, limit, |i| (i.created_at, i.id.clone()));
    Ok(Json(json!({
        "data": page.data.iter().map(items::item_json).collect::<Vec<_>>(),
        "has_more": page.has_more,
        "next_cursor": page.next_cursor,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> MemSink {
        MemSink { project_id: "proj_1".into(), response_id: "resp_1".into(), seq: 0 }
    }

    fn names(evs: &[Ev]) -> Vec<&str> {
        evs.iter().map(|(n, _)| n.as_str()).collect()
    }

    #[tokio::test]
    async fn agent_turn_events_in_order() {
        let (mut fold, mut sink, mut evs) = (OutputFold::default(), mem(), Vec::new());
        evs.extend(fold.text(&mut sink, "Looking ").await.unwrap());
        evs.extend(fold.text(&mut sink, "it up.").await.unwrap());
        evs.extend(fold.tool_started(&mut sink, &json!({ "type": "tool.started", "toolCallId": "c1", "name": "websearch", "input": { "q": "x" } })).await.unwrap());
        evs.extend(fold.tool_ended(&mut sink, &json!({ "type": "tool.completed", "toolCallId": "c1", "name": "websearch", "output": "3 results" }), false).await.unwrap());
        evs.extend(fold.tool_ended(&mut sink, &json!({ "type": "tool.failed", "toolCallId": "c2", "name": "bash", "error": "denied" }), true).await.unwrap());
        evs.extend(fold.text(&mut sink, "Found it.").await.unwrap());
        evs.extend(fold.finish(&mut sink, &End::Completed).await.unwrap());
        assert_eq!(names(&evs), [
            "response.output_item.added", "response.output_text.delta", "response.output_text.delta",
            "response.output_text.done", "response.output_item.done",
            "response.output_item.added", "response.tool_call.started", "response.tool_call.completed", "response.output_item.done",
            "response.output_item.added", "response.tool_call.failed", "response.output_item.done",
            "response.output_item.added", "response.output_text.delta", "response.output_text.done", "response.output_item.done",
        ]);
        let out = fold.output_json();
        let kinds: Vec<&str> = out.as_array().unwrap().iter().map(|i| i["type"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["message", "tool_call", "tool_call", "message"]);
        assert_eq!(out[0]["content"], json!([{ "type": "output_text", "text": "Looking it up." }]));
        assert_eq!((out[1]["status"].as_str(), out[1]["input"]["q"].as_str(), out[1]["output"].as_str()), (Some("completed"), Some("x"), Some("3 results")));
        assert_eq!((out[2]["status"].as_str(), out[2]["error"].as_str()), (Some("failed"), Some("denied")));
        assert_eq!(evs[1].1["item_id"], out[0]["id"], "deltas name the stored item");
        assert_eq!(evs[4].1["item"]["status"], "completed");
        assert_ne!(out[3]["id"], out[0]["id"], "text after a tool is a new message");

        // A failed turn closes the open message as failed.
        let (mut fold, mut sink) = (OutputFold::default(), mem());
        fold.text(&mut sink, "Half").await.unwrap();
        let evs = fold.finish(&mut sink, &End::Failed("boom".into())).await.unwrap();
        assert_eq!(names(&evs), ["response.output_text.done", "response.output_item.done"]);
        assert_eq!((fold.output[0].status.as_str(), fold.output[0].error.as_deref()), ("failed", Some("boom")));
    }

    #[tokio::test]
    async fn model_function_calls_complete_with_their_arguments() {
        let (mut fold, mut sink, mut evs) = (OutputFold::default(), mem(), Vec::new());
        evs.extend(fold.text(&mut sink, "Checking.").await.unwrap());
        evs.extend(fold.call_started(&mut sink, 0, "call_abc", "weather").await.unwrap());
        evs.extend(fold.call_args(0, "{\"city\":"));
        evs.extend(fold.call_args(0, "\"Paris\"}"));
        evs.extend(fold.finish(&mut sink, &End::Completed).await.unwrap());
        assert_eq!(names(&evs), [
            "response.output_item.added", "response.output_text.delta", "response.output_text.done", "response.output_item.done",
            "response.output_item.added", "response.function_call_arguments.delta", "response.function_call_arguments.delta",
            "response.function_call_arguments.done", "response.output_item.done",
        ]);
        let call = &fold.output_json()[1];
        assert_eq!((call["call_id"].as_str(), call["name"].as_str(), call["arguments"].as_str(), call["status"].as_str()), (Some("call_abc"), Some("weather"), Some("{\"city\":\"Paris\"}"), Some("completed")));
    }

    #[test]
    fn request_checks() {
        assert_eq!(conversation_param(&Some(json!({ "id": "conv_1" }))).unwrap().as_deref(), Some("conv_1"));
        assert_eq!(conversation_param(&Some(json!(3))).unwrap_err().param.as_deref(), Some("conversation"));
        assert_eq!(chat_tools(&[json!({ "type": "web_search" })]).unwrap_err().param.as_deref(), Some("tools[0].type"));
        assert_eq!(chat_tools(&[json!({ "type": "function", "name": "f", "parameters": { "type": "object" } })]).unwrap()[0]["function"]["name"], "f");
        assert_eq!(chat_tool_choice(&Some(json!({ "type": "function", "name": "f" }))).unwrap(), Some(json!({ "type": "function", "function": { "name": "f" } })));
        assert_eq!(agent_text(&[NewItem::function_call_output("c", "x")]).unwrap_err().param.as_deref(), Some("input[0]"));
        assert_eq!(agent_text(&items::parse_input(&json!("  Hi  ")).unwrap()).unwrap(), "Hi");
        let mut row = RespRow::new("resp_1".into(), None, Some("agt_1".into()), None, Some("conv_1".into()), None, json!({}));
        row.status = "completed".into();
        let v = response_json(&row);
        assert_eq!((v["status"].as_str(), v["required_actions"].as_array().map(Vec::len)), (Some("completed"), Some(0)));
        // Pending approvals turn a finished response into `requires_action`, never a running one.
        let pending = vec!["apr_1".to_string(), "apr_2".to_string()];
        let v = response_json_with(&row, &pending);
        assert_eq!(v["status"], "requires_action");
        assert_eq!(v["required_actions"], json!([{ "type": "approval", "approval_id": "apr_1" }, { "type": "approval", "approval_id": "apr_2" }]));
        let mut running = RespRow::new("resp_2".into(), None, Some("agt_1".into()), None, Some("conv_1".into()), None, json!({}));
        running.status = "in_progress".into();
        assert_eq!((response_json_with(&running, &pending)["status"].as_str(), response_json_with(&running, &pending)["required_actions"].as_array().map(Vec::len)), (Some("in_progress"), Some(0)));
        assert!(v["incomplete_details"].is_null());
        row.status = "incomplete".into();
        assert_eq!(response_json(&row)["incomplete_details"], json!({ "reason": "max_output_tokens" }));
        row.status = "failed".into();
        assert_eq!(response_json(&row)["status"], "failed");
    }
}

/// Model-mode billing end to end: the project pays (usage rows at list + 15%),
/// no member's credits are touched, and a call that fails upstream bills nothing.
/// Real Postgres, real routes, a fake upstream model.
#[cfg(test)]
mod billing_tests {
    use std::sync::Mutex;

    use axum::{body::Body, http::Request, Router};
    use tower::ServiceExt;

    use super::super::{projects, router_gated, Gate, ProjectEnv};
    use super::*;
    use crate::model_router::{catalog::starter_catalog, ChatCompletionRequest, ModelInfo, ModelRouter, ModelRouterError, UpstreamProvider};
    use crate::routes::test_support::{platform_schema, test_state, MockGateway};
    use crate::services::api_keys::{self, CreateProjectKeyInput};

    /// `gpt-4o` (upstream `openai/gpt-4o`) at a list price of $2 / $8 per 1M tokens.
    /// Every call reports 1,000 input and 500 output tokens.
    struct FakeUpstream {
        /// What the next stream sends: an `error` chunk after the usage.
        fail_stream: Mutex<bool>,
    }

    #[async_trait]
    impl UpstreamProvider for FakeUpstream {
        fn provider_id(&self) -> &str {
            "openrouter"
        }
        async fn list_models(&self) -> Result<Vec<ModelInfo>, ModelRouterError> {
            let mut extra = serde_json::Map::new();
            extra.insert("prompt_price".into(), json!(0.000002));
            extra.insert("completion_price".into(), json!(0.000008));
            Ok(vec![ModelInfo { id: "openai/gpt-4o".into(), object: "model".into(), created: 0, owned_by: "openai".into(), upstream_id: None, provider: Some("openrouter".into()), aliases: None, extra }])
        }
        async fn chat_completions(&self, request: ChatCompletionRequest) -> Result<axum::http::Response<Body>, ModelRouterError> {
            let usage = json!({ "prompt_tokens": 1000, "completion_tokens": 500 });
            let body = if request.stream == Some(true) {
                let mut s = format!("data: {}\n\n", json!({ "choices": [{ "delta": { "content": "hello" } }] }));
                if *self.fail_stream.lock().unwrap() {
                    s.push_str(&format!("data: {}\n\n", json!({ "usage": usage, "error": { "message": "upstream fell over" } })));
                } else {
                    s.push_str(&format!("data: {}\n\ndata: [DONE]\n\n", json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }], "usage": usage })));
                }
                s
            } else {
                json!({ "choices": [{ "message": { "role": "assistant", "content": "hello" }, "finish_reason": "stop" }], "usage": usage }).to_string()
            };
            axum::http::Response::builder().status(200).body(Body::from(body)).map_err(|e| ModelRouterError::Internal(e.to_string()))
        }
    }

    struct Ctx {
        db: PgPool,
        app: Router,
        upstream: Arc<FakeUpstream>,
        /// The model gateway as production mounts it: `auth_middleware` in front
        /// of the real `/v1/chat/completions` handler.
        gateway: Router,
    }

    async fn ctx() -> Ctx {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        let mut state = Arc::try_unwrap(state).unwrap_or_else(|_| panic!("fresh test state is not shared"));
        let upstream = Arc::new(FakeUpstream { fail_stream: Mutex::new(false) });
        state.model_router = ModelRouter::new(vec![upstream.clone() as Arc<dyn UpstreamProvider>], starter_catalog());
        let state = Arc::new(state);
        platform_schema(&state.db).await;
        // 007 references `users`; a bare one is enough for the personal-credit table to exist.
        sqlx::query("CREATE TABLE IF NOT EXISTS users (id TEXT PRIMARY KEY)").execute(&state.db).await.unwrap();
        for sql in [include_str!("../../../migrations_pg/007_inference_usage.sql"), include_str!("../../../migrations_pg/009_inference_pools.sql")] {
            sqlx::raw_sql(&sql.replace("public.", "")).execute(&state.db).await.expect("migration applies");
        }
        let app = router_gated(&state, Gate::Forced(true)).with_state(state.clone());
        let gateway = Router::new()
            .route("/v1/chat/completions", axum::routing::post(crate::routes::model_router::chat_completions))
            .layer(axum::middleware::from_fn_with_state(state.clone(), crate::auth::middleware::auth_middleware))
            .with_state(state.clone());
        Ctx { db: state.db.clone(), app, upstream, gateway }
    }

    async fn key(c: &Ctx) -> (String, String) {
        let owner = projects::Principal { user_id: "user_billing".into(), org_id: None, org_admin: false };
        let p = projects::create_project(&c.db, &owner, "Billing test", ProjectEnv::Live).await.unwrap();
        super::super::project_billing::put_test_card(&c.db, &p.id).await;
        let token = api_keys::create_project_key(
            &c.db,
            CreateProjectKeyInput {
                user_id: p.owner_user_id.clone(),
                organization_id: None,
                project_id: p.id.clone(),
                account_id: None,
                env: ProjectEnv::Live,
                name: "k".into(),
                scopes: vec!["inference".into()],
            },
        )
        .await
        .unwrap()
        .token;
        (p.id, token)
    }

    async fn post(c: &Ctx, token: &str, path: &str, body: Value) -> (u16, String) {
        let req = Request::builder().method("POST").uri(path).header("authorization", format!("Bearer {token}")).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let resp = c.app.clone().oneshot(req).await.unwrap();
        let status = resp.status().as_u16();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    /// (meter, quantity, amount_microusd, idempotency, model) for one ref.
    async fn rows(c: &Ctx, ref_id: &str) -> Vec<(String, f64, Option<i64>, Option<String>, Option<String>)> {
        sqlx::query_as("SELECT meter, quantity::float8, amount_microusd, idempotency, model FROM platform_usage_events WHERE ref_id = $1 ORDER BY meter")
            .bind(ref_id)
            .fetch_all(&c.db)
            .await
            .unwrap()
    }

    fn expected(prefix: &str, id: &str) -> Vec<(String, f64, Option<i64>, Option<String>, Option<String>)> {
        // 1,000 × $2/1M = 2,000 µ$ → +15% = 2,300; 500 × $8/1M = 4,000 µ$ → 4,600.
        vec![
            ("tokens_in".into(), 1000.0, Some(2300), Some(format!("{prefix}:{id}:tokens_in")), Some("gpt-4o".into())),
            ("tokens_out".into(), 500.0, Some(4600), Some(format!("{prefix}:{id}:tokens_out")), Some("gpt-4o".into())),
        ]
    }

    async fn total_rows(c: &Ctx) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM platform_usage_events").fetch_one(&c.db).await.unwrap()
    }

    #[tokio::test]
    async fn the_project_pays_for_model_responses_and_messages() {
        let c = ctx().await;
        let (project_id, token) = key(&c).await;

        // Responses, JSON.
        let (s, b) = post(&c, &token, "/v1/responses", json!({ "model": "gpt-4o", "input": "hi" })).await;
        assert_eq!(s, 200, "{b}");
        let id = serde_json::from_str::<Value>(&b).unwrap()["id"].as_str().unwrap().to_string();
        assert_eq!(rows(&c, &id).await, expected("resp", &id));
        let billed: String = sqlx::query_scalar("SELECT DISTINCT project_id FROM platform_usage_events WHERE ref_id = $1").bind(&id).fetch_one(&c.db).await.unwrap();
        assert_eq!(billed, project_id);

        // Responses, streamed: billed once, at the end.
        let (s, b) = post(&c, &token, "/v1/responses", json!({ "model": "gpt-4o", "input": "hi", "stream": true })).await;
        assert_eq!(s, 200, "{b}");
        let id = b.split("\"id\":\"").nth(1).unwrap().split('"').next().unwrap().to_string();
        assert!(id.starts_with("resp_"), "{b}");
        assert_eq!(rows(&c, &id).await, expected("resp", &id));

        // Messages, JSON.
        let (s, b) = post(&c, &token, "/v1/messages", json!({ "model": "gpt-4o", "max_tokens": 50, "messages": [{ "role": "user", "content": "hi" }] })).await;
        assert_eq!(s, 200, "{b}");
        let id = serde_json::from_str::<Value>(&b).unwrap()["id"].as_str().unwrap().to_string();
        assert!(id.starts_with("mdl_"));
        assert_eq!(rows(&c, &id).await, expected("mdl", &id));
        assert_eq!(total_rows(&c).await, 6);

        // A stream that fails upstream bills nothing.
        *c.upstream.fail_stream.lock().unwrap() = true;
        let (s, b) = post(&c, &token, "/v1/messages", json!({ "model": "gpt-4o", "max_tokens": 50, "stream": true, "messages": [{ "role": "user", "content": "hi" }] })).await;
        assert_eq!(s, 200, "{b}");
        assert!(b.contains("upstream fell over"), "{b}");
        let (s, b) = post(&c, &token, "/v1/responses", json!({ "model": "gpt-4o", "input": "hi", "stream": true })).await;
        assert_eq!(s, 200, "{b}");
        assert!(b.contains("response.failed"), "{b}");
        assert_eq!(total_rows(&c).await, 6, "a failed call records no usage");

        // No double billing: nothing settled against the key creator's credits.
        let personal: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM inference_usage").fetch_one(&c.db).await.unwrap();
        assert_eq!(personal, 0);
    }

    async fn get(c: &Ctx, token: &str, path: &str) -> (u16, Value) {
        let req = Request::builder().method("GET").uri(path).header("authorization", format!("Bearer {token}")).body(Body::empty()).unwrap();
        let resp = c.app.clone().oneshot(req).await.unwrap();
        let status = resp.status().as_u16();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn approval(c: &Ctx, project: &str, id: &str, response: &str, minutes_ago: i64, expires_in_secs: Option<i64>) {
        sqlx::query(
            "INSERT INTO platform_approvals (id, project_id, source, source_ref, response_id, kind, status, expires_at, created_at) \
             VALUES ($1, $2, 'twin', $1, $3, 'send_email', 'pending', CASE WHEN $4::bigint IS NULL THEN NULL ELSE NOW() + make_interval(secs => $4::float8) END, NOW() - make_interval(mins => $5::int))",
        )
        .bind(id)
        .bind(project)
        .bind(response)
        .bind(expires_in_secs)
        .bind(minutes_ago as i32)
        .execute(&c.db)
        .await
        .unwrap();
    }

    /// `GET /v1/responses/{id}` reads `requires_action` while an approval its
    /// turn raised is pending, and the stored status again once none is.
    #[tokio::test]
    async fn pending_approvals_make_a_stored_response_require_action() {
        let c = ctx().await;
        let (project, token) = key(&c).await;
        sqlx::query("INSERT INTO platform_responses (id, project_id, status, store, completed_at) VALUES ('resp_wait', $1, 'completed', true, NOW())")
            .bind(&project)
            .execute(&c.db)
            .await
            .unwrap();
        let (s, v) = get(&c, &token, "/v1/responses/resp_wait").await;
        assert_eq!((s, v["status"].as_str(), v["required_actions"].clone()), (200, Some("completed"), json!([])), "{v}");

        // Two pending (oldest first), one already past its deadline (expired, so left out).
        approval(&c, &project, "apr_new", "resp_wait", 1, None).await;
        approval(&c, &project, "apr_old", "resp_wait", 5, Some(600)).await;
        approval(&c, &project, "apr_late", "resp_wait", 9, Some(-1)).await;
        let (s, v) = get(&c, &token, "/v1/responses/resp_wait").await;
        assert_eq!((s, v["status"].as_str()), (200, Some("requires_action")), "{v}");
        assert_eq!(v["required_actions"], json!([{ "type": "approval", "approval_id": "apr_old" }, { "type": "approval", "approval_id": "apr_new" }]));

        // Resolved: back to the stored status.
        sqlx::query("UPDATE platform_approvals SET status = 'approved', resolved_at = NOW() WHERE id IN ('apr_old', 'apr_new')").execute(&c.db).await.unwrap();
        let (s, v) = get(&c, &token, "/v1/responses/resp_wait").await;
        assert_eq!((s, v["status"].as_str(), v["required_actions"].clone()), (200, Some("completed"), json!([])), "{v}");
    }

    /// A project's hosted runtime runs its `"allternit"` agents on Allternit
    /// Cloud with its own device token (owner `platform:<project>`). The project
    /// pays for those turns, so the consumer gates must not refuse it: they did
    /// (no verified email, no free allowance left) and every hosted agent turn
    /// failed. Nothing is charged to a user's credits; a project that can't pay
    /// is refused, and a consumer computer keeps every gate.
    #[tokio::test]
    async fn a_platform_runtime_runs_cloud_inference_for_its_project() {
        let c = ctx().await;
        sqlx::raw_sql(&include_str!("../../../migrations_pg/125_inference_attribution.sql").replace("public.", "")).execute(&c.db).await.expect("migration 125 applies");
        // The personal credit balance the gateway reads (migrations_pg/001, minimal).
        sqlx::query("CREATE TABLE IF NOT EXISTS user_credits (user_id TEXT PRIMARY KEY, balance_usd DOUBLE PRECISION)").execute(&c.db).await.unwrap();
        sqlx::raw_sql(&include_str!("../../../migrations_pg/011_user_trust.sql").replace("public.", "")).execute(&c.db).await.expect("migration 011 applies");
        let (project_id, _) = key(&c).await;
        let device = |id: &str, owner: String| {
            let db = c.db.clone();
            let token = format!("allternit_runtime_{id}_secret");
            let id = id.to_string();
            async move {
                sqlx::query("INSERT INTO users (id) VALUES ($1)").bind(&owner).execute(&db).await.unwrap();
                sqlx::query("INSERT INTO runtime_devices (id, user_id, name, status, credential_expires_at, credential_hash) VALUES ($1, $2, 'rt', 'online', '2999-01-01', $3)")
                    .bind(&id)
                    .bind(&owner)
                    .bind(crate::routes::runtime_pairing::sha256_hex(token.as_bytes()))
                    .execute(&db)
                    .await
                    .unwrap();
                // Its free allowance is long used up: the consumer path would refuse it.
                sqlx::query("INSERT INTO inference_usage (id, user_id, model, prompt_tokens, completion_tokens, cost_usd) VALUES ($1, $2, 'gpt-4o', 1, 1, 100.0)")
                    .bind(format!("old_{id}"))
                    .bind(&owner)
                    .execute(&db)
                    .await
                    .unwrap();
                token
            }
        };
        let runtime = device("rt_proj", format!("platform:{project_id}")).await;
        let consumer = device("rt_user", "user_free".to_string()).await;
        // The consumer's email is verified, so only its spent allowance refuses it.
        sqlx::query("INSERT INTO user_trust (user_id, email_verified) VALUES ('user_free', TRUE)").execute(&c.db).await.unwrap();
        let complete = |token: String| {
            let gateway = c.gateway.clone();
            async move {
                let body = json!({ "model": "gpt-4o", "messages": [{ "role": "user", "content": "Reply with exactly: pong" }] });
                let req = Request::builder().method("POST").uri("/v1/chat/completions").header("authorization", format!("Bearer {token}")).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
                let resp = gateway.oneshot(req).await.unwrap();
                let status = resp.status().as_u16();
                let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
                (status, String::from_utf8_lossy(&bytes).into_owned())
            }
        };

        let (s, b) = complete(runtime.clone()).await;
        assert_eq!(s, 200, "{b}");
        assert!(b.contains("hello"), "{b}");
        let (client, credits): (Option<String>, i64) = sqlx::query_as(
            "SELECT (SELECT client FROM inference_usage WHERE user_id = $1 AND id NOT LIKE 'old_%'), (SELECT COUNT(*) FROM user_credits WHERE user_id = $1)",
        )
        .bind(format!("platform:{project_id}"))
        .fetch_one(&c.db)
        .await
        .unwrap();
        assert_eq!((client.as_deref(), credits), (Some("runtime"), 0), "recorded as the runtime's call, no credits involved");

        assert_eq!(complete(consumer).await.0, 403, "a consumer computer still needs its allowance");

        sqlx::query("UPDATE platform_projects SET stripe_customer_id = NULL, stripe_subscription_id = NULL, billing_status = NULL WHERE id = $1").bind(&project_id).execute(&c.db).await.unwrap();
        let (s, b) = complete(runtime).await;
        assert_eq!(s, 402, "a project that can't pay gets no Cloud inference: {b}");
    }
}
