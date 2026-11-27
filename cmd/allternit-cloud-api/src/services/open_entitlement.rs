//! Allternit Open entitlement (CONTRACT-open-tier-2026-11-26.md §2–§4):
//! who may run open models on Allternit Cloud, and on which money.
//!
//! - [`evaluate`]: the pure rule. Plan, current terms, the open allowance
//!   row and the wallet in; the `open_models` object (minus `period_end`)
//!   out, with the first `reason` that blocks, in the contract's order:
//!   `terms_required`, `plan_required`, `allowance_used`,
//!   `bought_credits_required`, else none.
//! - [`load`]: the one loader both `GET /api/v1/billing/credits` and
//!   `GET /api/v1/me/usage` serialize, and the chat gate reads.
//! - The §4 error bodies (`open_plan_required`, `open_lane_requires_credits`,
//!   `open_research_required`).
//!
//! Plan mapping: `plus` runs open models on bought credits only; `super`
//! (Max) and `ultra` spend the plan allowance (50% of the grant, written by
//! `grant_plan_credits_keyed`) first, then bought. `free`, no plan and every
//! other id, `team` included, get no Cloud open models: `team` is only the
//! `plan_tier` label of Max/Ultra in `billing_subscriptions.rs`, never a
//! `plan_id`, and no plan grant is ever written for it. The Open Research
//! flag (D10) opens bought-credit spend on any plan, Free included, and the
//! `research_only` rows.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use crate::{
    error::ApiError,
    services::open_lane::{self, OpenLaneSettings},
};

/// Where a Free account picks a plan (the 402 `open_plan_required` body).
pub const PLANS_URL: &str = "https://ai.allternit.com/settings/billing";
/// Where an account buys credits (the 402 `open_lane_requires_credits` body).
pub const BILLING_URL: &str = crate::services::inference_settlement::CLOUD_CREDITS_BILLING_URL;

pub const REASON_TERMS_REQUIRED: &str = "terms_required";
pub const REASON_PLAN_REQUIRED: &str = "plan_required";
pub const REASON_ALLOWANCE_USED: &str = "allowance_used";
pub const REASON_BOUGHT_CREDITS_REQUIRED: &str = "bought_credits_required";

const PLAN_REQUIRED_MESSAGE: &str =
    "Open models on Allternit Cloud need a paid plan. On Free, run open models on your own device with Freeweights.";
const ALLOWANCE_USED_MESSAGE: &str = "This month's open-model allowance is used. Add credits to keep going.";
const BOUGHT_CREDITS_REQUIRED_MESSAGE: &str = "On Plus, open models run on credits you buy. Add credits to use them.";
const RESEARCH_REQUIRED_MESSAGE: &str = "This model is part of Open Research, available by agreement.";

/// Which money a plan id may spend on open models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenFunds {
    /// No Cloud open models (Free, no plan, unknown id, `team`).
    None,
    /// Bought credits only (`plus`; any plan with the research flag).
    BoughtOnly,
    /// Plan allowance first, then bought (`super`, `ultra`).
    AllowanceThenBought,
}

fn funds_for(plan_id: &str, research: bool) -> OpenFunds {
    match plan_id {
        "super" | "ultra" => OpenFunds::AllowanceThenBought,
        "plus" => OpenFunds::BoughtOnly,
        _ if research => OpenFunds::BoughtOnly,
        _ => OpenFunds::None,
    }
}

/// The entitlement for one account: every `open_models` key except
/// `period_end` (see [`OpenModelsObject`]).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OpenEntitlement {
    pub allowed: bool,
    pub reason: Option<&'static str>,
    pub terms_accepted: bool,
    pub plan: String,
    pub plan_allowance_usd: f64,
    pub used_usd: f64,
    pub plan_remaining_usd: f64,
    pub bought_balance_usd: f64,
    pub research: bool,
}

/// The contract §3 `open_models` object.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OpenModelsObject {
    #[serde(flatten)]
    pub entitlement: OpenEntitlement,
    /// `billing_subscriptions.current_period_end` (RFC 3339, seconds), or
    /// null without a plan.
    pub period_end: Option<String>,
}

/// The pure rule (contract §2, D1, D5, D6, D10). `balance_usd` and
/// `bought_usd` are the raw `user_credits` columns; bought is read per D1 as
/// `LEAST(bought, GREATEST(balance, 0))` and the plan part is the rest of the
/// positive balance. Plan credits open models may still spend =
/// `min(allowance - used, plan part)` on Max/Ultra, 0 elsewhere.
pub fn evaluate(
    plan_id: &str,
    terms_accepted: bool,
    allowance_usd: f64,
    used_usd: f64,
    balance_usd: f64,
    bought_usd: f64,
    research: bool,
) -> OpenEntitlement {
    let positive = balance_usd.max(0.0);
    let bought = bought_usd.clamp(0.0, positive);
    let plan_part = positive - bought;
    let funds = funds_for(plan_id, research);
    let used = used_usd.max(0.0);
    let plan_allowance = match funds {
        OpenFunds::AllowanceThenBought => allowance_usd.max(0.0),
        _ => 0.0,
    };
    let plan_remaining = match funds {
        OpenFunds::AllowanceThenBought => (plan_allowance - used).max(0.0).min(plan_part),
        _ => 0.0,
    };

    let reason = if !terms_accepted {
        Some(REASON_TERMS_REQUIRED)
    } else {
        match funds {
            OpenFunds::None => Some(REASON_PLAN_REQUIRED),
            OpenFunds::AllowanceThenBought if plan_remaining <= 0.0 && bought <= 0.0 => Some(REASON_ALLOWANCE_USED),
            OpenFunds::BoughtOnly if bought <= 0.0 => Some(REASON_BOUGHT_CREDITS_REQUIRED),
            _ => None,
        }
    };

    OpenEntitlement {
        allowed: reason.is_none(),
        reason,
        terms_accepted,
        plan: plan_id.to_string(),
        plan_allowance_usd: plan_allowance,
        used_usd: used,
        plan_remaining_usd: plan_remaining,
        bought_balance_usd: bought,
        research,
    }
}

/// The first instant the current acceptable use text is in force:
/// `AUP_VERSION` (a `YYYY-MM-DD` date) at 00:00 UTC. An acceptance stamped
/// before it was for older text (D18). An unreadable version accepts
/// nothing (fail closed).
fn aup_version_start() -> Option<DateTime<Utc>> {
    NaiveDate::parse_from_str(open_lane::AUP_VERSION, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|at| at.and_utc())
}

/// Drop an acceptance of an older AUP text, so `terms_ok()` holds only for
/// the current `AUP_VERSION` (and the person can accept the new text: the
/// settings update stamps `now` only on an empty acceptance).
pub fn current_terms(mut settings: OpenLaneSettings) -> OpenLaneSettings {
    let current = match (settings.aup_accepted_at, aup_version_start()) {
        (Some(accepted), Some(start)) => accepted >= start,
        _ => false,
    };
    if !current {
        settings.aup_accepted_at = None;
    }
    if !settings.terms_ok() {
        settings.handoff_enabled = false;
    }
    settings
}

/// A person's open-lane settings with a stale acceptance dropped.
pub async fn load_terms(db: &PgPool, user_id: &str) -> Result<OpenLaneSettings, ApiError> {
    Ok(current_terms(open_lane::load_settings(db, user_id).await?))
}

#[derive(sqlx::FromRow)]
struct LaneRow {
    aup_accepted_at: Option<DateTime<Utc>>,
    age_confirmed_at: Option<DateTime<Utc>>,
    handoff_enabled: bool,
    research_enabled_at: Option<DateTime<Utc>>,
}

/// Load the `open_models` object for one account: plan
/// (`current_plan_id`, admins read as Ultra), terms for the current AUP
/// version, the research flag, the open allowance row, the wallet and the
/// plan's period end.
pub async fn load(db: &PgPool, user_id: &str) -> Result<OpenModelsObject, ApiError> {
    let plan_id = crate::routes::billing_subscriptions::current_plan_id(db, user_id).await?;

    let lane: Option<LaneRow> = sqlx::query_as(
        "SELECT aup_accepted_at, age_confirmed_at, handoff_enabled, research_enabled_at FROM user_open_lane WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    let (terms_accepted, research) = match lane {
        Some(row) => {
            let settings = current_terms(OpenLaneSettings {
                aup_accepted_at: row.aup_accepted_at,
                age_confirmed_at: row.age_confirmed_at,
                handoff_enabled: row.handoff_enabled,
            });
            (settings.terms_ok(), row.research_enabled_at.is_some())
        }
        None => (false, false),
    };

    let (allowance_usd, used_usd): (f64, f64) =
        sqlx::query_as("SELECT allowance_usd, used_usd FROM user_open_allowance WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(db)
            .await?
            .unwrap_or((0.0, 0.0));

    let (balance_usd, bought_usd): (Option<f64>, f64) =
        sqlx::query_as("SELECT balance_usd, bought_usd FROM user_credits WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(db)
            .await?
            .unwrap_or((None, 0.0));

    let period_end: Option<DateTime<Utc>> = if plan_id == "free" {
        None
    } else {
        sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            r#"
            SELECT current_period_end
            FROM billing_subscriptions
            WHERE user_id = $1 AND status IN ('active', 'trialing', 'past_due', 'unpaid')
            ORDER BY (status IN ('active', 'trialing')) DESC, updated_at DESC
            LIMIT 1
            "#,
        )
        .bind(user_id)
        .fetch_optional(db)
        .await?
        .flatten()
    };

    Ok(OpenModelsObject {
        entitlement: evaluate(
            &plan_id,
            terms_accepted,
            allowance_usd,
            used_usd,
            balance_usd.unwrap_or(0.0),
            bought_usd,
            research,
        ),
        period_end: period_end.map(|at| at.to_rfc3339_opts(SecondsFormat::Secs, true)),
    })
}

fn open_error(status: StatusCode, code: &str, message: &str, extra: Value) -> Response {
    let mut error = json!({ "message": message, "type": "invalid_request_error", "code": code });
    if let (Some(error), Some(extra)) = (error.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            error.insert(key.clone(), value.clone());
        }
    }
    (status, Json(json!({ "error": error }))).into_response()
}

/// The §4 refusal for an account whose entitlement blocks open models on
/// funds or plan. `None` when allowed, or when the reason is
/// `terms_required` (the caller answers with the unchanged 403
/// `open_lane_terms_required`).
pub fn refusal(open_models: &OpenModelsObject) -> Option<Response> {
    let body = serde_json::to_value(open_models).unwrap_or(Value::Null);
    match open_models.entitlement.reason? {
        REASON_PLAN_REQUIRED => Some(open_error(
            StatusCode::PAYMENT_REQUIRED,
            "open_plan_required",
            PLAN_REQUIRED_MESSAGE,
            json!({ "open_models": body, "plans_url": PLANS_URL }),
        )),
        reason @ (REASON_ALLOWANCE_USED | REASON_BOUGHT_CREDITS_REQUIRED) => Some(open_error(
            StatusCode::PAYMENT_REQUIRED,
            "open_lane_requires_credits",
            if reason == REASON_ALLOWANCE_USED { ALLOWANCE_USED_MESSAGE } else { BOUGHT_CREDITS_REQUIRED_MESSAGE },
            json!({
                "reason": reason,
                "open_models": body,
                "packs": crate::routes::billing_checkout::credit_pack_catalog(),
                "billing_url": BILLING_URL,
            }),
        )),
        _ => None,
    }
}

/// 403 `open_research_required`: a `research_only` row without the flag.
pub fn research_required_response() -> Response {
    open_error(StatusCode::FORBIDDEN, "open_research_required", RESEARCH_REQUIRED_MESSAGE, json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Contract §2 rows and every §3 reason. Columns: plan, terms, allowance,
    /// used, balance, bought, research → allowed, reason, plan_allowance,
    /// plan_remaining, bought_balance.
    #[test]
    fn entitlement_rule_table() {
        #[allow(clippy::type_complexity)]
        let rows: &[(&str, (&str, bool, f64, f64, f64, f64, bool), (bool, Option<&str>, f64, f64, f64))] = &[
            // Free / no plan / unknown / team: no Cloud open models, even with bought credits (D6).
            ("free with bought", ("free", true, 0.0, 0.0, 40.0, 40.0, false), (false, Some("plan_required"), 0.0, 0.0, 40.0)),
            ("unknown plan id", ("enterprise", true, 0.0, 0.0, 40.0, 40.0, false), (false, Some("plan_required"), 0.0, 0.0, 40.0)),
            ("team is not a plan id", ("team", true, 55.0, 0.0, 40.0, 40.0, false), (false, Some("plan_required"), 0.0, 0.0, 40.0)),
            // terms_required comes first, whatever the plan and money.
            ("free without terms", ("free", false, 0.0, 0.0, 0.0, 0.0, false), (false, Some("terms_required"), 0.0, 0.0, 0.0)),
            ("ultra without terms", ("ultra", false, 110.0, 0.0, 220.0, 0.0, false), (false, Some("terms_required"), 110.0, 110.0, 0.0)),
            // Plus: bought credits only; plan credits never count.
            ("plus with bought", ("plus", true, 0.0, 0.0, 30.0, 8.0, false), (true, None, 0.0, 0.0, 8.0)),
            ("plus plan credits only", ("plus", true, 0.0, 0.0, 22.0, 0.0, false), (false, Some("bought_credits_required"), 0.0, 0.0, 0.0)),
            ("plus allowance row ignored", ("plus", true, 11.0, 0.0, 22.0, 0.0, false), (false, Some("bought_credits_required"), 0.0, 0.0, 0.0)),
            // Max / Ultra: plan allowance first, then bought (D5).
            ("max allowance left", ("super", true, 55.0, 20.0, 110.0, 0.0, false), (true, None, 55.0, 35.0, 0.0)),
            ("max allowance capped by plan part", ("super", true, 55.0, 0.0, 30.0, 20.0, false), (true, None, 55.0, 10.0, 20.0)),
            ("max allowance used, bought left", ("super", true, 55.0, 55.0, 60.0, 5.0, false), (true, None, 55.0, 0.0, 5.0)),
            ("max allowance used, no bought", ("super", true, 55.0, 55.0, 60.0, 0.0, false), (false, Some("allowance_used"), 55.0, 0.0, 0.0)),
            ("ultra overrun counted in used", ("ultra", true, 110.0, 112.0, 90.0, 0.0, false), (false, Some("allowance_used"), 110.0, 0.0, 0.0)),
            // D1: bought reads as LEAST(bought, GREATEST(balance, 0)).
            ("bought clamped to balance", ("plus", true, 0.0, 0.0, 3.0, 9.0, false), (true, None, 0.0, 0.0, 3.0)),
            ("negative balance holds nothing", ("ultra", true, 110.0, 0.0, -4.0, 9.0, false), (false, Some("allowance_used"), 110.0, 0.0, 0.0)),
            // Research flag (D10): bought on any plan, allowance too on Max/Ultra.
            ("research on free with bought", ("free", true, 0.0, 0.0, 500.0, 500.0, true), (true, None, 0.0, 0.0, 500.0)),
            ("research on free, no bought", ("free", true, 0.0, 0.0, 0.0, 0.0, true), (false, Some("bought_credits_required"), 0.0, 0.0, 0.0)),
            ("research on plus with bought", ("plus", true, 0.0, 0.0, 50.0, 50.0, true), (true, None, 0.0, 0.0, 50.0)),
            ("research on ultra uses allowance", ("ultra", true, 110.0, 0.0, 220.0, 0.0, true), (true, None, 110.0, 110.0, 0.0)),
        ];
        for (name, (plan, terms, allowance, used, balance, bought, research), (allowed, reason, plan_allowance, remaining, bought_out)) in rows {
            let got = evaluate(plan, *terms, *allowance, *used, *balance, *bought, *research);
            assert_eq!(got.allowed, *allowed, "{name}: allowed");
            assert_eq!(got.reason, *reason, "{name}: reason");
            assert_eq!(got.plan_allowance_usd, *plan_allowance, "{name}: plan_allowance_usd");
            assert_eq!(got.plan_remaining_usd, *remaining, "{name}: plan_remaining_usd");
            assert_eq!(got.bought_balance_usd, *bought_out, "{name}: bought_balance_usd");
            assert_eq!((got.plan.as_str(), got.terms_accepted, got.research), (*plan, *terms, *research), "{name}: echo");
        }
    }
}
