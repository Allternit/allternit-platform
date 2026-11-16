//! Replica tests on real runtimes (plan step 15, section 6): a source and a host,
//! each a real `/files/v2` router on its own temp folders; the host listens on a
//! real loopback port so the source pushes over HTTP exactly as in production.
//! Only the edges are faked: the signed-in user (an `AuthUser` extension).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use allternit_files_core::*;
use allternit_files_engine::crypto::{ChunkSealer, FilesRootKey};
use allternit_files_engine::replica;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::{Extension, Router};
use curve25519_dalek::montgomery::MontgomeryPoint;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use super::{routes, Config, FilesV2};
use crate::auth::AuthUser;

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

struct Node {
    tmp: tempfile::TempDir,
    data: PathBuf,
    app: Router,
}

/// One runtime: its own home, data folder and computer id, mounted at `/api/v1`.
fn node(computer_id: &str) -> Node {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(base.join("home")).unwrap();
    let cfg = Config {
        enabled: true,
        data_dir: base.join("data"),
        home: Some(base.join("home")),
        computer_id: computer_id.into(),
        memory_db: None,
        models_dir: None,
        code_dir: None,
        watch: false,
        clock: Some(Arc::new(|| "2026-11-15T16:42:00Z".parse().unwrap())),
    };
    let app = Router::new().nest("/api/v1", routes::<()>(Arc::new(FilesV2::new(cfg)))).layer(Extension(user()));
    Node { data: base.join("data"), tmp, app }
}

struct Resp {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Vec<u8>,
}

impl Resp {
    fn typed<T: serde::de::DeserializeOwned + serde::Serialize>(&self) -> T {
        let v: Value = serde_json::from_slice(&self.body).unwrap_or_else(|_| panic!("{} {}", self.status, String::from_utf8_lossy(&self.body)));
        let t: T = serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("{e}: {v}"));
        assert_eq!(serde_json::to_value(&t).unwrap(), v, "response has fields the contract doesn't");
        t
    }
}

impl Node {
    async fn call(&self, method: &str, uri: &str, content_type: Option<&str>, body: Body) -> Resp {
        let mut b = Request::builder().method(method).uri(format!("/api/v1/files/v2{uri}"));
        if let Some(ct) = content_type {
            b = b.header("content-type", ct);
        }
        let res = self.app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let body = res.into_body().collect().await.unwrap().to_bytes().to_vec();
        Resp { status, headers, body }
    }
    async fn get(&self, uri: &str) -> Resp {
        self.call("GET", uri, None, Body::empty()).await
    }
    async fn post(&self, uri: &str, body: Value) -> Resp {
        self.call("POST", uri, Some("application/json"), Body::from(body.to_string())).await
    }
    async fn put_json(&self, uri: &str, body: Value) -> Resp {
        self.call("PUT", uri, Some("application/json"), Body::from(body.to_string())).await
    }
    async fn inbox(&self) -> Root {
        let r: RootsResponse = self.get("/roots").await.typed();
        r.roots.into_iter().find(|r| r.kind == RootKind::Inbox).unwrap()
    }
    async fn write(&self, root: &Root, name: &str, data: Vec<u8>) -> Item {
        let uri = format!("/roots/{}/content?parent_id={}&name={name}&on_conflict=keep_both", root.id, root.root_item_id);
        let r = self.call("PUT", &uri, None, Body::from(data)).await;
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        r.typed::<MutationResponse>().item
    }
    fn home(&self) -> PathBuf {
        self.tmp.path().canonicalize().unwrap().join("home")
    }
}

/// Serves `app` on a loopback port and records every request the host sees.
async fn serve(app: Router, seen: Arc<Mutex<Vec<(String, Vec<String>)>>>) -> (String, tokio::task::JoinHandle<()>) {
    let app = app.layer(axum::middleware::from_fn(move |req: axum::extract::Request, next: axum::middleware::Next| {
        let seen = seen.clone();
        async move {
            let names = req.headers().keys().map(|k| k.as_str().to_string()).collect();
            seen.lock().unwrap().push((req.uri().path().to_string(), names));
            next.run(req).await
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/api/v1/files/v2", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (base, handle)
}

/// Gives the source its Files key the production way: the computer's own
/// `KeyShareRequest`, an envelope wrapped to its X25519 key, `PUT /replicas/key`.
async fn give_key(source: &Node, computer_id: &str) -> FilesRootKey {
    let req: KeyShareRequest = source.get("/replicas/key").await.typed();
    assert_eq!(req.device_id, computer_id);
    let device_pub: [u8; 32] = base64_32(&req.device_pub);
    assert_eq!(req.confirm_code, replica::confirm_code(&device_pub));
    let frk = FilesRootKey::generate("fk_test");
    let eph = [42u8; 32];
    let eph_pub = MontgomeryPoint::mul_base_clamped(eph).to_bytes();
    let shared = MontgomeryPoint(device_pub).mul_clamped(eph).to_bytes();
    let (nonce, wrapped) = replica::wrap_root_key(&frk, &shared, &eph_pub, &device_pub, computer_id).unwrap();
    let env = KeyEnvelope {
        key_id: "fk_test".into(),
        device_id: computer_id.into(),
        wrap_alg: replica::WRAP_ALG.into(),
        ephemeral_pub: b64(&eph_pub),
        nonce,
        wrapped_key: wrapped,
        created_at: "2026-11-15T16:40:00Z".into(),
    };
    let r = source.put_json("/replicas/key", serde_json::to_value(&env).unwrap()).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", String::from_utf8_lossy(&r.body));
    // An envelope wrapped to some other device is refused.
    let mut other = env.clone();
    other.device_id = "rd_phone".into();
    let r = source.put_json("/replicas/key", serde_json::to_value(&other).unwrap()).await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE, "{}", String::from_utf8_lossy(&r.body));
    frk
}

fn b64(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

fn base64_32(s: &str) -> [u8; 32] {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s).unwrap().try_into().unwrap()
}

fn noise(n: usize) -> Vec<u8> {
    let mut x: u64 = 0x2545f4914f6cdd1d;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

/// A source with a key and two files, a host on a real port, a replica, one pass.
struct Pair {
    source: Node,
    host: Node,
    frk: FilesRootKey,
    replica_id: String,
    lease: Item,
    lease_data: Vec<u8>,
    seen: Arc<Mutex<Vec<(String, Vec<String>)>>>,
    _server: tokio::task::JoinHandle<()>,
}

async fn replicated_pair() -> Pair {
    let source = node("rd_cloud");
    let host = node("rd_mac");
    let frk = give_key(&source, "rd_cloud").await;
    let inbox = source.inbox().await;
    let lease_data = noise(3 * 1024 * 1024 + 99);
    let lease = source.write(&inbox, "Lease.pdf", lease_data.clone()).await;
    source.write(&inbox, "notes.txt", b"the quarterly numbers are private".to_vec()).await;

    let seen = Arc::new(Mutex::new(Vec::new()));
    let (base, server) = serve(host.app.clone(), seen.clone()).await;
    let create = json!({"source_computer_id": "rd_cloud", "root_id": inbox.id, "host_device_id": "rd_mac", "mode": "full"});
    let on_host: ReplicasResponse = host.post("/replicas", create.clone()).await.typed();
    let mut on_source_body = create.clone();
    on_source_body["host_base_url"] = json!(base);
    let on_source: ReplicasResponse = source.post("/replicas", on_source_body).await.typed();
    let replica_id = on_host.replicas[0].replica_id.clone();
    assert_eq!(replica_id, on_source.replicas[0].replica_id, "both ends derive the same id");
    assert_eq!(replica_id, replica::replica_id("rd_cloud", &inbox.id, "rd_mac"));

    let job: JobRef = source.post(&format!("/replicas/{replica_id}/sync"), json!({})).await.typed();
    assert_eq!((job.kind, job.state), (JobRefKind::ReplicaSync, JobRefState::Done), "{job:?}");
    let listed: ReplicasResponse = source.get("/replicas").await.typed();
    assert_eq!(listed.replicas[0].state, ReplicaState::Current);
    assert_eq!(listed.replicas[0].key_id, "fk_test");
    let hosted: ReplicasResponse = host.get("/replicas").await.typed();
    assert_eq!(hosted.replicas[0].state, ReplicaState::Current);
    assert!(hosted.replicas[0].stored_bytes > lease_data.len() as u64);
    Pair { source, host, frk, replica_id, lease, lease_data, seen, _server: server }
}

/// A client with the Files key reading through the host's routes only.
async fn read_from_host(host: &Node, frk: &FilesRootKey, replica_id: &str, name: &str) -> Vec<u8> {
    let r = host.get(&format!("/replicas/{replica_id}/head")).await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.headers["x-allternit-served-by"], "replica");
    let head = replica::open_head(frk, replica_id, &r.body).unwrap();
    let sealer = ChunkSealer::for_replica(frk, replica_id);
    let mut blobs: HashMap<String, Vec<u8>> = HashMap::new();
    for p in &head.parts {
        let loc = sealer.locator(&p.hash).unwrap();
        let r = host.get(&format!("/replicas/{replica_id}/blobs/{loc}")).await;
        assert_eq!(r.headers["x-allternit-served-by"], "replica");
        blobs.insert(loc, r.body);
    }
    let catalog = replica::read_catalog(frk, &head, &mut |l: &str| Ok(blobs.get(l).cloned())).unwrap();
    let entry = catalog.items.iter().find(|e| e.name == name).unwrap().clone();
    for c in &entry.chunks {
        let loc = sealer.locator(&c.hash).unwrap();
        let r = host.get(&format!("/replicas/{replica_id}/blobs/{loc}")).await;
        assert_eq!(r.status, 200);
        blobs.insert(loc, r.body);
    }
    replica::read_file(frk, &catalog, &entry.item_id, &mut |l: &str| Ok(blobs.get(l).cloned())).unwrap()
}

fn files_under(p: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            if e.file_type().unwrap().is_dir() {
                out.extend(files_under(&e.path()));
            } else {
                out.push(e.path());
            }
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_the_primary_stopped_a_client_reads_a_replicated_file_from_the_replica() {
    let p = replicated_pair().await;
    let Pair { source, host, frk, replica_id, lease_data, .. } = p;
    // Stop the primary: its router and every file on its disk are gone.
    drop(source);
    let got = read_from_host(&host, &frk, &replica_id, "Lease.pdf").await;
    assert_eq!(got, lease_data);

    // The host itself holds only ciphertext: no names, no content.
    let mut haystack = Vec::new();
    for f in files_under(&host.data.join("replica-host")) {
        haystack.extend(std::fs::read(&f).unwrap());
    }
    let has = |n: &[u8]| haystack.windows(n.len()).any(|w| w == n);
    assert!(!has(b"Lease.pdf") && !has(b"quarterly numbers"), "the host can read the replica");
    assert!(!has(&lease_data[..64]));
    // Without the key the head does not open.
    let r = host.get(&format!("/replicas/{replica_id}/head")).await;
    assert!(replica::open_head(&FilesRootKey::generate("fk_test"), &replica_id, &r.body).is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restore_reports_hashes_verified_and_a_tampered_chunk_fails() {
    let p = replicated_pair().await;
    let restore = json!({"replica_id": p.replica_id, "target_computer_id": "rd_cloud", "item_ids": [p.lease.id]});
    let report: ReplicaRestoreReport = p.source.post("/replicas/restore", restore.clone()).await.typed();
    assert!(report.verified, "{report:?}");
    assert_eq!((report.files_restored, report.chunks_failed, report.failures.len()), (1, 0, 0));
    assert!(report.chunks_verified >= 1);
    assert_eq!(report.bytes, p.lease_data.len() as u64);
    // Restored next to the original as a keep-both copy, byte for byte.
    let inbox = p.source.home().join("Allternit/Inbox");
    let copies: Vec<PathBuf> = std::fs::read_dir(&inbox).unwrap().flatten().map(|e| e.path()).filter(|f| f.file_name().unwrap().to_string_lossy().starts_with("Lease")).collect();
    assert_eq!(copies.len(), 2, "{copies:?}");
    for c in copies {
        assert_eq!(std::fs::read(c).unwrap(), p.lease_data);
    }

    // Flip one byte of one sealed chunk on the host's disk.
    let sealer = ChunkSealer::for_replica(&p.frk, &p.replica_id);
    let manifest: Manifest = p.source.get(&format!("/items/{}/manifest", p.lease.id)).await.typed();
    let loc = sealer.locator(&manifest.chunks[0].hash).unwrap();
    let blob_path = p.host.data.join("replica-host").join(&p.replica_id).join("blobs").join(&loc[..2]).join(&loc);
    let mut blob = std::fs::read(&blob_path).unwrap();
    let mid = blob.len() - 20;
    blob[mid] ^= 0x80;
    std::fs::write(&blob_path, blob).unwrap();

    let report: ReplicaRestoreReport = p.source.post("/replicas/restore", restore).await.typed();
    assert!(!report.verified);
    assert_eq!(report.files_restored, 0);
    assert_eq!(report.chunks_failed, 1);
    assert_eq!(report.failures, vec![RestoreFailure { item_id: p.lease.id.clone(), reason: ErrorCode::IntegrityFailed }]);
    let leases = std::fs::read_dir(&inbox).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("Lease")).count();
    assert_eq!(leases, 2, "a failed restore leaves no partial file");
}

/// Regression guard (plan D14, SPEC section 10): replication must never wake a
/// computer or reset idle. Reads the source on purpose: a behaviour test can't
/// see a wake call added for some other computer id.
#[test]
fn replication_never_wakes_or_touches_activity() {
    const SOURCES: &[(&str, &str)] = &[("replicas.rs", include_str!("replicas.rs")), ("sync.rs", include_str!("sync.rs"))];
    const FORBIDDEN: &[&str] = &[
        "wake_ticket",
        "wake-ticket",
        "wake_consent",
        "wake-consent",
        "WakeConsent",
        "/wake",
        "touch_runtime_activity",
        "touch_computer_activity",
        "touch_activity",
        "touch_provisioned_activity",
        "mark_active",
        "runtime-devices/",
    ];
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
    assert!(hits.is_empty(), "replication must never wake or touch activity: {hits:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pass_sends_no_wake_ticket_and_an_unreachable_host_leaves_the_replica_behind() {
    let p = replicated_pair().await;
    let seen = p.seen.lock().unwrap().clone();
    assert!(!seen.is_empty());
    for (path, headers) in &seen {
        assert!(path.starts_with(&format!("/api/v1/files/v2/replicas/{}", p.replica_id)), "replication touched {path}");
        assert!(!headers.iter().any(|h| h.contains("wake")), "replication sent a wake header to {path}: {headers:?}");
    }
    // The host goes to sleep (nothing answers at its address). The next pass does
    // not wake it: it fails fast and records `behind`.
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dead_base = format!("http://{}/api/v1/files/v2", dead.local_addr().unwrap());
    drop(dead);
    let inbox = p.source.inbox().await;
    let moved = json!({"source_computer_id": "rd_cloud", "root_id": inbox.id, "host_device_id": "rd_mac", "mode": "full", "host_base_url": dead_base});
    let _: ReplicasResponse = p.source.post("/replicas", moved).await.typed();
    let before = p.seen.lock().unwrap().len();
    let job: JobRef = p.source.post(&format!("/replicas/{}/sync", p.replica_id), json!({})).await.typed();
    assert_eq!(job.state, JobRefState::Failed);
    assert_eq!(job.error, Some(ErrorCode::ReplicaUnavailable));
    let listed: ReplicasResponse = p.source.get("/replicas").await.typed();
    assert_eq!(listed.replicas[0].state, ReplicaState::Behind);
    assert_eq!(p.seen.lock().unwrap().len(), before, "nothing reached the sleeping host");
}
