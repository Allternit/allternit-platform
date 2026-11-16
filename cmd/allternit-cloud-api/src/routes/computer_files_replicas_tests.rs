//! Files drive step 15 on cloud-api: replica placement rules and the no-wake
//! peer relay. DB-backed like the other relay tests (`routes::test_support`);
//! the relayed runtime is a test connection.

use std::sync::atomic::Ordering;

use allternit_files_core::{ReplicaHostInfo, Volume, VolumeKind};
use tower::ServiceExt;

use super::*;
use crate::routes::runtime_relay::{register_test_connection, CloudMessage};
use crate::routes::test_support::{seed_runtime_device, test_state, MockGateway, DEV_USER};

fn report(free: u64, enabled: bool, removable: bool) -> StorageDiscovery {
    StorageDiscovery {
        computer_id: String::new(),
        reported_at: "2026-11-15T12:00:00Z".into(),
        volumes: vec![Volume {
            volume_id: "dev-1".into(),
            label: "This computer".into(),
            kind: if removable { VolumeKind::External } else { VolumeKind::Internal },
            total_bytes: free * 2,
            free_bytes: free,
            removable,
            encrypted: None,
        }],
        root_usage: Default::default(),
        replica_host: ReplicaHostInfo { enabled, capacity_bytes: 0, used_bytes: 0 },
    }
}

fn candidate(id: &str, report: StorageDiscovery, state: ComputerState, cloud: bool, age: i64) -> Candidate {
    Candidate { device_id: id.into(), label: id.into(), report, state, cloud, report_age_secs: age }
}

const GB: u64 = 1_000_000_000;

#[test]
fn placement_ranks_by_space_availability_and_sensitivity() {
    let all = || {
        vec![
            candidate("tight", report(GB + GB / 20, true, false), ComputerState::Awake, false, 60),
            candidate("cloud", report(500 * GB, true, false), ComputerState::Asleep, true, 600),
            candidate("mac", report(500 * GB, true, false), ComputerState::Awake, false, 60),
            candidate("off", report(500 * GB, false, false), ComputerState::Awake, false, 60),
            candidate("stale", report(500 * GB, true, false), ComputerState::Offline, false, 30 * 24 * 3600),
            candidate("usb", report(500 * GB, true, true), ComputerState::Awake, false, 60),
        ]
    };
    let ranked = rank(GB, false, all());
    let order: Vec<&str> = ranked.iter().map(|o| o.device_id.as_str()).collect();
    assert_eq!(&order[..3], ["mac", "usb", "cloud"], "{ranked:#?}");
    assert!(ranked[..3].iter().all(|o| o.eligible && o.score > 0));
    assert!(ranked[1].reason.contains("removable"), "{}", ranked[1].reason);
    assert!(ranked[2].reason.contains("cloud computer"), "{}", ranked[2].reason);
    let why = |id: &str| ranked.iter().find(|o| o.device_id == id).unwrap().clone();
    // 1 GB needs 1.1 GB with headroom; 1.05 GB free is not enough.
    assert!(!why("tight").eligible && why("tight").reason.starts_with("Not enough space"), "{:?}", why("tight"));
    assert!(!why("off").eligible && why("off").reason.contains("turned off"));
    assert!(!why("stale").eligible && why("stale").reason.contains("over a week"));
    assert!(ranked.iter().filter(|o| !o.eligible).all(|o| o.score == 0));
    assert!(ranked[3..].iter().all(|o| !o.eligible), "ineligible options come last");

    // A protected folder never goes to a cloud computer.
    let protected = rank(GB, true, all());
    let cloud = protected.iter().find(|o| o.device_id == "cloud").unwrap();
    assert!(!cloud.eligible && cloud.reason.contains("own devices"), "{cloud:?}");
}

#[test]
fn the_peer_relay_reaches_only_replica_host_routes() {
    let get = Method::GET;
    let put = Method::PUT;
    let post = Method::POST;
    assert!(peer_route_allowed(&post, "rpl_abc", "have-need"));
    assert!(peer_route_allowed(&get, "rpl_abc", "head"));
    assert!(peer_route_allowed(&put, "rpl_abc", "/head"));
    assert!(peer_route_allowed(&put, "rpl_abc", "blobs/9f86d081884c7d65"));
    for (m, id, rest) in [
        (&get, "rpl_abc", "blobs/../../items/x/content"),
        (&get, "rpl_abc", "blobs/a/b"),
        (&Method::DELETE, "rpl_abc", "head"),
        (&get, "rpl_abc", "have-need"),
        (&post, "rpl_abc", "sync"),
        (&get, "../roots", "head"),
        (&get, "", "head"),
    ] {
        assert!(!peer_route_allowed(m, id, rest), "{m} {id} {rest}");
    }
}

async fn apply_migration_143(db: &sqlx::PgPool) {
    let sql = include_str!("../../migrations_pg/143_computer_files_wake.sql").replace("public.", "");
    sqlx::raw_sql(&sql).execute(db).await.expect("migration 143");
}

/// A runtime device with Files capability and a real (prefixed) device token.
async fn runtime(db: &sqlx::PgPool, id: &str, owner: &str) -> String {
    seed_runtime_device(db, id, owner).await;
    let token = format!("{}{id}-secret", crate::routes::runtime_pairing::DEVICE_TOKEN_PREFIX);
    sqlx::query("UPDATE runtime_devices SET capabilities = $2, credential_hash = $3 WHERE id = $1")
        .bind(id)
        .bind(r#"["runtime:connect","runtime:files","runtime:execute"]"#)
        .bind(crate::routes::runtime_pairing::sha256_hex(token.as_bytes()))
        .execute(db)
        .await
        .unwrap();
    token
}

async fn peer(state: &Arc<ApiState>, method: &str, path: &str, token: Option<&str>, body: &[u8]) -> (StatusCode, serde_json::Value) {
    let mut b = axum::http::Request::builder().method(method).uri(path).header("content-type", "application/octet-stream");
    if let Some(t) = token {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    let res = routes().with_state(state.clone()).oneshot(b.body(axum::body::Body::from(body.to_vec())).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = http_body_util::BodyExt::collect(res.into_body()).await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

#[tokio::test]
#[serial_test::serial]
async fn the_peer_relay_forwards_to_a_connected_host_and_never_wakes_an_asleep_one() {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    let src = runtime(&state.db, "rt_peer_src", DEV_USER).await;
    runtime(&state.db, "rt_peer_host", DEV_USER).await;
    runtime(&state.db, "rt_peer_theirs", "user_b").await;
    std::env::set_var("ALLTERNIT_FILES_DRIVE_ENABLED", "1");
    let head = "/api/v1/computer-files/peer/rt_peer_host/replicas/rpl_1/head";
    let wakes = || crate::services::provisioning::WAKE_FOR_DEVICE_CALLS.load(Ordering::SeqCst);

    // No token, a route that isn't a replica route, another user's host.
    assert_eq!(peer(&state, "PUT", head, None, b"x").await.0, StatusCode::UNAUTHORIZED);
    let content = "/api/v1/computer-files/peer/rt_peer_host/replicas/rpl_1/content";
    assert_eq!(peer(&state, "GET", content, Some(&src), b"").await.0, StatusCode::FORBIDDEN);
    let theirs = "/api/v1/computer-files/peer/rt_peer_theirs/replicas/rpl_1/head";
    assert_eq!(peer(&state, "PUT", theirs, Some(&src), b"x").await.0, StatusCode::NOT_FOUND);

    // Asleep host (no relay connection): 503, never a wake.
    let before = wakes();
    let (status, body) = peer(&state, "PUT", head, Some(&src), b"sealed-head").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(matches!(body["code"].as_str(), Some("asleep" | "offline")), "{body}");
    assert_eq!(wakes(), before, "the peer relay never wakes a computer");

    // Connected host: the request arrives as the host's own replica route,
    // with the source's device token and no wake ticket.
    let (connection, mut outgoing) = register_test_connection("rt_peer_host").await;
    let seen = tokio::spawn(async move {
        while let Some(message) = outgoing.recv().await {
            if let CloudMessage::Request { request_id, method, path, headers, body, body_encoding } = message {
                crate::routes::runtime_relay::answer_test_request(&connection, &request_id, 204, "").await;
                return Some((method, path, headers, body, body_encoding));
            }
        }
        None
    });
    let (status, _) = peer(&state, "PUT", head, Some(&src), b"sealed-head").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (method, path, headers, body, encoding) = seen.await.unwrap().expect("relayed");
    assert_eq!((method.as_str(), path.as_str()), ("PUT", "/api/v1/files/v2/replicas/rpl_1/head"));
    assert_eq!(headers.get("authorization").map(String::as_str), Some(format!("Bearer {src}").as_str()));
    assert!(!headers.contains_key("x-allternit-wake-ticket"));
    assert_eq!((encoding.as_str(), STANDARD.decode(body).unwrap()), ("base64", b"sealed-head".to_vec()));
    assert_eq!(wakes(), before);
}

#[tokio::test]
#[serial_test::serial]
async fn placement_reads_the_heartbeat_reports_of_the_users_other_computers() {
    use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
    let _g = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
    std::env::set_var("ALLTERNIT_FILES_DRIVE_ENABLED", "1");
    snapshot::test_store::set(None);
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    apply_migration_143(&state.db).await;
    for (id, owner) in [("rt_pl_src", DEV_USER), ("rt_pl_big", DEV_USER), ("rt_pl_small", DEV_USER), ("rt_pl_theirs", "user_b")] {
        runtime(&state.db, id, owner).await;
    }
    let mut source = report(10 * GB, true, false);
    source.root_usage.insert("rt_inbox".into(), 2 * GB);
    for (id, owner, r) in [
        ("rt_pl_src", DEV_USER, source),
        ("rt_pl_big", DEV_USER, report(400 * GB, true, false)),
        ("rt_pl_small", DEV_USER, report(GB, true, false)),
        ("rt_pl_theirs", "user_b", report(900 * GB, true, false)),
    ] {
        let heartbeat = serde_json::json!({ "filesStorage": r });
        cf::store_storage_report(&state.db, owner, id, &heartbeat).await;
    }
    let req = crate::routes::test_support::authed_request("GET", "/api/v1/computer-files/rt_pl_src/placement?root_id=rt_inbox", "");
    let res = routes().with_state(state.clone()).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = http_body_util::BodyExt::collect(res.into_body()).await.unwrap().to_bytes();
    let body: PlacementResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!((body.computer_id.as_str(), body.needed_bytes), ("rt_pl_src", 2 * GB));
    let ids: Vec<&str> = body.options.iter().map(|o| o.device_id.as_str()).collect();
    assert_eq!(ids, ["rt_pl_big", "rt_pl_small"], "the source and other users' computers are never options");
    assert!(body.options[0].eligible && !body.options[1].eligible);

    let req = crate::routes::test_support::authed_request("GET", "/api/v1/computer-files/rt_pl_src/placement", "");
    assert_eq!(routes().with_state(state.clone()).oneshot(req).await.unwrap().status(), StatusCode::BAD_REQUEST);
}
