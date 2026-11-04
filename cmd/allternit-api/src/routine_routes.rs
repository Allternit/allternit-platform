//! Cowork routines (consumer-packaged Cowork P4.2): scheduled/background work
//! tied to a principal, executing on Fabric Transport. The routines tick
//! finds due routines and submits a canonical intent per fire (initiator =
//! owning user, delegator = the routine's principal, target resolved through
//! the delegation rules) — every routine run is a normal attributed
//! transport run: claim/lease/approvals/deliverables all apply.
//!
//! Schedule grammar (deliberately small): `*/N` (every N minutes), `@hourly`,
//! `@daily`. Unknown schedules are rejected at create time, never silently
//! misfired.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::AppState;

pub fn routine_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/cowork/routines", get(list_routines).post(create_routine))
        .route("/cowork/routines/:routine_id", delete(delete_routine))
        .route("/cowork/routines/:routine_id/run", post(run_routine_now))
}

#[derive(Debug)]
struct RoutineError {
    status: StatusCode,
    body: Json<Value>,
}

impl RoutineError {
    fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            body: Json(json!({"error": code, "message": message.into()})),
        }
    }
}

impl IntoResponse for RoutineError {
    fn into_response(self) -> Response {
        (self.status, self.body).into_response()
    }
}

/// Compute the interval in seconds for a schedule expression. None = invalid.
pub fn schedule_interval_secs(schedule: &str) -> Option<i64> {
    let schedule = schedule.trim();
    if let Some(rest) = schedule.strip_prefix("*/") {
        let minutes: i64 = rest.trim().parse().ok()?;
        if (1..=24 * 60).contains(&minutes) {
            return Some(minutes * 60);
        }
        return None;
    }
    match schedule {
        "@hourly" => Some(3600),
        "@daily" => Some(24 * 3600),
        _ => None,
    }
}

/// Next fire at, as RFC3339, given the schedule and a base time.
pub fn next_run_after(schedule: &str, base: chrono::DateTime<chrono::Utc>) -> Option<String> {
    let secs = schedule_interval_secs(schedule)?;
    Some((base + chrono::Duration::seconds(secs)).to_rfc3339())
}

#[derive(Debug, serde::Deserialize)]
struct CreateRoutineRequest {
    name: String,
    message: String,
    schedule: String,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    principal: Option<String>,
}

async fn create_routine(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<CreateRoutineRequest>,
) -> Result<Json<Value>, RoutineError> {
    let user = crate::auth::get_user(&headers).ok_or_else(|| {
        RoutineError::new(StatusCode::UNAUTHORIZED, "unauthorized", "authentication required")
    })?;
    if req.name.trim().is_empty() || req.message.trim().is_empty() {
        return Err(RoutineError::new(StatusCode::BAD_REQUEST, "invalid", "name and message are required"));
    }
    if schedule_interval_secs(&req.schedule).is_none() {
        return Err(RoutineError::new(
            StatusCode::BAD_REQUEST,
            "invalid_schedule",
            "schedule must be */N (minutes), @hourly, or @daily",
        ));
    }
    let workspace_raw = req.workspace.clone().unwrap_or_else(|| "default".to_string());
    let workspace = workspace_raw
        .strip_prefix("a://workspace/")
        .map(str::to_string)
        .unwrap_or(workspace_raw);
    let next = next_run_after(&req.schedule, chrono::Utc::now())
        .ok_or_else(|| RoutineError::new(StatusCode::BAD_REQUEST, "invalid_schedule", "bad schedule"))?;
    let id = format!("rt_{}", uuid::Uuid::new_v4());
    let conn = state
        .db
        .connect()
        .map_err(|e| RoutineError::new(StatusCode::INTERNAL_SERVER_ERROR, "store_error", e.to_string()))?;
    let cloud = crate::cowork_preferences_routes::cloud_continuation_enabled(&conn, &user.user_id);
    let ingest = crate::continuation::verify_continuation_token(&headers, &state.config);
    if cloud && !ingest {
        let prefs_url = conn
            .query_row(
                "SELECT continuation_api_url FROM user_cowork_preferences WHERE user_id = ?1",
                rusqlite::params![user.user_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten();
        let target = crate::continuation::resolve_target(&state.config, prefs_url.as_deref())
            .ok_or_else(|| {
                RoutineError::new(
                    StatusCode::CONFLICT,
                    "continuation_unconfigured",
                    "cloud continuation routines need ALLTERNIT_CONTINUATION_API_URL and ALLTERNIT_CONTINUATION_TOKEN",
                )
            })?;
        drop(conn);
        let token = state.config.continuation_token().ok_or_else(|| {
            RoutineError::new(
                StatusCode::CONFLICT,
                "continuation_unconfigured",
                "ALLTERNIT_CONTINUATION_TOKEN is not set",
            )
        })?;
        let client = crate::net_client::client();
        let url = format!("{}/api/v1/cowork/routines", target.trim_end_matches('/'));
        let res = client
            .post(&url)
            .header(crate::continuation::CONTINUATION_TOKEN_HEADER, token)
            .json(&json!({
                "name": req.name,
                "message": req.message,
                "schedule": req.schedule,
                "workspace": workspace,
                "principal": req.principal,
            }))
            .send()
            .await
            .map_err(|e| RoutineError::new(StatusCode::BAD_GATEWAY, "continuation_forward_failed", e.to_string()))?;
        if !res.status().is_success() {
            let body = res.text().await.unwrap_or_default();
            return Err(RoutineError::new(
                StatusCode::BAD_GATEWAY,
                "continuation_forward_failed",
                body,
            ));
        }
        let remote: Value = res.json().await.unwrap_or(json!({}));
        let conn = state
            .db
            .connect()
            .map_err(|e| RoutineError::new(StatusCode::INTERNAL_SERVER_ERROR, "store_error", e.to_string()))?;
        conn.execute(
            "INSERT INTO cowork_routines (id, user_id, workspace, principal, name, message, schedule, enabled, next_run_at, continuation)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, 'cloud')",
            rusqlite::params![
                remote.get("id").and_then(|v| v.as_str()).unwrap_or(&id),
                user.user_id,
                workspace,
                req.principal,
                req.name,
                req.message,
                req.schedule,
                next,
            ],
        )
        .map_err(|e| RoutineError::new(StatusCode::INTERNAL_SERVER_ERROR, "store_error", e.to_string()))?;
        return Ok(Json(json!({
            "id": remote.get("id").and_then(|v| v.as_str()).unwrap_or(&id),
            "continuation": "cloud",
            "enabled": false,
            "remote": remote,
        })));
    }
    let enabled = 1;
    let continuation = if ingest { "cloud" } else { "local" };
    conn.execute(
        "INSERT INTO cowork_routines (id, user_id, workspace, principal, name, message, schedule, enabled, next_run_at, continuation)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            id,
            user.user_id,
            workspace,
            req.principal,
            req.name.trim(),
            req.message.trim(),
            req.schedule.trim(),
            enabled,
            next,
            continuation,
        ],
    )
    .map_err(|e| RoutineError::new(StatusCode::INTERNAL_SERVER_ERROR, "store_error", e.to_string()))?;
    Ok(Json(json!({
        "id": id,
        "name": req.name.trim(),
        "schedule": req.schedule.trim(),
        "next_run_at": next,
        "enabled": true,
        "continuation": continuation,
    })))
}

async fn list_routines(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, RoutineError> {
    let user = crate::auth::get_user(&headers).ok_or_else(|| {
        RoutineError::new(StatusCode::UNAUTHORIZED, "unauthorized", "authentication required")
    })?;
    let conn = state
        .db
        .connect()
        .map_err(|e| RoutineError::new(StatusCode::INTERNAL_SERVER_ERROR, "store_error", e.to_string()))?;
    let mut stmt = conn
        .prepare(
            "SELECT id, workspace, principal, name, message, schedule, enabled, last_run_at, next_run_at
             FROM cowork_routines WHERE user_id = ?1 ORDER BY created_at ASC",
        )
        .map_err(|e| RoutineError::new(StatusCode::INTERNAL_SERVER_ERROR, "store_error", e.to_string()))?;
    let rows: Vec<Value> = stmt
        .query_map(rusqlite::params![user.user_id], |r| {
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "workspace": r.get::<_, String>(1)?,
                "principal": r.get::<_, Option<String>>(2)?,
                "name": r.get::<_, String>(3)?,
                "message": r.get::<_, String>(4)?,
                "schedule": r.get::<_, String>(5)?,
                "enabled": r.get::<_, i64>(6)? == 1,
                "last_run_at": r.get::<_, Option<String>>(7)?,
                "next_run_at": r.get::<_, Option<String>>(8)?,
            }))
        })
        .map_err(|e| RoutineError::new(StatusCode::INTERNAL_SERVER_ERROR, "store_error", e.to_string()))?
        .collect::<Result<_, _>>()
        .map_err(|e| RoutineError::new(StatusCode::INTERNAL_SERVER_ERROR, "store_error", e.to_string()))?;
    Ok(Json(json!({ "routines": rows })))
}

async fn delete_routine(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(routine_id): Path<String>,
) -> Result<Json<Value>, RoutineError> {
    let user = crate::auth::get_user(&headers).ok_or_else(|| {
        RoutineError::new(StatusCode::UNAUTHORIZED, "unauthorized", "authentication required")
    })?;
    let conn = state
        .db
        .connect()
        .map_err(|e| RoutineError::new(StatusCode::INTERNAL_SERVER_ERROR, "store_error", e.to_string()))?;
    let n = conn
        .execute(
            "DELETE FROM cowork_routines WHERE id = ?1 AND user_id = ?2",
            rusqlite::params![routine_id, user.user_id],
        )
        .map_err(|e| RoutineError::new(StatusCode::INTERNAL_SERVER_ERROR, "store_error", e.to_string()))?;
    if n == 0 {
        return Err(RoutineError::new(StatusCode::NOT_FOUND, "not_found", "routine not found"));
    }
    Ok(Json(json!({ "deleted": routine_id })))
}

/// Fire a routine immediately (sets next_run_at to now, then runs the tick).
async fn run_routine_now(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(routine_id): Path<String>,
) -> Result<Json<Value>, RoutineError> {
    let user = crate::auth::get_user(&headers).ok_or_else(|| {
        RoutineError::new(StatusCode::UNAUTHORIZED, "unauthorized", "authentication required")
    })?;
    let now = chrono::Utc::now().to_rfc3339();
    let conn = state
        .db
        .connect()
        .map_err(|e| RoutineError::new(StatusCode::INTERNAL_SERVER_ERROR, "store_error", e.to_string()))?;
    let n = conn
        .execute(
            "UPDATE cowork_routines SET next_run_at = ?2 WHERE id = ?1 AND user_id = ?3 AND enabled = 1",
            rusqlite::params![routine_id, now, user.user_id],
        )
        .map_err(|e| RoutineError::new(StatusCode::INTERNAL_SERVER_ERROR, "store_error", e.to_string()))?;
    if n == 0 {
        return Err(RoutineError::new(StatusCode::NOT_FOUND, "not_found", "routine not found"));
    }
    drop(conn);
    let fired = run_due_routines(&state.db);
    Ok(Json(json!({ "id": routine_id, "fired": fired })))
}

/// The routines tick: submit one canonical intent per due routine and
/// advance its schedule. Called from the api's background task loop.
pub fn run_due_routines(db: &crate::db::DbHandle) -> usize {
    let due: Vec<(String, String, String, Option<String>, String, String, String, String)> = {
        let Ok(conn) = db.connect() else { return 0 };
        let mut stmt = match conn.prepare(
            "SELECT id, user_id, workspace, principal, name, message, schedule, next_run_at
             FROM cowork_routines WHERE enabled = 1 AND next_run_at IS NOT NULL AND next_run_at <= ?1",
        ) {
            Ok(stmt) => stmt,
            Err(_) => return 0,
        };
        let now = chrono::Utc::now().to_rfc3339();
        stmt.query_map(rusqlite::params![now], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
            ))
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    };

    let mut fired = 0;
    for (id, user_id, workspace, principal, name, message, schedule, slot) in due {
        // Target: the routine's principal override, else the same
        // delegation-rule lookup Al uses for a chat turn (first-word
        // action type of the routine's message). A special `routine`
        // rule is not required.
        let outcome = (|| -> Result<String, String> {
            let mut conn = db
                .connect()
                .map_err(|e| e.to_string())?;
            let extracted = crate::al_persona_routes::fallback_extract(&message);
            let target = match principal.clone() {
                Some(p) => p,
                None => allternit_cowork_runtime::sqlite_store::resolve_delegation_rule(
                    &conn,
                    &workspace,
                    &extracted.action_type,
                )
                .map_err(|e| e.message)?
                .ok_or_else(|| {
                    format!(
                        "no delegation rule for action_type `{}`",
                        extracted.action_type
                    )
                })?,
            };
            let al_principal = format!("a://workspace/{workspace}/principal/al");
            let cloud = crate::cowork_preferences_routes::cloud_continuation_enabled(&conn, &user_id);
            let envelope = allternit_cowork_runtime::IntentEnvelope {
                version: "a/0.1".to_string(),
                // Keyed by the scheduled slot, so a crash between submit and
                // advancing next_run_at re-submits the SAME intent, which
                // `submit_intent` answers with the existing run (created:false).
                intent_id: format!("{id}_{slot}"),
                workspace: format!("a://workspace/{workspace}"),
                initiator: user_id.clone(),
                delegator: Some(al_principal.clone()),
                target: Some(target.clone()),
                action: allternit_cowork_runtime::IntentAction {
                    action_type: extracted.action_type.clone(),
                    description: format!("routine `{name}`: {message}"),
                    payload: Some(json!({
                        "routine_id": id,
                        "name": name,
                        "message": message,
                        "via": "routine",
                        "agentic": { "task": message },
                    })),
                },
                permissions: vec![],
                compute: if cloud {
                    Some(json!({ "policy": "cloud" }))
                } else {
                    None
                },
                model: None,
                approval: None,
                return_channel: Some(json!({ "channel": "cowork", "routine_id": id })),
                causation_chain: vec![user_id.clone(), al_principal],
            };
            let submission = allternit_cowork_runtime::sqlite_store::submit_intent(&mut conn, &envelope)
                .map_err(|e| e.message)?;
            allternit_cowork_runtime::sqlite_store::set_run_owner(&mut conn, &submission.run_id, &user_id)
                .map_err(|e| e.message)?;
            Ok(submission.run_id)
        })();

        let now = chrono::Utc::now();
        let next = next_run_after(&schedule, now).unwrap_or_else(|| (now + chrono::Duration::hours(1)).to_rfc3339());
        if let Ok(conn) = db.connect() {
            match outcome {
                Ok(run_id) => {
                    let _ = conn.execute(
                        "UPDATE cowork_routines SET last_run_at = ?2, next_run_at = ?3 WHERE id = ?1",
                        rusqlite::params![id, now.to_rfc3339(), next],
                    );
                    tracing::info!(routine = %id, run = %run_id, "Routine fired on Fabric Transport");
                    fired += 1;
                }
                Err(reason) => {
                    // Do not advance the schedule on a submission failure —
                    // retry next tick so a transient store error never skips
                    // a fire. Back off 5 minutes to avoid a hot error loop.
                    let retry_at = (now + chrono::Duration::minutes(5)).to_rfc3339();
                    let _ = conn.execute(
                        "UPDATE cowork_routines SET next_run_at = ?2 WHERE id = ?1",
                        rusqlite::params![id, retry_at],
                    );
                    tracing::warn!(routine = %id, reason = %reason, "Routine fire failed; retrying in 5 minutes");
                }
            }
        }
    }
    fired
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_interval_parsing() {
        assert_eq!(schedule_interval_secs("*/5"), Some(300));
        assert_eq!(schedule_interval_secs("*/30"), Some(1800));
        assert_eq!(schedule_interval_secs("@hourly"), Some(3600));
        assert_eq!(schedule_interval_secs("@daily"), Some(86400));
        assert_eq!(schedule_interval_secs("0 9 * * *"), None);
        assert_eq!(schedule_interval_secs("*/0"), None);
        assert_eq!(schedule_interval_secs("*/2000"), None);
    }

    #[test]
    fn next_run_advances_by_interval() {
        let base = chrono::DateTime::parse_from_rfc3339("2026-09-14T12:00:00Z").unwrap().with_timezone(&chrono::Utc);
        let next = next_run_after("*/15", base).unwrap();
        assert_eq!(next, "2026-09-14T12:15:00+00:00");
        let next = next_run_after("@daily", base).unwrap();
        assert_eq!(next, "2026-09-15T12:00:00+00:00");
    }

    #[test]
    fn due_routine_submits_canonical_intent_with_message() {
        let db = crate::db::DbHandle::new_memory().expect("test db");
        {
            let conn = db.connect().unwrap();
            allternit_cowork_runtime::sqlite_store::upsert_delegation_rule(
                &conn,
                "default",
                "organize",
                "a://workspace/default/principal/gizzi",
                100,
            )
            .unwrap();
            conn.execute(
                "INSERT INTO cowork_routines
                    (id, user_id, workspace, principal, name, message, schedule, enabled, next_run_at)
                 VALUES (?1, ?2, 'default', NULL, ?3, ?4, '*/5', 1, ?5)",
                rusqlite::params![
                    "rt_test",
                    "user_joe",
                    "nightly organize",
                    "organize the granted folder",
                    "2020-01-01T00:00:00+00:00",
                ],
            )
            .unwrap();
        }
        let fired = run_due_routines(&db);
        assert_eq!(fired, 1, "due routine must fire once");

        let conn = db.connect().unwrap();
        let payload: String = conn
            .query_row(
                "SELECT payload FROM cowork_jobs ORDER BY created_at DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .expect("job row");
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(v["message"], "organize the granted folder");
        assert_eq!(v["agentic"]["task"], "organize the granted folder");
        assert_eq!(v["routine_id"], "rt_test");
        assert_eq!(v["via"], "routine");

        let next: String = conn
            .query_row(
                "SELECT next_run_at FROM cowork_routines WHERE id = 'rt_test'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(next.as_str() > "2020-01-01T00:00:00+00:00", "schedule must advance after a successful fire");
    }

    #[test]
    fn due_routine_without_rule_does_not_skip_the_fire() {
        let db = crate::db::DbHandle::new_memory().expect("test db");
        {
            let conn = db.connect().unwrap();
            conn.execute(
                "INSERT INTO cowork_routines
                    (id, user_id, workspace, principal, name, message, schedule, enabled, next_run_at)
                 VALUES ('rt_norule', 'user_joe', 'default', NULL, 'x', 'nope this', '*/5', 1, '2020-01-01T00:00:00+00:00')",
                [],
            )
            .unwrap();
        }
        assert_eq!(run_due_routines(&db), 0);
        let conn = db.connect().unwrap();
        let next: String = conn
            .query_row(
                "SELECT next_run_at FROM cowork_routines WHERE id = 'rt_norule'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(next.as_str() > "2020-01-01T00:00:00+00:00", "failed fire must back off, not stay due forever");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM cowork_jobs", [], |r| r.get(0))
            .unwrap_or(0);
        assert_eq!(count, 0, "no job without a delegation target");
    }
}
