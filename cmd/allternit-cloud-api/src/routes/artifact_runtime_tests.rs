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
    setup_with_db().await.0
}

/// Migrations 085 and 161 (the app columns and `app_installs` the tier gate reads).
async fn setup_with_db() -> (Router, PgPool) {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    sqlx::raw_sql(include_str!("../../migrations_pg/085_artifacts_v2.sql"))
        .execute(&state.db)
        .await
        .expect("085 applies");
    sqlx::raw_sql(include_str!("../../migrations_pg/161_apps_mcp_2026.sql"))
        .execute(&state.db)
        .await
        .expect("161 applies");
    let db = state.db.clone();
    (super::super::routes().with_state(state), db)
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

// ---------------------------------------------------------------------------
// App tiers and growth-only consent (plan v3 lane 8)
// ---------------------------------------------------------------------------

#[test]
fn app_tier_growth_rule_same_snapshot_does_not_ask() {
    let snap = json!({"storage": true, "ai": true, "connectors": [{"connector": "gh", "tools": ["list"]}]});
    // No grant yet: ask for everything the viewer's tier reaches.
    let ask = growth(Tier::App, &snap, None).expect("first use asks");
    assert_eq!(ask.requested_tier, Tier::App);
    assert_eq!(ask.added_capabilities, vec!["ai".to_string(), "storage".to_string()], "connectors need full");
    // Same snapshot, same tier: never asks again.
    let granted = json!({"storage": true, "ai": true});
    assert_eq!(growth(Tier::App, &snap, Some((Tier::App, &granted))), None);
    // A grant above what the app needs still covers it.
    assert_eq!(growth(Tier::App, &json!({"storage": true}), Some((Tier::Full, &granted))), None);
    // Tier rises: ask, with the newly reachable capability.
    let ask = growth(Tier::Full, &snap, Some((Tier::App, &granted))).expect("higher tier asks");
    assert_eq!((ask.requested_tier, ask.added_capabilities), (Tier::Full, vec!["connector:gh/list".to_string()]));
    // Same tier, one more connector tool: ask for just that tool.
    let full_grant = json!({"storage": true, "ai": true, "connectors": [{"connector": "gh", "tools": ["list"]}]});
    let grown = json!({"storage": true, "ai": true, "connectors": [{"connector": "gh", "tools": ["list", "write"]}]});
    let ask = growth(Tier::Full, &grown, Some((Tier::Full, &full_grant))).expect("added tool asks");
    assert_eq!(ask.added_capabilities, vec!["connector:gh/write".to_string()]);
}

#[test]
fn app_tier_vendor_apps_and_outsiders_never_get_full() {
    assert_eq!(max_tier("vendor", Some("u_me"), Some("org_a"), "u_me", Some("org_a")), Tier::App, "vendor apps cap at app");
    assert_eq!(max_tier("artifact", Some("u_me"), None, "u_me", None), Tier::Full, "own app");
    assert_eq!(max_tier("artifact", Some("u_a"), Some("org_a"), "u_b", Some("org_a")), Tier::Full, "org app");
    assert_eq!(max_tier("artifact", Some("u_a"), Some("org_a"), "u_c", Some("org_b")), Tier::App, "another org");
    assert_eq!(max_tier("artifact", Some("u_a"), None, "u_c", None), Tier::App, "someone else's personal app");
    // A full app seen by an outsider runs at app: connectors are out of reach.
    assert_eq!(Tier::Full.min(max_tier("vendor", None, None, "u_c", None)), Tier::App);
    assert!(Tier::App < AppMethod::Connectors.min_tier());
    assert_eq!(AppMethod::Storage.min_tier(), Tier::App);
}

async fn make_app(router: &Router, owner: &Value, id: &str, tier: &str) -> Value {
    let (status, body) = send(
        router,
        "PUT",
        &format!("/api/v2/artifacts/{id}/app"),
        Some(owner),
        Some(json!({ "name": "Tracker", "tier": tier })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn grant(db: &PgPool, user: &str, id: &str, tier: &str, caps: Value) {
    sqlx::query(
        "INSERT INTO app_installs (id, user_id, app_kind, app_ref, granted_tier, granted_capabilities) \
         VALUES ($1, $2, 'artifact', $3, $4, $5) \
         ON CONFLICT (user_id, app_kind, app_ref) DO UPDATE \
         SET granted_tier = EXCLUDED.granted_tier, granted_capabilities = EXCLUDED.granted_capabilities",
    )
    .bind(format!("ain_{}", uuid::Uuid::new_v4().simple()))
    .bind(user)
    .bind(id)
    .bind(tier)
    .bind(caps)
    .execute(db)
    .await
    .unwrap();
}

#[tokio::test]
async fn app_runtime_same_snapshot_does_not_ask_and_a_higher_tier_asks() {
    let (router, db) = setup_with_db().await;
    let owner = who("u_app_owner", Some("org_a"), None);
    let member = who("u_app_member", Some("org_a"), None);
    let id = page(&router, &owner, json!({"storage": true, "connectors": [{"connector": "gh", "tools": ["list"]}]})).await;
    share_with(&router, &owner, &id, "people", json!([{"principal_type": "user", "principal_id": "u_app_member", "level": "view"}])).await;
    make_app(&router, &owner, &id, "app").await;
    let write = url(&id, "storage/k?scope=shared");

    // Not installed yet: the gate asks, and says what for.
    let (status, body) = send(&router, "PUT", &write, Some(&member), Some(json!({"value": "v"}))).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("consent_required")), "{body}");
    assert_eq!(body["requested_tier"], "app");
    assert_eq!(body["added_capabilities"], json!(["storage"]), "connectors are above app tier");
    // The owner runs their own app without an install.
    let (status, body) = send(&router, "PUT", &url(&id, "storage/k"), Some(&owner), Some(json!({"value": "v"}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Installed: the grant is the consent (no separate shared-storage ask).
    grant(&db, "u_app_member", &id, "app", json!({"storage": true})).await;
    let (status, body) = send(&router, "PUT", &write, Some(&member), Some(json!({"value": "v"}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Re-publishing the same snapshot never asks again.
    make_app(&router, &owner, &id, "app").await;
    let (status, body) = send(&router, "PUT", &write, Some(&member), Some(json!({"value": "w"}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, ctx) = send(&router, "GET", &url(&id, "context"), Some(&member), None).await;
    assert_eq!((ctx["app"]["tier"].as_str(), ctx["app"]["needs_consent"].as_bool()), (Some("app"), Some(false)));

    // A higher tier asks again, for the tier and what it adds.
    make_app(&router, &owner, &id, "full").await;
    let (status, body) = send(&router, "PUT", &write, Some(&member), Some(json!({"value": "v"}))).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("consent_required")), "{body}");
    assert_eq!(body["requested_tier"], "full");
    assert_eq!(body["added_capabilities"], json!(["connector:gh/list"]));
    let (_, ctx) = send(&router, "GET", &url(&id, "context"), Some(&member), None).await;
    assert_eq!(ctx["app"]["needs_consent"], true);

    // Browser tier reaches no viewer data at all.
    make_app(&router, &owner, &id, "browser").await;
    let (status, body) = send(&router, "GET", &url(&id, "storage/k"), Some(&owner), None).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("tier_too_low")), "{body}");
    assert_eq!((body["tier"].as_str(), body["required_tier"].as_str()), (Some("browser"), Some("app")));
}

#[tokio::test]
async fn app_install_rule_link_shared_capability_app_cannot_be_installed() {
    let router = setup().await;
    let owner = who("u_link_owner", Some("org_l"), None);
    let member = who("u_link_member", Some("org_l"), None);
    // An org member shares by link only when the org allows outside sharing.
    let admin = who("u_link_admin", Some("org_l"), Some("admin"));
    let (status, _) = send(&router, "PUT", "/api/v2/org/artifact-settings", Some(&admin), Some(json!({"external_sharing": true}))).await;
    assert_eq!(status, StatusCode::OK);
    let id = page(&router, &owner, json!({"storage": true})).await;
    share_with(&router, &owner, &id, "link", json!([])).await;

    // full can't go out by link.
    let (status, body) = send(
        &router, "PUT", &format!("/api/v2/artifacts/{id}/app"), Some(&owner),
        Some(json!({"name": "Tracker", "tier": "full", "install_scope": "link"})),
    )
    .await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("full_tier_not_allowed")), "{body}");

    let made = make_app(&router, &owner, &id, "app").await;
    let public = format!("/api/v2/public/apps/{}", made["app"]["slug"].as_str().unwrap());
    let (status, record) = send(&router, "GET", &public, None, None).await;
    assert_eq!(status, StatusCode::OK, "{record}");
    assert_eq!((record["install"].as_bool(), record["install_reason"].as_str()), (Some(false), Some("tier_too_high")));
    // The owner and the org install any tier.
    let (_, record) = send(&router, "GET", &public, Some(&owner), None).await;
    assert_eq!(record["install"], true);
    let (_, record) = send(&router, "GET", &public, Some(&member), None).await;
    assert_eq!((record["install"].as_bool(), record.get("install_reason")), (Some(true), None));
    // Browser tier installs from a link.
    make_app(&router, &owner, &id, "browser").await;
    let (_, record) = send(&router, "GET", &public, None, None).await;
    assert_eq!(record["install"], true);
}
