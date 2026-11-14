//! Several server replicas on one database (plan step S3a), over real HTTP
//! against the real router and the real stores.
//!
//! Each "replica" is an in-process server: its own `SelfHostState`, router,
//! replica id, wake listener and `GizziTurnRunner`. The tests that need two
//! replicas run on PostgreSQL (a fresh schema per test) when
//! `SELFHOST_TEST_PG_URL` is set and are skipped otherwise; the restart and
//! drain tests also run on SQLite.
//!
//! gizzi is an edge here (the model provider behind it is what we cannot
//! call): each replica gets a small axum stand-in for gizzi's session and
//! event routes. A prompt with "bash" in it runs one bash call through the
//! server's real internal tool route, the way gizzi's `GIZZI_TOOL_HOST=remote`
//! does, so the turn waits for a worker until one answers.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use allternit_api::selfhost::agent_loop::GizziTurnRunner;
use allternit_api::selfhost::replica;
use allternit_api::selfhost::settings::Settings;
use allternit_api::selfhost::store::pg::PgStore;
use allternit_api::selfhost::store::sqlite::SqliteStore;
use allternit_api::selfhost::store::*;
use allternit_api::selfhost::{router, wake, SelfHostState};
use axum::extract::{Path, State};
use axum::response::sse::{Event as SseEvent, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};

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

fn http() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

fn pg_url() -> Option<String> {
    let url = std::env::var("SELFHOST_TEST_PG_URL").ok().filter(|u| !u.is_empty());
    if url.is_none() {
        eprintln!("SELFHOST_TEST_PG_URL not set; Postgres run skipped");
    }
    url
}

/// Two connections (two pools) to one fresh schema, like two replicas.
async fn pg_pair(url: &str) -> (DynStore, DynStore) {
    let schema = format!("selfhost_r_{}", uuid::Uuid::new_v4().simple());
    let a = PgStore::connect_in_schema(url, 20, &schema).await.expect("connect");
    a.migrate().await.expect("migrate");
    let b = PgStore::connect_in_schema(url, 20, &schema).await.expect("connect");
    (Arc::new(a), Arc::new(b))
}

/// One server replica with its own gizzi stand-in.
struct Node {
    st: SelfHostState,
    base: String,
    fake: FakeGizzi,
    runner: Arc<GizziTurnRunner>,
    _tmp: Option<tempfile::TempDir>,
}

async fn node(store: DynStore, replica_id: &str, data_dir: Option<&std::path::Path>) -> Node {
    let (tmp, dir) = match data_dir {
        Some(d) => (None, d.to_path_buf()),
        None => {
            let t = tempfile::tempdir().unwrap();
            let d = t.path().to_path_buf();
            (Some(t), d)
        }
    };
    let mut settings = Settings::default();
    settings.data_dir = dir.clone();
    let fake = FakeGizzi::new();
    let gizzi = serve(fake.router()).await;
    let runner = GizziTurnRunner::connect(&gizzi, &dir);
    let st = SelfHostState::new(store, settings)
        .unwrap()
        .with_setup_token(None)
        .with_replica_id(replica_id)
        .with_turn_runner(runner.clone());
    replica::heartbeat_once(&st).await.unwrap();
    wake::spawn(st.clone()).await;
    let base = serve(router(st.clone())).await;
    *fake.0.server.lock().unwrap() = Some((base.clone(), st.internal_token.to_string()));
    Node { st, base, fake, runner, _tmp: tmp }
}

struct Ids {
    dev: String,
    aek: String,
    env_id: String,
    session_id: String,
}

async fn seed(st: &SelfHostState) -> Ids {
    let dev = new_secret("ask");
    st.store
        .create_api_key(NewApiKey {
            name: "dev".into(),
            role: Role::Developer,
            prefix: dev[..12].into(),
            key_hash: hash_secret(&dev),
            expires_at: None,
        })
        .await
        .unwrap();
    let a = st
        .store
        .create_agent(NewAgent { name: "a".into(), model: "claude-sonnet".into(), ..Default::default() })
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
    let aek = new_secret("aek");
    st.store
        .create_environment_key(NewEnvironmentKey {
            environment_id: e.id.clone(),
            name: None,
            prefix: aek[..12].into(),
            key_hash: hash_secret(&aek),
            expires_at: None,
        })
        .await
        .unwrap();
    Ids { dev, aek, env_id: e.id, session_id: s.id }
}

async fn call(base: &str, path: &str, bearer: Option<&str>, secret: Option<&str>, body: Value) -> (u16, Value) {
    let mut rb = http().post(format!("{base}{path}")).json(&body);
    if let Some(t) = bearer {
        rb = rb.bearer_auth(t).header("x-worker-hostname", "box-1").header("x-worker-version", "test 1.0");
    }
    if let Some(s) = secret {
        rb = rb.header("x-session-secret", s);
    }
    let res = rb.send().await.unwrap();
    let status = res.status().as_u16();
    let bytes = res.bytes().await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn send(n: &Node, ids: &Ids, content: &str) -> (u16, Value) {
    call(&n.base, &format!("/v1/sessions/{}/messages", ids.session_id), Some(&ids.dev), None, json!({ "content": content })).await
}

async fn session(st: &SelfHostState, id: &str) -> SessionRecord {
    st.store.get_session(id).await.unwrap()
}

async fn wait_status(st: &SelfHostState, id: &str, want: &str, within: Duration) {
    let end = Instant::now() + within;
    loop {
        let s = session(st, id).await;
        if s.status == want {
            return;
        }
        assert!(Instant::now() < end, "session never reached {want}; it is {}", s.status);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn events(st: &SelfHostState, id: &str) -> Vec<EventRecord> {
    st.store.list_events(id, 0, 500, None).await.unwrap()
}

fn restarted_errors(evs: &[EventRecord]) -> usize {
    evs.iter()
        .filter(|e| e.event_type == "error" && e.data["code"] == json!(SERVER_RESTARTED))
        .count()
}

/// Run the turn's one bash call as a worker: claim, next, result. Returns
/// the lease (work id, session secret); the worker keeps it.
async fn serve_one_call(n: &Node, ids: &Ids, output: &str) -> (String, String) {
    let (s, c) = call(
        &n.base,
        &format!("/v1/environments/{}/work/claim", ids.env_id),
        Some(&ids.aek),
        None,
        json!({ "worker_id": "w1", "block_ms": 5000 }),
    )
    .await;
    assert_eq!(s, 200, "{c}");
    let work_id = c["work_id"].as_str().unwrap().to_string();
    let secret = c["session_secret"].as_str().unwrap().to_string();
    let (s, job) = call(&n.base, &format!("/v1/work/{work_id}/tool_calls/next"), None, Some(&secret), json!({ "block_ms": 5000 })).await;
    assert_eq!(s, 200, "{job}");
    assert_eq!(job["name"], json!("bash"));
    let call_id = job["tool_call_id"].as_str().unwrap().to_string();
    let (s, _) = call(&n.base, &format!("/v1/tool_calls/{call_id}/result"), None, Some(&secret), json!({ "output": output })).await;
    assert_eq!(s, 200);
    (work_id, secret)
}

/// The next bash call on a lease the worker already holds.
async fn serve_next_call(n: &Node, work_id: &str, secret: &str, output: &str) {
    let (s, job) = call(&n.base, &format!("/v1/work/{work_id}/tool_calls/next"), None, Some(secret), json!({ "block_ms": 5000 })).await;
    assert_eq!(s, 200, "{job}");
    let call_id = job["tool_call_id"].as_str().unwrap().to_string();
    let (s, _) = call(&n.base, &format!("/v1/tool_calls/{call_id}/result"), None, Some(secret), json!({ "output": output })).await;
    assert_eq!(s, 200);
}

/// SSE frames `(seq, event type)` of a session, read in the background.
async fn sse(base: &str, dev: &str, session_id: &str) -> mpsc::UnboundedReceiver<(i64, String)> {
    let res = http()
        .get(format!("{base}/v1/sessions/{session_id}/events/stream"))
        .bearer_auth(dev)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 200);
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut body = res.bytes_stream();
        let mut buf = String::new();
        while let Some(Ok(chunk)) = body.next().await {
            buf.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(end) = buf.find("\n\n") {
                let frame: String = buf.drain(..end + 2).collect();
                let mut id = None;
                let mut kind = String::new();
                for line in frame.lines() {
                    if let Some(v) = line.strip_prefix("id:") {
                        id = v.trim().parse::<i64>().ok();
                    } else if let Some(v) = line.strip_prefix("event:") {
                        kind = v.trim().to_string();
                    }
                }
                if let Some(id) = id {
                    if tx.send((id, kind)).is_err() {
                        return;
                    }
                }
            }
        }
    });
    rx
}

/// Frames until `pred` matches one, within `within`. Panics on timeout.
async fn frames_until(
    rx: &mut mpsc::UnboundedReceiver<(i64, String)>,
    within: Duration,
    pred: impl Fn(&(i64, String)) -> bool,
) -> Vec<(i64, String)> {
    let mut seen = Vec::new();
    let end = tokio::time::Instant::now() + within;
    loop {
        match tokio::time::timeout_at(end, rx.recv()).await {
            Ok(Some(f)) => {
                let hit = pred(&f);
                seen.push(f);
                if hit {
                    return seen;
                }
            }
            Ok(None) => panic!("SSE stream ended; frames so far {seen:?}"),
            Err(_) => panic!("no matching SSE frame within {within:?}; frames so far {seen:?}"),
        }
    }
}

// ─── Two replicas ───────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn two_replicas_live_events_interrupt_and_move() {
    let Some(url) = pg_url() else { return };
    let (sa, sb) = pg_pair(&url).await;
    let a = node(sa, "replica-a", None).await;
    let b = node(sb, "replica-b", None).await;
    let ids = seed(&a.st).await;

    // replicas = 2, from either replica.
    for n in [&a, &b] {
        let info: Value = http().get(format!("{}/v1/server/info", n.base)).send().await.unwrap().json().await.unwrap();
        assert_eq!(info["replicas"], json!(2), "{info}");
    }

    // A client on B watches; the turn runs on A.
    let mut live = sse(&b.base, &ids.dev, &ids.session_id).await;
    let started = Instant::now();
    let (s, msg) = send(&a, &ids, "run bash").await;
    assert_eq!(s, 202, "{msg}");
    let user_seq = msg["seq"].as_i64().unwrap();
    let first = frames_until(&mut live, Duration::from_secs(2), |f| f.1 == "status" && f.0 > user_seq).await;
    assert!(started.elapsed() < Duration::from_secs(2), "live events on B took {:?}", started.elapsed());
    assert!(first.contains(&(user_seq, "message".to_string())), "B saw A's user message: {first:?}");
    let s = session(&a.st, &ids.session_id).await;
    assert_eq!(s.status, "waiting_for_worker");
    assert_eq!(s.turn_replica.as_deref(), Some("replica-a"));
    assert_eq!(s.gizzi_replica.as_deref(), Some("replica-a"));
    assert_eq!(a.runner_turns(), 1);

    // Interrupt on B stops the turn on A within 5 s.
    let stop = Instant::now();
    let (s, body) = call(&b.base, &format!("/v1/sessions/{}/interrupt", ids.session_id), Some(&ids.dev), None, json!({})).await;
    assert_eq!(s, 200, "{body}");
    assert_eq!(body["status"], json!("interrupted"));
    while a.runner_turns() > 0 {
        assert!(stop.elapsed() < Duration::from_secs(5), "A still runs the turn {:?} after the interrupt", stop.elapsed());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let s = session(&a.st, &ids.session_id).await;
    assert_eq!(s.status, "interrupted", "A's lost turn left the status alone");
    assert_eq!(s.turn_replica, None);
    let second = frames_until(&mut live, Duration::from_secs(6), |f| f.1 == "status").await;

    // The next message, on B: B's child never had the session, so it is
    // rebuilt from the events and now belongs to B.
    let (s, _) = send(&b, &ids, "hi").await;
    assert_eq!(s, 202);
    wait_status(&b.st, &ids.session_id, "idle", Duration::from_secs(10)).await;
    assert_eq!(b.fake.0.imports.lock().unwrap().len(), 1, "rehydrated on B");
    let s = session(&b.st, &ids.session_id).await;
    assert_eq!(s.gizzi_replica.as_deref(), Some("replica-b"));
    assert_eq!(s.turn_replica, None, "released at turn end");

    // No duplicates, no gaps, in what B streamed.
    let all = events(&b.st, &ids.session_id).await;
    let last = all.last().unwrap().seq;
    let rest = frames_until(&mut live, Duration::from_secs(6), |f| f.0 == last).await;
    let streamed: Vec<i64> = first.iter().chain(&second).chain(&rest).map(|f| f.0).collect();
    let stored: Vec<i64> = all.iter().map(|e| e.seq).collect();
    assert_eq!(streamed, stored, "SSE on B = the stored events, in order, once");
    assert!(second.iter().any(|f| f.1 == "status"), "{second:?}");
}

impl Node {
    fn runner_turns(&self) -> usize {
        use allternit_api::selfhost::routes_sessions::TurnRunner;
        self.runner.running_turns()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn dead_replica_turn_fails_once_with_two_keepers() {
    let Some(url) = pg_url() else { return };
    let (sa, sb) = pg_pair(&url).await;
    let b = node(sa, "replica-b", None).await;
    let c = node(sb, "replica-c", None).await;
    let ids = seed(&b.st).await;

    // Replica A ran a turn, then went silent 60 s ago.
    let st = &b.st;
    st.store
        .replica_heartbeat("replica-a", Utc::now() - chrono::Duration::minutes(5), None, Utc::now() - chrono::Duration::seconds(60))
        .await
        .unwrap();
    st.store
        .transition_session_status(&ids.session_id, &["idle"], "running", None)
        .await
        .unwrap()
        .expect("running");
    assert!(st.store.claim_turn(&ids.session_id, "replica-a").await.unwrap());
    let mut live = sse(&c.base, &ids.dev, &ids.session_id).await;

    // Two keepers at once: the session is failed exactly once.
    let (fb, fc) = tokio::join!(replica::keeper_once(&b.st), replica::keeper_once(&c.st));
    assert_eq!(fb.unwrap().len() + fc.unwrap().len(), 1);
    // And again later: nothing more.
    assert!(replica::keeper_once(&b.st).await.unwrap().is_empty());

    let s = session(st, &ids.session_id).await;
    assert_eq!(s.status, "error");
    assert_eq!(s.status_reason.as_deref(), Some(SERVER_RESTARTED));
    assert_eq!(s.turn_replica, None);
    assert_eq!(restarted_errors(&events(st, &ids.session_id).await), 1);
    let ids_left: Vec<String> = st.store.list_replicas().await.unwrap().into_iter().map(|r| r.replica_id).collect();
    assert!(!ids_left.contains(&"replica-a".to_string()), "{ids_left:?}");
    assert_eq!(ids_left.len(), 2);
    // A watcher on another replica sees it live.
    frames_until(&mut live, Duration::from_secs(2), |f| f.1 == "error").await;

    // A live replica's turn is left alone.
    let ids2 = seed(&b.st).await;
    st.store.transition_session_status(&ids2.session_id, &["idle"], "running", None).await.unwrap();
    assert!(st.store.claim_turn(&ids2.session_id, "replica-b").await.unwrap());
    assert!(replica::keeper_once(&c.st).await.unwrap().is_empty());
    assert_eq!(session(st, &ids2.session_id).await.status, "running");
}

// ─── Restart and drain (SQLite and PostgreSQL) ──────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sqlite_restart_mid_turn_fails_the_turn_once_at_boot() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("agents-server.db");
    let id1 = replica::load_or_create_id(tmp.path()).unwrap();
    let first = node(Arc::new(SqliteStore::open(&db).unwrap()), &id1, Some(tmp.path())).await;
    let ids = seed(&first.st).await;
    let (s, _) = send(&first, &ids, "run bash").await;
    assert_eq!(s, 202);
    wait_status(&first.st, &ids.session_id, "waiting_for_worker", Duration::from_secs(5)).await;

    // The process dies; a new one starts on the same data dir.
    let id2 = replica::load_or_create_id(tmp.path()).unwrap();
    assert_eq!(id1, id2, "the replica id survives a restart");
    let mut settings = Settings::default();
    settings.data_dir = tmp.path().to_path_buf();
    let st = SelfHostState::new(Arc::new(SqliteStore::open(&db).unwrap()), settings)
        .unwrap()
        .with_replica_id(id2);
    let failed = replica::recover_at_boot(&st).await.unwrap();
    assert_eq!(failed.len(), 1);
    assert!(replica::recover_at_boot(&st).await.unwrap().is_empty(), "exactly once");

    let s = session(&st, &ids.session_id).await;
    assert_eq!(s.status, "error");
    assert_eq!(s.status_reason.as_deref(), Some(SERVER_RESTARTED));
    let evs = events(&st, &ids.session_id).await;
    assert_eq!(restarted_errors(&evs), 1, "{evs:?}");
    // The lost turn's open tool call was closed too.
    assert!(!st.store.has_open_tool_calls(&ids.session_id).await.unwrap());
}

async fn drain_case(store: DynStore) {
    let n = node(store, "replica-d", None).await;
    let ids = seed(&n.st).await;
    let (s, _) = send(&n, &ids, "run bash").await;
    assert_eq!(s, 202);
    wait_status(&n.st, &ids.session_id, "waiting_for_worker", Duration::from_secs(5)).await;

    let started = Instant::now();
    let failed = replica::drain(&n.st, Duration::from_millis(600)).await;
    assert!(started.elapsed() >= Duration::from_millis(600), "waited the grace");
    assert_eq!(failed.len(), 1);
    assert_eq!(n.runner_turns(), 0);
    let s = session(&n.st, &ids.session_id).await;
    assert_eq!(s.status, "error");
    assert_eq!(restarted_errors(&events(&n.st, &ids.session_id).await), 1);
    let rows = n.st.store.list_replicas().await.unwrap();
    assert!(rows.iter().all(|r| r.replica_id != "replica-d"), "row removed: {rows:?}");

    // A draining replica takes no new turn and is not ready.
    let (s, body) = send(&n, &ids, "hi").await;
    assert_eq!(s, 503, "{body}");
    let ready = http().get(format!("{}/health/ready", n.base)).send().await.unwrap();
    assert_eq!(ready.status().as_u16(), 503);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn graceful_drain_fails_remaining_turns_after_the_grace() {
    drain_case(Arc::new(SqliteStore::open_memory().unwrap())).await;
    if let Some(url) = pg_url() {
        drain_case(pg_pair(&url).await.0).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drain_lets_a_turn_that_finishes_in_time_finish() {
    let n = node(Arc::new(SqliteStore::open_memory().unwrap()), "replica-e", None).await;
    let ids = seed(&n.st).await;
    let (s, _) = send(&n, &ids, "run bash").await;
    assert_eq!(s, 202);
    wait_status(&n.st, &ids.session_id, "waiting_for_worker", Duration::from_secs(5)).await;
    let drain = tokio::spawn({
        let st = n.st.clone();
        async move { replica::drain(&st, Duration::from_secs(20)).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    // Workers are still served during the drain.
    serve_one_call(&n, &ids, "hi\n").await;
    let failed = drain.await.unwrap();
    assert!(failed.is_empty());
    assert_eq!(session(&n.st, &ids.session_id).await.status, "idle");
    assert_eq!(restarted_errors(&events(&n.st, &ids.session_id).await), 0);
}

// ─── Context handoff (G2b) ──────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tool_call_after_a_handoff_reaches_the_queue() {
    let n = node(Arc::new(SqliteStore::open_memory().unwrap()), "replica-h", None).await;
    let ids = seed(&n.st).await;
    let (s, _) = send(&n, &ids, "hi").await;
    assert_eq!(s, 202);
    wait_status(&n.st, &ids.session_id, "idle", Duration::from_secs(5)).await;
    let root = session(&n.st, &ids.session_id).await.gizzi_session_id.unwrap();

    // gizzi hands the session off while it takes the next prompt (a model
    // switch): the turn runs on a new lineage head and its bash call names
    // the head, which the server never saw.
    n.fake.0.handoff_next.store(true, Ordering::SeqCst);
    let (s, _) = send(&n, &ids, "run bash").await;
    assert_eq!(s, 202);
    let (work_id, secret) = serve_one_call(&n, &ids, "hi\n").await;
    wait_status(&n.st, &ids.session_id, "idle", Duration::from_secs(10)).await;
    let evs = events(&n.st, &ids.session_id).await;
    let last = evs.iter().rev().find(|e| e.event_type == "message").unwrap();
    assert_eq!(last.data["content"], json!("ran: hi\n"), "{evs:?}");
    let head = session(&n.st, &ids.session_id).await.gizzi_session_id.unwrap();
    assert_ne!(head, root, "the head is the session from now on");

    // The next turn goes straight to the head.
    let (s, _) = send(&n, &ids, "run bash again").await;
    assert_eq!(s, 202);
    serve_next_call(&n, &work_id, &secret, "again\n").await;
    wait_status(&n.st, &ids.session_id, "idle", Duration::from_secs(10)).await;
    assert_eq!(n.fake.prompted().last().unwrap(), &head);
}

// ─── gizzi stand-in ─────────────────────────────────────────────────────────

#[derive(Clone)]
struct FakeGizzi(Arc<FakeInner>);

#[derive(Default, Clone)]
struct FakeSession {
    messages: Vec<Value>,
    continues_from: Option<String>,
    handoff: Option<String>,
}

struct FakeInner {
    bus: broadcast::Sender<Value>,
    sessions: Mutex<HashMap<String, FakeSession>>,
    imports: Mutex<Vec<Value>>,
    /// Session id each prompt ran on.
    prompted: Mutex<Vec<String>>,
    next: AtomicU64,
    /// Hand the next prompted session off to a new head first.
    handoff_next: AtomicBool,
    server: Mutex<Option<(String, String)>>,
}

impl FakeGizzi {
    fn new() -> Self {
        let (bus, _) = broadcast::channel(256);
        Self(Arc::new(FakeInner {
            bus,
            sessions: Mutex::new(HashMap::new()),
            imports: Mutex::new(Vec::new()),
            prompted: Mutex::new(Vec::new()),
            next: AtomicU64::new(1),
            handoff_next: AtomicBool::new(false),
            server: Mutex::new(None),
        }))
    }

    fn id(&self, prefix: &str) -> String {
        format!("{prefix}_{}", self.0.next.fetch_add(1, Ordering::SeqCst))
    }

    fn prompted(&self) -> Vec<String> {
        self.0.prompted.lock().unwrap().clone()
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
    f.0.sessions.lock().unwrap().insert(id.clone(), FakeSession::default());
    Json(json!({ "id": id }))
}

async fn fake_import(State(f): State<FakeGizzi>, Json(body): Json<Value>) -> Json<Value> {
    let id = f.id("ses");
    let messages = body["messages"].as_array().cloned().unwrap_or_default();
    f.0.sessions.lock().unwrap().insert(id.clone(), FakeSession { messages, ..Default::default() });
    f.0.imports.lock().unwrap().push(body);
    Json(json!({ "id": id }))
}

async fn fake_get(State(f): State<FakeGizzi>, Path(id): Path<String>) -> axum::response::Response {
    use axum::response::IntoResponse;
    match f.0.sessions.lock().unwrap().get(&id) {
        Some(s) => {
            let mut v = json!({ "id": id });
            if let Some(c) = &s.continues_from {
                v["continuesFrom"] = json!(c);
            }
            if let Some(h) = &s.handoff {
                v["handoff"] = json!({ "sessionID": h, "reason": "model_switch" });
            }
            Json(v).into_response()
        }
        None => (axum::http::StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response(),
    }
}

async fn fake_messages(State(f): State<FakeGizzi>, Path(id): Path<String>) -> Json<Value> {
    Json(Value::Array(
        f.0.sessions.lock().unwrap().get(&id).map(|s| s.messages.clone()).unwrap_or_default(),
    ))
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

/// A turn, like gizzi's: follows handoff links to the head (and with
/// `handoff_next` makes a new head first), then answers; "bash" in the text
/// runs one bash call through the server's internal route under the id of
/// the session the turn runs on.
async fn fake_prompt(
    State(f): State<FakeGizzi>,
    Path(asked): Path<String>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let sid = {
        let mut sessions = f.0.sessions.lock().unwrap();
        let Some(mut cur) = sessions.get(&asked).map(|_| asked.clone()) else {
            return (axum::http::StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
        };
        while let Some(h) = sessions.get(&cur).and_then(|s| s.handoff.clone()) {
            cur = h;
        }
        if f.0.handoff_next.swap(false, Ordering::SeqCst) {
            let head = f.id("ses_head");
            sessions.insert(head.clone(), FakeSession { continues_from: Some(cur.clone()), ..Default::default() });
            sessions.get_mut(&cur).unwrap().handoff = Some(head.clone());
            cur = head;
        }
        cur
    };
    f.0.prompted.lock().unwrap().push(sid.clone());
    let earlier = f.0.sessions.lock().unwrap().get(&sid).map(|s| s.messages.len()).unwrap_or(0);
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
        let res = http()
            .post(format!("{server}/internal/v1/sessions/{sid}/tool_calls"))
            .bearer_auth(token)
            .json(&json!({ "tool_call_id": call_id, "name": "bash", "input": input, "timeout_ms": 30000 }))
            .send()
            .await
            .unwrap();
        let ok = res.status().is_success();
        let result: Value = res.json().await.unwrap_or(Value::Null);
        let output = if ok {
            result["output"].as_str().unwrap_or_default().to_string()
        } else {
            format!("tool host error: {result}")
        };
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
    if let Some(s) = f.0.sessions.lock().unwrap().get_mut(&sid) {
        s.messages.push(user);
        s.messages.push(assistant.clone());
    }
    Json(assistant).into_response()
}
