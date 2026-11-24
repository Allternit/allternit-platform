//! Computers console history (step 2.2). Sample checks and bucket math
//! first, then ingest, the metrics route, the console's lastSample and the
//! sweep against a test schema with migrations 153 and 154 applied.

use axum::http::StatusCode;
use serde_json::json;
use tower::ServiceExt;

use super::*;
use crate::routes::test_support::{authed_request, test_state, MockGateway, DEV_USER};

fn sample(cpu: f64) -> Value {
    json!({
        "sampledAt": "2026-11-20T12:00:00Z",
        "uptimeSec": 3600,
        "os": { "name": "Ubuntu 24.04.1 LTS", "version": "24.04", "family": "linux", "arch": "x86_64", "hostname": "box" },
        "cpu": { "percent": cpu, "cores": 4, "load1": 0.5 },
        "memory": { "usedBytes": 2_000_000_000u64, "totalBytes": 8_000_000_000u64 },
        "disk": [
            { "mount": "/", "usedBytes": 10, "totalBytes": 100, "fs": "ext4" },
            { "mount": "/data", "usedBytes": 90, "totalBytes": 100 }
        ],
        "net": { "rxBytesPerSec": 1000.0, "txBytesPerSec": 250.0 },
        "primaryDisk": "/",
        "notInTheContract": "dropped"
    })
}

#[test]
fn a_good_sample_keeps_the_fullest_disk_and_drops_unknown_fields() {
    let row = parse_sample(&sample(12.5)).expect("a host-probe snapshot parses");
    assert_eq!(row.cpu_percent, 12.5);
    assert_eq!((row.cpu_cores, row.load1), (4, Some(0.5)));
    assert_eq!((row.mem_used_bytes, row.mem_total_bytes), (2_000_000_000, 8_000_000_000));
    assert_eq!(row.mem_percent, Some(25.0), "percent 0-100, computed at ingest");
    assert_eq!(row.disk_percent, Some(90.0));
    assert_eq!(
        (row.disk_mount.as_deref(), row.disk_used_bytes, row.disk_total_bytes),
        (Some("/data"), Some(90), Some(100)),
        "the fullest mount, not the first"
    );
    assert_eq!((row.net_rx_bytes_per_sec, row.net_tx_bytes_per_sec), (1000.0, 250.0));
    assert_eq!(row.uptime_sec, 3600);
    assert!(row.snapshot.get("notInTheContract").is_none());
    assert_eq!(row.snapshot["primaryDisk"], "/");
    assert_eq!(row.snapshot["os"]["hostname"], "box");
    assert_eq!(row.snapshot["disk"][0]["fs"], "ext4");
}

#[test]
fn bad_samples_are_ignored_and_cpu_is_capped() {
    let mut no_cpu = sample(1.0);
    no_cpu.as_object_mut().unwrap().remove("cpu");
    let mut negative_rate = sample(1.0);
    negative_rate["net"]["rxBytesPerSec"] = json!(-5.0);
    let mut wordy = sample(1.0);
    wordy["cpu"]["percent"] = json!("high");
    let mut bad_time = sample(1.0);
    bad_time["sampledAt"] = json!("yesterday");
    let mut long_mount = sample(1.0);
    long_mount["disk"][0]["mount"] = json!("x".repeat(1000));
    let mut huge = sample(1.0);
    huge["memory"]["totalBytes"] = json!(u64::MAX);
    for bad in [no_cpu, negative_rate, wordy, bad_time, long_mount, huge, json!("nope"), json!({})] {
        assert!(parse_sample(&bad).is_none(), "{bad}");
    }
    let row = parse_sample(&sample(250.0)).unwrap();
    assert_eq!(row.cpu_percent, 100.0);
    assert_eq!(row.snapshot["cpu"]["percent"], 100.0);
    // No disk at all is still a sample, with no disk columns.
    let mut no_disk = sample(1.0);
    no_disk["disk"] = json!([]);
    let row = parse_sample(&no_disk).unwrap();
    assert_eq!((row.disk_mount, row.disk_used_bytes), (None, None));
}

#[test]
fn floor_to_cuts_to_the_minute_and_the_bucket() {
    let t = DateTime::parse_from_rfc3339("2026-11-20T12:07:42Z").unwrap().with_timezone(&Utc);
    assert_eq!(floor_to(t, 60).to_rfc3339(), "2026-11-20T12:07:00+00:00");
    assert_eq!(floor_to(t, 300).to_rfc3339(), "2026-11-20T12:05:00+00:00");
}

#[test]
fn windows_are_the_four_the_console_asks_for() {
    assert_eq!(Window::parse("1h"), Some(Window { span_secs: 3_600, step_secs: 60 }));
    assert_eq!(Window::parse("24h"), Some(Window { span_secs: 86_400, step_secs: 300 }));
    assert_eq!(Window::parse("7d"), Some(Window { span_secs: 604_800, step_secs: 1_800 }));
    assert_eq!(Window::parse("30d"), Some(Window { span_secs: 2_592_000, step_secs: 7_200 }));
    assert_eq!(Window::parse("1y"), None);
}

#[test]
fn buckets_split_into_five_series_and_round() {
    let t0 = 1_763_640_000.0; // 2025-11-20T12:00:00Z
    let rows = vec![
        (t0, Some(12.345), Some(50.0), None, Some(1000.4), Some(10.6)),
        (t0 + 60.0, Some(20.0), None, Some(90.04), None, Some(0.0)),
    ];
    let series = series_from_buckets(&rows);
    let names: Vec<&str> = series.iter().map(|s| s.metric).collect();
    assert_eq!(names, vec!["cpu", "memory", "disk", "net_rx", "net_tx"]);
    assert_eq!(
        series[0].points,
        vec![("2025-11-20T12:00:00Z".to_string(), 12.3), ("2025-11-20T12:01:00Z".to_string(), 20.0)]
    );
    assert_eq!(series[1].points.len(), 1, "a bucket with no value is left out");
    assert_eq!(series[2].points, vec![("2025-11-20T12:01:00Z".to_string(), 90.0)]);
    assert_eq!(series[3].points, vec![("2025-11-20T12:00:00Z".to_string(), 1000.0)]);
    assert_eq!(series[4].points.len(), 2);
    let json = serde_json::to_value(&series[0]).unwrap();
    assert_eq!(json, json!({ "metric": "cpu", "points": [["2025-11-20T12:00:00Z", 12.3], ["2025-11-20T12:01:00Z", 20.0]] }));
}

// ------------------------------------------------------------- with a db ----

const MIGRATION_154: &str = include_str!("../../migrations_pg/154_computer_metrics.sql");

/// Minimal column subsets of migrations 001/014/017/066/081 (no FKs to
/// users/billing), plus the real migrations 153 and 154.
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
    let sql = MIGRATION_154.replace("public.", "");
    sqlx::raw_sql(&sql).execute(&state.db).await.expect("migration 154");
    // Additive and idempotent: the deploy may apply it twice.
    sqlx::raw_sql(&sql).execute(&state.db).await.expect("migration 154 again");
    // One row per machine (step 2.10): the console list reads these columns.
    let sql = include_str!("../../migrations_pg/159_computer_machine_identity.sql").replace("public.", "");
    sqlx::raw_sql(&sql).execute(&state.db).await.expect("migration 159");
    state
}

fn dev_token_on() -> std::sync::MutexGuard<'static, ()> {
    use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
    let g = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
    g
}

async fn send(router: Router, req: axum::http::Request<axum::body::Body>) -> (StatusCode, Value) {
    let res = router.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = http_body_util::BodyExt::collect(res.into_body()).await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

async fn get_metrics(state: &Arc<ApiState>, id: &str, query: &str) -> (StatusCode, Value) {
    let path = format!("/api/v1/computers/{id}/metrics{query}");
    send(routes().with_state(state.clone()), authed_request("GET", &path, "")).await
}

async fn count(db: &PgPool, kind: &str, id: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM computer_metric_samples WHERE computer_kind = $1 AND computer_id = $2")
        .bind(kind)
        .bind(id)
        .fetch_one(db)
        .await
        .unwrap()
}

fn mins_ago(m: i64) -> DateTime<Utc> {
    Utc::now() - Duration::minutes(m)
}

/// The caller's computers and someone else's, as the console test seeds them.
async fn seed(db: &PgPool) {
    sqlx::raw_sql(&format!(
        "INSERT INTO runtime_devices (id, user_id, name, runtime_type, platform, status, last_seen_at) VALUES
           ('rt_cloud', '{u}', 'Cloud computer', 'provisioned', 'linux', 'online', now()),
           ('rt_desk', '{u}', 'Studio Mac', 'desktop', 'darwin', 'online', now()),
           ('rt_box', '{u}', 'build-box', 'vps', 'linux', 'online', now()),
           ('rt_other', 'someone-else', 'theirs', 'desktop', 'darwin', 'online', now());
         INSERT INTO provisioned_instances (id, user_id, incus_name, status, device_id) VALUES
           ('pi_1', '{u}', 'c-1', 'running', 'rt_cloud'),
           ('pi_other', 'someone-else', 'c-2', 'running', NULL);
         INSERT INTO paired_computers (id, user_id, name, os, secret_hash, mesh_ip, runtime_device_id, provisioned_instance_id) VALUES
           ('pc_box', '{u}', 'build-box', 'Debian 12', '{h}', '100.64.0.7', 'rt_box', NULL),
           ('pc_cloud', '{u}', 'Cloud computer', 'linux', 'x', '100.64.0.8', NULL, 'pi_1'),
           ('pc_other', 'someone-else', 'theirs', 'linux', 'x', NULL, NULL, NULL);",
        u = DEV_USER,
        h = crate::routes::paired_computers::sha256_hex("s3cret"),
    ))
    .execute(db)
    .await
    .unwrap();
}

#[tokio::test]
#[serial_test::serial]
async fn migration_154_is_additive_and_applies_twice() {
    let state = setup().await;
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name::text FROM information_schema.tables
         WHERE table_schema = current_schema() AND table_name IN ('computer_metric_samples', 'computer_alert_rules')
         ORDER BY 1",
    )
    .fetch_all(&state.db)
    .await
    .unwrap();
    assert_eq!(tables, vec!["computer_alert_rules", "computer_metric_samples"]);
    // A third run after rows exist changes nothing.
    sqlx::raw_sql(&format!(
        "INSERT INTO computer_alert_rules (id, user_id, computer_id, rule_type, threshold) VALUES ('r1', '{DEV_USER}', NULL, 'disk', 90)"
    ))
    .execute(&state.db)
    .await
    .unwrap();
    sqlx::raw_sql(&MIGRATION_154.replace("public.", "")).execute(&state.db).await.unwrap();
    let rules: i64 = sqlx::query_scalar("SELECT count(*) FROM computer_alert_rules").fetch_one(&state.db).await.unwrap();
    assert_eq!(rules, 1);
    let bad = sqlx::raw_sql(&format!(
        "INSERT INTO computer_alert_rules (id, user_id, rule_type) VALUES ('r2', '{DEV_USER}', 'memory')"
    ))
    .execute(&state.db)
    .await;
    assert!(bad.is_err(), "rule_type is offline, cpu or disk");
}

#[tokio::test]
#[serial_test::serial]
async fn a_heartbeat_without_a_sample_stores_nothing_and_with_one_stores_one_row_a_minute() {
    let state = setup().await;
    let db = &state.db;
    let old_node = json!({ "runtimeId": "rt_box", "nextDue": [] });
    assert!(!store_from_heartbeat(db, KIND_DEVICE, "rt_box", &old_node, Utc::now()).await);
    assert!(!store_from_heartbeat(db, KIND_DEVICE, "rt_box", &json!({ "hostSample": null }), Utc::now()).await);
    assert!(!store_from_heartbeat(db, KIND_DEVICE, "rt_box", &json!({ "hostSample": { "cpu": 1 } }), Utc::now()).await);
    assert_eq!(count(db, KIND_DEVICE, "rt_box").await, 0);

    let minute = floor_to(Utc::now(), 60);
    let body = json!({ "runtimeId": "rt_box", "hostSample": sample(30.0) });
    assert!(store_from_heartbeat(db, KIND_DEVICE, "rt_box", &body, minute + Duration::seconds(5)).await);
    assert!(
        !store_from_heartbeat(db, KIND_DEVICE, "rt_box", &body, minute + Duration::seconds(40)).await,
        "a second beat in the same minute is ignored"
    );
    assert!(store_from_heartbeat(db, KIND_DEVICE, "rt_box", &body, minute + Duration::seconds(65)).await);
    assert_eq!(count(db, KIND_DEVICE, "rt_box").await, 2);
    let (ts, res, n, snap): (DateTime<Utc>, i32, i32, Value) = sqlx::query_as(
        "SELECT ts, resolution_secs, sample_count, snapshot FROM computer_metric_samples ORDER BY ts LIMIT 1",
    )
    .fetch_one(db)
    .await
    .unwrap();
    assert_eq!((ts, res, n), (minute, 60, 1), "stored at the cloud's minute, not the node's clock");
    assert_eq!(snap["cpu"]["percent"], 30.0);
}

#[tokio::test]
#[serial_test::serial]
async fn the_paired_report_takes_an_optional_sample() {
    let state = setup().await;
    seed(&state.db).await;
    let report = |body: Value| {
        axum::http::Request::builder()
            .method("POST")
            .uri("/api/v1/computers/paired/pc_box/report")
            .header("content-type", "application/json")
            .header(crate::routes::paired_computers::SECRET_HEADER, "s3cret")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap()
    };
    let router = || crate::routes::paired_computers::routes().with_state(state.clone());
    let (status, _) = send(router(), report(json!({ "meshIp": "100.64.0.7", "vncReady": true }))).await;
    assert_eq!(status, StatusCode::OK, "an old computer's report still works");
    assert_eq!(count(&state.db, KIND_PAIRED, "pc_box").await, 0);
    let (status, _) =
        send(router(), report(json!({ "meshIp": "100.64.0.7", "vncReady": true, "hostSample": sample(5.0) }))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(count(&state.db, KIND_PAIRED, "pc_box").await, 1);
    let (status, _) =
        send(router(), report(json!({ "meshIp": "100.64.0.7", "hostSample": { "broken": true } }))).await;
    assert_eq!(status, StatusCode::OK, "a bad sample never fails the report");
}

#[tokio::test]
#[serial_test::serial]
async fn the_metrics_route_averages_raw_and_rolled_up_samples() {
    let _g = dev_token_on();
    let state = setup().await;
    let db = &state.db;
    seed(db).await;
    for (m, cpu) in [(10, 10.0), (9, 20.0), (8, 30.0)] {
        assert!(store_sample(db, KIND_DEVICE, "rt_desk", &sample(cpu), mins_ago(m)).await);
    }
    // A 5 minute rollup three days back (outside 1h and 24h, inside 7d).
    sqlx::query(
        "INSERT INTO computer_metric_samples (computer_kind, computer_id, ts, resolution_secs, sample_count,
           cpu_percent, mem_used_bytes, mem_total_bytes, mem_percent, disk_used_bytes, disk_total_bytes,
           disk_percent, net_rx_bytes_per_sec, net_tx_bytes_per_sec)
         VALUES ('device', 'rt_desk', $1, 300, 5, 80, 4, 8, 50, 1, 4, 25, 10, 20)",
    )
    .bind(floor_to(Utc::now() - Duration::days(3), 300))
    .execute(db)
    .await
    .unwrap();

    let (status, body) = get_metrics(&state, "rt_desk", "?window=1h").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let series = body["series"].as_array().unwrap();
    let names: Vec<&str> = series.iter().map(|s| s["metric"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["cpu", "memory", "disk", "net_rx", "net_tx"]);
    let cpu: Vec<f64> = series[0]["points"].as_array().unwrap().iter().map(|p| p[1].as_f64().unwrap()).collect();
    assert_eq!(cpu, vec![10.0, 20.0, 30.0], "1h: one point a minute, oldest first");
    assert_eq!(series[1]["points"][0][1], 25.0, "memory is percent used");
    assert_eq!(series[2]["points"][0][1], 90.0, "disk is the fullest mount");
    assert_eq!(series[3]["points"][0][1], 1000.0);

    let (_, body) = get_metrics(&state, "rt_desk", "").await;
    let cpu = body["series"][0]["points"].as_array().unwrap();
    assert!(!cpu.is_empty() && cpu.len() <= 2, "24h (the default) has 5 minute buckets: {cpu:?}");

    let (_, body) = get_metrics(&state, "rt_desk", "?window=7d").await;
    let cpu = body["series"][0]["points"].as_array().unwrap();
    assert_eq!(cpu.first().unwrap()[1], 80.0, "7d reads the rollup");
    assert_eq!(body["series"][1]["points"][0][1], 50.0);

    let (status, _) = get_metrics(&state, "rt_desk", "?window=1y").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A computer with no samples yet: five empty series, not an error.
    let (status, body) = get_metrics(&state, "pc_box", "?window=1h").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["series"].as_array().unwrap().iter().all(|s| s["points"].as_array().unwrap().is_empty()));
}

#[tokio::test]
#[serial_test::serial]
async fn a_console_id_reads_every_source_of_its_machine() {
    let _g = dev_token_on();
    let state = setup().await;
    let db = &state.db;
    seed(db).await;
    let at = mins_ago(3);
    // The cloud computer's runtime and its self-paired row both beat.
    store_sample(db, KIND_DEVICE, "rt_cloud", &sample(10.0), at).await;
    store_sample(db, KIND_PAIRED, "pc_cloud", &sample(30.0), at).await;
    // The paired box and its linked runtime device.
    store_sample(db, KIND_PAIRED, "pc_box", &sample(40.0), at).await;
    store_sample(db, KIND_DEVICE, "rt_box", &sample(60.0), at).await;

    let (status, body) = get_metrics(&state, "pi_1", "?window=1h").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["series"][0]["points"][0][1], 20.0);
    let (_, body) = get_metrics(&state, "pc_box", "?window=1h").await;
    assert_eq!(body["series"][0]["points"][0][1], 50.0);
}

#[tokio::test]
#[serial_test::serial]
async fn someone_elses_computer_answers_like_a_missing_one() {
    let _g = dev_token_on();
    let state = setup().await;
    let db = &state.db;
    seed(db).await;
    store_sample(db, KIND_DEVICE, "rt_other", &sample(10.0), mins_ago(2)).await;
    store_sample(db, KIND_PAIRED, "pc_other", &sample(10.0), mins_ago(2)).await;
    for id in ["rt_other", "pc_other", "pi_other", "nope", "pc_cloud", "rt_cloud"] {
        let (status, body) = get_metrics(&state, id, "?window=1h").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{id}: {body}");
    }
    let res = routes()
        .with_state(state.clone())
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/v1/computers/rt_desk/metrics")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[serial_test::serial]
async fn an_organization_member_reads_a_shared_paired_computer() {
    let caller = Caller { user_id: DEV_USER.into(), organization_id: Some("org_1".into()) };
    let state = setup().await;
    sqlx::raw_sql(
        "INSERT INTO paired_computers (id, user_id, name, secret_hash, organization_id) VALUES
           ('pc_team', 'teammate', 'team-box', 'x', 'org_1'),
           ('pc_elsewhere', 'stranger', 'their-box', 'x', 'org_2');",
    )
    .execute(&state.db)
    .await
    .unwrap();
    let team = sources_for(&state.db, &caller, "pc_team").await.unwrap();
    assert_eq!(team, Some(Sources { devices: vec![], paired: vec!["pc_team".into()] }));
    assert_eq!(sources_for(&state.db, &caller, "pc_elsewhere").await.unwrap(), None);
}

#[tokio::test]
#[serial_test::serial]
async fn the_console_fills_last_sample_from_the_newest_sample() {
    let _g = dev_token_on();
    let state = setup().await;
    let db = &state.db;
    seed(db).await;
    store_sample(db, KIND_DEVICE, "rt_desk", &sample(10.0), mins_ago(3)).await;
    store_sample(db, KIND_DEVICE, "rt_desk", &sample(40.0), mins_ago(1)).await;
    store_sample(db, KIND_PAIRED, "pc_box", &sample(15.0), mins_ago(4)).await;
    store_sample(db, KIND_DEVICE, "rt_box", &sample(25.0), mins_ago(2)).await;
    store_sample(db, KIND_PAIRED, "pc_cloud", &sample(35.0), mins_ago(1)).await;

    let (status, body) = send(
        crate::routes::computers_console::routes().with_state(state.clone()),
        authed_request("GET", "/api/v1/computers/console", ""),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = body["computers"].as_array().unwrap();
    let by_id = |id: &str| rows.iter().find(|r| r["id"] == id).unwrap_or_else(|| panic!("{id} in {body}"));
    assert_eq!(by_id("rt_desk")["lastSample"]["cpu"]["percent"], 40.0, "the newest sample");
    assert_eq!(by_id("pc_box")["lastSample"]["cpu"]["percent"], 25.0, "the newer of the row and its runtime");
    assert_eq!(by_id("pi_1")["lastSample"]["cpu"]["percent"], 35.0, "a cloud computer's self-paired row");
    assert_eq!(by_id("rt_desk")["lastSample"]["sampledAt"], "2026-11-20T12:00:00Z");

    // Rolled-up rows carry no snapshot: a computer with only those has no lastSample.
    sqlx::query("DELETE FROM computer_metric_samples WHERE computer_id = 'rt_desk'").execute(db).await.unwrap();
    sqlx::query(
        "INSERT INTO computer_metric_samples (computer_kind, computer_id, ts, resolution_secs, sample_count, cpu_percent)
         VALUES ('device', 'rt_desk', now() - interval '3 days', 300, 5, 50)",
    )
    .execute(db)
    .await
    .unwrap();
    let (_, body) = send(
        crate::routes::computers_console::routes().with_state(state.clone()),
        authed_request("GET", "/api/v1/computers/console", ""),
    )
    .await;
    let desk = body["computers"].as_array().unwrap().iter().find(|r| r["id"] == "rt_desk").unwrap().clone();
    assert!(desk.get("lastSample").is_none(), "{desk}");
}

/// One raw row at `ts` with the given cpu, memory used (of 8) and rx rate.
async fn raw_row(db: &PgPool, id: &str, ts: DateTime<Utc>, cpu: f64, mem: i64, rx: f64) {
    sqlx::query(
        "INSERT INTO computer_metric_samples (computer_kind, computer_id, ts, resolution_secs, sample_count,
           cpu_percent, cpu_cores, mem_used_bytes, mem_total_bytes, mem_percent, disk_mount, disk_used_bytes,
           disk_total_bytes, disk_percent, net_rx_bytes_per_sec, net_tx_bytes_per_sec, uptime_sec, snapshot)
         VALUES ('device', $1, $2, 60, 1, $3, 4, $4, 8, $4 * 12.5, '/', 50, 100, 50, $5, 0, 100, '{}'::jsonb)",
    )
    .bind(id)
    .bind(ts)
    .bind(cpu)
    .bind(mem)
    .bind(rx)
    .execute(db)
    .await
    .unwrap();
}

#[tokio::test]
#[serial_test::serial]
async fn the_sweep_rolls_up_old_samples_into_five_minute_averages() {
    let state = setup().await;
    let db = &state.db;
    let now = Utc::now();
    // On a 2 hour boundary, so the 30d read below puts both rollups in one bucket.
    let bucket = floor_to(now - Duration::hours(50), 7_200);
    raw_row(db, "rt_a", bucket, 10.0, 1, 100.0).await;
    raw_row(db, "rt_a", bucket + Duration::seconds(60), 20.0, 2, 200.0).await;
    raw_row(db, "rt_a", bucket + Duration::seconds(120), 30.0, 6, 600.0).await;
    // The next bucket, and a recent sample that stays raw.
    raw_row(db, "rt_a", bucket + Duration::seconds(300), 70.0, 4, 0.0).await;
    raw_row(db, "rt_a", now - Duration::hours(1), 99.0, 8, 0.0).await;
    // A rollup older than 30 days goes.
    sqlx::query(
        "INSERT INTO computer_metric_samples (computer_kind, computer_id, ts, resolution_secs, sample_count, cpu_percent)
         VALUES ('device', 'rt_a', $1, 300, 5, 1)",
    )
    .bind(floor_to(now - Duration::days(31), 300))
    .execute(db)
    .await
    .unwrap();

    let done = sweep(db, now).await.unwrap();
    assert_eq!(done, SweepCounts { rolled_up: 2, raw_deleted: 4, rollups_deleted: 1 });

    let rows: Vec<(DateTime<Utc>, i32, i32, f64, i64, f64, f64, Option<Value>)> = sqlx::query_as(
        "SELECT ts, resolution_secs, sample_count, cpu_percent, mem_used_bytes, mem_percent, net_rx_bytes_per_sec, snapshot
         FROM computer_metric_samples ORDER BY ts",
    )
    .fetch_all(db)
    .await
    .unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0], (bucket, 300, 3, 20.0, 3, 37.5, 300.0, None), "averages of the three samples, no snapshot");
    assert_eq!((rows[1].0, rows[1].2, rows[1].3), (bucket + Duration::seconds(300), 1, 70.0));
    assert_eq!((rows[2].1, rows[2].3), (60, 99.0), "the last 48 hours stay raw");

    // Read back over 30d, both rollups fall in one 2 hour bucket, weighted by sample count.
    let series = load_series(db, &Sources { devices: vec!["rt_a".into()], paired: vec![] }, Window::parse("30d").unwrap(), now)
        .await
        .unwrap();
    let bucket_cpu = series[0].points.first().unwrap().1;
    assert_eq!(bucket_cpu, 32.5, "(20 x 3 + 70 x 1) / 4");

    assert_eq!(sweep(db, now).await.unwrap(), SweepCounts::default(), "a second sweep finds nothing to do");
}
