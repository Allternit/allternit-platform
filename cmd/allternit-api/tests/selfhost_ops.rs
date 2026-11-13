//! Self-hosted operations: metrics listener, traces, secrets and the admin
//! CLI (plan steps 13 to 15).
//!
//! The keyring's legacy and previous-key cases are unit tests in
//! `src/token_crypto.rs` (`keyring_tests`).

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use allternit_api::selfhost::admin;
use allternit_api::selfhost::metrics_listener;
use allternit_api::selfhost::secrets::{put_sealed_credential, rotate_credentials};
use allternit_api::selfhost::settings::{Settings, SettingsError};
use allternit_api::selfhost::store::sqlite::SqliteStore;
use allternit_api::selfhost::store::*;
use allternit_api::selfhost::telemetry::{self, TurnOutcome};
use allternit_api::selfhost::{router, SelfHostState};
use allternit_api::token_crypto::{key_id_for, seal_v1_with, Keyring};
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde_json::{json, Value};
use tower::ServiceExt;

fn sqlite() -> DynStore {
    Arc::new(SqliteStore::open_memory().expect("open sqlite"))
}

async fn body_text(res: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn agent_env_session(store: &DynStore) -> (EnvironmentRecord, SessionRecord) {
    let a = store
        .create_agent(NewAgent {
            name: "a".into(),
            model: "claude-sonnet".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let e = store
        .create_environment(NewEnvironment {
            name: "e".into(),
            network: "none".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let s = store
        .create_session(NewSession {
            agent_id: a.id,
            environment_id: e.id.clone(),
            ..Default::default()
        })
        .await
        .unwrap();
    (e, s)
}

// ─── Step 13: metrics and traces ────────────────────────────────────────────

#[tokio::test]
async fn main_router_does_not_serve_metrics() {
    let state = SelfHostState::new(sqlite(), Settings::default()).unwrap();
    let res = router(state)
        .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn metrics_listener_reports_store_and_egress() {
    let store = sqlite();
    let (env, s) = agent_env_session(&store).await;
    store.enqueue_work(&env.id, &s.id).await.unwrap();
    let state = SelfHostState::new(store, Settings::default()).unwrap();
    let refused = url::Url::parse("https://evil.example/x").unwrap();
    assert!(state.egress.guard().check_url(&refused).is_err());

    let res = metrics_listener::metrics_router(state)
        .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let text = body_text(res).await;
    assert!(text.contains("allternit_selfhost_sessions{status=\"idle\"} 1"), "{text}");
    assert!(text.contains("allternit_selfhost_sessions{status=\"archived\"} 0"), "{text}");
    assert!(
        text.contains(&format!("allternit_selfhost_queue_depth{{environment=\"{}\"}} 1", env.id)),
        "{text}"
    );
    assert!(
        text.contains(&format!("allternit_selfhost_workers_online{{environment=\"{}\"}} 0", env.id)),
        "{text}"
    );
    assert!(
        text.contains("allternit_selfhost_egress_refusals_total{host=\"evil.example\"} 1"),
        "{text}"
    );
    assert!(
        text.contains("allternit_selfhost_egress_requests_total{host=\"evil.example\",result=\"refused\"} 1"),
        "{text}"
    );
    assert!(text.contains("allternit_selfhost_store_up 1"), "{text}");
}

#[tokio::test]
async fn one_trace_runs_session_turn_tool_call_and_worker_result() {
    let store = sqlite();
    let (env, s) = agent_env_session(&store).await;

    let turn = telemetry::start_turn(&store, &s.id, "claude-sonnet").await;
    let root = store
        .get_traceparent(TraceTarget::Session, &s.id)
        .await
        .unwrap()
        .expect("session root trace stored");
    let root = allternit_api::netops::hops::parse(&root).unwrap();
    assert_eq!(turn.traceparent().trace_id, root.trace_id);
    assert_ne!(turn.traceparent().span_id, root.span_id);

    // A second turn joins the same session trace.
    let again = telemetry::session_trace(&store, &s.id).await.unwrap();
    assert_eq!(again, root);

    store
        .insert_tool_call(NewToolCall {
            id: "toolu_trace_1".into(),
            session_id: s.id.clone(),
            name: "bash_trace_test".into(),
            input: json!({"command": "echo ok"}),
        })
        .await
        .unwrap();
    let tc = telemetry::start_tool_call(&store, &turn.traced, &s.id, "toolu_trace_1", "bash_trace_test").await;
    assert_eq!(
        store
            .get_traceparent(TraceTarget::ToolCall, "toolu_trace_1")
            .await
            .unwrap(),
        Some(tc.header())
    );

    let work = store.enqueue_work(&env.id, &s.id).await.unwrap();
    telemetry::record_work_enqueued(&store, &work.id, &turn.traced).await;
    let claim = store
        .claim_work(ClaimRequest {
            environment_id: env.id.clone(),
            worker_id: "w1".into(),
            reclaim_older_than_ms: None,
            hostname: None,
            version: None,
            now: Utc::now(),
        })
        .await
        .unwrap()
        .expect("claim");
    let claimed = telemetry::work_claim_span(&store, &claim.item, "w1").await;
    assert_eq!(claimed.traceparent.trace_id, root.trace_id);

    store
        .next_tool_call(&claim.item.id, claim.item.lease_generation)
        .await
        .unwrap()
        .expect("tool call handed out");
    let out = store.submit_tool_result("toolu_trace_1", "ok\n", false).await.unwrap();
    assert!(out.stored);
    let result = telemetry::work_result_span(&store, &out.call).await;
    assert_eq!(result.traceparent.trace_id, root.trace_id);
    assert_eq!(tc.traceparent.trace_id, root.trace_id);
    turn.finish(TurnOutcome::Completed);

    let state = SelfHostState::new(store, Settings::default()).unwrap();
    let text = metrics_listener::render(&state).await.unwrap();
    assert!(
        text.contains("allternit_selfhost_tool_calls_total{name=\"bash_trace_test\",outcome=\"success\"} 1"),
        "{text}"
    );
    assert!(text.contains("allternit_selfhost_turns_total{outcome=\"completed\"}"), "{text}");
}

#[tokio::test]
async fn malformed_traceparent_is_refused() {
    let store = sqlite();
    let (_, s) = agent_env_session(&store).await;
    let err = store
        .set_traceparent(TraceTarget::Session, &s.id, "not-a-trace")
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)), "{err}");
    let err = store
        .set_traceparent(
            TraceTarget::Session,
            "sess_missing",
            "00-0123456789abcdef0123456789abcdef-0123456789abcdef-01",
        )
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::NotFound), "{err}");
}

// ─── Step 14: secrets ───────────────────────────────────────────────────────

/// A tiny Vault: KV v2 reads at /v1/secret/data/agents/db and AppRole
/// login. The first login hands out a token the read rejects, so the client
/// must log in again.
async fn mock_vault() -> (String, Arc<AtomicUsize>) {
    let logins = Arc::new(AtomicUsize::new(0));
    let l = logins.clone();
    let app = Router::new()
        .route(
            "/v1/secret/data/agents/db",
            get(|headers: HeaderMap| async move {
                if headers.get("x-vault-token").and_then(|v| v.to_str().ok()) != Some("tok-good") {
                    return (StatusCode::FORBIDDEN, Json(json!({"errors": ["permission denied"]})));
                }
                (
                    StatusCode::OK,
                    Json(json!({"data": {"data": {"password": "pg-pass", "port": 5432}, "metadata": {"version": 3}}})),
                )
            }),
        )
        .route(
            "/v1/auth/approle/login",
            post(move |Json(body): Json<Value>| {
                let l = l.clone();
                async move {
                    if body["role_id"] != "role-1" || body["secret_id"] != "sid-1" {
                        return (StatusCode::BAD_REQUEST, Json(json!({"errors": ["invalid role or secret ID"]})));
                    }
                    let n = l.fetch_add(1, Ordering::SeqCst);
                    let token = if n == 0 { "tok-expired" } else { "tok-good" };
                    (StatusCode::OK, Json(json!({"auth": {"client_token": token}})))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), logins)
}

fn write(dir: &Path, name: &str, text: &str) -> String {
    let p = dir.join(name);
    std::fs::write(&p, text).unwrap();
    p.display().to_string()
}

#[tokio::test]
async fn vault_token_auth_reads_kv_v2() {
    let (addr, _) = mock_vault().await;
    let dir = tempfile::tempdir().unwrap();
    let token = write(dir.path(), "vault-token", "tok-good\n");
    let s = Settings::from_str(&format!(
        "inference: {{provider: anthropic}}\nsecrets:\n  backend: vault\n  vault:\n    address: '{addr}'\n    mount: secret\n    path: agents\n    auth: {{method: token, token: 'file:{token}'}}\n"
    ))
    .unwrap();
    assert_eq!(s.secret("vault:db#password").await.unwrap(), "pg-pass");
    // A non-string field comes back as its JSON text.
    assert_eq!(s.secret("vault:db#port").await.unwrap(), "5432");
    let missing_field = s.secret("vault:db#nope").await.unwrap_err();
    assert!(matches!(missing_field, SettingsError::Vault { .. }), "{missing_field}");
    let missing_path = s.secret("vault:other#password").await.unwrap_err();
    assert!(matches!(missing_path, SettingsError::Vault { .. }), "{missing_path}");
}

#[tokio::test]
async fn vault_approle_logs_in_again_after_a_403() {
    let (addr, logins) = mock_vault().await;
    let dir = tempfile::tempdir().unwrap();
    let sid = write(dir.path(), "secret-id", "sid-1");
    let s = Settings::from_str(&format!(
        "inference: {{provider: anthropic}}\ndatabase: {{url: 'vault:db#password'}}\nsecrets:\n  backend: vault\n  vault:\n    address: '{addr}'\n    path: agents\n    auth: {{method: approle, role_id: role-1, secret_id: 'file:{sid}'}}\n"
    ))
    .unwrap();
    assert_eq!(s.database_url_resolved().await.unwrap().as_deref(), Some("pg-pass"));
    assert_eq!(logins.load(Ordering::SeqCst), 2, "expired token, then a fresh login");
}

// ─── Step 15: admin CLI ─────────────────────────────────────────────────────

fn config(dir: &Path, extra: &str) -> String {
    write(
        dir,
        "settings.yaml",
        &format!(
            "data_dir: '{}'\ninference: {{provider: anthropic}}\n{extra}",
            dir.display()
        ),
    )
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[tokio::test]
async fn admin_migrate_twice_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(dir.path(), "");
    assert_eq!(admin::run(args(&["migrate", "--config", &cfg])).await, 0);
    assert_eq!(admin::run(args(&["migrate", "--config", &cfg])).await, 0);

    // The same on PostgreSQL when a test database is given.
    if let Some(url) = std::env::var("SELFHOST_TEST_PG_URL").ok().filter(|u| !u.is_empty()) {
        let pg_dir = tempfile::tempdir().unwrap();
        let cfg = config(pg_dir.path(), &format!("database: {{url: '{url}'}}\n"));
        assert_eq!(admin::run(args(&["migrate", "--config", &cfg])).await, 0);
        assert_eq!(admin::run(args(&["migrate", "--config", &cfg])).await, 0);
    }
}

#[tokio::test]
async fn admin_setup_mints_once_unless_forced() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(dir.path(), "");
    assert_eq!(admin::run(args(&["setup", "--config", &cfg])).await, 0);
    assert_eq!(admin::run(args(&["setup", "--config", &cfg])).await, 1, "a key exists");
    assert_eq!(
        admin::run(args(&["setup", "--force-new-admin", "--config", &cfg])).await,
        0
    );
    assert_eq!(admin::run(args(&["setup", "--bogus", "--config", &cfg])).await, 2);
    let store = SqliteStore::open(&dir.path().join("agents-server.db")).unwrap();
    let keys = store.list_api_keys(10, None).await.unwrap();
    assert_eq!(keys.len(), 2);
    assert!(keys.iter().all(|k| k.role == Role::Admin));
}

#[tokio::test]
async fn admin_rotate_key_reseals_with_the_current_key() {
    let dir = tempfile::tempdir().unwrap();
    let old = write(dir.path(), "old.key", "old-passphrase\n");
    let new = write(dir.path(), "new.key", "new-passphrase\n");
    let db = dir.path().join("agents-server.db");

    // Before the rotation: one row under the old key, one legacy v1 row, and
    // one under a key nobody has any more.
    let orphan_sealed;
    {
        let store: DynStore = Arc::new(SqliteStore::open(&db).unwrap());
        let old_ring = Keyring::new("old-passphrase", &[] as &[&str]).unwrap();
        put_sealed_credential(&store, &old_ring, "github", "gh-token").await.unwrap();
        let v1 = seal_v1_with("old-passphrase", "aws-secret").unwrap();
        store.put_credential("aws", v1.as_bytes(), "legacy").await.unwrap();
        let lost = Keyring::new("lost-passphrase", &[] as &[&str]).unwrap();
        orphan_sealed = lost.seal("gone").unwrap();
        store
            .put_credential("orphan", orphan_sealed.as_bytes(), lost.current_key_id())
            .await
            .unwrap();
    }

    let cfg = config(
        dir.path(),
        &format!("secrets: {{encryption_key_ref: 'file:{new}', previous_key_refs: ['file:{old}']}}\n"),
    );
    // The orphan cannot be opened, so the run fails, but it re-seals the rest
    // and leaves the orphan exactly as it was.
    assert_eq!(admin::run(args(&["rotate-key", "--config", &cfg])).await, 1);

    let store: DynStore = Arc::new(SqliteStore::open(&db).unwrap());
    let new_only = Keyring::new("new-passphrase", &[] as &[&str]).unwrap();
    for (name, plain) in [("github", "gh-token"), ("aws", "aws-secret")] {
        let rec = store.get_credential(name).await.unwrap();
        assert_eq!(rec.key_id, key_id_for("new-passphrase"), "{name}");
        let sealed = std::str::from_utf8(&rec.sealed).unwrap();
        assert_eq!(new_only.open(sealed).unwrap(), plain, "{name}");
    }
    assert_eq!(store.get_credential("orphan").await.unwrap().sealed, orphan_sealed.as_bytes());

    store.delete_credential("orphan").await.unwrap();
    drop(store);
    assert_eq!(admin::run(args(&["rotate-key", "--config", &cfg])).await, 0);
    let store: DynStore = Arc::new(SqliteStore::open(&db).unwrap());
    let ring = Keyring::new("new-passphrase", &["old-passphrase"]).unwrap();
    let report = rotate_credentials(&store, &ring).await.unwrap();
    assert!(report.rotated.is_empty() && report.failed.is_empty());
    assert_eq!(report.unchanged, 2);
}

#[tokio::test]
async fn admin_keys_need_setup_and_a_known_role() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(dir.path(), "");
    assert_eq!(
        admin::run(args(&["keys", "create", "--role", "viewer", "--config", &cfg])).await,
        1,
        "no admin key yet"
    );
    assert_eq!(admin::run(args(&["setup", "--config", &cfg])).await, 0);
    assert_eq!(
        admin::run(args(&["keys", "create", "--role", "owner", "--config", &cfg])).await,
        2
    );
    assert_eq!(
        admin::run(args(&["keys", "create", "--role", "viewer", "--config", &cfg])).await,
        0
    );
    let store = SqliteStore::open(&dir.path().join("agents-server.db")).unwrap();
    let admin_key = store
        .list_api_keys(10, None)
        .await
        .unwrap()
        .into_iter()
        .find(|k| k.role == Role::Admin)
        .unwrap();
    drop(store);
    assert_eq!(
        admin::run(args(&["keys", "revoke", &admin_key.id, "--config", &cfg])).await,
        1,
        "the last admin key cannot be revoked"
    );
}
