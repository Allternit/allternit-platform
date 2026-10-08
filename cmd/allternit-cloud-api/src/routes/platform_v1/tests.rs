//! Platform API foundation tests. Real Postgres (schema-per-test, see
//! `routes::test_support`); the migration under test is applied for real.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::{
    caller::{self, Plan},
    projects::{self, Principal, ProjectPatch},
    record_usage, router_gated, slots, Gate, ProjectEnv, UsageEvent,
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

async fn ctx_with_gate(gate: Gate) -> Ctx {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    for sql in [
        include_str!("../../../migrations_pg/003_api_keys.sql"),
        include_str!("../../../migrations_pg/050_platform_api_foundation.sql"),
        include_str!("../../../migrations_pg/080_platform_billing.sql"),
        include_str!("../../../migrations_pg/084_platform_payment_methods.sql"),
    ] {
        sqlx::raw_sql(&sql.replace("public.", ""))
            .execute(&state.db)
            .await
            .expect("migration applies");
    }
    let app = router_gated(&state, gate).with_state(state.clone());
    Ctx { state, app }
}

async fn ctx() -> Ctx {
    ctx_with_gate(Gate::Forced(true)).await
}

fn principal(user: &str) -> Principal {
    Principal { user_id: user.to_string(), org_id: None, org_admin: false }
}

async fn project(ctx: &Ctx, owner: &str, env: ProjectEnv) -> projects::Project {
    let p = projects::create_project(&ctx.state.db, &principal(owner), "Test project", env)
        .await
        .unwrap();
    super::project_billing::put_test_card(&ctx.state.db, &p.id).await;
    p
}

async fn mint(ctx: &Ctx, project: &projects::Project, account: Option<&str>, scopes: &[&str]) -> (String, String) {
    let created = api_keys::create_project_key(
        &ctx.state.db,
        CreateProjectKeyInput {
            user_id: project.owner_user_id.clone(),
            organization_id: None,
            project_id: project.id.clone(),
            account_id: account.map(str::to_string),
            env: ProjectEnv::parse(&project.env).unwrap(),
            name: "test key".into(),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
        },
    )
    .await
    .unwrap();
    (created.key.id, created.token)
}

struct Reply {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Value,
}

async fn call(app: &Router, method: &str, path: &str, token: Option<&str>, body: Option<Value>, idem: Option<&str>) -> Reply {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(t) = token {
        builder = builder.header("authorization", format!("Bearer {t}"));
    }
    if let Some(k) = idem {
        builder = builder.header("idempotency-key", k);
    }
    let request = match body {
        Some(b) => builder
            .header("content-type", "application/json")
            .body(Body::from(b.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Reply { status, headers, body }
}

async fn create_account(ctx: &Ctx, token: &str, name: &str) -> String {
    let r = call(&ctx.app, "POST", "/v1/accounts", Some(token), Some(json!({ "name": name })), None).await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    r.body["id"].as_str().unwrap().to_string()
}

// ---- keys ----------------------------------------------------------------

#[tokio::test]
async fn project_keys_mint_and_resolve() {
    let c = ctx().await;
    let sandbox = project(&c, "user_a", ProjectEnv::Sandbox).await;
    let live = project(&c, "user_a", ProjectEnv::Live).await;

    let (key_id, test_token) = mint(&c, &sandbox, None, &["agents", "usage"]).await;
    assert!(test_token.starts_with("alt_test_"));
    assert_eq!(test_token.len(), "alt_test_".len() + 64);
    let (_, live_token) = mint(&c, &live, None, &["agents"]).await;
    assert!(live_token.starts_with("alt_live_"));

    // Stored hashed, never plaintext.
    let stored: String = sqlx::query_scalar("SELECT token_hash FROM api_keys WHERE id = $1")
        .bind(&key_id)
        .fetch_one(&c.state.db)
        .await
        .unwrap();
    assert_ne!(stored, test_token);
    assert_eq!(stored, api_keys::hash_token(&test_token));

    let caller = caller::authenticate(&c.state.db, Some(&test_token)).await.unwrap();
    assert_eq!(caller.project_id, sandbox.id);
    assert_eq!(caller.project_env, ProjectEnv::Sandbox);
    assert_eq!(caller.key_id, key_id);
    assert_eq!(caller.scopes, vec!["agents", "usage"]);
    assert_eq!(caller.owner_user_id, "user_a");
    assert_eq!(caller.plan, Plan::Sandbox);
    assert_eq!(caller::authenticate(&c.state.db, Some(&live_token)).await.unwrap().project_env, ProjectEnv::Live);

    // Garbage, missing and revoked keys are 401.
    assert_eq!(caller::authenticate(&c.state.db, None).await.unwrap_err().code, "missing_api_key");
    assert_eq!(
        caller::authenticate(&c.state.db, Some(&format!("alt_test_{}", "0".repeat(64)))).await.unwrap_err().code,
        "invalid_api_key"
    );
    api_keys::revoke_project_key(&c.state.db, &sandbox.id, &key_id).await.unwrap();
    assert!(caller::authenticate(&c.state.db, Some(&test_token)).await.is_err());
}

#[tokio::test]
async fn project_key_scopes_are_validated() {
    assert!(api_keys::normalize_project_scopes(vec![]).is_err());
    assert!(api_keys::normalize_project_scopes(vec!["root".into()]).is_err());
    assert_eq!(
        api_keys::normalize_project_scopes(vec![" Agents ".into(), "agents".into(), "voice".into(), "compute".into()]).unwrap(),
        vec!["agents", "voice", "compute"]
    );
}

#[tokio::test]
async fn legacy_keys_are_unaffected_and_project_keys_stay_out_of_legacy_auth() {
    let c = ctx().await;
    let legacy = api_keys::create_api_key(
        &c.state.db,
        api_keys::CreateApiKeyInput {
            user_id: "user_legacy".into(),
            organization_id: None,
            name: "legacy".into(),
            scopes: vec![],
        },
    )
    .await
    .unwrap();
    assert!(legacy.token.starts_with("alt_"));
    assert!(!legacy.token.starts_with("alt_live_") && !legacy.token.starts_with("alt_test_"));

    // Legacy authentication, both entry points, exactly as before.
    let authed = api_keys::authenticate_api_key(&c.state.db, &legacy.token).await.unwrap().unwrap();
    assert_eq!(authed.user_id, "user_legacy");
    assert_eq!(authed.scopes, vec!["compute", "inference"]);
    let via_middleware = crate::auth::middleware::validate_token_against_db(&c.state.db, &legacy.token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(via_middleware.user_id, "user_legacy");
    assert_eq!(via_middleware.permissions, vec!["compute", "inference"]);

    // A legacy key is not a project key.
    let reply = call(&c.app, "GET", "/v1/accounts", Some(&legacy.token), None, None).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert_eq!(reply.body["error"]["type"], "authentication_error");

    // A project key never authenticates as the owning user on legacy paths.
    let p = project(&c, "user_legacy", ProjectEnv::Sandbox).await;
    let (_, token) = mint(&c, &p, None, &["compute", "inference", "agents"]).await;
    assert!(api_keys::authenticate_api_key(&c.state.db, &token).await.unwrap().is_none());
    assert!(crate::auth::middleware::validate_token_against_db(&c.state.db, &token)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn auth_errors_use_the_platform_error_format() {
    let c = ctx().await;
    let r = call(&c.app, "GET", "/v1/accounts", None, None, None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let e = &r.body["error"];
    assert_eq!(e["type"], "authentication_error");
    assert_eq!(e["code"], "missing_api_key");
    assert!(e["message"].is_string());
    assert!(e.get("param").is_some());
}

// ---- accounts and isolation ------------------------------------------------

#[tokio::test]
async fn accounts_crud_and_validation() {
    let c = ctx().await;
    let p = project(&c, "u1", ProjectEnv::Sandbox).await;
    let (_, token) = mint(&c, &p, None, &["agents"]).await;

    let r = call(&c.app, "POST", "/v1/accounts", Some(&token),
        Some(json!({ "name": "Acme", "external_ref": "cust_1", "metadata": { "tier": "gold" } })), None).await;
    assert_eq!(r.status, StatusCode::CREATED);
    let id = r.body["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("acct_"));
    assert_eq!(r.body["object"], "account");
    assert_eq!(r.body["metadata"]["tier"], "gold");

    // external_ref is unique per project.
    let dup = call(&c.app, "POST", "/v1/accounts", Some(&token),
        Some(json!({ "name": "Other", "external_ref": "cust_1" })), None).await;
    assert_eq!(dup.status, StatusCode::CONFLICT);
    assert_eq!(dup.body["error"]["code"], "external_ref_taken");

    // Validation and bad JSON.
    let bad = call(&c.app, "POST", "/v1/accounts", Some(&token), Some(json!({ "name": "  " })), None).await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    assert_eq!(bad.body["error"]["param"], "name");
    let junk = call(&c.app, "POST", "/v1/accounts", Some(&token), Some(json!([1])), None).await;
    assert_eq!(junk.status, StatusCode::BAD_REQUEST);
    assert_eq!(junk.body["error"]["type"], "invalid_request_error");

    let got = call(&c.app, "GET", &format!("/v1/accounts/{id}"), Some(&token), None, None).await;
    assert_eq!(got.body["name"], "Acme");

    let patched = call(&c.app, "PATCH", &format!("/v1/accounts/{id}"), Some(&token),
        Some(json!({ "name": "Acme Inc", "external_ref": null })), None).await;
    assert_eq!(patched.status, StatusCode::OK);
    assert_eq!(patched.body["name"], "Acme Inc");
    assert!(patched.body["external_ref"].is_null());
    assert_eq!(patched.body["metadata"]["tier"], "gold");

    let del = call(&c.app, "DELETE", &format!("/v1/accounts/{id}"), Some(&token), None, None).await;
    assert_eq!(del.body["deleted"], true);
    let gone = call(&c.app, "GET", &format!("/v1/accounts/{id}"), Some(&token), None, None).await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);

    // The freed external_ref can be reused.
    let again = call(&c.app, "POST", "/v1/accounts", Some(&token),
        Some(json!({ "name": "Acme 2", "external_ref": "cust_1" })), None).await;
    assert_eq!(again.status, StatusCode::CREATED);
}

#[tokio::test]
async fn accounts_need_a_resource_scope() {
    let c = ctx().await;
    let p = project(&c, "u1", ProjectEnv::Sandbox).await;
    let (_, token) = mint(&c, &p, None, &["usage", "inference"]).await;
    let r = call(&c.app, "GET", "/v1/accounts", Some(&token), None, None).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.body["error"]["type"], "permission_error");
    assert_eq!(r.body["error"]["code"], "insufficient_scope");
}

#[tokio::test]
async fn account_bound_keys_and_projects_are_isolated() {
    let c = ctx().await;
    let px = project(&c, "owner_x", ProjectEnv::Sandbox).await;
    let py = project(&c, "owner_y", ProjectEnv::Sandbox).await;
    let (_, admin_x) = mint(&c, &px, None, &["agents"]).await;
    let (_, admin_y) = mint(&c, &py, None, &["agents"]).await;
    let acct_a = create_account(&c, &admin_x, "A").await;
    let acct_b = create_account(&c, &admin_x, "B").await;
    let acct_y = create_account(&c, &admin_y, "Y").await;
    let (_, key_a) = mint(&c, &px, Some(&acct_a), &["agents", "usage"]).await;

    // Bound key sees only its own account.
    let list = call(&c.app, "GET", "/v1/accounts", Some(&key_a), None, None).await;
    let ids: Vec<&str> = list.body["data"].as_array().unwrap().iter().map(|a| a["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec![acct_a.as_str()]);
    assert_eq!(call(&c.app, "GET", &format!("/v1/accounts/{acct_a}"), Some(&key_a), None, None).await.status, StatusCode::OK);
    // Another account in the same project looks like it doesn't exist.
    let other = call(&c.app, "GET", &format!("/v1/accounts/{acct_b}"), Some(&key_a), None, None).await;
    assert_eq!(other.status, StatusCode::NOT_FOUND);
    assert_eq!(call(&c.app, "PATCH", &format!("/v1/accounts/{acct_b}"), Some(&key_a), Some(json!({ "name": "x" })), None).await.status, StatusCode::NOT_FOUND);
    // It can't create or delete accounts.
    assert_eq!(call(&c.app, "POST", "/v1/accounts", Some(&key_a), Some(json!({ "name": "n" })), None).await.status, StatusCode::FORBIDDEN);
    assert_eq!(call(&c.app, "DELETE", &format!("/v1/accounts/{acct_a}"), Some(&key_a), None, None).await.status, StatusCode::FORBIDDEN);
    // And can't ask for usage of a different account.
    let usage = call(&c.app, "GET", &format!("/v1/usage?account_id={acct_b}"), Some(&key_a), None, None).await;
    assert_eq!(usage.status, StatusCode::FORBIDDEN);
    assert_eq!(usage.body["error"]["code"], "account_mismatch");

    // Project X's key can't see project Y's account, and the lists are disjoint.
    assert_eq!(call(&c.app, "GET", &format!("/v1/accounts/{acct_y}"), Some(&admin_x), None, None).await.status, StatusCode::NOT_FOUND);
    assert_eq!(call(&c.app, "DELETE", &format!("/v1/accounts/{acct_y}"), Some(&admin_x), None, None).await.status, StatusCode::NOT_FOUND);
    let list_x = call(&c.app, "GET", "/v1/accounts", Some(&admin_x), None, None).await;
    assert_eq!(list_x.body["data"].as_array().unwrap().len(), 2);
    let list_y = call(&c.app, "GET", "/v1/accounts", Some(&admin_y), None, None).await;
    assert_eq!(list_y.body["data"].as_array().unwrap().len(), 1);

    // Deleting an account revokes its bound keys.
    assert_eq!(call(&c.app, "DELETE", &format!("/v1/accounts/{acct_a}"), Some(&admin_x), None, None).await.status, StatusCode::OK);
    assert_eq!(call(&c.app, "GET", "/v1/accounts", Some(&key_a), None, None).await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn accounts_paginate_with_cursors() {
    let c = ctx().await;
    let p = project(&c, "u1", ProjectEnv::Sandbox).await;
    let (_, token) = mint(&c, &p, None, &["agents"]).await;
    for n in 0..5 {
        create_account(&c, &token, &format!("acct {n}")).await;
    }
    let mut seen = Vec::new();
    let mut after: Option<String> = None;
    for expected_more in [true, true, false] {
        let path = match &after {
            Some(a) => format!("/v1/accounts?limit=2&after={a}"),
            None => "/v1/accounts?limit=2".to_string(),
        };
        let page = call(&c.app, "GET", &path, Some(&token), None, None).await;
        assert_eq!(page.status, StatusCode::OK);
        assert_eq!(page.body["has_more"], expected_more);
        for a in page.body["data"].as_array().unwrap() {
            seen.push(a["id"].as_str().unwrap().to_string());
        }
        after = page.body["next_cursor"].as_str().map(str::to_string);
        assert_eq!(after.is_some(), expected_more);
    }
    assert_eq!(seen.len(), 5);
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 5);
    assert_eq!(call(&c.app, "GET", "/v1/accounts?limit=0", Some(&token), None, None).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(call(&c.app, "GET", "/v1/accounts?after=zzzz", Some(&token), None, None).await.status, StatusCode::BAD_REQUEST);
}

// ---- idempotency, rate limits, slots -----------------------------------------

#[tokio::test]
async fn idempotency_replays_and_conflicts() {
    let c = ctx().await;
    let p = project(&c, "u1", ProjectEnv::Sandbox).await;
    let (_, token) = mint(&c, &p, None, &["agents"]).await;
    let body = json!({ "name": "Once" });

    let first = call(&c.app, "POST", "/v1/accounts", Some(&token), Some(body.clone()), Some("key-1")).await;
    assert_eq!(first.status, StatusCode::CREATED);
    assert!(first.headers.get("idempotent-replayed").is_none());

    let replay = call(&c.app, "POST", "/v1/accounts", Some(&token), Some(body.clone()), Some("key-1")).await;
    assert_eq!(replay.status, StatusCode::CREATED);
    assert_eq!(replay.body, first.body);
    assert_eq!(replay.headers.get("idempotent-replayed").unwrap(), "true");

    // Only one account was created.
    let list = call(&c.app, "GET", "/v1/accounts", Some(&token), None, None).await;
    assert_eq!(list.body["data"].as_array().unwrap().len(), 1);

    // Same key, different body → 409.
    let conflict = call(&c.app, "POST", "/v1/accounts", Some(&token), Some(json!({ "name": "Different" })), Some("key-1")).await;
    assert_eq!(conflict.status, StatusCode::CONFLICT);
    assert_eq!(conflict.body["error"]["type"], "conflict_error");
    assert_eq!(conflict.body["error"]["code"], "idempotency_conflict");

    // Keys are scoped to the API key: another key can reuse the string.
    let (_, token2) = mint(&c, &p, None, &["agents"]).await;
    let other = call(&c.app, "POST", "/v1/accounts", Some(&token2), Some(json!({ "name": "Different" })), Some("key-1")).await;
    assert_eq!(other.status, StatusCode::CREATED);

    // Error responses (4xx) replay too; no key means no replay.
    let bad = json!({ "name": "" });
    let e1 = call(&c.app, "POST", "/v1/accounts", Some(&token), Some(bad.clone()), Some("key-bad")).await;
    let e2 = call(&c.app, "POST", "/v1/accounts", Some(&token), Some(bad), Some("key-bad")).await;
    assert_eq!((e1.status, e2.status), (StatusCode::BAD_REQUEST, StatusCode::BAD_REQUEST));
    assert_eq!(e2.headers.get("idempotent-replayed").unwrap(), "true");

    // An expired record is not replayed.
    sqlx::query("UPDATE platform_idempotency SET created_at = NOW() - INTERVAL '25 hours'")
        .execute(&c.state.db)
        .await
        .unwrap();
    let fresh = call(&c.app, "POST", "/v1/accounts", Some(&token), Some(json!({ "name": "Different" })), Some("key-1")).await;
    assert_eq!(fresh.status, StatusCode::CREATED);
    assert!(fresh.headers.get("idempotent-replayed").is_none());
}

#[tokio::test]
async fn idempotency_key_must_be_valid() {
    let c = ctx().await;
    let p = project(&c, "u1", ProjectEnv::Sandbox).await;
    let (_, token) = mint(&c, &p, None, &["agents"]).await;
    let long = "k".repeat(256);
    let r = call(&c.app, "POST", "/v1/accounts", Some(&token), Some(json!({ "name": "x" })), Some(&long)).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.body["error"]["code"], "invalid_idempotency_key");
}

#[tokio::test]
async fn rate_limit_returns_429_and_headers() {
    let c = ctx().await;
    let p = project(&c, "u1", ProjectEnv::Sandbox).await;
    sqlx::query("UPDATE platform_projects SET rpm_override = 3 WHERE id = $1")
        .bind(&p.id)
        .execute(&c.state.db)
        .await
        .unwrap();
    let (_, token) = mint(&c, &p, None, &["agents"]).await;

    let mut remaining = Vec::new();
    for _ in 0..3 {
        let r = call(&c.app, "GET", "/v1/accounts", Some(&token), None, None).await;
        assert_eq!(r.status, StatusCode::OK);
        assert_eq!(r.headers.get("x-ratelimit-limit").unwrap(), "3");
        assert!(r.headers.get("x-ratelimit-reset").is_some());
        remaining.push(r.headers.get("x-ratelimit-remaining").unwrap().to_str().unwrap().to_string());
    }
    assert_eq!(remaining, vec!["2", "1", "0"]);

    let limited = call(&c.app, "GET", "/v1/accounts", Some(&token), None, None).await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.body["error"]["type"], "rate_limit_error");
    assert_eq!(limited.body["error"]["code"], "rate_limit_exceeded");
    assert_eq!(limited.headers.get("x-ratelimit-remaining").unwrap(), "0");
    assert!(limited.headers.get("retry-after").is_some());

    // The project counter is shared: a second key in the same project is limited too.
    let (_, token2) = mint(&c, &p, None, &["agents"]).await;
    let also = call(&c.app, "GET", "/v1/accounts", Some(&token2), None, None).await;
    assert_eq!(also.status, StatusCode::TOO_MANY_REQUESTS);

    // Another project is unaffected.
    let other = project(&c, "u2", ProjectEnv::Sandbox).await;
    let (_, other_token) = mint(&c, &other, None, &["agents"]).await;
    let ok = call(&c.app, "GET", "/v1/accounts", Some(&other_token), None, None).await;
    assert_eq!(ok.status, StatusCode::OK);
    assert_eq!(ok.headers.get("x-ratelimit-limit").unwrap(), "60");
}

#[tokio::test]
async fn call_slots_cap_per_project_and_expire() {
    let c = ctx().await;
    let p = project(&c, "u1", ProjectEnv::Sandbox).await; // sandbox: 1 concurrent call
    let db = &c.state.db;

    let first = slots::acquire_slot(db, &p.id, slots::KIND_CALL).await.unwrap();
    let err = slots::acquire_slot(db, &p.id, slots::KIND_CALL).await.err().unwrap();
    assert_eq!(err.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(err.code, "concurrency_limit");
    assert_eq!(err.kind, "rate_limit_error");

    first.release().await.unwrap();
    let second = slots::acquire_slot(db, &p.id, slots::KIND_CALL).await.unwrap();

    // Expired slots free capacity and are swept.
    sqlx::query("UPDATE platform_call_slots SET expires_at = NOW() - INTERVAL '1 second'")
        .execute(db)
        .await
        .unwrap();
    assert_eq!(slots::sweep_expired_slots(db).await.unwrap(), 1);
    let third = slots::acquire_slot(db, &p.id, slots::KIND_CALL).await.unwrap();
    drop(second); // already swept; the background release is a harmless no-op
    third.release().await.unwrap();

    // Plan and override caps.
    sqlx::query("UPDATE platform_projects SET plan = 'payg' WHERE id = $1").bind(&p.id).execute(db).await.unwrap();
    let mut held = Vec::new();
    for _ in 0..5 {
        held.push(slots::acquire_slot(db, &p.id, slots::KIND_CALL).await.unwrap());
    }
    assert!(slots::acquire_slot(db, &p.id, slots::KIND_CALL).await.is_err());
    sqlx::query("UPDATE platform_projects SET call_cap_override = 6 WHERE id = $1").bind(&p.id).execute(db).await.unwrap();
    held.push(slots::acquire_slot(db, &p.id, slots::KIND_CALL).await.unwrap());
    assert!(slots::acquire_slot(db, &p.id, slots::KIND_CALL).await.is_err());

    // Projects don't share a project cap.
    let other = project(&c, "u2", ProjectEnv::Sandbox).await;
    let theirs = slots::acquire_slot(db, &other.id, slots::KIND_CALL).await.unwrap();
    theirs.release().await.unwrap();

    // Unknown project.
    assert_eq!(slots::acquire_slot(db, "proj_missing", slots::KIND_CALL).await.err().unwrap().status, StatusCode::NOT_FOUND);
}

// ---- usage ------------------------------------------------------------------

fn event(project: &str, account: Option<&str>, key: &str, meter: &str, quantity: f64, idem: Option<&str>) -> UsageEvent {
    UsageEvent {
        project_id: project.to_string(),
        account_id: account.map(str::to_string),
        key_id: Some(key.to_string()),
        meter: meter.to_string(),
        quantity,
        unit: Some("unit".to_string()),
        ref_id: None,
        idempotency: idem.map(str::to_string),
    }
}

#[tokio::test]
async fn usage_events_record_idempotently_and_group() {
    let c = ctx().await;
    let p = project(&c, "u1", ProjectEnv::Sandbox).await;
    let other = project(&c, "u2", ProjectEnv::Sandbox).await;
    let (key1, token) = mint(&c, &p, None, &["usage", "agents"]).await;
    let acct_a = create_account(&c, &token, "A").await;
    let acct_b = create_account(&c, &token, "B").await;
    let (key_a, token_a) = mint(&c, &p, Some(&acct_a), &["usage"]).await;
    let db = &c.state.db;

    assert!(record_usage(db, event(&p.id, Some(&acct_a), &key_a, "sms_segment", 2.0, Some("m1"))).await.unwrap().is_some());
    // Same idempotency value: recorded once.
    assert!(record_usage(db, event(&p.id, Some(&acct_a), &key_a, "sms_segment", 2.0, Some("m1"))).await.unwrap().is_none());
    record_usage(db, event(&p.id, Some(&acct_a), &key_a, "voice_min_allternit", 1.5, None)).await.unwrap();
    record_usage(db, event(&p.id, Some(&acct_b), &key1, "sms_segment", 3.0, None)).await.unwrap();
    record_usage(db, event(&p.id, None, &key1, "tokens_in", 1000.0, None)).await.unwrap();
    // Another project's usage never shows up; the same idempotency value there is independent.
    assert!(record_usage(db, event(&other.id, None, "ak_x", "sms_segment", 99.0, Some("m1"))).await.unwrap().is_some());

    // Bad meter / quantity are rejected.
    assert_eq!(record_usage(db, event(&p.id, None, &key1, "bogus", 1.0, None)).await.unwrap_err().code, "unknown_meter");
    assert_eq!(record_usage(db, event(&p.id, None, &key1, "mms", -1.0, None)).await.unwrap_err().code, "invalid_quantity");

    let sum = |body: &Value, group: &str, meter: &str| -> Option<f64> {
        body["data"].as_array().unwrap().iter()
            .find(|r| r["group"] == json!(group) && r["meter"] == meter)
            .and_then(|r| r["quantity"].as_f64())
    };

    let by_meter = call(&c.app, "GET", "/v1/usage", Some(&token), None, None).await;
    assert_eq!(by_meter.status, StatusCode::OK);
    assert_eq!(by_meter.body["object"], "usage");
    assert_eq!(by_meter.body["group_by"], "meter");
    assert_eq!(sum(&by_meter.body, "sms_segment", "sms_segment"), Some(5.0));
    assert_eq!(sum(&by_meter.body, "voice_min_allternit", "voice_min_allternit"), Some(1.5));
    assert_eq!(sum(&by_meter.body, "tokens_in", "tokens_in"), Some(1000.0));

    let by_key = call(&c.app, "GET", "/v1/usage?group_by=key", Some(&token), None, None).await;
    assert_eq!(sum(&by_key.body, &key_a, "sms_segment"), Some(2.0));
    assert_eq!(sum(&by_key.body, &key1, "sms_segment"), Some(3.0));

    let by_account = call(&c.app, "GET", "/v1/usage?group_by=account", Some(&token), None, None).await;
    assert_eq!(sum(&by_account.body, &acct_a, "sms_segment"), Some(2.0));
    assert_eq!(sum(&by_account.body, &acct_b, "sms_segment"), Some(3.0));
    // Usage without an account groups under a null group.
    assert!(by_account.body["data"].as_array().unwrap().iter().any(|r| r["group"].is_null() && r["meter"] == "tokens_in"));

    // Filtering and account-bound keys.
    let only_b = call(&c.app, "GET", &format!("/v1/usage?account_id={acct_b}"), Some(&token), None, None).await;
    assert_eq!(sum(&only_b.body, "sms_segment", "sms_segment"), Some(3.0));
    let bound = call(&c.app, "GET", "/v1/usage", Some(&token_a), None, None).await;
    assert_eq!(sum(&bound.body, "sms_segment", "sms_segment"), Some(2.0));
    assert_eq!(sum(&bound.body, "tokens_in", "tokens_in"), None);

    // Time range excludes events.
    let past = call(&c.app, "GET", "/v1/usage?from=2020-01-01&to=2020-02-01", Some(&token), None, None).await;
    assert!(past.body["data"].as_array().unwrap().is_empty());

    // Validation and scope.
    assert_eq!(call(&c.app, "GET", "/v1/usage?group_by=zzz", Some(&token), None, None).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(call(&c.app, "GET", "/v1/usage?from=nope", Some(&token), None, None).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(call(&c.app, "GET", "/v1/usage?from=2026-02-01&to=2026-01-01", Some(&token), None, None).await.status, StatusCode::BAD_REQUEST);
    let (_, no_usage) = mint(&c, &p, None, &["agents"]).await;
    let denied = call(&c.app, "GET", "/v1/usage", Some(&no_usage), None, None).await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
}

// ---- gate -------------------------------------------------------------------

#[tokio::test]
async fn gate_off_makes_everything_inert() {
    let c = ctx_with_gate(Gate::Forced(false)).await;
    // Even a perfectly valid key gets nothing while the gate is off.
    let p = project(&c, "u1", ProjectEnv::Sandbox).await;
    let (_, token) = mint(&c, &p, None, &["agents", "usage"]).await;
    for (method, path) in [
        ("GET", "/v1/accounts"),
        ("POST", "/v1/accounts"),
        ("GET", "/v1/usage"),
        ("GET", "/api/v1/platform/projects"),
        ("POST", "/api/v1/platform/projects"),
    ] {
        let r = call(&c.app, method, path, Some(&token), Some(json!({})), None).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{method} {path}");
        assert_eq!(r.body["error"]["code"], "platform_api_disabled");
    }
    // No side effects: nothing was counted or created.
    let windows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM platform_rate_windows").fetch_one(&c.state.db).await.unwrap();
    assert_eq!(windows, 0);
}

#[tokio::test]
async fn console_routes_require_a_clerk_session() {
    let c = ctx().await;
    for (method, path) in [
        ("GET", "/api/v1/platform/projects"),
        ("POST", "/api/v1/platform/projects"),
        ("GET", "/api/v1/platform/projects/proj_x"),
        ("POST", "/api/v1/platform/projects/proj_x/keys"),
        ("DELETE", "/api/v1/platform/projects/proj_x/keys/ak_1"),
    ] {
        let r = call(&c.app, method, path, None, Some(json!({ "name": "x" })), None).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{method} {path}: {:?}", r.body);
        assert_eq!(r.body["error"]["type"], "authentication_error");
    }
    // A project API key is not a console session.
    let p = project(&c, "u1", ProjectEnv::Sandbox).await;
    let (_, token) = mint(&c, &p, None, &["agents"]).await;
    let r = call(&c.app, "POST", &format!("/api/v1/platform/projects/{}/keys", p.id), Some(&token), Some(json!({ "name": "x", "scopes": ["agents"] })), None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

// ---- console service layer (projects + keys) ----------------------------------

#[test]
fn clerk_claims_map_to_principals() {
    use super::console::principal_from_claims;
    let (p, email) = principal_from_claims(&json!({ "sub": "user_1", "email": "a@b.c", "o": { "id": "org_1", "rol": "admin" } }));
    assert_eq!((p.user_id.as_str(), p.org_id.as_deref(), p.org_admin), ("user_1", Some("org_1"), true));
    assert_eq!(email.as_deref(), Some("a@b.c"));
    let (member, _) = principal_from_claims(&json!({ "sub": "user_2", "o": { "id": "org_1", "rol": "member" } }));
    assert!(!member.org_admin);
    let (v1, _) = principal_from_claims(&json!({ "sub": "user_3", "org_id": "org_9", "org_role": "org:admin" }));
    assert_eq!((v1.org_id.as_deref(), v1.org_admin), (Some("org_9"), true));
    let (solo, _) = principal_from_claims(&json!({ "sub": "user_4" }));
    assert_eq!((solo.org_id, solo.org_admin), (None, false));
}

#[tokio::test]
async fn project_access_is_owner_or_org_admin() {
    let c = ctx().await;
    let db = &c.state.db;
    let owner = Principal { user_id: "owner".into(), org_id: Some("org_1".into()), org_admin: false };
    let proj = projects::create_project(db, &owner, "Shared", ProjectEnv::Live).await.unwrap();
    assert_eq!(proj.env, "live");
    assert_eq!(proj.plan, "sandbox");
    assert_eq!(proj.spend_cap_cents, 10000);
    assert_eq!(proj.org_id.as_deref(), Some("org_1"));

    let stranger = principal("stranger");
    assert_eq!(projects::get_project(db, &stranger, &proj.id).await.unwrap_err().status, StatusCode::NOT_FOUND);
    assert!(projects::list_projects(db, &stranger, None, 10).await.unwrap().is_empty());

    let member = Principal { user_id: "m".into(), org_id: Some("org_1".into()), org_admin: false };
    assert!(projects::get_project(db, &member, &proj.id).await.is_err());
    let admin = Principal { user_id: "a".into(), org_id: Some("org_1".into()), org_admin: true };
    assert_eq!(projects::get_project(db, &admin, &proj.id).await.unwrap().id, proj.id);
    let other_org_admin = Principal { user_id: "z".into(), org_id: Some("org_2".into()), org_admin: true };
    assert!(projects::get_project(db, &other_org_admin, &proj.id).await.is_err());
    assert_eq!(projects::list_projects(db, &owner, None, 10).await.unwrap().len(), 1);
}

#[tokio::test]
async fn project_update_archive_and_key_lifecycle() {
    let c = ctx().await;
    let db = &c.state.db;
    let who = principal("owner");
    let proj = projects::create_project(db, &who, "  Spaced  ", ProjectEnv::Sandbox).await.unwrap();
    assert_eq!(proj.name, "Spaced");
    assert!(projects::create_project(db, &who, "", ProjectEnv::Sandbox).await.is_err());

    let updated = projects::update_project(db, &who, &proj.id, ProjectPatch { name: Some("Renamed".into()), spend_cap_cents: Some(5000), archived: None }).await.unwrap();
    assert_eq!((updated.name.as_str(), updated.spend_cap_cents), ("Renamed", 5000));
    assert!(projects::update_project(db, &who, &proj.id, ProjectPatch { name: None, spend_cap_cents: Some(-1), archived: None }).await.is_err());

    // Keys: account binding must be a live account of this project.
    let (_, admin) = mint(&c, &proj, None, &["agents"]).await;
    let acct = create_account(&c, &admin, "Cust").await;
    projects::account_in_project(db, &proj.id, &acct).await.unwrap();
    let other = project(&c, "someone", ProjectEnv::Sandbox).await;
    assert!(projects::account_in_project(db, &other.id, &acct).await.is_err());

    let listed = api_keys::list_project_keys(db, &proj.id).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].prefix.starts_with("alt_test_"));
    assert_eq!(api_keys::revoke_project_key(db, &other.id, &listed[0].id).await.unwrap_err().to_string().contains("not found"), true);
    assert_eq!(call(&c.app, "GET", "/v1/accounts", Some(&admin), None, None).await.status, StatusCode::OK);

    // Archiving revokes keys; the key stops working immediately.
    projects::update_project(db, &who, &proj.id, ProjectPatch { name: None, spend_cap_cents: None, archived: Some(true) }).await.unwrap();
    assert!(api_keys::list_project_keys(db, &proj.id).await.unwrap().is_empty());
    assert_eq!(call(&c.app, "GET", "/v1/accounts", Some(&admin), None, None).await.status, StatusCode::UNAUTHORIZED);
    assert!(projects::list_projects(db, &who, None, 10).await.unwrap().is_empty());
}

// ---- OpenAPI ------------------------------------------------------------------

/// `(METHOD, path)` pairs listed under `paths:` in the spec, read line by line
/// (the file is hand-formatted: paths at 2 spaces, methods at 4).
fn spec_routes() -> Vec<(String, String)> {
    let yaml = include_str!("../../../openapi/platform-v1.yaml");
    assert!(yaml.starts_with("openapi: 3.1"), "spec must be OpenAPI 3.1");
    let mut routes = Vec::new();
    let mut in_paths = false;
    let mut path = None::<String>;
    for line in yaml.lines() {
        if line.starts_with("paths:") {
            in_paths = true;
            continue;
        }
        if in_paths && !line.is_empty() && !line.starts_with(' ') {
            break;
        }
        if !in_paths {
            continue;
        }
        if let Some(p) = line.strip_prefix("  ").and_then(|l| l.strip_suffix(':')) {
            if p.starts_with('/') {
                path = Some(p.to_string());
                continue;
            }
        }
        if let (Some(p), Some(m)) = (&path, line.strip_prefix("    ").and_then(|l| l.strip_suffix(':'))) {
            if ["get", "post", "put", "patch", "delete"].contains(&m) {
                routes.push((m.to_uppercase(), p.clone()));
            }
        }
    }
    routes
}

#[test]
fn openapi_matches_router() {
    let mut in_router: Vec<(String, String)> = super::registered_routes()
        .into_iter()
        .map(|(m, p)| (m.to_string(), p))
        .collect();
    let mut in_spec = spec_routes();
    in_router.sort();
    in_spec.sort();
    assert!(!in_router.is_empty());

    let missing_from_spec: Vec<_> = in_router.iter().filter(|r| !in_spec.contains(r)).collect();
    let missing_from_router: Vec<_> = in_spec.iter().filter(|r| !in_router.contains(r)).collect();
    assert!(
        missing_from_spec.is_empty() && missing_from_router.is_empty(),
        "OpenAPI and router disagree.\n  in router but not in openapi/platform-v1.yaml: {missing_from_spec:?}\n  in spec but not in router: {missing_from_router:?}"
    );
    assert!(in_router.iter().all(|(_, p)| p.starts_with("/v1/")));
}
