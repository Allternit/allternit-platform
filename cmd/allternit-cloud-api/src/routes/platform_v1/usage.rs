//! `GET /v1/usage?group_by=meter|key|account&from&to` (scope `usage`).
//!
//! Rows are `(group, meter, unit)` sums over `platform_usage_events` in
//! `[from, to)`; `group` is the meter, key id or account id per `group_by`
//! (a null `group` under `account` is project-level usage with no account).
//! A key bound to an account only ever sees that account's usage.

use axum::{extract::State, routing::get, Json};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::sync::Arc;

use super::{ApiQuery, PlatformCaller, PlatformError, RouteTable};
use crate::ApiState;

pub fn register(table: RouteTable) -> RouteTable {
    table.add("/v1/usage", &["GET"], get(get_usage))
}

#[derive(Debug, Deserialize)]
struct UsageQuery {
    group_by: Option<String>,
    from: Option<String>,
    to: Option<String>,
    account_id: Option<String>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct UsageRow {
    pub group: Option<String>,
    pub meter: String,
    pub unit: Option<String>,
    pub quantity: f64,
    pub events: i64,
}

#[derive(Debug, Serialize)]
struct UsageResponse {
    object: &'static str,
    group_by: String,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    data: Vec<UsageRow>,
    has_more: bool,
    next_cursor: Option<String>,
}

fn parse_time(raw: &str, param: &str) -> Result<DateTime<Utc>, PlatformError> {
    if let Ok(t) = DateTime::parse_from_rfc3339(raw) {
        return Ok(t.with_timezone(&Utc));
    }
    if let Ok(d) = NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        if let Some(t) = d.and_hms_opt(0, 0, 0) {
            return Ok(t.and_utc());
        }
    }
    Err(PlatformError::invalid_request(
        "invalid_time",
        "Use an RFC 3339 timestamp or a YYYY-MM-DD date.",
    )
    .with_param(param))
}

async fn get_usage(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    ApiQuery(query): ApiQuery<UsageQuery>,
) -> Result<Json<UsageResponse>, PlatformError> {
    caller.require("usage")?;
    let group_by = query.group_by.as_deref().unwrap_or("meter");
    let group_expr = match group_by {
        "meter" => "meter",
        "key" => "key_id",
        "account" => "account_id",
        _ => {
            return Err(PlatformError::invalid_request(
                "invalid_group_by",
                "group_by must be one of meter, key, account.",
            )
            .with_param("group_by"))
        }
    };
    let to = query.to.as_deref().map(|v| parse_time(v, "to")).transpose()?.unwrap_or_else(Utc::now);
    let from = query
        .from
        .as_deref()
        .map(|v| parse_time(v, "from"))
        .transpose()?
        .unwrap_or_else(|| to - Duration::days(30));
    if from >= to {
        return Err(PlatformError::invalid_request("invalid_range", "from must be before to.")
            .with_param("from"));
    }
    let account = caller.account_filter(query.account_id.as_deref())?;

    let rows = sqlx::query_as::<_, UsageRow>(&format!(
        r#"
        SELECT {group_expr} AS "group", meter, unit,
               COALESCE(SUM(quantity), 0)::float8 AS quantity, COUNT(*)::bigint AS events
        FROM platform_usage_events
        WHERE project_id = $1 AND created_at >= $2 AND created_at < $3
          AND ($4::text IS NULL OR account_id = $4)
        GROUP BY {group_expr}, meter, unit
        ORDER BY {group_expr} NULLS FIRST, meter, unit NULLS FIRST
        "#
    ))
    .bind(&caller.project_id)
    .bind(from)
    .bind(to)
    .bind(&account)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(UsageResponse {
        object: "usage",
        group_by: group_by.to_string(),
        from,
        to,
        data: rows,
        has_more: false,
        next_cursor: None,
    }))
}
