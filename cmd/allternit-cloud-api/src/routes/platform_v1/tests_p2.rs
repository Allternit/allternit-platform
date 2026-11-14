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
    ctx_from(test_state(Arc::new(MockGateway::new(None, vec![]))).await).await
}

async fn ctx_from(state: Arc<ApiState>) -> Ctx {
    crate::routes::test_support::platform_schema(&state.db).await;
    items_schema(&state.db).await;
    let app = router_gated(&state, Gate::Forced(true)).with_state(state.clone());
    Ctx { state, app }
}

/// Migration 121 (conversation items), which conversations now read and write.
/// Idempotent, so it is harmless once `test_support::platform_schema` lists it too.
pub(super) async fn items_schema(db: &sqlx::PgPool) {
    sqlx::raw_sql(&include_str!("../../../migrations_pg/121_platform_items.sql").replace("public.", "")).execute(db).await.expect("migration 121 applies");
}

/// A runtime that answers every turn with "echo: <text>" (or fails, or is still starting).
#[derive(Default)]
struct FakeHost {
    starting: AtomicBool,
    fail_turn: AtomicBool,
    /// The computer is up but its agent runtime isn't: turns are refused as starting.
    refuse_turn: AtomicBool,
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
        if self.refuse_turn.load(Ordering::SeqCst) {
            return Err(hosting::starting());
        }
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
    host_on(ctx().await)
}

/// Like [`ctx_with_host`], with a credential cipher so model keys can be stored.
async fn ctx_with_host_and_cipher() -> (Ctx, Arc<FakeHost>) {
    let mut state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    Arc::get_mut(&mut state).expect("fresh state").credential_cipher = Some(Arc::new(allternit_cloud_core::CredentialCipher::new("test cipher key material")));
    host_on(ctx_from(state).await)
}

fn host_on(c: Ctx) -> (Ctx, Arc<FakeHost>) {
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
    let p = projects::create_project(&c.state.db, &Principal { user_id: owner.into(), org_id: None, org_admin: false }, "P2 test", env).await.unwrap();
    super::project_billing::put_test_card(&c.state.db, &p.id).await;
    p
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
async fn a_turn_refused_while_the_runtime_starts_stores_nothing() {
    let (c, host) = ctx_with_host().await;
    let p = project(&c, "dev_12", ProjectEnv::Sandbox).await;
    let key = mint(&c, &p, None, &["agents"]).await;
    let acct = account(&c, &key, "A").await;
    let (_, agent) = call(&c.app, "POST", "/v1/agents", &key, Some(json!({ "account_id": acct, "name": "Ada" }))).await;
    let (_, conv) = call(&c.app, "POST", &format!("/v1/agents/{}/conversations", agent["id"].as_str().unwrap()), &key, None).await;
    let conv_id = conv["id"].as_str().unwrap().to_string();

    host.refuse_turn.store(true, Ordering::SeqCst);
    let (s, e) = call(&c.app, "POST", &format!("/v1/conversations/{conv_id}/messages"), &key, Some(json!({ "content": "hi" }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::SERVICE_UNAVAILABLE, Some("runtime_starting")));
    let (_, got) = call(&c.app, "GET", &format!("/v1/conversations/{conv_id}"), &key, None).await;
    assert!(got["messages"].as_array().unwrap().is_empty(), "nothing stored: {got}");

    host.refuse_turn.store(false, Ordering::SeqCst);
    let (_, m) = call(&c.app, "POST", &format!("/v1/conversations/{conv_id}/messages"), &key, Some(json!({ "content": "hi" }))).await;
    assert_eq!(m["content"], "echo: hi");
    let (_, got) = call(&c.app, "GET", &format!("/v1/conversations/{conv_id}"), &key, None).await;
    let roles: Vec<&str> = got["messages"].as_array().unwrap().iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["user", "assistant"]);
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
async fn a_set_or_rotated_model_key_reaches_the_runtime_before_the_next_turn() {
    let (c, host) = ctx_with_host_and_cipher().await;
    let p = project(&c, "dev_11", ProjectEnv::Sandbox).await;
    let key = mint(&c, &p, None, &["agents"]).await;
    let acct = account(&c, &key, "A").await;
    let (_, agent) = call(&c.app, "POST", "/v1/agents", &key, Some(json!({ "account_id": acct, "name": "Ada", "model": "openai/gpt-5-mini" }))).await;
    let (_, conv) = call(&c.app, "POST", &format!("/v1/agents/{}/conversations", agent["id"].as_str().unwrap()), &key, None).await;
    let send = |text: &'static str| {
        let (app, key, path) = (c.app.clone(), key.clone(), format!("/v1/conversations/{}/messages", conv["id"].as_str().unwrap()));
        async move { call(&app, "POST", &path, &key, Some(json!({ "content": text }))).await }
    };
    let key_pushes = || host.calls.lock().unwrap().iter().filter(|(m, p)| m == "PUT" && p == "/api/v1/platform/model-keys/openai").count();

    let (s, _) = call(&c.app, "PUT", "/v1/model_keys/openai", &key, Some(json!({ "api_key": "sk-openai-first-123456" }))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(send("one").await.1["content"], "echo: one");
    assert_eq!(key_pushes(), 1, "the key goes with the first sync");
    send("two").await;
    assert_eq!(key_pushes(), 1, "not re-sent while nothing changed");

    call(&c.app, "PUT", "/v1/model_keys/openai", &key, Some(json!({ "api_key": "sk-openai-second-123456" }))).await;
    send("three").await;
    assert_eq!(key_pushes(), 2, "a rotated key reaches the runtime before the next turn");
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
    // `/v1/decisions` is shared the same way (Desktop and computer use call it with
    // Clerk or `allternit_*` tokens).
    let (s, e) = call(&off, "POST", "/v1/decisions", "not-a-project-key", Some(json!({}))).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "{e}");
    assert_ne!(e["error"]["code"], "platform_api_disabled");
    // A project key is the Platform API's.
    let on = super::router_with_agency(&c.state, Gate::Forced(true), agency).with_state(c.state.clone());
    let (s, list) = call(&on, "GET", "/v1/agents", &key, None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(list["data"].is_array(), "{list}");
}

#[tokio::test]
async fn conversations_v2_hold_items_and_page_them() {
    let (c, _host) = ctx_with_host().await;
    let p = project(&c, "dev_v2", ProjectEnv::Sandbox).await;
    let key = mint(&c, &p, None, &["agents"]).await;
    let acct_a = account(&c, &key, "A").await;
    let acct_b = account(&c, &key, "B").await;

    // An agent-less conversation with starting items; bad items name their field.
    let (s, conv) = call(&c.app, "POST", "/v1/conversations", &key, Some(json!({ "account_id": acct_a, "metadata": { "k": "1" }, "items": [{ "role": "system", "content": "Be brief." }, { "role": "user", "content": "m0" }] }))).await;
    assert_eq!(s, StatusCode::CREATED, "{conv}");
    assert_eq!((conv["agent_id"].clone(), conv["account_id"].as_str(), conv["item_count"].as_i64(), conv["object"].as_str()), (Value::Null, Some(acct_a.as_str()), Some(2), Some("conversation")));
    let conv_id = conv["id"].as_str().unwrap().to_string();
    let (s, e) = call(&c.app, "POST", "/v1/conversations", &key, Some(json!({ "items": [{ "role": "robot", "content": "x" }] }))).await;
    assert_eq!((s, e["error"]["param"].as_str()), (StatusCode::BAD_REQUEST, Some("items[0].role")));
    let (s, e) = call(&c.app, "POST", &format!("/v1/conversations/{conv_id}/items"), &key, Some(json!({ "items": vec![json!({ "role": "user", "content": "x" }); 21] }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_items")));

    // Twenty through the route, the rest straight into the store: 250 items.
    let batch: Vec<Value> = (1..=20).map(|i| json!({ "role": "user", "content": format!("m{i}") })).collect();
    let (s, added) = call(&c.app, "POST", &format!("/v1/conversations/{conv_id}/items"), &key, Some(json!({ "items": batch }))).await;
    assert_eq!((s, added["data"].as_array().map(Vec::len), added["has_more"].as_bool()), (StatusCode::OK, Some(20), Some(false)), "{added}");
    let rest: Vec<super::items::NewItem> = (21..249).map(|i| super::items::NewItem::user_text(&format!("m{i}"))).collect();
    super::items::append(&c.state.db, &p.id, Some(&conv_id), None, rest).await.unwrap();

    async fn walk(c: &Ctx, key: &str, conv: &str, order: &str) -> Vec<Value> {
        let (mut out, mut after, mut pages) = (Vec::new(), String::new(), 0);
        loop {
            let q = if after.is_empty() { String::new() } else { format!("&after={after}") };
            let (s, page) = call(&c.app, "GET", &format!("/v1/conversations/{conv}/items?limit=100&order={order}{q}"), key, None).await;
            assert_eq!(s, StatusCode::OK, "{page}");
            out.extend(page["data"].as_array().unwrap().iter().cloned());
            pages += 1;
            match page["next_cursor"].as_str() {
                Some(next) if page["has_more"] == true => after = next.to_string(),
                _ => break,
            }
        }
        assert_eq!(pages, 3, "250 items at 100 a page");
        out
    }
    let asc = walk(&c, &key, &conv_id, "asc").await;
    let texts: Vec<&str> = asc.iter().map(|i| i["content"][0]["text"].as_str().unwrap()).collect();
    let want: Vec<String> = std::iter::once("Be brief.".to_string()).chain((0..249).map(|i| format!("m{i}"))).collect();
    assert_eq!(texts, want, "every item once, in order");
    let mut desc = walk(&c, &key, &conv_id, "desc").await;
    desc.reverse();
    assert_eq!(desc, asc, "desc is asc reversed");
    let (s, e) = call(&c.app, "GET", &format!("/v1/conversations/{conv_id}/items?order=sideways"), &key, None).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_order")));

    // One item; the inline legacy messages are the latest 200 user/assistant messages.
    let first = asc[1]["id"].as_str().unwrap();
    let (s, it) = call(&c.app, "GET", &format!("/v1/conversations/{conv_id}/items/{first}"), &key, None).await;
    assert_eq!((s, it["content"][0]["text"].as_str()), (StatusCode::OK, Some("m0")));
    let (_, full) = call(&c.app, "GET", &format!("/v1/conversations/{conv_id}"), &key, None).await;
    let msgs = full["messages"].as_array().unwrap();
    assert_eq!((full["item_count"].as_i64(), msgs.len(), msgs[0]["content"].as_str(), msgs[199]["content"].as_str()), (Some(250), 200, Some("m49"), Some("m248")));
    assert_eq!(msgs[0]["object"], "conversation.message");

    // PATCH replaces metadata; a key bound to another account sees nothing.
    let (s, patched) = call(&c.app, "PATCH", &format!("/v1/conversations/{conv_id}"), &key, Some(json!({ "metadata": { "j": "2" } }))).await;
    assert_eq!((s, patched["metadata"].clone()), (StatusCode::OK, json!({ "j": "2" })));
    let key_b = mint(&c, &p, Some(&acct_b), &["agents"]).await;
    assert_eq!(call(&c.app, "GET", &format!("/v1/conversations/{conv_id}"), &key_b, None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&c.app, "GET", &format!("/v1/conversations/{conv_id}/items/{first}"), &key_b, None).await.0, StatusCode::NOT_FOUND);
    let (_, listed) = call(&c.app, "GET", "/v1/conversations", &key_b, None).await;
    assert!(listed["data"].as_array().unwrap().is_empty(), "{listed}");
    let (_, listed) = call(&c.app, "GET", &format!("/v1/conversations?account_id={acct_a}"), &key, None).await;
    assert_eq!(listed["data"][0]["id"].as_str(), Some(conv_id.as_str()));

    // An agent's conversation takes messages, not items; the legacy reply is projected from items.
    let (_, agent) = call(&c.app, "POST", "/v1/agents", &key, Some(json!({ "account_id": acct_a, "name": "Ada" }))).await;
    let agent_id = agent["id"].as_str().unwrap();
    let (s, e) = call(&c.app, "POST", "/v1/conversations", &key, Some(json!({ "agent_id": agent_id, "items": [{ "role": "user", "content": "x" }] }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("conversation_has_agent")));
    let (s, aconv) = call(&c.app, "POST", "/v1/conversations", &key, Some(json!({ "agent_id": agent_id }))).await;
    assert_eq!((s, aconv["account_id"].as_str()), (StatusCode::CREATED, Some(acct_a.as_str())));
    let aconv_id = aconv["id"].as_str().unwrap();
    let (s, e) = call(&c.app, "POST", &format!("/v1/conversations/{aconv_id}/items"), &key, Some(json!({ "items": [{ "role": "user", "content": "x" }] }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("conversation_has_agent")));
    let (s, raw) = call_raw(&c.app, "POST", &format!("/v1/conversations/{aconv_id}/messages"), &key, json!({ "content": "hello" })).await;
    assert_eq!(s, StatusCode::OK, "{raw}");
    let keys: Vec<usize> = ["\"id\"", "\"object\"", "\"conversation_id\"", "\"role\"", "\"content\"", "\"status\"", "\"error\"", "\"created_at\""].iter().map(|k| raw.find(k).unwrap()).collect();
    assert!(keys.windows(2).all(|w| w[0] < w[1]), "conversation.message field order: {raw}");
    let reply: Value = serde_json::from_str(&raw).unwrap();
    let (_, aitems) = call(&c.app, "GET", &format!("/v1/conversations/{aconv_id}/items"), &key, None).await;
    let roles: Vec<&str> = aitems["data"].as_array().unwrap().iter().map(|i| i["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["user", "assistant"]);
    assert_eq!(aitems["data"][1]["id"], reply["id"], "the reply is the assistant item");
    let legacy_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM platform_conversation_messages WHERE conversation_id = $1").bind(aconv_id).fetch_one(&c.state.db).await.unwrap();
    assert_eq!(legacy_rows, 0, "the legacy table is no longer written");

    // DELETE takes the items with it.
    let (s, gone) = call(&c.app, "DELETE", &format!("/v1/conversations/{conv_id}"), &key, None).await;
    assert_eq!((s, gone), (StatusCode::OK, json!({ "id": conv_id, "object": "conversation.deleted", "deleted": true })));
    assert_eq!(call(&c.app, "GET", &format!("/v1/conversations/{conv_id}"), &key, None).await.0, StatusCode::NOT_FOUND);
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM platform_items WHERE conversation_id = $1").bind(&conv_id).fetch_one(&c.state.db).await.unwrap();
    assert_eq!(left, 0);
}

// ---- turns always end (POST /v1/responses with `agent`) --------------------------

/// A runtime that answers slowly (`delay` before the reply), or never (`hang`),
/// with a short turn cap so the timeout path runs in a test.
struct SlowHost {
    delay: std::time::Duration,
    hang: bool,
    cap: std::time::Duration,
}

#[async_trait::async_trait]
impl AgentHost for SlowHost {
    async fn runtime(&self, project_id: &str) -> Result<HostRuntime, PlatformError> {
        Ok(HostRuntime { owner: hosting::runtime_owner(project_id), runtime_id: "rt_slow".into() })
    }
    async fn call(&self, _rt: &HostRuntime, _method: &str, _path: &str, _body: &Value) -> Result<(u16, Value), PlatformError> {
        Ok((200, json!({ "sessionId": "ses_slow" })))
    }
    async fn stream(&self, _rt: &HostRuntime, _path: &str, _body: &Value) -> Result<RelayStream, PlatformError> {
        use futures::StreamExt as _;
        if self.hang {
            return Ok(Box::pin(futures::stream::pending()));
        }
        let delay = self.delay;
        let first = futures::stream::once(async { Ok(bytes::Bytes::from("data: {\"type\":\"text.delta\",\"text\":\"slow \"}\n\n")) });
        let rest = futures::stream::once(async move {
            tokio::time::sleep(delay).await;
            Ok(bytes::Bytes::from("data: {\"type\":\"text.delta\",\"text\":\"answer\"}\n\ndata: {\"type\":\"done\"}\n\n"))
        });
        Ok(Box::pin(first.chain(rest)))
    }
    fn turn_cap(&self) -> std::time::Duration {
        self.cap
    }
}

/// A project with an agent on `host`: (ctx, project id, agents key, agent id).
async fn slow_agent(host: SlowHost) -> (Ctx, String, String, String) {
    let c = ctx().await;
    let dyn_host: Arc<dyn AgentHost> = Arc::new(host);
    let app = router_gated(&c.state, Gate::Forced(true)).layer(axum::Extension(dyn_host)).with_state(c.state.clone());
    let c = Ctx { state: c.state, app };
    let p = project(&c, "dev_slow", ProjectEnv::Sandbox).await;
    let key = mint(&c, &p, None, &["agents"]).await;
    let acct = account(&c, &key, "A").await;
    let (s, agent) = call(&c.app, "POST", "/v1/agents", &key, Some(json!({ "account_id": acct, "name": "Ada" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{agent}");
    let agent_id = agent["id"].as_str().unwrap().to_string();
    (c, p.id, key, agent_id)
}

/// The project's newest response: (id, status, error).
async fn newest_response(db: &sqlx::PgPool, project_id: &str) -> Option<(String, String, Option<Value>)> {
    sqlx::query_as("SELECT id, status, error FROM platform_responses WHERE project_id = $1 ORDER BY created_at DESC LIMIT 1").bind(project_id).fetch_optional(db).await.unwrap()
}

/// Wait (up to 5 s) for the project's newest response to leave `in_progress`.
async fn ended_response(db: &sqlx::PgPool, project_id: &str) -> (String, String, Option<Value>) {
    for _ in 0..100 {
        if let Some(r) = newest_response(db, project_id).await.filter(|r| r.1 != "in_progress") {
            return r;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("the response never ended: {:?}", newest_response(db, project_id).await);
}

/// (type, role, status, content, error) of a response's items, in order.
async fn response_items(db: &sqlx::PgPool, response_id: &str) -> Vec<(String, Option<String>, String, Value, Option<String>)> {
    sqlx::query_as("SELECT type, role, status, content, error FROM platform_items WHERE response_id = $1 ORDER BY seq").bind(response_id).fetch_all(db).await.unwrap()
}

/// Start a request, wait until its response row exists, then drop the request
/// future: what hyper does when the client (or Cloudflare at 100 s) hangs up.
async fn hang_up_after_start(c: &Ctx, project_id: &str, key: &str, body: Value) {
    let (app, key) = (c.app.clone(), key.to_string());
    let req = tokio::spawn(async move { call_raw(&app, "POST", "/v1/responses", &key, body).await });
    for _ in 0..100 {
        if newest_response(&c.state.db, project_id).await.is_some_and(|r| r.1 == "in_progress") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let row = newest_response(&c.state.db, project_id).await.expect("the turn started");
    assert_eq!(row.1, "in_progress", "the client hangs up mid-turn");
    req.abort();
    assert!(req.await.unwrap_err().is_cancelled(), "the request future was dropped mid-turn");
}

#[tokio::test]
async fn a_response_whose_client_hangs_up_still_ends_completed() {
    let (c, project_id, key, agent_id) =
        slow_agent(SlowHost { delay: std::time::Duration::from_millis(400), hang: false, cap: hosting::AGENT_TURN_CAP }).await;

    // Non-streaming: the 524 case. The turn runs on and is stored completed.
    hang_up_after_start(&c, &project_id, &key, json!({ "agent": agent_id, "input": "hi" })).await;
    let (id, status, error) = ended_response(&c.state.db, &project_id).await;
    assert_eq!((status.as_str(), error), ("completed", None));
    let items = response_items(&c.state.db, &id).await;
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!((items[1].1.as_deref(), items[1].2.as_str(), &items[1].3), (Some("assistant"), "completed", &json!([{ "type": "output_text", "text": "slow answer" }])));
    // The conversation is free again: the next turn runs.
    let (s, next) = call(&c.app, "POST", "/v1/responses", &key, Some(json!({ "agent": agent_id, "input": "again", "previous_response_id": id }))).await;
    assert_eq!((s, next["status"].as_str()), (StatusCode::OK, Some("completed")), "{next}");

    // Streaming: a dropped SSE client doesn't leave the response running either.
    hang_up_after_start(&c, &project_id, &key, json!({ "agent": agent_id, "input": "hi", "stream": true, "previous_response_id": next["id"] })).await;
    let (id, status, _) = ended_response(&c.state.db, &project_id).await;
    assert_ne!(id, next["id"].as_str().unwrap());
    assert_eq!(status, "completed");
}

#[tokio::test]
async fn a_turn_past_the_cap_fails_with_turn_timeout_and_frees_the_conversation() {
    let (c, project_id, key, agent_id) =
        slow_agent(SlowHost { delay: std::time::Duration::ZERO, hang: true, cap: std::time::Duration::from_millis(300) }).await;
    let (s, r) = call(&c.app, "POST", "/v1/responses", &key, Some(json!({ "agent": agent_id, "input": "hi" }))).await;
    assert_eq!((s, r["status"].as_str(), r["error"]["code"].as_str()), (StatusCode::OK, Some("failed"), Some("turn_timeout")), "{r}");
    assert!(r["completed_at"].is_string(), "{r}");
    // The failed reply is stored, so the conversation shows it.
    let id = r["id"].as_str().unwrap();
    let items = response_items(&c.state.db, id).await;
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!((items[1].0.as_str(), items[1].1.as_deref(), items[1].2.as_str()), ("message", Some("assistant"), "failed"));
    assert!(items[1].4.as_deref().unwrap_or("").contains("didn't finish"), "{items:?}");
    assert_eq!(r["output"].as_array().map(Vec::len), Some(1), "{r}");
    let (_, status, _) = newest_response(&c.state.db, &project_id).await.unwrap();
    assert_eq!(status, "failed");
    // The lock is released: the next turn isn't `conversation_busy` (it times out too).
    let (s, again) = call(&c.app, "POST", "/v1/responses", &key, Some(json!({ "agent": agent_id, "input": "again", "previous_response_id": id }))).await;
    assert_eq!((s, again["error"]["code"].as_str()), (StatusCode::OK, Some("turn_timeout")), "{again}");

    // Streaming ends the same way, with `response.failed`.
    let (s, body) = call_raw(&c.app, "POST", "/v1/responses", &key, json!({ "agent": agent_id, "input": "x", "stream": true, "previous_response_id": again["id"] })).await;
    assert_eq!(s, StatusCode::OK);
    assert!(body.contains("event: response.failed") && body.contains("turn_timeout"), "{body}");
}

#[tokio::test]
async fn the_sweep_fails_responses_left_in_progress() {
    let (c, project_id, key, agent_id) =
        slow_agent(SlowHost { delay: std::time::Duration::ZERO, hang: false, cap: hosting::AGENT_TURN_CAP }).await;
    let db = &c.state.db;
    sqlx::raw_sql(&include_str!("../../../migrations_pg/138_platform_responses_in_progress.sql").replace("public.", "")).execute(db).await.expect("migration 138 applies");
    let (s, done) = call(&c.app, "POST", "/v1/responses", &key, Some(json!({ "agent": agent_id, "input": "hi" }))).await;
    assert_eq!((s, done["status"].as_str()), (StatusCode::OK, Some("completed")), "{done}");
    let conv_id = done["conversation_id"].as_str().unwrap();

    // Two rows a crashed turn left behind (one with an open assistant item) and one still running.
    for (id, minutes_ago) in [("resp_stuck_old", 60), ("resp_stuck_open", 20), ("resp_running", 1)] {
        sqlx::query(&format!(
            "INSERT INTO platform_responses (id, project_id, agent_id, conversation_id, status, created_at) VALUES ($1, $2, $3, $4, 'in_progress', NOW() - interval '{minutes_ago} minutes')"
        ))
        .bind(id)
        .bind(&project_id)
        .bind(&agent_id)
        .bind(conv_id)
        .execute(db)
        .await
        .unwrap();
        super::items::append(db, &project_id, Some(conv_id), Some(id), vec![super::items::NewItem::user_text("hello")]).await.unwrap();
    }
    super::items::append(db, &project_id, Some(conv_id), Some("resp_stuck_open"), vec![super::items::NewItem::assistant_text("half").with_status("in_progress")]).await.unwrap();

    assert_eq!(super::responses::fail_abandoned(db).await.unwrap(), 2);
    assert_eq!(super::responses::fail_abandoned(db).await.unwrap(), 0, "a second sweep finds nothing");

    for id in ["resp_stuck_old", "resp_stuck_open"] {
        let (status, error, completed): (String, Option<Value>, Option<chrono::DateTime<chrono::Utc>>) =
            sqlx::query_as("SELECT status, error, completed_at FROM platform_responses WHERE id = $1").bind(id).fetch_one(db).await.unwrap();
        assert_eq!((status.as_str(), error.as_ref().and_then(|e| e["code"].as_str()), completed.is_some()), ("failed", Some("turn_abandoned"), true), "{id}");
        let items = response_items(db, id).await;
        let replies: Vec<_> = items.iter().filter(|i| i.1.as_deref() == Some("assistant")).collect();
        assert_eq!(replies.len(), 1, "{id}: exactly one assistant item {items:?}");
        assert_eq!(replies[0].2, "failed", "{id}");
        assert!(items.iter().all(|i| i.2 != "in_progress"), "{id}: {items:?}");
    }
    // A live turn and a finished one are untouched.
    let running: String = sqlx::query_scalar("SELECT status FROM platform_responses WHERE id = 'resp_running'").fetch_one(db).await.unwrap();
    assert_eq!(running, "in_progress");
    let (_, finished) = call(&c.app, "GET", &format!("/v1/responses/{}", done["id"].as_str().unwrap()), &key, None).await;
    assert_eq!((finished["status"].as_str(), &finished["error"]), (Some("completed"), &Value::Null));
}
