//! Monthly Cloud Voice + bot phone overage billing. Ships OFF.
//!
//! `ALLTERNIT_VOICE_BILLING_MODE=off|dry_run|live` (default `off`; anything
//! unrecognised counts as `off`):
//!
//! * `off`: nothing is computed, stored or sent.
//! * `dry_run`: periods are computed and the exact Stripe request bodies are
//!   stored on the `voice_billing_periods` row. No Stripe call is ever made.
//! * `live`: periods are computed and stored `pending`. Stripe is called only
//!   when an admin approves that period (`approve_period`), never on a timer.
//!
//! Mechanism: Stripe invoice items (`POST /v1/invoiceitems`) attached to the
//! user's subscription, so they land on its next invoice. Chosen over Billing
//! Meters because the existing integration only creates Checkout sessions with
//! a flat plan price (billing_checkout / billing_subscriptions); meters would
//! need a metered item added to every live subscription (a subscription
//! mutation the codebase has never done), while invoice items need no change to
//! existing subscriptions and take a per-item idempotency key.
//!
//! Money math (UTC calendar month): cloud overage is the cloud seconds past the
//! plan's included allowance; phone is every phone second; both are rounded UP
//! to whole minutes over the month's total. Phone numbers cost a flat monthly
//! fee for each number held at any point in the month.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use crate::routes::billing_checkout::StripeCheckout;
use crate::services::voice_usage::{
    self, included_cloud_voice_seconds, plan_for_user, CLOUD_VOICE_RATE_USD_PER_MIN,
    ENGINE_CLOUD, ENGINE_PHONE, PHONE_NUMBER_USD_PER_MONTH, PHONE_RATE_USD_PER_MIN,
};
use crate::ApiError;

pub const ENV_MODE: &str = "ALLTERNIT_VOICE_BILLING_MODE";
pub const ENV_PRICE_MINUTE: &str = "STRIPE_PRICE_VOICE_MINUTE";
pub const ENV_PRICE_NUMBER: &str = "STRIPE_PRICE_PHONE_NUMBER";
/// Reserved for a future Billing Meters path; the invoice-item path does not read it.
pub const ENV_METER_MINUTES: &str = "STRIPE_METER_VOICE_MINUTES";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Off,
    DryRun,
    Live,
}

impl Mode {
    pub fn parse(value: Option<&str>) -> Mode {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("dry_run") => Mode::DryRun,
            Some("live") => Mode::Live,
            _ => Mode::Off,
        }
    }

    pub fn from_env() -> Mode {
        Mode::parse(std::env::var(ENV_MODE).ok().as_deref())
    }
}

const CENTS_PER_MINUTE_CLOUD: i64 = (CLOUD_VOICE_RATE_USD_PER_MIN * 100.0 + 0.5) as i64;
const CENTS_PER_MINUTE_PHONE: i64 = (PHONE_RATE_USD_PER_MIN * 100.0 + 0.5) as i64;
const CENTS_PER_NUMBER: i64 = (PHONE_NUMBER_USD_PER_MONTH * 100.0 + 0.5) as i64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Amounts {
    pub cloud_seconds: i64,
    pub included_seconds: i64,
    pub cloud_overage_min: i64,
    pub phone_seconds: i64,
    pub phone_min: i64,
    pub numbers: i64,
    pub cloud_cents: i64,
    pub phone_cents: i64,
    pub numbers_cents: i64,
    pub amount_cents: i64,
}

fn ceil_minutes(seconds: i64) -> i64 {
    (seconds.max(0) + 59) / 60
}

/// Pure period math for one user.
pub fn compute_amounts(plan_id: &str, cloud_seconds: i64, phone_seconds: i64, numbers: i64) -> Amounts {
    let included_seconds = included_cloud_voice_seconds(plan_id);
    let cloud_overage_min = ceil_minutes(cloud_seconds - included_seconds);
    let phone_min = ceil_minutes(phone_seconds);
    let numbers = numbers.max(0);
    let cloud_cents = cloud_overage_min * CENTS_PER_MINUTE_CLOUD;
    let phone_cents = phone_min * CENTS_PER_MINUTE_PHONE;
    let numbers_cents = numbers * CENTS_PER_NUMBER;
    Amounts {
        cloud_seconds,
        included_seconds,
        cloud_overage_min,
        phone_seconds,
        phone_min,
        numbers,
        cloud_cents,
        phone_cents,
        numbers_cents,
        amount_cents: cloud_cents + phone_cents + numbers_cents,
    }
}

/// `YYYY-MM` for a month, and its `[start, end)` bounds.
pub fn parse_period(period: &str) -> Result<(DateTime<Utc>, DateTime<Utc>), ApiError> {
    let bad = || ApiError::BadRequest("period must look like YYYY-MM.".to_string());
    let (y, m) = period.split_once('-').ok_or_else(bad)?;
    if y.len() != 4 || m.len() != 2 {
        return Err(bad());
    }
    let (y, m): (i32, u32) = (y.parse().map_err(|_| bad())?, m.parse().map_err(|_| bad())?);
    if !(1..=12).contains(&m) {
        return Err(bad());
    }
    let start = chrono::TimeZone::with_ymd_and_hms(&Utc, y, m, 1, 0, 0, 0).single().ok_or_else(bad)?;
    Ok(voice_usage::month_bounds(start))
}

pub fn period_label(now: DateTime<Utc>) -> String {
    now.format("%Y-%m").to_string()
}

/// Phone numbers a user held at any point in `[start, end)`.
async fn numbers_held(
    db: &PgPool,
    user_id: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<i64, ApiError> {
    let n: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::bigint FROM phone_numbers
        WHERE user_id = $1 AND created_at < $3 AND (released_at IS NULL OR released_at >= $2)
        "#,
    )
    .bind(user_id)
    .bind(start)
    .bind(end)
    .fetch_one(db)
    .await?;
    Ok(n)
}

async fn seconds_in(
    db: &PgPool,
    user_id: &str,
    engine: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<i64, ApiError> {
    let total: Option<i64> = sqlx::query_scalar(
        "SELECT SUM(seconds)::bigint FROM voice_usage \
         WHERE user_id = $1 AND engine = $2 AND occurred_at >= $3 AND occurred_at < $4",
    )
    .bind(user_id)
    .bind(engine)
    .bind(start)
    .bind(end)
    .fetch_one(db)
    .await?;
    Ok(total.unwrap_or(0))
}

/// Amounts for one user in the UTC month containing `now` (live estimate; no writes).
pub async fn estimate_month(db: &PgPool, user_id: &str, now: DateTime<Utc>) -> Result<Amounts, ApiError> {
    let (start, end) = voice_usage::month_bounds(now);
    let plan = plan_for_user(db, user_id).await?;
    Ok(compute_amounts(
        &plan,
        seconds_in(db, user_id, ENGINE_CLOUD, start, end).await?,
        seconds_in(db, user_id, ENGINE_PHONE, start, end).await?,
        numbers_held(db, user_id, start, end).await?,
    ))
}

/// The user's Stripe customer and the subscription the invoice items attach to.
async fn stripe_refs_for(db: &PgPool, user_id: &str) -> Result<(Option<String>, Option<String>), ApiError> {
    let sub: Option<(String, Option<String>)> = sqlx::query_as(
        r#"
        SELECT stripe_subscription_id, stripe_customer_id FROM billing_subscriptions
        WHERE user_id = $1 AND status IN ('active', 'trialing') AND stripe_subscription_id LIKE 'sub\_%'
        ORDER BY updated_at DESC LIMIT 1
        "#,
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    let account: Option<String> =
        sqlx::query_scalar("SELECT stripe_customer_id FROM user_billing_accounts WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(db)
            .await?;
    let customer = sub.as_ref().and_then(|(_, c)| c.clone()).or(account);
    Ok((customer, sub.map(|(s, _)| s)))
}

/// The Stripe invoice-item requests for one period row: path, idempotency key and form.
/// `price_*` are the Stripe price ids (a clearly marked placeholder in dry runs when unset).
pub fn build_request_bodies(
    user_id: &str,
    period: &str,
    customer: &str,
    subscription: Option<&str>,
    amounts: &Amounts,
    price_minute: &str,
    price_number: &str,
) -> Vec<Value> {
    let common = |price: &str, quantity: i64, description: String, kind: &str| {
        let mut form: Vec<(String, String)> = vec![
            ("customer".into(), customer.to_string()),
            ("price".into(), price.to_string()),
            ("quantity".into(), quantity.to_string()),
            ("description".into(), description),
            ("metadata[allternit_user_id]".into(), user_id.to_string()),
            ("metadata[allternit_voice_period]".into(), period.to_string()),
            ("metadata[allternit_voice_kind]".into(), kind.to_string()),
        ];
        if let Some(sub) = subscription {
            form.push(("subscription".into(), sub.to_string()));
        }
        form
    };
    let mut bodies = Vec::new();
    let minutes = amounts.cloud_overage_min + amounts.phone_min;
    if minutes > 0 {
        // Cloud overage and phone share the $0.08/min price, so one line.
        let form = common(
            price_minute,
            minutes,
            format!(
                "Allternit voice {period}: {} cloud min over included + {} phone min",
                amounts.cloud_overage_min, amounts.phone_min
            ),
            "minutes",
        );
        bodies.push(json!({
            "path": "/v1/invoiceitems",
            "idempotencyKey": format!("voice-billing-{period}-{user_id}-minutes"),
            "kind": "minutes",
            "form": form,
        }));
    }
    if amounts.numbers > 0 {
        let form = common(
            price_number,
            amounts.numbers,
            format!("Allternit bot phone number(s) {period}: {}", amounts.numbers),
            "numbers",
        );
        bodies.push(json!({
            "path": "/v1/invoiceitems",
            "idempotencyKey": format!("voice-billing-{period}-{user_id}-numbers"),
            "kind": "numbers",
            "form": form,
        }));
    }
    bodies
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PeriodRow {
    pub user_id: String,
    pub period: String,
    pub plan_id: String,
    pub cloud_seconds: i64,
    pub included_seconds: i64,
    pub cloud_overage_min: i64,
    pub phone_seconds: i64,
    pub phone_min: i64,
    pub numbers: i32,
    pub amount_cents: i64,
    pub state: String,
    pub stripe_customer_id: Option<String>,
    pub stripe_subscription_id: Option<String>,
    pub request_bodies: Value,
    pub stripe_refs: Value,
    pub error: Option<String>,
    pub approved_by: Option<String>,
    pub approved_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

const ROW_COLUMNS: &str = "user_id, period, plan_id, cloud_seconds, included_seconds, \
    cloud_overage_min, phone_seconds, phone_min, numbers, amount_cents, state, \
    stripe_customer_id, stripe_subscription_id, request_bodies, stripe_refs, error, \
    approved_by, approved_at, updated_at";

pub async fn list_periods(db: &PgPool, period: &str) -> Result<Vec<PeriodRow>, ApiError> {
    parse_period(period)?;
    let rows = sqlx::query_as::<_, PeriodRow>(&format!(
        "SELECT {ROW_COLUMNS} FROM voice_billing_periods WHERE period = $1 ORDER BY amount_cents DESC, user_id"
    ))
    .bind(period)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// Compute (or recompute) every billable user's row for `period` and store it.
/// Off: does nothing and returns 0. Rows already `invoiced` are never touched.
/// Users with nothing to bill (amount 0) get no row. Returns the rows written.
pub async fn compute_period(db: &PgPool, mode: Mode, period: &str) -> Result<usize, ApiError> {
    let (start, end) = parse_period(period)?;
    if mode == Mode::Off {
        return Ok(0);
    }
    let users: Vec<String> = sqlx::query_scalar(
        r#"
        SELECT user_id FROM voice_usage WHERE occurred_at >= $1 AND occurred_at < $2
        UNION
        SELECT user_id FROM phone_numbers
        WHERE created_at < $2 AND (released_at IS NULL OR released_at >= $1)
        ORDER BY 1
        "#,
    )
    .bind(start)
    .bind(end)
    .fetch_all(db)
    .await?;

    let price_minute = env_nonempty(ENV_PRICE_MINUTE).unwrap_or_else(|| format!("<{ENV_PRICE_MINUTE} unset>"));
    let price_number = env_nonempty(ENV_PRICE_NUMBER).unwrap_or_else(|| format!("<{ENV_PRICE_NUMBER} unset>"));
    let state = if mode == Mode::DryRun { "dry_run" } else { "pending" };
    let mut written = 0;
    for user in users {
        let plan = plan_for_user(db, &user).await?;
        let a = compute_amounts(
            &plan,
            seconds_in(db, &user, ENGINE_CLOUD, start, end).await?,
            seconds_in(db, &user, ENGINE_PHONE, start, end).await?,
            numbers_held(db, &user, start, end).await?,
        );
        if a.amount_cents == 0 {
            continue;
        }
        let (customer, subscription) = stripe_refs_for(db, &user).await?;
        let (bodies, note) = match customer.as_deref() {
            Some(c) => (
                build_request_bodies(&user, period, c, subscription.as_deref(), &a, &price_minute, &price_number),
                None,
            ),
            None => (Vec::new(), Some("no_stripe_customer: cannot be invoiced until the user has a Stripe customer")),
        };
        let result = sqlx::query(
            r#"
            INSERT INTO voice_billing_periods (
                user_id, period, plan_id, cloud_seconds, included_seconds, cloud_overage_min,
                phone_seconds, phone_min, numbers, amount_cents, state,
                stripe_customer_id, stripe_subscription_id, request_bodies, error
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)
            ON CONFLICT (user_id, period) DO UPDATE SET
                plan_id = EXCLUDED.plan_id, cloud_seconds = EXCLUDED.cloud_seconds,
                included_seconds = EXCLUDED.included_seconds,
                cloud_overage_min = EXCLUDED.cloud_overage_min,
                phone_seconds = EXCLUDED.phone_seconds, phone_min = EXCLUDED.phone_min,
                numbers = EXCLUDED.numbers, amount_cents = EXCLUDED.amount_cents,
                state = EXCLUDED.state, stripe_customer_id = EXCLUDED.stripe_customer_id,
                stripe_subscription_id = EXCLUDED.stripe_subscription_id,
                request_bodies = EXCLUDED.request_bodies, error = EXCLUDED.error,
                updated_at = now()
            WHERE voice_billing_periods.state <> 'invoiced'
            "#,
        )
        .bind(&user)
        .bind(period)
        .bind(&plan)
        .bind(a.cloud_seconds)
        .bind(a.included_seconds)
        .bind(a.cloud_overage_min)
        .bind(a.phone_seconds)
        .bind(a.phone_min)
        .bind(a.numbers as i32)
        .bind(a.amount_cents)
        .bind(state)
        .bind(customer.as_deref())
        .bind(subscription.as_deref())
        .bind(Value::Array(bodies))
        .bind(note)
        .execute(db)
        .await?;
        written += result.rows_affected() as usize;
    }
    Ok(written)
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApproveOutcome {
    pub invoiced: usize,
    pub failed: usize,
    pub skipped_already_invoiced: usize,
}

/// Send one period's invoice items to Stripe. Requires mode `live`, a period that
/// has ended, and the Stripe env. The caller has already authenticated the admin.
/// Safe to repeat: invoiced rows are skipped and every item carries an idempotency key.
pub async fn approve_period(
    db: &PgPool,
    mode: Mode,
    stripe: &dyn StripeCheckout,
    period: &str,
    admin_id: &str,
    now: DateTime<Utc>,
) -> Result<ApproveOutcome, ApiError> {
    let (_, end) = parse_period(period)?;
    if mode != Mode::Live {
        return Err(ApiError::Conflict(format!(
            "voice billing mode is not live (set {ENV_MODE}=live); nothing was sent."
        )));
    }
    if end > now {
        return Err(ApiError::Conflict("that period has not ended yet.".to_string()));
    }
    let secret = env_nonempty("STRIPE_SECRET_KEY")
        .ok_or_else(|| ApiError::ServiceUnavailable("STRIPE_SECRET_KEY is not set".to_string()))?;
    let price_minute = env_nonempty(ENV_PRICE_MINUTE)
        .ok_or_else(|| ApiError::ServiceUnavailable(format!("{ENV_PRICE_MINUTE} is not set")))?;
    let price_number = env_nonempty(ENV_PRICE_NUMBER)
        .ok_or_else(|| ApiError::ServiceUnavailable(format!("{ENV_PRICE_NUMBER} is not set")))?;

    let rows = list_periods(db, period).await?;
    if rows.is_empty() {
        return Err(ApiError::NotFound("no computed rows for that period; compute it first.".to_string()));
    }
    let mut outcome = ApproveOutcome { invoiced: 0, failed: 0, skipped_already_invoiced: 0 };
    for row in rows {
        if row.state == "invoiced" {
            outcome.skipped_already_invoiced += 1;
            continue;
        }
        let Some(customer) = row.stripe_customer_id.as_deref() else {
            mark_failed(db, &row, &row.stripe_refs, "no_stripe_customer", admin_id).await?;
            outcome.failed += 1;
            continue;
        };
        let a = compute_amounts(&row.plan_id, row.cloud_seconds, row.phone_seconds, row.numbers as i64);
        let bodies = build_request_bodies(
            &row.user_id,
            period,
            customer,
            row.stripe_subscription_id.as_deref(),
            &a,
            &price_minute,
            &price_number,
        );
        let mut refs = row.stripe_refs.clone();
        let mut error = None;
        for body in &bodies {
            let kind = body["kind"].as_str().unwrap_or("item").to_string();
            if refs.get(&kind).is_some() {
                continue; // sent by an earlier, partly failed approval
            }
            let form: Vec<(String, String)> = serde_json::from_value(body["form"].clone())
                .map_err(|e| ApiError::Internal(format!("stored form is invalid: {e}")))?;
            let key = body["idempotencyKey"].as_str().unwrap_or_default();
            match stripe.create_invoice_item(&secret, key, &form).await {
                Ok(id) => refs[&kind] = Value::String(id),
                Err(e) => {
                    error = Some(e.to_string());
                    break;
                }
            }
        }
        match error {
            None => {
                sqlx::query(
                    "UPDATE voice_billing_periods SET state = 'invoiced', request_bodies = $3, \
                     stripe_refs = $4, error = NULL, approved_by = $5, approved_at = now(), \
                     updated_at = now() WHERE user_id = $1 AND period = $2 AND state <> 'invoiced'",
                )
                .bind(&row.user_id)
                .bind(period)
                .bind(Value::Array(bodies))
                .bind(&refs)
                .bind(admin_id)
                .execute(db)
                .await?;
                outcome.invoiced += 1;
            }
            Some(e) => {
                mark_failed(db, &row, &refs, &e, admin_id).await?;
                outcome.failed += 1;
            }
        }
    }
    Ok(outcome)
}

async fn mark_failed(
    db: &PgPool,
    row: &PeriodRow,
    refs: &Value,
    error: &str,
    admin_id: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE voice_billing_periods SET state = 'failed', stripe_refs = $3, error = $4, \
         approved_by = $5, approved_at = now(), updated_at = now() \
         WHERE user_id = $1 AND period = $2 AND state <> 'invoiced'",
    )
    .bind(&row.user_id)
    .bind(&row.period)
    .bind(refs)
    .bind(error)
    .bind(admin_id)
    .execute(db)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::test_pool;
    use chrono::TimeZone;
    use std::sync::Mutex;

    async fn pool() -> PgPool {
        let db = test_pool().await;
        for sql in [
            include_str!("../../migrations_pg/029_voice_usage.sql"),
            include_str!("../../migrations_pg/024_phone_numbers.sql"),
            include_str!("../../migrations_pg/031_voice_billing_periods.sql"),
        ] {
            sqlx::raw_sql(&sql.replace("public.", "")).execute(&db).await.unwrap();
        }
        sqlx::raw_sql(
            "CREATE TABLE billing_subscriptions (stripe_subscription_id TEXT, user_id TEXT, plan_id TEXT, \
             plan_tier TEXT, status TEXT, stripe_customer_id TEXT, updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()); \
             CREATE TABLE user_billing_accounts (user_id TEXT PRIMARY KEY, stripe_customer_id TEXT NOT NULL);",
        )
        .execute(&db)
        .await
        .unwrap();
        db
    }

    async fn subscribe(db: &PgPool, user: &str, plan: &str) {
        sqlx::query(
            "INSERT INTO billing_subscriptions (stripe_subscription_id, user_id, plan_id, plan_tier, status, stripe_customer_id) \
             VALUES ($1,$2,$3,'pro','active',$4)",
        )
        .bind(format!("sub_{user}"))
        .bind(user)
        .bind(plan)
        .bind(format!("cus_{user}"))
        .execute(db)
        .await
        .unwrap();
    }

    async fn add_number(db: &PgPool, user: &str, id: &str, created: DateTime<Utc>, released: Option<DateTime<Utc>>) {
        sqlx::query(
            "INSERT INTO phone_numbers (id, user_id, runtime_id, bot_id, e164, carrier, created_at, released_at) \
             VALUES ($1,$2,'r','b',$3,'telnyx',$4,$5)",
        )
        .bind(id)
        .bind(user)
        .bind(format!("+1555{id}"))
        .bind(created)
        .bind(released)
        .execute(db)
        .await
        .unwrap();
    }

    fn oct(d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, d, h, 0, 0).unwrap()
    }

    #[derive(Default)]
    struct MockStripe {
        calls: Mutex<Vec<(String, Vec<(String, String)>)>>,
        fail_numbers: bool,
    }

    #[async_trait::async_trait]
    impl StripeCheckout for MockStripe {
        async fn create_checkout_session(&self, _: &str, _: &[(String, String)]) -> Result<String, ApiError> {
            unreachable!()
        }
        async fn create_billing_portal_session(&self, _: &str, _: &[(String, String)]) -> Result<String, ApiError> {
            unreachable!()
        }
        async fn create_invoice_item(&self, _: &str, key: &str, form: &[(String, String)]) -> Result<String, ApiError> {
            if self.fail_numbers && key.ends_with("-numbers") {
                return Err(ApiError::Internal("stripe down".into()));
            }
            self.calls.lock().unwrap().push((key.to_string(), form.to_vec()));
            Ok(format!("ii_{key}"))
        }
    }

    fn set_stripe_env() {
        std::env::set_var("STRIPE_SECRET_KEY", "sk_test_x");
        std::env::set_var(ENV_PRICE_MINUTE, "price_min");
        std::env::set_var(ENV_PRICE_NUMBER, "price_num");
    }

    fn clear_stripe_env() {
        std::env::remove_var("STRIPE_SECRET_KEY");
        std::env::remove_var(ENV_PRICE_MINUTE);
        std::env::remove_var(ENV_PRICE_NUMBER);
    }

    #[test]
    fn mode_parses_and_defaults_to_off() {
        assert_eq!(Mode::parse(None), Mode::Off);
        assert_eq!(Mode::parse(Some("")), Mode::Off);
        assert_eq!(Mode::parse(Some("yes")), Mode::Off);
        assert_eq!(Mode::parse(Some("dry_run")), Mode::DryRun);
        assert_eq!(Mode::parse(Some(" LIVE ")), Mode::Live);
    }

    #[test]
    fn math_includes_allowance_only_for_paid_plans_and_rounds_up() {
        // Plus: 100 min included. 100:00 exactly → no overage; 100:01 → 1 min.
        assert_eq!(compute_amounts("plus", 6000, 0, 0).amount_cents, 0);
        let a = compute_amounts("plus", 6001, 0, 0);
        assert_eq!((a.cloud_overage_min, a.amount_cents), (1, 8));
        // 2 h 1 s over → 61 min... 7200 - 6000 = 1200 s = 20 min exactly.
        assert_eq!(compute_amounts("plus", 7200, 0, 0).cloud_overage_min, 20);
        // Free has no allowance: 1 second bills a whole minute.
        let f = compute_amounts("free", 1, 0, 0);
        assert_eq!((f.included_seconds, f.cloud_overage_min, f.amount_cents), (0, 1, 8));
        // Phone: every second counts, rounded up in total; numbers are $2 each.
        let p = compute_amounts("plus", 0, 61, 2);
        assert_eq!((p.phone_min, p.phone_cents, p.numbers_cents, p.amount_cents), (2, 16, 400, 416));
    }

    #[test]
    fn period_parsing_and_bounds() {
        let (s, e) = parse_period("2026-12").unwrap();
        assert_eq!(s, Utc.with_ymd_and_hms(2026, 12, 1, 0, 0, 0).unwrap());
        assert_eq!(e, Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap());
        for bad in ["2026-13", "2026-00", "26-10", "2026-1", "abc", "2026/10", ""] {
            assert!(parse_period(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn bodies_carry_idempotency_keys_and_attach_to_the_subscription() {
        let a = compute_amounts("plus", 6600, 120, 1);
        let b = build_request_bodies("u1", "2026-10", "cus_1", Some("sub_1"), &a, "price_m", "price_n");
        assert_eq!(b.len(), 2);
        assert_eq!(b[0]["idempotencyKey"], "voice-billing-2026-10-u1-minutes");
        let form: Vec<(String, String)> = serde_json::from_value(b[0]["form"].clone()).unwrap();
        assert!(form.contains(&("quantity".into(), "12".into()))); // 10 cloud + 2 phone
        assert!(form.contains(&("subscription".into(), "sub_1".into())));
        assert_eq!(b[1]["kind"], "numbers");
        assert!(build_request_bodies("u1", "2026-10", "c", None, &compute_amounts("plus", 0, 0, 0), "m", "n").is_empty());
    }

    async fn seed(db: &PgPool) {
        subscribe(db, "u1", "plus").await;
        voice_usage::record_at(db, "u1", "cloud", 6000 + 650, "c1", oct(5, 1)).await.unwrap();
        voice_usage::record_at(db, "u1", "phone", 90, "p1", oct(6, 1)).await.unwrap();
        // Outside the month: must not count.
        voice_usage::record_at(db, "u1", "cloud", 9999, "c-sep", Utc.with_ymd_and_hms(2026, 9, 30, 23, 59, 59).unwrap()).await.unwrap();
        voice_usage::record_at(db, "u1", "cloud", 9999, "c-nov", Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap()).await.unwrap();
        add_number(db, "u1", "1", oct(1, 0), None).await;
        // Released before October: not counted. Released mid-month: counted.
        add_number(db, "u1", "2", Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap(), Some(Utc.with_ymd_and_hms(2026, 9, 15, 0, 0, 0).unwrap())).await;
        add_number(db, "u1", "3", Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap(), Some(oct(10, 0))).await;
        // Created after the month: not counted.
        add_number(db, "u1", "4", Utc.with_ymd_and_hms(2026, 11, 2, 0, 0, 0).unwrap(), None).await;
        // A user with nothing billable gets no row.
        subscribe(db, "u2", "plus").await;
        voice_usage::record_at(db, "u2", "cloud", 600, "c2", oct(5, 1)).await.unwrap();
    }

    #[tokio::test]
    async fn off_does_nothing() {
        let db = pool().await;
        seed(&db).await;
        assert_eq!(compute_period(&db, Mode::Off, "2026-10").await.unwrap(), 0);
        assert!(list_periods(&db, "2026-10").await.unwrap().is_empty());
        let stripe = MockStripe::default();
        assert!(approve_period(&db, Mode::Off, &stripe, "2026-10", "admin", oct(20, 0)).await.is_err());
        assert!(stripe.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn dry_run_stores_request_bodies_and_never_calls_stripe() {
        clear_stripe_env();
        let db = pool().await;
        seed(&db).await;
        assert_eq!(compute_period(&db, Mode::DryRun, "2026-10").await.unwrap(), 1);
        let rows = list_periods(&db, "2026-10").await.unwrap();
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.state, "dry_run");
        assert_eq!((r.cloud_overage_min, r.phone_min, r.numbers), (11, 2, 2)); // 650 s → 11 min
        assert_eq!(r.amount_cents, 13 * 8 + 2 * 200);
        assert_eq!(r.request_bodies.as_array().unwrap().len(), 2);
        // Prices unset: placeholders show in the stored body, and approve is refused.
        assert!(r.request_bodies[0]["form"].to_string().contains("unset"));
        let stripe = MockStripe::default();
        assert!(approve_period(&db, Mode::DryRun, &stripe, "2026-10", "admin", oct(20, 0)).await.is_err());
        assert!(stripe.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn live_waits_for_approval_then_sends_once() {
        set_stripe_env();
        let db = pool().await;
        seed(&db).await;
        assert_eq!(compute_period(&db, Mode::Live, "2026-10").await.unwrap(), 1);
        assert_eq!(list_periods(&db, "2026-10").await.unwrap()[0].state, "pending");
        let stripe = MockStripe::default();
        // Computing alone sent nothing; the period must also have ended.
        assert!(stripe.calls.lock().unwrap().is_empty());
        assert!(approve_period(&db, Mode::Live, &stripe, "2026-10", "admin", oct(20, 0)).await.is_err());

        let now = Utc.with_ymd_and_hms(2026, 11, 2, 0, 0, 0).unwrap();
        let o = approve_period(&db, Mode::Live, &stripe, "2026-10", "admin_1", now).await.unwrap();
        assert_eq!((o.invoiced, o.failed), (1, 0));
        assert_eq!(stripe.calls.lock().unwrap().len(), 2);
        let r = &list_periods(&db, "2026-10").await.unwrap()[0];
        assert_eq!(r.state, "invoiced");
        assert_eq!(r.approved_by.as_deref(), Some("admin_1"));
        assert_eq!(r.stripe_refs["minutes"], "ii_voice-billing-2026-10-u1-minutes");

        // Idempotent: approving again and recomputing don't send or change anything.
        let o = approve_period(&db, Mode::Live, &stripe, "2026-10", "admin_1", now).await.unwrap();
        assert_eq!((o.invoiced, o.skipped_already_invoiced), (0, 1));
        compute_period(&db, Mode::Live, "2026-10").await.unwrap();
        assert_eq!(list_periods(&db, "2026-10").await.unwrap()[0].state, "invoiced");
        assert_eq!(stripe.calls.lock().unwrap().len(), 2);
        clear_stripe_env();
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn partial_failure_is_retried_without_resending_what_went_through() {
        set_stripe_env();
        let db = pool().await;
        seed(&db).await;
        compute_period(&db, Mode::Live, "2026-10").await.unwrap();
        let now = Utc.with_ymd_and_hms(2026, 11, 2, 0, 0, 0).unwrap();
        let flaky = MockStripe { fail_numbers: true, ..Default::default() };
        let o = approve_period(&db, Mode::Live, &flaky, "2026-10", "a", now).await.unwrap();
        assert_eq!((o.invoiced, o.failed), (0, 1));
        let r = &list_periods(&db, "2026-10").await.unwrap()[0];
        assert_eq!(r.state, "failed");
        assert!(r.error.as_deref().unwrap().contains("stripe down"));
        assert!(r.stripe_refs.get("minutes").is_some());

        let ok = MockStripe::default();
        let o = approve_period(&db, Mode::Live, &ok, "2026-10", "a", now).await.unwrap();
        assert_eq!(o.invoiced, 1);
        let calls = ok.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "minutes were already sent");
        assert!(calls[0].0.ends_with("-numbers"));
        clear_stripe_env();
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn user_without_a_stripe_customer_fails_clearly_on_approve() {
        set_stripe_env();
        let db = pool().await;
        voice_usage::record_at(&db, "nocus", "phone", 600, "p", oct(3, 0)).await.unwrap();
        compute_period(&db, Mode::Live, "2026-10").await.unwrap();
        let r = &list_periods(&db, "2026-10").await.unwrap()[0];
        assert_eq!(r.state, "pending");
        assert!(r.error.as_deref().unwrap().starts_with("no_stripe_customer"));
        let stripe = MockStripe::default();
        let now = Utc.with_ymd_and_hms(2026, 11, 2, 0, 0, 0).unwrap();
        let o = approve_period(&db, Mode::Live, &stripe, "2026-10", "a", now).await.unwrap();
        assert_eq!((o.invoiced, o.failed), (0, 1));
        assert!(stripe.calls.lock().unwrap().is_empty());
        clear_stripe_env();
    }

    #[tokio::test]
    async fn estimate_month_matches_the_period_math() {
        let db = pool().await;
        seed(&db).await;
        let a = estimate_month(&db, "u1", oct(15, 0)).await.unwrap();
        assert_eq!(a.amount_cents, 13 * 8 + 2 * 200);
        assert_eq!(estimate_month(&db, "u2", oct(15, 0)).await.unwrap().amount_cents, 0);
    }
}
