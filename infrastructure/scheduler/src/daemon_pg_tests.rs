//! Postgres integration tests for the scheduler daemon.
//!
//! They need a server they may create databases on: set
//! `SCHEDULER_TEST_DATABASE_URL` (e.g. `postgres://postgres@127.0.0.1:5432/postgres`).
//! Without it they print a skip line and pass. Each test creates a throwaway
//! database, applies the real `cmd/allternit-cloud-api/migrations_pg` files
//! (including 018's claim columns) and drops it afterwards.

use super::*;
use crate::{ExecutionMode, MisfirePolicy};
use sqlx::Executor;

struct TestDb {
    admin: PgPool,
    name: String,
    pool: PgPool,
}

impl TestDb {
    async fn drop_db(self) {
        self.pool.close().await;
        let _ = self
            .admin
            .execute(format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.name).as_str())
            .await;
    }
}

async fn test_db() -> Option<TestDb> {
    let Ok(url) = std::env::var("SCHEDULER_TEST_DATABASE_URL") else {
        eprintln!("SCHEDULER_TEST_DATABASE_URL not set; skipping Postgres integration test");
        return None;
    };
    let admin = PgPool::connect(&url).await.expect("connect admin");
    let name = format!("sched_it_{}", Uuid::new_v4().simple());
    admin
        .execute(format!("CREATE DATABASE {name}").as_str())
        .await
        .expect("create db");
    let base = url.split('?').next().unwrap_or(&url);
    let db_url = match base.rfind('/') {
        Some(i) if i > "postgres://".len() && !base[i + 1..].contains('@') => format!("{}/{name}", &base[..i]),
        _ => format!("{base}/{name}"),
    };
    let pool = PgPool::connect(&db_url).await.expect("connect test db");

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cmd/allternit-cloud-api/migrations_pg");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("migrations_pg dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "sql"))
        .collect();
    files.sort();
    for f in files {
        let sql = std::fs::read_to_string(&f).unwrap();
        sqlx::raw_sql(&sql)
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("apply {}: {e}", f.display()));
    }
    Some(TestDb { admin, name, pool })
}

fn config() -> SchedulerConfig {
    SchedulerConfig {
        database_url: String::new(),
        api_url: "http://127.0.0.1:9".into(),
        api_key: None,
        poll_interval_secs: 60,
        max_schedules_per_tick: 100,
        misfire_threshold_secs: 300,
        misfire_policy: MisfirePolicy::FireOnce,
        execution_mode: ExecutionMode::Local,
        claim_ttl_secs: 900,
    }
}

async fn insert_schedule(pool: &PgPool, id: &str, next_run: DateTime<Utc>, enabled: bool, job: serde_json::Value) {
    sqlx::query(
        "INSERT INTO schedules (id, name, cron_expr, timezone, job_template, enabled, misfire_policy, next_run_at, run_count, misfire_count) \
         VALUES ($1, $1, '0 9 * * *', 'America/New_York', $2, $3, 'fire_once', $4, 0, 0)",
    )
    .bind(id)
    .bind(sqlx::types::Json(job))
    .bind(enabled)
    .bind(next_run)
    .execute(pool)
    .await
    .expect("insert schedule");
}

#[tokio::test]
async fn claims_only_due_enabled_rows() {
    let Some(db) = test_db().await else { return };
    let now = Utc::now();
    insert_schedule(&db.pool, "due", now - chrono::Duration::seconds(10), true, serde_json::json!({})).await;
    insert_schedule(&db.pool, "future", now + chrono::Duration::hours(1), true, serde_json::json!({})).await;
    insert_schedule(&db.pool, "disabled", now - chrono::Duration::seconds(10), false, serde_json::json!({})).await;

    let d = SchedulerDaemon::with_pool(config(), db.pool.clone()).unwrap();
    let claimed = d.claim_due_schedules(now).await.expect("claim query runs on Postgres");
    assert_eq!(claimed.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), vec!["due"]);
    assert_eq!(claimed[0].misfire_policy, "fire_once");
    assert_eq!(claimed[0].timezone, "America/New_York");

    // A second claimer sees nothing while the claim is fresh...
    let other = SchedulerDaemon::with_pool(config(), db.pool.clone()).unwrap();
    assert!(other.claim_due_schedules(now).await.unwrap().is_empty());
    // ...but takes over a claim older than the TTL.
    let later = now + chrono::Duration::seconds(config().claim_ttl_secs + 1);
    let taken = other.claim_due_schedules(later).await.unwrap();
    assert_eq!(taken.len(), 1);
    let holder: String = sqlx::query_scalar("SELECT claimed_by FROM schedules WHERE id = 'due'")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(holder, other.instance_id);
    db.drop_db().await;
}

#[tokio::test]
async fn two_daemons_fire_each_due_schedule_once() {
    let Some(db) = test_db().await else { return };
    let out = std::env::temp_dir().join(format!("sched-fires-{}", Uuid::new_v4().simple()));
    let now = Utc::now();
    let n = 20;
    for i in 0..n {
        let job = serde_json::json!({
            "command": "sh",
            "args": ["-c", format!("echo s{i} >> {}", out.display())],
        });
        insert_schedule(&db.pool, &format!("s{i}"), now - chrono::Duration::seconds(5), true, job).await;
    }

    let a = SchedulerDaemon::with_pool(config(), db.pool.clone()).unwrap();
    let b = SchedulerDaemon::with_pool(config(), db.pool.clone()).unwrap();
    let (ra, rb) = tokio::join!(a.run_once(), b.run_once());
    ra.unwrap();
    rb.unwrap();
    // A second round finds nothing due: next_run_at moved to the future.
    let (ra, rb) = tokio::join!(a.run_once(), b.run_once());
    ra.unwrap();
    rb.unwrap();

    let fired = std::fs::read_to_string(&out).unwrap_or_default();
    let mut lines: Vec<&str> = fired.lines().collect();
    lines.sort();
    lines.dedup();
    assert_eq!(fired.lines().count(), n, "each schedule fires exactly once: {fired}");
    assert_eq!(lines.len(), n);

    let triggers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trigger_history WHERE success")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(triggers, n as i64);
    let (runs, claimed, not_future): (i64, i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(run_count), 0)::bigint, \
                COUNT(*) FILTER (WHERE claimed_by IS NOT NULL), \
                COUNT(*) FILTER (WHERE next_run_at <= now()) \
         FROM schedules",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(runs, n as i64);
    assert_eq!(claimed, 0, "claims are released");
    assert_eq!(not_future, 0, "next_run_at advanced");

    // next_run_at is 09:00 New York wall-clock, i.e. 13:00 or 14:00 UTC.
    let hours: Vec<f64> = sqlx::query_scalar(
        "SELECT DISTINCT EXTRACT(HOUR FROM next_run_at AT TIME ZONE 'UTC')::float8 FROM schedules",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert!(hours.iter().all(|h| *h == 13.0 || *h == 14.0), "{hours:?}");

    let _ = std::fs::remove_file(&out);
    db.drop_db().await;
}

#[tokio::test]
async fn misfired_row_is_claimed_and_counted_once() {
    let Some(db) = test_db().await else { return };
    let out = std::env::temp_dir().join(format!("sched-misfire-{}", Uuid::new_v4().simple()));
    let job = serde_json::json!({ "command": "sh", "args": ["-c", format!("echo m >> {}", out.display())] });
    insert_schedule(&db.pool, "late", Utc::now() - chrono::Duration::hours(3), true, job).await;

    let a = SchedulerDaemon::with_pool(config(), db.pool.clone()).unwrap();
    let b = SchedulerDaemon::with_pool(config(), db.pool.clone()).unwrap();
    let (ra, rb) = tokio::join!(a.run_once(), b.run_once());
    ra.unwrap();
    rb.unwrap();

    assert_eq!(std::fs::read_to_string(&out).unwrap_or_default().lines().count(), 1);
    let (runs, misfires): (i64, i64) = sqlx::query_as("SELECT run_count, misfire_count FROM schedules WHERE id = 'late'")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!((runs, misfires), (1, 1));
    let _ = std::fs::remove_file(&out);
    db.drop_db().await;
}

#[tokio::test]
async fn concurrent_claims_are_disjoint() {
    let Some(db) = test_db().await else { return };
    let now = Utc::now();
    for i in 0..60 {
        insert_schedule(&db.pool, &format!("c{i}"), now - chrono::Duration::seconds(1), true, serde_json::json!({})).await;
    }
    let mut cfg = config();
    cfg.max_schedules_per_tick = 7;
    let daemons: Vec<_> = (0..4)
        .map(|_| SchedulerDaemon::with_pool(cfg.clone(), db.pool.clone()).unwrap())
        .collect();
    let mut seen = std::collections::HashSet::new();
    loop {
        let rounds = futures::future::join_all(daemons.iter().map(|d| d.claim_due_schedules(now))).await;
        let mut got = 0;
        for r in rounds {
            for s in r.unwrap() {
                got += 1;
                assert!(seen.insert(s.id.clone()), "{} claimed twice", s.id);
            }
        }
        if got == 0 {
            break;
        }
    }
    assert_eq!(seen.len(), 60);
    db.drop_db().await;
}
