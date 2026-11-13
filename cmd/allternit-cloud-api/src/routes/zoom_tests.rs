//! Zoom cloud side: webhook signature + url_validation + replay window,
//! deauthorization, refresh-token rotation under concurrent refreshes, and
//! the pod zoom-tokens route's 409s. Zoom itself is faked (vendor edge);
//! the database and routes are real.

use super::*;
use crate::routes::meeting_pod::issue_pod_token;
use crate::routes::test_support::{test_state, MockGateway};
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use std::sync::Mutex;
use tower::ServiceExt;

const SECRET: &str = "whsec-test-0123456789";

fn sign(secret: &str, ts: i64, body: &str) -> String {
    format!("v0={}", hmac_hex(secret, format!("v0:{ts}:{body}").as_bytes()))
}

async fn db_with_meetings() -> (Arc<ApiState>, PgPool) {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    sqlx::raw_sql(include_str!("../../migrations_pg/130_meetings.sql")).execute(&state.db).await.unwrap();
    let db = state.db.clone();
    (state, db)
}

fn headers(sig: &str, ts: i64) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("x-zm-signature", sig.parse().unwrap());
    h.insert("x-zm-request-timestamp", ts.to_string().parse().unwrap());
    h
}

async fn body_json(response: Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[test]
fn zoom_webhook_signature_checks_secret_body_and_window() {
    let now = 1_800_000_000;
    let body = br#"{"event":"app_deauthorized"}"#;
    let good = sign(SECRET, now, std::str::from_utf8(body).unwrap());
    assert!(verify_signature(SECRET, &now.to_string(), body, &good, now));
    assert!(verify_signature(SECRET, &now.to_string(), body, &good, now + WEBHOOK_WINDOW_SECS));
    // Replay: same signed request presented after the window.
    assert!(!verify_signature(SECRET, &now.to_string(), body, &good, now + WEBHOOK_WINDOW_SECS + 1));
    assert!(!verify_signature(SECRET, &now.to_string(), body, &good, now - WEBHOOK_WINDOW_SECS - 1));
    assert!(!verify_signature("other-secret", &now.to_string(), body, &good, now));
    assert!(!verify_signature(SECRET, &now.to_string(), br#"{"event":"x"}"#, &good, now));
    // The timestamp is part of the signed message.
    assert!(!verify_signature(SECRET, &(now + 1).to_string(), body, &good, now));
    assert!(!verify_signature(SECRET, &now.to_string(), body, good.trim_start_matches("v0="), now));
}

#[tokio::test]
async fn zoom_webhook_url_validation_and_rejections() {
    let (_state, db) = db_with_meetings().await;
    let now = Utc::now().timestamp();
    let body = r#"{"event":"endpoint.url_validation","payload":{"plainToken":"qgg8vlvZRS6UYooatFL8Aw"}}"#;
    let ok = handle_webhook(&db, Some(SECRET), &headers(&sign(SECRET, now, body), now), body.as_bytes(), now)
        .await
        .unwrap();
    let (status, json) = body_json(ok).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["plainToken"], "qgg8vlvZRS6UYooatFL8Aw");
    assert_eq!(json["encryptedToken"], hmac_hex(SECRET, b"qgg8vlvZRS6UYooatFL8Aw"));

    let unsigned = handle_webhook(&db, Some(SECRET), &HeaderMap::new(), body.as_bytes(), now).await.unwrap_err();
    assert_eq!(unsigned.status, StatusCode::UNAUTHORIZED);
    let stale = now - WEBHOOK_WINDOW_SECS - 10;
    let replay = handle_webhook(&db, Some(SECRET), &headers(&sign(SECRET, stale, body), stale), body.as_bytes(), now)
        .await
        .unwrap_err();
    assert_eq!(replay.status, StatusCode::UNAUTHORIZED);
    let forged = handle_webhook(&db, Some(SECRET), &headers(&sign("guess", now, body), now), body.as_bytes(), now)
        .await
        .unwrap_err();
    assert_eq!(forged.status, StatusCode::UNAUTHORIZED);
}

async fn insert_session(db: &PgPool, user: &str, platform: &str, url_key: &str, state: &str) -> Uuid {
    let meeting: Uuid = sqlx::query_scalar(
        "INSERT INTO meetings (user_id, platform, join_url, url_key, source) VALUES ($1, $2, $3, $4, 'link') RETURNING id",
    )
    .bind(user)
    .bind(platform)
    .bind(format!("https://zoom.us/j/{}", url_key.trim_start_matches("zoom:")))
    .bind(url_key)
    .fetch_one(db)
    .await
    .unwrap();
    sqlx::query_scalar(
        "INSERT INTO meeting_sessions (meeting_id, user_id, url_key, state, platform) VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(meeting)
    .bind(user)
    .bind(url_key)
    .bind(state)
    .bind(platform)
    .fetch_one(db)
    .await
    .unwrap()
}

async fn insert_connection(db: &PgPool, user: &str, zoom_user: &str, status: &str) {
    sqlx::query(
        "INSERT INTO zoom_connections (user_id, zoom_user_id, encrypted_refresh_token, status) VALUES ($1, $2, 'rt-0', $3)",
    )
    .bind(user)
    .bind(zoom_user)
    .bind(status)
    .execute(db)
    .await
    .unwrap();
}

#[tokio::test]
async fn zoom_deauthorization_deletes_tokens_and_ends_active_sessions() {
    let (_state, db) = db_with_meetings().await;
    insert_connection(&db, "user_a", "zu_123", "active").await;
    insert_connection(&db, "user_b", "zu_999", "active").await;
    let live = insert_session(&db, "user_a", "zoom", "zoom:81234567890", "live").await;
    let other = insert_session(&db, "user_b", "zoom", "zoom:81234567891", "live").await;

    let now = Utc::now().timestamp();
    let body = r#"{"event":"app_deauthorized","payload":{"user_id":"zu_123","account_id":"acc"}}"#;
    let response = handle_webhook(&db, Some(SECRET), &headers(&sign(SECRET, now, body), now), body.as_bytes(), now)
        .await
        .unwrap();
    let (status, json) = body_json(response).await;
    assert_eq!((status, json["ended_sessions"].as_i64()), (StatusCode::OK, Some(1)));

    let left: Vec<String> = sqlx::query_scalar("SELECT user_id FROM zoom_connections").fetch_all(&db).await.unwrap();
    assert_eq!(left, vec!["user_b".to_string()]);
    let (state, reason): (String, Option<String>) =
        sqlx::query_as("SELECT state, end_reason FROM meeting_sessions WHERE id = $1").bind(live).fetch_one(&db).await.unwrap();
    assert_eq!((state.as_str(), reason.as_deref()), ("ended", Some("left_by_user")));
    let untouched: String = sqlx::query_scalar("SELECT state FROM meeting_sessions WHERE id = $1").bind(other).fetch_one(&db).await.unwrap();
    assert_eq!(untouched, "live");
}

/// Zoom's refresh rotation: each refresh token works once and returns the
/// next one; presenting an old one is `invalid_grant`.
struct RotatingZoom {
    current: Mutex<u32>,
    stale_seen: Mutex<bool>,
    expires_in: i64,
}

#[async_trait]
impl ZoomApi for RotatingZoom {
    async fn exchange_code(&self, _code: &str) -> std::result::Result<ZoomTokens, ZoomCallError> {
        Err(ZoomCallError::Status(400))
    }
    async fn refresh(&self, refresh_token: &str) -> std::result::Result<ZoomTokens, ZoomCallError> {
        let presented = *self.current.lock().unwrap();
        if refresh_token != format!("rt-{presented}") {
            *self.stale_seen.lock().unwrap() = true;
            return Err(ZoomCallError::InvalidGrant);
        }
        // Hold the "network call" long enough for a second caller to race.
        tokio::time::sleep(Duration::from_millis(150)).await;
        let next = {
            let mut current = self.current.lock().unwrap();
            *current += 1;
            *current
        };
        Ok(ZoomTokens {
            access_token: format!("at-{next}"),
            refresh_token: format!("rt-{next}"),
            expires_in: self.expires_in,
            scopes: vec!["user:read:token".to_string()],
        })
    }
    async fn obf_token(&self, access: &str) -> std::result::Result<String, ZoomCallError> {
        Ok(format!("obf-for-{access}"))
    }
    async fn local_recording_token(&self, _a: &str, _n: &str) -> std::result::Result<String, ZoomCallError> {
        Err(ZoomCallError::Status(400))
    }
    async fn revoke(&self, _a: &str) -> std::result::Result<(), ZoomCallError> {
        Ok(())
    }
}

/// A pool with several connections on one fresh schema (the shared test
/// pool has one connection, which can't show a lock race).
async fn multi_conn_pool() -> (PgPool, String) {
    let url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://allternit:allternit_pg_2026@localhost:5432/allternit_test".to_string());
    let schema = format!("test_zoom_{}", Uuid::new_v4().simple());
    let hook_schema = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |conn, _| {
            let schema = hook_schema.clone();
            Box::pin(async move {
                sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {schema}")).execute(&mut *conn).await?;
                sqlx::query(&format!("SET search_path TO {schema}")).execute(&mut *conn).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../../migrations_pg/130_meetings.sql")).execute(&pool).await.unwrap();
    (pool, schema)
}

#[tokio::test]
async fn zoom_concurrent_refreshes_keep_the_newest_refresh_token() {
    let (db, schema) = multi_conn_pool().await;
    insert_connection(&db, "user_r", "zu_r", "active").await;
    // expires_in 0: every caller must refresh, so both hit Zoom in turn.
    let zoom = RotatingZoom { current: Mutex::new(0), stale_seen: Mutex::new(false), expires_in: 0 };
    let (a, b) = tokio::join!(
        fresh_access_token(&db, None, &zoom, "user_r"),
        fresh_access_token(&db, None, &zoom, "user_r"),
    );
    let mut got = vec![a.unwrap(), b.unwrap()];
    got.sort();
    assert_eq!(got, vec!["at-1".to_string(), "at-2".to_string()]);
    assert!(!*zoom.stale_seen.lock().unwrap(), "a refresh used an already-rotated token");
    let (refresh, status): (String, String) =
        sqlx::query_as("SELECT encrypted_refresh_token, status FROM zoom_connections WHERE user_id = 'user_r'")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!((refresh.as_str(), status.as_str()), ("rt-2", "active"));

    // A cached token with time left is reused, with no refresh.
    let cached = RotatingZoom { current: Mutex::new(2), stale_seen: Mutex::new(false), expires_in: 3600 };
    assert_eq!(fresh_access_token(&db, None, &cached, "user_r").await.unwrap(), "at-3");
    assert_eq!(fresh_access_token(&db, None, &cached, "user_r").await.unwrap(), "at-3");
    assert_eq!(*cached.current.lock().unwrap(), 3);

    // A refresh token Zoom no longer accepts marks the row needs_reauth.
    sqlx::query("UPDATE zoom_connections SET access_expires_at = now(), encrypted_refresh_token = 'rt-old'")
        .execute(&db)
        .await
        .unwrap();
    assert_eq!(fresh_access_token(&db, None, &cached, "user_r").await, Err(TokenError::NeedsReauth));
    let status: String = sqlx::query_scalar("SELECT status FROM zoom_connections").fetch_one(&db).await.unwrap();
    assert_eq!(status, "needs_reauth");
    assert_eq!(fresh_access_token(&db, None, &cached, "nobody").await, Err(TokenError::NotConnected));

    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE")).execute(&db).await.unwrap();
}

async fn post_tokens(router: &Router, sid: Uuid, token: Option<&str>) -> (StatusCode, Value) {
    let mut req = Request::builder().method("POST").uri(format!("/api/v1/meeting-sessions/{sid}/zoom-tokens"));
    if let Some(token) = token {
        req = req.header("authorization", format!("Bearer {token}"));
    }
    let response = router.clone().oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
    body_json(response).await
}

#[tokio::test]
async fn zoom_tokens_route_refuses_without_a_usable_connection() {
    let (state, db) = db_with_meetings().await;
    let router = routes().with_state(state);
    let sid = insert_session(&db, "user_t", "zoom", "zoom:81234567892", "joining").await;
    let token = issue_pod_token(&db, sid).await.unwrap();

    assert_eq!(post_tokens(&router, sid, None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(post_tokens(&router, sid, Some("mpt_wrong")).await.0, StatusCode::UNAUTHORIZED);

    let (status, json) = post_tokens(&router, sid, Some(&token)).await;
    assert_eq!((status, json["error"].as_str()), (StatusCode::CONFLICT, Some("zoom_not_connected")));

    insert_connection(&db, "user_t", "zu_t", "needs_reauth").await;
    let (status, json) = post_tokens(&router, sid, Some(&token)).await;
    assert_eq!((status, json["error"].as_str()), (StatusCode::CONFLICT, Some("zoom_not_connected")));
    assert_eq!(json["needs_reauth"], true);

    sqlx::query("UPDATE meeting_sessions SET state = 'ended', end_reason = 'left_by_user', ended_at = now() WHERE id = $1")
        .bind(sid)
        .execute(&db)
        .await
        .unwrap();
    let (status, json) = post_tokens(&router, sid, Some(&token)).await;
    assert_eq!((status, json["error"].as_str()), (StatusCode::CONFLICT, Some("session_not_active")));
}

#[tokio::test]
async fn zoom_mint_tokens_gives_obf_jwt_and_no_privilege_token_for_personal_rooms() {
    let (_state, db) = db_with_meetings().await;
    insert_connection(&db, "user_m", "zu_m", "active").await;
    let zoom = RotatingZoom { current: Mutex::new(0), stale_seen: Mutex::new(false), expires_in: 3600 };
    let config = ZoomConfig { client_id: "cid".into(), client_secret: "csecret-xyz".into(), redirect_uri: DEFAULT_REDIRECT_URI.into() };
    let body = mint_tokens(&db, None, &zoom, &config, "user_m", "zoom:my:jordan").await.unwrap();
    assert_eq!(body["obf_token"], "obf-for-at-1");
    assert!(body["app_privilege_token"].is_null());
    assert!(!body.to_string().contains("csecret-xyz"));
    let jwt = body["sdk_jwt"].as_str().unwrap();
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
    validation.required_spec_claims.clear();
    let claims = jsonwebtoken::decode::<Value>(jwt, &jsonwebtoken::DecodingKey::from_secret(b"csecret-xyz"), &validation)
        .unwrap()
        .claims;
    assert_eq!(claims["appKey"], "cid");
    assert_eq!(claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap(), 7200);
    assert_eq!(claims["exp"], claims["tokenExp"]);
}

#[test]
fn zoom_oauth_state_and_return_to() {
    let now = 1_800_000_000;
    let state = sign_state("secret", "user_1", "allternit://meetings/zoom-connected", now);
    assert_eq!(
        verify_state("secret", &state, now + 60),
        Some(("user_1".to_string(), "allternit://meetings/zoom-connected".to_string()))
    );
    assert_eq!(verify_state("secret", &state, now + STATE_TTL_SECS + 1), None);
    assert_eq!(verify_state("other", &state, now), None);
    let (body, sig) = state.split_once('.').unwrap();
    let forged_body = URL_SAFE_NO_PAD.encode(
        json!({ "u": "attacker", "r": "https://evil.example", "n": "x", "exp": now + 600 }).to_string(),
    );
    assert_eq!(verify_state("secret", &format!("{forged_body}.{sig}"), now), None);
    assert!(!body.is_empty());

    assert_eq!(safe_return_to(Some("allternit://meetings/zoom-connected")), "allternit://meetings/zoom-connected");
    assert_eq!(safe_return_to(Some("https://m.allternit.com/settings")), "https://m.allternit.com/settings");
    for bad in ["https://evil.example/x", "https://allternit.com.evil.example/", "javascript:alert(1)", "https://u:p@ai.allternit.com/"] {
        assert_eq!(safe_return_to(Some(bad)), DEFAULT_RETURN_TO, "{bad}");
    }
    assert_eq!(with_param("https://ai.allternit.com/settings/meetings?tab=a#top", "zoom", "connected"),
        "https://ai.allternit.com/settings/meetings?tab=a&zoom=connected#top");
}
