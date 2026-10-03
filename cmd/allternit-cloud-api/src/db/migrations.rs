//! Ordered migration runner for PostgreSQL.
//!
//! `migrations_pg/` holds the canonical PG schema history:
//!
//! * `001_initial.sql` is a pg_dump **snapshot** of the base schema, used to
//!   rebuild fresh installs. It contains psql meta-commands and is applied
//!   out-of-band (it cannot run through a plain SQL connection).
//! * `002`–`010` are incremental, hand-written DDL applied in version order.
//!   Several were applied manually to production before this runner existed;
//!   `schema_migrations` (below) is how the runner tells applied migrations
//!   apart from pending ones.
//!
//! The runner is **opt-in** (`RUN_PG_MIGRATIONS=true`): the production
//! database predates it, so the operator seeds `schema_migrations` for the
//! already-applied versions on first use, then leaves the flag on.

use sqlx::PgPool;

use crate::ApiError;

/// A single schema migration.
pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
}

macro_rules! migration {
    ($version:expr, $file:literal) => {
        Migration {
            version: $version,
            name: $file,
            sql: include_str!(concat!("../../migrations_pg/", $file)),
        }
    };
}

/// Incremental Postgres migrations, in application order. `001_initial.sql`
/// is intentionally excluded (pg_dump snapshot; see module docs).
pub const MIGRATIONS: &[Migration] = &[
    migration!(2, "002_widen_int_to_bigint.sql"),
    migration!(3, "003_api_keys.sql"),
    migration!(4, "004_hosted_runtime_nodes.sql"),
    migration!(5, "005_billing_subscriptions.sql"),
    migration!(6, "006_hosted_retail_rates.sql"),
    migration!(7, "007_inference_usage.sql"),
    migration!(8, "008_billing_guards.sql"),
    migration!(9, "009_inference_pools.sql"),
    migration!(10, "010_user_inference_keys.sql"),
    migration!(15, "015_webhook_events.sql"),
    // 016–021 were applied to production out-of-band (see module docs); 022
    // is the first hand-written migration to also ride the startup runner.
    migration!(22, "022_telegram_onboarding.sql"),
    migration!(27, "027_teams_app.sql"),
];

/// Bookkeeping table. One row per applied migration version.
const DDL_SCHEMA_MIGRATIONS: &str = r#"
CREATE TABLE IF NOT EXISTS schema_migrations (
    version BIGINT PRIMARY KEY,
    name TEXT NOT NULL,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
)
"#;

/// Run every pending migration in order, each inside its own transaction,
/// recording successes in `schema_migrations`. Returns the number applied.
/// Fails fatally on the first migration error — a half-applied schema is
/// worse than a refused startup.
pub async fn run_migrations(pool: &PgPool, migrations: &[Migration]) -> Result<usize, ApiError> {
    sqlx::query(DDL_SCHEMA_MIGRATIONS)
        .execute(pool)
        .await?;

    let applied: Vec<i64> = sqlx::query_scalar("SELECT version FROM schema_migrations")
        .fetch_all(pool)
        .await?;

    let mut count = 0;
    for migration in migrations {
        if applied.contains(&migration.version) {
            continue;
        }
        let mut tx = pool.begin().await?;
        // Simple query protocol: each file holds multiple statements.
        sqlx::raw_sql(migration.sql).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO schema_migrations (version, name) VALUES ($1, $2)")
            .bind(migration.version)
            .bind(migration.name)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        tracing::info!(
            version = migration.version,
            name = migration.name,
            "Postgres migration applied"
        );
        count += 1;
    }
    Ok(count)
}

/// Whether the startup migration runner is enabled (`RUN_PG_MIGRATIONS=true`).
pub fn migrations_enabled() -> bool {
    std::env::var("RUN_PG_MIGRATIONS")
        .map(|value| value == "true" || value == "1")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    async fn scratch_pool() -> PgPool {
        let url = "postgres://allternit:allternit_pg_2026@localhost:5432/allternit_test";
        let schema = format!("mig_test_{}", uuid::Uuid::new_v4().simple());
        let schema_for_hook = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
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
            .connect(url)
            .await
            .unwrap();
        pool
    }

    const STUB_MIGRATIONS: &[Migration] = &[
        Migration {
            version: 1,
            name: "001_stub.sql",
            sql: "CREATE TABLE IF NOT EXISTS stub_one (id TEXT PRIMARY KEY);",
        },
        Migration {
            version: 2,
            name: "002_stub.sql",
            sql: "CREATE TABLE IF NOT EXISTS stub_two (id TEXT PRIMARY KEY);\n\
                 INSERT INTO stub_one (id) VALUES ('seeded_by_002');",
        },
    ];

    #[tokio::test]
    #[serial]
    async fn runner_applies_in_order_and_is_idempotent() {
        let pool = scratch_pool().await;

        let applied = run_migrations(&pool, STUB_MIGRATIONS).await.unwrap();
        assert_eq!(applied, 2, "both pending migrations applied");

        // Order: 002 inserted into a table 001 created.
        let seeded: String = sqlx::query_scalar("SELECT id FROM stub_one")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(seeded, "seeded_by_002");

        // Bookkeeping.
        let versions: Vec<i64> = sqlx::query_scalar(
            "SELECT version FROM schema_migrations ORDER BY version",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(versions, vec![1, 2]);

        // Second run applies nothing.
        let applied_again = run_migrations(&pool, STUB_MIGRATIONS).await.unwrap();
        assert_eq!(applied_again, 0, "applied migrations are skipped");
    }

    #[tokio::test]
    #[serial]
    async fn failing_migration_aborts_without_recording() {
        let pool = scratch_pool().await;
        let bad: &[Migration] = &[Migration {
            version: 1,
            name: "001_bad.sql",
            sql: "THIS IS NOT VALID SQL;",
        }];
        assert!(run_migrations(&pool, bad).await.is_err());

        let recorded: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM schema_migrations")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(recorded, 0, "failed migration leaves no bookkeeping row");
    }

    #[test]
    fn embedded_migrations_are_ordered_and_unique() {
        let mut versions: Vec<i64> = MIGRATIONS.iter().map(|migration| migration.version).collect();
        let unique_len = versions.len();
        versions.sort_unstable();
        versions.dedup();
        assert_eq!(
            versions.len(),
            unique_len,
            "embedded migration versions must be unique"
        );
        assert!(
            MIGRATIONS
                .windows(2)
                .all(|pair| pair[0].version < pair[1].version),
            "embedded migrations must be listed in ascending version order"
        );
        assert!(!MIGRATIONS.is_empty());
    }
}
