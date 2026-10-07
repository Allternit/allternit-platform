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
//! Tools: `knowledge_search {query, limit?}`, `channel_post {text, channel?, target?}`.
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

use crate::routes::runtime_pairing::{device_token_from_headers, runtime_device_for_token};
use crate::ApiState;

pub fn runtime_routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/runtime-devices/me/platform-agents/:agent_id/tools/:tool", post(tool_h))
}

/// Tools served here.
pub const CLOUD_TOOLS: [&str; 2] = ["knowledge_search", "channel_post"];

pub const MAX_POST_CHARS: usize = 4000;
const MAX_QUERY_CHARS: usize = 500;

fn refuse(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "ok": false, "error": code, "message": message }))).into_response()
}

#[derive(Debug, Deserialize, Default)]
struct ToolBody {
    #[serde(default)]
    input: Value,
}

struct AgentRef {
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
    Ok(AgentRef { project_id: project_id.to_string(), account_id, name, tools })
}

async fn tool_h(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
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
        _ => channel_post(&state, &agent, &input, &ProdChannels).await,
    }
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
        let agent = |acct: &str| AgentRef { project_id: "proj_c".into(), account_id: format!("proj_c_{acct}"), name: "Ada".into(), tools: vec!["channel_post".into()] };
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
