//! Auth and route behavior of the selfhost router, through real HTTP
//! requests against the real SQLite store.

use std::net::SocketAddr;
use std::sync::Arc;

use allternit_api::selfhost::auth::{session_grant, Internal};
use allternit_api::selfhost::settings::Settings;
use allternit_api::selfhost::store::sqlite::SqliteStore;
use allternit_api::selfhost::store::*;
use allternit_api::selfhost::{router, SelfHostState};
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode};
use axum::routing::get;
use axum::Router;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::Utc;
use serde_json::{json, Value};
use tower::ServiceExt;

fn state(settings: Settings) -> SelfHostState {
    SelfHostState::new(Arc::new(SqliteStore::open_memory().unwrap()), settings)
        .unwrap()
        .with_setup_token(None)
}

async fn mint_key(st: &SelfHostState, role: Role) -> String {
    let secret = new_secret("ask");
    st.store
        .create_api_key(NewApiKey {
            name: role.as_str().into(),
            role,
            prefix: secret[..12].into(),
            key_hash: hash_secret(&secret),
            expires_at: None,
        })
        .await
        .unwrap();
    secret
}

async fn mint_env_key(st: &SelfHostState, env_id: &str) -> String {
    let secret = new_secret("aek");
    st.store
        .create_environment_key(NewEnvironmentKey {
            environment_id: env_id.into(),
            name: None,
            prefix: secret[..12].into(),
            key_hash: hash_secret(&secret),
            expires_at: None,
        })
        .await
        .unwrap();
    secret
}

struct Resp {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

async fn send(app: &Router, req: Request<Body>) -> Resp {
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into()))
    };
    Resp { status, headers, body }
}

async fn call(app: &Router, method: Method, uri: &str, bearer: Option<&str>, body: Option<Value>) -> Resp {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = bearer {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    let req = match body {
        Some(v) => b
            .header("content-type", "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    send(app, req).await
}

fn assert_error(r: &Resp, status: StatusCode, kind: &str, code: &str) {
    assert_eq!(r.status, status, "body: {}", r.body);
    assert_eq!(r.body["error"]["type"], json!(kind), "body: {}", r.body);
    assert_eq!(r.body["error"]["code"], json!(code), "body: {}", r.body);
    assert!(r.body["error"]["message"].is_string());
}

#[tokio::test]
async fn no_key_is_401_and_bad_key_is_401() {
    let st = state(Settings::default());
    let app = router(st.clone());
    let r = call(&app, Method::GET, "/v1/agents", None, None).await;
    assert_error(&r, StatusCode::UNAUTHORIZED, "authentication_error", "unauthorized");
    let r = call(&app, Method::GET, "/v1/agents", Some("ask_not_a_real_key"), None).await;
    assert_error(&r, StatusCode::UNAUTHORIZED, "authentication_error", "unauthorized");
    // Health and server info need no key.
    assert_eq!(call(&app, Method::GET, "/health/live", None, None).await.status, StatusCode::OK);
    assert_eq!(call(&app, Method::GET, "/health/ready", None, None).await.status, StatusCode::OK);
    let info = call(&app, Method::GET, "/v1/server/info", None, None).await;
    assert_eq!(info.status, StatusCode::OK);
    assert_eq!(info.body["provider"], json!("anthropic"));
    assert_eq!(info.body["features"]["postgres"], json!(false));
    assert_eq!(info.body["auth_modes"], json!(["api_key"]));
}

#[tokio::test]
async fn revoked_key_stops_working() {
    let st = state(Settings::default());
    let app = router(st.clone());
    let _admin = mint_key(&st, Role::Admin).await;
    let dev = mint_key(&st, Role::Developer).await;
    assert_eq!(call(&app, Method::GET, "/v1/agents", Some(&dev), None).await.status, StatusCode::OK);
    let key = st.store.find_api_key_by_hash(&hash_secret(&dev)).await.unwrap().unwrap();
    st.store.revoke_api_key(&key.id, Utc::now()).await.unwrap();
    let r = call(&app, Method::GET, "/v1/agents", Some(&dev), None).await;
    assert_error(&r, StatusCode::UNAUTHORIZED, "authentication_error", "unauthorized");
}

#[tokio::test]
async fn environment_key_is_403_on_client_routes() {
    let st = state(Settings::default());
    let app = router(st.clone());
    let e = st
        .store
        .create_environment(NewEnvironment { name: "e".into(), network: "none".into(), ..Default::default() })
        .await
        .unwrap();
    let aek = mint_env_key(&st, &e.id).await;
    let r = call(&app, Method::GET, "/v1/agents", Some(&aek), None).await;
    assert_error(&r, StatusCode::FORBIDDEN, "permission_error", "forbidden");
    let r = call(&app, Method::GET, &format!("/v1/sessions?environment_id={}", e.id), Some(&aek), None).await;
    assert_error(&r, StatusCode::FORBIDDEN, "permission_error", "forbidden");
}

#[tokio::test]
async fn viewer_reads_developer_writes_admin_manages_keys() {
    let st = state(Settings::default());
    let app = router(st.clone());
    let viewer = mint_key(&st, Role::Viewer).await;
    let dev = mint_key(&st, Role::Developer).await;
    let admin = mint_key(&st, Role::Admin).await;
    let agent = json!({ "name": "helper", "model": "claude-sonnet" });

    assert_eq!(call(&app, Method::GET, "/v1/agents", Some(&viewer), None).await.status, StatusCode::OK);
    let r = call(&app, Method::POST, "/v1/agents", Some(&viewer), Some(agent.clone())).await;
    assert_error(&r, StatusCode::FORBIDDEN, "permission_error", "forbidden");

    let created = call(&app, Method::POST, "/v1/agents", Some(&dev), Some(agent)).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    assert_eq!(created.body["object"], json!("agent"));
    assert_eq!(created.body["status"], json!("ready"));
    let id = created.body["id"].as_str().unwrap().to_string();
    let r = call(&app, Method::DELETE, &format!("/v1/agents/{id}"), Some(&viewer), None).await;
    assert_error(&r, StatusCode::FORBIDDEN, "permission_error", "forbidden");

    let r = call(&app, Method::GET, "/v1/api_keys", Some(&dev), None).await;
    assert_error(&r, StatusCode::FORBIDDEN, "permission_error", "forbidden");
    let r = call(&app, Method::POST, "/v1/api_keys", Some(&dev), Some(json!({"name": "x", "role": "viewer"}))).await;
    assert_error(&r, StatusCode::FORBIDDEN, "permission_error", "forbidden");
    let keys = call(&app, Method::GET, "/v1/api_keys", Some(&admin), None).await;
    assert_eq!(keys.status, StatusCode::OK);
    assert_eq!(keys.body["data"].as_array().unwrap().len(), 3);
    assert!(keys.body["data"][0].get("secret").is_none(), "secrets are never listed");
    let made = call(&app, Method::POST, "/v1/api_keys", Some(&admin), Some(json!({"name": "ci", "role": "developer"}))).await;
    assert_eq!(made.status, StatusCode::CREATED);
    assert!(made.body["secret"].as_str().unwrap().starts_with("ask_"));
}

#[tokio::test]
async fn session_secret_only_works_for_its_own_session() {
    let st = state(Settings::default());
    let app = router(st.clone());
    let a = st
        .store
        .create_agent(NewAgent { name: "a".into(), model: "m".into(), ..Default::default() })
        .await
        .unwrap();
    let e = st
        .store
        .create_environment(NewEnvironment { name: "e".into(), network: "none".into(), ..Default::default() })
        .await
        .unwrap();
    let mk = || NewSession { agent_id: a.id.clone(), environment_id: e.id.clone(), ..Default::default() };
    let s1 = st.store.create_session(mk()).await.unwrap();
    let s2 = st.store.create_session(mk()).await.unwrap();
    st.store.enqueue_work(&e.id, &s1.id).await.unwrap();
    st.store.enqueue_work(&e.id, &s2.id).await.unwrap();
    let claim = |w: &str| ClaimRequest {
        environment_id: e.id.clone(),
        worker_id: w.into(),
        reclaim_older_than_ms: None,
        hostname: None,
        version: None,
        now: Utc::now(),
    };
    let c1 = st.store.claim_work(claim("w1")).await.unwrap().unwrap();
    let c2 = st.store.claim_work(claim("w2")).await.unwrap().unwrap();
    assert_eq!(c1.item.session_id, s1.id);

    let mut h = HeaderMap::new();
    h.insert("x-session-secret", HeaderValue::from_str(&c1.session_secret).unwrap());
    let g = session_grant(&st, &h).await.unwrap();
    g.require_session(&s1.id).unwrap();
    g.require_work(&c1.item.id).unwrap();
    let err = g.require_session(&s2.id).unwrap_err();
    assert_eq!(err.status, StatusCode::FORBIDDEN);
    assert!(g.require_work(&c2.item.id).is_err());

    // A secret ends with its lease.
    st.store.release_work(&c1.item.id, c1.item.lease_generation, Utc::now()).await.unwrap();
    let err = session_grant(&st, &h).await.unwrap_err();
    assert_eq!(err.status, StatusCode::UNAUTHORIZED);

    // A session secret is no client credential.
    let mut b = Request::builder().uri("/v1/agents");
    b = b.header("x-session-secret", c2.session_secret.as_str());
    let r = send(&app, b.body(Body::empty()).unwrap()).await;
    assert_error(&r, StatusCode::UNAUTHORIZED, "authentication_error", "unauthorized");
}

#[tokio::test]
async fn internal_token_needs_loopback_and_internal_route() {
    let st = state(Settings::default());
    let token = st.internal_token.to_string();
    let probe = Router::new()
        .route("/internal/v1/ping", get(|_: Internal| async { "ok" }))
        .route("/v1/ping", get(|_: Internal| async { "ok" }))
        .with_state(st.clone());
    let req = |path: &str, ip: [u8; 4], tok: &str| {
        let mut r = Request::builder()
            .uri(path)
            .header("authorization", format!("Bearer {tok}"))
            .body(Body::empty())
            .unwrap();
        r.extensions_mut().insert(ConnectInfo(SocketAddr::from((ip, 40000))));
        r
    };
    assert_eq!(send(&probe, req("/internal/v1/ping", [127, 0, 0, 1], &token)).await.status, StatusCode::OK);
    assert_eq!(send(&probe, req("/internal/v1/ping", [10, 0, 0, 5], &token)).await.status, StatusCode::FORBIDDEN);
    assert_eq!(send(&probe, req("/internal/v1/ping", [127, 0, 0, 1], "wrong")).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(send(&probe, req("/v1/ping", [127, 0, 0, 1], &token)).await.status, StatusCode::FORBIDDEN);
    // And it is no client credential.
    let app = router(st);
    let r = call(&app, Method::GET, "/v1/agents", Some(&token), None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn setup_token_mints_the_first_admin_key_once() {
    let st = state(Settings::default()).with_setup_token(Some("setup-123".into()));
    let app = router(st.clone());
    let body = json!({ "name": "first", "role": "admin" });

    // No token, empty table: 401 that says how to get in.
    let r = call(&app, Method::POST, "/v1/api_keys", None, Some(body.clone())).await;
    assert_error(&r, StatusCode::UNAUTHORIZED, "authentication_error", "unauthorized");
    assert!(r.body["error"]["message"].as_str().unwrap().contains("admin setup"));

    let wrong_role = Request::builder()
        .method(Method::POST)
        .uri("/v1/api_keys")
        .header("content-type", "application/json")
        .header("x-allternit-self-hosted-token", "setup-123")
        .body(Body::from(json!({ "name": "first", "role": "viewer" }).to_string()))
        .unwrap();
    let r = send(&app, wrong_role).await;
    assert_error(&r, StatusCode::BAD_REQUEST, "invalid_request_error", "first_key_must_be_admin");

    let ok = Request::builder()
        .method(Method::POST)
        .uri("/v1/api_keys")
        .header("content-type", "application/json")
        .header("x-allternit-self-hosted-token", "setup-123")
        .body(Body::from(body.to_string()))
        .unwrap();
    let r = send(&app, ok).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let secret = r.body["secret"].as_str().unwrap().to_string();
    assert!(secret.starts_with("ask_"));
    assert_eq!(r.body["role"], json!("admin"));
    assert_eq!(r.body["prefix"], json!(&secret[..12]));

    // The token is spent once a key exists.
    let r = call(&app, Method::POST, "/v1/api_keys", Some("setup-123"), Some(body.clone())).await;
    assert_error(&r, StatusCode::UNAUTHORIZED, "authentication_error", "unauthorized");
    // The minted key works, and the last admin key cannot be revoked.
    let keys = call(&app, Method::GET, "/v1/api_keys", Some(&secret), None).await;
    assert_eq!(keys.status, StatusCode::OK);
    let id = keys.body["data"][0]["id"].as_str().unwrap().to_string();
    let r = call(&app, Method::DELETE, &format!("/v1/api_keys/{id}"), Some(&secret), None).await;
    assert_error(&r, StatusCode::CONFLICT, "conflict_error", "last_admin_key");
}

#[tokio::test]
async fn sessions_flow_until_the_loop_lands() {
    let st = state(Settings::default());
    let app = router(st.clone());
    let dev = mint_key(&st, Role::Developer).await;
    let agent = call(&app, Method::POST, "/v1/agents", Some(&dev), Some(json!({"name": "a", "model": "claude-sonnet"}))).await;
    let env = call(&app, Method::POST, "/v1/environments", Some(&dev), Some(json!({"name": "e"}))).await;
    assert_eq!(env.status, StatusCode::CREATED, "{}", env.body);
    assert_eq!(env.body["network"], json!("none"), "network defaults to none");
    assert_eq!(env.body["queue_depth"], json!(0));
    assert_eq!(env.body["worker_count"], json!(0));
    let sess = call(
        &app,
        Method::POST,
        "/v1/sessions",
        Some(&dev),
        Some(json!({"agent_id": agent.body["id"], "environment_id": env.body["id"], "title": "t"})),
    )
    .await;
    assert_eq!(sess.status, StatusCode::CREATED, "{}", sess.body);
    assert_eq!(sess.body["status"], json!("idle"));
    let sid = sess.body["id"].as_str().unwrap().to_string();

    // The stub runner: 501, and nothing stored.
    let r = call(&app, Method::POST, &format!("/v1/sessions/{sid}/messages"), Some(&dev), Some(json!({"content": "hi"}))).await;
    assert_error(&r, StatusCode::NOT_IMPLEMENTED, "api_error", "not_implemented");
    let events = call(&app, Method::GET, &format!("/v1/sessions/{sid}/events"), Some(&dev), None).await;
    assert_eq!(events.body["data"], json!([]));
    let s = call(&app, Method::GET, &format!("/v1/sessions/{sid}"), Some(&dev), None).await;
    assert_eq!(s.body["status"], json!("idle"));

    // Unknown fields are refused, empty content too.
    let r = call(&app, Method::POST, &format!("/v1/sessions/{sid}/messages"), Some(&dev), Some(json!({"content": ""}))).await;
    assert_error(&r, StatusCode::BAD_REQUEST, "invalid_request_error", "invalid_field");
    let r = call(&app, Method::POST, "/v1/environments", Some(&dev), Some(json!({"name": "e", "surprise": 1}))).await;
    assert_error(&r, StatusCode::BAD_REQUEST, "invalid_request_error", "invalid_body");

    // Agent and environment are in use until the session is archived.
    let aid = agent.body["id"].as_str().unwrap();
    let r = call(&app, Method::DELETE, &format!("/v1/agents/{aid}"), Some(&dev), None).await;
    assert_error(&r, StatusCode::CONFLICT, "conflict_error", "agent_in_use");
    let archived = call(&app, Method::POST, &format!("/v1/sessions/{sid}/archive"), Some(&dev), None).await;
    assert_eq!(archived.body["status"], json!("archived"));
    assert!(archived.body["archived_at"].is_string());
    let r = call(&app, Method::POST, &format!("/v1/sessions/{sid}/messages"), Some(&dev), Some(json!({"content": "hi"}))).await;
    assert_error(&r, StatusCode::CONFLICT, "conflict_error", "session_archived");
    let events = call(&app, Method::GET, &format!("/v1/sessions/{sid}/events?types=status"), Some(&dev), None).await;
    assert_eq!(events.body["data"][0]["data"]["status"], json!("archived"));
    assert_eq!(events.body["data"][0]["seq"], json!(1));
    let listed = call(&app, Method::GET, "/v1/sessions", Some(&dev), None).await;
    assert_eq!(listed.body["data"], json!([]), "archived sessions are hidden by default");
    let r = call(&app, Method::DELETE, &format!("/v1/agents/{aid}"), Some(&dev), None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body, json!({"id": aid, "deleted": true}));
    let r = call(&app, Method::GET, &format!("/v1/agents/{aid}"), Some(&dev), None).await;
    assert_error(&r, StatusCode::NOT_FOUND, "not_found_error", "not_found");
}

#[tokio::test]
async fn unmapped_model_is_422_and_env_lists_only_narrow() {
    let settings = Settings::from_str(
        "inference:\n  provider: bedrock\n  model_map: {claude-sonnet: anthropic.claude-sonnet}\nmcp: {allowed_hostnames: [mcp.corp]}\ngit: {allowed_hostnames: [git.corp]}\n",
    )
    .unwrap();
    let st = state(settings);
    let app = router(st.clone());
    let dev = mint_key(&st, Role::Developer).await;
    let r = call(&app, Method::POST, "/v1/agents", Some(&dev), Some(json!({"name": "a", "model": "gpt-4o"}))).await;
    assert_error(&r, StatusCode::UNPROCESSABLE_ENTITY, "unprocessable_error", "model_not_mapped");
    let ok = call(&app, Method::POST, "/v1/agents", Some(&dev), Some(json!({"name": "a", "model": "claude-sonnet"}))).await;
    assert_eq!(ok.status, StatusCode::CREATED);

    // MCP servers: exact host, https only.
    for url in ["https://evil.mcp.corp/sse", "http://mcp.corp/sse"] {
        let r = call(
            &app,
            Method::POST,
            "/v1/agents",
            Some(&dev),
            Some(json!({"name": "a", "model": "claude-sonnet", "mcp_servers": [{"name": "m", "url": url}]})),
        )
        .await;
        assert_error(&r, StatusCode::FORBIDDEN, "permission_error", "host_not_allowed");
    }
    let r = call(
        &app,
        Method::POST,
        "/v1/agents",
        Some(&dev),
        Some(json!({"name": "a", "model": "claude-sonnet",
                    "mcp_servers": [{"name": "m", "url": "https://mcp.corp/sse", "headers": {"Authorization": "Bearer abc", "X-Ref": "secret://tok"}}]})),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    assert_eq!(r.body["mcp_servers"][0]["headers"]["Authorization"], json!("[redacted]"));
    assert_eq!(r.body["mcp_servers"][0]["headers"]["X-Ref"], json!("secret://tok"));

    // An environment can only narrow the server lists.
    let r = call(&app, Method::POST, "/v1/environments", Some(&dev), Some(json!({"name": "e", "git": {"allowed_hostnames": ["github.com"]}}))).await;
    assert_error(&r, StatusCode::FORBIDDEN, "permission_error", "host_not_allowed");
    let r = call(&app, Method::POST, "/v1/environments", Some(&dev), Some(json!({"name": "e", "git": {"allowed_hostnames": ["git.corp"]}, "mcp": {"allowed_hostnames": ["mcp.corp"]}}))).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
}

fn preflight(origin: &str) -> Request<Body> {
    Request::builder()
        .method(Method::OPTIONS)
        .uri("/v1/agents")
        .header("origin", origin)
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", "authorization,content-type")
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn cors_allows_listed_origins_only() {
    // No origins: no CORS headers at all.
    let app = router(state(Settings::default()));
    let r = send(&app, preflight("https://ai.example.com")).await;
    assert!(r.headers.get("access-control-allow-origin").is_none());

    let settings = Settings::from_str(
        "inference: {provider: anthropic}\nserver: {cors_origins: ['https://ai.example.com']}\n",
    )
    .unwrap();
    let app = router(state(settings));
    let r = send(&app, preflight("https://ai.example.com")).await;
    assert!(r.status.is_success(), "preflight needs no auth: {}", r.status);
    assert_eq!(r.headers["access-control-allow-origin"], "https://ai.example.com");
    assert_eq!(r.headers["access-control-max-age"], "600");
    assert!(r.headers.get("access-control-allow-credentials").is_none());
    let allowed = r.headers["access-control-allow-headers"].to_str().unwrap().to_ascii_lowercase();
    for h in ["authorization", "content-type", "idempotency-key", "last-event-id", "x-session-secret"] {
        assert!(allowed.contains(h), "{h} missing from {allowed}");
    }
    let r = send(&app, preflight("https://evil.example.com")).await;
    assert!(r.headers.get("access-control-allow-origin").is_none());
    // `null` only when listed literally.
    let r = send(&app, preflight("null")).await;
    assert!(r.headers.get("access-control-allow-origin").is_none());
    let with_null = Settings::from_str(
        "inference: {provider: anthropic}\nserver: {cors_origins: ['null']}\n",
    )
    .unwrap();
    let r = send(&router(state(with_null)), preflight("null")).await;
    assert_eq!(r.headers["access-control-allow-origin"], "null");
    // Actual responses carry the exposed header list.
    let settings = Settings::from_str(
        "inference: {provider: anthropic}\nserver: {cors_origins: ['https://ai.example.com']}\n",
    )
    .unwrap();
    let app = router(state(settings));
    let r = send(
        &app,
        Request::builder()
            .uri("/v1/server/info")
            .header("origin", "https://ai.example.com")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(r.headers["access-control-expose-headers"].to_str().unwrap().to_ascii_lowercase(), "last-event-id");
}

// ─── OIDC against a local issuer ────────────────────────────────────────────

mod oidc {
    use super::*;
    use aws_lc_rs::rand::SystemRandom;
    use aws_lc_rs::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};

    struct Issuer {
        url: String,
        key: EcdsaKeyPair,
    }

    async fn issuer() -> Issuer {
        let key = EcdsaKeyPair::generate(&ECDSA_P256_SHA256_FIXED_SIGNING).unwrap();
        let point = key.public_key().as_ref().to_vec();
        let (x, y) = (URL_SAFE_NO_PAD.encode(&point[1..33]), URL_SAFE_NO_PAD.encode(&point[33..65]));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let disco = json!({ "issuer": url, "jwks_uri": format!("{url}/jwks") });
        let jwks = json!({ "keys": [{ "kty": "EC", "crv": "P-256", "kid": "k1", "use": "sig", "x": x, "y": y }] });
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(move || { let d = disco.clone(); async move { axum::Json(d) } }))
            .route("/jwks", get(move || { let j = jwks.clone(); async move { axum::Json(j) } }));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Issuer { url, key }
    }

    fn token(iss: &Issuer, claims: Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(json!({"alg": "ES256", "kid": "k1", "typ": "JWT"}).to_string());
        let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
        let msg = format!("{header}.{payload}");
        let sig = iss.key.sign(&SystemRandom::new(), msg.as_bytes()).unwrap();
        format!("{msg}.{}", URL_SAFE_NO_PAD.encode(sig.as_ref()))
    }

    fn claims(iss: &Issuer, groups: &[&str], aud: &str) -> Value {
        json!({
            "iss": iss.url, "aud": aud, "sub": "user-1",
            "exp": Utc::now().timestamp() + 300, "groups": groups,
        })
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn oidc_roles_audience_and_signature() {
        let iss = issuer().await;
        let settings = Settings::from_str(&format!(
            "inference: {{provider: anthropic}}\nauth:\n  oidc:\n    issuer: '{}'\n    audiences: [agents]\n    role_map: {{agents-admins: admin, agents-viewers: viewer}}\n",
            iss.url
        ))
        .unwrap();
        let st = state(settings);
        let app = router(st.clone());
        let info = call(&app, Method::GET, "/v1/server/info", None, None).await;
        assert_eq!(info.body["auth_modes"], json!(["api_key", "oidc"]));

        let viewer = token(&iss, claims(&iss, &["agents-viewers"], "agents"));
        assert_eq!(call(&app, Method::GET, "/v1/agents", Some(&viewer), None).await.status, StatusCode::OK);
        let r = call(&app, Method::POST, "/v1/agents", Some(&viewer), Some(json!({"name": "a", "model": "m"}))).await;
        assert_error(&r, StatusCode::FORBIDDEN, "permission_error", "forbidden");

        // The highest mapped role wins.
        let admin = token(&iss, claims(&iss, &["agents-viewers", "agents-admins"], "agents"));
        assert_eq!(call(&app, Method::GET, "/v1/api_keys", Some(&admin), None).await.status, StatusCode::OK);

        // No mapped group and default_role none: refused.
        let nobody = token(&iss, claims(&iss, &["staff"], "agents"));
        let r = call(&app, Method::GET, "/v1/agents", Some(&nobody), None).await;
        assert_error(&r, StatusCode::FORBIDDEN, "permission_error", "forbidden");

        let other_aud = token(&iss, claims(&iss, &["agents-admins"], "someone-else"));
        let r = call(&app, Method::GET, "/v1/agents", Some(&other_aud), None).await;
        assert_error(&r, StatusCode::UNAUTHORIZED, "authentication_error", "unauthorized");

        let mut expired_claims = claims(&iss, &["agents-admins"], "agents");
        expired_claims["exp"] = json!(Utc::now().timestamp() - 3600);
        let r = call(&app, Method::GET, "/v1/agents", Some(&token(&iss, expired_claims)), None).await;
        assert_error(&r, StatusCode::UNAUTHORIZED, "authentication_error", "unauthorized");

        // A changed payload breaks the signature.
        let parts: Vec<&str> = admin.split('.').collect();
        let forged_payload = URL_SAFE_NO_PAD.encode(claims(&iss, &["agents-admins"], "agents").to_string().replace("user-1", "user-2"));
        let forged = format!("{}.{}.{}", parts[0], forged_payload, parts[2]);
        let r = call(&app, Method::GET, "/v1/agents", Some(&forged), None).await;
        assert_error(&r, StatusCode::UNAUTHORIZED, "authentication_error", "unauthorized");

        // The issuer is on the egress allowlist; nothing else was contacted.
        let stats = st.egress.guard().stats();
        assert!(stats.keys().all(|h| h == "127.0.0.1"), "{stats:?}");
    }
}
