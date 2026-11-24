//! Computers console alerts (surfaces/docs/dev/computers-console-contract.mdx,
//! "Alert push", D4, step 2.8).
//!
//! * Rules (`computer_alert_rules`, migrations 154 + 157), owner-only:
//!   - `GET /api/v1/computers/alert-rules` -> `{ rules }`
//!   - `POST /api/v1/computers/alert-rules` `{ rule, threshold?, computerId?, durationMinutes?, enabled? }`
//!   - `PATCH /api/v1/computers/alert-rules/:id` `{ threshold?, durationMinutes?, enabled? }`
//!   - `DELETE /api/v1/computers/alert-rules/:id`
//!   - `GET /api/v1/computers/alerts?computerId=&limit=` -> `{ alerts }`, the
//!     incidents the Activity tab lists, newest first.
//!
//!   `rule` is `offline` (threshold = minutes offline, 1-1440, default 10),
//!   `cpu` (threshold = percent, 1-100, default 90, over for `durationMinutes`,
//!   1-120, default 5) or `disk` (threshold = percent of the fullest mount,
//!   1-100, default 90). `computerId` is a console row id of one of the
//!   caller's own computers; left out, the rule covers all of them. One rule
//!   per type per scope. Every error body is `{ error, message }`.
//! * Evaluation ([`evaluate_all`], every minute from [`start_alert_evaluator`]):
//!   for each enabled rule and each computer it covers, read the console row
//!   (routes::computers_console) and the 60 s samples
//!   (routes::computer_metrics). A breach is recorded in
//!   `computer_alert_state`; once it has lasted long enough the alert fires
//!   exactly once (an atomic `UPDATE ... WHERE fired_at IS NULL`, safe on
//!   several replicas) and stays quiet until the computer is back under the
//!   threshold, which writes a `resolved` event and clears the incident. A
//!   computer that is asleep, stopped or starting is not offline, and one with
//!   no fresh sample is left as it was.
//! * Delivery ([`deliver`]): one `computer_alert` message through the push
//!   paths the app already has: Web Push (browsers, the Desktop app, and from
//!   there APNs + FCM) when VAPID is set, else APNs + FCM directly. It honours
//!   a `computer.alert` switch in `notification_preferences` (default on) and
//!   quiet hours (held, then summarised). A `computer.alert` event also goes on
//!   the user event backbone so the feed the Desktop reads carries it.
//!
//! Push data (what iOS `COMPUTER_ALERT` and Android decode):
//! `{ type: "computer_alert", computerId, rule, value, category: "COMPUTER_ALERT", event: "computer.alert", level: "needs_you" }`;
//! `value` is percent for cpu/disk and minutes for offline.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, patch},
    Json, Router,
};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::sync::Arc;

use crate::routes::apns;
use crate::routes::computer_metrics::{sources_for, Sources};
use crate::routes::computers_console::{load_console, ConsoleComputer};
use crate::routes::notifications::{self, Level};
use crate::routes::paired_computers::{signed_in_caller, Caller};
use crate::routes::platform_v1::new_id;
use crate::routes::web_push::{self, PushMessage, Vapid};
use crate::{ApiError, ApiState};

/// The `notification_preferences` switch and the backbone event name.
pub const EVENT: &str = "computer.alert";
const EVALUATE_EVERY: std::time::Duration = std::time::Duration::from_secs(60);
const MAX_RULES: i64 = 50;
const DEFAULT_OFFLINE_MINUTES: f64 = 10.0;
const DEFAULT_PERCENT: f64 = 90.0;
const DEFAULT_CPU_MINUTES: i32 = 5;
/// A sample older than this is not "now" for cpu and disk.
const FRESH_SAMPLE_MINUTES: i64 = 5;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/computers/alert-rules", get(list_rules).post(create_rule))
        .route("/api/v1/computers/alert-rules/:id", patch(update_rule).delete(delete_rule))
        .route("/api/v1/computers/alerts", get(list_alerts))
}

fn fail(status: StatusCode, error: &str, message: &str) -> Response {
    (status, Json(json!({ "error": error, "message": message }))).into_response()
}

fn db_fail(e: sqlx::Error) -> Response {
    tracing::warn!("computer alerts: {e}");
    fail(StatusCode::INTERNAL_SERVER_ERROR, "db_error", "Something went wrong saving that. Try again in a moment.")
}

// ------------------------------------------------------------ the rules ----

pub const RULE_TYPES: [&str; 3] = ["offline", "cpu", "disk"];

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct RuleRow {
    pub id: String,
    pub user_id: String,
    pub computer_id: Option<String>,
    pub rule_type: String,
    pub threshold: Option<f64>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub duration_minutes: i32,
}

impl RuleRow {
    fn json(&self) -> Value {
        json!({
            "id": self.id,
            "computerId": self.computer_id,
            "rule": self.rule_type,
            "threshold": self.threshold,
            "durationMinutes": self.duration_minutes,
            "enabled": self.enabled,
            "createdAt": self.created_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        })
    }
}

const RULE_COLS: &str = "id, user_id, computer_id, rule_type, threshold, enabled, created_at, duration_minutes";

/// A checked threshold for `rule`, or the plain sentence to show.
pub fn check_threshold(rule: &str, threshold: Option<f64>) -> Result<f64, &'static str> {
    let default = if rule == "offline" { DEFAULT_OFFLINE_MINUTES } else { DEFAULT_PERCENT };
    let t = threshold.unwrap_or(default);
    if !t.is_finite() {
        return Err("The threshold must be a number.");
    }
    match rule {
        "offline" if !(1.0..=1440.0).contains(&t) => Err("Offline alerts need 1 to 1440 minutes."),
        "cpu" | "disk" if !(1.0..=100.0).contains(&t) => Err("Pick a percent from 1 to 100."),
        _ => Ok(t),
    }
}

pub fn check_duration(minutes: Option<i64>) -> Result<i32, &'static str> {
    match minutes {
        None => Ok(DEFAULT_CPU_MINUTES),
        Some(m) if (1..=120).contains(&m) => Ok(m as i32),
        Some(_) => Err("CPU must stay high for 1 to 120 minutes."),
    }
}

async fn list_rules(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let rows: Vec<RuleRow> = match sqlx::query_as(&format!("SELECT {RULE_COLS} FROM computer_alert_rules WHERE user_id = $1 ORDER BY created_at, id"))
        .bind(&caller.user_id)
        .fetch_all(&state.db)
        .await
    {
        Ok(r) => r,
        Err(e) => return Ok(db_fail(e)),
    };
    Ok(Json(json!({ "rules": rows.iter().map(RuleRow::json).collect::<Vec<_>>() })).into_response())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateRule {
    rule: String,
    threshold: Option<f64>,
    computer_id: Option<String>,
    duration_minutes: Option<i64>,
    enabled: Option<bool>,
}

async fn create_rule(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<CreateRule>) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    if !RULE_TYPES.contains(&body.rule.as_str()) {
        return Ok(fail(StatusCode::BAD_REQUEST, "invalid_rule", "The rule must be offline, cpu or disk."));
    }
    let threshold = match check_threshold(&body.rule, body.threshold) {
        Ok(t) => t,
        Err(m) => return Ok(fail(StatusCode::BAD_REQUEST, "invalid_threshold", m)),
    };
    let duration = match check_duration(body.duration_minutes) {
        Ok(d) => d,
        Err(m) => return Ok(fail(StatusCode::BAD_REQUEST, "invalid_duration", m)),
    };
    let computer_id = body.computer_id.map(|c| c.trim().to_string()).filter(|c| !c.is_empty());
    if let Some(id) = &computer_id {
        match own_computer(&state.db, &caller.user_id, id).await {
            Ok(true) => {}
            Ok(false) => return Ok(fail(StatusCode::NOT_FOUND, "not_found", "That computer was not found.")),
            Err(e) => return Ok(db_fail(e)),
        }
    }
    let count: i64 = match sqlx::query_scalar("SELECT count(*) FROM computer_alert_rules WHERE user_id = $1").bind(&caller.user_id).fetch_one(&state.db).await {
        Ok(n) => n,
        Err(e) => return Ok(db_fail(e)),
    };
    if count >= MAX_RULES {
        return Ok(fail(StatusCode::CONFLICT, "too_many_rules", "You have reached the limit of 50 alert rules. Delete one first."));
    }
    let id = new_id("car_");
    let inserted = sqlx::query(
        "INSERT INTO computer_alert_rules (id, user_id, computer_id, rule_type, threshold, enabled, duration_minutes)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT DO NOTHING",
    )
    .bind(&id)
    .bind(&caller.user_id)
    .bind(&computer_id)
    .bind(&body.rule)
    .bind(threshold)
    .bind(body.enabled.unwrap_or(true))
    .bind(duration)
    .execute(&state.db)
    .await;
    match inserted {
        Ok(r) if r.rows_affected() == 0 => {
            return Ok(fail(StatusCode::CONFLICT, "rule_exists", "You already have this alert for that scope. Change it instead."))
        }
        Ok(_) => {}
        Err(e) => return Ok(db_fail(e)),
    }
    match fetch_rule(&state.db, &caller.user_id, &id).await {
        Ok(Some(row)) => Ok((StatusCode::CREATED, Json(json!({ "rule": row.json() }))).into_response()),
        Ok(None) => Ok(fail(StatusCode::INTERNAL_SERVER_ERROR, "db_error", "Something went wrong saving that. Try again in a moment.")),
        Err(e) => Ok(db_fail(e)),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateRule {
    threshold: Option<f64>,
    duration_minutes: Option<i64>,
    enabled: Option<bool>,
}

async fn update_rule(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<String>, Json(body): Json<UpdateRule>) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let current = match fetch_rule(&state.db, &caller.user_id, &id).await {
        Ok(Some(r)) => r,
        Ok(None) => return Ok(fail(StatusCode::NOT_FOUND, "not_found", "That alert rule was not found.")),
        Err(e) => return Ok(db_fail(e)),
    };
    let threshold = match body.threshold {
        Some(t) => match check_threshold(&current.rule_type, Some(t)) {
            Ok(t) => Some(t),
            Err(m) => return Ok(fail(StatusCode::BAD_REQUEST, "invalid_threshold", m)),
        },
        None => current.threshold,
    };
    let duration = match body.duration_minutes {
        Some(_) => match check_duration(body.duration_minutes) {
            Ok(d) => d,
            Err(m) => return Ok(fail(StatusCode::BAD_REQUEST, "invalid_duration", m)),
        },
        None => current.duration_minutes,
    };
    let enabled = body.enabled.unwrap_or(current.enabled);
    let saved = sqlx::query("UPDATE computer_alert_rules SET threshold = $3, duration_minutes = $4, enabled = $5, updated_at = now() WHERE id = $1 AND user_id = $2")
        .bind(&id)
        .bind(&caller.user_id)
        .bind(threshold)
        .bind(duration)
        .bind(enabled)
        .execute(&state.db)
        .await;
    if let Err(e) = saved {
        return Ok(db_fail(e));
    }
    // A changed or switched-off rule starts its incidents over.
    if body.threshold.is_some() || body.duration_minutes.is_some() || !enabled {
        let _ = sqlx::query("DELETE FROM computer_alert_state WHERE rule_id = $1").bind(&id).execute(&state.db).await;
    }
    match fetch_rule(&state.db, &caller.user_id, &id).await {
        Ok(Some(row)) => Ok(Json(json!({ "rule": row.json() })).into_response()),
        Ok(None) => Ok(fail(StatusCode::NOT_FOUND, "not_found", "That alert rule was not found.")),
        Err(e) => Ok(db_fail(e)),
    }
}

async fn delete_rule(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<String>) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let gone = sqlx::query("DELETE FROM computer_alert_rules WHERE id = $1 AND user_id = $2").bind(&id).bind(&caller.user_id).execute(&state.db).await;
    match gone {
        Ok(r) if r.rows_affected() == 0 => Ok(fail(StatusCode::NOT_FOUND, "not_found", "That alert rule was not found.")),
        Ok(_) => {
            let _ = sqlx::query("DELETE FROM computer_alert_state WHERE rule_id = $1").bind(&id).execute(&state.db).await;
            Ok(Json(json!({ "ok": true })).into_response())
        }
        Err(e) => Ok(db_fail(e)),
    }
}

async fn fetch_rule(db: &PgPool, user: &str, id: &str) -> Result<Option<RuleRow>, sqlx::Error> {
    sqlx::query_as(&format!("SELECT {RULE_COLS} FROM computer_alert_rules WHERE id = $1 AND user_id = $2")).bind(id).bind(user).fetch_optional(db).await
}

/// Whether console id `id` is one of `user`'s own computers (not one shared
/// through an organization: rules are the owner's).
async fn own_computer(db: &PgPool, user: &str, id: &str) -> Result<bool, sqlx::Error> {
    Ok(load_console(db, user, None).await?.iter().any(|c| c.id == id))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AlertsQuery {
    computer_id: Option<String>,
    limit: Option<i64>,
}

async fn list_alerts(State(state): State<Arc<ApiState>>, headers: HeaderMap, Query(q): Query<AlertsQuery>) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    type Row = (String, String, String, String, String, Option<f64>, String, DateTime<Utc>);
    let rows: Result<Vec<Row>, sqlx::Error> = sqlx::query_as(
        "SELECT id, computer_id, computer_name, rule_type, state, value, message, created_at
         FROM computer_alert_events
         WHERE user_id = $1 AND ($2::text IS NULL OR computer_id = $2)
         ORDER BY created_at DESC, id DESC LIMIT $3",
    )
    .bind(&caller.user_id)
    .bind(q.computer_id.as_deref().filter(|c| !c.is_empty()))
    .bind(limit)
    .fetch_all(&state.db)
    .await;
    let rows = match rows {
        Ok(r) => r,
        Err(e) => return Ok(db_fail(e)),
    };
    let alerts: Vec<Value> = rows
        .into_iter()
        .map(|(id, computer_id, computer_name, rule, state, value, message, at)| {
            json!({
                "id": id, "computerId": computer_id, "computerName": computer_name, "rule": rule,
                "state": state, "value": value, "message": message,
                "at": at.to_rfc3339_opts(SecondsFormat::Secs, true),
            })
        })
        .collect();
    Ok(Json(json!({ "alerts": alerts })).into_response())
}

// ------------------------------------------------------------ evaluation ----

/// What one rule sees on one computer right now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict {
    /// Over the line; the number is the cpu/disk percent (offline: 0, the
    /// minutes come from how long the breach has lasted).
    Breach(f64),
    /// Fine.
    Clear,
    /// Cannot tell (no fresh data): leave things as they are.
    Unknown,
}

/// The offline rule: only a computer that should be reachable and is not.
pub fn offline_verdict(status: &str) -> Verdict {
    match status {
        "offline" => Verdict::Breach(0.0),
        "online" | "asleep" | "stopped" | "suspended" | "starting" => Verdict::Clear,
        _ => Verdict::Unknown,
    }
}

/// The fullest mount of a `HostSnapshot`, percent used.
pub fn disk_percent(sample: &Value) -> Option<f64> {
    sample
        .get("disk")?
        .as_array()?
        .iter()
        .filter_map(|d| {
            let used = d.get("usedBytes")?.as_f64()?;
            let total = d.get("totalBytes")?.as_f64()?;
            (total > 0.0).then(|| used / total * 100.0)
        })
        .fold(None, |best: Option<f64>, p| Some(best.map_or(p, |b| b.max(p))))
}

/// The disk rule from the console row's newest sample.
pub fn disk_verdict(c: &ConsoleComputer, threshold: f64) -> Verdict {
    if c.status != "online" {
        return Verdict::Unknown;
    }
    match c.last_sample.as_ref().and_then(disk_percent) {
        Some(p) if p > threshold => Verdict::Breach(p),
        Some(_) => Verdict::Clear,
        None => Verdict::Unknown,
    }
}

/// What the raw samples of the last `duration` minutes say about CPU.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CpuWindow {
    pub count: i64,
    pub min: Option<f64>,
    pub avg: Option<f64>,
    pub oldest: Option<DateTime<Utc>>,
    pub newest: Option<DateTime<Utc>>,
}

/// CPU over the threshold for the whole window: every sample over, the newest
/// fresh, and the samples reaching back to (nearly) the start of the window.
pub fn cpu_verdict(w: &CpuWindow, threshold: f64, duration_minutes: i32, now: DateTime<Utc>) -> Verdict {
    let (Some(min), Some(avg), Some(oldest), Some(newest)) = (w.min, w.avg, w.oldest, w.newest) else { return Verdict::Unknown };
    if w.count == 0 || newest < now - Duration::minutes(FRESH_SAMPLE_MINUTES) {
        return Verdict::Unknown;
    }
    if min <= threshold {
        return Verdict::Clear;
    }
    // Over the line; whether it has been long enough is `due`'s call.
    let _ = (oldest, duration_minutes);
    Verdict::Breach(avg)
}

/// Whether a breach that began at `since` has lasted long enough to fire.
pub fn due(rule_type: &str, threshold: f64, duration_minutes: i32, w: Option<&CpuWindow>, since: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    match rule_type {
        "offline" => now - since >= Duration::minutes(threshold.max(1.0) as i64),
        "cpu" => w.is_some_and(|w| w.oldest.is_some_and(|o| o <= now - Duration::minutes((i64::from(duration_minutes) - 2).max(0)))),
        _ => true,
    }
}

/// The state of one (rule, computer) before this pass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Incident {
    pub since: DateTime<Utc>,
    pub fired: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Nothing,
    /// Breach seen (record it); fire when `fire`.
    Breach { fire: bool },
    /// Back under the line: close the incident (`announce` when it had fired).
    Clear { announce: bool },
}

/// The one decision per (rule, computer) per pass. Fires at most once per
/// incident: a fired incident never fires again until it has cleared.
pub fn decide(incident: Option<Incident>, verdict: Verdict, due_now: bool) -> Action {
    match (verdict, incident) {
        (Verdict::Unknown, _) => Action::Nothing,
        (Verdict::Clear, None) => Action::Nothing,
        (Verdict::Clear, Some(i)) => Action::Clear { announce: i.fired },
        (Verdict::Breach(_), Some(i)) if i.fired => Action::Nothing,
        (Verdict::Breach(_), _) => Action::Breach { fire: due_now },
    }
}

fn plural(n: i64, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// The plain sentences of an alert: `(title, body)`.
pub fn copy(rule: &str, name: &str, value: f64, threshold: f64, duration_minutes: i32, resolved: bool) -> (String, String) {
    let v = value.round() as i64;
    if resolved {
        let body = match rule {
            "offline" => "It is back online.".to_string(),
            "cpu" => format!("CPU is back under {}%.", threshold.round() as i64),
            _ => format!("Disk is back under {}% full.", threshold.round() as i64),
        };
        return (format!("{name} is back to normal"), body);
    }
    match rule {
        "offline" => (format!("{name} is offline"), format!("It has been offline for {}.", plural(v, "minute"))),
        "cpu" => (format!("{name} is busy"), format!("CPU has stayed over {}% for {} (now {v}%).", threshold.round() as i64, plural(i64::from(duration_minutes), "minute"))),
        _ => (format!("{name} is running out of space"), format!("Disk is {v}% full.")),
    }
}

/// The push for a fired alert. The iOS app reads `type`, `computerId` from
/// `data` and the `COMPUTER_ALERT` category; Android reads the same keys from
/// the FCM data (routes::fcm::message adds `computerId`, `rule`, `value`).
pub fn push_message(computer_id: &str, name: &str, rule: &str, value: f64, threshold: f64, duration_minutes: i32) -> PushMessage {
    let (title, body) = copy(rule, name, value, threshold, duration_minutes, false);
    PushMessage {
        kind: "computer_alert",
        title,
        body,
        url: format!("/computers/{computer_id}"),
        tag: format!("computer-alert-{computer_id}-{rule}"),
        data: json!({
            "type": "computer_alert",
            "computerId": computer_id,
            "rule": rule,
            "value": value.round(),
            "category": "COMPUTER_ALERT",
            "event": EVENT,
            "level": "needs_you",
        }),
        ttl_secs: 3600,
        high_urgency: true,
    }
}

/// Send `msg` to `user`: honours the `computer.alert` switch and quiet hours.
/// A no-op (and free) where no push service is configured.
pub async fn deliver(db: &PgPool, user: &str, msg: &PushMessage) {
    if notifications::preference(db, user, EVENT).await == Some(false) {
        return;
    }
    if notifications::hold_if_quiet(db, user, EVENT, msg, Level::NeedsYou, None, None).await {
        return;
    }
    match Vapid::from_env() {
        Some(v) => {
            web_push::send_to_user(db, web_push::default_transport().as_ref(), &v, user, msg).await;
        }
        None => {
            apns::send_to_user(db, user, msg).await;
            crate::routes::fcm::send_to_user(db, user, msg).await;
        }
    }
}

async fn cpu_window(db: &PgPool, sources: &Sources, duration_minutes: i32, now: DateTime<Utc>) -> Result<CpuWindow, sqlx::Error> {
    if sources.devices.is_empty() && sources.paired.is_empty() {
        return Ok(CpuWindow { count: 0, min: None, avg: None, oldest: None, newest: None });
    }
    let (count, min, avg, oldest, newest): (i64, Option<f64>, Option<f64>, Option<DateTime<Utc>>, Option<DateTime<Utc>>) = sqlx::query_as(
        "SELECT count(*), min(cpu_percent), avg(cpu_percent), min(ts), max(ts)
         FROM computer_metric_samples
         WHERE resolution_secs = 60 AND cpu_percent IS NOT NULL AND ts >= $3
           AND ((computer_kind = 'device' AND computer_id = ANY($1))
             OR (computer_kind = 'paired' AND computer_id = ANY($2)))",
    )
    .bind(&sources.devices)
    .bind(&sources.paired)
    .bind(now - Duration::minutes(i64::from(duration_minutes)))
    .fetch_one(db)
    .await?;
    Ok(CpuWindow { count, min, avg, oldest, newest })
}

/// What one pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PassCounts {
    pub fired: u64,
    pub resolved: u64,
}

/// Evaluate every enabled rule of every owner once.
pub async fn evaluate_all(db: &PgPool, now: DateTime<Utc>) -> Result<PassCounts, sqlx::Error> {
    let rules: Vec<RuleRow> = sqlx::query_as(&format!("SELECT {RULE_COLS} FROM computer_alert_rules WHERE enabled ORDER BY user_id, created_at")).fetch_all(db).await?;
    let mut counts = PassCounts::default();
    let mut i = 0;
    while i < rules.len() {
        let user = rules[i].user_id.clone();
        let end = rules[i..].iter().position(|r| r.user_id != user).map_or(rules.len(), |p| i + p);
        match evaluate_user(db, &user, &rules[i..end], now).await {
            Ok(c) => {
                counts.fired += c.fired;
                counts.resolved += c.resolved;
            }
            Err(e) => tracing::warn!(user = %user, "computer alerts: {e}"),
        }
        i = end;
    }
    Ok(counts)
}

pub async fn evaluate_user(db: &PgPool, user: &str, rules: &[RuleRow], now: DateTime<Utc>) -> Result<PassCounts, sqlx::Error> {
    let computers = load_console(db, user, None).await?;
    let caller = Caller { user_id: user.to_string(), organization_id: None };
    let mut counts = PassCounts::default();
    for rule in rules {
        let threshold = rule.threshold.unwrap_or(if rule.rule_type == "offline" { DEFAULT_OFFLINE_MINUTES } else { DEFAULT_PERCENT });
        for c in computers.iter().filter(|c| rule.computer_id.as_deref().map_or(true, |id| id == c.id)) {
            let mut window = None;
            let verdict = match rule.rule_type.as_str() {
                "offline" => offline_verdict(&c.status),
                "disk" => disk_verdict(c, threshold),
                _ if c.status != "online" => Verdict::Unknown,
                _ => match sources_for(db, &caller, &c.id).await? {
                    Some(sources) => {
                        let w = cpu_window(db, &sources, rule.duration_minutes, now).await?;
                        let v = cpu_verdict(&w, threshold, rule.duration_minutes, now);
                        window = Some(w);
                        v
                    }
                    None => Verdict::Unknown,
                },
            };
            let state: Option<(DateTime<Utc>, Option<DateTime<Utc>>)> =
                sqlx::query_as("SELECT breach_since, fired_at FROM computer_alert_state WHERE rule_id = $1 AND computer_id = $2").bind(&rule.id).bind(&c.id).fetch_optional(db).await?;
            let incident = state.map(|(since, fired)| Incident { since, fired: fired.is_some() });
            let since = incident.map_or(now, |i| i.since);
            let due_now = due(&rule.rule_type, threshold, rule.duration_minutes, window.as_ref(), since, now);
            match decide(incident, verdict, due_now) {
                Action::Nothing => {}
                Action::Breach { fire } => {
                    sqlx::query("INSERT INTO computer_alert_state (rule_id, computer_id, breach_since) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING").bind(&rule.id).bind(&c.id).bind(now).execute(db).await?;
                    if !fire {
                        continue;
                    }
                    let value = match (&rule.rule_type[..], verdict) {
                        ("offline", _) => ((now - since).num_minutes().max(threshold as i64)) as f64,
                        (_, Verdict::Breach(v)) => v,
                        _ => 0.0,
                    };
                    let event_id = new_id("cae_");
                    // The claim: only one pass (one replica) gets the row.
                    let claimed = sqlx::query("UPDATE computer_alert_state SET fired_at = $3, event_id = $4 WHERE rule_id = $1 AND computer_id = $2 AND fired_at IS NULL")
                        .bind(&rule.id)
                        .bind(&c.id)
                        .bind(now)
                        .bind(&event_id)
                        .execute(db)
                        .await?
                        .rows_affected();
                    if claimed == 0 {
                        continue;
                    }
                    let msg = push_message(&c.id, &c.name, &rule.rule_type, value, threshold, rule.duration_minutes);
                    record_event(db, &event_id, user, rule, c, "fired", Some(value), &format!("{} {}", msg.title, msg.body), now).await?;
                    counts.fired += 1;
                    backbone(db, user, &event_id, &msg, now).await;
                    deliver(db, user, &msg).await;
                }
                Action::Clear { announce } => {
                    let closed = sqlx::query("DELETE FROM computer_alert_state WHERE rule_id = $1 AND computer_id = $2").bind(&rule.id).bind(&c.id).execute(db).await?.rows_affected();
                    if announce && closed > 0 {
                        let (title, body) = copy(&rule.rule_type, &c.name, 0.0, threshold, rule.duration_minutes, true);
                        record_event(db, &new_id("cae_"), user, rule, c, "resolved", None, &format!("{title} {body}"), now).await?;
                        counts.resolved += 1;
                    }
                }
            }
        }
    }
    Ok(counts)
}

#[allow(clippy::too_many_arguments)]
async fn record_event(db: &PgPool, id: &str, user: &str, rule: &RuleRow, c: &ConsoleComputer, state: &str, value: Option<f64>, message: &str, now: DateTime<Utc>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO computer_alert_events (id, user_id, rule_id, computer_id, computer_name, rule_type, state, value, message, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(id)
    .bind(user)
    .bind(&rule.id)
    .bind(&c.id)
    .bind(&c.name)
    .bind(&rule.rule_type)
    .bind(state)
    .bind(value)
    .bind(message)
    .bind(now)
    .execute(db)
    .await?;
    Ok(())
}

/// Put the alert on the user event backbone (the feed the Desktop reads).
/// Idempotent on the event id; a failure never blocks the push.
async fn backbone(db: &PgPool, user: &str, event_id: &str, msg: &PushMessage, now: DateTime<Utc>) {
    let mut data = msg.data.clone();
    data["title"] = json!(msg.title);
    data["summary"] = json!(msg.body);
    data["actionUrl"] = json!(msg.url);
    if let Err(e) = crate::routes::allternit_events::emit_user_event(db, user, EVENT, &data, Some(now), "computer-alert", Some(event_id)).await {
        tracing::warn!("computer alert event: {e}");
    }
}

pub fn start_alert_evaluator(state: Arc<ApiState>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(EVALUATE_EVERY).await;
            if let Err(e) = evaluate_all(&state.db, Utc::now()).await {
                tracing::warn!("computer alerts evaluation: {e}");
            }
        }
    });
}

#[cfg(test)]
#[path = "computer_alerts_tests.rs"]
mod tests;
