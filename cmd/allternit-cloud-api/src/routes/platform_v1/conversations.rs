//! Conversations (spec §4 Conversations, Conversations v2). Scope `agents`.
//!
//! A conversation's transcript is its items (`platform_items`, see [`super::items`]):
//! messages, the agent's hosted tool steps (`tool_call`), function calls and
//! approval requests, in `seq` order.
//!
//! * `POST /v1/conversations` `{agent_id?, account_id?, metadata?, items?}` opens one,
//!   with or without an agent; starting `items` (up to 20) only without an agent
//!   (`409 conversation_has_agent`). No model run.
//! * `GET  /v1/conversations` lists the project's conversations, newest first
//!   (filters `agent_id`, `account_id`; cursor pages).
//! * `GET  /v1/conversations/{id}` returns it with its latest 200 user/assistant
//!   messages inline (`messages`, the legacy `conversation.message` shape) and
//!   `item_count`. `PATCH` replaces its `metadata`; `DELETE` removes it and its items.
//! * `GET  /v1/conversations/{id}/items` pages its items (`order=asc|desc`);
//!   `POST` appends up to 20 without running a model (agent-less conversations
//!   only); `GET /v1/conversations/{id}/items/{item_id}` reads one.
//! * Legacy aliases, unchanged in shape: `POST /v1/agents/{id}/conversations` opens
//!   one for the agent; `GET /v1/agents/{id}/conversations` lists the agent's,
//!   oldest first, without messages.
//! * `POST /v1/conversations/{id}/messages` `{content, stream?}` sends a message
//!   and answers with the agent's reply (`conversation.message`, projected from
//!   the reply's item); with `stream: true` it answers as server-sent events:
//!   `message.delta` `{delta}` while the agent writes, `tool.started` /
//!   `tool.completed` / `tool.failed` `{tool_call_id, name, input?, output?, error?}`
//!   as it uses tools (clients that don't know them skip them), then
//!   `message.completed` (the stored reply) or `error` (`{error:{type,code,message}}`).
//!   The user's message, each tool step (`tool_call` item) and the reply are stored.
//! * `POST /v1/agents/{id}/ag-ui` runs a turn for an AG-UI frontend (see [`super::ag_ui`]).
//!
//! The first message brings the project's hosted runtime up (see [`super::hosting`]);
//! while it starts, the call answers `503 runtime_starting` and the developer
//! retries. Before each turn the runtime gets the agent's latest definition if it
//! changed, and a session for the conversation if it has none on that runtime.
//! One turn at a time per conversation (`409 conversation_busy`).
//!
//! **Metering.** Before a turn the project's spend cap is checked
//! (`402 spend_cap_reached`, see [`super::billing`]). The runtime's final event
//! carries the turn's token usage; it is recorded as `tokens_in` / `tokens_out`
//! usage rows (ref = the reply's item id) billed at the provider's list price
//! + 15%, or at 0 when the agent runs on the project's own model key. A turn
//! the runtime reports no usage for records nothing. An agent on `"allternit"`
//! runs on Allternit Cloud's recommended model (sent to the runtime as
//! `allternit/<model>`, see [`runtime_model`]); the runtime can't price it, so
//! its tokens are priced here at that model's list price + 15%.

use std::{collections::HashSet, convert::Infallible, sync::Arc, sync::Mutex};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{sse::Event, IntoResponse, Response, Sse},
    routing::{get, post},
    Extension, Json,
};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool};

use super::{
    agents, billing,
    hosting::{host_for, AgentHost, HostRuntime, SseLines},
    items::{self, Item, NewItem},
    build_page, new_id, ApiJson, ApiQuery, PageParams, PlatformCaller, PlatformError, RouteTable,
};
use crate::ApiState;

const MAX_CONTENT: usize = 16_000;
const MAX_MESSAGES_RETURNED: i64 = 200;
/// Items per `POST /v1/conversations` or `POST /v1/conversations/{id}/items`.
const MAX_APPEND: usize = 20;
const MAX_METADATA_BYTES: usize = 8 * 1024;

pub fn register(table: RouteTable) -> RouteTable {
    table
        .add("/v1/agents/:id/conversations", &["GET", "POST"], get(list_agent_conversations).post(create_agent_conversation))
        .add("/v1/conversations", &["GET", "POST"], get(list_conversations).post(create_conversation))
        .add("/v1/conversations/:id", &["GET", "PATCH", "DELETE"], get(get_conversation).patch(update_conversation).delete(delete_conversation))
        .add("/v1/conversations/:id/messages", &["POST"], post(send_message))
        .add("/v1/conversations/:id/items", &["GET", "POST"], get(list_items).post(append_items))
        .add("/v1/conversations/:id/items/:item_id", &["GET"], get(get_item))
}

type HostExt = Option<Extension<Arc<dyn AgentHost>>>;

/// Conversations with a turn running right now (one at a time per conversation).
fn busy() -> &'static Mutex<HashSet<String>> {
    static BUSY: std::sync::OnceLock<Mutex<HashSet<String>>> = std::sync::OnceLock::new();
    BUSY.get_or_init(Default::default)
}

pub(crate) struct TurnLock(String);
impl TurnLock {
    pub(crate) fn take(id: &str) -> Option<Self> {
        busy().lock().unwrap().insert(id.to_string()).then(|| TurnLock(id.to_string()))
    }
}
impl Drop for TurnLock {
    fn drop(&mut self) {
        busy().lock().unwrap().remove(&self.0);
    }
}

pub(crate) fn conversation_busy() -> PlatformError {
    PlatformError::conflict("conversation_busy", "The agent is still answering the previous message in this conversation.")
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub(crate) struct ConvRow {
    pub(crate) id: String,
    /// Null for a conversation without an account (migration 121).
    pub(crate) account_id: Option<String>,
    /// Null for a conversation made without an agent (used by `model` responses).
    pub(crate) agent_id: Option<String>,
    runtime_id: Option<String>,
    runtime_session_id: Option<String>,
    metadata: Value,
    created_at: DateTime<Utc>,
}

pub(crate) const CONV_COLUMNS: &str = "id, account_id, agent_id, runtime_id, runtime_session_id, metadata, created_at";

/// The legacy `conversation.message` wire shape (field order is the wire order).
/// Built from a message item by [`items::legacy_message`].
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Message {
    pub id: String,
    #[sqlx(skip)]
    pub object: &'static str,
    pub conversation_id: String,
    pub role: String,
    pub content: String,
    pub status: String,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// The public conversation object. `messages` on the routes that inline them,
/// `item_count` where it is counted.
fn conversation_json(c: &ConvRow, messages: Option<Vec<Message>>, item_count: Option<i64>) -> Value {
    let mut v = json!({
        "id": c.id, "object": "conversation", "agent_id": c.agent_id, "account_id": c.account_id,
        "metadata": c.metadata, "created_at": c.created_at,
    });
    if let Some(n) = item_count {
        v["item_count"] = json!(n);
    }
    if let Some(m) = messages {
        v["messages"] = json!(m);
    }
    v
}

fn check_metadata(metadata: &Value) -> Result<(), PlatformError> {
    if !metadata.is_object() || metadata.to_string().len() > MAX_METADATA_BYTES {
        return Err(PlatformError::invalid_request("invalid_metadata", "metadata must be a JSON object of at most 8 KB.").with_param("metadata"));
    }
    Ok(())
}

/// `items` (an array of 1 to 20 item inputs, see [`items::parse_input`]) as new
/// items; errors name the `items[…]` field.
fn parse_items(v: &Value, param: &str) -> Result<Vec<NewItem>, PlatformError> {
    let n = v.as_array().map(Vec::len).ok_or_else(|| PlatformError::invalid_request("invalid_items", "items must be an array.").with_param(param))?;
    if n == 0 || n > MAX_APPEND {
        return Err(PlatformError::invalid_request("invalid_items", format!("items must hold 1 to {MAX_APPEND} items.")).with_param(param));
    }
    items::parse_input(v).map_err(|mut e| {
        e.param = e.param.map(|p| p.replacen("input", param, 1));
        e
    })
}

/// The `409 conversation_has_agent` refusal: items appended by hand to an agent's
/// conversation (its runtime keeps its own session history).
fn has_agent() -> PlatformError {
    PlatformError::conflict("conversation_has_agent", "This conversation has an agent, whose runtime keeps the history; send it messages instead of appending items.")
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateAgentBody {
    metadata: Option<Value>,
}

async fn create_agent_conversation(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(agent_id): Path<String>,
    body: Option<ApiJson<CreateAgentBody>>,
) -> Result<(StatusCode, Json<Value>), PlatformError> {
    caller.require("agents")?;
    let agent = agents::fetch_visible(&state, &caller, &agent_id).await?;
    let metadata = body.and_then(|ApiJson(b)| b.metadata).unwrap_or_else(|| json!({}));
    check_metadata(&metadata)?;
    let row = insert_conversation(&state.db, &caller.project_id, Some(agent.account_id.as_str()), Some(agent.id.as_str()), &metadata).await?;
    Ok((StatusCode::CREATED, Json(conversation_json(&row, Some(vec![]), None))))
}

async fn insert_conversation(db: &PgPool, project_id: &str, account_id: Option<&str>, agent_id: Option<&str>, metadata: &Value) -> Result<ConvRow, PlatformError> {
    Ok(sqlx::query_as::<_, ConvRow>(&format!(
        "INSERT INTO platform_conversations (id, project_id, account_id, agent_id, metadata) VALUES ($1, $2, $3, $4, $5) RETURNING {CONV_COLUMNS}"
    ))
    .bind(new_id("conv_"))
    .bind(project_id)
    .bind(account_id)
    .bind(agent_id)
    .bind(metadata)
    .fetch_one(db)
    .await?)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    agent_id: Option<String>,
    account_id: Option<String>,
    metadata: Option<Value>,
    items: Option<Value>,
}

async fn create_conversation(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    body: Option<ApiJson<CreateBody>>,
) -> Result<(StatusCode, Json<Value>), PlatformError> {
    caller.require("agents")?;
    let body = body.map(|ApiJson(b)| b).unwrap_or(CreateBody { agent_id: None, account_id: None, metadata: None, items: None });
    let metadata = body.metadata.unwrap_or_else(|| json!({}));
    check_metadata(&metadata)?;
    let new_items = match &body.items {
        Some(v) if v.as_array().is_some_and(Vec::is_empty) => Vec::new(),
        Some(v) => parse_items(v, "items")?,
        None => Vec::new(),
    };
    let (account_id, agent_id) = match body.agent_id.as_deref() {
        Some(agent_id) => {
            if !new_items.is_empty() {
                return Err(has_agent());
            }
            // Same visibility as the agent: another account's agent is a 404.
            let agent = agents::fetch_visible(&state, &caller, agent_id).await?;
            if body.account_id.as_deref().is_some_and(|a| a != agent.account_id) {
                return Err(PlatformError::invalid_request("account_mismatch", "account_id must be the agent's account.").with_param("account_id"));
            }
            (Some(agent.account_id), Some(agent.id))
        }
        None => {
            let account = caller.account_filter(body.account_id.as_deref())?;
            if let Some(a) = &account {
                let found: Option<(String,)> = sqlx::query_as("SELECT id FROM platform_accounts WHERE id = $1 AND project_id = $2 AND deleted_at IS NULL")
                    .bind(a)
                    .bind(&caller.project_id)
                    .fetch_optional(&state.db)
                    .await?;
                if found.is_none() {
                    return Err(PlatformError::not_found("account_not_found", "No such account.").with_param("account_id"));
                }
            }
            (account, None)
        }
    };
    let row = insert_conversation(&state.db, &caller.project_id, account_id.as_deref(), agent_id.as_deref(), &metadata).await?;
    let count = new_items.len() as i64;
    if let Err(e) = items::append(&state.db, &caller.project_id, Some(&row.id), None, new_items).await {
        // Nothing half-made: the conversation goes with its items.
        let _ = sqlx::query("DELETE FROM platform_conversations WHERE id = $1").bind(&row.id).execute(&state.db).await;
        return Err(e);
    }
    Ok((StatusCode::CREATED, Json(conversation_json(&row, None, Some(count)))))
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(flatten)]
    page: PageParams,
    agent_id: Option<String>,
    account_id: Option<String>,
}

async fn list_conversations(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    ApiQuery(q): ApiQuery<ListQuery>,
) -> Result<Json<Value>, PlatformError> {
    caller.require("agents")?;
    let account = caller.account_filter(q.account_id.as_deref())?;
    let limit = q.page.limit()?;
    let (after_at, after_id) = match q.page.cursor()? {
        Some((at, id)) => (Some(at), Some(id)),
        None => (None, None),
    };
    // Newest first; a deleted agent's conversations are gone, as on GET.
    let rows = sqlx::query_as::<_, ConvRow>(&format!(
        "SELECT c.{} FROM platform_conversations c LEFT JOIN platform_agents a ON a.id = c.agent_id \
         WHERE c.project_id = $1 AND (c.agent_id IS NULL OR a.deleted_at IS NULL) \
           AND ($2::text IS NULL OR c.account_id = $2) AND ($3::text IS NULL OR c.agent_id = $3) \
           AND ($4::timestamptz IS NULL OR (c.created_at, c.id) < ($4, $5)) \
         ORDER BY c.created_at DESC, c.id DESC LIMIT $6",
        CONV_COLUMNS.replace(", ", ", c.")
    ))
    .bind(&caller.project_id)
    .bind(&account)
    .bind(&q.agent_id)
    .bind(after_at)
    .bind(after_id)
    .bind(limit + 1)
    .fetch_all(&state.db)
    .await?;
    let page = build_page(rows, limit, |c| (c.created_at, c.id.clone()));
    Ok(Json(json!({
        "data": page.data.iter().map(|c| conversation_json(c, None, None)).collect::<Vec<_>>(),
        "has_more": page.has_more,
        "next_cursor": page.next_cursor,
    })))
}

async fn list_agent_conversations(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(agent_id): Path<String>,
    ApiQuery(page): ApiQuery<PageParams>,
) -> Result<Json<Value>, PlatformError> {
    caller.require("agents")?;
    // Same visibility as the agent: another account's agent is a 404.
    let agent = agents::fetch_visible(&state, &caller, &agent_id).await?;
    let limit = page.limit()?;
    let (after_at, after_id) = match page.cursor()? {
        Some((at, id)) => (Some(at), Some(id)),
        None => (None, None),
    };
    let rows = sqlx::query_as::<_, ConvRow>(&format!(
        "SELECT {CONV_COLUMNS} FROM platform_conversations \
         WHERE agent_id = $1 AND project_id = $2 \
           AND ($3::timestamptz IS NULL OR (created_at, id) > ($3, $4)) \
         ORDER BY created_at, id LIMIT $5"
    ))
    .bind(&agent.id)
    .bind(&caller.project_id)
    .bind(after_at)
    .bind(after_id)
    .bind(limit + 1)
    .fetch_all(&state.db)
    .await?;
    let page = build_page(rows, limit, |c| (c.created_at, c.id.clone()));
    Ok(Json(json!({
        "data": page.data.iter().map(|c| conversation_json(c, None, None)).collect::<Vec<_>>(),
        "has_more": page.has_more,
        "next_cursor": page.next_cursor,
    })))
}

/// The `409 conversation_has_no_agent` refusal: an agent turn on a conversation
/// made without an agent.
pub(crate) fn no_agent() -> PlatformError {
    PlatformError::conflict("conversation_has_no_agent", "This conversation has no agent; run it with a model response instead.")
}

/// A conversation the caller may see (its account, its project, a live agent
/// when it has one).
pub(crate) async fn fetch_conversation(db: &PgPool, caller: &PlatformCaller, id: &str) -> Result<ConvRow, PlatformError> {
    let account = caller.account_filter(None)?;
    sqlx::query_as::<_, ConvRow>(&format!(
        "SELECT c.{} FROM platform_conversations c LEFT JOIN platform_agents a ON a.id = c.agent_id \
         WHERE c.id = $1 AND c.project_id = $2 AND (c.agent_id IS NULL OR a.deleted_at IS NULL) AND ($3::text IS NULL OR c.account_id = $3)",
        CONV_COLUMNS.replace(", ", ", c.")
    ))
    .bind(id)
    .bind(&caller.project_id)
    .bind(&account)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| PlatformError::not_found("conversation_not_found", "No such conversation."))
}

const ITEM_COLUMNS: &str = "id, seq, project_id, conversation_id, response_id, type, role, status, content, legacy_id, error, created_at";

/// The latest 200 user and assistant messages, oldest first, as `conversation.message`.
async fn messages(db: &PgPool, conversation_id: &str) -> Result<Vec<Message>, PlatformError> {
    let rows = sqlx::query_as::<_, Item>(&format!(
        "SELECT {ITEM_COLUMNS} FROM (SELECT {ITEM_COLUMNS} FROM platform_items \
           WHERE conversation_id = $1 AND type = 'message' AND role IN ('user', 'assistant') \
           ORDER BY seq DESC LIMIT $2) recent ORDER BY seq"
    ))
    .bind(conversation_id)
    .bind(MAX_MESSAGES_RETURNED)
    .fetch_all(db)
    .await?;
    Ok(rows.iter().filter_map(items::legacy_message).collect())
}

async fn item_count(db: &PgPool, conversation_id: &str) -> Result<i64, PlatformError> {
    Ok(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM platform_items WHERE conversation_id = $1").bind(conversation_id).fetch_one(db).await?)
}

async fn get_conversation(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(id): Path<String>,
) -> Result<Json<Value>, PlatformError> {
    caller.require("agents")?;
    let conv = fetch_conversation(&state.db, &caller, &id).await?;
    let msgs = messages(&state.db, &conv.id).await?;
    let count = item_count(&state.db, &conv.id).await?;
    Ok(Json(conversation_json(&conv, Some(msgs), Some(count))))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateBody {
    metadata: Value,
}

/// `PATCH /v1/conversations/{id}`: replaces `metadata` (the spec's contract, like accounts).
async fn update_conversation(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<UpdateBody>,
) -> Result<Json<Value>, PlatformError> {
    caller.require("agents")?;
    check_metadata(&body.metadata)?;
    let conv = fetch_conversation(&state.db, &caller, &id).await?;
    let row = sqlx::query_as::<_, ConvRow>(&format!(
        "UPDATE platform_conversations SET metadata = $3, updated_at = NOW() WHERE id = $1 AND project_id = $2 RETURNING {CONV_COLUMNS}"
    ))
    .bind(&conv.id)
    .bind(&caller.project_id)
    .bind(&body.metadata)
    .fetch_one(&state.db)
    .await?;
    let count = item_count(&state.db, &row.id).await?;
    Ok(Json(conversation_json(&row, None, Some(count))))
}

/// `DELETE /v1/conversations/{id}`: the conversation and its items (and its
/// pre-items legacy message rows). Not while a turn is running (`409 conversation_busy`).
async fn delete_conversation(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(id): Path<String>,
) -> Result<Json<Value>, PlatformError> {
    caller.require("agents")?;
    let conv = fetch_conversation(&state.db, &caller, &id).await?;
    let _lock = TurnLock::take(&conv.id).ok_or_else(conversation_busy)?;
    let mut tx = state.db.begin().await?;
    // The legacy table's foreign key has no cascade; items (121) cascade, responses and approvals unlink.
    sqlx::query("DELETE FROM platform_conversation_messages WHERE conversation_id = $1").bind(&conv.id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM platform_conversations WHERE id = $1 AND project_id = $2").bind(&conv.id).bind(&caller.project_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({ "id": conv.id, "object": "conversation.deleted", "deleted": true })))
}

#[derive(Debug, Deserialize)]
struct ItemsQuery {
    #[serde(flatten)]
    page: PageParams,
    order: Option<String>,
}

/// `order` → newest first? `asc` (the default) or `desc`.
fn descending(order: Option<&str>) -> Result<bool, PlatformError> {
    match order {
        None | Some("asc") => Ok(false),
        Some("desc") => Ok(true),
        Some(_) => Err(PlatformError::invalid_request("invalid_order", "order must be asc or desc.").with_param("order")),
    }
}

async fn list_items(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(id): Path<String>,
    ApiQuery(q): ApiQuery<ItemsQuery>,
) -> Result<Json<Value>, PlatformError> {
    caller.require("agents")?;
    let limit = q.page.limit()?;
    let desc = descending(q.order.as_deref())?;
    let conv = fetch_conversation(&state.db, &caller, &id).await?;
    let page = items::page(&state.db, &caller.project_id, &conv.id, q.page.after.as_deref(), limit, desc).await?;
    Ok(Json(json!({ "data": page.data, "has_more": page.has_more, "next_cursor": page.next_cursor })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AppendBody {
    items: Value,
}

async fn append_items(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<AppendBody>,
) -> Result<Json<Value>, PlatformError> {
    caller.require("agents")?;
    let new_items = parse_items(&body.items, "items")?;
    let conv = fetch_conversation(&state.db, &caller, &id).await?;
    if conv.agent_id.is_some() {
        return Err(has_agent());
    }
    // Not into the middle of a running turn.
    let _lock = TurnLock::take(&conv.id).ok_or_else(conversation_busy)?;
    let stored = items::append(&state.db, &caller.project_id, Some(&conv.id), None, new_items).await?;
    Ok(Json(json!({ "data": stored.iter().map(items::item_json).collect::<Vec<_>>(), "has_more": false, "next_cursor": null })))
}

async fn get_item(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path((id, item_id)): Path<(String, String)>,
) -> Result<Json<Value>, PlatformError> {
    caller.require("agents")?;
    let conv = fetch_conversation(&state.db, &caller, &id).await?;
    let item = items::get(&state.db, &caller.project_id, &item_id)
        .await?
        .filter(|it| it.conversation_id.as_deref() == Some(conv.id.as_str()))
        .ok_or_else(|| PlatformError::not_found("item_not_found", "No such item in this conversation."))?;
    Ok(Json(items::item_json(&item)))
}

// ---- turns --------------------------------------------------------------------

/// One hosted tool step of a turn, as the runtime reported it.
#[derive(Debug, Clone, PartialEq)]
struct ToolStep {
    /// The runtime's `toolCallId` (none when it sent none).
    id: Option<String>,
    name: String,
    input: Value,
    output: Value,
    /// `in_progress | completed | failed`.
    status: &'static str,
    error: Option<String>,
}

/// What a turn's runtime events add up to: the reply text, its tool steps, its
/// usage and whether (and why) it failed. Fed every event of the turn stream.
#[derive(Debug, Default)]
pub(crate) struct TurnLog {
    pub(crate) text: String,
    pub(crate) usage: Value,
    failure: Option<String>,
    done: bool,
    tools: Vec<ToolStep>,
}

impl TurnLog {
    /// One runtime event: `text.delta`, `tool.started|completed|failed`, `done`, `error`.
    pub(crate) fn feed(&mut self, ev: &Value) {
        match ev["type"].as_str() {
            Some("text.delta") => self.text.push_str(ev["text"].as_str().unwrap_or("")),
            Some(kind @ ("tool.started" | "tool.completed" | "tool.failed")) => self.tool(kind, ev),
            Some("done") => {
                if let Some(t) = ev["text"].as_str() {
                    self.text = t.to_string();
                }
                self.usage = ev["usage"].clone();
                self.done = true;
            }
            Some("error") => {
                self.failure = Some(ev["message"].as_str().unwrap_or("the agent's turn failed").to_string());
                self.usage = ev["usage"].clone();
            }
            _ => {}
        }
    }

    /// The stream broke before the turn ended.
    pub(crate) fn cut_off(&mut self, e: impl std::fmt::Display) {
        self.failure = Some(format!("the agent's reply was cut off: {e}"));
    }

    /// Why the turn failed, if it did (an error event, or no `done`).
    pub(crate) fn failure(&self) -> Option<String> {
        self.failure.clone().or_else(|| (!self.done).then(|| "the agent's reply ended early".to_string()))
    }

    fn tool(&mut self, kind: &str, ev: &Value) {
        let id = ev["toolCallId"].as_str().filter(|c| !c.is_empty()).map(str::to_string);
        let name = ev["name"].as_str().filter(|n| !n.is_empty());
        // The step this event continues: same id, or (no id) the latest open step of that name.
        let found = match &id {
            Some(id) => self.tools.iter().position(|t| t.id.as_deref() == Some(id.as_str())),
            None if kind != "tool.started" => self.tools.iter().rposition(|t| t.id.is_none() && t.status == "in_progress" && Some(t.name.as_str()) == name),
            None => None,
        };
        let i = match found {
            Some(i) => i,
            None => {
                self.tools.push(ToolStep { id, name: name.unwrap_or("tool").to_string(), input: Value::Null, output: Value::Null, status: "in_progress", error: None });
                self.tools.len() - 1
            }
        };
        let step = &mut self.tools[i];
        for (k, slot) in [("input", &mut step.input), ("output", &mut step.output)] {
            if !ev[k].is_null() {
                *slot = ev[k].clone();
            }
        }
        match kind {
            "tool.completed" => step.status = "completed",
            "tool.failed" => {
                step.status = "failed";
                step.error = Some(match &ev["error"] {
                    Value::String(s) => s.clone(),
                    Value::Null => "The tool failed.".to_string(),
                    other => other.to_string(),
                });
            }
            _ => {}
        }
    }

    /// The tool steps as `tool_call` items, in the order they started. A step
    /// the turn never finished is `incomplete`.
    fn tool_items(&self) -> Vec<NewItem> {
        self.tools
            .iter()
            .map(|t| NewItem::tool_call(&t.name, t.input.clone(), t.output.clone(), if t.status == "in_progress" { "incomplete" } else { t.status }, t.error.as_deref()))
            .collect()
    }
}

/// Store the user's message of a turn (once the runtime took the turn).
pub(crate) async fn store_user(db: &PgPool, project_id: &str, conversation_id: &str, text: &str) -> Result<(), PlatformError> {
    items::append(db, project_id, Some(conversation_id), None, vec![NewItem::user_text(text)]).await.map(|_| ())
}

/// Store the end of a turn in one transaction: its tool steps (`tool_call`
/// items), then the assistant's reply, `failed` with the reason when `failure`
/// is set. Returns the reply's item.
pub(crate) async fn store_turn(db: &PgPool, project_id: &str, conversation_id: &str, log: &TurnLog, failure: Option<&str>) -> Result<Item, PlatformError> {
    let mut batch = log.tool_items();
    let reply = NewItem::assistant_text(&log.text);
    batch.push(match failure {
        Some(f) => reply.failed(f),
        None => reply,
    });
    items::append(db, project_id, Some(conversation_id), None, batch)
        .await?
        .pop()
        .ok_or_else(|| PlatformError::api_error("internal_error", "The reply was not stored."))
}

/// A reply item as the legacy `conversation.message` (struct, so the wire field order holds).
fn reply_message(it: &Item) -> Result<Message, PlatformError> {
    items::legacy_message(it).ok_or_else(|| PlatformError::api_error("internal_error", "The reply is not a message."))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SendBody {
    content: String,
    #[serde(default)]
    stream: bool,
}

/// gizzi's provider id for Allternit Cloud inference (`allternit/<model>`).
pub(crate) const CLOUD_PROVIDER: &str = "allternit";

/// The model the runtime runs an agent on. `"allternit"` (the default) is
/// Allternit Cloud's recommended model, sent as a concrete `allternit/<model>`:
/// the runtime has no Allternit plan and no signed-in CLI, so a bare
/// `"allternit"` fell back to the computer's own default brain (an unsigned
/// coding CLI) and every turn failed with "Model not found". The runtime calls
/// Cloud with its device token; the project pays through [`meter_turn`].
/// `provider/model` (the project's own key) is sent as it is.
pub(crate) fn runtime_model(router: &crate::model_router::ModelRouter, model: &str) -> String {
    match router.default_alias() {
        Some(alias) if model == agents::DEFAULT_MODEL => format!("{CLOUD_PROVIDER}/{alias}"),
        _ => model.to_string(),
    }
}

/// The agent's definition as the runtime takes it (`PUT /api/v1/platform/agents/{id}`):
/// its settings plus the twin settings kept here (autonomy rules, the account's memory).
async fn runtime_spec(state: &ApiState, a: &agents::Agent) -> Result<Value, PlatformError> {
    let db = &state.db;
    let (rules,): (Value,) = sqlx::query_as("SELECT autonomy_rules FROM platform_agents WHERE id = $1").bind(&a.id).fetch_one(db).await?;
    let memory = super::twin::account_memory_for_runtime(db, &a.account_id).await?;
    Ok(json!({
        "name": a.name, "instructions": a.instructions, "greeting": a.greeting, "model": runtime_model(&state.model_router, &a.model),
        "voice": a.voice, "tools": a.tools, "autonomy": a.autonomy, "transferTargets": a.transfer_targets,
        "accountId": a.account_id, "autonomyRules": rules, "accountMemory": memory,
    }))
}

/// Make sure the project's runtime holds the agent's current definition (and the
/// project's model key it runs on). Used before a turn, a channel connection, and
/// right after the agent's twin settings change.
pub(crate) async fn ensure_agent(state: &ApiState, host: &dyn AgentHost, caller: &PlatformCaller, agent_id: &str) -> Result<HostRuntime, PlatformError> {
    Ok(ensure_agent_key(state, host, caller, agent_id).await?.0)
}

/// [`ensure_agent`], also saying whether the agent runs on the project's own model key.
/// Waking a hosted runtime is billable work: a project with no card on file gets
/// `402 payment_method_required` here (conversations, calls, channels, twin).
async fn ensure_agent_key(state: &ApiState, host: &dyn AgentHost, caller: &PlatformCaller, agent_id: &str) -> Result<(HostRuntime, bool), PlatformError> {
    super::billing::require_payment_method(&state.db, &caller.project_id).await?;
    let agent = agents::fetch_visible(state, caller, agent_id).await?;
    // An agent on a provider's model runs on the project's own key.
    let own_key = match agent.model.split_once('/') {
        Some((provider, _)) => {
            let cipher = state.credential_cipher.as_deref().ok_or_else(|| PlatformError::api_error("model_keys_unavailable", "Model keys aren't available on this deployment."))?;
            let key = super::model_keys::key_for(&state.db, cipher, &caller.project_id, provider).await?.ok_or_else(|| {
                PlatformError::invalid_request("model_key_missing", format!("This agent uses {}, so the project needs a {provider} key: PUT /v1/model_keys/{provider}.", agent.model))
            })?;
            Some((provider.to_string(), key))
        }
        None => None,
    };
    let rt = host.runtime(&caller.project_id).await?;
    let (synced_runtime, synced_at): (Option<String>, Option<DateTime<Utc>>) =
        sqlx::query_as("SELECT runtime_id, synced_at FROM platform_agents WHERE id = $1").bind(&agent.id).fetch_one(&state.db).await?;
    let fresh = synced_runtime.as_deref() == Some(rt.runtime_id.as_str()) && synced_at.is_some_and(|t| t >= agent.updated_at);
    if !fresh {
        if let Some((provider, key)) = &own_key {
            let (status, _) = host.call(&rt, "PUT", &format!("/api/v1/platform/model-keys/{provider}"), &json!({ "apiKey": key })).await?;
            if status >= 400 {
                return Err(PlatformError::api_error("model_key_sync_failed", "The hosted runtime refused the project's model key."));
            }
        }
        let (status, body) = host.call(&rt, "PUT", &format!("/api/v1/platform/agents/{}", agent.id), &runtime_spec(state, &agent).await?).await?;
        if status >= 400 {
            tracing::warn!(agent = %agent.id, status, %body, "platform: runtime refused the agent");
            return Err(PlatformError::api_error("agent_sync_failed", "The hosted runtime refused this agent's definition."));
        }
        sqlx::query("UPDATE platform_agents SET runtime_id = $2, runtime_bot_id = $1, synced_at = NOW() WHERE id = $1")
            .bind(&agent.id)
            .bind(&rt.runtime_id)
            .execute(&state.db)
            .await?;
    }
    Ok((rt, own_key.is_some()))
}

/// Make sure the runtime has the agent's current definition and the conversation has a session there.
/// Returns the runtime, the session and whether the agent runs on the project's own model key.
pub(crate) async fn prepare(state: &ApiState, host: &dyn AgentHost, caller: &PlatformCaller, conv: &ConvRow) -> Result<(HostRuntime, String, bool), PlatformError> {
    let agent_id = conv.agent_id.as_deref().ok_or_else(no_agent)?;
    let (rt, own_key) = ensure_agent_key(state, host, caller, agent_id).await?;
    let session = match (&conv.runtime_id, &conv.runtime_session_id) {
        (Some(r), Some(s)) if r == &rt.runtime_id => s.clone(),
        _ => {
            let (status, body) = host
                .call(&rt, "POST", &format!("/api/v1/platform/agents/{agent_id}/sessions"), &json!({ "conversationId": conv.id }))
                .await?;
            let Some(sid) = body["sessionId"].as_str().filter(|_| status < 400).map(str::to_string) else {
                tracing::warn!(conv = %conv.id, status, %body, "platform: runtime refused a session");
                return Err(PlatformError::api_error("session_failed", "The hosted runtime couldn't open this conversation."));
            };
            sqlx::query("UPDATE platform_conversations SET runtime_id = $2, runtime_session_id = $3 WHERE id = $1")
                .bind(&conv.id)
                .bind(&rt.runtime_id)
                .bind(&sid)
                .execute(&state.db)
                .await?;
            sid
        }
    };
    Ok((rt, session, own_key))
}

/// Who a turn's usage belongs to.
#[derive(Clone)]
pub(crate) struct TurnMeter {
    pub(crate) project_id: String,
    pub(crate) account_id: Option<String>,
    pub(crate) key_id: String,
    pub(crate) own_key: bool,
    pub(crate) agent_id: String,
    pub(crate) conversation_id: String,
    pub(crate) started_at: chrono::DateTime<chrono::Utc>,
    /// For an agent on Allternit Cloud: its `allternit/<model>` and that model's
    /// list price per 1M tokens (input, output). The runtime can't price Cloud
    /// models, so its usage arrives without a cost; this prices it.
    pub(crate) cloud_price: Option<(String, f64, f64)>,
}

impl TurnMeter {
    /// `409 conversation_has_no_agent` for a conversation without an agent.
    pub(crate) fn new(caller: &PlatformCaller, conv: &ConvRow, own_key: bool) -> Result<Self, PlatformError> {
        Ok(Self {
            project_id: caller.project_id.clone(),
            account_id: conv.account_id.clone(),
            key_id: caller.key_id.clone(),
            own_key,
            agent_id: conv.agent_id.clone().ok_or_else(no_agent)?,
            conversation_id: conv.id.clone(),
            started_at: chrono::Utc::now(),
            cloud_price: None,
        })
    }

    /// Look up Allternit Cloud's list price for the model an agent not on the
    /// project's own key runs on (see [`runtime_model`]).
    pub(crate) async fn with_cloud_price(mut self, router: &crate::model_router::ModelRouter) -> Self {
        if self.own_key {
            return self;
        }
        if let Some(alias) = router.default_alias() {
            if let Ok(prices) = router.retail_prices(alias).await {
                let (input, output) = crate::model_router::items_bridge::list_per_1m(&prices);
                self.cloud_price = Some((format!("{CLOUD_PROVIDER}/{alias}"), input, output));
            }
        }
        self
    }

    /// The list price per 1M tokens of `model` on Allternit Cloud, input or output side.
    fn cloud_per_1m(&self, model: Option<&str>, output: bool) -> Option<f64> {
        let (cloud_model, input_price, output_price) = self.cloud_price.as_ref()?;
        (model == Some(cloud_model.as_str())).then_some(if output { *output_price } else { *input_price })
    }
}

/// Record a turn's token usage (`usage` from the runtime's final event) against
/// the reply `message_id`. Idempotent per message; failures are logged, never
/// surfaced to the developer (the reply already happened). Also exports the
/// turn as an `invoke_agent` span when the project has trace export set up.
pub(crate) async fn meter_turn(db: &PgPool, m: &TurnMeter, message_id: &str, usage: &Value, failure: Option<&str>) {
    let model = usage["model"].as_str();
    let span = super::project_tracing::SpanOut::new(format!("invoke_agent {}", m.agent_id), m.started_at, chrono::Utc::now())
        .attr("gen_ai.operation.name", "invoke_agent")
        .attr("gen_ai.agent.id", m.agent_id.as_str())
        .attr("gen_ai.conversation.id", m.conversation_id.as_str())
        .attr("allternit.message.id", message_id)
        .attr_opt("allternit.account.id", m.account_id.as_deref())
        .attr_opt("gen_ai.request.model", model)
        .attr_opt("gen_ai.provider.name", model.and_then(|x| x.split_once('/')).map(|(p, _)| p))
        .attr_opt("gen_ai.usage.input_tokens", usage["inputTokens"].as_i64())
        .attr_opt("gen_ai.usage.output_tokens", usage["outputTokens"].as_i64())
        .error(failure)
        .trace(&m.conversation_id);
    super::project_tracing::emit(db, &m.project_id, span);
    if !usage.is_object() {
        return;
    }
    for (meter, tokens, cost) in [("tokens_in", "inputTokens", "inputCostMicrousd"), ("tokens_out", "outputTokens", "outputCostMicrousd")] {
        let quantity = usage[tokens].as_i64().unwrap_or(0).max(0);
        if quantity == 0 {
            continue;
        }
        let amount = match (usage[cost].as_i64(), m.cloud_per_1m(model, meter == "tokens_out")) {
            (Some(list), _) => Some(billing::token_charge_microusd(list, m.own_key)),
            (None, _) if m.own_key => Some(0),
            (None, Some(per_1m)) => Some(billing::token_charge_microusd(crate::model_router::items_bridge::list_microusd(per_1m, quantity as u64), false)),
            (None, None) => {
                tracing::warn!(message_id, model, "platform: no list price for this turn's model; tokens recorded unpriced");
                None
            }
        };
        let event = super::UsageEvent {
            project_id: m.project_id.clone(),
            account_id: m.account_id.clone(),
            key_id: Some(m.key_id.clone()),
            meter: meter.to_string(),
            quantity: quantity as f64,
            unit: Some("token".to_string()),
            ref_id: Some(message_id.to_string()),
            idempotency: Some(format!("turn:{message_id}:{meter}")),
        };
        if let Err(e) = super::record_usage_priced(db, event, amount, Some(model.unwrap_or("unknown"))).await {
            tracing::error!(message_id, "platform: turn usage not recorded: {e}");
        }
    }
}

/// Open a conversation row for an agent (no runtime work yet). Calls use one
/// per call so their turns run in a hosted-runtime session like any conversation.
pub(crate) async fn open(db: &PgPool, project_id: &str, account_id: &str, agent_id: &str, metadata: &Value) -> Result<String, PlatformError> {
    let id = new_id("conv_");
    sqlx::query("INSERT INTO platform_conversations (id, project_id, account_id, agent_id, metadata) VALUES ($1, $2, $3, $4, $5)")
        .bind(&id)
        .bind(project_id)
        .bind(account_id)
        .bind(agent_id)
        .bind(metadata)
        .execute(db)
        .await?;
    Ok(id)
}

/// The runtime and session a conversation's next turn runs in (agent synced,
/// session opened if needed), plus the runtime path of that turn.
pub(crate) async fn turn_target(state: &ApiState, host: &dyn AgentHost, caller: &PlatformCaller, conversation_id: &str) -> Result<(HostRuntime, String), PlatformError> {
    let conv = fetch_conversation(&state.db, caller, conversation_id).await?;
    let (rt, session, _own_key) = prepare(state, host, caller, &conv).await?;
    let agent_id = conv.agent_id.as_deref().ok_or_else(no_agent)?;
    Ok((rt, format!("/api/v1/platform/agents/{agent_id}/sessions/{session}/turn")))
}

/// Read a buffered turn to its end: the reply text, or why it failed.
pub(crate) async fn collect_turn(mut upstream: crate::routes::voice_calls_cloud::RelayStream) -> Result<String, String> {
    let (mut lines, mut text, mut failure, mut done) = (SseLines::default(), String::new(), None::<String>, false);
    while let Some(chunk) = upstream.next().await {
        let chunk = chunk.map_err(|e| format!("the agent's reply was cut off: {e}"))?;
        for ev in lines.push(&chunk) {
            match ev["type"].as_str() {
                Some("text.delta") => text.push_str(ev["text"].as_str().unwrap_or("")),
                Some("done") => {
                    text = ev["text"].as_str().unwrap_or(&text).to_string();
                    done = true;
                }
                Some("error") => failure = Some(ev["message"].as_str().unwrap_or("the agent's turn failed").to_string()),
                _ => {}
            }
        }
    }
    match failure.or_else(|| (!done).then(|| "the agent's reply ended early".to_string())) {
        Some(f) => Err(f),
        None => Ok(text),
    }
}

/// A runtime tool step as the conversation stream shows it.
fn tool_event(ev: &Value) -> Value {
    let mut v = json!({ "tool_call_id": ev["toolCallId"], "name": ev["name"] });
    for k in ["input", "output", "error"] {
        if !ev[k].is_null() {
            v[k] = ev[k].clone();
        }
    }
    v
}

fn error_event(code: &str, message: &str) -> Event {
    Event::default().event("error").data(json!({ "error": { "type": "api_error", "code": code, "message": message } }).to_string())
}

async fn send_message(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: HostExt,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<SendBody>,
) -> Result<Response, PlatformError> {
    caller.require("agents")?;
    let content = body.content.trim().to_string();
    if content.is_empty() || content.chars().count() > MAX_CONTENT {
        return Err(PlatformError::invalid_request("invalid_content", format!("content must be 1 to {MAX_CONTENT} characters.")).with_param("content"));
    }
    let conv = fetch_conversation(&state.db, &caller, &id).await?;
    super::spend_allowed(&state.db, &caller.project_id).await?;
    let lock = TurnLock::take(&conv.id).ok_or_else(conversation_busy)?;
    let host = host_for(&state, layered);
    let (rt, session, own_key) = prepare(&state, host.as_ref(), &caller, &conv).await?;
    let meter = TurnMeter::new(&caller, &conv, own_key)?.with_cloud_price(&state.model_router).await;
    let path = format!("/api/v1/platform/agents/{}/sessions/{}/turn", meter.agent_id, session);
    let mut upstream = host.stream(&rt, &path, &json!({ "text": content })).await?;
    // Stored once the runtime took the turn: a refused turn (still starting) leaves nothing behind.
    store_user(&state.db, &caller.project_id, &conv.id, &content).await?;
    let db = state.db.clone();
    let project_id = caller.project_id.clone();

    if !body.stream {
        let (mut lines, mut log) = (SseLines::default(), TurnLog::default());
        while let Some(chunk) = upstream.next().await {
            let chunk = chunk.map_err(|e| PlatformError::api_error("runtime_unavailable", format!("The agent's reply was cut off: {e}")))?;
            for ev in lines.push(&chunk) {
                log.feed(&ev);
            }
        }
        drop(lock);
        let failure = log.failure();
        let reply = store_turn(&db, &project_id, &conv.id, &log, failure.as_deref()).await?;
        meter_turn(&db, &meter, &reply.id, &log.usage, failure.as_deref()).await;
        return Ok(Json(reply_message(&reply)?).into_response());
    }

    let conv_id = conv.id.clone();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
    tokio::spawn(async move {
        let _lock = lock;
        let (mut lines, mut log) = (SseLines::default(), TurnLog::default());
        'read: while let Some(chunk) = upstream.next().await {
            let bytes = match chunk {
                Ok(b) => b,
                Err(e) => {
                    log.cut_off(e);
                    break 'read;
                }
            };
            for ev in lines.push(&bytes) {
                log.feed(&ev);
                // The developer hung up: keep reading so the reply is still stored.
                match ev["type"].as_str() {
                    Some("text.delta") => {
                        let d = ev["text"].as_str().unwrap_or("");
                        let _ = tx.send(Event::default().event("message.delta").data(json!({ "delta": d }).to_string()));
                    }
                    Some(kind @ ("tool.started" | "tool.completed" | "tool.failed")) => {
                        let _ = tx.send(Event::default().event(kind).data(tool_event(&ev).to_string()));
                    }
                    _ => {}
                }
            }
        }
        let failure = log.failure();
        let stored = store_turn(&db, &project_id, &conv_id, &log, failure.as_deref()).await;
        if let Ok(m) = &stored {
            meter_turn(&db, &meter, &m.id, &log.usage, failure.as_deref()).await;
        }
        let last = match (stored.and_then(|it| reply_message(&it)), failure) {
            (Ok(_), Some(f)) => error_event("turn_failed", &f),
            (Ok(m), None) => Event::default().event("message.completed").data(serde_json::to_string(&m).unwrap_or_default()),
            (Err(e), _) => error_event("internal_error", &e.message),
        };
        let _ = tx.send(last);
    });
    let events = futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|e| (Ok::<_, Infallible>(e), rx)) });
    Ok(Sse::new(events).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(n: NewItem, id: &str, at: DateTime<Utc>) -> Item {
        Item {
            id: id.into(),
            seq: 7,
            project_id: "proj_1".into(),
            conversation_id: Some("conv_1".into()),
            response_id: None,
            kind: n.kind,
            role: n.role,
            status: n.status,
            content: n.content,
            legacy_id: None,
            error: n.error,
            created_at: at,
        }
    }

    #[test]
    fn a_reply_item_projects_to_the_legacy_message_byte_for_byte() {
        let at = DateTime::parse_from_rfc3339("2026-11-12T10:20:30.123456Z").unwrap().with_timezone(&Utc);
        // What the pre-items route serialized for the same reply (struct field order is the wire order).
        let before = Message {
            id: "item_0123456789abcdef01234567".into(),
            object: "conversation.message",
            conversation_id: "conv_1".into(),
            role: "assistant".into(),
            content: "Half an answer".into(),
            status: "failed".into(),
            error: Some("model unavailable".into()),
            created_at: at,
        };
        let it = stored(NewItem::assistant_text("Half an answer").failed("model unavailable"), "item_0123456789abcdef01234567", at);
        let now = reply_message(&it).unwrap();
        assert_eq!(serde_json::to_string(&now).unwrap(), serde_json::to_string(&before).unwrap());
        assert_eq!(
            serde_json::to_string(&now).unwrap(),
            r#"{"id":"item_0123456789abcdef01234567","object":"conversation.message","conversation_id":"conv_1","role":"assistant","content":"Half an answer","status":"failed","error":"model unavailable","created_at":"2026-11-12T10:20:30.123456Z"}"#
        );
        // A backfilled message keeps its cmsg_ id; a tool step is not a message.
        let mut old = stored(NewItem::user_text("hi"), "item_x", at);
        old.legacy_id = Some("cmsg_abc".into());
        assert_eq!(items::legacy_message_json(&old).unwrap()["id"], "cmsg_abc");
        assert!(reply_message(&stored(NewItem::tool_call("bash", Value::Null, Value::Null, "completed", None), "item_y", at)).is_err());
    }

    #[test]
    fn a_turn_log_keeps_the_reply_and_every_tool_step() {
        let mut log = TurnLog::default();
        for ev in [
            json!({ "type": "text.delta", "text": "Looking " }),
            json!({ "type": "tool.started", "toolCallId": "c1", "name": "websearch" }),
            json!({ "type": "tool.completed", "toolCallId": "c1", "name": "websearch", "input": { "q": "x" }, "output": "3 results" }),
            json!({ "type": "tool.failed", "toolCallId": "c2", "name": "bash", "error": "denied" }),
            json!({ "type": "tool.started", "name": "fetch", "input": { "url": "u" } }),
            json!({ "type": "text.delta", "text": "Found it." }),
            json!({ "type": "done", "text": "Looking Found it.", "usage": { "inputTokens": 5 } }),
        ] {
            log.feed(&ev);
        }
        assert_eq!(log.failure(), None);
        assert_eq!((log.text.as_str(), log.usage["inputTokens"].as_i64()), ("Looking Found it.", Some(5)));
        let tools = log.tool_items();
        assert_eq!(tools, vec![
            NewItem::tool_call("websearch", json!({ "q": "x" }), json!("3 results"), "completed", None),
            NewItem::tool_call("bash", Value::Null, Value::Null, "failed", Some("denied")),
            NewItem::tool_call("fetch", json!({ "url": "u" }), Value::Null, "incomplete", None),
        ]);

        // An id-less step's end joins its start; a turn with no `done` ended early.
        let mut log = TurnLog::default();
        log.feed(&json!({ "type": "tool.started", "name": "fetch" }));
        log.feed(&json!({ "type": "tool.completed", "name": "fetch", "output": "ok" }));
        assert_eq!(log.tool_items(), vec![NewItem::tool_call("fetch", Value::Null, json!("ok"), "completed", None)]);
        assert_eq!(log.failure().as_deref(), Some("the agent's reply ended early"));
        log.feed(&json!({ "type": "error", "message": "boom" }));
        assert_eq!(log.failure().as_deref(), Some("boom"));
        let mut cut = TurnLog::default();
        cut.cut_off("reset");
        assert_eq!(cut.failure().as_deref(), Some("the agent's reply was cut off: reset"));
    }

    #[test]
    fn item_params_are_checked() {
        assert_eq!((descending(None).unwrap(), descending(Some("asc")).unwrap(), descending(Some("desc")).unwrap()), (false, false, true));
        let e = descending(Some("newest")).unwrap_err();
        assert_eq!((e.status.as_u16(), e.code.as_str(), e.param.as_deref()), (400, "invalid_order", Some("order")));

        let ok = parse_items(&json!([{ "role": "user", "content": "hi" }, { "type": "function_call_output", "call_id": "c", "output": "1" }]), "items").unwrap();
        assert_eq!(ok, vec![NewItem::user_text("hi"), NewItem::function_call_output("c", "1")]);
        let param = |v: Value| parse_items(&v, "items").unwrap_err().param;
        assert_eq!(param(json!([{ "role": "robot", "content": "x" }])), Some("items[0].role".into()));
        assert_eq!(param(json!([{ "role": "user", "content": "x" }, { "role": "user", "content": [{ "type": "audio" }] }])), Some("items[1].content[0].type".into()));
        assert_eq!(param(json!("hi")), Some("items".into()));
        assert_eq!(param(json!([])), Some("items".into()));
        assert_eq!(param(Value::Array(vec![json!({ "role": "user", "content": "x" }); MAX_APPEND + 1])), Some("items".into()));
    }
}
