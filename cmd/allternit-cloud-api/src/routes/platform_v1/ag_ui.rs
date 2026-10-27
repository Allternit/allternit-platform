//! An agent behind the AG-UI protocol (https://docs.ag-ui.com), so AG-UI
//! frontends such as CopilotKit's `HttpAgent` can drive it. Scope `agents`.
//!
//! `POST /v1/agents/{id}/ag-ui` takes AG-UI's `RunAgentInput`
//! `{threadId, runId, messages, tools, context, state, forwardedProps}` and runs
//! one conversation turn (see [`super::conversations`]): same key auth, spend
//! cap (`402`), one turn at a time (`409 conversation_busy`), hosted runtime
//! (`503 runtime_starting`) and token metering. Those refusals are plain HTTP
//! errors before any stream starts.
//!
//! **Threads.** `threadId` is either one of this agent's conversation ids, or
//! any other string, which is kept on a conversation it opens
//! (`platform_conversations.external_thread_id`) so later runs with the same
//! `threadId` continue it. No `threadId` opens a new conversation and its id is
//! the run's thread. The server keeps the history: the turn's input is the last
//! `user` message in `messages` (its text parts when content is an array).
//! Client-side `tools`, `context` and `state` are not used (yet).
//!
//! **Stream.** `text/event-stream`, one `data:` JSON event per AG-UI event:
//! `RUN_STARTED` → `TEXT_MESSAGE_START` / `_CONTENT` / `_END` → for each tool the
//! agent uses `TOOL_CALL_START` / `_ARGS` / `_END` / `TOOL_CALL_RESULT` → then
//! `RUN_FINISHED`, or `RUN_ERROR {message, code}` when the turn fails. The reply
//! is stored on the conversation like any other.

use std::{convert::Infallible, sync::Arc};

use axum::{
    extract::{Path, State},
    response::{sse::Event, IntoResponse, Response, Sse},
    routing::post,
    Extension,
};
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    agents,
    conversations::{self, TurnLock, TurnMeter},
    hosting::{host_for, AgentHost, SseLines},
    new_id, ApiJson, PlatformCaller, PlatformError, RouteTable,
};
use crate::ApiState;

const MAX_CONTENT: usize = 16_000;
const MAX_THREAD_ID: usize = 200;

pub fn register(table: RouteTable) -> RouteTable {
    table.add("/v1/agents/:id/ag-ui", &["POST"], post(run_agent))
}

type HostExt = Option<Extension<Arc<dyn AgentHost>>>;

/// AG-UI `RunAgentInput`. Unknown fields are accepted: the protocol grows.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunAgentInput {
    thread_id: Option<String>,
    run_id: Option<String>,
    #[serde(default)]
    messages: Vec<Value>,
}

/// The text of the last `user` message: a string, or the `text` parts of an array.
fn last_user_text(messages: &[Value]) -> Option<String> {
    let m = messages.iter().rev().find(|m| m["role"] == "user")?;
    let text = match &m["content"] {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter(|p| p["type"] == "text").filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    };
    Some(text.trim().to_string()).filter(|t| !t.is_empty())
}

/// The conversation a `threadId` names for this agent, opening one when it names none.
async fn thread_conversation(state: &ApiState, caller: &PlatformCaller, agent: &agents::Agent, thread_id: Option<&str>) -> Result<String, PlatformError> {
    let account = caller.account_filter(None)?;
    if let Some(t) = thread_id {
        let found: Option<(String,)> = sqlx::query_as(
            "SELECT id FROM platform_conversations WHERE agent_id = $1 AND project_id = $2 \
             AND ($3::text IS NULL OR account_id = $3) AND (id = $4 OR external_thread_id = $4) \
             ORDER BY (id = $4) DESC LIMIT 1",
        )
        .bind(&agent.id)
        .bind(&caller.project_id)
        .bind(&account)
        .bind(t)
        .fetch_optional(&state.db)
        .await?;
        if let Some((id,)) = found {
            return Ok(id);
        }
    }
    // A concurrent first run of the same thread lands on the same row (unique per agent).
    let id = new_id("conv_");
    sqlx::query(
        "INSERT INTO platform_conversations (id, project_id, account_id, agent_id, metadata, external_thread_id) \
         VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
    )
    .bind(&id)
    .bind(&caller.project_id)
    .bind(&agent.account_id)
    .bind(&agent.id)
    .bind(json!({ "source": "ag-ui" }))
    .bind(thread_id)
    .execute(&state.db)
    .await?;
    let (id,): (String,) = sqlx::query_as("SELECT id FROM platform_conversations WHERE id = $1 OR (agent_id = $2 AND project_id = $3 AND external_thread_id = $4) ORDER BY (id = $1) DESC LIMIT 1")
        .bind(&id)
        .bind(&agent.id)
        .bind(&caller.project_id)
        .bind(thread_id)
        .fetch_one(&state.db)
        .await?;
    Ok(id)
}

/// Turns the runtime's turn events into AG-UI events. Text is one assistant
/// message, closed before a tool call (a later answer starts a new one); each
/// tool call reports its arguments, end and result.
pub(crate) struct AgUiFold {
    thread_id: String,
    run_id: String,
    /// The open text message, if any.
    open: Option<String>,
    /// The latest assistant message id (the parent of tool calls).
    last: Option<String>,
    /// Tool calls started and not yet ended: id → arguments sent.
    calls: std::collections::HashMap<String, bool>,
}

impl AgUiFold {
    pub(crate) fn new(thread_id: &str, run_id: &str) -> Self {
        Self { thread_id: thread_id.into(), run_id: run_id.into(), open: None, last: None, calls: Default::default() }
    }

    pub(crate) fn started(&self) -> Value {
        json!({ "type": "RUN_STARTED", "threadId": self.thread_id, "runId": self.run_id })
    }

    fn close_text(&mut self, out: &mut Vec<Value>) {
        if let Some(id) = self.open.take() {
            out.push(json!({ "type": "TEXT_MESSAGE_END", "messageId": id }));
        }
    }

    fn args(&mut self, call: &str, ev: &Value, out: &mut Vec<Value>) {
        let sent = self.calls.get_mut(call);
        if let (Some(sent @ false), Some(input)) = (sent, Some(&ev["input"]).filter(|i| !i.is_null())) {
            *sent = true;
            let delta = match input {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            out.push(json!({ "type": "TOOL_CALL_ARGS", "toolCallId": call, "delta": delta }));
        }
    }

    /// One runtime event (`text.delta`, `tool.*`) → AG-UI events.
    pub(crate) fn feed(&mut self, ev: &Value, out: &mut Vec<Value>) {
        match ev["type"].as_str().unwrap_or("") {
            "text.delta" => {
                let delta = ev["text"].as_str().unwrap_or("");
                if delta.is_empty() {
                    return;
                }
                let id = match &self.open {
                    Some(id) => id.clone(),
                    None => {
                        let id = new_id("msg_");
                        out.push(json!({ "type": "TEXT_MESSAGE_START", "messageId": id, "role": "assistant" }));
                        self.open = Some(id.clone());
                        self.last = Some(id.clone());
                        id
                    }
                };
                out.push(json!({ "type": "TEXT_MESSAGE_CONTENT", "messageId": id, "delta": delta }));
            }
            kind @ ("tool.started" | "tool.completed" | "tool.failed") => {
                let call = ev["toolCallId"].as_str().filter(|c| !c.is_empty()).map(str::to_string).unwrap_or_else(|| new_id("call_"));
                if kind == "tool.started" || !self.calls.contains_key(&call) {
                    if self.calls.contains_key(&call) {
                        return;
                    }
                    self.close_text(out);
                    let parent = self.last.get_or_insert_with(|| new_id("msg_")).clone();
                    out.push(json!({ "type": "TOOL_CALL_START", "toolCallId": call, "toolCallName": ev["name"].as_str().unwrap_or("tool"), "parentMessageId": parent }));
                    self.calls.insert(call.clone(), false);
                }
                self.args(&call, ev, out);
                if kind != "tool.started" {
                    self.calls.remove(&call);
                    out.push(json!({ "type": "TOOL_CALL_END", "toolCallId": call }));
                    let content = if kind == "tool.failed" { ev["error"].as_str().unwrap_or("The tool failed.") } else { ev["output"].as_str().unwrap_or("") };
                    out.push(json!({ "type": "TOOL_CALL_RESULT", "messageId": new_id("msg_"), "toolCallId": call, "content": content, "role": "tool" }));
                }
            }
            _ => {}
        }
    }

    /// The end of the run: the open message closes, then `RUN_FINISHED` or `RUN_ERROR`.
    pub(crate) fn finish(&mut self, failure: Option<(&str, &str)>, out: &mut Vec<Value>) {
        self.close_text(out);
        for (call, _) in std::mem::take(&mut self.calls) {
            out.push(json!({ "type": "TOOL_CALL_END", "toolCallId": call }));
        }
        out.push(match failure {
            Some((code, message)) => json!({ "type": "RUN_ERROR", "message": message, "code": code }),
            None => json!({ "type": "RUN_FINISHED", "threadId": self.thread_id, "runId": self.run_id }),
        });
    }
}

async fn run_agent(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: HostExt,
    Path(agent_id): Path<String>,
    ApiJson(input): ApiJson<RunAgentInput>,
) -> Result<Response, PlatformError> {
    caller.require("agents")?;
    let agent = agents::fetch_visible(&state, &caller, &agent_id).await?;
    let content = last_user_text(&input.messages)
        .ok_or_else(|| PlatformError::invalid_request("invalid_messages", "messages must end with a user message that has text.").with_param("messages"))?;
    if content.chars().count() > MAX_CONTENT {
        return Err(PlatformError::invalid_request("invalid_content", format!("The user message must be at most {MAX_CONTENT} characters.")).with_param("messages"));
    }
    let thread_id = input.thread_id.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    if thread_id.as_ref().is_some_and(|t| t.chars().count() > MAX_THREAD_ID) {
        return Err(PlatformError::invalid_request("invalid_thread_id", format!("threadId must be at most {MAX_THREAD_ID} characters.")).with_param("threadId"));
    }
    let run_id = input.run_id.filter(|r| !r.trim().is_empty()).unwrap_or_else(|| new_id("run_"));

    super::spend_allowed(&state.db, &caller.project_id).await?;
    let conv_id = thread_conversation(&state, &caller, &agent, thread_id.as_deref()).await?;
    let conv = conversations::fetch_conversation(&state.db, &caller, &conv_id).await?;
    let lock = TurnLock::take(&conv.id).ok_or_else(|| PlatformError::conflict("conversation_busy", "The agent is still answering the previous message in this conversation."))?;
    let host = host_for(&state, layered);
    let (rt, session, own_key) = conversations::prepare(&state, host.as_ref(), &caller, &conv).await?;
    let meter = TurnMeter { project_id: caller.project_id.clone(), account_id: conv.account_id.clone(), key_id: caller.key_id.clone(), own_key };
    let path = format!("/api/v1/platform/agents/{}/sessions/{}/turn", conv.agent_id, session);
    let mut upstream = host.stream(&rt, &path, &json!({ "text": content })).await?;
    conversations::insert_message(&state.db, &conv.id, "user", &content, "completed", None).await?;

    let db = state.db.clone();
    let mut fold = AgUiFold::new(thread_id.as_deref().unwrap_or(&conv.id), &run_id);
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let _ = tx.send(fold.started());
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
                let mut out = Vec::new();
                match ev["type"].as_str() {
                    Some("text.delta") => {
                        text.push_str(ev["text"].as_str().unwrap_or(""));
                        fold.feed(&ev, &mut out);
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
                    _ => fold.feed(&ev, &mut out),
                }
                // The client hung up: keep reading so the reply is still stored.
                for e in out {
                    let _ = tx.send(e);
                }
            }
        }
        let failure = failure.or_else(|| (!done).then(|| "the agent's reply ended early".to_string()));
        let stored = match &failure {
            Some(f) => conversations::insert_message(&db, &conv.id, "assistant", &text, "failed", Some(f)).await,
            None => conversations::insert_message(&db, &conv.id, "assistant", &text, "completed", None).await,
        };
        if let Ok(m) = &stored {
            conversations::meter_turn(&db, &meter, &m.id, &usage).await;
        }
        let mut out = Vec::new();
        match (stored, &failure) {
            (Ok(_), Some(f)) => fold.finish(Some(("turn_failed", f)), &mut out),
            (Ok(_), None) => fold.finish(None, &mut out),
            (Err(e), _) => fold.finish(Some(("internal_error", &e.message)), &mut out),
        }
        for e in out {
            let _ = tx.send(e);
        }
    });
    let events = futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|v| (Ok::<_, Infallible>(Event::default().data(v.to_string())), rx)) });
    Ok(Sse::new(events).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_events_become_ag_ui_events() {
        let msgs = vec![json!({ "id": "1", "role": "user", "content": "old" }), json!({ "id": "2", "role": "assistant", "content": "hi" }), json!({ "id": "3", "role": "user", "content": [{ "type": "text", "text": " Find x " }, { "type": "binary", "mimeType": "image/png" }] })];
        assert_eq!(last_user_text(&msgs).as_deref(), Some("Find x"), "the last user message's text parts");
        assert_eq!(last_user_text(&[json!({ "role": "assistant", "content": "x" })]), None);

        let mut fold = AgUiFold::new("t1", "r1");
        let mut out = vec![fold.started()];
        for ev in [
            json!({ "type": "text.delta", "text": "Looking " }),
            json!({ "type": "tool.started", "toolCallId": "c1", "name": "websearch" }),
            json!({ "type": "tool.completed", "toolCallId": "c1", "name": "websearch", "input": { "q": "x" }, "output": "3 results" }),
            json!({ "type": "tool.failed", "toolCallId": "c2", "name": "bash", "error": "denied" }),
            json!({ "type": "text.delta", "text": "Found it." }),
        ] {
            fold.feed(&ev, &mut out);
        }
        fold.finish(None, &mut out);
        let types: Vec<&str> = out.iter().map(|e| e["type"].as_str().unwrap()).collect();
        assert_eq!(types, [
            "RUN_STARTED", "TEXT_MESSAGE_START", "TEXT_MESSAGE_CONTENT", "TEXT_MESSAGE_END",
            "TOOL_CALL_START", "TOOL_CALL_ARGS", "TOOL_CALL_END", "TOOL_CALL_RESULT",
            "TOOL_CALL_START", "TOOL_CALL_END", "TOOL_CALL_RESULT",
            "TEXT_MESSAGE_START", "TEXT_MESSAGE_CONTENT", "TEXT_MESSAGE_END", "RUN_FINISHED",
        ]);
        assert_eq!(out[0], json!({ "type": "RUN_STARTED", "threadId": "t1", "runId": "r1" }));
        assert_eq!(out[4]["parentMessageId"], out[1]["messageId"], "tool calls hang off the assistant message");
        assert_eq!((out[5]["delta"].as_str(), out[7]["content"].as_str()), (Some(r#"{"q":"x"}"#), Some("3 results")));
        assert_eq!(out[10]["content"], "denied");
        assert_ne!(out[11]["messageId"], out[1]["messageId"], "text after a tool is a new message");
        assert_eq!(out[14], json!({ "type": "RUN_FINISHED", "threadId": "t1", "runId": "r1" }));

        let mut failed = AgUiFold::new("t1", "r1");
        let mut out = vec![];
        failed.feed(&json!({ "type": "text.delta", "text": "Half" }), &mut out);
        failed.finish(Some(("turn_failed", "boom")), &mut out);
        assert_eq!(out.last().unwrap(), &json!({ "type": "RUN_ERROR", "message": "boom", "code": "turn_failed" }));
    }
}
