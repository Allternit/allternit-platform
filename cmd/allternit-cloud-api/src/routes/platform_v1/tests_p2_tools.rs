//! Platform API P2 tools: knowledge files (`/v1/agents/{id}/knowledge`) with
//! real Postgres and a fake object store. The runtime tool route itself is
//! tested in `agent_tools.rs`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

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
    services::{
        api_keys::{self, CreateProjectKeyInput},
        r2::{ObjectStore, R2Error},
    },
    ApiState,
};

#[derive(Default)]
struct FakeStore {
    objects: Mutex<Vec<(String, Vec<u8>)>>,
}

#[async_trait::async_trait]
impl ObjectStore for FakeStore {
    fn presign_get(&self, _b: &str, _k: &str, _t: Duration) -> Result<String, R2Error> {
        Err(R2Error::Unavailable)
    }
    async fn head(&self, _b: &str, _k: &str) -> Result<Option<u64>, R2Error> {
        Ok(None)
    }
    async fn put(&self, _b: &str, key: &str, bytes: Vec<u8>, _ct: &str) -> Result<(), R2Error> {
        self.objects.lock().unwrap().push((key.to_string(), bytes));
        Ok(())
    }
    async fn delete(&self, _b: &str, key: &str) -> Result<(), R2Error> {
        self.objects.lock().unwrap().retain(|(k, _)| k != key);
        Ok(())
    }
}

struct Ctx {
    state: Arc<ApiState>,
    app: Router,
    store: Arc<FakeStore>,
}

async fn ctx() -> Ctx {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    for sql in [
        include_str!("../../../migrations_pg/003_api_keys.sql"),
        include_str!("../../../migrations_pg/050_platform_api_foundation.sql"),
        include_str!("../../../migrations_pg/063_platform_agents.sql"),
        include_str!("../../../migrations_pg/064_platform_conversations.sql"),
        include_str!("../../../migrations_pg/067_platform_agent_knowledge.sql"),
        include_str!("../../../migrations_pg/080_platform_billing.sql"),
        include_str!("../../../migrations_pg/084_platform_payment_methods.sql"),
    ] {
        sqlx::raw_sql(&sql.replace("public.", "")).execute(&state.db).await.expect("migration applies");
    }
    let store = Arc::new(FakeStore::default());
    let dyn_store: Arc<dyn ObjectStore> = store.clone();
    let app = router_gated(&state, Gate::Forced(true)).layer(axum::Extension(dyn_store)).with_state(state.clone());
    Ctx { state, app, store }
}

async fn mint(c: &Ctx, p: &projects::Project, account: Option<&str>) -> String {
    api_keys::create_project_key(
        &c.state.db,
        CreateProjectKeyInput {
            user_id: p.owner_user_id.clone(),
            organization_id: None,
            project_id: p.id.clone(),
            account_id: account.map(str::to_string),
            env: ProjectEnv::parse(&p.env).unwrap(),
            name: "k".into(),
            scopes: vec!["agents".into()],
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
    let bytes = axum::body::to_bytes(resp.into_body(), 4 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// A project with two accounts and an agent in each; the project key, both account ids and agent ids.
async fn setup(c: &Ctx, owner: &str) -> (projects::Project, String, [String; 2], [String; 2]) {
    let p = projects::create_project(&c.state.db, &Principal { user_id: owner.into(), org_id: None, org_admin: false }, "Tools", ProjectEnv::Sandbox).await.unwrap();
    super::project_billing::put_test_card(&c.state.db, &p.id).await;
    let key = mint(c, &p, None).await;
    let mut accounts = vec![];
    let mut agents = vec![];
    for name in ["Lakeside", "Riverside"] {
        let (_, a) = call(&c.app, "POST", "/v1/accounts", &key, Some(json!({ "name": name }))).await;
        let acct = a["id"].as_str().unwrap().to_string();
        let (s, g) = call(&c.app, "POST", "/v1/agents", &key, Some(json!({ "account_id": acct, "name": "Desk", "tools": ["knowledge_search"] }))).await;
        assert_eq!(s, StatusCode::CREATED, "{g}");
        agents.push(g["id"].as_str().unwrap().to_string());
        accounts.push(acct);
    }
    (p, key, [accounts[0].clone(), accounts[1].clone()], [agents[0].clone(), agents[1].clone()])
}

#[tokio::test]
async fn knowledge_files_upload_list_search_and_delete() {
    let c = ctx().await;
    let (_, key, _, [agent, _]) = setup(&c, "dev_k1").await;
    let path = format!("/v1/agents/{agent}/knowledge");

    let (s, f) = call(&c.app, "POST", &path, &key, Some(json!({ "name": "hours.md", "content": "# Hours\n\nWe are open Saturday mornings.\n\nParking is behind the building." }))).await;
    assert_eq!(s, StatusCode::CREATED, "{f}");
    assert_eq!((f["object"].as_str(), f["content_type"].as_str(), f["chunk_count"].as_i64()), (Some("knowledge_file"), Some("text/markdown"), Some(1)));
    let file_id = f["id"].as_str().unwrap().to_string();
    assert_eq!(c.store.objects.lock().unwrap().len(), 1, "the original is stored");

    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode("<p>Cleanings cost $120 &amp; take an hour.</p><script>x()</script>");
    let (s, h) = call(&c.app, "POST", &path, &key, Some(json!({ "name": "prices.html", "content_base64": b64 }))).await;
    assert_eq!((s, h["content_type"].as_str()), (StatusCode::CREATED, Some("text/html")), "{h}");

    let (_, list) = call(&c.app, "GET", &path, &key, None).await;
    assert_eq!(list["data"].as_array().unwrap().len(), 2);

    let hits = super::knowledge::search(&c.state.db, &agent, "how much is a cleaning", 5).await.unwrap();
    assert_eq!(hits.first().map(|h| h.file_name.as_str()), Some("prices.html"), "{hits:?}");
    assert!(!hits[0].content.contains("x()"), "scripts are not indexed");

    let (s, d) = call(&c.app, "DELETE", &format!("{path}/{file_id}"), &key, None).await;
    assert_eq!((s, d["deleted"].as_bool()), (StatusCode::OK, Some(true)));
    assert_eq!(c.store.objects.lock().unwrap().len(), 1, "the stored original is removed too");
    assert!(super::knowledge::search(&c.state.db, &agent, "Saturday", 5).await.unwrap().is_empty());
    assert_eq!(call(&c.app, "DELETE", &format!("{path}/{file_id}"), &key, None).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn knowledge_files_are_checked_and_limited() {
    let c = ctx().await;
    let (_, key, _, [agent, _]) = setup(&c, "dev_k2").await;
    let path = format!("/v1/agents/{agent}/knowledge");
    for (body, code) in [
        (json!({ "name": "a.pdf", "content_type": "application/pdf", "content": "x" }), "unsupported_content_type"),
        (json!({ "name": "a.txt" }), "invalid_content"),
        (json!({ "name": "a.txt", "content": "x", "content_base64": "eA==" }), "invalid_content"),
        (json!({ "name": "a.txt", "content_base64": "/w==" }), "invalid_content"),
        (json!({ "name": "a.txt", "content": "   " }), "invalid_content"),
        (json!({ "name": "big.txt", "content": "a ".repeat(600 * 1024) }), "file_too_large"),
        (json!({ "name": "", "content": "x" }), "invalid_name"),
    ] {
        let (s, e) = call(&c.app, "POST", &path, &key, Some(body.clone())).await;
        assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some(code)), "{body}");
    }
    for i in 0..super::knowledge::MAX_FILES {
        let (s, e) = call(&c.app, "POST", &path, &key, Some(json!({ "name": format!("f{i}.txt"), "content": "hello" }))).await;
        assert_eq!(s, StatusCode::CREATED, "{e}");
    }
    let (s, e) = call(&c.app, "POST", &path, &key, Some(json!({ "name": "one-more.txt", "content": "hello" }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("knowledge_limit_reached")));
}

#[tokio::test]
async fn knowledge_files_are_isolated_by_account_and_project() {
    let c = ctx().await;
    let (p, key, [acct_a, _], [agent_a, agent_b]) = setup(&c, "dev_k3").await;
    let (s, _) = call(&c.app, "POST", &format!("/v1/agents/{agent_b}/knowledge"), &key, Some(json!({ "name": "b.txt", "content": "Riverside secret pricing" }))).await;
    assert_eq!(s, StatusCode::CREATED);

    // A key bound to account A can't see, add to or delete from account B's agent.
    let key_a = mint(&c, &p, Some(&acct_a)).await;
    let (s, e) = call(&c.app, "GET", &format!("/v1/agents/{agent_b}/knowledge"), &key_a, None).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::NOT_FOUND, Some("agent_not_found")));
    let (s, _) = call(&c.app, "POST", &format!("/v1/agents/{agent_b}/knowledge"), &key_a, Some(json!({ "name": "x.txt", "content": "x" }))).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert!(call(&c.app, "GET", &format!("/v1/agents/{agent_a}/knowledge"), &key_a, None).await.1["data"].as_array().unwrap().is_empty());

    // Another project's key sees nothing of this one.
    let (_, other_key, _, _) = setup(&c, "dev_k4").await;
    assert_eq!(call(&c.app, "GET", &format!("/v1/agents/{agent_b}/knowledge"), &other_key, None).await.0, StatusCode::NOT_FOUND);

    // Search is per agent: A's agent finds nothing of B's file.
    assert!(super::knowledge::search(&c.state.db, &agent_a, "Riverside pricing", 5).await.unwrap().is_empty());
    assert_eq!(super::knowledge::search(&c.state.db, &agent_b, "Riverside pricing", 5).await.unwrap().len(), 1);
}
