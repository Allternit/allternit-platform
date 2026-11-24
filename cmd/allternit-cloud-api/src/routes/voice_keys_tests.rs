//! Route tests for voice keys and the handoff relay. Each test gets its own
//! Postgres schema (routes::test_support) with migrations 128 and 135 applied; a
//! signed-in Clerk session is stood in for by the test-only
//! `x-test-session-user` header.

use super::*;
use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
use crate::routes::test_support::{test_state, MockGateway};
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;

async fn setup() -> (Router, PgPool) {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    for (name, sql) in [
        ("128", include_str!("../../migrations_pg/128_voice_keys.sql")),
        ("135", include_str!("../../migrations_pg/135_voice_handoff_sealed.sql")),
    ] {
        sqlx::raw_sql(&sql.replace("public.", "")).execute(&state.db).await.unwrap_or_else(|e| panic!("{name} applies: {e}"));
    }
    let db = state.db.clone();
    (routes().with_state(state), db)
}

enum Caller<'a> {
    Session(&'a str),
    Bearer(&'a str),
    Nobody,
}

async fn send(router: &Router, method: &str, path: &str, caller: Caller<'_>, body: Option<&str>) -> (StatusCode, HeaderMap, Value) {
    let mut builder = Request::builder().method(method).uri(path);
    builder = match caller {
        Caller::Session(user) => builder.header("x-test-session-user", user),
        Caller::Bearer(token) => builder.header("authorization", format!("Bearer {token}")),
        Caller::Nobody => builder,
    };
    let req = match body {
        Some(body) => builder.header("content-type", "application/json").body(Body::from(body.to_string())).unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = router.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
    (status, headers, body)
}

#[tokio::test]
async fn keys_roundtrip_for_the_signed_in_owner_only() {
    let (router, _) = setup().await;
    let a = || Caller::Session("user_a");
    let body = r#"{"api_key":"sk_test_123","voice_id":"21m00Tcm4TlvDq8ikWAM","label":"Studio"}"#;
    let (status, headers, saved) = send(&router, "PUT", "/api/v1/voice/keys/elevenlabs", a(), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(headers[header::CACHE_CONTROL], "private, no-store");
    assert_eq!(saved["provider"], "elevenlabs");
    assert_eq!(saved["api_key"], "sk_test_123");
    assert_eq!(saved["voice_id"], "21m00Tcm4TlvDq8ikWAM");
    assert!(saved["updated_at"].as_str().is_some());

    let (status, _, _) = send(&router, "PUT", "/api/v1/voice/keys/openai", a(), Some(r#"{"api_key":"sk-o"}"#)).await;
    assert_eq!(status, StatusCode::OK);

    let (status, _, listed) = send(&router, "GET", "/api/v1/voice/keys", a(), None).await;
    assert_eq!(status, StatusCode::OK);
    let keys = listed["keys"].as_array().unwrap();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0]["api_key"], "sk_test_123");
    assert_eq!(keys[1]["provider"], "openai");
    assert_eq!(keys[1]["voice_id"], Value::Null);

    let (_, _, other) = send(&router, "GET", "/api/v1/voice/keys", Caller::Session("user_b"), None).await;
    assert_eq!(other["keys"], serde_json::json!([]), "another user sees nothing");

    let (status, _, _) = send(&router, "DELETE", "/api/v1/voice/keys", a(), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, _, listed) = send(&router, "GET", "/api/v1/voice/keys", a(), None).await;
    assert_eq!(listed["keys"], serde_json::json!([]));
}

#[tokio::test]
async fn bad_provider_and_bad_bodies_are_400_without_echoing_the_key() {
    let (router, _) = setup().await;
    let (status, _, body) = send(&router, "PUT", "/api/v1/voice/keys/anthropic", Caller::Session("user_a"), Some(r#"{"api_key":"sk-secret-1"}"#)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, _, body) = send(&router, "PUT", "/api/v1/voice/keys/openai", Caller::Session("user_a"), Some(r#"{"api_key":12345,"x":"sk-secret-2"}"#)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!body.to_string().contains("sk-secret") && !body.to_string().contains("12345"), "{body}");
    let long = format!(r#"{{"api_key":"{}"}}"#, "k".repeat(513));
    let (status, _, _) = send(&router, "PUT", "/api/v1/voice/keys/openai", Caller::Session("user_a"), Some(&long)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn handoff_flow_over_http() {
    let (router, db) = setup().await;
    let a = || Caller::Session("user_a");
    let (status, _, created) = send(&router, "POST", "/api/v1/voice/handoff", a(), None).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let sid = created["sid"].as_str().unwrap().to_string();
    assert!(created["expires_at"].as_str().is_some());

    let (status, _, got) = send(&router, "GET", &format!("/api/v1/voice/handoff/{sid}"), a(), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["receiver_pubkey"], Value::Null);

    let receiver = format!("/api/v1/voice/handoff/{sid}/receiver");
    let (status, _, _) = send(&router, "PUT", &receiver, Caller::Session("user_b"), Some(r#"{"pubkey":"BAAA"}"#)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "other user");
    let (status, _, _) = send(&router, "PUT", &receiver, a(), Some(r#"{"pubkey":"BPubB-_x"}"#)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = send(&router, "PUT", &receiver, a(), Some(r#"{"pubkey":"BOther"}"#)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (_, _, got) = send(&router, "GET", &format!("/api/v1/voice/handoff/{sid}"), a(), None).await;
    assert_eq!(got["receiver_pubkey"], "BPubB-_x");

    sqlx::query("UPDATE voice_key_handoffs SET expires_at = NOW() - interval '1 second'").execute(&db).await.unwrap();
    let (status, _, _) = send(&router, "GET", &format!("/api/v1/voice/handoff/{sid}"), a(), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "expired");
}

#[tokio::test]
async fn sealed_relay_is_one_time_and_owner_only() {
    let (router, db) = setup().await;
    let a = || Caller::Session("user_a");
    let b = || Caller::Session("user_b");
    let (_, _, created) = send(&router, "POST", "/api/v1/voice/handoff", a(), None).await;
    let sid = created["sid"].as_str().unwrap().to_string();
    let handoff = format!("/api/v1/voice/handoff/{sid}");
    let sealed = format!("{handoff}/sealed");
    // 12-byte nonce and 34-byte ciphertext+tag, unpadded base64url.
    let nonce = "AAECAwQFBgcICQoL";
    let ciphertext = "c2VhbGVkLWtleS1ieXRlcy1wbHVzLXRhZy0xMjM0NTY3OA";
    let good = format!(r#"{{"nonce":"{nonce}","ciphertext":"{ciphertext}"}}"#);

    let (status, _, _) = send(&router, "PUT", &sealed, a(), Some(&good)).await;
    assert_eq!(status, StatusCode::CONFLICT, "no receiver yet");
    let (status, _, _) = send(&router, "PUT", &format!("{handoff}/receiver"), a(), Some(r#"{"pubkey":"BPubB"}"#)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, body) = send(&router, "GET", &sealed, a(), None).await;
    assert_eq!((status, body), (StatusCode::NO_CONTENT, Value::Null), "not sealed yet");

    let (status, _, _) = send(&router, "PUT", &sealed, b(), Some(&good)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "other user PUT");
    let (status, _, _) = send(&router, "GET", &sealed, b(), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "other user GET");

    let bad_nonce = format!(r#"{{"nonce":"AAECAwQFBgcICQo","ciphertext":"{ciphertext}"}}"#);
    let oversize = format!(r#"{{"nonce":"{nonce}","ciphertext":"{}"}}"#, "A".repeat(2049));
    for (label, body) in [("15-char nonce", bad_nonce.as_str()), ("2049-char ciphertext", oversize.as_str())] {
        let (status, _, reply) = send(&router, "PUT", &sealed, a(), Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{label}");
        assert!(!reply.to_string().contains("AAECAwQF") && !reply.to_string().contains("AAAAAAAA"), "{label} echoed: {reply}");
    }

    let (status, _, _) = send(&router, "PUT", &sealed, a(), Some(&good)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = send(&router, "PUT", &sealed, a(), Some(&good)).await;
    assert_eq!(status, StatusCode::CONFLICT, "already sealed");
    let (_, _, got) = send(&router, "GET", &handoff, a(), None).await;
    assert_eq!(got["delivered"], false);

    let (status, headers, taken) = send(&router, "GET", &sealed, a(), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CACHE_CONTROL], "private, no-store");
    assert_eq!(taken, serde_json::json!({ "nonce": nonce, "ciphertext": ciphertext }));
    let (stored, delivered_at): (Option<String>, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as("SELECT sealed_ciphertext, delivered_at FROM voice_key_handoffs WHERE sid = $1")
            .bind(&sid)
            .fetch_one(&db)
            .await
            .unwrap();
    assert!(stored.is_none() && delivered_at.is_some(), "the relay no longer holds the key");
    let (_, _, got) = send(&router, "GET", &handoff, a(), None).await;
    assert_eq!(got["delivered"], true);
    let (status, _, _) = send(&router, "GET", &sealed, a(), None).await;
    assert_eq!(status, StatusCode::CONFLICT, "second take");
    let (status, _, _) = send(&router, "PUT", &sealed, a(), Some(&good)).await;
    assert_eq!(status, StatusCode::CONFLICT, "seal after delivery");

    // Sealing near expiry leaves the receiver time to take it.
    let (_, _, created) = send(&router, "POST", "/api/v1/voice/handoff", a(), None).await;
    let sid2 = created["sid"].as_str().unwrap().to_string();
    let handoff2 = format!("/api/v1/voice/handoff/{sid2}");
    send(&router, "PUT", &format!("{handoff2}/receiver"), a(), Some(r#"{"pubkey":"BPubB"}"#)).await;
    sqlx::query("UPDATE voice_key_handoffs SET expires_at = NOW() + interval '2 seconds' WHERE sid = $1")
        .bind(&sid2)
        .execute(&db)
        .await
        .unwrap();
    let (status, _, _) = send(&router, "PUT", &format!("{handoff2}/sealed"), a(), Some(&good)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let ahead: f64 = sqlx::query_scalar("SELECT EXTRACT(EPOCH FROM expires_at - NOW())::float8 FROM voice_key_handoffs WHERE sid = $1")
        .bind(&sid2)
        .fetch_one(&db)
        .await
        .unwrap();
    assert!(ahead >= 25.0, "expires {ahead}s ahead");

    // Expired rows are 404 for both verbs.
    sqlx::query("UPDATE voice_key_handoffs SET expires_at = NOW() - interval '1 second'").execute(&db).await.unwrap();
    let (status, _, _) = send(&router, "GET", &format!("{handoff2}/sealed"), a(), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "expired GET");
    let (status, _, _) = send(&router, "PUT", &sealed, a(), Some(&good)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "expired PUT");
}

#[tokio::test]
async fn api_tokens_are_403_and_no_credentials_are_401() {
    let (router, db) = setup().await;
    let token = "allternit_voice_wildcard_token_0123456789";
    sqlx::query("INSERT INTO api_tokens (id, token_hash, name, user_id, permissions) VALUES ('tok_v', $1, 'v', 'user_a', '[\"*\"]')")
        .bind(crate::services::api_keys::hash_token(token))
        .execute(&db)
        .await
        .unwrap();
    for (method, path) in [
        ("GET", "/api/v1/voice/keys"),
        ("DELETE", "/api/v1/voice/keys"),
        ("POST", "/api/v1/voice/handoff"),
        ("GET", "/api/v1/voice/handoff/abc"),
        ("PUT", "/api/v1/voice/handoff/abc/sealed"),
        ("GET", "/api/v1/voice/handoff/abc/sealed"),
    ] {
        let (status, _, _) = send(&router, method, path, Caller::Bearer(token), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path} with an API token");
        let (status, _, _) = send(&router, method, path, Caller::Nobody, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path} signed out");
    }
    let (status, _, _) = send(&router, "PUT", "/api/v1/voice/keys/openai", Caller::Bearer(token), Some(r#"{"api_key":"sk"}"#)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
#[serial_test::serial]
// The env lock is held across awaits on purpose: the env var must not flip mid-request.
#[allow(clippy::await_holding_lock)]
async fn the_dev_token_is_refused_too() {
    let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (router, _) = setup().await;
    std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
    let (status, _, _) = send(&router, "GET", "/api/v1/voice/keys", Caller::Bearer("dev-api-token"), None).await;
    std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
    assert_eq!(status, StatusCode::FORBIDDEN);
}
