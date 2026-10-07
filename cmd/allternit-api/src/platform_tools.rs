//! Hosted-agent tools whose data lives in the cloud (Platform API spec §5):
//! `agent_knowledge_search` and `agent_channel_post`.
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
use rusqlite::params;
use serde_json::{json, Value};

use crate::db::DbHandle;

/// The `_meta` key gizzi puts the calling session's id under.
pub const SESSION_META: &str = "allternit/session";

/// MCP tool name → the agent tool it implements.
pub fn agent_tool_for(name: &str) -> Option<&'static str> {
    match name {
        "agent_knowledge_search" => Some("knowledge_search"),
        "agent_channel_post" => Some("channel_post"),
        _ => None,
    }
}

pub fn is_tool(name: &str) -> bool {
    agent_tool_for(name).is_some()
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
        let res = reqwest::Client::new()
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
    let Some((bot_owner, tools)) = row.filter(|(o, _)| o == owner) else {
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
    let Some(tool) = agent_tool_for(name) else {
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
}
