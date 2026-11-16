//! Route tests for `/files/v2`: real engine, real temp folders, no mocks of our
//! own code. Responses are checked against the generated contract types
//! (deserialize, serialize back, compare) so a field the schema doesn't have,
//! or a wrong type, fails.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use allternit_files_core::*;
use allternit_files_engine::chunker;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::{Extension, Router};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use super::{routes, Config, FilesV2};
use crate::auth::AuthUser;

const RELAYED: (&str, &str) = ("x-allternit-relayed", "1");
const UNLOCK: (&str, &str) = ("x-allternit-unlock", "face-id");

struct Env {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    outside: PathBuf,
    app: Router,
}

fn user() -> AuthUser {
    AuthUser {
        user_id: "u1".into(),
        email: None,
        name: Some("Eoj".into()),
        avatar_url: None,
        tenant_id: None,
        organization_id: None,
        organization_role: None,
        organization_slug: None,
    }
}

fn config(tmp: &Path) -> Config {
    Config {
        enabled: true,
        data_dir: tmp.join("data"),
        home: Some(tmp.join("home")),
        computer_id: "rd_test".into(),
        memory_db: None,
        models_dir: None,
        code_dir: None,
        watch: false,
        clock: Some(Arc::new(|| "2026-11-15T16:42:00Z".parse().unwrap())),
    }
}

fn env_with(f: impl FnOnce(&mut Config)) -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(base.join("home")).unwrap();
    std::fs::create_dir_all(base.join("outside")).unwrap();
    let mut cfg = config(&base);
    f(&mut cfg);
    let app = routes::<()>(Arc::new(FilesV2::new(cfg))).layer(Extension(user()));
    Env { home: base.join("home"), outside: base.join("outside"), _tmp: tmp, app }
}

fn env() -> Env {
    env_with(|_| {})
}

struct Resp {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Vec<u8>,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|_| panic!("not json: {}", String::from_utf8_lossy(&self.body)))
    }
    /// The body as contract type `T`, failing on any field or type the schema doesn't have.
    fn typed<T: serde::de::DeserializeOwned + serde::Serialize>(&self) -> T {
        let v = self.json();
        let t: T = serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("{e}: {v}"));
        assert_eq!(serde_json::to_value(&t).unwrap(), v, "response has fields the contract doesn't");
        t
    }
    fn error(&self) -> ErrorBody {
        let e: ErrorBody = self.typed();
        assert_eq!(e.error, e.code);
        assert_eq!(self.status, super::status_for(e.code.clone()), "status must follow SPEC section 16");
        e
    }
}

impl Env {
    async fn call(&self, method: &str, uri: &str, headers: &[(&str, &str)], body: Body) -> Resp {
        let mut b = Request::builder().method(method).uri(format!("/files/v2{uri}"));
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let res = self.app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let body = res.into_body().collect().await.unwrap().to_bytes().to_vec();
        Resp { status, headers, body }
    }
    async fn get(&self, uri: &str, headers: &[(&str, &str)]) -> Resp {
        self.call("GET", uri, headers, Body::empty()).await
    }
    async fn post(&self, uri: &str, headers: &[(&str, &str)], body: Value) -> Resp {
        let mut h = headers.to_vec();
        h.push(("content-type", "application/json"));
        self.call("POST", uri, &h, Body::from(body.to_string())).await
    }
    async fn roots(&self) -> RootsResponse {
        let r = self.get("/roots", &[]).await;
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        r.typed()
    }
    /// Adds `dir` as a user root from this computer.
    async fn add(&self, dir: &Path) -> Root {
        std::fs::create_dir_all(dir).unwrap();
        let r = self
            .post("/roots", &[], json!({"name": "Docs", "local_path": dir.to_string_lossy(), "policy": policy_json(true)}))
            .await;
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        r.typed::<RootResponse>().root
    }
    async fn put(&self, root: &Root, query: &str, headers: &[(&str, &str)], data: Vec<u8>) -> Resp {
        self.call("PUT", &format!("/roots/{}/content?parent_id={}&{query}", root.id, root.root_item_id), headers, Body::from(data))
            .await
    }
}

fn policy_json(shareable: bool) -> Value {
    json!({"read": true, "write": true, "shareable": shareable, "agent_process": shareable, "protected": false,
           "offline": true, "replica": true, "versions": {"keep_count": 5, "keep_days": 0}, "trash_days": 30})
}

fn fixture(raw: &str) -> Value {
    serde_json::from_str::<Value>(raw).unwrap()["message"].clone()
}

fn noise(n: usize) -> Vec<u8> {
    let mut x: u64 = 0x9e3779b97f4a7c15;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

/// Regression guard for SPEC section 1 principle 2 and section 9 rule 1
/// ("Files requests never wake and never touch activity"): no v2 source line
/// may call an idle-resetting function. Reads the module's own source on
/// purpose; a behaviour test can't see a call added for some other id.
#[test]
fn files_v2_never_touches_activity() {
    const SOURCES: &[(&str, &str)] = &[
        ("mod.rs", include_str!("mod.rs")),
        ("routes.rs", include_str!("routes.rs")),
        ("feed.rs", include_str!("feed.rs")),
        ("roots.rs", include_str!("roots.rs")),
        ("memory.rs", include_str!("memory.rs")),
        ("meta.rs", include_str!("meta.rs")),
        ("storage.rs", include_str!("storage.rs")),
        ("errors.rs", include_str!("errors.rs")),
        ("replicas.rs", include_str!("replicas.rs")),
        ("sync.rs", include_str!("sync.rs")),
        ("snapshot.rs", include_str!("snapshot.rs")),
        ("cloud.rs", include_str!("cloud.rs")),
        ("drops.rs", include_str!("drops.rs")),
    ];
    const FORBIDDEN: &[&str] =
        &["touch_computer_activity", "touch_runtime_activity", "touch_activity", "touch_provisioned_activity", "last_activity_at", "mark_active"];
    let mut hits = Vec::new();
    for (file, src) in SOURCES {
        for (n, line) in src.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            for f in FORBIDDEN {
                if code.contains(f) {
                    hits.push(format!("{file}:{}: {f}", n + 1));
                }
            }
        }
    }
    assert!(hits.is_empty(), "Files traffic must never touch activity: {hits:?}");
}

#[tokio::test]
async fn relayed_caller_cannot_add_secret_or_outside_folders_and_gets_a_locked_down_root() {
    let e = env();
    std::fs::create_dir_all(e.home.join(".ssh")).unwrap();
    std::fs::create_dir_all(e.home.join("Library/Keychains")).unwrap();
    let home = e.home.to_string_lossy().to_string();
    for path in [
        format!("{home}/.ssh"),
        "~/.ssh".to_string(),
        format!("{home}/Library"),
        e.outside.to_string_lossy().to_string(),
        home.clone(),
        format!("{home}/../outside"),
    ] {
        let r = e.post("/roots", &[RELAYED], json!({"name": "X", "local_path": path, "policy": policy_json(true)})).await;
        assert_eq!(r.error().code, ErrorCode::ScopeViolation, "{path}");
    }
    // A symlink inside home that points outside is resolved first.
    std::os::unix::fs::symlink(&e.outside, e.home.join("escape")).unwrap();
    let r = e.post("/roots", &[RELAYED], json!({"name": "X", "local_path": "~/escape", "policy": policy_json(true)})).await;
    assert_eq!(r.error().code, ErrorCode::ScopeViolation);

    std::fs::create_dir_all(e.home.join("Docs")).unwrap();
    let r = e.post("/roots", &[RELAYED], json!({"name": "Docs", "local_path": "~/Docs", "policy": policy_json(true)})).await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let root = r.typed::<RootResponse>().root;
    assert_eq!(root.kind, RootKind::UserAdded);
    assert!(!root.policy.shareable && !root.policy.agent_process, "relay-added roots start unshared, agents off");
    assert!(root.policy.write);
    // And the relay can't switch them on afterwards.
    let r = e
        .call("PATCH", &format!("/roots/{}", root.id), &[RELAYED, ("content-type", "application/json")], Body::from(json!({"policy": policy_json(true)}).to_string()))
        .await;
    assert_eq!(r.error().code, ErrorCode::Forbidden);
}

#[tokio::test]
async fn relayed_read_through_an_escaping_symlink_is_a_scope_violation() {
    let e = env();
    std::fs::write(e.outside.join("secret.txt"), b"top secret").unwrap();
    let docs = e.home.join("Docs");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(docs.join("ok.txt"), b"fine").unwrap();
    std::os::unix::fs::symlink(e.outside.join("secret.txt"), docs.join("link.txt")).unwrap();
    let root = e.add(&docs).await;
    let list = e.get(&format!("/roots/{}/list?parent_id={}", root.id, root.root_item_id), &[RELAYED]).await;
    let list: ListResponse = list.typed();
    let link = list.items.iter().find(|i| i.name == "link.txt").expect("the link is listed");
    assert!(!link.permissions.read);
    let ok = list.items.iter().find(|i| i.name == "ok.txt").unwrap();
    for uri in [format!("/items/{}/content", link.id), format!("/items/{}/manifest", link.id)] {
        let r = e.get(&uri, &[RELAYED]).await;
        assert_ne!(r.status, 200, "{uri}");
        assert!(!String::from_utf8_lossy(&r.body).contains("top secret"));
    }
    let r = e.get(&format!("/items/{}/content", link.id), &[RELAYED]).await;
    assert_eq!(r.error().code, ErrorCode::ScopeViolation);
    let r = e.get(&format!("/items/{}/content", ok.id), &[RELAYED]).await;
    assert_eq!((r.status, r.body.as_slice()), (StatusCode::OK, &b"fine"[..]));
}

#[tokio::test]
async fn chunk_put_takes_exactly_4_mib_and_refuses_over_4_mib_plus_64_kib() {
    let e = env();
    let root = e.add(&e.home.join("Docs")).await;
    let data = noise(4 * 1024 * 1024);
    let hash = chunker::hash_bytes(&data);
    let create = json!({
        "root_id": root.id, "parent_id": root.root_item_id, "name": "one.bin", "size": data.len(),
        "content_hash": hash, "chunker": chunker::params(),
        "chunks": [{"hash": hash, "offset": 0, "size": data.len()}], "total_chunks": 1, "on_conflict": "fail"
    });
    let s: UploadStatus = e.post("/uploads", &[], create).await.typed();
    let big = noise(4 * 1024 * 1024 + 64 * 1024 + 1);
    let r = e.call("PUT", &format!("/uploads/{}/chunks/{hash}", s.session_id), &[], Body::from(big.clone())).await;
    assert_eq!(r.status, 413);
    assert_eq!(r.error().code, ErrorCode::PayloadTooLarge);
    let r = e.call("PUT", &format!("/uploads/{}/chunks/{hash}", s.session_id), &[], Body::from(data.clone())).await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let put: ChunkPutResponse = r.typed();
    assert_eq!(put.remaining, 0);
    // Other routes keep the 1 MiB limit.
    let r = e.call("POST", "/chunks/have-need", &[("content-type", "application/json")], Body::from(vec![b' '; 1024 * 1024 + 1])).await;
    assert_eq!(r.error().code, ErrorCode::PayloadTooLarge);
    // The small write takes 4 MiB too, and 413s past the limit.
    assert_eq!(e.put(&root, "name=small.bin", &[], data).await.status, 200);
    assert_eq!(e.put(&root, "name=big.bin", &[], big).await.error().code, ErrorCode::PayloadTooLarge);
}

#[tokio::test]
async fn stale_base_rev_keep_both_names_the_copy_in_the_writers_local_time() {
    let e = env();
    let root = e.add(&e.home.join("Docs")).await;
    let v1: MutationResponse = e.put(&root, "name=Lease.pdf", &[], b"v1".to_vec()).await.typed();
    let base = v1.item.rev.clone();
    let v2: MutationResponse = e.put(&root, &format!("name=Lease.pdf&base_rev={base}"), &[], b"v2".to_vec()).await.typed();
    assert!(v2.conflict.is_none());
    // A phone in CST (UTC-6) still holding v1: the runtime clock says 16:42 UTC.
    let q = format!("name=Lease.pdf&base_rev={base}&on_conflict=keep_both&device_name=iPhone&utc_offset_minutes=-360");
    let r = e.put(&root, &q, &[], b"phone edit".to_vec()).await;
    let m: MutationResponse = r.typed();
    let c = m.conflict.expect("a stale base_rev with keep_both returns ConflictInfo");
    assert_eq!(c.conflict_name, "Lease (conflict iPhone 2026-11-15 1042).pdf");
    assert_eq!(c.conflict_item.name, c.conflict_name);
    assert_eq!(c.base_rev, base);
    assert_eq!(c.current_rev, v2.item.rev);
    // No offset: UTC (SPEC section 4). No device_name: the header.
    let r = e.put(&root, &format!("name=Lease.pdf&base_rev={base}"), &[("x-allternit-device-name", "Studio Mac")], b"mac".to_vec()).await;
    let m: MutationResponse = r.typed();
    assert_eq!(m.conflict.unwrap().conflict_name, "Lease (conflict Studio Mac 2026-11-15 1642).pdf");
    // fail: rev_mismatch carrying the current item.
    let r = e.put(&root, &format!("name=Lease.pdf&base_rev={base}&on_conflict=fail"), &[], b"x".to_vec()).await;
    let err = r.error();
    assert_eq!(err.code, ErrorCode::RevMismatch);
    assert_eq!(err.current.unwrap().rev, v2.item.rev);
    // Out-of-range offset is refused.
    let r = e.put(&root, &format!("name=Lease.pdf&base_rev={base}&utc_offset_minutes=900"), &[], b"x".to_vec()).await;
    assert_eq!(r.error().code, ErrorCode::InvalidRequest);
}

/// The contract fixtures for runtime responses, run through the handlers:
/// requests are taken from the fixtures, responses must be the fixture's type.
#[tokio::test]
async fn contract_fixtures_round_trip_through_the_handlers() {
    let e = env();
    let roots = e.roots().await;
    for kind in [RootKind::AllternitHome, RootKind::Inbox, RootKind::Shared, RootKind::AgentOutput, RootKind::Downloads, RootKind::Meetings, RootKind::Scans, RootKind::MemoryDrive, RootKind::Code, RootKind::Models] {
        assert!(roots.roots.iter().any(|r| r.kind == kind), "{kind:?} root missing");
    }
    let inbox = roots.roots.iter().find(|r| r.kind == RootKind::Inbox).unwrap();
    let home = roots.roots.iter().find(|r| r.kind == RootKind::AllternitHome).unwrap();
    assert_eq!(inbox.parent_root_id.as_deref(), Some(home.id.as_str()));
    assert_eq!(inbox.path_hint.as_deref(), Some("~/Allternit/Inbox"));
    let code = roots.roots.iter().find(|r| r.kind == RootKind::Code).unwrap();
    assert!(!code.provider_ready && code.path_hint.is_none(), "a provider with no source is not ready and shows no internal path");
    let models = roots.roots.iter().find(|r| r.kind == RootKind::Models).unwrap();
    assert_eq!(models.policy.versions.keep_count, 0);

    // ping, whoami
    let p: PingResponse = e.get("/ping", &[]).await.typed();
    assert_eq!((p.protocol.as_str(), p.computer_id.as_str()), ("files/2", "rd_test"));
    let w: WhoAmIResponse = e.get("/whoami", &[("x-allternit-device-id", "dev_iphone16")]).await.typed();
    assert_eq!(w.principal.kind, PrincipalKind::OwnDevice);

    // small-write-query.json as a create, then list-home / stat-agent-output shapes.
    let sq = fixture(include_str!("../../../../contracts/files-drive/fixtures/small-write-query.json"));
    let q = format!("name={}&on_conflict=keep_both&device_name={}&utc_offset_minutes={}", "Lease.pdf", sq["device_name"].as_str().unwrap(), sq["utc_offset_minutes"]);
    let w: MutationResponse = e.put(inbox, &q, &[], b"hello world".to_vec()).await.typed();
    let l: ListResponse = e.get(&format!("/roots/{}/list?parent_id={}", inbox.id, inbox.root_item_id), &[]).await.typed();
    assert_eq!(l.items.len(), 1);
    let s: StatResponse = e.get(&format!("/items/{}", w.item.id), &[]).await.typed();
    assert_eq!(s.item.rev, w.item.rev);

    // content: Range and If-None-Match.
    let r = e.get(&format!("/items/{}/content", w.item.id), &[("range", "bytes=0-4")]).await;
    assert_eq!((r.status, r.body.as_slice()), (StatusCode::PARTIAL_CONTENT, &b"hello"[..]));
    assert_eq!(r.headers["content-range"], "bytes 0-4/11");
    let r = e.get(&format!("/items/{}/content", w.item.id), &[("range", "bytes=-5")]).await;
    assert_eq!(r.body, b"world");
    let etag = format!("\"{}\"", w.item.rev);
    let r = e.get(&format!("/items/{}/content", w.item.id), &[("if-none-match", &etag)]).await;
    assert_eq!(r.status, StatusCode::NOT_MODIFIED);
    let r = e.get(&format!("/items/{}/content", w.item.id), &[("range", "bytes=50-")]).await;
    assert_eq!(r.status, StatusCode::RANGE_NOT_SATISFIABLE);

    // manifest, have-need
    let m: Manifest = e.get(&format!("/items/{}/manifest", w.item.id), &[]).await.typed();
    let hn: HaveNeedResponse = e.post("/chunks/have-need", &[], json!({"hashes": [m.content_hash]})).await.typed();
    assert_eq!(hn.have.len() + hn.need.len(), 1);

    // upload-create.json with real chunks: create → status → put → commit.
    let data = noise(3 * 1024 * 1024);
    let c = chunker::chunk_bytes(&data);
    let mut up = fixture(include_str!("../../../../contracts/files-drive/fixtures/upload-create.json"));
    up["root_id"] = json!(inbox.id);
    up["parent_id"] = json!(inbox.root_item_id);
    up["size"] = json!(c.size);
    up["content_hash"] = json!(c.content_hash);
    up["chunks"] = serde_json::to_value(&c.chunks).unwrap();
    up["total_chunks"] = json!(c.chunks.len());
    let st: UploadStatus = e.post("/uploads", &[], up).await.typed();
    assert_eq!(st.need.len(), c.chunks.len());
    let early = e.post(&format!("/uploads/{}/commit", st.session_id), &[], json!({})).await;
    let err = early.error();
    assert_eq!(err.code, ErrorCode::ChunkMissing);
    assert_eq!(err.missing_chunks.unwrap().len(), c.chunks.len());
    for ch in &c.chunks {
        let bytes = data[ch.offset as usize..(ch.offset + ch.size) as usize].to_vec();
        let _: ChunkPutResponse = e.call("PUT", &format!("/uploads/{}/chunks/{}", st.session_id, ch.hash), &[], Body::from(bytes)).await.typed();
    }
    let st: UploadStatus = e.get(&format!("/uploads/{}", st.session_id), &[]).await.typed();
    assert!(st.need.is_empty());
    let done: UploadCommitResponse = e.post(&format!("/uploads/{}/commit", st.session_id), &[], json!({})).await.typed();
    assert_eq!(done.item.name, "Walkthrough.mov");
    assert_eq!(std::fs::read(e.home.join("Allternit/Inbox/Walkthrough.mov")).unwrap(), data);

    // errors: protected, not found, feature disabled.
    let memory = roots.roots.iter().find(|r| r.kind == RootKind::MemoryDrive).unwrap();
    let r = e.get(&format!("/roots/{}/list?parent_id={}", memory.id, memory.root_item_id), &[]).await;
    assert_eq!(r.error().code, ErrorCode::ProtectedRootLocked);
    let r = e.get(&format!("/roots/{}/list?parent_id={}", memory.id, memory.root_item_id), &[UNLOCK]).await;
    assert_eq!(r.status, 200);
    assert_eq!(e.get("/items/it_nope", &[]).await.error().code, ErrorCode::NotFound);
    let _: ReplicasResponse = e.get("/replicas", &[]).await.typed();
    let off = env_with(|c| c.enabled = false);
    assert_eq!(off.get("/ping", &[]).await.error().code, ErrorCode::FeatureDisabled);
    assert!(!off.home.join("Allternit").exists(), "nothing is created while the flag is off");
}

#[tokio::test]
async fn subscribe_streams_changes_after_the_cursor() {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let e = env();
    let roots = e.roots().await;
    let shared = roots.roots.iter().find(|r| r.kind == RootKind::Shared).unwrap().clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = Router::new().nest("/api/v1", e.app.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/v1/files/v2/subscribe")).await.unwrap();
    let sub = json!({"type": "subscribe", "roots": [{"root_id": shared.id, "cursor": shared.cursor}]});
    ws.send(Message::Text(sub.to_string().into())).await.unwrap();
    let w: MutationResponse = e.put(&shared, "name=note.txt", &[], b"hi".to_vec()).await.typed();
    let frame = tokio::time::timeout(std::time::Duration::from_secs(10), ws.next()).await.unwrap().unwrap().unwrap();
    let msg: FeedMessage = serde_json::from_str(frame.to_text().unwrap()).unwrap();
    match msg {
        FeedMessage::FeedChanges(c) => {
            assert_eq!(c.root_id, shared.id);
            assert_eq!(c.cursor, w.cursor);
            assert!(c.changes.iter().any(|ch| ch.item_id == w.item.id));
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn memory_drive_writes_become_git_commits_and_need_the_unlock() {
    use crate::memory_drive_service as service;
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let db = crate::db::DbHandle::new(base.join("test.db")).unwrap();
    let brains = base.join("brains");
    crate::memory_drive_writer::configure_root(&db, &brains).unwrap();
    service::provision(&db, &brains, "u1").unwrap();
    let commits = |db: &crate::db::DbHandle| service::resolve(db, "u1").unwrap().storage().unwrap().history(None, 100).unwrap().len();
    let before = commits(&db);
    let mdb = db.clone();
    let e = env_with(move |c| c.memory_db = Some(mdb));
    let roots = e.roots().await;
    let mem = roots.roots.iter().find(|r| r.kind == RootKind::MemoryDrive).unwrap().clone();
    assert!(mem.provider_ready && mem.policy.protected && mem.path_hint.is_none());
    // Locked without the unlock proof, for reads and writes.
    let r = e.put(&mem, "name=notes.md", &[], b"# Notes\n".to_vec()).await;
    assert_eq!(r.error().code, ErrorCode::ProtectedRootLocked);
    let r = e.put(&mem, "name=notes.md", &[UNLOCK], b"# Notes\n".to_vec()).await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let w: MutationResponse = r.typed();
    assert_eq!(commits(&db), before + 1, "a Files write is one Memory Drive commit");
    let files = service::resolve(&db, "u1").unwrap().storage().unwrap().snapshot(None).unwrap().files;
    assert_eq!(files.get("notes.md").map(String::as_str), Some("# Notes\n"));
    // A stale base_rev fails closed (no conflict copies in Memory Drive).
    let r = e.put(&mem, "name=notes.md&base_rev=1-dir", &[UNLOCK], b"# x\n".to_vec()).await;
    assert_eq!(r.error().code, ErrorCode::RevMismatch);
    let r = e.post("/mkdir", &[UNLOCK], json!({"parent_id": mem.root_item_id, "name": "x", "on_conflict": "fail"})).await;
    assert_eq!(r.error().code, ErrorCode::Forbidden);
    let r = e.post("/delete", &[UNLOCK], json!({"item_id": w.item.id, "base_rev": w.item.rev, "permanent": false})).await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(commits(&db), before + 2);
    assert!(!service::resolve(&db, "u1").unwrap().storage().unwrap().snapshot(None).unwrap().files.contains_key("notes.md"));
}
