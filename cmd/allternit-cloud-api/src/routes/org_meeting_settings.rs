//! Meetings settings and usage (docs/meetings/CONTRACT.md section 4):
//!
//! - `GET|PUT /api/v1/me/meeting-settings`: the user's auto-join rule, recap
//!   email and default reply mode. GET works on the free plan (the settings
//!   page shows the upgrade prompt with the current values); PUT needs a
//!   paid plan.
//! - `GET|PUT /api/v1/orgs/:org/meeting-settings`: org controls. GET for a
//!   member of the org (the caller's active org), PUT for an org admin.
//!   Defaults are returned when the org has no row.
//! - `GET /api/v1/me/meeting-usage`: bot minutes this UTC month against the
//!   plan's included minutes and concurrency cap.
//!
//! Included minutes and concurrency caps per plan are placeholders until
//! Eoj gate 5 (pricing), set from the 0.3 spike's measured cost.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::HeaderMap,
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool};

use super::meetings::{caller, require_paid, ts, MeetingError, Result};
use crate::ApiState;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/me/meeting-settings", get(get_my_settings).put(put_my_settings))
        .route("/api/v1/me/meeting-usage", get(get_my_usage))
        .route("/api/v1/orgs/:org/meeting-settings", get(get_org_settings).put(put_org_settings))
}

// ---------------------------------------------------------------------------
// Plan limits
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlanLimits {
    pub included_minutes: i64,
    pub concurrent_cap: i64,
}

/// Placeholders until Eoj gate 5 (pricing).
pub(crate) fn plan_limits(plan: &str) -> PlanLimits {
    match plan {
        "plus" => PlanLimits { included_minutes: 300, concurrent_cap: 1 },
        "super" => PlanLimits { included_minutes: 1200, concurrent_cap: 2 },
        "ultra" => PlanLimits { included_minutes: 3000, concurrent_cap: 5 },
        _ => PlanLimits { included_minutes: 0, concurrent_cap: 0 },
    }
}

// ---------------------------------------------------------------------------
// User settings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
pub(crate) struct UserSettings {
    pub auto_join: String,
    pub recap_email: bool,
    pub default_reply_mode: String,
}

impl Default for UserSettings {
    fn default() -> Self {
        UserSettings { auto_join: "off".into(), recap_email: true, default_reply_mode: "chat".into() }
    }
}

fn user_settings_json(s: &UserSettings) -> Value {
    json!({ "auto_join": s.auto_join, "recap_email": s.recap_email, "default_reply_mode": s.default_reply_mode })
}

pub(crate) async fn load_user_settings(db: &PgPool, user_id: &str) -> Result<UserSettings> {
    Ok(sqlx::query_as::<_, UserSettings>(
        "SELECT auto_join, recap_email, default_reply_mode FROM user_meeting_settings WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?
    .unwrap_or_default())
}

fn body_object(body: &Bytes) -> Result<serde_json::Map<String, Value>> {
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(MeetingError::invalid("The body must be a JSON object.")),
    }
}

fn want_bool(value: &Value, key: &str) -> Result<bool> {
    value.as_bool().ok_or_else(|| MeetingError::invalid(format!("{key} must be true or false.")))
}

async fn get_my_settings(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Json<Value>> {
    let caller = caller(&state, &headers).await?;
    let settings = load_user_settings(&state.db, &caller.id).await?;
    Ok(Json(user_settings_json(&settings)))
}

async fn put_my_settings(State(state): State<Arc<ApiState>>, headers: HeaderMap, body: Bytes) -> Result<Json<Value>> {
    let caller = caller(&state, &headers).await?;
    require_paid(&state.db, &caller).await?;
    let body = body_object(&body)?;
    let mut settings = load_user_settings(&state.db, &caller.id).await?;
    for (key, value) in &body {
        match key.as_str() {
            "auto_join" => match value.as_str() {
                Some(rule @ ("off" | "all" | "all_except_1on1")) => settings.auto_join = rule.to_string(),
                _ => return Err(MeetingError::invalid("auto_join must be off, all or all_except_1on1.")),
            },
            "recap_email" => settings.recap_email = want_bool(value, key)?,
            "default_reply_mode" => match value.as_str() {
                Some(mode @ ("chat" | "voice")) => settings.default_reply_mode = mode.to_string(),
                _ => return Err(MeetingError::invalid("default_reply_mode must be chat or voice.")),
            },
            other => return Err(MeetingError::invalid(format!("Unknown field '{other}'."))),
        }
    }
    sqlx::query(
        "INSERT INTO user_meeting_settings (user_id, auto_join, recap_email, default_reply_mode, updated_at) \
         VALUES ($1, $2, $3, $4, now()) \
         ON CONFLICT (user_id) DO UPDATE SET auto_join = EXCLUDED.auto_join, recap_email = EXCLUDED.recap_email, \
             default_reply_mode = EXCLUDED.default_reply_mode, updated_at = now()",
    )
    .bind(&caller.id)
    .bind(&settings.auto_join)
    .bind(settings.recap_email)
    .bind(&settings.default_reply_mode)
    .execute(&state.db)
    .await?;
    Ok(Json(user_settings_json(&settings)))
}

// ---------------------------------------------------------------------------
// Org settings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
pub(crate) struct OrgSettings {
    pub org_id: String,
    pub enabled: bool,
    pub roles: Vec<String>,
    pub allow_external_guests: bool,
    pub guest_quiet: bool,
    pub allow_member_connectors: bool,
    pub retention_days: i32,
    #[sqlx(default)]
    pub updated_at: Option<DateTime<Utc>>,
}

impl OrgSettings {
    pub fn defaults(org_id: &str) -> Self {
        OrgSettings {
            org_id: org_id.to_string(),
            enabled: true,
            roles: vec!["org:admin".into(), "org:member".into()],
            allow_external_guests: true,
            guest_quiet: true,
            allow_member_connectors: false,
            retention_days: 90,
            updated_at: None,
        }
    }
}

fn org_settings_json(s: &OrgSettings) -> Value {
    json!({
        "org_id": s.org_id,
        "enabled": s.enabled,
        "roles": s.roles,
        "allow_external_guests": s.allow_external_guests,
        "guest_quiet": s.guest_quiet,
        "allow_member_connectors": s.allow_member_connectors,
        "retention_days": s.retention_days,
        "updated_at": s.updated_at.map(ts),
    })
}

pub(crate) async fn load_org_settings(db: &PgPool, org_id: &str) -> Result<OrgSettings> {
    Ok(sqlx::query_as::<_, OrgSettings>(
        "SELECT org_id, enabled, roles, allow_external_guests, guest_quiet, allow_member_connectors, \
         retention_days, updated_at FROM org_meeting_settings WHERE org_id = $1",
    )
    .bind(org_id)
    .fetch_optional(db)
    .await?
    .unwrap_or_else(|| OrgSettings::defaults(org_id)))
}

/// Clerk session token v2 gives `admin` / `member`, v1 gives `org:admin`;
/// settings store the `org:` form.
pub(crate) fn normalise_role(role: &str) -> String {
    let role = role.trim();
    match role {
        "basic_member" => "org:member".to_string(),
        r if r.starts_with("org:") => r.to_string(),
        r => format!("org:{r}"),
    }
}

/// The caller must be in `org` (their active Clerk org).
fn member_of(caller: &super::meetings::Caller, org: &str) -> Result<()> {
    if caller.org_id.as_deref() == Some(org) {
        Ok(())
    } else {
        Err(MeetingError::forbidden("Switch to this organization to see its Meetings settings."))
    }
}

async fn get_org_settings(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(org): Path<String>,
) -> Result<Json<Value>> {
    let caller = caller(&state, &headers).await?;
    require_paid(&state.db, &caller).await?;
    member_of(&caller, &org)?;
    Ok(Json(org_settings_json(&load_org_settings(&state.db, &org).await?)))
}

async fn put_org_settings(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    body: Bytes,
) -> Result<Json<Value>> {
    let caller = caller(&state, &headers).await?;
    require_paid(&state.db, &caller).await?;
    member_of(&caller, &org)?;
    if !caller.is_org_admin() {
        return Err(MeetingError::forbidden("Only an organization admin can change Meetings settings."));
    }
    let body = body_object(&body)?;
    let mut s = load_org_settings(&state.db, &org).await?;
    for (key, value) in &body {
        match key.as_str() {
            "org_id" => {
                if value.as_str() != Some(org.as_str()) {
                    return Err(MeetingError::invalid("org_id must match the organization in the URL."));
                }
            }
            "enabled" => s.enabled = want_bool(value, key)?,
            "allow_external_guests" => s.allow_external_guests = want_bool(value, key)?,
            "guest_quiet" => s.guest_quiet = want_bool(value, key)?,
            "allow_member_connectors" => s.allow_member_connectors = want_bool(value, key)?,
            "retention_days" => {
                s.retention_days = value
                    .as_i64()
                    .filter(|d| (1..=3650).contains(d))
                    .ok_or_else(|| MeetingError::invalid("retention_days must be 1 to 3650."))?
                    as i32
            }
            "roles" => {
                let list = value
                    .as_array()
                    .filter(|l| !l.is_empty() && l.len() <= 20)
                    .ok_or_else(|| {
                        MeetingError::invalid("roles must list 1 to 20 roles. To turn Meetings off, set enabled to false.")
                    })?;
                let mut roles = Vec::new();
                for role in list {
                    let role = role
                        .as_str()
                        .map(str::trim)
                        .filter(|r| !r.is_empty() && r.len() <= 64)
                        .ok_or_else(|| MeetingError::invalid("Each role must be a short string such as org:member."))?;
                    let role = normalise_role(role);
                    if !roles.contains(&role) {
                        roles.push(role);
                    }
                }
                s.roles = roles;
            }
            "updated_at" => {}
            other => return Err(MeetingError::invalid(format!("Unknown field '{other}'."))),
        }
    }
    let saved = sqlx::query_as::<_, OrgSettings>(
        "INSERT INTO org_meeting_settings (org_id, enabled, roles, allow_external_guests, guest_quiet, \
             allow_member_connectors, retention_days, updated_by, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now()) \
         ON CONFLICT (org_id) DO UPDATE SET enabled = EXCLUDED.enabled, roles = EXCLUDED.roles, \
             allow_external_guests = EXCLUDED.allow_external_guests, guest_quiet = EXCLUDED.guest_quiet, \
             allow_member_connectors = EXCLUDED.allow_member_connectors, \
             retention_days = EXCLUDED.retention_days, updated_by = EXCLUDED.updated_by, updated_at = now() \
         RETURNING org_id, enabled, roles, allow_external_guests, guest_quiet, allow_member_connectors, \
             retention_days, updated_at",
    )
    .bind(&org)
    .bind(s.enabled)
    .bind(&s.roles)
    .bind(s.allow_external_guests)
    .bind(s.guest_quiet)
    .bind(s.allow_member_connectors)
    .bind(s.retention_days)
    .bind(&caller.id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(org_settings_json(&saved)))
}

// ---------------------------------------------------------------------------
// Usage
// ---------------------------------------------------------------------------

async fn get_my_usage(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Json<Value>> {
    let caller = caller(&state, &headers).await?;
    let plan = require_paid(&state.db, &caller).await?;
    let (start, end) = crate::services::voice_usage::month_bounds(Utc::now());
    let seconds: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(seconds), 0)::bigint FROM meeting_bot_minutes \
         WHERE user_id = $1 AND occurred_at >= $2 AND occurred_at < $3",
    )
    .bind(&caller.id)
    .bind(start)
    .bind(end)
    .fetch_one(&state.db)
    .await?;
    let used = (seconds + 59) / 60;
    let limits = plan_limits(&plan);
    Ok(Json(json!({
        "period_start": ts(start),
        "period_end": ts(end),
        "used_minutes": used,
        "included_minutes": limits.included_minutes,
        "overage_minutes": (used - limits.included_minutes).max(0),
        "concurrent_cap": limits.concurrent_cap,
        "plan": plan,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_normalise_to_the_org_prefix() {
        assert_eq!(normalise_role("admin"), "org:admin");
        assert_eq!(normalise_role("org:admin"), "org:admin");
        assert_eq!(normalise_role(" member "), "org:member");
        assert_eq!(normalise_role("basic_member"), "org:member");
        assert_eq!(normalise_role("org:billing"), "org:billing");
    }

    #[test]
    fn free_has_no_minutes_or_bots() {
        assert_eq!(plan_limits("free"), PlanLimits { included_minutes: 0, concurrent_cap: 0 });
        assert!(plan_limits("plus").concurrent_cap >= 1);
    }
}
