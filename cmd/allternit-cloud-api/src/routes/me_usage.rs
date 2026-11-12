//! GET /api/v1/me/usage — Allternit subscription + remaining compute.
//!
//! Desktop forwards the paired runtime device token through local
//! `allternit-api` to this route. Clerk sessions and `allternit_*` API tokens
//! also work. The payload is the compute meter the shell should show: plan
//! (Free / Plus / Max / Ultra) and remaining credits or free-inference
//! allowance. Local Ollama is not this signal.
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

fn free_inference_monthly_usd() -> f64 {
    std::env::var("FREE_INFERENCE_MONTHLY_USD")
        .ok()
        .and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|value| *value >= 0.0)
        .unwrap_or(2.0)
}

pub(crate) fn compose_usage(
    plan_id: &str,
    plan_status: &str,
    plan_tier: &str,
    credit_balance: Option<f64>,
    month_to_date_usage_usd: f64,
    free_used_usd: f64,
    free_allowance_usd: f64,
    resets_at: DateTime<Utc>,
    recent_transactions: Vec<UsageTransaction>,
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
        let remaining = (free_allowance_usd - free_used_usd).max(0.0);
        (Some(remaining), free_allowance_usd, free_used_usd)
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
    }
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

    let free_used: f64 = sqlx::query_scalar(
        r#"
        SELECT COALESCE(SUM(cost_usd), 0)
        FROM inference_usage
        WHERE user_id = $1 AND created_at >= date_trunc('month', NOW())
        "#,
    )
    .bind(&user_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0.0);

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

    Ok(Json(compose_usage(
        &plan_id,
        &plan_status,
        &plan_tier,
        credit_balance,
        hosted.estimated_cost_usd,
        free_used,
        free_inference_monthly_usd(),
        next_month_start(Utc::now()),
        recent
            .into_iter()
            .map(|row| UsageTransaction {
                amount_usd: row.amount_usd,
                source: row.source,
                created_at: row.created_at.to_rfc3339(),
            })
            .collect(),
    )))
}

#[cfg(test)]
#[path = "me_usage_tests.rs"]
mod report_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_plan_reports_remaining_inference_allowance() {
        let usage = compose_usage(
            "free",
            "none",
            "free",
            None,
            0.0,
            0.40,
            2.0,
            Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap(),
            vec![],
        );
        assert_eq!(usage.plan, "free");
        assert_eq!(usage.label, "Free");
        assert_eq!(usage.credits, Some(1.6));
        assert_eq!(usage.weekly_limit, 2.0);
        assert_eq!(usage.weekly_used, 0.40);
    }

    #[test]
    fn plus_plan_reports_credit_balance_and_monthly_grant() {
        let usage = compose_usage(
            "plus",
            "active",
            "pro",
            Some(10.5),
            1.25,
            0.0,
            2.0,
            Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap(),
            vec![],
        );
        assert_eq!(usage.plan, "plus");
        assert_eq!(usage.label, "Plus");
        assert_eq!(usage.credits, Some(10.5));
        assert_eq!(usage.weekly_limit, 22.0);
        assert_eq!(usage.weekly_used, 1.25);
        assert_eq!(usage.plan_tier, "pro");
        assert_eq!(usage.status, "active");
    }

    #[test]
    fn canceled_plus_is_treated_as_free() {
        let usage = compose_usage(
            "plus",
            "canceled",
            "pro",
            None,
            0.0,
            2.0,
            2.0,
            Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap(),
            vec![],
        );
        assert_eq!(usage.plan, "free");
        assert_eq!(usage.credits, Some(0.0));
        assert_eq!(usage.weekly_limit, 2.0);
    }
}
