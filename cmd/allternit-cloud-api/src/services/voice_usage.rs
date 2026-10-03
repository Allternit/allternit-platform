//! Voice minutes metering and the pricing constants for Cloud Voice and bot
//! phone calls. Metering only: nothing here talks to Stripe.
//!
//! The constants live in this one place; the phone path reuses them. Pricing
//! (Eoj, 2026-10-02): Cloud Voice $0.08/min with 100 min/month included in
//! Plus; phone $0.08/min plus $2/month per number.

use chrono::{DateTime, Datelike, TimeZone, Utc};
use sqlx::PgPool;

use crate::ApiError;

/// Cloud Voice minutes included each calendar month (UTC) with Plus, in seconds.
pub const PLUS_INCLUDED_CLOUD_VOICE_SECONDS: i64 = 100 * 60;
/// Cloud Voice price per minute once the included minutes are used up.
pub const CLOUD_VOICE_RATE_USD_PER_MIN: f64 = 0.08;
/// Bot phone call price per minute.
pub const PHONE_RATE_USD_PER_MIN: f64 = 0.08;
/// Bot phone number price per month.
pub const PHONE_NUMBER_USD_PER_MONTH: f64 = 2.0;

pub const ENGINE_CLOUD: &str = "cloud";
pub const ENGINE_PHONE: &str = "phone";

/// Cloud Voice seconds included each month for a plan id (`free`, `plus`,
/// `super`, `ultra`). Free has no Cloud Voice. Super and Ultra get at least
/// what Plus gets (the task fixes only Plus's number).
pub fn included_cloud_voice_seconds(plan_id: &str) -> i64 {
    match plan_id {
        "plus" | "super" | "ultra" => PLUS_INCLUDED_CLOUD_VOICE_SECONDS,
        _ => 0,
    }
}

/// Whether a plan can keep using Cloud Voice past its included minutes (paid
/// overage). Free cannot.
pub fn plan_allows_overage(plan_id: &str) -> bool {
    matches!(plan_id, "plus" | "super" | "ultra")
}

/// The user's plan id: the subscription mirrored from Stripe
/// (`billing_subscriptions`), admins as `ultra`, otherwise `free`. Same lookup
/// as `GET /api/v1/me/usage`.
pub async fn plan_for_user(db: &PgPool, user_id: &str) -> Result<String, ApiError> {
    if crate::auth::is_admin_user(user_id) {
        return Ok("ultra".to_string());
    }
    let plan: Option<String> = sqlx::query_scalar(
        r#"
        SELECT plan_id FROM billing_subscriptions
        WHERE user_id = $1 AND status IN ('active', 'trialing')
        ORDER BY updated_at DESC
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    Ok(match plan.as_deref() {
        Some(p @ ("plus" | "super" | "ultra")) => p.to_string(),
        _ => "free".to_string(),
    })
}

/// `[start, end)` of the UTC calendar month containing `now`.
pub fn month_bounds(now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    let start = Utc
        .with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
        .single()
        .unwrap_or(now);
    let (y, m) = if now.month() == 12 {
        (now.year() + 1, 1)
    } else {
        (now.year(), now.month() + 1)
    };
    let end = Utc.with_ymd_and_hms(y, m, 1, 0, 0, 0).single().unwrap_or(now);
    (start, end)
}

/// Record finished voice seconds. Idempotent on `(engine, reference)`: a
/// duplicate report returns Ok and changes nothing.
pub async fn record(
    db: &PgPool,
    user_id: &str,
    engine: &str,
    seconds: i32,
    reference: &str,
) -> Result<(), ApiError> {
    record_at(db, user_id, engine, seconds, reference, Utc::now()).await
}

pub async fn record_at(
    db: &PgPool,
    user_id: &str,
    engine: &str,
    seconds: i32,
    reference: &str,
    occurred_at: DateTime<Utc>,
) -> Result<(), ApiError> {
    if engine != ENGINE_CLOUD && engine != ENGINE_PHONE {
        return Err(ApiError::BadRequest(format!("unknown voice engine: {engine}")));
    }
    if seconds < 0 {
        return Err(ApiError::BadRequest("seconds must not be negative".to_string()));
    }
    sqlx::query(
        r#"
        INSERT INTO voice_usage (user_id, engine, seconds, ref, occurred_at)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (engine, ref) DO NOTHING
        "#,
    )
    .bind(user_id)
    .bind(engine)
    .bind(seconds)
    .bind(reference)
    .bind(occurred_at)
    .execute(db)
    .await?;
    Ok(())
}

/// Seconds the user used on `engine` in the current UTC calendar month.
pub async fn month_usage(db: &PgPool, user_id: &str, engine: &str) -> Result<i64, ApiError> {
    month_usage_at(db, user_id, engine, Utc::now()).await
}

pub async fn month_usage_at(
    db: &PgPool,
    user_id: &str,
    engine: &str,
    now: DateTime<Utc>,
) -> Result<i64, ApiError> {
    let (start, end) = month_bounds(now);
    let total: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT SUM(seconds)::bigint FROM voice_usage
        WHERE user_id = $1 AND engine = $2 AND occurred_at >= $3 AND occurred_at < $4
        "#,
    )
    .bind(user_id)
    .bind(engine)
    .bind(start)
    .bind(end)
    .fetch_one(db)
    .await?;
    Ok(total.unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::test_pool;

    pub(crate) async fn voice_pool() -> PgPool {
        let db = test_pool().await;
        sqlx::raw_sql(
            &include_str!("../../migrations_pg/029_voice_usage.sql").replace("public.", ""),
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE billing_subscriptions (user_id TEXT, plan_id TEXT, plan_tier TEXT, \
             status TEXT, updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW())",
        )
        .execute(&db)
        .await
        .unwrap();
        db
    }

    #[test]
    fn plan_allowances() {
        assert_eq!(included_cloud_voice_seconds("free"), 0);
        assert_eq!(included_cloud_voice_seconds("plus"), 6000);
        assert!(!plan_allows_overage("free"));
        assert!(plan_allows_overage("plus"));
    }

    #[test]
    fn month_bounds_roll_over_the_year() {
        let dec = Utc.with_ymd_and_hms(2026, 12, 31, 23, 59, 59).unwrap();
        let (s, e) = month_bounds(dec);
        assert_eq!(s, Utc.with_ymd_and_hms(2026, 12, 1, 0, 0, 0).unwrap());
        assert_eq!(e, Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap());
    }

    #[tokio::test]
    async fn record_is_idempotent_on_engine_and_ref() {
        let db = voice_pool().await;
        record(&db, "u1", "cloud", 60, "sess-1").await.unwrap();
        record(&db, "u1", "cloud", 60, "sess-1").await.unwrap();
        // Same ref on another engine is a different call.
        record(&db, "u1", "phone", 30, "sess-1").await.unwrap();
        assert_eq!(month_usage(&db, "u1", "cloud").await.unwrap(), 60);
        assert_eq!(month_usage(&db, "u1", "phone").await.unwrap(), 30);
        assert_eq!(month_usage(&db, "u2", "cloud").await.unwrap(), 0);
    }

    #[tokio::test]
    async fn month_usage_splits_at_the_month_boundary() {
        let db = voice_pool().await;
        let oct31 = Utc.with_ymd_and_hms(2026, 10, 31, 23, 59, 59).unwrap();
        let nov1 = Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap();
        record_at(&db, "u1", "cloud", 100, "a", oct31).await.unwrap();
        record_at(&db, "u1", "cloud", 7, "b", nov1).await.unwrap();
        assert_eq!(month_usage_at(&db, "u1", "cloud", oct31).await.unwrap(), 100);
        assert_eq!(month_usage_at(&db, "u1", "cloud", nov1).await.unwrap(), 7);
    }

    #[tokio::test]
    async fn record_rejects_bad_input() {
        let db = voice_pool().await;
        assert!(record(&db, "u1", "video", 1, "x").await.is_err());
        assert!(record(&db, "u1", "cloud", -1, "x").await.is_err());
    }
}
