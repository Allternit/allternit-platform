//! Files drive step 13: browsing a computer through the relay never wakes it
//! and never resets its idle timer; a sleeping computer answers metadata from
//! its asleep snapshot (SPEC.md sections 9, 11). DB-backed, like the other
//! relay tests (`routes::test_support`); R2 is the only thing faked.

use super::*;
use crate::routes::computer_files::{self as cf, mint_wake_ticket, wake_key};
use crate::routes::computer_files_snapshot as snap;
use crate::routes::test_support::{seed_runtime_device, test_state, MockGateway, DEV_USER};
use crate::services::r2::{ObjectStore, R2Error};
use allternit_files_core::{WakeConsentRequest, WakeReason};
use std::sync::atomic::Ordering;

const OLD_ACTIVITY: &str = "2026-01-01T00:00:00Z";

/// Every call of `ProvisioningService::wake_for_device` so far.
fn wake_for_device_calls() -> usize {
    crate::services::provisioning::WAKE_FOR_DEVICE_CALLS.load(Ordering::SeqCst)
}

/// A runtime device with Files capability, backed by a provisioned (credit)
/// computer in `status`.
async fn computer(state: &ApiState, runtime: &str, owner: &str, status: &str) {
    seed_runtime_device(&state.db, runtime, owner).await;
    sqlx::query("UPDATE runtime_devices SET capabilities = $2 WHERE id = $1")
        .bind(runtime)
        .bind(r#"["runtime:connect","runtime:files","runtime:execute"]"#)
        .execute(&state.db)
        .await
        .unwrap();
    for ddl in [
        "CREATE TABLE IF NOT EXISTS provisioned_instances (id TEXT PRIMARY KEY, user_id TEXT NOT NULL, device_id TEXT, status TEXT NOT NULL, tier TEXT, last_activity_at TIMESTAMPTZ, last_owner_activity_at TIMESTAMPTZ, last_started_at TIMESTAMPTZ, created_at TIMESTAMPTZ NOT NULL DEFAULT NOW())",
        "CREATE TABLE IF NOT EXISTS hosted_runtime_instances (id TEXT, user_id TEXT, provider TEXT, runtime_device_id TEXT, status TEXT, last_activity_at TIMESTAMPTZ)",
    ] {
        sqlx::query(ddl).execute(&state.db).await.unwrap();
    }
    sqlx::query("INSERT INTO provisioned_instances (id, user_id, device_id, status, tier, last_activity_at, last_owner_activity_at) VALUES ($1, $2, $3, $4, 'credit', $5::timestamptz, $5::timestamptz)")
        .bind(format!("pi_{runtime}"))
        .bind(owner)
        .bind(runtime)
        .bind(status)
        .bind(OLD_ACTIVITY)
        .execute(&state.db)
        .await
        .unwrap();
}

async fn activity(state: &ApiState, runtime: &str) -> (String, chrono::DateTime<chrono::Utc>) {
    sqlx::query_as("SELECT status, last_activity_at FROM provisioned_instances WHERE device_id = $1")
        .bind(runtime)
        .fetch_one(&state.db)
        .await
        .unwrap()
}

fn request(method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> RelayRequest {
    RelayRequest {
        method: method.to_string(),
        path: path.to_string(),
        headers: headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        body: body.to_string(),
        body_encoding: "utf8".to_string(),
    }
}

fn get(path: &str) -> RelayRequest {
    request("GET", path, &[], "")
}

async fn relay(state: &ApiState, user: &str, runtime: &str, req: RelayRequest) -> (StatusCode, serde_json::Value) {
    let response = relay_request_to_runtime(
        &state.db,
        &state.contabo_runtime_service,
        &state.quota_service,
        &state.provisioning_service,
        user,
        runtime,
        req,
    )
    .await
    .map(IntoResponse::into_response)
    .unwrap_or_else(IntoResponse::into_response);
    let status = response.status();
    let bytes = http_body_util::BodyExt::collect(response.into_body()).await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
}

/// Answer every relayed request on a test connection with 200 `{}`.
fn auto_answer(connection: Arc<RuntimeConnection>, mut outgoing: mpsc::UnboundedReceiver<CloudMessage>) {
    tokio::spawn(async move {
        while let Some(message) = outgoing.recv().await {
            if let CloudMessage::Request { request_id, .. } = message {
                answer_test_request(&connection, &request_id, 200, "{}").await;
            }
        }
    });
}

/// R2, faked: an in-memory bucket (the edge, not our code).
#[derive(Default)]
struct MemoryStore {
    objects: std::sync::Mutex<HashMap<String, Vec<u8>>>,
}

#[async_trait::async_trait]
impl ObjectStore for MemoryStore {
    fn presign_get(&self, _b: &str, _k: &str, _t: Duration) -> Result<String, R2Error> {
        Err(R2Error::Unavailable)
    }
    async fn head(&self, _b: &str, k: &str) -> Result<Option<u64>, R2Error> {
        Ok(self.objects.lock().unwrap().get(k).map(|v| v.len() as u64))
    }
    async fn put(&self, _b: &str, k: &str, bytes: Vec<u8>, _c: &str) -> Result<(), R2Error> {
        self.objects.lock().unwrap().insert(k.to_string(), bytes);
        Ok(())
    }
    async fn get(&self, _b: &str, k: &str) -> Result<Option<Vec<u8>>, R2Error> {
        Ok(self.objects.lock().unwrap().get(k).cloned())
    }
}

fn policy(protected: bool) -> serde_json::Value {
    serde_json::json!({
        "read": true, "write": true, "shareable": !protected, "agent_process": false, "protected": protected,
        "offline": true, "replica": true, "versions": {"keep_count": 20, "keep_days": 30}, "trash_days": 30
    })
}

/// The snapshot a sleeping `runtime` left in R2: a home root with a folder
/// and a file, plus a protected Memory Drive root that must never show.
async fn seed_snapshot(store: &MemoryStore, runtime: &str) {
    let manifest = serde_json::json!({
        "schema": "files-snapshot/1", "computer_id": runtime, "written_at": "2026-11-15T03:12:00Z",
        "encryption": "none", "names_encrypted": false,
        "roots": [
            {"id": "rt_home", "kind": "allternit_home", "name": "Allternit", "policy": policy(false),
             "root_item_id": "it_home0000", "cursor": "c1.rt_home.000412", "provider_ready": true},
            {"id": "rt_mem", "kind": "memory_drive", "name": "Memory Drive", "policy": policy(true),
             "root_item_id": "it_mem0000", "cursor": "c1.rt_mem.000001", "provider_ready": true}
        ]
    });
    let root = serde_json::json!({
        "schema": "files-snapshot-root/1", "root_id": "rt_home", "cursor": "c1.rt_home.000412",
        "written_at": "2026-11-15T03:12:00Z", "encryption": "none", "names_encrypted": false,
        "items": [
            {"id": "it_docs", "parent_id": "it_home0000", "name": "Docs", "kind": "folder", "size": 0, "rev": "3-dir", "modified_at": "2026-11-15T14:00:00Z"},
            {"id": "it_lease", "parent_id": "it_docs", "name": "Lease.pdf", "kind": "file", "size": 482113, "rev": "7-a1", "modified_at": "2026-11-15T14:05:00Z"}
        ]
    });
    let mem = serde_json::json!({
        "schema": "files-snapshot-root/1", "root_id": "rt_mem", "cursor": "c1.rt_mem.000001",
        "written_at": "2026-11-15T03:12:00Z", "encryption": "none", "names_encrypted": false,
        "items": [{"id": "it_secret", "parent_id": "it_mem0000", "name": "secret.md", "kind": "file", "size": 1, "rev": "1-x", "modified_at": "2026-11-15T14:00:00Z"}]
    });
    let bucket = crate::services::user_files::BUCKET;
    store.put(bucket, &snap::manifest_key(runtime), manifest.to_string().into_bytes(), "").await.unwrap();
    store.put(bucket, &snap::root_key(runtime, "rt_home"), root.to_string().into_bytes(), "").await.unwrap();
    store.put(bucket, &snap::root_key(runtime, "rt_mem"), mem.to_string().into_bytes(), "").await.unwrap();
}

/// Turn the `files_drive` flag on for everyone and install the fake R2.
/// Callers are `#[serial]`.
fn files_on(store: Arc<MemoryStore>) {
    std::env::set_var("ALLTERNIT_FILES_DRIVE_ENABLED", "1");
    let store: Arc<dyn ObjectStore> = store;
    snap::test_store::set(Some(store));
}

fn files_off() {
    std::env::remove_var("ALLTERNIT_FILES_DRIVE_ENABLED");
    snap::test_store::set(None);
}

fn assert_snapshot_freshness(body: &serde_json::Value) {
    assert_eq!(body["freshness"]["served_by"], "snapshot", "{body}");
    assert_eq!(body["freshness"]["computer_state"], "asleep", "{body}");
    assert_eq!(body["freshness"]["snapshot_at"], "2026-11-15T03:12:00Z", "{body}");
}

/// The plan's step 13 Done test: list, stat and the change feed to a
/// sleeping computer come from the snapshot with `computer_state: asleep`,
/// and `wake_for_device` is never called.
#[tokio::test]
#[serial_test::serial]
async fn sleeping_computer_answers_list_stat_and_changes_from_the_snapshot_without_waking() {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    computer(&state, "rt_sleep", DEV_USER, "sleeping").await;
    let store = Arc::new(MemoryStore::default());
    seed_snapshot(&store, "rt_sleep").await;
    files_on(store);
    let before_calls = wake_for_device_calls();
    let before = activity(&state, "rt_sleep").await;

    // list
    let (status, list) = relay(&state, DEV_USER, "rt_sleep", get("/api/v1/files/v2/roots/rt_home/list?parent_id=it_home0000")).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_snapshot_freshness(&list);
    assert_eq!(list["parent"]["id"], "it_home0000");
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert_eq!(list["items"][0]["name"], "Docs");
    assert_eq!(list["cursor"], "c1.rt_home.000412");
    let (status, docs) = relay(&state, DEV_USER, "rt_sleep", get("/api/v1/files/v2/roots/rt_home/list?parent_id=it_docs")).await;
    assert_eq!(status, StatusCode::OK, "{docs}");
    assert_eq!(docs["items"][0]["name"], "Lease.pdf");
    assert_eq!(docs["items"][0]["permissions"]["execute"], false);

    // stat
    let (status, stat) = relay(&state, DEV_USER, "rt_sleep", get("/api/v1/files/v2/items/it_lease")).await;
    assert_eq!(status, StatusCode::OK, "{stat}");
    assert_snapshot_freshness(&stat);
    assert_eq!(stat["item"]["size"], 482113);
    assert_eq!(stat["item"]["root_id"], "rt_home");

    // change feed: at the snapshot's cursor nothing is new; an older cursor re-lists.
    let (status, same) = relay(&state, DEV_USER, "rt_sleep", get("/api/v1/files/v2/roots/rt_home/changes?cursor=c1.rt_home.000412")).await;
    assert_eq!(status, StatusCode::OK, "{same}");
    assert_snapshot_freshness(&same);
    assert_eq!((same["reset_required"].as_bool(), same["changes"].as_array().map(Vec::len)), (Some(false), Some(0)));
    let (_, old) = relay(&state, DEV_USER, "rt_sleep", get("/api/v1/files/v2/roots/rt_home/changes?cursor=c1.rt_home.000100")).await;
    assert_eq!(old["reset_required"], true, "{old}");

    // roots and name search come from the same snapshot.
    let (_, roots) = relay(&state, DEV_USER, "rt_sleep", get("/api/v1/files/v2/roots")).await;
    assert_snapshot_freshness(&roots);
    let search = request("POST", "/api/v1/files/v2/search", &[], r#"{"query":"lease","mode":"name"}"#);
    let (status, hits) = relay(&state, DEV_USER, "rt_sleep", search).await;
    assert_eq!(status, StatusCode::OK, "{hits}");
    assert_eq!((hits["index_state"].as_str(), hits["hits"][0]["item"]["id"].as_str()), (Some("names_only"), Some("it_lease")));

    // Nothing woke it, nothing billed it, nothing reset its idle clock.
    assert_eq!(wake_for_device_calls(), before_calls, "browsing called wake_for_device");
    assert_eq!(activity(&state, "rt_sleep").await, before, "status stays sleeping, idle clock untouched");

    // The counter is live: legacy /files (still wake-on-demand, SPEC section 9 note) does call it.
    computer(&state, "rt_legacy", DEV_USER, "stopped").await;
    relay(&state, DEV_USER, "rt_legacy", get("/api/v1/files/list?path=~")).await;
    assert!(wake_for_device_calls() > before_calls, "legacy path reaches wake_for_device");
    files_off();
}

#[tokio::test]
#[serial_test::serial]
async fn protected_roots_never_leave_the_snapshot() {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    computer(&state, "rt_prot", DEV_USER, "sleeping").await;
    let store = Arc::new(MemoryStore::default());
    seed_snapshot(&store, "rt_prot").await;
    files_on(store);
    let (_, roots) = relay(&state, DEV_USER, "rt_prot", get("/api/v1/files/v2/roots")).await;
    let ids: Vec<_> = roots["roots"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(ids, vec!["rt_home".to_string()]);
    assert!(roots["roots"][0].get("path_hint").is_none());
    let (status, _) = relay(&state, DEV_USER, "rt_prot", get("/api/v1/files/v2/roots/rt_mem/list?parent_id=it_mem0000")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = relay(&state, DEV_USER, "rt_prot", get("/api/v1/files/v2/items/it_secret")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Ingest drops them too (and another computer's manifest is refused).
    let manifest: allternit_files_core::SnapshotManifest = serde_json::from_slice(
        &store_bytes(&snap::manifest_key("rt_prot")).await,
    )
    .unwrap();
    let cleaned = snap::clean_manifest(manifest.clone(), "rt_prot").unwrap();
    assert_eq!(cleaned.roots.unwrap().len(), 1);
    assert!(snap::clean_manifest(manifest, "rt_other").is_err());
    files_off();
}

async fn store_bytes(key: &str) -> Vec<u8> {
    snap::snapshot_store().unwrap().get(crate::services::user_files::BUCKET, key).await.unwrap().unwrap()
}

#[tokio::test]
#[serial_test::serial]
async fn sleeping_computer_without_a_snapshot_never_wakes_and_content_asks_for_consent() {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    computer(&state, "rt_bare", DEV_USER, "sleeping").await;
    files_on(Arc::new(MemoryStore::default()));
    let before_calls = wake_for_device_calls();
    let before = activity(&state, "rt_bare").await;
    for i in 0..50 {
        let path = match i % 3 {
            0 => "/api/v1/files/v2/roots/rt_home/list?parent_id=it_home0000",
            1 => "/api/v1/files/v2/items/it_1",
            _ => "/api/v1/files/v2/roots/rt_home/changes?cursor=c1",
        };
        let (status, body) = relay(&state, DEV_USER, "rt_bare", get(path)).await;
        assert_eq!((status, body["code"].as_str()), (StatusCode::SERVICE_UNAVAILABLE, Some("asleep")), "{path}: {body}");
        assert_eq!(body["wake"]["consent_path"], "/api/v1/computer-files/rt_bare/wake-consent");
    }
    let (status, body) = relay(&state, DEV_USER, "rt_bare", get("/api/v1/files/v2/items/it_1/content")).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::CONFLICT, Some("wake_consent_required")), "{body}");
    assert_eq!(body["wake"]["setting"], "ask");
    let write = request("PUT", "/api/v1/files/v2/roots/rt_home/content?parent_id=it_home0000&name=a.txt", &[], "hi");
    assert_eq!(relay(&state, DEV_USER, "rt_bare", write).await.0, StatusCode::CONFLICT);
    assert_eq!(wake_for_device_calls(), before_calls);
    assert_eq!(activity(&state, "rt_bare").await, before);
    files_off();
}

#[tokio::test]
#[serial_test::serial]
async fn v2_traffic_to_an_awake_computer_does_not_reset_idle_but_legacy_does() {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    computer(&state, "rt_awake", DEV_USER, "running").await;
    let (connection, outgoing) = register_test_connection("rt_awake").await;
    auto_answer(connection, outgoing);
    let before = activity(&state, "rt_awake").await;
    for _ in 0..50 {
        let (status, _) = relay(&state, DEV_USER, "rt_awake", get("/api/v1/files/v2/roots")).await;
        assert_eq!(status, StatusCode::OK);
    }
    assert_eq!(activity(&state, "rt_awake").await, before, "v2 Files traffic must not touch activity");
    // Legacy /files keeps today's behaviour (SPEC section 9 note).
    let (status, _) = relay(&state, DEV_USER, "rt_awake", get("/api/v1/files/list?path=~")).await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(activity(&state, "rt_awake").await.1, before.1, "legacy /files still touches");
}

fn ticket_for(user: &str, runtime: &str, now: u64) -> String {
    let request = WakeConsentRequest { reason: WakeReason::OpenFile, item_id: None, device_id: "dev_web".into() };
    mint_wake_ticket(wake_key().unwrap(), user, runtime, &request, now).0
}

#[tokio::test]
#[serial_test::serial]
async fn tickets_expired_foreign_and_reused() {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    computer(&state, "rt_t", DEV_USER, "running").await;
    let now = chrono::Utc::now().timestamp() as u64;
    let content = "/api/v1/files/v2/items/it_1/content";
    let with = |ticket: &str| request("GET", content, &[("X-Allternit-Wake-Ticket", ticket)], "");

    // Expired -> wake_consent_required.
    let expired = ticket_for(DEV_USER, "rt_t", now - 600);
    let (status, body) = relay(&state, DEV_USER, "rt_t", with(&expired)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::CONFLICT, Some("wake_consent_required")), "{body}");
    assert_eq!(body["wake"]["consent_path"], "/api/v1/computer-files/rt_t/wake-consent");

    // Another user's ticket, or one for another computer -> 403.
    let foreign = ticket_for("user_b", "rt_t", now);
    assert_eq!(relay(&state, DEV_USER, "rt_t", with(&foreign)).await.0, StatusCode::FORBIDDEN);
    let other = ticket_for(DEV_USER, "rt_other", now);
    assert_eq!(relay(&state, DEV_USER, "rt_t", with(&other)).await.0, StatusCode::FORBIDDEN);
    // User B cannot reach A's computer at all.
    assert_eq!(relay(&state, "user_b", "rt_t", with(&foreign)).await.0, StatusCode::NOT_FOUND);

    // A valid ticket reused for 3 content requests: no wake, no activity
    // touch, all answered.
    let (connection, outgoing) = register_test_connection("rt_t").await;
    auto_answer(connection, outgoing);
    let good = ticket_for(DEV_USER, "rt_t", now);
    let calls = wake_for_device_calls();
    let before = activity(&state, "rt_t").await;
    for _ in 0..3 {
        assert_eq!(relay(&state, DEV_USER, "rt_t", with(&good)).await.0, StatusCode::OK);
    }
    assert_eq!(wake_for_device_calls(), calls);
    assert_eq!(activity(&state, "rt_t").await, before);
}

/// Migration 143, applied twice (idempotent), with `public.` dropped for the
/// per-test schema.
async fn apply_migration_143(db: &sqlx::PgPool) {
    let sql = include_str!("../../migrations_pg/143_computer_files_wake.sql").replace("public.", "");
    for _ in 0..2 {
        sqlx::raw_sql(&sql).execute(db).await.expect("migration 143");
    }
}

async fn paired_computers(db: &sqlx::PgPool) {
    sqlx::query("CREATE TABLE IF NOT EXISTS paired_computers (id TEXT PRIMARY KEY, user_id TEXT NOT NULL, name TEXT NOT NULL, secret_hash TEXT NOT NULL, mesh_ip TEXT, last_seen_at TIMESTAMPTZ, provisioned_instance_id TEXT)")
        .execute(db)
        .await
        .unwrap();
}

type Call = (StatusCode, serde_json::Value);

async fn call(state: &Arc<ApiState>, method: &str, path: &str, body: &str, ticket: Option<&str>) -> Call {
    use crate::routes::test_support::authed_request;
    use tower::ServiceExt;
    let mut req = authed_request(method, path, body);
    if let Some(t) = ticket {
        req.headers_mut().insert("x-allternit-wake-ticket", t.parse().unwrap());
    }
    let response = cf::routes().with_state(state.clone()).oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = http_body_util::BodyExt::collect(response.into_body()).await.unwrap().to_bytes();
    (status, serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_default())
}

#[tokio::test]
#[serial_test::serial]
async fn wake_consent_touches_once_and_honours_settings_ownership_and_pc_ids() {
    use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
    let _g = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    computer(&state, "rt_c", DEV_USER, "running").await;
    seed_runtime_device(&state.db, "rt_b", "user_b").await;
    paired_computers(&state.db).await;
    let consent = r#"{"reason":"open_file","device_id":"dev_web"}"#;
    let path = "/api/v1/computer-files/rt_c/wake-consent";

    // Fail safe: flag on but migration 143 not applied -> feature_disabled,
    // and nothing is woken.
    files_on(Arc::new(MemoryStore::default()));
    let (status, off) = call(&state, "POST", path, consent, None).await;
    assert_eq!((status, off["code"].as_str()), (StatusCode::FORBIDDEN, Some("feature_disabled")), "{off}");
    apply_migration_143(&state.db).await;
    // Flag off -> feature_disabled too.
    files_off();
    assert_eq!(call(&state, "GET", "/api/v1/computer-files/rt_c/endpoints", "", None).await.1["code"], "feature_disabled");
    files_on(Arc::new(MemoryStore::default()));

    let _conn = register_test_connection("rt_c").await;
    let before = activity(&state, "rt_c").await;
    let (status, first) = call(&state, "POST", path, consent, None).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let ticket = first["ticket"].as_str().unwrap().to_string();
    assert!(ticket.starts_with("awt1."));
    assert_eq!(first["computer_state"], "awake");
    let touched = activity(&state, "rt_c").await;
    assert_ne!(touched.1, before.1, "consent records one activity touch");
    // Same ticket again: same ticket back, no second touch.
    let (status, again) = call(&state, "POST", path, consent, Some(&ticket)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["ticket"], ticket.as_str());
    assert_eq!(activity(&state, "rt_c").await, touched);
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM computer_files_wake_tickets WHERE computer_id = 'rt_c' AND woke_at IS NOT NULL")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(rows, 1);

    // Settings: default ask, PUT never, then consent is denied.
    let settings = "/api/v1/computer-files/rt_c/wake-settings";
    assert_eq!(call(&state, "GET", settings, "", None).await.1["wake"], "ask");
    assert_eq!(call(&state, "PUT", settings, r#"{"wake":"never"}"#, None).await.0, StatusCode::OK);
    let (status, denied) = call(&state, "POST", path, consent, None).await;
    assert_eq!((status, denied["code"].as_str()), (StatusCode::FORBIDDEN, Some("wake_denied")));

    // Another user's computer: 404 on every route.
    for (m, p, b) in [
        ("POST", "/api/v1/computer-files/rt_b/wake-consent", consent),
        ("GET", "/api/v1/computer-files/rt_b/wake-settings", ""),
        ("GET", "/api/v1/computer-files/rt_b/endpoints", ""),
        ("GET", "/api/v1/computer-files/rt_b/storage", ""),
    ] {
        assert_eq!(call(&state, m, p, b, None).await.0, StatusCode::NOT_FOUND, "{p}");
    }

    // Endpoints: a cloud computer is relay-only.
    let (status, ep) = call(&state, "GET", "/api/v1/computer-files/rt_c/endpoints", "", None).await;
    assert_eq!(status, StatusCode::OK, "{ep}");
    assert_eq!(ep["host"], "cloud_computer");
    let kinds: Vec<_> = ep["candidates"].as_array().unwrap().iter().map(|c| c["kind"].clone()).collect();
    assert_eq!(kinds, vec![serde_json::json!("relay")]);

    // pc_ ids (SPEC "Computer ids"): a paired cloud computer resolves to its
    // runtime device; a machine paired by code stays mesh-only; another
    // user's pc_ is not found.
    sqlx::query("INSERT INTO paired_computers (id, user_id, name, secret_hash, provisioned_instance_id) VALUES ('pc_cloud', $1, 'Cloud', 'x', 'pi_rt_c')")
        .bind(DEV_USER)
        .execute(&state.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO paired_computers (id, user_id, name, secret_hash, mesh_ip, last_seen_at) VALUES ('pc_mac', $1, 'Mac', 'x', '100.64.0.7', NOW())")
        .bind(DEV_USER)
        .execute(&state.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO paired_computers (id, user_id, name, secret_hash, mesh_ip) VALUES ('pc_theirs', 'user_b', 'B', 'x', '100.64.0.9')")
        .execute(&state.db)
        .await
        .unwrap();
    let (_, cloud) = call(&state, "GET", "/api/v1/computer-files/pc_cloud/endpoints", "", None).await;
    assert_eq!((cloud["computer_id"].as_str(), cloud["host"].as_str()), (Some("rt_c"), Some("cloud_computer")), "{cloud}");
    let (_, mac) = call(&state, "GET", "/api/v1/computer-files/pc_mac/endpoints", "", None).await;
    assert_eq!((mac["computer_id"].as_str(), mac["host"].as_str()), (Some("pc_mac"), Some("own_device")), "{mac}");
    assert_eq!(mac["candidates"].as_array().unwrap().len(), 1);
    assert_eq!(mac["candidates"][0]["kind"], "mesh");
    assert_eq!(mac["candidates"][0]["base_url"], "http://100.64.0.7:8013/api/v1/files/v2");
    assert_eq!(call(&state, "POST", "/api/v1/computer-files/pc_mac/wake-consent", consent, None).await.1["code"], "offline");
    assert_eq!(call(&state, "GET", "/api/v1/computer-files/pc_theirs/endpoints", "", None).await.0, StatusCode::NOT_FOUND);

    // Storage discovery: the heartbeat's report, for the signing runtime only.
    assert_eq!(call(&state, "GET", "/api/v1/computer-files/rt_c/storage", "", None).await.0, StatusCode::NOT_FOUND);
    let heartbeat = serde_json::json!({ "filesStorage": {
        "computer_id": "rt_someone_else", "reported_at": "2026-11-15T14:05:00Z",
        "volumes": [{"volume_id": "vol_int", "label": "Disk", "kind": "internal", "total_bytes": 100, "free_bytes": 40, "removable": false}],
        "root_usage": {"rt_home": 7}, "replica_host": {"enabled": false, "capacity_bytes": 0, "used_bytes": 0}
    }});
    cf::store_storage_report(&state.db, DEV_USER, "rt_c", &heartbeat).await;
    let (status, report) = call(&state, "GET", "/api/v1/computer-files/rt_c/storage", "", None).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!((report["computer_id"].as_str(), report["volumes"][0]["free_bytes"].as_u64()), (Some("rt_c"), Some(40)));

    files_off();
    std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
}

#[test]
fn files_headers_pass_the_relay_allow_list() {
    let mut headers = HashMap::new();
    for name in ["X-Allternit-Wake-Ticket", "Range", "If-Match", "X-Allternit-Unlock", "X-Allternit-Device-Id", "Cookie"] {
        headers.insert(name.to_string(), "v".to_string());
    }
    let kept = filtered_headers(headers);
    for name in ["x-allternit-wake-ticket", "range", "if-match", "x-allternit-unlock", "x-allternit-device-id"] {
        assert!(kept.keys().any(|k| k.eq_ignore_ascii_case(name)), "{name} must relay");
    }
    assert!(!kept.keys().any(|k| k.eq_ignore_ascii_case("cookie")));
    let mut response = HashMap::new();
    response.insert("Content-Range".to_string(), "bytes 0-9/100".to_string());
    response.insert("Accept-Ranges".to_string(), "bytes".to_string());
    response.insert("Retry-After".to_string(), "30".to_string());
    response.insert("Set-Cookie".to_string(), "a=b".to_string());
    assert_eq!(filtered_response_headers(response).len(), 3);
}

#[test]
fn proxy_body_accepts_both_body_encoding_spellings() {
    for json in [
        r#"{"method":"GET","path":"/p","body":"AA==","bodyEncoding":"base64"}"#,
        r#"{"method":"GET","path":"/p","body":"AA==","body_encoding":"base64"}"#,
    ] {
        let parsed: BrowserProxyRequest = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.body_encoding, "base64");
    }
}

#[test]
fn asleep_requests_are_sorted_into_snapshot_metadata_and_content() {
    use snap::{classify, AsleepRequest as A, SnapshotQuery as Q};
    assert!(is_files_v2_path("/api/v1/files/v2/roots?x=1"));
    assert!(is_files_v2_path("/api/v1/files/v2"));
    assert!(!is_files_v2_path("/api/v1/files/list"));
    assert!(!is_files_v2_path("/api/v1/files/v2x"));
    assert_eq!(classify("GET", "/api/v1/files/v2/roots", b""), A::Snapshot(Q::Roots));
    assert!(matches!(classify("GET", "/api/v1/files/v2/roots/r/list?parent_id=p", b""), A::Snapshot(Q::List { .. })));
    assert!(matches!(classify("GET", "/api/v1/files/v2/items/i", b""), A::Snapshot(Q::Stat { .. })));
    assert!(matches!(classify("GET", "/api/v1/files/v2/roots/r/changes?cursor=c", b""), A::Snapshot(Q::Changes { .. })));
    assert!(matches!(classify("POST", "/api/v1/files/v2/search", br#"{"query":"a","mode":"name"}"#), A::Snapshot(Q::Search(_))));
    assert_eq!(classify("POST", "/api/v1/files/v2/search", br#"{"query":"a","mode":"semantic"}"#), A::Content);
    assert_eq!(classify("GET", "/api/v1/files/v2/roots/r/trash", b""), A::Metadata);
    for (method, path) in [
        ("GET", "/api/v1/files/v2/items/i/content"),
        ("GET", "/api/v1/files/v2/items/i/manifest"),
        ("GET", "/api/v1/files/v2/chunks/abc"),
        ("GET", "/api/v1/files/v2/uploads/s"),
        ("PUT", "/api/v1/files/v2/roots/r/content"),
        ("POST", "/api/v1/files/v2/move"),
    ] {
        assert_eq!(classify(method, path, b""), A::Content, "{method} {path}");
    }
}

/// The iPhone's File Provider extension has no Clerk session. It sends the
/// `compute`-scoped API key the app mints for it ("Files on <device>",
/// iOS step 19, #2449), the same kind of key the relay already takes. Every
/// owner route under `/api/v1/computer-files/:id/*` takes it with the same
/// ownership checks; a key without `compute` is refused, and another user's
/// computer stays "not found".
#[tokio::test]
#[serial_test::serial]
async fn the_phones_compute_api_key_works_on_the_owner_routes_with_the_same_checks() {
    use crate::services::api_keys::{create_api_key, CreateApiKeyInput};
    use tower::ServiceExt;
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    crate::routes::test_support::events_backbone_schema(&state.db).await;
    computer(&state, "rt_key", DEV_USER, "running").await;
    seed_runtime_device(&state.db, "rt_key_b", "user_b").await;
    apply_migration_143(&state.db).await;
    files_on(Arc::new(MemoryStore::default()));
    let mint = |scopes: Vec<&str>| {
        let db = state.db.clone();
        let scopes: Vec<String> = scopes.into_iter().map(str::to_string).collect();
        async move {
            create_api_key(&db, CreateApiKeyInput { user_id: DEV_USER.into(), organization_id: None, name: "Files on iPhone".into(), scopes })
                .await
                .unwrap()
                .token
        }
    };
    let compute = mint(vec!["compute"]).await;
    let inference_only = mint(vec!["inference"]).await;
    let call_as = |method: &'static str, path: String, body: &'static str, key: String| {
        let state = state.clone();
        async move {
            let req = axum::http::Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {key}"))
                .body(axum::body::Body::from(body))
                .unwrap();
            let response = cf::routes().with_state(state).oneshot(req).await.unwrap();
            let status = response.status();
            let bytes = http_body_util::BodyExt::collect(response.into_body()).await.unwrap().to_bytes();
            (status, serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_default())
        }
    };
    let _conn = register_test_connection("rt_key").await;
    let consent = r#"{"reason":"open_file","device_id":"dev_iphone"}"#;
    for (method, rest, body) in [("GET", "endpoints", ""), ("GET", "wake-settings", ""), ("POST", "wake-consent", consent)] {
        let path = format!("/api/v1/computer-files/rt_key/{rest}");
        let (status, answer) = call_as(method, path.clone(), body, compute.clone()).await;
        assert_eq!(status, StatusCode::OK, "{path}: {answer}");
        let (status, _) = call_as(method, path.clone(), body, inference_only.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}: a key without `compute` is refused");
        let theirs = format!("/api/v1/computer-files/rt_key_b/{rest}");
        assert_eq!(call_as(method, theirs, body, compute.clone()).await.0, StatusCode::NOT_FOUND, "{rest}: same ownership check");
    }
    // Authenticated with the key; the storage report just isn't there yet.
    let (status, answer) = call_as("GET", "/api/v1/computer-files/rt_key/storage".into(), "", compute.clone()).await;
    assert_eq!((status, answer["code"].as_str()), (StatusCode::NOT_FOUND, Some("not_found")), "{answer}");
    files_off();
}
