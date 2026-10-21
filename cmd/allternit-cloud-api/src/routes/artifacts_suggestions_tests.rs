//! Tests for agent suggestions, agent presence and agent comments. DB-backed
//! tests use the same per-test schema harness as artifacts_comments_tests.

use super::*;
use crate::routes::test_support::{test_state, MockGateway};
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use tower::ServiceExt;

#[test]
fn change_validation() {
    // text
    let t = validate_change("doc", &json!({"kind":"text","find":"about 40%","replace":"38%","extra":1})).unwrap();
    assert_eq!(t, json!({"kind":"text","block":null,"find":"about 40%","replace":"38%"}));
    assert!(validate_change("doc", &json!({"kind":"text","block":2,"find":"a","replace":""})).is_ok());
    assert!(validate_change("doc", &json!({"kind":"text","block":-1,"find":"a","replace":"b"})).is_err());
    assert!(validate_change("doc", &json!({"kind":"text","block":1.5,"find":"a","replace":"b"})).is_err());
    assert!(validate_change("doc", &json!({"kind":"text","find":"","replace":"b"})).is_err());
    assert!(validate_change("doc", &json!({"kind":"text","find":"a"})).is_err());
    assert!(validate_change("doc", &json!({"kind":"text","find":"x".repeat(20_001),"replace":"b"})).is_err());
    assert!(validate_change("doc", &json!({"kind":"text","find":"x".repeat(20_000),"replace":"b"})).is_ok());
    // block
    assert!(validate_change("doc", &json!({"kind":"block","op":"replace","block":0,"markdown":"# Hi"})).is_ok());
    assert!(validate_change("doc", &json!({"kind":"block","op":"insert_after","block":3,"markdown":"x"})).is_ok());
    let d = validate_change("doc", &json!({"kind":"block","op":"delete","block":1,"markdown":"ignored"})).unwrap();
    assert_eq!(d, json!({"kind":"block","op":"delete","block":1}));
    assert!(validate_change("doc", &json!({"kind":"block","op":"move","block":0,"markdown":"x"})).is_err());
    assert!(validate_change("doc", &json!({"kind":"block","op":"replace","markdown":"x"})).is_err());
    assert!(validate_change("doc", &json!({"kind":"block","op":"replace","block":null,"markdown":"x"})).is_err());
    assert!(validate_change("doc", &json!({"kind":"block","op":"replace","block":0})).is_err());
    // cell
    let c = validate_change("sheet", &json!({"kind":"cell","address":"b4","value":"Nov 10"})).unwrap();
    assert_eq!(c, json!({"kind":"cell","sheet":null,"address":"B4","value":"Nov 10"}));
    assert!(validate_change("sheet", &json!({"kind":"cell","sheet":"Q4","address":"AA10","value":"=SUM(A1:A3)"})).is_ok());
    assert!(validate_change("sheet", &json!({"kind":"cell","address":"B0","value":"x"})).is_err());
    assert!(validate_change("sheet", &json!({"kind":"cell","address":"4B","value":"x"})).is_err());
    assert!(validate_change("sheet", &json!({"kind":"cell","address":"B4"})).is_err());
    assert!(validate_change("sheet", &json!({"kind":"cell","sheet":3,"address":"B4","value":"x"})).is_err());
    // kind mismatch and unknown kinds
    assert!(validate_change("sheet", &json!({"kind":"text","find":"a","replace":"b"})).is_err());
    assert!(validate_change("doc", &json!({"kind":"cell","address":"B4","value":"x"})).is_err());
    assert!(validate_change("doc", &json!({"kind":"nope"})).is_err());
    assert!(validate_change("doc", &json!("text")).is_err());
}

#[test]
fn agent_presence_ids() {
    use super::super::artifacts_comments::{agent_presence_id, agent_slug};
    assert_eq!(agent_slug("Research Bot!"), "research-bot");
    assert_eq!(agent_slug("  --Gizzi--  "), "gizzi");
    assert_eq!(agent_slug("日本"), "agent");
    assert_eq!(agent_presence_id(Some("ses_1"), "Gizzi"), "agent:ses_1:gizzi");
    assert_eq!(agent_presence_id(None, "Gizzi"), "agent:none:gizzi");
}

// ----- DB-backed route tests -----

async fn setup() -> (Router, PgPool) {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    for sql in [
        include_str!("../../migrations_pg/085_artifacts_v2.sql"),
        include_str!("../../migrations_pg/086_artifact_presence.sql"),
        include_str!("../../migrations_pg/098_artifact_agent_watch.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&state.db).await.expect("migration applies");
    }
    let db = state.db.clone();
    (
        super::super::artifacts_v2::routes()
            .merge(super::super::artifacts_comments::routes())
            .merge(routes())
            .with_state(state),
        db,
    )
}

fn who(id: &str) -> Value {
    json!({ "id": id, "org_id": null, "email": format!("{id}@example.com"), "name": id })
}

async fn call(router: &Router, method: &str, path: &str, who: &Value, body: Option<Value>) -> (StatusCode, Value) {
    let b = Request::builder().method(method).uri(path).header("x-test-caller", who.to_string());
    let req = match body {
        Some(body) => b
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
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

async fn share(router: &Router, owner: &Value, id: &str, user: &str, level: &str) {
    let (st, b) = call(router, "PUT", &format!("/api/v2/artifacts/{id}/sharing"), owner,
        Some(json!({"visibility":"people","shares":[{"principal_type":"user","principal_id":user,"level":level}]}))).await;
    assert_eq!(st, StatusCode::OK, "{b}");
}

#[tokio::test]
async fn suggestion_lifecycle() {
    let (router, _db) = setup().await;
    let owner = who("sg_owner");
    let viewer = who("sg_viewer");
    let id = make(&router, &owner, "doc").await;
    share(&router, &owner, &id, "sg_viewer", "view").await;
    let base = format!("/api/v2/artifacts/{id}/suggestions");
    let body = json!({
        "base_version": 1,
        "agent_name": "Gizzi",
        "session_id": "ses_1",
        "cost_usd": 0.4321,
        "changes": [
            {"change": {"kind":"text","block":0,"find":"b","replace":"c"}, "reason": "Fix the typo"},
            {"change": {"kind":"block","op":"insert_after","block":0,"markdown":"## Next"}},
        ],
    });

    // View access cannot suggest.
    let (st, _) = call(&router, "POST", &base, &viewer, Some(body.clone())).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Bad inputs.
    let (st, _) = call(&router, "POST", &base, &owner, Some(json!({"base_version":1,"agent_name":"G","changes":[]}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let many: Vec<Value> = (0..51).map(|_| json!({"change":{"kind":"text","find":"a","replace":"b"}})).collect();
    let (st, _) = call(&router, "POST", &base, &owner, Some(json!({"base_version":1,"agent_name":"G","changes":many}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = call(&router, "POST", &base, &owner,
        Some(json!({"base_version":1,"agent_name":"G","changes":[{"change":{"kind":"text","find":"a","replace":"b"},"reason":"x".repeat(501)}]}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = call(&router, "POST", &base, &owner,
        Some(json!({"base_version":1,"agent_name":"G","changes":[{"change":{"kind":"cell","address":"B4","value":"x"}}]}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // Create: returned in order, with numbers for cost.
    let (st, created) = call(&router, "POST", &base, &owner, Some(body)).await;
    assert_eq!(st, StatusCode::CREATED, "{created}");
    let items = created["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert!(items[0]["id"].as_str().unwrap().starts_with("sug_"));
    assert_eq!(items[0]["status"], "pending");
    assert_eq!(items[0]["agent_name"], "Gizzi");
    assert_eq!(items[0]["session_id"], "ses_1");
    assert_eq!(items[0]["reason"], "Fix the typo");
    assert_eq!(items[0]["base_version"], 1);
    assert_eq!(items[0]["artifact_id"], id.as_str());
    assert!((items[0]["cost_usd"].as_f64().unwrap() - 0.4321).abs() < 1e-9);
    assert!(items[0]["resolved_at"].is_null());
    assert_eq!(items[1]["change"]["op"], "insert_after");
    assert!(items[1]["reason"].is_null());
    let s1 = items[0]["id"].as_str().unwrap().to_string();
    let s2 = items[1]["id"].as_str().unwrap().to_string();

    // Viewer can list; default is pending, oldest first.
    let (st, list) = call(&router, "GET", &base, &viewer, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(list["items"][0]["id"], s1.as_str());
    assert_eq!(list["items"][1]["id"], s2.as_str());
    let (st, _) = call(&router, "GET", &format!("{base}?status=bogus"), &owner, None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // PATCH: edit access, a known status, pending only.
    let (st, _) = call(&router, "PATCH", &format!("{base}/{s1}"), &viewer, Some(json!({"status":"accepted"}))).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = call(&router, "PATCH", &format!("{base}/{s1}"), &owner, Some(json!({"status":"pending"}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = call(&router, "PATCH", &format!("{base}/sug_nope"), &owner, Some(json!({"status":"accepted"}))).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, acc) = call(&router, "PATCH", &format!("{base}/{s1}"), &owner, Some(json!({"status":"accepted"}))).await;
    assert_eq!(st, StatusCode::OK, "{acc}");
    assert_eq!(acc["status"], "accepted");
    assert!(acc["resolved_at"].is_string());
    let (st, again) = call(&router, "PATCH", &format!("{base}/{s1}"), &owner, Some(json!({"status":"rejected"}))).await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert_eq!(again["code"], "already_resolved");

    let (_, pending) = call(&router, "GET", &base, &owner, None).await;
    assert_eq!(pending["items"].as_array().unwrap().len(), 1);
    let (_, all) = call(&router, "GET", &format!("{base}?status=all"), &owner, None).await;
    assert_eq!(all["items"].as_array().unwrap().len(), 2);

    // Bulk resolve changes only the pending ones it names.
    let (st, _) = call(&router, "POST", &format!("{base}/resolve"), &viewer, Some(json!({"ids":[s2],"status":"rejected"}))).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, bulk) = call(&router, "POST", &format!("{base}/resolve"), &owner,
        Some(json!({"ids":[s1, s2, "sug_nope"],"status":"stale"}))).await;
    assert_eq!(st, StatusCode::OK, "{bulk}");
    let changed = bulk["items"].as_array().unwrap();
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0]["id"], s2.as_str());
    assert_eq!(changed[0]["status"], "stale");
    let (_, pending) = call(&router, "GET", &base, &owner, None).await;
    assert!(pending["items"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn suggestions_need_doc_or_sheet() {
    let (router, _db) = setup().await;
    let owner = who("sg_kind");
    let page = make(&router, &owner, "page").await;
    let (st, body) = call(&router, "POST", &format!("/api/v2/artifacts/{page}/suggestions"), &owner,
        Some(json!({"base_version":1,"agent_name":"G","changes":[{"change":{"kind":"text","find":"a","replace":"b"}}]}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "unsupported_kind");

    let sheet = make(&router, &owner, "sheet").await;
    let (st, body) = call(&router, "POST", &format!("/api/v2/artifacts/{sheet}/suggestions"), &owner,
        Some(json!({"base_version":1,"agent_name":"G","changes":[{"change":{"kind":"cell","sheet":null,"address":"B4","value":"Nov 10"}}]}))).await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    assert_eq!(body["items"][0]["change"]["address"], "B4");
    assert!(body["items"][0]["cost_usd"].is_null());
    assert!(body["items"][0]["session_id"].is_null());
}

#[tokio::test]
async fn agent_presence_rows() {
    let (router, _db) = setup().await;
    let owner = who("ap_owner");
    let viewer = who("ap_viewer");
    let id = make(&router, &owner, "doc").await;
    share(&router, &owner, &id, "ap_viewer", "view").await;
    let path = format!("/api/v2/artifacts/{id}/presence");
    let agent = json!({
        "state": "editing",
        "agent": {"name":"Research Bot","session_id":"ses_9","task":"Fixing the date","anchor":{"kind":"block","block":3},"cost_usd":0.12},
    });

    // View access cannot post agent presence; bad anchors are refused.
    let (st, _) = call(&router, "POST", &path, &viewer, Some(agent.clone())).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = call(&router, "POST", &path, &owner,
        Some(json!({"state":"editing","agent":{"name":"X","anchor":{"kind":"block","pad":"x".repeat(5000)}}}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = call(&router, "POST", &path, &owner, Some(json!({"state":"editing","agent":{"name":"  "}}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    let (st, _) = call(&router, "POST", &path, &owner, Some(agent)).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = call(&router, "POST", &path, &owner, Some(json!({"state":"viewing"}))).await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    // The owner sees their own agent (not themselves); the viewer sees both.
    let (_, mine) = call(&router, "GET", &path, &owner, None).await;
    let users = mine["users"].as_array().unwrap();
    assert_eq!(users.len(), 1, "{mine}");
    let a = &users[0];
    assert_eq!(a["user_id"], "agent:ses_9:research-bot");
    assert_eq!(a["kind"], "agent");
    assert_eq!(a["name"], "Research Bot");
    assert_eq!(a["state"], "editing");
    assert_eq!(a["task"], "Fixing the date");
    assert_eq!(a["anchor"]["block"], 3);
    assert_eq!(a["session_id"], "ses_9");
    assert!((a["cost_usd"].as_f64().unwrap() - 0.12).abs() < 1e-9);
    let (_, theirs) = call(&router, "GET", &path, &viewer, None).await;
    let users = theirs["users"].as_array().unwrap();
    assert_eq!(users.len(), 2);
    let person = users.iter().find(|u| u["user_id"] == "ap_owner").unwrap();
    assert_eq!(person["kind"], "user");
    assert!(person["task"].is_null());

    // Long tasks are cut to 200 characters; "left" removes only the agent row.
    call(&router, "POST", &path, &owner,
        Some(json!({"state":"viewing","agent":{"name":"Research Bot","session_id":"ses_9","task":"t".repeat(300)}}))).await;
    let (_, mine) = call(&router, "GET", &path, &owner, None).await;
    assert_eq!(mine["users"][0]["task"].as_str().unwrap().chars().count(), 200);
    assert!(mine["users"][0]["anchor"].is_null());
    call(&router, "POST", &path, &owner,
        Some(json!({"state":"left","agent":{"name":"Research Bot","session_id":"ses_9"}}))).await;
    let (_, theirs) = call(&router, "GET", &path, &viewer, None).await;
    let users = theirs["users"].as_array().unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0]["user_id"], "ap_owner");
}

#[tokio::test]
async fn agent_comments_post_under_the_agent_name() {
    let (router, _db) = setup().await;
    let owner = who("ac_owner");
    let commenter = who("ac_commenter");
    // Docs share as Viewer or Editor only, so use a page for a comment-level share.
    let id = make(&router, &owner, "page").await;
    share(&router, &owner, &id, "ac_commenter", "comment").await;
    let base = format!("/api/v2/artifacts/{id}/comments");

    // Posting as an agent needs edit access.
    let (st, _) = call(&router, "POST", &base, &commenter, Some(json!({"body":"q?","agent_name":"Gizzi"}))).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    let (st, c) = call(&router, "POST", &base, &owner,
        Some(json!({"body":"Which date did you mean, @gizzi?","agent_name":"Research Bot"}))).await;
    assert_eq!(st, StatusCode::CREATED, "{c}");
    assert_eq!(c["author_id"], "assistant");
    assert_eq!(c["author_name"], "Research Bot");
    assert_eq!(c["agent_name"], "Research Bot");
    assert_eq!(c["to_assistant"], false, "an agent's question never prompts the server assistant");
    assert!(c.get("assistant_reply").is_none());

    let (st, plain) = call(&router, "POST", &base, &owner, Some(json!({"body":"a person"}))).await;
    assert_eq!(st, StatusCode::CREATED);
    assert!(plain["agent_name"].is_null());
    assert_eq!(plain["author_id"], "ac_owner");

    let (_, list) = call(&router, "GET", &base, &commenter, None).await;
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["author_name"], "Research Bot");
    assert_eq!(items[0]["agent_name"], "Research Bot");
}

// ----- Agents on one canvas: artboard changes (SPEC-agent-canvas-2026-10-20) -----

fn layer(id: &str, artboard: &str) -> Value {
    json!({"id": id, "type": "text", "name": id, "artboard": artboard, "x": 0, "y": 0, "w": 10, "h": 10, "props": {"text": "Hi"}})
}

fn artboard_change(artboard: &str, layers: Vec<Value>, before: Vec<Value>) -> Value {
    json!({"kind": "artboard", "artboard": artboard, "name": "Hero", "layers": layers, "before": {"layers": before}})
}

#[test]
fn artboard_change_validation() {
    let ok = validate_change(
        "design",
        &json!({
            "kind": "artboard", "artboard": "shape:hero", "name": " Hero ", "extra": 1,
            "frame": {"fill": "#0b0b0f", "w": 1440, "x": 9, "props": {"radius": 8}},
            "layers": [layer("shape:t1", "shape:hero"), layer("t2", "hero")],
            "before": {"frame": {"fill": "#ffffff"}, "layers": [layer("shape:t1", "shape:hero")], "junk": true},
        }),
    )
    .unwrap();
    assert_eq!(ok["kind"], "artboard");
    assert_eq!(ok["artboard"], "shape:hero");
    assert_eq!(ok["name"], "Hero");
    assert!(ok.get("extra").is_none());
    assert_eq!(ok["frame"], json!({"fill": "#0b0b0f", "w": 1440, "props": {"radius": 8}}), "unknown frame fields are dropped");
    assert_eq!(ok["layers"].as_array().unwrap().len(), 2);
    assert_eq!(ok["layers"][0]["props"]["text"], "Hi", "layers are kept verbatim");
    assert_eq!(ok["before"], json!({"frame": {"fill": "#ffffff"}, "layers": [layer("shape:t1", "shape:hero")]}));
    // frame is optional; an empty layer list clears the artboard.
    let cleared = validate_change("design", &artboard_change("hero", vec![], vec![layer("a", "hero")])).unwrap();
    assert!(cleared.get("frame").is_none());

    // Shape errors.
    let bad = |c: Value| validate_change("design", &c).is_err();
    assert!(bad(json!({"kind":"artboard","artboard":"","layers":[],"before":{"layers":[]}})));
    assert!(bad(json!({"kind":"artboard","artboard":"shape:","layers":[],"before":{"layers":[]}})));
    assert!(bad(json!({"kind":"artboard","artboard":"hero","before":{"layers":[]}})), "layers is required");
    assert!(bad(json!({"kind":"artboard","artboard":"hero","layers":[]})), "before is required");
    assert!(bad(artboard_change("hero", vec![layer("a", "other")], vec![])), "a layer on another artboard");
    assert!(bad(artboard_change("hero", vec![], vec![layer("a", "other")])), "before holds the same artboard");
    assert!(bad(artboard_change("hero", vec![layer("a", "hero"), layer("shape:a", "hero")], vec![])), "duplicate ids");
    assert!(bad(artboard_change("hero", vec![json!({"id":"a","artboard":"hero"})], vec![])), "a layer needs a type");
    assert!(bad(artboard_change("hero", vec![json!("text")], vec![])));
    assert!(bad(json!({"kind":"artboard","artboard":"hero","frame":{"w":0},"layers":[],"before":{"layers":[]}})));
    assert!(bad(json!({"kind":"artboard","artboard":"hero","frame":{"fill":3},"layers":[],"before":{"layers":[]}})));
    assert!(bad(json!({"kind":"artboard","artboard":"hero","frame":"dark","layers":[],"before":{"layers":[]}})));

    // The kind matrix: artboard only on designs, and only artboard on designs.
    let code = |kind: &str, c: Value| match validate_change(kind, &c) {
        Err(ArtifactError::Coded { code, .. }) => code,
        other => panic!("expected a coded error, got {other:?}"),
    };
    let ab = artboard_change("hero", vec![], vec![]);
    assert_eq!(code("doc", ab.clone()), "unsupported_kind");
    assert_eq!(code("sheet", ab.clone()), "unsupported_kind");
    assert_eq!(code("page", ab), "unsupported_kind");
    assert_eq!(code("design", json!({"kind":"text","find":"a","replace":"b"})), "unsupported_kind");
    assert_eq!(code("design", json!({"kind":"cell","address":"B4","value":"x"})), "unsupported_kind");
    assert_eq!(code("doc", json!({"kind":"nope"})), "unsupported_kind");

    // Size limits: 2000 layers, 2 MB of JSON each for layers and before.
    let many: Vec<Value> = (0..2001).map(|i| layer(&format!("l{i}"), "hero")).collect();
    assert_eq!(code("design", artboard_change("hero", many.clone(), vec![])), "change_too_large");
    assert_eq!(code("design", artboard_change("hero", vec![], many)), "change_too_large");
    let most: Vec<Value> = (0..2000).map(|i| layer(&format!("l{i}"), "hero")).collect();
    assert!(validate_change("design", &artboard_change("hero", most, vec![])).is_ok());
    let mut huge = layer("big", "hero");
    huge["props"]["html"] = json!("x".repeat(2 * 1024 * 1024));
    assert_eq!(code("design", artboard_change("hero", vec![huge.clone()], vec![])), "change_too_large");
    assert_eq!(code("design", artboard_change("hero", vec![], vec![huge])), "change_too_large");
}

fn canvas_body() -> String {
    json!({
        "v": 2, "kind": "canvas", "designSystem": null,
        "artboards": [
            {"id": "shape:hero", "name": "Hero", "x": 0, "y": 0, "w": 1440, "h": 900, "fill": "#ffffff"},
            {"id": "pricing", "name": "Pricing", "x": 1600, "y": 0, "w": 1440, "h": 900, "fill": "#ffffff"},
        ],
        "layers": [layer("shape:h1", "shape:hero"), layer("p1", "pricing")],
    })
    .to_string()
}

#[test]
fn artboard_body_checks() {
    let body = canvas_body();
    let check = |c: Value| check_artboard_against_body(&body, &validate_change("design", &c).unwrap());
    // Ids match with or without tldraw's shape: prefix.
    assert!(check(artboard_change("hero", vec![layer("h1", "hero"), layer("h2", "hero")], vec![layer("h1", "hero")])).is_ok());
    assert!(check(artboard_change("shape:pricing", vec![layer("p1", "pricing")], vec![])).is_ok());
    let code = |r: Result<()>| match r {
        Err(ArtifactError::Coded { code, .. }) => code,
        other => panic!("expected a coded error, got {other:?}"),
    };
    assert_eq!(code(check(artboard_change("footer", vec![], vec![]))), "unknown_artboard");
    assert_eq!(code(check(artboard_change("hero", vec![layer("shape:p1", "hero")], vec![]))), "layer_id_in_use");
    // A body that isn't a canvas yet isn't checked.
    let c = validate_change("design", &artboard_change("hero", vec![], vec![])).unwrap();
    assert!(check_artboard_against_body("", &c).is_ok());
    assert!(check_artboard_against_body("<html></html>", &c).is_ok());
    assert!(check_artboard_against_body(r#"{"html":"<p>hi</p>"}"#, &c).is_ok());
}

async fn make_design(router: &Router, owner: &Value, body: &str, format: &str) -> String {
    let (st, created) = call(router, "POST", "/api/v2/artifacts", owner,
        Some(json!({"kind": "design", "title": "Landing", "body": body, "body_format": format}))).await;
    assert_eq!(st, StatusCode::CREATED, "{created}");
    created["id"].as_str().unwrap().to_string()
}

fn design_post(session: &str, agent: &str, change: Value) -> Value {
    json!({"base_version": 1, "agent_name": agent, "session_id": session, "cost_usd": 0.05,
           "changes": [{"change": change, "reason": "Made the heading dark"}]})
}

#[tokio::test]
async fn design_suggestions_kind_matrix_and_limits() {
    let (router, _db) = setup().await;
    let owner = who("ds_owner");
    let canvas = make_design(&router, &owner, &canvas_body(), "application/vnd.allternit.design+json").await;
    let base = format!("/api/v2/artifacts/{canvas}/suggestions");

    // A design canvas takes an artboard change and stores it normalised.
    let change = artboard_change("hero", vec![layer("h1", "hero")], vec![layer("h1", "hero")]);
    let (st, created) = call(&router, "POST", &base, &owner, Some(design_post("ses_a", "Oki", change))).await;
    assert_eq!(st, StatusCode::CREATED, "{created}");
    let item = &created["items"][0];
    assert_eq!(item["change"]["kind"], "artboard");
    assert_eq!(item["change"]["artboard"], "hero");
    assert_eq!(item["change"]["before"]["layers"][0]["id"], "h1");
    assert_eq!(item["reason"], "Made the heading dark");
    assert_eq!(item["status"], "pending");
    let (_, listed) = call(&router, "GET", &base, &owner, None).await;
    assert_eq!(listed["items"][0]["change"]["layers"][0]["props"]["text"], "Hi");

    // Any other pair is unsupported_kind.
    let (st, body) = call(&router, "POST", &base, &owner,
        Some(design_post("ses_a", "Oki", json!({"kind":"text","find":"a","replace":"b"})))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "unsupported_kind");
    let doc = make(&router, &owner, "doc").await;
    let (st, body) = call(&router, "POST", &format!("/api/v2/artifacts/{doc}/suggestions"), &owner,
        Some(design_post("ses_a", "Oki", artboard_change("hero", vec![], vec![])))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "unsupported_kind");
    let html = make_design(&router, &owner, "<main>Hi</main>", "text/html").await;
    let (st, body) = call(&router, "POST", &format!("/api/v2/artifacts/{html}/suggestions"), &owner,
        Some(design_post("ses_a", "Oki", artboard_change("hero", vec![], vec![])))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "unsupported_kind", "an HTML design isn't a canvas yet");

    // Size limits.
    let many: Vec<Value> = (0..2001).map(|i| layer(&format!("l{i}"), "hero")).collect();
    let (st, body) = call(&router, "POST", &base, &owner,
        Some(design_post("ses_a", "Oki", artboard_change("hero", many, vec![])))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "change_too_large");
    let mut huge = layer("big", "hero");
    huge["props"]["html"] = json!("x".repeat(2 * 1024 * 1024 + 10));
    let (st, body) = call(&router, "POST", &base, &owner,
        Some(design_post("ses_a", "Oki", artboard_change("hero", vec![huge], vec![])))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "change_too_large");

    // Checked against the canvas: the artboard must exist, ids stay unique.
    let (st, body) = call(&router, "POST", &base, &owner,
        Some(design_post("ses_a", "Oki", artboard_change("footer", vec![], vec![])))).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "unknown_artboard");
    let (st, body) = call(&router, "POST", &base, &owner,
        Some(design_post("ses_a", "Oki", artboard_change("hero", vec![layer("p1", "hero")], vec![])))).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "layer_id_in_use");
}

#[tokio::test]
async fn design_suggestions_respect_artboard_claims() {
    let (router, db) = setup().await;
    let owner = who("dc_owner");
    let canvas = make_design(&router, &owner, &canvas_body(), "application/vnd.allternit.design+json").await;
    let base = format!("/api/v2/artifacts/{canvas}/suggestions");
    let presence = format!("/api/v2/artifacts/{canvas}/presence");
    let claim = |session: &str, name: &str, artboard: Value| {
        json!({"state":"editing","agent":{"name":name,"session_id":session,"task":"Dark restyle","anchor":{"kind":"artboard","artboard":artboard}}})
    };
    let (st, _) = call(&router, "POST", &presence, &owner, Some(claim("ses_oki", "Oki", json!("shape:hero")))).await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    // Another session can't suggest on the held artboard; the holder can, and
    // anyone can on a free one.
    let hero = artboard_change("hero", vec![layer("h1", "hero")], vec![layer("h1", "hero")]);
    let (st, body) = call(&router, "POST", &base, &owner, Some(design_post("ses_ash", "Ash", hero.clone()))).await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "artboard_claimed");
    assert_eq!(body["holder"], json!({"name": "Oki", "session_id": "ses_oki"}));
    let (st, _) = call(&router, "POST", &base, &owner, Some(design_post("ses_oki", "Oki", hero.clone()))).await;
    assert_eq!(st, StatusCode::CREATED);
    let pricing = artboard_change("pricing", vec![], vec![layer("p1", "pricing")]);
    let (st, _) = call(&router, "POST", &base, &owner, Some(design_post("ses_ash", "Ash", pricing))).await;
    assert_eq!(st, StatusCode::CREATED);

    // The claim lapses with the heartbeat's 45 s TTL.
    sqlx::query("UPDATE artifact_presence SET last_seen = now() - interval '60 seconds'")
        .execute(&db).await.unwrap();
    let (st, _) = call(&router, "POST", &base, &owner, Some(design_post("ses_ash", "Ash", hero))).await;
    assert_eq!(st, StatusCode::CREATED);
}

// ----- Phase 2: generated sliders ("controls") on artboard changes -----

fn with_controls(controls: Value) -> Value {
    let mut c = artboard_change("hero", vec![layer("shape:card", "hero")], vec![layer("shape:card", "hero")]);
    c["controls"] = controls;
    c
}

#[test]
fn artboard_controls_validation() {
    let ok = validate_change(
        "design",
        &with_controls(json!([
            {"id":"radius","label":"Corner radius","type":"range","layer":"card","prop":"radius","min":0,"max":32,"step":1,"value":12,"junk":1},
            {"id":"accent","label":"Accent","type":"color","layer":"shape:card","prop":"fill","value":" #6366f1 "},
            {"id":"density","label":"Density","type":"select","layer":null,"prop":"frame.padding","options":["compact","cozy"],"value":"cozy"},
            {"id":"width","label":"Width","type":"range","prop":"w","min":200,"max":800,"value":400},
        ])),
    )
    .unwrap();
    let controls = ok["controls"].as_array().unwrap();
    assert_eq!(controls.len(), 4);
    assert_eq!(controls[0], json!({"id":"radius","label":"Corner radius","type":"range","layer":"card","prop":"radius","min":0,"max":32,"step":1,"value":12}));
    assert_eq!(controls[1]["value"], "#6366f1");
    assert_eq!(controls[2]["options"], json!(["compact","cozy"]));
    assert!(controls[3]["layer"].is_null(), "a missing layer targets the artboard");
    assert!(controls[3].get("step").is_none());
    // A controls-only change: the same layers as now, plus sliders.
    let same = validate_change("design", &with_controls(json!([{"id":"r","label":"R","type":"range","layer":"card","prop":"radius","min":0,"max":8,"value":4}])));
    assert!(same.is_ok());
    // No controls, or an empty list, stores none.
    assert!(validate_change("design", &with_controls(Value::Null)).unwrap().get("controls").is_none());
    assert!(validate_change("design", &with_controls(json!([]))).unwrap().get("controls").is_none());

    let code = |controls: Value| match validate_change("design", &with_controls(controls)) {
        Err(ArtifactError::Coded { code, .. }) => code,
        other => panic!("expected a coded error, got {other:?}"),
    };
    let range = |extra: Value| {
        let mut c = json!({"id":"r","label":"R","type":"range","layer":"card","prop":"radius","min":0,"max":32,"value":12});
        for (k, v) in extra.as_object().unwrap() {
            c[k] = v.clone();
        }
        json!([c])
    };
    assert_eq!(code(json!({"id":"r"})), "bad_controls", "not an array");
    assert_eq!(code(json!(["x"])), "bad_controls");
    assert_eq!(code(range(json!({"type":"knob"}))), "bad_controls");
    assert_eq!(code(range(json!({"id":""}))), "bad_controls");
    assert_eq!(code(range(json!({"label":3}))), "bad_controls");
    assert_eq!(code(range(json!({"prop":""}))), "bad_controls");
    assert_eq!(code(range(json!({"layer":"missing"}))), "bad_controls", "layer must be in the change");
    assert_eq!(code(range(json!({"layer":7}))), "bad_controls");
    assert_eq!(code(range(json!({"prop":"frame.padding"}))), "bad_controls", "frame. needs layer null");
    assert_eq!(code(range(json!({"layer":null,"prop":"frame."}))), "bad_controls");
    assert_eq!(code(range(json!({"min":32,"max":0}))), "bad_controls");
    assert_eq!(code(range(json!({"max":"32"}))), "bad_controls");
    assert_eq!(code(range(json!({"step":0}))), "bad_controls");
    assert_eq!(code(range(json!({"step":64}))), "bad_controls");
    assert_eq!(code(range(json!({"value":40}))), "bad_controls");
    assert_eq!(code(range(json!({"value":"12"}))), "bad_controls");
    assert_eq!(code(json!([{"id":"c","label":"C","type":"color","layer":"card","prop":"fill","value":""}])), "bad_controls");
    assert_eq!(code(json!([{"id":"s","label":"S","type":"select","layer":null,"prop":"frame.d","options":[],"value":"a"}])), "bad_controls");
    assert_eq!(code(json!([{"id":"s","label":"S","type":"select","layer":null,"prop":"frame.d","options":["a",3],"value":"a"}])), "bad_controls");
    assert_eq!(code(json!([{"id":"s","label":"S","type":"select","layer":null,"prop":"frame.d","options":["a","b"],"value":"c"}])), "bad_controls");
    let dup = json!([
        {"id":"r","label":"R","type":"range","layer":"card","prop":"radius","min":0,"max":32,"value":12},
        {"id":"r","label":"R2","type":"color","layer":"card","prop":"fill","value":"#000"},
    ]);
    assert_eq!(code(dup), "bad_controls", "ids are unique");
    // At most 12 controls and 64 KB of JSON.
    let twelve: Vec<Value> = (0..12)
        .map(|i| json!({"id":format!("c{i}"),"label":"C","type":"color","layer":"card","prop":"fill","value":"#000"}))
        .collect();
    assert!(validate_change("design", &with_controls(json!(twelve))).is_ok());
    let thirteen: Vec<Value> = (0..13)
        .map(|i| json!({"id":format!("c{i}"),"label":"C","type":"color","layer":"card","prop":"fill","value":"#000"}))
        .collect();
    assert_eq!(code(json!(thirteen)), "bad_controls");
    let long = vec![json!({"id":"s","label":"S","type":"select","layer":null,"prop":"frame.d",
        "options":["a"], "value":"a", "pad":"y".repeat(70 * 1024)})];
    assert_eq!(code(json!(long)), "bad_controls");
}

#[tokio::test]
async fn design_suggestions_store_controls_and_refuse_bad_ones() {
    let (router, _db) = setup().await;
    let owner = who("ctl_owner");
    let canvas = make_design(&router, &owner, &canvas_body(), "application/vnd.allternit.design+json").await;
    let base = format!("/api/v2/artifacts/{canvas}/suggestions");
    let mut change = artboard_change("hero", vec![layer("h1", "hero")], vec![layer("h1", "hero")]);
    change["controls"] = json!([
        {"id":"radius","label":"Corner radius","type":"range","layer":"h1","prop":"radius","min":0,"max":32,"step":1,"value":12},
        {"id":"density","label":"Density","type":"select","layer":null,"prop":"frame.padding","options":["compact","cozy"],"value":"cozy"},
    ]);
    let (st, created) = call(&router, "POST", &base, &owner, Some(design_post("ses_c", "Oki", change.clone()))).await;
    assert_eq!(st, StatusCode::CREATED, "{created}");
    assert_eq!(created["items"][0]["change"]["controls"][0]["id"], "radius");
    let (_, listed) = call(&router, "GET", &base, &owner, None).await;
    assert_eq!(listed["items"][0]["change"]["controls"][1]["value"], "cozy");

    change["controls"][0]["value"] = json!(99);
    let (st, body) = call(&router, "POST", &base, &owner, Some(design_post("ses_c", "Oki", change))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "bad_controls");
}
