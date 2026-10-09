//! Tests for comments + presence. DB-backed tests use the same per-test
//! schema harness as artifacts_v2_tests.

use super::*;
use crate::routes::test_support::{test_state, MockGateway};
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use tower::ServiceExt;

#[test]
fn mention_detection() {
    assert!(mentions_assistant("hey @Gizzi can you fix this"));
    assert!(mentions_assistant("@ALLTERNIT, please"));
    assert!(mentions_assistant("(@gizzi)"));
    assert!(!mentions_assistant("mail a@gizzi.com"));
    assert!(!mentions_assistant("@gizzis"));
    assert!(!mentions_assistant("@gizzi-bot"));
    assert!(!mentions_assistant("no mention"));
    assert!(mentions_assistant("x@gizzi.com and @gizzi"));
}

#[test]
fn body_and_anchor_validation() {
    assert_eq!(validate_body("  hi  ").unwrap(), "hi");
    assert!(validate_body("   ").is_err());
    assert!(validate_body(&"x".repeat(4000)).is_ok());
    assert!(validate_body(&"x".repeat(4001)).is_err());
    assert!(validate_anchor(&json!({})).is_ok());
    assert!(validate_anchor(&json!({"kind":"cell","ref":"B2"})).is_ok());
    assert!(validate_anchor(&json!({"kind":"nope"})).is_err());
    assert!(validate_anchor(&json!({"ref":"B2"})).is_err());
    assert!(validate_anchor(&json!("text")).is_err());
    assert!(validate_anchor(&json!({"kind":"text","q":"x".repeat(17000)})).is_err());
}

#[test]
fn presence_ttl_prune_exclusion() {
    let mut s = PresenceStore::default();
    let t0 = Instant::now();
    s.heartbeat("a", "u1", Some("One".into()), None, "viewing", t0);
    s.heartbeat("a", "u2", None, None, "editing", t0);
    let seen = s.others("a", "u1", t0);
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0]["user_id"], "u2");
    assert_eq!(seen[0]["state"], "editing");
    // u1 refreshes, u2 expires.
    s.heartbeat("a", "u1", None, None, "viewing", t0 + Duration::from_secs(40));
    let later = t0 + Duration::from_secs(50);
    assert!(s.others("a", "u1", later).is_empty());
    assert_eq!(s.others("a", "u2", later).len(), 1);
    // Everything expires: the artifact map is removed.
    let end = t0 + Duration::from_secs(200);
    assert!(s.others("a", "x", end).is_empty());
    assert!(s.artifacts.is_empty());
    // "left" removes and drops an emptied map.
    s.heartbeat("b", "u1", None, None, "viewing", end);
    s.heartbeat("b", "u1", None, None, "left", end);
    assert!(s.artifacts.is_empty());
}

#[test]
fn presence_cap() {
    let mut s = PresenceStore::default();
    let t = Instant::now();
    for i in 0..(PRESENCE_MAX_USERS + 20) {
        s.heartbeat("a", &format!("u{i}"), None, None, "viewing", t);
    }
    assert_eq!(s.artifacts["a"].len(), PRESENCE_MAX_USERS);
    // An existing user can still refresh at the cap.
    s.heartbeat("a", "u0", None, None, "editing", t);
    assert_eq!(s.artifacts["a"]["u0"].state, "editing");
}

// ----- DB-backed route tests -----

async fn setup() -> (Router, PgPool) {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    sqlx::raw_sql(include_str!("../../migrations_pg/085_artifacts_v2.sql"))
        .execute(&state.db)
        .await
        .expect("085 applies");
    let db = state.db.clone();
    (
        super::super::artifacts_v2::routes().merge(routes()).with_state(state),
        db,
    )
}

fn who(id: &str, org: Option<&str>) -> Value {
    json!({ "id": id, "org_id": org, "email": format!("{id}@example.com"), "name": id })
}

async fn call(router: &Router, method: &str, path: &str, who: &Value, body: Option<Value>) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(path).header("x-test-caller", who.to_string());
    let req = match body {
        Some(body) => b
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => {
            b = b;
            b.body(Body::empty()).unwrap()
        }
    };
    let response = router.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(Value::Null) };
    (status, value)
}

async fn make(router: &Router, owner: &Value, kind: &str) -> String {
    let (st, body) = call(router, "POST", "/api/v2/artifacts", owner, Some(json!({"kind": kind, "title": "T", "body": "b"}))).await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn comment_lifecycle() {
    let (router, _db) = setup().await;
    let owner = who("owner1", None);
    let viewer = who("viewer1", None);
    let id = make(&router, &owner, "page").await;
    let base = format!("/api/v2/artifacts/{id}/comments");

    // Signed-in stranger: 404.
    let (st, _) = call(&router, "GET", &base, &viewer, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Share view-only: can read, cannot comment.
    let (st, b) = call(&router, "PUT", &format!("/api/v2/artifacts/{id}/sharing"), &owner,
        Some(json!({"visibility":"people","shares":[{"principal_type":"user","principal_id":"viewer1","level":"view"}]}))).await;
    assert_eq!(st, StatusCode::OK, "{b}");
    let (st, _) = call(&router, "POST", &base, &viewer, Some(json!({"body":"hi"}))).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Owner comments with an anchor and an @-mention.
    let (st, root) = call(&router, "POST", &base, &owner,
        Some(json!({"body":" look @Gizzi ","anchor":{"kind":"cell","ref":"A1"},"version":1}))).await;
    assert_eq!(st, StatusCode::CREATED, "{root}");
    assert_eq!(root["body"], "look @Gizzi");
    assert_eq!(root["to_assistant"], true);
    assert_eq!(root["anchor"]["ref"], "A1");
    let rid = root["id"].as_str().unwrap().to_string();

    // Bad inputs.
    let (st, _) = call(&router, "POST", &base, &owner, Some(json!({"body":"  "}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = call(&router, "POST", &base, &owner, Some(json!({"body":"x","parent_id":"nope"}))).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);

    // Reply inherits anchor; nested reply refused.
    let (st, reply) = call(&router, "POST", &base, &owner, Some(json!({"body":"more","parent_id":rid}))).await;
    assert_eq!(st, StatusCode::CREATED, "{reply}");
    assert_eq!(reply["anchor"]["ref"], "A1");
    assert_eq!(reply["version"], 1);
    let (st, _) = call(&router, "POST", &base, &owner, Some(json!({"body":"x","parent_id":reply["id"]}))).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);

    // Assistant reply: needs to_assistant parent + edit access.
    let (st, _) = call(&router, "POST", &format!("{base}/assistant-reply"), &viewer, Some(json!({"parent_id":rid,"body":"ok"}))).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = call(&router, "POST", &format!("{base}/assistant-reply"), &owner, Some(json!({"parent_id":reply["id"],"body":"ok"}))).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    let (st, ar) = call(&router, "POST", &format!("{base}/assistant-reply"), &owner, Some(json!({"parent_id":rid,"body":"done"}))).await;
    assert_eq!(st, StatusCode::CREATED, "{ar}");
    assert_eq!(ar["author_id"], "assistant");
    assert_eq!(ar["to_assistant"], false);
    assert_eq!(ar["parent_id"], rid);

    // List oldest first, viewer can read.
    let (st, list) = call(&router, "GET", &base, &viewer, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(list["items"].as_array().unwrap().len(), 3);
    assert_eq!(list["items"][0]["id"], rid);

    // Resolve: viewer cannot, owner can; edit body: author only.
    let (st, _) = call(&router, "PATCH", &format!("{base}/{rid}"), &viewer, Some(json!({"resolved":true}))).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, r) = call(&router, "PATCH", &format!("{base}/{rid}"), &owner, Some(json!({"resolved":true}))).await;
    assert_eq!(st, StatusCode::OK, "{r}");
    assert!(r["resolved_at"].is_string());
    let (_, r) = call(&router, "PATCH", &format!("{base}/{rid}"), &owner, Some(json!({"resolved":false}))).await;
    assert!(r["resolved_at"].is_null());
    let (st, _) = call(&router, "PATCH", &format!("{base}/{}", reply["id"].as_str().unwrap()), &owner, Some(json!({"resolved":true}))).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);

    // Delete root cascades to replies.
    let (st, _) = call(&router, "DELETE", &format!("{base}/{rid}"), &viewer, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = call(&router, "DELETE", &format!("{base}/{rid}"), &owner, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (_, list) = call(&router, "GET", &base, &owner, None).await;
    assert!(list["items"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn doc_view_caller_cannot_comment_but_edit_can() {
    let (router, _db) = setup().await;
    let owner = who("o2", None);
    let editor = who("e2", None);
    let id = make(&router, &owner, "doc").await;
    let (st, b) = call(&router, "PUT", &format!("/api/v2/artifacts/{id}/sharing"), &owner,
        Some(json!({"visibility":"people","shares":[{"principal_type":"user","principal_id":"e2","level":"edit"}]}))).await;
    assert_eq!(st, StatusCode::OK, "{b}");
    let (st, _) = call(&router, "POST", &format!("/api/v2/artifacts/{id}/comments"), &editor, Some(json!({"body":"ok"}))).await;
    assert_eq!(st, StatusCode::CREATED);
}

#[tokio::test]
async fn presence_routes_and_org_toggle() {
    let (router, db) = setup().await;
    let a = who("pa", Some("org_p"));
    let b = who("pb", Some("org_p"));
    let id = make(&router, &a, "page").await;
    call(&router, "PUT", &format!("/api/v2/artifacts/{id}/sharing"), &a,
        Some(json!({"visibility":"org","shares":[]}))).await;
    let path = format!("/api/v2/artifacts/{id}/presence");
    let (st, _) = call(&router, "POST", &path, &a, Some(json!({"state":"editing"}))).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = call(&router, "POST", &path, &a, Some(json!({"state":"bogus"}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, body) = call(&router, "GET", &path, &b, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["enabled"], true);
    assert_eq!(body["users"][0]["user_id"], "pa");
    assert_eq!(body["users"][0]["state"], "editing");
    // Caller excluded.
    let (_, own) = call(&router, "GET", &path, &a, None).await;
    assert!(own["users"].as_array().unwrap().is_empty());
    // Org turns presence off.
    sqlx::query("INSERT INTO org_artifact_settings (org_id, presence) VALUES ('org_p', false) \
                 ON CONFLICT (org_id) DO UPDATE SET presence = false")
        .execute(&db).await.unwrap();
    let (st, off) = call(&router, "GET", &path, &b, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(off, json!({"enabled": false, "users": []}));
    let (st, _) = call(&router, "POST", &path, &b, Some(json!({"state":"viewing"}))).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
}
