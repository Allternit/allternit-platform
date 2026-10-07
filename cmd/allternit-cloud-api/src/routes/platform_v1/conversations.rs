//! Conversations with hosted agents (spec §4 Conversations). Scope `agents`.
//!
//! * `POST /v1/agents/{id}/conversations` opens one (no runtime work yet).
//! * `GET  /v1/agents/{id}/conversations` lists the agent's conversations, oldest
//!   first, without messages (cursor pages).
//! * `GET  /v1/conversations/{id}` returns it with its messages, oldest first.
//! * `POST /v1/conversations/{id}/messages` `{content, stream?}` sends a message
//!   and answers with the agent's reply; with `stream: true` it answers as
//!   server-sent events: `message.delta` `{delta}` while the agent writes, then
//!   `message.completed` (the stored reply) or `error` (`{error:{type,code,message}}`).
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
//! usage rows (ref = the reply's message id) billed at the provider's list price
//! + 15%, or at 0 when the agent runs on the project's own model key. A turn
//! the runtime reports no usage for records nothing.

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
    build_page, new_id, ApiJson, ApiQuery, PageParams, PlatformCaller, PlatformError, RouteTable,
};
use crate::ApiState;

const MAX_CONTENT: usize = 16_000;
const MAX_MESSAGES_RETURNED: i64 = 200;

pub fn register(table: RouteTable) -> RouteTable {
    table
        .add("/v1/agents/:id/conversations", &["GET", "POST"], get(list_conversations).post(create_conversation))
        .add("/v1/conversations/:id", &["GET"], get(get_conversation))
        .add("/v1/conversations/:id/messages", &["POST"], post(send_message))
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

#[derive(Debug, Clone, Serialize, FromRow)]
struct ConvRow {
    id: String,
    account_id: String,
    agent_id: String,
    runtime_id: Option<String>,
    runtime_session_id: Option<String>,
    metadata: Value,
    created_at: DateTime<Utc>,
}

const CONV_COLUMNS: &str = "id, account_id, agent_id, runtime_id, runtime_session_id, metadata, created_at";

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

const MSG_COLUMNS: &str = "id, conversation_id, role, content, status, error, created_at";

fn with_object(mut m: Message) -> Message {
    m.object = "conversation.message";
    m
}

fn conversation_json(c: &ConvRow, messages: Option<Vec<Message>>) -> Value {
    let mut v = json!({
        "id": c.id, "object": "conversation", "agent_id": c.agent_id, "account_id": c.account_id,
        "metadata": c.metadata, "created_at": c.created_at,
    });
    if let Some(m) = messages {
        v["messages"] = json!(m);
    }
    v
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    metadata: Option<Value>,
}

async fn create_conversation(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(agent_id): Path<String>,
    body: Option<ApiJson<CreateBody>>,
) -> Result<(StatusCode, Json<Value>), PlatformError> {
    caller.require("agents")?;
    let agent = agents::fetch_visible(&state, &caller, &agent_id).await?;
    let metadata = body.and_then(|ApiJson(b)| b.metadata).unwrap_or_else(|| json!({}));
    if !metadata.is_object() || metadata.to_string().len() > 8 * 1024 {
        return Err(PlatformError::invalid_request("invalid_metadata", "metadata must be a JSON object of at most 8 KB.").with_param("metadata"));
    }
    let row = sqlx::query_as::<_, ConvRow>(&format!(
        "INSERT INTO platform_conversations (id, project_id, account_id, agent_id, metadata) VALUES ($1, $2, $3, $4, $5) RETURNING {CONV_COLUMNS}"
    ))
    .bind(new_id("conv_"))
    .bind(&caller.project_id)
    .bind(&agent.account_id)
    .bind(&agent.id)
    .bind(&metadata)
    .fetch_one(&state.db)
    .await?;
    Ok((StatusCode::CREATED, Json(conversation_json(&row, Some(vec![])))))
}

async fn list_conversations(
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
        "data": page.data.iter().map(|c| conversation_json(c, None)).collect::<Vec<_>>(),
        "has_more": page.has_more,
        "next_cursor": page.next_cursor,
    })))
}

/// A conversation the caller may see (its account, its project, a live agent).
async fn fetch_conversation(db: &PgPool, caller: &PlatformCaller, id: &str) -> Result<ConvRow, PlatformError> {
    let account = caller.account_filter(None)?;
    sqlx::query_as::<_, ConvRow>(&format!(
        "SELECT c.{} FROM platform_conversations c JOIN platform_agents a ON a.id = c.agent_id \
         WHERE c.id = $1 AND c.project_id = $2 AND a.deleted_at IS NULL AND ($3::text IS NULL OR c.account_id = $3)",
        CONV_COLUMNS.replace(", ", ", c.")
    ))
    .bind(id)
    .bind(&caller.project_id)
    .bind(&account)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| PlatformError::not_found("conversation_not_found", "No such conversation."))
}

async fn messages(db: &PgPool, conversation_id: &str) -> Result<Vec<Message>, PlatformError> {
    let rows = sqlx::query_as::<_, Message>(&format!(
        "SELECT {MSG_COLUMNS} FROM (SELECT {MSG_COLUMNS} FROM platform_conversation_messages WHERE conversation_id = $1 \
         ORDER BY created_at DESC, id DESC LIMIT $2) recent ORDER BY created_at, id"
    ))
    .bind(conversation_id)
    .bind(MAX_MESSAGES_RETURNED)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(with_object).collect())
}

async fn get_conversation(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(id): Path<String>,
) -> Result<Json<Value>, PlatformError> {
    caller.require("agents")?;
    let conv = fetch_conversation(&state.db, &caller, &id).await?;
    let msgs = messages(&state.db, &conv.id).await?;
    Ok(Json(conversation_json(&conv, Some(msgs))))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SendBody {
    content: String,
    #[serde(default)]
    stream: bool,
}

async fn insert_message(db: &PgPool, conversation_id: &str, role: &str, content: &str, status: &str, error: Option<&str>) -> Result<Message, PlatformError> {
    let m = sqlx::query_as::<_, Message>(&format!(
        "INSERT INTO platform_conversation_messages (id, conversation_id, role, content, status, error) VALUES ($1, $2, $3, $4, $5, $6) RETURNING {MSG_COLUMNS}"
    ))
    .bind(new_id("cmsg_"))
    .bind(conversation_id)
    .bind(role)
    .bind(content)
    .bind(status)
    .bind(error)
    .fetch_one(db)
    .await?;
    sqlx::query("UPDATE platform_conversations SET updated_at = NOW() WHERE id = $1").bind(conversation_id).execute(db).await?;
    Ok(with_object(m))
}

/// The agent's definition as the runtime takes it (`PUT /api/v1/platform/agents/{id}`).
fn runtime_spec(a: &agents::Agent) -> Value {
    json!({
        "name": a.name, "instructions": a.instructions, "greeting": a.greeting, "model": a.model,
        "voice": a.voice, "tools": a.tools, "autonomy": a.autonomy, "transferTargets": a.transfer_targets,
        "accountId": a.account_id,
    })
}

/// Make sure the runtime has the agent's current definition and the conversation has a session there.
/// Returns the runtime, the session and whether the agent runs on the project's own model key.
async fn prepare(state: &ApiState, host: &dyn AgentHost, caller: &PlatformCaller, conv: &ConvRow) -> Result<(HostRuntime, String, bool), PlatformError> {
    let agent = agents::fetch_visible(state, caller, &conv.agent_id).await?;
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
        let (status, body) = host.call(&rt, "PUT", &format!("/api/v1/platform/agents/{}", agent.id), &runtime_spec(&agent)).await?;
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
    let session = match (&conv.runtime_id, &conv.runtime_session_id) {
        (Some(r), Some(s)) if r == &rt.runtime_id => s.clone(),
        _ => {
            let (status, body) = host
                .call(&rt, "POST", &format!("/api/v1/platform/agents/{}/sessions", agent.id), &json!({ "conversationId": conv.id }))
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
    Ok((rt, session, own_key.is_some()))
}

/// Who a turn's usage belongs to.
#[derive(Clone)]
struct TurnMeter {
    project_id: String,
    account_id: String,
    key_id: String,
    own_key: bool,
}

/// Record a turn's token usage (`usage` from the runtime's final event) against
/// the reply `message_id`. Idempotent per message; failures are logged, never
/// surfaced to the developer (the reply already happened).
async fn meter_turn(db: &PgPool, m: &TurnMeter, message_id: &str, usage: &Value) {
    if !usage.is_object() {
        return;
    }
    let model = usage["model"].as_str();
    for (meter, tokens, cost) in [("tokens_in", "inputTokens", "inputCostMicrousd"), ("tokens_out", "outputTokens", "outputCostMicrousd")] {
        let quantity = usage[tokens].as_i64().unwrap_or(0).max(0);
        if quantity == 0 {
            continue;
        }
        let amount = match usage[cost].as_i64() {
            Some(list) => Some(billing::token_charge_microusd(list, m.own_key)),
            None if m.own_key => Some(0),
            None => {
                tracing::warn!(message_id, model, "platform: no list price for this turn's model; tokens recorded unpriced");
                None
            }
        };
        let event = super::UsageEvent {
            project_id: m.project_id.clone(),
            account_id: Some(m.account_id.clone()),
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
    let (rt, session) = prepare(state, host, caller, &conv).await?;
    Ok((rt, format!("/api/v1/platform/agents/{}/sessions/{}/turn", conv.agent_id, session)))
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
    let lock = TurnLock::take(&conv.id).ok_or_else(|| PlatformError::conflict("conversation_busy", "The agent is still answering the previous message in this conversation."))?;
    let host = host_for(&state, layered);
    let (rt, session, own_key) = prepare(&state, host.as_ref(), &caller, &conv).await?;
    let meter = TurnMeter { project_id: caller.project_id.clone(), account_id: conv.account_id.clone(), key_id: caller.key_id.clone(), own_key };
    let path = format!("/api/v1/platform/agents/{}/sessions/{}/turn", conv.agent_id, session);
    let mut upstream = host.stream(&rt, &path, &json!({ "text": content })).await?;
    // Stored once the runtime took the turn: a refused turn (still starting) leaves nothing behind.
    insert_message(&state.db, &conv.id, "user", &content, "completed", None).await?;
    let db = state.db.clone();

    if !body.stream {
        let (mut lines, mut text, mut failure) = (SseLines::default(), String::new(), None::<String>);
        let (mut done, mut usage) = (false, Value::Null);
        while let Some(chunk) = upstream.next().await {
            let chunk = chunk.map_err(|e| PlatformError::api_error("runtime_unavailable", format!("The agent's reply was cut off: {e}")))?;
            for ev in lines.push(&chunk) {
                match ev["type"].as_str() {
                    Some("text.delta") => text.push_str(ev["text"].as_str().unwrap_or("")),
                    Some("done") => {
                        text = ev["text"].as_str().unwrap_or(&text).to_string();
                        usage = ev["usage"].clone();
                        done = true;
                    }
                    Some("error") => {
                        failure = Some(ev["message"].as_str().unwrap_or("the agent's turn failed").to_string());
                        usage = ev["usage"].clone();
                    }
                    _ => {}
                }
            }
        }
        drop(lock);
        let failure = failure.or_else(|| (!done).then(|| "the agent's reply ended early".to_string()));
        let msg = match &failure {
            Some(f) => insert_message(&db, &conv.id, "assistant", &text, "failed", Some(f)).await?,
            None => insert_message(&db, &conv.id, "assistant", &text, "completed", None).await?,
        };
        meter_turn(&db, &meter, &msg.id, &usage).await;
        return Ok(Json(msg).into_response());
    }

    let conv_id = conv.id.clone();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
    tokio::spawn(async move {
        let _lock = lock;
        let (mut lines, mut text, mut failure, mut done, mut usage) = (SseLines::default(), String::new(), None::<String>, false, Value::Null);
        'read: while let Some(chunk) = upstream.next().await {
            let bytes = match chunk {
                Ok(b) => b,
                Err(e) => {
                    failure = Some(format!("the agent's reply was cut off: {e}"));
                    break 'read;
                }
            };
            for ev in lines.push(&bytes) {
                match ev["type"].as_str() {
                    Some("text.delta") => {
                        let d = ev["text"].as_str().unwrap_or("").to_string();
                        text.push_str(&d);
                        // The developer hung up: keep reading so the reply is still stored.
                        let _ = tx.send(Event::default().event("message.delta").data(json!({ "delta": d }).to_string()));
                    }
                    Some("done") => {
                        text = ev["text"].as_str().unwrap_or(&text).to_string();
                        usage = ev["usage"].clone();
                        done = true;
                    }
                    Some("error") => {
                        failure = Some(ev["message"].as_str().unwrap_or("the agent's turn failed").to_string());
                        usage = ev["usage"].clone();
                    }
                    _ => {}
                }
            }
        }
        let failure = failure.or_else(|| (!done).then(|| "the agent's reply ended early".to_string()));
        let stored = match &failure {
            Some(f) => insert_message(&db, &conv_id, "assistant", &text, "failed", Some(f)).await,
            None => insert_message(&db, &conv_id, "assistant", &text, "completed", None).await,
        };
        if let Ok(m) = &stored {
            meter_turn(&db, &meter, &m.id, &usage).await;
        }
        let last = match (stored, failure) {
            (Ok(_), Some(f)) => error_event("turn_failed", &f),
            (Ok(m), None) => Event::default().event("message.completed").data(serde_json::to_string(&m).unwrap_or_default()),
            (Err(e), _) => error_event("internal_error", &e.message),
        };
        let _ = tx.send(last);
    });
    let events = futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|e| (Ok::<_, Infallible>(e), rx)) });
    Ok(Sse::new(events).into_response())
}
