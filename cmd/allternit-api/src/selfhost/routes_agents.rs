//! `/v1/agents`: CRUD over the store. Shapes follow the Platform v1 agent.

use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::auth::User;
use super::settings::Settings;
use super::store::{AgentPatch, AgentRecord, NewAgent};
use super::{
    check_len, deleted, list_envelope, on_delete, on_get, on_patch, on_post, page_limit, rfc3339,
    ApiError, ApiJson, ApiQuery, PageQuery, RouteBook, SelfHostState,
};

pub fn mount(book: RouteBook) -> RouteBook {
    book.add("/v1/agents", vec![on_get(list_agents), on_post(create_agent)])
        .add(
            "/v1/agents/:id",
            vec![on_get(get_agent), on_patch(update_agent), on_delete(delete_agent)],
        )
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Builtin,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTool {
    #[serde(rename = "type")]
    pub kind: ToolKind,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServer {
    pub name: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateAgentBody {
    pub name: String,
    #[serde(default)]
    pub instructions: Option<String>,
    pub model: String,
    #[serde(default)]
    pub tools: Option<Vec<AgentTool>>,
    #[serde(default)]
    pub mcp_servers: Option<Vec<McpServer>>,
    #[serde(default)]
    pub skills: Option<Vec<String>>,
    #[serde(default)]
    pub metadata: Option<Map<String, Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateAgentBody {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub tools: Option<Vec<AgentTool>>,
    #[serde(default)]
    pub mcp_servers: Option<Vec<McpServer>>,
    #[serde(default)]
    pub skills: Option<Vec<String>>,
    #[serde(default)]
    pub metadata: Option<Map<String, Value>>,
}

/// 422 `model_not_mapped` before any outbound request.
pub fn check_model(settings: &Settings, model: &str) -> Result<(), ApiError> {
    if model.trim().is_empty() {
        return Err(ApiError::invalid("invalid_field", "model cannot be empty."));
    }
    if !settings.model_allowed(model) {
        return Err(ApiError::unprocessable(
            "model_not_mapped",
            format!("The model {model:?} has no entry in the server's model map."),
        ));
    }
    Ok(())
}

fn check_tools(tools: &[AgentTool]) -> Result<(), ApiError> {
    for t in tools {
        check_len("tools[].name", &t.name, 1, 100)?;
        if let Some(d) = &t.description {
            check_len("tools[].description", d, 0, 2000)?;
        }
        if t.kind == ToolKind::Custom && t.input_schema.is_none() {
            return Err(ApiError::invalid(
                "invalid_field",
                format!("The custom tool {:?} needs an input_schema.", t.name),
            ));
        }
    }
    Ok(())
}

/// MCP servers: HTTPS only, host on `mcp.allowed_hostnames` exactly (no
/// subdomain matching). A refusal is 403 `host_not_allowed`.
fn check_mcp_servers(settings: &Settings, servers: &[McpServer]) -> Result<(), ApiError> {
    for s in servers {
        check_len("mcp_servers[].name", &s.name, 1, 100)?;
        let url = url::Url::parse(&s.url).map_err(|_| {
            ApiError::invalid("invalid_field", format!("mcp_servers[].url {:?} is not a URL.", s.url))
        })?;
        let host = url.host_str().unwrap_or("").to_ascii_lowercase();
        if url.scheme() != "https" {
            return Err(ApiError::forbidden(
                "host_not_allowed",
                format!("MCP server {:?} must use https.", s.name),
            ));
        }
        let with_port = url.port().map(|p| format!("{host}:{p}"));
        let listed = settings.mcp.allowed_hostnames.iter().any(|h| {
            let h = h.to_ascii_lowercase();
            h == host || with_port.as_deref() == Some(h.as_str())
        });
        if !listed {
            return Err(ApiError::forbidden(
                "host_not_allowed",
                format!("MCP host {host:?} is not in mcp.allowed_hostnames."),
            ));
        }
    }
    Ok(())
}

fn check_skills(skills: &[String]) -> Result<(), ApiError> {
    for s in skills {
        check_len("skills[]", s, 1, 200)?;
    }
    Ok(())
}

/// Header values are never returned in plain text; secret references
/// (`secret://name`) are shown as written.
fn redact_mcp(servers: &Value) -> Value {
    let Some(list) = servers.as_array() else {
        return Value::Array(vec![]);
    };
    Value::Array(
        list.iter()
            .map(|s| {
                let mut s = s.clone();
                if let Some(h) = s.get_mut("headers").and_then(Value::as_object_mut) {
                    for v in h.values_mut() {
                        let keep = v.as_str().is_some_and(|x| x.starts_with("secret://"));
                        if !keep {
                            *v = Value::String("[redacted]".into());
                        }
                    }
                }
                s
            })
            .collect(),
    )
}

pub fn agent_json(a: &AgentRecord) -> Value {
    json!({
        "id": a.id,
        "object": "agent",
        "name": a.name,
        "instructions": a.instructions,
        "model": a.model,
        "tools": a.tools,
        "mcp_servers": redact_mcp(&a.mcp_servers),
        "skills": a.skills,
        "metadata": a.metadata,
        "status": "ready",
        "created_at": rfc3339(a.created_at),
        "updated_at": rfc3339(a.updated_at),
    })
}

fn to_value<T: Serialize>(v: &T) -> Result<Value, ApiError> {
    serde_json::to_value(v).map_err(|e| ApiError::internal(e.to_string()))
}

async fn list_agents(
    State(st): State<SelfHostState>,
    _user: User,
    ApiQuery(q): ApiQuery<PageQuery>,
) -> Result<Json<Value>, ApiError> {
    let limit = page_limit(q.limit)?;
    let rows = st.store.list_agents(limit + 1, q.after.as_deref()).await?;
    Ok(Json(list_envelope(rows, limit, |a| a.id.clone(), agent_json)))
}

async fn create_agent(
    State(st): State<SelfHostState>,
    _user: User,
    ApiJson(body): ApiJson<CreateAgentBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_len("name", &body.name, 1, 100)?;
    let instructions = body.instructions.unwrap_or_default();
    check_len("instructions", &instructions, 0, 32_000)?;
    check_model(&st.settings, &body.model)?;
    let tools = body.tools.unwrap_or_default();
    check_tools(&tools)?;
    let mcp = body.mcp_servers.unwrap_or_default();
    check_mcp_servers(&st.settings, &mcp)?;
    let skills = body.skills.unwrap_or_default();
    check_skills(&skills)?;
    let agent = st
        .store
        .create_agent(NewAgent {
            name: body.name,
            instructions,
            model: body.model,
            tools: to_value(&tools)?,
            mcp_servers: to_value(&mcp)?,
            skills: to_value(&skills)?,
            metadata: Value::Object(body.metadata.unwrap_or_default()),
        })
        .await?;
    Ok((StatusCode::CREATED, Json(agent_json(&agent))))
}

async fn get_agent(
    State(st): State<SelfHostState>,
    _user: User,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let a = st.store.get_agent(&id).await.map_err(not_found_agent)?;
    Ok(Json(agent_json(&a)))
}

async fn update_agent(
    State(st): State<SelfHostState>,
    _user: User,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<UpdateAgentBody>,
) -> Result<Json<Value>, ApiError> {
    if let Some(n) = &body.name {
        check_len("name", n, 1, 100)?;
    }
    if let Some(i) = &body.instructions {
        check_len("instructions", i, 0, 32_000)?;
    }
    if let Some(m) = &body.model {
        check_model(&st.settings, m)?;
    }
    if let Some(t) = &body.tools {
        check_tools(t)?;
    }
    if let Some(m) = &body.mcp_servers {
        check_mcp_servers(&st.settings, m)?;
    }
    if let Some(s) = &body.skills {
        check_skills(s)?;
    }
    let patch = AgentPatch {
        name: body.name,
        instructions: body.instructions,
        model: body.model,
        tools: body.tools.as_ref().map(to_value).transpose()?,
        mcp_servers: body.mcp_servers.as_ref().map(to_value).transpose()?,
        skills: body.skills.as_ref().map(to_value).transpose()?,
        metadata: body.metadata.map(Value::Object),
    };
    let a = st.store.update_agent(&id, patch).await.map_err(not_found_agent)?;
    Ok(Json(agent_json(&a)))
}

async fn delete_agent(
    State(st): State<SelfHostState>,
    _user: User,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    st.store.delete_agent(&id).await.map_err(not_found_agent)?;
    Ok(Json(deleted(&id)))
}

fn not_found_agent(e: super::store::StoreError) -> ApiError {
    match e {
        super::store::StoreError::NotFound => ApiError::not_found("No agent with that id."),
        other => other.into(),
    }
}
