//! Computers console: cloud computer settings (contract
//! `surfaces/docs/dev/computers-console-contract.mdx`, step 2.4).
//!
//! `:id` is the console row id. These routes act only on the caller's cloud
//! computers (`provisioned_instances.id`, same owner rule as
//! `/provisioned-instances/:id/start`). A paired or desktop id is 404
//! `{ "error": "not_a_cloud_computer" }`; clients read 404/405/501 as "not
//! available here".
//!
//! - `GET /api/v1/computers/templates` → `{ templates: [{ id, name, description? }] }`
//!   (the starter allow-list; empty = default Ubuntu only).
//! - `POST /api/v1/computers/:id/restart` → `{ instance }`; 409 `asleep` when
//!   it is not running (never woken), 409 `busy` mid-change.
//! - `POST /api/v1/computers/:id/move` `{ location, confirm: true }` → 202
//!   `{ instance }`: Orgo keeps a computer in the city it was created in (its
//!   `PATCH /computers/{id}/move` changes workspace, not city), so a move is a
//!   fresh computer in the new city and erases the disk (Eoj, 2026-11-21).
//!   The new one is created before the old one is deleted. 400
//!   `confirm_required` / `invalid_location`; 409 `same_location`; 503
//!   `region_unavailable` `{ location }` (old computer untouched).
//! - `POST /api/v1/computers/:id/rebuild` `{ confirm: true }` → 202 `{ instance }`
//!   (status `provisioning` while first boot runs); 400 `confirm_required`;
//!   409 `no_recent_backup` (no Orgo daily backup in the last 48 h; nothing
//!   was deleted); 503 `region_unavailable` `{ location }`.
//!
//! - `GET /api/v1/computers/:id/backups` → `{ backups: [{ id, createdAt,
//!   sizeBytes?, kind }], canRestore }` (step 2.6): the finished Orgo daily
//!   backups, newest first; `canRestore` is Orgo's "can be restored now"
//!   (null when it does not say). Read only, never wakes the computer. 503
//!   `backup_check_failed` when Orgo's answer can't be read.
//! - `POST /api/v1/computers/:id/backups/restore` `{ backupId, confirm: true }`
//!   → 202 `{ instance, backupId }`: an in-place restore (same computer,
//!   same id), which erases what changed since that backup. 400
//!   `confirm_required` / `backup_id_required`; 404 `backup_not_found`; 409
//!   `restore_unavailable` / `busy`; 501 `not_supported` while
//!   `ALLTERNIT_ORGO_BACKUP_RESTORE` is off (the default).
//!
//! - `GET /api/v1/computers/:id/ssh-keys` → `{ keys: [{ id, label,
//!   fingerprint, kind, keyType, createdAt, expiresAt }], connect }` (step
//!   2.7): the computer's SSH public keys, oldest first (`kind` `computer` or
//!   `agent`; agent keys carry `expiresAt`), and `connect` `{ host, port,
//!   user, command }` from Orgo while the computer runs and has keys, else
//!   null. Never wakes the computer.
//! - `POST /api/v1/computers/:id/ssh-keys` `{ label, publicKey, kind?,
//!   expiresInMinutes? }` → 201 with the key plus `requiresRestart`: 400
//!   `private_key` / `invalid_public_key` / `one_key_only` /
//!   `public_key_required` / `too_many_keys` / `invalid_kind` /
//!   `invalid_expiry`; 409 `key_exists`, `asleep`, `busy`.
//! - `DELETE /api/v1/computers/:id/ssh-keys/:keyId` → `{ removed,
//!   requiresRestart }`; 404 `key_not_found`. Removing the last key turns
//!   SSH off on the computer.
//!
//! Every error body carries a plain `message`.
//!
//! The location and template on create are fields of
//! `POST /api/v1/provisioned-instances` (`routes/provisioned_instances.rs`).

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::services::provisioning::{CloudActionError, SshKeyInput, STARTER_TEMPLATES};
use crate::{ApiError, ApiState};

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/computers/templates", get(list_templates))
        .route("/api/v1/computers/:id/restart", post(restart))
        .route("/api/v1/computers/:id/move", post(move_computer))
        .route("/api/v1/computers/:id/rebuild", post(rebuild))
        .route("/api/v1/computers/:id/backups", get(list_backups))
        .route(
            "/api/v1/computers/:id/backups/restore",
            post(restore_backup),
        )
        .route(
            "/api/v1/computers/:id/ssh-keys",
            get(list_ssh_keys).post(add_ssh_key),
        )
        .route(
            "/api/v1/computers/:id/ssh-keys/:key_id",
            delete(remove_ssh_key),
        )
}

/// The JSON response for a cloud action error.
pub(crate) fn action_error_response(error: &CloudActionError) -> Response {
    let (status, body) = error.parts();
    (
        StatusCode::from_u16(status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
        Json(body),
    )
        .into_response()
}

async fn list_templates(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    Ok(Json(serde_json::json!({ "templates": STARTER_TEMPLATES })))
}

async fn restart(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    Ok(
        match state
            .provisioning_service
            .restart_cloud(&id, &user.id)
            .await
        {
            Ok(instance) => Json(serde_json::json!({ "instance": instance })).into_response(),
            Err(error) => action_error_response(&error),
        },
    )
}

#[derive(Debug, Default, Deserialize)]
pub struct MoveRequest {
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    confirm: bool,
}

async fn move_computer(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Option<Json<MoveRequest>>,
) -> Result<Response, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    if let Err(error) = state
        .provisioning_service
        .console_cloud_row(&id, &user.id)
        .await
    {
        return Ok(action_error_response(&error));
    }
    let request = body.map(|Json(request)| request).unwrap_or_default();
    let Some(location) = request.location.filter(|l| !l.trim().is_empty()) else {
        return Ok(action_error_response(&CloudActionError::InvalidLocation));
    };
    if !request.confirm {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "confirm_required",
                "message": "Moving starts a fresh computer in the new city and erases this one's files. Send {\"confirm\": true}.",
            })),
        )
            .into_response());
    }
    Ok(
        match state
            .provisioning_service
            .move_cloud(&id, &user.id, &location)
            .await
        {
            Ok(instance) => (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({ "instance": instance })),
            )
                .into_response(),
            Err(error) => action_error_response(&error),
        },
    )
}

#[derive(Debug, Default, Deserialize)]
pub struct RebuildRequest {
    #[serde(default)]
    confirm: bool,
}

async fn rebuild(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Option<Json<RebuildRequest>>,
) -> Result<Response, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    if let Err(error) = state
        .provisioning_service
        .console_cloud_row(&id, &user.id)
        .await
    {
        return Ok(action_error_response(&error));
    }
    let confirmed = body.is_some_and(|Json(request)| request.confirm);
    if !confirmed {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "confirm_required",
                "message": "Rebuild erases this computer's disk. Send {\"confirm\": true}.",
            })),
        )
            .into_response());
    }
    Ok(
        match state
            .provisioning_service
            .rebuild_cloud(&id, &user.id)
            .await
        {
            Ok(instance) => (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({ "instance": instance })),
            )
                .into_response(),
            Err(error) => action_error_response(&error),
        },
    )
}

async fn list_backups(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    Ok(
        match state
            .provisioning_service
            .cloud_backups(&id, &user.id)
            .await
        {
            Ok(list) => Json(serde_json::json!({
                "backups": list.backups,
                "canRestore": list.can_restore,
            }))
            .into_response(),
            Err(error) => action_error_response(&error),
        },
    )
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreBackupRequest {
    #[serde(default)]
    backup_id: Option<String>,
    #[serde(default)]
    confirm: bool,
}

async fn restore_backup(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Option<Json<RestoreBackupRequest>>,
) -> Result<Response, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    if let Err(error) = state
        .provisioning_service
        .console_cloud_row(&id, &user.id)
        .await
    {
        return Ok(action_error_response(&error));
    }
    let request = body.map(|Json(request)| request).unwrap_or_default();
    let Some(backup_id) = request.backup_id.filter(|b| !b.trim().is_empty()) else {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "backup_id_required",
                "message": "Pick a backup to restore.",
            })),
        )
            .into_response());
    };
    if !request.confirm {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "confirm_required",
                "message": "Restoring puts this computer back to that backup and erases what changed since. Send {\"confirm\": true}.",
            })),
        )
            .into_response());
    }
    Ok(
        match state
            .provisioning_service
            .restore_cloud_backup(&id, &user.id, &backup_id)
            .await
        {
            Ok(instance) => (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({ "instance": instance, "backupId": backup_id })),
            )
                .into_response(),
            Err(error) => action_error_response(&error),
        },
    )
}

/// The note shown when Orgo needs one restart before a key change works.
const RESTART_NOTE: &str = "Saved. Restart this computer once to put the change in place; until then the old keys keep working.";

async fn list_ssh_keys(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    Ok(
        match state.provisioning_service.ssh_keys(&id, &user.id).await {
            Ok(list) => Json(serde_json::json!({ "keys": list.keys, "connect": list.connect }))
                .into_response(),
            Err(error) => action_error_response(&error),
        },
    )
}

async fn add_ssh_key(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Option<Json<SshKeyInput>>,
) -> Result<Response, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    let input = body.map(|Json(input)| input).unwrap_or_default();
    Ok(
        match state
            .provisioning_service
            .add_ssh_key(&id, &user.id, input)
            .await
        {
            Ok((key, requires_restart)) => {
                let mut body = serde_json::to_value(&key).unwrap_or_default();
                body["requiresRestart"] = serde_json::Value::Bool(requires_restart);
                if requires_restart {
                    body["message"] = serde_json::Value::String(RESTART_NOTE.to_string());
                }
                (StatusCode::CREATED, Json(body)).into_response()
            }
            Err(error) => action_error_response(&error),
        },
    )
}

async fn remove_ssh_key(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path((id, key_id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    Ok(
        match state
            .provisioning_service
            .remove_ssh_key(&id, &user.id, &key_id)
            .await
        {
            Ok(requires_restart) => {
                let mut body =
                    serde_json::json!({ "removed": key_id, "requiresRestart": requires_restart });
                if requires_restart {
                    body["message"] = serde_json::Value::String(RESTART_NOTE.to_string());
                }
                Json(body).into_response()
            }
            Err(error) => action_error_response(&error),
        },
    )
}
