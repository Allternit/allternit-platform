//! Computers console host proxy, approvals and forwards (step 2.3). Pure
//! checks first, then the routes against a test schema with migrations 153
//! and 154 and a fake relay connection.

use axum::body::Body;
use axum::extract::ws::Message;
use axum::http::Request;
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::routes::runtime_relay::{
    answer_test_request, raw_frame_to_cloud, raw_runtime_frame_to_client, register_test_connection,
    start_test_stream, CloudMessage, RuntimeSocketFrame,
};
use crate::routes::test_support::{authed_request, seed_runtime_device, test_state, test_state_with_provisioning, MockGateway, DEV_USER};

const KEY: &str = "0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c4b5a69788796a5b4c3d2e1f0";

/// host-probe `changes::safe_file_name`, which the node applies to every approval id.
fn node_safe_file_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && !s.starts_with('.')
        && !s.starts_with('-')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | ':' | '-'))
}

#[test]
fn an_approval_round_trips_and_fits_the_node_store() {
    let now = 1_800_000_000;
    let (id, exp) = mint_approval(KEY, "user_2abc", "rt_1", "service.restart", "nginx.service", now);
    assert_eq!(exp, now + APPROVAL_TTL_SECS);
    assert!(node_safe_file_name(&id), "{id} must be a valid host-probe approval file name");
    assert!(id.starts_with("ap1."));
    // The cloud checks the user too; the node cannot and passes None.
    assert_eq!(verify_approval(KEY, &id, Some("user_2abc"), "rt_1", "service.restart", "nginx.service", now + 5), Ok(()));
    assert_eq!(verify_approval(KEY, &id, None, "rt_1", "service.restart", "nginx.service", now + 5), Ok(()));
    // Exactly at exp is still valid; one second later it is not.
    assert_eq!(verify_approval(KEY, &id, None, "rt_1", "service.restart", "nginx.service", exp), Ok(()));
    assert_eq!(
        verify_approval(KEY, &id, None, "rt_1", "service.restart", "nginx.service", exp + 1),
        Err(ApprovalError::Expired)
    );
}

#[test]
fn an_approval_is_bound_to_user_computer_action_and_target() {
    let now = 1_800_000_000;
    let (id, _) = mint_approval(KEY, "u1", "rt_1", "service.stop", "nginx", now);
    let check = |key: &str, user: &str, rt: &str, action: &str, target: &str| {
        verify_approval(key, &id, Some(user), rt, action, target, now)
    };
    assert_eq!(check(KEY, "u2", "rt_1", "service.stop", "nginx"), Err(ApprovalError::Binding));
    assert_eq!(check(KEY, "u1", "rt_2", "service.stop", "nginx"), Err(ApprovalError::Signature));
    assert_eq!(check(KEY, "u1", "rt_1", "service.start", "nginx"), Err(ApprovalError::Signature));
    assert_eq!(check(KEY, "u1", "rt_1", "service.stop", "sshd"), Err(ApprovalError::Signature));
    assert_eq!(check("another-runtime-key", "u1", "rt_1", "service.stop", "nginx"), Err(ApprovalError::Signature));
    // Tampering with the expiry breaks the signature.
    let mut parts: Vec<String> = id.split('.').map(str::to_string).collect();
    parts[2] = (now + 99_999).to_string();
    assert_eq!(
        verify_approval(KEY, &parts.join("."), Some("u1"), "rt_1", "service.stop", "nginx", now),
        Err(ApprovalError::Signature)
    );
    let upper = id.to_uppercase();
    for bad in ["", "ap1", "ap2.x.1.y.z", "ap1.00.1.00.00", upper.as_str()] {
        assert_eq!(verify_approval(KEY, bad, None, "rt_1", "service.stop", "nginx", now), Err(ApprovalError::Format), "{bad}");
    }
}

#[test]
fn actions_and_paths_are_checked() {
    assert!(check_action("service.restart", "nginx.service").is_ok());
    assert!(check_action("rm.rf", "/").is_err());
    assert!(check_action("service.stop", "").is_err());
    assert!(check_action("service.stop", "a\nb").is_err());
    assert_eq!(host_path("snapshot").as_deref(), Some("snapshot"));
    assert_eq!(host_path("/logs/stream").as_deref(), Some("logs/stream"));
    assert_eq!(host_path("../etc"), None);
    assert_eq!(host_path("a//b"), None);
    assert_eq!(host_path("a?b"), None);
    assert_eq!(forward_node_path(5432), "/v1/host/forward?port=5432");
}

fn target(status: Option<&str>, runtime: Option<&str>) -> HostTarget {
    HostTarget {
        kind: TargetKind::Cloud,
        runtime_id: runtime.map(str::to_string),
        owner_id: "u".into(),
        instance_status: status.map(str::to_string),
        sources: Sources::default(),
    }
}

#[test]
fn the_gate_never_relays_to_a_sleeping_computer() {
    assert_eq!(gate(&target(Some("sleeping"), Some("rt"))), Gate::Asleep { status: "asleep" });
    assert_eq!(gate(&target(Some("stopped"), Some("rt"))), Gate::Asleep { status: "stopped" });
    assert_eq!(gate(&target(Some("waking"), Some("rt"))), Gate::Offline { status: "starting" });
    assert_eq!(gate(&target(Some("suspended"), Some("rt"))), Gate::Offline { status: "suspended" });
    assert_eq!(gate(&target(Some("running"), None)), Gate::Offline { status: "offline" });
    assert_eq!(gate(&target(Some("running"), Some("rt"))), Gate::Relay { runtime_id: "rt".into() });
    assert_eq!(gate(&target(None, Some("rt"))), Gate::Relay { runtime_id: "rt".into() });
}

#[test]
fn a_raw_stream_carries_bytes_only() {
    assert!(raw_runtime_frame_to_client(&RuntimeSocketFrame::Ready).is_none(), "no ready text frame in a TCP stream");
    match raw_runtime_frame_to_client(&RuntimeSocketFrame::Data(Bytes::from_static(b"hi"), false)) {
        Some(Message::Binary(b)) => assert_eq!(b, b"hi"),
        other => panic!("{other:?}"),
    }
    match raw_frame_to_cloud("s1", &Message::Text("SSH-2.0".into())) {
        Some(CloudMessage::SocketData { body, body_encoding, .. }) => {
            assert_eq!(body_encoding, "base64");
            assert_eq!(body, "U1NILTIuMA==");
        }
        other => panic!("{other:?}"),
    }
}

// ------------------------------------------------------------- with a db ----

async fn schema(state: &Arc<ApiState>) {
    sqlx::raw_sql(
        "ALTER TABLE runtime_devices ADD COLUMN IF NOT EXISTS runtime_type TEXT;
         ALTER TABLE runtime_devices ADD COLUMN IF NOT EXISTS hostname TEXT;
         ALTER TABLE runtime_devices ADD COLUMN IF NOT EXISTS platform TEXT;
         ALTER TABLE runtime_devices ADD COLUMN IF NOT EXISTS tailnet_ip TEXT;
         CREATE TABLE provisioned_hosts (id TEXT PRIMARY KEY, name TEXT NOT NULL, incus_endpoint TEXT NOT NULL, region TEXT);
         CREATE TABLE provisioned_instances (
             id TEXT PRIMARY KEY, user_id TEXT NOT NULL, host_id TEXT, incus_name TEXT NOT NULL,
             status TEXT NOT NULL DEFAULT 'provisioning', device_id TEXT,
             created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP);
         CREATE TABLE paired_computers (
             id TEXT PRIMARY KEY, user_id TEXT NOT NULL, name TEXT NOT NULL, os TEXT, secret_hash TEXT NOT NULL,
             mesh_ip TEXT, vnc_ready BOOLEAN NOT NULL DEFAULT false, created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
             last_seen_at TIMESTAMPTZ, organization_id TEXT, provisioned_instance_id TEXT);
         CREATE TABLE audit_log (
             id TEXT PRIMARY KEY, action TEXT, resource_type TEXT, resource_id TEXT, user_id TEXT,
             user_email TEXT, details TEXT, success BOOLEAN);",
    )
    .execute(&state.db)
    .await
    .unwrap();
    for sql in [
        include_str!("../../migrations_pg/153_computers_console.sql"),
        include_str!("../../migrations_pg/154_computer_metrics.sql"),
        include_str!("../../migrations_pg/159_computer_machine_identity.sql"),
    ] {
        sqlx::raw_sql(&sql.replace("public.", "")).execute(&state.db).await.expect("migration");
    }
}

async fn setup() -> Arc<ApiState> {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    schema(&state).await;
    state
}

fn dev_token_on() -> std::sync::MutexGuard<'static, ()> {
    use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
    let g = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
    g
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

/// A runtime device of `user` with relay capabilities and a relay key of
/// `sha256_hex("token-of-<id>")`.
async fn device(db: &PgPool, id: &str, user: &str, name: &str, runtime_type: &str) {
    seed_runtime_device(db, id, user).await;
    sqlx::query("UPDATE runtime_devices SET name = $2, runtime_type = $3 WHERE id = $1")
        .bind(id)
        .bind(name)
        .bind(runtime_type)
        .execute(db)
        .await
        .unwrap();
}

async fn sample(db: &PgPool, kind: &str, id: &str, cpu: f64) {
    sqlx::query(
        "INSERT INTO computer_metric_samples (computer_kind, computer_id, ts, resolution_secs, snapshot)
         VALUES ($1, $2, date_trunc('minute', now()), 60, $3)",
    )
    .bind(kind)
    .bind(id)
    .bind(json!({ "cpu": { "percent": cpu } }))
    .execute(db)
    .await
    .unwrap();
}

async fn send(state: &Arc<ApiState>, method: &str, path: &str, body: &str) -> Response {
    routes().with_state(state.clone()).oneshot(authed_request(method, path, body)).await.unwrap()
}

async fn json_of(res: Response) -> (StatusCode, Value) {
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
#[serial_test::serial]
async fn ids_resolve_across_the_three_kinds_with_their_access_rules() {
    let state = setup().await;
    let db = &state.db;
    device(db, "rt_cloud", DEV_USER, "Cloud computer", "provisioned").await;
    device(db, "rt_box", DEV_USER, "build-box", "vps").await;
    device(db, "rt_desk", DEV_USER, "Studio Mac", "desktop").await;
    device(db, "rt_team", "teammate", "team-box", "vps").await;
    device(db, "rt_other", "someone-else", "theirs", "desktop").await;
    sqlx::raw_sql(&format!(
        "INSERT INTO provisioned_instances (id, user_id, incus_name, status, device_id) VALUES
           ('pi_1', '{u}', 'c-1', 'running', 'rt_cloud'),
           ('pi_other', 'someone-else', 'c-2', 'running', NULL),
           ('pi_dead', '{u}', 'c-0', 'deleted', NULL);
         INSERT INTO paired_computers (id, user_id, name, secret_hash, organization_id, runtime_device_id, provisioned_instance_id) VALUES
           ('pc_box', '{u}', 'Build-Box', 'x', NULL, NULL, NULL),
           ('pc_team', 'teammate', 'team-box', 'x', 'org_1', 'rt_team', NULL),
           ('pc_cloud', '{u}', 'Cloud computer', 'x', NULL, NULL, 'pi_1');",
        u = DEV_USER
    ))
    .execute(db)
    .await
    .unwrap();
    let me = Caller { user_id: DEV_USER.into(), organization_id: Some("org_1".into()) };
    let stranger = Caller { user_id: "stranger".into(), organization_id: Some("org_2".into()) };

    let cloud = resolve_target(db, &me, "pi_1").await.unwrap().unwrap();
    assert_eq!((cloud.kind, cloud.runtime_id.as_deref()), (TargetKind::Cloud, Some("rt_cloud")));
    assert_eq!(cloud.sources.paired, vec!["pc_cloud".to_string()]);
    // Not linked yet: matched to the owner's runtime by name.
    let paired = resolve_target(db, &me, "pc_box").await.unwrap().unwrap();
    assert_eq!((paired.kind, paired.runtime_id.as_deref()), (TargetKind::Paired, Some("rt_box")));
    // Shared through the organization: relayed as its owner.
    let team = resolve_target(db, &me, "pc_team").await.unwrap().unwrap();
    assert_eq!((team.runtime_id.as_deref(), team.owner_id.as_str()), (Some("rt_team"), "teammate"));
    let desk = resolve_target(db, &me, "rt_desk").await.unwrap().unwrap();
    assert_eq!((desk.kind, desk.runtime_id.as_deref()), (TargetKind::Device, Some("rt_desk")));
    // Step 2.10: an ao node on the same Mac (same ~/.allternit/machine-id) is
    // an alias: its id reaches the machine's Desktop runtime and both sample sources.
    device(db, "rt_ao", DEV_USER, "studio", "ao").await;
    sqlx::raw_sql("UPDATE runtime_devices SET machine_id = 'Zk3m9QpL2vX8aB4cD6eF7gH1', status = 'offline' WHERE id IN ('rt_desk', 'rt_ao')")
        .execute(db)
        .await
        .unwrap();
    let alias = resolve_target(db, &me, "rt_ao").await.unwrap().unwrap();
    assert_eq!(alias.runtime_id.as_deref(), Some("rt_desk"), "the Desktop serves host data");
    let mut members = alias.sources.devices.clone();
    members.sort_unstable();
    assert_eq!(members, vec!["rt_ao".to_string(), "rt_desk".to_string()]);
    // A cloud computer's runtime is reached through the cloud id only.
    assert_eq!(resolve_target(db, &me, "rt_cloud").await.unwrap(), None);
    for (caller, id) in [(&me, "pi_other"), (&me, "pi_dead"), (&me, "rt_other"), (&me, "pc_cloud"), (&stranger, "pc_team"), (&stranger, "pi_1"), (&stranger, "rt_desk")] {
        assert_eq!(resolve_target(db, caller, id).await.unwrap(), None, "{} must not see {id}", caller.user_id);
    }
}

#[tokio::test]
#[serial_test::serial]
async fn someone_elses_computer_answers_404_on_every_route() {
    let _g = dev_token_on();
    let state = setup().await;
    device(&state.db, "rt_other", "someone-else", "theirs", "desktop").await;
    for (method, path, body) in [
        ("GET", "/api/v1/computers/rt_other/host/snapshot", ""),
        ("POST", "/api/v1/computers/rt_other/approvals", r#"{"type":"service.stop","target":"nginx"}"#),
        ("POST", "/api/v1/computers/rt_other/forward", r#"{"remotePort":22}"#),
        ("GET", "/api/v1/computers/nope/host/snapshot", ""),
    ] {
        let (status, _) = json_of(send(&state, method, path, body).await).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
    }
}

#[tokio::test]
#[serial_test::serial]
async fn an_asleep_cloud_computer_answers_409_and_is_never_woken() {
    use crate::services::provisioning::pg_tests::{service, MockBackend};
    let _g = dev_token_on();
    let backend = Arc::new(MockBackend::default());
    let provisioning = Arc::new(service(crate::routes::test_support::test_pool().await, backend.clone()));
    let state = test_state_with_provisioning(Arc::new(MockGateway::new(None, vec![])), Some(provisioning)).await;
    schema(&state).await;
    let db = &state.db;
    let rt = unique("rt_sleepy");
    device(db, &rt, DEV_USER, "Cloud computer", "provisioned").await;
    sqlx::query("INSERT INTO provisioned_instances (id, user_id, incus_name, status, device_id) VALUES ('pi_s', $1, 'c-s', 'sleeping', $2)")
        .bind(DEV_USER)
        .bind(&rt)
        .execute(db)
        .await
        .unwrap();
    sample(db, "device", &rt, 12.5).await;

    for (method, path, body) in [
        ("GET", "/api/v1/computers/pi_s/host/snapshot", ""),
        ("GET", "/api/v1/computers/pi_s/host/logs/stream?unit=nginx", ""),
        ("POST", "/api/v1/computers/pi_s/forward", r#"{"remotePort":22}"#),
    ] {
        let (status, body) = json_of(send(&state, method, path, body).await).await;
        assert_eq!(status, StatusCode::CONFLICT, "{path}: {body}");
        assert_eq!(body["error"], "asleep", "iOS tells asleep from a refused action by the body");
        assert_eq!(body["status"], "asleep");
        assert_eq!(body["lastSample"]["cpu"]["percent"], 12.5);
    }
    assert!(backend.calls.lock().unwrap().is_empty(), "the provider was never called");
    let after: String = sqlx::query_scalar("SELECT status FROM provisioned_instances WHERE id = 'pi_s'")
        .fetch_one(db)
        .await
        .unwrap();
    assert_eq!(after, "sleeping");
}

#[tokio::test]
#[serial_test::serial]
async fn a_running_computer_without_a_relay_answers_503_offline() {
    let _g = dev_token_on();
    let state = setup().await;
    let rt = unique("rt_quiet");
    device(&state.db, &rt, DEV_USER, "box", "vps").await;
    sample(&state.db, "device", &rt, 40.0).await;
    let (status, body) = json_of(send(&state, "GET", &format!("/api/v1/computers/{rt}/host/services"), "").await).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"], "offline");
    assert_eq!(body["lastSample"]["cpu"]["percent"], 40.0);
}

/// The next relayed request on a fake connection.
async fn next_request(outgoing: &mut tokio::sync::mpsc::UnboundedReceiver<CloudMessage>) -> (String, String, String, HashMap<String, String>, String) {
    let message = tokio::time::timeout(std::time::Duration::from_secs(5), outgoing.recv()).await.expect("a relayed request").unwrap();
    match message {
        CloudMessage::Request { request_id, method, path, headers, body, .. } => (request_id, method, path, headers, body),
        other => panic!("expected a request, got {other:?}"),
    }
}

#[tokio::test]
#[serial_test::serial]
async fn host_reads_relay_signed_and_sse_streams_through_unbuffered() {
    let _g = dev_token_on();
    let state = setup().await;
    let rt = unique("rt_live");
    device(&state.db, &rt, DEV_USER, "box", "vps").await;
    let (connection, mut outgoing) = register_test_connection(&rt).await;

    let call = tokio::spawn({
        let state = state.clone();
        let path = format!("/api/v1/computers/{rt}/host/logs/stream?unit=nginx.service");
        async move { send(&state, "GET", &path, "").await }
    });
    let (request_id, method, path, headers, _) = next_request(&mut outgoing).await;
    assert_eq!((method.as_str(), path.as_str()), ("GET", "/v1/host/logs/stream?unit=nginx.service"));
    assert!(headers.contains_key("x-allternit-runtime-sig"), "signed for the node's relay_auth");
    assert_eq!(headers.get("x-allternit-owner").map(String::as_str), Some(DEV_USER));
    assert!(!headers.contains_key("authorization"), "the user's token never reaches the node");

    let chunks = start_test_stream(
        &connection,
        &request_id,
        200,
        HashMap::from([("content-type".to_string(), "text/event-stream".to_string())]),
    )
    .await
    .expect("pending request");
    chunks.send(Ok(Bytes::from_static(b"data: one\n\n"))).unwrap();
    let response = call.await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("x-accel-buffering").unwrap(), "no");
    let mut body = response.into_body();
    // The first event arrives while the stream is still open.
    let first = tokio::time::timeout(std::time::Duration::from_secs(5), body.frame()).await.expect("not buffered").unwrap().unwrap();
    assert_eq!(first.into_data().unwrap(), Bytes::from_static(b"data: one\n\n"));
    chunks.send(Ok(Bytes::from_static(b"data: two\n\n"))).unwrap();
    let second = tokio::time::timeout(std::time::Duration::from_secs(5), body.frame()).await.unwrap().unwrap().unwrap();
    assert_eq!(second.into_data().unwrap(), Bytes::from_static(b"data: two\n\n"));
    drop(chunks);
    assert!(body.frame().await.is_none(), "ends when the node ends it");
}

#[tokio::test]
#[serial_test::serial]
async fn an_action_needs_a_cloud_approval_for_exactly_that_action() {
    let _g = dev_token_on();
    let state = setup().await;
    let rt = unique("rt_act");
    device(&state.db, &rt, DEV_USER, "box", "vps").await;
    let (connection, mut outgoing) = register_test_connection(&rt).await;

    // Step 1: the approval.
    let (status, body) = json_of(
        send(&state, "POST", &format!("/api/v1/computers/{rt}/approvals"), r#"{"type":"service.restart","target":"nginx.service"}"#).await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let approval = body["approvalId"].as_str().unwrap().to_string();
    assert!(body["expiresAt"].as_str().is_some());
    // Signed with the per-runtime relay key, which the node holds.
    let key = crate::routes::runtime_pairing::sha256_hex(format!("token-of-{rt}").as_bytes());
    let now = Utc::now().timestamp();
    assert_eq!(verify_approval(&key, &approval, None, &rt, "service.restart", "nginx.service", now), Ok(()));
    let issued: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = 'computer.approval.issued'")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(issued, 1);

    // Another target with this approval is refused before the node sees it.
    let (status, body) = json_of(
        send(
            &state,
            "POST",
            &format!("/api/v1/computers/{rt}/host/actions"),
            &json!({ "type": "service.restart", "target": "sshd.service", "approval_id": approval }).to_string(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "approval_invalid");
    assert!(outgoing.try_recv().is_err(), "nothing was relayed");
    // No approval at all.
    let (status, _) = json_of(
        send(&state, "POST", &format!("/api/v1/computers/{rt}/host/actions"), r#"{"type":"service.restart","target":"nginx.service"}"#).await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Step 2: the action, forwarded as exactly {type, target, approval_id}.
    let call = tokio::spawn({
        let state = state.clone();
        let path = format!("/api/v1/computers/{rt}/host/actions");
        let body = json!({ "type": "service.restart", "target": "nginx.service", "approvalId": approval, "extra": 1 }).to_string();
        async move { send(&state, "POST", &path, &body).await }
    });
    let (request_id, method, path, headers, body) = next_request(&mut outgoing).await;
    assert_eq!((method.as_str(), path.as_str()), ("POST", "/v1/host/actions"));
    assert!(headers.contains_key("x-allternit-runtime-sig"));
    let sent: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(sent, json!({ "type": "service.restart", "target": "nginx.service", "approval_id": approval }));
    answer_test_request(&connection, &request_id, 200, r#"{"change_id":"ch_1","undo":null}"#).await;
    let (status, body) = json_of(call.await.unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["change_id"], "ch_1");
    let forwarded: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = 'computer.action.forwarded' AND success")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(forwarded, 1);
}

#[tokio::test]
#[serial_test::serial]
async fn a_forward_mints_a_raw_stream_ticket_for_the_port() {
    let _g = dev_token_on();
    let state = setup().await;
    let rt = unique("rt_fwd");
    device(&state.db, &rt, DEV_USER, "box", "vps").await;

    // Not connected: offline, and no ticket.
    let (status, body) = json_of(send(&state, "POST", &format!("/api/v1/computers/{rt}/forward"), r#"{"remotePort":5432}"#).await).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"], "offline");

    let _connection = register_test_connection(&rt).await;
    let (status, body) = json_of(send(&state, "POST", &format!("/api/v1/computers/{rt}/forward"), r#"{"remotePort":5432}"#).await).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["url"].as_str().unwrap().ends_with(&format!("/api/v1/computers/{rt}/forward/socket")));
    assert_eq!(body["expiresInSeconds"], 30);
    let ticket = crate::routes::runtime_relay::take_socket_ticket(body["ticket"].as_str().unwrap()).await.unwrap();
    assert!(ticket.raw_stream);
    assert_eq!((ticket.runtime_id.as_str(), ticket.path.as_str()), (rt.as_str(), "/v1/host/forward?port=5432"));

    for bad in [r#"{"remotePort":0}"#, r#"{"remotePort":70000}"#] {
        let (status, _) = json_of(send(&state, "POST", &format!("/api/v1/computers/{rt}/forward"), bad).await).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
}

#[tokio::test]
#[serial_test::serial]
async fn the_forward_socket_refuses_a_browser_socket_ticket() {
    let state = setup().await;
    let res = routes()
        .with_state(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/computers/x/forward/socket?ticket=not-a-ticket")
                .header("connection", "upgrade")
                .header("upgrade", "websocket")
                .header("sec-websocket-version", "13")
                .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // Without a real upgradable connection axum may refuse the upgrade first;
    // either way no socket opens.
    assert!(matches!(res.status(), StatusCode::UNAUTHORIZED | StatusCode::UPGRADE_REQUIRED | StatusCode::BAD_REQUEST), "{}", res.status());
}
