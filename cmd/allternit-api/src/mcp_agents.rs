//! Allternit Agents MCP surface — the read-only tools, the run-status MCP App
//! resource, and the OAuth (RFC 9728) plumbing that let ChatGPT and Claude
//! connect to `/mcp/server` as a publishable "Allternit Plugin".
//!
//! Clerk is the authorization server. Access tokens issued through its OAuth
//! flow are JWTs without a `sid` claim; browser session tokens carry `sid` and
//! keep working unchanged on the full tool catalog. An OAuth caller sees only
//! the read-only agent tools below and must hold `agents:read` with `aud`
//! equal to the public MCP URL.

use axum::{
    extract::{Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rusqlite::params;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::auth::{
    is_clerk_issuer, user_from_claims, verify_token_claims, AuthConfig, AuthUser, JwksManager,
    CLERK_PROXY_ISSUER,
};
use crate::AppState;

pub const REQUIRED_SCOPE: &str = "agents:read";
pub const RUN_STATUS_URI: &str = "ui://allternit/run-status.v1.html";
pub const MCP_APP_MIME: &str = "text/html;profile=mcp-app";
const DEFAULT_MCP_PUBLIC_URL: &str = "https://mcp.allternit.com/mcp";
const RUN_STATUS_HTML: &str = include_str!("../assets/run-status.v1.html");
const MAX_RESULT_CHARS: usize = 20_000;

pub const SERVER_INSTRUCTIONS: &str = mcp_protocol::servers::AGENTS_INSTRUCTIONS;

// ─── OAuth protected-resource metadata ─────────────────────────────────────────

/// Public MCP URL (`MCP_PUBLIC_URL`) — the OAuth resource / expected `aud`.
pub fn public_mcp_url() -> String {
    std::env::var("MCP_PUBLIC_URL")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_MCP_PUBLIC_URL.to_string())
}

pub(crate) fn oauth_issuer() -> String {
    std::env::var("MCP_OAUTH_ISSUER")
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| CLERK_PROXY_ISSUER.to_string())
}

/// RFC 9728 §3.1: `/.well-known/oauth-protected-resource` inserted between the
/// origin and the resource path.
pub fn resource_metadata_url(resource: &str) -> String {
    let (origin, path) = match resource.find("://") {
        Some(i) => match resource[i + 3..].find('/') {
            Some(j) => resource.split_at(i + 3 + j),
            None => (resource, ""),
        },
        None => (resource, ""),
    };
    format!(
        "{origin}/.well-known/oauth-protected-resource{}",
        path.trim_end_matches('/')
    )
}

pub fn protected_resource_metadata(resource: &str) -> Value {
    json!({
        "resource": resource,
        "authorization_servers": [oauth_issuer()],
        "scopes_supported": [REQUIRED_SCOPE],
        "bearer_methods_supported": ["header"],
        "resource_name": "Allternit Agents"
    })
}

/// Public (no auth) discovery routes, including the path-suffixed variant.
pub fn well_known_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(protected_resource_doc),
        )
        .route(
            "/.well-known/oauth-protected-resource/*rest",
            get(protected_resource_doc_at),
        )
}

async fn protected_resource_doc() -> Json<Value> {
    Json(protected_resource_metadata(&public_mcp_url()))
}

/// Path-suffixed variant. `/.well-known/oauth-protected-resource/mcp/bots/<id>`
/// describes that vendor-bot connector (scope `bots:act`); every other suffix
/// keeps describing the agents server. Nothing here says whether the bot exists.
async fn protected_resource_doc_at(axum::extract::Path(rest): axum::extract::Path<String>) -> Json<Value> {
    match crate::mcp_vendor_bots::bot_id_from_resource_path(&rest) {
        Some(id) => Json(crate::mcp_vendor_bots::bot_protected_resource_metadata(id)),
        None => protected_resource_doc().await,
    }
}

fn challenge_value(error: Option<(&str, &str)>) -> HeaderValue {
    challenge_value_for(&public_mcp_url(), error)
}

/// The RFC 9728 challenge for one resource (the agents server or a vendor-bot connector).
pub(crate) fn challenge_value_for(resource: &str, error: Option<(&str, &str)>) -> HeaderValue {
    let meta = resource_metadata_url(resource);
    let v = match error {
        Some((code, desc)) => format!(
            "Bearer error=\"{code}\", error_description=\"{desc}\", resource_metadata=\"{meta}\""
        ),
        None => format!("Bearer resource_metadata=\"{meta}\""),
    };
    HeaderValue::from_str(&v).unwrap_or_else(|_| HeaderValue::from_static("Bearer"))
}

/// Outermost layer on the protected router: any 401 from `/mcp/server` (missing
/// or invalid token, rejected by `auth_middleware`) gets the RFC 9728
/// `WWW-Authenticate` challenge that starts the client's OAuth flow.
pub async fn mcp_challenge_layer(request: Request, next: Next) -> Response {
    let path = request.uri().path().to_string();
    let is_mcp = path.starts_with("/mcp/server");
    let bot_resource = path
        .strip_prefix("/mcp/bots/")
        .map(|id| id.trim_end_matches('/'))
        .filter(|id| !id.is_empty() && !id.contains('/'))
        .map(crate::mcp_vendor_bots::bot_resource_url);
    let mut response = next.run(request).await;
    if (is_mcp || bot_resource.is_some())
        && response.status() == StatusCode::UNAUTHORIZED
        && !response.headers().contains_key(header::WWW_AUTHENTICATE)
    {
        let challenge = match &bot_resource {
            Some(resource) => challenge_value_for(resource, None),
            None => challenge_value(None),
        };
        response.headers_mut().insert(header::WWW_AUTHENTICATE, challenge);
    }
    response
}

// ─── OAuth access-token verification ───────────────────────────────────────────

#[derive(Debug, PartialEq, Eq)]
pub enum McpTokenError {
    Invalid(String),
    InsufficientScope,
}

fn claim_has_audience(claims: &Value, resource: &str) -> bool {
    let norm = |s: &str| s.trim_end_matches('/').to_string();
    match claims.get("aud") {
        Some(Value::String(s)) => norm(s) == norm(resource),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str())
            .any(|s| norm(s) == norm(resource)),
        _ => false,
    }
}

fn claim_scopes(claims: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for key in ["scope", "scp"] {
        match claims.get(key) {
            Some(Value::String(s)) => out.extend(s.split_whitespace().map(str::to_string)),
            Some(Value::Array(a)) => {
                out.extend(a.iter().filter_map(|v| v.as_str()).map(str::to_string))
            }
            _ => {}
        }
    }
    out
}

/// Full verification of a Clerk OAuth access token for this MCP resource:
/// JWKS signature, issuer, expiry, `aud == resource`, scope contains
/// `agents:read`.
pub async fn verify_oauth_access_token(
    jwks: &JwksManager,
    token: &str,
    config: &AuthConfig,
    resource: &str,
) -> Result<AuthUser, McpTokenError> {
    verify_oauth_claims(jwks, token, config, resource, REQUIRED_SCOPE).await.map(|(user, _)| user)
}

/// [`verify_oauth_access_token`] for any resource and scope, also returning the
/// verified claims (the vendor-bot connector names the OAuth client from them).
pub async fn verify_oauth_claims(
    jwks: &JwksManager,
    token: &str,
    config: &AuthConfig,
    resource: &str,
    scope: &str,
) -> Result<(AuthUser, Value), McpTokenError> {
    let claims = verify_token_claims(jwks, token, config)
        .await
        .map_err(|e| McpTokenError::Invalid(e.to_string()))?;
    if !claim_has_audience(&claims, resource) {
        return Err(McpTokenError::Invalid("Invalid audience".into()));
    }
    if !claim_scopes(&claims).iter().any(|s| s == scope) {
        return Err(McpTokenError::InsufficientScope);
    }
    let user = user_from_claims(&claims).map_err(|e| McpTokenError::Invalid(e.to_string()))?;
    Ok((user, claims))
}

pub(crate) fn peek_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()
}

/// Decides how a caller is treated. `Ok(true)` = OAuth caller restricted to
/// the read-only agent tools; `Ok(false)` = an existing auth mode (session
/// token, desktop, internal, data-plane) with the unchanged catalog; `Err` =
/// a ready 401/403 response with the challenge header.
///
/// The peek at unverified claims only *routes*; a token classified as OAuth is
/// then fully verified, and a forged `sid` can only reach what a session token
/// already gets, after `auth_middleware` verified its signature.
pub async fn authorize_bearer(state: &AppState, headers: &HeaderMap) -> Result<bool, Response> {
    let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
    else {
        return Ok(false);
    };
    let Some(claims) = peek_claims(token) else {
        return Ok(false);
    };
    let issuer = claims.get("iss").and_then(|v| v.as_str()).unwrap_or("");
    if claims.get("sid").is_some() || !is_clerk_issuer(issuer, &state.auth_config.clerk_issuer) {
        return Ok(false);
    }
    match verify_oauth_access_token(&state.jwks, token, &state.auth_config, &public_mcp_url()).await
    {
        Ok(_) => Ok(true),
        Err(McpTokenError::Invalid(msg)) => {
            let mut resp = (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "invalid_token", "message": msg})),
            )
                .into_response();
            resp.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                challenge_value(Some(("invalid_token", "The access token is invalid"))),
            );
            Err(resp)
        }
        Err(McpTokenError::InsufficientScope) => {
            let mut resp = (
                StatusCode::FORBIDDEN,
                Json(json!({"error": "insufficient_scope", "scope": REQUIRED_SCOPE})),
            )
                .into_response();
            resp.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                challenge_value(Some(("insufficient_scope", "agents:read is required"))),
            );
            Err(resp)
        }
    }
}

// ─── Tool descriptors ──────────────────────────────────────────────────────────

const READ_ONLY: fn() -> Value = || {
    json!({
        "readOnlyHint": true,
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": false
    })
};

fn agent_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": {"type": "string"},
            "name": {"type": "string"},
            "description": {"type": ["string", "null"]},
            "type": {"type": "string"},
            "model": {"type": "string"},
            "provider": {"type": "string"},
            "status": {"type": "string"},
            "created_at": {"type": ["string", "null"]},
            "updated_at": {"type": ["string", "null"]},
            "last_run_at": {"type": ["string", "null"]}
        },
        "required": ["id", "name", "status"]
    })
}

fn run_summary_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "run_id": {"type": "string"},
            "agent_id": {"type": "string"},
            "agent_name": {"type": ["string", "null"]},
            "status": {"type": "string"},
            "started_at": {"type": ["string", "null"]},
            "completed_at": {"type": ["string", "null"]},
            "duration_ms": {"type": ["integer", "null"]},
            "error": {"type": ["string", "null"]}
        },
        "required": ["run_id", "agent_id", "status"]
    })
}

pub fn tool_descriptors() -> Vec<Value> {
    vec![
        json!({
            "name": "list_agents",
            "title": "List agents",
            "description": "List the caller's Allternit agents, most recently updated first. Optionally filter by status.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": {"type": "string", "description": "Only agents with this status, e.g. idle or running."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 100, "default": 25}
                },
                "additionalProperties": false
            },
            "outputSchema": {
                "type": "object",
                "properties": {"agents": {"type": "array", "items": agent_schema()}},
                "required": ["agents"]
            },
            "annotations": READ_ONLY()
        }),
        json!({
            "name": "get_agent",
            "title": "Get agent",
            "description": "Get one of the caller's agents by id.",
            "inputSchema": {
                "type": "object",
                "properties": {"agent_id": {"type": "string"}},
                "required": ["agent_id"],
                "additionalProperties": false
            },
            "outputSchema": {
                "type": "object",
                "properties": {"agent": agent_schema()},
                "required": ["agent"]
            },
            "annotations": READ_ONLY()
        }),
        json!({
            "name": "list_runs",
            "title": "List runs",
            "description": "List the caller's agent runs, newest first. Filter by agent_id and/or status (running, completed, failed).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": {"type": "string"},
                    "status": {"type": "string", "enum": ["running", "completed", "failed"]},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 100, "default": 25}
                },
                "additionalProperties": false
            },
            "outputSchema": {
                "type": "object",
                "properties": {"runs": {"type": "array", "items": run_summary_schema()}},
                "required": ["runs"]
            },
            "annotations": READ_ONLY()
        }),
        json!({
            "name": "get_run",
            "title": "Get run",
            "description": "Get a run's status and timestamps. Does not include the run's output; use get_run_result for that.",
            "inputSchema": {
                "type": "object",
                "properties": {"run_id": {"type": "string"}},
                "required": ["run_id"],
                "additionalProperties": false
            },
            "outputSchema": run_summary_schema(),
            "annotations": READ_ONLY()
        }),
        json!({
            "name": "get_run_result",
            "title": "Get run result",
            "description": "Get the output (or error) of a run. Long output is truncated and flagged.",
            "inputSchema": {
                "type": "object",
                "properties": {"run_id": {"type": "string"}},
                "required": ["run_id"],
                "additionalProperties": false
            },
            "outputSchema": {
                "type": "object",
                "properties": {
                    "run_id": {"type": "string"},
                    "status": {"type": "string"},
                    "output": {"type": ["string", "null"]},
                    "error": {"type": ["string", "null"]},
                    "truncated": {"type": "boolean"}
                },
                "required": ["run_id", "status", "truncated"]
            },
            "annotations": READ_ONLY()
        }),
        json!({
            "name": "render_run_status",
            "title": "Show run status",
            "description": "Display a run's status card. Call only when the user wants to see it; use get_run to read status as data.",
            "inputSchema": {
                "type": "object",
                "properties": {"run_id": {"type": "string"}},
                "required": ["run_id"],
                "additionalProperties": false
            },
            "outputSchema": run_summary_schema(),
            "annotations": READ_ONLY(),
            "_meta": {
                "ui": {"resourceUri": RUN_STATUS_URI, "visibility": ["model", "app"]},
                "ui/resourceUri": RUN_STATUS_URI
            }
        }),
    ]
}

pub fn is_agents_tool(name: &str) -> bool {
    matches!(
        name,
        "list_agents" | "get_agent" | "list_runs" | "get_run" | "get_run_result" | "render_run_status"
    )
}

// ─── Resources ─────────────────────────────────────────────────────────────────

pub fn resource_descriptors() -> Vec<Value> {
    vec![json!({
        "uri": RUN_STATUS_URI,
        "name": "run-status",
        "title": "Run status",
        "description": "Status card for one agent run.",
        "mimeType": MCP_APP_MIME
    })]
}

pub fn read_resource(uri: &str) -> Option<Value> {
    if uri != RUN_STATUS_URI {
        return None;
    }
    Some(json!({
        "contents": [{
            "uri": RUN_STATUS_URI,
            "mimeType": MCP_APP_MIME,
            "text": RUN_STATUS_HTML,
            "_meta": {"ui": {
                "csp": {"connectDomains": [], "resourceDomains": []},
                "prefersBorder": true
            }}
        }]
    }))
}

// ─── Tool execution (scoped to the caller) ─────────────────────────────────────

fn iso(ts: Option<String>) -> Value {
    match ts {
        Some(s) if s.len() == 19 && s.as_bytes()[10] == b' ' => json!(format!("{}Z", s.replacen(' ', "T", 1))),
        Some(s) => json!(s),
        None => Value::Null,
    }
}

fn limit_arg(args: &Value) -> i64 {
    args.get("limit").and_then(|v| v.as_i64()).unwrap_or(25).clamp(1, 100)
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty())
}

fn agent_json(row: &rusqlite::Row) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": row.get::<_, String>(0)?,
        "name": row.get::<_, String>(1)?,
        "description": row.get::<_, Option<String>>(2)?,
        "type": row.get::<_, String>(3)?,
        "model": row.get::<_, String>(4)?,
        "provider": row.get::<_, String>(5)?,
        "status": row.get::<_, String>(6)?,
        "created_at": iso(row.get(7)?),
        "updated_at": iso(row.get(8)?),
        "last_run_at": iso(row.get(9)?),
    }))
}

const AGENT_COLS: &str =
    "id, name, description, type, model, provider, status, created_at, updated_at, last_run_at";
const RUN_COLS: &str = "r.id, r.agent_id, a.name, r.status, r.created_at, r.completed_at, r.duration_ms, r.error";
const RUN_FROM: &str = "FROM agent_runs r LEFT JOIN agents a ON a.id = r.agent_id AND a.user_id = r.user_id";

fn run_json(row: &rusqlite::Row) -> rusqlite::Result<Value> {
    Ok(json!({
        "run_id": row.get::<_, String>(0)?,
        "agent_id": row.get::<_, String>(1)?,
        "agent_name": row.get::<_, Option<String>>(2)?,
        "status": row.get::<_, String>(3)?,
        "started_at": iso(row.get(4)?),
        "completed_at": iso(row.get(5)?),
        "duration_ms": row.get::<_, Option<i64>>(6)?,
        "error": row.get::<_, Option<String>>(7)?,
    }))
}

struct ToolOut {
    structured: Value,
    text: String,
    meta: Option<Value>,
}

fn run_summary_text(run: &Value) -> String {
    format!(
        "Run {} is {}.",
        run["run_id"].as_str().unwrap_or("?"),
        run["status"].as_str().unwrap_or("unknown")
    )
}

fn run_tool(db: &crate::db::DbHandle, user_id: &str, name: &str, args: &Value) -> Result<ToolOut, String> {
    let conn = db.connect().map_err(|e| e.to_string())?;
    let db_err = |e: rusqlite::Error| format!("Database error: {e}");
    match name {
        "list_agents" => {
            let mut sql = format!("SELECT {AGENT_COLS} FROM agents WHERE user_id = ?1");
            let mut binds: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id.to_string())];
            if let Some(status) = str_arg(args, "status") {
                sql.push_str(" AND status = ?2");
                binds.push(Box::new(status.to_string()));
            }
            sql.push_str(&format!(" ORDER BY updated_at DESC LIMIT {}", limit_arg(args)));
            let mut stmt = conn.prepare(&sql).map_err(db_err)?;
            let agents = stmt
                .query_map(rusqlite::params_from_iter(binds.iter().map(|b| b.as_ref())), agent_json)
                .map_err(db_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_err)?;
            Ok(ToolOut {
                text: format!("{} agent(s).", agents.len()),
                structured: json!({ "agents": agents }),
                meta: None,
            })
        }
        "get_agent" => {
            let id = str_arg(args, "agent_id").ok_or("agent_id is required")?;
            let agent = conn
                .query_row(
                    &format!("SELECT {AGENT_COLS} FROM agents WHERE id = ?1 AND user_id = ?2"),
                    params![id, user_id],
                    agent_json,
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => "Agent not found".to_string(),
                    e => db_err(e),
                })?;
            Ok(ToolOut {
                text: format!("Agent {}.", agent["name"].as_str().unwrap_or("?")),
                structured: json!({ "agent": agent }),
                meta: None,
            })
        }
        "list_runs" => {
            let mut sql = format!("SELECT {RUN_COLS} {RUN_FROM} WHERE r.user_id = ?1");
            let mut binds: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id.to_string())];
            for (key, col) in [("agent_id", "r.agent_id"), ("status", "r.status")] {
                if let Some(v) = str_arg(args, key) {
                    binds.push(Box::new(v.to_string()));
                    sql.push_str(&format!(" AND {col} = ?{}", binds.len()));
                }
            }
            sql.push_str(&format!(" ORDER BY r.created_at DESC LIMIT {}", limit_arg(args)));
            let mut stmt = conn.prepare(&sql).map_err(db_err)?;
            let runs = stmt
                .query_map(rusqlite::params_from_iter(binds.iter().map(|b| b.as_ref())), run_json)
                .map_err(db_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_err)?;
            Ok(ToolOut {
                text: format!("{} run(s).", runs.len()),
                structured: json!({ "runs": runs }),
                meta: None,
            })
        }
        "get_run" | "render_run_status" => {
            let id = str_arg(args, "run_id").ok_or("run_id is required")?;
            let run = conn
                .query_row(
                    &format!("SELECT {RUN_COLS} {RUN_FROM} WHERE r.id = ?1 AND r.user_id = ?2"),
                    params![id, user_id],
                    run_json,
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => "Run not found".to_string(),
                    e => db_err(e),
                })?;
            let meta = (name == "render_run_status")
                .then(|| json!({"ui": {"resourceUri": RUN_STATUS_URI}}));
            Ok(ToolOut { text: run_summary_text(&run), structured: run, meta })
        }
        "get_run_result" => {
            let id = str_arg(args, "run_id").ok_or("run_id is required")?;
            let (status, output, error): (String, Option<String>, Option<String>) = conn
                .query_row(
                    "SELECT status, output, error FROM agent_runs WHERE id = ?1 AND user_id = ?2",
                    params![id, user_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => "Run not found".to_string(),
                    e => db_err(e),
                })?;
            let truncated = output.as_ref().map_or(false, |o| o.chars().count() > MAX_RESULT_CHARS);
            let output = output.map(|o| o.chars().take(MAX_RESULT_CHARS).collect::<String>());
            let text = match (&output, &error) {
                (Some(o), _) => o.clone(),
                (None, Some(e)) => format!("Run failed: {e}"),
                _ => format!("Run is {status}; no output yet."),
            };
            Ok(ToolOut {
                text,
                structured: json!({
                    "run_id": id, "status": status, "output": output,
                    "error": error, "truncated": truncated
                }),
                meta: None,
            })
        }
        _ => Err(format!("Unknown tool: {name}")),
    }
}

/// Runs an agents tool; `None` when `name` is not one of them. The result is a
/// complete MCP `tools/call` result (errors are `isError`, never JSON-RPC errors).
pub async fn call_tool(state: &Arc<AppState>, user_id: &str, name: &str, args: Value) -> Option<Value> {
    if !is_agents_tool(name) {
        return None;
    }
    let db = state.db.clone();
    let (uid, tool) = (user_id.to_string(), name.to_string());
    let out = tokio::task::spawn_blocking(move || run_tool(&db, &uid, &tool, &args))
        .await
        .unwrap_or_else(|_| Err("internal error".to_string()));
    Some(match out {
        Ok(o) => {
            let mut result = json!({
                "content": [{"type": "text", "text": o.text}],
                "structuredContent": o.structured,
                "isError": false
            });
            if let Some(meta) = o.meta {
                result["_meta"] = meta;
            }
            result
        }
        Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::test_clerk_token_with_claims;

    const RES: &str = "https://mcp.allternit.com/mcp";

    fn now() -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
    }

    fn claims(aud: Value, scope: &str, exp_offset: i64) -> Value {
        json!({"iss": CLERK_PROXY_ISSUER, "sub": "user_1", "aud": aud, "scope": scope,
               "iat": now(), "exp": now() + exp_offset})
    }

    async fn verify(c: Value) -> Result<AuthUser, McpTokenError> {
        let cfg = AuthConfig::default();
        let jwks = JwksManager::new(&cfg);
        let token = test_clerk_token_with_claims(&jwks, c).await;
        verify_oauth_access_token(&jwks, &token, &cfg, RES).await
    }

    #[tokio::test]
    async fn good_token_verifies() {
        let user = verify(claims(json!(RES), "openid agents:read", 300)).await.unwrap();
        assert_eq!(user.user_id, "user_1");
    }

    #[tokio::test]
    async fn audience_array_and_trailing_slash_accepted() {
        assert!(verify(claims(json!(["other", "https://mcp.allternit.com/mcp/"]), "agents:read", 300)).await.is_ok());
    }

    #[tokio::test]
    async fn wrong_audience_rejected() {
        let err = verify(claims(json!("https://evil.example/mcp"), "agents:read", 300)).await.unwrap_err();
        assert_eq!(err, McpTokenError::Invalid("Invalid audience".into()));
    }

    #[tokio::test]
    async fn missing_audience_rejected() {
        let mut c = claims(json!(RES), "agents:read", 300);
        c.as_object_mut().unwrap().remove("aud");
        assert!(matches!(verify(c).await, Err(McpTokenError::Invalid(_))));
    }

    #[tokio::test]
    async fn missing_scope_rejected() {
        assert_eq!(
            verify(claims(json!(RES), "openid profile", 300)).await.unwrap_err(),
            McpTokenError::InsufficientScope
        );
    }

    #[tokio::test]
    async fn expired_token_rejected() {
        assert!(matches!(
            verify(claims(json!(RES), "agents:read", -3600)).await,
            Err(McpTokenError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn wrong_issuer_rejected() {
        let mut c = claims(json!(RES), "agents:read", 300);
        c["iss"] = json!("https://evil.example");
        assert!(matches!(verify(c).await, Err(McpTokenError::Invalid(_))));
    }

    #[tokio::test]
    async fn unknown_signing_key_rejected() {
        let cfg = AuthConfig::default();
        let signer = JwksManager::new(&cfg);
        let verifier = JwksManager::new(&cfg);
        let token = test_clerk_token_with_claims(&signer, claims(json!(RES), "agents:read", 300)).await;
        // `verifier` never learned the signer's key (kid lookup fails before any network use
        // only when cached; seed an unrelated key so the cache is warm).
        let _ = test_clerk_token_with_claims(&verifier, claims(json!(RES), "agents:read", 300)).await;
        assert!(verify_oauth_access_token(&verifier, &token, &cfg, RES).await.is_err());
    }

    #[test]
    fn scope_claim_forms() {
        assert_eq!(claim_scopes(&json!({"scp": ["agents:read"]})), vec!["agents:read"]);
        assert_eq!(claim_scopes(&json!({"scope": "a agents:read"})), vec!["a", "agents:read"]);
    }

    #[test]
    fn metadata_document_and_url() {
        assert_eq!(
            resource_metadata_url(RES),
            "https://mcp.allternit.com/.well-known/oauth-protected-resource/mcp"
        );
        assert_eq!(
            resource_metadata_url("https://mcp.allternit.com"),
            "https://mcp.allternit.com/.well-known/oauth-protected-resource"
        );
        let doc = protected_resource_metadata(RES);
        assert_eq!(doc["resource"], RES);
        assert_eq!(doc["scopes_supported"], json!(["agents:read"]));
        assert_eq!(doc["authorization_servers"][0], "https://allternit.com/__clerk");
    }

    #[test]
    fn instructions_are_short() {
        assert!(SERVER_INSTRUCTIONS.len() < 512, "{}", SERVER_INSTRUCTIONS.len());
    }

    #[test]
    fn tools_list_shape() {
        let tools = tool_descriptors();
        let names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            ["list_agents", "get_agent", "list_runs", "get_run", "get_run_result", "render_run_status"]
        );
        for t in &tools {
            assert!(is_agents_tool(t["name"].as_str().unwrap()));
            assert_eq!(t["annotations"]["readOnlyHint"], true);
            assert_eq!(t["annotations"]["destructiveHint"], false);
            assert_eq!(t["annotations"]["openWorldHint"], false);
            assert_eq!(t["inputSchema"]["type"], "object");
            assert_eq!(t["outputSchema"]["type"], "object");
            let has_ui = t.get("_meta").is_some();
            assert_eq!(has_ui, t["name"] == "render_run_status");
        }
        let render = tools.iter().find(|t| t["name"] == "render_run_status").unwrap();
        assert_eq!(render["_meta"]["ui"]["resourceUri"], RUN_STATUS_URI);
    }

    #[test]
    fn resources_list_and_read_ui() {
        let list = resource_descriptors();
        assert_eq!(list[0]["uri"], RUN_STATUS_URI);
        assert_eq!(list[0]["mimeType"], "text/html;profile=mcp-app");
        let read = read_resource(RUN_STATUS_URI).unwrap();
        let c = &read["contents"][0];
        assert_eq!(c["mimeType"], "text/html;profile=mcp-app");
        assert_eq!(c["_meta"]["ui"]["prefersBorder"], true);
        assert_eq!(c["_meta"]["ui"]["csp"]["connectDomains"], json!([]));
        let html = c["text"].as_str().unwrap();
        assert!(html.contains("ui/initialize") && html.contains("ui/notifications/tool-result"));
        assert!(html.contains("hostContext") && !html.contains("innerHTML"));
        assert!(read_resource("ui://allternit/other.html").is_none());
    }

    fn seeded_db() -> (tempfile::TempDir, crate::db::DbHandle) {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::DbHandle::new(dir.path().join("t.db")).unwrap();
        let conn = db.connect().unwrap();
        for (id, user) in [("a1", "user_1"), ("a2", "user_2")] {
            conn.execute(
                "INSERT INTO agents (id, user_id, name, model, provider) VALUES (?1, ?2, ?3, 'm', 'p')",
                params![id, user, format!("agent-{id}")],
            )
            .unwrap();
        }
        for (id, agent, user, status) in [
            ("r1", "a1", "user_1", "completed"),
            ("r2", "a1", "user_1", "failed"),
            ("r3", "a2", "user_2", "completed"),
        ] {
            conn.execute(
                "INSERT INTO agent_runs (id, agent_id, user_id, status, output, error, duration_ms, completed_at)
                 VALUES (?1, ?2, ?3, ?4, 'done', NULL, 1500, CURRENT_TIMESTAMP)",
                params![id, agent, user, status],
            )
            .unwrap();
        }
        (dir, db)
    }

    #[test]
    fn tools_are_scoped_to_caller() {
        let (_d, db) = seeded_db();
        let agents = run_tool(&db, "user_1", "list_agents", &json!({})).unwrap();
        assert_eq!(agents.structured["agents"].as_array().unwrap().len(), 1);
        assert!(run_tool(&db, "user_1", "get_agent", &json!({"agent_id": "a2"})).is_err());
        let runs = run_tool(&db, "user_1", "list_runs", &json!({})).unwrap();
        assert_eq!(runs.structured["runs"].as_array().unwrap().len(), 2);
        let failed = run_tool(&db, "user_1", "list_runs", &json!({"status": "failed", "agent_id": "a1"})).unwrap();
        assert_eq!(failed.structured["runs"][0]["run_id"], "r2");
        assert!(run_tool(&db, "user_1", "get_run", &json!({"run_id": "r3"})).is_err());
        assert!(run_tool(&db, "user_1", "get_run_result", &json!({"run_id": "r3"})).is_err());
        let run = run_tool(&db, "user_2", "get_run", &json!({"run_id": "r3"})).unwrap();
        assert_eq!(run.structured["agent_name"], "agent-a2");
        assert_eq!(run.structured["duration_ms"], 1500);
        let res = run_tool(&db, "user_1", "get_run_result", &json!({"run_id": "r1"})).unwrap();
        assert_eq!(res.structured["output"], "done");
        assert_eq!(res.structured["truncated"], false);
        let render = run_tool(&db, "user_1", "render_run_status", &json!({"run_id": "r1"})).unwrap();
        assert_eq!(render.meta.unwrap()["ui"]["resourceUri"], RUN_STATUS_URI);
    }
}
