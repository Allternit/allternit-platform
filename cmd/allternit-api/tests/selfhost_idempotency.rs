//! `Idempotency-Key` replay on the selfhost create routes, through real HTTP
//! requests against the real router and store. Every test runs on SQLite;
//! with `SELFHOST_TEST_PG_URL` set it runs on PostgreSQL too, each in its own
//! schema.
//!
//! The turn runner is the one edge replaced: it stands in for the gizzi
//! child and ends the turn at once, so the session takes the next message.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use allternit_api::selfhost::routes_sessions::TurnRunner;
use allternit_api::selfhost::settings::Settings;
use allternit_api::selfhost::store::pg::PgStore;
use allternit_api::selfhost::store::sqlite::SqliteStore;
use allternit_api::selfhost::store::*;
use allternit_api::selfhost::{router, ApiError, SelfHostState};
use axum::async_trait;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::Router;
use chrono::Utc;
use serde_json::{json, Value};
use tokio::sync::Notify;
use tower::ServiceExt;

/// Ends each turn at once. `fail_next` makes the next message a 503 before
/// anything is stored; `hold` keeps a turn open until released.
#[derive(Default)]
struct Runner {
    fail_next: AtomicBool,
    hold: AtomicBool,
    entered: Notify,
    release: Notify,
}

#[async_trait]
impl TurnRunner for Runner {
    fn available(&self) -> Result<(), ApiError> {
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(ApiError::unavailable("agent_loop_unavailable", "Starting."));
        }
        Ok(())
    }

    async fn start_turn(
        &self,
        state: &SelfHostState,
        session: &SessionRecord,
        _user_event: &EventRecord,
    ) -> Result<(), ApiError> {
        if self.hold.load(Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        state.store.set_session_status(&session.id, "idle", None).await?;
        Ok(())
    }

    async fn interrupt(&self, _state: &SelfHostState, _session_id: &str) -> Result<(), ApiError> {
        Ok(())
    }
}

fn sqlite() -> DynStore {
    Arc::new(SqliteStore::open_memory().expect("open sqlite"))
}

async fn postgres() -> Option<DynStore> {
    let url = std::env::var("SELFHOST_TEST_PG_URL").ok().filter(|u| !u.is_empty())?;
    let schema = format!("selfhost_idem_{}", uuid::Uuid::new_v4().simple());
    let store = PgStore::connect_in_schema(&url, 60, &schema)
        .await
        .expect("connect to SELFHOST_TEST_PG_URL");
    store.migrate().await.expect("migrate");
    Some(Arc::new(store))
}

macro_rules! on_both_stores {
    ($($name:ident),* $(,)?) => {
        mod on_sqlite {
            $(
                #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
                async fn $name() {
                    super::$name(super::sqlite()).await;
                }
            )*
        }
        mod on_postgres {
            $(
                #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
                async fn $name() {
                    let Some(store) = super::postgres().await else {
                        eprintln!("SELFHOST_TEST_PG_URL not set; Postgres run skipped");
                        return;
                    };
                    super::$name(store).await;
                }
            )*
        }
    };
}

on_both_stores!(
    repeat_message_replays_and_stores_one,
    other_body_same_key_is_422,
    revoked_key_is_401_not_a_replay,
    no_header_runs_every_time,
    failed_request_is_not_stored,
    running_first_request_is_409,
    create_routes_replay,
);

struct Fx {
    app: Router,
    st: SelfHostState,
    runner: Arc<Runner>,
    dev: String,
    sid: String,
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

async fn fixture(store: DynStore) -> Fx {
    let runner = Arc::new(Runner::default());
    let st = SelfHostState::new(store, Settings::default())
        .unwrap()
        .with_setup_token(None)
        .with_turn_runner(runner.clone());
    let app = router(st.clone());
    let dev = mint_key(&st, Role::Developer).await;
    let agent = post(&app, "/v1/agents", &dev, None, json!({"name": "a", "model": "claude-sonnet"})).await;
    assert_eq!(agent.status, StatusCode::CREATED, "{}", agent.body);
    let env = post(&app, "/v1/environments", &dev, None, json!({"name": "e"})).await;
    assert_eq!(env.status, StatusCode::CREATED, "{}", env.body);
    let sess = post(
        &app,
        "/v1/sessions",
        &dev,
        None,
        json!({"agent_id": agent.body["id"], "environment_id": env.body["id"]}),
    )
    .await;
    assert_eq!(sess.status, StatusCode::CREATED, "{}", sess.body);
    let sid = sess.body["id"].as_str().unwrap().to_string();
    Fx { app, st, runner, dev, sid }
}

struct Resp {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

impl Resp {
    fn replayed(&self) -> bool {
        self.headers.get("idempotency-replayed").map(|v| v.as_bytes()) == Some(b"true")
    }
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

async fn post(app: &Router, uri: &str, bearer: &str, key: Option<&str>, body: Value) -> Resp {
    let mut b = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("authorization", format!("Bearer {bearer}"))
        .header("content-type", "application/json");
    if let Some(k) = key {
        b = b.header("idempotency-key", k);
    }
    send(app, b.body(Body::from(body.to_string())).unwrap()).await
}

impl Fx {
    async fn message(&self, key: Option<&str>, content: &str) -> Resp {
        let uri = format!("/v1/sessions/{}/messages", self.sid);
        post(&self.app, &uri, &self.dev, key, json!({ "content": content })).await
    }

    /// User messages in the session log, through the API.
    async fn user_messages(&self) -> Vec<Value> {
        let req = Request::builder()
            .uri(format!("/v1/sessions/{}/events?types=message&limit=100", self.sid))
            .header("authorization", format!("Bearer {}", self.dev))
            .body(Body::empty())
            .unwrap();
        let r = send(&self.app, req).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        r.body["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["data"]["role"] == json!("user"))
            .cloned()
            .collect()
    }
}

fn assert_error(r: &Resp, status: StatusCode, kind: &str, code: &str) {
    assert_eq!(r.status, status, "body: {}", r.body);
    assert_eq!(r.body["error"]["type"], json!(kind), "body: {}", r.body);
    assert_eq!(r.body["error"]["code"], json!(code), "body: {}", r.body);
    assert!(r.body["error"]["message"].is_string());
    assert!(!r.replayed());
}

async fn repeat_message_replays_and_stores_one(store: DynStore) {
    let fx = fixture(store).await;
    let first = fx.message(Some("k-1"), "hi").await;
    assert_eq!(first.status, StatusCode::ACCEPTED, "{}", first.body);
    assert!(!first.replayed(), "the first answer is not a replay");
    let again = fx.message(Some("k-1"), "hi").await;
    assert_eq!(again.status, StatusCode::ACCEPTED, "{}", again.body);
    assert!(again.replayed());
    assert_eq!(again.body, first.body, "same stored event");
    assert_eq!(again.headers.get("content-type"), first.headers.get("content-type"));
    let users = fx.user_messages().await;
    assert_eq!(users.len(), 1, "one message stored: {users:?}");
    assert_eq!(users[0]["id"], first.body["id"]);
}

async fn other_body_same_key_is_422(store: DynStore) {
    let fx = fixture(store).await;
    assert_eq!(fx.message(Some("k-2"), "hi").await.status, StatusCode::ACCEPTED);
    let r = fx.message(Some("k-2"), "something else").await;
    assert_error(&r, StatusCode::UNPROCESSABLE_ENTITY, "unprocessable_error", "idempotency_key_reuse");
    assert_eq!(fx.user_messages().await.len(), 1);
    // A key belongs to its route: the same key on another path is new.
    let env = post(&fx.app, "/v1/environments", &fx.dev, Some("k-2"), json!({"name": "e2"})).await;
    assert_eq!(env.status, StatusCode::CREATED, "{}", env.body);
    assert!(!env.replayed());
}

async fn revoked_key_is_401_not_a_replay(store: DynStore) {
    let fx = fixture(store).await;
    assert_eq!(fx.message(Some("k-3"), "hi").await.status, StatusCode::ACCEPTED);
    let key = fx.st.store.find_api_key_by_hash(&hash_secret(&fx.dev)).await.unwrap().unwrap();
    fx.st.store.revoke_api_key(&key.id, Utc::now()).await.unwrap();
    let r = fx.message(Some("k-3"), "hi").await;
    assert_error(&r, StatusCode::UNAUTHORIZED, "authentication_error", "unauthorized");
    // No key at all, and a viewer, get the route's own answers too.
    let uri = format!("/v1/sessions/{}/messages", fx.sid);
    let req = Request::builder()
        .method(Method::POST)
        .uri(&uri)
        .header("content-type", "application/json")
        .header("idempotency-key", "k-3")
        .body(Body::from(json!({"content": "hi"}).to_string()))
        .unwrap();
    assert_error(&send(&fx.app, req).await, StatusCode::UNAUTHORIZED, "authentication_error", "unauthorized");
    let viewer = mint_key(&fx.st, Role::Viewer).await;
    let r = post(&fx.app, &uri, &viewer, Some("k-3"), json!({"content": "hi"})).await;
    assert_error(&r, StatusCode::FORBIDDEN, "permission_error", "forbidden");
    // Another caller's same key is its own record: it runs.
    let other = mint_key(&fx.st, Role::Developer).await;
    let r = post(&fx.app, &uri, &other, Some("k-3"), json!({"content": "hi"})).await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{}", r.body);
    assert!(!r.replayed());
}

async fn no_header_runs_every_time(store: DynStore) {
    let fx = fixture(store).await;
    let a = fx.message(None, "hi").await;
    let b = fx.message(None, "hi").await;
    assert_eq!(a.status, StatusCode::ACCEPTED, "{}", a.body);
    assert_eq!(b.status, StatusCode::ACCEPTED, "{}", b.body);
    assert!(!a.replayed() && !b.replayed());
    assert_ne!(a.body["id"], b.body["id"]);
    assert_eq!(fx.user_messages().await.len(), 2);
}

async fn failed_request_is_not_stored(store: DynStore) {
    let fx = fixture(store).await;
    fx.runner.fail_next.store(true, Ordering::SeqCst);
    let r = fx.message(Some("k-4"), "hi").await;
    assert_error(&r, StatusCode::SERVICE_UNAVAILABLE, "api_error", "agent_loop_unavailable");
    // Only 2xx is kept: the retry runs.
    let r = fx.message(Some("k-4"), "hi").await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{}", r.body);
    assert!(!r.replayed());
    assert_eq!(fx.user_messages().await.len(), 1);
    // A key over 255 characters is refused before anything runs.
    let long = "k".repeat(256);
    let r = fx.message(Some(&long), "hi").await;
    assert_error(&r, StatusCode::BAD_REQUEST, "invalid_request_error", "invalid_idempotency_key");
    assert_eq!(fx.user_messages().await.len(), 1);
}

async fn running_first_request_is_409(store: DynStore) {
    let fx = Arc::new(fixture(store).await);
    fx.runner.hold.store(true, Ordering::SeqCst);
    let first = tokio::spawn({
        let fx = fx.clone();
        async move { fx.message(Some("k-5"), "hi").await }
    });
    fx.runner.entered.notified().await;
    let r = fx.message(Some("k-5"), "hi").await;
    assert_error(&r, StatusCode::CONFLICT, "conflict_error", "idempotency_in_progress");
    fx.runner.hold.store(false, Ordering::SeqCst);
    fx.runner.release.notify_one();
    let first = first.await.unwrap();
    assert_eq!(first.status, StatusCode::ACCEPTED, "{}", first.body);
    let again = fx.message(Some("k-5"), "hi").await;
    assert!(again.replayed());
    assert_eq!(again.body, first.body);
    assert_eq!(fx.user_messages().await.len(), 1);
}

async fn create_routes_replay(store: DynStore) {
    let fx = fixture(store).await;
    let body = json!({"name": "agent-b", "model": "claude-sonnet"});
    let a = post(&fx.app, "/v1/agents", &fx.dev, Some("k-6"), body.clone()).await;
    let b = post(&fx.app, "/v1/agents", &fx.dev, Some("k-6"), body).await;
    assert_eq!(a.status, StatusCode::CREATED, "{}", a.body);
    assert_eq!(b.status, StatusCode::CREATED, "{}", b.body);
    assert!(b.replayed());
    assert_eq!(a.body, b.body);
    let agents = fx.st.store.list_agents(100, None).await.unwrap();
    assert_eq!(agents.iter().filter(|g| g.name == "agent-b").count(), 1, "one agent made");
}
