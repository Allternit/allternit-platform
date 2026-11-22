//! Computers console alerts (step 2.8): the decisions first, then the rules
//! routes and the evaluator against a test schema with migrations 153, 154
//! and 157. No network: with no push service configured, delivery is a no-op.

use axum::http::StatusCode;
use tower::ServiceExt;

use super::*;
use crate::routes::test_support::{authed_request, test_state, MockGateway, DEV_USER};

fn at(min: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000, 0).unwrap() + Duration::minutes(min)
}

// ----------------------------------------------------------- decisions ----

#[test]
fn an_incident_fires_once_and_clears_before_it_can_fire_again() {
    let none = None;
    let seen = Some(Incident { since: at(0), fired: false });
    let fired = Some(Incident { since: at(0), fired: true });
    assert_eq!(decide(none, Verdict::Breach(0.0), false), Action::Breach { fire: false });
    assert_eq!(decide(seen, Verdict::Breach(0.0), true), Action::Breach { fire: true });
    assert_eq!(decide(fired, Verdict::Breach(0.0), true), Action::Nothing, "no repeats while it stays active");
    assert_eq!(decide(fired, Verdict::Clear, false), Action::Clear { announce: true });
    assert_eq!(decide(seen, Verdict::Clear, false), Action::Clear { announce: false }, "a blip that never fired says nothing");
    assert_eq!(decide(fired, Verdict::Unknown, true), Action::Nothing, "no data keeps the incident as it is");
}

#[test]
fn offline_waits_for_its_minutes_and_only_real_offline_counts() {
    assert_eq!(offline_verdict("offline"), Verdict::Breach(0.0));
    for s in ["online", "asleep", "stopped", "starting"] {
        assert_eq!(offline_verdict(s), Verdict::Clear, "{s} is not an outage");
    }
    assert!(!due("offline", 10.0, 5, None, at(0), at(9)));
    assert!(due("offline", 10.0, 5, None, at(0), at(10)));
    assert!(due("disk", 90.0, 5, None, at(0), at(0)));
}

#[test]
fn cpu_needs_every_sample_over_and_a_fresh_newest_one() {
    let w = |min: f64, oldest: i64, newest: i64| CpuWindow { count: 5, min: Some(min), avg: Some(min + 5.0), oldest: Some(at(oldest)), newest: Some(at(newest)) };
    let now = at(10);
    assert_eq!(cpu_verdict(&w(95.0, 5, 10), 90.0, 5, now), Verdict::Breach(100.0));
    assert_eq!(cpu_verdict(&w(60.0, 5, 10), 90.0, 5, now), Verdict::Clear, "one low sample is a dip");
    assert_eq!(cpu_verdict(&w(95.0, 5, 2), 90.0, 5, now), Verdict::Unknown, "stale samples say nothing");
    assert!(due("cpu", 90.0, 5, Some(&w(95.0, 5, 10)), now, now));
    assert!(!due("cpu", 90.0, 5, Some(&w(95.0, 9, 10)), now, now), "only a minute of samples so far");
}

#[test]
fn disk_uses_the_fullest_mount_and_ignores_computers_that_are_not_online() {
    let sample = json!({ "disk": [{ "mount": "/", "usedBytes": 10, "totalBytes": 100 }, { "mount": "/data", "usedBytes": 95, "totalBytes": 100 }] });
    assert_eq!(disk_percent(&sample), Some(95.0));
    assert_eq!(disk_percent(&json!({ "disk": [] })), None);
}

#[test]
fn rule_values_are_checked_with_plain_messages() {
    assert_eq!(check_threshold("offline", None), Ok(10.0));
    assert_eq!(check_threshold("cpu", None), Ok(90.0));
    assert!(check_threshold("cpu", Some(120.0)).is_err());
    assert!(check_threshold("offline", Some(0.0)).is_err());
    assert!(check_threshold("disk", Some(f64::NAN)).is_err());
    assert_eq!(check_duration(None), Ok(5));
    assert!(check_duration(Some(500)).is_err());
}

#[test]
fn the_push_matches_what_the_phones_decode() {
    let msg = push_message("pi_1", "Cloud computer", "disk", 93.4, 90.0, 5);
    assert_eq!(msg.kind, "computer_alert");
    assert_eq!(msg.url, "/computers/pi_1");
    assert_eq!(msg.title, "Cloud computer is running out of space");
    assert_eq!(msg.body, "Disk is 93% full.");
    for (k, v) in [("type", "computer_alert"), ("computerId", "pi_1"), ("rule", "disk"), ("category", "COMPUTER_ALERT")] {
        assert_eq!(msg.data[k], v);
    }
    assert_eq!(msg.data["value"], 93.0);
    // Android: data-only, strings only, with computerId, rule and value as top-level keys.
    let fcm = crate::routes::fcm::message(&msg, "tok");
    let data = fcm["message"]["data"].as_object().unwrap();
    assert!(data.values().all(Value::is_string));
    assert_eq!((data["type"].as_str(), data["computerId"].as_str(), data["rule"].as_str(), data["value"].as_str()), (Some("computer_alert"), Some("pi_1"), Some("disk"), Some("93")));
    // iOS: the category and the `data` keys the tap handler reads.
    let target = apns::PushTarget {
        table: apns::TokenTable::Device,
        id: "d1".into(),
        user_id: "u1".into(),
        device_id: "phone".into(),
        token: "t".into(),
        environment: "sandbox".into(),
        capabilities: vec!["actions".into()],
        kind: None,
        attributes_type: None,
        activity_key: None,
        focus: None,
    };
    let body = apns::payload(&msg, &target);
    assert_eq!(body["aps"]["category"], "COMPUTER_ALERT");
    assert_eq!(body["aps"]["interruption-level"], "time-sensitive");
    assert_eq!(body["data"]["type"], "computer_alert");
    assert_eq!(body["data"]["computerId"], "pi_1");
    assert_eq!(copy("offline", "Mac", 12.0, 10.0, 5, false).1, "It has been offline for 12 minutes.");
}

// --------------------------------------------------------- with a db ----

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
        include_str!("../../migrations_pg/157_computer_alerts.sql"),
    ] {
        let sql = sql.replace("public.", "");
        sqlx::raw_sql(&sql).execute(&state.db).await.expect("migration");
        sqlx::raw_sql(&sql).execute(&state.db).await.expect("migration twice");
    }
    state
}

fn dev_token_on() -> std::sync::MutexGuard<'static, ()> {
    use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
    let g = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
    g
}

async fn call(state: &Arc<ApiState>, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let res = routes().with_state(state.clone()).oneshot(authed_request(method, path, &body.to_string())).await.unwrap();
    let status = res.status();
    let bytes = http_body_util::BodyExt::collect(res.into_body()).await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

async fn seed(db: &PgPool) {
    sqlx::raw_sql(&format!(
        "INSERT INTO runtime_devices (id, user_id, name, runtime_type, platform, status, last_seen_at) VALUES
           ('rt_box', '{u}', 'build-box', 'vps', 'linux', 'offline', now() - interval '1 day'),
           ('rt_other', 'someone-else', 'theirs', 'vps', 'linux', 'offline', now() - interval '1 day');",
        u = DEV_USER
    ))
    .execute(db)
    .await
    .unwrap();
}

#[tokio::test]
#[serial_test::serial]
async fn rules_are_owner_only_checked_and_one_per_scope() {
    let _g = dev_token_on();
    let state = setup().await;
    seed(&state.db).await;
    let (s, b) = call(&state, "POST", "/api/v1/computers/alert-rules", json!({ "rule": "disk" })).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    assert_eq!((b["rule"]["threshold"].as_f64(), b["rule"]["computerId"].clone()), (Some(90.0), Value::Null));
    let (s, b) = call(&state, "POST", "/api/v1/computers/alert-rules", json!({ "rule": "disk", "threshold": 80 })).await;
    assert_eq!((s, b["error"].as_str()), (StatusCode::CONFLICT, Some("rule_exists")));
    assert!(b["message"].as_str().is_some_and(|m| !m.is_empty()));
    let (s, b) = call(&state, "POST", "/api/v1/computers/alert-rules", json!({ "rule": "cpu", "threshold": 250 })).await;
    assert_eq!((s, b["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_threshold")));
    let (s, b) = call(&state, "POST", "/api/v1/computers/alert-rules", json!({ "rule": "offline", "computerId": "rt_other" })).await;
    assert_eq!((s, b["error"].as_str()), (StatusCode::NOT_FOUND, Some("not_found")), "someone else's computer");
    let (s, b) = call(&state, "POST", "/api/v1/computers/alert-rules", json!({ "rule": "memory" })).await;
    assert_eq!((s, b["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_rule")));
    // Someone else's rule is not ours to read, change or delete.
    sqlx::raw_sql("INSERT INTO computer_alert_rules (id, user_id, rule_type, threshold) VALUES ('car_theirs', 'someone-else', 'cpu', 80)").execute(&state.db).await.unwrap();
    let (s, _) = call(&state, "PATCH", "/api/v1/computers/alert-rules/car_theirs", json!({ "enabled": false })).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = call(&state, "DELETE", "/api/v1/computers/alert-rules/car_theirs", json!({})).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (_, list) = call(&state, "GET", "/api/v1/computers/alert-rules", json!({})).await;
    assert_eq!(list["rules"].as_array().unwrap().len(), 1);
    let id = list["rules"][0]["id"].as_str().unwrap().to_string();
    let (s, b) = call(&state, "PATCH", &format!("/api/v1/computers/alert-rules/{id}"), json!({ "threshold": 70, "enabled": false })).await;
    assert_eq!((s, b["rule"]["threshold"].as_f64(), b["rule"]["enabled"].as_bool()), (StatusCode::OK, Some(70.0), Some(false)));
    let (s, _) = call(&state, "DELETE", &format!("/api/v1/computers/alert-rules/{id}"), json!({})).await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
#[serial_test::serial]
async fn an_offline_computer_fires_once_then_resolves_when_it_returns() {
    let _g = dev_token_on();
    let state = setup().await;
    let db = &state.db;
    seed(db).await;
    let (s, b) = call(&state, "POST", "/api/v1/computers/alert-rules", json!({ "rule": "offline", "threshold": 10, "computerId": "rt_box" })).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let now = Utc::now();
    let n = |p: &PassCounts| (p.fired, p.resolved);
    assert_eq!(n(&evaluate_all(db, now).await.unwrap()), (0, 0), "just seen offline: not yet 10 minutes");
    assert_eq!(n(&evaluate_all(db, now + Duration::minutes(11)).await.unwrap()), (1, 0));
    assert_eq!(n(&evaluate_all(db, now + Duration::minutes(16)).await.unwrap()), (0, 0), "no repeat while it stays offline");
    let (_, b) = call(&state, "GET", "/api/v1/computers/alerts?computerId=rt_box", json!({})).await;
    assert_eq!(b["alerts"].as_array().unwrap().len(), 1);
    assert_eq!((b["alerts"][0]["state"].as_str(), b["alerts"][0]["rule"].as_str()), (Some("fired"), Some("offline")));
    assert!(b["alerts"][0]["message"].as_str().unwrap().contains("build-box is offline"));
    // Back online: the incident closes and a new outage can fire again.
    sqlx::query("UPDATE runtime_devices SET status = 'online', last_seen_at = now() WHERE id = 'rt_box'").execute(db).await.unwrap();
    let after = evaluate_all(db, now + Duration::minutes(17)).await.unwrap();
    assert_eq!(n(&after), (0, 1));
    let (_, b) = call(&state, "GET", "/api/v1/computers/alerts?computerId=rt_box", json!({})).await;
    assert_eq!(b["alerts"].as_array().unwrap().len(), 2, "fired, then resolved");
}

#[tokio::test]
#[serial_test::serial]
async fn a_full_disk_fires_once_and_clears_with_a_resolved_event() {
    let _g = dev_token_on();
    let state = setup().await;
    let db = &state.db;
    sqlx::raw_sql(&format!(
        "INSERT INTO runtime_devices (id, user_id, name, runtime_type, platform, status, last_seen_at)
           VALUES ('rt_desk', '{DEV_USER}', 'Studio Mac', 'desktop', 'darwin', 'online', now());"
    ))
    .execute(db)
    .await
    .unwrap();
    let put = |used: i64| {
        let snap = json!({ "disk": [{ "mount": "/", "usedBytes": used, "totalBytes": 100 }] });
        sqlx::query(
            "INSERT INTO computer_metric_samples (computer_kind, computer_id, ts, resolution_secs, cpu_percent, disk_percent, snapshot)
             VALUES ('device', 'rt_desk', now(), 60, 10, $1, $2)
             ON CONFLICT (computer_kind, computer_id, resolution_secs, ts) DO UPDATE SET snapshot = EXCLUDED.snapshot",
        )
        .bind(used as f64)
        .bind(snap)
    };
    call(&state, "POST", "/api/v1/computers/alert-rules", json!({ "rule": "disk", "threshold": 90 })).await;
    put(95).execute(db).await.unwrap();
    let first = evaluate_all(db, Utc::now()).await.unwrap();
    assert_eq!(first.fired, 1);
    assert_eq!(evaluate_all(db, Utc::now()).await.unwrap(), PassCounts::default(), "once per incident");
    sqlx::query("DELETE FROM computer_metric_samples").execute(db).await.unwrap();
    put(40).execute(db).await.unwrap();
    assert_eq!(evaluate_all(db, Utc::now()).await.unwrap().resolved, 1);
    let (_, b) = call(&state, "GET", "/api/v1/computers/alerts", json!({})).await;
    let states: Vec<&str> = b["alerts"].as_array().unwrap().iter().filter_map(|a| a["state"].as_str()).collect();
    assert_eq!(states.len(), 2);
    assert!(states.contains(&"fired") && states.contains(&"resolved"));
    put(96).execute(db).await.unwrap();
    assert_eq!(evaluate_all(db, Utc::now()).await.unwrap().fired, 1, "a new incident fires again");
}
