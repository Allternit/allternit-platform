//! Platform API P2 tests: hosted agents. Real Postgres (schema-per-test); the
//! migrations P2 builds on are applied for real.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

use super::{
    hosting::{self, AgentHost, HostRuntime},
    projects::{self, Principal},
    router_gated, Gate, PlatformError, ProjectEnv,
};
use crate::routes::voice_calls_cloud::RelayStream;
use crate::{
    routes::test_support::{test_state, MockGateway},
    services::api_keys::{self, CreateProjectKeyInput},
    ApiState,
};

struct Ctx {
    state: Arc<ApiState>,
    app: Router,
}

async fn ctx() -> Ctx {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    for sql in [
        include_str!("../../../migrations_pg/003_api_keys.sql"),
        include_str!("../../../migrations_pg/050_platform_api_foundation.sql"),
        include_str!("../../../migrations_pg/063_platform_agents.sql"),
        include_str!("../../../migrations_pg/064_platform_conversations.sql"),
    ] {
        sqlx::raw_sql(&sql.replace("public.", "")).execute(&state.db).await.expect("migration applies");
    }
    let app = router_gated(&state, Gate::Forced(true)).with_state(state.clone());
    Ctx { state, app }
}

/// A runtime that answers every turn with "echo: <text>" (or fails, or is still starting).
#[derive(Default)]
struct FakeHost {
    starting: AtomicBool,
    fail_turn: AtomicBool,
    runtime: Mutex<String>,
    calls: Mutex<Vec<(String, String)>>,
}

#[async_trait::async_trait]
impl AgentHost for FakeHost {
    async fn runtime(&self, project_id: &str) -> Result<HostRuntime, PlatformError> {
        if self.starting.load(Ordering::SeqCst) {
            return Err(hosting::starting());
        }
        let id = self.runtime.lock().unwrap().clone();
        Ok(HostRuntime { owner: hosting::runtime_owner(project_id), runtime_id: if id.is_empty() { "rt_1".into() } else { id } })
    }
    async fn call(&self, _rt: &HostRuntime, method: &str, path: &str, _body: &Value) -> Result<(u16, Value), PlatformError> {
        self.calls.lock().unwrap().push((method.to_string(), path.to_string()));
        Ok((200, json!({ "sessionId": format!("ses_{}", self.calls.lock().unwrap().len()) })))
    }
    async fn stream(&self, _rt: &HostRuntime, path: &str, body: &Value) -> Result<RelayStream, PlatformError> {
        self.calls.lock().unwrap().push(("STREAM".into(), path.to_string()));
        let text = body["text"].as_str().unwrap_or("").to_string();
        let sse = if self.fail_turn.load(Ordering::SeqCst) {
            "data: {\"type\":\"error\",\"message\":\"model unavailable\"}\n\n".to_string()
        } else {
            format!("data: {{\"type\":\"text.delta\",\"text\":\"echo: \"}}\n\ndata: {{\"type\":\"text.delta\",\"text\":\"{text}\"}}\n\ndata: {{\"type\":\"done\",\"text\":\"echo: {text}\"}}\n\n")
        };
        // Split mid-event, the way relay chunks arrive.
        let (a, b) = sse.split_at(sse.len() / 2);
        let chunks = vec![Ok(bytes::Bytes::from(a.to_string())), Ok(bytes::Bytes::from(b.to_string()))];
        Ok(Box::pin(futures::stream::iter(chunks)))
    }
}

async fn ctx_with_host() -> (Ctx, Arc<FakeHost>) {
    let c = ctx().await;
    let host = Arc::new(FakeHost::default());
    let dyn_host: Arc<dyn AgentHost> = host.clone();
    let app = router_gated(&c.state, Gate::Forced(true)).layer(axum::Extension(dyn_host)).with_state(c.state.clone());
    (Ctx { state: c.state, app }, host)
}

async fn call_raw(app: &Router, method: &str, path: &str, token: &str, body: Value) -> (StatusCode, String) {
    let req = Request::builder().method(method).uri(path).header("authorization", format!("Bearer {token}")).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn project(c: &Ctx, owner: &str, env: ProjectEnv) -> projects::Project {
    projects::create_project(&c.state.db, &Principal { user_id: owner.into(), org_id: None, org_admin: false }, "P2 test", env).await.unwrap()
}

async fn mint(c: &Ctx, p: &projects::Project, account: Option<&str>, scopes: &[&str]) -> String {
    api_keys::create_project_key(
        &c.state.db,
        CreateProjectKeyInput {
            user_id: p.owner_user_id.clone(),
            organization_id: None,
            project_id: p.id.clone(),
            account_id: account.map(str::to_string),
            env: ProjectEnv::parse(&p.env).unwrap(),
            name: "k".into(),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
        },
    )
    .await
    .unwrap()
    .token
}

async fn call(app: &Router, method: &str, path: &str, token: &str, body: Option<Value>) -> (StatusCode, Value) {
    let b = Request::builder().method(method).uri(path).header("authorization", format!("Bearer {token}"));
    let req = match body {
        Some(v) => b.header("content-type", "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn account(c: &Ctx, token: &str, name: &str) -> String {
    let (s, b) = call(&c.app, "POST", "/v1/accounts", token, Some(json!({ "name": name }))).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    b["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn agents_crud_defaults_and_rules() {
    let c = ctx().await;
    let p = project(&c, "dev_1", ProjectEnv::Sandbox).await;
    let key = mint(&c, &p, None, &["agents"]).await;
    let acct = account(&c, &key, "Lakeside Dental").await;

    let (s, a) = call(&c.app, "POST", "/v1/agents", &key, Some(json!({ "account_id": acct, "name": "Front desk", "instructions": "Book cleanings." }))).await;
    assert_eq!(s, StatusCode::CREATED, "{a}");
    assert!(a["id"].as_str().unwrap().starts_with("agent_"));
    assert_eq!((a["object"].as_str(), a["model"].as_str(), a["voice"].as_str(), a["autonomy"].as_str(), a["status"].as_str()), (Some("agent"), Some("allternit"), Some("af_heart"), Some("ask"), Some("pending")));
    assert_eq!(a["greeting"], "Hi, this is Front desk, an AI assistant. How can I help?");
    let id = a["id"].as_str().unwrap().to_string();

    // AI disclosure, stock voices, the launch tools, E.164 targets.
    for (body, code) in [
        (json!({ "greeting": "Thanks for calling Lakeside!" }), "greeting_missing_ai_disclosure"),
        (json!({ "voice": "eojs_voice" }), "invalid_voice"),
        (json!({ "tools": ["shell"] }), "invalid_tool"),
        (json!({ "transfer_targets": ["555-0100"] }), "invalid_transfer_target"),
        (json!({ "autonomy": "always" }), "invalid_autonomy"),
        (json!({ "model": "gpt" }), "invalid_model"),
    ] {
        let (s, e) = call(&c.app, "PATCH", &format!("/v1/agents/{id}"), &key, Some(body.clone())).await;
        assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some(code)), "{body}");
    }
    let (s, e) = call(&c.app, "PATCH", &format!("/v1/agents/{id}"), &key, Some(json!({ "account_id": "acct_x" }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "unknown fields, including account_id, are refused: {e}");

    let (s, u) = call(&c.app, "PATCH", &format!("/v1/agents/{id}"), &key, Some(json!({
        "greeting": "Hello, Lakeside's AI assistant here.", "voice": "bm_george", "tools": ["send_text", "call", "send_text"],
        "transfer_targets": ["+16515550100"], "autonomy": "tell", "model": "anthropic/claude-sonnet-5-5", "business_hours": { "tz": "America/Chicago" },
    }))).await;
    assert_eq!(s, StatusCode::OK, "{u}");
    assert_eq!(u["tools"], json!(["send_text", "call"]));
    assert_eq!(u["name"], "Front desk", "absent fields are unchanged");
    let (_, u) = call(&c.app, "PATCH", &format!("/v1/agents/{id}"), &key, Some(json!({ "business_hours": null }))).await;
    assert!(u["business_hours"].is_null());

    let (s, got) = call(&c.app, "GET", &format!("/v1/agents/{id}"), &key, None).await;
    assert_eq!((s, got["voice"].as_str()), (StatusCode::OK, Some("bm_george")));
    let (_, list) = call(&c.app, "GET", "/v1/agents", &key, None).await;
    assert_eq!(list["data"].as_array().unwrap().len(), 1);

    let (s, d) = call(&c.app, "DELETE", &format!("/v1/agents/{id}"), &key, None).await;
    assert_eq!((s, d["deleted"].as_bool()), (StatusCode::OK, Some(true)));
    assert_eq!(call(&c.app, "GET", &format!("/v1/agents/{id}"), &key, None).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn agents_need_the_agents_scope_and_an_account() {
    let c = ctx().await;
    let p = project(&c, "dev_2", ProjectEnv::Sandbox).await;
    let admin = mint(&c, &p, None, &["agents", "numbers"]).await;
    let numbers_only = mint(&c, &p, None, &["numbers"]).await;
    let (s, e) = call(&c.app, "POST", "/v1/agents", &numbers_only, Some(json!({ "name": "x" }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("insufficient_scope")));
    let (s, e) = call(&c.app, "POST", "/v1/agents", &admin, Some(json!({ "name": "x" }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("missing_account_id")));
    let (s, e) = call(&c.app, "POST", "/v1/agents", &admin, Some(json!({ "name": "x", "account_id": "acct_nope" }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::NOT_FOUND, Some("account_not_found")));
}

#[tokio::test]
async fn agents_are_isolated_by_account_and_project() {
    let c = ctx().await;
    let p = project(&c, "dev_3", ProjectEnv::Sandbox).await;
    let other = project(&c, "dev_4", ProjectEnv::Sandbox).await;
    let admin = mint(&c, &p, None, &["agents"]).await;
    let admin_other = mint(&c, &other, None, &["agents"]).await;
    let a = account(&c, &admin, "A").await;
    let b = account(&c, &admin, "B").await;
    let key_a = mint(&c, &p, Some(&a), &["agents"]).await;

    // A bound key creates in its own account without naming it, and can't name another.
    let (s, mine) = call(&c.app, "POST", "/v1/agents", &key_a, Some(json!({ "name": "A bot" }))).await;
    assert_eq!((s, mine["account_id"].as_str()), (StatusCode::CREATED, Some(a.as_str())));
    let (s, _) = call(&c.app, "POST", "/v1/agents", &key_a, Some(json!({ "name": "x", "account_id": b }))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (_, theirs) = call(&c.app, "POST", "/v1/agents", &admin, Some(json!({ "name": "B bot", "account_id": b }))).await;
    let theirs = theirs["id"].as_str().unwrap().to_string();

    // It sees only its account's agents; the other account's agent looks missing.
    let (_, list) = call(&c.app, "GET", "/v1/agents", &key_a, None).await;
    assert_eq!(list["data"].as_array().unwrap().len(), 1);
    for (m, body) in [("GET", None), ("PATCH", Some(json!({ "name": "hijack" }))), ("DELETE", None)] {
        assert_eq!(call(&c.app, m, &format!("/v1/agents/{theirs}"), &key_a, body).await.0, StatusCode::NOT_FOUND, "{m}");
    }
    // Another project never sees them.
    assert_eq!(call(&c.app, "GET", &format!("/v1/agents/{theirs}"), &admin_other, None).await.0, StatusCode::NOT_FOUND);
    let (_, none) = call(&c.app, "GET", "/v1/agents", &admin_other, None).await;
    assert!(none["data"].as_array().unwrap().is_empty());
    // The project admin sees both, and can filter by account.
    let (_, all) = call(&c.app, "GET", "/v1/agents", &admin, None).await;
    assert_eq!(all["data"].as_array().unwrap().len(), 2);
    let (_, only_b) = call(&c.app, "GET", &format!("/v1/agents?account_id={b}"), &admin, None).await;
    assert_eq!(only_b["data"][0]["id"].as_str(), Some(theirs.as_str()));
}

#[tokio::test]
async fn sandbox_projects_hold_three_agents() {
    let c = ctx().await;
    let p = project(&c, "dev_5", ProjectEnv::Sandbox).await;
    let key = mint(&c, &p, None, &["agents"]).await;
    let acct = account(&c, &key, "A").await;
    let mut ids = Vec::new();
    for n in 0..3 {
        let (s, a) = call(&c.app, "POST", "/v1/agents", &key, Some(json!({ "account_id": acct, "name": format!("bot {n}") }))).await;
        assert_eq!(s, StatusCode::CREATED);
        ids.push(a["id"].as_str().unwrap().to_string());
    }
    let (s, e) = call(&c.app, "POST", "/v1/agents", &key, Some(json!({ "account_id": acct, "name": "fourth" }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("agent_limit_reached")));
    call(&c.app, "DELETE", &format!("/v1/agents/{}", ids[0]), &key, None).await;
    assert_eq!(call(&c.app, "POST", "/v1/agents", &key, Some(json!({ "account_id": acct, "name": "fourth" }))).await.0, StatusCode::CREATED, "a deleted agent frees its slot");
}

#[tokio::test]
async fn conversations_sync_the_agent_once_and_reply() {
    let (c, host) = ctx_with_host().await;
    let p = project(&c, "dev_6", ProjectEnv::Sandbox).await;
    let key = mint(&c, &p, None, &["agents"]).await;
    let acct = account(&c, &key, "A").await;
    let (_, agent) = call(&c.app, "POST", "/v1/agents", &key, Some(json!({ "account_id": acct, "name": "Ada" }))).await;
    let agent_id = agent["id"].as_str().unwrap().to_string();

    let (s, conv) = call(&c.app, "POST", &format!("/v1/agents/{agent_id}/conversations"), &key, Some(json!({ "metadata": { "ticket": "481" } }))).await;
    assert_eq!(s, StatusCode::CREATED, "{conv}");
    let conv_id = conv["id"].as_str().unwrap().to_string();
    assert!(conv_id.starts_with("conv_") && conv["messages"].as_array().unwrap().is_empty());
    assert!(host.calls.lock().unwrap().is_empty(), "opening a conversation needs no runtime");

    // While the runtime starts: 503 runtime_starting, nothing stored.
    host.starting.store(true, Ordering::SeqCst);
    let (s, e) = call(&c.app, "POST", &format!("/v1/conversations/{conv_id}/messages"), &key, Some(json!({ "content": "hi" }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::SERVICE_UNAVAILABLE, Some("runtime_starting")));
    host.starting.store(false, Ordering::SeqCst);

    let (s, m) = call(&c.app, "POST", &format!("/v1/conversations/{conv_id}/messages"), &key, Some(json!({ "content": "hello" }))).await;
    assert_eq!(s, StatusCode::OK, "{m}");
    assert_eq!((m["role"].as_str(), m["content"].as_str(), m["status"].as_str(), m["object"].as_str()), (Some("assistant"), Some("echo: hello"), Some("completed"), Some("conversation.message")));
    let (_, again) = call(&c.app, "POST", &format!("/v1/conversations/{conv_id}/messages"), &key, Some(json!({ "content": "twice" }))).await;
    assert_eq!(again["content"], "echo: twice");
    let kinds: Vec<String> = host.calls.lock().unwrap().iter().map(|(m, p)| format!("{m} {p}")).collect();
    assert_eq!(kinds.iter().filter(|k| k.starts_with("PUT /api/v1/platform/agents/")).count(), 1, "the agent is sent once: {kinds:?}");
    assert_eq!(kinds.iter().filter(|k| k.ends_with("/sessions")).count(), 1, "one session per conversation: {kinds:?}");

    // The agent is now ready; an edit is re-sent before the next turn.
    let (_, a) = call(&c.app, "GET", &format!("/v1/agents/{agent_id}"), &key, None).await;
    assert_eq!(a["status"], "ready");
    call(&c.app, "PATCH", &format!("/v1/agents/{agent_id}"), &key, Some(json!({ "instructions": "Be brief." }))).await;
    call(&c.app, "POST", &format!("/v1/conversations/{conv_id}/messages"), &key, Some(json!({ "content": "x" }))).await;
    assert_eq!(host.calls.lock().unwrap().iter().filter(|(m, _)| m == "PUT").count(), 2);

    // A new runtime (the old one was removed) gets the agent and a new session.
    *host.runtime.lock().unwrap() = "rt_2".into();
    call(&c.app, "POST", &format!("/v1/conversations/{conv_id}/messages"), &key, Some(json!({ "content": "y" }))).await;
    assert_eq!(host.calls.lock().unwrap().iter().filter(|(_, p)| p.ends_with("/sessions")).count(), 2);

    let (_, full) = call(&c.app, "GET", &format!("/v1/conversations/{conv_id}"), &key, None).await;
    let roles: Vec<&str> = full["messages"].as_array().unwrap().iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(roles, vec!["user", "assistant", "user", "assistant", "user", "assistant", "user", "assistant"]);
    assert_eq!(full["metadata"]["ticket"], "481");
}

#[tokio::test]
async fn streamed_replies_and_failures() {
    let (c, host) = ctx_with_host().await;
    let p = project(&c, "dev_7", ProjectEnv::Sandbox).await;
    let key = mint(&c, &p, None, &["agents"]).await;
    let acct = account(&c, &key, "A").await;
    let (_, agent) = call(&c.app, "POST", "/v1/agents", &key, Some(json!({ "account_id": acct, "name": "Ada" }))).await;
    let (_, conv) = call(&c.app, "POST", &format!("/v1/agents/{}/conversations", agent["id"].as_str().unwrap()), &key, None).await;
    let conv_id = conv["id"].as_str().unwrap().to_string();

    let (s, body) = call_raw(&c.app, "POST", &format!("/v1/conversations/{conv_id}/messages"), &key, json!({ "content": "hi", "stream": true })).await;
    assert_eq!(s, StatusCode::OK);
    assert!(body.contains("event: message.delta") && body.contains("\"delta\":\"echo: \""), "{body}");
    let completed = body.split("event: message.completed\ndata: ").nth(1).unwrap().lines().next().unwrap();
    let m: Value = serde_json::from_str(completed).unwrap();
    assert_eq!((m["content"].as_str(), m["status"].as_str()), (Some("echo: hi"), Some("completed")));

    host.fail_turn.store(true, Ordering::SeqCst);
    let (_, failed) = call(&c.app, "POST", &format!("/v1/conversations/{conv_id}/messages"), &key, Some(json!({ "content": "again" }))).await;
    assert_eq!((failed["status"].as_str(), failed["error"].as_str()), (Some("failed"), Some("model unavailable")));
    let (_, body) = call_raw(&c.app, "POST", &format!("/v1/conversations/{conv_id}/messages"), &key, json!({ "content": "x", "stream": true })).await;
    assert!(body.contains("event: error") && body.contains("turn_failed"), "{body}");

    let (s, e) = call(&c.app, "POST", &format!("/v1/conversations/{conv_id}/messages"), &key, Some(json!({ "content": "   " }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_content")));
}

#[tokio::test]
async fn conversations_are_isolated_and_deleting_an_agent_drops_its_bot() {
    let (c, host) = ctx_with_host().await;
    let p = project(&c, "dev_8", ProjectEnv::Sandbox).await;
    let admin = mint(&c, &p, None, &["agents"]).await;
    let a = account(&c, &admin, "A").await;
    let b = account(&c, &admin, "B").await;
    let key_b = mint(&c, &p, Some(&b), &["agents"]).await;
    let (_, agent) = call(&c.app, "POST", "/v1/agents", &admin, Some(json!({ "account_id": a, "name": "A bot" }))).await;
    let agent_id = agent["id"].as_str().unwrap().to_string();
    let (_, conv) = call(&c.app, "POST", &format!("/v1/agents/{agent_id}/conversations"), &admin, None).await;
    let conv_id = conv["id"].as_str().unwrap().to_string();

    assert_eq!(call(&c.app, "POST", &format!("/v1/agents/{agent_id}/conversations"), &key_b, None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&c.app, "GET", &format!("/v1/conversations/{conv_id}"), &key_b, None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&c.app, "POST", &format!("/v1/conversations/{conv_id}/messages"), &key_b, Some(json!({ "content": "hi" }))).await.0, StatusCode::NOT_FOUND);

    call(&c.app, "POST", &format!("/v1/conversations/{conv_id}/messages"), &admin, Some(json!({ "content": "hi" }))).await;
    assert_eq!(call(&c.app, "DELETE", &format!("/v1/agents/{agent_id}"), &admin, None).await.0, StatusCode::OK);
    assert!(host.calls.lock().unwrap().iter().any(|(m, p)| m == "DELETE" && p.ends_with(&agent_id)), "the runtime drops the bot");
    assert_eq!(call(&c.app, "GET", &format!("/v1/conversations/{conv_id}"), &admin, None).await.0, StatusCode::NOT_FOUND, "a deleted agent's conversations are gone");
}

#[tokio::test]
async fn model_keys_are_stored_encrypted_and_masked() {
    let c = ctx().await;
    let p = project(&c, "dev_9", ProjectEnv::Sandbox).await;
    let cipher = allternit_cloud_core::CredentialCipher::new("test cipher key material");
    let k = super::model_keys::store_key(&c.state.db, &cipher, &p.id, "anthropic", "sk-ant-test-1234567890").await.unwrap();
    assert_eq!((k.provider.as_str(), k.masked.as_str(), k.object), ("anthropic", "…7890", "model_key"));
    let stored: String = sqlx::query_scalar("SELECT key_encrypted FROM platform_model_keys WHERE project_id = $1").bind(&p.id).fetch_one(&c.state.db).await.unwrap();
    assert!(!stored.contains("sk-ant-test"), "never stored in plaintext");
    assert_eq!(super::model_keys::key_for(&c.state.db, &cipher, &p.id, "anthropic").await.unwrap().as_deref(), Some("sk-ant-test-1234567890"));
    assert!(super::model_keys::store_key(&c.state.db, &cipher, &p.id, "mistral", "x".repeat(20).as_str()).await.is_err());
    assert!(super::model_keys::store_key(&c.state.db, &cipher, &p.id, "openai", "short").await.is_err());

    // Through the API: listed masked; without a cipher configured, PUT is 503.
    let key = mint(&c, &p, None, &["agents"]).await;
    let (_, list) = call(&c.app, "GET", "/v1/model_keys", &key, None).await;
    assert_eq!(list["data"][0]["masked"], "…7890");
    assert!(!list.to_string().contains("sk-ant"));
    let (s, e) = call(&c.app, "PUT", "/v1/model_keys/openai", &key, Some(json!({ "api_key": "sk-openai-123456" }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::SERVICE_UNAVAILABLE, Some("model_keys_unavailable")));
    let (s, _) = call(&c.app, "DELETE", "/v1/model_keys/anthropic", &key, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(call(&c.app, "DELETE", "/v1/model_keys/anthropic", &key, None).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_provider_model_agent_needs_the_projects_key() {
    let (c, host) = ctx_with_host().await;
    let p = project(&c, "dev_10", ProjectEnv::Sandbox).await;
    let key = mint(&c, &p, None, &["agents"]).await;
    let acct = account(&c, &key, "A").await;
    let (s, agent) = call(&c.app, "POST", "/v1/agents", &key, Some(json!({ "account_id": acct, "name": "Ada", "model": "anthropic/claude-sonnet-5-5" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{agent}");
    let (_, conv) = call(&c.app, "POST", &format!("/v1/agents/{}/conversations", agent["id"].as_str().unwrap()), &key, None).await;
    let (s, e) = call(&c.app, "POST", &format!("/v1/conversations/{}/messages", conv["id"].as_str().unwrap()), &key, Some(json!({ "content": "hi" }))).await;
    assert!(s.is_server_error() || s == StatusCode::BAD_REQUEST, "{s} {e}");
    assert!(matches!(e["error"]["code"].as_str(), Some("model_keys_unavailable" | "model_key_missing")), "{e}");
    assert!(host.calls.lock().unwrap().is_empty(), "nothing reaches the runtime without the key");
    let (s, e) = call(&c.app, "POST", "/v1/agents", &key, Some(json!({ "account_id": acct, "name": "x", "tools": ["calendar"] }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("tool_not_available")));
}

#[tokio::test]
async fn beta_owners_use_the_api_while_it_is_off_for_everyone_else() {
    let c = ctx().await;
    let beta = project(&c, "owner_beta", ProjectEnv::Sandbox).await;
    let other = project(&c, "owner_other", ProjectEnv::Sandbox).await;
    let beta_key = mint(&c, &beta, None, &["agents"]).await;
    let other_key = mint(&c, &other, None, &["agents"]).await;
    let app = router_gated(&c.state, Gate::BetaOnly(vec!["owner_beta".into()])).with_state(c.state.clone());
    assert_eq!(call(&app, "GET", "/v1/agents", &beta_key, None).await.0, StatusCode::OK);
    let (s, e) = call(&app, "GET", "/v1/agents", &other_key, None).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::NOT_FOUND, Some("platform_api_disabled")));
    let (s, e) = call(&app, "GET", "/v1/agents", "alt_test_not_a_real_key", None).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::NOT_FOUND, Some("platform_api_disabled")), "a bad key learns nothing more");
    let off = router_gated(&c.state, Gate::BetaOnly(vec![])).with_state(c.state.clone());
    assert_eq!(call(&off, "GET", "/v1/agents", &beta_key, None).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn v1_agents_is_shared_with_the_agency_api() {
    let c = ctx().await;
    let p = project(&c, "dev_11", ProjectEnv::Sandbox).await;
    let key = mint(&c, &p, None, &["agents"]).await;
    let agency = Arc::new(crate::routes::agency_forward::AgencyForward::new("http://127.0.0.1:9", Some("peer".into())));
    // Platform API off: a non-project credential still reaches the Agency API (which
    // authenticates it: 401 here), never `platform_api_disabled`.
    let off = super::router_with_agency(&c.state, Gate::Forced(false), agency.clone()).with_state(c.state.clone());
    let (s, e) = call(&off, "GET", "/v1/agents", "not-a-project-key", None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "{e}");
    assert_ne!(e["error"]["code"], "platform_api_disabled");
    // A project key is the Platform API's.
    let on = super::router_with_agency(&c.state, Gate::Forced(true), agency).with_state(c.state.clone());
    let (s, list) = call(&on, "GET", "/v1/agents", &key, None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(list["data"].is_array(), "{list}");
}
