//! Computers console list (step 2.1). Pure joins first, then the route
//! against a test schema with migration 153 applied.

use axum::http::StatusCode;
use tower::ServiceExt;

use super::*;
use crate::routes::test_support::{authed_request, test_state, MockGateway, DEV_USER};

fn at(minutes_ago: i64) -> Option<DateTime<Utc>> {
    Some(Utc::now() - Duration::minutes(minutes_ago))
}

fn device(id: &str, name: &str, runtime_type: &str, platform: &str) -> DeviceRow {
    DeviceRow {
        id: id.into(),
        name: Some(name.into()),
        runtime_type: Some(runtime_type.into()),
        hostname: None,
        platform: Some(platform.into()),
        tailnet_ip: None,
        status: Some("online".into()),
        last_seen_at: at(1),
        last_sample: None,
        last_sample_at: None,
    }
}

fn paired(id: &str, name: &str) -> PairedRow {
    PairedRow {
        id: id.into(),
        name: name.into(),
        os: Some("Ubuntu 24.04".into()),
        mesh_ip: Some("100.64.0.9".into()),
        last_seen_at: at(60),
        runtime_device_id: None,
        shared: false,
        last_sample: None,
        last_sample_at: None,
    }
}

fn cloud(id: &str, status: &str, device_id: Option<&str>) -> CloudRow {
    CloudRow {
        id: id.into(),
        status: status.into(),
        device_id: device_id.map(str::to_string),
        device_name: None,
        tailnet_ip: None,
        location: Some("nyc".into()),
        host_region: Some("us-east".into()),
        last_sample: None,
        last_sample_at: None,
    }
}

#[test]
fn a_paired_computer_and_its_runtime_device_are_one_row() {
    let connected: HashSet<String> = ["rt_box".to_string()].into();
    let rows = build_console(
        vec![],
        vec![paired("pc_1", "build-box")],
        vec![device("rt_box", "Build-Box", "vps", "linux")],
        &connected,
        Utc::now(),
    );
    assert_eq!(rows.len(), 1, "matched by owner + name until runtime_device_id is stored");
    let row = &rows[0];
    assert_eq!((row.id.as_str(), row.kind, row.group.as_str()), ("pc_1", "paired", GROUP_SERVERS));
    // The paired row's own report is an hour old, but its runtime holds a relay.
    assert_eq!(row.status, "online");
    assert_eq!((row.reach, row.address.as_deref()), ("mesh", Some("100.64.0.9")));
    assert_eq!((row.os.as_str(), row.os_family), ("Ubuntu 24.04", "linux"));
}

#[test]
fn a_stored_link_wins_and_a_shared_row_never_matches_by_name() {
    let mut linked = paired("pc_1", "mac");
    linked.runtime_device_id = Some("rt_desk".into());
    linked.os = None;
    let mut shared = paired("pc_2", "box");
    shared.shared = true;
    let rows = build_console(
        vec![],
        vec![linked, shared],
        vec![device("rt_desk", "Studio Mac", "desktop", "darwin"), device("rt_box", "box", "vps", "linux")],
        &HashSet::new(),
        Utc::now(),
    );
    assert_eq!(rows.len(), 3);
    // A paired row linked to a Desktop runtime is that desk, listed once.
    assert_eq!((rows[0].id.as_str(), rows[0].kind, rows[0].group.as_str()), ("pc_1", "desktop", GROUP_DESK));
    assert_eq!((rows[0].os.as_str(), rows[0].os_family), ("macOS", "macos"));
    assert_eq!((rows[1].id.as_str(), rows[1].group.as_str()), ("pc_2", GROUP_SHARED));
    assert_eq!((rows[2].id.as_str(), rows[2].kind, rows[2].reach), ("rt_box", "paired", "relay"));
}

#[test]
fn cloud_status_never_reads_asleep_as_online() {
    assert_eq!(cloud_status("running", true), "online");
    assert_eq!(cloud_status("running", false), "offline");
    assert_eq!(cloud_status("sleeping", true), "asleep");
    assert_eq!(cloud_status("waking", false), "starting");
    assert_eq!(cloud_status("provisioning", false), "starting");
    assert_eq!(cloud_status("suspended", false), "suspended");
    let rows = build_console(vec![cloud("pi_1", "sleeping", Some("rt_c"))], vec![], vec![], &HashSet::new(), Utc::now());
    let row = &rows[0];
    assert_eq!((row.kind, row.status.as_str(), row.region.as_deref()), ("cloud", "asleep", Some("nyc")));
    assert_eq!((row.name.as_str(), row.provider.as_str(), row.os_family), ("Cloud computer", "Allternit Cloud", "linux"));
}

#[test]
fn a_stale_heartbeat_reads_offline() {
    let mut d = device("rt_1", "old", "ao", "win32");
    d.last_seen_at = at(30);
    let rows = build_console(vec![], vec![], vec![d], &HashSet::new(), Utc::now());
    assert_eq!((rows[0].status.as_str(), rows[0].os.as_str(), rows[0].os_family), ("offline", "Windows", "windows"));
}

#[test]
fn json_is_camel_case_and_leaves_unknowns_out() {
    let rows = build_console(vec![cloud("pi_1", "running", None)], vec![], vec![], &HashSet::new(), Utc::now());
    let v = serde_json::to_value(&rows[0]).unwrap();
    assert_eq!(v["osFamily"], "linux");
    assert_eq!(v["providerLogo"], "allternit");
    assert_eq!(v["reach"], "relay");
    for absent in ["lastSample", "servicesCount", "lastBackupAt", "address"] {
        assert!(v.get(absent).is_none(), "{absent} is left out until it is known");
    }
}

// ------------------------------------------------------------- with a db ----

/// Minimal column subsets of migrations 001/014/017/066/081 (no FKs to
/// users/billing), plus the real migration 153.
async fn setup() -> Arc<ApiState> {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
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
             last_seen_at TIMESTAMPTZ, organization_id TEXT, provisioned_instance_id TEXT);",
    )
    .execute(&state.db)
    .await
    .unwrap();
    let sql = include_str!("../../migrations_pg/153_computers_console.sql").replace("public.", "");
    sqlx::raw_sql(&sql).execute(&state.db).await.expect("migration 153");
    // Additive and idempotent: the deploy may apply it twice.
    sqlx::raw_sql(&sql).execute(&state.db).await.expect("migration 153 again");
    // The list reads the newest heartbeat sample (step 2.2).
    let sql = include_str!("../../migrations_pg/154_computer_metrics.sql").replace("public.", "");
    sqlx::raw_sql(&sql).execute(&state.db).await.expect("migration 154");
    state
}

async fn call(state: &Arc<ApiState>) -> (StatusCode, serde_json::Value) {
    let res = routes()
        .with_state(state.clone())
        .oneshot(authed_request("GET", "/api/v1/computers/console", ""))
        .await
        .unwrap();
    let status = res.status();
    let bytes = http_body_util::BodyExt::collect(res.into_body()).await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

fn dev_token_on() -> std::sync::MutexGuard<'static, ()> {
    use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
    let g = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
    g
}

#[tokio::test]
#[serial_test::serial]
async fn the_console_lists_only_the_callers_computers_once_each() {
    let _g = dev_token_on();
    let state = setup().await;
    let db = &state.db;
    sqlx::raw_sql(&format!(
        "INSERT INTO provisioned_hosts (id, name, incus_endpoint, region) VALUES ('h1', 'orgo', 'orgo://ws', 'us-east');
         INSERT INTO runtime_devices (id, user_id, name, runtime_type, platform, status, last_seen_at) VALUES
           ('rt_cloud', '{u}', 'Cloud computer', 'provisioned', 'linux', 'online', now()),
           ('rt_desk', '{u}', 'Studio Mac', 'desktop', 'darwin', 'online', now()),
           ('rt_box', '{u}', 'build-box', 'vps', 'linux', 'offline', now() - interval '1 day'),
           ('rt_phone', '{u}', 'iPhone', 'ios', 'ios', 'online', now()),
           ('rt_gone', '{u}', 'old', 'vps', 'linux', 'online', now()),
           ('rt_other', 'someone-else', 'theirs', 'desktop', 'darwin', 'online', now());
         UPDATE runtime_devices SET revoked_at = now() WHERE id = 'rt_gone';
         INSERT INTO provisioned_instances (id, user_id, host_id, incus_name, status, device_id, location) VALUES
           ('pi_1', '{u}', 'h1', 'c-1', 'sleeping', 'rt_cloud', 'lon'),
           ('pi_dead', '{u}', 'h1', 'c-0', 'deleted', NULL, NULL),
           ('pi_other', 'someone-else', 'h1', 'c-2', 'running', NULL, NULL);
         INSERT INTO paired_computers (id, user_id, name, os, secret_hash, mesh_ip, runtime_device_id, provisioned_instance_id) VALUES
           ('pc_box', '{u}', 'build-box', 'Debian 12', 'x', '100.64.0.7', 'rt_box', NULL),
           ('pc_cloud', '{u}', 'Cloud computer', 'linux', 'x', '100.64.0.8', NULL, 'pi_1'),
           ('pc_other', 'someone-else', 'theirs', 'linux', 'x', NULL, NULL, NULL);",
        u = DEV_USER
    ))
    .execute(db)
    .await
    .unwrap();

    let (status, body) = call(&state).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = body["computers"].as_array().unwrap();
    let ids: Vec<&str> = rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["pi_1", "rt_desk", "pc_box"], "no deleted, revoked, phone, self-paired cloud or other users' rows");

    assert_eq!(rows[0]["kind"], "cloud");
    assert_eq!(rows[0]["status"], "asleep", "listing never wakes it and never reads it as online");
    assert_eq!(rows[0]["region"], "lon");
    assert_eq!(rows[1]["kind"], "desktop");
    assert_eq!(rows[1]["osFamily"], "macos");
    assert_eq!(rows[2]["kind"], "paired");
    assert_eq!(rows[2]["os"], "Debian 12");
    assert_eq!(rows[2]["address"], "100.64.0.7");
    assert_eq!(rows[2]["status"], "offline");

    // The instance row is untouched: no wake was started.
    let after: String = sqlx::query_scalar("SELECT status FROM provisioned_instances WHERE id = 'pi_1'")
        .fetch_one(db)
        .await
        .unwrap();
    assert_eq!(after, "sleeping");
}

#[tokio::test]
#[serial_test::serial]
async fn the_console_needs_a_signed_in_caller() {
    let _g = dev_token_on();
    let state = setup().await;
    let res = routes()
        .with_state(state.clone())
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/v1/computers/console")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}
