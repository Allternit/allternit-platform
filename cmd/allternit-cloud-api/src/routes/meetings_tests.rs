//! Route tests for Allternit Meetings (docs/meetings/CONTRACT.md). Each test
//! gets its own Postgres schema (routes::test_support) with migration 130
//! applied, plus 085/109 (artifacts v2) for the artifact tools and a minimal
//! `billing_subscriptions` for the plan lookup. Clerk-session callers are
//! stood in for by the test-only `x-test-caller` header; pods use real pod
//! tokens from `issue_pod_token`.

use super::*;
use crate::routes::meeting_pod::issue_pod_token;
use crate::routes::test_support::{test_state, MockGateway};
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use tower::ServiceExt;

const MEET: &str = "https://meet.google.com/abc-defg-hij";

async fn setup() -> (Router, PgPool) {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    for sql in [
        include_str!("../../migrations_pg/085_artifacts_v2.sql"),
        include_str!("../../migrations_pg/109_artifact_library_archive.sql"),
        include_str!("../../migrations_pg/130_meetings.sql"),
        "CREATE TABLE billing_subscriptions (user_id TEXT, plan_id TEXT, status TEXT, updated_at TIMESTAMPTZ DEFAULT now())",
    ] {
        sqlx::raw_sql(sql).execute(&state.db).await.expect("schema applies");
    }
    let db = state.db.clone();
    (routes().with_state(state), db)
}

async fn subscribe(db: &PgPool, user: &str, plan: &str) {
    sqlx::query("INSERT INTO billing_subscriptions (user_id, plan_id, status) VALUES ($1, $2, 'active')")
        .bind(user)
        .bind(plan)
        .execute(db)
        .await
        .unwrap();
}

fn who(id: &str) -> Value {
    json!({ "id": id })
}

fn org_who(id: &str, org: &str, role: &str) -> Value {
    json!({ "id": id, "org_id": org, "org_role": role })
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

fn request(method: &str, path: &str, header: (&str, String), body: Option<Value>) -> Request<Body> {
    let builder = Request::builder().method(method).uri(path).header(header.0, header.1);
    match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

async fn call(router: &Router, method: &str, path: &str, caller: &Value, body: Option<Value>) -> (StatusCode, Value) {
    send(router, request(method, path, ("x-test-caller", caller.to_string()), body)).await
}

async fn pod(router: &Router, path: &str, token: &str, body: Value) -> (StatusCode, Value) {
    send(router, request("POST", path, ("authorization", format!("Bearer {token}")), Some(body))).await
}

async fn add(router: &Router, caller: &Value, url: &str) -> String {
    let (status, body) = call(router, "POST", "/api/v1/meetings", caller, Some(json!({ "url": url }))).await;
    assert!(status == StatusCode::CREATED || status == StatusCode::OK, "{status} {body}");
    body["id"].as_str().unwrap().to_string()
}

async fn send_now(router: &Router, caller: &Value, meeting: &str) -> (StatusCode, Value) {
    call(router, "POST", &format!("/api/v1/meetings/{meeting}/sessions"), caller, Some(json!({ "when": "now" }))).await
}

#[tokio::test]
async fn free_plan_gets_402_on_every_route_except_reading_settings() {
    let (router, db) = setup().await;
    let free = who("user_free");
    // A paid user's meeting, so the free caller's 402 comes before any lookup.
    subscribe(&db, "user_paid", "plus").await;
    let meeting = add(&router, &who("user_paid"), MEET).await;
    for (method, path, body) in [
        ("GET", "/api/v1/meetings".to_string(), None),
        ("POST", "/api/v1/meetings".to_string(), Some(json!({ "url": MEET }))),
        ("GET", format!("/api/v1/meetings/{meeting}"), None),
        ("PATCH", format!("/api/v1/meetings/{meeting}"), Some(json!({ "title": "x" }))),
        ("POST", format!("/api/v1/meetings/{meeting}/sessions"), Some(json!({ "when": "now" }))),
        ("GET", format!("/api/v1/meeting-sessions/{}", Uuid::new_v4()), None),
        ("PUT", "/api/v1/me/meeting-settings".to_string(), Some(json!({ "auto_join": "all" }))),
        ("GET", "/api/v1/me/meeting-usage".to_string(), None),
        ("GET", "/api/v1/orgs/org_1/meeting-settings".to_string(), None),
    ] {
        let (status, body) = call(&router, method, &path, &free, body).await;
        assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{method} {path}: {body}");
        assert_eq!(body["error"], "meetings_requires_paid_plan", "{method} {path}");
    }
    let (status, body) = call(&router, "GET", "/api/v1/me/meeting-settings", &free, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "auto_join": "off", "recap_email": true, "default_reply_mode": "chat" }));
}

#[tokio::test]
async fn adding_a_link_dedupes_by_url_key_and_start_and_restores_deleted_rows() {
    let (router, db) = setup().await;
    let me = who("user_a");
    subscribe(&db, "user_a", "super").await;

    let (status, first) = call(&router, "POST", "/api/v1/meetings", &me, Some(json!({ "url": MEET, "title": "Standup" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    assert_eq!(first["url_key"], "meet:abc-defg-hij");
    assert_eq!(first["platform_label"], "Google Meet");
    assert_eq!(first["gizzi"], "off");
    assert_eq!(first["page_url"], format!("https://ai.allternit.com/meetings/{}", first["id"].as_str().unwrap()));

    // Another spelling of the same call is the same row.
    let (status, again) = call(&router, "POST", "/api/v1/meetings", &me, Some(json!({ "url": "MEET.google.com/ABC-DEFG-HIJ?pli=1" }))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["id"], first["id"]);

    // A start time makes a different occurrence.
    let (status, dated) = call(
        &router,
        "POST",
        "/api/v1/meetings",
        &me,
        Some(json!({ "url": MEET, "starts_at": "2030-01-02T15:00:00Z" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_ne!(dated["id"], first["id"]);
    assert_eq!(dated["gizzi"], "off", "rule is off by default");

    let (status, body) = call(&router, "POST", "/api/v1/meetings", &me, Some(json!({ "url": "https://example.com/call" }))).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::BAD_REQUEST, Some("unsupported_link")));

    let id = first["id"].as_str().unwrap();
    let (status, _) = call(&router, "DELETE", &format!("/api/v1/meetings/{id}"), &me, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, list) = call(&router, "GET", "/api/v1/meetings?range=upcoming", &me, None).await;
    assert!(list["meetings"].as_array().unwrap().iter().all(|m| m["id"] != first["id"]), "deleted row hidden");
    let (status, restored) = call(&router, "POST", "/api/v1/meetings", &me, Some(json!({ "url": MEET }))).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(restored["id"], first["id"], "re-adding restores the soft-deleted row");
}

#[tokio::test]
async fn get_meeting_shows_gizzi_and_the_latest_session_to_its_owner_only() {
    let (router, db) = setup().await;
    let me = who("user_a");
    subscribe(&db, "user_a", "plus").await;
    subscribe(&db, "user_b", "plus").await;
    let id = add(&router, &me, MEET).await;

    let (status, body) = call(&router, "GET", &format!("/api/v1/meetings/{id}"), &me, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!((body["gizzi"].as_str(), body["latest_session"].is_null()), (Some("off"), true));

    let (_, session) = send_now(&router, &me, &id).await;
    sqlx::query("UPDATE meeting_sessions SET summary = 'Shipped it.' WHERE id = $1")
        .bind(Uuid::parse_str(session["id"].as_str().unwrap()).unwrap())
        .execute(&db)
        .await
        .unwrap();
    let (_, body) = call(&router, "GET", &format!("/api/v1/meetings/{id}"), &me, None).await;
    assert_eq!(body["gizzi"], "launching");
    assert_eq!(body["latest_session"]["id"], session["id"]);
    assert_eq!(body["latest_session"]["summary"], "Shipped it.");
    let mut keys: Vec<&str> = body["latest_session"].as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["end_reason", "ended_at", "id", "live_at", "state", "summary"]);
    assert!(body["live_at"].is_null(), "not live yet");
    sqlx::query("UPDATE meeting_sessions SET state = 'joining' WHERE id = $1")
        .bind(Uuid::parse_str(session["id"].as_str().unwrap()).unwrap())
        .execute(&db)
        .await
        .unwrap();
    let token = issue_pod_token(&db, Uuid::parse_str(session["id"].as_str().unwrap()).unwrap()).await.unwrap();
    let ingest = format!("/api/v1/meeting-sessions/{}/ingest", session["id"].as_str().unwrap());
    assert_eq!(pod(&router, &ingest, &token, json!({ "state": { "state": "live" } })).await.0, StatusCode::OK);
    let (_, body) = call(&router, "GET", &format!("/api/v1/meetings/{id}"), &me, None).await;
    assert!(body["live_at"].is_string(), "live call carries live_at: {body}");
    assert_eq!(body["latest_session"]["live_at"], body["live_at"]);

    let (status, body) = call(&router, "GET", &format!("/api/v1/meetings/{id}"), &who("user_b"), None).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::NOT_FOUND, Some("meeting_not_found")));
    let (status, _) = call(&router, "GET", "/api/v1/meetings/not-a-uuid", &me, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn one_bot_per_url_later_requesters_attach_but_cannot_control() {
    let (router, db) = setup().await;
    let (a, b) = (who("user_a"), who("user_b"));
    subscribe(&db, "user_a", "plus").await;
    subscribe(&db, "user_b", "plus").await;
    let ma = add(&router, &a, MEET).await;
    let mb = add(&router, &b, "https://meet.google.com/abc-defg-hij?authuser=2").await;

    let (status, s1) = send_now(&router, &a, &ma).await;
    assert_eq!(status, StatusCode::CREATED, "{s1}");
    assert_eq!((s1["state"].as_str(), s1["requested_by"].as_str()), (Some("launching"), Some("user_a")));
    let (status, s2) = send_now(&router, &b, &mb).await;
    assert_eq!(status, StatusCode::OK, "attach, not a second bot: {s2}");
    assert_eq!(s2["id"], s1["id"]);

    let sid = s1["id"].as_str().unwrap();
    let (status, view) = call(&router, "GET", &format!("/api/v1/meeting-sessions/{sid}"), &b, None).await;
    assert_eq!(status, StatusCode::OK, "attached requester can watch");
    assert_eq!(view["meeting"]["id"], mb.as_str(), "their own meeting row");
    let (status, body) = call(&router, "POST", &format!("/api/v1/meeting-sessions/{sid}/control"), &b, Some(json!({ "action": "leave" }))).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::FORBIDDEN, Some("forbidden")));
    let (status, _) = call(&router, "GET", &format!("/api/v1/meeting-sessions/{sid}"), &who("user_c"), None).await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "user_c is on free");
    subscribe(&db, "user_c", "plus").await;
    let (status, body) = call(&router, "GET", &format!("/api/v1/meeting-sessions/{sid}"), &who("user_c"), None).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::NOT_FOUND, Some("session_not_found")));

    // Stop listening only applies to a live call.
    let (status, body) = call(&router, "POST", &format!("/api/v1/meeting-sessions/{sid}/control"), &a, Some(json!({ "action": "stop_listening" }))).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::CONFLICT, Some("conflict")));
    // A meeting with a bot in it can't be deleted.
    let (status, _) = call(&router, "DELETE", &format!("/api/v1/meetings/{ma}"), &a, None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (_, live_row) = call(&router, "GET", &format!("/api/v1/meetings/{ma}"), &a, None).await;
    assert!(live_row["recap"].is_null(), "no recap while the bot is in the call");
    // Leave ends the session for everyone; the next request starts a new one.
    let (status, ended) = call(&router, "POST", &format!("/api/v1/meeting-sessions/{sid}/control"), &a, Some(json!({ "action": "leave" }))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!((ended["state"].as_str(), ended["end_reason"].as_str()), (Some("ended"), Some("left_by_user")));
    sqlx::query(
        "UPDATE meeting_sessions SET summary = 'Done.', decisions = '[\"a\",\"b\"]', \
         action_items = '[{\"text\":\"x\",\"owner\":null,\"due\":null}]' WHERE id = $1",
    )
    .bind(Uuid::parse_str(sid).unwrap())
    .execute(&db)
    .await
    .unwrap();
    let (_, past) = call(&router, "GET", "/api/v1/meetings?range=past", &a, None).await;
    let row = past["meetings"].as_array().unwrap().iter().find(|m| m["id"] == ma.as_str()).expect("ended meeting is past");
    assert_eq!(row["recap"], json!({ "ready": true, "decisions": 2, "action_items": 1 }));
    assert!(row["live_at"].is_null());
    let (status, s3) = send_now(&router, &b, &mb).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_ne!(s3["id"], s1["id"]);
}

#[tokio::test]
async fn concurrent_bots_are_capped_per_plan() {
    let (router, db) = setup().await;
    let me = who("user_a");
    subscribe(&db, "user_a", "plus").await; // plus: 1 at a time
    let m1 = add(&router, &me, MEET).await;
    let m2 = add(&router, &me, "https://zoom.us/j/85512345678").await;
    assert_eq!(send_now(&router, &me, &m1).await.0, StatusCode::CREATED);
    // Zoom needs the user's connected Zoom account first.
    let (status, body) = send_now(&router, &me, &m2).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::CONFLICT, Some("zoom_not_connected")));
    sqlx::query("INSERT INTO zoom_connections (user_id, encrypted_refresh_token) VALUES ('user_a', 'sealed')")
        .execute(&db)
        .await
        .unwrap();
    let (status, body) = send_now(&router, &me, &m2).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("no_capacity")));
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meeting_sessions").fetch_one(&db).await.unwrap();
    assert_eq!(count, 1, "nothing created on 429");
    // Scheduling for the start holds no pod, so it is not capped.
    sqlx::query("UPDATE meetings SET starts_at = now() + interval '1 day' WHERE id = $1")
        .bind(Uuid::parse_str(&m2).unwrap())
        .execute(&db)
        .await
        .unwrap();
    let (status, body) = call(&router, "POST", &format!("/api/v1/meetings/{m2}/sessions"), &me, Some(json!({ "when": "start" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["state"], "scheduled");
    assert!(body["scheduled_join_at"].is_string());
}

#[tokio::test]
async fn pod_ingest_is_idempotent_enforces_the_state_machine_and_meters_once() {
    let (router, db) = setup().await;
    let me = who("user_a");
    subscribe(&db, "user_a", "plus").await;
    let meeting = add(&router, &me, MEET).await;
    let (_, session) = send_now(&router, &me, &meeting).await;
    let sid = Uuid::parse_str(session["id"].as_str().unwrap()).unwrap();
    let token = issue_pod_token(&db, sid).await.unwrap();
    let ingest = format!("/api/v1/meeting-sessions/{sid}/ingest");

    let (status, _) = pod(&router, &ingest, "mpt_wrong", json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // A user session can't post as the pod.
    let (status, _) = call(&router, "POST", &ingest, &me, Some(json!({}))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let batch = json!({
        "batch_id": Uuid::new_v4(),
        "state": { "state": "joining" },
        "participants": [{ "platform_participant_id": "p1", "display_name": "Jordan", "class": "guest", "joined_at": "2026-11-13T15:00:00Z" }],
        "transcript": [
            { "seq": 1, "idempotency_key": "seg-1", "speaker": "Jordan", "confidence": 0.8, "source": "audio", "start_ms": 0, "end_ms": 900, "text": "Morning." },
            { "seq": 2, "idempotency_key": "seg-2", "speaker": "Ana", "confidence": 0.4, "source": "captions", "start_ms": 900, "end_ms": 1800, "text": "Hi." }
        ],
        "chat": [{ "idempotency_key": "meet:m1", "direction": "in", "sender": "Jordan", "text": "Gizzi, notes please", "at": "2026-11-13T15:00:05Z" }],
        "summary": { "summary": "Agreed on Friday.", "decisions": ["Ship Friday"], "action_items": [{ "text": "QA pass", "owner": "Ana" }] },
        "screen_notes": [
            { "idempotency_key": "scr-1", "at_ms": 1000, "description": "Slide 1" },
            { "idempotency_key": "scr-2", "at_ms": 2000, "description": "Slide 2" }
        ],
        "notice_at": "2026-11-13T15:00:01Z"
    });
    let (status, counts) = pod(&router, &ingest, &token, batch.clone()).await;
    assert_eq!(status, StatusCode::OK, "{counts}");
    assert_eq!(counts, json!({ "accepted": 4, "duplicates": 0, "dropped": 1 }), "2 segments, 1 chat, 1 note; 2nd note over the cap");

    // A retry of the same batch is all duplicates (the capped note too, since
    // it is still within 30 s, is dropped again).
    let (status, counts) = pod(&router, &ingest, &token, batch).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(counts, json!({ "accepted": 0, "duplicates": 4, "dropped": 1 }));

    // Illegal transition: joining -> paused is a 409; the batch's rows stay.
    let (status, body) = pod(
        &router,
        &ingest,
        &token,
        json!({ "state": { "state": "paused" }, "transcript": [{ "seq": 3, "idempotency_key": "seg-3", "start_ms": 1800, "end_ms": 2000, "text": "Next." }] }),
    )
    .await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::CONFLICT, Some("conflict")), "{body}");
    assert_eq!(body["accepted"], 1);
    let state: String = sqlx::query_scalar("SELECT state FROM meeting_sessions WHERE id = $1").bind(sid).fetch_one(&db).await.unwrap();
    assert_eq!(state, "joining", "illegal state ignored");
    // Ending needs a reason.
    let (status, _) = pod(&router, &ingest, &token, json!({ "state": { "state": "ended" } })).await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (status, page) = call(&router, "GET", &format!("/api/v1/meeting-sessions/{sid}/transcript?after_seq=1&limit=1"), &me, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["segments"].as_array().unwrap().len(), 1);
    assert_eq!((page["segments"][0]["seq"].as_i64(), page["next_after_seq"].as_i64()), (Some(2), Some(2)));
    let (_, view) = call(&router, "GET", &format!("/api/v1/meeting-sessions/{sid}"), &me, None).await;
    assert_eq!(view["session"]["summary"], "Agreed on Friday.");
    assert_eq!(view["session"]["decisions"], json!(["Ship Friday"]));
    assert_eq!(view["session"]["action_items"], json!([{ "text": "QA pass", "owner": "Ana", "due": null }]));
    let (status, _) = pod(&router, &ingest, &token, json!({ "chat": [{ "idempotency_key": "out-1", "direction": "out", "sender": "Gizzi", "text": "Noted.", "visibility": "page_only" }] })).await;
    assert_eq!(status, StatusCode::OK);
    let visibility: String = sqlx::query_scalar("SELECT visibility FROM meeting_chat_messages WHERE idempotency_key = 'out-1'")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(visibility, "page_only");
    let (status, _) = pod(&router, &ingest, &token, json!({ "chat": [{ "idempotency_key": "out-2", "sender": "Gizzi", "text": "x", "visibility": "loud" }] })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(view["participants"][0]["display_name"], "Jordan");
    assert!(view["participants"][0]["notice_at"].is_string(), "notice recorded for people present");

    // live, then ended: the meter row is written once.
    assert_eq!(pod(&router, &ingest, &token, json!({ "state": { "state": "live" } })).await.0, StatusCode::OK);
    sqlx::query("UPDATE meeting_sessions SET live_at = now() - interval '90 seconds' WHERE id = $1").bind(sid).execute(&db).await.unwrap();
    let end = json!({ "state": { "state": "ended", "end_reason": "empty_room" } });
    assert_eq!(pod(&router, &ingest, &token, end.clone()).await.0, StatusCode::OK);
    assert_eq!(pod(&router, &ingest, &token, end).await.0, StatusCode::OK, "repeat end is a no-op");
    meter_session(&db, sid).await.unwrap();
    let (rows, seconds): (i64, i64) =
        sqlx::query_as("SELECT COUNT(*), COALESCE(SUM(seconds), 0)::bigint FROM meeting_bot_minutes WHERE session_id = $1")
            .bind(sid)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(rows, 1);
    assert!((90..=95).contains(&seconds), "{seconds}");
    let (_, usage) = call(&router, "GET", "/api/v1/me/meeting-usage", &me, None).await;
    assert_eq!((usage["used_minutes"].as_i64(), usage["plan"].as_str()), (Some(2), Some("plus")), "{usage}");

    // An ended session can't go live again; the token dies 5 min after the end.
    let (status, _) = pod(&router, &ingest, &token, json!({ "state": { "state": "live" } })).await;
    assert_eq!(status, StatusCode::CONFLICT);
    sqlx::query("UPDATE meeting_sessions SET ended_at = now() - interval '6 minutes' WHERE id = $1").bind(sid).execute(&db).await.unwrap();
    assert_eq!(pod(&router, &ingest, &token, json!({})).await.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn personal_tools_need_the_owners_approval_and_artifact_tools_run_once_per_call() {
    let (router, db) = setup().await;
    let (a, b) = (who("user_a"), who("user_b"));
    subscribe(&db, "user_a", "plus").await;
    subscribe(&db, "user_b", "plus").await;
    let ma = add(&router, &a, MEET).await;
    let mb = add(&router, &b, MEET).await;
    let (_, session) = send_now(&router, &a, &ma).await;
    send_now(&router, &b, &mb).await;
    let sid = session["id"].as_str().unwrap().to_string();
    let token = issue_pod_token(&db, Uuid::parse_str(&sid).unwrap()).await.unwrap();
    let tools = format!("/api/v1/meeting-sessions/{sid}/tool-calls");
    let gmail = json!({ "call_id": "c1", "tool": "gmail__send", "args": { "to": "x@y.z" }, "asked_by": "Jordan", "speaker_confidence": 0.9 });

    let (status, first) = pod(&router, &tools, &token, gmail.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["status"], "approval_required");
    let (_, poll) = pod(&router, &tools, &token, gmail.clone()).await;
    assert_eq!(poll["approval_id"], first["approval_id"], "polling reuses the request");

    let (_, unclear) = pod(&router, &tools, &token, json!({ "call_id": "c2", "tool": "gmail__send", "speaker_confidence": 0.2 })).await;
    assert_eq!(unclear["status"], "clarify");

    let (_, owner_view) = call(&router, "GET", &format!("/api/v1/meeting-sessions/{sid}"), &a, None).await;
    assert_eq!(owner_view["pending_approvals"][0]["id"], first["approval_id"]);
    let (_, other_view) = call(&router, "GET", &format!("/api/v1/meeting-sessions/{sid}"), &b, None).await;
    assert_eq!(other_view["pending_approvals"], json!([]), "only the owner sees approvals");

    let aid = first["approval_id"].as_str().unwrap();
    let approve = format!("/api/v1/meeting-sessions/{sid}/approvals/{aid}");
    let (status, _) = call(&router, "POST", &approve, &b, Some(json!({ "decision": "approve" }))).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "an attached requester can't approve the owner's tools");
    let (status, approval) = call(&router, "POST", &approve, &a, Some(json!({ "decision": "approve" }))).await;
    assert_eq!((status, approval["status"].as_str()), (StatusCode::OK, Some("approved")));
    let (status, _) = call(&router, "POST", &approve, &a, Some(json!({ "decision": "deny" }))).await;
    assert_eq!(status, StatusCode::CONFLICT, "already answered");
    // Approved: it runs in cloud through the owner's connectors. This owner
    // has no paired runtime, so nothing is reachable and nothing is stored.
    let (_, unreachable) = pod(&router, &tools, &token, gmail.clone()).await;
    assert_eq!(unreachable, json!({ "status": "error", "error": "connectors_unavailable" }));
    let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meeting_tool_calls").fetch_one(&db).await.unwrap();
    assert_eq!(stored, 0, "an unreachable run can be retried");
    // A call that already ran returns its stored result, not a second run.
    sqlx::query(
        "INSERT INTO meeting_tool_calls (session_id, call_id, tool, status, result) \
         VALUES ($1, 'c1', 'gmail__send', 'ok', '{\"status\":\"ok\",\"result\":{\"sent\":true}}')",
    )
    .bind(Uuid::parse_str(&sid).unwrap())
    .execute(&db)
    .await
    .unwrap();
    let (_, again) = pod(&router, &tools, &token, gmail).await;
    assert_eq!(again, json!({ "status": "ok", "result": { "sent": true } }));
    let (status, _) = pod(&router, &tools, &token, json!({ "call_id": "c1", "tool": "slack__post" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "a call_id belongs to one tool");

    let doc = json!({ "call_id": "d1", "tool": "create_doc", "args": { "title": "Notes", "body": "# Notes" } });
    let (status, created) = pod(&router, &tools, &token, doc.clone()).await;
    assert_eq!((status, created["status"].as_str()), (StatusCode::OK, Some("ok")), "{created}");
    let (_, retried) = pod(&router, &tools, &token, doc).await;
    assert_eq!(retried["result"]["artifact"]["artifact_id"], created["result"]["artifact"]["artifact_id"]);
    let artifact_id = created["result"]["artifact"]["artifact_id"].as_str().unwrap().to_string();
    let update = json!({ "call_id": "u1", "tool": "update_artifact", "args": { "artifact_id": artifact_id, "body": "# Notes v2" } });
    let (_, updated) = pod(&router, &tools, &token, update.clone()).await;
    assert_eq!(updated["result"]["artifact"]["version"], 2);
    let (_, updated) = pod(&router, &tools, &token, update).await;
    assert_eq!(updated["result"]["artifact"]["version"], 2, "same call, no second version");
    let (_, list) = call(&router, "GET", &format!("/api/v1/meeting-sessions/{sid}/artifacts"), &b, None).await;
    assert_eq!(list["artifacts"].as_array().unwrap().len(), 1);
    let (owner, format): (String, String) = sqlx::query_as(
        "SELECT a.owner_id, v.body_format FROM artifacts a JOIN artifact_versions v ON v.artifact_id = a.id \
         WHERE a.id = $1 AND v.version = 2",
    )
    .bind(&artifact_id)
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!((owner.as_str(), format.as_str()), ("user_a", "text/markdown"));
}

#[tokio::test]
async fn org_settings_gate_meetings_and_only_admins_change_them() {
    let (router, db) = setup().await;
    let admin = org_who("user_admin", "org_1", "admin");
    let member = org_who("user_member", "org_1", "org:member");
    for user in ["user_admin", "user_member", "user_out"] {
        subscribe(&db, user, "plus").await;
    }

    let (status, defaults) = call(&router, "GET", "/api/v1/orgs/org_1/meeting-settings", &member, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!((defaults["enabled"].as_bool(), defaults["guest_quiet"].as_bool()), (Some(true), Some(true)));
    let (status, _) = call(&router, "GET", "/api/v1/orgs/org_1/meeting-settings", &who("user_out"), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "non-member");
    let (status, _) = call(&router, "PUT", "/api/v1/orgs/org_1/meeting-settings", &member, Some(json!({ "enabled": false }))).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "members can't change settings");

    let (status, saved) = call(&router, "PUT", "/api/v1/orgs/org_1/meeting-settings", &admin, Some(json!({ "roles": ["admin"] }))).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["roles"], json!(["org:admin"]));
    let (status, body) = call(&router, "POST", "/api/v1/meetings", &member, Some(json!({ "url": MEET }))).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::FORBIDDEN, Some("forbidden")), "role not allowed");
    assert_eq!(call(&router, "POST", "/api/v1/meetings", &admin, Some(json!({ "url": MEET }))).await.0, StatusCode::CREATED);

    let (status, _) = call(&router, "PUT", "/api/v1/orgs/org_1/meeting-settings", &admin, Some(json!({ "enabled": false }))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call(&router, "POST", "/api/v1/meetings", &admin, Some(json!({ "url": "https://zoom.us/j/85512345678" }))).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::FORBIDDEN, Some("meetings_disabled_by_org")));
    let (status, _) = call(&router, "PUT", "/api/v1/orgs/org_1/meeting-settings", &admin, Some(json!({ "retention_days": 0 }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn user_settings_drive_the_scheduled_status() {
    let (router, db) = setup().await;
    let me = who("user_a");
    subscribe(&db, "user_a", "plus").await;
    let (status, saved) = call(&router, "PUT", "/api/v1/me/meeting-settings", &me, Some(json!({ "auto_join": "all_except_1on1" }))).await;
    assert_eq!((status, saved["auto_join"].as_str()), (StatusCode::OK, Some("all_except_1on1")));
    let (status, _) = call(&router, "PUT", "/api/v1/me/meeting-settings", &me, Some(json!({ "auto_join": "sometimes" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (_, future) = call(&router, "POST", "/api/v1/meetings", &me, Some(json!({ "url": MEET, "starts_at": "2030-01-02T15:00:00Z" }))).await;
    assert_eq!(future["gizzi"], "scheduled", "the rule auto-joins a future group call");
    let id = future["id"].as_str().unwrap();
    sqlx::query("UPDATE meetings SET attendee_count = 2, is_one_on_one = true WHERE id = $1")
        .bind(Uuid::parse_str(id).unwrap())
        .execute(&db)
        .await
        .unwrap();
    let (_, one_on_one) = call(&router, "GET", &format!("/api/v1/meetings/{id}"), &me, None).await;
    assert_eq!((one_on_one["gizzi"].as_str(), one_on_one["people_label"].as_str()), (Some("off"), Some("2 people")));
    let (_, forced) = call(&router, "PATCH", &format!("/api/v1/meetings/{id}"), &me, Some(json!({ "auto_join": true }))).await;
    assert_eq!(forced["gizzi"], "scheduled", "per-meeting override beats the rule");
    let (_, cleared) = call(&router, "PATCH", &format!("/api/v1/meetings/{id}"), &me, Some(json!({ "auto_join": null }))).await;
    assert_eq!((cleared["auto_join"].is_null(), cleared["gizzi"].as_str()), (true, Some("off")));
}

#[tokio::test]
async fn events_stream_opens_with_the_current_state() {
    let (router, db) = setup().await;
    let me = who("user_a");
    subscribe(&db, "user_a", "plus").await;
    let meeting = add(&router, &me, MEET).await;
    let (_, session) = send_now(&router, &me, &meeting).await;
    let sid = session["id"].as_str().unwrap();
    let response = router
        .clone()
        .oneshot(request("GET", &format!("/api/v1/meeting-sessions/{sid}/events"), ("x-test-caller", me.to_string()), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers()["content-type"].to_str().unwrap().starts_with("text/event-stream"));
    let mut body = response.into_body();
    let frame = body.frame().await.unwrap().unwrap().into_data().unwrap();
    let text = String::from_utf8_lossy(&frame);
    assert!(text.contains("event: meeting.session.state"), "{text}");
    assert!(text.contains("\"state\":\"launching\""), "{text}");
}

/// The full app router builds (axum panics on overlapping routes) and the
/// meetings paths are served by this module, not the data-plane fallback.
#[tokio::test]
async fn full_router_serves_meetings_without_route_collisions() {
    let gateway = Arc::new(MockGateway::new(Some(MockGateway::healthy_node()), vec![]));
    let app = crate::create_router(test_state(gateway.clone()).await);
    for (method, path) in [
        ("GET", "/api/v1/meetings"),
        ("GET", "/api/v1/me/meeting-settings"),
        ("GET", "/api/v1/orgs/org_1/meeting-settings"),
        ("POST", "/api/v1/meeting-sessions/00000000-0000-0000-0000-000000000000/ingest"),
    ] {
        let response = app
            .clone()
            .oneshot(Request::builder().method(method).uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{method} {path}");
    }
    assert!(gateway.recorded().is_empty(), "never relayed to a runtime");
}
