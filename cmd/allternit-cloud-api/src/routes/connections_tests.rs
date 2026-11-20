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

/// `GET /catalog` (contract 11): session only, matched before `:id`, its
/// 400s, and the private 5-minute cache on a page. `group=brains` and
/// `limit=0` never read the database. Ranking and paging are unit-tested in
/// services::connections_catalog.
#[tokio::test]
async fn catalog_needs_a_session_refuses_api_tokens_and_caches() {
    let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    let router = routes().with_state(state);
    let (status, _) = send(&router, "GET", "/api/v1/connections/catalog", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
    let (status, _) =
        send(&router, "GET", "/api/v1/connections/catalog", Some(("authorization", "Bearer dev-api-token")), None).await;
    std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
    assert_eq!(status, StatusCode::FORBIDDEN, "API tokens never reach the catalog");

    let session = Some(("x-test-session-user", "user_catalog"));
    let req = Request::builder()
        .uri("/api/v1/connections/catalog?group=brains&limit=60")
        .header("x-test-session-user", "user_catalog")
        .body(Body::empty())
        .unwrap();
    let response = router.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get(header::CACHE_CONTROL).unwrap(), "private, max-age=300");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!((body["version"].as_u64(), body["group"].as_str()), (Some(1), Some("brains")));
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), conn::KEY_ROWS.len());
    assert!(entries.iter().all(|e| e["group"] == "brains" && e["ways"] == serde_json::json!(["paste"])));
    assert_eq!(body["next_cursor"], Value::Null);

    let (status, body) = send(&router, "GET", "/api/v1/connections/catalog?limit=0", session, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["entries"], serde_json::json!([]));
    assert!(body["counts"]["apps"].as_u64().unwrap() > 1_000, "{}", body["counts"]);
    for base in ["gmail", "drive", "slack", "github", "notion"] {
        let (_, found) = send(&router, "GET", &format!("/api/v1/connections/catalog?group=apps&q={base}&limit=60"), session, None).await;
        if let Some(entries) = found["entries"].as_array() {
            assert!(entries.iter().all(|e| e["id"] != base), "{base} is a base row");
        }
    }

    let (status, body) = send(&router, "GET", "/api/v1/connections/catalog?group=gadgets", session, None).await;
    assert_eq!((status, body), (StatusCode::BAD_REQUEST, serde_json::json!({ "error": "unknown_group" })));
    let (status, body) = send(&router, "GET", "/api/v1/connections/catalog?cursor=abc", session, None).await;
    assert_eq!((status, body), (StatusCode::BAD_REQUEST, serde_json::json!({ "error": "bad_cursor" })));
}

/// Accepts keys that start with `good`, rejects the rest.
struct PrefixValidator;

#[async_trait::async_trait]
impl KeyValidator for PrefixValidator {
    async fn validate(&self, provider_id: &str, api_key: &str) -> Result<Vec<String>, ApiError> {
        if api_key.starts_with("good") {
            Ok(if provider_id == "xai" { vec!["grok-4".to_string()] } else { Vec::new() })
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
    assert_eq!(row["models"], serde_json::json!(["xai/grok-4"]), "the vendor's models, as routable ids");
    assert!(!row.to_string().contains("good-xai-key-1234"), "the key never comes back");

    // The voice ElevenLabs key is checked, then goes to the voice store.
    let (status, row) =
        send(&router, "POST", "/api/v1/connections/elevenlabs/submit", session, Some(r#"{"value":"bad_eleven_1234"}"#)).await;
    assert_eq!((status, row["reason_code"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("key_rejected")), "{row}");
    let (status, row) =
        send(&router, "POST", "/api/v1/connections/elevenlabs/submit", session, Some(r#"{"value":"good_eleven_1234"}"#)).await;
    assert_eq!((status, row["state"].as_str()), (StatusCode::OK, Some("on")), "{row}");

    let (status, body) = send(&router, "GET", "/api/v1/connections", session, None).await;
    assert_eq!(status, StatusCode::OK);
    println!("GET /api/v1/connections → {}", serde_json::to_string_pretty(&body).unwrap());
    let rows = body["connections"].as_array().unwrap();
    let find = |id: &str| rows.iter().find(|row| row["id"] == id).unwrap_or_else(|| panic!("no {id}")).clone();
    assert_eq!(body["version"], 1);
    assert_eq!(find("allternit")["found"], "Free · needs credits");
    assert_eq!(find("this-mac")["found"], "Studio Mac");
    assert_eq!(find("slack")["found"], "Acme");
    for id in ["chatgpt", "local", "codex", "claude-code"] {
        assert!(rows.iter().all(|row| row["id"] != id), "{id}: the online Mac's runtime row answers");
    }
    assert_eq!(find("claude")["available"], false);

    // Connect: a key row hands back the key page; an unavailable row is a 409.
    let (status, body) = send(&router, "POST", "/api/v1/connections/anthropic-key/connect", session, None).await;
    assert_eq!((status, body["login"]["kind"].as_str()), (StatusCode::OK, Some("paste")));
    let (status, body) = send(&router, "POST", "/api/v1/connections/telegram/connect", session, Some("{}")).await;
    assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("start_from_bot")));
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

/// Contract 9.1/9.2 end to end on the real schema, with no computer and the
/// compute gate refusing agent entry for this account: plan rows need
/// `caps=runner`, connect answers the disclosure until it is accepted, then
/// a login from the runner; submit/login go to the runner; DELETE signs out.
/// The runner box never shows in compute-status.
#[tokio::test]
async fn plan_rows_disclosure_login_and_logout_without_a_computer() {
    let Some((db, base, name)) = migrated_db().await else {
        eprintln!("skipped: TEST_DATABASE_URL is not set or cannot create a database");
        return;
    };
    let user = "user_plan";
    db.execute(format!("INSERT INTO users (id, email) VALUES ('{user}', 'plan@example.com');").as_str()).await.unwrap();
    let base_state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    let mut state = Arc::try_unwrap(base_state).ok().expect("one owner");
    state.db = db.clone();
    let router = routes().with_state(Arc::new(state));
    let session = Some(("x-test-session-user", user));
    // A box with the fake runner program (its hello says ChatGPT is signed in).
    let (service, _box) = crate::services::brain_runners::test_service_with_box(db.clone(), user).await;

    TEST_RUNNERS
        .scope(service, async {
            // The gate would refuse this account agent entry.
            let gate = crate::routes::account_compute::GateConfig { gate: true, ..crate::routes::account_compute::GateConfig::from_env() };
            assert!(matches!(
                crate::routes::account_compute::require_compute_with(&db, user, &gate).await,
                Err(ApiError::ComputeRequired(_))
            ));

            // An older client: plan rows say update the app.
            let (_, body) = send(&router, "GET", "/api/v1/connections", session, None).await;
            let rows = body["connections"].as_array().unwrap().clone();
            let find = |rows: &[Value], id: &str| rows.iter().find(|r| r["id"] == id).cloned().unwrap_or_else(|| panic!("no {id}"));
            assert_eq!(find(&rows, "claude")["reason_code"], "update_app");
            assert_eq!(find(&rows, "chatgpt")["unavailable_reason"], "Update the app to connect this plan.");

            let (_, body) = send(&router, "GET", "/api/v1/connections?caps=runner", session, None).await;
            let rows = body["connections"].as_array().unwrap().clone();
            let chatgpt = find(&rows, "chatgpt");
            assert_eq!((chatgpt["available"].as_bool(), chatgpt["state"].as_str(), chatgpt["runs_on"].as_str()), (Some(true), Some("on"), Some("runner")));
            assert_eq!(chatgpt["person_only"], true);
            assert_eq!(chatgpt["models"][0], "plan/chatgpt/gpt-6.1-sol");
            let claude = find(&rows, "claude");
            assert_eq!(claude["disclosure"], serde_json::json!({ "version": "2026-11-14.1", "accepted_at": null }));

            // Connect before accepting: the disclosure, with the vendor's name.
            let (status, body) = send(&router, "POST", "/api/v1/connections/claude/connect", session, Some("{}")).await;
            assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("disclosure_required")), "{body}");
            assert_eq!(body["error"]["disclosure"]["title"], "Before you connect your Claude plan");
            assert_eq!(body["error"]["disclosure"]["body"].as_array().unwrap().len(), 4);
            let (status, _) =
                send(&router, "POST", "/api/v1/connections/claude/connect", session, Some(r#"{"accept_disclosure":"2020-01-01.1"}"#)).await;
            assert_eq!(status, StatusCode::CONFLICT, "a stale version is not an acceptance");
            // Submit and login poll need the acceptance too.
            let (status, body) = send(&router, "POST", "/api/v1/connections/grok/submit", session, Some(r#"{"value":"abc"}"#)).await;
            assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("disclosure_required")));

            // Accept: a login from the runner, not compute_required.
            let (status, body) =
                send(&router, "POST", "/api/v1/connections/claude/connect", session, Some(r#"{"accept_disclosure":"2026-11-14.1"}"#)).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!((body["login"]["kind"].as_str(), body["login"]["code"].as_str()), (Some("device_code"), Some("KQ7M-24TX")));
            let (status, body) = send(&router, "GET", "/api/v1/connections/claude/login", session, None).await;
            assert_eq!((status, body["status"].as_str(), body["row"]["state"].as_str()), (StatusCode::OK, Some("done"), Some("on")), "{body}");
            assert!(body["row"]["disclosure"]["accepted_at"].is_string());

            // The runner box is not a computer.
            let status = crate::routes::account_compute::status_body(&db, user, &gate, &std::collections::HashSet::new(), Utc::now())
                .await
                .unwrap();
            assert_eq!((status["state"].as_str(), status["gate"].as_bool()), (Some("none"), Some(true)));
            assert!(status["computer"].is_null(), "{status}");

            // Disconnect signs out on the runner; the consent stays.
            let (status, _) = send(&router, "DELETE", "/api/v1/connections/chatgpt", session, None).await;
            assert_eq!(status, StatusCode::NO_CONTENT);
            let (_, body) = send(&router, "GET", "/api/v1/connections?caps=runner", session, None).await;
            let rows = body["connections"].as_array().unwrap().clone();
            assert_eq!(find(&rows, "chatgpt")["state"], "off");
            assert!(find(&rows, "claude")["disclosure"]["accepted_at"].is_string());
        })
        .await;

    db.close().await;
    let admin = PgPool::connect(&base).await.unwrap();
    admin.execute(format!("DROP DATABASE {name} WITH (FORCE)").as_str()).await.unwrap();
}

/// Copilot (contract 9.5, 9.11) against a fake GitHub: with no App configured
/// the row is the section 3 "Coming soon." row; with it, connect hands out a
/// GitHub authorize URL, the callback stores a sealed grant once (the state is
/// single use), the row turns on with its chat models, and DELETE revokes the
/// grant at GitHub and removes it.
#[tokio::test]
async fn copilot_connect_callback_and_revoke() {
    use crate::services::copilot::{fake, TEST_CONFIG};
    let Some((db, base, name)) = migrated_db().await else {
        eprintln!("skipped: TEST_DATABASE_URL is not set or cannot create a database");
        return;
    };
    let user = "user_copilot";
    db.execute(format!("INSERT INTO users (id, email) VALUES ('{user}', 'copilot@example.com');").as_str()).await.unwrap();
    let base_state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    let mut state = Arc::try_unwrap(base_state).ok().expect("one owner");
    let cipher = Arc::new(allternit_cloud_core::CredentialCipher::new("test cipher key material"));
    state.db = db.clone();
    state.credential_cipher = Some(cipher.clone());
    let router = routes().with_state(Arc::new(state));
    let session = Some(("x-test-session-user", user));
    let copilot_row = |body: &Value| body["connections"].as_array().unwrap().iter().find(|r| r["id"] == "copilot").cloned().unwrap();

    // No App env vars: unchanged.
    TEST_CONFIG
        .scope(None, async {
            let (_, body) = send(&router, "GET", "/api/v1/connections?caps=runner", session, None).await;
            let row = copilot_row(&body);
            assert_eq!((row["available"].as_bool(), row["unavailable_reason"].as_str()), (Some(false), Some("Coming soon.")), "{row}");
            assert_eq!(row["fallbacks"], serde_json::json!(["github"]));
            assert!(row.get("person_only").is_none(), "{row}");
            let (status, body) = send(&router, "POST", "/api/v1/connections/copilot/connect", session, None).await;
            assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("not_available")), "{body}");
        })
        .await;

    let github = fake::start().await;
    TEST_CONFIG
        .scope(Some(github.config()), async {
            let (_, body) = send(&router, "GET", "/api/v1/connections", session, None).await;
            let row = copilot_row(&body);
            assert_eq!((row["available"].as_bool(), row["state"].as_str(), row["person_only"].as_bool()), (Some(true), Some("off"), Some(true)), "{row}");

            // A return URL outside Allternit is refused.
            let (status, _) =
                send(&router, "POST", "/api/v1/connections/copilot/connect", session, Some(r#"{"return_to":"https://evil.example/"}"#)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            let (status, body) = send(
                &router,
                "POST",
                "/api/v1/connections/copilot/connect",
                session,
                Some(r#"{"return_to":"allternit://connections"}"#),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!((body["login"]["kind"].as_str(), body["login"]["poll"].as_bool()), (Some("oauth"), Some(true)));
            let url = reqwest::Url::parse(body["login"]["url"].as_str().unwrap()).unwrap();
            assert_eq!(url.path(), "/login/oauth/authorize");
            let param = |k: &str| url.query_pairs().find(|(key, _)| key == k).map(|(_, v)| v.to_string()).unwrap();
            assert_eq!(param("client_id"), "Iv_test");
            assert_eq!(param("redirect_uri"), "https://api.allternit.com/api/v1/connections/copilot/callback");
            let signed = param("state");
            let (_, body) = send(&router, "GET", "/api/v1/connections/copilot/login", session, None).await;
            assert_eq!((body["status"].as_str(), body["row"]["state"].as_str()), (Some("pending"), Some("connecting")), "{body}");

            // The callback carries no session; the signed state names the account.
            let callback = |code: &str| format!("/api/v1/connections/copilot/callback?code={code}&state={}", urlencoding(&signed));
            let response = router.clone().oneshot(Request::get(callback("good")).body(Body::empty()).unwrap()).await.unwrap();
            assert_eq!(response.status(), StatusCode::FOUND);
            assert_eq!(response.headers()["location"], "allternit://connections?connected=copilot");
            let sealed: String = sqlx::query_scalar("SELECT sealed_access FROM copilot_grants WHERE user_id = $1")
                .bind(user)
                .fetch_one(&db)
                .await
                .unwrap();
            assert!(!sealed.contains("ghu_"), "stored sealed");
            assert_eq!(cipher.decrypt(&sealed).unwrap(), "ghu_fake_user_token");
            // Single use: the same state again is refused.
            let again = router.clone().oneshot(Request::get(callback("good")).body(Body::empty()).unwrap()).await.unwrap();
            assert_eq!(again.headers()["location"], "https://ai.allternit.com/connections?connected=copilot&error=bad_state");
            // A forged state never reaches GitHub.
            let forged = router
                .clone()
                .oneshot(Request::get("/api/v1/connections/copilot/callback?code=good&state=abc.def").body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert!(forged.headers()["location"].to_str().unwrap().ends_with("error=bad_state"));
            assert_eq!(github.calls.lock().unwrap().token_bodies.len(), 1, "one code exchange");

            let (_, body) = send(&router, "GET", "/api/v1/connections", session, None).await;
            let row = copilot_row(&body);
            assert_eq!((row["state"].as_str(), row["found"].as_str()), (Some("on"), Some("GitHub · @octo")), "{row}");
            assert_eq!(row["models"], serde_json::json!(["copilot/gpt-5", "copilot/claude-sonnet-4.5"]), "chat models only");
            assert!(!body.to_string().contains("ghu_"), "the token never comes back");
            let (_, body) = send(&router, "GET", "/api/v1/connections/copilot/login", session, None).await;
            assert_eq!(body["status"], "done");

            // Disconnect: revoked at GitHub, gone here.
            let (status, _) = send(&router, "DELETE", "/api/v1/connections/copilot", session, None).await;
            assert_eq!(status, StatusCode::NO_CONTENT);
            assert_eq!(github.calls.lock().unwrap().revoked, vec!["ghu_fake_user_token".to_string()]);
            let left: i64 = sqlx::query_scalar("SELECT count(*) FROM copilot_grants").fetch_one(&db).await.unwrap();
            assert_eq!(left, 0);
            let (_, body) = send(&router, "GET", "/api/v1/connections", session, None).await;
            assert_eq!(copilot_row(&body)["state"], "off");
        })
        .await;

    db.close().await;
    let admin = PgPool::connect(&base).await.unwrap();
    admin.execute(format!("DROP DATABASE {name} WITH (FORCE)").as_str()).await.unwrap();
}

fn urlencoding(value: &str) -> String {
    reqwest::Url::parse_with_params("http://x/", &[("v", value)]).unwrap().query().unwrap()[2..].to_string()
}

// ---- claude-org (contract 10) -------------------------------------------

mod claude_org {
    use super::*;
    use crate::auth::anthropic_client::*;

    fn clear_env() {
        for name in [LINK_ENV, PEM_ENV, ISSUER_BASE_ENV, API_BASE_ENV, AUTH_BASE_ENV, crate::services::claude_link::REDIRECT_ENV]
            .into_iter()
            .chain(REQUIRED_ENV)
        {
            std::env::remove_var(name);
        }
    }

    /// The feature on; both Anthropic bases at a closed port, so the status
    /// read fails fast and the stored link answers.
    fn configure() {
        clear_env();
        for (name, value) in [
            (LINK_ENV, "1".to_string()),
            (PEM_ENV, crate::auth::es256::testing::pem()),
            (CLIENT_AUD_ENV, "https://auth.claude.com/oauth/client/aud-test".to_string()),
            (WIF_AUD_ENV, "https://api.anthropic.com".to_string()),
            (FEDERATION_RULE_ID_ENV, "fdrl_test".to_string()),
            (ORGANIZATION_ID_ENV, "00000000-0000-0000-0000-000000000001".to_string()),
            (SERVICE_ACCOUNT_ID_ENV, "svac_test".to_string()),
            (WORKSPACE_ID_ENV, "wrkspc_test".to_string()),
            (OAUTH_CLIENT_ID_ENV, "client_test".to_string()),
            (OAUTH_APP_ID_ENV, "oapp_test".to_string()),
            (API_BASE_ENV, "http://127.0.0.1:1".to_string()),
            (AUTH_BASE_ENV, "http://127.0.0.1:1".to_string()),
        ] {
            std::env::set_var(name, value);
        }
    }

    struct Ctx {
        router: Router,
        db: PgPool,
        base: String,
        name: String,
    }

    async fn ctx() -> Option<Ctx> {
        let (db, base, name) = migrated_db().await?;
        let base_state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        let mut state = Arc::try_unwrap(base_state).ok().expect("one owner");
        state.db = db.clone();
        state.credential_cipher = Some(Arc::new(allternit_cloud_core::CredentialCipher::new("test cipher key material")));
        Some(Ctx { router: routes().with_state(Arc::new(state)), db, base, name })
    }

    impl Ctx {
        async fn link(&self, user: &str, status: &str, expires_in_days: i32) {
            self.db.execute(format!("INSERT INTO users (id, email) VALUES ('{user}', '{user}@example.com');").as_str()).await.unwrap();
            sqlx::query("INSERT INTO anthropic_link_profiles (user_id, user_profile_id) VALUES ($1, $2)")
                .bind(user)
                .bind(format!("prof_{user}"))
                .execute(&self.db)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO anthropic_connections (user_id, user_profile_id, connection_id, status, refresh_expires_at)
                 VALUES ($1, $2, $3, $4, now() + make_interval(days => $5))",
            )
            .bind(user)
            .bind(format!("prof_{user}"))
            .bind(format!("ocon_{user}"))
            .bind(status)
            .bind(expires_in_days)
            .execute(&self.db)
            .await
            .unwrap();
        }

        async fn row(&self, user: &str, id: &str) -> Value {
            let (status, body) = send(&self.router, "GET", "/api/v1/connections", Some(("x-test-session-user", user)), None).await;
            assert_eq!(status, StatusCode::OK);
            body["connections"].as_array().unwrap().iter().find(|r| r["id"] == id).cloned().unwrap()
        }

        async fn done(self) {
            self.db.close().await;
            let admin = PgPool::connect(&self.base).await.unwrap();
            admin.execute(format!("DROP DATABASE {} WITH (FORCE)", self.name).as_str()).await.unwrap();
        }
    }

    macro_rules! ctx_or_skip {
        () => {
            match ctx().await {
                Some(c) => c,
                None => {
                    eprintln!("skipped: TEST_DATABASE_URL is not set or cannot create a database");
                    return;
                }
            }
        };
    }

    #[tokio::test]
    #[serial_test::serial(anthropic_client_env)]
    async fn claude_org_row_flag_off_not_available_yet() {
        clear_env();
        let c = ctx_or_skip!();
        let row = c.row("user_org_off", "claude-org").await;
        assert_eq!((row["available"].as_bool(), row["state"].as_str(), row["reason_code"].as_str()), (Some(false), Some("off"), Some("not_available_yet")), "{row}");
        assert_eq!(row["fallbacks"], serde_json::json!(["allternit-credits", "anthropic-key"]));
        assert_eq!(row["models"], serde_json::json!([]));
        let (status, body) = send(&c.router, "POST", "/api/v1/connections/claude-org/connect", Some(("x-test-session-user", "user_org_off")), None).await;
        assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("not_available")), "{body}");
        c.done().await;
    }

    #[tokio::test]
    #[serial_test::serial(anthropic_client_env)]
    async fn claude_org_row_linked_signed_in() {
        configure();
        let c = ctx_or_skip!();
        let unlinked = c.row("user_org_none", "claude-org").await;
        assert_eq!((unlinked["available"].as_bool(), unlinked["state"].as_str(), unlinked["reason_code"].as_str()), (Some(true), Some("off"), None), "{unlinked}");
        c.link("user_org_on", "linked", 300).await;
        let row = c.row("user_org_on", "claude-org").await;
        assert_eq!((row["available"].as_bool(), row["state"].as_str()), (Some(true), Some("on")), "{row}");
        assert_eq!(row["found"], "Claude organization · billed to your organization");
        assert_eq!(row["runs_on"], "server");
        assert_eq!(row["source"], "cloud");
        assert_eq!(row["terms"], "cleared");
        for model in row["models"].as_array().unwrap() {
            assert!(model.as_str().unwrap().starts_with("linked/claude/"), "{row}");
        }
        // Disconnect goes through claude-link unlink and leaves the row off.
        let (status, _) = send(&c.router, "DELETE", "/api/v1/connections/claude-org", Some(("x-test-session-user", "user_org_on")), None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(c.row("user_org_on", "claude-org").await["state"], "off");
        c.done().await;
        clear_env();
    }

    #[tokio::test]
    #[serial_test::serial(anthropic_client_env)]
    async fn claude_org_row_relink_needed_reason() {
        configure();
        let c = ctx_or_skip!();
        c.link("user_org_relink", "linked", 10).await;
        let row = c.row("user_org_relink", "claude-org").await;
        assert_eq!((row["state"].as_str(), row["reason_code"].as_str()), (Some("needs"), Some("relink_needed")), "{row}");
        assert_eq!(row["reason"], "Link your Claude organization again");
        assert_eq!(row["models"], serde_json::json!([]));
        c.done().await;
        clear_env();
    }

    #[tokio::test]
    #[serial_test::serial(anthropic_client_env)]
    async fn claude_org_row_disconnected_reason() {
        configure();
        let c = ctx_or_skip!();
        c.link("user_org_disc", "disconnected", 300).await;
        let row = c.row("user_org_disc", "claude-org").await;
        assert_eq!((row["state"].as_str(), row["reason_code"].as_str()), (Some("needs"), Some("disconnected")), "{row}");
        assert_eq!(row["reason"], "Your Claude organization is disconnected");
        c.done().await;
        clear_env();
    }

    #[tokio::test]
    #[serial_test::serial(anthropic_client_env)]
    async fn claude_org_row_person_only_true() {
        // Off and on: the flag never depends on the feature.
        clear_env();
        let c = ctx_or_skip!();
        assert_eq!(c.row("user_org_p", "claude-org").await["person_only"], true);
        configure();
        assert_eq!(c.row("user_org_p", "claude-org").await["person_only"], true);
        c.done().await;
        clear_env();
    }

    #[tokio::test]
    #[serial_test::serial(anthropic_client_env)]
    async fn claude_row_unchanged() {
        // The CLI subscription row is the same with the feature off and on.
        clear_env();
        let c = ctx_or_skip!();
        let off = c.row("user_org_c", "claude").await;
        configure();
        c.link("user_org_c", "linked", 300).await;
        let on = c.row("user_org_c", "claude").await;
        assert_eq!(off, on);
        assert!(on.get("person_only").is_none() || on["person_only"] == true, "{on}");
        assert_eq!(on["fallbacks"], serde_json::json!(["anthropic-key"]));
        assert_ne!(on["id"], "claude-org");
        c.done().await;
        clear_env();
    }
}
