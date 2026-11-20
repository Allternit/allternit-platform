//! Link your Claude organization (CONTRACT-oauth-linked-billing-2026-11.md,
//! Part B, B2; flow in `services::claude_link`).
//!
//! - `POST   /api/v1/claude-link/start` `{"return_to"?: url}` →
//!   `{"login":{"kind":"oauth","url","expires_at","poll":true}}`
//! - `GET    /api/v1/claude-link/callback?code&state` (or `?error&state`), no
//!   session: always a 302 to the return URL with `connected=claude-org` or
//!   `error=` one of `bad_state`, `denied`, `exchange_failed`,
//!   `org_not_eligible`, `user_profile_already_linked`, `not_configured`,
//!   `could_not_start`. Like the Copilot callback (connections 9.5), the
//!   return URL is the allow-listed one the client passed to `start` (web,
//!   PWA, `allternit://`, `gizzi://`), so desktop and phone come back the same way.
//! - `GET    /api/v1/claude-link` →
//!   `{"state":"unlinked|linked|relink_needed|disconnected","org_name","linked_at","relink_by","spend_limit"}`
//! - `PATCH  /api/v1/claude-link` → reserved: 400 `no_fields`
//! - `DELETE /api/v1/claude-link` → 204
//!
//! Every route answers 503 `not_available_yet` until the feature is
//! configured (`ALLTERNIT_CLAUDE_LINK=1` plus the env in B1, and the
//! credential cipher); the callback then 302s with `error=not_configured`.
//! All but the callback need a Clerk session: API keys, platform keys and
//! OAuth tokens get 403.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::SecondsFormat;
use serde::Deserialize;
use serde_json::json;

use crate::auth::anthropic_client;
use crate::services::claude_link::{self, ClaudeLink, LinkError};
use crate::ApiState;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/claude-link", get(status).patch(patch_link).delete(unlink))
        .route("/api/v1/claude-link/start", post(start))
        .route("/api/v1/claude-link/callback", get(callback))
}

impl IntoResponse for LinkError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": { "code": self.code, "message": self.message } }))).into_response()
    }
}

fn reply(result: Result<Response, LinkError>) -> Response {
    let mut response = result.unwrap_or_else(IntoResponse::into_response);
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    response
}

/// The feature, or 503 `not_available_yet`.
fn configured(state: &ApiState) -> Result<ClaudeLink, LinkError> {
    if !anthropic_client::is_configured() {
        return Err(LinkError::not_available());
    }
    claude_link::service(state).ok_or_else(LinkError::not_available)
}

/// The signed-in caller: a Clerk session only.
async fn session_user(state: &ApiState, headers: &HeaderMap) -> Result<String, LinkError> {
    #[cfg(test)]
    if let Some(id) = headers.get("x-test-session-user").and_then(|v| v.to_str().ok()) {
        return Ok(id.to_string());
    }
    let forbidden = || LinkError::new(StatusCode::FORBIDDEN, "session_required", "This endpoint needs a signed-in session.");
    let bearer = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
    // OAuth tokens and client secrets never act as the account (contract Part A).
    if bearer.is_some_and(|t| {
        [
            crate::services::oauth_tokens::ACCESS_TOKEN_PREFIX,
            crate::services::oauth_tokens::REFRESH_TOKEN_PREFIX,
            crate::services::oauth_apps::CLIENT_SECRET_PREFIX,
        ]
        .iter()
        .any(|p| t.starts_with(p))
    }) {
        return Err(forbidden());
    }
    match crate::auth::require_session_user(&state.db, headers).await {
        Ok(user) => Ok(user.id),
        Err(crate::error::ApiError::Forbidden(_)) => Err(forbidden()),
        Err(_) => Err(LinkError::new(StatusCode::UNAUTHORIZED, "unauthorized", "Sign in first.")),
    }
}

#[derive(Deserialize, Default)]
struct StartBody {
    #[serde(default)]
    return_to: Option<String>,
}

async fn start(State(state): State<Arc<ApiState>>, headers: HeaderMap, body: Bytes) -> Response {
    reply(start_inner(&state, &headers, &body).await)
}

async fn start_inner(state: &ApiState, headers: &HeaderMap, body: &[u8]) -> Result<Response, LinkError> {
    let service = configured(state)?;
    let user = session_user(state, headers).await?;
    let body: StartBody = if body.iter().all(u8::is_ascii_whitespace) {
        StartBody::default()
    } else {
        serde_json::from_slice(body).map_err(|_| LinkError::new(StatusCode::BAD_REQUEST, "bad_request", "The body must be JSON."))?
    };
    let (url, expires_at) = service.start(&user, body.return_to.as_deref()).await?;
    Ok(Json(json!({ "login": {
        "kind": "oauth",
        "url": url,
        "expires_at": expires_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        "poll": true,
    }}))
    .into_response())
}

#[derive(Deserialize, Default)]
struct CallbackQuery {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// auth.claude.com's redirect. No session: the single-use `state` names the
/// account. Always a 302. Never logs the query (it carries the code).
async fn callback(State(state): State<Arc<ApiState>>, Query(query): Query<CallbackQuery>) -> Response {
    let target = match configured(&state) {
        Ok(service) => service.callback(query.code.as_deref(), query.state.as_deref(), query.error.as_deref()).await,
        Err(_) => claude_link::not_configured_return(),
    };
    let mut response = StatusCode::FOUND.into_response();
    if let Ok(location) = HeaderValue::from_str(&target) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    reply(Ok(response))
}

async fn status(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    reply(status_inner(&state, &headers).await)
}

async fn status_inner(state: &ApiState, headers: &HeaderMap) -> Result<Response, LinkError> {
    let service = configured(state)?;
    let user = session_user(state, headers).await?;
    Ok(Json(service.status(&user).await?.to_json()).into_response())
}

/// Reserved for later settings; v1 has no fields.
async fn patch_link(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    reply(patch_inner(&state, &headers).await)
}

async fn patch_inner(state: &ApiState, headers: &HeaderMap) -> Result<Response, LinkError> {
    configured(state)?;
    session_user(state, headers).await?;
    Err(LinkError::new(StatusCode::BAD_REQUEST, "no_fields", "There is nothing to change yet."))
}

async fn unlink(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    reply(unlink_inner(&state, &headers).await)
}

async fn unlink_inner(state: &ApiState, headers: &HeaderMap) -> Result<Response, LinkError> {
    let service = configured(state)?;
    let user = session_user(state, headers).await?;
    service.unlink(&user).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::anthropic_client::*;
    use crate::auth::es256;
    use crate::routes::test_support::{test_state, MockGateway};
    use crate::services::inference_keys::decrypt_parts;
    use allternit_cloud_core::CredentialCipher;
    use serde_json::Value;
    use axum::body::Body;
    use axum::extract::Request;
    use std::collections::{BTreeSet, HashMap};
    use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};
    use std::sync::Mutex;
    use tower::ServiceExt;

    const PAR_URI: &str = "urn:ietf:params:oauth:request_uri:req_fake";
    const ACCESS: &str = "sk-ant-at-SECRET-ACCESS";
    const REFRESH: &str = "sk-ant-rt-SECRET-REFRESH";

    #[derive(Debug, Clone)]
    struct Seen {
        method: String,
        path: String,
        query: String,
        auth: Option<String>,
        beta: Option<String>,
        body: Value,
    }

    /// One fake for both bases (api.anthropic.com and auth.claude.com).
    struct Fake {
        seen: Mutex<Vec<Seen>>,
        profiles_made: AtomicUsize,
        /// The code exchange's reply; `None` is a full success.
        token_reply: Mutex<Option<(u16, Value)>>,
        status_replies: Mutex<HashMap<String, Value>>,
        revoke_status: AtomicU16,
    }

    impl Fake {
        fn requests(&self, method: &str, path: &str) -> Vec<Seen> {
            self.seen.lock().unwrap().iter().filter(|s| s.method == method && s.path == path).cloned().collect()
        }
        fn code_exchanges(&self) -> Vec<Seen> {
            self.requests("POST", "/v1/oauth/token").into_iter().filter(|s| s.body["grant_type"] == "authorization_code").collect()
        }
        fn par_state(&self) -> String {
            self.requests("POST", "/v1/oauth/par").last().expect("a PAR").body["state"].as_str().unwrap().to_string()
        }
    }

    async fn handle(State(fake): State<Arc<Fake>>, request: Request) -> Response {
        let method = request.method().to_string();
        let path = request.uri().path().to_string();
        let query = request.uri().query().unwrap_or_default().to_string();
        let header = |name: &str| request.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
        let (auth, beta) = (header("authorization"), header("anthropic-beta"));
        let bytes = axum::body::to_bytes(request.into_body(), 1 << 20).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        fake.seen.lock().unwrap().push(Seen { method: method.clone(), path: path.clone(), query, auth, beta, body: body.clone() });
        let ok = |status: u16, value: Value| (StatusCode::from_u16(status).unwrap(), Json(value)).into_response();
        match (method.as_str(), path.as_str()) {
            ("POST", "/v1/oauth/token") if body["grant_type"] == JWT_BEARER => {
                ok(200, json!({ "access_token": "sk-ant-oat01-fake", "token_type": "bearer", "expires_in": 3600 }))
            }
            ("POST", "/v1/oauth/token") => match fake.token_reply.lock().unwrap().clone() {
                Some((status, value)) => ok(status, value),
                None => ok(
                    200,
                    json!({
                        "access_token": ACCESS, "token_type": "bearer", "expires_in": 3600,
                        "scope": "billing:attribution offline_access", "connection_id": "ocon_fake",
                        "refresh_token": REFRESH, "refresh_token_expires_in": 31_536_000,
                    }),
                ),
            },
            ("POST", "/v1/user_profiles") => {
                let n = fake.profiles_made.fetch_add(1, Ordering::SeqCst) + 1;
                ok(200, json!({ "id": format!("uprof_fake_{n}"), "type": "user_profile" }))
            }
            ("POST", "/v1/oauth/par") => ok(201, json!({ "request_uri": PAR_URI, "expires_in": 600 })),
            ("GET", p) if p.starts_with("/v1/user_profiles/") => {
                let id = p.trim_start_matches("/v1/user_profiles/").to_string();
                let reply = fake.status_replies.lock().unwrap().get(&id).cloned();
                ok(200, reply.unwrap_or_else(|| json!({ "id": id, "status": "not_connected" })))
            }
            ("POST", "/v1/oauth/revoke") => ok(fake.revoke_status.load(Ordering::SeqCst), json!({ "revoked_connection_ids": ["ocon_fake"] })),
            _ => ok(404, json!({ "error": "not_found" })),
        }
    }

    async fn fake_anthropic() -> (Arc<Fake>, String) {
        let fake = Arc::new(Fake {
            seen: Mutex::default(),
            profiles_made: AtomicUsize::new(0),
            token_reply: Mutex::new(None),
            status_replies: Mutex::default(),
            revoke_status: AtomicU16::new(200),
        });
        let app = Router::new().fallback(handle).with_state(fake.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (fake, format!("http://{addr}"))
    }

    fn clear_env() {
        for name in [LINK_ENV, PEM_ENV, ISSUER_BASE_ENV, API_BASE_ENV, AUTH_BASE_ENV, claude_link::REDIRECT_ENV].into_iter().chain(REQUIRED_ENV) {
            std::env::remove_var(name);
        }
    }

    /// The feature on, both base URLs at the fake.
    fn configure(base: &str) {
        clear_env();
        for (name, value) in [
            (LINK_ENV, "1".to_string()),
            (PEM_ENV, es256::testing::pem()),
            (CLIENT_AUD_ENV, "https://auth.claude.com/oauth/client/aud-test".to_string()),
            (WIF_AUD_ENV, "https://api.anthropic.com".to_string()),
            (FEDERATION_RULE_ID_ENV, "fdrl_test".to_string()),
            (ORGANIZATION_ID_ENV, "00000000-0000-0000-0000-000000000001".to_string()),
            (SERVICE_ACCOUNT_ID_ENV, "svac_test".to_string()),
            (WORKSPACE_ID_ENV, "wrkspc_test".to_string()),
            (OAUTH_CLIENT_ID_ENV, "client_test".to_string()),
            (OAUTH_APP_ID_ENV, "oapp_test".to_string()),
            (API_BASE_ENV, base.to_string()),
            (AUTH_BASE_ENV, base.to_string()),
        ] {
            std::env::set_var(name, value);
        }
    }

    struct Ctx {
        state: Arc<ApiState>,
        cipher: Arc<CredentialCipher>,
    }

    async fn ctx() -> Ctx {
        let base = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        let mut state = Arc::try_unwrap(base).ok().expect("one owner");
        let cipher = Arc::new(CredentialCipher::new("test cipher key material"));
        state.credential_cipher = Some(cipher.clone());
        // 151 widens inference_usage; a minimal one stands in for 007.
        sqlx::raw_sql("CREATE TABLE IF NOT EXISTS inference_usage (id TEXT PRIMARY KEY, created_at TIMESTAMPTZ DEFAULT now())")
            .execute(&state.db)
            .await
            .unwrap();
        sqlx::raw_sql(&include_str!("../../migrations_pg/151_anthropic_links.sql").replace("public.", ""))
            .execute(&state.db)
            .await
            .expect("151 applies");
        Ctx { state: Arc::new(state), cipher }
    }

    fn user() -> String {
        format!("user_{}", uuid::Uuid::new_v4().simple())
    }

    async fn send_raw(c: &Ctx, method: &str, uri: &str, headers: &[(&str, &str)], body: Option<Value>) -> Response {
        let mut req = axum::http::Request::builder().method(method).uri(uri);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let body = match body {
            Some(b) => {
                req = req.header("content-type", "application/json");
                Body::from(b.to_string())
            }
            None => Body::empty(),
        };
        routes().with_state(c.state.clone()).oneshot(req.body(body).unwrap()).await.unwrap()
    }

    async fn send(c: &Ctx, method: &str, uri: &str, user: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
        let headers: Vec<(&str, &str)> = user.map(|u| ("x-test-session-user", u)).into_iter().collect();
        let response = send_raw(c, method, uri, &headers, body).await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// The callback's Location.
    async fn callback_to(c: &Ctx, query: &str) -> String {
        let response = send_raw(c, "GET", &format!("/api/v1/claude-link/callback?{query}"), &[], None).await;
        assert_eq!(response.status(), StatusCode::FOUND);
        response.headers()["location"].to_str().unwrap().to_string()
    }

    fn enc(v: &str) -> String {
        urlencoding::encode(v).into_owned()
    }

    /// Start with `return_to`, then the callback with `code=good`. Returns the state.
    async fn link(c: &Ctx, fake: &Fake, user: &str) -> String {
        let (status, body) = send(c, "POST", "/api/v1/claude-link/start", Some(user), Some(json!({ "return_to": "allternit://connections" }))).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let state = fake.par_state();
        let location = callback_to(c, &format!("code=good&state={}", enc(&state))).await;
        assert_eq!(location, "allternit://connections?connected=claude-org");
        state
    }

    async fn insert_link(c: &Ctx, user: &str, profile: &str, expires_in_days: i64) {
        sqlx::query("INSERT INTO anthropic_link_profiles (user_id, user_profile_id) VALUES ($1, $2)").bind(user).bind(profile).execute(&c.state.db).await.unwrap();
        sqlx::query(
            "INSERT INTO anthropic_connections (user_id, user_profile_id, connection_id, refresh_expires_at)
             VALUES ($1, $2, 'ocon_' || $2, now() + make_interval(days => $3::int))",
        )
        .bind(user)
        .bind(profile)
        .bind(expires_in_days as i32)
        .execute(&c.state.db)
        .await
        .unwrap();
    }

    async fn count(c: &Ctx, sql: &str, user: &str) -> i64 {
        sqlx::query_scalar(sql).bind(user).fetch_one(&c.state.db).await.unwrap()
    }

    #[tokio::test]
    #[serial_test::serial(anthropic_client_env)]
    async fn start_sends_par_with_contract_fields() {
        let (fake, base) = fake_anthropic().await;
        configure(&base);
        let c = ctx().await;
        let u = user();

        let (status, body) = send(&c, "POST", "/api/v1/claude-link/start", Some(&u), Some(json!({ "return_to": "https://evil.example/" }))).await;
        assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("bad_return_to")));

        let (status, body) = send(&c, "POST", "/api/v1/claude-link/start", Some(&u), Some(json!({ "return_to": "allternit://connections" }))).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let login = &body["login"];
        assert_eq!((login["kind"].as_str(), login["poll"].as_bool()), (Some("oauth"), Some(true)));
        assert!(chrono::DateTime::parse_from_rfc3339(login["expires_at"].as_str().unwrap()).is_ok());
        let url = reqwest::Url::parse(login["url"].as_str().unwrap()).unwrap();
        assert_eq!(url.path(), "/oauth/authorize");
        let params: Vec<(String, String)> = url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
        assert_eq!(params, vec![("client_id".into(), "client_test".into()), ("request_uri".into(), PAR_URI.into())], "no other params");

        // The profile: operator token, no passthrough.
        let profiles = fake.requests("POST", "/v1/user_profiles");
        assert_eq!(profiles.len(), 1);
        assert!(profiles[0].auth.as_deref().unwrap().starts_with("Bearer sk-ant-oat01-"));
        assert_eq!(profiles[0].body, json!({}));

        let par = fake.requests("POST", "/v1/oauth/par");
        assert_eq!(par.len(), 1);
        let p = &par[0].body;
        let keys: BTreeSet<&str> = p.as_object().unwrap().keys().map(String::as_str).collect();
        let want: BTreeSet<&str> = [
            "response_type", "client_id", "redirect_uri", "scope", "state", "code_challenge", "code_challenge_method",
            "connection_type", "connection_auth_target", "user_profile_id", "client_assertion_type", "client_assertion",
        ]
        .into_iter()
        .collect();
        assert_eq!(keys, want, "exactly the contract's fields");
        assert_eq!(p["response_type"], "code");
        assert_eq!(p["client_id"], "client_test");
        assert_eq!(p["redirect_uri"], "https://api.allternit.com/api/v1/claude-link/callback");
        assert_eq!(p["scope"], "billing:attribution offline_access");
        assert_eq!(p["code_challenge_method"], "S256");
        assert_eq!(p["connection_type"], "organization_shared");
        assert_eq!(p["connection_auth_target"], "platform");
        assert_eq!(p["user_profile_id"], "uprof_fake_1");
        assert_eq!(p["client_assertion_type"], CLIENT_ASSERTION_TYPE);
        let client = AnthropicClient::from_env().unwrap();
        assert!(es256::verify_with_jwks(p["client_assertion"].as_str().unwrap(), &client.jwks()).is_ok());
        assert!(!par[0].query.contains("assertion"), "never in the query string");

        // The stored request: the verifier sealed, its S256 challenge sent.
        let state = p["state"].as_str().unwrap();
        let (owner, ct, nonce, return_to, used): (String, Vec<u8>, Vec<u8>, Option<String>, bool) = sqlx::query_as(
            "SELECT user_id, code_verifier_ciphertext, code_verifier_nonce, return_to, used_at IS NOT NULL FROM anthropic_link_requests WHERE state = $1",
        )
        .bind(state)
        .fetch_one(&c.state.db)
        .await
        .unwrap();
        assert_eq!((owner.as_str(), return_to.as_deref(), used), (u.as_str(), Some("allternit://connections"), false));
        let verifier = decrypt_parts(&c.cipher, &ct, &nonce).unwrap();
        assert!((43..=128).contains(&verifier.len()), "{}", verifier.len());
        assert!(!String::from_utf8_lossy(&ct).contains(&verifier));
        assert_eq!(p["code_challenge"].as_str(), Some(claude_link::pkce_challenge(&verifier).as_str()));
        clear_env();
    }

    #[tokio::test]
    #[serial_test::serial(anthropic_client_env)]
    async fn start_reuses_existing_profile() {
        let (fake, base) = fake_anthropic().await;
        configure(&base);
        let c = ctx().await;
        let u = user();
        for _ in 0..2 {
            let (status, _) = send(&c, "POST", "/api/v1/claude-link/start", Some(&u), None).await;
            assert_eq!(status, StatusCode::OK);
        }
        assert_eq!(fake.profiles_made.load(Ordering::SeqCst), 1, "made once");
        let par = fake.requests("POST", "/v1/oauth/par");
        assert_eq!((par[0].body["user_profile_id"].as_str(), par[1].body["user_profile_id"].as_str()), (Some("uprof_fake_1"), Some("uprof_fake_1")));
        assert_eq!(count(&c, "SELECT count(*) FROM anthropic_link_profiles WHERE user_id = $1", &u).await, 1);

        // A profile stored earlier (an unlinked account) is reused without a call.
        let other = user();
        sqlx::query("INSERT INTO anthropic_link_profiles (user_id, user_profile_id) VALUES ($1, 'uprof_existing')").bind(&other).execute(&c.state.db).await.unwrap();
        let (status, _) = send(&c, "POST", "/api/v1/claude-link/start", Some(&other), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(fake.profiles_made.load(Ordering::SeqCst), 1);
        assert_eq!(fake.requests("POST", "/v1/oauth/par")[2].body["user_profile_id"], "uprof_existing");
        clear_env();
    }

    #[tokio::test]
    #[serial_test::serial(anthropic_client_env)]
    async fn start_503_when_not_configured() {
        let (fake, base) = fake_anthropic().await;
        clear_env();
        let c = ctx().await;
        let u = user();
        for (method, uri) in [("POST", "/api/v1/claude-link/start"), ("GET", "/api/v1/claude-link"), ("PATCH", "/api/v1/claude-link"), ("DELETE", "/api/v1/claude-link")] {
            let (status, body) = send(&c, method, uri, Some(&u), None).await;
            assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::SERVICE_UNAVAILABLE, Some("not_available_yet")), "{method} {uri}");
        }
        assert_eq!(callback_to(&c, "code=x&state=y").await, "https://ai.allternit.com/connections?error=not_configured");

        // Every variable set but no credential cipher: still off.
        configure(&base);
        let mut bare = Arc::try_unwrap(test_state(Arc::new(MockGateway::new(None, vec![]))).await).ok().unwrap();
        bare.credential_cipher = None;
        let no_cipher = Ctx { state: Arc::new(bare), cipher: c.cipher.clone() };
        let (status, _) = send(&no_cipher, "POST", "/api/v1/claude-link/start", Some(&u), None).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(fake.seen.lock().unwrap().is_empty(), "nothing reaches Anthropic");
        clear_env();
    }

    #[tokio::test]
    #[serial_test::serial(anthropic_client_env)]
    async fn start_refuses_platform_key() {
        use crate::services::api_keys::{create_api_key, CreateApiKeyInput};
        let (fake, base) = fake_anthropic().await;
        configure(&base);
        let c = ctx().await;
        crate::routes::test_support::events_backbone_schema(&c.state.db).await;
        let u = user();
        let key = create_api_key(&c.state.db, CreateApiKeyInput { user_id: u.clone(), organization_id: None, name: "ci".into(), scopes: vec!["inference".into()] })
            .await
            .unwrap()
            .token;
        let oat = format!("alt_oat_{}", "a".repeat(64));
        for token in [key.as_str(), oat.as_str()] {
            let auth = format!("Bearer {token}");
            for (method, uri) in [("POST", "/api/v1/claude-link/start"), ("GET", "/api/v1/claude-link"), ("DELETE", "/api/v1/claude-link")] {
                let response = send_raw(&c, method, uri, &[("authorization", auth.as_str())], None).await;
                assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {uri} with {}", &token[..8]);
            }
        }
        let response = send_raw(&c, "POST", "/api/v1/claude-link/start", &[], None).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(fake.requests("POST", "/v1/oauth/par").is_empty());
        assert_eq!(count(&c, "SELECT count(*) FROM anthropic_link_requests WHERE user_id = $1", &u).await, 0);
        clear_env();
    }

    #[tokio::test]
    #[serial_test::serial(anthropic_client_env)]
    async fn callback_state_mismatch_bad_state() {
        let (fake, base) = fake_anthropic().await;
        configure(&base);
        let c = ctx().await;
        let u = user();
        send(&c, "POST", "/api/v1/claude-link/start", Some(&u), Some(json!({ "return_to": "allternit://connections" }))).await;
        let real = fake.par_state();
        assert_eq!(callback_to(&c, "code=good&state=not-the-state").await, "https://ai.allternit.com/connections?error=bad_state");
        assert_eq!(callback_to(&c, "code=good").await, "https://ai.allternit.com/connections?error=bad_state");
        assert!(fake.code_exchanges().is_empty(), "a bad state never reaches the token endpoint");
        let used: bool = sqlx::query_scalar("SELECT used_at IS NOT NULL FROM anthropic_link_requests WHERE state = $1").bind(&real).fetch_one(&c.state.db).await.unwrap();
        assert!(!used, "the real request is untouched");
        clear_env();
    }

    #[tokio::test]
    #[serial_test::serial(anthropic_client_env)]
    async fn callback_reused_state_rejected() {
        let (fake, base) = fake_anthropic().await;
        configure(&base);
        let c = ctx().await;
        let u = user();
        let state = link(&c, &fake, &u).await;
        assert_eq!(callback_to(&c, &format!("code=good&state={}", enc(&state))).await, "allternit://connections?error=bad_state");
        assert_eq!(fake.code_exchanges().len(), 1, "one exchange");

        // An expired request is refused the same way.
        send(&c, "POST", "/api/v1/claude-link/start", Some(&u), None).await;
        let stale = fake.par_state();
        sqlx::query("UPDATE anthropic_link_requests SET expires_at = now() - interval '1 minute' WHERE state = $1").bind(&stale).execute(&c.state.db).await.unwrap();
        assert_eq!(callback_to(&c, &format!("code=good&state={}", enc(&stale))).await, "https://ai.allternit.com/connections?error=bad_state");
        assert_eq!(fake.code_exchanges().len(), 1);
        clear_env();
    }

    #[tokio::test]
    #[serial_test::serial(anthropic_client_env)]
    async fn callback_access_denied_maps_denied() {
        let (fake, base) = fake_anthropic().await;
        configure(&base);
        let c = ctx().await;
        let u = user();
        send(&c, "POST", "/api/v1/claude-link/start", Some(&u), Some(json!({ "return_to": "https://m.allternit.com/connections" }))).await;
        let state = fake.par_state();
        assert_eq!(
            callback_to(&c, &format!("error=access_denied&state={}", enc(&state))).await,
            "https://m.allternit.com/connections?error=denied"
        );
        assert!(fake.code_exchanges().is_empty());
        // The state is spent.
        assert_eq!(callback_to(&c, &format!("code=good&state={}", enc(&state))).await, "https://m.allternit.com/connections?error=bad_state");
        assert_eq!(count(&c, "SELECT count(*) FROM anthropic_connections WHERE user_id = $1", &u).await, 0);
        clear_env();
    }

    #[tokio::test]
    #[serial_test::serial(anthropic_client_env)]
    async fn callback_user_profile_already_linked() {
        let (fake, base) = fake_anthropic().await;
        configure(&base);
        let c = ctx().await;
        let u = user();
        // From auth.claude.com's redirect.
        send(&c, "POST", "/api/v1/claude-link/start", Some(&u), None).await;
        let state = fake.par_state();
        assert_eq!(
            callback_to(&c, &format!("error=user_profile_already_linked&state={}", enc(&state))).await,
            "https://ai.allternit.com/connections?error=user_profile_already_linked"
        );
        // From the code exchange.
        *fake.token_reply.lock().unwrap() = Some((400, json!({ "error": "user_profile_already_linked" })));
        send(&c, "POST", "/api/v1/claude-link/start", Some(&u), None).await;
        let state = fake.par_state();
        assert_eq!(
            callback_to(&c, &format!("code=good&state={}", enc(&state))).await,
            "https://ai.allternit.com/connections?error=user_profile_already_linked"
        );
        assert_eq!(count(&c, "SELECT count(*) FROM anthropic_connections WHERE user_id = $1", &u).await, 0);
        clear_env();
    }

    #[tokio::test]
    #[serial_test::serial(anthropic_client_env)]
    async fn callback_unauthorized_client_maps_not_configured() {
        let (fake, base) = fake_anthropic().await;
        configure(&base);
        let c = ctx().await;
        let u = user();
        *fake.token_reply.lock().unwrap() = Some((400, json!({ "error": "unauthorized_client", "error_description": "no default workspace" })));
        send(&c, "POST", "/api/v1/claude-link/start", Some(&u), None).await;
        let state = fake.par_state();
        assert_eq!(callback_to(&c, &format!("code=good&state={}", enc(&state))).await, "https://ai.allternit.com/connections?error=not_configured");
        assert_eq!(fake.code_exchanges().len(), 1);
        assert_eq!(count(&c, "SELECT count(*) FROM anthropic_connections WHERE user_id = $1", &u).await, 0);

        // Any other refusal is exchange_failed.
        *fake.token_reply.lock().unwrap() = Some((400, json!({ "error": "invalid_grant" })));
        send(&c, "POST", "/api/v1/claude-link/start", Some(&u), None).await;
        let state = fake.par_state();
        assert_eq!(callback_to(&c, &format!("code=good&state={}", enc(&state))).await, "https://ai.allternit.com/connections?error=exchange_failed");
        clear_env();
    }

    #[tokio::test]
    #[serial_test::serial(anthropic_client_env)]
    async fn callback_success_stores_sealed_refresh_no_access_token() {
        let (fake, base) = fake_anthropic().await;
        configure(&base);
        let c = ctx().await;
        let u = user();
        let state = link(&c, &fake, &u).await;

        let exchanges = fake.code_exchanges();
        assert_eq!(exchanges.len(), 1);
        let e = &exchanges[0].body;
        assert_eq!((e["code"].as_str(), e["redirect_uri"].as_str()), (Some("good"), Some("https://api.allternit.com/api/v1/claude-link/callback")));
        assert_eq!(e["client_id"], "client_test");
        assert_eq!(e["client_assertion_type"], CLIENT_ASSERTION_TYPE);
        assert!(e["client_assertion"].as_str().is_some_and(|a| !a.is_empty()));
        let par = fake.requests("POST", "/v1/oauth/par");
        assert_eq!(par[0].body["state"].as_str(), Some(state.as_str()));
        assert_eq!(
            par[0].body["code_challenge"].as_str(),
            Some(claude_link::pkce_challenge(e["code_verifier"].as_str().unwrap()).as_str()),
            "the verifier matches the challenge sent in the PAR"
        );

        let (connection_id, profile, status, ct, nonce, year_out): (String, String, String, Vec<u8>, Vec<u8>, bool) = sqlx::query_as(
            "SELECT connection_id, user_profile_id, status, refresh_token_ciphertext, refresh_token_nonce,
                    refresh_expires_at BETWEEN now() + interval '364 days' AND now() + interval '366 days'
             FROM anthropic_connections WHERE user_id = $1",
        )
        .bind(&u)
        .fetch_one(&c.state.db)
        .await
        .unwrap();
        assert_eq!((connection_id.as_str(), profile.as_str(), status.as_str(), year_out), ("ocon_fake", "uprof_fake_1", "linked", true));
        assert_eq!(decrypt_parts(&c.cipher, &ct, &nonce).unwrap(), REFRESH);
        let stored: String = sqlx::query_scalar("SELECT row_to_json(c)::text FROM anthropic_connections c WHERE user_id = $1").bind(&u).fetch_one(&c.state.db).await.unwrap();
        assert!(!stored.contains("SECRET-ACCESS"), "the access token is never stored");
        assert!(!stored.contains("SECRET-REFRESH"), "the refresh token is stored sealed");
        assert!(!String::from_utf8_lossy(&ct).contains("SECRET"));

        // Step 10 reads it by user id.
        let linked = claude_link::linked_profile(&c.state.db, &u).await.unwrap().unwrap();
        assert_eq!((linked.user_profile_id.as_str(), linked.state), ("uprof_fake_1", claude_link::LinkState::Linked));
        clear_env();
    }

    #[tokio::test]
    #[serial_test::serial(anthropic_client_env)]
    async fn status_maps_connected_disconnected_not_connected() {
        let (fake, base) = fake_anthropic().await;
        configure(&base);
        let c = ctx().await;
        let (a, b, n, none) = (user(), user(), user(), user());
        insert_link(&c, &a, "uprof_a", 300).await;
        insert_link(&c, &b, "uprof_b", 300).await;
        insert_link(&c, &n, "uprof_n", 300).await;
        {
            let mut replies = fake.status_replies.lock().unwrap();
            replies.insert(
                "uprof_a".into(),
                json!({ "id": "uprof_a", "connections": [{ "oauth_app_id": "oapp_test", "status": "connected",
                        "organization": { "name": "Acme Corp" }, "spend_limit": { "reached": false } }] }),
            );
            replies.insert("uprof_b".into(), json!({ "id": "uprof_b", "status": "disconnected", "last_revoked_reason": { "type": "user_revoked" } }));
            replies.insert("uprof_n".into(), json!({ "id": "uprof_n", "status": "not_connected" }));
        }

        let (status, body) = send(&c, "GET", "/api/v1/claude-link", Some(&a), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!((body["state"].as_str(), body["org_name"].as_str()), (Some("linked"), Some("Acme Corp")));
        // Anthropic states no spend limit: a remote field is ignored; only our
        // own last linked request sets it (none yet here).
        assert!(body["spend_limit"].is_null(), "{body}");
        assert!(body["linked_at"].is_string() && body["relink_by"].is_string(), "{body}");
        let reads = fake.requests("GET", "/v1/user_profiles/uprof_a");
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[0].beta.as_deref(), Some("user-profiles-search-2026-08-30"));
        // Anthropic's example passes the OAuth client id as `oauth_app_id`.
        assert_eq!(reads[0].query, "oauth_app_id=client_test");
        assert!(reads[0].auth.as_deref().unwrap().starts_with("Bearer sk-ant-oat01-"));
        // Cached for 60 s.
        send(&c, "GET", "/api/v1/claude-link", Some(&a), None).await;
        assert_eq!(fake.requests("GET", "/v1/user_profiles/uprof_a").len(), 1);

        let (_, body) = send(&c, "GET", "/api/v1/claude-link", Some(&b), None).await;
        assert_eq!(body["state"], "disconnected", "{body}");
        assert!(body["relink_by"].is_null());
        let (st, reason): (String, Option<String>) =
            sqlx::query_as("SELECT status, last_error FROM anthropic_connections WHERE user_id = $1").bind(&b).fetch_one(&c.state.db).await.unwrap();
        assert_eq!((st.as_str(), reason.as_deref()), ("disconnected", Some("user_revoked")));

        let (_, body) = send(&c, "GET", "/api/v1/claude-link", Some(&n), None).await;
        assert_eq!(body, json!({ "state": "unlinked", "org_name": null, "linked_at": null, "relink_by": null, "spend_limit": null }));
        assert!(claude_link::linked_profile(&c.state.db, &n).await.unwrap().is_none(), "billing sees it unlinked too");

        let (_, body) = send(&c, "GET", "/api/v1/claude-link", Some(&none), None).await;
        assert_eq!(body["state"], "unlinked");
        let reads = fake.seen.lock().unwrap().iter().filter(|s| s.method == "GET" && s.path.starts_with("/v1/user_profiles/")).count();
        assert_eq!(reads, 3, "no link, no remote read");
        let org: Option<String> = sqlx::query_scalar("SELECT org_name FROM anthropic_connections WHERE user_id = $1").bind(&a).fetch_one(&c.state.db).await.unwrap();
        assert_eq!(org.as_deref(), Some("Acme Corp"));
        clear_env();
    }

    #[tokio::test]
    #[serial_test::serial(anthropic_client_env)]
    async fn status_relink_needed_14_days() {
        let (fake, base) = fake_anthropic().await;
        configure(&base);
        let c = ctx().await;
        let (soon, later) = (user(), user());
        insert_link(&c, &soon, "uprof_soon", 13).await;
        insert_link(&c, &later, "uprof_later", 15).await;
        for id in ["uprof_soon", "uprof_later"] {
            fake.status_replies.lock().unwrap().insert(id.into(), json!({ "id": id, "status": "connected" }));
        }
        let (_, body) = send(&c, "GET", "/api/v1/claude-link", Some(&soon), None).await;
        assert_eq!(body["state"], "relink_needed", "{body}");
        assert!(body["org_name"].is_null(), "Anthropic named no organization");
        let relink_by = chrono::DateTime::parse_from_rfc3339(body["relink_by"].as_str().unwrap()).unwrap();
        let days = (relink_by.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_hours();
        assert!((12 * 24..=13 * 24).contains(&days), "relink_by is the refresh expiry: {days} h");
        let st: String = sqlx::query_scalar("SELECT status FROM anthropic_connections WHERE user_id = $1").bind(&soon).fetch_one(&c.state.db).await.unwrap();
        assert_eq!(st, "relink_needed");
        assert_eq!(claude_link::linked_profile(&c.state.db, &soon).await.unwrap().unwrap().state, claude_link::LinkState::RelinkNeeded);

        let (_, body) = send(&c, "GET", "/api/v1/claude-link", Some(&later), None).await;
        assert_eq!(body["state"], "linked", "{body}");
        clear_env();
    }

    #[tokio::test]
    #[serial_test::serial(anthropic_client_env)]
    async fn delete_revokes_and_clears() {
        let (fake, base) = fake_anthropic().await;
        configure(&base);
        let c = ctx().await;
        let u = user();
        link(&c, &fake, &u).await;

        let (status, body) = send(&c, "PATCH", "/api/v1/claude-link", Some(&u), Some(json!({ "anything": 1 }))).await;
        assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("no_fields")));

        let (status, _) = send(&c, "DELETE", "/api/v1/claude-link", Some(&u), None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let revokes = fake.requests("POST", "/v1/oauth/revoke");
        assert_eq!(revokes.len(), 1);
        let r = &revokes[0].body;
        assert_eq!(r["connection_id"], "ocon_fake");
        assert!(r.get("token").is_none(), "revoke by connection, not token");
        assert_eq!((r["client_id"].as_str(), r["client_assertion_type"].as_str()), (Some("client_test"), Some(CLIENT_ASSERTION_TYPE)));
        assert_eq!(count(&c, "SELECT count(*) FROM anthropic_connections WHERE user_id = $1", &u).await, 0, "sealed token gone");
        assert_eq!(count(&c, "SELECT count(*) FROM anthropic_link_profiles WHERE user_id = $1", &u).await, 1, "profile kept");
        let (_, body) = send(&c, "GET", "/api/v1/claude-link", Some(&u), None).await;
        assert_eq!(body["state"], "unlinked");

        // A failed remote revoke still unlinks; a re-link reuses the profile.
        link(&c, &fake, &u).await;
        fake.revoke_status.store(500, Ordering::SeqCst);
        let (status, _) = send(&c, "DELETE", "/api/v1/claude-link", Some(&u), None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(count(&c, "SELECT count(*) FROM anthropic_connections WHERE user_id = $1", &u).await, 0);
        assert_eq!(fake.profiles_made.load(Ordering::SeqCst), 1);
        clear_env();
    }

    #[tokio::test]
    #[serial_test::serial(anthropic_client_env)]
    async fn erase_revokes_remote_best_effort() {
        let (fake, base) = fake_anthropic().await;
        configure(&base);
        let c = ctx().await;
        let (u, down) = (user(), user());
        insert_link(&c, &u, "uprof_gone", 300).await;
        insert_link(&c, &down, "uprof_down", 300).await;
        // What account_erase::REVOKE leaves before release_computers runs.
        for id in [&u, &down] {
            sqlx::query("UPDATE anthropic_connections SET status = 'disconnected', refresh_token_ciphertext = NULL, refresh_token_nonce = NULL WHERE user_id = $1")
                .bind(id)
                .execute(&c.state.db)
                .await
                .unwrap();
        }
        claude_link::revoke_for_erase(&c.state, &u).await;
        let revokes = fake.requests("POST", "/v1/oauth/revoke");
        assert_eq!(revokes.len(), 1);
        assert_eq!(revokes[0].body["connection_id"], "ocon_uprof_gone");
        assert!(revokes[0].body["client_assertion"].is_string());

        // Anthropic refusing does not fail the erase; the row is left for ERASE.
        fake.revoke_status.store(503, Ordering::SeqCst);
        claude_link::revoke_for_erase(&c.state, &down).await;
        assert_eq!(fake.requests("POST", "/v1/oauth/revoke").len(), 2);
        assert_eq!(count(&c, "SELECT count(*) FROM anthropic_connections WHERE user_id = $1", &down).await, 1);

        // Feature off: no call at all.
        clear_env();
        claude_link::revoke_for_erase(&c.state, &u).await;
        assert_eq!(fake.requests("POST", "/v1/oauth/revoke").len(), 2);
    }
}
