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

// ----- DB-backed route tests -----

async fn setup() -> (Router, PgPool) {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    sqlx::raw_sql(include_str!("../../migrations_pg/085_artifacts_v2.sql"))
        .execute(&state.db)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../../migrations_pg/086_artifact_presence.sql"))
        .execute(&state.db)
        .await
        .expect("085 applies");
    sqlx::raw_sql(include_str!("../../migrations_pg/098_artifact_agent_watch.sql"))
        .execute(&state.db)
        .await
        .expect("098 applies");
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
async fn server_answers_a_mention_the_app_does_not_handle() {
    let (router, _db) = setup().await;
    let owner = who("srv_owner", None);
    let id = make(&router, &owner, "doc").await;
    let base = format!("/api/v2/artifacts/{id}/comments");
    let (st, root) = call(&router, "POST", &base, &owner, Some(json!({"body":"@gizzi what is this?"}))).await;
    assert_eq!(st, StatusCode::CREATED, "{root}");
    assert_eq!(root["assistant_reply"], "server");
    // The test server has no model: the reply says so instead of leaving the thread waiting.
    let mut reply = Value::Null;
    for _ in 0..50 {
        let (_, list) = call(&router, "GET", &base, &owner, None).await;
        if let Some(r) = list["items"].as_array().and_then(|a| a.iter().find(|c| c["author_id"] == "assistant").cloned()) {
            reply = r;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(reply["parent_id"], root["id"], "{reply}");
    assert!(reply["body"].as_str().unwrap_or_default().starts_with("I couldn't answer"), "{reply}");
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
        Some(json!({"body":" look @Gizzi ","anchor":{"kind":"cell","ref":"A1"},"version":1,"assistant_by_client":true}))).await;
    assert_eq!(st, StatusCode::CREATED, "{root}");
    assert!(root.get("assistant_reply").is_none(), "the app said it answers this one");
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

#[tokio::test]
async fn presence_expires_leaves_and_purges() {
    let (router, db) = setup().await;
    let a = who("qa", None);
    let b = who("qb", None);
    let id = make(&router, &a, "page").await;
    call(&router, "PUT", &format!("/api/v2/artifacts/{id}/sharing"), &a,
        Some(json!({"visibility":"people","shares":[{"principal_type":"user","principal_id":"qb","level":"edit"}]}))).await;
    let path = format!("/api/v2/artifacts/{id}/presence");
    call(&router, "POST", &path, &a, Some(json!({"state":"viewing"}))).await;
    let (_, seen) = call(&router, "GET", &path, &b, None).await;
    assert_eq!(seen["users"].as_array().unwrap().len(), 1);
    // A heartbeat from another instance is the same row: a second POST updates state.
    call(&router, "POST", &path, &a, Some(json!({"state":"editing"}))).await;
    let (_, seen) = call(&router, "GET", &path, &b, None).await;
    assert_eq!(seen["users"][0]["state"], "editing");
    // Older than 45 s: hidden, but not yet deleted.
    sqlx::query("UPDATE artifact_presence SET last_seen = now() - interval '60 seconds'")
        .execute(&db).await.unwrap();
    let (_, seen) = call(&router, "GET", &path, &b, None).await;
    assert!(seen["users"].as_array().unwrap().is_empty());
    // A fresh heartbeat revives it; "left" removes it.
    call(&router, "POST", &path, &a, Some(json!({"state":"viewing"}))).await;
    let (_, seen) = call(&router, "GET", &path, &b, None).await;
    assert_eq!(seen["users"].as_array().unwrap().len(), 1);
    call(&router, "POST", &path, &a, Some(json!({"state":"left"}))).await;
    let (_, seen) = call(&router, "GET", &path, &b, None).await;
    assert!(seen["users"].as_array().unwrap().is_empty());
    // The sweep deletes rows older than 10 minutes and keeps fresh ones.
    call(&router, "POST", &path, &a, Some(json!({"state":"viewing"}))).await;
    call(&router, "POST", &path, &b, Some(json!({"state":"viewing"}))).await;
    sqlx::query("UPDATE artifact_presence SET last_seen = now() - interval '11 minutes' WHERE user_id = 'qa'")
        .execute(&db).await.unwrap();
    purge_stale_presence(&db).await;
    let left: Vec<(String,)> = sqlx::query_as("SELECT user_id FROM artifact_presence")
        .fetch_all(&db).await.unwrap();
    assert_eq!(left, vec![("qb".to_string(),)]);
}

// ----- Frame claims on agent presence (SPEC-agent-canvas-2026-10-20) -----

#[test]
fn claimed_artboard_ids() {
    assert_eq!(claimed_artboard(None).unwrap(), None);
    assert_eq!(claimed_artboard(Some(&json!({"kind":"block","block":3}))).unwrap(), None);
    assert_eq!(claimed_artboard(Some(&json!({"artboard":null}))).unwrap(), None);
    assert_eq!(claimed_artboard(Some(&json!({"artboard":"shape:hero"}))).unwrap().as_deref(), Some("hero"));
    assert_eq!(claimed_artboard(Some(&json!({"kind":"artboard","artboard":" hero "}))).unwrap().as_deref(), Some("hero"));
    assert!(claimed_artboard(Some(&json!({"artboard":""}))).is_err());
    assert!(claimed_artboard(Some(&json!({"artboard":"shape:"}))).is_err());
    assert!(claimed_artboard(Some(&json!({"artboard":3}))).is_err());
    assert!(claimed_artboard(Some(&json!({"artboard":"x".repeat(201)}))).is_err());
}

#[tokio::test]
async fn artboard_claims_conflict_release_and_expire() {
    let (router, db) = setup().await;
    let owner = who("cl_owner", None);
    let editor = who("cl_editor", None);
    let id = make(&router, &owner, "design").await;
    call(&router, "PUT", &format!("/api/v2/artifacts/{id}/sharing"), &owner,
        Some(json!({"visibility":"people","shares":[{"principal_type":"user","principal_id":"cl_editor","level":"edit"}]}))).await;
    let path = format!("/api/v2/artifacts/{id}/presence");
    let beat = |session: &str, name: &str, anchor: Value| {
        json!({"state":"editing","agent":{"name":name,"session_id":session,"task":"Dark restyle","anchor":anchor}})
    };
    let hero = json!({"kind":"artboard","artboard":"shape:hero"});

    // Oki claims Hero; the claim shows as its anchor.
    let (st, _) = call(&router, "POST", &path, &owner, Some(beat("ses_oki", "Oki", hero.clone()))).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (_, seen) = call(&router, "GET", &path, &editor, None).await;
    let oki = seen["users"].as_array().unwrap().iter().find(|u| u["kind"] == "agent").unwrap().clone();
    assert_eq!(oki["anchor"]["artboard"], "shape:hero");

    // Another live agent session (even another person's agent) gets 409 naming
    // the holder, with or without the shape: prefix. Its row is not written.
    let (st, body) = call(&router, "POST", &path, &editor, Some(beat("ses_ash", "Ash", json!({"artboard":"hero"})))).await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "artboard_claimed");
    assert_eq!(body["code"], "artboard_claimed");
    assert_eq!(body["holder"], json!({"name": "Oki", "session_id": "ses_oki"}));
    assert!(body["message"].as_str().unwrap().contains("Oki"));
    let (_, seen) = call(&router, "GET", &path, &owner, None).await;
    assert!(seen["users"].as_array().unwrap().iter().all(|u| u["user_id"] != "agent:ses_ash:ash"));

    // The holder's own heartbeats keep the claim, also under a new task line;
    // another frame is free; a heartbeat without an artboard claims nothing.
    let (st, _) = call(&router, "POST", &path, &owner, Some(beat("ses_oki", "Oki", hero.clone()))).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = call(&router, "POST", &path, &editor, Some(beat("ses_ash", "Ash", json!({"artboard":"pricing"})))).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = call(&router, "POST", &path, &editor, Some(beat("ses_bo", "Bo", json!({"kind":"block","block":1})))).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    // Ash moving onto Hero is refused, and Ash keeps Pricing.
    let (st, _) = call(&router, "POST", &path, &editor, Some(beat("ses_ash", "Ash", hero.clone()))).await;
    assert_eq!(st, StatusCode::CONFLICT);
    let (st, _) = call(&router, "POST", &path, &owner, Some(beat("ses_cy", "Cy", json!({"artboard":"pricing"})))).await;
    assert_eq!(st, StatusCode::CONFLICT);

    // anchor: null releases Hero.
    let (st, _) = call(&router, "POST", &path, &owner, Some(beat("ses_oki", "Oki", Value::Null))).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = call(&router, "POST", &path, &editor, Some(beat("ses_cy", "Cy", hero.clone()))).await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    // state: left releases too.
    call(&router, "POST", &path, &editor, Some(json!({"state":"left","agent":{"name":"Cy","session_id":"ses_cy"}}))).await;
    let (st, _) = call(&router, "POST", &path, &owner, Some(beat("ses_oki", "Oki", hero.clone()))).await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    // A claim lapses when its heartbeat is older than the 45 s TTL.
    sqlx::query("UPDATE artifact_presence SET last_seen = now() - interval '60 seconds' WHERE user_id = 'agent:ses_oki:oki'")
        .execute(&db).await.unwrap();
    let (st, _) = call(&router, "POST", &path, &editor, Some(beat("ses_ash", "Ash", hero))).await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    // A bad artboard id is a 400.
    let (st, _) = call(&router, "POST", &path, &owner, Some(beat("ses_oki", "Oki", json!({"artboard": 7})))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn artboard_claims_hold_with_presence_off() {
    let (router, db) = setup().await;
    let owner = who("off_owner", Some("org_off"));
    let teammate = who("off_mate", Some("org_off"));
    let id = make(&router, &owner, "design").await;
    call(&router, "PUT", &format!("/api/v2/artifacts/{id}/sharing"), &owner,
        Some(json!({"visibility":"org","shares":[]}))).await;
    sqlx::query("INSERT INTO org_artifact_settings (org_id, presence) VALUES ('org_off', false) \
                 ON CONFLICT (org_id) DO UPDATE SET presence = false")
        .execute(&db).await.unwrap();
    let path = format!("/api/v2/artifacts/{id}/presence");
    let beat = |session: &str, name: &str, anchor: Value| {
        json!({"state":"editing","agent":{"name":name,"session_id":session,"task":"Dark restyle","anchor":anchor}})
    };
    let hero = json!({"kind":"artboard","artboard":"shape:hero"});
    let rows = |db: PgPool| async move {
        let r: Vec<(String,)> = sqlx::query_as("SELECT user_id FROM artifact_presence ORDER BY user_id")
            .fetch_all(&db).await.unwrap();
        r.into_iter().map(|(u,)| u).collect::<Vec<_>>()
    };

    // Oki's claim is recorded and enforced; a second session gets 409.
    let (st, _) = call(&router, "POST", &path, &owner, Some(beat("ses_oki", "Oki", hero.clone()))).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, body) = call(&router, "POST", &path, &owner, Some(beat("ses_ash", "Ash", json!({"artboard":"hero"})))).await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["holder"], json!({"name": "Oki", "session_id": "ses_oki"}));

    // GET hides the claim row while presence is off.
    let (st, seen) = call(&router, "GET", &path, &teammate, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(seen, json!({"enabled": false, "users": []}));

    // Only claim rows are kept: a person's heartbeat and an agent heartbeat
    // that claims nothing record nothing.
    call(&router, "POST", &path, &teammate, Some(json!({"state":"viewing"}))).await;
    call(&router, "POST", &path, &owner, Some(beat("ses_bo", "Bo", json!({"kind":"block","block":1})))).await;
    assert_eq!(rows(db.clone()).await, vec!["agent:ses_oki:oki".to_string()]);

    // The suggestions route sees the claim too (it reads the same rows).
    let holder = artboard_holder(&db, &id, "hero", Some("ses_ash"), "agent:ses_ash:ash").await.unwrap();
    assert_eq!(holder, Some((Some("Oki".to_string()), Some("ses_oki".to_string()))));

    // A heartbeat without the frame releases it, and Ash can take it.
    let (st, _) = call(&router, "POST", &path, &owner, Some(beat("ses_oki", "Oki", Value::Null))).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(rows(db.clone()).await.is_empty());
    let (st, _) = call(&router, "POST", &path, &owner, Some(beat("ses_ash", "Ash", hero))).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    // state: left releases too.
    call(&router, "POST", &path, &owner, Some(json!({"state":"left","agent":{"name":"Ash","session_id":"ses_ash"}}))).await;
    assert!(rows(db.clone()).await.is_empty());
}

#[test]
fn design_pin_anchors() {
    // A Design canvas pin keeps every field as sent.
    let pin = json!({"kind":"element","shapeId":"shape:hero","artboard":"shape:hero","label":"Hero","extra":{"zoom":1}});
    assert!(validate_anchor(&pin).is_ok());
    assert!(validate_agent_comment_anchor(&pin).is_ok());
    assert!(validate_anchor(&json!({"kind":"element","shapeId":7})).is_err());
    assert!(validate_anchor(&json!({"kind":"element","shapeId":"x".repeat(201)})).is_err());
    assert!(validate_anchor(&json!({"kind":"element","artboard":["hero"]})).is_err());
    // People's anchors go to 16 KB; an agent's to 4 KB.
    let big = json!({"kind":"element","shapeId":"shape:hero","text":"x".repeat(5000)});
    assert!(validate_anchor(&big).is_ok());
    assert!(validate_agent_comment_anchor(&big).is_err());
}

#[tokio::test]
async fn agent_pins_a_question_to_a_frame() {
    let (router, _db) = setup().await;
    let owner = who("pin_owner", None);
    let id = make(&router, &owner, "design").await;
    let base = format!("/api/v2/artifacts/{id}/comments");
    let anchor = json!({"kind":"element","shapeId":"shape:hero","artboard":"shape:hero","label":"Hero","note":{"any":"json"}});
    let (st, c) = call(&router, "POST", &base, &owner,
        Some(json!({"body":"Dark or light for the hero?","agent_name":"Oki","anchor":anchor}))).await;
    assert_eq!(st, StatusCode::CREATED, "{c}");
    assert_eq!(c["author_name"], "Oki");
    assert_eq!(c["anchor"], anchor, "the anchor is stored as sent");
    // A reply inherits the pin.
    let (_, reply) = call(&router, "POST", &base, &owner, Some(json!({"body":"Dark","parent_id":c["id"]}))).await;
    assert_eq!(reply["anchor"]["shapeId"], "shape:hero");
    // An agent's anchor over 4 KB is refused; a person's is not.
    let big = json!({"kind":"element","shapeId":"shape:hero","text":"x".repeat(5000)});
    let (st, _) = call(&router, "POST", &base, &owner, Some(json!({"body":"q","agent_name":"Oki","anchor":big}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = call(&router, "POST", &base, &owner, Some(json!({"body":"q","anchor":big}))).await;
    assert_eq!(st, StatusCode::CREATED);
}
