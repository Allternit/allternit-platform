//! "Sign in with Allternit" access tokens (`alt_oat_…`) as API callers
//! (CONTRACT-oauth-linked-billing-2026-11.md, Part A: A6 scopes, A7 test users,
//! A9 errors, the A1 "CORS:" paragraph). Real Postgres (schema-per-test) with
//! the Platform API migrations and 150 applied; every assertion is on an HTTP response.
//!
//! The chat route is the real `auth_middleware` in front of a handler that
//! echoes what it was given (the `AuthContext` and the `OAuthCaller`), so the
//! test pins who the request acts for and who pays without calling a model.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::Extension,
    http::{HeaderMap, Request, StatusCode},
    routing::post,
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

use super::{
    projects::{self, Principal},
    registered_routes, router_gated, Gate, OAuthCaller, ProjectEnv,
};
use crate::{
    auth::middleware::{auth_middleware, AuthContext, OAUTH_INFERENCE_PERMISSION},
    routes::test_support::{test_state, MockGateway},
    services::{
        api_keys::hash_token,
        oauth_apps::{self, AuditCtx, ClientType, CreateOAuthAppInput},
    },
    ApiState,
};

const OWNER: &str = "user_app_owner";
const PERSON: &str = "user_signed_in";

struct Ctx {
    state: Arc<ApiState>,
    app_id: Uuid,
}

async fn ctx() -> Ctx {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    for sql in [
        include_str!("../../../migrations_pg/003_api_keys.sql"),
        include_str!("../../../migrations_pg/050_platform_api_foundation.sql"),
        include_str!("../../../migrations_pg/080_platform_billing.sql"),
        include_str!("../../../migrations_pg/084_platform_payment_methods.sql"),
        include_str!("../../../migrations_pg/126_key_governance.sql"),
    ] {
        sqlx::raw_sql(&sql.replace("public.", "")).execute(&state.db).await.expect("migration applies");
    }
    sqlx::raw_sql("CREATE TABLE IF NOT EXISTS users (id TEXT PRIMARY KEY, email TEXT);")
        .execute(&state.db)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../../../migrations_pg/150_oauth_apps.sql")).execute(&state.db).await.unwrap();
    for (id, email) in [(OWNER, "owner@example.com"), (PERSON, "person@example.com")] {
        sqlx::query("INSERT INTO users (id, email) VALUES ($1, $2)").bind(id).bind(email).execute(&state.db).await.unwrap();
    }
    let owner = Principal { user_id: OWNER.to_string(), org_id: None, org_admin: false };
    let project = projects::create_project(&state.db, &owner, "Notes", ProjectEnv::Sandbox).await.unwrap();
    let app = oauth_apps::create_app(
        &state.db,
        CreateOAuthAppInput {
            project_id: project.id.clone(),
            owner_user_id: OWNER.to_string(),
            name: "Notes Sync".to_string(),
            client_type: ClientType::Public,
            logo_url: None,
            homepage_url: Some("https://notes.example.com".to_string()),
            privacy_url: None,
            scopes: ["inference", "usage", "profile", "offline_access"].iter().map(|s| s.to_string()).collect(),
            redirect_uris: vec!["https://notes.example.com/cb".to_string()],
        },
        &AuditCtx { actor: OWNER.to_string(), ..Default::default() },
    )
    .await
    .unwrap();
    // The person is a listed test user of the (testing-state) app.
    oauth_apps::add_test_user(&state.db, &project.id, app.app.id, "person@example.com", &AuditCtx {
        actor: OWNER.to_string(),
        ..Default::default()
    })
    .await
    .unwrap();
    Ctx { state, app_id: app.app.id }
}

/// A grant for `user` with `scopes` and one access token living `ttl_secs`
/// (negative = already expired). Returns `(grant_id, token)`.
async fn grant(c: &Ctx, user: &str, scopes: &[&str], ttl_secs: i64) -> (Uuid, String) {
    let grant_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO oauth_grants (id, app_id, user_id, scopes, monthly_cap_cents, sub)
         VALUES ($1, $2, $3, $4, 1000, $5)",
    )
    .bind(grant_id)
    .bind(c.app_id)
    .bind(user)
    .bind(scopes.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    .bind(format!("usr_{}", Uuid::new_v4().simple()))
    .execute(&c.state.db)
    .await
    .unwrap();
    let token = format!("alt_oat_{}", hex::encode(rand::random::<[u8; 32]>()));
    sqlx::query(
        "INSERT INTO oauth_tokens (id, grant_id, kind, token_hash, prefix, expires_at)
         VALUES ($1, $2, 'access', $3, $4, now() + make_interval(secs => $5))",
    )
    .bind(Uuid::new_v4())
    .bind(grant_id)
    .bind(hash_token(&token))
    .bind(&token[..16])
    .bind(ttl_secs as f64)
    .execute(&c.state.db)
    .await
    .unwrap();
    (grant_id, token)
}

/// The real `auth_middleware` in front of an echo of what the chat route gets.
fn chat_app(state: &Arc<ApiState>) -> Router {
    let echo = |Extension(auth): Extension<AuthContext>, oauth: Option<Extension<OAuthCaller>>| async move {
        axum::Json(json!({
            "user_id": auth.user.user_id,
            "token_id": auth.user.token_id,
            "permissions": auth.user.permissions,
            "oauth": oauth.map(|Extension(o)| json!({
                "user_id": o.user_id,
                "grant_id": o.grant_id,
                "app_id": o.app_id,
                "scopes": o.scopes,
                "billing": o.billing,
            })),
        }))
    };
    Router::new()
        .route("/v1/chat/completions", post(echo))
        .layer(axum::middleware::from_fn_with_state(state.clone(), auth_middleware))
        .with_state(state.clone())
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

async fn send(app: Router, request: Request<Body>) -> Reply {
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
    Reply { status, headers, body: serde_json::from_slice(&bytes).unwrap_or(Value::Null) }
}

fn bearer(method: &str, path: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(json!({ "model": "kimi-k3", "messages": [] }).to_string()))
        .unwrap()
}

async fn chat(c: &Ctx, token: &str) -> Reply {
    send(chat_app(&c.state), bearer("POST", "/v1/chat/completions", token)).await
}

fn assert_error(reply: &Reply, status: StatusCode, code: &str) {
    assert_eq!(reply.status, status, "{}", reply.body);
    assert_eq!(reply.body["error"]["code"], code, "{}", reply.body);
}

#[tokio::test]
async fn oat_inference_call_authenticates_as_user_wallet() {
    let c = ctx().await;
    let (grant_id, token) = grant(&c, PERSON, &["inference", "usage"], 3600).await;
    let r = chat(&c, &token).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    // Acts for the person who consented, never the app owner.
    assert_eq!(r.body["user_id"], PERSON);
    assert!(r.body["token_id"].as_str().unwrap().starts_with("oauth:"), "{}", r.body);
    // Not `inference`: the model route must take the OAuth billing path.
    assert_eq!(r.body["permissions"], json!([OAUTH_INFERENCE_PERMISSION]));
    assert_eq!(r.body["oauth"]["user_id"], PERSON);
    assert_eq!(r.body["oauth"]["grant_id"], grant_id.to_string());
    assert_eq!(r.body["oauth"]["app_id"], c.app_id.to_string());
    // `usage` maps to no platform scope.
    assert_eq!(r.body["oauth"]["scopes"], json!(["inference"]));
    assert_eq!(
        r.body["oauth"]["billing"],
        json!({ "kind": "user_wallet", "grant_id": grant_id.to_string(), "cap_cents": 1000 })
    );
    assert_eq!(r.headers.get("x-ratelimit-limit").unwrap(), "60");
}

#[tokio::test]
async fn oat_revoked_401() {
    let c = ctx().await;
    let (grant_id, token) = grant(&c, PERSON, &["inference"], 3600).await;
    assert_eq!(chat(&c, &token).await.status, StatusCode::OK);
    sqlx::query("UPDATE oauth_grants SET revoked_at = now() WHERE id = $1")
        .bind(grant_id)
        .execute(&c.state.db)
        .await
        .unwrap();
    assert_error(&chat(&c, &token).await, StatusCode::UNAUTHORIZED, "oauth_grant_revoked");
    // The Platform routes check the token before the scope.
    let platform = router_gated(&c.state, Gate::Forced(true)).with_state(c.state.clone());
    assert_error(&send(platform, bearer("GET", "/v1/accounts", &token)).await, StatusCode::UNAUTHORIZED, "oauth_grant_revoked");
    // A suspended app blocks a token whose grant is still live.
    let (_, other) = grant(&c, OWNER, &["inference"], 3600).await;
    sqlx::query("UPDATE oauth_apps SET state = 'suspended' WHERE id = $1").bind(c.app_id).execute(&c.state.db).await.unwrap();
    assert_error(&chat(&c, &other).await, StatusCode::UNAUTHORIZED, "oauth_grant_revoked");
}

#[tokio::test]
async fn oat_expired_401() {
    let c = ctx().await;
    let (_, token) = grant(&c, PERSON, &["inference"], -5).await;
    assert_error(&chat(&c, &token).await, StatusCode::UNAUTHORIZED, "oauth_token_expired");
    // An unknown token of the right shape.
    let unknown = format!("alt_oat_{}", "0".repeat(64));
    assert_error(&chat(&c, &unknown).await, StatusCode::UNAUTHORIZED, "invalid_token");
}

#[tokio::test]
async fn non_test_user_403_when_testing() {
    let c = ctx().await;
    // A grant made while the person was listed; then the owner removes them.
    let (_, token) = grant(&c, PERSON, &["inference"], 3600).await;
    sqlx::query("DELETE FROM oauth_app_test_users WHERE app_id = $1").bind(c.app_id).execute(&c.state.db).await.unwrap();
    assert_error(&chat(&c, &token).await, StatusCode::FORBIDDEN, "app_not_approved_for_user");
    // The owner is always allowed while testing.
    let (_, owner_token) = grant(&c, OWNER, &["inference"], 3600).await;
    assert_eq!(chat(&c, &owner_token).await.status, StatusCode::OK);
    // Once the app is live, anyone with a grant may call.
    sqlx::query("UPDATE oauth_apps SET state = 'live' WHERE id = $1").bind(c.app_id).execute(&c.state.db).await.unwrap();
    assert_eq!(chat(&c, &token).await.status, StatusCode::OK);
}

#[tokio::test]
async fn oat_without_inference_scope_403() {
    let c = ctx().await;
    let (_, token) = grant(&c, PERSON, &["usage", "profile", "offline_access"], 3600).await;
    assert_error(&chat(&c, &token).await, StatusCode::FORBIDDEN, "insufficient_scope");
}

#[tokio::test]
async fn oat_on_project_route_403_insufficient_scope() {
    let c = ctx().await;
    let (_, token) = grant(&c, PERSON, &["inference", "usage", "profile"], 3600).await;
    let platform = router_gated(&c.state, Gate::Forced(true)).with_state(c.state.clone());
    // Every `/v1` Platform route needs a project: none answers an OAuth token,
    // including `/v1/agents` and `/v1/decisions` (no Agency API forward) and
    // the inference-scoped `/v1/responses` and `/v1/messages`.
    let routes = registered_routes();
    assert!(routes.len() > 50, "{}", routes.len());
    for (method, path) in routes {
        let path = path
            .split('/')
            .map(|seg| if seg.starts_with('{') { "x_1" } else { seg })
            .collect::<Vec<_>>()
            .join("/");
        let r = send(platform.clone(), bearer(method, &path, &token)).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{method} {path}: {}", r.body);
        assert_eq!(r.body["error"]["code"], "insufficient_scope", "{method} {path}: {}", r.body);
    }
    // The console header does not turn a token into a session.
    let mut request = bearer("GET", "/v1/accounts", &token);
    request.headers_mut().insert(super::CONSOLE_PROJECT_HEADER, "proj_x".parse().unwrap());
    assert_error(&send(platform, request).await, StatusCode::FORBIDDEN, "insufficient_scope");
}

#[tokio::test]
async fn oat_not_accepted_as_account_token() {
    let c = ctx().await;
    let (_, token) = grant(&c, PERSON, &["inference", "usage", "profile", "offline_access"], 3600).await;
    let app = crate::create_router(c.state.clone());
    // Account routes behind `auth_middleware`: the token is not a credential there.
    for path in ["/api/v1/auth/me", "/api/v1/auth/tokens"] {
        let r = send(app.clone(), bearer("GET", path, &token)).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{path}: {}", r.body);
    }
    // Management routes that resolve the caller themselves.
    let mut headers = HeaderMap::new();
    headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
    assert!(crate::auth::resolve::resolve_user_id(&c.state.db, &headers).await.is_err());
}

#[tokio::test]
async fn oat_rate_limited_per_grant() {
    let c = ctx().await;
    let (_, token) = grant(&c, PERSON, &["inference"], 3600).await;
    for _ in 0..60 {
        let r = chat(&c, &token).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    }
    let limited = chat(&c, &token).await;
    assert_error(&limited, StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    assert!(limited.headers.get("retry-after").is_some());
    assert_eq!(limited.headers.get("x-ratelimit-remaining").unwrap(), "0");
    // Another grant (another person's sign-in) has its own window.
    let (_, other) = grant(&c, OWNER, &["inference"], 3600).await;
    let ok = chat(&c, &other).await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.body);
    assert_eq!(ok.headers.get("x-ratelimit-remaining").unwrap(), "59");
}

fn preflight(path: &str, origin: &str, method: &str) -> Request<Body> {
    Request::builder()
        .method("OPTIONS")
        .uri(path)
        .header("origin", origin)
        .header("access-control-request-method", method)
        .header("access-control-request-headers", "authorization,content-type")
        .body(Body::empty())
        .unwrap()
}

fn header<'a>(reply: &'a Reply, name: &str) -> Option<&'a str> {
    reply.headers.get(name).map(|v| v.to_str().unwrap())
}

#[tokio::test]
async fn oauth_token_preflight_cors_star() {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    let app = crate::create_router(state);
    for (path, method) in [
        ("/oauth2/token", "POST"),
        ("/oauth2/revoke", "POST"),
        ("/oauth2/userinfo", "GET"),
        ("/.well-known/oauth-authorization-server", "GET"),
    ] {
        // A browser-only app on its own origin, and one of ours: both get `*`.
        for origin in ["https://notes.example.com", "https://ai.allternit.com"] {
            let r = send(app.clone(), preflight(path, origin, method)).await;
            assert!(r.status.is_success(), "{path} {origin}: {}", r.status);
            assert_eq!(header(&r, "access-control-allow-origin"), Some("*"), "{path} {origin}");
            assert_eq!(header(&r, "access-control-allow-credentials"), None, "{path} {origin}");
            let methods = header(&r, "access-control-allow-methods").unwrap().to_ascii_lowercase();
            for m in ["get", "post", "options"] {
                assert!(methods.contains(m), "{path}: {methods}");
            }
            let headers = header(&r, "access-control-allow-headers").unwrap().to_ascii_lowercase();
            assert!(headers.contains("authorization") && headers.contains("content-type"), "{path}: {headers}");
        }
    }
    // The real request (not only the preflight) carries `*` and no credentials,
    // even from an origin the global policy allows with credentials.
    let request = Request::builder()
        .uri("/.well-known/oauth-authorization-server")
        .header("origin", "https://ai.allternit.com")
        .body(Body::empty())
        .unwrap();
    let r = send(app.clone(), request).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(header(&r, "access-control-allow-origin"), Some("*"));
    assert_eq!(header(&r, "access-control-allow-credentials"), None);
    let request = Request::builder()
        .method("POST")
        .uri("/oauth2/token")
        .header("origin", "https://notes.example.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("grant_type=authorization_code"))
        .unwrap();
    let r = send(app, request).await;
    assert_eq!(header(&r, "access-control-allow-origin"), Some("*"), "{} {}", r.status, r.body);
    assert_eq!(header(&r, "access-control-allow-credentials"), None);
}

#[tokio::test]
async fn other_routes_cors_unchanged() {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    let app = crate::create_router(state);
    // Our own web app keeps its origin-specific, credentialed CORS.
    for path in ["/api/v1/api-keys", "/v1/chat/completions", "/oauth2/authorize", "/api/v1/oauth/grants"] {
        let r = send(app.clone(), preflight(path, "https://ai.allternit.com", "POST")).await;
        assert_eq!(header(&r, "access-control-allow-origin"), Some("https://ai.allternit.com"), "{path}");
        assert_eq!(header(&r, "access-control-allow-credentials"), Some("true"), "{path}");
        // Any other origin gets no CORS grant, including on the OAuth consent
        // API and the authorize endpoint (a top-level navigation, not fetch).
        let r = send(app.clone(), preflight(path, "https://notes.example.com", "POST")).await;
        assert_eq!(header(&r, "access-control-allow-origin"), None, "{path}");
    }
    // A path that only starts like a public one is not public.
    let r = send(app, preflight("/.well-known/oauth-authorization-server/mcp", "https://notes.example.com", "GET")).await;
    assert_eq!(header(&r, "access-control-allow-origin"), None);
}
