//! Hosted-agent tools that need the cloud's data (spec §5): the runtime calls
//! these while an agent works, signed in as the project's hosted runtime.
//!
//! `POST /api/v1/runtime-devices/me/platform-agents/{agent_id}/tools/{tool}`
//! `Authorization: Bearer allternit_runtime_…` (the runtime's device token),
//! body `{"input": {...}}`.
//!
//! Checks, all fail closed:
//! 1. The device token is valid and its owner is a project's runtime owner
//!    (`platform:<project_id>`); anything else is 403.
//! 2. The agent is a live agent of that project (another project's agent is 404).
//! 3. The tool is in the agent's `tools` list (403 `tool_not_allowed`). The
//!    runtime checks this too, and its permission ruleset hides other tools.
//! 4. Everything the tool touches is the agent's own account's: its own
//!    knowledge files, its account's channel connections.
//!
//! The runtime names the agent from the gizzi session the tool call came from,
//! never from model input (`cmd/allternit-api/src/platform_tools.rs`).
//!
//! Tools: `knowledge_search {query, limit?}`, `channel_post {text, channel?, target?}`,
//! `send_text {to, text, number_id?, approval_id?}`, `call {to, purpose, number_id?, approval_id?}`.
//!
//! **send_text and call** go out from a number bound to the agent
//! (`phone_numbers.agent_id`); with several bound numbers the runtime says which
//! with `number_id`. The cloud keeps the rules every send keeps: consent and STOP,
//! the card on file, the spend cap and metering (a sandbox number is simulated and
//! not metered). The runtime keeps autonomy: when its rules held the action for the
//! developer, it passes the approval as `approval_id`, which must be an `approved`
//! twin row of `platform_approvals` for this agent and recipient; it is the text's
//! idempotency key and becomes `consumed` once the action ran. A runtime that passes
//! no `approval_id` has evaluated autonomy itself (the trust boundary of a project's
//! own runtime). Answers: `{"ok":true,"message_id","status","segments"}` and
//! `{"ok":true,"call_id","status"}`; refusals are `{"ok":false,"error","message"}`.
//!
//! **channel_post and Phase 4.** End customers connect channels with
//! `/v1/channels/{kind}/connect` (Phase 4). Their connections are rows of
//! `platform_channel_connections` (project_id, account_id, kind, external_id,
//! default_target, status, deleted_at). Until that table exists, or when the
//! account has no `connected` row, the tool answers `no_channel_connected`.
//! Slack (external_id = team id) and Discord (external_id = guild id) post
//! through the shared apps; other kinds answer `channel_kind_not_supported`.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use super::calls::{self, CreateCall, VoiceDeps};
use super::numbers::{api_number, phone_error, ApiNumber};
use super::{PlatformCaller, PlatformError};
use crate::routes::phone;
use crate::routes::runtime_pairing::{device_token_from_headers, runtime_device_for_token};
use crate::ApiState;

pub fn runtime_routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/runtime-devices/me/platform-agents/:agent_id/tools/:tool", post(tool_h))
}

/// Tools served here.
pub const CLOUD_TOOLS: [&str; 4] = ["knowledge_search", "channel_post", "send_text", "call"];

pub const MAX_POST_CHARS: usize = 4000;
const MAX_QUERY_CHARS: usize = 500;
/// The longest text an agent sends: ten concatenated SMS segments.
pub const MAX_TEXT_CHARS: usize = 1600;

fn refuse(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "ok": false, "error": code, "message": message }))).into_response()
}

/// A Platform API error as a tool refusal (same status, `ok:false`).
fn refusal(e: PlatformError) -> Response {
    refuse(e.status, &e.code, &e.message)
}

#[derive(Debug, Deserialize, Default)]
struct ToolBody {
    #[serde(default)]
    input: Value,
}

struct AgentRef {
    id: String,
    project_id: String,
    account_id: String,
    name: String,
    tools: Vec<String>,
}

/// The agent, when the signed-in runtime is its project's runtime.
async fn agent_for_runtime(db: &PgPool, headers: &HeaderMap, agent_id: &str) -> Result<AgentRef, Response> {
    let Some(token) = device_token_from_headers(headers) else {
        return Err(refuse(StatusCode::UNAUTHORIZED, "unauthorized", "Runtime credential required."));
    };
    let device = runtime_device_for_token(db, token, None)
        .await
        .map_err(|_| refuse(StatusCode::UNAUTHORIZED, "unauthorized", "Invalid runtime credential."))?;
    let Some(project_id) = device.user_id.strip_prefix("platform:").filter(|p| !p.is_empty()) else {
        return Err(refuse(StatusCode::FORBIDDEN, "not_a_platform_runtime", "Only a project's hosted runtime can use agent tools."));
    };
    let row: Option<(String, String, Vec<String>)> = sqlx::query_as(
        "SELECT account_id, name, tools FROM platform_agents WHERE id = $1 AND project_id = $2 AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .bind(project_id)
    .fetch_optional(db)
    .await
    .map_err(|e| {
        tracing::warn!("platform agent tools: {e}");
        refuse(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", "Couldn't look up the agent.")
    })?;
    let Some((account_id, name, tools)) = row else {
        return Err(refuse(StatusCode::NOT_FOUND, "agent_not_found", "No such agent."));
    };
    Ok(AgentRef { id: agent_id.to_string(), project_id: project_id.to_string(), account_id, name, tools })
}

async fn tool_h(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    voice: Option<axum::Extension<Arc<VoiceDeps>>>,
    Path((agent_id, tool)): Path<(String, String)>,
    body: Option<Json<ToolBody>>,
) -> Response {
    let agent = match agent_for_runtime(&state.db, &headers, &agent_id).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if !CLOUD_TOOLS.contains(&tool.as_str()) {
        return refuse(StatusCode::NOT_FOUND, "unknown_tool", "No such tool.");
    }
    if !agent.tools.iter().any(|t| t == &tool) {
        return refuse(StatusCode::FORBIDDEN, "tool_not_allowed", "This agent doesn't have that tool.");
    }
    let input = body.map(|Json(b)| b.input).unwrap_or(Value::Null);
    match tool.as_str() {
        "knowledge_search" => knowledge_search(&state.db, &agent_id, &input).await,
        "send_text" => send_text(&state, &agent, &input).await,
        "call" => place_agent_call(&state, &agent, &input, voice.map(|axum::Extension(d)| d)).await,
        _ => channel_post(&state, &agent, &input, &ProdChannels).await,
    }
}

// ---------------------------------------------------------------- send_text and call

/// The caller of record for an agent's phone tool: the project, bound to the
/// agent's account, acting as `agent_tool`.
async fn tool_caller(db: &PgPool, agent: &AgentRef) -> Result<PlatformCaller, Response> {
    calls::internal_caller(db, &agent.project_id, &agent.account_id, "agent_tool", &["messaging", "voice"]).await.map_err(refusal)
}

/// The number a phone tool uses: `number_id` when given, which must be a live
/// number of this project bound to this agent; otherwise the agent's only number.
async fn from_number(db: &PgPool, caller: &PlatformCaller, agent: &AgentRef, input: &Value) -> Result<ApiNumber, Response> {
    let bound: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM phone_numbers WHERE project_id = $1 AND agent_id = $2 AND released_at IS NULL ORDER BY created_at, id",
    )
    .bind(&agent.project_id)
    .bind(&agent.id)
    .fetch_all(db)
    .await
    .map_err(|e| refusal(e.into()))?;
    let id = match input["number_id"].as_str().map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) if bound.iter().any(|b| b == n) => n.to_string(),
        Some(_) => return Err(refuse(StatusCode::FORBIDDEN, "number_not_bound", "That number isn't bound to this agent.")),
        None => match bound.as_slice() {
            [] => return Err(refuse(StatusCode::CONFLICT, "no_number", "No number is bound to this agent, so it can't text or call. Tell the person.")),
            [only] => only.clone(),
            _ => return Err(refuse(StatusCode::BAD_REQUEST, "number_required", "This agent has several numbers; say which with `number_id`.")),
        },
    };
    api_number(db, caller, &id).await.map_err(refusal)
}

/// An approval the runtime passed as `approval_id`.
struct UsedApproval {
    source_ref: String,
    id: String,
}

/// Check the developer's approval for this action (`approval_id` absent: none
/// needed, see the module docs). The runtime sends its own approval id (the
/// `gap_…` id of its `gateway_approvals` row), which is the twin row's
/// `source_ref`; the public `apr_…` id of the same row is accepted too. The row
/// must be this project's, raised by this agent for this recipient (its action is
/// `text <to>` / `call <to>`), and `approved`; pending, denied, expired and
/// already-consumed approvals are refused.
async fn check_approval(db: &PgPool, agent: &AgentRef, input: &Value, to: &str) -> Result<Option<UsedApproval>, Response> {
    let Some(approval_id) = input["approval_id"].as_str().map(str::trim).filter(|a| !a.is_empty()) else { return Ok(None) };
    #[derive(sqlx::FromRow)]
    struct Row {
        id: String,
        source_ref: String,
        status: String,
        kind: String,
        agent_id: Option<String>,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    }
    let row: Option<Row> = sqlx::query_as(
        "SELECT id, source_ref, status, kind, proposed_action->>'agent_id' AS agent_id, expires_at FROM platform_approvals \
         WHERE project_id = $1 AND source = 'twin' AND (id = $2 OR source_ref = $2) ORDER BY (id = $2) DESC LIMIT 1",
    )
    .bind(&agent.project_id)
    .bind(approval_id)
    .fetch_optional(db)
    .await
    .map_err(|e| refusal(e.into()))?;
    let Some(Row { id, source_ref, status, kind, agent_id: approval_agent, expires_at }) = row else {
        return Err(refuse(StatusCode::NOT_FOUND, "approval_not_found", "No such approval."));
    };
    if approval_agent.as_deref() != Some(agent.id.as_str()) || !kind.contains(to) {
        return Err(refuse(StatusCode::FORBIDDEN, "approval_mismatch", "That approval was for a different action."));
    }
    let status = super::approvals::effective_status(&status, expires_at, chrono::Utc::now());
    if status != "approved" {
        return Err(refuse(StatusCode::CONFLICT, "approval_not_approved", &format!("That approval is {status}, not approved, so nothing was sent.")));
    }
    Ok(Some(UsedApproval { source_ref, id }))
}

/// `approved → consumed` once the approved action ran (best effort: it already ran).
async fn consume(db: &PgPool, agent: &AgentRef, approval: Option<UsedApproval>) {
    if let Some(a) = approval {
        if let Err(e) = super::approvals::mark_consumed(db, &agent.project_id, "twin", &a.source_ref).await {
            tracing::warn!(approval = %a.id, "platform agent tool: approval not marked consumed: {e}");
        }
    }
}

async fn send_text(state: &ApiState, agent: &AgentRef, input: &Value) -> Response {
    let db = &state.db;
    let to = input["to"].as_str().unwrap_or("").trim();
    // The text is sent as given (an approved text is the approved wording).
    let text = input["text"].as_str().unwrap_or("");
    if to.is_empty() {
        return refuse(StatusCode::BAD_REQUEST, "invalid_to", "to must be an E.164 number like +14155550101.");
    }
    if text.trim().is_empty() || text.chars().count() > MAX_TEXT_CHARS {
        return refuse(StatusCode::BAD_REQUEST, "invalid_text", &format!("text must be 1 to {MAX_TEXT_CHARS} characters."));
    }
    let caller = match tool_caller(db, agent).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let number = match from_number(db, &caller, agent, input).await {
        Ok(n) => n,
        Err(r) => return r,
    };
    let approval = match check_approval(db, agent, input, to).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let carrier = if number.simulated {
        None
    } else {
        match phone::carrier() {
            Ok(c) => Some(c),
            Err(e) => return refusal(phone_error(e)),
        }
    };
    // One approval sends one text: the carrier dedupes a retry on this key.
    let idem = approval.as_ref().map(|a| format!("approval:{}", a.id));
    match super::texts::send_inner(db, &caller, &number, to, text, carrier.as_deref(), idem.as_deref()).await {
        Ok(m) => {
            consume(db, agent, approval).await;
            Json(json!({ "ok": true, "message_id": m.id, "status": m.status, "segments": m.segments })).into_response()
        }
        Err(e) => refusal(e),
    }
}

async fn place_agent_call(state: &ApiState, agent: &AgentRef, input: &Value, deps: Option<Arc<VoiceDeps>>) -> Response {
    let db = &state.db;
    let to = input["to"].as_str().unwrap_or("").trim();
    let purpose = input["purpose"].as_str().unwrap_or("");
    if to.is_empty() {
        return refuse(StatusCode::BAD_REQUEST, "invalid_to", "to must be an E.164 number like +14155550101.");
    }
    let caller = match tool_caller(db, agent).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let number = match from_number(db, &caller, agent, input).await {
        Ok(n) => n,
        Err(r) => return r,
    };
    let approval = match check_approval(db, agent, input, to).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let body = CreateCall { agent_id: agent.id.clone(), from_number_id: number.id, to: to.to_string(), purpose: purpose.to_string(), record: false };
    let call = match calls::place_call(state, &caller, deps, body).await {
        Ok(c) => c,
        Err(e) => return refusal(e),
    };
    consume(db, agent, approval).await;
    // A sandbox call is `in_progress` at once and holds the project's call slot
    // until someone ends it; nobody plays the turns of a call an agent placed,
    // so it ends here (`completed`, reason `simulated`) and gives the slot back.
    let status = if call.simulated {
        match calls::finish(db, &call.id, "completed", "simulated", None, None).await {
            Ok(Some(row)) => row.status,
            Ok(None) => call.status,
            Err(e) => {
                tracing::warn!(call = %call.id, code = %e.code, "platform agent call: simulated call not ended");
                call.status
            }
        }
    } else {
        call.status
    };
    Json(json!({ "ok": true, "call_id": call.id, "status": status })).into_response()
}

async fn knowledge_search(db: &PgPool, agent_id: &str, input: &Value) -> Response {
    let query = input["query"].as_str().unwrap_or("").trim();
    if query.is_empty() || query.chars().count() > MAX_QUERY_CHARS {
        return refuse(StatusCode::BAD_REQUEST, "invalid_query", "query must be 1 to 500 characters.");
    }
    let limit = input["limit"].as_i64().unwrap_or(5);
    match super::knowledge::search(db, agent_id, query, limit).await {
        Ok(hits) => Json(json!({ "ok": true, "results": hits })).into_response(),
        Err(e) => {
            tracing::warn!(agent = %agent_id, "platform knowledge search failed: {e}");
            refuse(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", "The search failed.")
        }
    }
}

/// One of the account's connected channels.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Connection {
    pub id: String,
    pub kind: String,
    pub external_id: String,
    pub default_target: Option<String>,
}

/// The account's connected channels (optionally of one kind). Empty when
/// Phase 4's connection table doesn't exist yet.
pub async fn account_connections(db: &PgPool, project_id: &str, account_id: &str, kind: Option<&str>) -> Result<Vec<Connection>, sqlx::Error> {
    let (exists,): (bool,) = sqlx::query_as("SELECT to_regclass('platform_channel_connections') IS NOT NULL").fetch_one(db).await?;
    if !exists {
        return Ok(vec![]);
    }
    sqlx::query_as::<_, Connection>(
        "SELECT id, kind, external_id, default_target FROM platform_channel_connections \
         WHERE project_id = $1 AND account_id = $2 AND status = 'connected' AND deleted_at IS NULL \
           AND ($3::text IS NULL OR kind = $3) ORDER BY id",
    )
    .bind(project_id)
    .bind(account_id)
    .bind(kind)
    .fetch_all(db)
    .await
}

/// Sends to a vendor. Production posts through the shared Slack and Discord apps.
#[async_trait::async_trait]
pub trait ChannelSender: Send + Sync {
    async fn send(&self, state: &ApiState, conn: &Connection, target: &str, bot_name: &str, text: &str) -> Result<Value, (StatusCode, String, String)>;
}

pub struct ProdChannels;

#[async_trait::async_trait]
impl ChannelSender for ProdChannels {
    async fn send(&self, state: &ApiState, conn: &Connection, target: &str, bot_name: &str, text: &str) -> Result<Value, (StatusCode, String, String)> {
        let unavailable = |what: &str| (StatusCode::SERVICE_UNAVAILABLE, "channel_unavailable".to_string(), format!("{what} isn't configured on this deployment."));
        match conn.kind.as_str() {
            "slack" => {
                let cfg = crate::routes::slack_app::app_config().ok_or_else(|| unavailable("The Slack app"))?;
                let body = crate::routes::slack_app::SendBody {
                    channel: target.to_string(),
                    text: text.to_string(),
                    thread_ts: None,
                    username: Some(bot_name.to_string()),
                    icon_url: None,
                    files: vec![],
                };
                crate::routes::slack_app::send_message_as_team(state, &crate::routes::slack_app::ReqwestSlackHttp, &cfg, &conn.external_id, &body)
                    .await
                    .map_err(|e| (StatusCode::BAD_GATEWAY, "channel_post_failed".to_string(), format!("Slack refused the message: {e}")))
            }
            "discord" => {
                let cfg = crate::routes::discord_app::DiscordConfig::from_env().ok_or_else(|| unavailable("The Discord app"))?;
                let body = crate::routes::discord_app::SendBody {
                    guild_id: conn.external_id.clone(),
                    channel_id: target.to_string(),
                    thread_id: None,
                    bot_name: bot_name.to_string(),
                    avatar_url: None,
                    text: text.to_string(),
                };
                let api = crate::routes::discord_app::default_api(&cfg);
                crate::routes::discord_app::send_message(&state.db, api.as_ref(), &body)
                    .await
                    .map(|ids| json!({ "messageIds": ids }))
                    .map_err(|e| (StatusCode::BAD_GATEWAY, "channel_post_failed".to_string(), format!("Discord refused the message: {e:?}")))
            }
            other => Err((
                StatusCode::CONFLICT,
                "channel_kind_not_supported".to_string(),
                format!("Posting to {other} from a hosted agent isn't available yet; Slack and Discord are."),
            )),
        }
    }
}

async fn channel_post(state: &ApiState, agent: &AgentRef, input: &Value, sender: &dyn ChannelSender) -> Response {
    let text = input["text"].as_str().unwrap_or("").trim();
    if text.is_empty() || text.chars().count() > MAX_POST_CHARS {
        return refuse(StatusCode::BAD_REQUEST, "invalid_text", "text must be 1 to 4000 characters.");
    }
    let kind = input["channel"].as_str().map(str::trim).filter(|k| !k.is_empty());
    let conns = match account_connections(&state.db, &agent.project_id, &agent.account_id, kind).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("platform channel_post: {e}");
            return refuse(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", "Couldn't look up the account's channels.");
        }
    };
    let Some(conn) = conns.first() else {
        return refuse(StatusCode::CONFLICT, "no_channel_connected", "No channel is connected for this account, so there is nowhere to post. Tell the person.");
    };
    if kind.is_none() && conns.iter().any(|c| c.kind != conn.kind) {
        let kinds: Vec<&str> = conns.iter().map(|c| c.kind.as_str()).collect();
        return refuse(StatusCode::BAD_REQUEST, "channel_ambiguous", &format!("This account has several channels ({}). Say which with `channel`.", kinds.join(", ")));
    }
    let target = input["target"].as_str().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string).or_else(|| conn.default_target.clone());
    let Some(target) = target else {
        return refuse(StatusCode::BAD_REQUEST, "channel_target_missing", "The connection has no default channel; pass `target` (the channel id).");
    };
    match sender.send(state, conn, &target, &agent.name, text).await {
        Ok(v) => Json(json!({ "ok": true, "channel": conn.kind, "connectionId": conn.id, "result": v })).into_response(),
        Err((status, code, message)) => refuse(status, &code, &message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::{test_state, MockGateway};
    use axum::body::Body;
    use std::sync::Mutex;
    use tower::ServiceExt;

    async fn setup() -> Arc<ApiState> {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        for sql in [
            include_str!("../../../migrations_pg/003_api_keys.sql"),
            include_str!("../../../migrations_pg/050_platform_api_foundation.sql"),
            include_str!("../../../migrations_pg/063_platform_agents.sql"),
            include_str!("../../../migrations_pg/064_platform_conversations.sql"),
            include_str!("../../../migrations_pg/067_platform_agent_knowledge.sql"),
            include_str!("../../../migrations_pg/126_key_governance.sql"),
        ] {
            sqlx::raw_sql(&sql.replace("public.", "")).execute(&state.db).await.expect("migration applies");
        }
        state
    }

    async fn device(db: &PgPool, id: &str, owner: &str) -> String {
        let token = format!("allternit_runtime_{id}_secret");
        sqlx::query("INSERT INTO runtime_devices (id, user_id, name, status, credential_expires_at, credential_hash) VALUES ($1, $2, 'rt', 'online', '2999-01-01', $3)")
            .bind(id)
            .bind(owner)
            .bind(crate::routes::runtime_pairing::sha256_hex(token.as_bytes()))
            .execute(db)
            .await
            .unwrap();
        token
    }

    /// Project, two accounts, one agent per account (both with every cloud tool unless told otherwise).
    async fn seed(db: &PgPool, project: &str, tools_a: &[&str]) {
        sqlx::query("INSERT INTO platform_projects (id, owner_user_id, name, env) VALUES ($1, 'dev', 'P', 'sandbox')").bind(project).execute(db).await.unwrap();
        for (acct, agent, tools) in [("acct_a", "agent_a", tools_a.to_vec()), ("acct_b", "agent_b", CLOUD_TOOLS.to_vec())] {
            let acct = format!("{project}_{acct}");
            sqlx::query("INSERT INTO platform_accounts (id, project_id, name) VALUES ($1, $2, 'Acct')").bind(&acct).bind(project).execute(db).await.unwrap();
            sqlx::query("INSERT INTO platform_agents (id, project_id, account_id, name, greeting, tools) VALUES ($1, $2, $3, 'Ada', 'Hi, an AI.', $4)")
                .bind(format!("{project}_{agent}"))
                .bind(project)
                .bind(&acct)
                .bind(tools.iter().map(|s| s.to_string()).collect::<Vec<_>>())
                .execute(db)
                .await
                .unwrap();
        }
    }

    async fn knowledge(db: &PgPool, agent: &str, account: &str, project: &str, file: &str, text: &str) {
        sqlx::query("INSERT INTO platform_knowledge_files (id, project_id, account_id, agent_id, name, content_type, bytes, sha256, storage_key, chunk_count) VALUES ($1,$2,$3,$4,'f.md','text/markdown',1,'x','k',1)")
            .bind(file).bind(project).bind(account).bind(agent).execute(db).await.unwrap();
        sqlx::query("INSERT INTO platform_knowledge_chunks (file_id, agent_id, ord, content) VALUES ($1,$2,0,$3)").bind(file).bind(agent).bind(text).execute(db).await.unwrap();
    }

    async fn post(state: &Arc<ApiState>, token: Option<&str>, agent: &str, tool: &str, input: Value) -> (StatusCode, Value) {
        let mut req = axum::http::Request::builder().method("POST").uri(format!("/api/v1/runtime-devices/me/platform-agents/{agent}/tools/{tool}")).header("content-type", "application/json");
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let resp = runtime_routes().with_state(state.clone()).oneshot(req.body(Body::from(json!({ "input": input }).to_string())).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn knowledge_search_finds_only_the_agents_own_files() {
        let state = setup().await;
        let db = &state.db;
        seed(db, "proj_k", &["knowledge_search"]).await;
        knowledge(db, "proj_k_agent_a", "proj_k_acct_a", "proj_k", "kf_a", "Lakeside Dental is open Saturday mornings from 8 to noon.").await;
        knowledge(db, "proj_k_agent_b", "proj_k_acct_b", "proj_k", "kf_b", "Riverside Dental is open Saturday evenings and Sunday.").await;
        let token = device(db, "rt_k", "platform:proj_k").await;

        let (s, b) = post(&state, Some(&token), "proj_k_agent_a", "knowledge_search", json!({ "query": "open on Saturday" })).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        let results = b["results"].as_array().unwrap();
        assert_eq!(results.len(), 1, "{b}");
        assert_eq!(results[0]["file_id"], "kf_a", "only agent A's own file");
        // Any-word fallback when not every word matches.
        let (_, b) = post(&state, Some(&token), "proj_k_agent_a", "knowledge_search", json!({ "query": "saturday parking garage" })).await;
        assert_eq!(b["results"][0]["file_id"], "kf_a", "{b}");
        // A deleted file is never found.
        sqlx::query("UPDATE platform_knowledge_files SET deleted_at = NOW() WHERE id = 'kf_a'").execute(db).await.unwrap();
        let (_, b) = post(&state, Some(&token), "proj_k_agent_a", "knowledge_search", json!({ "query": "Saturday" })).await;
        assert_eq!(b["results"], json!([]));
    }

    #[tokio::test]
    async fn tools_fail_closed() {
        let state = setup().await;
        let db = &state.db;
        seed(db, "proj_f", &["web_search"]).await;
        seed(db, "proj_g", &["knowledge_search"]).await;
        let token = device(db, "rt_f", "platform:proj_f").await;
        let user_token = device(db, "rt_user", "user_1").await;

        // No or a bad credential; a non-platform runtime.
        assert_eq!(post(&state, None, "proj_f_agent_b", "knowledge_search", json!({ "query": "x" })).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(post(&state, Some("allternit_runtime_nope"), "proj_f_agent_b", "knowledge_search", json!({ "query": "x" })).await.0, StatusCode::UNAUTHORIZED);
        let (s, b) = post(&state, Some(&user_token), "proj_f_agent_b", "knowledge_search", json!({ "query": "x" })).await;
        assert_eq!((s, b["error"].as_str()), (StatusCode::FORBIDDEN, Some("not_a_platform_runtime")));
        // Another project's agent looks missing.
        let (s, b) = post(&state, Some(&token), "proj_g_agent_a", "knowledge_search", json!({ "query": "x" })).await;
        assert_eq!((s, b["error"].as_str()), (StatusCode::NOT_FOUND, Some("agent_not_found")));
        // A tool the agent doesn't list.
        let (s, b) = post(&state, Some(&token), "proj_f_agent_a", "knowledge_search", json!({ "query": "x" })).await;
        assert_eq!((s, b["error"].as_str()), (StatusCode::FORBIDDEN, Some("tool_not_allowed")));
        let (s, _) = post(&state, Some(&token), "proj_f_agent_b", "calendar", json!({})).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        // No connection table yet (Phase 4): nothing to post to.
        let (s, b) = post(&state, Some(&token), "proj_f_agent_b", "channel_post", json!({ "text": "hello" })).await;
        assert_eq!((s, b["error"].as_str()), (StatusCode::CONFLICT, Some("no_channel_connected")), "{b}");
    }

    #[derive(Default)]
    struct FakeSender(Mutex<Vec<(String, String, String)>>);
    #[async_trait::async_trait]
    impl ChannelSender for FakeSender {
        async fn send(&self, _s: &ApiState, conn: &Connection, target: &str, _bot: &str, text: &str) -> Result<Value, (StatusCode, String, String)> {
            self.0.lock().unwrap().push((conn.id.clone(), target.to_string(), text.to_string()));
            Ok(json!({ "ts": "1" }))
        }
    }

    #[tokio::test]
    async fn channel_post_uses_only_the_accounts_connections() {
        let state = setup().await;
        let db = &state.db;
        seed(db, "proj_c", &["channel_post"]).await;
        // The connection table as Phase 4 defines it (minimal columns this tool reads).
        sqlx::raw_sql(
            "CREATE TABLE IF NOT EXISTS platform_channel_connections (id text PRIMARY KEY, project_id text NOT NULL, account_id text NOT NULL, kind text NOT NULL, \
             external_id text NOT NULL, default_target text, status text NOT NULL, created_at timestamptz NOT NULL DEFAULT now(), deleted_at timestamptz)",
        )
        .execute(db)
        .await
        .unwrap();
        let agent = |acct: &str| AgentRef { id: "proj_c_agent".into(), project_id: "proj_c".into(), account_id: format!("proj_c_{acct}"), name: "Ada".into(), tools: vec!["channel_post".into()] };
        let sender = FakeSender::default();
        let body = |r: Response| async move { serde_json::from_slice::<Value>(&axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap()).unwrap() };

        // Only account B has a channel: account A's agent can't reach it.
        sqlx::query("INSERT INTO platform_channel_connections (id, project_id, account_id, kind, external_id, default_target, status) VALUES ('cc_b', 'proj_c', 'proj_c_acct_b', 'slack', 'T1', 'C1', 'connected')")
            .execute(db)
            .await
            .unwrap();
        let r = channel_post(&state, &agent("acct_a"), &json!({ "text": "hi" }), &sender).await;
        assert_eq!(body(r).await["error"], "no_channel_connected");
        let r = channel_post(&state, &agent("acct_b"), &json!({ "text": "hi" }), &sender).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(sender.0.lock().unwrap().as_slice(), &[("cc_b".to_string(), "C1".to_string(), "hi".to_string())]);

        // A disconnected row doesn't count; two kinds need a choice.
        sqlx::query("INSERT INTO platform_channel_connections (id, project_id, account_id, kind, external_id, status) VALUES ('cc_a1', 'proj_c', 'proj_c_acct_a', 'slack', 'T2', 'revoked')").execute(db).await.unwrap();
        let r = channel_post(&state, &agent("acct_a"), &json!({ "text": "hi" }), &sender).await;
        assert_eq!(body(r).await["error"], "no_channel_connected");
        sqlx::query("INSERT INTO platform_channel_connections (id, project_id, account_id, kind, external_id, default_target, status) VALUES ('cc_b2', 'proj_c', 'proj_c_acct_b', 'discord', 'G1', 'D1', 'connected')").execute(db).await.unwrap();
        let r = channel_post(&state, &agent("acct_b"), &json!({ "text": "hi" }), &sender).await;
        assert_eq!(body(r).await["error"], "channel_ambiguous");
        let r = channel_post(&state, &agent("acct_b"), &json!({ "text": "hi", "channel": "discord" }), &sender).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(sender.0.lock().unwrap().last().unwrap().0, "cc_b2");
        let r = channel_post(&state, &agent("acct_b"), &json!({ "text": "" }), &sender).await;
        assert_eq!(body(r).await["error"], "invalid_text");
    }
}

/// `send_text` / `call` and the agent's numbers reaching its runtime. Real
/// Postgres, the real routes; only the hosted runtime is fake.
#[cfg(test)]
mod phone_tests {
    use super::*;
    use crate::routes::platform_v1::{
        conversations,
        hosting::{self, AgentHost, HostRuntime},
        projects::{self, Principal},
        router_gated, Gate, ProjectEnv,
    };
    use crate::routes::test_support::{test_state, MockGateway};
    use crate::routes::voice_calls_cloud::RelayStream;
    use crate::services::api_keys::{self, CreateProjectKeyInput};
    use axum::body::Body;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    use tower::ServiceExt;

    /// Records the agent definitions it is sent. With `ack` it answers like a
    /// current runtime (`phoneNumbers: <count>`); without, like one from before the field.
    #[derive(Default)]
    struct SpecHost {
        ack: AtomicBool,
        specs: Mutex<Vec<Value>>,
    }

    #[async_trait::async_trait]
    impl AgentHost for SpecHost {
        async fn runtime(&self, project_id: &str) -> Result<HostRuntime, PlatformError> {
            Ok(HostRuntime { owner: hosting::runtime_owner(project_id), runtime_id: "rt_1".into() })
        }
        async fn call(&self, _rt: &HostRuntime, method: &str, path: &str, body: &Value) -> Result<(u16, Value), PlatformError> {
            if method == "PUT" && path.starts_with("/api/v1/platform/agents/") {
                self.specs.lock().unwrap().push(body.clone());
                let mut answer = json!({ "ok": true });
                if self.ack.load(Ordering::SeqCst) {
                    if let Some(n) = body.get("phoneNumbers").and_then(Value::as_array) {
                        answer["phoneNumbers"] = json!(n.len());
                    }
                }
                return Ok((200, answer));
            }
            Ok((200, json!({ "sessionId": "ses_1" })))
        }
        async fn stream(&self, _rt: &HostRuntime, _path: &str, _body: &Value) -> Result<RelayStream, PlatformError> {
            Err(hosting::starting())
        }
    }

    struct Ctx {
        state: Arc<ApiState>,
        app: Router,
        host: Arc<SpecHost>,
    }

    async fn ctx() -> Ctx {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        crate::routes::test_support::platform_schema(&state.db).await;
        let host = Arc::new(SpecHost::default());
        let dyn_host: Arc<dyn AgentHost> = host.clone();
        let app = router_gated(&state, Gate::Forced(true)).merge(runtime_routes()).layer(axum::Extension(dyn_host)).with_state(state.clone());
        Ctx { state, app, host }
    }

    async fn req(app: &Router, method: &str, path: &str, token: &str, body: Option<Value>) -> (StatusCode, Value) {
        let b = axum::http::Request::builder().method(method).uri(path).header("authorization", format!("Bearer {token}"));
        let r = match body {
            Some(v) => b.header("content-type", "application/json").body(Body::from(v.to_string())).unwrap(),
            None => b.body(Body::empty()).unwrap(),
        };
        let resp = app.clone().oneshot(r).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// A sandbox project (card on file), its key, one account and its runtime's device token.
    struct Proj {
        id: String,
        key: String,
        acct: String,
        runtime: String,
    }

    async fn sandbox(c: &Ctx, owner: &str) -> Proj {
        let p = projects::create_project(&c.state.db, &Principal { user_id: owner.into(), org_id: None, org_admin: false }, "phone tools", ProjectEnv::Sandbox).await.unwrap();
        crate::routes::platform_v1::project_billing::put_test_card(&c.state.db, &p.id).await;
        let key = api_keys::create_project_key(
            &c.state.db,
            CreateProjectKeyInput {
                user_id: p.owner_user_id.clone(),
                organization_id: None,
                project_id: p.id.clone(),
                account_id: None,
                env: ProjectEnv::Sandbox,
                name: "k".into(),
                scopes: ["agents", "numbers", "messaging", "voice"].iter().map(|s| s.to_string()).collect(),
            },
        )
        .await
        .unwrap()
        .token;
        let (_, a) = req(&c.app, "POST", "/v1/accounts", &key, Some(json!({ "name": "Lakeside Dental" }))).await;
        let token = format!("allternit_runtime_{}_secret", p.id);
        sqlx::query("INSERT INTO runtime_devices (id, user_id, name, status, credential_expires_at, credential_hash) VALUES ($1, $2, 'rt', 'online', '2999-01-01', $3)")
            .bind(format!("rt_{}", p.id))
            .bind(format!("platform:{}", p.id))
            .bind(crate::routes::runtime_pairing::sha256_hex(token.as_bytes()))
            .execute(&c.state.db)
            .await
            .unwrap();
        Proj { id: p.id, key, acct: a["id"].as_str().unwrap().to_string(), runtime: token }
    }

    async fn agent(c: &Ctx, p: &Proj) -> String {
        let (s, ag) = req(&c.app, "POST", "/v1/agents", &p.key, Some(json!({ "account_id": p.acct, "name": "Front desk", "tools": ["send_text", "call"] }))).await;
        assert_eq!(s, StatusCode::CREATED, "{ag}");
        ag["id"].as_str().unwrap().to_string()
    }

    /// A simulated number, bound to `agent` when given. Returns (id, e164).
    async fn number(c: &Ctx, p: &Proj, agent: Option<&str>) -> (String, String) {
        let (s, n) = req(&c.app, "POST", "/v1/numbers", &p.key, Some(json!({ "account_id": p.acct, "agent_id": agent }))).await;
        assert_eq!(s, StatusCode::CREATED, "{n}");
        (n["id"].as_str().unwrap().to_string(), n["e164"].as_str().unwrap().to_string())
    }

    async fn consent(c: &Ctx, p: &Proj, number: &str, to: &str) {
        let (s, b) = req(&c.app, "POST", &format!("/v1/numbers/{number}/consent"), &p.key, Some(json!({ "e164": to, "source": "web form" }))).await;
        assert_eq!(s, StatusCode::CREATED, "{b}");
    }

    async fn tool(c: &Ctx, p: &Proj, agent: &str, name: &str, input: Value) -> (StatusCode, Value) {
        req(&c.app, "POST", &format!("/api/v1/runtime-devices/me/platform-agents/{agent}/tools/{name}"), &p.runtime, Some(json!({ "input": input }))).await
    }

    async fn synced(db: &PgPool, agent: &str) -> bool {
        sqlx::query_scalar::<_, Option<chrono::DateTime<chrono::Utc>>>("SELECT synced_at FROM platform_agents WHERE id = $1").bind(agent).fetch_one(db).await.unwrap().is_some()
    }

    async fn mark_synced(db: &PgPool, agents: &[&str]) {
        sqlx::query("UPDATE platform_agents SET synced_at = NOW() + INTERVAL '1 hour', runtime_id = 'rt_1' WHERE id = ANY($1)").bind(agents).execute(db).await.unwrap();
    }

    async fn ensure(c: &Ctx, p: &Proj, agent: &str) {
        let caller = calls::internal_caller(&c.state.db, &p.id, &p.acct, "test", &["agents"]).await.unwrap();
        conversations::ensure_agent(&c.state, c.host.as_ref(), &caller, agent).await.unwrap();
    }

    fn last_spec_for(c: &Ctx, numbers: usize) -> Value {
        let specs = c.host.specs.lock().unwrap();
        let last = specs.last().cloned().expect("a definition was sent");
        assert_eq!(last["phoneNumbers"].as_array().map(Vec::len), Some(numbers), "{last}");
        last
    }

    #[tokio::test]
    async fn the_runtime_gets_only_the_agents_live_numbers_and_must_acknowledge_them() {
        let c = ctx().await;
        let p = sandbox(&c, "dev_spec").await;
        let (a, b, plain) = (agent(&c, &p).await, agent(&c, &p).await, agent(&c, &p).await);
        let (n_a, e_a) = number(&c, &p, Some(&a)).await;
        number(&c, &p, Some(&b)).await;
        let (released, _) = number(&c, &p, Some(&a)).await;
        let (s, _) = req(&c.app, "DELETE", &format!("/v1/numbers/{released}"), &p.key, None).await;
        assert_eq!(s, StatusCode::OK);

        // A runtime from before `phoneNumbers` takes the definition but doesn't
        // acknowledge the numbers: the agent stays unsynced and is sent again.
        ensure(&c, &p, &a).await;
        let spec = last_spec_for(&c, 1);
        assert_eq!(spec["phoneNumbers"], json!([{ "id": n_a, "e164": e_a }]), "only A's live number");
        assert!(!synced(&c.state.db, &a).await, "no stamp without the ack");
        let sent = c.host.specs.lock().unwrap().len();
        ensure(&c, &p, &a).await;
        assert_eq!(c.host.specs.lock().unwrap().len(), sent + 1, "re-sent while unacknowledged");

        // An agent without numbers needs no ack.
        ensure(&c, &p, &plain).await;
        assert_eq!(last_spec_for(&c, 0)["phoneNumbers"], json!([]));
        assert!(synced(&c.state.db, &plain).await);

        // A current runtime acknowledges: synced, and not sent again.
        c.host.ack.store(true, Ordering::SeqCst);
        ensure(&c, &p, &a).await;
        assert!(synced(&c.state.db, &a).await);
        let sent = c.host.specs.lock().unwrap().len();
        ensure(&c, &p, &a).await;
        assert_eq!(c.host.specs.lock().unwrap().len(), sent, "fresh agent is not re-sent");
    }

    #[tokio::test]
    async fn binding_rebinding_unbinding_and_releasing_resync_both_agents() {
        let c = ctx().await;
        let db = &c.state.db;
        let p = sandbox(&c, "dev_bind").await;
        let (a, b) = (agent(&c, &p).await, agent(&c, &p).await);
        let (n, _) = number(&c, &p, None).await;
        let patch = |agent_id: Value| {
            let (app, key, n) = (c.app.clone(), p.key.clone(), n.clone());
            async move {
                let (s, body) = req(&app, "PATCH", &format!("/v1/numbers/{n}"), &key, Some(json!({ "agent_id": agent_id }))).await;
                assert_eq!(s, StatusCode::OK, "{body}");
            }
        };

        mark_synced(db, &[&a, &b]).await;
        patch(json!(a)).await;
        assert_eq!((synced(db, &a).await, synced(db, &b).await), (false, true), "bind: the new agent resyncs");

        mark_synced(db, &[&a, &b]).await;
        patch(json!(b)).await;
        assert_eq!((synced(db, &a).await, synced(db, &b).await), (false, false), "rebind: both resync");

        mark_synced(db, &[&a, &b]).await;
        patch(Value::Null).await;
        assert_eq!((synced(db, &a).await, synced(db, &b).await), (true, false), "unbind: the old agent resyncs");

        patch(json!(a)).await;
        mark_synced(db, &[&a, &b]).await;
        let (s, _) = req(&c.app, "DELETE", &format!("/v1/numbers/{n}"), &p.key, None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!((synced(db, &a).await, synced(db, &b).await), (false, true), "release: the bound agent resyncs");
    }

    #[tokio::test]
    async fn an_agent_sends_only_from_its_own_numbers() {
        let c = ctx().await;
        let p = sandbox(&c, "dev_own").await;
        let other = sandbox(&c, "dev_other").await;
        let (a, b) = (agent(&c, &p).await, agent(&c, &p).await);
        let (n_a, _) = number(&c, &p, Some(&a)).await;
        let (n_b, _) = number(&c, &p, Some(&b)).await;
        let other_agent = agent(&c, &other).await;
        let (n_other, _) = number(&c, &other, Some(&other_agent)).await;
        let to = "+14155550123";
        consent(&c, &p, &n_a, to).await;
        consent(&c, &p, &n_b, to).await;

        for foreign in [&n_b, &n_other] {
            let (s, r) = tool(&c, &p, &a, "send_text", json!({ "to": to, "text": "Hi", "number_id": foreign })).await;
            assert_eq!((s, r["ok"].as_bool(), r["error"].as_str()), (StatusCode::FORBIDDEN, Some(false), Some("number_not_bound")), "{r}");
            let (s, r) = tool(&c, &p, &a, "call", json!({ "to": to, "purpose": "Confirm", "number_id": foreign })).await;
            assert_eq!((s, r["error"].as_str()), (StatusCode::FORBIDDEN, Some("number_not_bound")), "{r}");
        }
        let sent: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM platform_messages").fetch_one(&c.state.db).await.unwrap();
        assert_eq!(sent, 0);

        // A second number on A: it has to say which.
        number(&c, &p, Some(&a)).await;
        let (s, r) = tool(&c, &p, &a, "send_text", json!({ "to": to, "text": "Hi" })).await;
        assert_eq!((s, r["error"].as_str()), (StatusCode::BAD_REQUEST, Some("number_required")), "{r}");
    }

    #[tokio::test]
    async fn a_simulated_text_is_recorded_and_not_metered() {
        let c = ctx().await;
        let db = &c.state.db;
        let p = sandbox(&c, "dev_sim").await;
        let a = agent(&c, &p).await;
        let (n, e164) = number(&c, &p, Some(&a)).await;
        let to = "+14155550123";
        consent(&c, &p, &n, to).await;

        let (s, r) = tool(&c, &p, &a, "send_text", json!({ "to": to, "text": "Your cleaning is tomorrow at 10." })).await;
        assert_eq!(s, StatusCode::OK, "{r}");
        assert_eq!((r["ok"].as_bool(), r["status"].as_str(), r["segments"].as_i64()), (Some(true), Some("simulated"), Some(1)), "{r}");
        let row: (String, String, String, String, String) =
            sqlx::query_as("SELECT number_id, from_e164, to_e164, body, status FROM platform_messages WHERE id = $1 AND project_id = $2")
                .bind(r["message_id"].as_str().unwrap())
                .bind(&p.id)
                .fetch_one(db)
                .await
                .unwrap();
        assert_eq!(row, (n, e164, to.to_string(), "Your cleaning is tomorrow at 10.".to_string(), "simulated".to_string()));
        let metered: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM platform_usage_events WHERE project_id = $1").bind(&p.id).fetch_one(db).await.unwrap();
        assert_eq!(metered, 0, "a simulated text costs nothing");
    }

    async fn approval(db: &PgPool, p: &Proj, id: &str, source_ref: &str, action: &str, agent: &str, status: &str) {
        sqlx::query(
            "INSERT INTO platform_approvals (id, project_id, source, source_ref, account_id, kind, proposed_action, status) VALUES ($1, $2, 'twin', $3, $4, $5, $6, $7)",
        )
        .bind(id)
        .bind(&p.id)
        .bind(source_ref)
        .bind(&p.acct)
        .bind(action)
        .bind(json!({ "action": action, "agent_id": agent }))
        .bind(status)
        .execute(db)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn an_approved_text_sends_once_and_the_approval_is_consumed() {
        let c = ctx().await;
        let db = &c.state.db;
        let p = sandbox(&c, "dev_appr").await;
        let a = agent(&c, &p).await;
        let (n, _) = number(&c, &p, Some(&a)).await;
        let to = "+14155550123";
        consent(&c, &p, &n, to).await;
        approval(db, &p, "apr_1", "gap_1", &format!("text {to}"), &a, "pending").await;
        let messages = || async { sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM platform_messages WHERE project_id = $1").bind(&p.id).fetch_one(db).await.unwrap() };
        let status = || async { sqlx::query_scalar::<_, String>("SELECT status FROM platform_approvals WHERE id = 'apr_1'").fetch_one(db).await.unwrap() };
        let input = json!({ "to": to, "text": "See you at 10.", "approval_id": "gap_1" });

        let (s, r) = tool(&c, &p, &a, "send_text", input.clone()).await;
        assert_eq!((s, r["error"].as_str()), (StatusCode::CONFLICT, Some("approval_not_approved")), "{r}");
        assert_eq!(messages().await, 0, "nothing sent while pending");

        sqlx::query("UPDATE platform_approvals SET status = 'approved' WHERE id = 'apr_1'").execute(db).await.unwrap();
        let (s, r) = tool(&c, &p, &a, "send_text", input.clone()).await;
        assert_eq!((s, r["ok"].as_bool()), (StatusCode::OK, Some(true)), "{r}");
        assert_eq!(status().await, "consumed");
        let (s, r) = tool(&c, &p, &a, "send_text", input).await;
        assert_eq!((s, r["error"].as_str()), (StatusCode::CONFLICT, Some("approval_not_approved")), "{r}");
        assert_eq!(messages().await, 1, "one approval, one text");

        // An approval for someone else can't be spent on this recipient (public id works too).
        approval(db, &p, "apr_2", "gap_2", "text +14155550999", &a, "approved").await;
        let (s, r) = tool(&c, &p, &a, "send_text", json!({ "to": to, "text": "Hi", "approval_id": "apr_2" })).await;
        assert_eq!((s, r["error"].as_str()), (StatusCode::FORBIDDEN, Some("approval_mismatch")), "{r}");
        assert_eq!(messages().await, 1);
    }

    #[tokio::test]
    async fn an_agent_call_needs_consent_and_a_sandbox_call_gives_its_slot_back() {
        let c = ctx().await;
        let p = sandbox(&c, "dev_call").await;
        let a = agent(&c, &p).await;
        let (n, _) = number(&c, &p, Some(&a)).await;
        let to = "+14155550123";
        let input = json!({ "to": to, "purpose": "Confirm tomorrow's cleaning" });

        let (s, r) = tool(&c, &p, &a, "call", input.clone()).await;
        assert_eq!((s, r["ok"].as_bool(), r["error"].as_str()), (StatusCode::FORBIDDEN, Some(false), Some("no_consent")), "{r}");

        consent(&c, &p, &n, to).await;
        // Sandbox projects have one call slot: the second call only goes through
        // because the first (simulated, nobody plays it) ended and freed it.
        for _ in 0..2 {
            let (s, r) = tool(&c, &p, &a, "call", input.clone()).await;
            assert_eq!((s, r["ok"].as_bool(), r["status"].as_str()), (StatusCode::OK, Some(true), Some("completed")), "{r}");
            let (s, got) = req(&c.app, "GET", &format!("/v1/calls/{}", r["call_id"].as_str().unwrap()), &p.key, None).await;
            assert_eq!((s, got["status"].as_str(), got["end_reason"].as_str()), (StatusCode::OK, Some("completed"), Some("simulated")), "{got}");
        }
    }
}
