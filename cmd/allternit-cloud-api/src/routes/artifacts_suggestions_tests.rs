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
