//! End-to-end paid billing loop proof against a scratch schema in the local
//! `allternit_test` Postgres database:
//!
//!   1. Seed a user with prepaid credits (the grant path used by the Stripe
//!      webhook and the internal sync route).
//!   2. Simulate a hosted runtime session (start -> stop) and assert the
//!      finalized session cost was deducted from the balance exactly once and
//!      ledgered as a negative `credit_transactions` row.
//!   3. Drain the balance with a second, longer session and assert the
//!      balance clamps at zero.
//!   4. Assert `check_spend_cap` now returns `Forbidden` — the exact signal
//!      the reconciler's 60s loop uses to auto-stop the user's running
//!      instances with reason `monthly_spend_cap`.
//!
//! Gated behind `ALLTERNIT_E2E_BILLING=1` so it no-ops in CI; it only needs
//! the test Postgres (no docker, no Stripe, no Clerk).

use allternit_cloud_api::services::{
    CostService, CostServiceImpl, QuotaService, UserQuota,
};
use sqlx::PgPool;

const TEST_DATABASE_URL: &str =
    "postgres://allternit:allternit_pg_2026@localhost:5432/allternit_test";
const TEST_USER_ID: &str = "user_e2e_billing";
/// Matches the rate the lifecycle unit tests seed for the shared-cpu-1x-1024mb
/// shape, so the expected per-hour cost is deterministic.
const COST_PER_HOUR: f64 = 0.0079;

/// Scratch-schema pool: every connection is pinned to the schema so the
/// services under test and the assertions share one isolated view.
async fn scratch_pool(schema: &str) -> PgPool {
    let schema_for_hook = schema.to_string();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .after_connect(move |conn, _meta| {
            let schema = schema_for_hook.clone();
            Box::pin(async move {
                sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {}", schema))
                    .execute(&mut *conn)
                    .await?;
                sqlx::query(&format!("SET search_path TO {}", schema))
                    .execute(&mut *conn)
                    .await?;
                Ok(())
            })
        })
        .connect(TEST_DATABASE_URL)
        .await
        .expect("connect to allternit_test")
}

/// Minimal billing schema: credits (migration 024), hosted runtime instances
/// and usage sessions (013/014), and the cost rate record_runtime_started
/// snapshots from.
async fn create_billing_schema(db: &PgPool) {
    let statements = [
        r#"
        CREATE TABLE user_credits (
            user_id TEXT PRIMARY KEY,
            balance_usd DOUBLE PRECISION NOT NULL DEFAULT 0 CHECK (balance_usd >= 0),
            bought_usd DOUBLE PRECISION NOT NULL DEFAULT 0,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )
        "#,
        r#"
        CREATE TABLE credit_transactions (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            amount_usd DOUBLE PRECISION NOT NULL,
            transaction_id TEXT NOT NULL UNIQUE,
            source TEXT NOT NULL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )
        "#,
        r#"
        CREATE TABLE hosted_runtime_instances (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            runtime_device_id TEXT,
            status TEXT NOT NULL,
            provider TEXT,
            cost_rate_provider TEXT,
            cost_rate_region TEXT,
            cost_rate_instance_type TEXT,
            started_at TIMESTAMPTZ,
            stopped_at TIMESTAMPTZ,
            active_since TIMESTAMPTZ,
            last_activity_at TIMESTAMPTZ,
            stop_reason TEXT
        )
        "#,
        r#"
        CREATE TABLE hosted_runtime_usage_sessions (
            id TEXT PRIMARY KEY,
            hosted_instance_id TEXT NOT NULL,
            user_id TEXT NOT NULL,
            started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            ended_at TIMESTAMPTZ,
            duration_seconds BIGINT,
            cost_per_hour DOUBLE PRECISION NOT NULL DEFAULT 0,
            estimated_cost_usd DOUBLE PRECISION,
            stop_reason TEXT
        )
        "#,
        r#"
        CREATE UNIQUE INDEX idx_hosted_usage_one_open_session
            ON hosted_runtime_usage_sessions(hosted_instance_id)
            WHERE ended_at IS NULL
        "#,
        r#"
        CREATE TABLE cost_rates (
            provider TEXT NOT NULL,
            region TEXT NOT NULL,
            instance_type TEXT NOT NULL,
            cost_per_hour DOUBLE PRECISION NOT NULL DEFAULT 0,
            PRIMARY KEY (provider, region, instance_type)
        )
        "#,
    ];
    for statement in statements {
        sqlx::query(statement)
            .execute(db)
            .await
            .expect("create billing schema");
    }

    sqlx::query(
        "INSERT INTO cost_rates (provider, region, instance_type, cost_per_hour) VALUES ('contabo', 'local', 'shared-cpu-1x-1024mb', $1)",
    )
    .bind(COST_PER_HOUR)
    .execute(db)
    .await
    .expect("seed cost rate");
    sqlx::query(
        r#"
        INSERT INTO hosted_runtime_instances (
            id, user_id, runtime_device_id, status, provider,
            cost_rate_provider, cost_rate_region, cost_rate_instance_type
        ) VALUES ('hr_e2e', $1, 'rd_e2e', 'running', 'contabo', 'contabo', 'local', 'shared-cpu-1x-1024mb')
        "#,
    )
    .bind(TEST_USER_ID)
    .execute(db)
    .await
    .expect("seed hosted instance");
}

fn test_quota() -> UserQuota {
    UserQuota {
        user_id: TEST_USER_ID.to_string(),
        plan_tier_id: "pro".to_string(),
        max_active_devices: 5,
        max_pairings_per_day: 50,
        max_relay_sockets: 20,
        max_relay_mb_per_day: 5000,
        max_hosted_runtime_hours_monthly: 1000,
        can_create_hosted_runtime: true,
        max_hosted_runtimes: 5,
        max_hosted_runtime_memory_mb: 2048,
        hard_spend_cap_usd: Some(100.0),
    }
}

/// Open a session on the seeded instance and backdate it `hours` into the
/// past so the finalized cost is deterministic.
async fn run_session(db: &PgPool, hours: f64, stop_reason: &str) -> String {
    allternit_cloud_api::services::record_runtime_started(db, "hr_e2e")
        .await
        .expect("record_runtime_started");
    sqlx::query(
        "UPDATE hosted_runtime_usage_sessions SET started_at = NOW() - make_interval(hours => $1) WHERE hosted_instance_id = 'hr_e2e' AND ended_at IS NULL",
    )
    .bind(hours as i32)
    .execute(db)
    .await
    .expect("backdate open session");
    allternit_cloud_api::services::record_runtime_stopped(db, "hr_e2e", stop_reason)
        .await
        .expect("record_runtime_stopped");
    sqlx::query_scalar(
        "SELECT id FROM hosted_runtime_usage_sessions WHERE hosted_instance_id = 'hr_e2e' ORDER BY ended_at DESC LIMIT 1",
    )
    .fetch_one(db)
    .await
    .expect("closed session id")
}

#[tokio::test]
async fn billing_paid_loop() {
    if std::env::var("ALLTERNIT_E2E_BILLING").ok().as_deref() != Some("1") {
        eprintln!("skipping e2e_billing_paid_loop: set ALLTERNIT_E2E_BILLING=1 (needs the local test Postgres)");
        return;
    }

    let schema = format!("e2e_billing_{}", uuid::Uuid::new_v4().simple());
    let db = scratch_pool(&schema).await;
    create_billing_schema(&db).await;

    let result = run_paid_loop(&db).await;
    let _ = sqlx::query(&format!("DROP SCHEMA IF EXISTS {} CASCADE", schema))
        .execute(&db)
        .await;
    if let Err(error) = result {
        panic!("billing paid loop failed: {error}");
    }
}

async fn run_paid_loop(db: &PgPool) -> Result<(), String> {
    let cost_service = CostServiceImpl::new(db.clone());
    let quota_service = QuotaService::new(db.clone());
    let quota = test_quota();

    // 1. Grant: $20 of prepaid credits, as the Stripe webhook would.
    let balance = cost_service
        .add_credits(TEST_USER_ID, 20.0, "stripe-evt_e2e_seed", "stripe")
        .await
        .map_err(|e| format!("seed credits: {e}"))?;
    if (balance - 20.0).abs() > 1e-9 {
        return Err(format!("balance after grant: {balance}"));
    }

    // The balance must authorize usage before anything runs.
    quota_service
        .check_spend_cap(TEST_USER_ID, &quota)
        .await
        .map_err(|e| format!("spend cap blocked a funded user: {e}"))?;

    // 2. One one-hour session: start, stop, and verify the ledgered debit.
    let session_id = run_session(db, 1.0, "user_stopped").await;
    let balance = cost_service
        .get_credit_balance(TEST_USER_ID)
        .await
        .map_err(|e| format!("read balance: {e}"))?;
    let expected = 20.0 - COST_PER_HOUR;
    if (balance - expected).abs() > 1e-6 {
        return Err(format!(
            "balance after first session: {balance}, expected {expected}"
        ));
    }
    let (amount, source): (f64, String) = sqlx::query_as(
        "SELECT amount_usd, source FROM credit_transactions WHERE transaction_id = $1",
    )
    .bind(format!("hosted-session-{session_id}"))
    .fetch_optional(db)
    .await
    .map_err(|e| format!("read ledger: {e}"))?
    .ok_or_else(|| "no debit ledger row for the closed session".to_string())?;
    if (amount + COST_PER_HOUR).abs() > 1e-6 || source != "hosted_runtime_usage" {
        return Err(format!("debit ledger row: amount {amount}, source {source}"));
    }
    eprintln!("session {session_id} deducted ${COST_PER_HOUR}; balance ${balance:.4}");

    // A repeated stop for the same instance must not deduct again.
    allternit_cloud_api::services::record_runtime_stopped(db, "hr_e2e", "destroyed")
        .await
        .map_err(|e| format!("second stop: {e}"))?;
    let balance_after = cost_service
        .get_credit_balance(TEST_USER_ID)
        .await
        .map_err(|e| format!("read balance: {e}"))?;
    if (balance_after - expected).abs() > 1e-6 {
        return Err(format!(
            "repeated stop double-charged: balance {balance_after}, expected {expected}"
        ));
    }

    // 3. Drain: a second session long enough to exceed the remaining balance
    // clamps the balance at zero (never negative).
    run_session(db, 5000.0, "monthly_spend_cap").await;
    let balance = cost_service
        .get_credit_balance(TEST_USER_ID)
        .await
        .map_err(|e| format!("read balance: {e}"))?;
    if balance != 0.0 {
        return Err(format!("balance after draining session: {balance}"));
    }
    eprintln!("balance drained to zero by the second session");

    // 4. The spend cap must now block — this Forbidden is exactly what the
    // reconciler's 60s loop turns into an auto-stop with reason
    // monthly_spend_cap (see hosted_runtime_lifecycle::reconcile_hosted_runtimes).
    match quota_service.check_spend_cap(TEST_USER_ID, &quota).await {
        Err(allternit_cloud_api::error::ApiError::Forbidden(reason)) => {
            eprintln!("spend cap blocks the drained user as expected: {reason}");
        }
        other => {
            return Err(format!(
                "drained balance must produce the auto-stop Forbidden, got: {other:?}"
            ));
        }
    }

    // The ledger tells the whole story: one grant, two debits.
    let (grants, debits): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*) FILTER (WHERE amount_usd > 0), COUNT(*) FILTER (WHERE amount_usd < 0) FROM credit_transactions WHERE user_id = $1",
    )
    .bind(TEST_USER_ID)
    .fetch_one(db)
    .await
    .map_err(|e| format!("count ledger: {e}"))?;
    if grants != 1 || debits != 2 {
        return Err(format!("ledger shape: {grants} grants, {debits} debits"));
    }

    Ok(())
}
