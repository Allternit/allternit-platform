//! Computers console list (step 2.1). Pure joins first, then the route
//! against a test schema with migrations 153, 154 and 159 applied. Step 2.10:
//! every old pairing path of one machine gives one row.

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
        machine_id: None,
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
        machine_id: None,
        hostname: None,
        platform: None,
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
    assert_eq!(rows[0].runtime_id.as_deref(), Some("rt_desk"));
    assert_eq!(rows[1].runtime_id, None, "a shared row never matches by name");
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
    assert!(v["runtimeId"].is_null(), "runtimeId is always sent, null when there is no runtime");
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
    // One row per machine (step 2.10); additive, applied twice like the deploy may.
    let sql = include_str!("../../migrations_pg/159_computer_machine_identity.sql").replace("public.", "");
    sqlx::raw_sql(&sql).execute(&state.db).await.expect("migration 159");
    sqlx::raw_sql(&sql).execute(&state.db).await.expect("migration 159 again");
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
    // The relay key of each row (step 2.3), not the row id.
    assert_eq!(rows[0]["runtimeId"], "rt_cloud");
    assert_eq!(rows[1]["runtimeId"], "rt_desk");
    assert_eq!(rows[2]["runtimeId"], "rt_box");

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

// ------------------------------------------------ step 2.10: one machine ----

const MID: &str = "Zk3m9QpL2vX8aB4cD6eF7gH1";

fn on_machine(mut d: DeviceRow, mid: Option<&str>, hostname: &str) -> DeviceRow {
    d.machine_id = mid.map(str::to_string);
    d.hostname = Some(hostname.into());
    d
}

#[test]
fn desktop_and_gizzi_pair_on_one_mac_are_one_row() {
    // Desktop registers "<host> Desktop"; `gizzi pair` registers the hostname.
    // Both read ~/.allternit/machine-id.
    let desk = on_machine(device("rt_desk", "joes-mbp Desktop", "desktop", "darwin-arm64"), Some(MID), "joes-mbp.local");
    let cli = on_machine(device("rt_cli", "joes-mbp", "desktop", "darwin-arm64"), Some(MID), "joes-mbp");
    let connected: HashSet<String> = ["rt_cli".to_string()].into();
    let rows = build_console(vec![], vec![], vec![desk, cli], &connected, Utc::now());
    assert_eq!(rows.len(), 1, "{rows:#?}");
    let row = &rows[0];
    assert_eq!(row.id, "rt_desk", "the oldest device names a device-only machine");
    assert_eq!(row.aliases, vec!["rt_cli".to_string()]);
    assert_eq!(row.runtime_id.as_deref(), Some("rt_cli"), "the runtime holding a relay wins");
    assert_eq!(row.machine_key, format!("mid:{MID}"));
    assert_eq!(row.kind, "desktop");
}

#[test]
fn ao_fabric_pair_joins_the_desktop_row() {
    let desk = on_machine(device("rt_desk", "Studio Desktop", "desktop", "darwin-arm64"), Some(MID), "studio");
    let ao = on_machine(device("rt_ao", "studio", "ao", "macos-aarch64"), Some(MID), "studio");
    let rows = build_console(vec![], vec![], vec![desk, ao], &HashSet::new(), Utc::now());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].aliases, vec!["rt_ao".to_string()]);
    assert_eq!(rows[0].runtime_id.as_deref(), Some("rt_desk"), "with no relay up, the Desktop serves host data");
}

#[test]
fn an_old_gizzi_without_a_machine_id_joins_by_hostname() {
    let desk = on_machine(device("rt_desk", "Joes-MBP Desktop", "desktop", "darwin-arm64"), Some(MID), "Joes-MBP.local");
    let old = on_machine(device("rt_old", "joes-mbp", "desktop", "darwin-arm64"), None, "joes-mbp");
    let rows = build_console(vec![], vec![], vec![desk, old], &HashSet::new(), Utc::now());
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert_eq!(rows[0].aliases, vec!["rt_old".to_string()]);
}

#[test]
fn a_code_paired_computer_with_its_machine_id_is_the_desktop_row() {
    let desk = on_machine(device("rt_desk", "Studio Desktop", "desktop", "darwin-arm64"), Some(MID), "studio");
    let mut pc = paired("pc_1", "Studio Mac");
    pc.machine_id = Some(MID.into());
    let rows = build_console(vec![], vec![pc], vec![desk], &HashSet::new(), Utc::now());
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!((row.id.as_str(), row.paired_id.as_deref()), ("pc_1", Some("pc_1")), "a paired row keeps its id");
    assert_eq!(row.aliases, vec!["rt_desk".to_string()]);
    assert_eq!((row.kind, row.runtime_id.as_deref()), ("desktop", Some("rt_desk")));
    assert_eq!(row.address.as_deref(), Some("100.64.0.9"), "the screen-share mesh address is kept");
    assert_eq!((row.member_paired.len(), row.member_devices.len()), (1, 1));
}

#[test]
fn different_machines_never_merge() {
    // Two servers with their own machine ids and the same hostname, and two
    // old ones on a generic hostname: four rows.
    let a = on_machine(device("rt_a", "web", "vps", "linux-x64"), Some("aaaaaaaaaaaaaaaaaaaa"), "web1");
    let b = on_machine(device("rt_b", "web", "vps", "linux-x64"), Some("bbbbbbbbbbbbbbbbbbbb"), "web1");
    let c = on_machine(device("rt_c", "ubuntu", "ao", "linux-x86_64"), None, "ubuntu");
    let d = on_machine(device("rt_d", "ubuntu 2", "ao", "linux-x86_64"), None, "ubuntu");
    let mut shared = paired("pc_team", "web");
    shared.shared = true;
    shared.machine_id = Some("aaaaaaaaaaaaaaaaaaaa".into());
    let rows = build_console(vec![], vec![shared], vec![a, b, c, d], &HashSet::new(), Utc::now());
    let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, vec!["pc_team", "rt_a", "rt_b", "rt_c", "rt_d"], "a shared row never joins the caller's machine");
    assert!(rows.iter().all(|r| r.aliases.is_empty()));
}

#[test]
fn the_owners_name_wins_and_shared_rows_keep_theirs() {
    let desk = on_machine(device("rt_desk", "Studio Desktop", "desktop", "darwin-arm64"), Some(MID), "studio");
    let mut shared = paired("pc_team", "Team box");
    shared.shared = true;
    let mut rows = build_console(vec![], vec![shared], vec![desk], &HashSet::new(), Utc::now());
    let names = vec![(format!("mid:{MID}"), "Studio".to_string()), ("paired:pc_team".to_string(), "Mine now".to_string())];
    apply_names(&mut rows, &names);
    let by_id = |id: &str| rows.iter().find(|r| r.id == id).unwrap().name.clone();
    assert_eq!(by_id("rt_desk"), "Studio");
    assert_eq!(by_id("pc_team"), "Team box");
}

async fn machine_call(state: &Arc<ApiState>, method: &str, uri: &str, body: &str) -> (StatusCode, serde_json::Value) {
    let res = crate::routes::computer_machines::routes()
        .merge(crate::routes::paired_computers::routes())
        .merge(routes())
        .with_state(state.clone())
        .oneshot(authed_request(method, uri, body))
        .await
        .unwrap();
    let status = res.status();
    let bytes = http_body_util::BodyExt::collect(res.into_body()).await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

#[tokio::test]
#[serial_test::serial]
async fn one_machine_is_renamed_and_unpaired_by_any_of_its_ids() {
    let _g = dev_token_on();
    let state = setup().await;
    let db = &state.db;
    sqlx::raw_sql(&format!(
        "INSERT INTO runtime_devices (id, user_id, name, runtime_type, hostname, platform, status, last_seen_at, machine_id) VALUES
           ('rt_desk', '{u}', 'studio Desktop', 'desktop', 'studio.local', 'darwin-arm64', 'online', now(), '{m}'),
           ('rt_cli', '{u}', 'studio', 'desktop', 'studio', 'darwin-arm64', 'online', now(), '{m}'),
           ('rt_ao', '{u}', 'studio', 'ao', 'studio', 'macos-aarch64', 'online', now(), '{m}'),
           ('rt_box', '{u}', 'box', 'vps', 'box1', 'linux-x64', 'online', now(), NULL);
         INSERT INTO paired_computers (id, user_id, name, os, secret_hash, mesh_ip, machine_id) VALUES
           ('pc_mac', '{u}', 'Studio Mac', 'macos', 'x', '100.64.0.5', '{m}');",
        u = DEV_USER,
        m = MID
    ))
    .execute(db)
    .await
    .unwrap();

    let (status, body) = call(&state).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = body["computers"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "Desktop + gizzi + ao + code pairing of one Mac = one row: {body}");
    assert_eq!(rows[0]["id"], "pc_mac");
    assert_eq!(rows[0]["pairedId"], "pc_mac");
    // Same created_at in this fixture, so compare as a set.
    let mut aliases: Vec<&str> = rows[0]["aliases"].as_array().unwrap().iter().filter_map(|a| a.as_str()).collect();
    aliases.sort_unstable();
    assert_eq!(aliases, vec!["rt_ao", "rt_cli", "rt_desk"]);
    assert_eq!(rows[1]["aliases"], serde_json::json!([]));

    // Rename by an alias: the name covers the whole machine.
    let (status, body) = machine_call(&state, "PATCH", "/api/v1/computers/paired/rt_ao", r#"{"name":"  Studio  "}"#).await;
    assert_eq!((status, body["id"].as_str()), (StatusCode::OK, Some("pc_mac")), "{body}");
    let (_, body) = call(&state).await;
    assert_eq!(body["computers"][0]["name"], "Studio");
    let (status, body) = machine_call(&state, "PATCH", "/api/v1/computers/paired/pc_mac", r#"{"name":""}"#).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_name")));
    assert!(body["message"].as_str().is_some_and(|m| !m.is_empty()));

    // Unpair needs confirm, then removes every alias at once.
    let (status, body) = machine_call(&state, "POST", "/api/v1/computers/rt_cli/unpair", "{}").await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::BAD_REQUEST, Some("confirm_required")), "{body}");
    let (status, body) = machine_call(&state, "POST", "/api/v1/computers/rt_cli/unpair", r#"{"confirm":true}"#).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], "pc_mac");
    let live: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runtime_devices WHERE revoked_at IS NULL AND id IN ('rt_desk', 'rt_cli', 'rt_ao', 'rt_box')")
        .fetch_one(db)
        .await
        .unwrap();
    assert_eq!(live, 1, "only the other server's runtime is left");
    let paired: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM paired_computers").fetch_one(db).await.unwrap();
    assert_eq!(paired, 0);
    let names: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM computer_machine_names").fetch_one(db).await.unwrap();
    assert_eq!(names, 0);
    let (_, body) = call(&state).await;
    assert_eq!(body["computers"].as_array().unwrap().len(), 1);

    // Gone ids answer like missing ones, with a plain message.
    let (status, body) = machine_call(&state, "POST", "/api/v1/computers/rt_desk/unpair", r#"{"confirm":true}"#).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::NOT_FOUND, Some("not_found")));
    assert!(body["message"].as_str().is_some_and(|m| !m.is_empty()));
}
