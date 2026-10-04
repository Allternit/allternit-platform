//! `POST /api/v1/runtime-devices/:id/viewer-token` — short-lived live-desktop
//! (VNC) viewer token for a provisioned cloud computer, used by "Sign in on
//! the cloud computer" (vendor-account sign-in) to open the computer's
//! desktop in the browser.
//!
//! The token is the SAME signed format the Sessions `computers` ws-token mints
//! (`allternit-api` `bot_desktop_stream::sign_computer_token`: HS256, header
//! `{"alg":"HS256","typ":"DT"}`, claims `{bot_id, computer_id, purpose,
//! read_only, sandbox_id, user_id, exp}`, URL-safe base64 without padding,
//! HMAC key `ALLTERNIT_DESKTOP_WS_SECRET`), with `computer_id` = the runtime
//! device id and `purpose` = `"vnc"`, so the existing VNC ws verifier accepts
//! it. TTL is 300 s, the same as the Sessions path.
//!
//! Rules:
//! - Auth is the sibling runtime-devices convention (`resolve_user_scoped(..,
//!   "compute")`); no/invalid credentials answer 401.
//! - Only the owning user gets a token. Unknown, revoked and other users'
//!   ids all answer the same 404 (no existence oracle).
//! - Only provisioned cloud computers qualify (a `provisioned_instances` row
//!   for this device that is not deleted); anything else (a Desktop / BYO
//!   runtime) answers 409 `not_a_cloud_computer`.
//! - A sleeping computer is woken exactly like the relay proxy wakes it
//!   (`connect_or_wake_runtime`); if it is still booting the answer is 503
//!   `runtime_warming`, if it cannot be reached 503 `runtime_offline`.
//! - Inert when unconfigured: without `ALLTERNIT_DESKTOP_WS_SECRET` and
//!   `ALLTERNIT_VIEWER_WS_BASE` the route answers 503 `viewer_not_configured`
//!   (checked before any wake, so nothing is started for a token we cannot
//!   mint). Nothing is read at boot.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::sync::Arc;

use super::runtime_relay::{connect_or_wake_runtime, RelayConnect};
use crate::{ApiError, ApiState};

/// Same lifetime as the Sessions computers ws-token (300 s); never longer.
pub const VIEWER_TOKEN_TTL_SECONDS: u64 = 300;
const SECRET_ENV: &str = "ALLTERNIT_DESKTOP_WS_SECRET";
const WS_BASE_ENV: &str = "ALLTERNIT_VIEWER_WS_BASE";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/runtime-devices/:id/viewer-token", post(issue_viewer_token))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ViewerTokenRequest {
    /// View-only token. Defaults to false: signing in to a vendor account
    /// needs keyboard/mouse. The caller already owns the computer.
    #[serde(default)]
    read_only: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ViewerClaims {
    pub bot_id: String,
    pub computer_id: Option<String>,
    pub purpose: Option<String>,
    #[serde(default)]
    pub read_only: bool,
    pub sandbox_id: String,
    pub user_id: String,
    pub exp: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ViewerTokenError {
    Format,
    Signature,
    Expired,
    Mismatch,
}

fn sign(secret: &str, input: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC takes any key size");
    mac.update(input.as_bytes());
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

/// Mint a viewer token (format shared with `sign_computer_token`).
pub fn sign_viewer_token(secret: &str, runtime_id: &str, user_id: &str, read_only: bool, exp: u64) -> String {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"DT"}"#);
    let claims = ViewerClaims {
        bot_id: String::new(),
        computer_id: Some(runtime_id.to_string()),
        purpose: Some("vnc".to_string()),
        read_only,
        sandbox_id: runtime_id.to_string(),
        user_id: user_id.to_string(),
        exp,
    };
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap_or_default());
    let signing_input = format!("{header}.{payload}");
    format!("{signing_input}.{}", sign(secret, &signing_input))
}

/// Verify signature, expiry and that the token names this runtime + `vnc`.
pub fn verify_viewer_token(secret: &str, token: &str, runtime_id: &str) -> Result<ViewerClaims, ViewerTokenError> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(ViewerTokenError::Format);
    }
    let signature = URL_SAFE_NO_PAD.decode(parts[2]).map_err(|_| ViewerTokenError::Format)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC takes any key size");
    mac.update(format!("{}.{}", parts[0], parts[1]).as_bytes());
    mac.verify_slice(&signature).map_err(|_| ViewerTokenError::Signature)?;
    let payload = URL_SAFE_NO_PAD.decode(parts[1]).map_err(|_| ViewerTokenError::Format)?;
    let claims: ViewerClaims = serde_json::from_slice(&payload).map_err(|_| ViewerTokenError::Format)?;
    if claims.exp < chrono::Utc::now().timestamp() as u64 {
        return Err(ViewerTokenError::Expired);
    }
    if claims.computer_id.as_deref() != Some(runtime_id) || claims.purpose.as_deref() != Some("vnc") {
        return Err(ViewerTokenError::Mismatch);
    }
    Ok(claims)
}

fn coded_error(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(serde_json::json!({ "error": code, "code": code, "message": message }))).into_response()
}

fn viewer_config() -> Option<(String, String)> {
    let secret = std::env::var(SECRET_ENV).ok().filter(|v| !v.is_empty())?;
    let base = std::env::var(WS_BASE_ENV).ok().filter(|v| !v.trim().is_empty())?;
    Some((secret, base.trim().trim_end_matches('/').to_string()))
}

fn ws_url(base: &str, runtime_id: &str, token: &str) -> String {
    format!("{base}/{}/vnc?token={token}", urlencoding::encode(runtime_id))
}

async fn issue_viewer_token(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(runtime_id): Path<String>,
    body: Option<Json<ViewerTokenRequest>>,
) -> Result<Response, ApiError> {
    let user_id = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?.id;
    let read_only = body.map(|Json(b)| b.read_only).unwrap_or(false);

    // Ownership first, with one 404 for unknown / revoked / someone else's.
    let owned = sqlx::query_scalar::<_, String>(
        "SELECT id FROM runtime_devices WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(&runtime_id)
    .bind(&user_id)
    .fetch_optional(&state.db)
    .await?;
    if owned.is_none() {
        return Ok(coded_error(StatusCode::NOT_FOUND, "not_found", "Runtime not found"));
    }
    let is_cloud = sqlx::query_scalar::<_, String>(
        "SELECT id FROM provisioned_instances WHERE device_id = $1 AND user_id = $2 AND status <> 'deleted' LIMIT 1",
    )
    .bind(&runtime_id)
    .bind(&user_id)
    .fetch_optional(&state.db)
    .await?;
    if is_cloud.is_none() {
        return Ok(coded_error(
            StatusCode::CONFLICT,
            "not_a_cloud_computer",
            "Only a cloud computer has a live desktop view",
        ));
    }
    let Some((secret, base)) = viewer_config() else {
        return Ok(coded_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "viewer_not_configured",
            "The live desktop view is not configured on this server",
        ));
    };

    // Wake the way the relay proxy does.
    match connect_or_wake_runtime(
        &state.db,
        &state.contabo_runtime_service,
        &state.quota_service,
        &state.provisioning_service,
        &runtime_id,
        "runtime:connect",
        "/health",
    )
    .await?
    {
        RelayConnect::Connected(_) => {}
        RelayConnect::Warming => {
            return Ok(coded_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "runtime_warming",
                "The cloud computer is waking up; retry shortly",
            ))
        }
        RelayConnect::Offline => {
            return Ok(coded_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "runtime_offline",
                "The cloud computer is offline",
            ))
        }
    }

    let expires = chrono::Utc::now() + chrono::Duration::seconds(VIEWER_TOKEN_TTL_SECONDS as i64);
    let token = sign_viewer_token(&secret, &runtime_id, &user_id, read_only, expires.timestamp() as u64);
    Ok(Json(serde_json::json!({
        "token": token,
        "expiresAt": expires.to_rfc3339(),
        "wsUrl": ws_url(&base, &runtime_id, &token),
        "runtimeId": runtime_id,
    }))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
    use crate::routes::test_support::{
        authed_request, seed_runtime_device, test_state, MockGateway, DEV_USER,
    };
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const PATH: &str = "/api/v1/runtime-devices/rt_v1/viewer-token";

    async fn setup() -> (Router, sqlx::PgPool) {
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
        std::env::set_var(SECRET_ENV, "unit-test-secret");
        std::env::set_var(WS_BASE_ENV, "wss://api.test/api/v1/computers/");
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        sqlx::query(
            "CREATE TABLE provisioned_instances (id TEXT PRIMARY KEY, user_id TEXT NOT NULL, device_id TEXT, status TEXT NOT NULL DEFAULT 'running', created_at TIMESTAMPTZ NOT NULL DEFAULT NOW())",
        )
        .execute(&state.db)
        .await
        .unwrap();
        let db = state.db.clone();
        (routes().with_state(state), db)
    }

    async fn cloud_computer(db: &sqlx::PgPool, runtime: &str, owner: &str) {
        seed_runtime_device(db, runtime, owner).await;
        sqlx::query("INSERT INTO provisioned_instances (id, user_id, device_id) VALUES ($1, $2, $3)")
            .bind(format!("pi_{runtime}"))
            .bind(owner)
            .bind(runtime)
            .execute(db)
            .await
            .unwrap();
    }

    async fn json_body(response: Response) -> serde_json::Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    }

    fn cleanup() {
        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
        std::env::remove_var(SECRET_ENV);
        std::env::remove_var(WS_BASE_ENV);
    }

    #[test]
    fn token_round_trips_and_is_scoped_to_runtime() {
        let exp = chrono::Utc::now().timestamp() as u64 + 60;
        let token = sign_viewer_token("s", "rt_1", "u1", false, exp);
        let claims = verify_viewer_token("s", &token, "rt_1").unwrap();
        assert_eq!(claims.user_id, "u1");
        assert_eq!(claims.purpose.as_deref(), Some("vnc"));
        assert_eq!(verify_viewer_token("s", &token, "rt_2"), Err(ViewerTokenError::Mismatch));
        assert_eq!(verify_viewer_token("other", &token, "rt_1"), Err(ViewerTokenError::Signature));
        assert_eq!(verify_viewer_token("s", "a.b", "rt_1"), Err(ViewerTokenError::Format));
    }

    #[test]
    fn expired_token_is_rejected() {
        let token = sign_viewer_token("s", "rt_1", "u1", true, chrono::Utc::now().timestamp() as u64 - 5);
        assert_eq!(verify_viewer_token("s", &token, "rt_1"), Err(ViewerTokenError::Expired));
    }

    #[test]
    fn ws_url_encodes_the_runtime_id() {
        assert_eq!(ws_url("wss://h/c", "a b", "T"), "wss://h/c/a%20b/vnc?token=T");
    }

    #[tokio::test]
    async fn owner_gets_a_scoped_short_lived_token() {
        let _g = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (router, db) = setup().await;
        cloud_computer(&db, "rt_v1", DEV_USER).await;
        let _conn = crate::routes::runtime_relay::register_test_connection("rt_v1").await;
        let response = router.oneshot(authed_request("POST", PATH, "{}")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        let token = body["token"].as_str().unwrap();
        assert_eq!(body["runtimeId"], "rt_v1");
        assert_eq!(body["wsUrl"], format!("wss://api.test/api/v1/computers/rt_v1/vnc?token={token}"));
        let claims = verify_viewer_token("unit-test-secret", token, "rt_v1").unwrap();
        assert_eq!(claims.user_id, DEV_USER);
        assert!(!claims.read_only);
        let ttl = claims.exp as i64 - chrono::Utc::now().timestamp();
        assert!(ttl > 0 && ttl <= 300, "ttl {ttl}");
        let expires = chrono::DateTime::parse_from_rfc3339(body["expiresAt"].as_str().unwrap()).unwrap();
        assert_eq!(expires.timestamp() as u64, claims.exp);
        cleanup();
    }

    #[tokio::test]
    async fn other_users_and_unknown_ids_get_the_same_404() {
        let _g = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (router, db) = setup().await;
        cloud_computer(&db, "rt_v1", "someone-else").await;
        let foreign = router.clone().oneshot(authed_request("POST", PATH, "{}")).await.unwrap();
        let unknown = router
            .oneshot(authed_request("POST", "/api/v1/runtime-devices/nope/viewer-token", "{}"))
            .await
            .unwrap();
        assert_eq!(foreign.status(), StatusCode::NOT_FOUND);
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
        assert_eq!(json_body(foreign).await, json_body(unknown).await);
        cleanup();
    }

    #[tokio::test]
    async fn desktop_runtime_is_409_not_a_cloud_computer() {
        let _g = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (router, db) = setup().await;
        seed_runtime_device(&db, "rt_v1", DEV_USER).await;
        let response = router.oneshot(authed_request("POST", PATH, "{}")).await.unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(json_body(response).await["error"], "not_a_cloud_computer");
        cleanup();
    }

    #[tokio::test]
    async fn unconfigured_viewer_is_503_viewer_not_configured() {
        let _g = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (router, db) = setup().await;
        cloud_computer(&db, "rt_v1", DEV_USER).await;
        std::env::remove_var(WS_BASE_ENV);
        let response = router.oneshot(authed_request("POST", PATH, "{}")).await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(json_body(response).await["error"], "viewer_not_configured");
        cleanup();
    }

    #[tokio::test]
    async fn unauthenticated_is_401() {
        let _g = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (router, db) = setup().await;
        cloud_computer(&db, "rt_v1", DEV_USER).await;
        let request = Request::builder()
            .method("POST")
            .uri(PATH)
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        cleanup();
    }
}
