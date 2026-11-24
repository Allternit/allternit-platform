//! Console governance (Wave 4, L4B): org policy, key rules, roles and the
//! audit log, through the real console routes. Real Postgres
//! (schema-per-test); the Clerk session is stood in for by the test-only
//! claims header (`console::TEST_CLAIMS_HEADER`).

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::{console::TEST_CLAIMS_HEADER, router_gated, Gate};
use crate::{
    routes::test_support::{test_state, MockGateway},
    services::api_keys,
    ApiState,
};

/// `audit_log` as migrations_pg/001 creates it (test schemas start without it).
const AUDIT_LOG_TABLE: &str = r#"
CREATE TABLE audit_log (
    id text NOT NULL,
    "timestamp" timestamp with time zone DEFAULT CURRENT_TIMESTAMP,
    action text, resource_type text, resource_id text, user_id text, user_email text,
    token_id text, ip_address text, user_agent text, request_path text, request_method text,
    status_code bigint, details text, success boolean DEFAULT true
)"#;

async fn app() -> (Arc<ApiState>, Router) {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    sqlx::raw_sql(AUDIT_LOG_TABLE).execute(&state.db).await.expect("audit_log");
    for sql in [
        include_str!("../../../migrations_pg/003_api_keys.sql"),
        include_str!("../../../migrations_pg/050_platform_api_foundation.sql"),
        include_str!("../../../migrations_pg/080_platform_billing.sql"),
        include_str!("../../../migrations_pg/084_platform_payment_methods.sql"),
        include_str!("../../../migrations_pg/126_key_governance.sql"),
    ] {
        sqlx::raw_sql(&sql.replace("public.", "")).execute(&state.db).await.expect("migration applies");
    }
    let app = router_gated(&state, Gate::Forced(true)).with_state(state.clone());
    (state, app)
}

fn admin(user: &str, org: &str) -> Value {
    json!({ "sub": user, "email": format!("{user}@example.com"), "o": { "id": org, "rol": "admin" } })
}

fn member(user: &str, org: &str, role: &str) -> Value {
    json!({ "sub": user, "email": format!("{user}@example.com"), "org_id": org, "org_role": role })
}

fn personal(user: &str) -> Value {
    json!({ "sub": user, "email": format!("{user}@example.com") })
}

struct Reply {
    status: StatusCode,
    body: Value,
    text: String,
    headers: axum::http::HeaderMap,
}

async fn send(app: &Router, claims: &Value, method: &str, path: &str, body: Option<Value>) -> Reply {
    let builder = Request::builder().method(method).uri(path).header(TEST_CLAIMS_HEADER, claims.to_string());
    let request = match body {
        Some(v) => builder.header("content-type", "application/json").body(Body::from(v.to_string())).unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
    let text = String::from_utf8_lossy(&bytes).to_string();
    Reply { status, body: serde_json::from_str(&text).unwrap_or(Value::Null), text, headers }
}

async fn new_project(app: &Router, claims: &Value) -> String {
    let r = send(app, claims, "POST", "/api/v1/platform/projects", Some(json!({ "name": "Gov" }))).await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    r.body["id"].as_str().unwrap().to_string()
}

fn in_a_week() -> String {
    (chrono::Utc::now() + chrono::Duration::days(7)).to_rfc3339()
}

#[tokio::test]
async fn org_policy_is_org_admin_only() {
    let (_, app) = app().await;
    for who in [member("dev_1", "org_1", "org:member"), member("bill_1", "org_1", "org:billing"), personal("solo")] {
        let r = send(&app, &who, "GET", "/api/v1/platform/org/policy", None).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{who}: {:?}", r.body);
        assert_eq!(r.body["error"]["code"], "org_admin_required");
        let r = send(&app, &who, "PUT", "/api/v1/platform/org/policy", Some(json!({ "require_key_expiry": true }))).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{who}: {:?}", r.body);
    }

    let boss = admin("admin_1", "org_1");
    let r = send(&app, &boss, "GET", "/api/v1/platform/org/policy", None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!((r.body["require_key_expiry"].clone(), r.body["ip_allowlist"].clone()), (json!(false), json!([])));

    let put = json!({ "require_key_expiry": true, "max_key_ttl_days": 30, "spend_alert_email": " ops@example.com " });
    let r = send(&app, &boss, "PUT", "/api/v1/platform/org/policy", Some(put)).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["spend_alert_email"], "ops@example.com");
    let r = send(&app, &boss, "GET", "/api/v1/platform/org/policy", None).await;
    assert_eq!((r.body["require_key_expiry"].clone(), r.body["max_key_ttl_days"].clone()), (json!(true), json!(30)));
    // Another org's admin sees their own (empty) policy, not org_1's.
    let r = send(&app, &admin("admin_2", "org_2"), "GET", "/api/v1/platform/org/policy", None).await;
    assert_eq!(r.body["require_key_expiry"], false);
    // A header-injection attempt is not an address.
    let bad = json!({ "spend_alert_email": "ops@example.com\r\nBcc: x@evil.test" });
    assert_eq!(send(&app, &boss, "PUT", "/api/v1/platform/org/policy", Some(bad)).await.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn org_policy_rules_apply_to_key_create_and_patch() {
    let (_, app) = app().await;
    let boss = admin("admin_1", "org_1");
    let dev = member("dev_1", "org_1", "org:developer");
    let r = send(&app, &boss, "PUT", "/api/v1/platform/org/policy", Some(json!({ "require_key_expiry": true, "max_key_ttl_days": 30 }))).await;
    assert_eq!(r.status, StatusCode::OK);
    let project = new_project(&app, &dev).await;
    let keys = format!("/api/v1/platform/projects/{project}/keys");

    let r = send(&app, &dev, "POST", &keys, Some(json!({ "name": "ci", "scopes": ["agents"] }))).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{:?}", r.body);
    assert_eq!(r.body["error"]["code"], "expiry_required");

    let too_long = (chrono::Utc::now() + chrono::Duration::days(45)).to_rfc3339();
    let r = send(&app, &dev, "POST", &keys, Some(json!({ "name": "ci", "scopes": ["agents"], "expires_at": too_long }))).await;
    assert_eq!(r.body["error"]["code"], "expiry_too_long", "{:?}", r.body);

    let r = send(&app, &dev, "POST", &keys, Some(json!({ "name": "ci", "scopes": ["agents"], "expires_at": in_a_week() }))).await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(r.body["kind"], "standard");
    assert!(r.body["expires_at"].is_string());
    assert!(r.body["allowed_cidrs"].is_null());
    let key_id = r.body["id"].as_str().unwrap().to_string();

    // PATCH can't drop the expiry the policy requires, but can change it.
    let key_path = format!("{keys}/{key_id}");
    let r = send(&app, &dev, "PATCH", &key_path, Some(json!({ "expires_at": null }))).await;
    assert_eq!(r.body["error"]["code"], "expiry_required", "{:?}", r.body);
    let later = (chrono::Utc::now() + chrono::Duration::days(20)).to_rfc3339();
    let r = send(&app, &dev, "PATCH", &key_path, Some(json!({ "expires_at": later }))).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);

    // Viewer and billing roles cannot write keys, even on a project they own.
    for role in ["org:viewer", "org:billing"] {
        let who = member(&format!("u_{}", role.replace(':', "_")), "org_1", role);
        let own = new_project(&app, &who).await;
        let r = send(&app, &who, "POST", &format!("/api/v1/platform/projects/{own}/keys"),
            Some(json!({ "name": "x", "scopes": ["agents"], "expires_at": in_a_week() }))).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{role}: {:?}", r.body);
        assert_eq!(r.body["error"]["code"], "insufficient_role");
    }
}

#[tokio::test]
async fn key_create_is_audited_for_the_owner_only() {
    let (state, app) = app().await;
    let owner = personal("owner_1");
    let project = new_project(&app, &owner).await;
    let r = send(&app, &owner, "POST", &format!("/api/v1/platform/projects/{project}/keys"),
        Some(json!({ "name": "ci", "scopes": ["agents"] }))).await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let key_id = r.body["id"].as_str().unwrap().to_string();
    // The stored row carries the project scope.
    let scope: (Option<String>, Option<String>) =
        sqlx::query_as("SELECT project_id, organization_id FROM audit_log WHERE action = 'platform_key.create'")
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert_eq!(scope, (Some(project.clone()), None));

    let r = send(&app, &owner, "GET", "/api/v1/platform/audit-log", None).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let data = r.body["data"].as_array().unwrap();
    let created = data.iter().find(|e| e["action"] == "platform_key.create").expect("key create audited");
    assert_eq!(created["actor_email"], "owner_1@example.com");
    assert_eq!(created["resource"], format!("api_key:{key_id}"));
    assert_eq!(created["project_id"], project.as_str());
    // Newest first: the key was created after the project.
    assert_eq!(data[0]["action"], "platform_key.create");
    assert_eq!(data[1]["action"], "platform_project.create");
    assert!(r.body["next_cursor"].is_null());

    // Pages: one row, then the other, then the end.
    let r = send(&app, &owner, "GET", "/api/v1/platform/audit-log?limit=1", None).await;
    assert_eq!(r.body["data"][0]["action"], "platform_key.create");
    let cursor = r.body["next_cursor"].as_str().expect("more").to_string();
    let r = send(&app, &owner, "GET", &format!("/api/v1/platform/audit-log?limit=1&after={cursor}"), None).await;
    assert_eq!(r.body["data"][0]["action"], "platform_project.create");
    assert!(r.body["next_cursor"].is_null(), "{:?}", r.body);

    // CSV export of the same rows.
    let r = send(&app, &owner, "GET", "/api/v1/platform/audit-log?format=csv&action=platform_key.create", None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.headers["content-type"].to_str().unwrap().starts_with("text/csv"));
    assert_eq!(r.text.lines().count(), 2, "{}", r.text);
    assert!(r.text.contains(&format!("\"api_key:{key_id}\"")));

    // A stranger, and another org's admin, see none of it.
    for who in [personal("stranger"), admin("admin_9", "org_9")] {
        let r = send(&app, &who, "GET", "/api/v1/platform/audit-log", None).await;
        assert_eq!(r.status, StatusCode::OK);
        assert_eq!(r.body["data"], json!([]), "{who}");
        let r = send(&app, &who, "GET", &format!("/api/v1/platform/audit-log?project_id={project}"), None).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{who}");
    }
}

#[tokio::test]
async fn org_admin_sees_org_entries_including_policy_changes() {
    let (_, app) = app().await;
    let boss = admin("admin_1", "org_1");
    let dev = member("dev_1", "org_1", "org:member");
    let project = new_project(&app, &dev).await;
    send(&app, &boss, "PUT", "/api/v1/platform/org/policy", Some(json!({ "live_keys_admin_only": true }))).await;

    let r = send(&app, &boss, "GET", "/api/v1/platform/audit-log", None).await;
    let actions: Vec<&str> = r.body["data"].as_array().unwrap().iter().filter_map(|e| e["action"].as_str()).collect();
    assert_eq!(actions, vec!["org_policy.update", "platform_project.create"], "{:?}", r.body);
    // The member sees their project's entry but not the org policy change.
    let r = send(&app, &dev, "GET", "/api/v1/platform/audit-log", None).await;
    let actions: Vec<&str> = r.body["data"].as_array().unwrap().iter().filter_map(|e| e["action"].as_str()).collect();
    assert_eq!(actions, vec!["platform_project.create"]);
    let r = send(&app, &dev, "GET", &format!("/api/v1/platform/audit-log?project_id={project}"), None).await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn ip_allowlists_are_refused_while_the_switch_is_off() {
    let (_, app) = app().await;
    let owner = personal("owner_ip");
    let project = new_project(&app, &owner).await;
    let keys = format!("/api/v1/platform/projects/{project}/keys");
    let boss = admin("admin_ip", "org_ip");

    {
        let _off = api_keys::force_ip_allowlist(false);
        let r = send(&app, &owner, "POST", &keys, Some(json!({ "name": "ci", "scopes": ["agents"], "allowed_cidrs": ["10.0.0.0/8"] }))).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{:?}", r.body);
        assert_eq!(r.body["error"]["code"], "ip_allowlist_disabled");
        assert_eq!(r.body["error"]["message"], "IP allowlists are not enabled on this server");
        let r = send(&app, &boss, "PUT", "/api/v1/platform/org/policy", Some(json!({ "ip_allowlist": ["10.0.0.0/8"] }))).await;
        assert_eq!(r.body["error"]["code"], "ip_allowlist_disabled", "{:?}", r.body);
        assert_eq!(send(&app, &boss, "GET", "/api/v1/platform/org/policy", None).await.body["ip_allowlist_enabled"], false);

        // An empty list is fine (it is no restriction).
        let r = send(&app, &owner, "POST", &keys, Some(json!({ "name": "ci", "scopes": ["agents"], "allowed_cidrs": [] }))).await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let key_id = r.body["id"].as_str().unwrap().to_string();
        let r = send(&app, &owner, "PATCH", &format!("{keys}/{key_id}"), Some(json!({ "allowed_cidrs": ["10.0.0.0/8"] }))).await;
        assert_eq!(r.body["error"]["code"], "ip_allowlist_disabled", "{:?}", r.body);
    }

    let _on = api_keys::force_ip_allowlist(true);
    let r = send(&app, &owner, "POST", &keys, Some(json!({ "name": "ci", "scopes": ["agents"], "allowed_cidrs": [" 10.0.0.0/8 "] }))).await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(r.body["allowed_cidrs"], json!(["10.0.0.0/8"]));
    let r = send(&app, &owner, "POST", &keys, Some(json!({ "name": "ci", "scopes": ["agents"], "allowed_cidrs": ["office"] }))).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = send(&app, &boss, "PUT", "/api/v1/platform/org/policy", Some(json!({ "ip_allowlist": ["203.0.113.0/24"] }))).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["ip_allowlist"], json!(["203.0.113.0/24"]));
}

#[tokio::test]
async fn org_ip_allowlist_applies_to_project_keys_not_console_sessions() {
    let _on = api_keys::force_ip_allowlist(true);
    let (state, app) = app().await;
    let boss = admin("admin_net", "org_net");
    let project = new_project(&app, &boss).await;
    super::project_billing::put_test_card(&state.db, &project).await;
    let r = send(&app, &boss, "POST", &format!("/api/v1/platform/projects/{project}/keys"),
        Some(json!({ "name": "ci", "scopes": ["agents"] }))).await;
    let token = r.body["token"].as_str().unwrap().to_string();
    let r = send(&app, &boss, "PUT", "/api/v1/platform/org/policy", Some(json!({ "ip_allowlist": ["10.0.0.0/8"] }))).await;
    assert_eq!(r.status, StatusCode::OK);

    let key_call = |ip: &'static str| {
        let builder = Request::builder().method("GET").uri("/v1/accounts").header("authorization", format!("Bearer {token}"));
        super::tests::via_local_proxy(builder, Some(ip)).body(Body::empty()).unwrap()
    };
    let r = app.clone().oneshot(key_call("203.0.113.9")).await.unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let r = app.clone().oneshot(key_call("10.2.3.4")).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    // The console session (no key) is never held to the org list.
    let r = send(&app, &boss, "GET", &format!("/api/v1/platform/projects/{project}/keys"), None).await;
    assert_eq!(r.status, StatusCode::OK);
}
