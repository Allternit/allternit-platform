//! Spend: meter prices (spec §7), a project's spend this month, the monthly
//! spend cap and the `usage.threshold` webhooks.
//!
//! * **Prices.** Fixed-price meters are priced per unit from [`unit_price_microusd`].
//!   Text conversation tokens (`tokens_in`, `tokens_out`) have no fixed price:
//!   each row carries its billed amount (`amount_microusd`) = the provider's list
//!   price + 15% ([`token_charge_microusd`]), or 0 when the agent runs on the
//!   project's own model key. Hosted agents: the first [`FREE_AGENTS`] agent-months
//!   each month are free.
//! * **Spend** is the UTC calendar month's usage rows × prices, in micro-dollars
//!   ([`month_spend`]). It is what the month would bill at list price, before any
//!   plan credit or discount (those apply on the invoice).
//! * **Payment method.** Every project, sandbox included, needs a card on file
//!   and a plan (Stripe Checkout from the console, `project_billing`) before it
//!   can do billable work: [`spend_allowed`] answers `402 payment_method_required`
//!   first, with the console billing page in `error.url`.
//! * **Cap.** `platform_projects.spend_cap_cents` (default $100). Billable work
//!   calls [`spend_allowed`] first: at or over the cap it answers
//!   `402 spend_cap_reached`. One turn or text can still take a project a little
//!   past the cap; the next one is refused.
//! * **Thresholds.** After usage is recorded, [`check_thresholds`] sends
//!   `usage.threshold` at 50, 80 and 100% of the cap, each once per project and
//!   month (`platform_spend_alerts`).
//! * **Routes.** `GET /v1/projects/current/spend_cap` (scope `usage`) and
//!   `PUT` (scope `usage`, project-wide key). Over the API a cap can only be
//!   lowered; raising it is a console action (`PATCH /api/v1/platform/projects/{id}`),
//!   so a leaked key can't lift the project's fraud limit.

use std::sync::Arc;

use axum::{extract::State, routing::get, Json};
use chrono::{DateTime, Datelike, TimeZone, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use super::{events, ApiJson, PlatformCaller, PlatformError, RouteTable};
use crate::ApiState;

/// Text tokens bill at the provider's list price plus this many percent.
pub const TOKEN_MARKUP_PERCENT: i64 = 15;
/// Hosted agents free each month (spec §7: first 3 free, then $4 / agent / month).
pub const FREE_AGENTS: f64 = 3.0;
/// Hosted computer toolset actions free each month (Eoj 2026-10-08).
pub const FREE_COMPUTER_ACTIONS: f64 = 10_000.0;
/// Spend-cap thresholds that send `usage.threshold`, in percent.
pub const THRESHOLDS: [i64; 3] = [50, 80, 100];

pub fn register(table: RouteTable) -> RouteTable {
    table.add("/v1/projects/current/spend_cap", &["GET", "PUT"], get(get_cap).put(put_cap))
}

/// Price of one unit of a fixed-price meter, in micro-dollars (spec §7).
/// `None` for the token meters, whose rows carry their own amount.
pub fn unit_price_microusd(meter: &str) -> Option<i64> {
    Some(match meter {
        "voice_min_allternit" => 90_000,
        "voice_min_byok" => 60_000,
        "agent_month" => 4_000_000,
        "number_local_month" => 2_000_000,
        "number_tollfree_month" => 3_000_000,
        "sms_segment" => 12_000,
        "mms" => 30_000,
        // Quantity is already in cents (passed through at cost).
        "registration_passthrough_cents" => 10_000,
        "recording_min_month" => 2_000,
        // Hosted computer driver (Eoj 2026-10-08): 0.8¢ per computer-minute;
        // toolset actions 0.05¢ each after the first 10,000 a month.
        "computer_minute" => 8_000,
        "computer_action" => 500,
        _ => return None,
    })
}

/// What a project pays for tokens whose provider list price is `list_microusd`:
/// list + 15%, rounded up to the micro-dollar; nothing on the project's own key.
pub fn token_charge_microusd(list_microusd: i64, own_key: bool) -> i64 {
    if own_key || list_microusd <= 0 {
        return 0;
    }
    (list_microusd * (100 + TOKEN_MARKUP_PERCENT) + 99) / 100
}

/// One meter's month: summed quantity and summed row amounts.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MeterTotal {
    pub meter: String,
    pub quantity: f64,
    pub amount_microusd: i64,
}

/// Spend in micro-dollars from a month's per-meter totals.
pub fn spend_from_totals(totals: &[MeterTotal]) -> i64 {
    totals
        .iter()
        .map(|t| match unit_price_microusd(&t.meter) {
            Some(price) => {
                let units = match t.meter.as_str() {
                    "agent_month" => (t.quantity - FREE_AGENTS).max(0.0),
                    "computer_action" => (t.quantity - FREE_COMPUTER_ACTIONS).max(0.0),
                    _ => t.quantity.max(0.0),
                };
                (units * price as f64).round() as i64
            }
            None => t.amount_microusd.max(0),
        })
        .sum()
}

/// `[start, end)` of the UTC calendar month holding `now`, and its name (`2026-10`).
pub fn month_bounds(now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>, String) {
    let start = Utc.with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0).single().unwrap_or(now);
    let (y, m) = if now.month() == 12 { (now.year() + 1, 1) } else { (now.year(), now.month() + 1) };
    let end = Utc.with_ymd_and_hms(y, m, 1, 0, 0, 0).single().unwrap_or(now);
    (start, end, now.format("%Y-%m").to_string())
}

/// The project's spend so far in the month holding `now`, in micro-dollars.
pub async fn month_spend(db: &PgPool, project_id: &str, now: DateTime<Utc>) -> Result<i64, sqlx::Error> {
    let (start, end, _) = month_bounds(now);
    let totals = sqlx::query_as::<_, MeterTotal>(
        "SELECT meter, COALESCE(SUM(quantity), 0)::float8 AS quantity, COALESCE(SUM(amount_microusd), 0)::bigint AS amount_microusd \
         FROM platform_usage_events WHERE project_id = $1 AND created_at >= $2 AND created_at < $3 GROUP BY meter",
    )
    .bind(project_id)
    .bind(start)
    .bind(end)
    .fetch_all(db)
    .await?;
    Ok(spend_from_totals(&totals))
}

async fn cap_cents(db: &PgPool, project_id: &str) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT spend_cap_cents FROM platform_projects WHERE id = $1").bind(project_id).fetch_one(db).await
}

pub fn dollars(microusd: i64) -> String {
    format!("${:.2}", microusd as f64 / 1_000_000.0)
}

/// The console page where a project adds its card and picks a plan.
pub fn billing_page_url(project_id: &str) -> String {
    let base = std::env::var("ALLTERNIT_PLATFORM_CONSOLE_URL").unwrap_or_else(|_| "https://platform.allternit.com".into());
    format!("{}/platform/billing?project={project_id}", base.trim_end_matches('/'))
}

/// Billing states that may do billable work: a subscription that is paid up, or
/// one whose renewal failed while Stripe is still retrying (`past_due`, grace).
pub fn status_allows_work(billing_status: Option<&str>) -> bool {
    matches!(billing_status, Some("active" | "past_due"))
}

/// 402 `payment_method_required`, with the console billing page as `error.url`.
pub fn payment_method_required(project_id: &str) -> PlatformError {
    let url = billing_page_url(project_id);
    PlatformError::payment_required(
        "payment_method_required",
        format!(
            "This project has no payment method on file. Every project, sandbox included, needs a card and a plan \
             (Pay as you go or Growth) before it can do billable work; there is no free usage. Add one at {url}"
        ),
    )
    .with_url(url)
}

/// `Ok` once the project has a card on file and a subscription in good standing
/// (Eoj 2026-10-08: card required, $0 free usage, sandbox included); otherwise
/// `402 payment_method_required`. A failed check refuses the work (fail closed).
pub async fn require_payment_method(db: &PgPool, project_id: &str) -> Result<(), PlatformError> {
    let row: Result<Option<(Option<String>, Option<String>)>, sqlx::Error> =
        sqlx::query_as("SELECT stripe_customer_id, billing_status FROM platform_projects WHERE id = $1")
            .bind(project_id)
            .fetch_optional(db)
            .await;
    match row {
        Ok(Some((Some(_customer), status))) if status_allows_work(status.as_deref()) => Ok(()),
        Ok(_) => Err(payment_method_required(project_id)),
        Err(error) => {
            tracing::error!(%error, project_id, "platform payment method check failed; refusing billable work");
            Err(PlatformError::service_unavailable("spend_check_unavailable", "The project's billing couldn't be checked. Retry shortly."))
        }
    }
}

/// `Ok` while the project has a payment method on file ([`require_payment_method`])
/// and is under its monthly spend cap; `402 payment_method_required` without a
/// card, `402 spend_cap_reached` at or over the cap. A failed check refuses the
/// work (fail closed).
pub async fn spend_allowed(db: &PgPool, project_id: &str) -> Result<(), PlatformError> {
    require_payment_method(db, project_id).await?;
    let checked = async {
        let cap = cap_cents(db, project_id).await?;
        let spent = month_spend(db, project_id, Utc::now()).await?;
        Ok::<_, sqlx::Error>((cap, spent))
    }
    .await;
    let (cap, spent) = match checked {
        Ok(v) => v,
        Err(error) => {
            tracing::error!(%error, project_id, "platform spend check failed; refusing billable work");
            return Err(PlatformError::service_unavailable("spend_check_unavailable", "The project's spend couldn't be checked. Retry shortly."));
        }
    };
    if spent >= cap.saturating_mul(10_000) {
        let period = Utc::now().format("%Y-%m");
        return Err(PlatformError::payment_required(
            "spend_cap_reached",
            format!(
                "This project reached its monthly spend cap of {} ({} used in {period}). Raise the cap in the console to continue; it resets on the 1st (UTC).",
                dollars(cap.saturating_mul(10_000)),
                dollars(spent)
            ),
        ));
    }
    Ok(())
}

/// Thresholds (percent) that `spent` reaches under a cap of `cap_cents`.
pub fn thresholds_reached(spent_microusd: i64, cap_cents: i64) -> Vec<i64> {
    if spent_microusd <= 0 {
        return Vec::new();
    }
    let cap = cap_cents.max(0).saturating_mul(10_000);
    THRESHOLDS.iter().copied().filter(|t| spent_microusd.saturating_mul(100) >= cap.saturating_mul(*t)).collect()
}

/// Send `usage.threshold` for every threshold the project reached this month
/// and hasn't been told about yet. Returns the thresholds sent now.
pub async fn check_thresholds(db: &PgPool, project_id: &str) -> Result<Vec<i64>, sqlx::Error> {
    let now = Utc::now();
    let (_, _, period) = month_bounds(now);
    let cap = cap_cents(db, project_id).await?;
    let spent = month_spend(db, project_id, now).await?;
    let mut sent = Vec::new();
    for percent in thresholds_reached(spent, cap) {
        let first: Option<i32> = sqlx::query_scalar(
            "INSERT INTO platform_spend_alerts (project_id, period, percent, spend_microusd, cap_cents) VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (project_id, period, percent) DO NOTHING RETURNING percent",
        )
        .bind(project_id)
        .bind(&period)
        .bind(percent as i32)
        .bind(spent)
        .bind(cap)
        .fetch_optional(db)
        .await?;
        if first.is_none() {
            continue;
        }
        let data = json!({
            "meter": "spend",
            "percent": percent,
            "used": spent as f64 / 1_000_000.0,
            "limit": cap as f64 / 100.0,
            "period": period,
        });
        events::emit_event(db, project_id, None, "usage.threshold", data).await?;
        sent.push(percent);
    }
    Ok(sent)
}

/// [`check_thresholds`], logged instead of returned: usage recording never fails on it.
pub async fn after_usage(db: &PgPool, project_id: &str) {
    if let Err(error) = check_thresholds(db, project_id).await {
        tracing::warn!(%error, project_id, "platform spend thresholds not checked");
    }
}

async fn cap_json(db: &PgPool, caller: &PlatformCaller) -> Result<Value, PlatformError> {
    let now = Utc::now();
    let (_, _, period) = month_bounds(now);
    let cap = cap_cents(db, &caller.project_id).await?;
    let spent = month_spend(db, &caller.project_id, now).await?;
    let sent: Vec<i32> = sqlx::query_scalar("SELECT percent FROM platform_spend_alerts WHERE project_id = $1 AND period = $2 ORDER BY percent")
        .bind(&caller.project_id)
        .bind(&period)
        .fetch_all(db)
        .await?;
    let cap_micro = cap.saturating_mul(10_000);
    let payment_method = require_payment_method(db, &caller.project_id).await.is_ok();
    Ok(json!({
        "object": "spend_cap",
        "project_id": caller.project_id,
        "plan": caller.plan,
        "spend_cap_cents": cap,
        "period": period,
        "spent_cents": (spent + 9_999) / 10_000,
        "spent_microusd": spent,
        "remaining_cents": ((cap_micro - spent).max(0)) / 10_000,
        "reached": spent >= cap_micro,
        "thresholds_sent": sent,
        "payment_method": payment_method,
    }))
}

async fn get_cap(State(state): State<Arc<ApiState>>, caller: PlatformCaller) -> Result<Json<Value>, PlatformError> {
    caller.require("usage")?;
    Ok(Json(cap_json(&state.db, &caller).await?))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CapBody {
    spend_cap_cents: i64,
}

async fn put_cap(State(state): State<Arc<ApiState>>, caller: PlatformCaller, ApiJson(body): ApiJson<CapBody>) -> Result<Json<Value>, PlatformError> {
    caller.require("usage")?;
    caller.require_unbound()?;
    if !(0..=super::projects::MAX_SPEND_CAP_CENTS).contains(&body.spend_cap_cents) {
        return Err(PlatformError::invalid_request("invalid_spend_cap", "spend_cap_cents must be between 0 and 100000000.").with_param("spend_cap_cents"));
    }
    // Only ever lowers: a concurrent console raise is never undone by an API call.
    let updated = sqlx::query("UPDATE platform_projects SET spend_cap_cents = $2 WHERE id = $1 AND spend_cap_cents >= $2")
        .bind(&caller.project_id)
        .bind(body.spend_cap_cents)
        .execute(&state.db)
        .await?;
    if updated.rows_affected() == 0 {
        return Err(PlatformError::permission(
            "spend_cap_raise_in_console",
            "An API key can only lower the spend cap. Raise it in the console (platform.allternit.com).",
        )
        .with_param("spend_cap_cents"));
    }
    // A lower cap may already be crossed.
    after_usage(&state.db, &caller.project_id).await;
    Ok(Json(cap_json(&state.db, &caller).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(meter: &str, quantity: f64, amount: i64) -> MeterTotal {
        MeterTotal { meter: meter.into(), quantity, amount_microusd: amount }
    }

    #[test]
    fn tokens_bill_list_price_plus_15_percent_and_nothing_on_an_own_key() {
        assert_eq!(token_charge_microusd(1_000_000, false), 1_150_000);
        assert_eq!(token_charge_microusd(1, false), 2, "rounded up to the micro-dollar");
        assert_eq!(token_charge_microusd(1_000_000, true), 0);
        assert_eq!(token_charge_microusd(0, false), 0);
    }

    #[test]
    fn spend_prices_every_meter() {
        let totals = [
            t("voice_min_allternit", 10.0, 0),       // $0.90
            t("voice_min_byok", 10.0, 0),            // $0.60
            t("agent_month", 5.0, 0),                // 2 billable × $4
            t("number_local_month", 1.0, 0),         // $2
            t("number_tollfree_month", 1.0, 0),      // $3
            t("sms_segment", 100.0, 0),              // $1.20
            t("mms", 10.0, 0),                       // $0.30
            t("registration_passthrough_cents", 2000.0, 0), // $20
            t("recording_min_month", 100.0, 0),      // $0.20
            t("tokens_in", 50_000.0, 230_000),       // amounts as recorded
            t("tokens_out", 2_000.0, 115_000),
        ];
        let expected = 900_000 + 600_000 + 8_000_000 + 2_000_000 + 3_000_000 + 1_200_000 + 300_000 + 20_000_000 + 200_000 + 345_000;
        assert_eq!(spend_from_totals(&totals), expected);
        assert_eq!(spend_from_totals(&[t("agent_month", 3.0, 0)]), 0, "first three agents are free");
        assert_eq!(spend_from_totals(&[t("tokens_in", 1e6, 0)]), 0, "own-key tokens cost nothing");
    }

    #[test]
    fn thresholds_and_month_bounds() {
        assert!(thresholds_reached(0, 10_000).is_empty());
        assert_eq!(thresholds_reached(49_999_999, 10_000), Vec::<i64>::new());
        assert_eq!(thresholds_reached(50_000_000, 10_000), vec![50]);
        assert_eq!(thresholds_reached(80_000_000, 10_000), vec![50, 80]);
        assert_eq!(thresholds_reached(100_000_000, 10_000), vec![50, 80, 100]);
        assert_eq!(thresholds_reached(1, 0), vec![50, 80, 100], "a zero cap is reached by any spend");
        let (s, e, p) = month_bounds(Utc.with_ymd_and_hms(2026, 12, 15, 8, 0, 0).unwrap());
        assert_eq!((s.to_rfc3339(), e.to_rfc3339(), p.as_str()), ("2026-12-01T00:00:00+00:00".into(), "2027-01-01T00:00:00+00:00".into(), "2026-12"));
    }
}
