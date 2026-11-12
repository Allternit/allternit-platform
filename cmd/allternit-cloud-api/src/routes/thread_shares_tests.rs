//! Route tests for thread shares. Each test gets its own Postgres schema
//! (routes::test_support) with migration 124 applied; signed-in callers are
//! stood in for by the test-only `x-test-caller` header that
//! `artifacts_v2::caller` reads.

use super::*;
use crate::routes::test_support::{test_state, MockGateway};
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use sqlx::PgPool;
use tower::ServiceExt;

async fn setup() -> (Router, PgPool) {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    sqlx::raw_sql(include_str!("../../migrations_pg/124_thread_shares.sql"))
        .execute(&state.db)
        .await
        .expect("124 applies");
    let db = state.db.clone();
    (routes().with_state(state), db)
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

async fn send(router: &Router, method: &str, path: &str, user: Option<&str>, body: Option<Value>) -> Reply {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(user) = user {
        builder = builder.header("x-test-caller", json!({ "id": user }).to_string());
    }
    let req = match body {
        Some(body) => builder.header("content-type", "application/json").body(Body::from(body.to_string())).unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = router.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
    Reply { status, headers, body }
}

/// 1x1 transparent PNG.
const PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

fn snapshot() -> Value {
    json!([
        { "role": "system", "text": "You are a secret system prompt" },
        { "role": "user", "text": "Plot sales by month", "id": "msg_1", "createdAt": 1,
          "images": [PNG, "https://evil.example/x.png", "data:image/svg+xml;base64,PHN2Zy8+"],
          "metadata": { "apiKey": "sk-live-leak" } },
        { "role": "assistant", "text": "Here is the chart.", "model": "claude",
          "tools": ["run_code", { "name": "web_search", "input": { "q": "private query" }, "output": "tool output leak" }, 7],
          "reasoning": "hidden chain of thought" },
        { "role": "tool", "text": "raw tool result" },
        { "role": "assistant", "text": "   " },
    ])
}

async fn create(router: &Router, user: &str, session: &str) -> Reply {
    send(router, "POST", "/api/v2/thread-shares", Some(user), Some(json!({ "sessionId": session, "title": "Sales chart", "snapshot": snapshot() }))).await
}

#[tokio::test]
async fn create_then_public_serves_only_the_allowlist() {
    let (router, db) = setup().await;
    let created = create(&router, "user_a", "ses_1").await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let token = created.body["token"].as_str().unwrap().to_string();
    assert_eq!(base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&token).unwrap().len(), 20, "160-bit token");
    assert_eq!(created.body["url"], json!(format!("https://ai.allternit.com/t/{token}")));

    let public = send(&router, "GET", &format!("/api/v2/public/threads/{token}"), None, None).await;
    assert_eq!(public.status, StatusCode::OK, "{}", public.body);
    assert_eq!(public.headers[header::CACHE_CONTROL], "private, no-store");
    assert_eq!(public.headers["x-robots-tag"], "noindex");
    assert_eq!(public.body["title"], "Sales chart");
    assert!(public.body["updatedAt"].as_str().is_some_and(|s| DateTime::parse_from_rfc3339(s).is_ok()));
    assert_eq!(
        public.body["messages"],
        json!([
            { "role": "user", "text": "Plot sales by month", "images": [PNG], "tools": [] },
            { "role": "assistant", "text": "Here is the chart.", "images": [], "tools": ["run_code", "web_search"] },
        ])
    );
    let text = public.body.to_string();
    for leaked in ["secret system prompt", "sk-live-leak", "private query", "tool output leak", "hidden chain", "raw tool result", "msg_1", "evil.example", "svg"] {
        assert!(!text.contains(leaked), "{leaked} leaked: {text}");
    }
    let (count, bytes): (i32, i32) = sqlx::query_as("SELECT message_count, bytes FROM thread_shares WHERE token = $1")
        .bind(&token)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(count, 2);
    assert_eq!(bytes as usize, public.body["messages"].to_string().len());
}

#[tokio::test]
async fn sharing_a_session_again_keeps_the_token_and_lookup_finds_it() {
    let (router, _) = setup().await;
    let first = create(&router, "user_a", "ses_1").await;
    let token = first.body["token"].clone();
    let again = send(&router, "POST", "/api/v2/thread-shares", Some("user_a"),
        Some(json!({ "sessionId": "ses_1", "title": "Renamed", "snapshot": [{ "role": "user", "text": "second take" }] }))).await;
    assert_eq!(again.status, StatusCode::OK);
    assert_eq!(again.body["token"], token);
    let public = send(&router, "GET", &format!("/api/v2/public/threads/{}", token.as_str().unwrap()), None, None).await;
    assert_eq!(public.body["title"], "Renamed");
    assert_eq!(public.body["messages"][0]["text"], "second take");

    let found = send(&router, "GET", "/api/v2/thread-shares?sessionId=ses_1", Some("user_a"), None).await;
    assert_eq!(found.status, StatusCode::OK);
    assert_eq!(found.body["token"], token);
    assert!(found.body["url"].as_str().unwrap().ends_with(token.as_str().unwrap()));
    // Another user's lookup of the same session id, and an unshared session, are 404.
    assert_eq!(send(&router, "GET", "/api/v2/thread-shares?sessionId=ses_1", Some("user_b"), None).await.status, StatusCode::NOT_FOUND);
    assert_eq!(send(&router, "GET", "/api/v2/thread-shares?sessionId=ses_2", Some("user_a"), None).await.status, StatusCode::NOT_FOUND);
    // A different session gets its own link.
    let other = create(&router, "user_a", "ses_2").await;
    assert_eq!(other.status, StatusCode::CREATED);
    assert_ne!(other.body["token"], token);
}

#[tokio::test]
async fn only_the_owner_can_update_or_delete_and_delete_kills_the_link() {
    let (router, _) = setup().await;
    let token = create(&router, "user_a", "ses_1").await.body["token"].as_str().unwrap().to_string();
    let path = format!("/api/v2/thread-shares/{token}");
    let public = format!("/api/v2/public/threads/{token}");
    let resnap = json!({ "snapshot": [{ "role": "assistant", "text": "updated answer" }] });

    let stranger = send(&router, "PUT", &path, Some("user_b"), Some(resnap.clone())).await;
    assert_eq!(stranger.status, StatusCode::NOT_FOUND);
    assert_eq!(send(&router, "DELETE", &path, Some("user_b"), None).await.status, StatusCode::NOT_FOUND);
    assert_eq!(send(&router, "PUT", &path, None, Some(resnap.clone())).await.status, StatusCode::UNAUTHORIZED);
    let still = send(&router, "GET", &public, None, None).await;
    assert_eq!(still.body["messages"][0]["text"], "Plot sales by month", "a stranger changed nothing");

    let updated = send(&router, "PUT", &path, Some("user_a"), Some(resnap)).await;
    assert_eq!(updated.status, StatusCode::OK, "{}", updated.body);
    assert_eq!(updated.body["token"], json!(token));
    let after = send(&router, "GET", &public, None, None).await;
    assert_eq!(after.body["messages"], json!([{ "role": "assistant", "text": "updated answer", "images": [], "tools": [] }]));
    assert_eq!(after.body["title"], "Sales chart", "PUT without a title keeps it");

    assert_eq!(send(&router, "DELETE", &path, Some("user_a"), None).await.status, StatusCode::NO_CONTENT);
    let gone = send(&router, "GET", &public, None, None).await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
    assert_eq!(gone.body, json!({ "error": "not_found" }));
    assert_eq!(gone.headers[header::CACHE_CONTROL], "private, no-store");
    assert_eq!(send(&router, "DELETE", &path, Some("user_a"), None).await.status, StatusCode::NOT_FOUND);
    assert_eq!(send(&router, "GET", "/api/v2/thread-shares?sessionId=ses_1", Some("user_a"), None).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bad_requests_are_refused() {
    let (router, db) = setup().await;
    let post = |user: Option<&'static str>, body: Value| {
        let router = router.clone();
        async move { send(&router, "POST", "/api/v2/thread-shares", user, Some(body)).await.status }
    };
    assert_eq!(post(None, json!({ "sessionId": "s", "snapshot": snapshot() })).await, StatusCode::UNAUTHORIZED);
    assert_eq!(post(Some("u"), json!({ "snapshot": snapshot() })).await, StatusCode::BAD_REQUEST);
    assert_eq!(post(Some("u"), json!({ "sessionId": "s", "snapshot": "not a list" })).await, StatusCode::BAD_REQUEST);
    let only_hidden = json!({ "sessionId": "s", "snapshot": [{ "role": "system", "text": "x" }, { "role": "tool", "text": "y" }] });
    assert_eq!(post(Some("u"), only_hidden).await, StatusCode::UNPROCESSABLE_ENTITY);
    let huge = json!({ "sessionId": "s", "snapshot": [{ "role": "user", "text": "x".repeat(MAX_BODY) }] });
    let refused = send(&router, "POST", "/api/v2/thread-shares", Some("u"), Some(huge)).await;
    assert_eq!((refused.status, refused.body), (StatusCode::PAYLOAD_TOO_LARGE, json!({ "error": "too_large" })));
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_shares").fetch_one(&db).await.unwrap();
    assert_eq!(rows, 0);
    for token in ["short", "has spaces in it, sixteen+", "AAAAAAAAAAAAAAAAAAAAAAAAAAA"] {
        let path = format!("/api/v2/public/threads/{}", token.replace(' ', "%20"));
        assert_eq!(send(&router, "GET", &path, None, None).await.status, StatusCode::NOT_FOUND, "{token}");
    }
}

#[test]
fn sanitizer_caps_text_images_and_tool_names() {
    let big_image = format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(vec![0u8; MAX_IMAGE_BYTES + 1]));
    let max_image = format!("data:image/webp;base64,{}", base64::engine::general_purpose::STANDARD.encode(vec![1u8; MAX_IMAGE_BYTES]));
    let input = json!([
        { "role": "user", "text": "é".repeat(MAX_TEXT_CHARS + 5), "images": [big_image, max_image, "data:image/gif;base64,not*base64"] },
        { "role": "assistant", "tools": ["t".repeat(200)], "images": vec![PNG; 15] },
    ]);
    let out = sanitize_snapshot(&input);
    assert_eq!(out[0]["text"].as_str().unwrap().chars().count(), MAX_TEXT_CHARS);
    assert_eq!(out[0]["images"].as_array().unwrap().len(), 1, "too big and not-base64 images are dropped");
    assert_eq!(out[1]["tools"][0].as_str().unwrap().chars().count(), MAX_TOOL_NAME_CHARS);
    assert_eq!(out[1]["text"], "", "a message with only tools keeps an empty text");
    assert_eq!(out[1]["images"].as_array().unwrap().len(), MAX_IMAGES - 1, "10 images across the whole thread");
    assert!(sanitize_snapshot(&json!({ "messages": [] })).is_empty());
}
