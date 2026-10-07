//! Scheduler types

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Schedule status
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum ScheduleStatus {
    Active,
    Paused,
    Error,
}

/// Schedule record from database
#[derive(Debug, Clone, FromRow)]
#[allow(dead_code)]
pub struct Schedule {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub cron_expr: String,
    pub natural_lang: Option<String>,
    pub timezone: String,
    pub job_template: sqlx::types::Json<serde_json::Value>,
    pub enabled: bool,
    pub misfire_policy: String,
    pub last_run_at: Option<DateTime<Utc>>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub run_count: i64,
    pub misfire_count: i64,
    pub owner_id: Option<String>,
    pub tenant_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Schedule trigger record
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ScheduleTrigger {
    /// Schedule ID
    pub schedule_id: String,
    /// Run ID that was created
    pub run_id: String,
    /// When the trigger occurred
    pub triggered_at: DateTime<Utc>,
    /// Whether the trigger was successful
    pub success: bool,
    /// Error message if failed
    pub error: Option<String>,
}

/// Create schedule request
/// Reserved for a future scheduler HTTP CRUD API.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct CreateScheduleRequest {
    pub name: String,
    pub description: Option<String>,
    /// Cron expression or natural language
    pub schedule: String,
    /// Job configuration template
    pub job_template: serde_json::Value,
    pub enabled: Option<bool>,
    pub misfire_policy: Option<String>,
}

/// Schedule response
/// Reserved for a future scheduler HTTP CRUD API.
#[derive(Debug, Clone, Serialize)]
#[allow(dead_code)]
pub struct ScheduleResponse {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub cron_expr: String,
    pub natural_lang: Option<String>,
    pub enabled: bool,
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub run_count: i64,
}

#[allow(dead_code)]
impl From<Schedule> for ScheduleResponse {
    fn from(s: Schedule) -> Self {
        Self {
            id: s.id,
            name: s.name,
            description: s.description,
            cron_expr: s.cron_expr,
            natural_lang: s.natural_lang,
            enabled: s.enabled,
            next_run_at: s.next_run_at,
            last_run_at: s.last_run_at,
            run_count: s.run_count,
        }
    }
}

/// Calculate next run time from a cron expression
#[allow(dead_code)]
pub fn calculate_next_run(cron_expr: &str, after: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    // Use the cron parser to get next occurrence
    allternit_cron_parser::next_occurrence(cron_expr, after)
}

/// Parse natural language schedule
#[allow(dead_code)]
pub fn parse_schedule(input: &str) -> Result<(String, String), String> {
    let parsed = allternit_cron_parser::parse(input)
        .map_err(|e| format!("Failed to parse schedule: {}", e))?;

    Ok((parsed.expression, parsed.description))
}
