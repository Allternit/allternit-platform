//! Linked Claude organization billing against a real test database and a fake
//! Anthropic (axum on 127.0.0.1:0, `ALLTERNIT_ANTHROPIC_API_BASE` pointed at
//! it). Each test asserts what the person gets back, the exact headers and
//! body Anthropic received, and the usage and wallet rows.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use axum::{
    extract::{Request, State},
    routing::{get, post},
    Extension, Router,
};
use tower::ServiceExt;

use super::*;
use crate::auth::anthropic_client::{
    API_BASE_ENV, AUTH_BASE_ENV, CLIENT_AUD_ENV, FEDERATION_RULE_ID_ENV, ISSUER_BASE_ENV, LINK_ENV, OAUTH_APP_ID_ENV,
    OAUTH_CLIENT_ID_ENV, ORGANIZATION_ID_ENV, PEM_ENV, REQUIRED_ENV, SERVICE_ACCOUNT_ID_ENV, WIF_AUD_ENV, WORKSPACE_ID_ENV,
};
use crate::auth::{es256, AuthContext, AuthenticatedUser};
use crate::model_router::{catalog::starter_catalog, ModelRouter, ModelRouterError, UpstreamProvider};
use crate::routes::platform_v1::{CallerBilling, OAuthCaller};
use crate::routes::test_support::{test_state, MockGateway};

const MODEL: &str = "linked/claude/claude-sonnet-5-5";
const PERSON: (&str, &str) = ("x-allternit-turn", "person");

// ─── the fake Anthropic ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    query: String,
    headers: Vec<(String, String)>,
    body: Value,
}

impl Seen {
    fn header(&self, name: &str) -> Vec<String> {
        self.headers.iter().filter(|(k, _)| k == name).map(|(_, v)| v.clone()).collect()
    }
    /// Exactly one value of `name`.
    fn one(&self, name: &str) -> String {
        let values = self.header(name);
        assert_eq!(values.len(), 1, "{name}: {values:?}");
        values[0].clone()
    }
}

enum Reply {
    Json(u16, Value, Option<&'static str>),
    Sse(Vec<String>),
}

#[derive(Default)]
struct Fake {
    seen: Mutex<Vec<Seen>>,
    exchanges: AtomicUsize,
    replies: Mutex<VecDeque<Reply>>,
    profiles: Mutex<HashMap<String, Value>>,
}

impl Fake {
    fn requests(&self, method: &str, path: &str) -> Vec<Seen> {
        self.seen.lock().unwrap().iter().filter(|s| s.method == method && s.path == path).cloned().collect()
    }
    fn chats(&self) -> Vec<Seen> {
        self.requests("POST", "/v1/chat/completions")
    }
    fn reply(&self, reply: Reply) {
        self.replies.lock().unwrap().push_back(reply);
    }
}

fn chunk(text: &str) -> String {
    let body = json!({ "id": "c1", "object": "chat.completion.chunk", "created": 1, "model": "claude-sonnet-5-5",
                       "choices": [{ "index": 0, "delta": { "content": text } }] });
    format!("data: {body}\n\n")
}

fn ok_sse() -> Vec<String> {
    let usage = json!({ "id": "c1", "object": "chat.completion.chunk", "created": 1, "model": "claude-sonnet-5-5",
                        "choices": [], "usage": { "prompt_tokens": 11, "completion_tokens": 7 } });
    vec![chunk("Hi"), chunk(" there"), format!("data: {usage}\n\n"), "data: [DONE]\n\n".to_string()]
}

fn ok_json() -> Value {
    json!({ "id": "chatcmpl_fake", "object": "chat.completion", "created": 1, "model": "claude-sonnet-5-5",
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": "Hello from your org" }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 11, "completion_tokens": 7, "total_tokens": 18 } })
}

fn sse(chunks: Vec<String>) -> Response {
    let stream = futures::stream::iter(chunks.into_iter().map(|c| Ok::<_, std::convert::Infallible>(Bytes::from(c))));
    Response::builder().status(200).header("content-type", "text/event-stream").body(Body::from_stream(stream)).unwrap()
}

async fn handle(State(fake): State<Arc<Fake>>, request: Request) -> Response {
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let query = request.uri().query().unwrap_or_default().to_string();
    let headers = request.headers().iter().map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string())).collect();
    let bytes = axum::body::to_bytes(request.into_body(), 1 << 20).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    fake.seen.lock().unwrap().push(Seen { method: method.clone(), path: path.clone(), query, headers, body: body.clone() });
    match (method.as_str(), path.as_str()) {
        ("POST", "/v1/oauth/token") => {
            fake.exchanges.fetch_add(1, Ordering::SeqCst);
            // Unique in the process: the operator token cache outlives a test.
            static MINTED: AtomicUsize = AtomicUsize::new(0);
            let n = MINTED.fetch_add(1, Ordering::SeqCst) + 1;
            Json(json!({ "access_token": format!("sk-ant-oat01-linked-{n}"), "token_type": "bearer", "expires_in": 3600 })).into_response()
        }
        ("POST", "/v1/user_profiles") => Json(json!({ "id": "uprof_made", "type": "user_profile" })).into_response(),
        ("GET", p) if p.starts_with("/v1/user_profiles/") => {
            let id = p.trim_start_matches("/v1/user_profiles/").to_string();
            let reply = fake.profiles.lock().unwrap().get(&id).cloned();
            Json(reply.unwrap_or_else(|| json!({ "id": id, "type": "user_profile", "connection_status": { "type": "not_connected" } })))
                .into_response()
        }
        ("POST", "/v1/chat/completions") => {
            let queued = fake.replies.lock().unwrap().pop_front();
            match queued {
                Some(Reply::Json(status, value, retry_after)) => {
                    let mut response = (StatusCode::from_u16(status).unwrap(), Json(value)).into_response();
                    if let Some(seconds) = retry_after {
                        response.headers_mut().insert("retry-after", HeaderValue::from_static(seconds));
                    }
                    response
                }
                Some(Reply::Sse(chunks)) => sse(chunks),
                None if body["stream"] == true => sse(ok_sse()),
                None => Json(ok_json()).into_response(),
            }
        }
        _ => (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response(),
    }
}

async fn fake_anthropic() -> (Arc<Fake>, String) {
    let fake = Arc::new(Fake::default());
    let app = Router::new().fallback(handle).with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (fake, format!("http://{addr}"))
}

fn clear_env() {
    for name in [LINK_ENV, PEM_ENV, ISSUER_BASE_ENV, API_BASE_ENV, AUTH_BASE_ENV, claude_link::ANTHROPIC_VERSION_ENV].into_iter().chain(REQUIRED_ENV) {
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

// ─── the database and the route ──────────────────────────────────────────────

async fn ctx() -> Arc<ApiState> {
    ctx_with(None).await
}

async fn ctx_with(router: Option<ModelRouter>) -> Arc<ApiState> {
    let base = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    let mut state = Arc::try_unwrap(base).ok().expect("one owner");
    state.credential_cipher = Some(Arc::new(allternit_cloud_core::CredentialCipher::new("test cipher key material")));
    if let Some(router) = router {
        state.model_router = router;
    }
    sqlx::raw_sql("CREATE TABLE IF NOT EXISTS users (id TEXT PRIMARY KEY, email TEXT)").execute(&state.db).await.unwrap();
    for sql in [
        include_str!("../../../migrations_pg/007_inference_usage.sql"),
        include_str!("../../../migrations_pg/009_inference_pools.sql"),
        include_str!("../../../migrations_pg/125_inference_attribution.sql"),
        include_str!("../../../migrations_pg/151_anthropic_links.sql"),
    ] {
        sqlx::raw_sql(&sql.replace("public.", "")).execute(&state.db).await.expect("migration applies");
    }
    // The wallet (migrations_pg/001 + 002), with no CHECK so any debit would show.
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS user_credits (user_id TEXT PRIMARY KEY, balance_usd DOUBLE PRECISION DEFAULT 0,
             created_at TIMESTAMPTZ DEFAULT now(), updated_at TIMESTAMPTZ DEFAULT now());
         CREATE TABLE IF NOT EXISTS credit_transactions (id TEXT PRIMARY KEY, user_id TEXT, amount_usd DOUBLE PRECISION,
             transaction_id TEXT UNIQUE, source TEXT, created_at TIMESTAMPTZ DEFAULT now());",
    )
    .execute(&state.db)
    .await
    .unwrap();
    Arc::new(state)
}

async fn new_user(db: &sqlx::PgPool) -> String {
    let id = format!("user_{}", uuid::Uuid::new_v4().simple());
    sqlx::query("INSERT INTO users (id, email) VALUES ($1, $2)").bind(&id).bind(format!("{id}@example.com")).execute(db).await.unwrap();
    id
}

/// A stored link: linked `linked_secs_ago`, the refresh token ending in `refresh_days`.
async fn link(db: &sqlx::PgPool, user: &str, profile: &str, linked_secs_ago: f64, refresh_days: i32, status: &str, last_error: Option<&str>) {
    sqlx::query("INSERT INTO anthropic_link_profiles (user_id, user_profile_id) VALUES ($1, $2)").bind(user).bind(profile).execute(db).await.unwrap();
    sqlx::query(
        "INSERT INTO anthropic_connections (user_id, user_profile_id, connection_id, status, last_error, linked_at, refresh_expires_at)
         VALUES ($1, $2, 'ocon_' || $2, $3, $4, now() - make_interval(secs => $5), now() + make_interval(days => $6))",
    )
    .bind(user)
    .bind(profile)
    .bind(status)
    .bind(last_error)
    .bind(linked_secs_ago)
    .bind(refresh_days)
    .execute(db)
    .await
    .unwrap();
}

async fn linked_user(db: &sqlx::PgPool, profile: &str) -> String {
    let user = new_user(db).await;
    link(db, &user, profile, 3600.0, 300, "linked", None).await;
    user
}

async fn credits(db: &sqlx::PgPool, user: &str, usd: f64) {
    sqlx::query("INSERT INTO user_credits (user_id, balance_usd) VALUES ($1, $2)").bind(user).bind(usd).execute(db).await.unwrap();
}

async fn balance(db: &sqlx::PgPool, user: &str) -> Option<f64> {
    sqlx::query_scalar("SELECT balance_usd FROM user_credits WHERE user_id = $1").bind(user).fetch_optional(db).await.unwrap()
}

async fn ledger_rows(db: &sqlx::PgPool, user: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM credit_transactions WHERE user_id = $1").bind(user).fetch_one(db).await.unwrap()
}

type UsageRow = (String, i64, i64, f64, Option<String>, bool);

async fn usage(db: &sqlx::PgPool, user: &str) -> Vec<UsageRow> {
    sqlx::query_as("SELECT model, prompt_tokens, completion_tokens, cost_usd, billed_to, estimated FROM inference_usage WHERE user_id = $1 ORDER BY created_at")
        .bind(user)
        .fetch_all(db)
        .await
        .unwrap()
}

/// Stream settlement is spawned when the body ends: wait for `n` rows.
async fn wait_usage(db: &sqlx::PgPool, user: &str, n: usize) -> Vec<UsageRow> {
    for _ in 0..100 {
        let rows = usage(db, user).await;
        if rows.len() >= n {
            return rows;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    usage(db, user).await
}

fn app(state: &Arc<ApiState>, user: &str, oauth: Option<OAuthCaller>) -> Router {
    let auth = AuthContext {
        user: AuthenticatedUser { user_id: user.into(), token_id: "sess_linked".into(), permissions: vec!["inference".into()] },
        is_development: false,
    };
    let mut router = Router::new().route("/v1/chat/completions", post(crate::routes::model_router::chat_completions)).layer(Extension(auth));
    if let Some(oauth) = oauth {
        router = router.layer(Extension(oauth));
    }
    router.with_state(state.clone())
}

async fn call(app: &Router, headers: &[(&str, &str)], body: Value) -> (StatusCode, HeaderMap, String) {
    let mut builder = axum::http::Request::builder().method("POST").uri("/v1/chat/completions").header("content-type", "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = app.clone().oneshot(builder.body(Body::from(body.to_string())).unwrap()).await.unwrap();
    let (status, headers) = (response.status(), response.headers().clone());
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 24).await.unwrap();
    (status, headers, String::from_utf8_lossy(&bytes).to_string())
}

fn turn(stream: bool) -> Value {
    json!({ "model": MODEL, "stream": stream, "messages": [{ "role": "user", "content": "Hello" }] })
}

fn parsed(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or(Value::Null)
}

fn refusal_body(code: &str, message: &str) -> Value {
    json!({ "error": { "code": code, "message": message }, "fallbacks": ["allternit-credits"] })
}

// ─── who may use it ───────────────────────────────────────────────────────────

#[tokio::test]
#[serial_test::serial(anthropic_client_env)]
async fn oauth_app_caller_403_person_only() {
    let (fake, base) = fake_anthropic().await;
    configure(&base);
    let state = ctx().await;
    let u = linked_user(&state.db, "uprof_app").await;
    let grant_id = uuid::Uuid::new_v4();
    let oauth = OAuthCaller {
        user_id: u.clone(),
        app_id: uuid::Uuid::new_v4(),
        client_id: "oapp_notes".into(),
        grant_id,
        token_id: uuid::Uuid::new_v4(),
        oauth_scopes: vec!["inference".into()],
        scopes: vec!["inference".into()],
        billing: CallerBilling::UserWallet { grant_id, cap_cents: 1000 },
        expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
    };
    // Even with the person header: an app is never the person (D8).
    let (status, _, body) = call(&app(&state, &u, Some(oauth)), &[PERSON], turn(false)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(parsed(&body), refusal_body("plan_person_only", PERSON_ONLY_MESSAGE));
    assert!(fake.chats().is_empty(), "never reaches Anthropic");
    clear_env();
}

#[tokio::test]
#[serial_test::serial(anthropic_client_env)]
async fn non_person_turn_403() {
    let (fake, base) = fake_anthropic().await;
    configure(&base);
    let state = ctx().await;
    let u = linked_user(&state.db, "uprof_np").await;
    let session = app(&state, &u, None);
    // No person header (a bot, routine or sub-agent send).
    let (status, _, body) = call(&session, &[], turn(false)).await;
    assert_eq!((status, parsed(&body)), (StatusCode::FORBIDDEN, refusal_body("plan_person_only", PERSON_ONLY_MESSAGE)));
    // A paired computer's device token, header or not.
    let (status, _, body) = call(&session, &[PERSON, ("authorization", "Bearer allternit_runtime_abc")], turn(true)).await;
    assert_eq!((status, parsed(&body)["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("plan_person_only")), "{body}");
    assert!(fake.chats().is_empty());
    // Every other route that takes a model refuses a linked id too.
    assert!(crate::routes::model_router::is_person_only_model(MODEL));
    assert!(crate::routes::platform_v1::responses::refuse_person_only_model(Some(MODEL)).is_err());
    clear_env();
}

#[tokio::test]
#[serial_test::serial(anthropic_client_env)]
async fn flag_off_not_available_yet() {
    let (fake, base) = fake_anthropic().await;
    let state = ctx().await;
    let u = linked_user(&state.db, "uprof_off").await;
    let expected = json!({ "error": { "code": "claude_link_unavailable", "message": NOT_AVAILABLE_MESSAGE, "reason": "not_available_yet" },
                           "fallbacks": ["allternit-credits"] });
    clear_env();
    let (status, _, body) = call(&app(&state, &u, None), &[PERSON], turn(false)).await;
    assert_eq!((status, parsed(&body)), (StatusCode::FORBIDDEN, expected.clone()));
    // Everything set but the switch.
    configure(&base);
    std::env::set_var(LINK_ENV, "0");
    let (status, _, body) = call(&app(&state, &u, None), &[PERSON], turn(true)).await;
    assert_eq!((status, parsed(&body)), (StatusCode::FORBIDDEN, expected));
    assert!(fake.chats().is_empty());
    assert!(usage(&state.db, &u).await.is_empty());
    clear_env();
}

#[tokio::test]
#[serial_test::serial(anthropic_client_env)]
async fn unlinked_user_401_disconnected() {
    let (fake, base) = fake_anthropic().await;
    configure(&base);
    let state = ctx().await;
    let never = new_user(&state.db).await;
    let gone = new_user(&state.db).await;
    link(&state.db, &gone, "uprof_gone", 3600.0, 300, "disconnected", Some("user_revoked")).await;
    let not_connected = new_user(&state.db).await;
    link(&state.db, &not_connected, "uprof_nc", 3600.0, 300, "disconnected", Some("not_connected")).await;
    for u in [&never, &gone, &not_connected] {
        let (status, _, body) = call(&app(&state, u, None), &[PERSON], turn(false)).await;
        assert_eq!((status, parsed(&body)), (StatusCode::UNAUTHORIZED, refusal_body("claude_link_disconnected", DISCONNECTED_MESSAGE)));
    }
    assert!(fake.chats().is_empty(), "no link, no call");
    clear_env();
}

// ─── the call ─────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial_test::serial(anthropic_client_env)]
async fn linked_request_sends_db_profile_and_billing_policy() {
    let (fake, base) = fake_anthropic().await;
    configure(&base);
    let state = ctx().await;
    let u = linked_user(&state.db, "uprof_db_1").await;

    let mut request = turn(false);
    request["allternit"] = json!({ "handoff": false });
    request["prompt_cache_key"] = json!("k");
    let (status, _, body) = call(&app(&state, &u, None), &[PERSON], request).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(parsed(&body)["choices"][0]["message"]["content"], "Hello from your org");

    let chats = fake.chats();
    assert_eq!(chats.len(), 1);
    let sent = &chats[0];
    assert!(sent.one("authorization").starts_with("Bearer sk-ant-oat01-"), "operator token");
    assert_eq!(sent.one("anthropic-user-profile-id"), "uprof_db_1");
    assert_eq!(sent.one("anthropic-beta"), "user-profiles-billing-2026-10-01");
    assert_eq!(sent.one("anthropic-version"), "2026-10-21");
    assert_eq!(
        sent.body,
        json!({ "model": "claude-sonnet-5-5", "stream": false, "messages": [{ "role": "user", "content": "Hello" }],
                "billing_policy": { "type": "billing_policy", "bill_to": "user_profile" } }),
        "prefix stripped, our options and OpenAI-only fields dropped, billing policy added"
    );
    assert_eq!(usage(&state.db, &u).await, vec![(MODEL.to_string(), 11, 7, 0.0, Some("claude_org".to_string()), false)]);

    // Relink needed (refresh token ends within 14 days) still serves; the
    // version follows ALLTERNIT_ANTHROPIC_VERSION.
    let soon = new_user(&state.db).await;
    link(&state.db, &soon, "uprof_db_2", 3600.0, 5, "linked", None).await;
    std::env::set_var(claude_link::ANTHROPIC_VERSION_ENV, "2027-01-01");
    let (status, _, body) = call(&app(&state, &soon, None), &[PERSON], turn(false)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sent = fake.chats().pop().unwrap();
    assert_eq!((sent.one("anthropic-user-profile-id"), sent.one("anthropic-version")), ("uprof_db_2".to_string(), "2027-01-01".to_string()));
    clear_env();
}

#[tokio::test]
#[serial_test::serial(anthropic_client_env)]
async fn smuggled_profile_header_dropped() {
    let (fake, base) = fake_anthropic().await;
    configure(&base);
    let state = ctx().await;
    let u = linked_user(&state.db, "uprof_mine").await;
    let headers = [
        PERSON,
        ("anthropic-user-profile-id", "uprof_evil"),
        ("anthropic-beta", "evil-beta"),
        ("anthropic-version", "1999-01-01"),
        ("anthropic-organization-id", "org_evil"),
        ("x-api-key", "sk-evil"),
    ];
    let (status, _, body) = call(&app(&state, &u, None), &headers, turn(false)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sent = fake.chats().pop().unwrap();
    assert_eq!(sent.one("anthropic-user-profile-id"), "uprof_mine", "from the DB, by the caller's user id");
    assert_eq!(sent.one("anthropic-beta"), "user-profiles-billing-2026-10-01");
    assert_eq!(sent.one("anthropic-version"), "2026-10-21");
    assert!(sent.header("anthropic-organization-id").is_empty() && sent.header("x-api-key").is_empty());
    assert!(!sent.headers.iter().any(|(_, v)| v.contains("evil")), "no incoming header reaches Anthropic: {:?}", sent.headers);
    clear_env();
}

#[tokio::test]
#[serial_test::serial(anthropic_client_env)]
async fn smuggled_profile_in_body_dropped() {
    let (fake, base) = fake_anthropic().await;
    configure(&base);
    let state = ctx().await;
    let u = linked_user(&state.db, "uprof_mine").await;
    let mut request = turn(false);
    request["user_profile_id"] = json!("uprof_evil");
    request["anthropic_user_profile_id"] = json!("uprof_evil");
    request["anthropic-user-profile-id"] = json!("uprof_evil");
    request["userProfileId"] = json!("uprof_evil");
    request["metadata"] = json!({ "user_profile_id": "uprof_evil", "user_id": "keep-me" });
    request["extra_body"] = json!({ "user_profile_id": "uprof_evil" });
    request["extra_headers"] = json!({ "anthropic-user-profile-id": "uprof_evil" });
    let (status, _, body) = call(&app(&state, &u, None), &[PERSON], request).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sent = fake.chats().pop().unwrap();
    assert!(!sent.body.to_string().contains("uprof_evil"), "{}", sent.body);
    assert_eq!(sent.body["metadata"], json!({ "user_id": "keep-me" }));
    assert_eq!(sent.one("anthropic-user-profile-id"), "uprof_mine");
    clear_env();
}

#[tokio::test]
#[serial_test::serial(anthropic_client_env)]
async fn smuggled_billing_policy_replaced() {
    let (fake, base) = fake_anthropic().await;
    configure(&base);
    let state = ctx().await;
    let u = linked_user(&state.db, "uprof_mine").await;
    let mut request = turn(false);
    request["billing_policy"] = json!({ "type": "billing_policy", "bill_to": "operator" });
    request["billingPolicy"] = json!({ "bill_to": "operator" });
    request["bill_to"] = json!("operator");
    let (status, _, body) = call(&app(&state, &u, None), &[PERSON], request).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sent = fake.chats().pop().unwrap();
    assert_eq!(sent.body["billing_policy"], json!({ "type": "billing_policy", "bill_to": "user_profile" }));
    assert!(sent.body.get("billingPolicy").is_none() && sent.body.get("bill_to").is_none(), "{}", sent.body);
    clear_env();
}

#[tokio::test]
#[serial_test::serial(anthropic_client_env)]
async fn code_exec_tools_dropped() {
    let (fake, base) = fake_anthropic().await;
    configure(&base);
    let state = ctx().await;
    let u = linked_user(&state.db, "uprof_tools").await;
    let session = app(&state, &u, None);
    let weather = json!({ "type": "function", "function": { "name": "get_weather", "parameters": { "type": "object" } } });
    let code_exec = json!({ "type": "code_execution_20250825", "name": "code_execution" });
    let with = |tools: Value, choice: Value| {
        let mut request = turn(false);
        request["tools"] = tools;
        request["tool_choice"] = choice;
        request
    };

    // Function tools stay; the code execution tool goes.
    let (status, _, body) = call(&session, &[PERSON], with(json!([weather, code_exec, { "type": "code_interpreter" }]), json!("auto"))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sent = fake.chats().pop().unwrap();
    assert_eq!((sent.body["tools"].clone(), sent.body["tool_choice"].clone()), (json!([weather]), json!("auto")));

    // Only code execution: tools and tool_choice both go.
    let (status, _, _) = call(&session, &[PERSON], with(json!([code_exec]), json!("auto"))).await;
    assert_eq!(status, StatusCode::OK);
    let sent = fake.chats().pop().unwrap();
    assert!(sent.body.get("tools").is_none() && sent.body.get("tool_choice").is_none(), "{}", sent.body);

    // A tool_choice that insists on code execution: 400, no call.
    let before = fake.chats().len();
    for choice in [
        json!({ "type": "tool", "name": "code_execution" }),
        json!({ "type": "function", "function": { "name": "code_execution" } }),
        json!({ "type": "code_execution_20250825" }),
    ] {
        let (status, _, body) = call(&session, &[PERSON], with(json!([weather, code_exec]), choice.clone())).await;
        assert_eq!((status, parsed(&body)), (StatusCode::BAD_REQUEST, refusal_body("linked_tools_not_allowed", TOOLS_MESSAGE)), "{choice}");
    }
    let (status, _, body) = call(&session, &[PERSON], with(json!([code_exec]), json!("required"))).await;
    assert_eq!((status, parsed(&body)["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("linked_tools_not_allowed")));
    assert_eq!(fake.chats().len(), before, "refused before Anthropic");
    clear_env();
}

// ─── refusals ─────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial_test::serial(anthropic_client_env)]
async fn refusal_maps_codes_table() {
    let (fake, base) = fake_anthropic().await;
    configure(&base);
    let state = ctx().await;
    let auth_error = || Reply::Json(401, json!({ "type": "error", "error": { "type": "authentication_error", "message": "invalid x-api-key" } }), None);
    // (replies, expected status, code, message, retry-after)
    let cases: Vec<(Vec<Reply>, u16, &str, &str, Option<&str>)> = vec![
        // 403 user_profile_not_billable (chat completions: the code in error.code).
        (vec![Reply::Json(403, json!({ "error": { "type": "invalid_request_error", "code": "user_profile_not_billable", "message": "x" } }), None)],
         403, "claude_link_not_billable", NOT_BILLABLE_MESSAGE, None),
        // The same code where /v1/messages puts it (error.details.error_code).
        (vec![Reply::Json(403, json!({ "type": "error", "error": { "type": "permission_error", "details": { "error_code": "user_profile_not_billable" } } }), None)],
         403, "claude_link_not_billable", NOT_BILLABLE_MESSAGE, None),
        // 402 spend limit, with error.code a generic billing_error.
        (vec![Reply::Json(402, json!({ "error": { "type": "invalid_request_error", "code": "billing_error",
                                                   "details": { "error_code": "linked_organization_spend_limit_reached" } } }), None)],
         402, "claude_link_spend_limit", SPEND_LIMIT_MESSAGE, None),
        // 402 billing unavailable.
        (vec![Reply::Json(402, json!({ "error": { "type": "invalid_request_error", "code": "linked_organization_billing_unavailable" } }), None)],
         402, "claude_link_billing_unavailable", BILLING_UNAVAILABLE_MESSAGE, None),
        // billing_error with the code only in the message text.
        (vec![Reply::Json(402, json!({ "error": { "type": "invalid_request_error", "code": "billing_error",
                                                   "message": "linked_organization_billing_unavailable: billing is paused" } }), None)],
         402, "claude_link_billing_unavailable", BILLING_UNAVAILABLE_MESSAGE, None),
        // 401 with the operator token: re-exchanged once, refused again.
        (vec![auth_error(), auth_error()], 500, "claude_link_unavailable", UNAVAILABLE_MESSAGE, None),
        // 403 permission_error with no code: our org is not billed directly.
        (vec![Reply::Json(403, json!({ "error": { "type": "permission_error", "message": "not billed directly" } }), None)],
         403, "claude_link_unavailable", UNAVAILABLE_MESSAGE, None),
        // 400 user_profile_required: our bug.
        (vec![Reply::Json(400, json!({ "error": { "type": "invalid_request_error", "code": "user_profile_required" } }), None)],
         500, "claude_link_unavailable", UNAVAILABLE_MESSAGE, None),
        // 529 / 429 / 5xx: upstream, retry-after kept or defaulted.
        (vec![Reply::Json(529, json!({ "error": { "type": "overloaded_error" } }), Some("7"))], 503, "claude_link_upstream", UPSTREAM_MESSAGE, Some("7")),
        (vec![Reply::Json(429, json!({ "error": { "type": "rate_limit_error" } }), None)], 503, "claude_link_upstream", UPSTREAM_MESSAGE, Some("5")),
        (vec![Reply::Json(503, json!({ "error": { "type": "overloaded_error" } }), Some("30"))], 503, "claude_link_upstream", UPSTREAM_MESSAGE, Some("30")),
        (vec![Reply::Json(500, json!({ "error": { "type": "api_error" } }), None)], 503, "claude_link_upstream", UPSTREAM_MESSAGE, Some("5")),
    ];
    for (i, (replies, status, code, message, retry_after)) in cases.into_iter().enumerate() {
        let calls = replies.len();
        for reply in replies {
            fake.reply(reply);
        }
        // Linked an hour ago: no not-billable wait.
        let u = linked_user(&state.db, &format!("uprof_case_{i}")).await;
        let before = fake.chats().len();
        let (got, headers, body) = call(&app(&state, &u, None), &[PERSON], turn(false)).await;
        assert_eq!((got.as_u16(), parsed(&body)), (status, refusal_body(code, message)), "case {i}");
        assert_eq!(headers.get("retry-after").and_then(|v| v.to_str().ok()), retry_after, "case {i}");
        assert_eq!(fake.chats().len() - before, calls, "case {i}: no retry beyond the operator re-exchange");
        assert!(usage(&state.db, &u).await.is_empty(), "case {i}: a refusal bills nobody");
    }
    clear_env();
}

#[tokio::test]
#[serial_test::serial(anthropic_client_env)]
async fn refusal_zero_debit_one_upstream_call() {
    let (fake, base) = fake_anthropic().await;
    configure(&base);
    let state = ctx().await;
    let u = linked_user(&state.db, "uprof_zero").await;
    credits(&state.db, &u, 10.0).await;
    let unavailable = || Reply::Json(402, json!({ "error": { "type": "invalid_request_error", "code": "linked_organization_billing_unavailable" } }), None);

    for stream in [false, true] {
        fake.reply(unavailable());
        let before = fake.chats().len();
        let (status, _, body) = call(&app(&state, &u, None), &[PERSON], turn(stream)).await;
        assert_eq!((status, parsed(&body)), (StatusCode::PAYMENT_REQUIRED, refusal_body("claude_link_billing_unavailable", BILLING_UNAVAILABLE_MESSAGE)));
        assert_eq!(fake.chats().len() - before, 1, "one upstream call, no retry");
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(balance(&state.db, &u).await, Some(10.0), "wallet untouched");
    assert_eq!(ledger_rows(&state.db, &u).await, 0);
    assert!(usage(&state.db, &u).await.is_empty());
    clear_env();
}

/// A provider the shared catalog routes Claude through: any call here would
/// be a fallback to Allternit credits.
#[derive(Default)]
struct CreditsUpstream {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl UpstreamProvider for CreditsUpstream {
    fn provider_id(&self) -> &str {
        "openrouter"
    }
    async fn list_models(&self) -> Result<Vec<ModelInfo>, ModelRouterError> {
        Ok(Vec::new())
    }
    async fn chat_completions(&self, _request: ChatCompletionRequest) -> Result<axum::http::Response<Body>, ModelRouterError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Json(ok_json()).into_response())
    }
}

#[tokio::test]
#[serial_test::serial(anthropic_client_env)]
async fn refusal_never_falls_back() {
    let (fake, base) = fake_anthropic().await;
    configure(&base);
    let credits_upstream = Arc::new(CreditsUpstream::default());
    let router = ModelRouter::new(vec![credits_upstream.clone() as Arc<dyn UpstreamProvider>], starter_catalog());
    let state = ctx_with(Some(router)).await;
    let u = linked_user(&state.db, "uprof_nofall").await;
    credits(&state.db, &u, 10.0).await;

    fake.reply(Reply::Json(402, json!({ "error": { "type": "invalid_request_error", "code": "linked_organization_spend_limit_reached" } }), None));
    let (status, _, body) = call(&app(&state, &u, None), &[PERSON], turn(false)).await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
    assert_eq!(parsed(&body), refusal_body("claude_link_spend_limit", SPEND_LIMIT_MESSAGE), "the client offers credits; the person taps");

    // Mid-stream too: the error ends the message, nothing else answers.
    fake.reply(Reply::Sse(vec![chunk("Par"), "event: error\ndata: {\"error\":{\"type\":\"overloaded_error\"}}\n\n".to_string()]));
    let (status, _, body) = call(&app(&state, &u, None), &[PERSON], turn(true)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("claude_link_upstream") && body.trim_end().ends_with("data: [DONE]"), "{body}");

    assert_eq!(credits_upstream.calls.load(Ordering::SeqCst), 0, "never another payer");
    assert_eq!(fake.chats().len(), 2);
    assert_eq!(balance(&state.db, &u).await, Some(10.0));
    assert_eq!(ledger_rows(&state.db, &u).await, 0);
    clear_env();
}

#[tokio::test]
#[serial_test::serial(anthropic_client_env)]
async fn operator_401_reexchanges_once() {
    let (fake, base) = fake_anthropic().await;
    configure(&base);
    let state = ctx().await;
    let u = linked_user(&state.db, "uprof_401").await;
    let auth_error = || Reply::Json(401, json!({ "type": "error", "error": { "type": "authentication_error" } }), None);

    fake.reply(auth_error());
    let (status, _, body) = call(&app(&state, &u, None), &[PERSON], turn(false)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let chats = fake.chats();
    assert_eq!(chats.len(), 2, "the refused call and one retry");
    let (first, second) = (chats[0].one("authorization"), chats[1].one("authorization"));
    assert_ne!(first, second, "the rejected token was dropped");
    assert!(second.starts_with("Bearer sk-ant-oat01-linked-"), "a fresh exchange: {second}");
    assert!(fake.exchanges.load(Ordering::SeqCst) >= 1, "exchanged at this Anthropic");
    assert_eq!(chats[1].one("anthropic-user-profile-id"), "uprof_401");
    assert_eq!(usage(&state.db, &u).await.len(), 1);

    // Refused again after the re-exchange: unavailable, no third call.
    fake.reply(auth_error());
    fake.reply(auth_error());
    let (status, _, body) = call(&app(&state, &u, None), &[PERSON], turn(false)).await;
    assert_eq!((status, parsed(&body)), (StatusCode::INTERNAL_SERVER_ERROR, refusal_body("claude_link_unavailable", UNAVAILABLE_MESSAGE)));
    assert_eq!(fake.chats().len(), 4);
    clear_env();
}

// ─── streams ──────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial_test::serial(anthropic_client_env)]
async fn stream_http_error_before_first_byte() {
    let (fake, base) = fake_anthropic().await;
    configure(&base);
    let state = ctx().await;
    let u = linked_user(&state.db, "uprof_s402").await;
    fake.reply(Reply::Json(402, json!({ "error": { "type": "invalid_request_error", "code": "linked_organization_spend_limit_reached" } }), None));
    let (status, headers, body) = call(&app(&state, &u, None), &[PERSON], turn(true)).await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
    assert!(headers["content-type"].to_str().unwrap().starts_with("application/json"), "a plain refusal, not a stream");
    assert_eq!(parsed(&body), refusal_body("claude_link_spend_limit", SPEND_LIMIT_MESSAGE));
    assert_eq!(fake.chats().len(), 1);
    assert_eq!(fake.chats()[0].one("accept"), "text/event-stream");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(usage(&state.db, &u).await.is_empty());
    clear_env();
}

#[tokio::test]
#[serial_test::serial(anthropic_client_env)]
async fn stream_midstream_billing_unavailable_ends_with_error() {
    let (fake, base) = fake_anthropic().await;
    configure(&base);
    let state = ctx().await;
    let u = linked_user(&state.db, "uprof_mid").await;
    credits(&state.db, &u, 10.0).await;

    // An SSE `error` event, split across two network chunks.
    fake.reply(Reply::Sse(vec![
        chunk("Partial"),
        chunk(" text"),
        "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",\"code\":\"billing_error\",".to_string(),
        "\"details\":{\"error_code\":\"linked_organization_billing_unavailable\"},\"message\":\"Billing unavailable\"}}\n\n".to_string(),
        chunk(" never forwarded"),
        "data: [DONE]\n\n".to_string(),
    ]));
    let (status, _, body) = call(&app(&state, &u, None), &[PERSON], turn(true)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"Partial\"") && body.contains("\" text\""), "the text so far stays: {body}");
    assert!(!body.contains("never forwarded") && !body.contains("event: error"), "{body}");
    let events: Vec<&str> = body.split("\n\n").map(str::trim).filter(|e| !e.is_empty()).collect();
    assert_eq!(events.last(), Some(&"data: [DONE]"), "{body}");
    let error_event = events[events.len() - 2].strip_prefix("data: ").unwrap();
    assert_eq!(parsed(error_event), refusal_body("claude_link_billing_unavailable", BILLING_UNAVAILABLE_MESSAGE));

    // A JSON chunk carrying `error` (no event line) ends it the same way.
    fake.reply(Reply::Sse(vec![
        chunk("Hi"),
        "data: {\"error\":{\"type\":\"invalid_request_error\",\"code\":\"linked_organization_spend_limit_reached\"}}\n\n".to_string(),
        chunk("x"),
    ]));
    let (_, _, body) = call(&app(&state, &u, None), &[PERSON], turn(true)).await;
    assert!(body.contains("\"Hi\"") && body.contains("claude_link_spend_limit") && !body.contains("\"x\""), "{body}");

    // The partial messages are recorded for the org (estimated), never charged.
    let rows = wait_usage(&state.db, &u, 2).await;
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows.iter().all(|r| r.3 == 0.0 && r.4.as_deref() == Some("claude_org") && r.5), "{rows:?}");
    assert_eq!(balance(&state.db, &u).await, Some(10.0));
    assert_eq!(ledger_rows(&state.db, &u).await, 0);
    assert_eq!(fake.chats().len(), 2, "no retry, no other payer");
    clear_env();
}

// ─── money ────────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial_test::serial(anthropic_client_env)]
async fn no_wallet_debit_billed_to_claude_org() {
    let (fake, base) = fake_anthropic().await;
    configure(&base);
    let state = ctx().await;
    let rich = linked_user(&state.db, "uprof_rich").await;
    credits(&state.db, &rich, 10.0).await;
    // No credits row and an empty wallet: the credits gate would answer 402.
    let none = linked_user(&state.db, "uprof_none").await;
    let empty = linked_user(&state.db, "uprof_empty").await;
    credits(&state.db, &empty, 0.0).await;

    for u in [&rich, &none, &empty] {
        let (status, _, body) = call(&app(&state, u, None), &[PERSON], turn(false)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, _, body) = call(&app(&state, u, None), &[PERSON], turn(true)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("\" there\"") && body.trim_end().ends_with("data: [DONE]"), "{body}");
        let rows = wait_usage(&state.db, u, 2).await;
        let row = (MODEL.to_string(), 11, 7, 0.0, Some("claude_org".to_string()), false);
        assert_eq!(rows, vec![row.clone(), row], "tokens recorded, billed to the org, 0 to the wallet");
        assert_eq!(ledger_rows(&state.db, u).await, 0, "no debit, no auto top-up");
    }
    assert_eq!(balance(&state.db, &rich).await, Some(10.0));
    assert_eq!(balance(&state.db, &none).await, None);
    assert_eq!(balance(&state.db, &empty).await, Some(0.0));
    assert_eq!(fake.chats().len(), 6);
    clear_env();
}

// ─── the model list ───────────────────────────────────────────────────────────

#[tokio::test]
#[serial_test::serial(anthropic_client_env)]
async fn model_list_shows_linked_only_when_linked() {
    let (_fake, base) = fake_anthropic().await;
    configure(&base);
    let state = ctx().await;
    let linked = linked_user(&state.db, "uprof_list").await;
    let unlinked = new_user(&state.db).await;
    for (id, user, token) in [("tok_l", &linked, "allternit_linked_list_token_0123456789"), ("tok_u", &unlinked, "allternit_unlinked_list_token_0123456")] {
        sqlx::query("INSERT INTO api_tokens (id, token_hash, name, user_id, permissions) VALUES ($1, $2, 't', $3, '[\"*\"]')")
            .bind(id)
            .bind(crate::services::api_keys::hash_token(token))
            .bind(user)
            .execute(&state.db)
            .await
            .unwrap();
    }
    let router = Router::new().route("/v1/models", get(crate::routes::model_router::list_models)).with_state(state.clone());
    let models = |token: Option<&'static str>| {
        let router = router.clone();
        async move {
            let mut request = axum::http::Request::builder().uri("/v1/models");
            if let Some(token) = token {
                request = request.header("authorization", format!("Bearer {token}"));
            }
            let response = router.oneshot(request.body(Body::empty()).unwrap()).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
            parsed(&String::from_utf8_lossy(&bytes))["data"].as_array().unwrap().clone()
        }
    };
    let linked_ids = |rows: &[Value]| -> Vec<String> {
        rows.iter().filter_map(|r| r["id"].as_str()).filter(|id| id.starts_with(LINKED_PREFIX)).map(str::to_string).collect()
    };

    assert!(linked_ids(&models(None).await).is_empty(), "anonymous");
    assert!(linked_ids(&models(Some("allternit_unlinked_list_token_0123456")).await).is_empty(), "not linked");

    let rows = models(Some("allternit_linked_list_token_0123456789")).await;
    assert_eq!(linked_ids(&rows), vec!["linked/claude/claude-sonnet-5-5", "linked/claude/claude-sonnet-4-6"]);
    let first_linked = rows.iter().position(|r| r["id"].as_str().is_some_and(|id| id.starts_with(LINKED_PREFIX))).unwrap();
    assert!(rows[first_linked..].iter().all(|r| r["id"].as_str().unwrap().starts_with(LINKED_PREFIX)), "linked rows come last");
    assert_eq!(
        rows[first_linked],
        json!({ "id": "linked/claude/claude-sonnet-5-5", "object": "model", "created": 1790618686u64, "owned_by": "anthropic",
                "upstream_id": "claude-sonnet-5-5", "provider": "claude-org", "name": "Claude Sonnet 5.5 (your organization)",
                "context_length": 1000000, "lane": "linked", "billed_to": "claude_org", "group": "Claude organization", "default": false })
    );
    assert_eq!(rows[first_linked + 1]["hidden"], true, "an unlisted catalog model stays hidden");

    // Feature off, or the link disconnected: none.
    std::env::set_var(LINK_ENV, "0");
    assert!(linked_ids(&models(Some("allternit_linked_list_token_0123456789")).await).is_empty(), "feature off");
    std::env::set_var(LINK_ENV, "1");
    sqlx::query("UPDATE anthropic_connections SET status = 'disconnected' WHERE user_id = $1").bind(&linked).execute(&state.db).await.unwrap();
    assert!(linked_ids(&models(Some("allternit_linked_list_token_0123456789")).await).is_empty(), "disconnected");
    clear_env();
}

// ─── status (services::claude_link, Anthropic's documented shape) ─────────────

#[tokio::test]
#[serial_test::serial(anthropic_client_env)]
async fn status_parses_connection_status_type() {
    let (fake, base) = fake_anthropic().await;
    configure(&base);
    let state = ctx().await;
    let service = claude_link::service(&state).expect("configured");
    let connected = linked_user(&state.db, "uprof_sa").await;
    let revoked = linked_user(&state.db, "uprof_sb").await;
    {
        let mut profiles = fake.profiles.lock().unwrap();
        // `connection_status.type` wins over an older-guess `status` word.
        profiles.insert(
            "uprof_sa".into(),
            json!({ "id": "uprof_sa", "type": "user_profile", "status": "not_connected", "connection_status": { "type": "connected" },
                    "oauth_connections": { "data": [{ "id": "ocon_1", "oauth_app_id": "client_test" }] } }),
        );
        profiles.insert(
            "uprof_sb".into(),
            json!({ "id": "uprof_sb", "type": "user_profile",
                    "connection_status": { "type": "disconnected", "last_revoked_at": "2026-11-18T00:00:00Z", "last_revoked_reason": { "type": "user_revoked" } } }),
        );
    }

    let status = service.status(&connected).await.unwrap();
    assert_eq!((status.state, status.org_name.as_deref(), status.spend_limit_reached), ("linked", None, None), "no org name or spend limit from Anthropic");
    let read = fake.requests("GET", "/v1/user_profiles/uprof_sa").pop().unwrap();
    assert_eq!(read.query, "oauth_app_id=client_test", "our OAuth client id");
    assert_eq!(read.one("anthropic-beta"), "user-profiles-search-2026-08-30");
    assert_eq!(read.one("anthropic-version"), "2026-10-21");

    let status = service.status(&revoked).await.unwrap();
    assert_eq!(status.state, "disconnected");
    let reason: Option<String> = sqlx::query_scalar("SELECT last_error FROM anthropic_connections WHERE user_id = $1").bind(&revoked).fetch_one(&state.db).await.unwrap();
    assert_eq!(reason.as_deref(), Some("user_revoked"));

    // The spend limit comes from our own last linked request.
    fake.reply(Reply::Json(402, json!({ "error": { "type": "invalid_request_error", "code": "linked_organization_spend_limit_reached" } }), None));
    let (status, _, _) = call(&app(&state, &connected, None), &[PERSON], turn(false)).await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
    assert_eq!(service.status(&connected).await.unwrap().to_json()["spend_limit"], json!({ "reached": true }));
    let (status, _, _) = call(&app(&state, &connected, None), &[PERSON], turn(false)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(service.status(&connected).await.unwrap().to_json()["spend_limit"], json!({ "reached": false }));

    // Profile create: the version header, no beta, an empty body.
    let fresh = new_user(&state.db).await;
    assert_eq!(service.ensure_profile(&fresh).await.unwrap(), "uprof_made");
    let made = fake.requests("POST", "/v1/user_profiles").pop().unwrap();
    assert_eq!(made.one("anthropic-version"), "2026-10-21");
    assert!(made.header("anthropic-beta").is_empty());
    assert_eq!(made.body, json!({}));
    clear_env();
}
