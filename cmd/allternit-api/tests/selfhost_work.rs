//! The work queue (plan step 7), the agent loop (step 8) and rehydration
//! (step 9), over real HTTP against the real router and SQLite store.
//!
//! The agent loop tests run the real `GizziTurnRunner` against a small axum
//! stand-in for gizzi's session and event routes (gizzi is an edge here: the
//! model provider behind it is what we cannot call in a test). The stand-in
//! calls the server's real internal tool route, the way gizzi's
//! `GIZZI_TOOL_HOST=remote` does.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use allternit_api::selfhost::agent_loop::GizziTurnRunner;
use allternit_api::selfhost::settings::Settings;
use allternit_api::selfhost::store::sqlite::SqliteStore;
use allternit_api::selfhost::store::*;
use allternit_api::selfhost::{router, SelfHostState};
use axum::extract::{Path, State};
use axum::response::sse::{Event as SseEvent, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde_json::{json, Value};
use tokio::sync::broadcast;

// ─── Harness ────────────────────────────────────────────────────────────────

async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .unwrap();
    });
    format!("http://{addr}")
}

struct Resp {
    status: u16,
    closed: bool,
    body: Value,
}

struct Fx {
    st: SelfHostState,
    base: String,
    http: reqwest::Client,
    dev: String,
    aek: String,
    env_id: String,
    session_id: String,
    _tmp: tempfile::TempDir,
}

impl Fx {
    async fn post(&self, path: &str, auth: Auth<'_>, body: Value) -> Resp {
        let mut rb = self.http.post(format!("{}{path}", self.base)).json(&body);
        rb = match auth {
            Auth::Bearer(t) => rb.bearer_auth(t),
            Auth::Secret(s) => rb.header("x-session-secret", s),
            Auth::Worker(t, host) => rb
                .bearer_auth(t)
                .header("x-worker-hostname", host)
                .header("x-worker-version", "test 1.0"),
        };
        let res = rb.send().await.unwrap();
        let status = res.status().as_u16();
        let closed = res
            .headers()
            .get("x-session-closed")
            .and_then(|v| v.to_str().ok())
            == Some("true");
        let bytes = res.bytes().await.unwrap();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Resp { status, closed, body }
    }

    async fn claim(&self, block_ms: u64) -> Resp {
        self.post(
            &format!("/v1/environments/{}/work/claim", self.env_id),
            Auth::Worker(&self.aek, "box-1"),
            json!({ "worker_id": "w1", "block_ms": block_ms }),
        )
        .await
    }

    async fn next(&self, work_id: &str, secret: &str, block_ms: u64) -> Resp {
        self.post(
            &format!("/v1/work/{work_id}/tool_calls/next"),
            Auth::Secret(secret),
            json!({ "block_ms": block_ms }),
        )
        .await
    }

    async fn result(&self, call_id: &str, secret: &str, output: &str) -> Resp {
        self.post(
            &format!("/v1/tool_calls/{call_id}/result"),
            Auth::Secret(secret),
            json!({ "output": output }),
        )
        .await
    }

    async fn status(&self) -> String {
        self.st.store.get_session(&self.session_id).await.unwrap().status
    }

    async fn wait_status(&self, want: &str) {
        for _ in 0..200 {
            if self.status().await == want {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("session never reached {want}; it is {}", self.status().await);
    }

    async fn events(&self) -> Vec<EventRecord> {
        self.st.store.list_events(&self.session_id, 0, 500, None).await.unwrap()
    }
}

enum Auth<'a> {
    Bearer(&'a str),
    Secret(&'a str),
    Worker(&'a str, &'a str),
}

async fn mint_key(st: &SelfHostState) -> String {
    let secret = new_secret("ask");
    st.store
        .create_api_key(NewApiKey {
            name: "dev".into(),
            role: Role::Developer,
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

async fn new_session(st: &SelfHostState, model: &str) -> (String, String) {
    let a = st
        .store
        .create_agent(NewAgent { name: "a".into(), model: model.into(), ..Default::default() })
        .await
        .unwrap();
    let e = st
        .store
        .create_environment(NewEnvironment { name: "e".into(), network: "none".into(), ..Default::default() })
        .await
        .unwrap();
    let s = st
        .store
        .create_session(NewSession { agent_id: a.id, environment_id: e.id.clone(), ..Default::default() })
        .await
        .unwrap();
    (e.id, s.id)
}

/// `gizzi`: the stand-in's URL; None keeps the 501 stub runner.
async fn fixture(mut settings: Settings, gizzi: Option<&str>) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    settings.data_dir = tmp.path().to_path_buf();
    let mut st = SelfHostState::new(Arc::new(SqliteStore::open_memory().unwrap()), settings)
        .unwrap()
        .with_setup_token(None);
    if let Some(url) = gizzi {
        st = st.with_turn_runner(GizziTurnRunner::connect(url, tmp.path()));
    }
    let base = serve(router(st.clone())).await;
    let dev = mint_key(&st).await;
    let (env_id, session_id) = new_session(&st, "claude-sonnet").await;
    let aek = mint_env_key(&st, &env_id).await;
    Fx {
        st,
        base,
        http: reqwest::Client::builder().no_proxy().build().unwrap(),
        dev,
        aek,
        env_id,
        session_id,
        _tmp: tmp,
    }
}

// ─── Work queue (step 7) ────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn claim_next_result_returns_to_the_internal_call() {
    let fx = Arc::new(fixture(Settings::default(), None).await);

    // The agent loop hands a bash call to the queue and blocks.
    let loop_call = {
        let fx = fx.clone();
        tokio::spawn(async move {
            let token = fx.st.internal_token.to_string();
            fx.post(
                &format!("/internal/v1/sessions/{}/tool_calls", fx.session_id),
                Auth::Bearer(&token),
                json!({ "tool_call_id": "toolu_1", "name": "bash", "input": { "command": "echo ok" }, "timeout_ms": 20000 }),
            )
            .await
        })
    };

    // A worker that long-polls gets the item as soon as it is queued.
    let claim = fx.claim(10_000).await;
    assert_eq!(claim.status, 200, "{}", claim.body);
    assert_eq!(claim.body["session_id"], json!(fx.session_id));
    assert_eq!(claim.body["lease_generation"], json!(1));
    assert_eq!(claim.body["environment"]["network"], json!("none"));
    let work_id = claim.body["work_id"].as_str().unwrap().to_string();
    let secret = claim.body["session_secret"].as_str().unwrap().to_string();

    let call = fx.next(&work_id, &secret, 10_000).await;
    assert_eq!(call.status, 200, "{}", call.body);
    assert_eq!(call.body["tool_call_id"], json!("toolu_1"));
    assert_eq!(call.body["name"], json!("bash"));
    assert_eq!(call.body["input"]["command"], json!("echo ok"));

    let r = fx.result("toolu_1", &secret, "ok\n").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body, json!({ "tool_call_id": "toolu_1", "status": "completed" }));

    let done = loop_call.await.unwrap();
    assert_eq!(done.status, 200, "{}", done.body);
    assert_eq!(done.body, json!({ "output": "ok\n", "is_error": false }));

    // A repeat result stores nothing new; a repeat call returns the stored result.
    let again = fx.result("toolu_1", &secret, "different").await;
    assert_eq!(again.status, 200);
    let token = fx.st.internal_token.to_string();
    let repeat = fx
        .post(
            &format!("/internal/v1/sessions/{}/tool_calls", fx.session_id),
            Auth::Bearer(&token),
            json!({ "tool_call_id": "toolu_1", "name": "bash", "input": {} }),
        )
        .await;
    assert_eq!(repeat.body, json!({ "output": "ok\n", "is_error": false }));

    // The worker headers are recorded.
    let workers = fx
        .st
        .store
        .list_workers(&fx.env_id, Utc::now() - chrono::Duration::minutes(1))
        .await
        .unwrap();
    assert_eq!(workers[0].hostname.as_deref(), Some("box-1"));
    assert_eq!(workers[0].version.as_deref(), Some("test 1.0"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_generation_is_409() {
    let fx = fixture(Settings::default(), None).await;
    fx.st.store.enqueue_work(&fx.env_id, &fx.session_id).await.unwrap();
    let c1 = fx.claim(0).await;
    let work_id = c1.body["work_id"].as_str().unwrap().to_string();
    let rel = fx
        .post(&format!("/v1/work/{work_id}/release"), Auth::Bearer(&fx.aek), json!({ "lease_generation": 1 }))
        .await;
    assert_eq!(rel.status, 200);
    assert_eq!(rel.body["status"], json!("ready"));
    let c2 = fx.claim(0).await;
    assert_eq!(c2.body["lease_generation"], json!(2));

    for route in ["renew", "heartbeat", "complete", "release"] {
        let r = fx
            .post(&format!("/v1/work/{work_id}/{route}"), Auth::Bearer(&fx.aek), json!({ "lease_generation": 1 }))
            .await;
        assert_eq!(r.status, 409, "{route}: {}", r.body);
        assert_eq!(r.body["error"]["code"], json!("lease_stale"));
    }
    let ok = fx
        .post(&format!("/v1/work/{work_id}/renew"), Auth::Bearer(&fx.aek), json!({ "lease_generation": 2 }))
        .await;
    assert_eq!(ok.status, 200);
    assert_eq!(ok.body["status"], json!("claimed"));
    // The first claim's secret ended with its lease.
    let old = fx.next(&work_id, c1.body["session_secret"].as_str().unwrap(), 0).await;
    assert_eq!(old.status, 401);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oversize_result_is_413() {
    let fx = fixture(Settings::default(), None).await;
    for id in ["big", "fits"] {
        fx.st
            .store
            .insert_tool_call(NewToolCall {
                id: id.into(),
                session_id: fx.session_id.clone(),
                name: "read".into(),
                input: json!({}),
            })
            .await
            .unwrap();
    }
    fx.st.store.enqueue_work(&fx.env_id, &fx.session_id).await.unwrap();
    let c = fx.claim(0).await;
    let secret = c.body["session_secret"].as_str().unwrap().to_string();
    let r = fx.result("big", &secret, &"x".repeat(MAX_TOOL_RESULT_BYTES + 1)).await;
    assert_eq!(r.status, 413);
    assert_eq!(r.body["error"]["code"], json!("result_too_large"));
    let r = fx.result("fits", &secret, &"x".repeat(MAX_TOOL_RESULT_BYTES)).await;
    assert_eq!(r.status, 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn expired_lease_is_reclaimed_and_its_calls_come_back() {
    let fx = fixture(Settings::default(), None).await;
    fx.st
        .store
        .insert_tool_call(NewToolCall {
            id: "toolu_lost".into(),
            session_id: fx.session_id.clone(),
            name: "bash".into(),
            input: json!({ "command": "true" }),
        })
        .await
        .unwrap();
    let item = fx.st.store.enqueue_work(&fx.env_id, &fx.session_id).await.unwrap();
    // A worker claimed it two minutes ago, took the call, and died.
    let dead = fx
        .st
        .store
        .claim_work(ClaimRequest {
            environment_id: fx.env_id.clone(),
            worker_id: "dead".into(),
            reclaim_older_than_ms: None,
            hostname: None,
            version: None,
            now: Utc::now() - chrono::Duration::seconds(120),
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fx.st.store.next_tool_call(&item.id, 1).await.unwrap().unwrap().id, "toolu_lost");

    // Its lease ran out, so a plain claim takes it.
    let c = fx.claim(0).await;
    assert_eq!(c.status, 200, "{}", c.body);
    assert_eq!(c.body["work_id"], json!(item.id));
    assert_eq!(c.body["lease_generation"], json!(2));
    let secret = c.body["session_secret"].as_str().unwrap().to_string();
    // The call the dead worker held is handed out again, same id.
    let again = fx.next(&item.id, &secret, 0).await;
    assert_eq!(again.body["tool_call_id"], json!("toolu_lost"));
    // The dead worker is fenced off.
    let late = fx
        .post(&format!("/v1/work/{}/renew", item.id), Auth::Bearer(&fx.aek), json!({ "lease_generation": dead.item.lease_generation }))
        .await;
    assert_eq!(late.status, 409);

    // The item is now held by a live lease: a plain claim leaves it alone,
    // reclaim_older_than_ms takes it when it was not renewed for that long.
    assert_eq!(fx.claim(0).await.status, 204);
    let long = fx
        .post(
            &format!("/v1/environments/{}/work/claim", fx.env_id),
            Auth::Bearer(&fx.aek),
            json!({ "worker_id": "w2", "reclaim_older_than_ms": 30000 }),
        )
        .await;
    assert_eq!(long.status, 204, "renewed less than 30 s ago");
    let stolen = fx
        .post(
            &format!("/v1/environments/{}/work/claim", fx.env_id),
            Auth::Bearer(&fx.aek),
            json!({ "worker_id": "w2", "reclaim_older_than_ms": 0 }),
        )
        .await;
    assert_eq!(stolen.status, 200, "{}", stolen.body);
    assert_eq!(stolen.body["work_id"], json!(item.id));
    assert_eq!(stolen.body["lease_generation"], json!(3));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_or_archived_session_closes_its_item() {
    let mut settings = Settings::default();
    settings.sessions.idle_timeout = "2s".into();
    let fx = fixture(settings, None).await;
    fx.st.store.enqueue_work(&fx.env_id, &fx.session_id).await.unwrap();
    let c = fx.claim(0).await;
    let work_id = c.body["work_id"].as_str().unwrap().to_string();
    let secret = c.body["session_secret"].as_str().unwrap().to_string();

    // Fresh: a plain 204, poll again.
    let r = fx.next(&work_id, &secret, 0).await;
    assert_eq!(r.status, 204);
    assert!(!r.closed);

    tokio::time::sleep(Duration::from_millis(2300)).await;
    let r = fx.next(&work_id, &secret, 0).await;
    assert_eq!(r.status, 204);
    assert!(r.closed, "an idle session answers X-Session-Closed");
    let done = fx
        .post(&format!("/v1/work/{work_id}/complete"), Auth::Bearer(&fx.aek), json!({ "lease_generation": 1 }))
        .await;
    assert_eq!(done.body["status"], json!("completed"));
    assert!(fx.st.store.find_open_work_for_session(&fx.session_id).await.unwrap().is_none());

    // Archiving a session whose item a worker holds closes it at once.
    let agent_id = fx.st.store.get_session(&fx.session_id).await.unwrap().agent_id;
    let s2 = fx
        .st
        .store
        .create_session(NewSession { agent_id, environment_id: fx.env_id.clone(), ..Default::default() })
        .await
        .unwrap()
        .id;
    fx.st.store.enqueue_work(&fx.env_id, &s2).await.unwrap();
    let c2 = fx.claim(0).await;
    let w2 = c2.body["work_id"].as_str().unwrap().to_string();
    let sec2 = c2.body["session_secret"].as_str().unwrap().to_string();
    let polling = {
        let http = fx.http.clone();
        let url = format!("{}/v1/work/{w2}/tool_calls/next", fx.base);
        tokio::spawn(async move {
            let res = http
                .post(url)
                .header("x-session-secret", sec2)
                .json(&json!({ "block_ms": 10000 }))
                .send()
                .await
                .unwrap();
            (res.status().as_u16(), res.headers().get("x-session-closed").cloned())
        })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    let a = fx.post(&format!("/v1/sessions/{s2}/archive"), Auth::Bearer(&fx.dev), json!({})).await;
    assert_eq!(a.status, 200);
    let (status, closed) = tokio::time::timeout(Duration::from_secs(5), polling).await.unwrap().unwrap();
    assert_eq!(status, 204);
    assert_eq!(closed.unwrap().to_str().unwrap(), "true");
}

// ─── Agent loop (steps 8, 9) with a stand-in gizzi ──────────────────────────

#[derive(Clone)]
struct FakeGizzi(Arc<FakeInner>);

struct FakeInner {
    bus: broadcast::Sender<Value>,
    sessions: Mutex<HashMap<String, Vec<Value>>>,
    imports: Mutex<Vec<Value>>,
    prompts: AtomicU64,
    next: AtomicU64,
    /// The server under test and its internal token, for tool calls.
    server: Mutex<Option<(String, String)>>,
}

impl FakeGizzi {
    fn new() -> Self {
        let (bus, _) = broadcast::channel(256);
        Self(Arc::new(FakeInner {
            bus,
            sessions: Mutex::new(HashMap::new()),
            imports: Mutex::new(Vec::new()),
            prompts: AtomicU64::new(0),
            next: AtomicU64::new(1),
            server: Mutex::new(None),
        }))
    }

    fn id(&self, prefix: &str) -> String {
        format!("{prefix}_{}", self.0.next.fetch_add(1, Ordering::SeqCst))
    }

    /// What a gizzi restart does to its in-memory view: every session gone.
    fn forget(&self) {
        self.0.sessions.lock().unwrap().clear();
    }

    fn publish(&self, kind: &str, properties: Value) {
        let _ = self.0.bus.send(json!({ "type": kind, "properties": properties }));
    }

    fn router(&self) -> Router {
        Router::new()
            .route("/health", get(|| async { Json(json!({ "status": "ok" })) }))
            .route("/v1/session", post(fake_create))
            .route("/v1/session/import", post(fake_import))
            .route("/v1/session/:id", get(fake_get).patch(|| async { Json(json!({})) }))
            .route("/v1/session/:id/message", post(fake_prompt))
            .route("/v1/session/:id/messages", get(fake_messages))
            .route("/v1/session/:id/abort", post(|| async { Json(json!(true)) }))
            .route("/v1/mcp/add", post(|| async { Json(json!({})) }))
            .route("/v1/event", get(fake_events))
            .with_state(self.clone())
    }
}

async fn fake_create(State(f): State<FakeGizzi>) -> Json<Value> {
    let id = f.id("ses");
    f.0.sessions.lock().unwrap().insert(id.clone(), Vec::new());
    Json(json!({ "id": id }))
}

async fn fake_import(State(f): State<FakeGizzi>, Json(body): Json<Value>) -> Json<Value> {
    let id = f.id("ses");
    let msgs = body["messages"].as_array().cloned().unwrap_or_default();
    f.0.sessions.lock().unwrap().insert(id.clone(), msgs);
    f.0.imports.lock().unwrap().push(body);
    Json(json!({ "id": id }))
}

async fn fake_get(State(f): State<FakeGizzi>, Path(id): Path<String>) -> axum::response::Response {
    use axum::response::IntoResponse;
    if f.0.sessions.lock().unwrap().contains_key(&id) {
        Json(json!({ "id": id })).into_response()
    } else {
        (axum::http::StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response()
    }
}

async fn fake_messages(State(f): State<FakeGizzi>, Path(id): Path<String>) -> Json<Value> {
    Json(Value::Array(f.0.sessions.lock().unwrap().get(&id).cloned().unwrap_or_default()))
}

async fn fake_events(
    State(f): State<FakeGizzi>,
) -> Sse<impl futures::Stream<Item = Result<SseEvent, Infallible>>> {
    let mut rx = f.0.bus.subscribe();
    let stream = async_stream::stream! {
        yield Ok(SseEvent::default().data(json!({ "type": "server.connected", "properties": {} }).to_string()));
        loop {
            match rx.recv().await {
                Ok(v) => yield Ok(SseEvent::default().data(v.to_string())),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream)
}

/// A turn: text with "bash" in it runs one bash call through the server's
/// internal route (like `GIZZI_TOOL_HOST=remote`), anything else answers
/// with text. The reply says how many messages the session had before.
async fn fake_prompt(
    State(f): State<FakeGizzi>,
    Path(sid): Path<String>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let Some(earlier) = f.0.sessions.lock().unwrap().get(&sid).map(Vec::len) else {
        return (axum::http::StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
    };
    f.0.prompts.fetch_add(1, Ordering::SeqCst);
    let text = body["parts"][0]["text"].as_str().unwrap_or_default().to_string();
    let uid = f.id("msg_u");
    let aid = f.id("msg_a");
    let user = json!({
        "info": { "id": uid, "sessionID": sid, "role": "user" },
        "parts": [{ "id": f.id("prt"), "sessionID": sid, "messageID": uid, "type": "text", "text": text }],
    });
    f.publish("message.updated", json!({ "info": user["info"] }));
    let info = json!({ "id": aid, "sessionID": sid, "role": "assistant", "parentID": uid });
    f.publish("message.updated", json!({ "info": info }));

    let mut parts = Vec::new();
    let mut answer = format!("seen {earlier} earlier messages");
    if text.contains("bash") {
        let call_id = f.id("call");
        let input = json!({ "command": "echo hi" });
        let tool_pid = f.id("prt_tool");
        let running = json!({
            "id": tool_pid, "sessionID": sid, "messageID": aid, "type": "tool", "callID": call_id, "tool": "bash",
            "state": { "status": "running", "input": input, "time": { "start": 1 } },
        });
        f.publish("message.part.updated", json!({ "part": running }));
        let (server, token) = f.0.server.lock().unwrap().clone().expect("server set");
        let res = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(format!("{server}/internal/v1/sessions/{sid}/tool_calls"))
            .bearer_auth(token)
            .json(&json!({ "tool_call_id": call_id, "name": "bash", "input": input, "timeout_ms": 30000 }))
            .send()
            .await
            .unwrap();
        let result: Value = res.json().await.unwrap();
        let output = result["output"].as_str().unwrap_or_default().to_string();
        let done = json!({
            "id": tool_pid, "sessionID": sid, "messageID": aid, "type": "tool", "callID": call_id, "tool": "bash",
            "state": { "status": "completed", "input": input, "output": output, "title": "echo hi", "metadata": {}, "time": { "start": 1, "end": 2 } },
        });
        f.publish("message.part.updated", json!({ "part": done }));
        parts.push(done);
        answer = format!("ran: {output}");
    }
    let text_part = json!({
        "id": f.id("prt_text"), "sessionID": sid, "messageID": aid, "type": "text", "text": answer,
        "time": { "start": 3, "end": 4 },
    });
    f.publish("message.part.updated", json!({ "part": text_part }));
    parts.push(text_part);
    let assistant = json!({ "info": info, "parts": parts });
    if let Some(v) = f.0.sessions.lock().unwrap().get_mut(&sid) {
        v.push(user);
        v.push(assistant.clone());
    }
    Json(assistant).into_response()
}

async fn loop_fixture(settings: Settings) -> (Fx, FakeGizzi) {
    let fake = FakeGizzi::new();
    let gizzi = serve(fake.router()).await;
    let fx = fixture(settings, Some(&gizzi)).await;
    *fake.0.server.lock().unwrap() = Some((fx.base.clone(), fx.st.internal_token.to_string()));
    (fx, fake)
}

fn of_type<'a>(events: &'a [EventRecord], t: &str) -> Vec<&'a EventRecord> {
    events.iter().filter(|e| e.event_type == t).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn model_only_turn_finishes_with_no_worker() {
    let (fx, _fake) = loop_fixture(Settings::default()).await;
    let r = fx
        .post(&format!("/v1/sessions/{}/messages", fx.session_id), Auth::Bearer(&fx.dev), json!({ "content": "hi" }))
        .await;
    assert_eq!(r.status, 202, "{}", r.body);
    fx.wait_status("idle").await;

    let events = fx.events().await;
    let assistant: Vec<_> = of_type(&events, "message")
        .into_iter()
        .filter(|e| e.data["role"] == json!("assistant"))
        .collect();
    assert_eq!(assistant.len(), 1, "one assistant message, not repeated: {events:?}");
    assert_eq!(assistant[0].data["content"], json!("seen 0 earlier messages"));
    assert_eq!(events.last().unwrap().data["status"], json!("idle"));
    // No tool was needed, so nothing was queued.
    assert!(fx.st.store.find_open_work_for_session(&fx.session_id).await.unwrap().is_none());
    assert!(fx.st.store.get_session(&fx.session_id).await.unwrap().gizzi_session_id.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bash_call_waits_until_a_worker_attaches() {
    let (fx, _fake) = loop_fixture(Settings::default()).await;
    let r = fx
        .post(&format!("/v1/sessions/{}/messages", fx.session_id), Auth::Bearer(&fx.dev), json!({ "content": "run bash" }))
        .await;
    assert_eq!(r.status, 202);
    fx.wait_status("waiting_for_worker").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(fx.status().await, "waiting_for_worker", "no worker, so the call stays pending");
    assert_eq!(fx.st.store.queue_depth(&fx.env_id).await.unwrap(), 1);

    // A worker attaches and runs it.
    let c = fx.claim(5000).await;
    assert_eq!(c.status, 200);
    let work_id = c.body["work_id"].as_str().unwrap().to_string();
    let secret = c.body["session_secret"].as_str().unwrap().to_string();
    let call = fx.next(&work_id, &secret, 5000).await;
    assert_eq!(call.body["name"], json!("bash"));
    assert_eq!(call.body["input"]["command"], json!("echo hi"));
    let call_id = call.body["tool_call_id"].as_str().unwrap().to_string();
    assert_eq!(fx.result(&call_id, &secret, "hi\n").await.status, 200);
    fx.wait_status("idle").await;

    let events = fx.events().await;
    let calls = of_type(&events, "tool_call");
    let results = of_type(&events, "tool_result");
    assert_eq!(calls.len(), 1, "{events:?}");
    assert_eq!(results.len(), 1, "{events:?}");
    assert_eq!(calls[0].data["tool_call_id"], json!(call_id));
    assert_eq!(results[0].data["output"], json!("hi\n"));
    let statuses: Vec<&str> = of_type(&events, "status")
        .iter()
        .map(|e| e.data["status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses, ["waiting_for_worker", "running", "idle"]);
    let last_text = of_type(&events, "message").last().unwrap().data["content"].clone();
    assert_eq!(last_text, json!("ran: hi\n"));

    // The worker stays attached: the session is idle but not past its
    // idle timeout, so the poll is a plain 204.
    let r = fx.next(&work_id, &secret, 0).await;
    assert_eq!(r.status, 204);
    assert!(!r.closed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lost_gizzi_session_is_rebuilt_from_events() {
    let (fx, fake) = loop_fixture(Settings::default()).await;
    let send = |content: &'static str| {
        let path = format!("/v1/sessions/{}/messages", fx.session_id);
        let fx = &fx;
        async move { fx.post(&path, Auth::Bearer(&fx.dev), json!({ "content": content })).await }
    };
    assert_eq!(send("hi").await.status, 202);
    fx.wait_status("idle").await;
    let first = fx.st.store.get_session(&fx.session_id).await.unwrap().gizzi_session_id.unwrap();

    fake.forget();
    assert_eq!(send("again").await.status, 202);
    fx.wait_status("idle").await;

    let imports = fake.0.imports.lock().unwrap().clone();
    assert_eq!(imports.len(), 1);
    let msgs = imports[0]["messages"].as_array().unwrap();
    let roles: Vec<&str> = msgs.iter().map(|m| m["info"]["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["user", "assistant"]);
    assert_eq!(msgs[0]["parts"][0]["text"], json!("hi"));
    assert_eq!(msgs[1]["parts"][0]["text"], json!("seen 0 earlier messages"));

    let second = fx.st.store.get_session(&fx.session_id).await.unwrap().gizzi_session_id.unwrap();
    assert_ne!(first, second);
    let events = fx.events().await;
    let last = of_type(&events, "message").last().unwrap().data["content"].clone();
    assert_eq!(last, json!("seen 2 earlier messages"), "the next turn sees the earlier messages");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unmapped_model_at_turn_start_makes_no_call() {
    let settings = Settings::from_str(
        "inference:\n  provider: bedrock\n  model_map: {claude-sonnet: anthropic.claude-sonnet-v1}\n",
    )
    .unwrap();
    let (fx, fake) = loop_fixture(settings).await;
    // A session made before the map dropped its model (straight in the store,
    // past the create-time check).
    let (_, sid) = new_session(&fx.st, "gpt-4o").await;
    let r = fx
        .post(&format!("/v1/sessions/{sid}/messages"), Auth::Bearer(&fx.dev), json!({ "content": "hi" }))
        .await;
    assert_eq!(r.status, 422, "{}", r.body);
    assert_eq!(r.body["error"]["code"], json!("model_not_mapped"));
    assert_eq!(fake.0.prompts.load(Ordering::SeqCst), 0);
    assert!(fx.st.store.list_events(&sid, 0, 10, None).await.unwrap().is_empty());
    assert_eq!(fx.st.store.get_session(&sid).await.unwrap().status, "idle");
}
