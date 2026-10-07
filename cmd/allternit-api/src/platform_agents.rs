//! Runtime side of Platform API hosted agents (spec SPEC-allternit-platform-api §5).
//!
//! A developer's agent (`/v1/agents` on cloud-api) runs as a bot on the project's
//! hosted runtime: a cloud computer owned by the synthetic owner
//! `platform:<project_id>`. cloud-api relays these calls here, signed with the
//! runtime's device token ([`crate::relay_auth`]), so the routes are public to the
//! Clerk middleware and authenticate themselves:
//!
//! * `PUT    /api/v1/platform/agents/{agentId}`                       create or update the bot
//! * `DELETE /api/v1/platform/agents/{agentId}`                       remove it
//! * `POST   /api/v1/platform/agents/{agentId}/sessions`              open a conversation session
//! * `POST   /api/v1/platform/agents/{agentId}/sessions/{sid}/turn`   one turn, streamed as SSE
//! * `PUT|DELETE /api/v1/platform/model-keys/{provider}`              a project's own model key
//!
//! **Tools.** Each session gets a gizzi permission ruleset: deny everything, then
//! allow only the agent's tools ([`tool_rules`]) plus the twin's people tools.
//! A session whose rules can't be applied is not handed out (fail closed).
//!
//! **Own model keys.** `anthropic/…`, `openai/…` and `xai/…` agents run on the
//! project's key, set as gizzi provider auth on this runtime (`PUT /auth/{provider}`),
//! after which gizzi's instance is reloaded so the key applies to the next turn.
//!
//! The bot id is the agent id. A bot answers only its signed owner. Turns run
//! through the same turner as phone calls ([`crate::voice_calls::production_turner`]),
//! so instructions, memory, the twin persona and autonomy rules all apply.
//! SSE events: `{"type":"text.delta","text"}` while it writes, then
//! `{"type":"done","text"}` or `{"type":"error","message"}`.

use std::{convert::Infallible, sync::Arc};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{sse::Event, IntoResponse, Response, Sse},
    routing::{post, put},
    Extension, Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::relay_auth::{RelaySecret, RelayedAuth};
use crate::voice_calls::{TurnReply, VoiceTurner};
use crate::{db::DbHandle, AppState};

const MAX_TURN_CHARS: usize = 16_000;

pub struct PlatformDeps {
    pub secret: Arc<dyn RelaySecret>,
    pub turner: Arc<dyn VoiceTurner>,
}

pub fn platform_agents_router() -> Router<Arc<AppState>> {
    platform_agents_router_with(Arc::new(PlatformDeps {
        secret: crate::relay_auth::process_secret(),
        turner: crate::voice_calls::production_turner(),
    }))
}

pub fn platform_agents_router_with(deps: Arc<PlatformDeps>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/platform/agents/:agent_id", put(upsert_h).delete(delete_h))
        .route("/api/v1/platform/agents/:agent_id/sessions", post(session_h))
        .route("/api/v1/platform/agents/:agent_id/sessions/:session_id/turn", post(turn_h))
        .route("/api/v1/platform/model-keys/:provider", put(model_key_put_h).delete(model_key_delete_h))
        .layer(Extension(deps.secret.clone()))
        .layer(Extension(deps))
}

fn fail(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentSpec {
    name: String,
    #[serde(default)]
    instructions: String,
    #[serde(default)]
    greeting: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    voice: String,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    autonomy: String,
    #[serde(default)]
    transfer_targets: Vec<String>,
    #[serde(default)]
    account_id: String,
}

/// `provider/model` → (provider, model); `allternit` or anything else → the runtime default.
pub fn model_ref(model: &str) -> (String, String) {
    match model.split_once('/') {
        Some((p, m)) if !p.is_empty() && !m.is_empty() => (p.to_string(), m.to_string()),
        _ => crate::config::AppConfig::load().default_model(),
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The bot's owner, if it exists.
fn bot_owner(db: &DbHandle, bot_id: &str) -> Result<Option<String>, String> {
    let conn = db.connect().map_err(|e| e.to_string())?;
    conn.query_row("SELECT user_id FROM agents WHERE id = ?1", params![bot_id], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())
}

/// Write the bot row (and its bot-wide autonomy level) from the agent spec.
pub fn upsert_bot(db: &DbHandle, owner: &str, bot_id: &str, spec: &Value) -> Result<(), String> {
    let s: AgentSpec = serde_json::from_value(spec.clone()).map_err(|e| format!("invalid agent: {e}"))?;
    if s.name.trim().is_empty() {
        return Err("name is required".into());
    }
    let (provider, model) = model_ref(&s.model);
    let config = json!({
        "botProfile": { "displayName": s.name.trim() },
        "platformAgent": {
            "greeting": s.greeting, "voice": s.voice, "tools": s.tools, "model": s.model,
            "transferTargets": s.transfer_targets, "accountId": s.account_id,
        },
    });
    let conn = db.connect().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO agents (id, user_id, name, model, provider, system_prompt, is_bot, config, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, CURRENT_TIMESTAMP)
         ON CONFLICT(id) DO UPDATE SET name = excluded.name, model = excluded.model, provider = excluded.provider,
             system_prompt = excluded.system_prompt, config = excluded.config, updated_at = CURRENT_TIMESTAMP
         WHERE agents.user_id = excluded.user_id",
        params![bot_id, owner, s.name.trim(), model, provider, s.instructions, config.to_string()],
    )
    .map_err(|e| e.to_string())?;
    let level = if crate::autonomy::LEVELS.contains(&s.autonomy.as_str()) { s.autonomy.as_str() } else { "ask" };
    let now = crate::agent_gateway_routes::now();
    conn.execute(
        "INSERT INTO autonomy_policies (id, owner, bot_id, channel, person, level, limits_json, created_at, updated_at) VALUES (?1,?2,?3,'','',?4,'{}',?5,?5)
         ON CONFLICT(owner, bot_id, channel, person) DO UPDATE SET level = excluded.level, updated_at = excluded.updated_at",
        params![crate::agent_gateway_routes::id("pol"), owner, bot_id, level, now],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

async fn upsert_h(State(state): State<Arc<AppState>>, Path(agent_id): Path<String>, auth: RelayedAuth) -> Response {
    if !valid_id(&agent_id) {
        return fail(StatusCode::BAD_REQUEST, "invalid agent id");
    }
    match bot_owner(&state.db, &agent_id) {
        Ok(Some(o)) if o != auth.owner => return fail(StatusCode::FORBIDDEN, "agent belongs to another owner"),
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, &e),
        _ => {}
    }
    let spec: Value = match auth.json() {
        Ok(v) => v,
        Err(r) => return r,
    };
    match upsert_bot(&state.db, &auth.owner, &agent_id, &spec) {
        Ok(()) => Json(json!({ "agentId": agent_id, "botId": agent_id })).into_response(),
        Err(e) => fail(StatusCode::BAD_REQUEST, &e),
    }
}

async fn delete_h(State(state): State<Arc<AppState>>, Path(agent_id): Path<String>, auth: RelayedAuth) -> Response {
    match bot_owner(&state.db, &agent_id) {
        Ok(Some(o)) if o == auth.owner => {}
        Ok(Some(_)) => return fail(StatusCode::FORBIDDEN, "agent belongs to another owner"),
        Ok(None) => return Json(json!({ "agentId": agent_id, "deleted": true })).into_response(),
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
    let removed = state.db.connect().and_then(|c| {
        c.execute("DELETE FROM autonomy_policies WHERE owner = ?1 AND bot_id = ?2", params![auth.owner, agent_id])?;
        c.execute("DELETE FROM agents WHERE id = ?1 AND user_id = ?2", params![agent_id, auth.owner])
    });
    match removed {
        Ok(_) => Json(json!({ "agentId": agent_id, "deleted": true })).into_response(),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SessionBody {
    #[serde(default)]
    conversation_id: String,
}

/// The gizzi model reference a session should use for this bot, when it names one.
fn session_model(db: &DbHandle, bot_id: &str) -> Option<Value> {
    let conn = db.connect().ok()?;
    let model: String = conn
        .query_row("SELECT json_extract(config, '$.platformAgent.model') FROM agents WHERE id = ?1", params![bot_id], |r| r.get::<_, Option<String>>(0))
        .ok()
        .flatten()?;
    let (provider, model) = model.split_once('/')?;
    Some(json!({ "providerId": provider, "modelId": model }))
}

/// Model providers a project can bring its own key for.
pub const BYO_PROVIDERS: [&str; 3] = ["anthropic", "openai", "xai"];

/// The gizzi permission ruleset for a hosted agent: everything denied, then the
/// agent's tools allowed (gizzi applies the last matching rule). MCP tools are
/// named `<server>_<tool>`, hence the wildcards.
pub fn tool_rules(tools: &[String]) -> Value {
    let mut rules = vec![json!({ "permission": "*", "pattern": "*", "action": "deny" })];
    let mut allow = |p: &str| rules.push(json!({ "permission": p, "pattern": "*", "action": "allow" }));
    // The twin: knowing who someone is and remembering it is part of every agent.
    allow("*people_lookup*");
    allow("*people_remember*");
    for t in tools {
        match t.as_str() {
            "send_text" => allow("*phone_text*"),
            "call" => allow("*phone_call*"),
            "email" => allow("*allternit_mail*"),
            "web_fetch" => allow("webfetch"),
            "web_search" => allow("websearch"),
            "ask_human" => allow("question"),
            _ => {}
        }
    }
    Value::Array(rules)
}

fn agent_tools(db: &DbHandle, bot_id: &str) -> Vec<String> {
    db.connect()
        .ok()
        .and_then(|c| c.query_row("SELECT json_extract(config, '$.platformAgent.tools') FROM agents WHERE id = ?1", params![bot_id], |r| r.get::<_, Option<String>>(0)).ok().flatten())
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

/// Apply the agent's tool rules to a gizzi session.
async fn restrict_session(session_id: &str, rules: &Value) -> Result<(), String> {
    let client = crate::agent_session_routes::gizzi_client(&axum::http::HeaderMap::new());
    let url = format!("{}/v1/session/{}", crate::agent_session_routes::gizzi_base(), urlencoding::encode(session_id));
    let res = client.patch(url).json(&json!({ "permission": rules })).send().await.map_err(|e| e.to_string())?;
    if res.status().is_success() {
        Ok(())
    } else {
        Err(format!("gizzi refused the session's tool rules ({})", res.status()))
    }
}

/// gizzi answers its health check. Right after a computer starts or wakes it
/// can take half a minute; until then sessions and turns answer 503 (cloud-api
/// turns that into a retryable `runtime_starting`) instead of failing.
async fn gizzi_ready() -> bool {
    let client = crate::agent_session_routes::gizzi_client(&axum::http::HeaderMap::new());
    let url = format!("{}/global/health", crate::agent_session_routes::gizzi_base());
    matches!(client.get(url).timeout(std::time::Duration::from_secs(3)).send().await, Ok(r) if r.status().is_success())
}

fn starting() -> Response {
    fail(StatusCode::SERVICE_UNAVAILABLE, "the agent runtime is starting")
}

async fn session_h(State(state): State<Arc<AppState>>, Path(agent_id): Path<String>, auth: RelayedAuth) -> Response {
    let name = match state.db.connect().and_then(|c| {
        c.query_row("SELECT name, user_id FROM agents WHERE id = ?1", params![agent_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).optional()
    }) {
        Ok(Some((name, owner))) if owner == auth.owner => name,
        Ok(_) => return fail(StatusCode::NOT_FOUND, "agent not found"),
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    if !gizzi_ready().await {
        return starting();
    }
    let body: SessionBody = if auth.body.is_empty() { SessionBody::default() } else { auth.json().unwrap_or_default() };
    let title = if body.conversation_id.is_empty() { "API conversation".to_string() } else { format!("API {}", body.conversation_id) };
    let session_id = match crate::agent_session_routes::create_bot_thread_session(&state.db, &agent_id, &name, &title, false, None).await {
        Ok(id) => id,
        Err(e) => return fail(StatusCode::BAD_GATEWAY, &e),
    };
    let mut bag = state.db.get_session_metadata(&session_id).ok().flatten().unwrap_or_else(|| json!({}));
    bag["platformConversationId"] = json!(body.conversation_id);
    if let Some(model) = session_model(&state.db, &agent_id) {
        bag["projectModel"] = model;
    }
    let _ = state.db.set_session_metadata(&session_id, &bag);
    if let Err(e) = restrict_session(&session_id, &tool_rules(&agent_tools(&state.db, &agent_id))).await {
        tracing::warn!(%session_id, "platform session not handed out: {e}");
        return fail(StatusCode::BAD_GATEWAY, "couldn't limit the conversation to the agent's tools");
    }
    Json(json!({ "sessionId": session_id })).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelKeyBody {
    api_key: String,
}

async fn gizzi_auth(method: reqwest::Method, provider: &str, body: Option<Value>) -> Result<(), String> {
    let client = crate::agent_session_routes::gizzi_client(&axum::http::HeaderMap::new());
    let url = format!("{}/auth/{}", crate::agent_session_routes::gizzi_base(), urlencoding::encode(provider));
    let mut req = client.request(method, url);
    if let Some(b) = body {
        req = req.json(&b);
    }
    let res = req.send().await.map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("gizzi answered {}", res.status()));
    }
    // gizzi reads provider keys once per instance; reload it so the key applies to
    // the next turn. A platform runtime serves one project and keys change rarely.
    let res = client
        .post(format!("{}/instance/dispose", crate::agent_session_routes::gizzi_base()))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if res.status().is_success() { Ok(()) } else { Err(format!("gizzi reload answered {}", res.status())) }
}

/// Only a platform runtime (owner `platform:…`) takes a project's model keys.
fn platform_owner(owner: &str) -> bool {
    owner.starts_with("platform:")
}

async fn model_key_put_h(Path(provider): Path<String>, auth: RelayedAuth) -> Response {
    if !platform_owner(&auth.owner) || !BYO_PROVIDERS.contains(&provider.as_str()) {
        return fail(StatusCode::BAD_REQUEST, "unknown provider");
    }
    let body: ModelKeyBody = match auth.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    if body.api_key.trim().len() < 8 {
        return fail(StatusCode::BAD_REQUEST, "apiKey is required");
    }
    match gizzi_auth(reqwest::Method::PUT, &provider, Some(json!({ "type": "api", "key": body.api_key.trim() }))).await {
        Ok(()) => Json(json!({ "provider": provider, "set": true })).into_response(),
        Err(e) => fail(StatusCode::BAD_GATEWAY, &e),
    }
}

async fn model_key_delete_h(Path(provider): Path<String>, auth: RelayedAuth) -> Response {
    if !platform_owner(&auth.owner) || !BYO_PROVIDERS.contains(&provider.as_str()) {
        return fail(StatusCode::BAD_REQUEST, "unknown provider");
    }
    match gizzi_auth(reqwest::Method::DELETE, &provider, None).await {
        Ok(()) => Json(json!({ "provider": provider, "deleted": true })).into_response(),
        Err(e) => fail(StatusCode::BAD_GATEWAY, &e),
    }
}

#[derive(Deserialize)]
struct TurnBody {
    text: String,
}

/// The session belongs to this bot, which belongs to the signed owner.
fn owned_session(db: &DbHandle, owner: &str, bot_id: &str, session_id: &str) -> bool {
    let owner_ok = bot_owner(db, bot_id).ok().flatten().is_some_and(|o| o == owner);
    let bag = db.get_session_metadata(session_id).ok().flatten();
    owner_ok && bag.is_some_and(|b| b["agentId"].as_str() == Some(bot_id))
}

async fn turn_h(
    State(state): State<Arc<AppState>>,
    Extension(deps): Extension<Arc<PlatformDeps>>,
    Path((agent_id, session_id)): Path<(String, String)>,
    auth: RelayedAuth,
) -> Response {
    if !owned_session(&state.db, &auth.owner, &agent_id, &session_id) {
        return fail(StatusCode::NOT_FOUND, "conversation not found");
    }
    let body: TurnBody = match auth.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    let text = body.text.trim().to_string();
    if text.is_empty() || text.chars().count() > MAX_TURN_CHARS {
        return fail(StatusCode::BAD_REQUEST, "text must be 1 to 16000 characters");
    }
    if !gizzi_ready().await {
        return starting();
    }
    let (out_tx, out_rx) = mpsc::unbounded_channel::<Value>();
    let db = state.db.clone();
    tokio::spawn(async move {
        let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
        let mut written = String::new();
        let result = {
            let run = deps.turner.run(&db, &session_id, &agent_id, &text, tx);
            tokio::pin!(run);
            loop {
                tokio::select! {
                    r = &mut run => break r,
                    Some(ev) = rx.recv() => forward(ev, &mut written, &out_tx),
                }
            }
        };
        while let Ok(ev) = rx.try_recv() {
            forward(ev, &mut written, &out_tx);
        }
        let end = match result {
            Ok(TurnReply::Final(reply)) => {
                let _ = out_tx.send(json!({ "type": "text.delta", "text": reply }));
                json!({ "type": "done", "text": reply })
            }
            Ok(TurnReply::Streamed) => json!({ "type": "done", "text": written }),
            Err(message) => json!({ "type": "error", "message": message }),
        };
        let _ = out_tx.send(end);
    });
    let stream = futures::stream::unfold(out_rx, |mut rx| async move {
        rx.recv().await.map(|v| (Ok::<_, Infallible>(Event::default().data(v.to_string())), rx))
    });
    Sse::new(stream).into_response()
}

/// Pass text deltas on (and keep the full text); tool steps and other events stay on the runtime.
fn forward(ev: Value, written: &mut String, out: &mpsc::UnboundedSender<Value>) {
    if ev["type"] == "text.delta" {
        if let Some(t) = ev["text"].as_str() {
            written.push_str(t);
            let _ = out.send(json!({ "type": "text.delta", "text": t }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay_auth::{signed_headers, StaticRelaySecret};
    use async_trait::async_trait;
    use axum::body::Body;
    use tower::ServiceExt;

    const TOKEN: &str = "device-token-for-tests";
    const OWNER: &str = "platform:proj_1";

    struct EchoTurner;
    #[async_trait]
    impl VoiceTurner for EchoTurner {
        async fn run(&self, _db: &DbHandle, _s: &str, _b: &str, text: &str, events: mpsc::UnboundedSender<Value>) -> Result<TurnReply, String> {
            let _ = events.send(json!({ "type": "text.delta", "text": "You said: " }));
            let _ = events.send(json!({ "type": "tool.step", "name": "x" }));
            let _ = events.send(json!({ "type": "text.delta", "text": text }));
            Ok(TurnReply::Streamed)
        }
        async fn abort(&self, _s: &str) {}
    }

    async fn app() -> (Router, Arc<AppState>) {
        let dir = std::env::temp_dir().join(format!("allternit-platform-agents-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::test_helpers::app_state(&dir).await;
        let deps = Arc::new(PlatformDeps {
            secret: Arc::new(StaticRelaySecret { token: TOKEN.into(), owner: OWNER.into() }),
            turner: Arc::new(EchoTurner),
        });
        (platform_agents_router_with(deps).with_state(state.clone()), state)
    }

    fn signed(method: &str, path: &str, body: Value, owner: &str) -> axum::http::Request<Body> {
        let bytes = serde_json::to_vec(&body).unwrap();
        let mut req = axum::http::Request::builder().method(method).uri(path).header("content-type", "application/json");
        for (k, v) in signed_headers(TOKEN, owner, method, path, &bytes) {
            req = req.header(k, v);
        }
        req.body(Body::from(bytes)).unwrap()
    }

    #[test]
    fn tool_rules_deny_everything_then_allow_the_agents_tools() {
        let rules = tool_rules(&["send_text".into(), "web_search".into(), "calendar".into()]);
        let perms: Vec<(&str, &str)> = rules.as_array().unwrap().iter().map(|r| (r["permission"].as_str().unwrap(), r["action"].as_str().unwrap())).collect();
        assert_eq!(perms[0], ("*", "deny"), "deny first; the last matching rule wins in gizzi");
        assert!(perms.contains(&("*phone_text*", "allow")) && perms.contains(&("websearch", "allow")));
        assert!(!perms.iter().any(|(p, _)| *p == "*phone_call*" || *p == "bash" || *p == "webfetch"));
        assert!(perms.contains(&("*people_lookup*", "allow")));
    }

    #[test]
    fn model_names_map_to_provider_and_model() {
        assert_eq!(model_ref("anthropic/claude-sonnet-5-5"), ("anthropic".into(), "claude-sonnet-5-5".into()));
        assert_eq!(model_ref("allternit"), crate::config::AppConfig::load().default_model());
    }

    #[tokio::test]
    async fn upsert_writes_the_bot_and_its_autonomy_and_refuses_unsigned() {
        let (app, state) = app().await;
        let spec = json!({ "name": "Front desk", "instructions": "Book cleanings.", "model": "anthropic/claude-sonnet-5-5", "autonomy": "tell", "tools": ["call"] });
        let r = app.clone().oneshot(signed("PUT", "/api/v1/platform/agents/agent_1", spec.clone(), OWNER)).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let conn = state.db.connect().unwrap();
        let (owner, prompt, provider, is_bot): (String, String, String, i64) = conn
            .query_row("SELECT user_id, system_prompt, provider, is_bot FROM agents WHERE id = 'agent_1'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap();
        assert_eq!((owner.as_str(), prompt.as_str(), provider.as_str(), is_bot), (OWNER, "Book cleanings.", "anthropic", 1));
        let level: String = conn.query_row("SELECT level FROM autonomy_policies WHERE owner = ?1 AND bot_id = 'agent_1'", params![OWNER], |r| r.get(0)).unwrap();
        assert_eq!(level, "tell");

        // Unsigned is refused; another owner's signature can't take the bot over.
        let unsigned = axum::http::Request::builder().method("PUT").uri("/api/v1/platform/agents/agent_1").body(Body::from(spec.to_string())).unwrap();
        assert_eq!(app.clone().oneshot(unsigned).await.unwrap().status(), StatusCode::UNAUTHORIZED);

        let r = app.clone().oneshot(signed("DELETE", "/api/v1/platform/agents/agent_1", json!({}), OWNER)).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let left: i64 = state.db.connect().unwrap().query_row("SELECT count(*) FROM agents WHERE id = 'agent_1'", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0);
    }

    #[tokio::test]
    async fn a_turn_streams_text_and_ends_with_done() {
        let (app, state) = app().await;
        upsert_bot(&state.db, OWNER, "agent_2", &json!({ "name": "Ada" })).unwrap();
        state.db.set_session_metadata("ses_test", &json!({ "agentId": "agent_2" })).unwrap();
        let r = app.clone().oneshot(signed("POST", "/api/v1/platform/agents/agent_2/sessions/ses_test/turn", json!({ "text": "hello" }), OWNER)).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let body = String::from_utf8(axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap().to_vec()).unwrap();
        let events: Vec<Value> = body.lines().filter_map(|l| l.strip_prefix("data: ")).map(|d| serde_json::from_str(d).unwrap()).collect();
        assert_eq!(events.last().unwrap(), &json!({ "type": "done", "text": "You said: hello" }));
        assert!(events.iter().all(|e| e["type"] != "tool.step"), "tool steps stay on the runtime");

        // A session of another bot, or an empty turn, is refused.
        let r = app.clone().oneshot(signed("POST", "/api/v1/platform/agents/agent_x/sessions/ses_test/turn", json!({ "text": "hi" }), OWNER)).await.unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        let r = app.clone().oneshot(signed("POST", "/api/v1/platform/agents/agent_2/sessions/ses_test/turn", json!({ "text": "  " }), OWNER)).await.unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    }
}
