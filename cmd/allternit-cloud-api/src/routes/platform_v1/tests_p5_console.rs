//! Platform API P5 console tests: the console acting on a project over `/v1`
//! (Clerk session + `X-Allternit-Project`) and the conversation list the
//! console's Conversations page reads. Real Postgres (schema-per-test).

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::{
    projects::{self, Principal, ProjectPatch},
    router_gated, router_with_agency, Gate, ProjectEnv, TestSession, CONSOLE_PROJECT_HEADER,
};
use crate::{
    routes::test_support::{test_state, MockGateway},
    services::api_keys::{self, CreateProjectKeyInput},
    ApiState,
};

async fn state() -> Arc<ApiState> {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    for sql in [
        include_str!("../../../migrations_pg/003_api_keys.sql"),
        include_str!("../../../migrations_pg/020_channel_inbound_queue.sql"),
        include_str!("../../../migrations_pg/024_phone_numbers.sql"),
        include_str!("../../../migrations_pg/050_platform_api_foundation.sql"),
        include_str!("../../../migrations_pg/051_platform_numbers_messaging.sql"),
        include_str!("../../../migrations_pg/057_allternit_events_backbone.sql"),
        include_str!("../../../migrations_pg/063_platform_agents.sql"),
        include_str!("../../../migrations_pg/064_platform_conversations.sql"),
    ] {
        sqlx::raw_sql(&sql.replace("public.", "")).execute(&state.db).await.expect("migration applies");
    }
    state
}

fn who(user: &str) -> Principal {
    Principal { user_id: user.into(), org_id: None, org_admin: false }
}

/// The app as `user` signed in to the console would reach it.
fn app_as(state: &Arc<ApiState>, user: &str) -> Router {
    router_gated(state, Gate::Forced(true)).layer(axum::Extension(TestSession(who(user)))).with_state(state.clone())
}

async fn send(app: &Router, method: &str, path: &str, token: &str, project: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(path).header("authorization", format!("Bearer {token}"));
    if let Some(p) = project {
        b = b.header(CONSOLE_PROJECT_HEADER, p);
    }
    let req = match body {
        Some(v) => b.header("content-type", "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn mint(state: &ApiState, p: &projects::Project, account: Option<&str>, scopes: &[&str]) -> String {
    api_keys::create_project_key(
        &state.db,
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

const SESSION: &str = "console-session-jwt";

#[tokio::test]
async fn the_console_acts_on_its_own_project_over_v1() {
    let st = state().await;
    let p = projects::create_project(&st.db, &who("owner_1"), "Console", ProjectEnv::Sandbox).await.unwrap();
    let app = app_as(&st, "owner_1");
    let pid = Some(p.id.as_str());

    let (s, acct) = send(&app, "POST", "/v1/accounts", SESSION, pid, Some(json!({ "name": "Lakeside" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{acct}");
    let acct = acct["id"].as_str().unwrap().to_string();
    // /v1/agents with a console session is the Platform API's, not the Agency API's.
    let (s, agent) = send(&app, "POST", "/v1/agents", SESSION, pid, Some(json!({ "account_id": acct, "name": "Front desk" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{agent}");
    let agent_id = agent["id"].as_str().unwrap().to_string();
    let (s, list) = send(&app, "GET", "/v1/agents", SESSION, pid, None).await;
    assert_eq!((s, list["data"].as_array().map(Vec::len)), (StatusCode::OK, Some(1)), "{list}");

    // Every scope: usage and webhooks read fine too.
    assert_eq!(send(&app, "GET", "/v1/usage", SESSION, pid, None).await.0, StatusCode::OK);
    assert_eq!(send(&app, "GET", "/v1/webhooks", SESSION, pid, None).await.0, StatusCode::OK);

    // Idempotency is kept per console user.
    let path = format!("/v1/agents/{agent_id}/conversations");
    let idem = |app: &Router| {
        let req = Request::builder().method("POST").uri(&path).header("authorization", format!("Bearer {SESSION}"))
            .header(CONSOLE_PROJECT_HEADER, &p.id).header("idempotency-key", "same").body(Body::empty()).unwrap();
        app.clone().oneshot(req)
    };
    let first = axum::body::to_bytes(idem(&app).await.unwrap().into_body(), 1 << 20).await.unwrap();
    let again = axum::body::to_bytes(idem(&app).await.unwrap().into_body(), 1 << 20).await.unwrap();
    assert_eq!(first, again, "a repeated Idempotency-Key replays the stored response");
    let key_ids: Vec<String> = sqlx::query_scalar("SELECT key_id FROM platform_idempotency").fetch_all(&st.db).await.unwrap();
    assert_eq!(key_ids, vec!["console:owner_1".to_string()]);
}

#[tokio::test]
async fn the_console_cannot_reach_projects_it_does_not_manage() {
    let st = state().await;
    let p = projects::create_project(&st.db, &who("owner_2"), "Mine", ProjectEnv::Sandbox).await.unwrap();
    let pid = Some(p.id.as_str());

    // Someone else's session: the project doesn't exist for them.
    let (s, e) = send(&app_as(&st, "stranger"), "GET", "/v1/accounts", SESSION, pid, None).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::NOT_FOUND, Some("project_not_found")));
    let (s, _) = send(&app_as(&st, "stranger"), "GET", "/v1/agents", SESSION, pid, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "not forwarded to the Agency API either");

    // A session without the header is not a project key.
    let (s, e) = send(&app_as(&st, "owner_2"), "GET", "/v1/accounts", SESSION, None, None).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::UNAUTHORIZED, Some("invalid_api_key")));

    // Without a verifiable Clerk session (no test principal), the header gets nothing.
    let real = router_gated(&st, Gate::Forced(true)).with_state(st.clone());
    let (s, e) = send(&real, "GET", "/v1/accounts", "not-a-jwt", pid, None).await;
    assert_eq!((s, e["error"]["type"].as_str()), (StatusCode::UNAUTHORIZED, Some("authentication_error")), "{e}");
    let agency = Arc::new(crate::routes::agency_forward::AgencyForward::new("http://127.0.0.1:9", Some("peer".into())));
    let with_agency = router_with_agency(&st, Gate::Forced(true), agency).with_state(st.clone());
    let (s, e) = send(&with_agency, "GET", "/v1/agents", "not-a-jwt", pid, None).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::UNAUTHORIZED, Some("invalid_session")), "{e}");

    // A project key still authenticates as itself even if the header names another project.
    let other = projects::create_project(&st.db, &who("owner_3"), "Other", ProjectEnv::Sandbox).await.unwrap();
    let key = mint(&st, &p, None, &["agents"]).await;
    let (s, list) = send(&app_as(&st, "owner_3"), "GET", "/v1/agents", &key, Some(other.id.as_str()), None).await;
    assert_eq!((s, list["data"].as_array().map(Vec::len)), (StatusCode::OK, Some(0)));

    // Archived projects are gone for the console too.
    projects::update_project(&st.db, &who("owner_2"), &p.id, ProjectPatch { name: None, spend_cap_cents: None, archived: Some(true) }).await.unwrap();
    assert_eq!(send(&app_as(&st, "owner_2"), "GET", "/v1/accounts", SESSION, pid, None).await.0, StatusCode::NOT_FOUND);

    // While the API is off and the owner isn't a beta owner, the console header learns nothing either.
    let off = router_gated(&st, Gate::BetaOnly(vec!["someone_else".into()])).layer(axum::Extension(TestSession(who("owner_3")))).with_state(st.clone());
    let (s, e) = send(&off, "GET", "/v1/accounts", SESSION, Some(other.id.as_str()), None).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::NOT_FOUND, Some("platform_api_disabled")));
    let beta = router_gated(&st, Gate::BetaOnly(vec!["owner_3".into()])).layer(axum::Extension(TestSession(who("owner_3")))).with_state(st.clone());
    assert_eq!(send(&beta, "GET", "/v1/accounts", SESSION, Some(other.id.as_str()), None).await.0, StatusCode::OK);
}

#[tokio::test]
async fn conversations_list_per_agent_with_pages_and_isolation() {
    let st = state().await;
    let p = projects::create_project(&st.db, &who("owner_4"), "Convs", ProjectEnv::Sandbox).await.unwrap();
    let app = router_gated(&st, Gate::Forced(true)).with_state(st.clone());
    let admin = mint(&st, &p, None, &["agents"]).await;
    let (_, a) = send(&app, "POST", "/v1/accounts", &admin, None, Some(json!({ "name": "A" }))).await;
    let (_, b) = send(&app, "POST", "/v1/accounts", &admin, None, Some(json!({ "name": "B" }))).await;
    let (a, b) = (a["id"].as_str().unwrap().to_string(), b["id"].as_str().unwrap().to_string());
    let (_, agent) = send(&app, "POST", "/v1/agents", &admin, None, Some(json!({ "account_id": a, "name": "A bot" }))).await;
    let (_, other) = send(&app, "POST", "/v1/agents", &admin, None, Some(json!({ "account_id": a, "name": "Other bot" }))).await;
    let agent_id = agent["id"].as_str().unwrap().to_string();
    let path = format!("/v1/agents/{agent_id}/conversations");

    let mut made = Vec::new();
    for _ in 0..3 {
        let (s, c) = send(&app, "POST", &path, &admin, None, None).await;
        assert_eq!(s, StatusCode::CREATED);
        made.push(c["id"].as_str().unwrap().to_string());
    }
    send(&app, "POST", &format!("/v1/agents/{}/conversations", other["id"].as_str().unwrap()), &admin, None, None).await;

    let (s, page1) = send(&app, "GET", &format!("{path}?limit=2"), &admin, None, None).await;
    assert_eq!(s, StatusCode::OK, "{page1}");
    assert_eq!(page1["has_more"], true);
    assert!(page1["data"][0].get("messages").is_none(), "the list leaves messages out");
    assert_eq!(page1["data"][0]["object"], "conversation");
    let cursor = page1["next_cursor"].as_str().unwrap();
    let (_, page2) = send(&app, "GET", &format!("{path}?limit=2&after={cursor}"), &admin, None, None).await;
    assert_eq!(page2["has_more"], false);
    let ids: Vec<String> = page1["data"].as_array().unwrap().iter().chain(page2["data"].as_array().unwrap()).map(|c| c["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(ids, made, "oldest first, only this agent's");

    // A key bound to another account, or without the agents scope, sees nothing.
    let key_b = mint(&st, &p, Some(&b), &["agents"]).await;
    assert_eq!(send(&app, "GET", &path, &key_b, None, None).await.0, StatusCode::NOT_FOUND);
    let usage_only = mint(&st, &p, None, &["usage"]).await;
    assert_eq!(send(&app, "GET", &path, &usage_only, None, None).await.0, StatusCode::FORBIDDEN);
    let (s, e) = send(&app, "GET", &format!("{path}?limit=0"), &admin, None, None).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_limit")));
}
