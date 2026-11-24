//! Whole-disk file roots (step 2.11): the path check the relay applies, the
//! roots list, and the setting route against a test schema (migrations 153,
//! 154, 159, 160), including the relay's on/off answer per runtime.

use axum::http::StatusCode;
use serde_json::Value;
use tower::ServiceExt;

use super::*;
use crate::routes::test_support::{authed_request, test_state, MockGateway, DEV_USER};

#[test]
fn paths_inside_the_home_folder_stay_inside_the_drive() {
    for p in [
        "Allternit/Shared/a.txt",
        "~/Allternit",
        "~",
        "/home/joe/Allternit/x",
        "/Users/joe/Allternit",
        "/root/Allternit",
        "/var/home/joe/x",
        "C:\\Users\\joe\\Allternit",
        "c:/Users/joe/x",
        "",
    ] {
        assert!(!outside_home(p), "{p}");
    }
}

#[test]
fn absolute_system_paths_traversal_and_other_homes_are_outside() {
    for p in [
        "/",
        "/etc/passwd",
        "/var/log",
        "/home",
        "/Users",
        "C:\\",
        "D:\\data",
        "C:\\Windows",
        "~root/.ssh",
        "\\\\server\\share",
        "~/../etc",
        "Allternit/../../etc",
    ] {
        assert!(outside_home(p), "{p}");
    }
}

#[test]
fn only_file_routes_are_checked() {
    assert!(is_file_route("/api/v1/files/list?path=/etc"));
    assert!(is_file_route("/api/v1/files/v2/roots"));
    assert!(is_file_route(
        "/api/v1/computers/rt_1/files/download?path=/etc/hosts"
    ));
    assert!(!is_file_route("/api/v1/filesystem"));
    assert!(!is_file_route("/v1/host/snapshot"));
    assert!(!reaches_outside_drive(
        "GET",
        "/api/v1/terminal?path=/etc",
        b""
    ));
}

#[test]
fn a_request_reaches_outside_by_query_body_or_disk_kind() {
    assert!(reaches_outside_drive(
        "GET",
        "/api/v1/files/list?path=%2Fetc",
        b""
    ));
    assert!(!reaches_outside_drive(
        "GET",
        "/api/v1/files/list?path=Allternit%2FShared",
        b""
    ));
    assert!(reaches_outside_drive(
        "POST",
        "/api/v1/files/v2/roots",
        br#"{"name":"Disk","local_path":"/","policy":{}}"#
    ));
    assert!(!reaches_outside_drive(
        "POST",
        "/api/v1/files/v2/roots",
        br#"{"name":"Code","local_path":"~/code","policy":{}}"#
    ));
    assert!(reaches_outside_drive(
        "POST",
        "/api/v1/files/v2/roots",
        br#"{"kind":"disk","local_path":"~"}"#
    ));
    assert!(reaches_outside_drive(
        "POST",
        "/api/v1/files/move",
        br#"{"paths":["~/a","/etc/b"],"dest":"~/x"}"#
    ));
    // File content is never read as a request: a text file that looks like JSON passes.
    assert!(!reaches_outside_drive(
        "PUT",
        "/api/v1/files/v2/roots/r1/content?name=a.json",
        br#"{"path":"/etc"}"#
    ));
}

#[test]
fn roots_are_slash_on_unix_and_drive_letters_on_windows() {
    assert_eq!(disk_roots("linux", None)[0].path, "/");
    assert_eq!(disk_roots("macos", None)[0].path, "/");
    let sample = serde_json::json!({"disk": [{"mount": "D:\\"}, {"mount": "c:\\"}, {"mount": "\\\\?\\Volume{x}"}]});
    let win: Vec<String> = disk_roots("windows", Some(&sample))
        .into_iter()
        .map(|r| r.path)
        .collect();
    assert_eq!(win, vec!["C:\\".to_string(), "D:\\".to_string()]);
    assert_eq!(disk_roots("windows", None)[0].path, "C:\\");
}

// ------------------------------------------------------------- the route ----

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
    for sql in [
        include_str!("../../migrations_pg/153_computers_console.sql"),
        include_str!("../../migrations_pg/154_computer_metrics.sql"),
        include_str!("../../migrations_pg/159_computer_machine_identity.sql"),
        include_str!("../../migrations_pg/160_computer_whole_disk.sql"),
        // Additive and idempotent: the deploy may apply it twice.
        include_str!("../../migrations_pg/160_computer_whole_disk.sql"),
    ] {
        sqlx::raw_sql(&sql.replace("public.", ""))
            .execute(&state.db)
            .await
            .expect("migration");
    }
    sqlx::raw_sql(&format!(
        "INSERT INTO runtime_devices (id, user_id, name, runtime_type, platform, status, last_seen_at) VALUES
           ('rt_box', '{u}', 'build-box', 'vps', 'linux', 'online', now()),
           ('rt_desk', '{u}', 'Studio Mac', 'desktop', 'darwin', 'online', now());",
        u = DEV_USER
    ))
    .execute(&state.db)
    .await
    .unwrap();
    state
}

fn dev_token_on() -> std::sync::MutexGuard<'static, ()> {
    use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
    let g = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
    g
}

async fn call(state: &Arc<ApiState>, method: &str, path: &str, body: &str) -> (StatusCode, Value) {
    let res = routes()
        .merge(crate::routes::computers_console::routes())
        .with_state(state.clone())
        .oneshot(authed_request(method, path, body))
        .await
        .unwrap();
    let status = res.status();
    let bytes = http_body_util::BodyExt::collect(res.into_body())
        .await
        .unwrap()
        .to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

#[tokio::test]
#[serial_test::serial]
async fn whole_disk_is_off_until_the_owner_confirms_and_off_again_closes_the_relay() {
    let _g = dev_token_on();
    let state = setup().await;
    let db = &state.db;
    let outside = br#"{"name":"Disk","local_path":"/","policy":{}}"#;

    // Default: off, no roots, and the relay refuses a path outside the drive.
    let (status, body) = call(&state, "GET", "/api/v1/computers/rt_box/file-roots", "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["wholeDisk"]["enabled"], false);
    assert_eq!(body["wholeDisk"]["available"], true);
    assert_eq!(body["wholeDisk"]["roots"], serde_json::json!([]));
    let refused = refuse_outside_drive(
        db,
        DEV_USER,
        "rt_box",
        "POST",
        "/api/v1/files/v2/roots",
        outside,
    )
    .await;
    assert_eq!(refused.map(|r| r.status()), Some(StatusCode::FORBIDDEN));
    // Inside the drive is never refused.
    assert!(refuse_outside_drive(
        db,
        DEV_USER,
        "rt_box",
        "GET",
        "/api/v1/files/list?path=Allternit",
        b""
    )
    .await
    .is_none());

    // Turning it on needs confirm: true.
    let (status, body) = call(
        &state,
        "PATCH",
        "/api/v1/computers/rt_box/file-roots",
        r#"{"wholeDisk":true}"#,
    )
    .await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("confirm_required"))
    );
    assert!(!whole_disk_on(db, DEV_USER, "rt_box").await);

    let (status, body) = call(
        &state,
        "PATCH",
        "/api/v1/computers/rt_box/file-roots",
        r#"{"wholeDisk":true,"confirm":true}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["wholeDisk"]["enabled"], true);
    assert_eq!(body["wholeDisk"]["roots"][0]["path"], "/");
    assert!(whole_disk_on(db, DEV_USER, "rt_box").await);
    assert!(
        !whole_disk_on(db, "someone-else", "rt_box").await,
        "another user's runtime id never matches"
    );
    assert!(refuse_outside_drive(
        db,
        DEV_USER,
        "rt_box",
        "POST",
        "/api/v1/files/v2/roots",
        outside
    )
    .await
    .is_none());

    // The console row carries the same object.
    let (_, console) = call(&state, "GET", "/api/v1/computers/console", "").await;
    let row = console["computers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["runtimeId"] == "rt_box")
        .cloned()
        .unwrap();
    assert_eq!(row["wholeDisk"]["enabled"], true);

    // Off again: the relay refuses at once.
    let (status, body) = call(
        &state,
        "PATCH",
        "/api/v1/computers/rt_box/file-roots",
        r#"{"wholeDisk":false}"#,
    )
    .await;
    assert_eq!(
        (status, &body["wholeDisk"]["enabled"]),
        (StatusCode::OK, &Value::Bool(false))
    );
    assert!(!whole_disk_on(db, DEV_USER, "rt_box").await);
    let refused = refuse_outside_drive(
        db,
        DEV_USER,
        "rt_box",
        "POST",
        "/api/v1/files/v2/roots",
        outside,
    )
    .await;
    assert_eq!(refused.map(|r| r.status()), Some(StatusCode::FORBIDDEN));
}

#[tokio::test]
#[serial_test::serial]
async fn desktops_and_unknown_computers_cannot_turn_it_on() {
    let _g = dev_token_on();
    let state = setup().await;
    let (status, body) = call(
        &state,
        "PATCH",
        "/api/v1/computers/rt_desk/file-roots",
        r#"{"wholeDisk":true,"confirm":true}"#,
    )
    .await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::CONFLICT, Some("not_a_server"))
    );
    let (status, _) = call(
        &state,
        "PATCH",
        "/api/v1/computers/rt_nope/file-roots",
        r#"{"wholeDisk":true,"confirm":true}"#,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = call(
        &state,
        "PATCH",
        "/api/v1/computers/rt_box/file-roots",
        r#"{}"#,
    )
    .await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_body"))
    );
}

#[test]
fn a_device_paired_after_turning_it_on_starts_off() {
    let setting = Setting {
        machine_key: "mid:x".into(),
        enabled: true,
        runtime_ids: vec!["rt_old".into()],
    };
    // The setting was saved for rt_old; the machine now relays through rt_new.
    let row = ConsoleComputer {
        id: "rt_new".into(),
        kind: "paired",
        group: "Your servers".into(),
        name: "box".into(),
        provider: "Paired".into(),
        provider_logo: None,
        region: None,
        os: "Ubuntu".into(),
        os_family: "linux",
        address: None,
        reach: "relay",
        status: "online".into(),
        runtime_id: Some("rt_new".into()),
        last_sample: None,
        services_count: None,
        last_backup_at: None,
        aliases: vec![],
        paired_id: None,
        whole_disk: None,
        machine_key: "mid:x".into(),
        member_devices: vec!["rt_new".into()],
        member_paired: vec![],
    };
    assert!(!whole_disk_for(&row, Some(&setting)).enabled);
}
