//! GET /api/v1/me/usage — Allternit subscription + remaining compute.
//!
//! Desktop forwards the paired runtime device token through local
//! `allternit-api` to this route. Clerk sessions and `allternit_*` API tokens
//! also work. The payload is the compute meter the shell should show: plan
//! (Free / Plus / Max / Ultra), the remaining credits and `cloud_models`
//! (whether Allternit Cloud models may run; there is no free allowance,
//! CONTRACT-cloud-credits-2026-11-15). Local Ollama is not this signal.
//!
//! Console usage (Wave 3), same callers, outside `console_gate`:
//! - `GET /api/v1/me/usage/report` — totals grouped by day, model, project,
//!   key or client; JSON or CSV; `from` inclusive, `to` exclusive.
//! - `GET /api/v1/me/usage/requests` — billed requests, newest first, cursor.
//! - `GET /api/v1/me/usage/requests/:request_id` — one request, with its
//!   bodies when the project kept them (`log_bodies` + `PLATFORM_LOG_BODIES`).
//!
//! `scope=me` (default) is the caller's rows; `scope=org` is every row billed
//! to a project of the caller's Clerk org, for org admins, with the org read
//! from the session claims only.

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool};
use std::sync::Arc;

use crate::{
    auth,
    error::ApiError,
    routes::billing_subscriptions::find_plan,
    routes::platform_v1::{
        page,
        projects::{self, Principal},
    },
    routes::runtime_pairing::{device_token_from_headers, runtime_device_for_token},
    services, ApiState,
};

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageResponse {
    pub plan: String,
    pub label: String,
    pub status: String,
    pub plan_tier: String,
    pub weekly_used: f64,
    pub weekly_limit: f64,
    pub resets_at: Option<String>,
    pub credits: Option<f64>,
    pub month_to_date_usage_usd: f64,
    pub recent_transactions: Vec<UsageTransaction>,
    /// Rolling 5-hour, rolling 7-day and current-month spend windows (snake_case keys inside).
    pub windows: Vec<UsageWindow>,
    pub pace: UsagePace,
    /// Usage credits toggle (`user_credits.carry_past_windows`): credits carry the account past
    /// an enforced 5h / 7d window while the balance is above 0.
    pub carry_past_windows: bool,
    /// Same object as on `GET /api/v1/billing/credits` (contract D4); the
    /// key is snake_case on both endpoints.
    #[serde(rename = "cloud_models")]
    pub cloud_models: services::CloudModelsEntitlement,
}

/// One spend window. Paid Cloud plans have limits (decision E3, "B Roomy"); 5h and 7d are
/// enforced at the Cloud admission gate ([`check_usage_windows`]), month is the grant.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct UsageWindow {
    pub id: String,
    pub label: String,
    pub used_usd: f64,
    pub limit_usd: Option<f64>,
    pub used_ratio: Option<f64>,
    pub resets_at: String,
    pub enforced: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct UsagePace {
    pub window_id: String,
    pub burn_per_hour: f64,
    pub runs_out_at: Option<String>,
    /// "ok" | "warn" | "out"
    pub status: String,
}

/// Per-window spend limits for one plan, in USD. `None` = no limit configured.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WindowLimits {
    pub five_hour: Option<f64>,
    pub seven_day: Option<f64>,
    pub month: Option<f64>,
}

/// Per-plan window limits in USD of Cloud inference spend (`inference_usage.cost_usd`), keyed by
/// plan id. Eoj approved "B Roomy" 2026-11-26 (DECISIONS E3): month = the monthly grant. Hosted
/// computer time is not counted. Plans not listed (free, no plan) have no windows enforced.
pub(crate) const PLAN_WINDOW_LIMITS: &[(&str, WindowLimits)] = &[
    ("plus", WindowLimits { five_hour: Some(1.83), seven_day: Some(7.33), month: Some(22.0) }),
    ("super", WindowLimits { five_hour: Some(9.17), seven_day: Some(36.67), month: Some(110.0) }),
    ("ultra", WindowLimits { five_hour: Some(18.33), seven_day: Some(73.33), month: Some(220.0) }),
];

/// Windows refused at the admission gate (the month cap is the grant, held by the credits gate).
pub(crate) fn is_enforced_window(id: &str, limit: Option<f64>) -> bool {
    matches!(id, "5h" | "7d") && limit.map(|l| l > 0.0).unwrap_or(false)
}

/// The admission verdict for one request. Pure: spend, limits, the usage-credits toggle and the
/// balance are inputs. `Some(window id)` = refuse with 429 for that window. Over an enforced window
/// the request is allowed only when `carry_past_windows` is on AND the balance is above 0 (then it
/// is billed from the balance as today). 5h is checked before 7d.
pub(crate) fn refused_window(
    spend: WindowSpend,
    limits: WindowLimits,
    carry_past_windows: bool,
    balance: Option<f64>,
) -> Option<&'static str> {
    let over = [("5h", spend.five_hour, limits.five_hour), ("7d", spend.seven_day, limits.seven_day)]
        .into_iter()
        .find(|(id, used, limit)| is_enforced_window(id, *limit) && *used >= limit.unwrap_or(f64::MAX))
        .map(|(id, _, _)| id)?;
    let carried = carry_past_windows && balance.map(|b| b > 0.0).unwrap_or(false);
    if carried { None } else { Some(over) }
}

/// The paid Cloud plan whose windows apply to `user_id` (`plus` | `super` | `ultra`), else None
/// (free / no plan / not active). Admins are not limited.
async fn enforced_plan(db: &sqlx::PgPool, user_id: &str) -> Option<String> {
    if crate::auth::is_admin_user(user_id) {
        return None;
    }
    let plan: Option<String> = sqlx::query_scalar(
        "SELECT plan_id FROM billing_subscriptions \
         WHERE user_id = $1 AND status IN ('active', 'trialing') \
         ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
    .ok()
    .flatten();
    plan.filter(|p| matches!(p.as_str(), "plus" | "super" | "ultra"))
}

/// Cloud inference spend over each window (`inference_usage.cost_usd` only: hosted computer cost
/// lives elsewhere and is not counted).
pub(crate) async fn window_spend(db: &sqlx::PgPool, user_id: &str, now: DateTime<Utc>) -> WindowSpend {
    let (five_hour, seven_day, month): (f64, f64, f64) = sqlx::query_as(
        r#"
        SELECT
            COALESCE(SUM(cost_usd) FILTER (WHERE created_at >= $2), 0)::float8,
            COALESCE(SUM(cost_usd) FILTER (WHERE created_at >= $3), 0)::float8,
            COALESCE(SUM(cost_usd) FILTER (WHERE created_at >= $4), 0)::float8
        FROM inference_usage
        WHERE user_id = $1 AND created_at >= LEAST($2, $3, $4)
        "#,
    )
    .bind(user_id)
    .bind(now - Duration::hours(5))
    .bind(now - Duration::days(7))
    .bind(month_start(now))
    .fetch_one(db)
    .await
    .unwrap_or((0.0, 0.0, 0.0));
    WindowSpend { five_hour, seven_day, month }
}

/// When a rolling window drops back under its limit: the first in-window row whose roll-off
/// leaves spend below the limit, plus the window length. Falls back to now + window length.
async fn window_resets_at(
    db: &sqlx::PgPool,
    user_id: &str,
    window: &str,
    used: f64,
    limit: f64,
    now: DateTime<Utc>,
) -> DateTime<Utc> {
    let length = if window == "5h" { Duration::hours(5) } else { Duration::days(7) };
    let first: Option<DateTime<Utc>> = sqlx::query_scalar(
        r#"
        SELECT created_at FROM (
            SELECT created_at, SUM(cost_usd) OVER (ORDER BY created_at, id) AS rolled
            FROM inference_usage
            WHERE user_id = $1 AND created_at >= $2
        ) t
        WHERE $3::float8 - rolled < $4::float8
        ORDER BY created_at
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .bind(now - length)
    .bind(used)
    .bind(limit)
    .fetch_optional(db)
    .await
    .ok()
    .flatten();
    first.map(|t| t + length).filter(|t| *t > now).unwrap_or(now + length)
}

/// The usage-credits toggle (`user_credits.carry_past_windows`, migration 168). False without a
/// credits row.
pub(crate) async fn carry_past_windows(db: &sqlx::PgPool, user_id: &str) -> bool {
    sqlx::query_scalar("SELECT carry_past_windows FROM user_credits WHERE user_id = $1")
        .bind(user_id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .unwrap_or(false)
}

/// Enforced Cloud usage windows, checked at the Cloud admission gate right before
/// `check_inference_allowed` (model router `/v1/chat/completions` and the artifact runtime's
/// `billed_completion`). Free / no-plan accounts and admins are unchanged. `balance` is the
/// caller's `credit_balance_row`.
pub async fn check_usage_windows(db: &sqlx::PgPool, user_id: &str, balance: Option<f64>) -> Result<(), ApiError> {
    let Some(plan) = enforced_plan(db, user_id).await else {
        return Ok(());
    };
    let limits = window_limits_for(&plan);
    let now = Utc::now();
    let spend = window_spend(db, user_id, now).await;
    // Toggle read only when a window is actually over (keeps the common path at two queries).
    if refused_window(spend, limits, false, None).is_none() {
        return Ok(());
    }
    let carry = carry_past_windows(db, user_id).await;
    let Some(window) = refused_window(spend, limits, carry, balance) else {
        return Ok(());
    };
    let (used, limit) = if window == "5h" {
        (spend.five_hour, limits.five_hour.unwrap_or(0.0))
    } else {
        (spend.seven_day, limits.seven_day.unwrap_or(0.0))
    };
    let resets_at = window_resets_at(db, user_id, window, used, limit, now).await;
    Err(ApiError::UsageWindowExhausted { window, resets_at: resets_at.to_rfc3339() })
}

pub(crate) fn window_limits_for(plan_id: &str) -> WindowLimits {
    PLAN_WINDOW_LIMITS
        .iter()
        .find(|(id, _)| *id == plan_id)
        .map(|(_, limits)| *limits)
        .unwrap_or_default()
}

/// Spend (USD) from the inference ledger over each window.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WindowSpend {
    pub five_hour: f64,
    pub seven_day: f64,
    pub month: f64,
}

pub(crate) fn month_start(now: DateTime<Utc>) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
        .single()
        .unwrap_or(now)
}

/// Build `windows` and `pace` from ledger spend. Pure: `now` and `limits` are inputs.
pub(crate) fn compose_windows(
    spend: WindowSpend,
    limits: WindowLimits,
    now: DateTime<Utc>,
) -> (Vec<UsageWindow>, UsagePace) {
    let month_resets = next_month_start(now);
    let month_hours = ((now - month_start(now)).num_seconds() as f64 / 3600.0).max(1.0);
    // (id, label, used, limit, resets_at, hours elapsed in window)
    let rows: [(&str, &str, f64, Option<f64>, DateTime<Utc>, f64); 3] = [
        ("5h", "5-hour", spend.five_hour, limits.five_hour, now + Duration::hours(5), 5.0),
        ("7d", "7-day", spend.seven_day, limits.seven_day, now + Duration::days(7), 168.0),
        ("month", "This month", spend.month, limits.month, month_resets, month_hours),
    ];
    let windows: Vec<UsageWindow> = rows
        .iter()
        .map(|(id, label, used, limit, resets, _)| UsageWindow {
            id: id.to_string(),
            label: label.to_string(),
            used_usd: *used,
            limit_usd: *limit,
            used_ratio: limit.filter(|l| *l > 0.0).map(|l| used / l),
            resets_at: resets.to_rfc3339(),
            enforced: is_enforced_window(id, *limit),
        })
        .collect();

    // Pace follows the window closest to its limit; with no limits it reports month burn.
    let pick = rows
        .iter()
        .filter(|r| r.3.map(|l| l > 0.0).unwrap_or(false))
        .max_by(|a, b| {
            let ra = a.2 / a.3.unwrap_or(1.0);
            let rb = b.2 / b.3.unwrap_or(1.0);
            ra.partial_cmp(&rb).unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(&rows[2]);
    let (id, _, used, limit, resets, hours) = *pick;
    let burn = if hours > 0.0 { used / hours } else { 0.0 };
    let (runs_out_at, status) = match limit.filter(|l| *l > 0.0) {
        None => (None, "ok"),
        Some(l) if used >= l => (Some(now), "out"),
        Some(l) if burn > 0.0 => {
            let secs = ((l - used) / burn * 3600.0) as i64;
            let at = now + Duration::seconds(secs);
            (Some(at), if at < resets { "warn" } else { "ok" })
        }
        Some(_) => (None, "ok"),
    };
    let pace = UsagePace {
        window_id: id.to_string(),
        burn_per_hour: burn,
        runs_out_at: runs_out_at.map(|t| t.to_rfc3339()),
        status: status.to_string(),
    };
    (windows, pace)
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageTransaction {
    pub amount_usd: f64,
    pub source: String,
    pub created_at: String,
}

#[derive(Debug, FromRow)]
struct CreditTxnRow {
    amount_usd: f64,
    source: String,
    created_at: DateTime<Utc>,
}

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/me/usage", get(get_my_usage))
        // Usage credits toggle (C1b): `{carry_past_windows}`.
        .route("/api/v1/me/usage/settings", get(get_usage_settings).put(put_usage_settings))
        // Usage report + request logs (console Wave 3, lane L3B). Outside
        // console_gate on purpose: Gizzi and API tokens resolve here too.
        .route("/api/v1/me/usage/report", get(get_usage_report))
        .route("/api/v1/me/usage/requests", get(list_usage_requests))
        .route("/api/v1/me/usage/requests/:request_id", get(get_usage_request))
}

// ─── Usage report + request logs (console Wave 3, lane L3B) ─────────────────
//
// `inference_usage` holds one row per billed completion (failed calls write
// none, so the logs list billed requests only). Scope `me` is the caller's own
// rows; scope `org` is every row billed to a project of the caller's Clerk org,
// for an org admin only, with the org taken from the verified session claims.

/// Default report window in days: the last 30 UTC days, today included.
const DEFAULT_REPORT_DAYS: i64 = 30;
/// Longest report window, so one request cannot scan years of rows.
const MAX_REPORT_DAYS: i64 = 366;
/// Most groups one report returns.
const MAX_REPORT_ROWS: i64 = 1000;
const DEFAULT_REQUESTS_LIMIT: i64 = 50;
const MAX_REQUESTS_LIMIT: i64 = 200;
pub(crate) const CSV_HEADER: &str = "group,requests,prompt_tokens,completion_tokens,cost_usd";

/// Who is reading usage. The org fields come only from a verified Clerk
/// session's claims; API tokens and device tokens never carry an org.
#[derive(Debug, Clone)]
pub(crate) struct UsageViewer {
    pub user_id: String,
    pub org_id: Option<String>,
    pub org_admin: bool,
}

impl UsageViewer {
    fn principal(&self) -> Principal {
        Principal {
            user_id: self.user_id.clone(),
            org_id: self.org_id.clone(),
            org_admin: self.org_admin,
        }
    }
}

/// Whose rows a usage query covers.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum UsageScope {
    /// Rows the caller made (`inference_usage.user_id`).
    Me(String),
    /// Rows billed to any project whose `org_id` is this org.
    Org(String),
}

impl UsageScope {
    fn binds(&self) -> (Option<&str>, Option<&str>) {
        match self {
            Self::Me(user_id) => (Some(user_id.as_str()), None),
            Self::Org(org_id) => (None, Some(org_id.as_str())),
        }
    }
}

/// The filters every usage query shares. `to` is exclusive.
#[derive(Debug, Clone)]
pub(crate) struct UsageFilter {
    pub scope: UsageScope,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub project_id: Option<String>,
    pub client: Option<String>,
    pub model: Option<String>,
}

impl UsageFilter {
    pub(crate) fn scoped(scope: UsageScope) -> Self {
        Self {
            scope,
            from: None,
            to: None,
            project_id: None,
            client: None,
            model: None,
        }
    }
}

/// Predicate on `inference_usage` for a [`UsageFilter`], bound as $1..$7 by
/// [`bind_filter!`]. The org arm follows the projects.rs access rule: an org
/// admin manages every project whose `org_id` is their org.
const FILTER_SQL: &str = "($1::timestamptz IS NULL OR created_at >= $1) \
    AND ($2::timestamptz IS NULL OR created_at < $2) \
    AND (($3::text IS NOT NULL AND user_id = $3) \
         OR ($4::text IS NOT NULL AND project_id IN (SELECT id FROM platform_projects WHERE org_id = $4))) \
    AND ($5::text IS NULL OR project_id = $5) \
    AND ($6::text IS NULL OR client = $6) \
    AND ($7::text IS NULL OR model = $7)";

/// Bind a [`UsageFilter`] as $1..$7 of [`FILTER_SQL`].
macro_rules! bind_filter {
    ($query:expr, $filter:expr) => {{
        let filter: &UsageFilter = $filter;
        let (me, org) = filter.scope.binds();
        $query
            .bind(filter.from)
            .bind(filter.to)
            .bind(me)
            .bind(org)
            .bind(filter.project_id.as_deref())
            .bind(filter.client.as_deref())
            .bind(filter.model.as_deref())
    }};
}

/// One report row: a group and its totals.
#[derive(Debug, Clone, Serialize, FromRow, PartialEq)]
pub(crate) struct ReportRow {
    pub group: Option<String>,
    pub requests: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub cost_usd: f64,
}

/// One billed request. `request_id` falls back to the usage row id for rows
/// written before request ids existed, and the detail route accepts either.
#[derive(Debug, Clone, Serialize, FromRow)]
pub(crate) struct RequestRow {
    #[serde(skip)]
    pub id: String,
    pub request_id: String,
    #[serde(skip)]
    pub raw_request_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub model: String,
    pub project_id: Option<String>,
    pub key_id: Option<String>,
    pub client: Option<String>,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub cost_usd: f64,
    pub latency_ms: Option<i64>,
}

const REQUEST_COLUMNS: &str = "id, COALESCE(request_id, id) AS request_id, request_id AS raw_request_id, \
    created_at, model, project_id, key_id, client, prompt_tokens, completion_tokens, cost_usd, latency_ms";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GroupBy {
    Day,
    Model,
    Project,
    Key,
    Client,
}

impl GroupBy {
    pub(crate) fn parse(raw: Option<&str>) -> Result<Self, ApiError> {
        match raw.map(str::trim).filter(|v| !v.is_empty()).unwrap_or("day") {
            "day" => Ok(Self::Day),
            "model" => Ok(Self::Model),
            "project" => Ok(Self::Project),
            "key" => Ok(Self::Key),
            "client" => Ok(Self::Client),
            _ => Err(ApiError::BadRequest(
                "group_by must be day, model, project, key or client.".to_string(),
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Model => "model",
            Self::Project => "project",
            Self::Key => "key",
            Self::Client => "client",
        }
    }

    /// The SQL expression rows are grouped by (a fixed list, never input).
    fn column(self) -> &'static str {
        match self {
            Self::Day => "to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD')",
            Self::Model => "model",
            Self::Project => "project_id",
            Self::Key => "key_id",
            Self::Client => "client",
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ReportParams {
    pub from: Option<String>,
    pub to: Option<String>,
    pub group_by: Option<String>,
    pub project_id: Option<String>,
    pub client: Option<String>,
    pub model: Option<String>,
    pub scope: Option<String>,
    pub format: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct RequestsParams {
    pub cursor: Option<String>,
    pub limit: Option<String>,
    pub model: Option<String>,
    pub client: Option<String>,
    pub project_id: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub scope: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct DetailParams {
    pub scope: Option<String>,
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|v| !v.is_empty())
}

fn non_empty_owned(value: &Option<String>) -> Option<String> {
    non_empty(value).map(str::to_string)
}

/// The caller of a usage route: Clerk session, `allternit_*` / `alt_*` user
/// token, or a paired runtime's device token.
async fn usage_viewer(state: &ApiState, headers: &HeaderMap) -> Result<UsageViewer, ApiError> {
    match auth::resolve_user(&state.db, headers).await {
        Ok(user) => {
            // Org claims count only on a Clerk session (tokens carry permissions).
            let session = user.token_permissions.is_none();
            let org_id = if session { user.organization_id } else { None };
            let org_admin = org_id.is_some()
                && matches!(user.org_role.as_deref(), Some("admin") | Some("org:admin"));
            Ok(UsageViewer {
                user_id: user.id,
                org_id,
                org_admin,
            })
        }
        Err(primary) => {
            let Some(token) = device_token_from_headers(headers) else {
                return Err(primary);
            };
            let user_id = runtime_device_for_token(&state.db, token, None)
                .await?
                .user_id;
            Ok(UsageViewer {
                user_id,
                org_id: None,
                org_admin: false,
            })
        }
    }
}

/// `scope=me` (default) or `scope=org`; org needs a Clerk org admin session.
pub(crate) fn resolve_scope(viewer: &UsageViewer, scope: Option<&str>) -> Result<UsageScope, ApiError> {
    match scope.map(str::trim).filter(|v| !v.is_empty()).unwrap_or("me") {
        "me" => Ok(UsageScope::Me(viewer.user_id.clone())),
        "org" => match (&viewer.org_id, viewer.org_admin) {
            (Some(org_id), true) => Ok(UsageScope::Org(org_id.clone())),
            _ => Err(ApiError::Forbidden(
                "scope=org needs an organization admin session.".to_string(),
            )),
        },
        _ => Err(ApiError::BadRequest("scope must be me or org.".to_string())),
    }
}

/// A `project_id` filter must name a project the viewer may manage (the
/// projects.rs access rule); anything else is a 404, never an empty report.
pub(crate) async fn check_project(
    db: &PgPool,
    viewer: &UsageViewer,
    project_id: Option<&str>,
) -> Result<(), ApiError> {
    if let Some(project_id) = project_id {
        projects::get_project(db, &viewer.principal(), project_id)
            .await
            .map_err(|_| ApiError::NotFound("No such project.".to_string()))?;
    }
    Ok(())
}

fn day_start(date: NaiveDate) -> DateTime<Utc> {
    Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).expect("midnight exists"))
}

/// A `from` / `to` bound: `YYYY-MM-DD` (midnight UTC) or RFC 3339.
pub(crate) fn parse_bound(raw: &str, name: &str) -> Result<DateTime<Utc>, ApiError> {
    // A `+hh:mm` offset arrives as a space once the query string is decoded.
    let raw = raw.trim().replace(' ', "+");
    if let Ok(date) = NaiveDate::parse_from_str(&raw, "%Y-%m-%d") {
        return Ok(day_start(date));
    }
    DateTime::parse_from_rfc3339(&raw)
        .map(|at| at.with_timezone(&Utc))
        .map_err(|_| {
            ApiError::BadRequest(format!(
                "{name} must be a date (YYYY-MM-DD) or an RFC 3339 time."
            ))
        })
}

/// The report window: `from` inclusive, `to` exclusive. Defaults to the last
/// 30 UTC days, today included.
pub(crate) fn report_range(
    from: Option<&str>,
    to: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(DateTime<Utc>, DateTime<Utc>), ApiError> {
    let to = match to {
        Some(raw) => parse_bound(raw, "to")?,
        None => day_start(now.date_naive()) + Duration::days(1),
    };
    let from = match from {
        Some(raw) => parse_bound(raw, "from")?,
        None => to - Duration::days(DEFAULT_REPORT_DAYS),
    };
    if from >= to {
        return Err(ApiError::BadRequest(
            "from must be before to (to is exclusive).".to_string(),
        ));
    }
    if to - from > Duration::days(MAX_REPORT_DAYS) {
        return Err(ApiError::BadRequest(format!(
            "A report covers at most {MAX_REPORT_DAYS} days."
        )));
    }
    Ok((from, to))
}

/// Grouped totals, sorted by day ascending (group_by=day) or cost descending.
pub(crate) async fn usage_report_rows(
    db: &PgPool,
    filter: &UsageFilter,
    group_by: GroupBy,
) -> Result<Vec<ReportRow>, ApiError> {
    let order = if group_by == GroupBy::Day {
        "1 ASC"
    } else {
        "5 DESC, 1 ASC NULLS LAST"
    };
    let sql = format!(
        "SELECT {column} AS \"group\", COUNT(*)::bigint AS requests, \
            COALESCE(SUM(prompt_tokens), 0)::bigint AS prompt_tokens, \
            COALESCE(SUM(completion_tokens), 0)::bigint AS completion_tokens, \
            COALESCE(SUM(cost_usd), 0)::float8 AS cost_usd \
         FROM inference_usage WHERE {FILTER_SQL} \
         GROUP BY 1 ORDER BY {order} LIMIT {MAX_REPORT_ROWS}",
        column = group_by.column(),
    );
    Ok(bind_filter!(sqlx::query_as::<_, ReportRow>(&sql), filter)
        .fetch_all(db)
        .await?)
}

/// Billed requests, newest first, keyset-paged on `(created_at, id)`.
/// Returns the page and the cursor for the next one.
pub(crate) async fn usage_request_page(
    db: &PgPool,
    filter: &UsageFilter,
    after: Option<(DateTime<Utc>, String)>,
    limit: i64,
) -> Result<(Vec<RequestRow>, Option<String>), ApiError> {
    let (after_at, after_id) = match after {
        Some((at, id)) => (Some(at), Some(id)),
        None => (None, None),
    };
    let sql = format!(
        "SELECT {REQUEST_COLUMNS} FROM inference_usage WHERE {FILTER_SQL} \
         AND ($8::timestamptz IS NULL OR (created_at, id) < ($8, $9::text)) \
         ORDER BY created_at DESC, id DESC LIMIT $10"
    );
    let mut rows = bind_filter!(sqlx::query_as::<_, RequestRow>(&sql), filter)
        .bind(after_at)
        .bind(after_id)
        .bind(limit + 1)
        .fetch_all(db)
        .await?;
    let has_more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    let next_cursor = if has_more {
        rows.last().map(|row| page::encode_cursor(row.created_at, &row.id))
    } else {
        None
    };
    Ok((rows, next_cursor))
}

/// One request's metadata plus its captured bodies (`request` / `response`,
/// null unless the project kept them and they have not expired). An open-lane
/// handoff settles twice under one request id: the totals add up and the
/// model is the one that answered.
pub(crate) async fn usage_request_detail(
    db: &PgPool,
    scope: UsageScope,
    request_id: &str,
) -> Result<Value, ApiError> {
    let filter = UsageFilter::scoped(scope);
    let sql = format!(
        "SELECT {REQUEST_COLUMNS} FROM inference_usage WHERE {FILTER_SQL} \
         AND (request_id = $8 OR id = $8) ORDER BY created_at, id"
    );
    let rows = bind_filter!(sqlx::query_as::<_, RequestRow>(&sql), &filter)
        .bind(request_id)
        .fetch_all(db)
        .await?;
    let (Some(first), Some(last)) = (rows.first(), rows.last()) else {
        return Err(ApiError::NotFound("No such request.".to_string()));
    };
    let mut meta = last.clone();
    meta.created_at = first.created_at;
    meta.prompt_tokens = rows.iter().map(|row| row.prompt_tokens).sum();
    meta.completion_tokens = rows.iter().map(|row| row.completion_tokens).sum();
    meta.cost_usd = rows.iter().map(|row| row.cost_usd).sum();

    let bodies: Option<(Option<sqlx::types::Json<Value>>, Option<sqlx::types::Json<Value>>)> =
        match (meta.raw_request_id.as_deref(), meta.project_id.as_deref()) {
            (Some(raw_request_id), Some(project_id)) => {
                sqlx::query_as(
                    "SELECT request, response FROM inference_request_bodies \
                     WHERE request_id = $1 AND project_id = $2 AND expires_at > NOW()",
                )
                .bind(raw_request_id)
                .bind(project_id)
                .fetch_optional(db)
                .await?
            }
            _ => None,
        };
    let (request, response) = match bodies {
        Some((request, response)) => (request.map(|body| body.0), response.map(|body| body.0)),
        None => (None, None),
    };

    let mut body = serde_json::to_value(&meta)?;
    body["object"] = json!("usage_request");
    body["request"] = request.unwrap_or(Value::Null);
    body["response"] = response.unwrap_or(Value::Null);
    Ok(body)
}

/// Report rows as CSV, header row first.
pub(crate) fn report_csv(rows: &[ReportRow]) -> String {
    let mut out = String::from(CSV_HEADER);
    out.push('\n');
    for row in rows {
        out.push_str(&format!(
            "{},{},{},{},{:.6}\n",
            csv_field(row.group.as_deref().unwrap_or("")),
            row.requests,
            row.prompt_tokens,
            row.completion_tokens,
            row.cost_usd,
        ));
    }
    out
}

/// Quote a CSV field when needed, and defuse a leading formula character so a
/// spreadsheet shows the text instead of running it.
fn csv_field(raw: &str) -> String {
    let raw = if raw.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{raw}")
    } else {
        raw.to_string()
    };
    if raw.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", raw.replace('"', "\"\""))
    } else {
        raw
    }
}

/// `GET /api/v1/me/usage/report` — grouped usage rows (JSON or CSV).
async fn get_usage_report(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Query(params): Query<ReportParams>,
) -> Result<Response, ApiError> {
    let viewer = usage_viewer(&state, &headers).await?;
    let csv = match non_empty(&params.format).unwrap_or("json") {
        "json" => false,
        "csv" => true,
        _ => return Err(ApiError::BadRequest("format must be json or csv.".to_string())),
    };
    let group_by = GroupBy::parse(params.group_by.as_deref())?;
    let (from, to) = report_range(non_empty(&params.from), non_empty(&params.to), Utc::now())?;
    let filter = UsageFilter {
        scope: resolve_scope(&viewer, params.scope.as_deref())?,
        from: Some(from),
        to: Some(to),
        project_id: non_empty_owned(&params.project_id),
        client: non_empty_owned(&params.client),
        model: non_empty_owned(&params.model),
    };
    check_project(&state.db, &viewer, filter.project_id.as_deref()).await?;
    let rows = usage_report_rows(&state.db, &filter, group_by).await?;

    if csv {
        let filename = format!(
            "allternit-usage-{}-{}-{}.csv",
            group_by.as_str(),
            from.format("%Y-%m-%d"),
            to.format("%Y-%m-%d"),
        );
        return Ok((
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
                (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{filename}\"")),
            ],
            report_csv(&rows),
        )
            .into_response());
    }
    Ok(Json(json!({
        "object": "usage_report",
        "group_by": group_by.as_str(),
        "from": from.to_rfc3339(),
        "to": to.to_rfc3339(),
        "rows": rows,
    }))
    .into_response())
}

/// `GET /api/v1/me/usage/requests` — billed requests, newest first (cursor).
async fn list_usage_requests(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Query(params): Query<RequestsParams>,
) -> Result<Json<Value>, ApiError> {
    let viewer = usage_viewer(&state, &headers).await?;
    let limit = match non_empty(&params.limit) {
        None => DEFAULT_REQUESTS_LIMIT,
        Some(raw) => match raw.parse::<i64>() {
            Ok(limit) if (1..=MAX_REQUESTS_LIMIT).contains(&limit) => limit,
            _ => {
                return Err(ApiError::BadRequest(format!(
                    "limit must be between 1 and {MAX_REQUESTS_LIMIT}."
                )))
            }
        },
    };
    let after = match non_empty(&params.cursor) {
        None => None,
        Some(raw) => Some(
            page::decode_cursor(raw)
                .ok_or_else(|| ApiError::BadRequest("cursor is not a valid cursor.".to_string()))?,
        ),
    };
    let from = non_empty(&params.from).map(|raw| parse_bound(raw, "from")).transpose()?;
    let to = non_empty(&params.to).map(|raw| parse_bound(raw, "to")).transpose()?;
    if let (Some(from), Some(to)) = (from, to) {
        if from >= to {
            return Err(ApiError::BadRequest(
                "from must be before to (to is exclusive).".to_string(),
            ));
        }
    }
    let filter = UsageFilter {
        scope: resolve_scope(&viewer, params.scope.as_deref())?,
        from,
        to,
        project_id: non_empty_owned(&params.project_id),
        client: non_empty_owned(&params.client),
        model: non_empty_owned(&params.model),
    };
    check_project(&state.db, &viewer, filter.project_id.as_deref()).await?;
    let (rows, next_cursor) = usage_request_page(&state.db, &filter, after, limit).await?;
    let has_more = next_cursor.is_some();
    Ok(Json(json!({
        "object": "list",
        "data": rows,
        "has_more": has_more,
        "next_cursor": next_cursor,
    })))
}

/// `GET /api/v1/me/usage/requests/:request_id` — one request's metadata, plus
/// its request and response bodies when its project captured them.
async fn get_usage_request(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Query(params): Query<DetailParams>,
) -> Result<Json<Value>, ApiError> {
    let viewer = usage_viewer(&state, &headers).await?;
    let scope = resolve_scope(&viewer, params.scope.as_deref())?;
    Ok(Json(usage_request_detail(&state.db, scope, request_id.trim()).await?))
}

async fn caller_user_id(state: &ApiState, headers: &HeaderMap) -> Result<String, ApiError> {
    usage_viewer(state, headers).await.map(|viewer| viewer.user_id)
}

pub(crate) fn next_month_start(now: DateTime<Utc>) -> DateTime<Utc> {
    let year = now.year();
    let month = now.month();
    if month == 12 {
        Utc.with_ymd_and_hms(year + 1, 1, 1, 0, 0, 0)
            .single()
            .unwrap_or(now)
    } else {
        Utc.with_ymd_and_hms(year, month + 1, 1, 0, 0, 0)
            .single()
            .unwrap_or(now)
    }
}

fn plan_label(plan_id: &str) -> &'static str {
    match plan_id {
        "plus" => "Plus",
        "super" => "Super",
        "ultra" => "Ultra",
        _ => "Free",
    }
}

pub(crate) fn compose_usage(
    plan_id: &str,
    plan_status: &str,
    plan_tier: &str,
    credit_balance: Option<f64>,
    month_to_date_usage_usd: f64,
    resets_at: DateTime<Utc>,
    recent_transactions: Vec<UsageTransaction>,
    cloud_models: services::CloudModelsEntitlement,
) -> UsageResponse {
    let paid = matches!(plan_id, "plus" | "super" | "ultra")
        && matches!(plan_status, "active" | "trialing");
    let monthly_grant = find_plan(plan_id)
        .map(|plan| plan.monthly_credits_usd)
        .unwrap_or(0.0);
    let (credits, weekly_limit, weekly_used) = if paid || credit_balance.is_some() {
        let balance = credit_balance.unwrap_or(0.0);
        (
            Some(balance),
            if monthly_grant > 0.0 {
                monthly_grant
            } else {
                balance.max(0.0) + month_to_date_usage_usd
            },
            month_to_date_usage_usd,
        )
    } else {
        // No plan and no credits row: nothing to spend on Cloud models.
        (Some(0.0), 0.0, 0.0)
    };
    UsageResponse {
        plan: if paid { plan_id.to_string() } else { "free".to_string() },
        label: if paid {
            plan_label(plan_id).to_string()
        } else {
            "Free".to_string()
        },
        status: if paid {
            plan_status.to_string()
        } else {
            "none".to_string()
        },
        plan_tier: if paid {
            plan_tier.to_string()
        } else {
            "free".to_string()
        },
        weekly_used,
        weekly_limit,
        resets_at: Some(resets_at.to_rfc3339()),
        credits,
        month_to_date_usage_usd,
        recent_transactions,
        cloud_models,
        windows: Vec::new(),
        carry_past_windows: false,
        pace: UsagePace {
            window_id: "month".to_string(),
            burn_per_hour: 0.0,
            runs_out_at: None,
            status: "ok".to_string(),
        },
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct UsageSettings {
    pub carry_past_windows: bool,
}

async fn get_usage_settings(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Json<UsageSettings>, ApiError> {
    let user_id = caller_user_id(&state, &headers).await?;
    Ok(Json(UsageSettings { carry_past_windows: carry_past_windows(&state.db, &user_id).await }))
}

/// Sets the usage credits toggle. It lives on the credits row, so an account with no credits
/// row (never bought credits, no plan grant) gets 409: there is nothing to carry it.
async fn put_usage_settings(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(body): Json<UsageSettings>,
) -> Result<Json<UsageSettings>, ApiError> {
    let user_id = caller_user_id(&state, &headers).await?;
    let updated = sqlx::query("UPDATE user_credits SET carry_past_windows = $2 WHERE user_id = $1")
        .bind(&user_id)
        .bind(body.carry_past_windows)
        .execute(&state.db)
        .await
        .map_err(ApiError::DatabaseError)?
        .rows_affected();
    if updated == 0 {
        return Err(ApiError::Conflict("Add usage credits first.".to_string()));
    }
    Ok(Json(UsageSettings { carry_past_windows: body.carry_past_windows }))
}

async fn get_my_usage(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Json<UsageResponse>, ApiError> {
    let user_id = caller_user_id(&state, &headers).await?;

    let sub: Option<(String, String, String)> = sqlx::query_as(
        r#"
        SELECT plan_id, plan_tier, status
        FROM billing_subscriptions
        WHERE user_id = $1 AND status IN ('active', 'trialing')
        ORDER BY updated_at DESC
        LIMIT 1
        "#,
    )
    .bind(&user_id)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);
    let (plan_id, plan_tier, plan_status) = if crate::auth::is_admin_user(&user_id) {
        (
            "ultra".to_string(),
            "team".to_string(),
            "active".to_string(),
        )
    } else {
        sub.unwrap_or_else(|| {
            (
                "free".to_string(),
                "free".to_string(),
                "none".to_string(),
            )
        })
    };

    let credit_balance = services::credit_balance_row(&state.db, &user_id)
        .await
        .ok()
        .flatten();

    let hosted = services::hosted_usage_summary(&state.db, &user_id)
        .await
        .unwrap_or(services::HostedUsageSummary {
            total_seconds: 0,
            estimated_cost_usd: 0.0,
        });

    let cloud_models = services::cloud_models_entitlement(&state.db, &user_id, credit_balance).await;

    let now = Utc::now();
    let spend = window_spend(&state.db, &user_id, now).await;
    let carry = carry_past_windows(&state.db, &user_id).await;

    let recent = sqlx::query_as::<_, CreditTxnRow>(
        r#"
        SELECT amount_usd, source, created_at
        FROM credit_transactions
        WHERE user_id = $1
        ORDER BY created_at DESC
        LIMIT 20
        "#,
    )
    .bind(&user_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let paid_plan = if matches!(plan_id.as_str(), "plus" | "super" | "ultra")
        && matches!(plan_status.as_str(), "active" | "trialing")
    {
        plan_id.as_str()
    } else {
        "free"
    };
    let limits = window_limits_for(paid_plan);
    let mut usage = compose_usage(
        &plan_id,
        &plan_status,
        &plan_tier,
        credit_balance,
        hosted.estimated_cost_usd,
        next_month_start(Utc::now()),
        recent
            .into_iter()
            .map(|row| UsageTransaction {
                amount_usd: row.amount_usd,
                source: row.source,
                created_at: row.created_at.to_rfc3339(),
            })
            .collect(),
        cloud_models,
    );
    let (windows, pace) = compose_windows(spend, limits, now);
    usage.windows = windows;
    usage.pace = pace;
    usage.carry_past_windows = carry;
    Ok(Json(usage))
}

#[cfg(test)]
#[path = "me_usage_tests.rs"]
mod report_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn entitlement(balance: Option<f64>) -> services::CloudModelsEntitlement {
        let reason = services::inference_settlement::credits_reason(balance);
        services::CloudModelsEntitlement {
            allowed: reason.is_none(),
            reason,
            balance_usd: balance.unwrap_or(0.0).max(0.0),
            byok_providers: Vec::new(),
        }
    }

    #[test]
    fn free_plan_without_credits_has_nothing_to_spend() {
        let usage = compose_usage(
            "free",
            "none",
            "free",
            None,
            0.40,
            Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap(),
            vec![],
            entitlement(None),
        );
        assert_eq!(usage.plan, "free");
        assert_eq!(usage.label, "Free");
        assert_eq!(usage.credits, Some(0.0));
        assert_eq!(usage.weekly_limit, 0.0);
        assert_eq!(usage.weekly_used, 0.0);
        let json = serde_json::to_value(&usage).unwrap();
        assert_eq!(json["cloud_models"]["allowed"], false);
        assert_eq!(json["cloud_models"]["reason"], "credits_required");
    }

    #[test]
    fn plus_plan_reports_credit_balance_and_monthly_grant() {
        let usage = compose_usage(
            "plus",
            "active",
            "pro",
            Some(10.5),
            1.25,
            Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap(),
            vec![],
            entitlement(Some(10.5)),
        );
        assert_eq!(usage.plan, "plus");
        assert_eq!(usage.label, "Plus");
        assert_eq!(usage.credits, Some(10.5));
        assert_eq!(usage.weekly_limit, 22.0);
        assert_eq!(usage.weekly_used, 1.25);
        assert_eq!(usage.plan_tier, "pro");
        assert_eq!(usage.status, "active");
        let json = serde_json::to_value(&usage).unwrap();
        assert_eq!(json["cloud_models"]["allowed"], true);
        assert!(json["cloud_models"]["reason"].is_null());
    }

    #[test]
    fn canceled_plus_is_treated_as_free() {
        let usage = compose_usage(
            "plus",
            "canceled",
            "pro",
            None,
            0.0,
            Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap(),
            vec![],
            entitlement(None),
        );
        assert_eq!(usage.plan, "free");
        assert_eq!(usage.credits, Some(0.0));
        assert_eq!(usage.weekly_limit, 0.0);
    }
}
