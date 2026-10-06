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

use super::{
    projects::{self, Principal},
    router_gated, Gate, ProjectEnv,
};
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
    ] {
        sqlx::raw_sql(&sql.replace("public.", "")).execute(&state.db).await.expect("migration applies");
    }
    let app = router_gated(&state, Gate::Forced(true)).with_state(state.clone());
    Ctx { state, app }
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
