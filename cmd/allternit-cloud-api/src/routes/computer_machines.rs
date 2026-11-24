//! One computer, one row (step 2.10, surfaces/docs/dev/computer-pairing.mdx).
//!
//! The console groups every pairing of a machine into one row
//! (`computers_console::build_console`). These routes act on the whole
//! machine, by its row id or any of its `aliases`:
//!
//! * `POST /api/v1/computers/:id/unpair` `{ confirm: true }` -> `{ removed: true, id, revokedDevices, removedPaired }`:
//!   revokes every runtime device of the machine (Desktop, `gizzi pair`,
//!   `ao fabric pair`), deletes its code-paired rows and forgets the name the
//!   owner gave it. Owner only. A cloud computer is deleted, not unpaired.
//! * `PATCH /api/v1/computers/paired/:id` `{ name }` -> `{ id, name }`: the
//!   name shown for the machine on every surface. Stored by machine key
//!   (`computer_machine_names`, migration 159), so it covers every alias and
//!   survives re-pairing. Owner only; works for cloud rows too.
//!
//! Every error body is `{ error, message }`.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::routes::computers_console::{find_machine, ConsoleComputer, GROUP_SHARED};
use crate::routes::paired_computers::{signed_in_caller, Caller};
use crate::services::audit::{write_audit_log, AuditEvent};
use crate::{ApiError, ApiState};

pub const MAX_NAME: usize = 80;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/computers/:id/unpair", post(unpair))
}

fn fail(status: StatusCode, error: &str, message: &str) -> Response {
    (status, Json(json!({ "error": error, "message": message }))).into_response()
}

fn db_fail(e: sqlx::Error) -> Response {
    tracing::warn!("computer machines: {e}");
    fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "db_error",
        "Something went wrong. Try again in a moment.",
    )
}

/// The caller's machine `id` names, or the error response to send.
async fn own_machine(state: &ApiState, caller: &Caller, id: &str) -> Result<ConsoleComputer, Response> {
    let row = find_machine(&state.db, &caller.user_id, caller.organization_id.as_deref(), id)
        .await
        .map_err(db_fail)?;
    let Some(row) = row else {
        return Err(fail(StatusCode::NOT_FOUND, "not_found", "That computer isn't in your Computers list."));
    };
    if row.group == GROUP_SHARED {
        return Err(fail(
            StatusCode::FORBIDDEN,
            "not_owner",
            "Someone else in your organization paired this computer. Only they can change it.",
        ));
    }
    Ok(row)
}

#[derive(Deserialize, Default)]
struct UnpairBody {
    #[serde(default)]
    confirm: bool,
}

async fn unpair(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<UnpairBody>>,
) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    if !body.map(|Json(b)| b.confirm).unwrap_or(false) {
        return Ok(fail(
            StatusCode::BAD_REQUEST,
            "confirm_required",
            "Unpairing signs this computer out of Allternit everywhere. Send confirm: true to go ahead.",
        ));
    }
    let row = match own_machine(&state, &caller, &id).await {
        Ok(row) => row,
        Err(response) => return Ok(response),
    };
    if row.kind == "cloud" {
        return Ok(fail(
            StatusCode::CONFLICT,
            "cloud_computer",
            "This is an Allternit Cloud computer. Delete it from its Settings tab instead of unpairing it.",
        ));
    }
    let result = remove_machine(&state.db, &caller.user_id, &row).await;
    let (revoked, removed) = match result {
        Ok(counts) => counts,
        Err(e) => return Ok(db_fail(e)),
    };
    tracing::info!(computer = %row.id, revoked = revoked.len(), removed = removed.len(), "computer unpaired");
    write_audit_log(
        &state.db,
        AuditEvent {
            action: "computer.unpair".into(),
            resource_type: "computer".into(),
            resource_id: Some(row.id.clone()),
            user_id: Some(caller.user_id.clone()),
            user_email: None,
            details: Some(json!({ "revokedDevices": revoked, "removedPaired": removed })),
            success: true,
        },
    )
    .await;
    Ok(Json(json!({
        "removed": true,
        "id": row.id,
        "revokedDevices": revoked,
        "removedPaired": removed,
    }))
    .into_response())
}

/// Revoke every runtime device of the machine, delete its paired rows and
/// its name, in one transaction. Only the caller's own rows are touched.
pub async fn remove_machine(
    db: &sqlx::PgPool,
    user_id: &str,
    row: &ConsoleComputer,
) -> Result<(Vec<String>, Vec<String>), sqlx::Error> {
    let mut tx = db.begin().await?;
    let revoked: Vec<String> = sqlx::query_scalar(
        "UPDATE runtime_devices SET status = 'revoked', revoked_at = CURRENT_TIMESTAMP
         WHERE user_id = $1 AND id = ANY($2) AND revoked_at IS NULL
         RETURNING id",
    )
    .bind(user_id)
    .bind(&row.member_devices)
    .fetch_all(&mut *tx)
    .await?;
    let removed: Vec<String> = sqlx::query_scalar(
        "DELETE FROM paired_computers
         WHERE user_id = $1 AND id = ANY($2) AND provisioned_instance_id IS NULL
         RETURNING id",
    )
    .bind(user_id)
    .bind(&row.member_paired)
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM computer_machine_names WHERE user_id = $1 AND machine_key = $2")
        .bind(user_id)
        .bind(&row.machine_key)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((revoked, removed))
}

/// A machine name: trimmed, 1-80 characters, no control characters.
pub fn check_name(name: &str) -> Result<String, &'static str> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
        return Err("Give the computer a name of 1 to 80 characters.");
    }
    Ok(name.to_string())
}

/// `PATCH /api/v1/computers/paired/:id` (mounted in routes::paired_computers,
/// which owns that path).
pub async fn rename(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let raw = body.as_ref().and_then(|Json(b)| b.get("name")).and_then(Value::as_str).unwrap_or_default();
    let name = match check_name(raw) {
        Ok(name) => name,
        Err(message) => return Ok(fail(StatusCode::BAD_REQUEST, "invalid_name", message)),
    };
    let row = match own_machine(&state, &caller, &id).await {
        Ok(row) => row,
        Err(response) => return Ok(response),
    };
    let saved = sqlx::query(
        "INSERT INTO computer_machine_names (user_id, machine_key, name, updated_at) VALUES ($1, $2, $3, now())
         ON CONFLICT (user_id, machine_key) DO UPDATE SET name = EXCLUDED.name, updated_at = now()",
    )
    .bind(&caller.user_id)
    .bind(&row.machine_key)
    .bind(&name)
    .execute(&state.db)
    .await;
    if let Err(e) = saved {
        return Ok(db_fail(e));
    }
    Ok(Json(json!({ "id": row.id, "name": name })).into_response())
}
