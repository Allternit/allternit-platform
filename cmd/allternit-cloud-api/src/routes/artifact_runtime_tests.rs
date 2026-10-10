//! Tests for the page runtime API: every route needs a signed-in caller with
//! access, capabilities must be declared, shared writes need consent, the 20
//! MB cap holds, and outside invitees never get AI or connectors. Uses the
//! same harness as `artifacts_v2_tests` (a Postgres schema per test with
//! migration 085 and the `x-test-caller` header).

use super::*;
use crate::routes::test_support::{test_state, MockGateway};
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn setup() -> Router {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    sqlx::raw_sql(include_str!("../../migrations_pg/085_artifacts_v2.sql"))
        .execute(&state.db)
        .await
        .expect("085 applies");
    super::super::routes().with_state(state)
}

fn who(id: &str, org: Option<&str>, role: Option<&str>) -> Value {
    json!({ "id": id, "org_id": org, "org_role": role, "email": format!("{id}@example.com") })
}

async fn send(router: &Router, method: &str, path: &str, caller: Option<&Value>, body: Option<Value>) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(caller) = caller {
        builder = builder.header("x-test-caller", caller.to_string());
    }
    let req = match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = router.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes)))
    };
    (status, body)
}

async fn page(router: &Router, owner: &Value, caps: Value) -> String {
    let (status, body) = send(
        router,
        "POST",
        "/api/v2/artifacts",
        Some(owner),
        Some(json!({ "kind": "page", "title": "App", "body": "<h1>hi</h1>", "capabilities": caps })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().to_string()
}

async fn share_with(router: &Router, owner: &Value, id: &str, visibility: &str, shares: Value) {
    let (status, body) = send(
        router,
        "PUT",
        &format!("/api/v2/artifacts/{id}/sharing"),
        Some(owner),
        Some(json!({ "visibility": visibility, "shares": shares })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

fn url(id: &str, rest: &str) -> String {
    format!("/api/v2/artifact-runtime/{id}/{rest}")
}

#[tokio::test]
async fn every_runtime_route_requires_authentication() {
    let router = setup().await;
    for (method, path) in [
        ("GET", "/api/v2/artifact-runtime/art_x/context"),
        ("GET", "/api/v2/artifact-runtime/art_x/storage"),
        ("GET", "/api/v2/artifact-runtime/art_x/storage/k"),
        ("PUT", "/api/v2/artifact-runtime/art_x/storage/k"),
        ("DELETE", "/api/v2/artifact-runtime/art_x/storage/k"),
        ("GET", "/api/v2/artifact-runtime/art_x/consents"),
        ("PUT", "/api/v2/artifact-runtime/art_x/consents"),
        ("POST", "/api/v2/artifact-runtime/art_x/ai"),
        ("GET", "/api/v2/org/artifact-shared-outside"),
    ] {
        let body = matches!(method, "PUT" | "POST").then(|| json!({}));
        let (status, _) = send(&router, method, path, None, body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
    }
}

#[tokio::test]
async fn no_access_is_a_404_not_a_leak() {
    let router = setup().await;
    let alice = who("u_alice", None, None);
    let stranger = who("u_stranger", None, None);
    let id = page(&router, &alice, json!({"storage": true, "ai": true})).await;
    for (method, rest, body) in [
        ("GET", "context", None),
        ("GET", "storage", None),
        ("PUT", "storage/k", Some(json!({"value": "v"}))),
        ("GET", "consents", None),
        ("POST", "ai", Some(json!({"prompt": "hi"}))),
    ] {
        let (status, _) = send(&router, method, &url(&id, rest), Some(&stranger), body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {rest}");
    }
    // A public link doesn't open the runtime to anonymous callers either.
    let (status, _) = send(&router, "GET", &url(&id, "storage"), None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn storage_must_be_declared() {
    let router = setup().await;
    let alice = who("u_alice", None, None);
    let id = page(&router, &alice, json!({})).await;
    let (status, body) = send(&router, "PUT", &url(&id, "storage/k"), Some(&alice), Some(json!({"value": "v"}))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "capability_not_declared");
}

#[tokio::test]
async fn personal_storage_is_per_viewer() {
    let router = setup().await;
    let alice = who("u_alice", None, None);
    let bob = who("u_bob", None, None);
    let id = page(&router, &alice, json!({"storage": true})).await;
    share_with(&router, &alice, &id, "people", json!([{"principal_type": "user", "principal_id": "u_bob", "level": "view"}])).await;

    let put = |c: &Value, v: &str| {
        let c = c.clone();
        let path = url(&id, "storage/score?scope=personal");
        let router = router.clone();
        let v = v.to_string();
        async move { send(&router, "PUT", &path, Some(&c), Some(json!({"value": v}))).await }
    };
    assert_eq!(put(&alice, "10").await.0, StatusCode::OK);
    assert_eq!(put(&bob, "99").await.0, StatusCode::OK);

    let (_, a) = send(&router, "GET", &url(&id, "storage/score?scope=personal"), Some(&alice), None).await;
    let (_, b) = send(&router, "GET", &url(&id, "storage/score?scope=personal"), Some(&bob), None).await;
    assert_eq!((a["value"].as_str(), b["value"].as_str()), (Some("10"), Some("99")));

    let (_, listed) = send(&router, "GET", &url(&id, "storage?scope=personal&prefix=sc"), Some(&bob), None).await;
    assert_eq!(listed["keys"], json!(["score"]));
    let (status, _) = send(&router, "DELETE", &url(&id, "storage/score?scope=personal"), Some(&bob), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, after) = send(&router, "GET", &url(&id, "storage/score?scope=personal"), Some(&bob), None).await;
    assert!(after["value"].is_null());
    let (_, alice_still) = send(&router, "GET", &url(&id, "storage/score?scope=personal"), Some(&alice), None).await;
    assert_eq!(alice_still["value"], "10");
}

#[tokio::test]
async fn shared_writes_need_consent_and_are_visible_to_every_viewer() {
    let router = setup().await;
    let alice = who("u_alice", None, None);
    let bob = who("u_bob", None, None);
    let id = page(&router, &alice, json!({"storage": true})).await;
    share_with(&router, &alice, &id, "people", json!([{"principal_type": "user", "principal_id": "u_bob", "level": "view"}])).await;

    let path = url(&id, "storage/board?scope=shared");
    let (status, body) = send(&router, "PUT", &path, Some(&bob), Some(json!({"value": "x"}))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "consent_required");
    assert_eq!(body["capability"], "storage_shared");
    // Deleting is a shared write too.
    let (status, _) = send(&router, "DELETE", &path, Some(&bob), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // A refusal isn't consent.
    let (status, _) = send(&router, "PUT", &url(&id, "consents"), Some(&bob), Some(json!({"capability": "storage_shared", "granted": false}))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(&router, "PUT", &path, Some(&bob), Some(json!({"value": "x"}))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = send(&router, "PUT", &url(&id, "consents"), Some(&bob), Some(json!({"capability": "storage_shared", "granted": true}))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = send(&router, "PUT", &path, Some(&bob), Some(json!({"value": "hello"}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Reading shared data needs no consent; the owner sees what Bob wrote.
    let (_, read) = send(&router, "GET", &path, Some(&alice), None).await;
    assert_eq!(read["value"], "hello");
    // And it's not in anyone's personal scope.
    let (_, personal) = send(&router, "GET", &url(&id, "storage/board?scope=personal"), Some(&bob), None).await;
    assert!(personal["value"].is_null());
}

#[tokio::test]
async fn storage_stops_at_twenty_megabytes_across_everyone() {
    let router = setup().await;
    let alice = who("u_alice", None, None);
    let id = page(&router, &alice, json!({"storage": true})).await;
    let four_mb = "x".repeat(4 * 1024 * 1024);
    for i in 0..4 {
        let (status, body) = send(&router, "PUT", &url(&id, &format!("storage/k{i}")), Some(&alice), Some(json!({"value": four_mb}))).await;
        assert_eq!(status, StatusCode::OK, "k{i}: {body}");
    }
    // Four MB blocks leave a little under 4 MB: a 4 MB fifth doesn't fit...
    let (status, body) = send(&router, "PUT", &url(&id, "storage/k4"), Some(&alice), Some(json!({"value": four_mb}))).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::PAYLOAD_TOO_LARGE, Some("storage_full")));
    // ...overwriting an existing key with the same size still does...
    let (status, _) = send(&router, "PUT", &url(&id, "storage/k0"), Some(&alice), Some(json!({"value": four_mb}))).await;
    assert_eq!(status, StatusCode::OK);
    // ...and freeing space makes room again.
    send(&router, "DELETE", &url(&id, "storage/k1"), Some(&alice), None).await;
    let (status, _) = send(&router, "PUT", &url(&id, "storage/k4"), Some(&alice), Some(json!({"value": four_mb}))).await;
    assert_eq!(status, StatusCode::OK);
    let (_, ctx) = send(&router, "GET", &url(&id, "context"), Some(&alice), None).await;
    assert_eq!(ctx["storage"]["limit_bytes"], 20 * 1024 * 1024);
}

#[tokio::test]
async fn single_values_and_keys_are_bounded() {
    let router = setup().await;
    let alice = who("u_alice", None, None);
    let id = page(&router, &alice, json!({"storage": true})).await;
    let big = "x".repeat(5 * 1024 * 1024 + 1);
    let (status, body) = send(&router, "PUT", &url(&id, "storage/big"), Some(&alice), Some(json!({"value": big}))).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::PAYLOAD_TOO_LARGE, Some("value_too_large")));
    let long_key = "k".repeat(201);
    let (status, _) = send(&router, "PUT", &url(&id, &format!("storage/{long_key}")), Some(&alice), Some(json!({"value": "v"}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = send(&router, "PUT", &url(&id, "storage/k"), Some(&alice), Some(json!({"value": 5}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = send(&router, "GET", &url(&id, "storage/k?scope=team"), Some(&alice), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn ai_needs_a_declaration_and_the_viewers_consent() {
    let router = setup().await;
    let alice = who("u_alice", None, None);
    let none = page(&router, &alice, json!({"storage": true})).await;
    let (status, body) = send(&router, "POST", &url(&none, "ai"), Some(&alice), Some(json!({"prompt": "hi"}))).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::CONFLICT, Some("capability_not_declared")));

    let id = page(&router, &alice, json!({"ai": true})).await;
    let (status, body) = send(&router, "POST", &url(&id, "ai"), Some(&alice), Some(json!({"prompt": "hi"}))).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("consent_required")));
    assert_eq!(body["capability"], "ai");
}

#[tokio::test]
async fn consent_only_for_what_the_page_declared() {
    let router = setup().await;
    let alice = who("u_alice", None, None);
    let id = page(&router, &alice, json!({"storage": true})).await;
    let (status, _) = send(&router, "PUT", &url(&id, "consents"), Some(&alice), Some(json!({"capability": "ai", "granted": true}))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = send(&router, "PUT", &url(&id, "consents"), Some(&alice), Some(json!({"capability": "root", "granted": true}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = send(&router, "PUT", &url(&id, "consents"), Some(&alice), Some(json!({"capability": "storage_shared", "granted": "yes"}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn connector_approval_keeps_the_per_tool_off_list() {
    let router = setup().await;
    let alice = who("u_alice", None, None);
    let id = page(&router, &alice, json!({"connectors": [{"connector": "github", "tools": ["list_issues", "get_issue"]}]})).await;
    let (status, _) = send(
        &router,
        "PUT",
        &url(&id, "consents"),
        Some(&alice),
        Some(json!({"capability": "connectors", "granted": true, "denied_tools": ["github/get_issue"]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, ctx) = send(&router, "GET", &url(&id, "context"), Some(&alice), None).await;
    assert_eq!(ctx["consents"]["connectors"]["granted"], true);
    assert_eq!(ctx["consents"]["connectors"]["denied_tools"], json!(["github/get_issue"]));
    assert_eq!(ctx["allowed"]["connectors"]["ok"], true);
    assert_eq!(ctx["allowed"]["ai"]["reason"], "not_declared");
}

#[tokio::test]
async fn outside_invitees_never_get_ai_or_connectors() {
    let router = setup().await;
    let owner = who("u_owner", Some("org_a"), Some("admin"));
    let guest = who("u_guest", Some("org_b"), None);
    let member = who("u_member", Some("org_a"), None);
    let id = page(&router, &owner, json!({"storage": true, "ai": true})).await;
    share_with(
        &router,
        &owner,
        &id,
        "people",
        json!([
            {"principal_type": "user", "principal_id": "u_guest", "level": "view"},
            {"principal_type": "user", "principal_id": "u_member", "level": "view"},
        ]),
    )
    .await;
    send(&router, "PUT", &url(&id, "consents"), Some(&guest), Some(json!({"capability": "ai", "granted": true}))).await;
    let (status, body) = send(&router, "POST", &url(&id, "ai"), Some(&guest), Some(json!({"prompt": "hi"}))).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("outside_invitee")), "{body}");
    let (_, ctx) = send(&router, "GET", &url(&id, "context"), Some(&guest), None).await;
    assert_eq!(ctx["allowed"]["ai"]["reason"], "outside_invitee");
    assert_eq!(ctx["allowed"]["storage"]["ok"], true);
    // An org member shared the same way is inside.
    let (_, ctx) = send(&router, "GET", &url(&id, "context"), Some(&member), None).await;
    assert_eq!(ctx["allowed"]["ai"]["ok"], true);
}

#[tokio::test]
async fn org_switches_reach_the_runtime() {
    let router = setup().await;
    let admin = who("u_admin", Some("org_a"), Some("org:admin"));
    let id = page(&router, &admin, json!({"storage": true, "connectors": [{"connector": "gh", "tools": ["list"]}]})).await;
    let (status, _) = send(&router, "PUT", "/api/v2/org/artifact-settings", Some(&admin), Some(json!({"connectors": false}))).await;
    assert_eq!(status, StatusCode::OK);
    let (_, ctx) = send(&router, "GET", &url(&id, "context"), Some(&admin), None).await;
    assert_eq!(ctx["allowed"]["connectors"]["reason"], "org_off");
    assert_eq!(ctx["allowed"]["storage"]["ok"], true);
    let (status, _) = send(&router, "PUT", "/api/v2/org/artifact-settings", Some(&admin), Some(json!({"enabled": false}))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = send(&router, "PUT", &url(&id, "storage/k"), Some(&admin), Some(json!({"value": "v"}))).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("org_disabled")));
}

#[tokio::test]
async fn pages_that_use_ai_or_connectors_cannot_be_linked() {
    let router = setup().await;
    let alice = who("u_alice", None, None);
    let id = page(&router, &alice, json!({"ai": true})).await;
    let (status, body) = send(
        &router,
        "PUT",
        &format!("/api/v2/artifacts/{id}/sharing"),
        Some(&alice),
        Some(json!({"visibility": "link", "shares": []})),
    )
    .await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("link_not_allowed")));
    // Storage alone doesn't block a link.
    let storage_only = page(&router, &alice, json!({"storage": true})).await;
    share_with(&router, &alice, &storage_only, "link", json!([])).await;
}

#[tokio::test]
async fn shared_outside_list_is_for_org_admins() {
    let router = setup().await;
    let admin = who("u_admin", Some("org_a"), Some("admin"));
    let member = who("u_member", Some("org_a"), None);
    let solo = who("u_solo", None, None);
    let (status, _) = send(&router, "PUT", "/api/v2/org/artifact-settings", Some(&admin), Some(json!({"external_sharing": true}))).await;
    assert_eq!(status, StatusCode::OK);
    let linked = page(&router, &member, json!({"storage": true})).await;
    share_with(&router, &member, &linked, "link", json!([])).await;
    let _private = page(&router, &member, json!({})).await;

    let (status, _) = send(&router, "GET", "/api/v2/org/artifact-shared-outside", Some(&member), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(&router, "GET", "/api/v2/org/artifact-shared-outside", Some(&solo), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = send(&router, "GET", "/api/v2/org/artifact-shared-outside", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{body}");
    assert_eq!(items[0]["id"], linked.as_str());
    assert_eq!(items[0]["by_link"], true);
    assert_eq!(items[0]["allowed"], false);
}

#[test]
fn ai_rate_limit_is_per_viewer_and_artifact() {
    for _ in 0..rules::AI_CALLS_PER_MINUTE {
        assert!(ai_rate_ok("rate_u1", "rate_a1"));
    }
    assert!(!ai_rate_ok("rate_u1", "rate_a1"));
    assert!(ai_rate_ok("rate_u2", "rate_a1"));
    assert!(ai_rate_ok("rate_u1", "rate_a2"));
}
