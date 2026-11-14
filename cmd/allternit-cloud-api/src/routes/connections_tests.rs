//! Route tests for the connect kit's account rows. The auth test runs on the
//! schema-per-test pool; the end-to-end test needs `TEST_DATABASE_URL` (a
//! server it can create a database on) and runs every SQL statement against
//! the real migrations. The vendor key check is stubbed (an edge).

use super::*;
use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
use crate::error::ApiError;
use crate::routes::test_support::{test_state, MockGateway};
use crate::services::inference_keys::{InferenceKeyService, KeyValidator, KEY_REJECTED_MESSAGE};
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use sqlx::{Executor, PgPool};
use std::path::Path as FsPath;
use tower::ServiceExt;

async fn send(router: &Router, method: &str, path: &str, caller: Option<(&str, &str)>, body: Option<&str>) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some((name, value)) = caller {
        builder = builder.header(name, value);
    }
    let req = match body {
        Some(body) => builder.header("content-type", "application/json").body(Body::from(body.to_string())).unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = router.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
    (status, body)
}

#[tokio::test]
async fn needs_a_session_and_refuses_api_tokens() {
    let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    let router = routes().with_state(state);
    let (status, _) = send(&router, "GET", "/api/v1/connections", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
    let (status, _) = send(&router, "GET", "/api/v1/connections", Some(("authorization", "Bearer dev-api-token")), None).await;
    std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
    assert_eq!(status, StatusCode::FORBIDDEN, "API tokens never reach the rows");
}

/// Accepts keys that start with `good`, rejects the rest.
struct PrefixValidator;

#[async_trait::async_trait]
impl KeyValidator for PrefixValidator {
    async fn validate(&self, _provider_id: &str, api_key: &str) -> Result<(), ApiError> {
        if api_key.starts_with("good") {
            Ok(())
        } else {
            Err(ApiError::BadRequest(KEY_REJECTED_MESSAGE.to_string()))
        }
    }
}

async fn migrated_db() -> Option<(PgPool, String, String)> {
    let base = std::env::var("TEST_DATABASE_URL").ok()?;
    let admin = PgPool::connect(&base).await.ok()?;
    let name = format!("conn_{}", uuid::Uuid::new_v4().simple());
    admin.execute(format!("CREATE DATABASE {name}").as_str()).await.ok()?;
    let url = format!("{}/{name}", base.rsplit_once('/').unwrap().0);
    let pool = PgPool::connect(&url).await.unwrap();
    let dir = FsPath::new(env!("CARGO_MANIFEST_DIR")).join("migrations_pg");
    let mut files: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    files.retain(|p| p.extension().is_some_and(|e| e == "sql"));
    files.sort();
    for file in files {
        let sql = std::fs::read_to_string(&file).unwrap();
        pool.execute(sql.as_str()).await.unwrap_or_else(|e| panic!("{file:?}: {e}"));
    }
    Some((pool, base, name))
}

#[tokio::test]
async fn rows_keys_and_logins_on_the_real_schema() {
    let Some((db, base, name)) = migrated_db().await else {
        eprintln!("skipped: TEST_DATABASE_URL is not set or cannot create a database");
        return;
    };
    let user = "user_conn";
    db.execute(
        format!(
            r#"
            INSERT INTO users (id, email) VALUES ('{user}', 'conn@example.com');
            INSERT INTO runtime_devices (id, user_id, name, runtime_type, capabilities, status, last_seen_at)
                VALUES ('rt_mac', '{user}', 'Studio Mac', 'desktop', '["runtime:connect"]', 'online', now());
            INSERT INTO slack_installs (team_id, team_name, user_id) VALUES ('T1', 'Acme', '{user}');
            "#
        )
        .as_str(),
    )
    .await
    .unwrap();

    let base_state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    let mut state = Arc::try_unwrap(base_state).ok().expect("one owner");
    let cipher = Arc::new(allternit_cloud_core::CredentialCipher::new("test cipher key material"));
    state.db = db.clone();
    state.credential_cipher = Some(cipher.clone());
    state.inference_key_service =
        Some(Arc::new(InferenceKeyService::with_validator(db.clone(), cipher, Arc::new(PrefixValidator))));
    let router = routes().with_state(Arc::new(state));
    let session = Some(("x-test-session-user", user));

    // A rejected key is stored nowhere and comes back as `needs`.
    let (status, row) =
        send(&router, "POST", "/api/v1/connections/xai-key/submit", session, Some(r#"{"value":"bad-xai-key-0000"}"#)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{row}");
    assert_eq!((row["state"].as_str(), row["reason_code"].as_str()), (Some("needs"), Some("key_rejected")));

    let (status, row) =
        send(&router, "POST", "/api/v1/connections/xai-key/submit", session, Some(r#"{"value":"good-xai-key-1234"}"#)).await;
    assert_eq!(status, StatusCode::OK, "{row}");
    assert_eq!(row["state"], "on");
    assert_eq!(row["found"], "Key works · goo…1234");
    assert!(!row.to_string().contains("good-xai-key-1234"), "the key never comes back");

    // The voice ElevenLabs key goes to the voice store.
    let (status, row) =
        send(&router, "POST", "/api/v1/connections/elevenlabs/submit", session, Some(r#"{"value":"sk_eleven_123456"}"#)).await;
    assert_eq!((status, row["state"].as_str()), (StatusCode::OK, Some("on")), "{row}");

    let (status, body) = send(&router, "GET", "/api/v1/connections", session, None).await;
    assert_eq!(status, StatusCode::OK);
    println!("GET /api/v1/connections → {}", serde_json::to_string_pretty(&body).unwrap());
    let rows = body["connections"].as_array().unwrap();
    let find = |id: &str| rows.iter().find(|row| row["id"] == id).unwrap_or_else(|| panic!("no {id}")).clone();
    assert_eq!(body["version"], 1);
    assert_eq!(find("allternit")["found"], "Free · $2 a month free");
    assert_eq!(find("this-mac")["found"], "Studio Mac");
    assert_eq!(find("slack")["found"], "Acme");
    assert_eq!(find("chatgpt")["available"], true, "an online Mac makes ChatGPT available");
    assert_eq!(find("claude")["available"], false);

    // Connect: a key row hands back the key page; an unavailable row is a 409.
    let (status, body) = send(&router, "POST", "/api/v1/connections/anthropic-key/connect", session, None).await;
    assert_eq!((status, body["login"]["kind"].as_str()), (StatusCode::OK, Some("paste")));
    let (status, body) = send(&router, "POST", "/api/v1/connections/grok/connect", session, Some("{}")).await;
    assert_eq!((status, body["reason_code"].as_str()), (StatusCode::CONFLICT, Some("not_available_yet")));
    let (_, body) = send(&router, "GET", "/api/v1/connections/xai-key/login", session, None).await;
    assert_eq!(body["status"], "done");

    // Delete removes the key from the account.
    let (status, _) = send(&router, "DELETE", "/api/v1/connections/xai-key", session, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, body) = send(&router, "GET", "/api/v1/connections/xai-key/login", session, None).await;
    assert_eq!((body["status"].as_str(), body["row"]["state"].as_str()), (Some("pending"), Some("off")));

    db.close().await;
    let admin = PgPool::connect(&base).await.unwrap();
    admin.execute(format!("DROP DATABASE {name} WITH (FORCE)").as_str()).await.unwrap();
}
