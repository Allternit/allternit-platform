//! `POST /v1/messages`: the Anthropic Messages-shaped model route (scope
//! `inference`, ids `mdl_`). Owned by **lane 2 (Responses + Messages)** of the
//! Platform API parity program. SMS moved to `/v1/texts` ([`super::texts`]).
//!
//! Billing: the project pays, as for `model` responses ([`bridge::run_items`]):
//! `tokens_in` / `tokens_out` usage rows at list price + 15%, idempotent per
//! message (`mdl:{id}:{meter}`), toward the spend cap.
//!
//! Runs on the same [`bridge::run_items`] as `model` responses: the Anthropic
//! `messages` become items (`text`/`image` blocks → message parts, `tool_use` →
//! `function_call` with the block id as `call_id`, `tool_result` →
//! `function_call_output`), and the model's answer comes back as `text` and
//! `tool_use` blocks.
//!
//! **Threads** (our addition, backed by durable conversations, no expiry):
//! * no `thread`: stateless, nothing stored (Anthropic behaviour);
//! * `{"type":"create"}`: opens an agent-less conversation; `system` is stored as
//!   its first (system) item and `tools` with the thread, so later turns reuse them;
//! * `{"type":"continue","previous_message_id":"mdl_…"}`: `messages` holds only the
//!   new turn; the thread's history, system and tools are loaded. `system` or
//!   `tools` here are `400 thread_fingerprint_mismatch`; a `previous_message_id`
//!   that isn't the thread's latest is `409 previous_message_not_latest`.
//!
//! A thread turn is a `platform_responses` row with an `mdl_` id (never listed
//! under `/v1/responses`); its items carry that id.
//!
//! **Errors keep the Platform envelope** (`{"error":{type,code,message,param}}`
//! from `error.rs`), not Anthropic's `{"type":"error","error":{…}}`: one error
//! shape across `/v1`, and the SDKs parse that one. Mid-stream failures are an
//! `error` event with the same envelope.

use std::{convert::Infallible, sync::Arc};

use axum::{
    extract::State,
    response::{sse::Event, IntoResponse, Response, Sse},
    routing::post,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use super::{
    conversations::{self, ConvRow, TurnLock},
    items::{self, NewItem},
    new_id,
    responses::{latest_in_conversation, open_model_conversation, refuse_file_images, valid_tool_name},
    ApiJson, PlatformCaller, PlatformError, RouteTable,
};
use crate::model_router::items_bridge::{self as bridge, Completion, Delta, RunItems, RunOutput, UsageRef};
use crate::ApiState;

const MAX_MESSAGES: usize = 500;
const MAX_TOOLS: usize = 128;

pub fn register(table: RouteTable) -> RouteTable {
    table.add("/v1/messages", &["POST"], post(create))
}

/// Unknown Anthropic options (`stop_sequences`, `top_k`, …) are accepted and ignored.
#[derive(Debug, Deserialize)]
struct MessagesBody {
    model: Option<String>,
    system: Option<Value>,
    #[serde(default)]
    messages: Vec<Value>,
    tools: Option<Vec<Value>>,
    tool_choice: Option<Value>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    #[serde(default)]
    stream: bool,
    metadata: Option<Value>,
    thread: Option<Value>,
}

fn invalid(param: &str, message: impl Into<String>) -> PlatformError {
    PlatformError::invalid_request("invalid_request", message).with_param(param)
}

// ---- blocks ↔ items -----------------------------------------------------------

fn message_item(role: &str, parts: Vec<Value>) -> NewItem {
    NewItem { kind: "message".into(), role: Some(role.into()), status: "completed".into(), content: Value::Array(parts), error: None }
}

fn image_part(block: &Value, param: &str) -> Result<Value, PlatformError> {
    let src = &block["source"];
    let url = match src["type"].as_str() {
        Some("base64") => match (src["media_type"].as_str(), src["data"].as_str()) {
            (Some(mt), Some(data)) if mt.starts_with("image/") && !data.is_empty() => format!("data:{mt};base64,{data}"),
            _ => return Err(invalid(&format!("{param}.source"), "A base64 image needs an image/* media_type and data.")),
        },
        Some("url") => match src["url"].as_str().filter(|u| !u.is_empty()) {
            Some(u) => u.to_string(),
            None => return Err(invalid(&format!("{param}.source.url"), "An url image needs a url.")),
        },
        _ => return Err(invalid(&format!("{param}.source.type"), "Image sources are base64 or url (no files on this surface).")),
    };
    Ok(json!({ "type": "input_image", "image_url": url }))
}

fn tool_result_text(block: &Value, param: &str) -> Result<String, PlatformError> {
    let text = match &block["content"] {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Array(parts) => {
            let mut out = String::new();
            for (k, p) in parts.iter().enumerate() {
                match p["type"].as_str() {
                    Some("text") => out.push_str(p["text"].as_str().unwrap_or("")),
                    _ => return Err(invalid(&format!("{param}.content[{k}].type"), "tool_result content takes text blocks only here.")),
                }
            }
            out
        }
        _ => return Err(invalid(&format!("{param}.content"), "tool_result content must be a string or text blocks.")),
    };
    Ok(if block["is_error"].as_bool() == Some(true) { format!("Error: {text}") } else { text })
}

/// Anthropic `messages` → items, in order. Consecutive text/image blocks of one
/// message stay one message item; `tool_use` / `tool_result` blocks become their
/// own items where they stand.
fn blocks_to_items(messages: &[Value]) -> Result<Vec<NewItem>, PlatformError> {
    let mut out = Vec::new();
    for (i, m) in messages.iter().enumerate() {
        let param = format!("messages[{i}]");
        let role = match m["role"].as_str() {
            Some(r @ ("user" | "assistant")) => r,
            _ => return Err(invalid(&format!("{param}.role"), "role must be user or assistant.")),
        };
        let text_type = if role == "assistant" { "output_text" } else { "input_text" };
        let blocks = match &m["content"] {
            Value::String(s) if !s.is_empty() => {
                out.push(message_item(role, vec![json!({ "type": text_type, "text": s })]));
                continue;
            }
            Value::Array(b) if !b.is_empty() => b,
            _ => return Err(invalid(&format!("{param}.content"), "content must be a non-empty string or array of blocks.")),
        };
        let mut parts: Vec<Value> = Vec::new();
        let flush = |parts: &mut Vec<Value>, out: &mut Vec<NewItem>| {
            if !parts.is_empty() {
                out.push(message_item(role, std::mem::take(parts)));
            }
        };
        for (j, b) in blocks.iter().enumerate() {
            let bp = format!("{param}.content[{j}]");
            match (b["type"].as_str(), role) {
                (Some("text"), _) => {
                    let text = b["text"].as_str().ok_or_else(|| invalid(&format!("{bp}.text"), "A text block needs text."))?;
                    parts.push(json!({ "type": text_type, "text": text }));
                }
                (Some("image"), "user") => parts.push(image_part(b, &bp)?),
                (Some("tool_use"), "assistant") => {
                    flush(&mut parts, &mut out);
                    let id = b["id"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| invalid(&format!("{bp}.id"), "A tool_use block needs an id."))?;
                    let name = b["name"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| invalid(&format!("{bp}.name"), "A tool_use block needs a name."))?;
                    let input = if b["input"].is_null() { json!({}) } else { b["input"].clone() };
                    out.push(NewItem::function_call(id, name, &input.to_string()));
                }
                (Some("tool_result"), "user") => {
                    flush(&mut parts, &mut out);
                    let id = b["tool_use_id"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| invalid(&format!("{bp}.tool_use_id"), "A tool_result block needs tool_use_id."))?;
                    out.push(NewItem::function_call_output(id, &tool_result_text(b, &bp)?));
                }
                (Some(t @ ("image" | "tool_use" | "tool_result")), _) => {
                    return Err(invalid(&format!("{bp}.type"), format!("{t} blocks don't belong in a {role} message.")));
                }
                (Some(t), _) => return Err(invalid(&format!("{bp}.type"), format!("Unsupported block type {t:?}; use text, image, tool_use or tool_result."))),
                (None, _) => return Err(invalid(&format!("{bp}.type"), "Each block needs a type.")),
            }
        }
        flush(&mut parts, &mut out);
    }
    Ok(out)
}

/// Output items → Anthropic content blocks (`text`, `tool_use` with the
/// `call_id` as its id and the arguments parsed back to an object).
fn items_to_blocks(items: &[NewItem]) -> Vec<Value> {
    let mut out = Vec::new();
    for it in items {
        match it.kind.as_str() {
            "message" => {
                for p in it.content.as_array().into_iter().flatten() {
                    if let Some(t) = p["text"].as_str().filter(|t| !t.is_empty()) {
                        out.push(json!({ "type": "text", "text": t }));
                    }
                }
            }
            "function_call" => {
                let input = it.content["arguments"].as_str().and_then(|a| serde_json::from_str::<Value>(a).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
                out.push(json!({ "type": "tool_use", "id": it.content["call_id"], "name": it.content["name"], "input": input }));
            }
            _ => {}
        }
    }
    out
}

/// A finished model turn as output items.
fn completion_items(c: &Completion) -> Vec<NewItem> {
    let mut out = Vec::new();
    if !c.text.is_empty() {
        out.push(NewItem::assistant_text(&c.text));
    }
    out.extend(c.tool_calls.iter().map(|t| NewItem::function_call(&t.id, &t.name, &t.arguments)));
    out
}

fn stop_reason(c: &Completion) -> &'static str {
    if !c.tool_calls.is_empty() {
        "tool_use"
    } else if c.finish_reason.as_deref() == Some("length") {
        "max_tokens"
    } else {
        "end_turn"
    }
}

fn system_text(v: &Option<Value>) -> Result<Option<String>, PlatformError> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone()).filter(|s| !s.trim().is_empty())),
        Some(Value::Array(blocks)) => {
            let mut out = Vec::new();
            for (i, b) in blocks.iter().enumerate() {
                match (b["type"].as_str(), b["text"].as_str()) {
                    (Some("text"), Some(t)) => out.push(t.to_string()),
                    _ => return Err(invalid(&format!("system[{i}]"), "system blocks must be text blocks.")),
                }
            }
            Ok(Some(out.join("\n\n")).filter(|s| !s.trim().is_empty()))
        }
        Some(_) => Err(invalid("system", "system must be a string or text blocks.")),
    }
}

/// Anthropic tools → chat tools. Server tools (typed, like `web_search_…`) aren't run here.
fn chat_tools(tools: &[Value]) -> Result<Vec<Value>, PlatformError> {
    if tools.len() > MAX_TOOLS {
        return Err(invalid("tools", format!("At most {MAX_TOOLS} tools.")));
    }
    tools
        .iter()
        .enumerate()
        .map(|(i, t)| {
            if !matches!(t["type"].as_str(), None | Some("custom")) {
                return Err(invalid(&format!("tools[{i}].type"), "Server tools aren't supported; define client tools with name and input_schema."));
            }
            let name = t["name"].as_str().filter(|n| valid_tool_name(n)).ok_or_else(|| invalid(&format!("tools[{i}].name"), "A tool name is 1-64 letters, digits, underscores or dashes."))?;
            let schema = t.get("input_schema").filter(|s| s.is_object()).ok_or_else(|| invalid(&format!("tools[{i}].input_schema"), "input_schema must be a JSON Schema object."))?;
            Ok(bridge::chat_tool(name, t["description"].as_str(), Some(schema)))
        })
        .collect()
}

fn chat_tool_choice(tc: &Option<Value>) -> Result<Option<Value>, PlatformError> {
    let Some(tc) = tc.as_ref().filter(|v| !v.is_null()) else { return Ok(None) };
    match tc["type"].as_str() {
        Some("auto") => Ok(Some(json!("auto"))),
        Some("any") => Ok(Some(json!("required"))),
        Some("none") => Ok(Some(json!("none"))),
        Some("tool") => match tc["name"].as_str() {
            Some(n) => Ok(Some(json!({ "type": "function", "function": { "name": n } }))),
            None => Err(invalid("tool_choice.name", "A tool tool_choice needs a name.")),
        },
        _ => Err(invalid("tool_choice.type", "tool_choice.type must be auto, any, tool or none.")),
    }
}

// ---- threads ------------------------------------------------------------------

enum Thread {
    None,
    Create,
    Continue(String),
}

fn parse_thread(v: &Option<Value>) -> Result<Thread, PlatformError> {
    let Some(v) = v.as_ref().filter(|v| !v.is_null()) else { return Ok(Thread::None) };
    match v["type"].as_str() {
        Some("create") => Ok(Thread::Create),
        Some("continue") => match v["previous_message_id"].as_str().filter(|s| !s.is_empty()) {
            Some(id) => Ok(Thread::Continue(id.to_string())),
            None => Err(invalid("thread.previous_message_id", "A continue thread needs previous_message_id (mdl_…).")),
        },
        _ => Err(invalid("thread.type", "thread.type must be create or continue.")),
    }
}

/// The thread a continue picks up: its conversation and stored tools.
async fn load_thread(db: &PgPool, caller: &PlatformCaller, previous: &str) -> Result<(ConvRow, Vec<Value>), PlatformError> {
    let account = caller.account_filter(None)?;
    let row: Option<(Option<String>, serde_json::Value)> = sqlx::query_as(
        "SELECT conversation_id, metadata FROM platform_responses \
         WHERE id = $1 AND project_id = $2 AND left(id, 4) = 'mdl_' AND ($3::text IS NULL OR account_id = $3)",
    )
    .bind(previous)
    .bind(&caller.project_id)
    .bind(&account)
    .fetch_optional(db)
    .await?;
    let missing = || PlatformError::not_found("message_not_found", "No such message thread.").with_param("thread.previous_message_id");
    let (Some(conv_id), meta) = row.ok_or_else(missing)? else { return Err(missing()) };
    if latest_in_conversation(db, &conv_id).await?.as_deref() != Some(previous) {
        return Err(PlatformError::conflict("previous_message_not_latest", "previous_message_id is not the latest message of its thread; continue from the latest one.")
            .with_param("thread.previous_message_id"));
    }
    let conv = conversations::fetch_conversation(db, caller, &conv_id).await.map_err(|_| missing())?;
    let tools = meta["thread"]["tools"].as_array().cloned().unwrap_or_default();
    Ok((conv, tools))
}

/// A thread turn being stored.
struct Stored {
    db: PgPool,
    project_id: String,
    conversation_id: String,
    id: String,
}

impl Stored {
    async fn finish(&self, output: Vec<NewItem>, status: &str, usage: (u64, u64), error: Option<&str>) {
        let stored = match items::append(&self.db, &self.project_id, Some(&self.conversation_id), Some(&self.id), output).await {
            Ok(items) => items,
            Err(e) => {
                tracing::error!(message = %self.id, "platform: model message output not stored: {}", e.message);
                Vec::new()
            }
        };
        let output: Vec<Value> = stored.iter().map(items::item_json).collect();
        let error = error.map(|f| json!({ "code": "model_failed", "message": f }));
        if let Err(e) = sqlx::query(
            "UPDATE platform_responses SET status = $2, output = $3, usage = $4, error = $5, completed_at = NOW() WHERE id = $1",
        )
        .bind(&self.id)
        .bind(status)
        .bind(Value::Array(output))
        .bind(json!({ "input_tokens": usage.0, "output_tokens": usage.1 }))
        .bind(error)
        .execute(&self.db)
        .await
        {
            tracing::error!(%e, message = %self.id, "platform: model message outcome not stored");
        }
    }
}

// ---- handler ------------------------------------------------------------------

fn message_json(id: &str, model: &str, content: Vec<Value>, stop: Option<&str>, usage: (u64, u64)) -> Value {
    json!({
        "id": id, "type": "message", "role": "assistant", "content": content, "model": model,
        "stop_reason": stop, "stop_sequence": null,
        "usage": { "input_tokens": usage.0, "output_tokens": usage.1 },
    })
}

fn sse(name: &str, mut data: Value) -> Event {
    data["type"] = json!(name);
    items::sse(name, &data)
}

/// Turn the router's deltas into Anthropic stream events.
#[derive(Default)]
struct BlockStream {
    next: usize,
    /// The text block being written.
    text: Option<usize>,
    /// Tool-use blocks by the model's call index.
    tools: Vec<(usize, usize)>,
}

impl BlockStream {
    fn feed(&mut self, d: &Delta) -> Vec<Event> {
        let mut out = Vec::new();
        match d {
            Delta::Text(t) => {
                let index = match self.text {
                    Some(i) => i,
                    None => {
                        let i = self.next;
                        self.next += 1;
                        self.text = Some(i);
                        out.push(sse("content_block_start", json!({ "index": i, "content_block": { "type": "text", "text": "" } })));
                        i
                    }
                };
                out.push(sse("content_block_delta", json!({ "index": index, "delta": { "type": "text_delta", "text": t } })));
            }
            Delta::ToolStart { index, id, name } => {
                if let Some(i) = self.text.take() {
                    out.push(sse("content_block_stop", json!({ "index": i })));
                }
                let i = self.next;
                self.next += 1;
                self.tools.push((*index, i));
                out.push(sse("content_block_start", json!({ "index": i, "content_block": { "type": "tool_use", "id": id, "name": name, "input": {} } })));
            }
            Delta::ToolArgs { index, delta } => {
                if let Some(&(_, i)) = self.tools.iter().find(|(c, _)| c == index) {
                    out.push(sse("content_block_delta", json!({ "index": i, "delta": { "type": "input_json_delta", "partial_json": delta } })));
                }
            }
            Delta::Done(_) | Delta::Failed(_) => {}
        }
        out
    }

    /// Stop every open block, in index order.
    fn close(&mut self) -> Vec<Event> {
        let mut open: Vec<usize> = self.text.take().into_iter().chain(std::mem::take(&mut self.tools).into_iter().map(|(_, i)| i)).collect();
        open.sort_unstable();
        open.into_iter().map(|i| sse("content_block_stop", json!({ "index": i }))).collect()
    }
}

async fn create(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    ApiJson(body): ApiJson<MessagesBody>,
) -> Result<Response, PlatformError> {
    caller.require("inference")?;
    let db = &state.db;
    let model = body.model.as_deref().map(str::trim).filter(|m| !m.is_empty()).ok_or_else(|| invalid("model", "model is required."))?.to_string();
    let max_tokens = body.max_tokens.filter(|n| *n >= 1).ok_or_else(|| invalid("max_tokens", "max_tokens is required (at least 1)."))?;
    if body.temperature.is_some_and(|t| !(0.0..=2.0).contains(&t)) {
        return Err(invalid("temperature", "temperature must be between 0 and 2."));
    }
    let metadata = body.metadata.clone().unwrap_or_else(|| json!({}));
    if !metadata.is_object() || metadata.to_string().len() > 8 * 1024 {
        return Err(PlatformError::invalid_request("invalid_metadata", "metadata must be a JSON object of at most 8 KB.").with_param("metadata"));
    }
    if body.messages.is_empty() || body.messages.len() > MAX_MESSAGES {
        return Err(invalid("messages", format!("messages must hold 1 to {MAX_MESSAGES} messages.")));
    }
    let thread = parse_thread(&body.thread)?;
    let system = system_text(&body.system)?;
    let mut input = blocks_to_items(&body.messages)?;
    refuse_file_images(&input)?;

    // The chain and the tools this turn runs with.
    let (chain, tools_raw, previous) = match &thread {
        Thread::Continue(prev) => {
            if body.system.is_some() || body.tools.is_some() {
                return Err(PlatformError::invalid_request("thread_fingerprint_mismatch", "system and tools are set when a thread is created; leave them out on continue.")
                    .with_param(if body.system.is_some() { "system" } else { "tools" }));
            }
            let (conv, tools) = load_thread(db, &caller, prev).await?;
            if conv.agent_id.is_some() {
                return Err(conversations::no_agent());
            }
            (Some(conv), tools, Some(prev.clone()))
        }
        _ => (None, body.tools.clone().unwrap_or_default(), None),
    };
    let tools = chat_tools(&tools_raw)?;
    let tool_choice = chat_tool_choice(&body.tool_choice)?;
    // A new thread keeps its system prompt as its first item; a stateless call
    // sends it for this turn only.
    let instructions = match thread {
        Thread::Create => {
            if let Some(s) = &system {
                input.insert(0, message_item("system", vec![json!({ "type": "input_text", "text": s })]));
            }
            None
        }
        Thread::None => system.as_deref(),
        Thread::Continue(_) => None,
    };
    let lock = match &chain {
        Some(c) => Some(TurnLock::take(&c.id).ok_or_else(|| PlatformError::conflict("conversation_busy", "The previous message in this thread is still being answered."))?),
        None => None,
    };
    let history = match &chain {
        Some(c) => items::history(db, &c.id, None).await?,
        None => Vec::new(),
    };
    // The id is minted first so the usage rows (`mdl:{id}:{meter}`) can point at it.
    let id = new_id("mdl_");
    let account_id = chain.as_ref().map(|c| c.account_id.clone()).unwrap_or_else(|| caller.account_id.clone());
    let run = bridge::run_items(
        &state,
        &caller,
        RunItems {
            model: &model,
            history: &history,
            input: &input,
            instructions,
            tools,
            tool_choice,
            max_tokens: Some(max_tokens),
            temperature: body.temperature,
            stream: body.stream,
            usage: UsageRef { id: id.clone(), prefix: "mdl", account_id: account_id.clone() },
        },
    )
    .await?;

    // The model took the turn: store it when this is a thread.
    let stored = match (&thread, &chain) {
        (Thread::None, _) => None,
        (_, Some(c)) => Some(c.id.clone()),
        (_, None) => Some(open_model_conversation(db, &caller).await?),
    };
    let stored = match stored {
        None => None,
        Some(conv_id) => {
            sqlx::query(
                "INSERT INTO platform_responses (id, project_id, account_id, model, conversation_id, previous_response_id, status, metadata, store) \
                 VALUES ($1, $2, $3, $4, $5, $6, 'in_progress', $7, true)",
            )
            .bind(&id)
            .bind(&caller.project_id)
            .bind(&account_id)
            .bind(&model)
            .bind(&conv_id)
            .bind(&previous)
            .bind(json!({ "request": metadata, "thread": { "tools": tools_raw } }))
            .execute(db)
            .await?;
            items::append(db, &caller.project_id, Some(&conv_id), Some(&id), input).await?;
            Some(Stored { db: db.clone(), project_id: caller.project_id.clone(), conversation_id: conv_id, id: id.clone() })
        }
    };

    match run {
        RunOutput::Complete(c) => {
            let output = completion_items(&c);
            let body = message_json(&id, &model, items_to_blocks(&output), Some(stop_reason(&c)), (c.input_tokens, c.output_tokens));
            let status = if c.finish_reason.as_deref() == Some("length") { "incomplete" } else { "completed" };
            if let Some(s) = &stored {
                s.finish(output, status, (c.input_tokens, c.output_tokens), None).await;
            }
            drop(lock);
            Ok(Json(body).into_response())
        }
        RunOutput::Stream(mut rx) => {
            let (tx, out_rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
            tokio::spawn(async move {
                let _lock = lock;
                // The developer hung up: keep going so the thread is still stored.
                let _ = tx.send(sse("message_start", json!({ "message": message_json(&id, &model, vec![], None, (0, 0)) })));
                let mut blocks = BlockStream::default();
                let mut result = None;
                while let Some(d) = rx.recv().await {
                    for e in blocks.feed(&d) {
                        let _ = tx.send(e);
                    }
                    match d {
                        Delta::Done(c) => {
                            result = Some(Ok(c));
                            break;
                        }
                        Delta::Failed(f) => {
                            result = Some(Err(f));
                            break;
                        }
                        _ => {}
                    }
                }
                for e in blocks.close() {
                    let _ = tx.send(e);
                }
                match result.unwrap_or_else(|| Err("the model's answer ended early".into())) {
                    Ok(c) => {
                        let _ = tx.send(sse("message_delta", json!({
                            "delta": { "stop_reason": stop_reason(&c), "stop_sequence": null },
                            "usage": { "input_tokens": c.input_tokens, "output_tokens": c.output_tokens },
                        })));
                        if let Some(s) = &stored {
                            let status = if c.finish_reason.as_deref() == Some("length") { "incomplete" } else { "completed" };
                            s.finish(completion_items(&c), status, (c.input_tokens, c.output_tokens), None).await;
                        }
                        let _ = tx.send(sse("message_stop", json!({})));
                    }
                    Err(f) => {
                        if let Some(s) = &stored {
                            s.finish(Vec::new(), "failed", (0, 0), Some(&f)).await;
                        }
                        // Platform error envelope, as everywhere on /v1 (see the module docs).
                        let _ = tx.send(Event::default().event("error").data(json!({ "error": { "type": "api_error", "code": "model_failed", "message": f } }).to_string()));
                    }
                }
            });
            let events = futures::stream::unfold(out_rx, |mut rx| async move { rx.recv().await.map(|e| (Ok::<_, Infallible>(e), rx)) });
            Ok(Sse::new(events).into_response())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_router::items_bridge::ToolCallOut;

    #[test]
    fn content_blocks_to_items_and_back() {
        let items = blocks_to_items(&[
            json!({ "role": "user", "content": "Weather in Paris?" }),
            json!({ "role": "assistant", "content": [
                { "type": "text", "text": "Checking." },
                { "type": "tool_use", "id": "toolu_1", "name": "weather", "input": { "city": "Paris" } }
            ] }),
            json!({ "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "toolu_1", "content": [{ "type": "text", "text": "18C" }] },
                { "type": "text", "text": "And " },
                { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "AAA" } }
            ] }),
        ])
        .unwrap();
        let kinds: Vec<(&str, Option<&str>)> = items.iter().map(|i| (i.kind.as_str(), i.role.as_deref())).collect();
        assert_eq!(kinds, [
            ("message", Some("user")), ("message", Some("assistant")), ("function_call", None),
            ("function_call_output", None), ("message", Some("user")),
        ]);
        assert_eq!(items[2], NewItem::function_call("toolu_1", "weather", r#"{"city":"Paris"}"#), "tool_use id is the call_id");
        assert_eq!(items[3], NewItem::function_call_output("toolu_1", "18C"));
        assert_eq!(items[4].content, json!([{ "type": "input_text", "text": "And " }, { "type": "input_image", "image_url": "data:image/png;base64,AAA" }]));

        // The assistant turn comes back as the blocks it was.
        assert_eq!(items_to_blocks(&items[1..3]), vec![
            json!({ "type": "text", "text": "Checking." }),
            json!({ "type": "tool_use", "id": "toolu_1", "name": "weather", "input": { "city": "Paris" } }),
        ]);

        let c = Completion { text: String::new(), tool_calls: vec![ToolCallOut { id: "call_x".into(), name: "f".into(), arguments: "not json".into() }], ..Default::default() };
        assert_eq!(items_to_blocks(&completion_items(&c)), vec![json!({ "type": "tool_use", "id": "call_x", "name": "f", "input": {} })]);
        assert_eq!(stop_reason(&c), "tool_use");
    }

    #[test]
    fn bad_blocks_name_the_param() {
        let param = |m: Value| blocks_to_items(&[m]).unwrap_err().param;
        assert_eq!(param(json!({ "role": "system", "content": "x" })), Some("messages[0].role".into()));
        assert_eq!(param(json!({ "role": "user", "content": [{ "type": "tool_use", "id": "a", "name": "b" }] })), Some("messages[0].content[0].type".into()));
        assert_eq!(param(json!({ "role": "user", "content": [{ "type": "thinking", "thinking": "…" }] })), Some("messages[0].content[0].type".into()));
        assert_eq!(param(json!({ "role": "user", "content": [{ "type": "image", "source": { "type": "file", "file_id": "f" } }] })), Some("messages[0].content[0].source.type".into()));
        assert_eq!(chat_tool_choice(&Some(json!({ "type": "any" }))).unwrap(), Some(json!("required")));
        assert!(matches!(parse_thread(&Some(json!({ "type": "continue" }))), Err(e) if e.param.as_deref() == Some("thread.previous_message_id")));
    }

    #[test]
    fn stream_blocks_in_order() {
        let mut s = BlockStream::default();
        let mut n = 0;
        for d in [
            Delta::Text("Hi".into()),
            Delta::ToolStart { index: 0, id: "call_1".into(), name: "f".into() },
            Delta::ToolArgs { index: 0, delta: "{}".into() },
        ] {
            n += s.feed(&d).len();
        }
        assert_eq!(n, 5, "start+delta, stop+start, delta");
        assert_eq!(s.close().len(), 1, "only the tool block is still open");
    }
}
