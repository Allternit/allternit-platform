//! Route tests for the artifacts v2 API (contract §3). Each test gets its
//! own Postgres schema (routes::test_support) with migration 085 applied.
//! Clerk-session callers (org, role, email) are stood in for by the
//! test-only `x-test-caller` header; API-token and device-token callers go
//! through the real resolvers.

use super::*;
use crate::routes::test_support::{test_state, MockGateway};
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;

async fn setup() -> (Router, PgPool) {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    sqlx::raw_sql(include_str!("../../migrations_pg/085_artifacts_v2.sql"))
        .execute(&state.db)
        .await
        .expect("085 applies");
    let db = state.db.clone();
    (routes().with_state(state), db)
}

fn who(id: &str, org: Option<&str>, email: Option<&str>) -> Value {
    json!({ "id": id, "org_id": org, "email": email })
}

fn request(method: &str, path: &str, caller: Option<&Value>, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(caller) = caller {
        builder = builder.header("x-test-caller", caller.to_string());
    }
    match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

async fn send(router: &Router, req: Request<Body>) -> (StatusCode, Value) {
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

async fn call(router: &Router, method: &str, path: &str, caller: &Value, body: Option<Value>) -> (StatusCode, Value) {
    send(router, request(method, path, Some(caller), body)).await
}

async fn create(router: &Router, caller: &Value, kind: &str, title: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        "/api/v2/artifacts",
        caller,
        Some(json!({ "kind": kind, "title": title, "body": format!("{title} v1") })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().to_string()
}

async fn share(router: &Router, owner: &Value, id: &str, visibility: &str, shares: Value) -> (StatusCode, Value) {
    call(
        router,
        "PUT",
        &format!("/api/v2/artifacts/{id}/sharing"),
        owner,
        Some(json!({ "visibility": visibility, "shares": shares })),
    )
    .await
}

#[tokio::test]
async fn every_endpoint_requires_authentication() {
    let (router, _db) = setup().await;
    for (method, path) in [
        ("GET", "/api/v2/artifacts"),
        ("POST", "/api/v2/artifacts"),
        ("GET", "/api/v2/artifacts/art_x"),
        ("PATCH", "/api/v2/artifacts/art_x"),
        ("DELETE", "/api/v2/artifacts/art_x"),
        ("GET", "/api/v2/artifacts/art_x/versions"),
        ("POST", "/api/v2/artifacts/art_x/versions"),
        ("GET", "/api/v2/artifacts/art_x/versions/1"),
        ("GET", "/api/v2/artifacts/art_x/sharing"),
        ("PUT", "/api/v2/artifacts/art_x/sharing"),
        ("GET", "/api/v2/org/artifact-settings"),
        ("PUT", "/api/v2/org/artifact-settings"),
    ] {
        let body = (method != "GET" && method != "DELETE").then(|| json!({}));
        let (status, _) = send(&router, request(method, path, None, body)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path} must require auth");
    }
}

#[tokio::test]
async fn create_get_list_and_idempotent_create() {
    let (router, _db) = setup().await;
    let alice = who("u_alice", None, Some("alice@example.com"));
    let bob = who("u_bob", None, None);

    let (status, created) = call(
        &router,
        "POST",
        "/api/v2/artifacts",
        &alice,
        Some(json!({
            "kind": "diagram", "title": "Flow", "icon": "chart",
            "body": "graph TD; A-->B", "origin": {"surface": "chat", "session_id": "s1"}
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("art_") && id.len() == 30, "{id}");
    assert_eq!(created["my_access"], "owner");
    assert_eq!(created["runtime_version"], 2);
    assert_eq!(created["visibility"], "private");
    assert_eq!(created["current_version"], 1);
    assert_eq!(created["version"]["body"], "graph TD; A-->B");
    assert_eq!(created["version"]["body_format"], "text/vnd.mermaid", "kind default");
    assert_eq!(created["version"]["author_id"], "u_alice");

    let (status, got) = call(&router, "GET", &format!("/api/v2/artifacts/{id}"), &alice, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["title"], "Flow");
    assert_eq!(got["origin"]["session_id"], "s1", "owner sees the full origin");

    // Bob can't see it (404, not 403).
    let (status, _) = call(&router, "GET", &format!("/api/v2/artifacts/{id}"), &bob, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // List + filters + pagination.
    create(&router, &alice, "code", "Script").await;
    create(&router, &alice, "page", "Landing").await;
    create(&router, &bob, "page", "Bob's page").await;
    let (_, list) = call(&router, "GET", "/api/v2/artifacts", &alice, None).await;
    let titles: Vec<_> = list["items"].as_array().unwrap().iter().map(|i| i["title"].clone()).collect();
    assert_eq!(titles, [json!("Landing"), json!("Script"), json!("Flow")], "newest first, own only");
    let (_, list) = call(&router, "GET", "/api/v2/artifacts?kind=code", &alice, None).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    let (_, list) = call(&router, "GET", "/api/v2/artifacts?q=flo", &alice, None).await;
    assert_eq!(list["items"][0]["id"], json!(id));
    let (_, list) = call(&router, "GET", "/api/v2/artifacts?origin=chat", &alice, None).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    let (_, page1) = call(&router, "GET", "/api/v2/artifacts?limit=2", &alice, None).await;
    assert_eq!(page1["items"].as_array().unwrap().len(), 2);
    let cursor = page1["next_cursor"].as_str().unwrap();
    let (_, page2) = call(&router, "GET", &format!("/api/v2/artifacts?limit=2&cursor={cursor}"), &alice, None).await;
    assert_eq!(page2["items"].as_array().unwrap().len(), 1);
    assert_eq!(page2["items"][0]["title"], "Flow");
    assert!(page2["next_cursor"].is_null());
    let (status, _) = call(&router, "GET", "/api/v2/artifacts?scope=everyone", &alice, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Idempotent create with a client id; someone else's id is a conflict.
    let legacy = json!({
        "id": "art_legacy_canvas_42", "kind": "page", "title": "Old canvas", "body": "<p>hi</p>",
        "runtime_version": 1, "origin": {"legacy_source": "canvas", "legacy_id": "42"}
    });
    let (status, first) = call(&router, "POST", "/api/v2/artifacts", &alice, Some(legacy.clone())).await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    assert_eq!(first["runtime_version"], 1);
    let (status, again) = call(&router, "POST", "/api/v2/artifacts", &alice, Some(legacy.clone())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["id"], "art_legacy_canvas_42");
    assert_eq!(again["current_version"], 1);
    let (status, body) = call(&router, "POST", "/api/v2/artifacts", &bob, Some(legacy)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "id_conflict");

    // Legacy needs origin.legacy_source; bad ids are refused.
    let (status, body) = call(
        &router, "POST", "/api/v2/artifacts", &alice,
        Some(json!({"kind": "page", "title": "x", "body": "", "runtime_version": 1})),
    ).await;
    assert_eq!((status, body["code"].clone()), (StatusCode::UNPROCESSABLE_ENTITY, json!("legacy_source_required")));
    let (status, _) = call(
        &router, "POST", "/api/v2/artifacts", &alice,
        Some(json!({"id": "doc/../x", "kind": "page", "title": "x", "body": ""})),
    ).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn access_matrix_and_version_visibility() {
    let (router, _db) = setup().await;
    let owner = who("u_owner", Some("org_a"), Some("owner@a.com"));
    let editor = who("u_editor", Some("org_a"), None);
    let commenter = who("u_commenter", Some("org_a"), None);
    let viewer = who("u_viewer", Some("org_z"), Some("viewer@z.com"));
    let member = who("u_member", Some("org_a"), None);
    let stranger = who("u_stranger", Some("org_z"), None);
    let id = create(&router, &owner, "page", "Plan").await;
    let path = format!("/api/v2/artifacts/{id}");

    // Org allows outside invites so the email share is accepted.
    let admin = json!({"id": "u_owner", "org_id": "org_a", "org_role": "org:admin"});
    let (status, _) = call(&router, "PUT", "/api/v2/org/artifact-settings", &admin, Some(json!({"outside_invites": true}))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = share(
        &router, &owner, &id, "people",
        json!([
            {"principal_type": "user", "principal_id": "u_editor", "level": "edit"},
            {"principal_type": "user", "principal_id": "u_commenter", "level": "comment"},
            {"principal_type": "email", "principal_id": "Viewer@Z.com", "level": "view"},
        ]),
    ).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    for (caller, expected) in [
        (&owner, Some("owner")),
        (&editor, Some("edit")),
        (&commenter, Some("comment")),
        (&viewer, Some("view")),
        (&member, None),
        (&stranger, None),
    ] {
        let (status, body) = call(&router, "GET", &path, caller, None).await;
        match expected {
            Some(level) => {
                assert_eq!(status, StatusCode::OK, "{caller}");
                assert_eq!(body["my_access"], level, "{caller}");
            }
            None => assert_eq!(status, StatusCode::NOT_FOUND, "{caller}"),
        }
    }

    // Edit actions.
    let (status, body) = call(&router, "PATCH", &path, &editor, Some(json!({"title": "Plan v2"}))).await;
    assert_eq!((status, body["title"].clone()), (StatusCode::OK, json!("Plan v2")));
    let (status, _) = call(&router, "PATCH", &path, &commenter, Some(json!({"title": "nope"}))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(&router, "POST", &format!("{path}/versions"), &commenter, Some(json!({"base_version": 1, "body": "x"}))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(&router, "PATCH", &path, &stranger, Some(json!({"title": "nope"}))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Owner-only actions are 403 for editors (rule 6).
    for (method, p, body) in [
        ("PATCH", path.clone(), Some(json!({"shared_version": 1}))),
        ("PATCH", path.clone(), Some(json!({"capabilities": {"storage": true}}))),
        ("DELETE", path.clone(), None),
        ("GET", format!("{path}/sharing"), None),
        ("PUT", format!("{path}/sharing"), Some(json!({"visibility": "private"}))),
    ] {
        let (status, _) = call(&router, method, &p, &editor, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {p}");
    }

    // Versions: owner pins v1 as the shared version, then saves v2.
    let (status, _) = call(&router, "POST", &format!("{path}/versions"), &editor, Some(json!({"base_version": 1, "body": "Plan v2 body", "author": "assistant"}))).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = call(&router, "PATCH", &path, &owner, Some(json!({"shared_version": 1}))).await;
    assert_eq!(status, StatusCode::OK);
    let (_, as_viewer) = call(&router, "GET", &path, &viewer, None).await;
    assert_eq!(as_viewer["version"]["version"], 1);
    let (_, as_editor) = call(&router, "GET", &path, &editor, None).await;
    assert_eq!(as_editor["version"]["version"], 2);
    assert_eq!(as_editor["version"]["author_id"], "assistant");
    let (_, versions) = call(&router, "GET", &format!("{path}/versions"), &viewer, None).await;
    assert_eq!(versions["items"].as_array().unwrap().len(), 1);
    let (_, versions) = call(&router, "GET", &format!("{path}/versions"), &editor, None).await;
    assert_eq!(versions["items"].as_array().unwrap().len(), 2);
    let (status, _) = call(&router, "GET", &format!("{path}/versions/2"), &viewer, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, v1) = call(&router, "GET", &format!("{path}/versions/1"), &viewer, None).await;
    assert_eq!((status, v1["body"].clone()), (StatusCode::OK, json!("Plan v1")));
    let (status, _) = call(&router, "PATCH", &path, &owner, Some(json!({"shared_version": 9}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    // Non-owners see a trimmed origin.
    let (_, as_editor) = call(&router, "GET", &path, &editor, None).await;
    assert!(as_editor["origin"].as_object().unwrap().is_empty());

    // The outside invite was accepted on first open.
    let (_, sharing) = call(&router, "GET", &format!("{path}/sharing"), &owner, None).await;
    let email_row = sharing["shares"].as_array().unwrap().iter().find(|s| s["principal_type"] == "email").unwrap().clone();
    assert_eq!(email_row["principal_id"], "viewer@z.com");
    assert!(email_row["accepted_at"].is_string() && email_row["expires_at"].is_null(), "{email_row}");

    // Org visibility: same-org members get view; listed under scope=shared.
    let (status, _) = share(&router, &owner, &id, "org", json!([{"principal_type": "user", "principal_id": "u_editor", "level": "edit"}])).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call(&router, "GET", &path, &member, None).await;
    assert_eq!((status, body["my_access"].clone()), (StatusCode::OK, json!("view")));
    let (status, _) = call(&router, "GET", &path, &stranger, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&router, "GET", &path, &commenter, None).await;
    assert_eq!(status, StatusCode::OK, "commenter's row was dropped but org view remains");
    let (_, shared) = call(&router, "GET", "/api/v2/artifacts?scope=shared", &member, None).await;
    assert_eq!(shared["items"][0]["id"], json!(id));
    let (_, mine) = call(&router, "GET", "/api/v2/artifacts?scope=mine", &member, None).await;
    assert!(mine["items"].as_array().unwrap().is_empty());
    let (_, shared) = call(&router, "GET", "/api/v2/artifacts?scope=shared", &stranger, None).await;
    assert!(shared["items"].as_array().unwrap().is_empty());

    // Group share: the whole of another org.
    let (status, _) = share(&router, &owner, &id, "people", json!([{"principal_type": "group", "principal_id": "org_z", "level": "comment"}])).await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = call(&router, "GET", &path, &stranger, None).await;
    assert_eq!(body["my_access"], "comment");
    let (status, _) = call(&router, "GET", &path, &member, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "org view ended with visibility people");

    // Delete: owner only, permanent.
    let (status, _) = call(&router, "DELETE", &path, &owner, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(&router, "GET", &path, &owner, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn stale_append_is_409_with_current_version() {
    let (router, _db) = setup().await;
    let alice = who("u_alice", None, None);
    let id = create(&router, &alice, "code", "main.rs").await;
    let path = format!("/api/v2/artifacts/{id}/versions");
    let (status, body) = call(&router, "POST", &path, &alice, Some(json!({"base_version": 1, "body": "fn main() {}", "meta": {"language": "rust"}}))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["current_version"], 2);
    assert_eq!(body["version"]["body_format"], "text/plain", "carried from v1");
    assert_eq!(body["version"]["meta"]["language"], "rust");
    let (status, body) = call(&router, "POST", &path, &alice, Some(json!({"base_version": 1, "body": "stale"}))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "stale_version");
    assert_eq!(body["current_version"], 2);
    let (_, versions) = call(&router, "GET", &path, &alice, None).await;
    assert_eq!(versions["items"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn body_over_16_mib_is_413() {
    let (router, _db) = setup().await;
    let alice = who("u_alice", None, None);
    let big = "x".repeat(MAX_BODY_BYTES + 1);
    let (status, body) = call(&router, "POST", "/api/v2/artifacts", &alice, Some(json!({"kind": "code", "title": "big", "body": big}))).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body["code"], "body_too_large");
}

#[tokio::test]
async fn link_sharing_and_public_route() {
    let (router, _db) = setup().await;
    let alice = who("u_alice", None, None);
    let id = create(&router, &alice, "page", "Public page").await;
    let public = format!("/api/v2/public/artifacts/{id}");

    let (status, _) = send(&router, request("GET", &public, None, None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "private until shared");
    let (status, sharing) = share(&router, &alice, &id, "link", json!([])).await;
    assert_eq!(status, StatusCode::OK, "{sharing}");
    assert!(sharing["link_url"].as_str().unwrap().ends_with(&id));
    assert_eq!(sharing["policy"]["link_allowed"], true);
    let (status, body) = send(&router, request("GET", &public, None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["body"], "Public page v1");
    assert_eq!(body["kind"], "page");
    // A link doesn't give signed-in strangers access through the private API.
    let (status, _) = call(&router, "GET", &format!("/api/v2/artifacts/{id}"), &who("u_x", None, None), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // AI/connectors can't be turned on while link-shared ...
    let (status, body) = call(&router, "PATCH", &format!("/api/v2/artifacts/{id}"), &alice, Some(json!({"capabilities": {"ai": true}}))).await;
    assert_eq!((status, body["code"].clone()), (StatusCode::UNPROCESSABLE_ENTITY, json!("link_not_allowed")));
    // ... and an artifact that uses them can't be link-shared.
    let ai = create(&router, &alice, "page", "AI page").await;
    let (status, _) = call(&router, "PATCH", &format!("/api/v2/artifacts/{ai}"), &alice, Some(json!({"capabilities": {"ai": true}}))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = share(&router, &alice, &ai, "link", json!([])).await;
    assert_eq!((status, body["code"].clone()), (StatusCode::UNPROCESSABLE_ENTITY, json!("link_not_allowed")));
    let conn = create(&router, &alice, "page", "Connector page").await;
    call(&router, "PATCH", &format!("/api/v2/artifacts/{conn}"), &alice, Some(json!({"capabilities": {"connectors": [{"connector": "github", "tools": ["list_issues"]}]}}))).await;
    let (status, _) = share(&router, &alice, &conn, "link", json!([])).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Org artifact: link needs the org's external sharing (admin only).
    let member = who("u_m", Some("org_a"), None);
    let admin = json!({"id": "u_admin", "org_id": "org_a", "org_role": "admin"});
    let org_page = create(&router, &member, "page", "Org page").await;
    let (status, body) = share(&router, &member, &org_page, "link", json!([])).await;
    assert_eq!((status, body["code"].clone()), (StatusCode::UNPROCESSABLE_ENTITY, json!("link_not_allowed")));
    let (status, _) = call(&router, "PUT", "/api/v2/org/artifact-settings", &member, Some(json!({"external_sharing": true}))).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "members can't change org settings");
    let (status, settings) = call(&router, "PUT", "/api/v2/org/artifact-settings", &admin, Some(json!({"external_sharing": true}))).await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert_eq!(settings["external_sharing"], true);
    assert_eq!(settings["updated_by"], "u_admin");
    let (status, _) = share(&router, &member, &org_page, "link", json!([])).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(&router, request("GET", &format!("/api/v2/public/artifacts/{org_page}"), None, None)).await;
    assert_eq!(status, StatusCode::OK);
    // Turning external sharing off closes existing links at once.
    call(&router, "PUT", "/api/v2/org/artifact-settings", &admin, Some(json!({"external_sharing": false}))).await;
    let (status, _) = send(&router, request("GET", &format!("/api/v2/public/artifacts/{org_page}"), None, None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // ... unless the artifact is individually allowed.
    call(&router, "PUT", "/api/v2/org/artifact-settings", &admin, Some(json!({"allowed_external": [org_page]}))).await;
    let (status, _) = send(&router, request("GET", &format!("/api/v2/public/artifacts/{org_page}"), None, None)).await;
    assert_eq!(status, StatusCode::OK);
    let (_, got) = call(&router, "GET", "/api/v2/org/artifact-settings", &member, None).await;
    assert_eq!(got["allowed_external"], json!([org_page]));
    assert_eq!(got["can_edit"], false);
}

#[tokio::test]
async fn sharing_rules() {
    let (router, _db) = setup().await;
    let alice = who("u_alice", None, None);
    let doc = create(&router, &alice, "doc", "Doc").await;
    let (status, body) = share(&router, &alice, &doc, "people", json!([{"principal_type": "user", "principal_id": "u_b", "level": "comment"}])).await;
    assert_eq!((status, body["code"].clone()), (StatusCode::UNPROCESSABLE_ENTITY, json!("doc_level_not_allowed")));
    let (status, body) = share(&router, &alice, &doc, "people", json!([{"principal_type": "email", "principal_id": "b@x.com", "level": "view"}])).await;
    assert_eq!((status, body["code"].clone()), (StatusCode::UNPROCESSABLE_ENTITY, json!("doc_email_not_allowed")));
    let (status, _) = share(&router, &alice, &doc, "people", json!([{"principal_type": "user", "principal_id": "u_b", "level": "edit"}])).await;
    assert_eq!(status, StatusCode::OK);

    let page = create(&router, &alice, "page", "Page").await;
    let fifty_one: Vec<Value> = (0..51)
        .map(|i| json!({"principal_type": "email", "principal_id": format!("p{i}@x.com"), "level": "view"}))
        .collect();
    let (status, body) = share(&router, &alice, &page, "people", json!(fifty_one)).await;
    assert_eq!((status, body["code"].clone()), (StatusCode::UNPROCESSABLE_ENTITY, json!("too_many_outside_invites")));
    let (status, body) = share(&router, &alice, &page, "people", json!(fifty_one[..50])).await;
    assert_eq!(status, StatusCode::OK);
    let expires = body["shares"][0]["expires_at"].as_str().expect("outside invite expires");
    let expires = DateTime::parse_from_rfc3339(expires).unwrap().with_timezone(&Utc);
    let days = (expires - Utc::now()).num_hours() as f64 / 24.0;
    assert!((29.9..=30.0).contains(&days), "expires in ~30 days, got {days}");
    let (status, body) = share(&router, &alice, &page, "private", json!([{"principal_type": "user", "principal_id": "u_b", "level": "view"}])).await;
    assert_eq!((status, body["code"].clone()), (StatusCode::UNPROCESSABLE_ENTITY, json!("private_has_no_shares")));
    let (status, body) = share(&router, &alice, &page, "org", json!([])).await;
    assert_eq!((status, body["code"].clone()), (StatusCode::UNPROCESSABLE_ENTITY, json!("org_required")));
    let (status, body) = share(&router, &alice, &page, "private", json!([])).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["shares"].as_array().unwrap().is_empty(), "private drops every share");

    // Orgs refuse outside invites unless the admin allows them.
    let member = who("u_m", Some("org_a"), None);
    let org_page = create(&router, &member, "page", "Org page").await;
    let (status, body) = share(&router, &member, &org_page, "people", json!([{"principal_type": "email", "principal_id": "b@x.com", "level": "view"}])).await;
    assert_eq!((status, body["code"].clone()), (StatusCode::UNPROCESSABLE_ENTITY, json!("outside_invites_not_allowed")));
    assert_eq!(body["code"], "outside_invites_not_allowed");
}

#[tokio::test]
async fn api_tokens_and_device_tokens_resolve_to_their_user() {
    let (router, db) = setup().await;
    for (token, user, perms) in [
        ("allternit_test_full", "u_token", r#"["*"]"#),
        ("allternit_test_inference", "u_token", r#"["inference"]"#),
    ] {
        sqlx::query("INSERT INTO api_tokens (id, token_hash, name, user_id, permissions) VALUES ($1, $2, 'test', $3, $4)")
            .bind(token)
            .bind(crate::auth::middleware::hash_api_token(token))
            .bind(user)
            .bind(perms)
            .execute(&db)
            .await
            .unwrap();
    }
    // The grace-window lookup reads these (migrations_pg/001 has them).
    sqlx::query("ALTER TABLE runtime_devices ADD COLUMN previous_credential_hash TEXT, ADD COLUMN previous_credential_expires_at TIMESTAMPTZ")
        .execute(&db)
        .await
        .unwrap();
    let device_token = "allternit_runtime_artifacts_test";
    sqlx::query(
        "INSERT INTO runtime_devices (id, user_id, name, status, credential_expires_at, credential_hash) \
         VALUES ('rt_art', 'u_device_owner', 'laptop', 'online', '2999-01-01', $1)",
    )
    .bind(crate::routes::runtime_pairing::sha256_hex(device_token.as_bytes()))
    .execute(&db)
    .await
    .unwrap();

    let bearer = |method: &str, path: &str, token: &str, body: Option<Value>| {
        let mut req = request(method, path, None, body);
        req.headers_mut().insert(header::AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {token}")).unwrap());
        req
    };
    let new = json!({"kind": "code", "title": "t", "body": "x"});
    let (status, body) = send(&router, bearer("POST", "/api/v2/artifacts", "allternit_test_full", Some(new.clone()))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["owner"]["id"], "u_token");
    let (status, _) = send(&router, bearer("GET", "/api/v2/artifacts", "allternit_test_inference", None)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "tokens need the compute scope");

    let (status, body) = send(&router, bearer("POST", "/api/v2/artifacts", device_token, Some(new))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["owner"]["id"], "u_device_owner");
    let id = body["id"].as_str().unwrap().to_string();
    let (status, body) = send(&router, bearer("GET", &format!("/api/v2/artifacts/{id}"), device_token, None)).await;
    assert_eq!((status, body["my_access"].clone()), (StatusCode::OK, json!("owner")));
    let (status, _) = send(&router, bearer("GET", "/api/v2/artifacts", "allternit_runtime_wrong", None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    sqlx::query("UPDATE runtime_devices SET revoked_at = now() WHERE id = 'rt_art'").execute(&db).await.unwrap();
    let (status, _) = send(&router, bearer("GET", "/api/v2/artifacts", device_token, None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "revoked devices lose access");
}
