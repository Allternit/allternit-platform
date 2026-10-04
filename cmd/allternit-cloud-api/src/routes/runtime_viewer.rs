//! `POST /api/v1/runtime-devices/:id/viewer-token` — short-lived live-desktop
//! (VNC) viewer ticket for a provisioned cloud computer, used by "Sign in on
//! the cloud computer" (vendor-account sign-in) to open the computer's
//! desktop in the browser.
//!
//! The desktop is served by the runtime itself, through the same outbound
//! relay socket tunnel every other browser-facing runtime WebSocket uses (see
//! `runtime_relay`): the browser connects to cloud-api, cloud-api tunnels the
//! frames to the runtime, and the runtime's allternit-api
//! (`runtime_viewer::runtime_vnc_ws_handler`, path
//! `/api/v1/runtime-viewer/vnc?token=…`) proxies to its local VNC server. No
//! Sessions computer record and no shared desktop secret are involved.
//!
//! The viewer token is the Sessions ws-token format (HS256, header
//! `{"alg":"HS256","typ":"DT"}`, claims `{bot_id, computer_id, purpose,
//! read_only, sandbox_id, user_id, exp}`, URL-safe base64 without padding)
//! signed with the **per-runtime relay key** (`runtime_devices.credential_hash`
//! = `sha256_hex(device_token)`), the same key that signs every relayed
//! request, which the runtime already holds. `computer_id` = the runtime id,
//! `purpose` = `"vnc"`, TTL 300 s. The response's `wsUrl` carries a 30 s relay
//! socket ticket and is for immediate use; ask again to reconnect.
//!
//! `GET /api/v1/runtime-devices/:id/viewer-check` is the automated self-check for
//! the same path, so it can be verified without a person at a browser: it does
//! the same ownership, cloud-computer and wake steps, then asks the runtime (over
//! the relay, with a 60 s view-only token) to connect to its VNC socket and read
//! the RFB banner. 200 `{ok:true, runtimeId, relay:"connected", vnc:{reachable:true,
//! rfb, latencyMs}, checkedAt}`; on a failure `{ok:false, runtimeId, stage, reason}`
//! with `stage` one of `wake` (503), `relay` (502/504) or `vnc` (502). Nothing is
//! opened or changed on the computer. See surfaces/docs/api/cloud-api.mdx ("Checking the live desktop without a browser").
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
//! - No env is required. `ALLTERNIT_VIEWER_WS_BASE` (optional) overrides the
//!   public ws base (default: the request's own host, `wss://`).

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::sync::Arc;

use super::runtime_relay::{connect_or_wake_runtime, issue_socket_ticket, RelayConnect, RelayRequest};
use crate::{ApiError, ApiState};

/// Same lifetime as the Sessions computers ws-token (300 s); never longer.
pub const VIEWER_TOKEN_TTL_SECONDS: u64 = 300;
/// The runtime-side allternit-api route the relay tunnel dials.
pub const VIEWER_RUNTIME_PATH: &str = "/api/v1/runtime-viewer/vnc";
const WS_BASE_ENV: &str = "ALLTERNIT_VIEWER_WS_BASE";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/runtime-devices/:id/viewer-token", post(issue_viewer_token))
        .route("/api/v1/runtime-devices/:id/viewer-check", get(viewer_check))
}

/// The runtime-side self-check route (allternit-api `runtime_viewer`).
pub const VIEWER_CHECK_RUNTIME_PATH: &str = "/api/v1/runtime-viewer/vnc-check";

/// Turn the runtime's answer to the VNC probe into the check's reply.
fn interpret_probe(runtime_id: &str, status: u16, body: &serde_json::Value) -> (StatusCode, serde_json::Value) {
    match (status, body.get("ok").and_then(serde_json::Value::as_bool)) {
        (200, Some(true)) => (
            StatusCode::OK,
            serde_json::json!({
                "ok": true,
                "runtimeId": runtime_id,
                "relay": "connected",
                "vnc": { "reachable": true, "rfb": body["rfb"], "latencyMs": body["latencyMs"] },
                "checkedAt": chrono::Utc::now().to_rfc3339(),
            }),
        ),
        (502, Some(false)) => (
            StatusCode::BAD_GATEWAY,
            serde_json::json!({ "ok": false, "runtimeId": runtime_id, "stage": "vnc", "reason": body["reason"] }),
        ),
        (404, _) => (
            StatusCode::BAD_GATEWAY,
            serde_json::json!({ "ok": false, "runtimeId": runtime_id, "stage": "relay", "reason": "runtime_has_no_viewer", "message": "The runtime is too old to have a live desktop view. Update it." }),
        ),
        (403, _) => (
            StatusCode::BAD_GATEWAY,
            serde_json::json!({ "ok": false, "runtimeId": runtime_id, "stage": "relay", "reason": "runtime_rejected_token", "message": "The runtime did not accept the viewer token; its relay key may be out of date." }),
        ),
        (other, _) => (
            StatusCode::BAD_GATEWAY,
            serde_json::json!({ "ok": false, "runtimeId": runtime_id, "stage": "relay", "reason": format!("runtime_answered_{other}") }),
        ),
    }
}

async fn viewer_check(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(runtime_id): Path<String>) -> Result<Response, ApiError> {
    let user_id = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?.id;
    let relay_key: Option<String> = sqlx::query_scalar("SELECT credential_hash FROM runtime_devices WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL")
        .bind(&runtime_id)
        .bind(&user_id)
        .fetch_optional(&state.db)
        .await?;
    let Some(relay_key) = relay_key else {
        return Ok(coded_error(StatusCode::NOT_FOUND, "not_found", "Runtime not found"));
    };
    let is_cloud = sqlx::query_scalar::<_, String>("SELECT id FROM provisioned_instances WHERE device_id = $1 AND user_id = $2 AND status <> 'deleted' LIMIT 1")
        .bind(&runtime_id)
        .bind(&user_id)
        .fetch_optional(&state.db)
        .await?;
    if is_cloud.is_none() {
        return Ok(coded_error(StatusCode::CONFLICT, "not_a_cloud_computer", "Only a cloud computer has a live desktop view"));
    }
    let exp = (chrono::Utc::now() + chrono::Duration::seconds(60)).timestamp() as u64;
    let token = sign_viewer_token(&relay_key, &runtime_id, &user_id, true, exp);
    let request = RelayRequest {
        method: "GET".into(),
        path: format!("{VIEWER_CHECK_RUNTIME_PATH}?token={token}"),
        headers: Default::default(),
        body: String::new(),
        body_encoding: "utf8".into(),
    };
    let response = super::runtime_relay::relay_request_to_runtime(
        &state.db,
        &state.contabo_runtime_service,
        &state.quota_service,
        &state.provisioning_service,
        &user_id,
        &runtime_id,
        request,
    )
    .await?;
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024).await.unwrap_or_default();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    // The relay answers these itself when the computer is asleep or gone.
    if status == 503 {
        let reason = body.get("error").and_then(serde_json::Value::as_str).unwrap_or("runtime_offline");
        return Ok((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "ok": false, "runtimeId": runtime_id, "stage": "wake", "reason": reason })),
        )
            .into_response());
    }
    if status == 504 {
        return Ok((
            StatusCode::GATEWAY_TIMEOUT,
            Json(serde_json::json!({ "ok": false, "runtimeId": runtime_id, "stage": "relay", "reason": "runtime_timeout" })),
        )
            .into_response());
    }
    let (code, out) = interpret_probe(&runtime_id, status, &body);
    Ok((code, Json(out)).into_response())
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

/// Public ws base for the browser: the optional env override, else the host
/// the request came in on (`x-forwarded-host` behind the proxy). `ws://` only
/// for a loopback host.
fn ws_base(headers: &HeaderMap) -> Option<String> {
    if let Some(base) = std::env::var(WS_BASE_ENV).ok().filter(|v| !v.trim().is_empty()) {
        return Some(base.trim().trim_end_matches('/').to_string());
    }
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(axum::http::header::HOST))
        .and_then(|v| v.to_str().ok())
        .map(|h| h.split(',').next().unwrap_or(h).trim())
        .filter(|h| !h.is_empty())?;
    let local = host.starts_with("localhost") || host.starts_with("127.0.0.1") || host.starts_with("[::1]");
    Some(format!("{}://{host}", if local { "ws" } else { "wss" }))
}

fn ws_url(base: &str, runtime_id: &str, ticket: &str) -> String {
    format!(
        "{base}/api/v1/runtime-devices/{}/socket?ticket={}",
        urlencoding::encode(runtime_id),
        urlencoding::encode(ticket)
    )
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
    let Some(base) = ws_base(&headers) else {
        return Ok(coded_error(
            StatusCode::BAD_REQUEST,
            "no_host",
            "Cannot work out the public address for the live desktop view",
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

    // Per-runtime key: the same digest that signs relayed requests.
    let relay_key: Option<String> = sqlx::query_scalar(
        "SELECT credential_hash FROM runtime_devices WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(&runtime_id)
    .bind(&user_id)
    .fetch_optional(&state.db)
    .await?;
    let Some(relay_key) = relay_key else {
        return Ok(coded_error(StatusCode::NOT_FOUND, "not_found", "Runtime not found"));
    };
    let expires = chrono::Utc::now() + chrono::Duration::seconds(VIEWER_TOKEN_TTL_SECONDS as i64);
    let token = sign_viewer_token(&relay_key, &runtime_id, &user_id, read_only, expires.timestamp() as u64);
    let tunnel_path = format!("{VIEWER_RUNTIME_PATH}?token={token}");
    let ticket = issue_socket_ticket(&state, &user_id, &runtime_id, tunnel_path).await?;
    let ticket = ticket["ticket"].as_str().unwrap_or_default().to_string();
    Ok(Json(serde_json::json!({
        "token": token,
        "expiresAt": expires.to_rfc3339(),
        "wsUrl": ws_url(&base, &runtime_id, &ticket),
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
        std::env::remove_var(WS_BASE_ENV);
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
    fn ws_url_targets_the_relay_socket_and_encodes_the_runtime_id() {
        assert_eq!(
            ws_url("wss://h", "a b", "T"),
            "wss://h/api/v1/runtime-devices/a%20b/socket?ticket=T"
        );
    }

    #[test]
    fn ws_base_comes_from_the_request_host_or_the_env_override() {
        let _g = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(WS_BASE_ENV);
        let mut h = HeaderMap::new();
        assert_eq!(ws_base(&h), None);
        h.insert("host", "localhost:8080".parse().unwrap());
        assert_eq!(ws_base(&h).as_deref(), Some("ws://localhost:8080"));
        h.insert("x-forwarded-host", "api.allternit.com".parse().unwrap());
        assert_eq!(ws_base(&h).as_deref(), Some("wss://api.allternit.com"));
        std::env::set_var(WS_BASE_ENV, "wss://edge.test/");
        assert_eq!(ws_base(&h).as_deref(), Some("wss://edge.test"));
        std::env::remove_var(WS_BASE_ENV);
    }

    #[tokio::test]
    async fn owner_gets_a_scoped_short_lived_token() {
        let _g = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (router, db) = setup().await;
        cloud_computer(&db, "rt_v1", DEV_USER).await;
        let _conn = crate::routes::runtime_relay::register_test_connection("rt_v1").await;
        let mut request = authed_request("POST", PATH, "{}");
        request.headers_mut().insert("x-forwarded-host", "api.test".parse().unwrap());
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        let token = body["token"].as_str().unwrap();
        assert_eq!(body["runtimeId"], "rt_v1");
        let ws_url = body["wsUrl"].as_str().unwrap();
        let ticket = ws_url
            .strip_prefix("wss://api.test/api/v1/runtime-devices/rt_v1/socket?ticket=")
            .expect("ws url targets the relay socket");
        // The ticket tunnels to the runtime-side viewer route, carrying the token.
        let t = crate::routes::runtime_relay::take_socket_ticket(ticket).await.unwrap();
        assert_eq!(t.runtime_id, "rt_v1");
        assert_eq!(t.path, format!("{VIEWER_RUNTIME_PATH}?token={token}"));
        // Signed with the per-runtime relay key, not a shared env secret.
        let relay_key = crate::routes::runtime_pairing::sha256_hex(b"token-of-rt_v1");
        let claims = verify_viewer_token(&relay_key, token, "rt_v1").unwrap();
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
    async fn needs_no_env_and_asks_for_a_host_when_there_is_none() {
        let _g = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (router, db) = setup().await;
        cloud_computer(&db, "rt_v1", DEV_USER).await;
        let response = router.oneshot(authed_request("POST", PATH, "{}")).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(json_body(response).await["error"], "no_host");
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

    #[test]
    fn the_runtimes_probe_answer_becomes_a_stage_and_reason() {
        let (code, ok) = interpret_probe("rt", 200, &serde_json::json!({ "ok": true, "rfb": "RFB 003.008", "latencyMs": 3 }));
        assert_eq!((code, ok["ok"].as_bool(), ok["vnc"]["rfb"].as_str(), ok["relay"].as_str()), (StatusCode::OK, Some(true), Some("RFB 003.008"), Some("connected")));
        let (code, vnc) = interpret_probe("rt", 502, &serde_json::json!({ "ok": false, "reason": "vnc_unreachable" }));
        assert_eq!((code, vnc["stage"].as_str(), vnc["reason"].as_str()), (StatusCode::BAD_GATEWAY, Some("vnc"), Some("vnc_unreachable")));
        for (status, reason) in [(404, "runtime_has_no_viewer"), (403, "runtime_rejected_token"), (500, "runtime_answered_500")] {
            let (code, out) = interpret_probe("rt", status, &serde_json::Value::Null);
            assert_eq!((code, out["stage"].as_str(), out["reason"].as_str(), out["ok"].as_bool()), (StatusCode::BAD_GATEWAY, Some("relay"), Some(reason), Some(false)));
        }
    }

    #[tokio::test]
    async fn the_check_has_the_same_404_409_and_401_as_the_token() {
        let _g = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (router, db) = setup().await;
        let check = "/api/v1/runtime-devices/rt_v1/viewer-check";
        cloud_computer(&db, "rt_foreign", "someone-else").await;
        let unknown = router.clone().oneshot(authed_request("GET", "/api/v1/runtime-devices/rt_foreign/viewer-check", "")).await.unwrap();
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
        seed_runtime_device(&db, "rt_v1", DEV_USER).await;
        let desktop = router.clone().oneshot(authed_request("GET", check, "")).await.unwrap();
        assert_eq!(desktop.status(), StatusCode::CONFLICT);
        assert_eq!(json_body(desktop).await["error"], "not_a_cloud_computer");
        let anon = Request::builder().method("GET").uri(check).body(Body::empty()).unwrap();
        assert_eq!(router.oneshot(anon).await.unwrap().status(), StatusCode::UNAUTHORIZED);
        cleanup();
    }
}
