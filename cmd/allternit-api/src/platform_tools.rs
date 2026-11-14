//! Hosted-agent tools whose data lives in the cloud (Platform API spec §5):
//! `agent_knowledge_search` and `agent_channel_post`, and the cloud side of a hosted
//! agent's `phone_text` / `phone_call` ([`phone_tool`], listed by `phone_outbound`).
//!
//! They are listed on the internal connectors MCP (`connector_routes`) only on a
//! project's hosted runtime (owner `platform:<project_id>`), and a session can
//! reach them only when its permission ruleset allows them
//! ([`crate::platform_agents::tool_rules`]: deny everything, then the agent's tools).
//!
//! **Which agent is calling.** One runtime serves every account of a project, so
//! the agent must never come from model input. gizzi sends the calling session's
//! id in the MCP call's `_meta` ([`SESSION_META`]); the session's metadata names
//! its bot (the agent), and that bot must belong to this runtime's owner and list
//! the tool. No session, someone else's session or an unlisted tool → refused.
//!
//! The work itself runs on cloud-api
//! (`POST /api/v1/runtime-devices/me/platform-agents/{agent}/tools/{tool}`, this
//! runtime's device credential), which checks the agent and tool again and keeps
//! every lookup inside the agent's own account.

use async_trait::async_trait;
use axum::{http::StatusCode, Json};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

use crate::db::DbHandle;

/// The `_meta` key gizzi puts the calling session's id under.
pub const SESSION_META: &str = "allternit/session";

/// MCP tool name → the agent tool it implements.
pub fn agent_tool_for(name: &str) -> Option<&'static str> {
    match name {
        "agent_knowledge_search" => Some("knowledge_search"),
        "agent_channel_post" => Some("channel_post"),
        "phone_text" => Some("send_text"),
        "phone_call" => Some("call"),
        _ => None,
    }
}

/// The tools this module lists and dispatches itself; `phone_text` / `phone_call`
/// are listed and dispatched by [`crate::phone_outbound`], which hands a project
/// runtime's calls to [`phone_tool`].
pub fn is_tool(name: &str) -> bool {
    matches!(name, "agent_knowledge_search" | "agent_channel_post")
}

fn platform_owner(owner: &str) -> bool {
    owner.starts_with("platform:") && owner.len() > "platform:".len()
}

/// Descriptors, only on a project's hosted runtime.
pub fn mcp_tools(owner: &str) -> Vec<Value> {
    if !platform_owner(owner) {
        return vec![];
    }
    vec![
        json!({
            "name": "agent_knowledge_search",
            "title": "Search knowledge files",
            "description": "Search the knowledge files your developer gave you (prices, hours, policies, FAQs). Returns the best matching passages with their file names. Answer from them; if nothing matches, say you don't know.",
            "inputSchema": { "type": "object", "properties": {
                "query": { "type": "string", "minLength": 1, "maxLength": 500, "description": "What to look for, in plain words" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 10 }
            }, "required": ["query"], "additionalProperties": false }
        }),
        json!({
            "name": "agent_channel_post",
            "title": "Post to a connected channel",
            "description": "Post a message to a channel (Slack or Discord) that your business connected. Only that business's own channels. If it refuses, tell the person why.",
            "inputSchema": { "type": "object", "properties": {
                "text": { "type": "string", "minLength": 1, "maxLength": 4000 },
                "channel": { "type": "string", "description": "slack or discord; needed only when several are connected" },
                "target": { "type": "string", "description": "A channel id, when not the connection's default channel" }
            }, "required": ["text"], "additionalProperties": false }
        }),
    ]
}

/// The cloud side of a tool call.
#[async_trait]
pub trait CloudTools: Send + Sync {
    async fn call(&self, agent_id: &str, tool: &str, input: &Value) -> Result<(u16, Value), String>;
}

pub struct ProdCloud;

#[async_trait]
impl CloudTools for ProdCloud {
    async fn call(&self, agent_id: &str, tool: &str, input: &Value) -> Result<(u16, Value), String> {
        let bearer = crate::phone_sync::runtime_bearer().ok_or("this runtime isn't paired")?;
        let url = format!(
            "{}/api/v1/runtime-devices/me/platform-agents/{}/tools/{}",
            crate::phone_sync::cloud_base(),
            urlencoding::encode(agent_id),
            urlencoding::encode(tool)
        );
        let res = crate::net_client::client()
            .post(url)
            .bearer_auth(bearer)
            .timeout(std::time::Duration::from_secs(30))
            .json(&json!({ "input": input }))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = res.status().as_u16();
        Ok((status, res.json().await.unwrap_or(Value::Null)))
    }
}

fn refusal(code: &str, message: &str) -> Value {
    json!({ "ok": false, "error": code, "message": message })
}

/// The agent behind a session, when it is this owner's bot and lists `tool`.
pub fn calling_agent(db: &DbHandle, owner: &str, session_id: Option<&str>, tool: &str) -> Result<String, Value> {
    let Some(session_id) = session_id.filter(|s| !s.is_empty()) else {
        return Err(refusal("no_session", "This tool only works inside a hosted agent's conversation."));
    };
    let agent_id = db
        .get_session_metadata(session_id)
        .ok()
        .flatten()
        .and_then(|bag| bag["agentId"].as_str().map(str::to_string))
        .ok_or_else(|| refusal("no_session", "This tool only works inside a hosted agent's conversation."))?;
    let row: Option<(String, Option<String>)> = db.connect().ok().and_then(|c| {
        c.query_row("SELECT user_id, json_extract(config, '$.platformAgent.tools') FROM agents WHERE id = ?1", params![agent_id], |r| Ok((r.get(0)?, r.get(1)?))).ok()
    });
    let Some((_, tools)) = row.filter(|(o, _)| o == owner) else {
        return Err(refusal("no_session", "This tool only works inside a hosted agent's conversation."));
    };
    let tools: Vec<String> = tools.and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    if !tools.iter().any(|t| t == tool) {
        return Err(refusal("tool_not_allowed", "This agent doesn't have that tool."));
    }
    Ok(agent_id)
}

/// Run a tool. A refusal is an `ok:false` result with a plain sentence, not a tool error.
pub async fn call_mcp_tool(db: &DbHandle, owner: &str, name: &str, args: Value, meta: Option<&Value>) -> Result<Value, (StatusCode, Json<Value>)> {
    call_mcp_tool_with(db, &ProdCloud, owner, name, args, meta).await
}

pub async fn call_mcp_tool_with(db: &DbHandle, cloud: &dyn CloudTools, owner: &str, name: &str, args: Value, meta: Option<&Value>) -> Result<Value, (StatusCode, Json<Value>)> {
    let Some(tool) = agent_tool_for(name).filter(|_| is_tool(name)) else {
        return Err((StatusCode::NOT_FOUND, Json(json!({ "error": format!("unknown tool {name}") }))));
    };
    if !platform_owner(owner) {
        return Ok(refusal("not_available", "This tool is only for hosted agents."));
    }
    let session = meta.and_then(|m| m.get(SESSION_META)).and_then(Value::as_str);
    let agent_id = match calling_agent(db, owner, session, tool) {
        Ok(a) => a,
        Err(r) => return Ok(r),
    };
    match cloud.call(&agent_id, tool, &args).await {
        Ok((status, body)) if (200..300).contains(&status) => Ok(body),
        Ok((_, body)) if body.get("ok") == Some(&Value::Bool(false)) => Ok(body),
        Ok((status, _)) => Ok(refusal("tool_failed", &format!("The tool failed (status {status}). Try again shortly."))),
        Err(e) => {
            tracing::warn!(agent = %agent_id, tool, "platform tool: cloud unreachable: {e}");
            Ok(refusal("tool_failed", "The tool couldn't be reached. Try again shortly."))
        }
    }
}

// ---------------------------------------------------------------- phone (texts and calls)

/// Longest call purpose a tool call may carry.
const MAX_PURPOSE_CHARS: usize = 2000;

/// `phone_text` / `phone_call` for a hosted agent (owner `platform:…`).
///
/// The number is one the developer bound to this agent (`AgentSpec.phoneNumbers`,
/// stored with no sealed `sms` account), so this does not go through
/// [`crate::phone_outbound::text_in_session`]: cloud-api sends from the agent tool
/// route and keeps consent, opt-out, card, spend and metering. Autonomy stays here:
/// the agent's level for `sms` / `call` decides. A held send files one approval on
/// the calling session (`thread_id = session_id = session`), correlation
/// `text:<to>` / `call:<to>`; once the developer approves it, the next call sends
/// the approved text (or purpose) word for word with its `approval_id`, and the
/// approval is consumed only after the cloud accepted it.
pub async fn phone_tool(db: &DbHandle, cloud: &dyn CloudTools, owner: &str, name: &str, args: Value, meta: Option<&Value>) -> Result<Value, (StatusCode, Json<Value>)> {
    let (tool, channel, verb) = match agent_tool_for(name) {
        Some(t @ "send_text") => (t, "sms", "text"),
        Some(t @ "call") => (t, "call", "call"),
        _ => return Err((StatusCode::NOT_FOUND, Json(json!({ "error": format!("unknown tool {name}") })))),
    };
    if !platform_owner(owner) {
        return Ok(refusal("not_available", "This tool is only for hosted agents."));
    }
    let session = meta.and_then(|m| m.get(SESSION_META)).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    let agent_id = match calling_agent(db, owner, session, tool) {
        Ok(a) => a,
        Err(r) => return Ok(r),
    };
    let session = session.unwrap_or_default();
    let arg = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
    let number_id = match agent_number(db, owner, &agent_id, &arg("from")) {
        Ok(n) => n,
        Err(r) => return Ok(r),
    };
    let to = arg("to");
    if !crate::channel_phone::is_e164(&to) {
        return Ok(refusal("invalid_number", &format!("The number to {verb} must look like +14155550123.")));
    }
    let content = if tool == "send_text" {
        // SMS can't draw interactive cards: ```openui fences go out as text.
        let t = crate::openui_text::to_plain_text(args.get("text").and_then(Value::as_str).unwrap_or(""));
        if t.trim().is_empty() {
            return Ok(refusal("empty", "There's nothing to send."));
        }
        // The cloud's cap for one agent text.
        if t.chars().count() > crate::channel_phone::SMS_MAX_CHARS {
            return Ok(refusal("too_long", "That text is too long (1600 characters at most). Shorten it and try again."));
        }
        t
    } else {
        let p = arg("purpose");
        if p.is_empty() {
            return Ok(refusal("purpose_required", "Say what the call is for."));
        }
        if p.chars().count() > MAX_PURPOSE_CHARS {
            return Ok(refusal("too_long", "That call purpose is too long. Shorten it and try again."));
        }
        p
    };
    let field = if tool == "send_text" { "text" } else { "purpose" };
    let act = crate::autonomy::Action { owner, bot_id: &agent_id, channel, persons: vec![to.clone()], action: if tool == "call" { "call" } else { "message" }, amount_cents: 0 };
    let aut = crate::autonomy::evaluate_ctx(db, &act, false, &crate::autonomy::Ctx { thread_id: Some(session), text: &content, destination: "" });
    let corr = format!("{verb}:{to}");
    let mut approval: Option<String> = None;
    let mut content = content;
    match aut.as_ref().map(|e| &e.verdict) {
        Some(crate::autonomy::Verdict::Block { reason }) => {
            crate::autonomy::record_in(db, &act, aut.as_ref().map(|e| e.level.as_str()).unwrap_or("default"), "blocked", reason, Some(session));
            return Ok(refusal("blocked", reason));
        }
        Some(crate::autonomy::Verdict::Draft { reason }) => {
            crate::autonomy::finish(db, &act, aut.as_ref(), "drafted", &format!("{corr}:{session}"), &content, reason, Some(session), json!({ "to": to, field: content }));
            return Ok(refusal("drafted", &format!("{reason} It was saved as a draft and not sent.")));
        }
        Some(crate::autonomy::Verdict::Hold { reason }) => {
            let conn = db.connect().map_err(internal)?;
            let approved: Option<(String, String)> = conn
                .query_row(
                    "SELECT id, detail_json FROM gateway_approvals WHERE owner = ?1 AND thread_id = ?2 AND bot_id = ?3 AND authority = 'allternit' AND state = 'approved' AND consumed = 0 AND correlation_id = ?4 ORDER BY created_at LIMIT 1",
                    params![owner, session, agent_id, corr],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(internal)?;
            match approved {
                // What the developer approved goes out, word for word.
                Some((id, detail)) => {
                    let detail: Value = serde_json::from_str(&detail).unwrap_or(Value::Null);
                    if let Some(approved) = detail.get(field).and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
                        content = approved.to_string();
                    }
                    approval = Some(id);
                }
                None => {
                    let pending = conn
                        .query_row(
                            "SELECT 1 FROM gateway_approvals WHERE owner = ?1 AND thread_id = ?2 AND bot_id = ?3 AND authority = 'allternit' AND state = 'pending' AND correlation_id = ?4",
                            params![owner, session, agent_id, corr],
                            |_| Ok(()),
                        )
                        .optional()
                        .map_err(internal)?
                        .is_some();
                    if !pending {
                        let summary = match tool {
                            "send_text" => format!("Text {to}: {}", preview(&content)),
                            _ => format!("Call {to}: {}", preview(&content)),
                        };
                        // cloud-api matches the approval's kind to `text <to>` / `call <to>` exactly.
                        let cx = crate::gateway_runner::Cx { owner: owner.to_string(), thread_id: session.to_string(), bot_id: agent_id.clone(), generation: 1, session_id: session.to_string(), exec: json!({ "vendor": channel }) };
                        let detail = json!({ "to": to, field: content, "summary": summary, "callingSession": session });
                        let aid = crate::gateway_runner::create_approval(db, &cx, "allternit", &format!("{verb} {to}"), None, detail, Some(&corr)).map_err(|e| internal(e.message))?;
                        crate::autonomy::finish(db, &act, aut.as_ref(), "held", &aid, &content, reason, Some(session), json!({ "approvalId": aid, "to": to, field: content }));
                    }
                    return Ok(refusal("approval_required", &format!("{reason} The developer has been asked to approve it; try again once they approve.")));
                }
            }
        }
        _ => {}
    }
    let mut input = json!({ "to": to, field: content, "number_id": number_id });
    if let Some(a) = &approval {
        input["approval_id"] = json!(a);
    }
    match cloud.call(&agent_id, tool, &input).await {
        Ok((status, body)) if (200..300).contains(&status) => {
            if let (Some(a), Ok(c)) = (&approval, db.connect()) {
                let _ = c.execute("UPDATE gateway_approvals SET consumed = 1 WHERE id = ?1", params![a]);
            }
            let key = body.get("message_id").or_else(|| body.get("call_id")).and_then(Value::as_str).unwrap_or(&corr).to_string();
            crate::autonomy::finish(db, &act, aut.as_ref(), "sent", &key, &content, "", Some(session), json!({ "to": to, field: content }));
            Ok(body)
        }
        Ok((_, body)) if body.get("ok") == Some(&Value::Bool(false)) => Ok(body),
        Ok((status, _)) => Ok(refusal("tool_failed", &format!("The {verb} failed (status {status}). Try again shortly."))),
        Err(e) => {
            tracing::warn!(agent = %agent_id, tool, "platform phone tool: cloud unreachable: {e}");
            Ok(refusal("tool_failed", "The tool couldn't be reached. Try again shortly."))
        }
    }
}

fn internal(e: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))
}

fn preview(text: &str) -> String {
    let t: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > 200 {
        format!("{}…", t.chars().take(200).collect::<String>())
    } else {
        t
    }
}

/// The number the agent sends from: `from` (one of its numbers, E.164) when given, else its only one.
fn agent_number(db: &DbHandle, owner: &str, agent_id: &str, from: &str) -> Result<String, Value> {
    use crate::phone_outbound::{pick_number, OutError};
    let number_id = if from.is_empty() {
        None
    } else {
        if !crate::channel_phone::is_e164(from) {
            return Err(refusal("invalid_number", "from must be one of your numbers, like +14155550123."));
        }
        let id: Option<String> = db.connect().ok().and_then(|c| {
            c.query_row("SELECT number_id FROM channel_phone_numbers WHERE owner = ?1 AND bot_id = ?2 AND e164 = ?3", params![owner, agent_id, from], |r| r.get(0)).optional().ok().flatten()
        });
        Some(id.ok_or_else(|| refusal("number_not_found", &format!("{from} isn't one of your numbers.")))?)
    };
    match pick_number(db, owner, Some(agent_id), number_id.as_deref()) {
        Ok(n) => Ok(n.number_id),
        Err(OutError::NotFound(_)) => Err(refusal("no_number", "You don't have a phone number yet. Your developer binds one to you first.")),
        Err(OutError::BadRequest(_)) => {
            let list: Vec<String> = db
                .connect()
                .ok()
                .and_then(|c| {
                    let mut q = c.prepare("SELECT e164 FROM channel_phone_numbers WHERE owner = ?1 AND bot_id = ?2 ORDER BY e164").ok()?;
                    let v = q.query_map(params![owner, agent_id], |r| r.get(0)).ok()?.filter_map(Result::ok).collect();
                    Some(v)
                })
                .unwrap_or_default();
            Err(refusal("number_required", &format!("You have several numbers ({}); say which one to use with from.", list.join(", "))))
        }
        Err(e) => Err(refusal("tool_failed", &e.sentence())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const OWNER: &str = "platform:proj_1";

    #[derive(Default)]
    struct FakeCloud(Mutex<Vec<(String, String, Value)>>);
    #[async_trait]
    impl CloudTools for FakeCloud {
        async fn call(&self, agent_id: &str, tool: &str, input: &Value) -> Result<(u16, Value), String> {
            self.0.lock().unwrap().push((agent_id.into(), tool.into(), input.clone()));
            Ok((200, json!({ "ok": true, "results": [] })))
        }
    }

    async fn db() -> DbHandle {
        let dir = std::env::temp_dir().join(format!("allternit-platform-tools-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::test_helpers::app_state(&dir).await.db.clone()
    }

    fn meta(session: &str) -> Value {
        json!({ SESSION_META: session })
    }

    #[test]
    fn tools_are_listed_only_on_a_platform_runtime() {
        assert_eq!(mcp_tools(OWNER).len(), 2);
        assert!(mcp_tools("user_123").is_empty() && mcp_tools("platform:").is_empty());
        assert!(is_tool("agent_knowledge_search") && is_tool("agent_channel_post") && !is_tool("knowledge_search"));
    }

    #[tokio::test]
    async fn the_calling_agent_comes_from_the_session_never_from_input() {
        let db = db().await;
        crate::platform_agents::upsert_bot(&db, OWNER, "agent_a", &json!({ "name": "A", "tools": ["knowledge_search"] })).unwrap();
        crate::platform_agents::upsert_bot(&db, OWNER, "agent_b", &json!({ "name": "B", "tools": ["web_search"] })).unwrap();
        crate::platform_agents::upsert_bot(&db, "platform:proj_2", "agent_c", &json!({ "name": "C", "tools": ["knowledge_search"] })).unwrap();
        db.set_session_metadata("ses_a", &json!({ "agentId": "agent_a" })).unwrap();
        db.set_session_metadata("ses_b", &json!({ "agentId": "agent_b" })).unwrap();
        db.set_session_metadata("ses_c", &json!({ "agentId": "agent_c" })).unwrap();
        let cloud = FakeCloud::default();
        let args = json!({ "query": "hours", "agent_id": "agent_c" });

        let out = call_mcp_tool_with(&db, &cloud, OWNER, "agent_knowledge_search", args.clone(), Some(&meta("ses_a"))).await.unwrap();
        assert_eq!(out["ok"], true);
        assert_eq!(cloud.0.lock().unwrap()[0].0, "agent_a", "the session's agent, not the agent_id in the input");

        // Fail closed: no session meta, an unknown session, another owner's session, a tool the agent lacks.
        for (m, code) in [(None, "no_session"), (Some(meta("ses_x")), "no_session"), (Some(meta("ses_c")), "no_session"), (Some(meta("ses_b")), "tool_not_allowed")] {
            let out = call_mcp_tool_with(&db, &cloud, OWNER, "agent_knowledge_search", args.clone(), m.as_ref()).await.unwrap();
            assert_eq!(out["error"], code, "{m:?}");
        }
        let out = call_mcp_tool_with(&db, &cloud, "user_1", "agent_knowledge_search", args.clone(), Some(&meta("ses_a"))).await.unwrap();
        assert_eq!(out["error"], "not_available");
        assert_eq!(cloud.0.lock().unwrap().len(), 1, "nothing refused reached the cloud");
    }

    /// A cloud tool route that records each call and answers in order.
    struct ScriptedCloud {
        calls: Mutex<Vec<(String, String, Value)>>,
        replies: Mutex<Vec<(u16, Value)>>,
    }
    impl ScriptedCloud {
        fn new(replies: Vec<(u16, Value)>) -> Self {
            Self { calls: Mutex::default(), replies: Mutex::new(replies) }
        }
    }
    #[async_trait]
    impl CloudTools for ScriptedCloud {
        async fn call(&self, agent_id: &str, tool: &str, input: &Value) -> Result<(u16, Value), String> {
            self.calls.lock().unwrap().push((agent_id.into(), tool.into(), input.clone()));
            Ok(self.replies.lock().unwrap().remove(0))
        }
    }

    /// agent_a (send_text, call) with one bound number; ses_a is its conversation.
    async fn phone_agent(autonomy: &str) -> std::sync::Arc<crate::AppState> {
        let dir = std::env::temp_dir().join(format!("allternit-platform-phone-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::test_helpers::app_state(&dir).await;
        let db = state.db.clone();
        let spec = json!({ "name": "A", "tools": ["send_text", "call"], "autonomy": autonomy, "phoneNumbers": [{ "id": "pn_1", "e164": "+14155550100" }] });
        crate::platform_agents::upsert_bot(&db, OWNER, "agent_a", &spec).unwrap();
        db.set_session_metadata("ses_a", &json!({ "agentId": "agent_a" })).unwrap();
        state
    }

    fn sent(id: &str) -> (u16, Value) {
        (200, json!({ "ok": true, "message_id": id, "status": "sent", "segments": 1 }))
    }

    fn approvals(db: &DbHandle) -> Vec<(String, String, i64)> {
        let conn = db.connect().unwrap();
        let mut q = conn.prepare("SELECT id, state, consumed FROM gateway_approvals WHERE thread_id = 'ses_a' ORDER BY created_at, rowid").unwrap();
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect()
    }

    #[tokio::test]
    async fn an_asked_text_holds_once_then_sends_the_approved_words_once() {
        let state = phone_agent("ask").await;
        let db = state.db.clone();
        let cloud = ScriptedCloud::new(vec![sent("msg_1")]);
        let args = |text: &str| json!({ "to": "+14155550123", "text": text });

        let out = phone_tool(&db, &cloud, OWNER, "phone_text", args("Your cleaning is at 3pm."), Some(&meta("ses_a"))).await.unwrap();
        assert_eq!(out["error"], "approval_required", "{out}");
        // The model retries with other words: still the one pending approval, still nothing sent.
        let out = phone_tool(&db, &cloud, OWNER, "phone_text", args("Reminder: cleaning at 3."), Some(&meta("ses_a"))).await.unwrap();
        assert_eq!(out["error"], "approval_required");
        assert!(cloud.calls.lock().unwrap().is_empty(), "a held text never reaches the cloud");
        let list = approvals(&db);
        assert_eq!(list.len(), 1, "one approval per pending text");
        let aid = list[0].0.clone();
        let action: String = db.connect().unwrap().query_row("SELECT action FROM gateway_approvals WHERE id = ?1", params![aid], |r| r.get(0)).unwrap();
        assert_eq!(action, "text +14155550123", "cloud-api checks the approval's kind against `text <to>` exactly");
        let (session, payload): (Option<String>, String) = db.connect().unwrap()
            .query_row("SELECT session_id, payload FROM bot_events WHERE event_type = 'approval.requested' AND bot_id = 'agent_a'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        let payload: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(session.as_deref(), Some("ses_a"));
        assert_eq!(payload["session_id"], "ses_a", "the cloud ties it to the turn's response");
        assert_eq!(payload["summary"], "Text +14155550123: Your cleaning is at 3pm.", "the console shows what will be sent");

        // The developer approves through the runtime's real path.
        let tx = crate::gateway_runner::transport(&state);
        crate::gateway_runner::respond_approval(&db, tx.as_ref(), OWNER, &aid, "approve", ("user", "dev")).await.unwrap();
        let out = phone_tool(&db, &cloud, OWNER, "phone_text", args("Something else entirely"), Some(&meta("ses_a"))).await.unwrap();
        assert_eq!(out["message_id"], "msg_1", "{out}");
        let calls = cloud.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!((calls[0].0.as_str(), calls[0].1.as_str()), ("agent_a", "send_text"));
        assert_eq!(calls[0].2, json!({ "to": "+14155550123", "text": "Your cleaning is at 3pm.", "number_id": "pn_1", "approval_id": aid }), "the approved words, with the approval");
        assert_eq!(approvals(&db)[0], (aid.clone(), "approved".into(), 1), "consumed after the cloud accepted it");

        // The approval is spent: the next text holds again.
        let out = phone_tool(&db, &cloud, OWNER, "phone_text", args("One more"), Some(&meta("ses_a"))).await.unwrap();
        assert_eq!(out["error"], "approval_required");
        assert_eq!(approvals(&db).len(), 2);
        assert_eq!(cloud.calls.lock().unwrap().len(), 1);

        // A call holds the same way, under `call <to>`.
        let out = phone_tool(&db, &cloud, OWNER, "phone_call", json!({ "to": "+14155550123", "purpose": "Confirm 3pm" }), Some(&meta("ses_a"))).await.unwrap();
        assert_eq!(out["error"], "approval_required");
        let action: String = db.connect().unwrap().query_row("SELECT action FROM gateway_approvals WHERE correlation_id = 'call:+14155550123'", [], |r| r.get(0)).unwrap();
        assert_eq!(action, "call +14155550123");
    }

    #[tokio::test]
    async fn tell_sends_directly_and_refusals_never_reach_the_cloud() {
        let state = phone_agent("tell").await;
        let db = state.db.clone();
        crate::platform_agents::upsert_bot(&db, OWNER, "agent_b", &json!({ "name": "B", "tools": ["web_search"], "autonomy": "tell" })).unwrap();
        crate::platform_agents::upsert_bot(&db, "platform:proj_2", "agent_c", &json!({ "name": "C", "tools": ["send_text"], "autonomy": "tell" })).unwrap();
        db.set_session_metadata("ses_b", &json!({ "agentId": "agent_b" })).unwrap();
        db.set_session_metadata("ses_c", &json!({ "agentId": "agent_c" })).unwrap();
        let cloud = ScriptedCloud::new(vec![(200, json!({ "ok": true, "call_id": "call_1", "status": "queued" }))]);

        let out = phone_tool(&db, &cloud, OWNER, "phone_call", json!({ "to": "+14155550123", "purpose": "Confirm tomorrow's cleaning" }), Some(&meta("ses_a"))).await.unwrap();
        assert_eq!(out["call_id"], "call_1", "{out}");
        assert_eq!(cloud.calls.lock().unwrap()[0].2, json!({ "to": "+14155550123", "purpose": "Confirm tomorrow's cleaning", "number_id": "pn_1" }));
        assert!(approvals(&db).is_empty());
        let long = json!({ "to": "+14155550123", "text": "x".repeat(crate::channel_phone::SMS_MAX_CHARS + 1) });
        assert_eq!(phone_tool(&db, &cloud, OWNER, "phone_text", long, Some(&meta("ses_a"))).await.unwrap()["error"], "too_long", "over the cloud's 1600-character cap");

        let text = json!({ "to": "+14155550123", "text": "hi" });
        for (session, code) in [("ses_b", "tool_not_allowed"), ("ses_c", "no_session")] {
            let out = phone_tool(&db, &cloud, OWNER, "phone_text", text.clone(), Some(&meta(session))).await.unwrap();
            assert_eq!(out["error"], code, "{session}");
        }
        assert_eq!(cloud.calls.lock().unwrap().len(), 1, "nothing refused reached the cloud");
    }
}
