//! Route tests for the artifacts v2 API (contract §3). Each test gets its
//! own Postgres schema (routes::test_support) with migrations 085, 109, 161,
//! 163 and 166 applied.
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
    sqlx::raw_sql(include_str!("../../migrations_pg/109_artifact_library_archive.sql"))
        .execute(&state.db).await.expect("109 applies");
    sqlx::raw_sql(include_str!("../../migrations_pg/161_apps_mcp_2026.sql"))
        .execute(&state.db).await.expect("161 applies");
    sqlx::raw_sql(include_str!("../../migrations_pg/166_plan_artifacts.sql"))
        .execute(&state.db).await.expect("166 applies");
    // cloud_skills: an app's skill_ref must name one of the publisher's skills.
    sqlx::raw_sql(&include_str!("../../migrations_pg/163_cloud_skills.sql").replace("public.", ""))
        .execute(&state.db).await.expect("163 applies");
    let db = state.db.clone();
    (routes().with_state(state), db)
}

/// A cloud skill `owner` owns, written straight into the table.
async fn seed_skill(db: &PgPool, owner: &str, name: &str, body: &str) {
    sqlx::query("INSERT INTO cloud_skills (id, owner_user_id, name, description, skill_md) VALUES ($1, $2, $3, $4, $5)")
        .bind(format!("skl_{owner}_{name}"))
        .bind(owner)
        .bind(name)
        .bind(format!("{name} skill"))
        .bind(format!("---\nname: {name}\ndescription: {name} skill\n---\n\n{body}\n"))
        .execute(db)
        .await
        .unwrap();
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
async fn library_archive_is_per_viewer_and_preserves_access() {
    let (router, db) = setup().await;
    let owner = who("archive_owner", Some("org_archive"), None);
    let viewer = who("archive_viewer", Some("org_archive"), None);
    let outsider = who("archive_outsider", None, None);
    let id = create(&router, &owner, "doc", "Keep this document").await;
    let (status, _) = share(&router, &owner, &id, "org", json!([])).await;
    assert_eq!(status, StatusCode::OK);
    let library = format!("/api/v2/artifacts/{id}/library");
    for _ in 0..2 {
        let (status, _) = call(&router, "PUT", &library, &viewer, Some(json!({"archived": true}))).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "archive is idempotent and available to viewers");
    }
    let (status, saved) = call(&router, "GET", "/api/v2/artifacts", &viewer, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(saved["items"].as_array().unwrap().is_empty());
    let (_, archived) = call(&router, "GET", "/api/v2/artifacts?archived=true&q=Keep&kind=doc", &viewer, None).await;
    assert_eq!(archived["items"][0]["id"], json!(id));
    assert_eq!(archived["archive_supported"], json!(true));
    let (_, owner_saved) = call(&router, "GET", "/api/v2/artifacts", &owner, None).await;
    assert_eq!(owner_saved["items"][0]["id"], json!(id));
    let (status, _) = call(&router, "GET", &format!("/api/v2/artifacts/{id}"), &viewer, None).await;
    assert_eq!(status, StatusCode::OK, "archiving does not revoke read access");
    let (status, _) = call(&router, "PUT", &library, &outsider, Some(json!({"archived": true}))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&router, "PUT", &library, &viewer, Some(json!({"archived": false}))).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, restored) = call(&router, "GET", "/api/v2/artifacts", &viewer, None).await;
    assert_eq!(restored["items"][0]["id"], json!(id));
    // Revoking the share also hides an archived copy; the archive is never an access grant.
    call(&router, "PUT", &library, &viewer, Some(json!({"archived": true}))).await;
    share(&router, &owner, &id, "private", json!([])).await;
    let (_, revoked) = call(&router, "GET", "/api/v2/artifacts?archived=true", &viewer, None).await;
    assert!(revoked["items"].as_array().unwrap().is_empty());
    call(&router, "DELETE", &format!("/api/v2/artifacts/{id}"), &owner, None).await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM artifact_library_archives WHERE artifact_id = $1")
        .bind(&id).fetch_one(&db).await.unwrap();
    assert_eq!(count, 0, "delete cascades to library metadata");
}

#[tokio::test]
async fn library_archive_authenticates_and_validates_the_request() {
    let (router, _) = setup().await;
    let owner = who("archive_owner", None, None);
    let id = create(&router, &owner, "doc", "Archive validation").await;
    let path = format!("/api/v2/artifacts/{id}/library");
    let (status, _) = send(&router, request("PUT", &path, None, Some(json!({"archived": true})))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    for body in [json!({}), json!({"archived": "true"}), json!({"archived": true, "user_id": "someone_else"})] {
        let (status, _) = call(&router, "PUT", &path, &owner, Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
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

#[tokio::test]
async fn make_an_app_mints_the_slug_once_and_the_public_record_reads_it() {
    let (router, db) = setup().await;
    let owner = who("u_app_owner", None, None);
    seed_skill(&db, "u_app_owner", "tips", "Split the bill.").await;
    let id = create(&router, &owner, "page", "Tip calculator").await;
    let app_path = format!("/api/v2/artifacts/{id}/app");

    let (status, body) = call(&router, "GET", &app_path, &owner, None).await;
    assert_eq!((status, body["app"].clone()), (StatusCode::OK, Value::Null), "not an app yet");

    let (status, made) = call(
        &router, "PUT", &app_path, &owner,
        Some(json!({"name": " Tips ", "icon": "calculator", "tier": "browser", "skill_ref": "skill://tips"})),
    ).await;
    assert_eq!(status, StatusCode::OK, "{made}");
    let slug = made["app"]["slug"].as_str().unwrap().to_string();
    assert_eq!(slug.len(), 16);
    assert!(slug.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()), "{slug} is DNS-safe");
    assert_eq!(made["app"]["name"], "Tips");
    assert_eq!(made["app"]["pinned_version"], 1);
    assert_eq!(made["app"]["install_scope"], "private", "defaults to the artifact's visibility");
    assert_eq!(made["app"]["capability_snapshot"], json!({}));

    // Re-publishing keeps the slug and re-takes the capability snapshot.
    sqlx::query("UPDATE artifacts SET capabilities = '{\"storage\": true}' WHERE id = $1")
        .bind(&id).execute(&db).await.unwrap();
    let (status, again) = call(&router, "PUT", &app_path, &owner, Some(json!({"name": "Tips", "tier": "app"}))).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["app"]["slug"], json!(slug), "the slug is minted once");
    assert_eq!(again["app"]["tier"], "app");
    assert_eq!(again["app"]["capability_snapshot"], json!({"storage": true}));

    let public = format!("/api/v2/public/apps/{slug}");
    let (status, _) = send(&router, request("GET", &public, None, None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a private artifact's app is not public");
    let (status, _) = share(&router, &owner, &id, "people", json!([])).await;
    assert_eq!(status, StatusCode::OK);
    let (status, record) = send(&router, request("GET", &public, None, None)).await;
    assert_eq!(status, StatusCode::OK, "{record}");
    assert_eq!(record["slug"], json!(slug));
    assert_eq!(record["name"], "Tips");
    assert_eq!(record["tier"], "app");
    assert_eq!(record["install"], false, "only browser-tier apps install from the record");
    assert!(record.get("body").is_none(), "the record never carries the artifact body");
    call(&router, "PUT", &app_path, &owner, Some(json!({"name": "Tips", "tier": "browser"}))).await;
    let (_, record) = send(&router, request("GET", &public, None, None)).await;
    assert_eq!(record["install"], true);

    let (status, _) = send(&router, request("GET", "/api/v2/public/apps/nosuchapp0000000", None, None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(&router, request("GET", "/api/v2/public/apps/Bad.Slug", None, None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn app_skill_ref_must_resolve_for_the_publisher() {
    let (router, db) = setup().await;
    let owner = who("u_ref_owner", None, None);
    seed_skill(&db, "u_ref_owner", "tips", "Split the bill.").await;
    seed_skill(&db, "u_ref_other", "secret", "Not yours.").await;
    let id = create(&router, &owner, "page", "Ref check").await;
    let app_path = format!("/api/v2/artifacts/{id}/app");
    for bad in ["skill://nope", "skill://user/secret", "skill://secret/SKILL.md", "skill://app/x/tips", "skill://user/../tips"] {
        let (status, got) = call(&router, "PUT", &app_path, &owner, Some(json!({"name": "X", "tier": "browser", "skill_ref": bad}))).await;
        assert_eq!((status, got["error"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("skill_ref_not_found")), "{bad} -> {got}");
    }
    for good in ["skill://user/tips", "skill://tips", "skill://user/tips/SKILL.md"] {
        let (status, got) = call(&router, "PUT", &app_path, &owner, Some(json!({"name": "X", "tier": "browser", "skill_ref": good}))).await;
        assert_eq!(status, StatusCode::OK, "{good} -> {got}");
        assert_eq!(got["app"]["skill_ref"], good);
    }
}

/// B sees A's app skill only through a live install; uninstall removes it;
/// a share-link install starts switched off, the owner's own install on.
#[tokio::test]
async fn app_skills_follow_live_installs() {
    use crate::routes::cloud_skills::PgSkillStore;
    use crate::routes::mcp_edge_skills::{available, read, resources, SkillCaller};
    let (router, db) = setup().await;
    let a = who("u_appsk_a", None, None);
    seed_skill(&db, "u_appsk_a", "tips", "A's tip steps.").await;
    let id = create(&router, &a, "page", "Tips app").await;
    let app_path = format!("/api/v2/artifacts/{id}/app");
    let publish = |scope: &str| json!({"name": "Tips", "tier": "browser", "install_scope": scope, "skill_ref": "skill://user/tips"});
    let (status, made) = call(&router, "PUT", &app_path, &a, Some(publish("people"))).await;
    assert_eq!(status, StatusCode::OK, "{made}");
    let slug = made["app"]["slug"].as_str().unwrap().to_string();
    let uri = format!("skill://app/{slug}/tips/SKILL.md");

    let store = PgSkillStore { db: &db };
    let b = SkillCaller { user_id: "u_appsk_b", personal: true };
    let uris = |list: Vec<Value>| list.iter().map(|r| r["uri"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert!(!uris(resources(&store, &b).await).contains(&uri), "no install, not listed");
    assert!(read(&store, &b, &uri).await.is_none(), "no install, not readable");
    assert!(read(&store, &b, "skill://user/tips/SKILL.md").await.is_none(), "B never reads A's user skill");

    let install = |user: &'static str| {
        sqlx::query("INSERT INTO app_installs (id, user_id, app_kind, app_ref, granted_tier) VALUES ($1, $2, 'artifact', $3, 'browser')")
            .bind(format!("ain_{user}"))
            .bind(user)
            .bind(id.clone())
    };
    install("u_appsk_b").execute(&db).await.unwrap();
    assert!(uris(resources(&store, &b).await).contains(&uri), "the MCP edge lists it for the installer");
    let text = read(&store, &b, &uri).await.expect("installer reads the app skill").1;
    assert!(text.contains("name: tips") && text.contains("A's tip steps."), "{text}");
    assert!(read(&store, &b, "skill://user/tips/SKILL.md").await.is_none(), "the install grants the app copy only");
    let item = |list: Vec<Value>| list.into_iter().find(|i| i["uri"] == json!(uri));
    let got = item(available(&store, &b).await).expect("in available");
    assert_eq!((got["source"].as_str(), got["from"].as_str(), got["defaultEnabled"].as_bool()), (Some("app"), Some("Tips"), Some(true)));

    let (status, _) = call(&router, "PUT", &app_path, &a, Some(publish("link"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(item(available(&store, &b).await).unwrap()["defaultEnabled"], false, "a share-link install starts off");
    install("u_appsk_a").execute(&db).await.unwrap();
    let mine = SkillCaller { user_id: "u_appsk_a", personal: true };
    assert_eq!(item(available(&store, &mine).await).unwrap()["defaultEnabled"], true, "the owner's own app stays on");

    sqlx::query("DELETE FROM app_installs WHERE user_id = 'u_appsk_b'").execute(&db).await.unwrap();
    assert!(item(available(&store, &b).await).is_none(), "uninstall removes it");
    assert!(read(&store, &b, &uri).await.is_none());
}

#[tokio::test]
async fn only_the_owner_makes_an_app_and_fields_are_checked() {
    let (router, _db) = setup().await;
    let owner = who("u_app_owner2", None, None);
    let editor = who("u_app_editor", None, None);
    let outsider = who("u_app_outsider", None, None);
    let id = create(&router, &owner, "page", "Owner only").await;
    share(&router, &owner, &id, "people", json!([{"principal_type": "user", "principal_id": "u_app_editor", "level": "edit"}])).await;
    let app_path = format!("/api/v2/artifacts/{id}/app");
    let good = json!({"name": "Mine", "tier": "browser"});

    let (status, _) = call(&router, "PUT", &app_path, &editor, Some(good.clone())).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(&router, "PUT", &app_path, &outsider, Some(good.clone())).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&router, "GET", &app_path, &editor, None).await;
    assert_eq!(status, StatusCode::OK, "anyone who can see the artifact sees its app package");
    let (status, _) = send(&router, request("PUT", &app_path, None, Some(good))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    for (body, code) in [
        (json!({"name": "", "tier": "browser"}), "invalid_app_name"),
        (json!({"name": "X", "tier": "admin"}), "invalid_tier"),
        (json!({"name": "X", "tier": "app", "install_scope": "world"}), "invalid_install_scope"),
        (json!({"name": "X", "tier": "app", "tools": {"a": 1}}), "invalid_tools"),
        (json!({"name": "X", "tier": "app", "skill_ref": "https://x"}), "invalid_skill_ref"),
        (json!({"name": "X", "tier": "app", "pinned_version": 2}), "invalid_pinned_version"),
    ] {
        let (status, got) = call(&router, "PUT", &app_path, &owner, Some(body.clone())).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body} -> {got}");
        assert_eq!(got["error"], code, "{got}");
    }
    let (status, _) = call(&router, "PUT", &app_path, &owner, Some(json!({"name": "X", "tier": "app", "owner": "me"}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "unknown fields are refused");
}

#[tokio::test]
async fn put_app_accepts_export_mcp_and_keeps_it_when_omitted() {
    let (router, db) = setup().await;
    let owner = who("u_app_export_mcp", None, None);
    let id = create(&router, &owner, "page", "Exports").await;
    let app_path = format!("/api/v2/artifacts/{id}/app");
    sqlx::query("UPDATE artifacts SET capabilities = '{\"storage\": true}' WHERE id = $1")
        .bind(&id).execute(&db).await.unwrap();

    let (status, made) = call(&router, "PUT", &app_path, &owner,
        Some(json!({"name": "Exports", "tier": "app", "export_mcp": true}))).await;
    assert_eq!(status, StatusCode::OK, "{made}");
    assert_eq!(made["app"]["capability_snapshot"], json!({"storage": true, "export_mcp": true}));

    let (_, kept) = call(&router, "PUT", &app_path, &owner, Some(json!({"name": "Exports", "tier": "app"}))).await;
    assert_eq!(kept["app"]["capability_snapshot"]["export_mcp"], json!(true), "omitted keeps it");

    let (_, off) = call(&router, "PUT", &app_path, &owner,
        Some(json!({"name": "Exports", "tier": "app", "export_mcp": false}))).await;
    assert_eq!(off["app"]["capability_snapshot"]["export_mcp"], json!(false));

    let (status, _) = call(&router, "PUT", &app_path, &owner,
        Some(json!({"name": "Exports", "tier": "app", "snapshot": {"export_mcp": true}}))).await;
    assert!(status.is_client_error(), "unknown fields stay refused");
}

async fn create_plan(router: &Router, caller: &Value, session: &str, body: &str) -> (StatusCode, Value) {
    call(
        router,
        "POST",
        "/api/v2/artifacts",
        caller,
        Some(json!({
            "id": format!("art_plan_{session}"),
            "kind": "plan",
            "title": "Q4 launch plan",
            "origin": {"surface": "gizzi", "session_id": session},
            "body": body,
        })),
    )
    .await
}

fn item_ids(list: &Value) -> Vec<String> {
    list["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn plans_stay_out_of_the_default_list_and_keep_one_row_per_chat() {
    let (router, _db) = setup().await;
    let owner = who("u_plan_owner", Some("org_plan"), None);
    let doc = create(&router, &owner, "doc", "Q4 launch notes").await;

    let (status, plan) = create_plan(&router, &owner, "ses_1", "# Q4 launch\n\n1. Ship it").await;
    assert_eq!(status, StatusCode::CREATED, "{plan}");
    let plan_id = "art_plan_ses_1";
    assert_eq!(plan["id"], json!(plan_id));
    assert_eq!(plan["version"]["body_format"], json!("text/markdown"), "plans default to Markdown");
    assert_eq!(plan["plan"]["status"], json!("open"), "a new plan reads as open");

    // D11: the default list (and a title search, and an org member's list)
    // never shows a plan; kind=plan does.
    let (_, all) = call(&router, "GET", "/api/v2/artifacts", &owner, None).await;
    assert_eq!(item_ids(&all), vec![doc.clone()]);
    assert!(all["items"][0].get("plan").is_none(), "only plan rows carry plan: {all}");
    let (_, searched) = call(&router, "GET", "/api/v2/artifacts?q=Q4", &owner, None).await;
    assert_eq!(item_ids(&searched), vec![doc.clone()]);
    let (_, plans) = call(&router, "GET", "/api/v2/artifacts?kind=plan", &owner, None).await;
    assert_eq!(item_ids(&plans), vec![plan_id.to_string()]);
    assert_eq!(plans["items"][0]["plan"]["status"], json!("open"));

    // One plan per chat: a second create with the same id returns the first
    // row unchanged, and a revision is a new version of that row.
    let (status, again) = create_plan(&router, &owner, "ses_1", "# Q4 launch\n\n1. Ship it\n2. Tell people").await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["id"], json!(plan_id));
    assert_eq!(again["current_version"], json!(1));
    let (status, _) = call(
        &router,
        "POST",
        &format!("/api/v2/artifacts/{plan_id}/versions"),
        &owner,
        Some(json!({"base_version": 1, "body": "# Q4 launch\n\n1. Ship it\n2. Tell people", "author": "assistant"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, plans) = call(&router, "GET", "/api/v2/artifacts?kind=plan", &owner, None).await;
    assert_eq!(item_ids(&plans), vec![plan_id.to_string()], "a revision adds a version, not a row");
    assert_eq!(plans["items"][0]["current_version"], json!(2));
    let (_, all) = call(&router, "GET", "/api/v2/artifacts", &owner, None).await;
    assert_eq!(item_ids(&all), vec![doc], "a revised plan still stays out of the default list");

    // A plan is not installable as an app.
    let (status, got) = call(
        &router,
        "PUT",
        &format!("/api/v2/artifacts/{plan_id}/app"),
        &owner,
        Some(json!({"name": "Plan", "tier": "browser"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{got}");
    assert_eq!(got["error"], "kind_not_installable", "{got}");
}

#[tokio::test]
async fn plan_status_patch_is_owner_only_and_plan_only() {
    let (router, _db) = setup().await;
    let owner = who("u_plan_owner2", None, None);
    let editor = who("u_plan_editor", None, None);
    let outsider = who("u_plan_outsider", None, None);
    let (status, _) = create_plan(&router, &owner, "ses_2", "1. Do the thing").await;
    assert_eq!(status, StatusCode::CREATED);
    let plan_path = "/api/v2/artifacts/art_plan_ses_2";
    let (status, _) = share(&router, &owner, "art_plan_ses_2", "people", json!([{"principal_type": "user", "principal_id": "u_plan_editor", "level": "edit"}])).await;
    assert_eq!(status, StatusCode::OK);

    let approve = json!({"plan_status": "approved"});
    let (status, _) = call(&router, "PATCH", plan_path, &editor, Some(approve.clone())).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "only the owner sets a plan's status");
    let (status, _) = call(&router, "PATCH", plan_path, &outsider, Some(approve.clone())).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, got) = call(&router, "PATCH", plan_path, &owner, Some(approve)).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(got["plan"]["status"], json!("approved"));
    assert_eq!(got["current_version"], json!(1), "a status change is not a new version");

    let (status, got) = call(&router, "PATCH", plan_path, &owner, Some(json!({"plan_status": "finished"}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{got}");
    assert_eq!(got["error"], "invalid_plan_status");
    let (status, got) = call(&router, "PATCH", plan_path, &owner, Some(json!({"plan_status": null}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{got}");

    let doc = create(&router, &owner, "doc", "Not a plan").await;
    let (status, got) = call(&router, "PATCH", &format!("/api/v2/artifacts/{doc}"), &owner, Some(json!({"plan_status": "done"}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{got}");
    assert_eq!(got["error"], "not_a_plan");

    let (status, got) = call(&router, "PATCH", plan_path, &owner, Some(json!({"plan_status": "done"}))).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    let (_, plans) = call(&router, "GET", "/api/v2/artifacts?kind=plan", &owner, None).await;
    assert_eq!(plans["items"][0]["plan"]["status"], json!("done"));
    let (_, one) = call(&router, "GET", plan_path, &owner, None).await;
    assert_eq!(one["plan"]["status"], json!("done"));
}

#[tokio::test]
async fn artifacts_v2_counts_match_the_list_per_kind() {
    let (router, _db) = setup().await;
    let owner = who("counts_owner", Some("org_counts"), None);
    let viewer = who("counts_viewer", Some("org_counts"), None);
    create(&router, &owner, "doc", "First doc").await;
    create(&router, &owner, "doc", "Second doc").await;
    let shared = create(&router, &owner, "sheet", "Shared sheet").await;
    let (status, _) = share(&router, &owner, &shared, "org", json!([])).await;
    assert_eq!(status, StatusCode::OK);

    for (caller, scope) in [(&owner, "all"), (&owner, "mine"), (&viewer, "all"), (&viewer, "shared"), (&viewer, "mine")] {
        let (status, counts) = call(&router, "GET", &format!("/api/v1/artifacts/counts?scope={scope}"), caller, None).await;
        assert_eq!(status, StatusCode::OK, "{counts}");
        let (_, list) = call(&router, "GET", &format!("/api/v2/artifacts?scope={scope}&limit=100"), caller, None).await;
        let items = list["items"].as_array().unwrap();
        let mut by_kind: std::collections::BTreeMap<String, i64> = Default::default();
        for item in items {
            *by_kind.entry(item["kind"].as_str().unwrap().to_string()).or_default() += 1;
        }
        assert_eq!(counts["total"], json!(items.len()), "{scope}: {counts}");
        assert_eq!(counts["byKind"], json!(by_kind), "{scope}: {counts}");
    }
    let (_, viewer_counts) = call(&router, "GET", "/api/v1/artifacts/counts?scope=shared", &viewer, None).await;
    assert_eq!(viewer_counts, json!({ "total": 1, "byKind": { "sheet": 1 } }));
    let (status, _) = call(&router, "GET", "/api/v1/artifacts/counts?scope=everyone", &owner, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
